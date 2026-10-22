/* S4 oracle self-test — drives the patched runtime's mml_alloc /
 * mml_free wrappers directly. Each case exercises one detector;
 * fatal detectors (double-free, unowned-free) abort the process, so
 * they run in dedicated modes selected by env MODE.
 *
 *   reuse   alloc A, free A, alloc B at the SAME address, free B —
 *           must be silent (the reviewer's address-reuse bug made
 *           this report a phantom double-free).
 *   live    alloc, free — clean exit, no ORACLE output.
 *   leak    alloc, no free — expect `ORACLE leaked 1` at exit.
 *   null    free(NULL) — expect `ORACLE free-null`, continue.
 *   double  alloc, free, free — expect `ORACLE double-free` + abort.
 *   unowned free(p not from mml_alloc) — expect `ORACLE
 *           unowned-free` + abort.
 *
 * Build (from repo root):
 *   sh s4/selftest.sh
 */
#include "mmlrt.h"
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* mmlrt_posix provides main() and forwards to mml_main_args; this
 * standalone test supplies it and selects a mode via env MODE. */
long long mml_main_args(const mml_str_t *args, long long len) {
    const char *mode = getenv("MODE");
    (void)args; (void)len;
    if (!mode) mode = "live";

    if (!strcmp(mode, "reuse")) {
        /* Same-size free/malloc cycles reuse the address on
         * mainstream allocators; the loop tolerates implementations
         * that cycle through several addresses before repeating. */
        void *first = NULL;
        int reused = 0, i;
        for (i = 0; i < 64 && !reused; i++) {
            void *p = mml_alloc(16);
            if (!first) first = p;
            else if (p == first) reused = 1;
            mml_free(p);          /* must not report double-free */
        }
        if (!reused) {
            /* Reuse not observed — still a valid run; report so the
             * harness can note coverage rather than fake a pass. */
            printf("reuse: address never recycled (inconclusive)\n");
        } else {
            printf("reuse: address recycled, free accepted\n");
        }
        return 0;
    }
    if (!strcmp(mode, "live")) {
        void *a = mml_alloc(16);
        mml_free(a);
        printf("live: ok\n");
        return 0;
    }
    if (!strcmp(mode, "leak")) {
        (void)mml_alloc(8);
        printf("leak: returning\n");
        return 0;
    }
    if (!strcmp(mode, "null")) {
        mml_free(NULL);
        printf("null: continued\n");
        return 0;
    }
    if (!strcmp(mode, "double")) {
        void *a = mml_alloc(16);
        mml_free(a);
        mml_free(a);              /* ORACLE double-free + abort */
        printf("double: UNREACHABLE\n");
        return 1;
    }
    if (!strcmp(mode, "unowned")) {
        mml_free((void *)(uintptr_t)0xDEAD);  /* ORACLE unowned-free */
        printf("unowned: UNREACHABLE\n");
        return 1;
    }
    fprintf(stderr, "unknown mode %s\n", mode);
    return 2;
}
