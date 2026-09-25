# p07 (all, opt-in E2E_TAILSCALE=1): tailscale discovery against the live tailnet, discovery ONLY.
# The provider has no filter and no allow rule matches, so every peer is discover-only: nothing is ever
# mounted. Never runs `tailscale up/down/set`. p07's own FAIL lines print counts only. On any FAIL, run.sh's
# daemon-log tail and the kept $T (d.log, state.json) name real peers: never paste them into commits, issues or fixtures.

config_p07() {
  [[ ${E2E_TAILSCALE:-} == 1 ]] || return 0
  printf '[[discovery]]\ntype = "tailscale"\n'
}

# the daemon's tailscale machine ids / the distinct first DNSName labels from the CLI (Self and sharee nodes excluded)
p07_ids() { bifrost --json machines | jq -r '.[] | select(.source == "tailscale") | .id' | sort -u; }
p07_want() {
  tailscale status --json |
    jq -r '.Peer // {} | .[] | select(.ShareeNode | not) | .DNSName | split(".")[0] | ascii_downcase | select(. != "")' | sort -u
}
p07_some() { [[ -n $(p07_ids) ]]; }
p07_discovered() {
  bifrost --json machines | jq -e '[.[] | select(.source == "tailscale") | .state == "discovered"] | all' >/dev/null
}
p07_no_mounts() {
  local ids
  ids=$(p07_ids | jq -Rsc 'split("\n") | map(select(. != ""))')
  bifrost --json machines |
    jq -e '[.[] | select(.source == "tailscale") | .mounts | length] | add == 0' >/dev/null &&
    bifrost --json mounts | jq -e --argjson ids "$ids" '[.[] | select(.machine as $m | $ids | index($m))] == []' >/dev/null
}
p07_match() {
  local got want
  got=$(p07_ids) want=$(p07_want)
  [[ $got == "$want" ]] || { echo "daemon has $(grep -c . <<<"$got") ids, tailscale status has $(grep -c . <<<"$want")"; return 1; }
}

check_p07() {
  [[ ${E2E_TAILSCALE:-} == 1 ]] || { echo "skip: p07 (set E2E_TAILSCALE=1)"; return 0; }
  ok "p07: tailscale binary present" command -v tailscale
  ok "p07: >=1 machine with source tailscale" wait_until 30 p07_some
  ok "p07: every tailscale machine is discovered" p07_discovered
  ok "p07: zero tailscale mounts" p07_no_mounts
  ok "p07: ids == distinct first DNSName labels" p07_match
}
