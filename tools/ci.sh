#!/bin/bash
# Run every static check and the Rust test suite, stopping at the first failure.
# Used both locally and by the CI workflow (.github/workflows/ci.yml), so the two stay identical.
#
#   tools/ci.sh
#
# Requires cargo-deny 0.21.0, the version the CI installs (`cargo install --locked cargo-deny@0.21.0`).
# Cargo commands run with `--locked`: a Cargo.lock out of sync with Cargo.toml fails instead of
# being updated silently.
# The pty scripts of tools/ (check_inline.py, check_app.py) and the fuzzing itself are not run
# here: see README.md.

set -euo pipefail

cd "$(dirname "$0")/.."

step() {
    printf '\n==> %s\n' "$*"
    "$@"
}

if ! command -v cargo-deny >/dev/null 2>&1; then
    echo "error: cargo-deny is not installed (cargo install --locked cargo-deny@0.21.0)" >&2
    exit 1
fi

step cargo fmt --check
step cargo clippy --locked --all-targets --all-features -- -D warnings
step cargo test --locked --all-features
# Release build: code under `cfg(debug_assertions)` (the hidden `panic` command) must
# compile cleanly without it, and its release-only tests must pass. The tests use the `ci`
# profile (Cargo.toml): `release` with overflow checks, so an overflow panics there too.
step cargo clippy --locked --release --all-targets --all-features -- -D warnings
step cargo test --locked --profile ci --all-features
RUSTDOCFLAGS="-D warnings" step cargo doc --locked --no-deps
# Fuzz targets (fuzz/, a crate outside the workspace): running them needs nightly, but they
# build with this toolchain, so an API change that breaks them fails here.
step cargo fmt --check --manifest-path fuzz/Cargo.toml
step cargo clippy --locked --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings -W clippy::pedantic
step cargo deny --locked check

printf '\nAll checks passed.\n'
