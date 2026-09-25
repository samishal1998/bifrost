#!/usr/bin/env bash
# The gate: fmt, clippy -D warnings, tests.
# ponytail: no CI yaml until a git remote exists; this script is the gate (PRD §29 P0 CI, B3); upgrade: add a CI workflow calling scripts/check.sh once a remote exists
set -euo pipefail
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/home/samimishal/projects/rust/bifrost-target}"
cd "$(dirname "$0")/.."
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
