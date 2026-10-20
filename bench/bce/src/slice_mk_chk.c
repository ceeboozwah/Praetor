/* BCE E6 checked C twin — explicit range check per formation:
 * if (k > len) abort(); isolates enforcement cost from construction. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

typedef struct { const int64_t *ptr; uint64_t len; } sv;

static sv mk(const int64_t *base, uint64_t cap, uint64_t lo, uint64_t hi) {
    if (hi > cap || lo > hi) abort();   /* the check Praetor emits */
    sv t = { base + lo, hi - lo };
    return t;
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    uint64_t n = strtoull(argv[1], 0, 10);
    int64_t *b = malloc(4096 * sizeof(int64_t));
    for (uint64_t i = 0; i < 4096; i++) b[i] = (int64_t)i;
    int64_t acc = 0;
    uint64_t k = 1;
    for (uint64_t i = 0; i < n; i++) {
        sv t = mk(b, 4096, 0, k);
        acc += t.ptr[t.len - 1];
        if (++k > 4096) k = 1;
    }
    printf("%lld\n", (long long)acc);
    free(b);
    return 0;
}
