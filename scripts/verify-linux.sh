#!/usr/bin/env bash
# verify-linux.sh — disciplined first-run verification protocol for N58B.
# Run from the repo root on a Linux x86_64 host:
#     bash scripts/verify-linux.sh
# Each phase prints PASS/FAIL; a summary is printed at the end. Non-destructive.
set -u
cd "$(dirname "$0")/.."
ROOT="$(pwd)"
OURC="$ROOT/target/debug/ourc"
OUR="$ROOT/target/debug/our"
WORK="/tmp/mml-verify-$$"
mkdir -p "$WORK"
PASS=0; FAIL=0; SKIP=0
ok()   { PASS=$((PASS+1)); echo "  PASS  $1"; }
bad()  { FAIL=$((FAIL+1)); echo "  FAIL  $1"; }
skip() { SKIP=$((SKIP+1)); echo "  SKIP  $1"; }
TIMEOUT=""
command -v timeout >/dev/null && TIMEOUT="timeout 30"
command -v gtimeout >/dev/null && TIMEOUT="gtimeout 30"

echo "== phase 0: host + toolchain =="
# Record platform facts — a "Linux works" claim without these is too broad.
uname -a
[ -f /etc/os-release ] && grep -E '^(PRETTY_NAME|ID|VERSION_ID)=' /etc/os-release
echo "arch: $(uname -m)"
ldd --version 2>/dev/null | head -1 || echo "ldd: not glibc?"
# Mirror pipeline.rs cc(): xcrun on macOS, else first of clang/cc/gcc that answers.
CC=""
if [ "$(uname -s)" = "Darwin" ] && command -v xcrun >/dev/null; then
    CC="$(xcrun -f clang 2>/dev/null)"
fi
if [ -z "$CC" ]; then
    for c in clang cc gcc; do
        command -v "$c" >/dev/null && { CC="$c"; break; }
    done
fi
echo "resolved C driver: $CC"
"$CC" --version 2>/dev/null | head -1
command -v ld >/dev/null && ld --version 2>/dev/null | head -1
command -v cargo >/dev/null || { echo "FAIL  cargo missing"; exit 1; }
cargo build -p ourc 2>&1 | tail -1
[ -x "$OURC" ] && ok "toolchain builds" || { bad "toolchain builds"; exit 1; }

echo "== phase 1: triple + link surface =="
echo 'fn main() -> i32 { return 0; }' > "$WORK/t.lang"
"$OURC" "$WORK/t.lang" --show-llvm 2>/dev/null | grep -m2 '^target' || true
"$OURC" "$WORK/t.lang" -o "$WORK/t" && "$WORK/t" && ok "smoke binary runs" || bad "smoke binary runs"

echo "== phase 2: cargo test --workspace (workspace + fmt + lsp + project + 741 pins) =="
if cargo test --workspace 2>&1 | tail -15 | tee "$WORK/cargo.log" | grep -q "0 failed"; then
    grep -h "test result" "$WORK/cargo.log" >/dev/null 2>&1 || true
    ok "cargo test"
else
    bad "cargo test — see $WORK/cargo.log"
fi

echo "== phase 3: native pins (already in phase 2; net pair re-run explicitly) =="
for t in 514_net 515_net_errors; do
    if "$OURC" "native-tests/$t.lang" -o "$WORK/$t" && "$WORK/$t"; then
        [ $? -eq 42 ] && ok "$t (exit 42)" || ok "$t"
    else
        bad "$t"
    fi
done

echo "== phase 4: real programs — build all 16 =="
for f in programs/*.lang; do
    n="$(basename "$f" .lang)"
    if "$OURC" "$f" -o "$WORK/$n" 2>"$WORK/$n.buildlog"; then
        ok "build $n"
    else
        bad "build $n — see $WORK/$n.buildlog"
    fi
done
# hexdump is a multi-module project — the binary takes the module dir name
if "$OURC" programs/hexdump/main.lang -o "$WORK/hexdump" 2>"$WORK/hexdump.buildlog"; then
    ok "build hexdump"
else
    bad "build hexdump — see $WORK/hexdump.buildlog"
fi

echo "== phase 5: real programs — run self-terminating =="
run_expect() { # name expected-exit args...
    local n="$1" want="$2"; shift 2
    $TIMEOUT "$WORK/$n" "$@" >/dev/null 2>&1
    local rc=$?
    [ "$rc" -eq "$want" ] && ok "run $n (exit $rc)" || bad "run $n — exit $rc, want $want"
}
run_expect graph 42
run_expect lru 42
run_expect prodcons 0
run_expect psum 0
run_expect swapstress 42
run_expect workq 42
rm -f /tmp/n41idx_*.txt          # indexer's selftest writes fixed-path fixtures
run_expect indexer 42            # no-args selftest
run_expect kvserver 42           # selftest prints 'selftest ok'

echo "hello portability" > "$WORK/in.txt"
run_expect fcopy 0 "$WORK/in.txt" "$WORK/out.txt"
cmp -s "$WORK/in.txt" "$WORK/out.txt" && ok "fcopy content" || bad "fcopy content"
run_expect ugrep 0 portability "$WORK/in.txt"
run_expect wordfreq 0 "$WORK/in.txt"
echo '{"a":[1,2,3],"b":{"c":"d"}}' > "$WORK/in.json"
run_expect minijson 0 "$WORK/in.json"
printf 'key = "value"\nn = 42\n' > "$WORK/in.cfg"
run_expect cfgparse 0 "$WORK/in.cfg"
run_expect wordfreq_vec 0 "$WORK/in.txt"
"$WORK/hexdump" "$ROOT/programs/indexer.lang" | head -1 | grep -q '^00000000' \
    && ok "run hexdump" || bad "run hexdump"

echo "== phase 6: live socket servers =="
"$WORK/echo" &>/dev/null & EPID=$!
sleep 1
if command -v nc >/dev/null; then
    echo "ping" | nc -w 3 127.0.0.1 8080 2>/dev/null | grep -q ping \
        && ok "echo server round-trip" || bad "echo server round-trip"
elif (exec 3<>/dev/tcp/127.0.0.1/8080) 2>/dev/null; then
    (exec 3<>/dev/tcp/127.0.0.1/8080 && echo ping >&3 && head -1 <&3 | grep -q ping) \
        && ok "echo server round-trip" || bad "echo server round-trip"
else
    skip "no TCP client for echo test"
fi
kill $EPID 2>/dev/null

"$WORK/httpd" 18080 &>/dev/null & HPID=$!
sleep 1
if curl -sf -m 3 http://127.0.0.1:18080/ >/dev/null 2>&1 || \
   curl -s -m 3 -o /dev/null -w '%{http_code}' http://127.0.0.1:18080/ 2>/dev/null | grep -qE '^(200|400|404)$'; then
    ok "httpd bind/listen/answer"
else
    bad "httpd bind/listen/answer"
fi
kill $HPID 2>/dev/null

echo "== phase 7: project tool =="
cd "$WORK" && "$OUR" new proj 2>/dev/null && cd proj
"$OUR" check  && "$OUR" test && "$OUR" build && "$OUR" run >/dev/null 2>&1 \
    && ok "our new → check/test/build/run" || bad "project tool flow"
cd "$ROOT"

echo "== phase 8: debug info (non-blocking) =="
if command -v lldb >/dev/null || command -v gdb >/dev/null; then
    "$OURC" "$WORK/t.lang" -g -o "$WORK/t.dbg" && ok "-g build links"
    # interactive breakpoint/variable checks are manual; DWARF presence is the gate
    if command -v readelf >/dev/null; then
        readelf -S "$WORK/t.dbg" | grep -q '.debug_info' && ok "DWARF present" || bad "DWARF missing"
    elif command -v dwarfdump >/dev/null; then
        dwarfdump "$WORK/t.dbg" >/dev/null 2>&1 && ok "DWARF present" || bad "DWARF missing"
    else
        skip "no readelf/dwarfdump — check DWARF manually"
    fi
else
    skip "no debugger — DWARF check deferred"
fi

echo ""
echo "==== summary: $PASS passed, $FAIL failed, $SKIP skipped (artifacts in $WORK) ===="
[ $FAIL -eq 0 ]
