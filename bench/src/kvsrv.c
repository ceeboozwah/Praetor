/* bench/kvsrv — iterative TCP key/value store: one request per
 * connection (`SET k v\n` -> `OK`, `GET k\n` -> `VAL v`/`ERR`).
 * Same map as the suite: FNV-1a, linear probe, owned key copies. */
#include <arpa/inet.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <unistd.h>

typedef struct { char *k; uint64_t klen, hash; int64_t v; } Ent;

static uint64_t fnv(const char *k, uint64_t n) {
    uint64_t h = 0xcbf29ce484222325ULL;
    for (uint64_t i = 0; i < n; i++) h = (h ^ (uint8_t)k[i]) * 0x100000001b3ULL;
    return h;
}

typedef struct { Ent *slots; uint64_t cap, len, used; } Map;

static void map_put(Map *m, const char *k, uint64_t klen, uint64_t h, int64_t v);

static void map_grow(Map *m) {
    Ent *old = m->slots;
    uint64_t ocap = m->cap;
    m->slots = calloc(m->cap * 2, sizeof(Ent));
    m->cap *= 2;
    m->len = m->used = 0;
    for (uint64_t i = 0; i < ocap; i++)
        if (old[i].k) {
            map_put(m, old[i].k, old[i].klen, old[i].hash, old[i].v);
            free(old[i].k);
        }
    free(old);
}

/* owned-copy insert: `k` is transient, the table keeps its own. */
static void map_put(Map *m, const char *k, uint64_t klen, uint64_t h, int64_t v) {
    if ((m->used + 1) * 10 >= m->cap * 7) map_grow(m);
    uint64_t i = h & (m->cap - 1);
    while (m->slots[i].k) {
        Ent *e = &m->slots[i];
        if (e->hash == h && e->klen == klen && memcmp(e->k, k, klen) == 0) {
            e->v = v;
            return;
        }
        i = (i + 1) & (m->cap - 1);
    }
    char *kb = malloc(klen);
    memcpy(kb, k, klen);
    m->slots[i] = (Ent){kb, klen, h, v};
    m->len++;
    m->used++;
}

static int64_t *map_get(Map *m, const char *k, uint64_t klen, uint64_t h) {
    uint64_t i = h & (m->cap - 1);
    while (m->slots[i].k) {
        Ent *e = &m->slots[i];
        if (e->hash == h && e->klen == klen && memcmp(e->k, k, klen) == 0)
            return &e->v;
        i = (i + 1) & (m->cap - 1);
    }
    return NULL;
}

static void handle(int c, Map *m) {
    char buf[1024];
    ssize_t n = read(c, buf, sizeof(buf));
    if (n <= 0) return;
    uint64_t i;
    int op = -1;
    if (n >= 4 && !memcmp(buf, "SET ", 4)) op = 1;
    else if (n >= 4 && !memcmp(buf, "GET ", 4)) op = 2;
    if (op < 0) { write(c, "ERR", 3); return; }
    i = 4;
    uint64_t klo = i;
    while (i < (uint64_t)n && buf[i] != ' ' && buf[i] != '\n') i++;
    uint64_t khi = i;
    if (khi == klo) { write(c, "ERR", 3); return; }
    const char *k = buf + klo;
    uint64_t klen = khi - klo;
    if (op == 1) {
        i++;
        if (i >= (uint64_t)n) { write(c, "ERR", 3); return; }
        int64_t v = 0;
        while (i < (uint64_t)n && buf[i] >= '0' && buf[i] <= '9')
            v = v * 10 + (buf[i++] - '0');
        map_put(m, k, klen, fnv(k, klen), v);
        write(c, "OK", 2);
    } else {
        int64_t *g = map_get(m, k, klen, fnv(k, klen));
        if (!g) { write(c, "ERR", 3); return; }
        char out[32];
        int olen = snprintf(out, sizeof(out), "VAL %lld", (long long)*g);
        write(c, out, (size_t)olen);
    }
}

int main(int argc, char **argv) {
    if (argc != 3) return 1;
    int port = atoi(argv[1]);
    long nconn = strtol(argv[2], 0, 10);
    int srv = socket(AF_INET, SOCK_STREAM, 0);
    int one = 1;
    setsockopt(srv, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
    struct sockaddr_in a = {0};
    a.sin_family = AF_INET;
    a.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    a.sin_port = htons((uint16_t)port);
    if (bind(srv, (struct sockaddr *)&a, sizeof(a))) return 3;
    if (listen(srv, 16)) return 4;
    Map m = {calloc(64, sizeof(Ent)), 64, 0, 0};
    long i = 0;
    for (; i < nconn; i++) {
        int c = accept(srv, NULL, NULL);
        if (c < 0) return 5;
        handle(c, &m);
        close(c);
    }
    close(srv);
    for (uint64_t j = 0; j < m.cap; j++) free(m.slots[j].k);
    free(m.slots);
    printf("%ld\n", i);
    return 0;
}
