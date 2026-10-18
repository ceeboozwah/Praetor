/* mmlrt_posix.c — the POSIX half of the runtime boundary (was
 * mmlrt.c; split in N58C-2). Only this file knows about pthreads,
 * POSIX fds, and BSD sockets.
 *
 *   spawn { ... }   -> mml_task_spawn(fn, env) / mml_task_join(t)
 *   mutex T         -> mml_mutex_new / _lock / _unlock_data
 *   channel T       -> mml_chan_new / _send / _recv / _close
 *
 * Namespaces (mmlrt.h): mml_fd_* are file descriptors; mml_sock_* are
 * sockets as i64 handles. On POSIX both are ints underneath — the
 * split exists because on Windows a SOCKET is neither a CRT fd nor
 * 32 bits. Socket I/O never routes through the fd functions.
 */
#include "mmlrt.h"
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <errno.h>
#include <unistd.h>
#include <sched.h>
#include <dirent.h>
#include <sys/types.h>
#include <sys/stat.h>
#include <sys/wait.h>
#ifdef __APPLE__
#include <mach-o/dyld.h>   /* _NSGetExecutablePath — mml_self_exe */
#endif
#include <sys/socket.h>
#include <netinet/in.h>
#include <arpa/inet.h>
#include <stdio.h>

/* ---------- files ---------- */

/* N24C — open(2) is variadic; declaring it as a fixed-arity extern
 * passes the trailing `mode` argument incorrectly on some ABIs.
 * This fixed-arity shim makes the C-side call — the raw escape
 * hatch for callers that already hold host flags. */
int mml_fd_open_mode(const char *p, int flags, int mode) {
    return open(p, flags, mode);
}

/* N58B — .lang code passes the portable MML_OF_* bits; this file
 * owns the host's O_* values (O_CREAT is 0x40 on Linux, 0x200 on
 * macOS). */
int mml_fd_open_flags(const char *p, int oflags, int mode) {
    int rd = oflags & MML_OF_READ, wr = oflags & MML_OF_WRITE;
    int f = (rd && wr) ? O_RDWR : wr ? O_WRONLY : O_RDONLY;
    if (oflags & MML_OF_CREATE) f |= O_CREAT;
    if (oflags & MML_OF_TRUNC)  f |= O_TRUNC;
    if (oflags & MML_OF_APPEND) f |= O_APPEND;
    return open(p, f, mode);
}

long long mml_fd_read(int fd, void *buf, long long n) {
    return (long long)read(fd, buf, (size_t)n);
}

long long mml_fd_write(int fd, const void *buf, long long n) {
    return (long long)write(fd, buf, (size_t)n);
}

int mml_fd_close(int fd) {
    return close(fd);
}

/* N30 — Result-shaped stdlib APIs need real error values, not the
 * raw `-1`. `errno` is thread-local, so reading it immediately after
 * a failed syscall is exact; wrappers do `Err(mml_errno())`. */
int mml_errno(void) {
    return errno;
}

/* ---------- sockets ----------
 * i64 handles at the boundary; on POSIX they carry plain int fds. */

static int sock_init(void) { return 0; }   /* POSIX needs no startup */

long long mml_sock_socket(int domain, int ty, int proto) {
    (void)sock_init();
    return (long long)socket(domain, ty, proto);
}

int mml_sock_bind(long long s, const void *addr, unsigned len) {
    return bind((int)s, (const struct sockaddr *)addr, len);
}

int mml_sock_connect(long long s, const void *addr, unsigned len) {
    return connect((int)s, (const struct sockaddr *)addr, len);
}

int mml_sock_listen(long long s, int backlog) {
    return listen((int)s, backlog);
}

long long mml_sock_accept(long long s) {
    return (long long)accept((int)s, NULL, NULL);
}

int mml_sock_reuse_addr(long long s) {
    int one = 1;
    return setsockopt((int)s, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
}

/* N58C — sockets are their own namespace; on POSIX these still land
 * on the same syscalls, but nothing above ever assumes that. */
long long mml_sock_recv(long long s, void *buf, long long n) {
    return (long long)read((int)s, buf, (size_t)n);
}

long long mml_sock_send(long long s, const void *buf, long long n) {
    return (long long)write((int)s, buf, (size_t)n);
}

int mml_sock_close(long long s) {
    return close((int)s);
}

int mml_sock_errno(void) {
    return errno;
}

/* N58B — sockaddr layout is an OS ABI fact: BSD carries a leading
 * sin_len byte, Linux and Windows don't. The runtime owns the struct;
 * callers pass a 16-byte buffer and get back the host's sockaddr_in. */
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

int mml_getpid(void) { return (int)getpid(); }
int mml_yield(void) { return sched_yield(); }
int mml_sleep_us(int usec) { return usleep((useconds_t)usec); }
void mml_exit(int code) { _exit(code); }

/* N32 — thin shims over the C process environment. Values come back
 * as borrowed views into process-global storage; the language
 * exposes no environment mutation, so those views stay valid for
 * the process lifetime. */
int mml_env_get(const char *name, long long nlen,
                const char **out_ptr, long long *out_len) {
    if (nlen < 0 || nlen >= 4096) return 3;   /* name too long */
    char buf[4096];
    memcpy(buf, name, (size_t)nlen);
    buf[nlen] = 0;
    const char *v = getenv(buf);
    if (!v) return 1;                          /* missing */
    long long vl = (long long)strlen(v);
    if (!mml_str_check(v, vl)) return 2;       /* not UTF-8 */
    *out_ptr = v;
    *out_len = vl;
    return 0;
}

long long mml_getcwd(char *buf, long long cap) {
    errno = 0;
    if (!getcwd(buf, (size_t)cap)) return -errno;
    return (long long)strlen(buf);
}

/* N81 — `setenv` for the toolchain dispatcher: the re-exec'd child
 * inherits OUR_DISPATCHED so a lying sibling is caught. */
int mml_env_set(const char *name, long long nlen,
                const char *val, long long vlen) {
    if (nlen < 0 || nlen >= 4096) return -EINVAL;
    char nb[4096];
    memcpy(nb, name, (size_t)nlen);
    nb[nlen] = 0;
    char *vb = malloc((size_t)vlen + 1);
    if (!vb) return -ENOMEM;
    memcpy(vb, val, (size_t)vlen);
    vb[vlen] = 0;
    int r = setenv(nb, vb, 1);
    int e = errno;
    free(vb);
    return r ? -e : 0;
}

/* N81 — the running executable's path, for `our toolchain`'s
 * sibling scan and re-exec dispatch. */
long long mml_self_exe(char *buf, long long cap) {
#ifdef __APPLE__
    uint32_t sz = (uint32_t)cap;
    if (_NSGetExecutablePath(buf, &sz) != 0) return -ENAMETOOLONG;
    return (long long)strlen(buf);
#else
    ssize_t n = readlink("/proc/self/exe", buf, (size_t)cap - 1);
    if (n < 0) return -errno;
    buf[n] = 0;
    return (long long)n;
#endif
}

/* ---------- tasks ---------- */

struct mml_task {
    pthread_t th;
    mml_task_fn fn;
    void *env;
};

static void *mml_task_trampoline(void *arg) {
    mml_task *t = (mml_task *)arg;
    return t->fn(t->env);
}

mml_task *mml_task_spawn(mml_task_fn fn, void *env) {
    mml_task *t = (mml_task *)malloc(sizeof(mml_task));
    t->fn = fn;
    t->env = env;
    pthread_create(&t->th, NULL, mml_task_trampoline, t);
    return t;
}

void mml_task_join(mml_task *t) {
    pthread_join(t->th, NULL);
    free(t);
}

/* ---------- mutexes ----------
 * A `mutex T` value is a pointer to the runtime object { lock word,
 * data ptr [, inline payload] }. The GUARD a `lock` produces is the
 * object pointer itself — generated code reaches the payload through
 * `mml_mutex_data`, so both forms work: `mutex v` carries T inline,
 * `mutex h` wraps existing storage. `unlock` takes the guard. */

struct mml_mutex {
    pthread_mutex_t mu;
    void *data;
    char payload[];
};

mml_mutex *mml_mutex_new(size_t payload_sz) {
    mml_mutex *m = (mml_mutex *)malloc(sizeof(mml_mutex) + payload_sz);
    pthread_mutex_init(&m->mu, NULL);
    m->data = m->payload;
    return m;
}

mml_mutex *mml_mutex_wrap(void *p) {
    mml_mutex *m = (mml_mutex *)malloc(sizeof(mml_mutex));
    pthread_mutex_init(&m->mu, NULL);
    m->data = p;
    return m;
}

void *mml_mutex_data(mml_mutex *m) { return m->data; }

void *mml_mutex_lock(mml_mutex *m) {
    pthread_mutex_lock(&m->mu);
    return m;
}

void mml_mutex_unlock(mml_mutex *m) { pthread_mutex_unlock(&m->mu); }

/* ---------- channels ----------
 * Unbounded queue + closed flag; recv blocks while empty-and-open,
 * writes zeroed bytes on closed-and-empty (matches the checker's
 * "opaque payload of declared type" contract for a dead channel). */

typedef struct mml_qn {
    struct mml_qn *next;
    /* esz payload bytes follow */
} mml_qn;

struct mml_chan {
    pthread_mutex_t mu;
    pthread_cond_t cv;
    mml_qn *head, *tail;
    int closed;
    size_t esz;
};

mml_chan *mml_chan_new(size_t esz) {
    mml_chan *c = (mml_chan *)malloc(sizeof(mml_chan));
    pthread_mutex_init(&c->mu, NULL);
    pthread_cond_init(&c->cv, NULL);
    c->head = c->tail = NULL;
    c->closed = 0;
    c->esz = esz;
    return c;
}

void mml_chan_send(mml_chan *c, const void *src) {
    mml_qn *n = (mml_qn *)malloc(sizeof(mml_qn) + c->esz);
    n->next = NULL;
    memcpy(n + 1, src, c->esz);
    pthread_mutex_lock(&c->mu);
    if (c->tail) c->tail->next = n; else c->head = n;
    c->tail = n;
    pthread_cond_signal(&c->cv);
    pthread_mutex_unlock(&c->mu);
}

void mml_chan_recv(mml_chan *c, void *dst) {
    pthread_mutex_lock(&c->mu);
    while (!c->head && !c->closed) pthread_cond_wait(&c->cv, &c->mu);
    mml_qn *n = c->head;
    if (n) {
        c->head = n->next;
        if (!c->head) c->tail = NULL;
        memcpy(dst, n + 1, c->esz);
    } else {
        memset(dst, 0, c->esz);
    }
    pthread_mutex_unlock(&c->mu);
    free(n);
}

void mml_chan_close(mml_chan *c) {
    pthread_mutex_lock(&c->mu);
    c->closed = 1;
    pthread_cond_broadcast(&c->cv);
    pthread_mutex_unlock(&c->mu);
}

/* ---------- directories (N65A) ----------
 * DIR* becomes an opaque i64 handle — the language sees an opaque
 * token, POSIX sees the pointer. Entry names are borrowed views
 * into the DIR buffer (valid until the next call); the .lang
 * wrapper copies each into caller-owned storage immediately. */

long long mml_dir_open(const char *path, long long len) {
    char *buf = malloc((size_t)len + 1);
    if (!buf) return -(long long)ENOMEM;
    memcpy(buf, path, (size_t)len);
    buf[len] = '\0';
    DIR *d = opendir(buf);
    free(buf);
    if (!d) return -(long long)errno;
    return (long long)(intptr_t)d;
}

int mml_dir_next(long long dh, const char **name, long long *nlen, int *kind) {
    DIR *d = (DIR *)(intptr_t)dh;
    for (;;) {
        errno = 0;
        struct dirent *de = readdir(d);
        if (!de) return errno ? -errno : 0;
        if (!strcmp(de->d_name, ".") || !strcmp(de->d_name, "..")) continue;
        size_t nl = strlen(de->d_name);
        if (!mml_str_check(de->d_name, (long long)nl)) continue;
        int k = 0;
#ifdef DT_REG
        if (de->d_type == DT_REG) k = 1;
        else if (de->d_type == DT_DIR) k = 2;
        else if (de->d_type == DT_UNKNOWN)
#endif
        {
            /* Filesystem won't classify — stat the entry itself. */
            struct stat st;
            if (fstatat(dirfd(d), de->d_name, &st, 0) == 0) {
                if (S_ISREG(st.st_mode)) k = 1;
                else if (S_ISDIR(st.st_mode)) k = 2;
            }
        }
        *name = de->d_name;
        *nlen = (long long)nl;
        *kind = k;
        return 1;
    }
}

int mml_dir_close(long long dh) {
    return closedir((DIR *)(intptr_t)dh) ? -errno : 0;
}

int mml_mkdir(const char *path, long long len, int mode) {
    char *buf = malloc((size_t)len + 1);
    if (!buf) return -ENOMEM;
    memcpy(buf, path, (size_t)len);
    buf[len] = '\0';
    int r = mkdir(buf, (mode_t)mode);
    free(buf);
    return r ? -errno : 0;
}

long long mml_realpath(const char *path, long long len,
                       char *buf, long long cap) {
    char *tmp = malloc((size_t)len + 1);
    if (!tmp) return -ENOMEM;
    memcpy(tmp, path, (size_t)len);
    tmp[len] = '\0';
    errno = 0;
    char *r = realpath(tmp, NULL);
    free(tmp);
    if (!r) return errno ? -errno : -EINVAL;
    long long n = (long long)strlen(r);
    if (n > cap) { free(r); return -ENAMETOOLONG; }
    memcpy(buf, r, (size_t)n);
    free(r);
    return n;
}

int mml_path_kind(const char *path, long long len) {
    char *buf = malloc((size_t)len + 1);
    if (!buf) return -ENOMEM;
    memcpy(buf, path, (size_t)len);
    buf[len] = '\0';
    struct stat st;
    int r = stat(buf, &st);
    int e = errno;
    free(buf);
    if (r) return -e;
    if (S_ISREG(st.st_mode)) return 1;
    if (S_ISDIR(st.st_mode)) return 2;
    return 0;
}

long long mml_strerror(int e, char *buf, long long cap) {
    const char *m = strerror(e);
    long long n = (long long)strlen(m);
    if (n > cap) return -ENAMETOOLONG;
    memcpy(buf, m, (size_t)n);
    return n;
}

int mml_remove(const char *path, long long len) {
    char *buf = malloc((size_t)len + 1);
    if (!buf) return -ENOMEM;
    memcpy(buf, path, (size_t)len);
    buf[len] = '\0';
    int r = remove(buf);  /* files and empty dirs both */
    free(buf);
    return r ? -errno : 0;
}

int mml_rename(const char *from, long long flen,
               const char *to, long long tlen) {
    char *a = malloc((size_t)flen + 1);
    char *b = malloc((size_t)tlen + 1);
    if (!a || !b) { free(a); free(b); return -ENOMEM; }
    memcpy(a, from, (size_t)flen); a[flen] = '\0';
    memcpy(b, to, (size_t)tlen);   b[tlen] = '\0';
    int r = rename(a, b);
    int e = errno;
    free(a); free(b);
    return r ? -e : 0;
}

/* ---------- process spawn (N65B) ----------
 * Synchronous spawn-and-wait. fork+execvp keeps it portable and
 * makes `cwd` a plain chdir in the child — no posix_spawn file-action
 * portability questions. An access(X_OK) pre-check turns the common
 * failure (missing/non-executable file) into a negative status; a
 * child that still fails exec _exit(127)s — positive, but only for
 * the genuinely-rare post-fork failure (bad interpreter, etc).
 */


/* PATH search for the spawn preflight: bare names walk $PATH like
 * execvp does — access() alone would reject "clang" even when it
 * resolves (s0 relies on the OS PATH lookup in Command::new). */
static int exe_spawnable(const char *exe) {
    if (strchr(exe, '/')) return access(exe, X_OK);
    const char *path = getenv("PATH");
    if (!path) path = "/usr/bin:/bin:/usr/sbin:/sbin";
    char buf[4096];
    const char *p = path;
    while (*p) {
        const char *colon = strchr(p, ':');
        size_t n = colon ? (size_t)(colon - p) : strlen(p);
        size_t el = strlen(exe);
        if (n + 1 + el < sizeof(buf)) {
            memcpy(buf, p, n);
            buf[n] = '/';
            memcpy(buf + n + 1, exe, el + 1);
            if (access(buf, X_OK) == 0) return 0;
        }
        p = colon ? colon + 1 : p + n;
        if (!colon) break;
    }
    errno = ENOENT;
    return -1;
}

long long mml_proc_run(const char *exe, long long elen,
                       const char *args, long long alen,
                       const char *cwd, long long clen) {
    /* Count args: `args` is NUL-separated ("a\0b\0"); trailing NULs
     * tolerated. */
    int nargs = 0;
    for (long long i = 0; i < alen; i++)
        if (args[i] != '\0' && (i == 0 || args[i - 1] == '\0')) nargs++;
    char *exebuf = malloc((size_t)elen + 1);
    char *argbuf = malloc((size_t)alen + 1);
    char **av = malloc(sizeof(char *) * (size_t)(nargs + 2));
    char *cwdbuf = clen > 0 ? malloc((size_t)clen + 1) : NULL;
    if (!exebuf || !argbuf || !av || (clen > 0 && !cwdbuf)) {
        free(exebuf); free(argbuf); free(av); free(cwdbuf);
        return -(long long)ENOMEM;
    }
    memcpy(exebuf, exe, (size_t)elen); exebuf[elen] = '\0';
    memcpy(argbuf, args, (size_t)alen); argbuf[alen] = '\0';
    if (cwdbuf) { memcpy(cwdbuf, cwd, (size_t)clen); cwdbuf[clen] = '\0'; }
    if (exe_spawnable(exebuf) != 0) {
        long long e = -(long long)errno;
        free(exebuf); free(argbuf); free(av); free(cwdbuf);
        return e;
    }
    av[0] = exebuf;
    int ai = 1;
    int in_arg = 0;
    for (long long i = 0; i < alen; i++) {
        if (argbuf[i] != '\0' && !in_arg) { av[ai++] = &argbuf[i]; in_arg = 1; }
        else if (argbuf[i] == '\0') in_arg = 0;
    }
    av[ai] = NULL;

    pid_t pid = fork();
    if (pid == 0) {
        if (cwdbuf) chdir(cwdbuf);
        execvp(exebuf, av);
        _exit(127);
    }
    free(exebuf); free(argbuf); free(av); free(cwdbuf);
    if (pid < 0) return -(long long)errno;
    int st = 0;
    while (waitpid(pid, &st, 0) < 0 && errno == EINTR) {}
    if (WIFEXITED(st)) return (long long)WEXITSTATUS(st);
    if (WIFSIGNALED(st)) return 128 + (long long)WTERMSIG(st);
    return -1;
}

/* The captured variant forks the same way; the only delta is the
 * child redirecting fd 1 or 2 to `cap` before exec. Capture goes to
 * a FILE, not a pipe: no reader-deadlock window, and the caller
 * slurps the artifact exactly like it slurps any other file. */
long long mml_proc_run_cap(const char *exe, long long elen,
                           const char *args, long long alen,
                           const char *cwd, long long clen,
                           const char *cap, long long caplen,
                           long long which) {
    int nargs = 0;
    for (long long i = 0; i < alen; i++)
        if (args[i] != '\0' && (i == 0 || args[i - 1] == '\0')) nargs++;
    char *exebuf = malloc((size_t)elen + 1);
    char *argbuf = malloc((size_t)alen + 1);
    char **av = malloc(sizeof(char *) * (size_t)(nargs + 2));
    char *cwdbuf = clen > 0 ? malloc((size_t)clen + 1) : NULL;
    char *capbuf = caplen > 0 ? malloc((size_t)caplen + 1) : NULL;
    if (!exebuf || !argbuf || !av || (clen > 0 && !cwdbuf)
        || (caplen > 0 && !capbuf)) {
        free(exebuf); free(argbuf); free(av); free(cwdbuf); free(capbuf);
        return -(long long)ENOMEM;
    }
    memcpy(exebuf, exe, (size_t)elen); exebuf[elen] = '\0';
    memcpy(argbuf, args, (size_t)alen); argbuf[alen] = '\0';
    if (cwdbuf) { memcpy(cwdbuf, cwd, (size_t)clen); cwdbuf[clen] = '\0'; }
    if (capbuf) { memcpy(capbuf, cap, (size_t)caplen); capbuf[caplen] = '\0'; }
    if (exe_spawnable(exebuf) != 0) {
        long long e = -(long long)errno;
        free(exebuf); free(argbuf); free(av); free(cwdbuf); free(capbuf);
        return e;
    }
    av[0] = exebuf;
    int ai = 1;
    int in_arg = 0;
    for (long long i = 0; i < alen; i++) {
        if (argbuf[i] != '\0' && !in_arg) { av[ai++] = &argbuf[i]; in_arg = 1; }
        else if (argbuf[i] == '\0') in_arg = 0;
    }
    av[ai] = NULL;

    pid_t pid = fork();
    if (pid == 0) {
        if (cwdbuf) chdir(cwdbuf);
        if (capbuf && which >= 1 && which <= 3) {
            int fd = open(capbuf, O_WRONLY | O_CREAT | O_TRUNC, 0644);
            if (fd >= 0) {
                if (which == 1 || which == 3)
                    dup2(fd, STDOUT_FILENO);
                if (which >= 2)
                    dup2(fd, STDERR_FILENO);
                if (fd != 1 && fd != 2) close(fd);
            }
        }
        execvp(exebuf, av);
        _exit(127);
    }
    free(exebuf); free(argbuf); free(av); free(cwdbuf); free(capbuf);
    if (pid < 0) return -(long long)errno;
    int st = 0;
    while (waitpid(pid, &st, 0) < 0 && errno == EINTR) {}
    if (WIFEXITED(st)) return (long long)WEXITSTATUS(st);
    if (WIFSIGNALED(st)) return 128 + (long long)WTERMSIG(st);
    return -1;
}

/* ---------- process entry ----------
 * `char **argv` becomes the language-level `slice str` view: each
 * {ptr,len} element points straight into the kernel's argv storage —
 * no copy, no heap; the element array is a stack VLA in the entry
 * frame, which outlives the user's `main` exactly like a
 * function-root region. `args[0]` is the program name.
 *
 * `str` promises UTF-8; argv bytes that aren't valid UTF-8 can't
 * become `str` values, so the bridge validates each argument with
 * the same check `str_from_utf8` uses and traps on invalid bytes.
 */

int main(int argc, char **argv) {
    int n = argc > 0 ? argc : 0;
    mml_str_t args[n > 0 ? n : 1];
    for (int i = 0; i < n; i++) {
        args[i].ptr = argv[i];
        args[i].len = (long long)strlen(argv[i]);
        mml_str_validate(argv[i], args[i].len);
    }
    mml_stash_argv(n, args);
    return (int)mml_main_args(args, n);
}

/* ---------- platform ---------- */
/* The C preprocessor knows the build host exactly — the same values
 * as s0's `target_triple()`. */
long long mml_target_triple(const char **out) {
#if defined(__APPLE__) && defined(__aarch64__)
    static const char T[] = "arm64-apple-darwin";
#elif defined(__APPLE__) && defined(__x86_64__)
    static const char T[] = "x86_64-apple-darwin";
#elif defined(__linux__) && defined(__x86_64__)
    static const char T[] = "x86_64-unknown-linux-gnu";
#elif defined(__linux__) && defined(__aarch64__)
    static const char T[] = "aarch64-unknown-linux-gnu";
#else
    static const char T[] = "unknown-unknown-unknown";
#endif
    *out = T;
    return (long long)(sizeof(T) - 1);
}
