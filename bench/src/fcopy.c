/* bench/fcopy — copy argv[1] to argv[2], 64KB chunks. */
#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) return 1;
    int src = open(argv[1], O_RDONLY);
    if (src < 0) return 2;
    int dst = open(argv[2], O_WRONLY | O_CREAT | O_TRUNC, 0644);
    if (dst < 0) return 3;
    char buf[65536];
    long long total = 0;
    for (;;) {
        ssize_t n = read(src, buf, 65536);
        if (n < 0) return 4;
        if (n == 0) break;
        ssize_t w = write(dst, buf, (size_t)n);
        if (w < n) return 5;
        total += n;
    }
    close(src);
    close(dst);
    printf("%lld\n", total);
    return 0;
}
