/* bench/prodcons_bchan — N heap-allocated packets through a BOUNDED
 * channel (capacity argv[2]), producer thread -> main consumer.
 * malloc ownership crosses the send; the consumer frees each packet.
 * Track-B idiomatic: the POSIX bounded queue = ring + mutex +
 * not_full/not_empty condvars. */
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

typedef struct Node { int64_t v; } Node;

typedef struct {
    pthread_mutex_t mu;
    pthread_cond_t not_full, not_empty;
    Node **ring;
    int64_t cap, count, head, tail;
    int closed;
} Chan;

static Chan *chan_new(int64_t cap) {
    Chan *c = calloc(1, sizeof(Chan));
    pthread_mutex_init(&c->mu, NULL);
    pthread_cond_init(&c->not_full, NULL);
    pthread_cond_init(&c->not_empty, NULL);
    c->ring = calloc((size_t)cap, sizeof(Node *));
    c->cap = cap;
    return c;
}

static void chan_send(Chan *c, Node *m) {
    pthread_mutex_lock(&c->mu);
    while (c->count == c->cap)
        pthread_cond_wait(&c->not_full, &c->mu);
    c->ring[c->tail] = m;
    c->tail = (c->tail + 1) % c->cap;
    c->count++;
    pthread_cond_signal(&c->not_empty);
    pthread_mutex_unlock(&c->mu);
}

static Node *chan_recv(Chan *c) {
    pthread_mutex_lock(&c->mu);
    while (!c->count && !c->closed)
        pthread_cond_wait(&c->not_empty, &c->mu);
    Node *m = NULL;
    if (c->count) {
        m = c->ring[c->head];
        c->head = (c->head + 1) % c->cap;
        c->count--;
        pthread_cond_signal(&c->not_full);
    }
    pthread_mutex_unlock(&c->mu);
    return m;
}

typedef struct { Chan *ch; int64_t n; } Arg;

static void *produce(void *p) {
    Arg *a = p;
    for (int64_t i = 0; i < a->n; i++) {
        Node *m = malloc(sizeof(Node));
        m->v = i;
        chan_send(a->ch, m);
    }
    Node *stop = malloc(sizeof(Node));
    stop->v = -1;
    chan_send(a->ch, stop);
    return NULL;
}

int main(int argc, char **argv) {
    if (argc != 3) return 1;
    int64_t n = strtoll(argv[1], 0, 10);
    int64_t cap = strtoll(argv[2], 0, 10);
    Chan *ch = chan_new(cap);
    Arg a = {ch, n};
    pthread_t t;
    pthread_create(&t, NULL, produce, &a);
    int64_t total = 0;
    for (;;) {
        Node *g = chan_recv(ch);
        if (!g) break;
        if (g->v < 0) { free(g); break; }
        total += g->v;
        free(g);
    }
    pthread_join(t, NULL);
    printf("%lld\n", (long long)total);
    return 0;
}
