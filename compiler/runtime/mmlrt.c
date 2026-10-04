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
