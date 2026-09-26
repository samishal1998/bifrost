# p08 (all): DNS TXT bf1 discovery against CoreDNS on 127.0.0.1:5353 (contract §12). Machines are inline node
# records at the root; other-01 comes from the index and its own record. Hostile records: bad-node publishes an
# ssh option as its host, evil a path-traversal id; both must be skipped with a warning and the canary $T/pwned
# must never exist. Big RRset: 20 discover-only bulk-NN nodes push the root answer past a 1232-byte UDP reply,
# so they are only seen through hickory's TCP retry. This is the ONLY fragment with [policy.*] tables (B5).

# p08_zone SERIAL TAGS : render dns/zone.tmpl plus the bulk nodes into $T/dns/db (tmp + mv; coredns reloads on
# a higher serial).
p08_zone() {
  local i
  {
    sed -e "s|@SERIAL@|$1|" -e "s|@TAGS@|$2|" -e "s|@T@|$T|" "$E2E_DIR/dns/zone.tmpl"
    for i in $(seq -w 1 20); do
      printf '_bifrost IN TXT "v=bf1 node=bulk-%s host=192.0.2.%d port=22 user=bulk tags=bulk path=/srv/bulk"\n' \
        "$i" "$((10#$i))"
    done
  } >"$T/dns/db.tmp"
  chmod 644 "$T/dns/db.tmp" && mv "$T/dns/db.tmp" "$T/dns/db"
}

# UDP only (+ignore: no TCP retry), so coredns' log shows TCP root queries from the daemon alone
p08_udp() { dig @127.0.0.1 -p 5353 +notcp +ignore +bufsize=1232 +time=1 +tries=1 TXT _bifrost.test.bifrost; }
p08_dig() { p08_udp | grep -q 'v=bf1'; }
p08_tc() { p08_udp | grep -q '^;; flags:.* tc[ ;]'; }
p08_tcp_bulk() {
  [[ $(dig @127.0.0.1 -p 5353 +tcp +short +time=1 +tries=1 TXT _bifrost.test.bifrost | grep -c 'node=bulk-') == 20 ]]
}
# coredns' `log` (no -q: grep reads it all, so docker never dies of SIGPIPE under pipefail)
p08_tcp_log() { docker logs bf-e2e-dns 2>&1 | grep '"TXT IN _bifrost.test.bifrost. tcp ' >/dev/null; }
p08_bulk() {
  bifrost --json machines | jq -e '[.[] | select(.id | startswith("bulk-"))
    | select(.verdict == "discover-only" and (.address | startswith("192.0.2.")))] | length == 20' >/dev/null
}

setup_p08() {
  # coredns runs as nonroot: it reads the bind-mounted dir through the "other" bits
  mkdir -p "$T/dns" && chmod 755 "$T/dns"
  cp "$E2E_DIR/dns/Corefile" "$T/dns/" && chmod 644 "$T/dns/Corefile"
  p08_zone 1 " tags=dev"
  docker rm -f bf-e2e-dns >/dev/null 2>&1 || true
  docker run -d --name bf-e2e-dns -p 127.0.0.1:5353:53/udp -p 127.0.0.1:5353:53/tcp \
    -v "$T/dns:/zones:ro" coredns/coredns:1.11.3 -conf /zones/Corefile >/dev/null
  wait_until 20 p08_dig
}

config_p08() {
  cat <<'TOML'
[[discovery]]
type = "dns"
domain = "test.bifrost"
nameservers = ["127.0.0.1:5353"]

[discovery.filter]
include_tags = ["dev"]

[discovery.mount]
honor_hints = true

[policy.deny]
tags = ["misc"]
TOML
}

# p08_verdict ID VERDICT : the machine's verdict text
p08_verdict() { [[ $(bifrost --json machines | jq -r --arg id "$1" '.[] | select(.id == $id) | .verdict') == "$2" ]]; }
p08_no_machine() { bifrost --json machines | jq -e --arg id "$1" 'all(.[]; .id != $id)' >/dev/null; }
p08_no_mount() { bifrost --json mounts | jq -e --arg id "$1" 'all(.[]; .machine != $id)' >/dev/null; }
p08_unmounted() { ! is_mounted "$T/machines/agent-dns" && ! state_is agent-dns mounted; }

check_p08() {
  local mp=$T/machines/agent-dns
  ok "p08: dig answers the bf1 root" p08_dig
  ok "p08: agent-dns mounted within 40s" wait_until 40 state_is agent-dns mounted
  ok "p08: agent-dns allowed (dns.filter.include)" p08_verdict agent-dns "allowed (dns.filter.include)"
  ok "p08: w.txt visible (path hint /home/bf/data)" on_fuse test -f "$mp/w.txt"
  ok "p08: other-01 denied (policy.deny tags=misc)" p08_verdict other-01 "denied (policy.deny tags=misc)"
  ok "p08: other-01 has no mount" p08_no_mount other-01
  ok "p08: other-01 not in mountinfo" not is_mounted "$T/machines/other-01"
  ok "p08: bad-node absent" p08_no_machine bad-node
  ok "p08: evil absent" p08_no_machine evil
  ok "p08: bad-node warned in d.log" grep -q 'bf1 node skipped.*node="bad-node".*invalid host' "$T/d.log"
  ok "p08: evil warned in d.log" grep -q 'bf1 node skipped.*node="evil".*invalid native id' "$T/d.log"
  ok "p08: canary \$T/pwned never created" not test -e "$T/pwned"
  ok "p08: big RRset: all 20 bulk nodes discover-only" wait_until 10 p08_bulk
  ok "p08: big RRset: the daemon read the root over TCP (coredns log)" p08_tcp_log
  ok "p08: big RRset: a 1232-byte UDP answer is truncated (tc)" p08_tc
  ok "p08: big RRset: over TCP all 20 bulk values" p08_tcp_bulk
  p08_zone 2 ""
  ok "p08: dev tag removed + serial bump → agent-dns unmounted within 30s" wait_until 30 p08_unmounted
  ok "p08: agent-dns now discover-only" p08_verdict agent-dns discover-only
  ok "p08: canary still absent" not test -e "$T/pwned"
}
