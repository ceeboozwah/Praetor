/* mmlrt_win32.c — the Win32 half of the runtime boundary (N58C-2).
 * Implements the mmlrt.h contract with pure Win32 + UCRT: no POSIX
 * emulation. STATUS: written to the design contract
 * (WINDOWS-PORT.md) but not yet compiled or run on Windows — every
 * behavior here is unverified until the Windows protocol passes.
 *
 *   spawn { ... }   -> CreateThread via _beginthreadex
 *   mutex T         -> SRWLOCK (exclusive)
 *   channel T       -> SRWLOCK + CONDITION_VARIABLE
 *   files           -> _wopen/_read/_write/_close, always _O_BINARY
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

/* Raw escape hatch — caller already holds host flag bits. Path is
 * still UTF-8: the conversion is ours, not theirs. */
int mml_fd_open_mode(const char *p, int flags, int mode) {
    wchar_t *w = u8_to_u16(p, (long long)strlen(p));
    if (!w) { errno = ENOENT; return -1; }
    int fd = _wopen(w, flags | _O_BINARY, mode & (_S_IREAD | _S_IWRITE));
    free(w);
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
                                   PVOID ctx) {
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
    uintptr_t h = _beginthreadex(NULL, 0, mml_task_trampoline, t, 0, NULL);
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

/* ---------- process entry ----------
 * Wide argv is the only correct argv on Windows — ANSI `main` bytes
 * are codepage-lossy. Each UTF-16 arg converts strictly; an arg that
 * can't be UTF-8 traps at startup, exactly like POSIX argv bytes
 * that fail mml_str_validate. Storage is runtime-owned (leaked —
 * process-lifetime views, same as POSIX borrowing kernel storage). */

int wmain(int argc, wchar_t **argv) {
    /* keep abort() quiet and dialog-free for the trap corpus */
    _set_abort_behavior(0, _WRITE_ABORT_MSG | _CALL_REPORTFAULT);

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
