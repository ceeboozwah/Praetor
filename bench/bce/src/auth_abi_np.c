/* BCE-0.3 E3 second C twin — NO context parameter: isolates the
 * cost of carrying one extra argument per call. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

static int64_t work(int64_t x, int64_t n) {
    int64_t acc = x;
    for (int64_t i = 0; i < n; i++) {
        acc = acc * 1103515245 + 12345;
        acc = acc & 0x7FFFFFFF;
    }
    return acc;
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int64_t n = strtoll(argv[1], 0, 10), acc = 0;
    for (int64_t i = 0; i < n; i++) acc += work(i, 64);
    printf("%lld\n", (long long)acc);
    return 0;
}
