# Deliberate simplifications (the ponytail ledger)

This ledger lists every corner Bifröst cuts on purpose: the simplification, the ceiling it imposes, the upgrade path
when that ceiling starts to hurt, and where it lives. It has two sources, contract §15 ("Deliberate simplifications")
and the `// ponytail:` comments in the code, and it reconciles them. The house rule behind it is in
[decisions.md](decisions.md#ponytail-style): every deliberate simplification with a known ceiling gets a
`ponytail:` comment at its location and a §15 row. When you lift a ceiling, delete the comment and update this ledger
and contract §15. When you cut a new corner, add both.

## Contents

- [The ledger](#ledger)
- [Reconciliation: comments against §15 rows](#reconciliation)
- [Expected absences](#expected-absences)
- [Finding them yourself](#finding)

<a id="ledger"></a>
## The ledger

Rows 1–31 are contract §15 in order. Rows X1–X11 are `ponytail:` comments with no §15 row. "Comment" says whether a
`ponytail:` comment exists at the location; where the comment's wording or location differs from the §15 row, the
code wins and the row says how. Locations are file + item (function, type, constant), never line numbers.

| # | Simplification | Ceiling | Upgrade path | Location (file, item) | Comment |
|---|---|---|---|---|---|
| 1 | `BoxFuture` alias instead of async-trait; the PRD §5 `ctx` parameters dropped; `inspect` returns `MountState` (errors fold into `Degraded`) | implementations write `Box::pin(async move { .. })`; no runtime context reaches a plugin | `DiscoveryContext` / `DriverContext` when a plugin needs runtime context | `crates/bifrost-core/src/lib.rs`, `BoxFuture` | yes |
| 2 | Static discovery is `registry.replace(config.static_observations())`, with no provider object or task | static machines can only come from the config. (Since r3 the status output shows a synthetic `static` provider row; the comment is about internals only.) | implement `DiscoveryProvider` if static ever needs another source | `crates/bifrost-daemon/src/actor.rs`, `Actor::apply` | yes |
| 3 | Config reload polls the file every 2 s on a plain `std::thread` instead of using notify | an edit applies after 2–4 s; SIGHUP and `POST /v1/config/reload` are immediate | notify 8.x if latency matters | `crates/bifrost-daemon/src/reload.rs`, `spawn_poller` | yes |
| 4 | Hand-rolled glob (`*`, `?`), CIDR, duration parser, FNV-1a, jitter, `~`/`$VAR` expansion and mountinfo parser | no character classes or brace globs; a duration has one unit; no `~user` or `${VAR:-default}`; the mountinfo parser reads only mount point, fstype and source | globset if rules need classes; `~user` / defaults if configs need them; a mountinfo crate if more fields are needed | `crates/bifrost-core/src/validate.rs`, `Glob` (covers glob, CIDR, duration, FNV-1a, jitter); `crates/bifrost-config/src/lib.rs`, `expand`; `crates/bifrost-mount/src/table.rs`, `parse_mountinfo` | yes, 3 comments (§15 names only `core/validate.rs` and `mount/table.rs`; the expansion comment is in config) |
| 5 | SSH timings are constants: `ConnectTimeout=10`, `ServerAliveInterval=15` × `ServerAliveCountMax=3` | a dead link takes at least 45 s to detect; the command line overrides the same keys in ssh_config | an `[ssh]` config section if users need tuning | `crates/bifrost-mount/src/lib.rs`, `SSH_OPTS` | yes |
| 6 | rclone `--dir-cache-time=15s` is a constant | a dead rclone remote can look healthy for up to 15 s; listings are refetched every 15 s | make it a config key if listing traffic matters | `crates/bifrost-mount/src/rclone.rs`, `rclone_argv` | yes |
| 7 | The liveness probe timeout is fixed at 5 s | slow links under load can read as `Degraded` (harmless: nothing acts until the grace period) | a config key if false `Degraded` reports annoy | `crates/bifrost-mount/src/check.rs`, `liveness` | yes |
| 8 | HTTP inventory: 10 s timeout, 1 MiB body, 1000 entries, no ETag or Cache-Control | an expensive inventory is fetched in full every interval | `ETag` / `If-None-Match` | `crates/bifrost-discovery/src/http.rs`, `BODY_CAP` / `MAX_ENTRIES` | yes |
| 9 | Removal hysteresis is `3 × interval` (the registry's expiry floor), with no grace tombstones | a machine absent from successful refreshes for 3 intervals is unmounted (gracefully); a DNS observation lasts `max(record TTL, 3 × interval)` (`MachineRegistry::apply_ok`) | tombstones for `offline_grace_period` if inventories flap for longer | `crates/bifrost-core/src/registry.rs`, `MachineRegistry::expire` (the floor is set in `crates/bifrost-daemon/src/actor.rs`, `Actor::discovery`) | yes |
| 10 | Providers of the same kind tie-break by provider name, not config order | two DNS providers reporting one id: the alphabetically first name wins | carry a config rank in `Source` | `crates/bifrost-core/src/registry.rs`, `Source` | yes |
| 11 | Winner-takes-all merge by trust | a DNS or HTTP include can't mount an id that a more trusted source reports unless it is allowed | a per-id `prefer = "<provider>"` rule | `crates/bifrost-core/src/policy.rs`, `evaluate` | yes |
| 12 | No client SSE consumer; the TUI polls `/v1/status` every second inline (no poller task or channel, E6) | event latency up to 1 s; debugging SSE needs `curl --unix-socket` | `Client::events()` plus a `bifrost events` command | `crates/bifrost-client/src/lib.rs`, module comment; `crates/bifrost-tui/src/main.rs`, `run` | yes, 2 comments |
| 13 | The child log is truncated at each spawn, with no rotation or size cap | a noisy long-lived rclone grows its log on disk. (The daemon itself reads only the last 2 KiB for errors and the last 64 KiB for the log route.) | cap it on the health tick | `crates/bifrost-mount/src/lib.rs`, `mount_with` (the log open) | yes |
| 14 | No explicit wake detector | after a resume, probes and discovery happen within one interval | compare `SystemTime` and `Instant` elapsed and force the ticks | `crates/bifrost-daemon/src/actor.rs`, `Actor::run` | yes |
| 15 | No mount process is ever signalled except our own spawn that timed out (timed-out helper commands are killed on drop); no orphan scan | a process stuck connecting lives until `ConnectTimeout`; a lazily detached sshfs lingers until its last reference closes | none, deliberately: killing loses data | `crates/bifrost-mount/src/lib.rs`, `mount_with` (the timeout branch) | yes |
| 16 | `vfs_cache_mode` and `ssh_config` are not part of the spec fingerprint | changing them applies to new mounts only | include them in the fingerprint (note: the encoding is pinned by `fingerprint_stable_vector`, so this changes every existing marker) | `crates/bifrost-core/src/model.rs`, `MountSpec::fingerprint` | yes |
| 17 | Drivers are re-probed only at startup, on reload, on the fallback tick and on `POST /v1/reconcile` | a freshly installed tool is seen within `reconcile_interval` | none given (the §15 row has "—", and the comment names no upgrade either) | `crates/bifrost-daemon/src/actor.rs`, `Actor::probe` | yes |
| 18 | Graceful shutdown doesn't unmount | stopping the daemon leaves mounts unsupervised until the next start adopts them | run `bifrost unmount <target>` for each mount before stopping, if wanted | `crates/bifrost-daemon/src/main.rs`, `run` | yes |
| 19 | Only mounts directly under the current root are adopted | after a root change across a restart, mounts under the old root are left alone | unmount any marker mount outside the root at startup | `crates/bifrost-mount/src/lib.rs`, `adopt` | yes |
| 20 | No concurrency cap on mount operations | dozens of simultaneous auto-mounts each start ssh at once | a tokio `Semaphore` in the executor | `crates/bifrost-daemon/src/actor.rs`, `Actor::start_mount` | yes |
| 21 | DNSSEC is not validated. (Before v0.1.1 this row also said inline root records were unsupported; sign-off 14 removed that half.) | a spoofed or on-path DNS answer is trusted; it still has to pass every bf1 validator and the policy | hickory's `dnssec` feature and validation | `crates/bifrost-discovery/src/dns.rs`, `root` | yes |
| 22 | Provider warnings (skipped records) go only to the log | invisible from the CLI and TUI | a `warnings` field in `ProviderDto` | `crates/bifrost-daemon/src/actor.rs`, `Actor::snapshot`; `crates/bifrost-discovery/src/dns.rs`, `DnsProvider::discover`; `crates/bifrost-discovery/src/http.rs`, `parse_inventory`; `crates/bifrost-discovery/src/tailscale.rs`, `parse_status` | yes, 4 comments |
| 23 | macOS adoption without a marker trusts a state.json record; an unknown fstype guesses `sshfs` | inspect and unmount are shared by the drivers, so the guess is harmless. (Since e914166 this applies on macOS only; on Linux an unmarked entry is foreign.) | read the NFS source to tell the drivers apart | `crates/bifrost-mount/src/lib.rs`, `adopt` | yes |
| 24 | The TUI uses truecolor only | Terminal.app renders the RGB colours approximately | map to 256-colour indices when `COLORTERM` isn't truecolor | `crates/bifrost-tui/src/ui.rs`, the palette constants (`NIGHT` … `ROSE`) | yes |
| 25 | macOS code has no field test | the runtime behaviour of macFUSE, FUSE-T and nfsmount is unproven. (The row said "compile-checked only"; CI now also builds and unit-tests on macos-15, but there is still no macOS E2E.) | a macOS runner for the E2E | `crates/bifrost-mount/src/table.rs`, the macOS `read` (§15 says `mount/*`) | yes, one comment for the whole row |
| 26 | Discovery intervals have no jitter; one `failures` counter covers both mount and unmount | synchronised provider polls; after a long busy unmount the next mount failure backs off near `retry_max` (r2) | ±10% jitter | `crates/bifrost-daemon/src/actor.rs`, `discover_loop` | yes |
| 27 | A missing config runs the empty default (root `~/machines`) | a typo in `BIFROST_CONFIG` silently runs with no machines (logged at warn); a file that appears later with another `mount.root` is rejected (A22) and needs a restart | an opt-in `--require-config` | `crates/bifrost-daemon/src/main.rs`, `config`; decision: [missing-config-empty-default](decisions.md#missing-config-empty-default) | yes |
| 28 | A discovered machine's default template is `remote = "~"` (the remote login directory), not PRD §2's `~/machines/agent-01/home/sami/project` shape (B2) | discovered machines mount their home unless the provider's `[discovery.mount]` sets `remote` | a per-provider default in the docs, or a smarter template | `crates/bifrost-config/src/lib.rs`, `V::config` (the discovery `mount.remote` default) | yes |
| 29 | No CI yaml until a git remote exists; `scripts/check.sh` is the gate (PRD §29 P0 CI, B3) | checks run only when an agent or a human runs them | add a CI workflow calling `scripts/check.sh` once a remote exists | `scripts/check.sh` | **no: superseded.** The upgrade was carried out in e30f0aa: `.github/workflows/ci.yml` runs `scripts/check.sh` on every push to `main` and every pull request, and the header of `scripts/check.sh` says so. The row is obsolete; nothing to comment |
| 30 | A provider that fails permanently freezes its last view forever (B9) | the machines it reported stay listed, and mounted, until it recovers or is removed from the config | expire frozen observations after a long cap such as `offline_grace_period` | `crates/bifrost-core/src/registry.rs`, `MachineRegistry::mark_failed` | yes |
| 31 | `bifrostd` and `bifrost-discovery` are never darwin-checked (reqwest pulls in ring, whose C build needs an Apple toolchain; B9) | macOS compile errors in the daemon or providers show only on a Mac | a macOS runner, or the `SDKROOT=/ CC_…=true AR_…=true cargo check --target aarch64-apple-darwin --workspace` experiment | `crates/bifrost-daemon/src/main.rs`, `main` | **yes, but obsolete.** The `macos` job in `.github/workflows/ci.yml` (e30f0aa, 12044f4) builds all targets and runs `cargo test --workspace` on macos-15 (an arm64 image, so for `aarch64-apple-darwin` only), daemon and discovery included. `release.yml` also builds `x86_64-apple-darwin` there, as a cross-compile that is never tested. The comment and the row should be removed |
| X1 | state.json is written synchronously on the actor | a slow state directory stalls the actor for the write (a few hundred bytes) | `spawn_blocking` with ordered writes | `crates/bifrost-daemon/src/actor.rs`, `Actor::persist` | yes (source: S2 sign-off 1, "state.json is written synchronously on the actor") |
| X2 | The ownership check on the mount root doesn't look at its ancestors or at group-shared directories | a root under a directory other users can write, or a group-writable root, passes | walk the ancestors and test `0o020` if shared roots matter | `crates/bifrost-daemon/src/main.rs`, `run` (step 5, the `private` check); decision: [private-state-and-socket-dirs](decisions.md#private-state-and-socket-dirs) | yes (source: r2 sign-off 9 and r3 sign-off 12, "the root's ancestors and group-shared roots stay unchecked") |
| X3 | Tailscale's `which_in` copies `bifrost_mount::check::{which_in, search_path}`, because discovery can't depend on mount | two copies of the binary lookup to keep in step | one shared crate if a third copy appears | `crates/bifrost-discovery/src/tailscale.rs`, `which_in` | yes (source: S3 carry-over 2d, 36dc831) |
| X4 | A mount attempt dropped whole by the actor's outer timeout keeps its empty `<root>/<id>` until the next attempt for that id | an empty directory can linger | a `Drop` guard, once a dropped attempt's child can no longer mount there | `crates/bifrost-mount/src/lib.rs`, `mount_with` (before `prepare_mountpoint`) | yes (code comment only; the r1 change that removes the directory after a failed attempt is 2e09dce) |
| X5 | The hung-probe in-flight key is path + pid; two pid-less adoptions at one path share it | a stuck probe of one pid-less instance also blocks probes of another at the same path | key by mountinfo field 1 (the mount id) if that ever bites | `crates/bifrost-mount/src/lib.rs`, `inspect_path` | yes (source: S4a sign-off 11, 374fcb3) |
| X6 | A lazily detached rclone with open files can outlive its mount while row 9 starts a new one on the same `--cache-dir` | rclone warns this "can potentially cause data corruption" | when nothing is mounted at the path, scan `/proc/*/cmdline` for this exact `--cache-dir=` and return `Failed("cache still used by rclone pid N")` so the backoff waits | `crates/bifrost-mount/src/rclone.rs`, `RcloneDriver::argv` | yes (source: 785e351, review finding conf-J-R1a) |
| X7 | rclone keys the VFS cache inside `--cache-dir` by a hash of the `--sftp-*` flags | pending writes resume only while the ssh path, ssh_config path, host, port and user are unchanged; after a change they stay un-uploaded under `<state>/rclone/<id>` | warn when `vfs/` holds another `:sftp{…}` directory | `crates/bifrost-mount/src/rclone.rs`, `RcloneDriver::argv` | yes (source: 785e351, review finding sec-S3J-DATA-1) |
| X8 | A graceful rclone unmount doesn't wait for VFS write-back | writes closed less than about 5 s before the unmount stay in `<state>/rclone/<id>` until that id mounts again with the same flags | `rclone rc vfs/stats` over a 0600 Unix socket, and `Busy` while uploads are pending | `crates/bifrost-mount/src/rclone.rs`, `RcloneDriver::unmount` | yes (source: 785e351, review finding conf-J-R1b) |
| X9 | `rclone nfsmount` serves the remote on an unauthenticated, random 127.0.0.1 NFS port | any local user can reach it; fine on single-user Macs | put `rclone` (FUSE) before `rclone-nfs` in the macOS `auto_order` when macFUSE or FUSE-T is installed | `crates/bifrost-mount/src/rclone.rs`, `rclone_argv` (the subcommand) | yes (source: 785e351, review finding sec-S3J-SEC-3) |
| X10 | The DNS provider rebuilds the system resolver on every refresh | hickory's cache never outlives one refresh, so each refresh queries again (the stub resolver or upstream still caches) | reuse the resolver while the system config is unchanged, if the extra queries matter | `crates/bifrost-discovery/src/dns.rs`, `DnsProvider::discover` | yes (source: r1 sign-off 7, e2284f5) |
| X11 | The TUI Logs view wraps lines by characters, not display width | a line of wide glyphs may still clip | split by display width if that shows up | `crates/bifrost-tui/src/ui.rs`, `render` (the Logs view) | yes (code comment only; the wrapping itself came from a5d7c4c) |

<a id="reconciliation"></a>
## Reconciliation: comments against §15 rows

`git grep -n "ponytail:" -- crates` finds **47** comments (checked for this ledger):

| Group | Comments |
|---|---|
| §15 rows with one comment | 27 rows (1–3, 5–11, 13–21, 23–28, 30, 31) → 27 comments |
| §15 rows with several comments | #4 → 3, #12 → 2, #22 → 4 → 9 comments |
| §15 row with no comment | #29 (superseded by CI) → 0 |
| comments with no §15 row | X1–X11 → 11 comments |
| **Total** | 27 + 9 + 11 = **47** |

Per file: `bifrost-daemon/src/actor.rs` 7, `bifrost-mount/src/lib.rs` 7, `bifrost-mount/src/rclone.rs` 5,
`bifrost-daemon/src/main.rs` 4, `bifrost-core/src/registry.rs` 3, `bifrost-discovery/src/dns.rs` 3, and 2 each in
`bifrost-config/src/lib.rs`, `bifrost-discovery/src/http.rs`, `bifrost-discovery/src/tailscale.rs`,
`bifrost-mount/src/table.rs` and `bifrost-tui/src/ui.rs`, and 1 each in `bifrost-client/src/lib.rs`,
`bifrost-core/src/{lib,model,policy,validate}.rs`, `bifrost-daemon/src/reload.rs`, `bifrost-mount/src/check.rs` and
`bifrost-tui/src/main.rs`. No `ponytail:` comment exists outside `crates/` (scripts, the E2E harness, workflows,
`install.sh`).

What doesn't line up:

- **§15 row without a comment:** only #29, which CI superseded.
- **Comment without a §15 row:** X1–X11. They are real ceilings added after the contract was written (commits found
  with `git log -S` on each comment):
  - implementation, accepted in a sign-off: X1 (6e7ee55, S2-E; S2 sign-off 1);
  - stage reviews and carry-overs: X3 (36dc831, S4-O, the S3 carry-over 2d) and X6–X9 (785e351, the S3-J review);
  - final review rounds: X4 (2e09dce) and X10 (e2284f5) from r1, X5 (374fcb3) and X2 (fb3238e) from r2, X2's
    wording also in the r3 sign-off;
  - code only: X11 (a5d7c4c, the TUI Logs wrapping fix).

  Add §15 rows for them the next time the contract is edited, or keep this ledger as the list.
- **Stale:** the #31 comment in `crates/bifrost-daemon/src/main.rs` (CI builds and tests the daemon and discovery on
  macOS). #25's "compile-checked only" wording is outdated in the same way, though its real ceiling (no macOS E2E)
  still holds.
- **Rows whose wording changed since the contract was frozen:** #21 (v0.1.1 dropped "inline records unsupported"),
  #23 (macOS only since e914166), #2 (status now shows a `static` row, r3).

<a id="expected-absences"></a>
## Expected absences

Two `ponytail:` comments that the contract describes do not exist, on purpose:

- **The A10 fallback comment** ("a same-fstype entry at `<root>/<id>` counts as ours…", contract §6). It was to be
  added only if sshfs ignored a user `fsname=`. S1 sign-off 4 verified that the user `fsname=` becomes the mountinfo
  source, so the fallback was never written.
- **The `run.sh` skip comment** ("a missing listed phase file is skipped, not an error", contract §12). S4a sign-off 5
  made a missing phase file fatal, so the skip and its comment are gone.

<a id="finding"></a>
## Finding them yourself

```sh
git grep -n "ponytail:" -- crates       # every code comment, with its location (unscoped, it also matches the
                                         # contract's prose and these guides)
git grep -c "ponytail:" -- crates        # per-file counts (47 in total when this ledger was written)
sed -n '/^## 15\./,/^### Critical/p' docs/design/contract.md   # the §15 table
```

A multi-line comment (the ones in `crates/bifrost-config/src/lib.rs` `V::config` and
`crates/bifrost-discovery/src/dns.rs` `DnsProvider::discover`) continues on the following `//` lines.
