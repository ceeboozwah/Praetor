/* BCE E8 C twin — {ptr,len} moved through the same chain, 1 alloc. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

typedef struct { int64_t *ptr; uint64_t len; } buf;

static buf fwd2(buf b) { return b; }
static buf fwd1(buf b) { return fwd2(b); }

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int64_t n = strtoll(argv[1], 0, 10), acc = 0;
    buf c = { malloc(4 * sizeof(int64_t)), 4 };
    c.ptr[0] = 7;
    for (int64_t i = 0; i < n; i++) {
        c = fwd1(c);
        acc += c.ptr[0];
    }
    printf("%lld\n", (long long)acc);
    free(c.ptr);
    return 0;
}
