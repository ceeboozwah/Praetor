/* BCE E3c C twin — L2-resident REVERSE scan, matched order. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int64_t n = strtoll(argv[1], 0, 10), acc = 0;
    int64_t *b = malloc(8192 * sizeof(int64_t));
    for (uint64_t i = 0; i < 8192; i++) b[i] = (int64_t)i;
    for (int64_t k = 0; k < n; k++) {
        for (uint64_t j = 0; j < 8192; j++) acc += b[8192 - 1 - j];
    }
    printf("%lld\n", (long long)acc);
    free(b);
    return 0;
}
