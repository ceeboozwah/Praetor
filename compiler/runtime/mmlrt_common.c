/* mmlrt_common.c — platform-neutral runtime (N58C-2 split).
 * Bounds/shift/range traps, UTF-8 validation, the argv stash, and
 * the allocator — none of this touches an OS API, so it is shared
 * verbatim by every platform backend. */
#include "mmlrt.h"
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* ---------- checks ---------- */

void mml_bounds_check(long long idx, long long len) {
    if (idx < 0 || idx >= len) {
        fprintf(stderr, "mml: index %lld out of bounds (len %lld)\n", idx, len);
        abort();
    }
}

/* N50 — the trap half of the inlined bounds check: codegen emits the
 * `idx < len` compare inline and calls here only on failure, so the
 * hot path is a single fused cmp+br while reporting stays in one
 * place. noreturn is load-bearing — the LLVM declare says so too. */
#if defined(_MSC_VER)
__declspec(noreturn)
#else
__attribute__((noreturn))
#endif
void mml_bounds_fail(long long idx, long long len) {
    fprintf(stderr, "mml: index %lld out of bounds (len %lld)\n", idx, len);
    abort();
}

/* N25A — shift amount must be in [0, width); no C-style masking. */
void mml_shift_check(long long amt, long long width) {
    if (amt < 0 || amt >= width) {
        fprintf(stderr, "mml: shift amount %lld out of range (0..%lld)\n",
                amt, width);
        abort();
    }
}

void mml_range_check(long long lo, long long hi, long long limit) {
    if (lo < 0 || lo > hi || hi > limit) {
        fprintf(stderr, "mml: range [%lld..%lld) out of bounds (len %lld)\n",
                lo, hi, limit);
        abort();
    }
}

/* ---------- strings ---------- */

/* Value equality over the view's bytes: length first, then memcmp.
 * O(n) by design — str identity comparison is not a thing. */
int mml_str_eq(const void *a, long long alen, const void *b, long long blen) {
    if (alen != blen) return 0;
    return memcmp(a, b, (size_t)alen) == 0;
}

/* The `str_from_utf8` gate: reject (trap, like a bounds check) any
 * byte sequence that is not well-formed UTF-8 — including overlong
 * encodings, surrogates, stray continuations, and truncation.
 * Table-free walk: for each lead byte, check the payload range and
 * that the continuation bytes are 0x80..0xBF. */
void mml_str_validate(const void *p, long long len) {
    const unsigned char *s = (const unsigned char *)p;
    long long i = 0;
    while (i < len) {
        unsigned char c = s[i];
        int cont;
        unsigned char lo = 0x80, hi = 0xBF; /* first-continuation range */
        if (c < 0x80) { i++; continue; }
        else if (c >= 0xC2 && c <= 0xDF) cont = 1;
        else if (c == 0xE0) { cont = 2; lo = 0xA0; }      /* no overlong */
        else if (c >= 0xE1 && c <= 0xEC) cont = 2;
        else if (c == 0xED) { cont = 2; hi = 0x9F; }      /* no surrogates */
        else if (c >= 0xEE && c <= 0xEF) cont = 2;
        else if (c == 0xF0) { cont = 3; lo = 0x90; }      /* no overlong */
        else if (c >= 0xF1 && c <= 0xF3) cont = 3;
        else if (c == 0xF4) { cont = 3; hi = 0x8F; }      /* <= U+10FFFF */
        else goto bad; /* 0x80..0xC1, 0xF5..0xFF — never valid leads */
        if (i + cont >= len) goto bad;                  /* truncated */
        if (s[i + 1] < lo || s[i + 1] > hi) goto bad;
        for (int k = 2; k <= cont; k++) {
            if (s[i + k] < 0x80 || s[i + k] > 0xBF) goto bad;
        }
        i += cont + 1;
    }
    return;
bad:
    fprintf(stderr, "mml: invalid UTF-8 in `str_from_utf8` (byte %lld of %lld)\n",
            i, len);
    abort();
}

/* Non-trapping UTF-8 scan for callers that want a status, not an
 * abort — same table-free walk as mml_str_validate. */
int mml_str_check(const void *p, long long len) {
    const unsigned char *s = (const unsigned char *)p;
    long long i = 0;
    while (i < len) {
        unsigned char c = s[i];
        int cont;
        unsigned char lo = 0x80, hi = 0xBF; /* first-continuation range */
        if (c < 0x80) { i++; continue; }
        else if (c >= 0xC2 && c <= 0xDF) cont = 1;
        else if (c == 0xE0) { cont = 2; lo = 0xA0; }
        else if (c >= 0xE1 && c <= 0xEC) cont = 2;
        else if (c == 0xED) { cont = 2; hi = 0x9F; }
        else if (c >= 0xEE && c <= 0xEF) cont = 2;
        else if (c == 0xF0) { cont = 3; lo = 0x90; }
        else if (c >= 0xF1 && c <= 0xF3) cont = 3;
        else if (c == 0xF4) { cont = 3; hi = 0x8F; }
        else return 0;
        if (i + cont >= len) return 0;
        if (s[i + 1] < lo || s[i + 1] > hi) return 0;
        for (int k = 2; k <= cont; k++) {
            if (s[i + k] < 0x80 || s[i + k] > 0xBF) return 0;
        }
        i += cont + 1;
    }
    return 1;
}

/* ---------- argv stash (N32) ----------
 * The platform entry point builds the str-shaped array on storage
 * that outlives every frame and stashes it here; `process.args()`
 * serves it back as a `slice str` — the same {ptr,len} layout. */
static long long g_argc = 0;
static const void *g_argv_arr = 0;
void mml_stash_argv(long long argc, const void *arr) {
    g_argc = argc;
    g_argv_arr = arr;
}
const void *mml_argv(void) { return g_argv_arr; }
long long mml_argc(void) { return g_argc; }
/* i-th argv as a borrowed view; out-params keep `str` itself out of
 * the FFI surface. */
int mml_arg(long long i, const char **out_ptr, long long *out_len) {
    const mml_str_t *arr = (const mml_str_t *)g_argv_arr;
    if (i < 0 || i >= g_argc) return 1;
    *out_ptr = arr[i].ptr;
    *out_len = arr[i].len;
    return 0;
}

/* ---------- explicit allocation (N16A) ----------
 * `system_allocator()` is the NULL token — the system heap. The
 * checker guarantees `free` reaches here exactly once per root.
 * NULL-returning malloc is a hard trap (consistent with bounds
 * checks). */
void *mml_alloc(long long size) {
    /* QA-002 — the boundary encodes the invariant: a negative size
     * becomes SIZE_MAX in the size_t cast, and sizes must be
     * representable rather than merely "usually malloc fails". */
    if (size < 0) {
        fprintf(stderr, "mml: allocation size %lld is negative\n", size);
        abort();
    }
    void *p = malloc((size_t)size);
    if (!p) {
        fprintf(stderr, "mml: allocation of %lld bytes failed\n", size);
        abort();
    }
    return p;
}

void *mml_alloc_slice(long long size, long long count) {
    /* QA-002 — reject BEFORE multiplying: `size * count` evaluated
     * first is signed-overflow UB, and the product must also be
     * representable as size_t for the eventual malloc call. */
    if (size < 0 || count < 0) {
        fprintf(stderr, "mml: alloc_slice negative operand (%lld x %lld)\n",
                size, count);
        abort();
    }
    if (size != 0 && count > LLONG_MAX / size) {
        fprintf(stderr, "mml: alloc_slice size overflow (%lld x %lld)\n",
                size, count);
        abort();
    }
    long long bytes = size * count;
    if ((unsigned long long)bytes > (unsigned long long)SIZE_MAX) {
        fprintf(stderr, "mml: alloc_slice result %lld exceeds size_t\n",
                bytes);
        abort();
    }
    return mml_alloc(bytes > 0 ? bytes : 1);
}

void mml_free(void *p) {
    free(p);
}
