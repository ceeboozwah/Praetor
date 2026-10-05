/* bench/wordfreq — count tokens + unique words over argv[1].
 * Open-addressing table matching the lab StrMap: FNV-1a, linear
 * probe, stored hashes, power-of-2 cap, grow at 70%. Keys are
 * zero-copy views into the file buffer — same as the .lang side. */
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <unistd.h>

typedef struct { const char *k; uint64_t len, hash; int64_t v; } Ent;

static uint64_t fnv(const char *k, uint64_t n) {
    uint64_t h = 0xcbf29ce484222325ULL;
    for (uint64_t i = 0; i < n; i++) h = (h ^ (uint8_t)k[i]) * 0x100000001b3ULL;
    return h;
}

typedef struct { Ent *slots; uint64_t cap, len, used; } Map;

static Map map_new(uint64_t cap) {
    Map m;
    m.cap = cap;
    m.len = m.used = 0;
    m.slots = calloc(cap, sizeof(Ent));
    return m;
}

static void map_insert(Map *m, const char *k, uint64_t klen, uint64_t h, int64_t v);

static void map_grow(Map *m) {
    Ent *old = m->slots;
    uint64_t ocap = m->cap;
    m->slots = calloc(m->cap * 2, sizeof(Ent));
    m->cap *= 2;
    m->len = m->used = 0;
    for (uint64_t i = 0; i < ocap; i++)
        if (old[i].k) map_insert(m, old[i].k, old[i].len, old[i].hash, old[i].v);
    free(old);
}

static void map_insert(Map *m, const char *k, uint64_t klen, uint64_t h, int64_t v) {
    if ((m->used + 1) * 10 >= m->cap * 7) map_grow(m);
    uint64_t i = h & (m->cap - 1);
    while (m->slots[i].k) {
        Ent *e = &m->slots[i];
        if (e->hash == h && e->len == klen && memcmp(e->k, k, klen) == 0) {
            e->v = v;
            return;
        }
        i = (i + 1) & (m->cap - 1);
    }
    m->slots[i] = (Ent){k, klen, h, v};
    m->len++;
    m->used++;
}

static Ent *map_get(Map *m, const char *k, uint64_t klen, uint64_t h) {
    uint64_t i = h & (m->cap - 1);
    while (m->slots[i].k) {
        Ent *e = &m->slots[i];
        if (e->hash == h && e->len == klen && memcmp(e->k, k, klen) == 0) return e;
        i = (i + 1) & (m->cap - 1);
    }
    return NULL;
}

static int is_ws(uint8_t c) { return c == ' ' || c == '\n' || c == '\r' || c == '\t'; }

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    int fd = open(argv[1], O_RDONLY);
    if (fd < 0) return 2;
    struct stat st;
    if (fstat(fd, &st)) return 2;
    char *buf = malloc((size_t)st.st_size + 1);
    if (!buf) return 2;
    if (read(fd, buf, (size_t)st.st_size) != st.st_size) return 2;
    close(fd);
    buf[st.st_size] = 0;

    Map m = map_new(16);
    int64_t toks = 0;
    uint64_t n = (uint64_t)st.st_size, wlo = 0;
    int in_word = 0;
    for (uint64_t i = 0; i <= n; i++) {
        int closing = i == n || is_ws((uint8_t)buf[i]);
        if (in_word && closing) {
            const char *k = buf + wlo;
            uint64_t klen = i - wlo, h = fnv(k, klen);
            Ent *e = map_get(&m, k, klen, h);
            if (e) e->v++;
            else map_insert(&m, k, klen, h, 1);
            toks++;
            in_word = 0;
        }
        if (!in_word && !closing) {
            wlo = i;
            in_word = 1;
        }
    }
    printf("%lld %lld\n", (long long)toks, (long long)m.len);
    free(m.slots);
    free(buf);
    return 0;
}
