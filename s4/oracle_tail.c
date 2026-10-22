/* ---------- S4 ownership oracle (test-only) ----------
 * Appended to a patched copy of mmlrt_common.c where the real
 * bodies were renamed mml_alloc_raw/mml_free_raw. Tracks every
 * live user allocation and reports, on stderr as `ORACLE:` lines:
 *   ORACLE double-free <p>    — free of a pointer already freed
 *   ORACLE free-null          — mml_free(NULL)
 *   ORACLE unowned-free <p>   — free of memory mml_alloc never gave
 *   ORACLE leaked <n>         — live allocations remaining at exit
 *   ORACLE table-full         — tracking capacity exceeded (tooling)
 * Freed bytes are poisoned before release so a stale read sees
 * 0xDD… garbage rather than the old payload (probabilistic UAF
 * signal; ASan runs carry the deterministic check). Tombstones:
 * a freed slot keeps its pointer with live=0 so probe chains
 * survive deletion. */
#define ORACLE_CAP (1u << 16)

typedef struct {
    void *ptr;
    long long size;
    unsigned char live;
} OracleSlot;

static OracleSlot oracle_tab[ORACLE_CAP];
static volatile int oracle_lock;
static long long oracle_live;

static void oracle_acquire(void) {
    while (__atomic_test_and_set(&oracle_lock, __ATOMIC_ACQUIRE)) {
    }
}
static void oracle_release(void) {
    __atomic_clear(&oracle_lock, __ATOMIC_RELEASE);
}

static void oracle_report_leaks(void) {
    oracle_acquire();
    if (oracle_live > 0) {
        fprintf(stderr, "ORACLE leaked %lld\n", oracle_live);
    }
    oracle_release();
}

static unsigned oracle_hash(void *p) {
    uintptr_t x = (uintptr_t)p;
    x ^= x >> 16;
    return (unsigned)(x & (ORACLE_CAP - 1));
}

void *mml_alloc(long long size) {
    void *p = mml_alloc_raw(size);
    oracle_acquire();
    {
        static int hooked = 0;
        if (!hooked) {
            hooked = 1;
            atexit(oracle_report_leaks);
        }
    }
    {
        unsigned i = oracle_hash(p);
        unsigned n;
        for (n = 0; n < ORACLE_CAP; n++) {
            OracleSlot *s = &oracle_tab[(i + n) & (ORACLE_CAP - 1)];
            if (!s->live) {
                s->ptr = p;
                s->size = size;
                s->live = 1;
                oracle_live++;
                break;
            }
        }
        if (n == ORACLE_CAP) {
            fprintf(stderr, "ORACLE table-full\n");
            abort();
        }
    }
    oracle_release();
    return p;
}

void mml_free(void *p) {
    if (p == NULL) {
        fprintf(stderr, "ORACLE free-null\n");
        return;
    }
    oracle_acquire();
    {
        unsigned i = oracle_hash(p);
        unsigned n;
        OracleSlot *hit = NULL;
        for (n = 0; n < ORACLE_CAP; n++) {
            OracleSlot *s = &oracle_tab[(i + n) & (ORACLE_CAP - 1)];
            if (s->ptr == p) {
                hit = s;
                break;
            }
            if (s->ptr == NULL) break;
        }
        if (hit == NULL) {
            oracle_release();
            fprintf(stderr, "ORACLE unowned-free %p\n", p);
            abort();
        }
        if (!hit->live) {
            oracle_release();
            fprintf(stderr, "ORACLE double-free %p\n", p);
            abort();
        }
        {
            long long sz = hit->size;
            hit->live = 0;
            oracle_live--;
            oracle_release();
            if (sz > 0) {
                memset(p, 0xDD, (size_t)sz);
            }
        }
    }
    mml_free_raw(p);
}
