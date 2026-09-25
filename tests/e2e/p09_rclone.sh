# p09 (all): rclone. static2-rc (driver="rclone") mounts through --sftp-ssh, writes round-trip to the sshd, and
# recovers from kill -9 (picked only by $(mpid static2-rc), A13). Plus the rclone half of the host-key negative
# (A12): unknown-key-rc ("localhost" is not in the scratch known_hosts) must fail and never mount.

config_p09() {
  cat <<'EOF'
[[machines]]
name = "static2-rc"
host = "127.0.0.1"
port = 2222
user = "bf"
remote = "/home/bf"
driver = "rclone"

[[machines]]
name = "unknown-key-rc"
host = "localhost"
port = 2222
user = "bf"
remote = "/home/bf"
driver = "rclone"
EOF
}

P09_FILE=p09-$RANDOM$RANDOM.txt # unique per run; the sshd container is shared by every phase
P09_DATA="rclone round trip $P09_FILE"

p09_src() { [[ $(mnt_src "$T/machines/static2-rc") == "fuse.rclone bifrost:static2-rc@"* ]]; }
p09_drivers() {
  local o
  o=$(bifrost drivers)
  echo "$o"
  grep -q '^✓ sshfs ' <<<"$o" && grep -q '^✓ rclone ' <<<"$o"
}
p09_write() { on_fuse sh -c 'printf "%s\n" "$1" >"$2"' sh "$P09_DATA" "$T/machines/static2-rc/$P09_FILE"; }
p09_read() { [[ $(on_fuse cat "$T/machines/static2-rc/$P09_FILE") == "$P09_DATA" ]]; }
# what the sshd stores, not what the mount shows (vfs-cache-mode writes uploads 5s after close)
p09_remote() { [[ $(docker exec bf-e2e-sshd cat "/home/bf/$P09_FILE" 2>/dev/null) == "$P09_DATA" ]]; }
p09_new_pid() {
  local p
  p=$(mpid static2-rc)
  state_is static2-rc mounted && [[ -n $p && $p != "$1" ]]
}
p09_hk_error() { mjq unknown-key-rc '(.last_error // "") | contains("Host key verification failed")'; }

check_p09() {
  local mp=$T/machines/static2-rc pid seq
  # 40s: in `all`, p06 stopped and restarted the sshd under every mount and p13a restarted the daemon
  ok "p09: static2-rc mounted within 40s" wait_until 40 state_is static2-rc mounted
  # the list is empty until the daemon's first driver probe; run alone, p09 starts right after startup
  ok "p09: bifrost drivers shows sshfs ✓ and rclone ✓" wait_until 10 p09_drivers
  ok "p09: static2-rc driver is rclone" mjq static2-rc '.driver == "rclone"'
  ok "p09: mountinfo fuse.rclone, source bifrost:static2-rc@" p09_src
  ok "p09: hello.txt visible" on_fuse test -f "$mp/hello.txt"
  ok "p09: write through the mount" p09_write
  ok "p09: read it back through the mount" p09_read
  ok "p09: the sshd has the file within 20s" wait_until 20 p09_remote

  pid=$(mpid static2-rc)
  seq=$(last_seq)
  ok "p09: static2-rc has a pid" sig 0 "$pid"
  ok "p09: kill -KILL static2-rc's pid" sig KILL "$pid"
  ok "p09: kill -KILL => mounted within 20s with a new pid" wait_until 20 p09_new_pid "$pid"
  ok "p09: kill -KILL went through Stale" \
    events "$seq" '.type == "MountDegraded" and .mount == "static2-rc" and (.reason | startswith("stale"))' '> 0'
  ok "p09: the file reads back after recovery" p09_read
  ok "p09: remove the file through the mount" on_fuse rm "$mp/$P09_FILE"

  ok "p09: unknown-key-rc failed" wait_until 20 state_is unknown-key-rc failed
  ok "p09: unknown-key-rc driver is rclone" mjq unknown-key-rc '.driver == "rclone"'
  ok "p09: last_error has 'Host key verification failed'" wait_until 10 p09_hk_error
  ok "p09: unknown-key-rc not in mountinfo" not is_mounted "$T/machines/unknown-key-rc"
  ok "p09: unknown-key-rc never MountStarted" events -1 '.type == "MountStarted" and .mount == "unknown-key-rc"' '== 0'
}
