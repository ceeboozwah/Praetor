#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

static void * volatile sink;

__attribute__((noinline))
static int64_t work(int64_t x, int64_t n) {
    int64_t acc = x;
    sink = 0;
    for (int64_t i = 0; i < n; i++) {
        switch (i & 15) {
        case 0: acc = acc * 1103515245 + 1; break;
        case 1: acc = acc * 1103563516 + 2; break;
        case 2: acc = acc * 1103611787 + 3; break;
        case 3: acc = acc * 1103660058 + 4; break;
        case 4: acc = acc * 1103708329 + 5; break;
        case 5: acc = acc * 1103756600 + 6; break;
        case 6: acc = acc * 1103804871 + 7; break;
        case 7: acc = acc * 1103853142 + 8; break;
        case 8: acc = acc * 1103901413 + 9; break;
        case 9: acc = acc * 1103949684 + 10; break;
        case 10: acc = acc * 1103997955 + 11; break;
        case 11: acc = acc * 1104046226 + 12; break;
        case 12: acc = acc * 1104094497 + 13; break;
        case 13: acc = acc * 1104142768 + 14; break;
        case 14: acc = acc * 1104191039 + 15; break;
        case 15: acc = acc * 1104239310 + 16; break;
        default: acc = acc + 1;
        }
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
