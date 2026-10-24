#!/usr/bin/env bash
# O1 ring selftest — builds the unpatched platform runtime and
# exercises the bounded-channel contract against every queue
# layout: fixed ring (default), MML_CHAN_RING=0 fallback, and the
# segmented fallback. Usage: sh s4/ring_selftest.sh (from repo root)
set -u
cd "$(dirname "$0")/.."
WORK=${WORK:-/tmp/o1-ring-selftest}
mkdir -p "$WORK"

CC=${CC:-cc}
plat=mmlrt_posix.c
[ "$(uname -s 2>/dev/null)" = "Windows_NT" ] && plat=mmlrt_win32.c

cp compiler/runtime/mmlrt.h "$WORK/"
cp compiler/runtime/mmlrt_common.c "$WORK/"
cp compiler/runtime/$plat "$WORK/"
cp s4/ring_selftest.c "$WORK/selftest.c"

$CC -O2 -g -c "$WORK/mmlrt_common.c" -o "$WORK/common.o" -I"$WORK" || exit 1
$CC -O2 -g -c "$WORK/$(basename $plat .c).c" -o "$WORK/plat.o" -I"$WORK" || exit 1
$CC -O2 -g "$WORK/selftest.c" "$WORK/common.o" "$WORK/plat.o" \
    -o "$WORK/selftest" -I"$WORK" || exit 1

fail=0
run() {  # name, ring-env, [seg-env]
    desc=$1; shift
    out=$(env "$@" MODE="$desc" "$WORK/selftest" 2>&1); rc=$?
    if [ $rc -eq 0 ]; then echo "PASS $desc ($*)"
    else echo "FAIL $desc ($*): exit $rc"; echo "$out"; fail=1; fi
}

# fixed ring (default budget)
for m in sizes wrap big full reject drain close mpmc unbound; do
    run "$m"
done
# legacy fallback — ring disabled, segmented disabled
for m in sizes wrap full reject drain close mpmc fallback; do
    run "$m" MML_CHAN_RING=0
done
# segmented fallback — ring disabled, MML_CHAN_SEG=4
for m in sizes wrap full reject drain close mpmc fallback; do
    run "$m" MML_CHAN_RING=0 MML_CHAN_SEG=4
done
# ring forced on for an over-budget case: raise the budget so the
# 24 MiB bound*esz fits the ring instead of falling back
run fallback MML_CHAN_RING=33554432

exit $fail
