# E2E helpers (contract §12). Sourced by run.sh and by drivers of the #[ignore] mount tests.
# Needs: $T (a mktemp -d scratch dir). Never touches ~/.ssh or ~/machines.

# Binaries come from the shared cargo target dir (P2).
BF_BIN=${BF_BIN:-${CARGO_TARGET_DIR:-$PWD/target}/debug}
PATH=$BF_BIN:$PATH
# every CLI call is bounded (mount/unmount poll ≤60s): a wedged daemon gives FAIL + log tail, not a hung gate.
# `timeout` runs the binary from PATH, so prefix assignments (VAR=x bifrost ...) and exit codes pass through.
bifrost() { timeout 75 bifrost "$@"; }
E2E_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

# wait_until SECS CMD... : run CMD every 0.5s until it succeeds; 1 after SECS.
wait_until() {
  local end=$((SECONDS + $1)); shift
  until "$@" >/dev/null 2>&1; do
    ((SECONDS < end)) || { echo "timeout: $*" >&2; return 1; }
    sleep 0.5
  done
}

# mstate ID : the mount's state ("mounted", "failed", ...), empty if unknown.
mstate() { bifrost --json mounts | jq -r --arg id "$1" '.[] | select(.id == $id) | .state'; }

# mpid ID : the mount's child pid, the ONLY way tests pick a process to kill (A13: never pkill -f).
mpid() { bifrost --json mounts | jq -r --arg id "$1" '.[] | select(.id == $id) | .pid // empty'; }

# is_mounted PATH : PATH is a mount point (exact field match, not a substring grep).
is_mounted() { awk -v p="$1" '$5 == p { f = 1 } END { exit !f }' /proc/self/mountinfo; }

# start_daemon : bifrostd in the background (log appended to $T/d.log), waits until it answers.
# Refuses unless config, state and socket all live under $T: never the user's real config or ~/machines.
start_daemon() {
  : "${T:?}" "${BIFROST_CONFIG:?must point under \$T}" "${BIFROST_STATE_DIR:?}" "${BIFROST_SOCKET:?}"
  local v
  for v in "$BIFROST_CONFIG" "$BIFROST_STATE_DIR" "$BIFROST_SOCKET"; do
    [[ $v == "$T"/* ]] || { echo "start_daemon: $v is outside \$T" >&2; return 1; }
  done
  [[ -f $BIFROST_CONFIG ]] || { echo "start_daemon: $BIFROST_CONFIG missing (daemon would default to ~/machines)" >&2; return 1; }
  bifrostd >>"$T/d.log" 2>&1 9>&- &   # 9>&-: the run lock must not leak into the daemon or its sshfs children
  DPID=$!
  wait_until 10 bifrost daemon status
}

# stop_daemon : SIGTERM and reap; mounts stay (adoption on the next start).
stop_daemon() {
  [[ -n ${DPID:-} ]] || return 0
  kill -TERM "$DPID" 2>/dev/null || true
  local i # bounded: a daemon that ignores SIGTERM gets SIGKILL after 20s instead of hanging the run
  for ((i = 0; i < 40; i++)); do kill -0 "$DPID" 2>/dev/null || break; sleep 0.5; done
  kill -KILL "$DPID" 2>/dev/null || true
  wait "$DPID" 2>/dev/null || true
  DPID=
}

# start_sshd : throwaway OpenSSH (tests/e2e/sshd) on 127.0.0.1:2222 and 127.0.0.2:2222, a scratch
# ed25519 key, a scratch known_hosts and $T/ssh_config with strict host-key checking.
# Exports BIFROST_E2E_SSH=host:port:user:ssh_config for `cargo test -p bifrost-mount -- --ignored`.
start_sshd() {
  : "${T:?T must be a mktemp -d dir}"
  docker build -q -t bifrost-e2e-sshd "$E2E_DIR/sshd" >/dev/null
  [[ -f $T/id ]] || ssh-keygen -t ed25519 -N '' -q -f "$T/id"
  docker rm -f bf-e2e-sshd >/dev/null 2>&1 || true
  docker run -d --name bf-e2e-sshd -p 127.0.0.1:2222:22 -p 127.0.0.2:2222:22 \
    -e PUBKEY="$(cat "$T/id.pub")" bifrost-e2e-sshd >/dev/null
  wait_until 20 ssh-keyscan -p 2222 127.0.0.1
  ssh-keyscan -p 2222 127.0.0.1 127.0.0.2 >"$T/known_hosts" 2>/dev/null
  cat >"$T/ssh_config" <<EOF
Host *
  IdentityFile $T/id
  IdentitiesOnly yes
  UserKnownHostsFile $T/known_hosts
  GlobalKnownHostsFile /dev/null
  StrictHostKeyChecking yes
  CheckHostIP no
EOF
  export BIFROST_E2E_SSH=127.0.0.1:2222:bf:$T/ssh_config
}

# stop_sshd : remove the container (host keys live in the image, so a restart keeps known_hosts valid).
stop_sshd() { docker rm -f bf-e2e-sshd >/dev/null 2>&1 || true; }

# --- assertions for the run.sh phases ---

FAILS=0
# ok DESC CMD... : run CMD, print "PASS: DESC" or "FAIL: DESC" (plus CMD's last output lines); a FAIL is
# counted and the run goes on. CMD runs in a subshell, so it can't change shell state (DPID etc.).
ok() {
  local d=$1 out; shift
  if out=$("$@" 2>&1); then
    echo "PASS: $d"
  else
    echo "FAIL: $d"
    [[ -z $out ]] || tail -n 5 <<<"$out" | sed 's/^/  | /'
    FAILS=$((FAILS + 1))
  fi
}
not() { ! "$@"; }

# state_is ID STATE : the mount's state is STATE.
state_is() { [[ $(mstate "$1") == "$2" ]]; }

# mjq ID FILTER : jq FILTER over the mount's MountDto is true, e.g. `mjq static1 .held`.
mjq() { bifrost --json mounts | jq -e --arg id "$1" ".[] | select(.id == \$id) | $2" >/dev/null; }

# events SEQ FILTER CMP : the count of status events with seq > SEQ whose .event matches FILTER satisfies CMP
# ("> 0", "== 0"). The ring holds the running daemon instance's last 200 events.
events() {
  bifrost --json status |
    jq -e --argjson s "$1" "[.events[] | select(.seq > \$s) | .event | select($2)] | length $3" >/dev/null
}
last_seq() { bifrost --json status | jq '[.events[].seq] | max // 0'; }

# mnt_src PATH : "<fstype> <source>" of the mount at PATH (the mountinfo fields after the "-" separator).
mnt_src() {
  awk -v p="$1" '$5 == p { for (i = 7; i <= NF; i++) if ($i == "-") { print $(i + 1), $(i + 2); exit } }' \
    /proc/self/mountinfo
}

# gone PATH : not a mount point, and the directory is removed.
gone() { ! is_mounted "$1" && [[ ! -e $1 ]]; }

# sig SIG PID : kill -SIG PID, only for a plain positive pid (an empty `mpid` never becomes a bare `kill`).
sig() { [[ $2 =~ ^[1-9][0-9]*$ ]] && kill "-$1" "$2"; }

# on_fuse CMD... : CMD on a FUSE path, SIGKILLed after 5s so a hung mount can't hang the run.
on_fuse() { timeout -s KILL 5 "$@"; }
