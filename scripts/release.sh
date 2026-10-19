#!/bin/sh
# release.sh — build the release tarball our-<ver>-<triple>.tar.gz.
#
# Contents:
#   our            — the self-hosted project driver (stage-1 binary:
#                    stage-0 `ourc` compiled selfhost/our.lang)
#   ourc           — the single-file compiler (stage-0 binary — s1
#                    has no `ourc`-mode surface: lsp/--explain/
#                    `test <dir>`/`fmt <file>` are still Rust-only)
#   install.sh     — the tarball installer (copies + versioned
#                    siblings per CHANNELS.md §2; scripts/install.sh
#                    is the build-from-clone path)
#   VERSION        — release metadata (version + host triple)
#   docs           — README / LANGUAGE / PACKAGES / INSTALL /
#                    CHANNELS
#
# Both binaries embed stdlib + runtime, so the archive needs
# nothing else (INSTALL.md §4).
set -eu

cd "$(dirname "$0")/.."
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' compiler/Cargo.toml | head -1)
triple=$(rustc -vV | sed -n 's/^host: //p')
name="our-$ver-$triple"

cargo build --release -p ourc

# Stage-1 `our` — stage-0 ourc compiles the self-hosted driver.
# OUR_STDLIB/OUR_RUNTIME pin the resources the binary embeds.
rm -rf target/release/s1stage
mkdir -p target/release/s1stage
OUR_STDLIB="$PWD/stdlib" OUR_RUNTIME="$PWD/compiler/runtime" \
    target/release/ourc selfhost/our.lang -o target/release/s1stage/our

rm -rf "target/release/$name" "target/release/$name.tar.gz"
mkdir -p "target/release/$name"
cp target/release/s1stage/our "target/release/$name/our"
cp target/release/ourc "target/release/$name/ourc"
cp scripts/install-dist.sh "target/release/$name/install.sh"
chmod +x "target/release/$name/install.sh"
cp README.md LANGUAGE.md PACKAGES.md INSTALL.md CHANNELS.md \
    "target/release/$name/"
printf 'version = "%s"\ntriple = "%s"\n' "$ver" "$triple" \
    > "target/release/$name/VERSION"
tar -czf "target/release/$name.tar.gz" -C target/release "$name"
echo "built target/release/$name.tar.gz"
