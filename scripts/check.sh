#!/usr/bin/env bash
# OS: Linux
# TIME: 2026-11-14
# DOC: Offline acceptance gate for dukascopy-data-reader.
#      Format, check, clippy, tests, doctests, docs and every feature set.
#      Python tests run only when NumPy is importable.
# USAGE:
#   ./scripts/check.sh
#
# Q:
#   Q1: Real archive smoke is not part of this script; it needs the mirror at
#       the root configured in config/00.smoke.toml. Run STEP.md module 6.
#   Q2: cargo-deny/cargo-audit are not installed here; run them explicitly.

set -Eeuo pipefail

cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo check --workspace --no-default-features --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
cargo test --workspace --doc --locked
RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps --locked
cargo check --workspace --all-features --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings

if python3 -c 'import numpy' >/dev/null 2>&1; then
    cargo test --workspace --all-features --locked
else
    printf '%s\n' \
        'skip: Python feature tests (NumPy is not importable by python3)' >&2
fi
