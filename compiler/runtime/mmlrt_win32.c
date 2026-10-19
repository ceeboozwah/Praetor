/* mmlrt_win32.c — the Win32 half of the runtime boundary (N58C-2).
 * Implements the mmlrt.h contract with pure Win32 + UCRT: no POSIX
 * emulation. STATUS: written to the design contract
 * (WINDOWS-PORT.md) but not yet compiled or run on Windows — every
 * behavior here is unverified until the Windows protocol passes.
 *
 *   spawn { ... }   -> CreateThread via _beginthreadex
 *   mutex T         -> SRWLOCK (exclusive)
 *   channel T       -> SRWLOCK + CONDITION_VARIABLE
 *   files           -> CreateFileW + _open_osfhandle, always binary
 *   sockets         -> Winsock2, lazy WSAStartup, i64 SOCKET handles
 *   process text    -> wmain + wide APIs, strict UTF-8 at the edge
 */
#define WIN32_LEAN_AND_MEAN
#define _CRT_SECURE_NO_WARNINGS
#define _WINSOCK_DEPRECATED_NO_WARNINGS
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <io.h>
#include <direct.h>
#include <fcntl.h>
#include <sys/stat.h>
#include <process.h>
#include <stdlib.h>
#include <string.h>
#include <stdio.h>
#include <errno.h>
#include <malloc.h>
#include "mmlrt.h"

/* ---------- UTF-8 <-> UTF-16 ----------
 * `str` is valid UTF-8 on every platform; all conversion happens
 * here, exactly once, at the edge. UTF-16->UTF-8 uses
 * WC_ERR_INVALID_CHARS so unpaired surrogates fail loudly instead of
 * emitting U+FFFD — same contract as POSIX argv validation. */

static wchar_t *u8_to_u16(const char *s, long long nlen) {
    int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                s, (int)nlen, NULL, 0);
    if (n <= 0) return NULL;
    wchar_t *w = (wchar_t *)malloc((size_t)(n + 1) * sizeof(wchar_t));
    if (!w) return NULL;
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, s, (int)nlen, w, n);
    w[n] = 0;
    return w;
}

/* Returns a malloc'd UTF-8 buffer, or NULL if the wide string
 * contains unpaired surrogates. Caller owns; several sites leak by
 * design (process-lifetime views). */
static char *u16_to_u8(const wchar_t *w) {
    int n = WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS,
                                w, -1, NULL, 0, NULL, NULL);
    if (n <= 0) return NULL;
    char *u = (char *)malloc((size_t)n);
    if (!u) return NULL;
    if (!WideCharToMultiByte(CP_UTF8, WC_ERR_INVALID_CHARS,
                             w, -1, u, n, NULL, NULL)) {
        free(u);
        return NULL;
    }
    return u;
}

/* ---------- files ---------- */

/* GetLastError -> errno for the fd layer's Err(mml_errno) contract
 * (N79C #8 — CreateFileW reports Win32 codes, not CRT errno). */
static int win_err_to_errno(DWORD e) {
    switch (e) {
    case ERROR_FILE_NOT_FOUND:
    case ERROR_PATH_NOT_FOUND:       return ENOENT;
    case ERROR_ACCESS_DENIED:
    case ERROR_SHARING_VIOLATION:
    case ERROR_LOCK_VIOLATION:
    case ERROR_NETWORK_ACCESS_DENIED: return EACCES;
    case ERROR_FILE_EXISTS:
    case ERROR_ALREADY_EXISTS:       return EEXIST;
    case ERROR_DISK_FULL:
    case ERROR_HANDLE_DISK_FULL:     return ENOSPC;
    case ERROR_DIR_NOT_EMPTY:        return ENOTEMPTY;
    case ERROR_INVALID_NAME:
    case ERROR_BAD_PATHNAME:
    case ERROR_FILENAME_EXCED_RANGE: return ENAMETOOLONG;
    case ERROR_DIRECTORY:            return ENOTDIR;
    default:                         return EINVAL;
    }
}

/* Raw escape hatch — caller already holds host flag bits. Path is
 * still UTF-8: the conversion is ours, not theirs.
 *
 * N79C #8: _wopen can't parse the "\\?\" verbatim paths
 * mml_realpath returns (EINVAL — "device does not recognize the
 * command"). CreateFileW takes them natively and keeps long-path,
 * UNC, and device coverage; _open_osfhandle turns the handle into
 * an ordinary CRT fd so _read/_write/_close are untouched.
 * Append rides through as the _O_APPEND fd flag, so _write still
 * seeks to EOF first. */
int mml_fd_open_mode(const char *p, int flags, int mode) {
    wchar_t *w = u8_to_u16(p, (long long)strlen(p));
    if (!w) { errno = ENOENT; return -1; }
    DWORD access;
    switch (flags & (_O_RDONLY | _O_WRONLY | _O_RDWR)) {
    case _O_WRONLY: access = GENERIC_WRITE; break;
    case _O_RDWR:   access = GENERIC_READ | GENERIC_WRITE; break;
    default:        access = GENERIC_READ; break;
    }
    if (flags & _O_APPEND) access |= FILE_APPEND_DATA;
    DWORD disp;
    if ((flags & (_O_CREAT | _O_EXCL)) == (_O_CREAT | _O_EXCL)) {
        disp = CREATE_NEW;
    } else if ((flags & (_O_CREAT | _O_TRUNC)) == (_O_CREAT | _O_TRUNC)) {
        disp = CREATE_ALWAYS;
    } else if (flags & _O_CREAT) {
        disp = OPEN_ALWAYS;
    } else if (flags & _O_TRUNC) {
        disp = TRUNCATE_EXISTING;
    } else {
        disp = OPEN_EXISTING;
    }
    DWORD attrs = FILE_ATTRIBUTE_NORMAL;
    if (!(mode & _S_IWRITE)) attrs = FILE_ATTRIBUTE_READONLY;
    HANDLE h = CreateFileW(w, access,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        NULL, disp, attrs, NULL);
    DWORD ge = GetLastError();   /* capture before free() */
    free(w);
    if (h == INVALID_HANDLE_VALUE) {
        _set_doserrno((int)ge);
        errno = win_err_to_errno(ge);
        return -1;
    }
    /* Only CRT-meaningful flag bits cross: accmode, append, text. */
    int fd = _open_osfhandle((intptr_t)h,
        (flags & (_O_APPEND | _O_RDWR | _O_TEXT | _O_NOINHERIT)) |
        _O_BINARY);
    if (fd < 0) { CloseHandle(h); return -1; }
    return fd;
}

int mml_fd_open_flags(const char *p, int oflags, int mode) {
    int rd = oflags & MML_OF_READ, wr = oflags & MML_OF_WRITE;
    int f = (rd && wr) ? _O_RDWR : wr ? _O_WRONLY : _O_RDONLY;
    if (oflags & MML_OF_CREATE) f |= _O_CREAT;
    if (oflags & MML_OF_TRUNC)  f |= _O_TRUNC;
    if (oflags & MML_OF_APPEND) f |= _O_APPEND;
    /* _O_BINARY added by mml_fd_open_mode — never _O_TEXT (axiom). */
    return mml_fd_open_mode(p, f, mode);
}

/* _read/_write take `unsigned int` — clamp chunk size; partial I/O
 * is already legal under the language's "wrote <= n" contract. */
long long mml_fd_read(int fd, void *buf, long long n) {
    if (n > 0x7fffffff) n = 0x7fffffff;
    return (long long)_read(fd, buf, (unsigned)n);
}

long long mml_fd_write(int fd, const void *buf, long long n) {
    if (n > 0x7fffffff) n = 0x7fffffff;
    return (long long)_write(fd, buf, (unsigned)n);
}

int mml_fd_close(int fd) { return _close(fd); }

int mml_errno(void) { return errno; }

/* ---------- sockets ----------
 * SOCKET is UINT_PTR — the boundary type is i64; POSIX-int-shaped
 * call sites never see the difference. Winsock initializes lazily
 * on the first socket call and cleans up at exit; std.net never
 * sees WSAStartup. */

static INIT_ONCE g_wsa_once = INIT_ONCE_STATIC_INIT;
static int g_wsa_ok = 0;

static BOOL CALLBACK wsa_init_once(PINIT_ONCE once, PVOID param,
                                   PVOID *ctx) {
    (void)once; (void)param; (void)ctx;
    WSADATA wd;
    if (WSAStartup(MAKEWORD(2, 2), &wd) != 0) return FALSE;
    g_wsa_ok = 1;
    atexit((void (*)(void))WSACleanup);
    return TRUE;
}

static int sock_init(void) {
    if (!InitOnceExecuteOnce(&g_wsa_once, wsa_init_once, NULL, NULL))
        return -1;
    return g_wsa_ok ? 0 : -1;
}

long long mml_sock_socket(int domain, int ty, int proto) {
    if (sock_init() != 0) { WSASetLastError(WSANOTINITIALISED); return -1; }
    return (long long)socket(domain, ty, proto);   /* INVALID_SOCKET -> -1 */
}

int mml_sock_bind(long long s, const void *addr, unsigned len) {
    return bind((SOCKET)s, (const struct sockaddr *)addr, (int)len);
}

int mml_sock_connect(long long s, const void *addr, unsigned len) {
    return connect((SOCKET)s, (const struct sockaddr *)addr, (int)len);
}

int mml_sock_listen(long long s, int backlog) {
    return listen((SOCKET)s, backlog);
}

long long mml_sock_accept(long long s) {
    return (long long)accept((SOCKET)s, NULL, NULL);
}

int mml_sock_reuse_addr(long long s) {
    char one = 1;
    return setsockopt((SOCKET)s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
}

long long mml_sock_recv(long long s, void *buf, long long n) {
    if (n > 0x7fffffff) n = 0x7fffffff;
    return (long long)recv((SOCKET)s, (char *)buf, (int)n, 0);
}

long long mml_sock_send(long long s, const void *buf, long long n) {
    if (n > 0x7fffffff) n = 0x7fffffff;
    return (long long)send((SOCKET)s, (const char *)buf, (int)n, 0);
}

int mml_sock_close(long long s) {
    return closesocket((SOCKET)s);
}

/* WSA codes -> the POSIX errno values std.net callers learned. The
 * Err contract already says the payload is host-shaped; mapping the
 * common dozen keeps `Err(e)` comparable across platforms. */
int mml_sock_errno(void) {
    switch (WSAGetLastError()) {
    case WSAEWOULDBLOCK:  return EAGAIN;
    case WSAEINTR:        return EINTR;
    case WSAEBADF:        return EBADF;
    case WSAEACCES:       return EACCES;
    case WSAEINVAL:       return EINVAL;
    case WSAEMFILE:       return EMFILE;
    case WSAENOTSOCK:     return EBADF;
    case WSAECONNRESET:   return ECONNRESET;
    case WSAECONNABORTED: return ECONNABORTED;
    case WSAECONNREFUSED: return ECONNREFUSED;
    case WSAEADDRINUSE:   return EADDRINUSE;
    case WSAEADDRNOTAVAIL:return EADDRNOTAVAIL;
    case WSAETIMEDOUT:    return ETIMEDOUT;
    case WSAENETUNREACH:  return ENETUNREACH;
    case WSAEHOSTUNREACH: return EHOSTUNREACH;
    case WSAEISCONN:      return EISCONN;
    case WSAENOTCONN:     return ENOTCONN;
    case WSAEINPROGRESS:  return EINPROGRESS;
    default:              return WSAGetLastError();
    }
}

void mml_sockaddr_v4(unsigned char *sa, int o0, int o1, int o2, int o3,
                     int port) {
    struct sockaddr_in in;
    memset(&in, 0, sizeof(in));
    in.sin_family = AF_INET;
    in.sin_port = htons((unsigned short)port);
    in.sin_addr.s_addr = htonl(((unsigned)o0 << 24) |
                               ((unsigned)o1 << 16) |
                               ((unsigned)o2 << 8) | (unsigned)o3);
    memcpy(sa, &in, sizeof(in) <= 16 ? sizeof(in) : 16);
}

/* ---------- process ---------- */

int mml_getpid(void) { return (int)GetCurrentProcessId(); }
int mml_yield(void) { return SwitchToThread() ? 0 : -1; }
int mml_sleep_us(int usec) {
    if (usec <= 0) { Sleep(0); return 0; }
    Sleep((DWORD)((usec + 999) / 1000));   /* ms granularity, round up */
    return 0;
}
void mml_exit(int code) { _exit(code); }   /* no flush — same as POSIX */

int mml_env_get(const char *name, long long nlen,
                const char **out_ptr, long long *out_len) {
    if (nlen < 0 || nlen >= 4096) return 3;
    wchar_t *wn = u8_to_u16(name, nlen);
    if (!wn) return 3;
    DWORD need = GetEnvironmentVariableW(wn, NULL, 0);
    if (need == 0) { free(wn); return 1; }            /* missing */
    wchar_t *wv = (wchar_t *)malloc((size_t)need * sizeof(wchar_t));
    if (!wv) { free(wn); return 4; }
    DWORD got = GetEnvironmentVariableW(wn, wv, need);
    free(wn);
    if (got == 0 || got >= need) { free(wv); return 1; }
    char *v = u16_to_u8(wv);                          /* strict */
    free(wv);
    if (!v) return 2;                                 /* not UTF-8 */
    *out_ptr = v;                                     /* leaked view */
    *out_len = (long long)strlen(v);
    return 0;
}

long long mml_getcwd(char *buf, long long cap) {
    wchar_t *w = _wgetcwd(NULL, 0);
    if (!w) return -ENOMEM;
    char *u = u16_to_u8(w);
    free(w);
    if (!u) return -EINVAL;
    long long n = (long long)strlen(u);
    if (n + 1 > cap) { free(u); return -ENAMETOOLONG; }
    memcpy(buf, u, (size_t)n + 1);
    free(u);
    return n;
}

/* N81 — SetEnvironmentVariableW for the toolchain dispatcher's
 * OUR_DISPATCHED marker on re-exec. */
int mml_env_set(const char *name, long long nlen,
                const char *val, long long vlen) {
    if (nlen < 0 || nlen >= 4096) return -ERROR_INVALID_PARAMETER;
    wchar_t *wn = u8_to_u16(name, nlen);
    wchar_t *wv = u8_to_u16(val, vlen);
    if (!wn || !wv) { free(wn); free(wv); return -ERROR_INVALID_PARAMETER; }
    int ok = SetEnvironmentVariableW(wn, wv);
    DWORD e = GetLastError();
    free(wn); free(wv);
    return ok ? 0 : -(long long)e;
}

/* N81 — the running executable's path for sibling dispatch. */
long long mml_self_exe(char *buf, long long cap) {
    DWORD n = GetModuleFileNameW(NULL, NULL, 0);
    if (n == 0) return -(long long)GetLastError();
    wchar_t *w = (wchar_t *)malloc(((size_t)n + 1) * sizeof(wchar_t));
    if (!w) return -ENOMEM;
    DWORD got = GetModuleFileNameW(NULL, w, n + 1);
    if (got == 0 || got > n) {
        DWORD e = GetLastError();
        free(w);
        return -(long long)e;
    }
    char *u = u16_to_u8(w);
    free(w);
    if (!u) return -EINVAL;
    long long m = (long long)strlen(u);
    if (m + 1 > cap) { free(u); return -ENAMETOOLONG; }
    memcpy(buf, u, (size_t)m + 1);
    free(u);
    return m;
}

long long mml_realpath(const char *path, long long len,
                       char *buf, long long cap) {
    wchar_t *w = u8_to_u16(path, len);
    if (!w) return -EINVAL;
    /* BACKUP_SEMANTICS opens directories too — canonicalize does
     * not require the target to be a file. */
    HANDLE h = CreateFileW(w, 0, FILE_SHARE_READ | FILE_SHARE_WRITE |
        FILE_SHARE_DELETE, NULL, OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS, NULL);
    free(w);
    if (h == INVALID_HANDLE_VALUE) return -(long long)GetLastError();
    DWORD n = GetFinalPathNameByHandleW(h, NULL, 0, VOLUME_NAME_DOS);
    if (!n) { CloseHandle(h); return -(long long)GetLastError(); }
    wchar_t *fw = (wchar_t *)malloc(((size_t)n + 1) * sizeof(wchar_t));
    if (!fw) { CloseHandle(h); return -ENOMEM; }
    DWORD got = GetFinalPathNameByHandleW(h, fw, n + 1, VOLUME_NAME_DOS);
    CloseHandle(h);
    if (!got || got > n) { free(fw); return -EIO; }
    fw[got] = 0;
    char *u = u16_to_u8(fw);
    free(fw);
    if (!u) return -EINVAL;
    long long un = (long long)strlen(u);
    if (un > cap) { free(u); return -ENAMETOOLONG; }
    memcpy(buf, u, (size_t)un);
    free(u);
    return un;
}

int mml_path_kind(const char *path, long long len) {
    wchar_t *w = u8_to_u16(path, len);
    if (!w) return -EINVAL;
    DWORD a = GetFileAttributesW(w);
    free(w);
    if (a == INVALID_FILE_ATTRIBUTES) return -(int)GetLastError();
    if (a & FILE_ATTRIBUTE_DIRECTORY) return 2;
    return 1;
}

long long mml_strerror(int e, char *buf, long long cap) {
    /* Negative e is a -GetLastError — flip back for FormatMessage. */
    DWORD code = e < 0 ? (DWORD)(-e) : (DWORD)e;
    wchar_t wbuf[512];
    DWORD n = FormatMessageW(
        FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS,
        NULL, code, 0, wbuf, 512, NULL);
    char *u;
    if (n > 0) {
        wbuf[n] = 0;
        u = u16_to_u8(wbuf);
    } else {
        /* errno-space codes: CRT strerror. */
        u = _strdup(strerror(e < 0 ? errno : e));
    }
    if (!u) return -EINVAL;
    long long un = (long long)strlen(u);
    /* FormatMessage trails "\r\n" — Rust's io::Error doesn't. */
    while (un > 0 && (u[un - 1] == '\r' || u[un - 1] == '\n')) un--;
    if (un > cap) { free(u); return -ENAMETOOLONG; }
    memcpy(buf, u, (size_t)un);
    free(u);
    return un;
}

/* ---------- tasks ---------- */

struct mml_task {
    HANDLE h;
    mml_task_fn fn;
    void *env;
};

static unsigned __stdcall mml_task_trampoline(void *arg) {
    mml_task *t = (mml_task *)arg;
    t->fn(t->env);
    return 0;
}

mml_task *mml_task_spawn(mml_task_fn fn, void *env) {
    mml_task *t = (mml_task *)malloc(sizeof(mml_task));
    t->fn = fn;
    t->env = env;
    uintptr_t h = _beginthreadex(NULL, 0,
        mml_task_trampoline, t, 0, NULL);
    t->h = (HANDLE)h;
    return t;
}

void mml_task_join(mml_task *t) {
    WaitForSingleObject(t->h, INFINITE);
    CloseHandle(t->h);
    free(t);
}

/* ---------- mutexes (SRWLOCK — exclusive) ---------- */

struct mml_mutex {
    SRWLOCK mu;
    void *data;
    char payload[];
};

mml_mutex *mml_mutex_new(size_t payload_sz) {
    mml_mutex *m = (mml_mutex *)malloc(sizeof(mml_mutex) + payload_sz);
    InitializeSRWLock(&m->mu);
    m->data = m->payload;
    return m;
}

mml_mutex *mml_mutex_wrap(void *p) {
    mml_mutex *m = (mml_mutex *)malloc(sizeof(mml_mutex));
    InitializeSRWLock(&m->mu);
    m->data = p;
    return m;
}

void *mml_mutex_data(mml_mutex *m) { return m->data; }

void *mml_mutex_lock(mml_mutex *m) {
    AcquireSRWLockExclusive(&m->mu);
    return m;
}

void mml_mutex_unlock(mml_mutex *m) { ReleaseSRWLockExclusive(&m->mu); }

/* ---------- channels ---------- */

typedef struct mml_qn {
    struct mml_qn *next;
} mml_qn;

struct mml_chan {
    SRWLOCK mu;
    CONDITION_VARIABLE cv;
    mml_qn *head, *tail;
    int closed;
    size_t esz;
};

mml_chan *mml_chan_new(size_t esz) {
    mml_chan *c = (mml_chan *)malloc(sizeof(mml_chan));
    InitializeSRWLock(&c->mu);
    InitializeConditionVariable(&c->cv);
    c->head = c->tail = NULL;
    c->closed = 0;
    c->esz = esz;
    return c;
}

void mml_chan_send(mml_chan *c, const void *src) {
    mml_qn *n = (mml_qn *)malloc(sizeof(mml_qn) + c->esz);
    n->next = NULL;
    memcpy(n + 1, src, c->esz);
    AcquireSRWLockExclusive(&c->mu);
    if (c->tail) c->tail->next = n; else c->head = n;
    c->tail = n;
    WakeConditionVariable(&c->cv);
    ReleaseSRWLockExclusive(&c->mu);
}

void mml_chan_recv(mml_chan *c, void *dst) {
    AcquireSRWLockExclusive(&c->mu);
    while (!c->head && !c->closed)
        SleepConditionVariableSRW(&c->cv, &c->mu, INFINITE, 0);
    mml_qn *n = c->head;
    if (n) {
        c->head = n->next;
        if (!c->head) c->tail = NULL;
        memcpy(dst, n + 1, c->esz);
    } else {
        memset(dst, 0, c->esz);
    }
    ReleaseSRWLockExclusive(&c->mu);
    free(n);
}

void mml_chan_close(mml_chan *c) {
    AcquireSRWLockExclusive(&c->mu);
    c->closed = 1;
    WakeAllConditionVariable(&c->cv);
    ReleaseSRWLockExclusive(&c->mu);
}

/* ---------- directories (N65A) — BLIND ----------
 * FindFirstFileW/FindNextFileW behind the opaque i64 handle — a
 * small state block holds the search HANDLE, the current find data,
 * a pending-first flag, and the last converted UTF-8 name (freed on
 * the next call and at close — the POSIX "valid until next call"
 * rule holds). Names that won't convert to strict UTF-8 are
 * invisible, same as POSIX's mml_str_check filter. */

typedef struct {
    HANDLE h;
    WIN32_FIND_DATAW cur;
    int pending;
    char *name;
} mml_dir_state;

long long mml_dir_open(const char *path, long long len) {
    /* Build the "<path>\*" wide pattern: convert len+2 chars so the
     * appended slash+star fit, then terminate. */
    wchar_t *pat = (wchar_t *)malloc(sizeof(wchar_t) * ((size_t)len + 3));
    if (!pat) return -(long long)ENOMEM;
    int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, path,
                                (int)len, pat, (int)len + 2);
    if (n <= 0) { free(pat); return -(long long)ERROR_INVALID_NAME; }
    pat[n] = L'\\'; pat[n + 1] = L'*'; pat[n + 2] = 0;
    mml_dir_state *st = calloc(1, sizeof(mml_dir_state));
    if (!st) { free(pat); return -(long long)ENOMEM; }
    st->h = FindFirstFileW(pat, &st->cur);
    free(pat);
    if (st->h == INVALID_HANDLE_VALUE) {
        free(st);
        return -(long long)GetLastError();
    }
    st->pending = 1;
    return (long long)(intptr_t)st;
}

int mml_dir_next(long long dh, const char **name, long long *nlen, int *kind) {
    mml_dir_state *st = (mml_dir_state *)(intptr_t)dh;
    for (;;) {
        if (st->pending) st->pending = 0;
        else if (!FindNextFileW(st->h, &st->cur)) {
            DWORD e = GetLastError();
            return e == ERROR_NO_MORE_FILES ? 0 : -(int)e;
        }
        if (!wcscmp(st->cur.cFileName, L".") ||
            !wcscmp(st->cur.cFileName, L"..")) continue;
        char *u = u16_to_u8(st->cur.cFileName);
        if (!u) continue;  /* not representable as UTF-8: invisible */
        free(st->name);
        st->name = u;
        *name = u;
        *nlen = (long long)strlen(u);
        *kind = (st->cur.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY) ? 2 : 1;
        return 1;
    }
}

int mml_dir_close(long long dh) {
    mml_dir_state *st = (mml_dir_state *)(intptr_t)dh;
    if (st->h != INVALID_HANDLE_VALUE) FindClose(st->h);
    free(st->name);
    free(st);
    return 0;
}

int mml_mkdir(const char *path, long long len, int mode) {
    (void)mode; /* no mode bits on Windows */
    wchar_t *w = u8_to_u16(path, len);
    if (!w) return -ENOENT;
    /* CreateDirectoryW, not _wmkdir — same "\\?\"-verbatim wall as
     * _wopen (N79C #8). -GetLastError matches realpath/rename. */
    int ok = CreateDirectoryW(w, NULL);
    DWORD e = GetLastError();
    free(w);
    return ok ? 0 : -(long long)e;
}

int mml_remove(const char *path, long long len) {
    wchar_t *w = u8_to_u16(path, len);
    if (!w) return -ENOENT;
    /* DeleteFileW handles files only; RemoveDirectoryW the dirs —
     * try both, as POSIX remove() does. Win32 calls: verbatim "\\?\"
     * paths work here too. */
    int ok = DeleteFileW(w);
    if (!ok) ok = RemoveDirectoryW(w);
    DWORD e = GetLastError();
    free(w);
    return ok ? 0 : -(long long)e;
}

/* N81 — the dep-cache tmp→final swap: MoveFileExW across an
 * existing target needs REPLACE_EXISTING; COPY_ALLOWED covers the
 * (rare) cross-volume move. */
int mml_rename(const char *from, long long flen,
               const char *to, long long tlen) {
    wchar_t *a = u8_to_u16(from, flen);
    wchar_t *b = u8_to_u16(to, tlen);
    if (!a || !b) { free(a); free(b); return -ENOENT; }
    int ok = MoveFileExW(a, b,
        MOVEFILE_REPLACE_EXISTING | MOVEFILE_COPY_ALLOWED);
    DWORD e = GetLastError();
    free(a); free(b);
    return ok ? 0 : -(long long)e;
}

/* ---------- process spawn (N65B) — BLIND ----------
 * CreateProcessW takes one command *line*, not argv — args arrive
 * NUL-separated and are quoted per the MSVCRT rules: wrap in
 * double quotes, backslash-escape inner quotes and trailing
 * backslashes. `cwd` maps to lpCurrentDirectory; stdio inherits. */

static void qarg(wchar_t **dst, const wchar_t *w) {
    wchar_t *d = *dst;
    *d++ = L'"';
    size_t slashes = 0;
    for (const wchar_t *c = w; *c; c++) {
        if (*c == L'\\') { slashes++; }
        else if (*c == L'"') {
            while (slashes--) { *d++ = L'\\'; *d++ = L'\\'; }
            *d++ = L'\\'; *d++ = L'"';
            slashes = 0;
            continue;
        } else slashes = 0;
        *d++ = *c;
    }
    while (slashes--) { *d++ = L'\\'; *d++ = L'\\'; } /* doubled at close-quote */
    *d++ = L'"';
    *d = 0;
    *dst = d;
}

long long mml_proc_run(const char *exe, long long elen,
                       const char *args, long long alen,
                       const char *cwd, long long clen) {
    /* Worst-case wide size: each byte at most one wchar plus quoting
     * overhead bounded by 2*len + separators. */
    size_t wcap = (size_t)(elen + alen) * 4 + 16;
    wchar_t *wline = malloc(sizeof(wchar_t) * wcap);
    wchar_t *wexe = u8_to_u16(exe, elen);
    if (!wline || !wexe) { free(wline); free(wexe); return -(long long)ENOMEM; }
    wchar_t *d = wline;
    qarg(&d, wexe); /* argv[0] */
    int in_arg = 0;
    for (long long i = 0; i < alen; i++) {
        if (args[i] != '\0' && !in_arg) {
            *d++ = L' ';
            /* measure this arg's extent */
            long long j = i;
            while (j < alen && args[j] != '\0') j++;
            wchar_t *wa = u8_to_u16(&args[i], j - i);
            if (wa) { qarg(&d, wa); free(wa); }
        }
        in_arg = args[i] != '\0';
    }
    free(wexe);
    wchar_t *wcwd = clen > 0 ? u8_to_u16(cwd, clen) : NULL;
    STARTUPINFOW si; PROCESS_INFORMATION pi;
    memset(&si, 0, sizeof(si)); si.cb = sizeof(si);
    memset(&pi, 0, sizeof(pi));
    if (!CreateProcessW(NULL, wline, NULL, NULL, TRUE, 0, NULL, wcwd,
                        &si, &pi)) {
        long long e = -(long long)GetLastError();
        free(wline); free(wcwd);
        return e;
    }
    free(wline); free(wcwd);
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 0;
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    return (long long)code; /* Windows has no signals — exit codes only */
}

/* `mml_proc_run_cap` — same command-line build; the captured stream
 * becomes a real file HANDLE on STARTUPINFO. Inheriting the other
 * handle keeps the un-captured direction a live tty. */
long long mml_proc_run_cap(const char *exe, long long elen,
                           const char *args, long long alen,
                           const char *cwd, long long clen,
                           const char *cap, long long caplen,
                           long long which) {
    size_t wcap2 = (size_t)(elen + alen) * 4 + 16;
    wchar_t *wline = malloc(sizeof(wchar_t) * wcap2);
    wchar_t *wexe = u8_to_u16(exe, elen);
    if (!wline || !wexe) { free(wline); free(wexe); return -(long long)ENOMEM; }
    wchar_t *d = wline;
    qarg(&d, wexe);
    int in_arg = 0;
    for (long long i = 0; i < alen; i++) {
        if (args[i] != '\0' && !in_arg) {
            *d++ = L' ';
            long long j = i;
            while (j < alen && args[j] != '\0') j++;
            wchar_t *wa = u8_to_u16(&args[i], j - i);
            if (wa) { qarg(&d, wa); free(wa); }
        }
        in_arg = args[i] != '\0';
    }
    free(wexe);
    wchar_t *wcwd = clen > 0 ? u8_to_u16(cwd, clen) : NULL;
    STARTUPINFOW si; PROCESS_INFORMATION pi;
    memset(&si, 0, sizeof(si)); si.cb = sizeof(si);
    memset(&pi, 0, sizeof(pi));
    HANDLE hcap = NULL;
    if (caplen > 0 && which >= 1 && which <= 3) {
        wchar_t *wcap = u8_to_u16(cap, caplen);
        if (wcap) {
            hcap = CreateFileW(wcap, GENERIC_WRITE, FILE_SHARE_READ,
                               NULL, CREATE_ALWAYS,
                               FILE_ATTRIBUTE_NORMAL, NULL);
            free(wcap);
        }
        if (hcap && hcap != INVALID_HANDLE_VALUE) {
            si.dwFlags |= STARTF_USESTDHANDLES;
            si.hStdInput = GetStdHandle(STD_INPUT_HANDLE);
            si.hStdOutput = which == 1 || which == 3 ? hcap
                                       : GetStdHandle(STD_OUTPUT_HANDLE);
            si.hStdError = which >= 2 ? hcap
                                      : GetStdHandle(STD_ERROR_HANDLE);
        } else {
            hcap = NULL;
        }
    }
    if (!CreateProcessW(NULL, wline, NULL, NULL, TRUE, 0, NULL, wcwd,
                        &si, &pi)) {
        long long e = -(long long)GetLastError();
        free(wline); free(wcwd);
        if (hcap) CloseHandle(hcap);
        return e;
    }
    free(wline); free(wcwd);
    if (hcap) CloseHandle(hcap);
    WaitForSingleObject(pi.hProcess, INFINITE);
    DWORD code = 0;
    GetExitCodeProcess(pi.hProcess, &code);
    CloseHandle(pi.hProcess);
    CloseHandle(pi.hThread);
    return (long long)code;
}

/* ---------- process entry ----------
 * Wide argv is the only correct argv on Windows — ANSI `main` bytes
 * are codepage-lossy. Each UTF-16 arg converts strictly; an arg that
 * can't be UTF-8 traps at startup, exactly like POSIX argv bytes
 * that fail mml_str_validate. Storage is runtime-owned (leaked —
 * process-lifetime views, same as POSIX borrowing kernel storage). */

int wmain(int argc, wchar_t **argv) {
    /* keep abort() quiet and dialog-free for the trap corpus */
    _set_abort_behavior(0, _WRITE_ABORT_MSG | _CALL_REPORTFAULT);

    /* std fds are CRT text mode by default — LF becomes CRLF through
     * _write, which would corrupt every canonical byte surface
     * (dumps, diffs, pipes). Same axiom as mml_fd_open_mode: binary. */
    _setmode(1, _O_BINARY);
    _setmode(2, _O_BINARY);

    int n = argc > 0 ? argc : 0;
    mml_str_t *args = (mml_str_t *)malloc((size_t)(n > 0 ? n : 1)
                                          * sizeof(mml_str_t));
    for (int i = 0; i < n; i++) {
        char *u = u16_to_u8(argv[i]);
        if (!u) {
            fprintf(stderr, "mml: argv[%d] is not representable as UTF-8\n", i);
            abort();
        }
        args[i].ptr = u;
        args[i].len = (long long)strlen(u);
    }
    mml_stash_argv(n, args);
    return (int)mml_main_args(args, n);
}

/* ---------- platform ---------- */
long long mml_target_triple(const char **out) {
#if defined(_M_X64) || defined(__x86_64__)
    static const char T[] = "x86_64-pc-windows-msvc";
#elif defined(_M_ARM64) || defined(__aarch64__)
    static const char T[] = "aarch64-pc-windows-msvc";
#else
    static const char T[] = "x86_64-pc-windows-msvc";
#endif
    *out = T;
    return (long long)(sizeof(T) - 1);
}
