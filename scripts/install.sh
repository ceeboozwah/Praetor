#!/bin/sh
# install.sh [prefix] — build `our` + `ourc` in release mode and
# install them to <prefix>/bin (default ~/.local/bin) as BOTH the
# current copy and a versioned sibling (CHANNELS.md §2):
#
#   <prefix>/our, ourc           — last installed wins
#   <prefix>/our-<ver>, ourc-<ver> — stays; the dispatch pool a
#                                  [project] toolchain req selects
#                                  from
#
# `our` installed here is the SAME binary the release tarball ships:
# the self-hosted Stage-1 driver — Stage-0 `ourc` compiling
# selfhost/our.lang — NOT the Rust Stage-0 driver at
# compiler/src/bin/our.rs (that one stays a dev-loop artifact in
# target/release and is never installed). `ourc` is the Stage-0
# bootstrap/reference binary.
#
# The binaries are self-contained (stdlib + runtime are embedded,
# INSTALL.md §1) — nothing else is installed, nothing else is
# needed. Old versioned siblings are never removed.
set -eu

prefix="${1:-$HOME/.local/bin}"
cd "$(dirname "$0")/.."

cargo build --release -p ourc

# Windows (Git Bash) produces .exe names; POSIX does not.
exe=""
[ -f "target/release/ourc.exe" ] && exe=".exe"

# Stage-1 `our` — identical inputs to scripts/release.sh:
# OUR_STDLIB/OUR_RUNTIME pin the resources the binary embeds.
rm -rf target/release/s1stage
mkdir -p target/release/s1stage
OUR_STDLIB="$PWD/stdlib" OUR_RUNTIME="$PWD/compiler/runtime" \
    "target/release/ourc$exe" selfhost/our.lang \
    -o target/release/s1stage/our

ver=$(target/release/s1stage/our$exe --version | sed 's/^our //')
mkdir -p "$prefix"
cp "target/release/s1stage/our$exe" "$prefix/our-$ver$exe"
cp "target/release/s1stage/our$exe" "$prefix/our$exe"
cp "target/release/ourc$exe" "$prefix/ourc-$ver$exe"
cp "target/release/ourc$exe" "$prefix/ourc$exe"
echo "installed $ver -> $prefix/{our,ourc}$exe (+ versioned siblings)"
case ":$PATH:" in
    *":$prefix:"*) ;;
    *) echo "note: $prefix is not on PATH" ;;
esac
