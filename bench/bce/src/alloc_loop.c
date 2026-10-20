/* Allocator dimension C twin — malloc/write/free per iteration.
 * `sink` is volatile: the allocation escapes through a boundary the
 * optimizer cannot see through, so the alloc cycle CANNOT be
 * eliminated (verified: disasm must contain malloc/free calls). */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

static int64_t * volatile sink;

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int64_t n = strtoll(argv[1], 0, 10), acc = 0;
    for (int64_t i = 0; i < n; i++) {
        int64_t *b = malloc(4 * sizeof(int64_t));
        b[0] = i;
        sink = b;                 /* escape — defeats elimination */
        acc += b[0];
        free(b);
    }
    printf("%lld\n", (long long)acc);
    return 0;
}
