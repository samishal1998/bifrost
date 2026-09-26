# Completeness critique of the Bifröst V1 final contract

I checked the contract against PRD §1–§35 and verified the cited crate APIs and tool flags. Crate sources came from `~/.cargo/registry/src` and, for hickory, from the crates.io `.crate` tarballs, which aren't in the local cache. Tool flags came from the installed `sshfs 3.7.3`, `rclone 1.75.1` and `fusermount3 3.14.0`. Each finding has a severity, the section it affects, the problem, and the fix.

## A. Compile, correctness and security defects

1. **[BLOCKER] `gen` is a reserved keyword in edition 2024** (§2 `MountRuntime.gen` and the `mount_done/unmount_done/health(gen, …)` parameters; §5 `Msg::{MountDone,UnmountDone,Health,ChildExited}.gen`). The workspace is `edition = "2024"`, so these don't compile. **Fix:** rename to `generation` everywhere; `task_gen` is fine.

2. **[BLOCKER] S1 agents depend on each other's function bodies** (§13). Agent C (`sshfs_argv`, `adopt`, readiness) needs `MountSpec::{fingerprint, source}`, `marker` and `parse_marker`. Agent B needs `DriverSelector: TryFrom<String>`. All of these are agent A's `model.rs` work in the same stage, so C's golden tests and B's tests fail until A merges. **Fix:** implement these in S0, next to `validate.rs` (about 40 lines).

3. **[BLOCKER] `Msg` and `Tick` (§5) aren't in the S0 frozen list** ("§2, §3, §6, §7, §8, §9"). But `api.rs` (agent D, S1) sends `Msg::Api`, and `reload.rs` (agent M, S4) sends `Msg::Config`. **Fix:** add §5 `Msg` and `Tick` to S0 and say they live in `actor.rs`.

4. **[HIGH] The reload boundary crosses file ownership** (§8, §13). `reload::apply` (M, S4) must diff providers, spawn and abort tasks, and touch the registry and drivers. Those are actor internals owned by E (S2), and startup needs the same apply path anyway. There are also two reload routes: `ApiCmd::Reload` and `Msg::Config{reply}`. **Fix:**
   - E implements config apply and provider diffing in `actor.rs` in S2, and uses it at startup too.
   - M owns only `reload::spawn_poller(path: PathBuf, tx: UnboundedSender<Msg>)` plus `p12`.
   - Delete `reload::apply` and `ApiCmd::Reload`.
   - `POST /v1/config/reload` runs `spawn_blocking(load)` and sends `Msg::Config{reply: Some}`.

5. **[HIGH] Rebuilding drivers on reload bypasses `Deps`** (§8). "Rebuild `drivers(&settings)`" inside the actor would swap the `FakeDriver`s for real drivers in the daemon tests. **Fix:** `Deps.drivers: Arc<dyn Fn(&DriverSettings) -> Vec<Arc<dyn MountDriver>> + Send + Sync>`, mirroring `build_provider`.

6. **[HIGH] A missing config unmounts adopted mounts** (§8 step 4, §14 risk 3). With the empty default there are no network providers, so `ready` is true immediately. Mounts adopted under the default `~/machines` then hit row 5 and get unmounted. The text "nothing was adopted from a config anyway" is wrong: adoption comes from the mount table, not the config. **Fix:** keep `ready = false` until a config file has actually loaded.

7. **[HIGH] Provider tasks sleep before the first discovery** (§5 triggers). `loop { select!{sleep(interval), notified}; discover }` means the first tailscale/DNS/HTTP result arrives only after `interval`, which also delays warm-up. **Fix:** `loop { discover; select!{sleep, notified} }`.

8. **[HIGH] `clean()` cuts off the error text.** It truncates to the first 128 characters (§2), but it is applied to "the last 2 KiB of the log" (§6 spawn), whose first line is the argv header. The same truncation applies to ssh stderr tails. On an early sshfs exit, `last_error` would show argv instead of the error. The psec message ("No ED25519 host key… \r\nHost key verification failed.") is about 122 characters, so it passes only by luck. **Fix:**
   - Change the signature to `clean(s, max)` and add a `tail(s, max)` that keeps the last lines and skips the header.
   - Use 512 characters for errors and log lines.

9. **[HIGH] Mount race leaves a path permanently Refused** (§5 executor, §6 `mount()`).
   - Problem 1: the outer timeout (`mount_timeout + 20s`) can be shorter than the worst case inside the driver: preflight 15s + `mount_timeout` + lazy unmount 10s. Dropping the future orphans a child that is still mounting. The next `mount()` then hits step 2 "occupied" and returns Refused until a restart, because adoption runs only at startup.
   - Problem 2: step 6 lazily unmounts "if the path is present", even if the entry is foreign.
   - **Fix:**
     - (a) Remove the outer mount timeout, since the driver already bounds itself; or make it +60s.
     - (b) Step 2: if the entry's marker id equals `id` and the fingerprint matches, return `Ok(handle{pid: None})`; if the fingerprint differs, lazy unmount and continue; if it's foreign, Refused.
     - (c) On timeout, unmount only an entry that carries our marker.

10. **[MED] The fsname fallback is incomplete** (§6). If sshfs ignores the user's `fsname=`, the Linux readiness check (step 6) and `inspect` both require the marker. Every mount would then time out or read as Missing. The fallback must relax both to path + fstype.
    - Evidence it probably works: `strings /usr/bin/sshfs` shows it inserts `-osubtype=sshfs,fsname=%s` at argv[1], so a later user `-o fsname=` should win. S1 should still verify.

11. **[HIGH] The p06 check "second `bifrost --json reconcile` is all noop" fails as designed.**
    - `run.sh` concatenates every phase's config, so psec's `unknown-key` fails permanently.
    - `POST /v1/reconcile` clears `mount_retry_at`, which turns that into a `Mount` action and then `Waiting(InFlight)`. It also means repeated reconciles cause side effects, against PRD §11.
    - **Fix:** remove "clear mount_retry_at" from `POST /v1/reconcile`; `POST …/mount` is already the "retry now" action. Filter the p06 jq to `static1`.

12. **[HIGH] psec fails the M1 gate.** `unknown-key-rc` uses `driver="rclone"`, which is still a safe stub in M1. It sits in `Waiting(NoDriver)` with no `last_error`. **Fix:** move the rclone half into `p09`.

13. **[MED] `pkill -f 'fsname=bifrost:static1@'` also kills the auto_unmount helper** (p06, `sshfs_kill9_auto_unmount_missing`). libfuse's `fusermount3` helper carries `fsname=` in its argv, so SIGKILL takes it out too. The mount then goes Stale instead of Missing, and the unit test becomes flaky. **Fix:** `kill -KILL $(mpid static1)`.

14. **[MED] `Reason::Manual` is never produced** (§5 rows 3 and 5 always use NotDesired). **Fix:** `why = if c.is_some_and(|c| c.held) { Manual } else { NotDesired }`, or delete the variant.

15. **[MED] `failures_reset_only_on_periodic_healthy` can't be tested.** `health()` has no way to know whether an inspect was periodic. **Fix:** add `mounted_at: Option<Instant>` (set in `mount_done(Ok)`), and reset `failures` on Healthy only when `now ≥ mounted_at + retry_max`. Rename the test to match.

16. **[MED] Row 11 can drop a working mount.** It remounts even when the new spec can't mount: the named driver is unavailable, or `online == Some(false)`. The result is a graceful unmount followed by `Waiting` forever. **Fix:** gate row 11 on `cand.driver.is_ok() && cand.online != Some(false)`; otherwise return `Degraded("change pending: …")`.

17. **[MED] `liveness` treats EACCES/EPERM as Degraded** (§6 check). A remote root that isn't searchable causes a lazy-unmount/remount loop every `grace`. **Fix:** any reply from the server, including `NotFound` and `PermissionDenied`, means Healthy.

18. **[MED] Warm-up hole for DNS.** An NXDOMAIN on the DNS index returns `Ok(vec![])`, which counts as "reported". After a restart, adopted DNS mounts then get unmounted. **Fix:** `ready` requires a non-empty `Ok` from each provider, or the grace period.

19. **[HIGH, security] `ids` matches `native_id`, which DNS and HTTP control** (§4). A global `policy.allow ids=["<tailscale node ID>"]` is also satisfied by a DNS node that publishes `id=<same>` with any `host=`. This contradicts §4's claim that "untrusted data can't widen trust". **Fix:**
    - Match `native_id` only in the owning provider's `include_ids` / `exclude_ids`; global `ids` matches the machine id only.
    - Reword §4: global allow rules on id, name, tag or metadata can be satisfied by any provider; scope them with `providers=[…]` or a provider-level include.

20. **[MED, security] HTTP CIDR allow can be bypassed.** `include_cidrs` passes if any IP literal in `addresses` is in range, while the connection goes to `host`, which can be a hostname. **Fix:** in the HTTP provider, when `host` is present, `addresses = [host]`.

21. **[MED] `set_permissions(0o700)` chmods existing directories** (§8 steps 2 and 9). For example, `BIFROST_SOCKET=~/b.sock` would chmod `$HOME` to 0700. **Fix:** chmod only directories the daemon created; the socket itself stays 0600.

22. **[MED] Changing `mount.root` on hot reload breaks mounting.** The actor does no I/O, so the new root is never created or canonicalized, and `mount()` returns Refused because the parent can't be canonicalized. **Fix:** on reload, reject a root change with "mount.root change requires restart" and keep the old config.

23. **[MED] rclone VFS cache may be shared across hosts.** The default `vfs_cache_mode=writes` caches under `~/.cache/rclone/vfs/<fs-name>`. An on-the-fly `:sftp:` remote may key only on the remote path, so two hosts with the same path could share a cache (unverified). **Fix:** add `--cache-dir=<state>/rclone/<id>` per mount; the flag is verified to exist. This also lets pending writes resume when that id is mounted again.

## B. PRD requirements with no concrete home

1. **§20 events.** Nothing says where `MachineEligible` and `DriverUnavailable` are emitted, and no test covers them. **Fix:** the actor keeps the previous verdicts and probe results and emits on a change to Allowed, or Available→Unavailable. Add test `events_eligible_and_driver_unavailable_on_transition`.
2. **§6.1 and §2 root mounts.** PRD §6.1 uses `remote = "/"`, so `"/"` must parse; add it to `remote_path_rules` and assert `sftp_path("/") == "/"`. The discovered-machine default template `remote="~"` differs from §2's `~/machines/agent-01/home/sami/project`; add a §15 row for that.
3. **§29 P0 CI.** CI is skipped under the pre-committed decision, but §15 has no row for it. Add one ("no CI yaml until a git remote exists; `scripts/check.sh` is the gate").
4. **§31 M1 gate.** "bifrostd with the §31 config produces ~/machines/agent-01/" can't pass here (there is no host `agent-01`), it touches the real `~/machines`, and a failed mount leaves an empty directory, so the check proves nothing. **Fix:** make p04 use the §31 shorthand shape (`name/host/user/remote`) with `host="127.0.0.1"` and `port=2222` under `$T`.
5. **§35 "allow/deny filtering works" has no end-to-end test of global policy.** Add `[policy.deny] tags=["misc"]` to p08 and assert other-01's verdict is `denied (policy.deny tags=misc)`. Only one phase fragment may define `[policy.*]`, or the TOML tables collide.
6. **§29 P13 "orphan driver processes".** Nothing checks it. Add to p13a: after restart, `pgrep -xc sshfs` equals 1, meaning adoption didn't duplicate the process.
7. **§29 P13 "macOS permission failures".** Nothing concrete handles this. Add a `last_error`/doctor hint when the log tail matches `kernel extension|System Extension|not permitted`, plus a README note.
8. **§33.10, §33.12, §35 ("core independent of Tailscale/DNS/SSHFS/rclone; plugins fit without redesign").** `ProviderKind{Static,Tailscale,Http,Dns}`, `DRIVER_NAMES`, `default_auto_order` and `dns_label` all live in core, so adding a provider or driver means editing core. **Fix:**
   - `Source { trust: u8, kind: String, provider: String }`, with ranks assigned in bifrost-config: static 0, tailscale 1, http 2, dns 3.
   - `ProviderDto.kind: String`.
   - Move `DRIVER_NAMES` and `default_auto_order` to bifrost-config.
   - Core checks only the grammar of `DriverSelector::Named`.
   - Move `dns_label` to `discovery/dns.rs`.
9. **Missing §15 ceilings:**
   - a provider that fails permanently freezes its last view forever;
   - `bifrostd` and discovery are never darwin-checked. Optionally try once: `SDKROOT=/ CC_aarch64_apple_darwin=true AR_aarch64_apple_darwin=true cargo check --target aarch64-apple-darwin --workspace`. This is an experiment: this host has no clang or xcrun, so ring's C build can't cross-compile otherwise.
10. **Config poller edge cases.** A config file deleted or unreadable at runtime isn't handled; it should keep the active config and warn once. The poller also calls `std::fs::read` inside tokio, which can hang a worker if the config sits on an sshfs/NFS home. Run the poller on a plain `std::thread`.
11. **Startup and wiring gaps:**
    - `<state>/logs` is never created; create it with mode 0700 in startup step 2.
    - `actor::spawn` has no socket parameter to fill `StatusDto.socket`.
    - Handling of a `build_provider` `Err` is unspecified. Proposed: set `ProviderDto.last_error`, spawn no task, and let grace-based warm-up apply.
12. **FakeDriver can't drive the listed daemon tests** (`child_exit_triggers_probe_and_remount`, `exit_during_unmount_ignored`, `api_unmount_busy_stays_degraded`). Add, frozen in S0:
    - `exits: Mutex<BTreeMap<MountId, OnExit>>`
    - `fn exit(&self, id, detail)`
    - `busy: Mutex<BTreeSet<MountId>>`
13. **`argv_never_weakens_host_keys` (C, S1) scans `rclone_argv` (J, S3).** The S0 stub must return `vec![]` rather than `todo!()`, and J extends the test. Add `sftp_server` to the forbidden list; `sshfs -h` shows it takes a remote command.
14. **`probe_fake_sshfs_in_path` needs to mutate PATH.** `set_var` is `unsafe` in edition 2024 and racy between test threads. **Fix:** `check::which_in(name, path: &OsStr)` and an internal `probe_with(path)`.
15. **Unspecified behaviour to pin down:**
    - Ambiguous API targets: a mount id that is also another machine's id. The mount id should win.
    - Where `adopt` gets `pid`: the state record with the same `local_path`, else `None`. p13a asserts "same pid".
    - Availability for a desired mount with a driver error: `Failed` with detail, not `Eligible`.
    - A machine with no mounts: `Eligible`.

## C. Internal inconsistencies

1. §10 calls `crossterm::event::poll`, but crossterm isn't a direct dependency. Use `ratatui::crossterm::event::poll`; `ratatui::run` also exists in 0.30.2.
2. `WaitReason::Backoff(Instant)` can't be displayed as "backoff 3s", because `Display` has no `now`. Use `Backoff(Duration)`, computed in `decide`.
3. `backoff()` computes `failures−1`, which underflows at 0. Use `saturating_sub(1)` and add 0 to `backoff_bounds`.
4. Row 11 says `"change pending: busy"`, but `unmount_done(Err)` emits `"unmount failed: …"`. Pick one string.
5. §8 says "GET handlers only read the watch channel", but the `/log` route reads a file. §11 says the socket is "never under /tmp", but E2E uses `$T/bf.sock` in mktemp. Both should say "default path".
6. The `mount` and `unmount` routes break on the body the client sends. The client always sets `Content-Type: application/json`, so `post(path, &())` sends `null`. I checked axum's `Option<Json<UnmountReq>>`: with a JSON content type it parses the body, so `null` is rejected. **Fix:** the CLI always sends `UnmountReq{force}`, and body-less routes send `{}`.
7. The E2E tool list omits `curl`, which p05 uses (it is installed). `run.sh all` relies on glob order, and `p13_hardening` vs `p13a_adopt` sorts differently under `C` and `en_US.UTF-8` collation; this host uses the latter. Set `LC_ALL=C` or list the files explicitly.
8. §7 applies `?` to hickory's `NetError` inside a function returning `Result<Self, String>`; that needs `.map_err(|e| e.to_string())`. `BIFROST_LOG` has no `error` level.

## D. API and tool claims

All of these check out:

| Area | Verified |
|---|---|
| hickory-resolver 0.26.3 | `builder_tokio`, `builder_with_config`, `NameServerConfig::udp_and_tcp` and `.connections[].port`, `ResolverOpts{timeout, attempts}`, `txt_lookup → Result<Lookup, NetError>`, `Lookup::{answers, valid_until}` (std `Instant`), `Record{ttl, data}`, `TXT.txt_data`, `NetError::is_no_records_found`, `net::runtime::TokioRuntimeProvider: Default`. The crate isn't in the local cache, so the S0 lockfile step needs network. |
| axum 0.8.9 | `impl Listener for UnixListener` is `#[cfg(unix)]`; `keep_alive` is gated by `cfg(feature="tokio")`; `OptionalFromRequest` for `Json` exists. |
| reqwest 0.12.28 | `rustls-tls-native-roots` pulls ring through `__rustls-ring`. There's no clang/xcrun here, so excluding it from the darwin check is justified. |
| ratatui, tokio, libc | ratatui/crossterm 0.29, tokio `process_group`/`kill_on_drop` and libc `getmntinfo` are as stated. |
| MSRV | Every pinned crate needs Rust ≤ 1.88, so `rust-version = "1.89"` works. |
| sshfs 3.7.3 (now installed) | `-f -p -F`, `reconnect`, `idmap`, `transform_symlinks` and `auto_unmount` all exist. `--version` prints everything to stdout. Its built-in ssh-option list includes BatchMode, ConnectTimeout, ControlMaster, ControlPath and ServerAlive*. |
| rclone 1.75.1 | `--config=/dev/null` prints "Configuration is in memory only". `--devname`, `--volname`, `--dir-cache-time`, `--vfs-cache-mode`, `--read-only` and `--cache-dir` exist; the mount-specific ones appear under `rclone mount --help`, not `rclone help flags`. `rclone help flags sftp` lists `--sftp-ssh`, so feature detection works. |

What doesn't hold, or is redundant:
- hickory's `ResolverOpts::default()` already uses a 5s timeout and 2 attempts. The two `options_mut()` lines do nothing for custom nameservers, and for the system resolver they override resolv.conf. Delete them.
- Edition 2024 facts the contract missed: `gen` is reserved (A1), and `std::env::set_var` is `unsafe` (B14).

## E. Ponytail violations (unrequested code or data)

1. `GET /v1/machines/{id}` isn't in the PRD. `machines show` can filter `GET /v1/machines` instead; cut the route.
2. `MountHints.driver`, `Bf1.driver` and the HTTP `driver` field are validated, but no DTO carries them, so they are never used or shown. They also make providers aware of driver names (§33.4). Cut them; `driver=` becomes an ignored key.
3. `MachineRegistry::next_expiry` and its actor deadline are redundant. A pass runs on every provider result, and `expires_at ≥ 3×interval`, so expiry is already accurate to one interval. Cut both.
4. The `ApiCmd::Discover` reply oneshot isn't needed, because the route returns 202 immediately; notify directly. `ApiCmd::Reload` is a duplicate (A4).
5. Redundant fields:
   - `Candidate.fingerprint` duplicates `spec.fingerprint()`;
   - `MachineDto.eligible` duplicates the verdict;
   - `DriverDto.auto_rank` can be derived from `auto_order`;
   - tailscale `metadata.tailscale_id` duplicates `native_id`.
6. Optional: the TUI poller task plus channel could be one `rt.block_on(timeout(500ms, get("/v1/status")))` per second in the UI loop.

### Critical Files for Implementation
- crates/bifrost-core/src/reconcile.rs
- crates/bifrost-core/src/policy.rs
- crates/bifrost-mount/src/lib.rs
- crates/bifrost-daemon/src/actor.rs
- tests/e2e/run.sh