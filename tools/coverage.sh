#!/bin/bash
# Line coverage of the Rust code, unit tests and pty harness combined.
#
#   tools/coverage.sh            # summary per file
#   tools/coverage.sh --unit     # unit tests only (no pty, faster)
#   tools/coverage.sh --html     # also write an HTML report to target/llvm-cov/html
#
# Requires cargo-llvm-cov and llvm-tools (once:
# `rustup component add llvm-tools-preview && cargo install --locked cargo-llvm-cov`)
# and, unless --unit, the Python venv of tools/ (see README.md).
#
# main.rs, headless.rs and the signal handling run only in the real binary: unit tests
# alone under-report them. The binaries built here are instrumented and replace the usual
# ones in target/debug (the harness runs those). Cargo would not rebuild them on its own
# (it ignores the compiler wrapper), and they would leave `.profraw` files wherever they
# run: on exit, this script removes the workspace packages' build artifacts, so the next
# `cargo build` produces plain binaries again.

set -euo pipefail

cd "$(dirname "$0")/.."

unit_only=false
report_args=(--summary-only)
for arg in "$@"; do
    case "$arg" in
        --unit) unit_only=true ;;
        --html) report_args=(--html) ;;
        *)
            echo "error: unknown argument: $arg" >&2
            exit 2
            ;;
    esac
done

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
    echo "error: cargo-llvm-cov is not installed (cargo install --locked cargo-llvm-cov)" >&2
    exit 1
fi

# `source <(...)` does not work with the bash 3.2 shipped with macOS: use eval.
eval "$(cargo llvm-cov show-env --export-prefix 2>/dev/null)"
cargo llvm-cov clean --workspace
# Whatever happens, do not leave instrumented binaries in target/ (see the header).
trap 'cargo clean -q -p tilefab -p tilefab-term -p tilefab-core -p tilefab-demo -p tilefab-input -p tilefab-widget' EXIT

echo "==> unit tests"
cargo test -q --all-features >/dev/null

if ! $unit_only; then
    echo "==> pty harness (instrumented binaries)"
    cargo build -q --all-features
    cargo build -q --all-features --example inline
    tools/.venv/bin/python tools/check_app.py
    tools/.venv/bin/python tools/check_inline.py
fi

echo "==> report"
cargo llvm-cov report "${report_args[@]}"
