/* CHAN-003 invariant harness — exercises mml_chan_new_bounded +
 * mml_chan_send_bounded against mmlrt.h on both platforms (spec:
 * bench/chan3/SPEC.md). Uses only the mml_task/mml_sleep surface
 * so the same binary validates POSIX and Win32. Exit 0 = all
 * invariants held; nonzero = failures. */

#include "mmlrt.h"
#include <stdio.h>
#include <string.h>
#include <stdlib.h>
#include <stdatomic.h>

static int fails = 0;
#define CHECK(cond, tag) do { \
    if (!(cond)) { fprintf(stderr, "FAIL %s: %s\n", tag, #cond); \
                   fails++; return; } } while (0)
#define CHECKF(cond, tag) do { \
    if (!(cond)) { fprintf(stderr, "FAIL %s: %s\n", tag, #cond); \
                   fails++; return NULL; } } while (0)

/* the runtime owns main() and calls here after its own init —
 * the harness runs as the generated-code entry point. */
long long mml_main_args(const mml_str_t *a, long long n);

typedef struct { mml_chan *ch; long long n; int us; } prod_t;
typedef struct { mml_chan *ch; long long n; int us;
                 long long got, order_ok; } cons_t;

static void *produce(void *env) {
    prod_t *p = env;
    for (long long i = 0; i < p->n; i++) {
        long long v = i + 1;
        int rc = mml_chan_send_bounded(p->ch, &v);
        if (rc != MML_CHAN_SENT) return NULL;   /* unexpected */
        if (p->us) mml_sleep_us(p->us);
    }
    return NULL;
}

static void *consume(void *env) {
    cons_t *c = env;
    long long last = 0;
    c->order_ok = 1;
    for (c->got = 0; c->got < c->n; c->got++) {
        long long v = 0;
        mml_chan_recv(c->ch, &v);
        if (v <= last) c->order_ok = 0;
        last = v;
        if (c->us) mml_sleep_us(c->us);
    }
    return NULL;
}

/* I1+I2 — FIFO order at bound=7 via send_bounded; depth never
 * exceeds bound (MML_CHAN_STATS=1 cross-checks msgs_peak). */
static void t_fifo_bound(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 7);
    prod_t p = { ch, 500, 0 };
    cons_t c = { ch, 500, 40, 0, 1 };
    mml_task *t = mml_task_spawn(produce, &p);
    consume(&c);
    mml_task_join(t);
    CHECK(c.got == 500 && c.order_ok, "I1");
    mml_chan_close(ch);
}

typedef struct { mml_chan *ch; _Atomic int rc; } blk_t;
static void *blocked_send(void *env) {
    blk_t *b = env;
    long long v = 99;
    atomic_store(&b->rc, mml_chan_send_bounded(b->ch, &v));
    return NULL;
}

/* I3 — a send_bounded blocked on a full channel wakes on close,
 * reports CLOSED, and does NOT enqueue: after close+drain, no
 * late message can appear. */
static void t_close_no_late_enqueue(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 1);
    long long v = 1;
    CHECK(mml_chan_send_bounded(ch, &v) == MML_CHAN_SENT, "I3");
    blk_t b = { ch, -1 };
    mml_task *t = mml_task_spawn(blocked_send, &b);
    mml_sleep_us(20000);                     /* let it block */
    CHECK(atomic_load(&b.rc) == -1, "I3-prime");
    mml_chan_close(ch);
    mml_task_join(t);
    CHECK(atomic_load(&b.rc) == MML_CHAN_CLOSED, "I3");
    long long got = -1;
    mml_chan_recv(ch, &got); CHECK(got == 1, "I3");
    got = -1;
    mml_chan_recv(ch, &got);
    CHECK(got == 0, "I3");                   /* closed+empty=zeros,
                                                no late enqueue */
}

/* I7 — deterministic close race: the blocked sender loses the
 * admission race regardless of when the receiver drains, because
 * `closed` is checked first after the wait. */
static void t_close_race(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 1);
    long long v = 7;
    CHECK(mml_chan_send_bounded(ch, &v) == MML_CHAN_SENT, "I7");
    blk_t b = { ch, -1 };
    mml_task *snd = mml_task_spawn(blocked_send, &b);
    mml_sleep_us(20000);                     /* sender blocked */
    /* receiver drains the first message (frees the slot) while
     * close races in — order of the two is deliberately
     * interleaved: close first, then drain. */
    mml_chan_close(ch);
    long long got = -1;
    mml_chan_recv(ch, &got); CHECK(got == 7, "I7");
    mml_task_join(snd);
    CHECK(atomic_load(&b.rc) == MML_CHAN_CLOSED, "I7");
    got = -1;
    mml_chan_recv(ch, &got); CHECK(got == 0, "I7");
}

/* I8 — ownership preservation: MML_CHAN_CLOSED transfers
 * nothing; the caller's value remains intact and usable. */
static void t_ownership_preserved(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 1);
    mml_chan_close(ch);
    long long v = 42;
    CHECK(mml_chan_send_bounded(ch, &v) == MML_CHAN_CLOSED, "I8");
    CHECK(v == 42, "I8");                    /* still ours */
    long long got = -1;
    mml_chan_recv(ch, &got); CHECK(got == 0, "I8");
}

/* I4 — close wakes a receiver blocked on an empty queue. */
static void *blocked_recv(void *env) {
    blk_t *b = env;
    long long v = 7;
    mml_chan_recv(b->ch, &v);
    atomic_store(&b->rc, v == 0 ? 1 : 0);
    return NULL;
}
static void t_close_wakes_receiver(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 4);
    blk_t b = { ch, -1 };
    mml_task *t = mml_task_spawn(blocked_recv, &b);
    mml_sleep_us(20000);
    mml_chan_close(ch);
    mml_task_join(t);
    CHECK(atomic_load(&b.rc) == 1, "I4");
}

/* I5 — two producers share bound=8; all messages arrive. */
static void t_two_producers(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 8);
    prod_t a = { ch, 250, 0 }, bp = { ch, 250, 0 };
    cons_t c = { ch, 500, 0, 0, 1 };
    mml_task *t1 = mml_task_spawn(produce, &a);
    mml_task *t2 = mml_task_spawn(produce, &bp);
    for (c.got = 0; c.got < 500; c.got++) {
        long long v; mml_chan_recv(ch, &v);
        if (v <= 0 || v > 250) c.order_ok = 0;
    }
    mml_task_join(t1); mml_task_join(t2);
    CHECK(c.got == 500 && c.order_ok, "I5");
    mml_chan_close(ch);
}

/* I6 — refill: drain below bound unblocks senders; no deadlock
 * in the wake path. */
static void t_refill(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 4);
    prod_t p = { ch, 300, 0 };
    cons_t c = { ch, 300, 60, 0, 1 };
    mml_task *t = mml_task_spawn(produce, &p);
    consume(&c);
    mml_task_join(t);
    CHECK(c.got == 300 && c.order_ok, "I6");
    mml_chan_close(ch);
}

long long mml_main_args(const mml_str_t *a, long long n) {
    (void)a; (void)n;
    printf("bound_test: MML_CHAN_SEG=%s\n",
           getenv("MML_CHAN_SEG") ? getenv("MML_CHAN_SEG") : "unset");
    t_fifo_bound();
    t_close_no_late_enqueue();
    t_close_race();
    t_ownership_preserved();
    t_close_wakes_receiver();
    t_two_producers();
    t_refill();
    if (fails) { printf("RESULT: %d FAIL\n", fails); return 1; }
    printf("RESULT: PASS\n");
    return 0;
}
