#!/bin/bash
# s1test-one.sh <fixture.lang> <workdir> — Stage-1 `our` ntest runner.
# Mirrors compiler/src/driver/ntest.rs semantics: @compile-error /
# @exit / @args / @ir-has / @ir-lacks against a binary built by
# target/our (the self-hosted driver).
f="$1"
WORK="$2"
stem="$(basename "$f" .lang)"
OUR="${OUR:-./target/our}"

# ntest skips files with no recognized directive (@exit/@compile-
# error/@ir-*/@args); bare `// @fail` is a different suite's marker.
if ! grep -q "@exit\|@compile-error\|@ir-has\|@ir-lacks\|@args" "$f"; then
    echo "SKIP $stem"
    exit 0
fi
has_cerr="$(grep -c '@compile-error' "$f")"
cerr="$(sed -n 's|.*@compile-error *||p' "$f" | head -1)"

if [ "$has_cerr" != "0" ]; then
    "$OUR" "$f" -o "$WORK/$stem" > /dev/null 2> "$WORK/$stem.err"
    rc=$?
    # empty want = "must fail to compile" (contains("") is true)
    if [ $rc -ne 0 ] && { [ -z "$cerr" ] || grep -qF "$cerr" "$WORK/$stem.err"; }; then
        echo "PASS $stem"
    else
        echo "FAIL-C $stem"
    fi
    exit 0
fi

"$OUR" "$f" -o "$WORK/$stem" > /dev/null 2> "$WORK/$stem.err"
if [ $? -ne 0 ]; then
    echo "FAIL-B $stem"
    exit 0
fi

# @ir-has / @ir-lacks on the emitted .ll
while IFS= read -r s; do
    [ -z "$s" ] && continue
    if ! grep -qF "$s" "$WORK/$stem.ll"; then
        echo "FAIL-H $stem"
        exit 0
    fi
done < <(sed -n 's|.*@ir-has *||p' "$f")
while IFS= read -r s; do
    [ -z "$s" ] && continue
    if grep -qF "$s" "$WORK/$stem.ll"; then
        echo "FAIL-L $stem"
        exit 0
    fi
done < <(sed -n 's|.*@ir-lacks *||p' "$f")

exit_w="$(sed -n 's|.*@exit *||p' "$f" | head -1 | tr -d ' ')"
if [ -z "$exit_w" ]; then
    echo "PASS $stem"   # ir-shape-only case
    exit 0
fi

argline="$(sed -n 's|.*@args *||p' "$f" | head -1)"
if [ -n "$argline" ]; then
    eval "set -- $argline"
else
    set --
fi
"$WORK/$stem" "$@" > "$WORK/$stem.out" 2> "$WORK/$stem.run.err"
got=$?

if [ "$exit_w" = "-1" ]; then
    # abnormal termination: no exit code (POSIX) = signal death
    if [ $got -ge 128 ]; then echo "PASS $stem"; else echo "FAIL-E $stem $got!=-1"; fi
elif [ "$got" = "$exit_w" ]; then
    echo "PASS $stem"
else
    echo "FAIL-E $stem $got!=$exit_w"
fi
