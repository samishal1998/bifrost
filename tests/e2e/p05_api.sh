# p05 (m1): CLI + API over the socket, socket mode, single instance, exit 3, SSE.

p05_json() { bifrost --json "$1" | jq -e . >/dev/null; }
p05_mode() { [[ $(stat -c %a "$BIFROST_SOCKET") == 600 ]]; }

p05_exit3() {
  local rc=0
  BIFROST_SOCKET=$T/nope bifrost status || rc=$?
  ((rc == 3))
}

p05_second() {
  local rc=0
  timeout 10 bifrostd >"$T/second.log" 2>&1 || rc=$?
  cat "$T/second.log"
  ((rc != 0 && rc != 124)) && grep -q 'already running' "$T/second.log"
}

p05_sse() {
  sed -n 's/^data: //p' "$T/sse.txt" |
    jq -en '[inputs | select(.event.type? == "UnmountStarted" and .event.mount? == "static1")] | length > 0'
}

check_p05() {
  local c cpid
  for c in status machines mounts drivers; do
    ok "p05: bifrost $c exits 0" bifrost "$c"
    ok "p05: bifrost --json $c parses" p05_json "$c"
  done
  ok "p05: socket mode 600" p05_mode
  ok "p05: second bifrostd exits non-zero with 'already running'" p05_second
  ok "p05: no daemon at the socket => exit 3" p05_exit3

  timeout 8 curl -sN --unix-socket "$BIFROST_SOCKET" http://bifrost/v1/events >"$T/sse.txt" 2>&1 &
  cpid=$!
  sleep 1 # let curl subscribe
  ok "p05: bifrost unmount static1 (SSE)" bifrost unmount static1
  ok "p05: bifrost mount static1 (SSE)" bifrost mount static1
  wait "$cpid" || true # timeout ends curl: exit 124
  ok "p05: SSE frame 'event: UnmountStarted'" grep -q '^event: UnmountStarted' "$T/sse.txt"
  ok "p05: SSE data is the static1 UnmountStarted record" p05_sse
  ok "p05: static1 mounted again" wait_until 20 state_is static1 mounted
}
