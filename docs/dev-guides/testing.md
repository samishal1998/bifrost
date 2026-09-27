# Testing

This guide explains how Bifröst is tested: the layers from pure core tests to the Docker end-to-end harness, what
each layer can and cannot prove, how to run it, and the conventions every test follows (fakes instead of mocks, no
`set_var`, a caller-chosen search path for binary lookup, temp directories with short socket paths). It ends with a
map from each risk and security principle in the design contract to the tests that pin it. The E2E harness has its
own guide, [e2e-harness.md](e2e-harness.md), and CI is covered in [release-and-ci.md](release-and-ci.md).

## Contents

- [Layers at a glance](#layers)
- [Running each layer](#running)
- [Layer details](#layer-details)
  - [Pure core tests with fakes](#core-fakes)
  - [Crate unit tests](#crate-unit)
  - [Binary-level CLI tests](#cli-binary)
  - [Ignored real-tool tests](#ignored)
  - [E2E harness](#e2e)
  - [Installer test](#installer)
  - [macOS](#macos)
- [Conventions](#conventions)
- [The gate: scripts/check.sh](#gate)
- [CI](#ci)
- [Where each risk and security item is pinned](#pinned)
- [Choosing a layer for a new test](#choosing)

<a id="layers"></a>
## Layers at a glance

| Layer | Where | Proves | Needs | In `check.sh` | In CI |
|---|---|---|---|---|---|
| Pure core | `crates/bifrost-core/src/*.rs` `#[cfg(test)]` | validators, fingerprint, policy, registry merge and expiry, the reconcile decision table, runtime state transitions | nothing | yes | Linux + macOS |
| Crate unit | every other crate's `#[cfg(test)]` modules | config parsing and errors, argv goldens and safety, mount-table parsing, probes against fake binaries, bf1/tailscale/HTTP parsing, the daemon actor with fakes, API routes, client, TUI state and rendering, CLI output goldens | loopback, temp dirs, `/dev/fuse` for three Linux probe tests (`probe_fake_sshfs_in_path`, `probe_fake_rclone_in_path`, `rclone_nfs_unavailable_on_linux`) | yes | Linux + macOS |
| Binary-level CLI | `crates/bifrost-cli/tests/config_check.rs` | the built `bifrost` binary: output, exit codes, request bodies, local fallback | the binary (cargo builds it) | yes | Linux + macOS |
| Ignored real-tool | `#[ignore]` tests in `bifrost-mount` (5) and `bifrost-discovery` (2) | real sshfs/rclone mounts, the ssh preflight, host-key failure, kill -9 behaviour, CoreDNS over UDP+TCP, the live `tailscale status` | Docker sshd (`BIFROST_E2E_SSH`), the p08 CoreDNS container, or a real tailnet | no | no |
| E2E | `tests/e2e/` | the whole system: daemon, CLI, SSE, recovery, adoption, every provider and driver, hot reload, hostile records | Linux, Docker, FUSE, sshfs, rclone, jq, curl, dig, python3 | no | no |
| Installer | `scripts/test-install.sh` | `install.sh` happy path and every refusal | Linux, dash, python3 | no | Linux |
| macOS compile | `cargo check`/`clippy --target aarch64-apple-darwin` (README "Development") | the macOS `cfg` branches compile | the darwin target | no | no (the macos-15 job builds natively instead) |

Unit tests use no network beyond loopback and never need root (contract §12 "Unit tests"). Per-crate counts are in each
crate guide's At a glance table (273 `#[test]`/`#[tokio::test]` functions when these guides were written, 7 of
them `#[ignore]`). Recount with `grep -rE '^\s*#\[(tokio::)?test' crates | wc -l` after adding tests. The sign-offs
record the count at each stage (for example "240 tests" at S4a, commit b46b8d8).

<a id="running"></a>
## Running each layer

```bash
export CARGO_TARGET_DIR=/path/to/shared-target      # optional; see decisions.md#shared-target-dir-and-worktrees

scripts/check.sh                                    # fmt --check, clippy -D warnings, cargo test --workspace
cargo test -p bifrost-core                          # one crate
cargo test -p bifrost-core row11                    # tests whose name contains "row11"
cargo test -p bifrost-cli --test config_check       # the binary-level CLI tests only

# ignored mount tests against the harness's sshd (hold the E2E lock: start_sshd replaces bf-e2e-sshd)
flock -n -o /tmp/bf-e2e.lock bash -c '
  T=$(mktemp -d); source tests/e2e/lib.sh; start_sshd
  cargo test -p bifrost-mount -- --ignored; rc=$?
  stop_sshd; exit $rc'

# ignored CoreDNS test (from its doc comment in crates/bifrost-discovery/src/dns.rs)
flock -n -o /tmp/bf-e2e.lock bash -c '
  T=$(mktemp -d); source tests/e2e/lib.sh; source tests/e2e/p08_dns.sh; setup_p08
  cargo test -p bifrost-discovery -- --ignored coredns_discovery; rc=$?
  docker rm -f bf-e2e-dns >/dev/null; exit $rc'

cargo test -p bifrost-discovery -- --ignored tailscale_live_status   # your real tailnet; prints counts only

tests/e2e/run.sh m1          # or: all, or explicit phase files (e2e-harness.md)
scripts/test-install.sh      # builds the three binaries first unless given a BIN_DIR
```

`start_sshd` and `setup_p08` run `docker rm -f` on the fixed container names `bf-e2e-sshd` and `bf-e2e-dns`, so
running them while `tests/e2e/run.sh` is active would pull the fixture out from under that run. The `flock` wrappers
above (run from the repository root) take the harness's own lock and fail at once when a run holds it; `-o` keeps the
lock's descriptor out of the test processes and any sshfs they leave behind, as `run.sh` does with `9>&-`.

Each ignored mount test mounts under its own root in `$TMPDIR` (`bifrost-mount-e2e-<id>-<random>`). On success
`tidy()` removes it entry by entry; a failed test leaves it behind, possibly with a mount in it, so check
`/proc/self/mountinfo` before deleting one.

<a id="layer-details"></a>
## Layer details

<a id="core-fakes"></a>
### Pure core tests with fakes

`bifrost-core` does no I/O ([decisions.md#core-no-io](decisions.md#core-no-io)), so its tests are plain
`#[test]`s that take `now: Instant` and random inputs as parameters. The planner is a pure function of its inputs
([decisions.md#pure-planner-decision-table](decisions.md#pure-planner-decision-table)), which is what makes one test
per table row possible.

| File | What its tests pin |
|---|---|
| `validate.rs` | every grammar (`name_rejects_traversal`, `host_rejects_option_injection`, `remote_path_rules`, `user_rejects_dash_at_space`, …), `clean`/`tail` (terminal escapes, header skipping), durations, glob, CIDR, FNV-1a vectors, `backoff_bounds` |
| `model.rs` | `fingerprint_changes_on_every_field`, `fingerprint_stable_vector` (a hard-coded hex), `marker_roundtrip_exact`, serde newtypes validating on deserialize |
| `policy.rs` | deny beats allow, global deny beats static, per-provider exclude, discover-only default, CIDR fail-closed, A19 (`global_ids_match_machine_id_only`, `native_id_scoped_to_owning_provider`) |
| `registry.rs` | trust order, "DNS cannot redirect or deny a more trusted machine", age-out instead of removal, freezing on provider failure, the 3 × interval TTL floor |
| `reconcile.rs` | `prd_fake_discovery_fake_driver` (PRD §28's example), `plan_twice_second_all_noop`, `plan_is_deterministic`, one test per decision-table row (`row01_inflight_waits` … `row14_offline_but_healthy_noop`), backoff and failure-reset rules, sticky `auto`, `availability_tables` |
| `fake.rs` | the fakes themselves (`fake_driver_fail_next_state_busy_exit`) |

The fakes live in `crates/bifrost-core/src/fake.rs`. The module is always compiled (not `cfg(test)`) and marked
`#[doc(hidden)]`, so the daemon's tests can use them too.

| Fake | Knobs |
|---|---|
| `FakeDiscovery { name, result }` | `result: Mutex<Result<Vec<MachineObservation>, DiscoveryError>>`, returned on every `discover()` |
| `FakeDriver::new(name)` | `set_state(id, MountState)` overrides `inspect`; `fail_next(id)` fails the next mount once; `panic_next(id)` panics in the next mount; `busy` makes a graceful unmount return `Busy`; `exit(id, detail)` calls the mount's `on_exit`; `calls` records `"mount <id>"` and `"unmount <id> force=<bool>"`; `mounted` is the fake mount table |
| `obs(id, addr)` | a minimal `MachineObservation` |
| `block_on(f)` | polls `f` once with `Waker::noop()` and panics if it is `Pending`. Fakes return `ready(..)` futures, so no runtime is needed |

<a id="crate-unit"></a>
### Crate unit tests

| Crate | Technique | Examples |
|---|---|---|
| `bifrost-config` | `parse(text, path, &env)` with an `env` closure instead of the process environment; `temp_home()` only where `ssh_config` must exist on disk | `parses_full_example` (the contract §3 TOML verbatim), `errors_sorted_deterministic`, `unknown_field_has_line_col`, `trust_ranks_and_sources` |
| `bifrost-discovery` | tailscale: a fake `tailscale` shell script passed as `TailscaleProvider::new(name, Some(path))` plus `crates/bifrost-discovery/tests/fixtures/tailscale_status.json`; HTTP: a tiny tokio `TcpListener` on `127.0.0.1:0` serving a canned response (`serve()` in `http.rs` tests); DNS: the pure `parse_bf1`/`root`/`node` functions | `tailscale_new_peer_appears`, `redirect_not_followed`, `body_cap_enforced`, the `bf1_*` and `inline_*` tests, `system_resolver_has_tcp_fallback` |
| `bifrost-mount` | pure argv builders taking a `Flavor`, so macOS argv is golden-tested on Linux; mountinfo parsing from strings; `probe_with(path)` against fake scripts in a temp dir; `step2`/`adopt` over hand-built `MountEntry` lists | `sshfs_argv_macfuse_golden`, `argv_never_weakens_host_keys`, `positionals_never_start_with_dash`, `mount_step2_own_marker_adopt_or_detach_foreign_refused`, `liveness_keyed_per_instance`, `failed_mount_leaves_no_mountpoint` |
| `bifrost-client` | an axum server (dev-dependency) on a temp socket | `uds_roundtrip`, `not_running_enoent_and_econnrefused` |
| `bifrost-daemon` | `actor.rs`: the real actor with a `Rig` (below); `api.rs`: `router()` on a temp socket with a stub actor task answering `Msg::Api`; `main.rs`: the full `run()` startup with `r.deps()`; `reload.rs`: `Poller::poll(hup)` called directly, no thread, no sleeps | `warmup_protects_adopted_until_ok`, `executor_panic_recovers`, `routes_roundtrip`, `socket_0600_stale_replaced`, `poller_ignores_half_written_change` |
| `bifrost-tui` | `App::on_key` is pure (returns a `Command`); `ui::render` into `ratatui::backend::TestBackend` (100 × 30) | `key_m_emits_mount_for_selection`, `shift_u_asks_confirmation`, `renders_machines_with_glyphs_and_teal`, `no_color_disables_styles` |
| `bifrost-cli` (unit) | `output.rs` renders DTO fixtures to strings | `machines_table_golden`, `mounts_table_golden`, `status_block_golden` |

The daemon test rig (`crates/bifrost-daemon/src/actor.rs`, `mod tests`):

- `rig(name)` makes `std::env::temp_dir()/bf-d-<name>-<pid>` with `state/logs`, then **canonicalizes** it. The daemon
  canonicalizes `mount.root`, and on macOS `temp_dir()` is under `/var`, a symlink to `/private/var`; without this,
  path comparisons fail there (commit 0c7f17a, the macOS CI fix).
- `Rig::text` writes a config with every daemon interval at `1h`, so nothing ticks on its own. A test drives time by
  sending `Msg::Tick(Tick::Health)` or `Msg::Tick(Tick::Fallback)` itself.
- `TestDriver` wraps a `FakeDriver` named `"sshfs"`, so the default Linux `auto_order` picks it, and adds an inspect
  counter and an optional unmount delay.
- `Rig::deps()` returns `Deps` whose `drivers` factory yields the `TestDriver` and whose `build_provider` yields the
  `FakeDiscovery` (or `Err("build failed")` when `fail_build` is set). This is the reason `Deps` holds factories
  (A5): a reload that changes driver settings rebuilds drivers through the factory and still gets the fake.
- `H::until(what, pred)` waits on the status watch channel for at most 5 s and panics with the whole snapshot;
  `H::mount`, `unmount`, `reconcile`, `reload`, `shutdown` go through the same `ApiCmd`/`Msg` paths as the API.

<a id="cli-binary"></a>
### Binary-level CLI tests

`crates/bifrost-cli/tests/config_check.rs` runs the built binary through `env!("CARGO_BIN_EXE_bifrost")`.

- `bifrost(home)` builds the `Command` with `HOME` pointed at a temp dir and `BIFROST_SOCKET`, `BIFROST_CONFIG` and
  `BIFROST_STATE_DIR` removed, so the caller's environment can't leak in.
- `stub(sock, n, reply)` is a raw HTTP/1.1 responder on a `std::os::unix::net::UnixListener` in a thread. It answers
  `n` requests and returns every request line and body it saw, which is how `unmount_sends_force_body` checks the
  exact JSON the CLI sends (C6).
- `exit3_when_daemon_absent` pins the exit-code contract ([decisions.md#cli-exit-codes](decisions.md#cli-exit-codes)).

<a id="ignored"></a>
### Ignored real-tool tests

| Test | File | Needs | Proves |
|---|---|---|---|
| `preflight_exit0_and_hostkey_failure` | `crates/bifrost-mount/src/sshfs.rs` | `BIFROST_E2E_SSH` | the `ssh -s sftp` preflight exits 0 on a good server; an unknown host key fails with "Host key verification failed" and nothing is mounted |
| `sshfs_mount_inspect_unmount` | `crates/bifrost-mount/src/sshfs.rs` | `BIFROST_E2E_SSH` | a real mount; the user `fsname=` marker is the mountinfo source (A10); a second `mount()` adopts; a busy graceful unmount returns `Busy`; unmount is idempotent |
| `sshfs_kill9_auto_unmount_missing` | `crates/bifrost-mount/src/sshfs.rs` | `BIFROST_E2E_SSH` | after SIGKILL the mount reads Missing or Stale (fuse3 3.14 leaves it ENOTCONN), `on_exit` fires, force unmount cleans up |
| `rclone_mount_write_roundtrip` | `crates/bifrost-mount/src/rclone.rs` | `BIFROST_E2E_SSH`, rclone | an rclone mount through `--sftp-ssh`, writes reach the server |
| `rclone_kill9_stale_then_lazy` | `crates/bifrost-mount/src/rclone.rs` | `BIFROST_E2E_SSH`, rclone | SIGKILL → Stale → lazy detach |
| `coredns_discovery` | `crates/bifrost-discovery/src/dns.rs` | the p08 CoreDNS on `127.0.0.1:5353` | the p08 zone over UDP and TCP, NXDOMAIN → empty view, REFUSED → `Failed` |
| `tailscale_live_status` | `crates/bifrost-discovery/src/tailscale.rs` | a logged-in `tailscale` | the parser on real data; prints counts only |

`BIFROST_E2E_SSH=host:port:user:ssh_config` is exported by `start_sshd` in `tests/e2e/lib.sh`. The helper `e2e()` in
`crates/bifrost-mount/src/lib.rs` splits it and mounts under a fresh root that doubles as the driver's state dir.

<a id="e2e"></a>
### E2E harness

`tests/e2e/run.sh` builds the workspace, starts a throwaway sshd (and CoreDNS and an HTTP inventory for `all`), runs
one daemon from a scratch directory and asserts through the CLI, `/proc/self/mountinfo` and the event ring. See
[e2e-harness.md](e2e-harness.md) for every phase and helper, and
[decisions.md#e2e-docker-harness](decisions.md#e2e-docker-harness) for why it is bash and Docker.

<a id="installer"></a>
### Installer test

`scripts/test-install.sh [BIN_DIR]` packages the host's binaries into a fake release on `127.0.0.1` and runs
`install.sh` against it: the happy path under dash and bash, from the file and piped, plus a corrupted tarball, a
missing `SHA256SUMS`, a hostile `BIFROST_VERSION`, an unset `HOME` and an unwritable directory. Details in
[release-and-ci.md](release-and-ci.md#test-install).

<a id="macos"></a>
### macOS

There is no macOS machine in the development loop. Two things cover the macOS code:

- The argv builders take a `Flavor` (`Linux`, `MacFuse`, `FuseT`), so the macOS argv is golden-tested on Linux
  (`sshfs_argv_macfuse_golden`, `sshfs_argv_fuset_golden`, `rclone_argv_nfsmount_forces_writes`).
- The CI `macos` job (macos-15) runs `cargo build --workspace --all-targets` and `cargo test --workspace` on real
  macOS. Tests that only make sense on Linux are `#[cfg(target_os = "linux")]` (for example
  `probe_fake_sshfs_in_path`, which needs `/dev/fuse`), and assertions that differ carry a `cfg!(target_os = "macos")`
  branch (for example the unmarked-entry case in `adopt_marker_record_foreign_outside_root`).

Nothing mounts on macOS in any test: macFUSE, FUSE-T and `nfsmount` behaviour is unproven (contract §15 #25).

<a id="conventions"></a>
## Conventions

**Tests first, named in the contract.** Every stage of the build was TDD: the implementer wrote the tests the
contract named (§12 and the sign-offs) before the code, and reviewers checked the names against the contract. A test
name states the behaviour it pins, not the function it calls: `busy_unmount_backoff_survives_healthy_probe`,
`reload_unbuildable_provider_freezes_observations`. Recurring shapes:

| Shape | Meaning | Example |
|---|---|---|
| `rowNN_*` | one row of the reconcile decision table (contract §5) | `row10_stale_remount_force` |
| `*_golden` | exact output: argv, CLI tables, status block | `rclone_argv_mount_golden` |
| `*_rejected` / `*_refused` | a validation or a startup refusal | `header_crlf_rejected`, `world_writable_root_refused` |
| `*_never_*` | a safety property checked over many inputs | `argv_never_weakens_host_keys` |
| a review finding's behaviour | a regression test added with each fix, cited in the sign-off | `liveness_keyed_per_instance` (S4a sign-off 11) |

The display strings for verdicts, actions, wait reasons and `Reason` are final (S1 sign-off 7); the golden tests lock
them in, and the E2E asserts some of them verbatim (`"allowed (dns.filter.include)"`,
`"denied (policy.deny tags=misc)"`). Changing one means updating goldens, E2E phases and the site.

**No `set_var`.** `std::env::set_var` is `unsafe` in edition 2024 and races across test threads (B14). Code that
reads the environment takes it as a parameter instead:

| Instead of the environment | Tests pass | Where |
|---|---|---|
| `$PATH` for binary lookup | a temp dir to `check::which_in(name, path)` and to each driver's `probe_with(path)` | `crates/bifrost-mount/src/check.rs`, `sshfs.rs`, `rclone.rs` |
| `$PATH` for `tailscale` | a path to the private `which_in(name, path)`, which appends the fixed directories itself | `crates/bifrost-discovery/src/tailscale.rs` |
| `HOME` and `$VAR` expansion | an `env: &dyn Fn(&str) -> Option<String>` closure to `bifrost_config::parse` | `crates/bifrost-config/src/lib.rs` |
| the CLI's environment | `Command::env` / `env_remove` on the child | `crates/bifrost-cli/tests/config_check.rs` |

`check::which_in` searches only the given list (and skips relative entries). The fixed fallback directories
(`/usr/local/bin:/usr/bin:/bin`, plus `/opt/homebrew/bin` on macOS) are added by `which()`/`search_path()`, so a test
with a temp dir sees exactly its fake binaries.

**Fake binaries are shell scripts.** Tests write `#!/bin/sh` scripts into a temp dir with mode 0755
(`script()` in `sshfs.rs` tests, `fake()` in `tailscale.rs` tests). `failed_mount_leaves_no_mountpoint` sleeps 200 ms
after writing its scripts: a child forked by another test thread can hold the new file's write descriptor until it
execs, which makes exec fail with `ETXTBSY`.

**Temp directories and short socket paths.** Each test makes its own directory under `std::env::temp_dir()`, named
`bf-<area>-<test>-<pid>` (`bf-d-*`, `bf-api-*`, `bf-cli-*`, `bf-reload-*`, `bf-config-*`) or
`bifrost-mount-<name>-<random>` in the mount crate; most pid-named ones first remove a leftover of the same name (not `temp_home()`'s `bf-config-*` or the
`bf-cli-config-check-*` directory of `config_check_output_deterministic`, which only `create_dir_all`).
Sockets go directly inside it with a short name (`s.sock`), or are a short file themselves
(`bf-client-<name>-<pid>.sock`). A Unix socket path must fit in 103 bytes; `bind_socket` in
`crates/bifrost-daemon/src/main.rs` refuses a longer one (pinned by `preexisting_parent_dirs_not_chmodded`), and
macOS's per-user `$TMPDIR` is already long. Keep test names short for the same reason.

**Never delete through a mount.** `Tmp` (mount crate) removes itself on drop and is never used for a mount. The
roots of the ignored mount tests come from `fresh_dir()` and are cleaned by `tidy()`, which removes entries one by
one and fails rather than recurse. The E2E harness keeps its scratch directory for the same reason.

**Kill by pid, never by pattern.** Tests signal a mount's process only by its handle's pid (`MountHandle.pid`, or
`mpid` in the E2E). `pkill -f fsname=…` would also kill the setuid `fusermount3` `auto_unmount` helper, whose argv
carries the same `fsname=` (A13).

**Bounded waits, no timing races.** Unit tests don't wait on tickers: the actor rig uses 1 h intervals and explicit
ticks, and the poller tests call `poll()` directly. The few sleeps are deliberate and short: letting a real deadline
or grace pass (`missing_config_never_unmounts_adopted`, `next_wakeup_never_spins`,
`reload_unbuildable_provider_freezes_observations` in `actor.rs`; `timed_guard_single_thread`,
`liveness_keyed_per_instance` in `check.rs`) or the 200 ms `ETXTBSY` window. Every wait (`H::until`, `H::api`) has a
timeout that fails with the state it saw.

**Private data.** `tailscale_live_status` prints counts only, and p07's FAIL lines print counts only; tailnet names
must never land in test output, fixtures or commits.

<a id="gate"></a>
## The gate: scripts/check.sh

```bash
cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

- It uses cargo's normal target directory unless `CARGO_TARGET_DIR` is set (commit 18076c8 removed a hard-coded
  local one).
- There is no `rustfmt.toml`: rustfmt defaults (P1).
- It does **not** run the ignored tests, the E2E harness, `scripts/test-install.sh` or the darwin check.
- During the build, the orchestrator ran it after every branch merge (contract §1 rule 4). Every change should pass
  it; CI runs it on Linux.

<a id="ci"></a>
## CI

`.github/workflows/ci.yml` runs on pushes to `main` and on pull requests:

| Job | Runner | Steps |
|---|---|---|
| `linux` | ubuntu-24.04 | `scripts/check.sh`, `cargo build --workspace`, `bash scripts/test-install.sh` |
| `macos` | macos-15 | `cargo build --workspace --all-targets`, `cargo test --workspace` |

Neither job runs the ignored tests or the E2E harness (they need Docker, FUSE and a real sshd), and the macOS job runs
no fmt or clippy. See [release-and-ci.md](release-and-ci.md#ci) for caching, permissions and the known redundancy.

<a id="pinned"></a>
## Where each risk and security item is pinned

The contract lists the top risks the design pre-empts (§14) and the security checklist (§11, PRD §23). This is where
each is held in place by a test. Unit tests are named; E2E phases are in [e2e-harness.md](e2e-harness.md#phases).

### Top risks (contract §14)

| # | Risk | Unit tests | E2E |
|---|---|---|---|
| 1 | A hung FUSE mount freezes the daemon | `timed_guard_single_thread`, `liveness_keyed_per_instance`, `run_times_out_and_captures` (check.rs); `mountinfo_*` (the table is read from `/proc`, never by touching a mount) | p06: `docker stop` → degraded while `kill -0 $DPID` holds |
| 2 | Data loss on unmount or remount | `row05_not_desired_graceful`, `gate_u_unmount_backoff`, `busy_unmount_backoff_survives_healthy_probe`, `api_unmount_busy_stays_degraded`, `prepare_mountpoint_creates_rejects_symlink_file_nonempty`, `busy_matches_the_errno_text_not_the_path`, `state_json_atomic_corrupt_quarantined`; ignored `sshfs_mount_inspect_unmount` (busy refused) | p04 unmount → directory removed only once unmounted |
| 3 | The startup race unmounts adopted mounts | `warmup_protects_adopted_until_ok`, `missing_config_never_unmounts_adopted`, `empty_config_file_not_loaded`, `row04_warmup_blocks_removal`, `row11_gated_on_driver_and_online` | p13a |
| 4 | Daemon death kills or corrupts children | `shutdown_writes_state_and_leaves_mounts`, `adopt_marker_record_foreign_outside_root` | p13a: `kill -9 bifrostd`, mount keeps working, same pid after restart, no duplicate sshfs |
| 5 | Remount storms | `auto_driver_sticky_fingerprint`, `row14_offline_but_healthy_noop`, `absent_ages_out_not_removed`, `failed_provider_freezes_expiry`, `ttl_floor_three_intervals`, `failures_reset_only_after_stable_healthy`, `reload_unbuildable_provider_freezes_observations` | p10 (HTTP 401 keeps the mount), p12 (`auto_order` swap remounts nothing), p13 (duplicate not remounted) |
| 6 | Fighting sshfs `reconnect` | `row13_degraded_within_grace`, `row12_grace_elapsed_lazy_unmount`, `grace_unmount_sets_offline` | p06: degraded → restored; offline after grace |
| 7 | Untrusted discovery escalates | `dns_cannot_redirect_tailscale_machine`, `dns_tags_cannot_deny_static_machine`, `native_id_scoped_to_owning_provider`, `hints_ignored_unless_honor_hints`, `bf1_host_injection_rejected`, `invalid_entries_isolated`, `host_overrides_addresses`, `hostile_peer_fields_rejected` | p08 (bad-node, evil, canary file), p10 (`inv-bad`, `inv-host`, `../x`), p13 (duplicate stays with the more trusted source) |
| 8 | rclone `--sftp-ssh` edge cases | `rclone_sftp_ssh_tokens_clean_cfg_quoted`, `rclone_never_uses_internal_ssh`, `probe_fake_rclone_in_path` | p09 |
| 9 | The SSH environment under a service manager | `tailscale_which_falls_back_to_fixed_dirs` (an empty `PATH` still finds binaries in the fixed directories); the mount crate's `check::search_path()` fallback and the `ssh_agent` report are not pinned by a test (`which_respects_exec_bit` only proves that executable, absolute entries are used) | psec and p09 surface "Host key verification failed" in `last_error` |
| 10 | macOS can't be run here | the `Flavor` goldens; `doctor_config_drivers_and_macos_hint` (the B7 permission hint fails doctor) | none; CI macos-15 runs the unit tests |
| 11 | Suspend and wake | not pinned (relies on `Instant` excluding suspend; no wake detector, §15 #14) | none |
| 12 | Parallel agents break each other | process, not tests: frozen signatures, worktrees ([decisions.md#shared-target-dir-and-worktrees](decisions.md#shared-target-dir-and-worktrees)) | none |
| 13 | Unverified tool behaviour | ignored `preflight_exit0_and_hostkey_failure`, `sshfs_mount_inspect_unmount` (fsname), `sshfs_kill9_auto_unmount_missing` | p06 (kill -9 goes through Stale) |

### Security checklist (contract §11)

| # | Principle | Unit tests | E2E |
|---|---|---|---|
| 1 | Discovery ≠ trust | `no_allow_rule_is_discover_only`, `global_ids_match_machine_id_only`, `api_mount_forbidden_for_discover_only`, `mount_forbidden_maps_403` | p07 (zero mounts), p08 (other-01 denied, 20 bulk nodes discover-only) |
| 2 | No SSH passwords | `preflight_argv_golden` and the argv goldens (`BatchMode=yes`), `unknown_field_has_line_col` (`deny_unknown_fields`: no password key can exist) | none |
| 3 | Prefer agent, keys, Tailscale SSH | `rclone_never_uses_internal_ssh` (the system `ssh` is always used); the agent line in `doctor`/`status` is not pinned | none |
| 4 | Never weaken host verification | `argv_never_weakens_host_keys`, `ssh_never_forwards_agent_x11_or_ports` | psec, p09 (`unknown-key-rc`) |
| 5 | Mount only explicit paths | `hints_ignored_unless_honor_hints`, `remote_path_rules`, `sftp_path_mapping` | p08 and p10 (hints used only with `honor_hints`) |
| 6 | Never execute discovery-supplied commands | `host_rejects_option_injection`, `positionals_never_start_with_dash`, `bf1_host_injection_rejected`, `rclone_sftp_ssh_tokens_clean_cfg_quoted` | p08 canary `$T/pwned` never created |
| 7 | TXT and HTTP are untrusted | the `bf1_*` tests, `max_nodes_caps_inline_plus_index`, `body_cap_enforced`, `entries_capped_at_1000`, `redirect_not_followed`, `transport_error_names_cause_not_url`, `clean_strips_escapes`, `third_party_debug_logs_capped` | p10: the token never reaches the log |
| 8 | Local path traversal and collisions | `name_rejects_traversal`, `local_traversal_rejected`, `duplicate_local_rejected`, `static_local_collision_conflict`, `root_slash_home_relative_rejected`, `log_route_rejects_traversal` | p04 (under `$T/machines` only) |
| + | Local API | `socket_0600_stale_replaced`, `preexisting_parent_dirs_not_chmodded`, `second_instance_lock_refused`, `world_writable_root_refused`, `world_writable_state_dirs_refused` | p05: socket mode 600, second daemon refused, exit 3 |

[security.md](security.md) explains each mechanism.

<a id="choosing"></a>
## Choosing a layer for a new test

1. A rule about data, grammar, policy, merging or planning: a core test. If it needs a driver or provider, use the
   fakes and `block_on`.
2. A config key or error: `bifrost-config`'s `mod tests` with the `p`/`ok`/`errs`/`assert_has` helpers.
3. Anything that builds an argv: a golden test **and** add the new argv to the loops in
   `argv_never_weakens_host_keys`, `ssh_never_forwards_agent_x11_or_ports` and `positionals_never_start_with_dash`.
4. Daemon behaviour over time (warm-up, reload, events, API commands): an actor test with the `Rig`.
5. What the user sees from `bifrost`: an `output.rs` golden, plus a binary test with `stub()` if it involves exit codes
   or request bodies.
6. Behaviour of a real tool (sshfs, rclone, ssh, CoreDNS): an ignored test that reads `BIFROST_E2E_SSH`, and an E2E
   check if it matters end to end.
7. Anything that crosses processes (adoption, signals, SSE, reload by file edit): an E2E phase.

[extending.md](extending.md) lists the exact tests to add for each kind of change.
