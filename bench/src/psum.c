/* bench/psum — parallel sum over N i64s. Four workers; each
 * mallocs, fills, sums, and frees its own quarter. */
#include <pthread.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdatomic.h>

typedef struct { int64_t lo, hi; _Atomic int64_t *acc; } Arg;

static void *worker(void *p) {
    Arg *a = p;
    int64_t n = a->hi - a->lo;
    int64_t *b = malloc((size_t)n * sizeof(int64_t));
    int64_t s = 0;
    for (int64_t i = 0; i < n; i++) b[i] = a->lo + i;
    for (int64_t i = 0; i < n; i++) s += b[i];
    free(b);
    atomic_fetch_add(a->acc, s);
    return NULL;
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int64_t n = strtoll(argv[1], 0, 10), q = n / 4;
    _Atomic int64_t acc = 0;
    pthread_t t[4];
    Arg a[4];
    int64_t bounds[5] = {0, q, 2 * q, 3 * q, n};
    for (int i = 0; i < 4; i++) {
        a[i] = (Arg){bounds[i], bounds[i + 1], &acc};
        pthread_create(&t[i], NULL, worker, &a[i]);
    }
    for (int i = 0; i < 4; i++) pthread_join(t[i], NULL);
    printf("%lld\n", (long long)acc);
    return 0;
}
