#!/usr/bin/env bash
# The gate: fmt, clippy -D warnings, tests. CI (.github/workflows/ci.yml) runs this too.
# Uses cargo's normal target dir unless CARGO_TARGET_DIR is set.
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
