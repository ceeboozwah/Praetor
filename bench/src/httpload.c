/* bench/httpload — shared driver: open N sequential loopback
 * connections, send a tiny GET, read the response, count 200s. */
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
    const char *req = "GET / HTTP/1.0\r\n\r\n";
    char buf[2048];
    long ok = 0;
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = htons((uint16_t)port);
    for (long i = 0; i < n; i++) {
        /* every i must land exactly one accepted conn — the server
         * exits on a conn COUNT, so a dropped connect hangs it.
         * Retry briefly (loopback backlog / ephemeral-port reuse). */
        int c = -1;
        for (int a_try = 0; a_try < 5000 && c < 0; a_try++) {
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
        if (c < 0) return 2;  /* dropped conn would hang the server — fail loudly */
        write(c, req, strlen(req));
        ssize_t r = read(c, buf, sizeof(buf) - 1);
        if (r > 0) { buf[r] = 0; if (strstr(buf, "200")) ok++; }
        close(c);
    }
    printf("%ld\n", ok);
    return 0;
}
