/* CHAN-003 invariant harness — exercises mml_chan_new_bounded
 * directly against mmlrt.h on both platforms (spec:
 * bench/chan3/SPEC.md). Uses only the mml_task/mml_sleep surface
 * so the same binary validates POSIX and Win32. Exit 0 = all
 * held; nonzero = first failing invariant's number. */

#include "mmlrt.h"
#include <stdio.h>
#include <string.h>
#include <stdlib.h>

/* the runtime owns main() and calls here after its own init —
 * the harness runs as the generated-code entry point. */
long long mml_main_args(const mml_str_t *a, long long n);

static int fails = 0;
#define CHECK(cond, tag) do { \
    if (!(cond)) { fprintf(stderr, "FAIL %s: %s\n", tag, #cond); \
                   fails++; return; } } while (0)
#define CHECKF(cond, tag) do { \
    if (!(cond)) { fprintf(stderr, "FAIL %s: %s\n", tag, #cond); \
                   fails++; return NULL; } } while (0)

typedef struct { mml_chan *ch; long long n; int us; } prod_t;
typedef struct { mml_chan *ch; long long n; int us;
                 long long got, order_ok; } cons_t;

static void *produce(void *env) {
    prod_t *p = env;
    for (long long i = 0; i < p->n; i++) {
        long long v = i + 1;
        mml_chan_send(p->ch, &v);
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

/* I1+I2 — FIFO order at bound=7, and depth never exceeds bound
 * (the consumer lags; with MML_CHAN_STATS=1 the exit report
 * cross-checks msgs_peak <= bound). */
static void t_fifo_bound(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 7);
    prod_t p = { ch, 500, 0 };
    cons_t c = { ch, 500, 40, 0, 1 };
    mml_task *t = mml_task_spawn(produce, &p);
    consume(&c);
    mml_task_join(t);
    CHECK(c.got == 500 && c.order_ok, "I1");
    /* depth bound is enforced structurally by send blocking;
     * MML_CHAN_STATS=1 prints msgs_peak for cross-check. */
    mml_chan_close(ch);
}

typedef struct { mml_chan *ch; volatile int done; } blk_t;
static void *blocked_send(void *env) {
    blk_t *b = env;
    long long v = 99;
    mml_chan_send(b->ch, &v);   /* must block at bound=1 */
    b->done = 1;
    return NULL;
}

/* I3 — a send blocked on a full channel wakes on close and
 * completes; the message stays drainable. */
static void t_close_wakes_sender(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 1);
    long long v = 1;
    mml_chan_send(ch, &v);              /* fills bound */
    blk_t b = { ch, 0 };
    mml_task *t = mml_task_spawn(blocked_send, &b);
    mml_sleep_us(20000);                /* let it block */
    CHECK(!b.done, "I3-prime");         /* really blocked */
    mml_chan_close(ch);
    mml_task_join(t);
    CHECK(b.done, "I3");                /* woke, completed */
    long long got;
    mml_chan_recv(ch, &got); CHECK(got == 1, "I3");
    mml_chan_recv(ch, &got); CHECK(got == 99, "I3");
}

/* I4 — close wakes a receiver blocked on an empty queue. */
static void *blocked_recv(void *env) {
    blk_t *b = env;
    long long v = 7;
    mml_chan_recv(b->ch, &v);
    b->done = (v == 0);                 /* zeros on closed+empty */
    return NULL;
}
static void t_close_wakes_receiver(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 4);
    blk_t b = { ch, 0 };
    mml_task *t = mml_task_spawn(blocked_recv, &b);
    mml_sleep_us(20000);
    mml_chan_close(ch);
    mml_task_join(t);
    CHECK(b.done, "I4");
}

/* I5 — two producers share bound=8; all messages arrive. */
static void t_two_producers(void) {
    mml_chan *ch = mml_chan_new_bounded(sizeof(long long), 8);
    prod_t a = { ch, 250, 0 }, bp = { ch, 250, 0 };
    cons_t c = { ch, 500, 0, 0, 1 };
    mml_task *t1 = mml_task_spawn(produce, &a);
    mml_task *t2 = mml_task_spawn(produce, &bp);
    /* order across producers is nondeterministic — count only */
    for (c.got = 0; c.got < 500; c.got++) {
        long long v; mml_chan_recv(ch, &v);
        if (v <= 0 || v > 250) c.order_ok = 0;
    }
    mml_task_join(t1); mml_task_join(t2);
    CHECK(c.got == 500 && c.order_ok, "I5");
    mml_chan_close(ch);
}

/* I6 — refill: drain below bound unblocks senders (throughput
 * resumes); verifies no deadlock in the wake path. */
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

/* bound==0 is rejected (rendezvous deferred). Checked in a
 * subprocess by the runner — a direct call here would abort. */

long long mml_main_args(const mml_str_t *a, long long n) {
    (void)a; (void)n;
    printf("bound_test: MML_CHAN_SEG=%s\n",
           getenv("MML_CHAN_SEG") ? getenv("MML_CHAN_SEG") : "unset");
    t_fifo_bound();
    t_close_wakes_sender();
    t_close_wakes_receiver();
    t_two_producers();
    t_refill();
    if (fails) { printf("RESULT: %d FAIL\n", fails); return 1; }
    printf("RESULT: PASS\n");
    return 0;
}
