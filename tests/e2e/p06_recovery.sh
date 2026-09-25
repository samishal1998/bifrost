# p06 (m1): recovery. Processes are picked only by $(mpid static1), never `pkill -f fsname=`, which would also
# kill the fusermount3 auto_unmount helper (A13).

# p06_new_pid OLD : static1 is mounted again, by a process other than OLD.
p06_new_pid() {
  local p
  p=$(mpid static1)
  state_is static1 mounted && [[ -n $p && $p != "$1" ]]
}
p06_ls() { on_fuse ls "$T/machines/static1" && on_fuse cat "$T/machines/static1/hello.txt"; }
p06_offline() { state_is static1 offline && not is_mounted "$T/machines/static1"; }
# A11: POST /v1/reconcile has no side effects, so the second one plans noop for static1 (unknown-key from psec
# fails for good, hence the filter).
p06_reconcile_noop() {
  bifrost --json reconcile | jq -e '[.[] | select(.mount == "static1") | .action] == ["noop"]' >/dev/null
}

check_p06() {
  local pid seq
  pid=$(mpid static1)
  ok "p06: kill -TERM static1's pid" sig TERM "$pid"
  ok "p06: kill -TERM => mounted within 20s with a new pid" wait_until 20 p06_new_pid "$pid"

  # sign-off 4: after kill -9 the mount stays (ENOTCONN), so recovery is Stale → lazy detach → remount
  pid=$(mpid static1)
  seq=$(last_seq)
  ok "p06: kill -KILL static1's pid" sig KILL "$pid"
  ok "p06: kill -KILL => mounted within 20s with a new pid" wait_until 20 p06_new_pid "$pid"
  ok "p06: ls works after kill -KILL (no ENOTCONN)" p06_ls
  ok "p06: kill -KILL went through Stale" \
    events "$seq" '.type == "MountDegraded" and .mount == "static1" and (.reason | startswith("stale"))' '> 0'

  ok "p06: first reconcile" bifrost --json reconcile
  ok "p06: second reconcile is noop for static1 (A11)" p06_reconcile_noop

  ok "p06: docker stop sshd" docker stop -t 1 bf-e2e-sshd
  ok "p06: degraded within 30s" wait_until 30 state_is static1 degraded
  ok "p06: daemon alive" kill -0 "$DPID"
  ok "p06: docker start sshd" docker start bf-e2e-sshd
  ok "p06: mounted within 40s" wait_until 40 state_is static1 mounted
  ok "p06: hello.txt readable" p06_ls

  ok "p06: docker stop sshd again" docker stop -t 1 bf-e2e-sshd
  ok "p06: offline and not in mountinfo within 45s (grace 20s, lazy detach)" wait_until 45 p06_offline
  ok "p06: docker start sshd again" docker start bf-e2e-sshd
  ok "p06: mounted within 40s" wait_until 40 state_is static1 mounted
  ok "p06: hello.txt readable" p06_ls
}
