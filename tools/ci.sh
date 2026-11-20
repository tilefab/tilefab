#!/bin/bash
# Run every static check and the Rust test suite, stopping at the first failure.
# Used both locally and by the CI workflow (.github/workflows/ci.yml), so the two stay identical.
#
#   tools/ci.sh
#
# Requires cargo-deny (`cargo install --locked cargo-deny`).
# The pty scripts of tools/ (check_inline.py, check_app.py) are not run here: see README.md.

set -euo pipefail

cd "$(dirname "$0")/.."

step() {
    printf '\n==> %s\n' "$*"
    "$@"
}

if ! command -v cargo-deny >/dev/null 2>&1; then
    echo "error: cargo-deny is not installed (cargo install --locked cargo-deny)" >&2
    exit 1
fi

step cargo fmt --check
step cargo clippy --all-targets --all-features -- -D warnings
step cargo test --all-features
# Release build: code under `cfg(debug_assertions)` (the hidden `panic` command) must
# compile cleanly without it, and its release-only tests must pass.
step cargo clippy --release --all-targets --all-features -- -D warnings
step cargo test --release --all-features
RUSTDOCFLAGS="-D warnings" step cargo doc --no-deps
step cargo deny check

printf '\nAll checks passed.\n'
