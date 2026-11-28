#!/bin/bash
# Run every static check and the Rust test suite, stopping at the first failure.
# Used both locally and by the CI workflow (.github/workflows/ci.yml), so the two stay identical.
#
#   tools/ci.sh
#
# Requires cargo-deny 0.21.0, the version the CI installs (`cargo install --locked cargo-deny@0.21.0`),
# and, once a release is tagged (`v*`), cargo-semver-checks 0.51.0
# (`cargo install --locked cargo-semver-checks@0.51.0`): see the end of this script.
# Cargo commands run with `--locked`: a Cargo.lock out of sync with Cargo.toml fails instead of
# being updated silently.
# The pty scripts of tools/ (check_inline.py, check_app.py, check_default_terminal.py) run in
# the `pty` job of the CI workflow, and the fuzzing in fuzz.yml: neither is run here (see
# CONTRIBUTING.md).

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
# The façade with fewer features: each optional layer alone, and none (the terminal layer
# only). The workspace runs above enable every feature, which hides a missing `cfg`.
for features in "" "input" "widget"; do
    step cargo clippy --locked -p tilefab --all-targets --no-default-features --features "$features" -- -D warnings
    step cargo test --locked -p tilefab --no-default-features --features "$features"
    RUSTDOCFLAGS="-D warnings" step cargo doc --locked --no-deps -p tilefab --no-default-features --features "$features"
done
# Fuzz targets (fuzz/, a crate outside the workspace): running them needs nightly, but they
# build with this toolchain, so an API change that breaks them fails here.
step cargo fmt --check --manifest-path fuzz/Cargo.toml
step cargo clippy --locked --manifest-path fuzz/Cargo.toml --all-targets -- -D warnings -W clippy::pedantic
step cargo deny --locked check
# The fuzz crate has its own Cargo.lock: libfuzzer-sys builds C++ (libFuzzer) in its build.rs.
step cargo deny --locked --manifest-path fuzz/Cargo.toml check

# The library crates, in the order of their dependencies (the applications are never published).
LIBRARIES=(-p tilefab-core -p tilefab-term -p tilefab-widget -p tilefab-input -p tilefab)

# Packaging: each library crate is built from its own archive, as `cargo publish` will do
# (files read by `include_str!`, the `LICENSE` links, versions of the path dependencies).
# Until the first release, the crates are `publish = false`, with which cargo looks for the
# workspace crates on crates.io instead of packaging them together: the check runs on a copy
# of the tracked and new files, where the root `publish = false` becomes `publish = true`.
# A tracked file deleted without `git rm` is still listed by `git ls-files --cached`: only
# the paths that exist (or are links) are copied, as `cargo package` would see them.
# It needs the network (the crates.io index). Remove the copy once `publish = false` is gone.
package_copy=$(mktemp -d)
trap 'rm -rf "$package_copy"' EXIT
git ls-files -z --cached --others --exclude-standard --deduplicate |
    while IFS= read -r -d '' path; do
        if [[ -e $path || -L $path ]]; then printf '%s\0' "$path"; fi
    done |
    tar --null -T - -cf - | tar -xf - -C "$package_copy"
sed -i.orig 's/^publish = false$/publish = true/' "$package_copy/Cargo.toml"
if ! grep -q '^publish = true$' "$package_copy/Cargo.toml"; then
    echo "error: no 'publish = false' line in the root Cargo.toml: update this step" >&2
    exit 1
fi
# The builds go to this project's target directory, to be reused from one run to the next.
package_target="$PWD/target/package-check"
(cd "$package_copy" && export CARGO_TARGET_DIR="$package_target" &&
    step cargo package --locked "${LIBRARIES[@]}")

# API compatibility with the last release (the newest `v*` tag): a breaking change needs a new
# version (0.1 → 0.2). Each crate is checked on its own: the façade's re-exports are covered
# through the crates that define them. Before the first release there is nothing to compare.
# A new toolchain may need a newer cargo-semver-checks (rustdoc JSON format): update both.
if last_release=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null); then
    if ! command -v cargo-semver-checks >/dev/null 2>&1; then
        echo "error: cargo-semver-checks is not installed (cargo install --locked cargo-semver-checks@0.51.0)" >&2
        exit 1
    fi
    step cargo semver-checks "${LIBRARIES[@]}" --baseline-rev "$last_release"
else
    printf '\n==> cargo semver-checks: skipped, no release tag (v*) yet\n'
fi

printf '\nAll checks passed.\n'
