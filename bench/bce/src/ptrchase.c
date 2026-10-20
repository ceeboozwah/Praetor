/* BCE-0.3 E2 C twin — same chase, unchecked: check-free chain. */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    uint64_t n = strtoull(argv[1], 0, 10);
    uint64_t *b = malloc(65536 * sizeof(uint64_t));
    for (uint64_t i = 0; i < 65536; i++) b[i] = (i + 4099) & 65535;
    uint64_t cur = 0;
    for (uint64_t i = 0; i < n; i++) cur = b[cur];
    printf("%llu\n", (unsigned long long)cur);
    free(b);
    return 0;
}
