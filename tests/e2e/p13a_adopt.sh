# p13a (m1): adoption. A daemon restart (kill -9 or SIGTERM) keeps static1's sshfs process and adopts it:
# same pid, no MountStarted, and no duplicate sshfs process (B6).

sshfs_count() { pgrep -xc sshfs || true; } # pgrep -c prints 0 and exits 1 when there is none

p13a_adopted() { [[ $1 =~ ^[1-9][0-9]*$ ]] && mjq static1 ".adopted and .state == \"mounted\" and .pid == $1"; }

# p13a_restarted HOW PID N : checks after a restart; PID = static1's pid, N = the sshfs count, both from before.
p13a_restarted() {
  ok "p13a ($1): adopted, mounted, same pid" wait_until 15 p13a_adopted "$2"
  sleep 3 # a health tick and a pass: time for a wrong remount to show
  ok "p13a ($1): still adopted with the same pid" p13a_adopted "$2"
  ok "p13a ($1): no MountStarted for static1" events -1 '.type == "MountStarted" and .mount == "static1"' '== 0'
  ok "p13a ($1): sshfs process count unchanged ($3)" test "$(sshfs_count)" = "$3"
  ok "p13a ($1): hello.txt readable" on_fuse test -f "$T/machines/static1/hello.txt"
}

check_p13a() {
  local pid n
  pid=$(mpid static1)
  n=$(sshfs_count)
  ok "p13a: static1 has a pid" sig 0 "$pid"

  ok "p13a: kill -9 bifrostd" sig KILL "$DPID"
  wait "$DPID" 2>/dev/null || true
  DPID=
  ok "p13a: mount works with the daemon dead" on_fuse test -f "$T/machines/static1/hello.txt"
  start_daemon
  p13a_restarted "kill -9" "$pid" "$n"

  stop_daemon
  start_daemon
  p13a_restarted SIGTERM "$pid" "$n"
}
