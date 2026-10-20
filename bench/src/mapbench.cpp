// bench/mapbench — insert N u64 keys, look them all up.
// Same table as every other implementation: FNV-1a on the key
// bytes, linear probe, stored hash, grow at 70%. Track A port.
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

struct Ent { uint64_t k, hash, v; };

static uint64_t fnv64(uint64_t k) {
    uint64_t h = 0xcbf29ce484222325ULL;
    for (int i = 0; i < 8; i++) {
        h = (h ^ (k & 0xff)) * 0x100000001b3ULL;
        k >>= 8;
    }
    return h;
}

struct Map { Ent *slots; uint64_t cap, len, used; };

static void map_insert(Map *m, uint64_t k, uint64_t h, uint64_t v);

static void map_grow(Map *m) {
    Ent *old = m->slots;
    uint64_t ocap = m->cap;
    m->slots = (Ent *)calloc(m->cap * 2, sizeof(Ent));
    m->cap *= 2;
    m->len = m->used = 0;
    for (uint64_t i = 0; i < ocap; i++)
        if (old[i].v) map_insert(m, old[i].k, old[i].hash, old[i].v);
    free(old);
}

static void map_insert(Map *m, uint64_t k, uint64_t h, uint64_t v) {
    if ((m->used + 1) * 10 >= m->cap * 7) map_grow(m);
    uint64_t i = h & (m->cap - 1);
    while (m->slots[i].v) {
        if (m->slots[i].hash == h && m->slots[i].k == k) {
            m->slots[i].v = v;
            return;
        }
        i = (i + 1) & (m->cap - 1);
    }
    m->slots[i] = Ent{k, h, v};
    m->len++;
    m->used++;
}

static uint64_t map_get(const Map *m, uint64_t k, uint64_t h) {
    uint64_t i = h & (m->cap - 1);
    while (m->slots[i].v) {
        if (m->slots[i].hash == h && m->slots[i].k == k)
            return m->slots[i].v;
        i = (i + 1) & (m->cap - 1);
    }
    return 0;
}

int main(int argc, char **argv) {
    if (argc != 2) return 1;
    uint64_t n = strtoull(argv[1], 0, 10);
    Map m = {(Ent *)calloc(16, sizeof(Ent)), 16, 0, 0};
    for (uint64_t i = 1; i <= n; i++) {
        uint64_t k = i * 0x9e3779b97f4a7c15ULL;
        map_insert(&m, k, fnv64(k), i);
    }
    uint64_t sum = 0;
    for (uint64_t i = 1; i <= n; i++) {
        uint64_t k = i * 0x9e3779b97f4a7c15ULL;
        sum += map_get(&m, k, fnv64(k));
    }
    printf("%llu %llu\n", (unsigned long long)m.len,
           (unsigned long long)sum);
    free(m.slots);
    return 0;
}
