#!/bin/sh
# release.sh — build the release archive our-<ver>-<triple>.<ext>.
#
# Contents:
#   our            — the self-hosted project driver (stage-1 binary:
#                    stage-0 `ourc` compiled selfhost/our.lang)
#   ourc           — the single-file compiler (stage-0 binary — s1
#                    has no `ourc`-mode surface: lsp/--explain/
#                    `test <dir>`/`fmt <file>` are still Rust-only)
#   install.sh     — the POSIX tarball installer (copies + versioned
#                    siblings per CHANNELS.md §2; scripts/install.sh
#                    is the build-from-clone path)
#   install.ps1    — the Windows .zip installer (same semantics,
#                    PowerShell); shipped alongside install.sh so
#                    Git-Bash users keep working too
#   VERSION        — release metadata (version + host triple)
#   docs           — README / LANGUAGE / PACKAGES / INSTALL /
#                    CHANNELS / CHANGELOG / RELEASES
#   licenses       — LICENSE-MIT + LICENSE-APACHE (dual-licensed
#                    as of 0.2.0)
#
# Both binaries embed stdlib + runtime, so the archive needs
# nothing else (INSTALL.md §4).
#
# Archive format: .tar.gz on POSIX (deterministic — sorted members,
# clamped mtimes, gzip -n); .zip on Windows via Compress-Archive.
set -eu

cd "$(dirname "$0")/.."
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' compiler/Cargo.toml | head -1)
triple=$(rustc -vV | sed -n 's/^host: //p')
name="our-$ver-$triple"

cargo build --release -p ourc

# Windows (Git Bash) produces .exe names; POSIX does not.
exe=""
case "$triple" in *windows*) exe=".exe";; esac

# Stage-1 `our` — stage-0 ourc compiles the self-hosted driver.
# OUR_STDLIB/OUR_RUNTIME pin the resources the binary embeds.
rm -rf target/release/s1stage
mkdir -p target/release/s1stage
OUR_STDLIB="$PWD/stdlib" OUR_RUNTIME="$PWD/compiler/runtime" \
    "target/release/ourc$exe" selfhost/our.lang \
    -o target/release/s1stage/our

rm -rf "target/release/$name" "target/release/$name.tar.gz" \
    "target/release/$name.zip"
mkdir -p "target/release/$name"
cp "target/release/s1stage/our$exe" "target/release/$name/our$exe"
cp "target/release/ourc$exe" "target/release/$name/ourc$exe"
cp scripts/install-dist.sh "target/release/$name/install.sh"
chmod +x "target/release/$name/install.sh"
cp scripts/install-dist.ps1 "target/release/$name/install.ps1"
cp README.md LANGUAGE.md PACKAGES.md INSTALL.md CHANNELS.md \
    CHANGELOG.md RELEASES.md LICENSE-MIT LICENSE-APACHE \
    "target/release/$name/"
printf 'version = "%s"\ntriple = "%s"\n' "$ver" "$triple" \
    > "target/release/$name/VERSION"

if [ -n "$exe" ]; then
    # Windows: .zip via PowerShell Compress-Archive.
    powershell -NoProfile -Command \
        "Compress-Archive -Path 'target/release/$name' -DestinationPath 'target/release/$name.zip' -Force"
    echo "built target/release/$name.zip"
else
    # Deterministic tarball: sorted member order (readdir order is
    # not stable), clamped member mtimes, gzip -n (no stored
    # timestamp). Two builds of the same commit must produce
    # byte-identical bytes.
    (cd target/release && find "$name" -print | LC_ALL=C sort \
        | tr '\n' '\0' | xargs -0 touch -t 202001010000)
    (cd target/release && find "$name" -mindepth 1 -print \
        | LC_ALL=C sort | tar -cf - --files-from -) \
        | gzip -n > "target/release/$name.tar.gz"
    echo "built target/release/$name.tar.gz"
fi

# QA-005 — integrity evidence. Each platform's build writes its own
# SHA256SUMS fragment in `shasum -a 256 -c` format; the release
# manager concatenates the fragments into the shipped SHA256SUMS
# asset. (Git for Windows ships `shasum` in usr/bin.)
arc="$name.tar.gz"; [ -n "$exe" ] && arc="$name.zip"
(cd target/release && shasum -a 256 "$arc" > "SHA256SUMS.$triple")
echo "wrote target/release/SHA256SUMS.$triple — concat all platform"
echo "fragments into the shipped SHA256SUMS (RELEASES.md §Integrity)"
