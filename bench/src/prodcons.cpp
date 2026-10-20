// bench/prodcons — N heap-allocated packets through an unbounded
// channel, producer thread -> main consumer. malloc ownership
// crosses the send; the consumer frees each packet.
// Track A port of prodcons.c: same queue, same condvar discipline.
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

struct Node { int64_t v; Node *next; };

struct Chan {
    pthread_mutex_t mu;
    pthread_cond_t cv;
    Node *head, *tail;
    int closed;
};

static Chan *chan_new(void) {
    Chan *c = (Chan *)calloc(1, sizeof(Chan));
    pthread_mutex_init(&c->mu, NULL);
    pthread_cond_init(&c->cv, NULL);
    return c;
}

static void chan_send(Chan *c, Node *m) {
    pthread_mutex_lock(&c->mu);
    m->next = NULL;
    if (c->tail) c->tail->next = m; else c->head = m;
    c->tail = m;
    pthread_cond_signal(&c->cv);
    pthread_mutex_unlock(&c->mu);
}

static Node *chan_recv(Chan *c) {
    pthread_mutex_lock(&c->mu);
    while (!c->head && !c->closed) pthread_cond_wait(&c->cv, &c->mu);
    Node *m = c->head;
    if (m) { c->head = m->next; if (!c->head) c->tail = NULL; }
    pthread_mutex_unlock(&c->mu);
    return m;
}

struct Arg { Chan *ch; int64_t n; };

static void *produce(void *p) {
    Arg *a = (Arg *)p;
    for (int64_t i = 0; i < a->n; i++) {
        Node *m = (Node *)malloc(sizeof(Node));
        m->v = i;
        chan_send(a->ch, m);
    }
    Node *stop = (Node *)malloc(sizeof(Node));
    stop->v = -1;
    chan_send(a->ch, stop);
    return NULL;
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int64_t n = strtoll(argv[1], 0, 10);
    Chan *ch = chan_new();
    Arg a = {ch, n};
    pthread_t t;
    pthread_create(&t, NULL, produce, &a);
    int64_t total = 0;
    for (;;) {
        Node *g = chan_recv(ch);
        if (g->v < 0) { free(g); break; }
        total += g->v;
        free(g);
    }
    pthread_join(t, NULL);
    printf("%lld\n", (long long)total);
    return 0;
}
