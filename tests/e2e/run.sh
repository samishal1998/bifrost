#!/usr/bin/env bash
# Bifröst E2E harness (contract §12). Usage: tests/e2e/run.sh [m1|all]
#
# Phase files are listed explicitly below (C7: no glob, so collation can't reorder p13/p13a). A phase file
# `<stem>.sh` only defines functions, named after <p> = the stem before its first "_" (p04_sshfs.sh → p04):
#   setup_<p>   optional: fixtures, run after the sshd is up and before the config is written
#   config_<p>  optional: prints a TOML fragment appended to config.tmpl.toml
#   check_<p>   required: assertions through `ok` (lib.sh), which prints PASS/FAIL lines
# Every check_<p> runs, in list order, against one daemon. Any FAIL ⇒ exit 1 with the daemon log tail.
# Everything lives under T=$(mktemp -d): socket, state, config, mount root, keys. ~/.ssh and ~/machines are
# never touched. Needs bash, docker, jq, curl, pgrep, fusermount3, sshfs, ssh-keygen, ssh-keyscan; python3
# (p10) and dig (p08) for `all`. One run at a time (flock /tmp/bf-e2e.lock): the sshd publishes 127.0.0.1:2222
# as container bf-e2e-sshd.
set -euo pipefail
export LC_ALL=C
# one run at a time, host-wide; before the EXIT trap, so a refused run never cleans up the active run's
# containers. The daemon and its sshfs children inherit fd 9: a leftover of a SIGKILLed run blocks too.
exec 9>/tmp/bf-e2e.lock
flock -n 9 || { echo "another e2e run is active (or its leftovers hold /tmp/bf-e2e.lock)" >&2; exit 2; }
cd "$(dirname "${BASH_SOURCE[0]}")/../.." # repo root: cargo, and lib.sh's ${CARGO_TARGET_DIR:-$PWD/target}

m1=(p04_sshfs p05_api p06_recovery psec_hostkey p13a_adopt)
case ${1:-m1} in
  m1) list=("${m1[@]}") ;;
  all) list=("${m1[@]}" p07_tailscale p08_dns p09_rclone p10_http p12_reload p13_hardening) ;;
  p[0-9]*_* | psec_*) list=("$@") ;; # explicit phase files, e.g. `run.sh p08_dns`: a stage agent's quick loop
  *) echo "usage: $0 [m1|all|<phase> ...]" >&2; exit 2 ;;
esac
tools=(docker jq curl pgrep fusermount3 sshfs ssh-keygen ssh-keyscan)
[[ ${1:-m1} == m1 ]] || tools+=(python3 dig) # p10's inventory.py, p08's dig
for t in "${tools[@]}"; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 2; }; done

T=$(realpath "$(mktemp -d)") # realpath: the daemon canonicalizes the root, and paths are compared as text
export T E2E=$T BIFROST_SOCKET=$T/bf.sock BIFROST_STATE_DIR=$T/state BIFROST_CONFIG=$T/config.toml \
  BIFROST_LOG=debug BF_TOKEN=s3cret
source tests/e2e/lib.sh

cleanup() {
  local rc=$? m
  trap - ERR
  set +e
  stop_daemon
  kill $(jobs -p) 2>/dev/null # helpers (curl SSE, inventory.py); sshfs children are never signalled
  # lazy detach from the mount table, never a stat/readdir that a dead FUSE mount could hang
  awk -v r="$T/machines/" 'index($5, r) == 1 { print $5 }' /proc/self/mountinfo | sort -r |
    while read -r m; do fusermount3 -uz "$m"; done
  docker ps -aq --filter 'name=^bf-e2e-' | xargs -r docker rm -f >/dev/null
  if ((rc)); then
    echo "--- daemon log tail ($T/d.log)"
    tail -n 60 "$T/d.log" 2>/dev/null
  fi
  echo "scratch kept: $T" # never rm -rf: it would recurse into a mount that failed to detach
  exit "$rc"
}
trap cleanup EXIT
set -E
trap 'echo "FAIL: aborted at ${BASH_SOURCE[0]}:$LINENO: $BASH_COMMAND"' ERR

phases=()
for f in "${list[@]}"; do
  # ponytail: a missing listed phase file is skipped, not an error; upgrade: make a missing file fatal once S4 lands (the final gate requires no skip lines)
  [[ -f tests/e2e/$f.sh ]] || { echo "skip: $f.sh (not present)"; continue; }
  source "tests/e2e/$f.sh"
  p=${f%%_*}
  declare -F "check_$p" >/dev/null || { echo "FAIL: $f.sh defines no check_$p"; exit 1; }
  phases+=("$p")
done

cargo build --workspace
# Snapshot the binaries into $T: worktrees share CARGO_TARGET_DIR (plan P2), so a parallel cargo build elsewhere
# could otherwise replace target/debug/bifrostd in the middle of this run.
mkdir -p "$T/bin" && cp "$BF_BIN"/{bifrost,bifrostd,bifrost-tui} "$T/bin/" && PATH=$T/bin:$PATH
start_sshd
for p in "${phases[@]}"; do
  if declare -F "setup_$p" >/dev/null; then "setup_$p"; fi
done
{
  cat tests/e2e/config.tmpl.toml
  for p in "${phases[@]}"; do
    if declare -F "config_$p" >/dev/null; then echo; "config_$p"; fi
  done
} >"$BIFROST_CONFIG"
start_daemon
for p in "${phases[@]}"; do
  echo "== $p"
  "check_$p"
done

((FAILS == 0)) || { echo "$FAILS check(s) failed"; exit 1; }
echo "all checks passed (${phases[*]})"
