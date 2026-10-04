/* mmlrt — the language's tiny runtime boundary (N13).
 *
 * The compiler lowers every concurrency primitive into these calls;
 * the semantic model stays identical whether the backend is sequential
 * or parallel — only this file knows about pthreads.
 *
 *   spawn { ... }   -> mml_task_spawn(fn, env) / mml_task_join(t)
 *   mutex T         -> mml_mutex_new / _lock / _unlock_data
 *   channel T       -> mml_chan_new / _send / _recv / _close
 */
#include <pthread.h>
#include <stdlib.h>
#include <string.h>
#include <fcntl.h>
#include <errno.h>
#include <unistd.h>
#include <sys/socket.h>

/* ---------- non-variadic shims (N24C) ----------
 *
 * open(2) is variadic; declaring it as a fixed-arity extern passes
 * the trailing `mode` argument incorrectly on some ABIs (it landed
 * as 0 -> mode-0000 files). Until the language grows variadic FFI
 * (default promotions + varargs calling convention), cross the
 * boundary through a fixed-arity shim that makes the C-side call.
 */

int mml_open_mode(const char *p, int flags, int mode) {
    return open(p, flags, mode);
}

/* N26B — name-shadowing shims: std.net exports `listen`/`accept`
 * wrappers under the qualified names `net.listen`/`net.accept`, so
 * inside that module the bare libc spellings collide. The boundary
 * names carry the mml_ prefix, same trick as mml_open_mode. */
int mml_listen(int fd, int backlog) {
    return listen(fd, backlog);
}

int mml_accept(int fd) {
    return accept(fd, NULL, NULL);
}

int mml_reuse_addr(int fd) {
    int one = 1;
    return setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
}

/* N30 — Result-shaped stdlib APIs need real error values, not the
 * raw `-1`. `errno` is thread-local, so reading it immediately after
 * a failed syscall is exact; wrappers do `Err(mml_errno())`. */
int mml_errno(void) {
    return errno;
}

/* N30 — more boundary renames: the Result layer takes the clean
 * names `fs.read`/`fs.write`/`fs.close` and `net.socket`/`net.bind`/
 * `net.connect`, so the extern spellings inside those modules move
 * behind mml_ (the same shadowing rule as mml_listen). */
ssize_t mml_read(int fd, void *buf, size_t n) {
    return read(fd, buf, n);
}

ssize_t mml_write(int fd, const void *buf, size_t n) {
    return write(fd, buf, n);
}

int mml_close(int fd) {
    return close(fd);
}

int mml_socket(int domain, int ty, int proto) {
    return socket(domain, ty, proto);
}

int mml_bind(int fd, const void *addr, unsigned len) {
    return bind(fd, (const struct sockaddr *)addr, len);
}

int mml_connect(int fd, const void *addr, unsigned len) {
    return connect(fd, (const struct sockaddr *)addr, len);
}

/* ---------- tasks ---------- */

typedef void *(*mml_task_fn)(void *env);

typedef struct {
    pthread_t th;
    mml_task_fn fn;
    void *env;
} mml_task;

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
 * `mutex h` wraps existing storage (the domain protects the referent,
 * not a copy). `unlock` takes the guard (the object pointer). */

typedef struct {
    pthread_mutex_t mu;
    void *data;
    char payload[];
} mml_mutex;

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

typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t cv;
    mml_qn *head, *tail;
    int closed;
    size_t esz;
} mml_chan;

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

/* ---------- slices (N15) ---------- */

#include <stdio.h>

void mml_bounds_check(long long idx, long long len) {
    if (idx < 0 || idx >= len) {
        fprintf(stderr, "mml: index %lld out of bounds (len %lld)\n", idx, len);
        abort();
    }
}

/* N25A — shift amount must be in [0, width); no C-style masking. */
void mml_shift_check(long long amt, long long width) {
    if (amt < 0 || amt >= width) {
        fprintf(stderr, "mml: shift amount %lld out of range (0..%lld)\n",
                amt, width);
        abort();
    }
}

void mml_range_check(long long lo, long long hi, long long limit) {
    if (lo < 0 || lo > hi || hi > limit) {
        fprintf(stderr, "mml: range [%lld..%lld) out of bounds (len %lld)\n",
                lo, hi, limit);
        abort();
    }
}

/* ---------- strings (N15B) ---------- */

/* Value equality over the view's bytes: length first, then memcmp.
 * O(n) by design — str identity comparison is not a thing. */
int mml_str_eq(const void *a, long long alen, const void *b, long long blen) {
    if (alen != blen) return 0;
    return memcmp(a, b, (size_t)alen) == 0;
}

/* The `str_from_utf8` gate: reject (trap, like a bounds check) any
 * byte sequence that is not well-formed UTF-8 — including overlong
 * encodings, surrogates, stray continuations, and truncation.
 * Table-free walk: for each lead byte, check the payload range and
 * that the continuation bytes are 0x80..0xBF. */
void mml_str_validate(const void *p, long long len) {
    const unsigned char *s = (const unsigned char *)p;
    long long i = 0;
    while (i < len) {
        unsigned char c = s[i];
        int cont;
        unsigned char lo = 0x80, hi = 0xBF; /* first-continuation range */
        if (c < 0x80) { i++; continue; }
        else if (c >= 0xC2 && c <= 0xDF) cont = 1;
        else if (c == 0xE0) { cont = 2; lo = 0xA0; }      /* no overlong */
        else if (c >= 0xE1 && c <= 0xEC) cont = 2;
        else if (c == 0xED) { cont = 2; hi = 0x9F; }      /* no surrogates */
        else if (c >= 0xEE && c <= 0xEF) cont = 2;
        else if (c == 0xF0) { cont = 3; lo = 0x90; }      /* no overlong */
        else if (c >= 0xF1 && c <= 0xF3) cont = 3;
        else if (c == 0xF4) { cont = 3; hi = 0x8F; }      /* <= U+10FFFF */
        else goto bad; /* 0x80..0xC1, 0xF5..0xFF — never valid leads */
        if (i + cont >= len) goto bad;                  /* truncated */
        if (s[i + 1] < lo || s[i + 1] > hi) goto bad;
        for (int k = 2; k <= cont; k++) {
            if (s[i + k] < 0x80 || s[i + k] > 0xBF) goto bad;
        }
        i += cont + 1;
    }
    return;
bad:
    fprintf(stderr, "mml: invalid UTF-8 in `str_from_utf8` (byte %lld of %lld)\n",
            i, len);
    abort();
}

/* Non-trapping UTF-8 scan for callers that want a status, not an
 * abort — same table-free walk as mml_str_validate. */
int mml_str_check(const void *p, long long len) {
    const unsigned char *s = (const unsigned char *)p;
    long long i = 0;
    while (i < len) {
        unsigned char c = s[i];
        int cont;
        unsigned char lo = 0x80, hi = 0xBF;
        if (c < 0x80) { i++; continue; }
        else if (c >= 0xC2 && c <= 0xDF) cont = 1;
        else if (c == 0xE0) { cont = 2; lo = 0xA0; }
        else if (c >= 0xE1 && c <= 0xEC) cont = 2;
        else if (c == 0xED) { cont = 2; hi = 0x9F; }
        else if (c >= 0xEE && c <= 0xEF) cont = 2;
        else if (c == 0xF0) { cont = 3; lo = 0x90; }
        else if (c >= 0xF1 && c <= 0xF3) cont = 3;
        else if (c == 0xF4) { cont = 3; hi = 0x8F; }
        else return 0;
        if (i + cont >= len) return 0;
        if (s[i + 1] < lo || s[i + 1] > hi) return 0;
        for (int k = 2; k <= cont; k++) {
            if (s[i + k] < 0x80 || s[i + k] > 0xBF) return 0;
        }
        i += cont + 1;
    }
    return 1;
}

/* ---------- process state (N32) ----------
 *
 * Thin shims over the C process environment. Values come back as
 * borrowed views into process-global storage; the language exposes
 * no environment mutation, so those views stay valid for the
 * process lifetime. */
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
void mml_exit(int code) { _exit(code); }

/* argv: `main` below builds the str-shaped array on its own frame
 * (which outlives every other frame) and stashes it here; served
 * back as a `slice str` — the same {ptr,len} layout. */
static long long g_argc = 0;
static const void *g_argv_arr = 0;
void mml_stash_argv(long long argc, const void *arr) {
    g_argc = argc;
    g_argv_arr = arr;
}
const void *mml_argv(void) { return g_argv_arr; }
long long mml_argc(void) { return g_argc; }
/* i-th argv as a borrowed view — same layout twin as mml_str_t
 * (declared below); out-params keep `str` itself out of the FFI
 * surface. */
int mml_arg(long long i, const char **out_ptr, long long *out_len) {
    struct strkv { const char *p; long long n; };
    const struct strkv *arr = (const struct strkv *)g_argv_arr;
    if (i < 0 || i >= g_argc) return 1;
    *out_ptr = arr[i].p;
    *out_len = arr[i].n;
    return 0;
}

/* ---------- explicit allocation (N16A) ----------
 *
 * `system_allocator()` is the NULL token — the system heap. The
 * checker guarantees `free` reaches here exactly once per root:
 * these functions are deliberately thin so the LLVM stays legible.
 * NULL-returning malloc is a hard trap (consistent with bounds
 * checks — the language has no error values yet).
 */
void *mml_alloc(long long size) {
    void *p = malloc((size_t)size);
    if (!p) {
        fprintf(stderr, "mml: allocation of %lld bytes failed\n", size);
        abort();
    }
    return p;
}

void *mml_alloc_slice(long long size, long long count) {
    if (count < 0) {
        fprintf(stderr, "mml: alloc_slice count %lld is negative\n", count);
        abort();
    }
    long long bytes = size * count;
    if (count != 0 && bytes / count != size) {
        fprintf(stderr, "mml: alloc_slice size overflow (%lld x %lld)\n",
                size, count);
        abort();
    }
    return mml_alloc(bytes > 0 ? bytes : 1);
}

void mml_free(void *p) {
    free(p);
}

/* ---------- process entry (N26A) ----------
 *
 * The platform `main` lives here. `char **argv` becomes the
 * language-level `slice str` view: each {ptr, len} element points
 * straight into the kernel's argv storage — no copy, no heap; the
 * element array is a stack VLA in the entry frame, which outlives
 * the user's `main` exactly like a function-root region. `args[0]`
 * is the program name, matching the machine convention.
 *
 * `str` promises UTF-8; argv bytes that aren't valid UTF-8 can't
 * become `str` values, so the bridge validates each argument with
 * the same check `str_from_utf8` uses and traps on invalid bytes.
 */

typedef struct { const char *ptr; long long len; } mml_str_t;

extern long long mml_main_args(const mml_str_t *args, long long len);

int main(int argc, char **argv) {
    int n = argc > 0 ? argc : 0;
    mml_str_t args[n > 0 ? n : 1];
    for (int i = 0; i < n; i++) {
        args[i].ptr = argv[i];
        args[i].len = (long long)strlen(argv[i]);
        mml_str_validate(argv[i], args[i].len);
    }
    mml_stash_argv(n, args);   /* N32 — `process.args()` reads this */
    return (int)mml_main_args(args, n);
}
