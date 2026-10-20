/* BCE E4 C twin — REVERSE traversal, matched access order, unchecked. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    uint64_t n = strtoull(argv[1], 0, 10);
    int64_t *b = malloc(n * sizeof(int64_t));
    for (uint64_t i = 0; i < n; i++) b[i] = (int64_t)i;
    int64_t acc = 0;
    for (uint64_t i = 0; i < n; i++) acc += b[n - 1 - i];
    printf("%lld\n", (long long)acc);
    free(b);
    return 0;
}
