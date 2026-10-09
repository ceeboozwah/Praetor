#!/bin/sh
# install.sh [prefix] — build `our` + `ourc` in release mode and copy
# them to <prefix>/bin (default ~/.local/bin). The binaries are
# self-contained (stdlib + runtime are embedded, INSTALL.md §1) —
# nothing else is installed, nothing else is needed.
set -eu

prefix="${1:-$HOME/.local/bin}"
cd "$(dirname "$0")/.."

cargo build --release -p ourc
mkdir -p "$prefix"
cp target/release/our target/release/ourc "$prefix/"
echo "installed $("$prefix/our" --version), $("$prefix/ourc" --version) -> $prefix"
case ":$PATH:" in
    *":$prefix:"*) ;;
    *) echo "note: $prefix is not on PATH" ;;
esac
