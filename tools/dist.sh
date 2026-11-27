#!/bin/bash
# Build the demo binary to hand to someone else: the same optimized build as
# `cargo build --release`, but the paths written into the binary are rewritten, then
# checked. Without that, the panic locations of the dependencies hold their path under the
# Cargo home (`$HOME/.cargo/registry/src/…`), which shows the user name of the machine that
# built it. `cargo install` is not concerned: it builds on the machine that runs the binary.
#
#   tools/dist.sh
#
# Output: target/dist/release/widget-demo (a target directory of its own: the flags below
# differ from the usual build, which would otherwise be built again each time).
#
# Cargo's `trim-paths` profile setting would do this from Cargo.toml, but it is not stable
# yet: the paths are remapped with rustc's `--remap-path-prefix` instead. Rustc tries the
# prefixes from the last one, so the most specific comes last.

set -euo pipefail

cd "$(dirname "$0")/.."

cargo_home="${CARGO_HOME:-$HOME/.cargo}"
remap=(
    "--remap-path-prefix=$HOME=~"
    "--remap-path-prefix=$PWD=."
    "--remap-path-prefix=$cargo_home=/cargo"
)
export RUSTFLAGS="${RUSTFLAGS:-} ${remap[*]}"

cargo build --locked --release -p widget-demo --target-dir target/dist

binary=target/dist/release/widget-demo
# Read first, then search: a `strings` that fails stops the script (`set -e`) instead of
# reading as "nothing found".
text=$(strings "$binary")
if grep -qF "$HOME" <<<"$text"; then
    echo "error: $binary still holds paths under $HOME:" >&2
    # `head` closes the pipe early: ignore that, the exit code below is the one that counts.
    grep -F "$HOME" <<<"$text" | head -5 >&2 || true
    exit 1
fi
echo "$binary: built, no path under the home directory"
