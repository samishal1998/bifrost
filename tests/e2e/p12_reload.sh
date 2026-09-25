# p12 (all): config hot reload (contract §8, §12). The poller applies an edit once two reads 2s apart agree;
# SIGHUP applies at once. box1 (auto → sshfs by the default order) is this phase's own mount. check_p12 edits
# $BIFROST_CONFIG and restores the original at the end, so later phases (p13) see the machines they expect.

# p12_box NAME REMOTE : a static machine on the e2e sshd, driver auto
p12_box() {
  printf '\n[[machines]]\nname = "%s"\nhost = "127.0.0.1"\nport = 2222\nuser = "bf"\nremote = "%s"\n' "$1" "$2"
}

config_p12() { p12_box box1 /home/bf; }

# p12_pids : [id, pid] of every mounted mount. p12_same PIDS : each of them is still mounted with the same pid
# (never remounted); mounts added since don't matter
p12_pids() { bifrost --json mounts | jq -c '[.[] | select(.state == "mounted") | [.id, .pid]]'; }
p12_same() {
  bifrost --json mounts |
    jq -e --argjson was "$1" '[.[] | select(.state == "mounted") | [.id, .pid]] as $now | all($was[]; IN($now[]))' \
      >/dev/null
}
# p12_status FILTER : jq FILTER over StatusDto is true
p12_status() { bifrost --json status | jq -e "$1" >/dev/null; }
p12_src() { [[ $(mnt_src "$T/machines/$1") == "fuse.$2 bifrost:$1@"* ]]; }
p12_ms() { echo $(($(date +%s%N) / 1000000)); }
# p12_hup FILE : install FILE as the config and SIGHUP the daemon; true if it applied within 1.5s. The poll alone
# can't: it needs two identical reads 2s apart.
p12_hup() {
  local seq end
  seq=$(last_seq)
  end=$(($(p12_ms) + 1500))
  cp "$1" "$BIFROST_CONFIG" && sig HUP "$DPID" || return 1
  until events "$seq" '.type == "ConfigurationReloaded" and .ok' '> 0'; do
    (($(p12_ms) < end)) || return 1
    sleep 0.1
  done
}
p12_restored() {
  p12_status 'all(.machines[]; .id != "box2" and .id != "box3") and .config_errors == []' &&
    gone "$T/machines/box2" && gone "$T/machines/box3"
}

check_p12() {
  local cfg=$BIFROST_CONFIG pids seq reloaded='.type == "ConfigurationReloaded" and .ok'
  cp "$cfg" "$T/p12.orig"
  # 40s: in `all`, p06 restarted the sshd under every mount and p13a restarted the daemon
  ok "p12: box1 mounted within 40s" wait_until 40 state_is box1 mounted
  ok "p12: box1 auto → sshfs" mjq box1 '.driver == "sshfs"'

  seq=$(last_seq)
  p12_box box2 /home/bf/data >>"$cfg"
  cp "$cfg" "$T/p12.good"
  ok "p12: appended box2 mounted within 10s" wait_until 10 state_is box2 mounted
  ok "p12: ConfigurationReloaded ok:true" events "$seq" "$reloaded" '> 0'
  ok "p12: box2 w.txt visible" on_fuse test -f "$T/machines/box2/w.txt"

  pids=$(p12_pids)
  ok "p12: box1 and box2 have pids" jq -e 'map(select(.[0] == "box1" or .[0] == "box2") | .[1]) | length == 2 and all(. > 0)' <<<"$pids"
  seq=$(last_seq)
  { cat "$T/p12.good"; echo '[[machines'; } >"$cfg"
  ok "p12: broken TOML → config_errors within 10s" wait_until 10 p12_status '.config_errors | length > 0'
  ok "p12: ConfigurationReloaded ok:false" events "$seq" '.type == "ConfigurationReloaded" and (.ok | not)' '> 0'
  sleep 3 # a health tick and a pass on the kept config
  ok "p12: broken config: every mount untouched (same pids)" p12_same "$pids"
  cp "$T/p12.good" "$cfg"
  ok "p12: restored → config_errors cleared within 10s" wait_until 10 p12_status '.config_errors == []'
  ok "p12: restored: every mount untouched (same pids)" p12_same "$pids"
  ok "p12: nothing unmounted while broken" events "$seq" '.type == "UnmountStarted"' '== 0'

  # SIGHUP, with the edit that swaps auto_order (into the template's [mount] table: a second one would be a dup)
  sed '/^\[mount\]$/a auto_order = ["rclone", "sshfs"]' "$T/p12.good" >"$T/p12.rc"
  seq=$(last_seq)
  ok "p12: SIGHUP applies an edit within 1.5s" p12_hup "$T/p12.rc"
  ok "p12: auto driver is now rclone" p12_status '.auto_driver == "rclone" and .config_errors == []'
  sleep 6 # > reconcile_interval: a fallback pass under the new order
  ok "p12: auto_order swap remounted nothing (sticky, same pids)" p12_same "$pids"
  ok "p12: nothing unmounted after the swap" events "$seq" '.type == "UnmountStarted"' '== 0'
  ok "p12: box1 still sshfs" mjq box1 '.driver == "sshfs"'
  p12_box box3 /home/bf >>"$cfg"
  ok "p12: new auto box3 mounted within 20s" wait_until 20 state_is box3 mounted
  ok "p12: box3 auto → rclone" mjq box3 '.driver == "rclone"'
  ok "p12: box3 mountinfo fuse.rclone" p12_src box3 rclone
  ok "p12: box3 added: every other mount untouched" p12_same "$pids"

  # back to the phase's starting config (poll path): box2 and box3 unmounted, the default order again
  cp "$T/p12.orig" "$cfg"
  ok "p12: original restored → box2, box3 gone within 20s" wait_until 20 p12_restored
  ok "p12: auto driver back to sshfs" p12_status '.auto_driver == "sshfs"'
  ok "p12: the restore remounted nothing but box2" p12_same "$(jq -c 'map(select(.[0] != "box2"))' <<<"$pids")"

  # SIGHUP on a missing config reports it like `bifrost config reload` (a poll alone only logs a warning)
  pids=$(p12_pids)
  mv "$cfg" "$T/p12.away" && sig HUP "$DPID"
  ok "p12: SIGHUP on a missing config → config_errors within 2s" \
    wait_until 2 p12_status 'any(.config_errors[]; test("cannot read"))'
  ok "p12: missing config: every mount untouched (same pids)" p12_same "$pids"
  mv "$T/p12.away" "$cfg"
  ok "p12: config back → config_errors cleared within 10s" wait_until 10 p12_status '.config_errors == []'
}
