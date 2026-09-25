# p08 (all): DNS TXT bf1 discovery against CoreDNS on 127.0.0.1:5353 (contract §12). Hostile records: bad-node
# publishes an ssh option as its host, evil a path-traversal id; both must be skipped with a warning and the
# canary $T/pwned must never exist. This is the ONLY fragment with [policy.*] tables (B5).

# p08_zone SERIAL TAGS : render dns/zone.tmpl into $T/dns/db (tmp + mv; coredns reloads on a higher serial).
p08_zone() {
  sed -e "s|@SERIAL@|$1|" -e "s|@TAGS@|$2|" -e "s|@T@|$T|" "$E2E_DIR/dns/zone.tmpl" >"$T/dns/db.tmp"
  chmod 644 "$T/dns/db.tmp" && mv "$T/dns/db.tmp" "$T/dns/db"
}

p08_dig() { dig @127.0.0.1 -p 5353 +short +time=1 +tries=1 TXT _bifrost.test.bifrost | grep -q 'v=bf1'; }

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
  ok "p08: dig answers the bf1 index" p08_dig
  ok "p08: agent-dns mounted within 40s" wait_until 40 state_is agent-dns mounted
  ok "p08: agent-dns allowed (dns.filter.include)" p08_verdict agent-dns "allowed (dns.filter.include)"
  ok "p08: w.txt visible (path hint /home/bf/data)" on_fuse test -f "$mp/w.txt"
  ok "p08: other-01 denied (policy.deny tags=misc)" p08_verdict other-01 "denied (policy.deny tags=misc)"
  ok "p08: other-01 has no mount" p08_no_mount other-01
  ok "p08: other-01 not in mountinfo" not is_mounted "$T/machines/other-01"
  ok "p08: bad-node absent" p08_no_machine bad-node
  ok "p08: evil absent" p08_no_machine evil
  ok "p08: bad-node warned in d.log" grep -q 'bf1 node skipped.*_bifrost\.bad-node\..*invalid host' "$T/d.log"
  ok "p08: evil warned in d.log" grep -q 'bf1 node skipped.*_bifrost\.evil\..*invalid native id' "$T/d.log"
  ok "p08: canary \$T/pwned never created" not test -e "$T/pwned"
  p08_zone 2 ""
  ok "p08: dev tag removed + serial bump → agent-dns unmounted within 30s" wait_until 30 p08_unmounted
  ok "p08: agent-dns now discover-only" p08_verdict agent-dns discover-only
  ok "p08: canary still absent" not test -e "$T/pwned"
}
