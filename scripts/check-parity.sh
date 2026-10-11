#!/usr/bin/env bash
# check-parity.sh LO HI — exhaustive Stage-0 vs Stage-1 checker
# parity sweep over native-tests/{LO..HI}_*.lang.
#
# A declared numeric range is expanded against actual file
# prefixes: every number in [LO,HI] is either tested or reported
# MISSING. Results are per-fixture PASS / FAIL / CRASH plus a
# summary with expected / missing / discovered / pass counts, so a
# "range green" claim always carries its denominator.
#
# Usage: scripts/check-parity.sh 140 175
#        scripts/check-parity.sh 1 119 | tee /tmp/sweep.txt
set -u
cd "$(dirname "$0")/.."

# Stage-1 checker frames are large (own'd Checker args + return
# slots inline per call site); deep expr trees need >8MB. Raise the
# main-thread stack to the hard limit so CRASH means a real bug,
# not frame-size accounting.
ulimit -s "$(ulimit -Hs)" 2>/dev/null || ulimit -s 65520 2>/dev/null || true

LO=${1:?usage: check-parity.sh LO HI}
HI=${2:?usage: check-parity.sh LO HI}
S0=./target/release/ourc
S1=./target/dumpcheck

expected=$((HI - LO + 1))
missing=""
files=0
pass=0
fail=0
crash=0
fails=""

for n in $(seq "$LO" "$HI"); do
    p=$(printf '%02d' "$n")
    matched=0
    for f in native-tests/${p}_*.lang; do
        [ -e "$f" ] || continue
        matched=1
        files=$((files + 1))
        "$S0" "$f" --dump-check > /tmp/cp_s0.txt 2>/dev/null
        "$S1" "$f" > /tmp/cp_s1.txt 2>/dev/null
        rc=$?
        if [ $rc -gt 128 ]; then
            crash=$((crash + 1))
            fails="$fails $(basename "$f")(sig$((rc-128)))"
            echo "CRASH $(basename "$f") (signal $((rc - 128)))"
        elif cmp -s /tmp/cp_s0.txt /tmp/cp_s1.txt; then
            pass=$((pass + 1))
            echo "PASS  $(basename "$f")"
        else
            fail=$((fail + 1))
            fails="$fails $(basename "$f")"
            echo "FAIL  $(basename "$f")"
        fi
    done
    if [ "$matched" -eq 0 ]; then
        missing="$missing $p"
    fi
done

echo "---"
echo "range:    $LO-$HI ($expected numbers)"
echo "files:    $files discovered ($pass pass, $fail fail, $crash crash)"
if [ -n "$missing" ]; then
    echo "missing:$missing"
else
    echo "missing: none"
fi
if [ -n "$fails" ]; then
    echo "failing:$fails"
fi
