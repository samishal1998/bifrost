# p13 (all): hardening against the p08 DNS and p10 HTTP fixtures, in the same daemon and config (contract §12).
# Needs p08_dns and p10_http earlier in the run: coredns on 127.0.0.1:5353 serving $T/dns/db, the "dns" provider
# (include_tags=["dev"], honor_hints) and inv-01 mounted from the "inventory" provider. p13 rewrites the zone:
#   IP change  agent-dns moves to 127.0.0.2 (start_sshd put both loopbacks in known_hosts) → remounted, new fingerprint
#   duplicate  inv-01 also published via DNS, with another host → still one machine, from the inventory (trust
#              http < dns), dns only shadowed: never redirected, never remounted
#   rename     node agent-dns becomes agent-dns2 → the old id ages out (3 × discovery_interval) and unmounts, the new
#              one mounts
# p13 is the last phase of `all`, so nothing needs the old zone back and it isn't restored.

# p13_zone SERIAL "LABEL|KEY=V ..."... : $T/dns/db with one node record per argument and the index listing them all
# (tmp + mv, 644: coredns runs nonroot and reloads on a higher serial)
p13_zone() {
  local serial=$1 r nodes=()
  shift
  for r; do nodes+=("${r%%|*}"); done
  {
    printf '$ORIGIN test.bifrost.\n$TTL 5\n@ IN SOA ns admin %s 60 60 3600 5\n@ IN NS ns\nns IN A 127.0.0.1\n' "$serial"
    printf '_bifrost IN TXT "v=bf1 nodes=%s"\n' "$(IFS=,; echo "${nodes[*]}")"
    for r; do printf '_bifrost.%s IN TXT "v=bf1 %s"\n' "${r%%|*}" "${r#*|}"; done
  } >"$T/dns/db.tmp"
  chmod 644 "$T/dns/db.tmp" && mv "$T/dns/db.tmp" "$T/dns/db"
}
# p13_serial N : coredns serves the zone with serial N (a zone it can't parse keeps the old serial)
p13_serial() { [[ $(dig @127.0.0.1 -p 5353 +short +time=1 +tries=1 SOA test.bifrost | awk '{ print $3 }') == "$1" ]]; }
# p13_moved OLD : agent-dns is mounted from another fingerprint than OLD (mnt_src "<fstype> <marker>")
p13_moved() {
  local s
  s=$(mnt_src "$T/machines/agent-dns")
  [[ $s == "fuse.sshfs bifrost:agent-dns@"* && $s != "$1" ]] && state_is agent-dns mounted
}
# p13_count LIST FILTER N : N entries of `bifrost --json LIST` match FILTER
p13_count() { bifrost --json "$1" | jq -e "[.[] | select($2)] | length == $3" >/dev/null; }

check_p13() {
  declare -F check_p08 check_p10 >/dev/null || { ok "p13: needs p08_dns and p10_http earlier in the run" false; return 0; }
  local mp=$T/machines/agent-dns old pid t0
  local at1="agent-dns|host=127.0.0.1 port=2222 user=bf tags=dev path=/home/bf/data"
  local at2="agent-dns|host=127.0.0.2 port=2222 user=bf tags=dev path=/home/bf/data"
  local dup="inv-01|host=127.0.0.2 port=2222 user=bf tags=dev path=/home/bf/data"
  local renamed="agent-dns2|host=127.0.0.2 port=2222 user=bf tags=dev path=/home/bf/data"

  # p08 left agent-dns discover-only (its dev tag removed): tag it again on 127.0.0.1
  p13_zone 3 "$at1"
  ok "p13: serial 3 served" wait_until 10 p13_serial 3
  ok "p13: agent-dns (127.0.0.1) mounted within 40s" wait_until 40 state_is agent-dns mounted
  old=$(mnt_src "$mp")

  p13_zone 4 "$at2"
  ok "p13: serial 4 served (agent-dns host=127.0.0.2)" wait_until 10 p13_serial 4
  ok "p13: IP change → remounted with a new fingerprint within 30s" wait_until 30 p13_moved "$old"
  ok "p13: agent-dns now connects to 127.0.0.2" mjq agent-dns '.remote == "bf@127.0.0.2:/home/bf/data"'
  ok "p13: w.txt visible through 127.0.0.2" on_fuse test -f "$mp/w.txt"

  pid=$(mpid inv-01)
  p13_zone 5 "$at2" "$dup"
  ok "p13: serial 5 served (inv-01 also in DNS)" wait_until 10 p13_serial 5
  ok "p13: duplicate: dns shadowed on inv-01 within 20s" wait_until 20 p10_mjq inv-01 'any(.shadowed[]; . == "dns")'
  ok "p13: duplicate: exactly one machine inv-01" p13_count machines '.id == "inv-01"' 1
  ok "p13: duplicate: inv-01 source inventory, address still 127.0.0.1" p10_mjq inv-01 \
    '.source == "inventory" and .address == "127.0.0.1" and .verdict == "allowed (inventory.filter.include)"'
  sleep 3 # a pass and a health tick with both observations
  ok "p13: duplicate: inv-01 has one mount, still bf@127.0.0.1" p13_count mounts \
    '.machine == "inv-01" and .remote == "bf@127.0.0.1:/home/bf"' 1
  ok "p13: duplicate: inv-01 not remounted (same pid)" mjq inv-01 ".state == \"mounted\" and .pid == $pid"

  t0=$SECONDS
  p13_zone 6 "$renamed" "$dup"
  sleep 3 # < 3 × discovery_interval (9s) since the last refresh that still listed agent-dns
  ok "p13: rename: agent-dns still mounted 3s later (removal hysteresis)" state_is agent-dns mounted
  ok "p13: serial 6 served (agent-dns → agent-dns2)" wait_until 10 p13_serial 6
  ok "p13: rename: old agent-dns unmounted and removed within 25s" wait_until 25 gone "$mp"
  echo "  (agent-dns gone $((SECONDS - t0))s after the rename)"
  ok "p13: rename: agent-dns no longer listed" p08_no_machine agent-dns
  ok "p13: rename: agent-dns2 mounted" wait_until 20 state_is agent-dns2 mounted
  ok "p13: rename: agent-dns2 w.txt visible" on_fuse test -f "$T/machines/agent-dns2/w.txt"
  ok "p13: rename: inv-01 untouched (same pid)" mjq inv-01 ".state == \"mounted\" and .pid == $pid"
}
