# p10 (all): HTTP inventory discovery against tests/e2e/inventory.py on 127.0.0.1:18080 (contract §12).
# inv.json (synthetic): inv-01 is the real machine (the sshd on 127.0.0.1:2222); inv-bad publishes an ssh option
# as its only address, inv-host as its host next to a valid address (A20: no fallback), and "../x" a traversal
# name. All three must be skipped with a warning. Then the server's token flips: the daemon gets HTTP 401 and
# keeps serving its last view (inv-01 stays mounted), and recovers when the token is restored.

# p10_serve : inventory.py as a job of the run's shell (cleanup's `kill $(jobs -p)` stops it), never inside
# `ok` (a subshell); 9>&-: the run lock must not leak into it. BF_TOKEN in the environment is what it accepts.
p10_serve() {
  python3 "$E2E_DIR/inventory.py" 127.0.0.1:18080 "$T/inv.json" >>"$T/inv.log" 2>&1 9>&- &
  P10_PID=$!
  wait_until 10 curl -s -o /dev/null http://127.0.0.1:18080/
}
p10_stop() {
  kill "$P10_PID" 2>/dev/null || true
  wait "$P10_PID" 2>/dev/null || true
}

setup_p10() {
  cat >"$T/inv.json" <<'EOF'
{"machines": [
  {"name": "inv-01", "id": "i-0001", "addresses": ["127.0.0.1"], "port": 2222, "online": true,
   "user": "bf", "path": "/home/bf", "driver": "rclone", "metadata": {"tags": ["e2e"], "env": "test"}},
  {"name": "inv-bad", "addresses": ["-oProxyCommand=x"], "port": 2222},
  {"name": "inv-host", "host": "-oProxyCommand=x", "addresses": ["127.0.0.1"], "port": 2222},
  {"name": "../x", "addresses": ["127.0.0.1"], "port": 2222}
]}
EOF
  p10_serve
}

config_p10() {
  cat <<'TOML'
[[discovery]]
type = "http"
name = "inventory"
url = "http://127.0.0.1:18080/bifrost/v1/machines"
headers = { Authorization = "Bearer ${BF_TOKEN}" }

[discovery.filter]
include_names = ["inv-*"]

[discovery.mount]
honor_hints = true
TOML
}

# p10_pjq FILTER : jq FILTER over the inventory ProviderDto is true
p10_pjq() { bifrost --json status | jq -e ".providers[] | select(.name == \"inventory\") | $1" >/dev/null; }
# p10_mjq ID FILTER : jq FILTER over the MachineDto is true
p10_mjq() { bifrost --json machines | jq -e --arg id "$1" ".[] | select(.id == \$id) | $2" >/dev/null; }
p10_no_machine() { bifrost --json machines | jq -e --arg id "$1" 'all(.[]; .id != $id)' >/dev/null; }
# p10_warned TEXT : a WARN line of d.log contains TEXT (fixed string)
p10_warned() { grep -F -- "$1" "$T/d.log" | grep -q ' WARN '; }

check_p10() {
  local mp=$T/machines/inv-01 allowed='.verdict == "allowed (inventory.filter.include)"'
  ok "p10: inv-01 mounted within 40s" wait_until 40 state_is inv-01 mounted
  ok "p10: hello.txt visible" on_fuse test -f "$mp/hello.txt"
  ok "p10: inv-01 allowed (inventory.filter.include)" p10_mjq inv-01 "$allowed"
  ok "p10: inv-01 fields from the inventory" p10_mjq inv-01 \
    '.address == "127.0.0.1" and .port == 2222 and .online == true and .tags == ["e2e"] and .metadata.env == "test"'
  ok "p10: user/path hints used (honor_hints)" mjq inv-01 '.remote == "bf@127.0.0.1:/home/bf"'
  ok "p10: provider ok with exactly 1 machine" p10_pjq '.machines == 1 and .last_error == null'
  ok "p10: inv-bad absent" p10_no_machine inv-bad
  ok "p10: inv-host absent" p10_no_machine inv-host
  ok "p10: ProxyCommand address warned" p10_warned 'machines[1]: address dropped: invalid host "-oProxyCommand=x"'
  ok "p10: address-less entry skipped" p10_warned 'machines[1]: skipped: no valid host or address'
  ok "p10: ProxyCommand host warned (A20)" p10_warned 'machines[2]: skipped: invalid host "-oProxyCommand=x"'
  ok "p10: ../x name warned" p10_warned 'machines[3]: skipped: invalid name "../x"'
  ok "p10: the token never reaches the log" not grep -q "$BF_TOKEN" "$T/d.log"

  p10_stop && BF_TOKEN=wrong p10_serve # the daemon's Bearer s3cret is now the wrong token
  ok "p10: wrong token → provider last_error HTTP 401" wait_until 20 p10_pjq '.last_error == "HTTP 401"'
  sleep 10 # > 3 × discovery_interval (9s): an unfrozen view would have expired by now
  ok "p10: inv-01 still listed (last good view served)" p10_mjq inv-01 "$allowed"
  ok "p10: inv-01 stays mounted" state_is inv-01 mounted
  ok "p10: hello.txt still visible" on_fuse test -f "$mp/hello.txt"

  p10_stop && p10_serve
  ok "p10: token restored → provider ok" wait_until 20 p10_pjq '.last_error == null and .machines == 1'
  ok "p10: inv-01 mounted" state_is inv-01 mounted
}
