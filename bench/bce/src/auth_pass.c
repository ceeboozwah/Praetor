/* BCE E7 C twin — same 3-deep chain carrying an unused ctx ptr. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

static int64_t inner(void *ctx, int64_t x) { return x + 1; }
static int64_t mid(void *ctx, int64_t x)   { return inner(ctx, x); }
static int64_t outer(void *ctx, int64_t x) { return mid(ctx, x); }

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int64_t n = strtoll(argv[1], 0, 10), acc = 0;
    void *ctx = malloc(1);
    for (int64_t i = 0; i < n; i++) acc += outer(ctx, i);
    printf("%lld\n", (long long)acc);
    free(ctx);
    return 0;
}
