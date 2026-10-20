/* BCE E1 C twin — identical scan, unchecked by construction. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

static int64_t scan(const int64_t *s, uint64_t len) {
    int64_t acc = 0;
    for (uint64_t i = 0; i < len; i++) acc += s[i];
    return acc;
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    uint64_t n = strtoull(argv[1], 0, 10);
    int64_t *b = malloc(n * sizeof(int64_t));
    for (uint64_t i = 0; i < n; i++) b[i] = (int64_t)i;
    printf("%lld\n", (long long)scan(b, n));
    free(b);
    return 0;
}
