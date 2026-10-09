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
# The binaries are self-contained (stdlib + runtime are embedded,
# INSTALL.md §1) — nothing else is installed, nothing else is
# needed. Old versioned siblings are never removed.
set -eu

prefix="${1:-$HOME/.local/bin}"
cd "$(dirname "$0")/.."

cargo build --release -p ourc
ver=$(target/release/our --version | sed 's/^our //')
mkdir -p "$prefix"
for b in our ourc; do
    cp "target/release/$b" "$prefix/$b-$ver"
    cp "target/release/$b" "$prefix/$b"
done
echo "installed $ver -> $prefix/{our,ourc} (+ versioned siblings)"
case ":$PATH:" in
    *":$prefix:"*) ;;
    *) echo "note: $prefix is not on PATH" ;;
esac
