/* bench/kvload — shared driver: N sequential loopback connections.
 * Each conn sends `SET k<i mod 64> <i>` then, on the second half,
 * `GET k<i mod 64>`; counts OK/VAL replies. */
#include <arpa/inet.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) return 1;
    int port = atoi(argv[1]);
    long n = strtol(argv[2], 0, 10);
    char req[64], buf[256];
    long ok = 0;
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = htons((uint16_t)port);
    for (long i = 0; i < n; i++) {
        /* alternating SET/GET pairs: every GET hits the key its
         * predecessor just stored — ok count stays deterministic. */
        int is_set = i % 2 == 0;
        int len = is_set
            ? snprintf(req, sizeof(req), "SET k%ld %ld\n", (i / 2) % 64, i / 2)
            : snprintf(req, sizeof(req), "GET k%ld\n", (i / 2) % 64);
        /* every i must land exactly one accepted conn — the server
         * exits on a conn COUNT, so a dropped connect hangs it. */
        int c = -1;
        for (int a_try = 0; a_try < 500 && c < 0; a_try++) {
            c = socket(AF_INET, SOCK_STREAM, 0);
            /* client-side REUSEADDR: 3000 sequential loopback conns
             * leave TIME_WAIT sockets that would otherwise pin the
             * ephemeral port range. */
            int one = 1;
            setsockopt(c, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
            if (connect(c, (struct sockaddr *)&a, sizeof(a))) {
                close(c);
                c = -1;
                usleep(2000);
            }
        }
        if (c < 0) continue;
        write(c, req, (size_t)len);
        ssize_t r = read(c, buf, sizeof(buf) - 1);
        if (r > 0) { buf[r] = 0; if (!memcmp(buf, "OK", 2) || !memcmp(buf, "VAL", 3)) ok++; }
        close(c);
    }
    printf("%ld\n", ok);
    return 0;
}
