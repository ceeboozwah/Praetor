/* BCE E5 C twin — masked random access, unchecked. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    uint64_t n = strtoull(argv[1], 0, 10);
    int64_t *b = malloc(4194304 * sizeof(int64_t));
    for (uint64_t i = 0; i < 4194304; i++) b[i] = (int64_t)(i & 255);
    int64_t acc = 0;
    uint64_t h = 0x5DEECE66Dull;
    for (uint64_t i = 0; i < n; i++) {
        h = h * 63641362ull + 14426950ull;
        acc += b[h & 4194303];
    }
    printf("%lld\n", (long long)acc);
    free(b);
    return 0;
}
