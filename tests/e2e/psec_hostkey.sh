# psec (m1): host-key negative for sshfs. "localhost" reaches the same sshd, but only [127.0.0.1]:2222 and
# [127.0.0.2]:2222 are in the scratch known_hosts (CheckHostIP no), so strict checking must refuse it.
# The rclone half (unknown-key-rc) is p09's (A12).

config_psec() {
  cat <<'EOF'
[[machines]]
name = "unknown-key"
host = "localhost"
port = 2222
user = "bf"
remote = "/home/bf"
EOF
}

psec_error() { mjq unknown-key '(.last_error // "") | contains("Host key verification failed")'; }

check_psec() {
  ok "psec: unknown-key failed" wait_until 20 state_is unknown-key failed
  ok "psec: last_error has 'Host key verification failed'" wait_until 10 psec_error
  ok "psec: unknown-key not in mountinfo" not is_mounted "$T/machines/unknown-key"
  ok "psec: unknown-key never MountStarted" events -1 '.type == "MountStarted" and .mount == "unknown-key"' '== 0'
}
