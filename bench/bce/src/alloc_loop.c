/* Allocator dimension C twin — malloc/write/free per iteration. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int64_t n = strtoll(argv[1], 0, 10), acc = 0;
    for (int64_t i = 0; i < n; i++) {
        int64_t *b = malloc(4 * sizeof(int64_t));
        b[0] = i;
        acc += b[0];
        free(b);
    }
    printf("%lld\n", (long long)acc);
    return 0;
}
