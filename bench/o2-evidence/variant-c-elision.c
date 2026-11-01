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
#include <stdint.h>
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
 * "opaque payload of declared type" contract for a dead channel).
 *
 * CHAN-001: MML_CHAN_SEG env selects the storage layout — unset/0
 * keeps the per-message node; N links N-slot segments, so the queue
 * allocates ~1/N times per message (prodcons gap decomposition
 * attributed ~60% of the C delta to the per-packet node). Reclaim
 * is under the channel mutex only — a segment frees once the
 * consumer drains it AND the writer has moved to a newer segment. */

typedef struct mml_qn {
    struct mml_qn *next;
    /* esz payload bytes follow */
} mml_qn;

typedef struct mml_seg {
    struct mml_seg *next;
    size_t head, tail;
    /* cap * esz payload bytes follow */
} mml_seg;

/* CHAN-001 — documented bound: segments hold at most this many
 * slots; invalid env values are rejected, not clamped. */
#define MML_CHAN_SEG_MAX 4096

struct mml_chan {
    pthread_mutex_t mu;
    pthread_cond_t cv;          /* data-available */
    pthread_cond_t cv_space;    /* CHAN-003 — space-available */
    mml_qn *head, *tail;        /* legacy per-message layout */
    mml_seg *rseg, *wseg;       /* segmented layout */
    /* O1 — fixed-ring layout (bounded channels only): ring != NULL
     * selects it. ring_head/ring_tail index bound slots; n_msgs is
     * the occupancy authority (full ⇔ n_msgs == bound, empty ⇔ 0),
     * so no head==tail ambiguity can occur. */
    char *ring;
    size_t ring_head, ring_tail;
    unsigned long n_cv_waiters, n_space_waiters;
    int closed;
    size_t esz, cap;            /* cap == 0 → legacy */
    size_t bound;               /* CHAN-003 — 0 → unbounded */
    size_t n_msgs;              /* live queued messages (always
                                 * maintained; bound admission) */
    /* CHAN-002 — opt-in stats (MML_CHAN_STATS=1): live queued
     * messages, allocated slots/bytes and their peaks, plus
     * occupancy accumulators for avg depth/slot utilization.
     * All counters mutate under c->mu; zero cost when off.
     * O1 adds lifetime totals — queue allocations/frees and
     * blocking/wakeup events (cond_wait entries / signal calls). */
    int stats;
    unsigned long long st_msgs, st_msgs_peak;
    unsigned long long st_slots, st_slots_peak;
    unsigned long long st_bytes, st_bytes_peak;
    /* CHAN-002 — simultaneous snapshot: msgs/slots live at the
     * instant st_bytes reached its maximum. The *_peak counters
     * alone are non-simultaneous maxima; these give the exact
     * composition of the byte peak. */
    unsigned long long st_bp_msgs, st_bp_slots;
    unsigned long long st_occ_n, st_occ_d;
    unsigned long long st_allocs, st_frees;
    unsigned long long st_sleeps, st_wakes;
    unsigned long long st_sig, st_elided;
    struct mml_chan *st_next;
};

static pthread_mutex_t mml_chan_reg_mu = PTHREAD_MUTEX_INITIALIZER;
static mml_chan *mml_chan_reg = NULL;
static int mml_chan_reg_exit = 0;

static void mml_chan_stats_report(void) {
    pthread_mutex_lock(&mml_chan_reg_mu);
    mml_chan *c = mml_chan_reg;
    int i = 0;
    while (c) {
        pthread_mutex_lock(&c->mu);
        double occ = c->st_occ_d ? (double)c->st_occ_n /
                                 (double)c->st_occ_d : 0.0;
        fprintf(stderr,
                "mml-chan-stats[%d] esz=%zu cap=%zu ring=%d "
                "msgs_peak=%llu "
                "slots_peak=%llu bytes_peak=%llu "
                "at_bytes_peak_msgs=%llu at_bytes_peak_slots=%llu "
                "occ=%.3f allocs=%llu frees=%llu sleeps=%llu "
                "wakes=%llu sig=%llu elided=%llu\n",
                i++, c->esz, c->cap, c->ring != NULL,
                c->st_msgs_peak,
                c->st_slots_peak, c->st_bytes_peak,
                c->st_bp_msgs, c->st_bp_slots, occ,
                c->st_allocs, c->st_frees,
                c->st_sleeps, c->st_wakes,
                c->st_sig, c->st_elided);
        pthread_mutex_unlock(&c->mu);
        c = c->st_next;
    }
    pthread_mutex_unlock(&mml_chan_reg_mu);
}

static void mml_chan_stat_op(mml_chan *c) {
    c->st_occ_n += c->st_msgs;
    c->st_occ_d += c->st_slots;
}

static size_t mml_chan_seg_cap(void) {
    const char *s = getenv("MML_CHAN_SEG");
    if (!s || !*s) return 0;
    char *end;
    errno = 0;
    long v = strtol(s, &end, 10);
    if (end == s || *end != '\0' || errno == ERANGE ||
        v < 0 || v > MML_CHAN_SEG_MAX) {
        fprintf(stderr, "mml: MML_CHAN_SEG must be 0..%d\n",
                MML_CHAN_SEG_MAX);
        abort();
    }
    return (size_t)v;
}

/* O1 — fixed-ring byte budget for bounded channels. A bounded
 * channel semantically reserves its capacity anyway; committing
 * bound*esz bytes up front is honest below this limit. Larger
 * requests fall back to the segmented/legacy layouts, which
 * allocate lazily. MML_CHAN_RING overrides the budget in bytes
 * (0 disables the ring entirely — the A/B measurement knob);
 * invalid values are rejected, not clamped. */
#define MML_CHAN_RING_MAX_DFLT (16u * 1024u * 1024u)

static size_t mml_chan_ring_max(void) {
    const char *s = getenv("MML_CHAN_RING");
    if (!s || !*s) return MML_CHAN_RING_MAX_DFLT;
    char *end;
    errno = 0;
    long v = strtol(s, &end, 10);
    if (end == s || *end != '\0' || errno == ERANGE || v < 0) {
        fprintf(stderr, "mml: MML_CHAN_RING must be >= 0\n");
        abort();
    }
    return (size_t)v;
}

static mml_seg *mml_seg_new(size_t esz, size_t cap) {
    if (esz != 0 && cap > (SIZE_MAX - sizeof(mml_seg)) / esz) {
        fprintf(stderr, "mml: channel segment size overflow\n");
        abort();
    }
    mml_seg *s = (mml_seg *)malloc(sizeof(mml_seg) + cap * esz);
    if (!s) {
        fprintf(stderr, "mml: channel segment alloc failed\n");
        abort();
    }
    s->next = NULL;
    s->head = s->tail = 0;
    return s;
}

/* CHAN-003 — block while the channel is full and open; returns
 * with the lock held. The legacy void send keeps the old
 * close-wake-enqueue contract (it cannot report failure); the
 * status-returning bounded send uses this and then checks
 * c->closed so that closed channels admit no further enqueues. */
static void mml_chan_wait_space(mml_chan *c) {
    if (!c->bound) return;
    while (c->n_msgs >= c->bound && !c->closed) {
        if (c->stats) c->st_sleeps++;
        c->n_space_waiters++;
        pthread_cond_wait(&c->cv_space, &c->mu);
        c->n_space_waiters--;
    }
}

static mml_chan *mml_chan_alloc(size_t esz) {
    mml_chan *c = (mml_chan *)malloc(sizeof(mml_chan));
    if (!c) {
        fprintf(stderr, "mml: channel alloc failed\n");
        abort();
    }
    pthread_mutex_init(&c->mu, NULL);
    pthread_cond_init(&c->cv, NULL);
    pthread_cond_init(&c->cv_space, NULL);
    c->head = c->tail = NULL;
    c->rseg = c->wseg = NULL;
    c->ring = NULL;
    c->ring_head = c->ring_tail = 0;
    c->n_cv_waiters = c->n_space_waiters = 0;
    c->closed = 0;
    c->esz = esz;
    c->bound = 0;
    c->n_msgs = 0;
    c->cap = mml_chan_seg_cap();
    const char *st = getenv("MML_CHAN_STATS");
    c->stats = (st && st[0] == '1' && st[1] == '\0');
    c->st_msgs = c->st_msgs_peak = 0;
    c->st_slots = c->st_slots_peak = 0;
    c->st_bytes = c->st_bytes_peak = 0;
    c->st_bp_msgs = c->st_bp_slots = 0;
    c->st_occ_n = c->st_occ_d = 0;
    c->st_allocs = c->st_frees = 0;
    c->st_sleeps = c->st_wakes = 0;
    c->st_sig = c->st_elided = 0;
    c->st_next = NULL;
    if (c->stats) {
        pthread_mutex_lock(&mml_chan_reg_mu);
        c->st_next = mml_chan_reg;
        mml_chan_reg = c;
        if (!mml_chan_reg_exit) {
            mml_chan_reg_exit = 1;
            atexit(mml_chan_stats_report);
        }
        pthread_mutex_unlock(&mml_chan_reg_mu);
    }
    return c;
}

mml_chan *mml_chan_new(size_t esz) { return mml_chan_alloc(esz); }

/* CHAN-003 — bounded channel (bench/chan3/SPEC.md). bound >= 1;
 * send blocks at depth == bound until a recv frees a slot or the
 * channel closes. O1 — when bound*esz fits the ring budget
 * (MML_CHAN_RING, default 16 MiB), storage is a single
 * preallocated ring of bound slots: zero allocation per message.
 * Larger requests fall back to the segmented/legacy layouts,
 * which allocate lazily. */
mml_chan *mml_chan_new_bounded(size_t esz, size_t bound) {
    if (bound < 1) {
        fprintf(stderr, "mml: channel bound must be >= 1\n");
        abort();
    }
    mml_chan *c = mml_chan_alloc(esz);
    c->bound = bound;
    size_t bytes = (esz && bound > SIZE_MAX / esz)
                   ? SIZE_MAX : bound * esz;
    if (bytes <= mml_chan_ring_max()) {
        /* esz == 0 → bytes == 0 → still a distinct live pointer;
         * memcpy(dst, src, 0) is defined and n_msgs is the
         * occupancy authority, so colliding slots are harmless. */
        c->ring = (char *)malloc(bytes ? bytes : 1);
        if (!c->ring) {
            fprintf(stderr, "mml: channel ring alloc failed\n");
            abort();
        }
        if (c->stats) {
            c->st_allocs++;
            c->st_slots = c->st_slots_peak = bound;
            c->st_bytes = c->st_bytes_peak = bytes;
            c->st_bp_msgs = 0;
            c->st_bp_slots = bound;
        }
    }
    return c;
}

void mml_chan_send(mml_chan *c, const void *src) {
    /* CHAN-003 — the void contract cannot report closure, so it
     * is invalid on bounded channels (it would bypass
     * depth <= bound and no-enqueue-after-close). Rejected
     * loudly here; the eventual checker rules this out
     * statically. */
    if (c->bound) {
        fprintf(stderr,
                "mml: bounded channel requires "
                "mml_chan_send_bounded\n");
        abort();
    }
    if (c->cap) {
        pthread_mutex_lock(&c->mu);
        mml_chan_wait_space(c);
        if (!c->wseg || c->wseg->tail == c->cap) {
            mml_seg *s = mml_seg_new(c->esz, c->cap);
            if (c->stats) {
                c->st_allocs++;
                c->st_slots += c->cap;
                c->st_bytes += sizeof(mml_seg) + c->cap * c->esz;
                if (c->st_slots > c->st_slots_peak)
                    c->st_slots_peak = c->st_slots;
                if (c->st_bytes > c->st_bytes_peak) {
                    /* snapshot lags by this in-flight msg;
                     * corrected after st_msgs++ below */
                    c->st_bytes_peak = c->st_bytes;
                    c->st_bp_msgs = c->st_msgs + 1;
                    c->st_bp_slots = c->st_slots;
                }
            }
            if (c->wseg) c->wseg->next = s; else c->rseg = s;
            c->wseg = s;
        }
        memcpy((char *)(c->wseg + 1) + c->wseg->tail * c->esz,
               src, c->esz);
        c->wseg->tail++;
        c->n_msgs++;
        if (c->stats) {
            c->st_msgs++;
            if (c->st_msgs > c->st_msgs_peak)
                c->st_msgs_peak = c->st_msgs;
            mml_chan_stat_op(c);
        }
        if (c->stats) {c->st_wakes++; if(!c->n_cv_waiters)c->st_elided++; else c->st_sig++;}
        if (c->n_cv_waiters) pthread_cond_signal(&c->cv);
        pthread_mutex_unlock(&c->mu);
        return;
    }
    mml_qn *n = (mml_qn *)malloc(sizeof(mml_qn) + c->esz);
    if (!n) {
        fprintf(stderr, "mml: channel node alloc failed\n");
        abort();
    }
    n->next = NULL;
    memcpy(n + 1, src, c->esz);
    pthread_mutex_lock(&c->mu);
    mml_chan_wait_space(c);
    if (c->tail) c->tail->next = n; else c->head = n;
    c->tail = n;
    c->n_msgs++;
    if (c->stats) {
        c->st_allocs++;
        c->st_msgs++;
        c->st_slots++;
        c->st_bytes += sizeof(mml_qn) + c->esz;
        if (c->st_msgs > c->st_msgs_peak) c->st_msgs_peak = c->st_msgs;
        if (c->st_slots > c->st_slots_peak) c->st_slots_peak = c->st_slots;
        if (c->st_bytes > c->st_bytes_peak) {
            c->st_bytes_peak = c->st_bytes;
            c->st_bp_msgs = c->st_msgs;
            c->st_bp_slots = c->st_slots;
        }
        mml_chan_stat_op(c);
    }
    if (c->stats) c->st_wakes++;
    pthread_cond_signal(&c->cv);
    pthread_mutex_unlock(&c->mu);
}

/* CHAN-003 — status-returning bounded send (bench/chan3/SPEC.md).
 * MML_CHAN_SENT: enqueued, ownership moved to the channel.
 * MML_CHAN_CLOSED: the channel was closed before admission; the
 * message was NOT enqueued and ownership stays with the caller.
 * Guarantees 0 <= depth <= bound at all times and no successful
 * enqueue after close. */
int mml_chan_send_bounded(mml_chan *c, const void *src) {
    if (c->ring) {
        int rc;
        pthread_mutex_lock(&c->mu);
        mml_chan_wait_space(c);
        if (c->closed) { rc = MML_CHAN_CLOSED; goto ring_out; }
        memcpy(c->ring + c->ring_tail * c->esz, src, c->esz);
        c->ring_tail = (c->ring_tail + 1 == c->bound)
                       ? 0 : c->ring_tail + 1;
        c->n_msgs++;
        if (c->stats) {
            c->st_msgs++;
            if (c->st_msgs > c->st_msgs_peak)
                c->st_msgs_peak = c->st_msgs;
            mml_chan_stat_op(c);
        }
        rc = MML_CHAN_SENT;
ring_out:
        if (c->stats) c->st_wakes++;
        pthread_cond_signal(&c->cv);
        pthread_mutex_unlock(&c->mu);
        return rc;
    }
    if (c->cap) {
        int rc;
        pthread_mutex_lock(&c->mu);
        mml_chan_wait_space(c);
        if (c->closed) { rc = MML_CHAN_CLOSED; goto out; }
        if (!c->wseg || c->wseg->tail == c->cap) {
            mml_seg *s = mml_seg_new(c->esz, c->cap);
            if (c->stats) {
                c->st_allocs++;
                c->st_slots += c->cap;
                c->st_bytes += sizeof(mml_seg) + c->cap * c->esz;
                if (c->st_slots > c->st_slots_peak)
                    c->st_slots_peak = c->st_slots;
                if (c->st_bytes > c->st_bytes_peak) {
                    c->st_bytes_peak = c->st_bytes;
                    c->st_bp_msgs = c->st_msgs + 1;
                    c->st_bp_slots = c->st_slots;
                }
            }
            if (c->wseg) c->wseg->next = s; else c->rseg = s;
            c->wseg = s;
        }
        memcpy((char *)(c->wseg + 1) + c->wseg->tail * c->esz,
               src, c->esz);
        c->wseg->tail++;
        c->n_msgs++;
        if (c->stats) {
            c->st_msgs++;
            if (c->st_msgs > c->st_msgs_peak)
                c->st_msgs_peak = c->st_msgs;
            mml_chan_stat_op(c);
        }
        rc = MML_CHAN_SENT;
out:
        if (c->stats) c->st_wakes++;
        pthread_cond_signal(&c->cv);
        pthread_mutex_unlock(&c->mu);
        return rc;
    }
    mml_qn *n = (mml_qn *)malloc(sizeof(mml_qn) + c->esz);
    if (!n) {
        fprintf(stderr, "mml: channel node alloc failed\n");
        abort();
    }
    n->next = NULL;
    memcpy(n + 1, src, c->esz);
    int rc;
    pthread_mutex_lock(&c->mu);
    mml_chan_wait_space(c);
    if (c->closed) {
        /* rejected node: count its alloc+free pair */
        if (c->stats) { c->st_allocs++; c->st_frees++; }
        pthread_mutex_unlock(&c->mu);
        free(n);
        return MML_CHAN_CLOSED;
    }
    if (c->tail) c->tail->next = n; else c->head = n;
    c->tail = n;
    c->n_msgs++;
    if (c->stats) {
        c->st_allocs++;
        c->st_msgs++;
        c->st_slots++;
        c->st_bytes += sizeof(mml_qn) + c->esz;
        if (c->st_msgs > c->st_msgs_peak) c->st_msgs_peak = c->st_msgs;
        if (c->st_slots > c->st_slots_peak) c->st_slots_peak = c->st_slots;
        if (c->st_bytes > c->st_bytes_peak) {
            c->st_bytes_peak = c->st_bytes;
            c->st_bp_msgs = c->st_msgs;
            c->st_bp_slots = c->st_slots;
        }
        mml_chan_stat_op(c);
    }
    rc = MML_CHAN_SENT;
    if (c->stats) c->st_wakes++;
    pthread_cond_signal(&c->cv);
    pthread_mutex_unlock(&c->mu);
    return rc;
}

/* Shared blocking dequeue: waits for a message or closure, then
 * either copies the head message into dst (returns MML_CHAN_SENT)
 * or — closed and drained — returns MML_CHAN_CLOSED with dst
 * untouched. Identical semantics for legacy and segmented
 * layouts; works on bounded and unbounded channels. */
static int mml_chan_deq(mml_chan *c, void *dst) {
    int rc = MML_CHAN_CLOSED;
    if (c->ring) {
        pthread_mutex_lock(&c->mu);
        while (c->n_msgs == 0 && !c->closed) {
            if (c->stats) c->st_sleeps++;
            c->n_cv_waiters++;
            pthread_cond_wait(&c->cv, &c->mu);
            c->n_cv_waiters--;
        }
        if (c->n_msgs) {
            memcpy(dst, c->ring + c->ring_head * c->esz, c->esz);
            c->ring_head = (c->ring_head + 1 == c->bound)
                           ? 0 : c->ring_head + 1;
            c->n_msgs--;
            /* bound >= 1 whenever ring exists */
            if (c->stats) {c->st_wakes++; if(!c->n_space_waiters)c->st_elided++; else c->st_sig++;}
            if (c->n_space_waiters) pthread_cond_signal(&c->cv_space);
            if (c->stats) { c->st_msgs--; mml_chan_stat_op(c); }
            rc = MML_CHAN_SENT;
        }
        pthread_mutex_unlock(&c->mu);
        return rc;
    }
    if (c->cap) {
        mml_seg *garbage = NULL;
        pthread_mutex_lock(&c->mu);
        for (;;) {
            while (c->rseg && c->rseg != c->wseg &&
                   c->rseg->head == c->cap) {
                mml_seg *old = c->rseg;
                c->rseg = old->next;
                old->next = garbage;
                garbage = old;
                if (c->stats) {
                    c->st_frees++;
                    c->st_slots -= c->cap;
                    c->st_bytes -= sizeof(mml_seg) + c->cap * c->esz;
                }
            }
            if (c->rseg && c->rseg->head < c->rseg->tail) break;
            if (c->closed) break;
            if (c->stats) c->st_sleeps++;
            pthread_cond_wait(&c->cv, &c->mu);
        }
        if (c->rseg && c->rseg->head < c->rseg->tail) {
            memcpy(dst, (char *)(c->rseg + 1) +
                   c->rseg->head * c->esz, c->esz);
            c->rseg->head++;
            c->n_msgs--;
            if (c->bound) {
                if (c->stats) c->st_wakes++;
                pthread_cond_signal(&c->cv_space);
            }
            if (c->stats) { c->st_msgs--; mml_chan_stat_op(c); }
            rc = MML_CHAN_SENT;
        }
        pthread_mutex_unlock(&c->mu);
        while (garbage) {
            mml_seg *n = garbage->next;
            free(garbage);
            garbage = n;
        }
        return rc;
    }
    pthread_mutex_lock(&c->mu);
    while (!c->head && !c->closed) {
        if (c->stats) c->st_sleeps++;
        pthread_cond_wait(&c->cv, &c->mu);
    }
    mml_qn *n = c->head;
    if (n) {
        c->head = n->next;
        if (!c->head) c->tail = NULL;
        memcpy(dst, n + 1, c->esz);
        c->n_msgs--;
        if (c->bound) {
            if (c->stats) {c->st_wakes++; if(!c->n_space_waiters)c->st_elided++; else c->st_sig++;}
            if (c->n_space_waiters) pthread_cond_signal(&c->cv_space);
        }
        if (c->stats) {
            c->st_frees++;
            c->st_msgs--;
            c->st_slots--;
            c->st_bytes -= sizeof(mml_qn) + c->esz;
            mml_chan_stat_op(c);
        }
        rc = MML_CHAN_SENT;
    }
    pthread_mutex_unlock(&c->mu);
    free(n);
    return rc;
}

void mml_chan_recv(mml_chan *c, void *dst) {
    if (mml_chan_deq(c, dst) == MML_CHAN_CLOSED) {
        memset(dst, 0, c->esz);
    }
}

/* CHAN-004 — status-returning recv: MML_CHAN_SENT = a message was
 * delivered into dst; MML_CHAN_CLOSED = the channel is closed and
 * drained, dst untouched. Lets the language distinguish a real
 * zero-valued message from end-of-stream. */
int mml_chan_recv_bounded(mml_chan *c, void *dst) {
    return mml_chan_deq(c, dst);
}

void mml_chan_close(mml_chan *c) {
    pthread_mutex_lock(&c->mu);
    c->closed = 1;
    if (c->stats) c->st_wakes += 2;
    pthread_cond_broadcast(&c->cv);
    pthread_cond_broadcast(&c->cv_space);
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
