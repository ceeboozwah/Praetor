#!/usr/bin/env bash
# S4 oracle self-test — builds the instrumented runtime and drives
# every detector: live alloc/free, address reuse (the PR-review
# case), leak, free-null, double-free, unowned-free.
# Usage: sh s4/selftest.sh   (from repo root)
set -u
cd "$(dirname "$0")/.."
WORK=${WORK:-/tmp/s4-selftest}
mkdir -p "$WORK"

CC=${CC:-cc}
plat=mmlrt_posix.c
[ "$(uname -s 2>/dev/null)" = "Windows_NT" ] && plat=mmlrt_win32.c

# Patch a runtime copy: real bodies renamed, oracle wrappers added.
cp compiler/runtime/mmlrt.h "$WORK/"
sed -e 's/void \*mml_alloc(long long size) {/void *mml_alloc_raw(long long size) {/' \
    -e 's/void mml_free(void \*p) {/void mml_free_raw(void *p) {/' \
    compiler/runtime/mmlrt_common.c > "$WORK/mmlrt_common.c"
cat s4/oracle_tail.c >> "$WORK/mmlrt_common.c"
cp compiler/runtime/$plat "$WORK/"
cp s4/oracle_selftest.c "$WORK/selftest.c"

$CC -O0 -g -c "$WORK/mmlrt_common.c" -o "$WORK/common.o" -I"$WORK" || exit 1
$CC -O0 -g -c "$WORK/$(basename $plat .c).c" -o "$WORK/plat.o" -I"$WORK" || exit 1
$CC -O0 -g "$WORK/selftest.c" "$WORK/common.o" "$WORK/plat.o" \
    -o "$WORK/selftest" -I"$WORK" || exit 1

fail=0
run() {  # name, expected-substr, expect-abort(0/1)
    out=$(MODE="$1" "$WORK/selftest" 2>&1); rc=$?
    if [ "$3" = "1" ]; then
        [ $rc -eq 0 ] && { echo "FAIL $1: expected abort"; fail=1; return; }
    else
        [ $rc -ne 0 ] && { echo "FAIL $1: exit $rc"; echo "$out"; fail=1; return; }
    fi
    case "$out" in
        *"$2"*) echo "PASS $1" ;;
        *) echo "FAIL $1: expected '$2' in:"; echo "$out"; fail=1 ;;
    esac
}

run live    "live: ok"            0
run reuse   "reuse:"              0
run null    "ORACLE free-null"    0
run leak    "ORACLE leaked"       0
run double  "ORACLE double-free"  1
run unowned "ORACLE unowned-free" 1

exit $fail
