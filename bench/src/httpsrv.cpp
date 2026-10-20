// bench/httpsrv — iterative HTTP responder: accept argv-bounded
// connections, one read, one fixed response, close.
// Track A port of httpsrv.c: same socket discipline.
#include <arpa/inet.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc != 3) return 1;
    int port = atoi(argv[1]);
    long nconn = strtol(argv[2], 0, 10);
    int srv = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    setsockopt(srv, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
    struct sockaddr_in a;
    memset(&a, 0, sizeof(a));
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = htons((uint16_t)port);
    if (bind(srv, (struct sockaddr *)&a, sizeof(a))) return 3;
    if (listen(srv, 16)) return 4;
    const char *resp = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
    char buf[2048];
    long i = 0;
    for (; i < nconn; i++) {
        int c = accept(srv, NULL, NULL);
        if (c < 0) return 5;
        if (read(c, buf, sizeof(buf)) > 0) write(c, resp, strlen(resp));
        close(c);
    }
    close(srv);
    printf("%ld\n", i);
    return 0;
}
