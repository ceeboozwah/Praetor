#!/bin/sh
# install-dist.sh — shipped inside the release tarball as
# `install.sh`. Installs the tarball's `our`/`ourc` binaries to
# <bin-dir> (default ~/.local/bin) as BOTH the current copy and a
# versioned sibling (CHANNELS.md §2):
#
#   <bin>/our, ourc             — last installed wins
#   <bin>/our-<ver>, ourc-<ver> — stays; the dispatch pool a
#                                [project] toolchain req selects
#                                from
#
# The binaries are self-contained (stdlib + runtime are embedded,
# INSTALL.md §1) — nothing else is installed, nothing else is
# needed. Safe to re-run: installing the same version refreshes
# the copies in place; old versioned siblings are never removed.
set -eu

bin="${1:-$HOME/.local/bin}"
here=$(cd "$(dirname "$0")" && pwd)

ver=$("$here/our" --version | sed 's/^our //')
mkdir -p "$bin"
for b in our ourc; do
    cp "$here/$b" "$bin/$b-$ver"
    cp "$here/$b" "$bin/$b"
done
echo "installed $ver -> $bin/{our,ourc} (+ versioned siblings)"

# INSTALL.md §5 — warn, don't fail: single-file/project builds need
# clang; remote registry fetches need curl; everything else works.
command -v clang >/dev/null 2>&1 || \
    echo "note: clang not found on PATH — required for `our build`"
command -v curl >/dev/null 2>&1 || \
    echo "note: curl not found on PATH — required for remote registries"
case ":$PATH:" in
    *":$bin:"*) ;;
    *) echo "note: $bin is not on PATH" ;;
esac
