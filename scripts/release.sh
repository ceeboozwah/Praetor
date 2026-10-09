#!/bin/sh
# release.sh — build the release tarball our-<ver>-<triple>.tar.gz:
# the `our` + `ourc` binaries plus the user-facing docs. The binaries
# embed all resources, so the archive needs nothing else
# (INSTALL.md §4).
set -eu

cd "$(dirname "$0")/.."
ver=$(sed -n 's/^version = "\(.*\)"/\1/p' compiler/Cargo.toml | head -1)
triple=$(rustc -vV | sed -n 's/^host: //p')
name="our-$ver-$triple"

cargo build --release -p ourc
rm -rf "target/release/$name" "target/release/$name.tar.gz"
mkdir -p "target/release/$name"
cp target/release/our target/release/ourc "target/release/$name/"
cp README.md LANGUAGE.md PACKAGES.md INSTALL.md "target/release/$name/"
tar -czf "target/release/$name.tar.gz" -C target/release "$name"
echo "built target/release/$name.tar.gz"
