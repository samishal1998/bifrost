# p04 (m1): static sshfs mount in the PRD §31 shorthand shape (B4), under $T/machines only.

config_p04() {
  cat <<'EOF'
[[machines]]
name = "static1"
host = "127.0.0.1"
port = 2222
user = "bf"
remote = "/home/bf"
EOF
}

p04_src() { [[ $(mnt_src "$T/machines/static1") == "fuse.sshfs bifrost:static1@"* ]]; }

check_p04() {
  local mp=$T/machines/static1
  ok "p04: static1 mounted within 20s" wait_until 20 state_is static1 mounted
  ok "p04: hello.txt visible" on_fuse test -f "$mp/hello.txt"
  ok "p04: mountinfo fuse.sshfs, source bifrost:static1@" p04_src
  ok "p04: bifrost unmount static1" bifrost unmount static1
  ok "p04: not in mountinfo, directory removed" wait_until 10 gone "$mp"
  ok "p04: bifrost reconcile" bifrost reconcile
  sleep 3 # a pass and a health tick: a held mount must stay down
  ok "p04: still unmounted after reconcile (held)" gone "$mp"
  ok "p04: held" mjq static1 '.held'
  ok "p04: bifrost mount static1" bifrost mount static1
  ok "p04: mounted again" wait_until 20 state_is static1 mounted
  ok "p04: hello.txt visible again" on_fuse test -f "$mp/hello.txt"
}
