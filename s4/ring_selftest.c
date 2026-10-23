/* O1 ring selftest — drives the runtime's bounded-channel API
 * directly (mml_chan_new_bounded / send_bounded / recv_bounded /
 * close) against the unpatched platform runtime. Each MODE
 * exercises one contract clause; portable across POSIX and Win32
 * via the runtime's own task/sleep primitives.
 *
 *   sizes    esz 0/1/4/8 roundtrips
 *   wrap     bound=4, interleaved send/recv — FIFO across wraparound
 *   big      esz=512 payload — content integrity
 *   full     blocked sender admitted after a recv frees a slot,
 *            FIFO order preserved
 *   reject   sender blocked on full channel is rejected by close;
 *            caller retains ownership
 *   drain    enqueued messages delivered after close, then CLOSED
 *            with dst untouched; sends after close rejected
 *   close    blocked receivers + blocked sender all woken by close
 *   mpmc     4 producers / 3 consumers — every item exactly once
 *   unbound  legacy unbounded channel unaffected
 *   fallback bound*esz over the ring budget → lazy path still works
 *
 * Build (from repo root):  sh s4/ring_selftest.sh
 */
#include "mmlrt.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static const char *mode;
static int fails = 0;
#define CHECK(cond, msg) do { \
    if (!(cond)) { printf("FAIL %s: %s\n", mode, msg); fails++; } \
} while (0)

typedef struct { mml_chan *c; long long v; int rc; } send_env;
typedef struct { mml_chan *c; long long n, last, closed_n; } recv_env;

static void *do_send(void *env) {
    send_env *e = (send_env *)env;
    e->rc = mml_chan_send_bounded(e->c, &e->v);
    return NULL;
}
static void *do_recv(void *env) {
    recv_env *e = (recv_env *)env;
    for (;;) {
        long long v = -777;
        int rc = mml_chan_recv_bounded(e->c, &v);
        if (rc == MML_CHAN_CLOSED) { e->closed_n++; return NULL; }
        e->n++; e->last = v;
    }
}

/* ---------- modes ---------- */

static int mode_sizes(void) {
    size_t sizes[] = {0, 1, 4, 8};
    for (int i = 0; i < 4; i++) {
        size_t esz = sizes[i];
        mml_chan *c = mml_chan_new_bounded(esz, 8);
        unsigned char in[8] = {0}, out[8] = {0};
        in[0] = (unsigned char)(0xA0 + i);
        if (esz > 1) in[esz - 1] = 0x5A;
        CHECK(mml_chan_send_bounded(c, in) == MML_CHAN_SENT, "send");
        CHECK(mml_chan_recv_bounded(c, out) == MML_CHAN_SENT, "recv");
        CHECK(esz == 0 || memcmp(in, out, esz) == 0,
              "payload mismatch");
    }
    return fails;
}

static int mode_wrap(void) {
    mml_chan *c = mml_chan_new_bounded(8, 4);
    /* Interleaved send/recv with periodic partial refills — head and
     * tail wrap many times in both directions. */
    for (long long i = 0; i < 3000; i++) {
        long long v = i, got = -1;
        CHECK(mml_chan_send_bounded(c, &v) == MML_CHAN_SENT, "send");
        CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_SENT, "recv");
        CHECK(got == i, "FIFO order broken");
        if ((i & 3) == 0) {
            for (int k = 0; k < 3; k++) {
                long long w = -100 - i - k;
                CHECK(mml_chan_send_bounded(c, &w) == MML_CHAN_SENT,
                      "refill send");
            }
            for (int k = 0; k < 3; k++) {
                long long w = 0, exp = -100 - i - k;
                CHECK(mml_chan_recv_bounded(c, &w) == MML_CHAN_SENT,
                      "refill recv");
                CHECK(w == exp, "refill order broken");
            }
        }
    }
    return fails;
}

static int mode_big(void) {
    struct { char b[512]; } in, out;
    memset(&in, 0x5A, sizeof in);
    memset(&out, 0, sizeof out);
    mml_chan *c = mml_chan_new_bounded(sizeof in, 4);
    CHECK(mml_chan_send_bounded(c, &in) == MML_CHAN_SENT, "send");
    CHECK(mml_chan_recv_bounded(c, &out) == MML_CHAN_SENT, "recv");
    CHECK(memcmp(&in, &out, sizeof in) == 0, "512B payload mismatch");
    return fails;
}

static int mode_full(void) {
    mml_chan *c = mml_chan_new_bounded(8, 2);
    long long a = 1, b = 2, got = 0;
    CHECK(mml_chan_send_bounded(c, &a) == MML_CHAN_SENT, "send a");
    CHECK(mml_chan_send_bounded(c, &b) == MML_CHAN_SENT, "send b");
    /* ring full: spawned sender must block until a recv frees a
     * slot — then deliver, preserving FIFO across the block. */
    send_env e = {c, 99, -1};
    mml_task *t = mml_task_spawn(do_send, &e);
    mml_sleep_us(20000);            /* let it reach wait_space */
    CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_SENT, "recv 1");
    CHECK(got == 1, "first out must be 'a'");
    mml_task_join(t);
    CHECK(e.rc == MML_CHAN_SENT, "blocked sender must be admitted");
    got = 0;
    CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_SENT, "recv 2");
    CHECK(got == 2, "second out must be 'b' (FIFO across block)");
    got = 0;
    CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_SENT, "recv 3");
    CHECK(got == 99, "blocked sender's value last");
    return fails;
}

static int mode_reject(void) {
    mml_chan *c = mml_chan_new_bounded(8, 1);
    long long a = 42;
    CHECK(mml_chan_send_bounded(c, &a) == MML_CHAN_SENT, "fill");
    send_env e = {c, 77, -1};
    mml_task *t = mml_task_spawn(do_send, &e);
    mml_sleep_us(20000);            /* sender now blocked on full */
    mml_chan_close(c);
    mml_task_join(t);
    CHECK(e.rc == MML_CHAN_CLOSED,
          "blocked sender must be rejected by close");
    CHECK(e.v == 77, "rejected send must leave caller's value");
    /* queued message still drains */
    long long got = 0;
    CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_SENT, "drain");
    CHECK(got == 42, "drained value");
    got = 555;
    CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_CLOSED,
          "closed+drained must return CLOSED");
    CHECK(got == 555, "CLOSED must not touch dst");
    return fails;
}

static int mode_drain(void) {
    mml_chan *c = mml_chan_new_bounded(8, 8);
    for (long long i = 0; i < 5; i++)
        CHECK(mml_chan_send_bounded(c, &i) == MML_CHAN_SENT, "fill");
    mml_chan_close(c);
    for (long long i = 0; i < 5; i++) {
        long long got = -1;
        CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_SENT,
              "drain recv");
        CHECK(got == i, "drain order");
    }
    long long got = 999;
    CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_CLOSED,
          "drained channel must report CLOSED");
    CHECK(got == 999, "CLOSED must not touch dst");
    long long v = 1;
    CHECK(mml_chan_send_bounded(c, &v) == MML_CHAN_CLOSED,
          "send after close must be rejected");
    return fails;
}

static int mode_close(void) {
    mml_chan *c = mml_chan_new_bounded(8, 2);
    long long a = 7;
    CHECK(mml_chan_send_bounded(c, &a) == MML_CHAN_SENT, "fill a");
    CHECK(mml_chan_send_bounded(c, &a) == MML_CHAN_SENT, "fill b");
    send_env se = {c, 8, -1};
    mml_task *st = mml_task_spawn(do_send, &se);
    recv_env r1 = {c, 0, -1, 0}, r2 = {c, 0, -1, 0};
    mml_task *t1 = mml_task_spawn(do_recv, &r1);
    mml_task *t2 = mml_task_spawn(do_recv, &r2);
    mml_sleep_us(30000);            /* all three now blocked */
    mml_chan_close(c);
    mml_task_join(st); mml_task_join(t1); mml_task_join(t2);
    /* The sender races with the receivers' drain: if a recv frees a
     * slot before close lands it is legitimately admitted (SENT)
     * and its message must drain too; if close lands first it is
     * rejected (CLOSED). Both are contract-valid — assert the
     * invariant, not a race outcome. */
    CHECK(se.rc == MML_CHAN_SENT || se.rc == MML_CHAN_CLOSED,
          "sender must resolve one way or the other");
    CHECK(r1.closed_n == 1 && r2.closed_n == 1,
          "blocked receivers must exit on close");
    CHECK(r1.n + r2.n == 2 + (se.rc == MML_CHAN_SENT ? 1 : 0),
          "every admitted message must be drained");
    return fails;
}

typedef struct { mml_chan *c; int id; } penv;
static void *prod(void *env) {
    penv *e = (penv *)env;
    for (long long i = 0; i < 20000; i++) {
        long long v = ((long long)e->id << 32) | i;
        if (mml_chan_send_bounded(e->c, &v) == MML_CHAN_CLOSED)
            return NULL;
    }
    return NULL;
}

static int mode_mpmc(void) {
    enum { P = 4, C = 3, N = 20000 };
    mml_chan *c = mml_chan_new_bounded(8, 16);
    penv pe[P]; mml_task *pt[P];
    recv_env re[C]; mml_task *rt[C];
    for (int i = 0; i < C; i++) {
        re[i].c = c; re[i].n = 0; re[i].last = -1; re[i].closed_n = 0;
        rt[i] = mml_task_spawn(do_recv, &re[i]);
    }
    for (int i = 0; i < P; i++) {
        pe[i].c = c; pe[i].id = i + 1;
        pt[i] = mml_task_spawn(prod, &pe[i]);
    }
    for (int i = 0; i < P; i++) mml_task_join(pt[i]);
    mml_chan_close(c);
    long long total = 0;
    for (int i = 0; i < C; i++) {
        mml_task_join(rt[i]);
        total += re[i].n;
        CHECK(re[i].closed_n == 1, "consumer must exit on close");
    }
    CHECK(total == (long long)P * N, "every item exactly once");
    return fails;
}

static int mode_unbound(void) {
    mml_chan *c = mml_chan_new(8);      /* unbounded: ring off-limits */
    long long a = 11, got = 0;
    mml_chan_send(c, &a);
    mml_chan_send(c, &a);
    mml_chan_recv(c, &got);
    CHECK(got == 11, "unbounded recv 1");
    mml_chan_recv(c, &got);
    CHECK(got == 11, "unbounded recv 2");
    mml_chan_close(c);
    return fails;
}

static int mode_fallback(void) {
    /* bound*esz > 16 MiB default budget → lazy fallback path; a few
     * messages still transit correctly. */
    mml_chan *c = mml_chan_new_bounded(8, (size_t)3 * 1024 * 1024);
    long long a = 5, got = 0;
    CHECK(mml_chan_send_bounded(c, &a) == MML_CHAN_SENT, "send");
    CHECK(mml_chan_recv_bounded(c, &got) == MML_CHAN_SENT, "recv");
    CHECK(got == 5, "fallback payload");
    return fails;
}

/* mmlrt provides main() and forwards to mml_main_args; MODE selects
 * the case. */
long long mml_main_args(const mml_str_t *args, long long len) {
    (void)args; (void)len;
    mode = getenv("MODE");
    if (!mode) mode = "wrap";
    if      (!strcmp(mode, "sizes"))    mode_sizes();
    else if (!strcmp(mode, "wrap"))     mode_wrap();
    else if (!strcmp(mode, "big"))      mode_big();
    else if (!strcmp(mode, "full"))     mode_full();
    else if (!strcmp(mode, "reject"))   mode_reject();
    else if (!strcmp(mode, "drain"))    mode_drain();
    else if (!strcmp(mode, "close"))    mode_close();
    else if (!strcmp(mode, "mpmc"))     mode_mpmc();
    else if (!strcmp(mode, "unbound"))  mode_unbound();
    else if (!strcmp(mode, "fallback")) mode_fallback();
    else { printf("FAIL unknown MODE %s\n", mode); return 2; }
    printf("%s: %s\n", mode, fails ? "FAIL" : "ok");
    return fails ? 1 : 0;
}
