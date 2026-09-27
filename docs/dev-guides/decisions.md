# Design decisions

This is the decision log for Bifröst: one entry per design choice that shapes the code, with its status, the problem
it answers, what was decided, what was rejected and why, what it costs, where it lives and where the reasoning is
recorded. Read it before changing any of these choices, and add an entry when you make a new one. The code is the
source of truth. Where `docs/design/contract.md` says something different, the entry follows the code and says so.
The mechanics of each subsystem are in [architecture.md](architecture.md) and the [crate guides](crates/). The corners
that were cut on purpose are listed in [simplifications.md](simplifications.md), and the threat model is in
[security.md](security.md).

## Contents

| Area | Decisions |
|---|---|
| [Workspace and house style](#area-workspace) | [Rust, 8 crates](#rust-and-8-crate-workspace) · [core does no I/O](#core-no-io) · [`BoxFuture`, not async-trait](#boxfuture-not-async-trait) · [ponytail](#ponytail-style) · [worktrees and a shared target dir](#shared-target-dir-and-worktrees) |
| [Daemon](#area-daemon) | [single actor](#single-actor-daemon) · [pure planner](#pure-planner-decision-table) · [generations](#generations-for-stale-results) · [HTTP over a Unix socket](#http-over-unix-socket) · [SSE and polling clients](#sse-events-and-polling-clients) · [panics caught](#panics-caught-at-driver-boundary) · [log filter allowlist](#log-filter-allowlist) · [state.json is a hint](#state-json-is-a-hint) · [private directories](#private-state-and-socket-dirs) |
| [Discovery, trust and policy](#area-discovery) | [discover is not mount](#discover-is-not-mount) · [winner takes all](#winner-takes-all-trust) · [policy semantics](#policy-semantics) · [warm-up](#warm-up-readiness) · [validated newtypes](#validated-newtypes-at-trust-boundary) · [static is config](#static-provider-is-config) · [tailscale CLI JSON](#tailscale-via-cli-json) · [bf1 format](#bf1-dns-format-inline-and-index) · [DNS over TCP](#dns-tcp-fallback) · [HTTP limits](#http-inventory-limits) |
| [Mounting](#area-mounting) | [mount table](#mount-table-not-path-existence) · [sshfs child](#sshfs-foreground-child) · [ssh preflight](#ssh-preflight) · [host keys](#host-keys-never-weakened) · [rclone over system ssh](#rclone-via-system-ssh) · [no pid signalling](#no-pid-signalling-lazy-detach) · [busy never forced](#busy-unmount-never-forced) · [hung-FUSE guards](#hung-fuse-guards) · [adoption](#adoption-by-marker-and-fingerprint) |
| [Configuration](#area-config) | [deterministic validation](#config-deterministic-validation) · [polling reload](#config-polling-not-notify) · [missing config, root changes](#missing-config-empty-default) |
| [Clients](#area-clients) | [CLI exit codes](#cli-exit-codes) · [TUI inline polling](#tui-inline-polling) |
| [Testing, release and docs](#area-release) | [E2E harness](#e2e-docker-harness) · [release targets](#release-static-musl-and-darwin) · [install.sh](#install-sh-verify-always) · [docs site](#docs-starlight-on-pages) |
| | [How sources are cited](#citations) · [Adding a decision](#adding-a-decision) |

<a id="citations"></a>
## How sources are cited

| Citation | Where to find it |
|---|---|
| PRD §n | `bifrost_prd_and_implementation_plan.md` |
| contract §n | `docs/design/contract.md` |
| Changes from B #n | the first table in the contract (the defects of the panel's design B and their fixes) |
| A1–A23, B1–B15, C1–C8, D1, E1–E6, P1–P2 | the contract's "Amendments (applied)" table. The findings behind them are in `docs/design/critique.md` (A = defects, B = missing homes, C = inconsistencies, D = API claims, E = ponytail cuts, P = orchestrator additions) |
| S1 sign-off n, S2 sign-off n, S3 sign-off n / carry-over n, S4a sign-off n | the contract's "Orchestrator sign-offs (after S1 / S2 / S3 / S4a)" sections. They override earlier contract text |
| r1, r2, r3 | the three rounds of the final whole-repo review (S4a sign-offs 6–13 and the `fix(review-rN)` commits) |
| §15 #n | contract §15, "Deliberate simplifications" (see [simplifications.md](simplifications.md)) |
| a short hash | a commit: `git show <hash>` |

Each entry has the same fields: **Status**, **Context**, **Decision**, **Alternatives rejected**, **Consequences and
limits**, **Where in code**, **Source**. When no reason was recorded anywhere, the entry says "not recorded" and marks
any reasoning it adds as an inference.

---

<a id="area-workspace"></a>
## Workspace and house style

<a id="rust-and-8-crate-workspace"></a>
### Rust, in an 8-crate workspace

**Status:** accepted in S0 (6ec62be). It replaced design B's 10-crate layout.

**Context.** PRD §4 chose Rust for trait boundaries between discovery and mounting, one core library shared by the
daemon, CLI and TUI, process lifecycle handling, tokio, ratatui and static binaries. It also ruled out a native FUSE
filesystem in V1: mounting is a lifecycle wrapper around sshfs and rclone. PRD §14 sketches 14 crates, then says
"do not over-fragment" and offers an 8-crate practical layout. Design B had 10 crates, and nix, bytes, async-trait and
clap in the daemon (Changes from B #20).

**Decision.** Eight crates, the PRD §14 practical layout. A crate is split out only for PRD layering or to isolate
dependencies, never to let agents work in parallel (worktrees do that, see
[shared-target-dir-and-worktrees](#shared-target-dir-and-worktrees)).

| Crate | Role | Notable dependencies |
|---|---|---|
| `bifrost-core` | models, validation, policy, registry, pure planner, DTOs, fakes | serde, thiserror only |
| `bifrost-config` | TOML → validated `Config`, default paths, trust ranks, driver names | core, toml |
| `bifrost-discovery` | tailscale, DNS TXT and HTTP providers | hickory-resolver, reqwest (rustls), tokio |
| `bifrost-mount` | mount table, timed checks, sshfs and rclone drivers | tokio; libc on macOS only |
| `bifrost-client` | HTTP-over-UDS client | hyper, hyper-util, http-body-util |
| `bifrost-daemon` (bin `bifrostd`) | actor, API, startup, reload poller, state.json | axum, futures-util, tracing-subscriber |
| `bifrost-cli` (bin `bifrost`) | commands, tables, `doctor` | clap, client, config, mount (for local probes) |
| `bifrost-tui` (bin `bifrost-tui`) | ratatui UI | ratatui, client, config |

Providers and drivers are compiled in (PRD §25). The workspace uses edition 2024, `rust-version = "1.89"` (for
`File::try_lock`) and resolver 3. The release profile is `lto = "thin"`, `codegen-units = 1`, `strip = true`, and
deliberately not `panic = "abort"`, because a panicking task must not kill the daemon. Compared with the PRD §30
list, async-trait, notify, globset, nix and a direct crossterm dependency are gone: async-trait for
[`BoxFuture`](#boxfuture-not-async-trait), notify for [polling](#config-polling-not-notify), globset because policy
globs need only `*` and `?` (`validate::Glob`, §15 #4), and crossterm is reached through `ratatui::crossterm` (C1). The
contract records nix's removal but not the reason; the code shows what replaced it: std `File::try_lock` for the lock
(hence `rust-version = "1.89"`), tokio's `process_group`, `kill_on_drop` and signal handling, and `libc::getmntinfo`
for the one macOS syscall. axum, hyper, hyper-util,
http-body-util and futures-util were added; all of them were already compiled as dependencies of axum or reqwest
(contract §1).

**Alternatives rejected.**
- Go. PRD §4: it has no advantage when the product orchestrates external tools instead of implementing a filesystem.
- The 14-crate PRD layout or design B's 10 crates. They add manifests and cross-crate signatures without a
  dependency or release boundary to justify them.
- One crate per provider or driver. PRD §14 says to split "only when their dependencies or release boundaries justify
  it". Neither applies yet.
- Dynamic plugin libraries. PRD §25: no stable Rust ABI, harder distribution, security and testing. Future external
  plugins should be separate processes.

**Consequences and limits.**
- Adding a provider or driver touches `bifrost-discovery` or `bifrost-mount`, `bifrost-config` (`TRUST`,
  `DRIVER_NAMES`) and the daemon's `build_provider`, never core (see [extending.md](extending.md)).
- `bifrost-discovery` pulls in reqwest → rustls → ring, which has C code. That kept the daemon and discovery out of
  the original Linux-hosted darwin cross-check (§15 #31). The CI macOS job now builds and tests the whole workspace.
- The CLI links `bifrost-mount` so `doctor` and `drivers` can probe locally when the daemon is down.

**Where in code.** `Cargo.toml`, `crates/*/Cargo.toml`.

**Source.** PRD §4, §14, §25, §30; contract §1; Changes from B #20; 6ec62be.

<a id="core-no-io"></a>
### Core does no I/O and knows no provider or driver

**Status:** accepted in S0. B8 later removed the last provider and driver knowledge from core.

**Context.** PRD §15 and §33.10 require `bifrost-core` to know nothing about Tailscale, DNS, SSHFS, rclone, terminals
or Unix sockets. Design B had a tokio `watch::Receiver` inside `MountHandle` and a `HostKeyChecking` enum in core
(Changes from B #16). The critique (B8) found `ProviderKind`, `DRIVER_NAMES`, `default_auto_order` and `dns_label` in
core, so adding a provider meant editing core.

**Decision.**
- `bifrost-core` depends only on serde and thiserror. It never touches the filesystem, network, processes or the
  clock. Time and randomness are parameters: `MountRuntime::mount_done(generation, r, now, t, rand)`,
  `reconcile::next_wakeup(runtimes, grace, now)`, `validate::backoff(failures, initial, max, rand)`. The actor passes
  `Instant::now()` and `validate::random_u64()` (a std-only helper; the planner never calls it).
- Child exit reaches the daemon through `OnExit`, a boxed `FnOnce(String)` inside `MountRequest`, not a tokio channel.
- A provider is a string with a numeric rank: `registry::Source { trust: u8, kind, provider }`. The ranks live in
  `bifrost_config::TRUST`; driver names and the auto order in `bifrost_config::{DRIVER_NAMES, default_auto_order}`.
  Core checks only the grammar of `DriverSelector::Named`. `dns_label` lives in `bifrost_discovery::dns`.
- `core/src/fake.rs` (`FakeDiscovery`, `FakeDriver`, `block_on` with `Waker::noop`) is always compiled and
  `#[doc(hidden)]`, so the planner is tested with fakes and no runtime.

**Alternatives rejected.**
- tokio types in core (design B). They tie the planner to one runtime and make it untestable without one.
- A `ProviderKind` enum and driver lists in core (B8). Every new provider or driver would edit core, against PRD §33.10
  and §35.

**Consequences and limits.**
- The whole planner, policy and registry run in plain `#[test]`s (`prd_fake_discovery_fake_driver`,
  `plan_twice_second_all_noop`).
- `MountRuntime` stores no id, so a `MountFailed` from `mount_done(Err)` carries an empty `mount`. The actor stamps the
  id it keys the runtime by into every event (`actor::stamp`, S1 sign-off 2).

**Where in code.** `crates/bifrost-core/Cargo.toml`; `crates/bifrost-core/src/lib.rs`; `reconcile.rs`;
`registry.rs` (`Source`); `model.rs` (`OnExit`, `MountRequest`); `fake.rs`; `crates/bifrost-config/src/lib.rs`
(`TRUST`, `DRIVER_NAMES`, `default_auto_order`); `crates/bifrost-discovery/src/dns.rs` (`dns_label`).

**Source.** PRD §15, §33.10, §35; contract §2; Changes from B #16; B8; S1 sign-off 2.

<a id="boxfuture-not-async-trait"></a>
### `BoxFuture` alias instead of async-trait; no context parameters

**Status:** accepted in S0. Recorded as §15 #1.

**Context.** PRD §5 sketches `#[async_trait]` traits whose methods take `&DiscoveryContext` / `&DriverContext`, with
`inspect -> Result<MountState, MountError>` and `unmount(handle)`. The daemon holds drivers and providers as
`Arc<dyn MountDriver>` and `Arc<dyn DiscoveryProvider>`, so the traits must be dyn-compatible.

**Decision.** `pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>`. Implementations write
`Box::pin(async move { .. })`. The context parameters are dropped (there is nothing to carry). `inspect` returns a
plain `MountState` (errors fold into `Degraded`). `unmount(&handle, force)` borrows the handle and takes `force`.

**Alternatives rejected.**
- The async-trait crate: a proc-macro dependency for what one type alias does.
- Native `async fn` in traits: not dyn-compatible, and the daemon needs `dyn`.

**Consequences and limits.** Implementations are a little noisier. Adding runtime context later changes both trait
signatures (the §15 #1 upgrade path: `DiscoveryContext` / `DriverContext` when a plugin needs them).

**Where in code.** `crates/bifrost-core/src/lib.rs` (`BoxFuture`, `DiscoveryProvider`, `MountDriver`).

**Source.** PRD §5; contract §2; Changes from B #20; §15 #1.

<a id="ponytail-style"></a>
### Ponytail: the laziest solution that works, with every corner cut recorded

**Status:** accepted. It is the house style named in the contract's first paragraph.

**Context.** The judges flagged design B for "knob and feature sprawl" (Changes from B #15). A multi-agent build
needed a shared rule for how much to build.

**Decision.** Build the simplest thing that works. Every deliberate simplification with a known ceiling gets a
`// ponytail: <ceiling>; <upgrade path>` comment where it lives and a row in contract §15. The final review had a
dedicated "ponytail over-engineering" lens (contract §13, S4). Cuts made under this rule include:
- from design B: ssh timeout knobs, a stat timeout, a dir-cache-time knob, an HTTP timeout knob, `auto_mount`, clap
  in the daemon, `/v1/snapshot`, an `events` CLI command, `FailureKind`, the context parameters and extra events;
- from the critique: `GET /v1/machines/{id}` (E1), every driver hint (E2), `next_expiry` (E3), the Discover reply
  and `ApiCmd::Reload` (E4), redundant fields (E5), the TUI poller task (E6).

**Alternatives rejected.**
- A knob for every constant. Each knob is another test case and another doc entry. Constants that nobody has needed
  to change stay constants.
- Unrecorded shortcuts. Without the comment and the ledger, nobody can tell a deliberate ceiling from a bug.

**Consequences and limits.** Many values are constants (SSH timings, the 5 s probe, HTTP 10 s / 1 MiB, rclone's
15 s dir cache). The ledger is [simplifications.md](simplifications.md). A change that lifts a ceiling should delete
the comment and update the ledger.

**Where in code.** Every `ponytail:` comment (`git grep -n "ponytail:"`).

**Source.** contract introduction, §13 (S4 review lenses), §15; Changes from B #15; critique §E.

<a id="shared-target-dir-and-worktrees"></a>
### Parallel agents: git worktrees, one shared `CARGO_TARGET_DIR`, frozen signatures

**Status:** accepted. P2 replaced contract §1 rule 3 (per-worktree builds).

**Context.** The code was built by AI agents in stages S0–S4 (contract §13), up to five in parallel, on a host with
4 CPUs and about 2 GB of free RAM, with at most 2 agents running at once.

**Decision.**
- Agents are isolated by git worktrees (`git worktree add <dir> -b <agent>`), not by crate boundaries. Each agent
  owns an exclusive set of files.
- All agents share one `CARGO_TARGET_DIR`, so dependencies compile once (P2).
- S0 froze every manifest, `Cargo.lock`, every public signature (as compiling stubs) and the daemon wiring (`Msg`,
  `Tick`, `Deps`, `spawn`, `ApiCmd`, `AppState`: A3). A signature changes only with the orchestrator's sign-off.
- Safe-stub rule: an unimplemented provider returns `Err(Unavailable)` and an unimplemented driver `Unavailable`, so
  the milestone-1 daemon never reaches a `todo!()` (Changes from B #17).
- The orchestrator merges one branch at a time and runs `scripts/check.sh` after each merge.

**Alternatives rejected.**
- A target dir per worktree: duplicate dependency builds on a RAM-constrained host (P2).
- Splitting crates to parallelise work: crates are split only for layering or dependencies (contract §1).

**Consequences and limits.**
- Only one cargo build can run at a time on a shared target dir.
- `tests/e2e/run.sh` copies the binaries into `$T/bin` after building, so a parallel build elsewhere can't replace
  `bifrostd` mid-run (S2 sign-off 4, 280d909). `scripts/check.sh` honours `CARGO_TARGET_DIR` when it is set
  (18076c8 removed a hard-coded one).
- Some shapes are explained only by the freeze: `Deps` carries `#[allow(clippy::type_complexity)] // frozen shape
  (A5)`, and `actor::spawn` takes eight arguments.

**Where in code.** `scripts/check.sh`; `tests/e2e/run.sh`; `crates/bifrost-daemon/src/actor.rs` (`Msg`, `Tick`,
`Deps`, `spawn`); `crates/bifrost-daemon/src/api.rs` (`ApiCmd`, `AppState`).

**Source.** contract §1 (rules), §13; A3; P2; Changes from B #17; S2 sign-off 4; 280d909, 18076c8.

---

<a id="area-daemon"></a>
## Daemon

<a id="single-actor-daemon"></a>
### One actor task owns all daemon state

**Status:** accepted (S2-E, 6e7ee55).

**Context.** The daemon reacts to provider results, driver results, health probes, child exits, API calls, config
reloads and timers, all concurrently. Any filesystem call on a hung FUSE mount can block forever.

**Decision.**
- `Actor` in `crates/bifrost-daemon/src/actor.rs` owns the config, the registry, the mount runtimes, the holds, the
  probe results, the previous verdicts, the provider status and the event ring. Domain state has no locks.
- The inbox is `mpsc::unbounded_channel::<Msg>()`. `on_exit` is a synchronous callback that must never block, and the
  producers are rate-limited by their own intervals.
- `Actor::run` selects over the inbox, `sleep_until(deadline())` and finished executors. It drains the inbox, handles
  the whole batch (a `Shutdown` is taken last, so a `MountDone` queued behind it still reaches state.json, 73237db),
  runs one `pass`, then publishes a `watch::Sender<Arc<StatusDto>>` snapshot. Events go to a `broadcast` channel
  (capacity 256) and a ring of the last 200.
- The actor never awaits I/O and never touches a mount path. Driver calls run in executor tasks (`start_mount`,
  `start_unmount`, `inspect`, `probe`) that report back through `Msg`. Two exceptions do small, local, synchronous
  I/O on the actor:
  - state.json, written synchronously (S2 sign-off 1; a `ponytail:` comment on `Actor::persist`);
  - building an HTTP provider. `Actor::start_provider` runs on every apply that adds or changes a provider, and on
    the fallback tick for a provider whose build failed. It calls `deps.build_provider` → `main::build_provider` →
    `HttpProvider::new`, whose reqwest `ClientBuilder::build()` reads the system certificate store
    (`rustls_native_certs::load_native_certs`, because of the `rustls-tls-native-roots` feature). No `ponytail:`
    comment or §15 row records this one.
- The GET routes read only the `watch` channel (C5), so a busy actor never blocks `status`.

**Alternatives rejected.**
- Shared state behind mutexes: lock ordering bugs, and a lock held across a hung FUSE call freezes everything.
- One task per mount owning its state: conflicts, warm-up and policy need the global view.

**Consequences and limits.**
- Every asynchronous result needs a generation check (see [generations-for-stale-results](#generations-for-stale-results)).
- Timers are computed deadlines (`Actor::deadline`), not per-mount tasks.
- API commands that need an answer carry a `oneshot` (`ApiCmd`). `POST /v1/reconcile` is answered after the pass that
  follows its driver re-probe (S2 sign-off 1).
- A slow state directory would stall the actor on the state.json write, and a slow certificate store would stall it
  while an HTTP provider is built. Moving the build off the actor (`spawn_blocking`, or building inside the provider
  task) would be a code change.

**Where in code.** `crates/bifrost-daemon/src/actor.rs` (`Actor`, `Msg`, `spawn`, `Actor::run`, `handle`, `pass`,
`persist`); `crates/bifrost-daemon/src/api.rs` (`router`, `ask`).

**Source.** contract §5 "Concurrency model"; C5; S2 sign-off 1; 6e7ee55, 73237db.

<a id="pure-planner-decision-table"></a>
### A pure planner: one decision table, recomputed every pass

**Status:** accepted (S1-A, 9154b42). Refined by A14–A16, by r2 (d719eaa) and by r3 (6e54b57, which gated row 11
on warm-up).

**Context.** PRD §10–§11: the daemon is a reconciliation loop, and repeating a reconciliation with unchanged inputs
must have no side effects.

**Decision.**
- Each pass runs `registry.expire` → `registry.machines(policy)` → `reconcile::desired` → `reconcile::plan` →
  execute the side-effect actions → persist (the last step of `Actor::pass`). `Actor::run` then publishes the
  snapshot once `pass` returns.
- `reconcile::decide(candidate, runtime, now, ready, grace)` is a pure, first-match, 14-row table (contract §5). Only
  `Mount`, `Unmount` and `Remount` have side effects. A `Remount` runs as an unmount, and the new mount comes from row 9
  on a later pass, so the offline and backoff gates still apply.
- Every side-effect action calls `MountRuntime::begin()` before its task is spawned, so a re-plan during execution hits
  row 1 (`Waiting(InFlight)`).
- The runtime transitions (`mount_done`, `unmount_done`, `health`) are pure functions of their inputs as well.
- Between passes the actor keeps only its inputs (the registry, runtimes, holds, probe results and provider status).
  The machines, verdicts, candidates and plan are recomputed on every pass.

**Alternatives rejected.** Event-driven handlers ("machine lost → unmount") and planners that store a desired state
and diff it. Both need ordering rules between events and can issue duplicate actions. A table recomputed from scratch
has neither problem.

**Consequences and limits.**
- The idempotency argument is in contract §5. Tests: `plan_twice_second_all_noop`, `plan_is_deterministic`, one
  `rowNN_*` test per row, and `reconcile_endpoint_twice_second_all_noop` in the daemon.
- The `Display` strings of `Action`, `WaitReason`, `Reason` and `Verdict` are frozen (S1 sign-off 7): the CLI golden
  tests lock them.
- `next_wakeup` returns only deadlines strictly after `now` (S1 sign-off 1), so a deadline that passed without its row
  acting can't make the actor spin (`next_wakeup_never_spins`).

**Where in code.** `crates/bifrost-core/src/reconcile.rs` (`desired`, `decide`, `plan`, `next_wakeup`,
`MountRuntime`); `crates/bifrost-daemon/src/actor.rs` (`Actor::pass`).

**Source.** PRD §10, §11, §28; contract §5; A14, A15, A16, C2; S1 sign-offs 1 and 7; r2 sign-off 10; r3 sign-off 13.

<a id="generations-for-stale-results"></a>
### Generations drop stale results

**Status:** accepted. It fixes Changes from B #7. A1 renamed the field.

**Context.** Mount, unmount, health and child-exit results arrive asynchronously. In design B, `generation` was bumped
only on spawn, so a late health result could land after an unmount.

**Decision.**
- `MountRuntime::begin(phase)` bumps `generation` for mount and unmount alike. Every transition ignores a result
  whose generation differs, and also one issued for another phase (a `Mounting` result that arrives once the runtime
  is `Absent`), so a duplicate result can't count a failure twice.
- A new runtime seeds its generation with `created << 32` (`Actor::start_mount`), so no late result of a dropped
  runtime can match it.
- Provider results carry `task_gen`, bumped each time a provider task is (re)started. `Actor::discovery` drops a result
  from an aborted or removed task.
- `gen` is a reserved keyword in edition 2024, hence `generation` (A1). `task_gen` is fine.
- `Msg::ChildExited` is deliberately not generation-gated. A failed unmount leaves the mount `Mounted` at a newer
  generation, and a stale exit costs only one extra inspect (comment in `Actor::handle`).

**Alternatives rejected.** Cancelling in-flight tasks: a blocked FUSE syscall can't be cancelled, so its result must be
recognisable as stale instead.

**Consequences and limits.** Tests: `stale_generation_ignored_for_health_and_exit`, `stale_task_gen_dropped`,
`exit_after_failed_unmount_probed`.

**Where in code.** `crates/bifrost-core/src/reconcile.rs` (`MountRuntime::{begin, mount_done, unmount_done, health}`);
`crates/bifrost-daemon/src/actor.rs` (`start_mount`, `discovery`, `start_provider`, `handle`).

**Source.** contract §2, §5; Changes from B #7; A1; 9154b42.

<a id="http-over-unix-socket"></a>
### Local API: HTTP/1 over a Unix socket

**Status:** accepted. The decision was fixed before the design panel (contract §1 calls it "the pre-committed
HTTP-over-UDS decision"). Implemented in S1-D (257cce2).

**Context.** PRD §16–§17: the CLI and TUI are clients of the daemon over a Unix domain socket, speaking "HTTP over Unix
socket or a lightweight framed JSON protocol. HTTP over UDS is preferable because it keeps debugging trivial."

**Decision.**
- axum 0.8 serves on a `tokio::net::UnixListener`. `bifrost-client` does one hyper HTTP/1 handshake per request over a
  `UnixStream`. The DTOs live in `bifrost_core::api`, all routes are under `/v1`, and every error body is an `ErrorDto`.
- Every POST sends a JSON body: `{}`, or `UnmountReq { force }` for unmount. The client always sets
  `Content-Type: application/json`, and axum rejects a `null` body (C6).
- The socket path is `$BIFROST_SOCKET`, else `$XDG_RUNTIME_DIR/bifrost/bifrost.sock`, else
  `~/.cache/bifrost/bifrost.sock`, and on macOS `~/Library/Caches/bifrost/bifrost.sock`. The default is never under
  `/tmp` (C5). The path must be at most 103 bytes (`sun_path`), and the socket is chmodded 0600 after bind.

**Alternatives rejected.**
- A framed JSON protocol: its own framing and versioning, and debugging needs a custom client (PRD §17).
- TCP on localhost: any local user could connect, so it would need authentication. File permissions do that job on a
  Unix socket. (This reason is not recorded; it is an inference.)
- WebSockets or gRPC: PRD §17 says SSE is enough for V1.

**Consequences and limits.**
- `curl --unix-socket "$BIFROST_SOCKET" http://bifrost/v1/status` works, and the E2E harness uses it.
- axum and hyper were already in the dependency tree, so the cost is small (contract §1).
- The client opens one connection per request, with no pooling. That is fine at CLI and TUI rates.
- Access control is the socket's file permissions (see [security.md](security.md)).

**Where in code.** `crates/bifrost-daemon/src/api.rs` (`router` and the handlers); `crates/bifrost-daemon/src/main.rs`
(`bind_socket`); `crates/bifrost-client/src/lib.rs` (`Client::send`); `crates/bifrost-config/src/paths.rs`
(`socket_path`).

**Source.** PRD §16, §17; contract §1, §8 "Routes", §9; C5, C6; 257cce2.

<a id="sse-events-and-polling-clients"></a>
### Events over SSE; the shipped clients poll

**Status:** accepted. E6 and §15 #12.

**Context.** PRD §17: "SSE is enough for V1. No need for WebSockets initially." PRD §20 lists the events.

**Decision.**
- `GET /v1/events` streams `EventRecord`s as SSE: `id` is `seq`, `event` is the enum's serde tag, `data` is the JSON
  record, with a 15 s keep-alive. A lagging receiver gets `event: lagged` with `{"skipped":N}` and the stream
  continues. The last 200 records are also in `StatusDto.events`.
- Events are emitted on transitions only. The actor compares the previous verdicts and probes to emit
  `MachineEligible` and `DriverUnavailable` (B1).
- No shipped client consumes SSE. The CLI waits by polling `GET /v1/mounts` every 250 ms for up to 60 s (`settle`), and
  `bifrost discover` polls status until every provider's `refreshes` has grown (at most 30 s). The TUI polls
  `GET /v1/status` once a second.

**Alternatives rejected.** A client SSE consumer plus a `bifrost events` command (cut; it is the §15 #12 upgrade path).
WebSockets (PRD §17).

**Consequences and limits.**
- The TUI shows events up to 1 s late. Debugging SSE needs `curl --unix-socket`.
- An open SSE stream never ends by itself, so `run` (in `main.rs`) races axum's graceful shutdown against a 2 s
  sleep that starts at the stop signal (`tokio::select!`).

**Where in code.** `crates/bifrost-daemon/src/api.rs` (`events`); `crates/bifrost-daemon/src/main.rs` (`run`, the
shutdown race); `crates/bifrost-daemon/src/actor.rs` (`emit`,
`pass` for B1, `handle` for `Msg::Probed`); `crates/bifrost-cli/src/main.rs` (`settle`, `discover`);
`crates/bifrost-tui/src/main.rs` (`run`); `crates/bifrost-client/src/lib.rs`.

**Source.** PRD §17, §20; contract §8 "SSE"; B1, E4, E6; §15 #12.

<a id="panics-caught-at-driver-boundary"></a>
### Driver and provider panics are caught at the call

**Status:** accepted (S4-O, 36dc831). It was an open item in S2 sign-off 2, a carry-over in S3 (2a), and accepted in
S4a sign-off 3.

**Context.** If an executor task panics, the actor never gets its `MountDone` or `UnmountDone`, and the runtime stays
`Mounting` or `Unmounting` (row 1, `Waiting(InFlight)`) forever. The release profile unwinds on purpose (no
`panic = "abort"`).

**Decision.** `actor::caught(what, call, on_panic)` wraps every driver and provider call (`probe`, `mount`, `inspect`,
`unmount`, `discover`) with `futures_util::FutureExt::catch_unwind`, which catches panics both when the call is made and
while it is polled. A panic becomes:
- `MountDone` / `UnmountDone` `Err("driver panicked: …")` at the executor's generation, so it takes the normal backoff
  path;
- `Degraded` for `inspect`, `Unavailable` for `probe`, `Failed` for `discover`.

The message is cleaned to 512 characters.

**Alternatives rejected.**
- `panic = "abort"`: one buggy driver would take the daemon down.
- Mapping the `JoinError` from the executor `JoinSet` (the fix S2 sign-off 2 suggested). The code catches inside the
  task instead. The doc comment on `caught` gives the reason: "instead of killing the task, so the message the actor
  waits for is always sent and no runtime stays in flight (S3 sign-off 2a)". The code shows why a `JoinError` would
  not have been enough: it carries no mount id or generation, and only `start_mount` and `start_unmount` run in the
  `execs` `JoinSet`. `inspect` and `probe` use a plain `tokio::spawn`, and `discover_loop` runs in the provider task,
  so `execs.join_next()` would never have seen their panics. (The comment in `Actor::run` only restates the
  behaviour.)

**Consequences and limits.** Tested by `executor_panic_recovers` (with `FakeDriver::panic_next`).
`check::timed` separately turns a panicking blocking closure into an `io::Error`.

**Where in code.** `crates/bifrost-daemon/src/actor.rs` (`caught`, `discover_loop`, `start_mount`, `start_unmount`,
`inspect`, `probe`); `crates/bifrost-core/src/fake.rs` (`FakeDriver::panic_next`); `Cargo.toml` (`[profile.release]`).

**Source.** contract §1 (release profile); S2 sign-off 2; S3 carry-over 2a; S4a sign-off 3; 36dc831.

<a id="log-filter-allowlist"></a>
### `BIFROST_LOG` applies to bifrost's own targets only

**Status:** accepted (S4a sign-off 2, 36dc831). It replaced S3 carry-over 2c, which asked for a cap on a list of
crates (hickory, reqwest, hyper, rustls).

**Context.** With `BIFROST_LOG=debug`, hickory's debug logging wrote raw TXT answers into the daemon log. Those
answers are untrusted and can contain terminal escapes.

**Decision.** `log_filter(level)` is `Targets::new().with_default(level.min(WARN)).with_target("bifrost", level)`.
Bifröst's own crates log at `BIFROST_LOG` (error|warn|info|debug|trace, default info). Every other target is capped at
warn, and at error when `BIFROST_LOG=error` (never more verbose than `BIFROST_LOG`). No dependency was added: this is tracing-subscriber's `Targets`
filter with the existing `fmt`, `std` and `ansi` features. ANSI colour is used only when stderr is a terminal.

**Alternatives rejected.**
- A denylist of noisy crates (the carry-over's wording): a new transitive dependency would log at debug by default.
- `EnvFilter` directives: they need tracing-subscriber's `env-filter` feature, a new dependency, and carry-over 2c
  required none.

**Consequences and limits.** tokio, axum, hyper and hickory debug logs never appear. Debugging inside a third-party
crate needs a code change. `third_party_debug_logs_capped` checks that a hickory debug line is dropped and a hickory
warning kept, that `BIFROST_LOG=error` keeps third-party crates at error, and that `bifrost_discovery::dns` logs at
trace.

**Where in code.** `crates/bifrost-daemon/src/main.rs` (`log_filter`, `subscriber`, `main`).

**Source.** S3 carry-over 2c; S4a sign-off 2; 36dc831.

<a id="state-json-is-a-hint"></a>
### state.json: holds are authoritative, mount records are hints

**Status:** accepted (S2-E, 6e7ee55; 6f0411e).

**Context.** PRD §21: no database; persist only what safe recovery needs; never trust state blindly, check the OS
mount table. Design B persisted an `Override::Mounted` that could mount a DiscoverOnly machine (Changes from B #8).

**Decision.**
- `<state>/state.json` is `{ "version": 1, "held": [...], "mounts": { id: MountHandle } }`.
- `held` (the manual unmounts) is user intent and authoritative. The mount records are used only by `adopt`: for the
  informational pid and, on macOS only, to adopt an entry that carries no marker.
- It is written atomically (a new tmp file with mode 0600 → `sync_all` → rename) whenever `held` or a handle changes;
  `Actor::persist` skips the write when nothing changed.
- A file that is unreadable, unparseable or another version is renamed to `state.json.corrupt-<unix>`, and the daemon
  starts empty. An unreadable file is quarantined too, so the next write can't replace the holds without leaving a
  copy (6f0411e).
- Only `held` persists. There is no persistent "force mounted" override.

**Alternatives rejected.** A database (PRD §21). A persisted mount override (Changes from B #8: a way around policy).

**Consequences and limits.** A restart and a crash take the same adoption path. Losing state.json loses the holds
(manually unmounted mounts come back), but it never mounts anything policy doesn't allow. A pid from state.json is
never signalled. Tested by `state_json_atomic_corrupt_quarantined` and `api_unmount_holds_across_restart`.

**Where in code.** `crates/bifrost-daemon/src/state.rs` (`State`, `read`, `write`); `crates/bifrost-daemon/src/actor.rs`
(`persist`); `crates/bifrost-mount/src/lib.rs` (`adopt`).

**Source.** PRD §21; contract §8 "state.json"; Changes from B #8; S2 sign-off 1; 6e7ee55, 6f0411e.

<a id="private-state-and-socket-dirs"></a>
### Private directories: created 0700, never chmodded when they exist, refused when others can write them

**Status:** accepted. A21 (S2-E), extended by r2 (fb3238e, the mount root) and r3 (6376c65, the state dir, `logs/`
and the socket's parent).

**Context.**
- Critique A21: `set_permissions(0o700)` on every directory would chmod `$HOME` when `BIFROST_SOCKET=$HOME/b.sock`.
- r2: another user who can write the root could swap `<root>/<id>` for a symlink before the mount, or mount there
  first with our marker.
- r3: another user who can write the state directory could replace `logs/` and plant `<id>.log -> <a file of ours>`,
  which the child-log open (create + truncate) would clobber.

**Decision.**
- `create_dirs` creates missing components with mode 0700 and chmods only the directories it created.
- The lock file `<state>/bifrostd.lock` (0600) is refused if it is group- or other-writable. Its owner uid is the
  reference for the ownership checks.
- A pre-existing state dir, `<state>/logs`, socket parent and the canonical `mount.root` must be owned by that uid and
  not world-writable (`mode & 0o002 == 0`); otherwise the daemon exits 1.

**Alternatives rejected.**
- chmodding every directory on the path (the A21 bug).
- Checking `0o022`: it would refuse the 0775 directories a umask of 002 (user private groups) creates.
- Walking the ancestors and checking group-shared directories (`0o020`): not done; it is a recorded ponytail.

**Consequences and limits.** The full rules are in [security.md](security.md#filesystem-permissions). Tests:
`preexisting_parent_dirs_not_chmodded`, `world_writable_root_refused`, `world_writable_state_dirs_refused`,
`second_instance_lock_refused`, `socket_0600_stale_replaced`.

**Where in code.** `crates/bifrost-daemon/src/main.rs` (`create_dirs`, `private`, `lock`, `bind_socket`, `run`).

**Source.** contract §8 "Directories"; A21; r2 sign-off 9; r3 sign-off 12; 6f0411e, fb3238e, 6376c65.

---

<a id="area-discovery"></a>
## Discovery, trust and policy

<a id="discover-is-not-mount"></a>
### Discovery never implies mounting

**Status:** accepted.

**Context.** PRD §7: "Auto-discovery must never imply auto-mount-everything"; the safe default is "discover = yes,
mount = no" until an allow rule exists. PRD §6.3, §23.1 and §33.2 say the same.

**Decision.**
- Every non-static observation is `DiscoverOnly` unless its provider's include filter or a global `[policy.allow]`
  matches (`policy::evaluate`).
- `reconcile::desired` builds mount candidates only for `Allowed` machines.
- `POST /v1/mounts/{target}/mount` never overrides policy. A target that is not a valid `Name`, or that is neither a
  known mount nor a known machine, gets 404 (`Actor::targets`). A known target that is not a candidate gets 403 with
  its verdict (`Actor::api_mount`, `verdict_of`). For a candidate, the call only clears the hold, both retry timers
  (`mount_retry_at`, `unmount_retry_at`), `offline`, `failures` and `force_requested`.
- Record-supplied mount data is narrow. `honor_hints` defaults to false, so the `user=` / `path=` hints of DNS and HTTP
  records are ignored. There is no driver hint at all (E2: `driver=` is an ignored key in bf1 and HTTP).

**Alternatives rejected.** An `auto_mount` knob per provider (cut from design B). A manual mount that overrides policy
(Changes from B #8).

**Consequences and limits.** A provider with no filter only lists machines: E2E p07 asserts zero tailscale mounts.
Users must write include or allow rules. Tests: `no_allow_rule_is_discover_only`, `api_mount_forbidden_for_discover_only`,
`hints_ignored_unless_honor_hints`.

**Where in code.** `crates/bifrost-core/src/policy.rs` (`evaluate`); `crates/bifrost-core/src/reconcile.rs` (`desired`);
`crates/bifrost-daemon/src/actor.rs` (`api_mount`, `verdict_of`); `crates/bifrost-config/src/lib.rs` (the discovery
`mount` template defaults in `V::config`).

**Source.** PRD §6.3, §7, §23, §33.2; contract §4; Changes from B #8, #15; E2.

<a id="winner-takes-all-trust"></a>
### Winner takes all, by trust rank

**Status:** accepted. It fixes Changes from B #11 and #12. Recorded as §15 #11.

**Context.** PRD §5: a machine may be reported by several providers, and the observations should be merged. Design B
merged by provider config order, so untrusted DNS could supply the connect host, and any provider's exclude or deny, or a
tag published in DNS, could deny a static machine.

**Decision.**
- `Source` orders by `trust`, then kind, then provider name (a derived `Ord`). The ranks come from
  `bifrost_config::TRUST`: static 0 < tailscale 1 < http 2 < dns 3.
- The registry keeps one observation per (machine, source). `evaluate` first drops the observations excluded by their
  own provider's filter. The most trusted remaining observation is selected, and it alone supplies the host, port,
  user and path hints, tags, metadata and online state. The others are listed in `MachineDto.shadowed`.
- The global deny is evaluated against the selected observation only.
- Why this order is not written down in the contract. It matches how each source is authenticated (an inference):
  static comes from the user's config; tailscale identities are assigned by the coordination server; an HTTP
  inventory is reached through a URL and credentials the user configured; DNS answers are unauthenticated (no DNSSEC,
  §15 #21).

**Alternatives rejected.**
- A field-by-field merge (design B): the least trusted source could fill in the connect target.
- A per-id `prefer = "<provider>"` rule: the §15 #11 upgrade path, not needed yet.

**Consequences and limits.**
- A DNS or HTTP include can't mount an id that tailscale also reports unless it is allowed. To hand such a machine to
  DNS, exclude it in the more trusted provider's filter (for a static machine, remove it from `[[machines]]`).
- Two providers of the same kind reporting one id: the alphabetically first provider name wins (§15 #10).
- Tests: `trust_order_static_tailscale_http_dns`, `dns_cannot_redirect_tailscale_machine`,
  `dns_tags_cannot_deny_static_machine`. E2E p13 publishes `inv-01` through DNS as well; it stays one machine from the
  inventory with `dns` shadowed.

**Where in code.** `crates/bifrost-core/src/registry.rs` (`Source`, `Machine::shadowed`, `MachineRegistry::machines`);
`crates/bifrost-core/src/policy.rs` (`evaluate`); `crates/bifrost-config/src/lib.rs` (`TRUST`, `ProviderConfig::source`,
`static_source`).

**Source.** PRD §5; contract §4; Changes from B #11, #12; B8; §15 #10, #11; 472d94e.

<a id="policy-semantics"></a>
### Policy semantics

**Status:** accepted. A19 was a security fix during the critique.

**Context.** PRD §7 lists the filter primitives (exact id, hostname glob, address/CIDR, tag, provider, metadata,
explicit allow, explicit deny) and the rule "deny wins over allow".

**Decision.**

| Primitive | Keys | include / allow matches when | exclude / deny matches when |
|---|---|---|---|
| exact id (global) | `policy.*.ids` | the machine id equals an entry; `native_id` never matches (A19) | same |
| exact id (provider) | `include_ids` / `exclude_ids` | the machine id, or this provider's own `native_id`, equals an entry | same |
| name glob | `names` / `*_names` | the id matches a glob (`*`, `?`) | same |
| CIDR | `cidrs` / `*_cidrs` | an IP-literal address is inside an entry; hostnames are never resolved | same, **or** a non-static observation has no IP literal (fails closed) |
| tag | `tags` / `*_tags` | any listed tag is present | same |
| provider | `policy.*.providers` | the provider name or kind is listed | same |
| metadata | `metadata` / `*_metadata` | **all** pairs equal | **any** pair equal |

Include and allow are AND across non-empty kinds and OR within a kind. Exclude and deny fire on any single primitive.
`evaluate` runs: (1) drop observations excluded by their own provider filter; none left → `Denied`; (2) select the most
trusted remaining one; (3) global deny → `Denied`; (4) trust 0 (static) → `Allowed` ("static = explicit allow");
(5) the selected provider's non-empty include matches → `Allowed`; (6) a non-empty global allow matches → `Allowed`;
(7) otherwise `DiscoverOnly`. A verdict's `Display` names the rule that decided it (`denied (policy.deny tags=misc)`).

**Alternatives rejected.**
- Matching `native_id` in the global `ids` (the critique's A19, a security bug): a DNS record publishing
  `id=<a tailscale node ID>` with any `host=` satisfied a global allow.
- Resolving hostnames for CIDR rules: evaluation is in core, which does no I/O, and the answer would be one more
  untrusted input (an inference; the contract only states the rule).

**Consequences and limits.**
- Deny rules on attributes a source controls itself (names, tags and metadata from DNS or HTTP) are advisory: they can
  only deny what that same source chose to publish. The real protection is the allow rules plus SSH host-key checking
  (contract §4).
- A global allow on a name, tag or metadata can be satisfied by any provider, DNS and HTTP included. Scope it with
  `providers = [...]`, or use a provider include.
- An HTTP entry's `host` replaces its `addresses`, so CIDR rules see the connect target (A20, see
  [http-inventory-limits](#http-inventory-limits)).
- Tests: the `policy.rs` suite (`deny_wins_over_allow`, `global_deny_beats_static`,
  `provider_exclude_drops_only_that_observation`, `cidr_deny_fails_closed_without_ip`, `cidr_allow_requires_ip`,
  `global_ids_match_machine_id_only`, `native_id_scoped_to_owning_provider`, …). E2E p08 checks
  `denied (policy.deny tags=misc)`.

**Where in code.** `crates/bifrost-core/src/policy.rs` (`Match::{all, any}`, `id_hit`, `cidr_hit`, `provider_hit`,
`evaluate`, `Verdict`); `crates/bifrost-config/src/lib.rs` (`V::matcher`).

**Source.** PRD §7; contract §4; A19, A20, B5; critique A19; 472d94e.

<a id="warm-up-readiness"></a>
### Warm-up: no removals until discovery has actually worked

**Status:** accepted. It fixes Changes from B #5. A6 and A18 defined it; r3 (6e54b57) extended it to row 11.

**Context.**
- After a restart the registry is empty. Design B counted a provider `Err` as "reported", so adopted tailscale mounts
  were unmounted before tailscale answered.
- Critique A6: with a missing config, the empty default has no providers, so the daemon was ready at once and unmounted
  every adopted mount.
- Critique A18: a DNS NXDOMAIN returns `Ok(vec![])`, which counted as reported.
- r3: a DNS or HTTP result that beat `tailscale status` built an adopted mount's candidate from the lower-trust
  observation, and row 11 remounted it to that spec (and back once tailscale reported).

**Decision.**
- `ready` latches. It becomes true once a config file has loaded (`cfg_loaded_at` is set) **and** either every network
  provider has returned a non-empty `Ok` at least once, or `offline_grace_period` has passed since that load.
- Before `ready`, row 4 keeps an un-held mounted mount that is no longer desired (`Waiting(WarmingUp)`), and row 11
  doesn't remount on a spec change (`Degraded("change pending: warming up")`). Row 3 (Stale or force-requested), a
  held unmount, and rows 10 and 12 still act.
- The actor wakes at `cfg_loaded_at + grace`. At startup (`main::config`) and for the 2 s poller (`Poller::poll`
  skips an empty file unless woken by SIGHUP), a missing or 0-byte config file never counts as loaded.
- A 0-byte file applied by SIGHUP or `POST /v1/config/reload` does count. Both parse it to the empty default
  (`Poller::poll(hup = true)` sends it; `api::reload` calls `bifrost_config::load`, which parses `""` to `Ok`), and
  `Actor::apply` with `startup = None` sets `cfg_loaded_at` if it is not set yet (unless A22 rejects the default root
  `~/machines` as a root change). The empty default has no providers, so `ready` latches on the next pass and un-held
  adopted mounts become removable (gracefully). See [config-polling-not-notify](#config-polling-not-notify).

**Alternatives rejected.** Ready at start, or counting an `Err` or an empty `Ok` (design B, A18).

**Consequences and limits.**
- A static-only config is ready immediately.
- A provider that is down at login delays removals by at most the grace period.
- Accepted cost (r3): a config edit made while the daemon was down is applied up to `offline_grace_period` later if
  some provider is down.
- Not covered (r3): a fresh start with nothing adopted can still mount the lower-trust spec first (row 9) and remount
  once the more trusted provider reports.
- Tests: `warmup_protects_adopted_until_ok`, `missing_config_never_unmounts_adopted`, `row04_warmup_blocks_removal`,
  `row04_held_bypasses_warmup`, `row11_gated_on_driver_and_online`.

**Where in code.** `crates/bifrost-daemon/src/actor.rs` (`Actor::ready`, `deadline`, `apply` for `cfg_loaded_at`,
`discovery` for `reported`); `crates/bifrost-core/src/reconcile.rs` (`decide`, rows 4 and 11).

**Source.** contract §5 "Warm-up"; Changes from B #5; A6, A18 and the refinements after the amendments table; r3
sign-off 13; 6e54b57.

<a id="validated-newtypes-at-trust-boundary"></a>
### Validated newtypes at the trust boundary

**Status:** accepted in S0 (f7db2e6, reviewed in 4f03aa4).

**Context.** PRD §23: never execute discovery-supplied commands, treat TXT and HTTP data as untrusted, guard local
paths against traversal. Design B used `#[serde(transparent)]` newtypes, which skip the validators when state.json or
DTOs are deserialized (Changes from B #13). It also silently skipped a tailscale peer with a capitalised HostName
(B #21), and its only form for the remote login directory was `""` (B #22).

**Decision.** Every untrusted string becomes a validated type through `parse`, or is rejected. The newtypes serialize
with `#[serde(try_from = "String", into = "String")]`, so deserializing validates too.

| Type / function | Grammar |
|---|---|
| `Name` (machine and mount id, one path component) | ASCII-lowercased, then `[a-z0-9][a-z0-9._-]{0,62}` |
| `Host` | an IP literal (canonical: v4-mapped v6 stored as v4; unspecified rejected), or a hostname of ≤253 bytes with labels of `[A-Za-z0-9_-]`, 1–63 long, no leading `-`; one trailing `.` stripped; stored lowercase |
| `User` | `[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}` |
| `RemotePath` | `~`, `~/<rel>`, `/` or `/<abs>`; ≤1024 bytes; no control character, no `:`, no `..` component |
| `tag` / `meta_key` / `native_id` | `[a-z0-9][a-z0-9_.:-]{0,62}` (lowercased) / `[a-z0-9_.-]{1,64}` / `[A-Za-z0-9._:-]{1,128}` |
| `Glob`, `Cidr`, `parse_duration` | `[a-z0-9*?._-]{1,63}`; CIDR without cross-family matches and with v4-mapped v6 rejected; a number with one unit (`ms`, `s`, `m` or `h`), > 0 and ≤ 366 days |
| `DriverSelector` | `auto`, or the `Name` grammar without lowercasing (membership is checked in bifrost-config) |

Config machine names must already be lowercase (the error carries a "use lowercase" hint). `clean(s, max)` is the
display-only counterpart for text that is never parsed.

**Alternatives rejected.** Validating at each use site (easy to miss one). `#[serde(transparent)]` (Changes from B #13).

**Consequences and limits.** The argv builders accept only these types. A provider skips a record whose field fails.
Tests: `name_rejects_traversal`, `host_rejects_option_injection`, `remote_path_rules`,
`serde_newtypes_validate_on_deserialize`, `tag_meta_label_native_id_grammars`.

**Where in code.** `crates/bifrost-core/src/validate.rs`; `crates/bifrost-core/src/model.rs` (`DriverSelector`,
`parse_marker`); used by `bifrost-config`, the providers, the CLI's `name` value parser and the TUI's `id`.

**Source.** PRD §23; contract §2 (`validate.rs`); Changes from B #13, #21, #22; f7db2e6, 4f03aa4.

<a id="static-provider-is-config"></a>
### Static machines are config, not a provider task

**Status:** accepted. Recorded as §15 #2. r3 added a `static` row to the status output (6376c65).

**Context.** PRD §6.1: static configuration needs no daemon-side network discovery.

**Decision.** There is no `DiscoveryProvider` for static machines. Every config apply calls
`registry.replace(static_source(), cfg.static_observations())`, which is authoritative and never expires. Trust 0
means "explicit allow unless denied". `StatusDto.providers` starts with a synthetic `static` row (its machine count,
`refreshes` 0, never an error), so `status`, `doctor`, `discover` and the TUI show it. `static` is a reserved provider
name.

**Alternatives rejected.** A static provider task: the §15 #2 upgrade path, for when static machines need another
source than the config.

**Consequences and limits.** Static machines change only on a config apply. `bifrost discover` doesn't wait for
static. Static locals are claimed first, and a discovered id equal to one is reported in `conflicts`.

**Where in code.** `crates/bifrost-daemon/src/actor.rs` (`apply`, `snapshot`); `crates/bifrost-config/src/lib.rs`
(`Config::static_observations`, `static_mounts`, `static_source`); `crates/bifrost-core/src/registry.rs` (`replace`).

**Source.** PRD §6.1; contract §4 step 4, §8; B8; §15 #2; r3 sign-off 12; 6376c65.

<a id="tailscale-via-cli-json"></a>
### Tailscale through `tailscale status --json`

**Status:** accepted (S3-H, 9296898). Later changes: own tailnet sorts first (aa98d2a), sharee peers skipped (r1,
e2284f5), fixed-directory lookup (36dc831).

**Context.** PRD §6.2: "Use Tailscale's machine-readable local status interface." The provider must not be coupled to
any mount driver.

**Decision.**
- Run `tailscale status --json` with a 10 s timeout, stdout capped at 16 MiB, stderr at 64 KiB, `kill_on_drop`.
- The binary is looked up on every refresh: the constructor's path (tests only), then the absolute `$PATH` entries,
  then `/usr/local/bin:/usr/bin:/bin` (and `/opt/homebrew/bin` on macOS), then the macOS app bundle.
- `BackendState` other than `Running` → `Unavailable`. A non-zero exit → `Failed(tail(stderr, 512))`.
- `parse_status` is pure. `Self` is never read. Each peer is deserialized separately, so one malformed peer is skipped
  alone. `ShareeNode` peers (another user's device, listed only because we shared a node with them) are skipped
  silently.
- Peers are sorted by (HostName-only last, a foreign MagicDNS suffix after our own, DNSName), and the first peer with
  an id wins. The id is the first label of DNSName, else HostName. The addresses are the MagicDNS name (when MagicDNS
  is on), else the first IPv4, then every Tailscale IP. The `tag:` prefix is stripped from tags.

**Alternatives rejected.** Tailscale's LocalAPI over its own socket. Not recorded. An inference: the CLI's JSON is the
documented machine-readable interface, it works the same with the macOS app bundle, and it needs no second HTTP client
for a socket whose path differs per OS.

**Consequences and limits.**
- Discovery needs the tailscale CLI. A missing binary or a stopped backend is an `Err`, so the provider's last view
  freezes.
- Code differs from contract §7, which says "peers are processed sorted by DNSName". aa98d2a changed the sort so that
  a shared-in node or a peer choosing its own HostName never evicts one of our own nodes' ids. The commit asked for an
  orchestrator sign-off, and none is recorded in the contract.

**Where in code.** `crates/bifrost-discovery/src/tailscale.rs` (`TailscaleProvider`, `which`, `which_in`,
`status_json`, `parse_status`, `observation`).

**Source.** PRD §6.2; contract §7 "Tailscale"; S3 sign-off 1; S3 carry-over 2d; r1 sign-off 7; 9296898, aa98d2a,
e2284f5, 36dc831, fe0cb5b.

<a id="bf1-dns-format-inline-and-index"></a>
### DNS TXT `bf1`: inline node records plus index and per-node records

**Status:** the index-only format of v0.1.0 is **superseded**. v0.1.1 accepts inline node records and index records
mixed (fb160b8, sign-off 14; fix 9ea7c87).

**Context.** PRD §6.3 and §26 show two forms: inline records at `_bifrost.<domain>` (`"v=bf1 host=… user=…"`) and an
index (`nodes=…`) plus one record per node, preferring the index because "a single large TXT record becomes awkward
quickly". The PRD's inline form has no identity key: a machine would be named after `host=`, which is attacker data.
v0.1.0 therefore supported the index form only (old §15 #21: "publishers must use index + node records").

**Decision (v0.1.1).**
- The root `_bifrost.<domain>.` holds inline node records (`v=bf1 node=<label> …`) and index values
  (`v=bf1 nodes=a,b`), mixed freely. An index node publishes `_bifrost.<node>.<domain>.`.
- Identity is always a DNS label: the `node=` value, or the index label. It is never `id=` (that is only the native id)
  and never `host=`.
- Grammar (`parse_bf1`): the first token must be `v=bf1`, else the record is not ours and is ignored. A value is at most
  2048 bytes; keys `[a-z0-9_-]{1,32}`; values of 1–256 printable ASCII characters, no `"`. A duplicate key, a missing
  `=`, `node=` together with `nodes=`, or any known key that fails validation makes the value invalid. Unknown keys are
  ignored, `driver=` included (E2).
- Whole-node rejection: an invalid inline value rejects its node, because inline values are grouped by label before
  parsing. A label both inline and indexed, or with two distinct inline values, is ambiguous and skipped. `node=` inside
  a per-node record rejects that node.
- At most 256 labels across inline and index values (`MAX_NODES`). A `node=` value that isn't a DNS label is skipped
  without using a slot (9ea7c87). Labels contain no dots, so an index can't make the daemon query another domain.
- TTL: an inline node gets the root's validity, an index node `min(root, node)`. No records, or nothing valid, →
  `Ok(vec![])`, which never counts toward warm-up.

**Alternatives rejected.**
- The PRD's inline form without `node=`: its identity would come from untrusted `host=`.
- Index-only (v0.1.0): every machine needs its own record plus an index edit, which is awkward for small fleets and
  for managed DNS consoles (833b236 rewrote the docs around the inline form).

**Consequences and limits.**
- A root RRset with more than about 12–15 inline values exceeds a 1232-byte EDNS UDP reply (about 5 values at 512
  bytes without EDNS), so it needs TCP (see [dns-tcp-fallback](#dns-tcp-fallback)). The index form remains for large
  fleets, networks that block TCP/53, and per-machine dynamic-DNS keys (55a35a6).
- A root value with neither `node=` nor `nodes=` (a typo such as `name=agent-01`) parses as a valid, empty index value
  and logs nothing (55a35a6; see [security.md](security.md#residual-risks)).

**Where in code.** `crates/bifrost-discovery/src/dns.rs` (`parse_bf1`, `dns_label`, `root`, `one`, `node`,
`node_observation`, `MAX_NODES`).

**Source.** PRD §6.3, §26; contract §7 "DNS TXT bf1"; §15 #21; sign-off 14 (after S4a); 874a651, fb160b8, 9ea7c87,
833b236, 55a35a6.

<a id="dns-tcp-fallback"></a>
### Large bf1 answers go over TCP, with no resolver tuning

**Status:** accepted (v0.1.1, sign-off 14). D1 rules out resolver options.

**Context.** Inline records make the root RRset grow past what one UDP reply holds.

**Decision.**
- Every nameserver is `NameServerConfig::udp_and_tcp` (explicit ones get their configured port). hickory retries a
  truncated (TC) UDP answer over TCP, on the explicit path and on the system-resolver path alike
  (`system_resolver_has_tcp_fallback` pins the latter).
- No `options_mut()` calls (D1): `ResolverOpts::default()` is already 5 s × 2 attempts.
- Every query is absolute (trailing `.`), so resolv.conf search domains never apply.
- Explicit `nameservers` get one cached resolver that honours the TTLs. Without them, the system resolver config is
  re-read on every refresh (r1), so a network or VPN change, or a daemon started offline, doesn't leave the provider
  querying stale servers or dead.
- If TCP fails on the root RRset (`_bifrost.<domain>.`, the large answer that needs TCP), the lookup fails
  (`Failed`) and the provider keeps its last good view, so a truncated root never becomes a partial view.
- Per-node lookups (the index form) are different: a failed one only logs `bf1 node skipped` and the other nodes are
  still returned as `Ok` (`DnsProvider::discover`, contract §7 step 3). That refresh is a partial view; the skipped
  node is not refreshed and ages out.

**Alternatives rejected.** Raising the EDNS payload or tuning resolver options (D1). Snapshotting the system config at
start (the r1 bug). Treating a truncated answer as a partial view.

**Consequences and limits.**
- A network that blocks TCP/53 needs the index form.
- On Linux without `options edns0` in resolv.conf, the system path sends 512-byte queries, and truncation starts at
  about 5 inline values.
- `nameservers` answer only the bf1 TXT lookups. ssh resolves `host=` and `<node>.<domain>` with the system resolver
  (7c25612).
- The system resolver is rebuilt every refresh, so hickory's cache never outlives one refresh (a `ponytail:` comment
  in `DnsProvider::discover`).

**Where in code.** `crates/bifrost-discovery/src/dns.rs` (`DnsProvider::new`, `discover`, `txt`).

**Source.** contract §7 (resolver setup, "Big answers"); C8; D1; r1 sign-off 7; sign-off 14; e2284f5, fb160b8, 9ea7c87,
55a35a6, 7c25612.

<a id="http-inventory-limits"></a>
### HTTP inventory: fixed limits, no redirects, no cache headers

**Status:** accepted (S3-K, 802848f; review fixes d911f9a). Recorded as §15 #8.

**Context.** PRD §6.4: a generic JSON inventory endpoint. PRD phase 10: configurable authentication headers and a
cache/refresh policy.

**Decision.**
- The reqwest client (rustls with native roots) has a 10 s timeout, `redirect::Policy::none()` (auth headers never
  follow a redirect elsewhere), user agent `bifrost/<version>`, the configured headers marked sensitive, and
  `Accept: application/json`.
- `http://` is allowed only to loopback (config), and such a client is built with `no_proxy()`: an `HTTP_PROXY` or
  `ALL_PROXY` in the environment would otherwise receive the credentials in plain text.
- A non-2xx status → `Failed("HTTP <code>")`. The body is streamed with `chunk()` and capped at 1 MiB.
- Only the first 1000 entries are used. Each entry is converted on its own, and any invalid field skips that entry.
  With `host` present, `addresses = [host]` (A20). Without it, the first valid entry among at most 32 `addresses` is
  the only address kept. `metadata.tags` become tags, scalar values become `clean(v, 256)` strings, nested values are
  ignored, and `driver` is ignored (E2).
- The cache policy is the registry's: on failure the last good view is served (frozen), and an entry that disappears
  ages out after 3 × interval.

**Alternatives rejected.** `ETag` / `If-None-Match` (the §15 #8 upgrade path). Following redirects (header leak).

**Consequences and limits.**
- An expensive inventory is fetched in full every interval.
- Transport errors never contain the URL (`reqwest::Error::without_url`), because a query string may carry a token.
- Code differs from contract §7, which says invalid `addresses` are dropped and the rest kept. The code keeps only the
  first valid one (d911f9a): a trailing in-range IP must not carry a different first address past `include_cidrs`.
- Tests: `inventory_prd_example`, `invalid_entries_isolated`, `entries_capped_at_1000`, `host_overrides_addresses`,
  `body_cap_enforced`, `redirect_not_followed`, `transport_error_names_cause_not_url`.

**Where in code.** `crates/bifrost-discovery/src/http.rs` (`HttpProvider::new`, `discover`, `failed`,
`parse_inventory`, `observation`, `BODY_CAP`, `MAX_ENTRIES`).

**Source.** PRD §6.4, phase 10; contract §7 "HTTP"; A20; E2; §15 #8; 802848f, d911f9a.

---

<a id="area-mounting"></a>
## Mounting

<a id="mount-table-not-path-existence"></a>
### The kernel mount table decides what is mounted

**Status:** accepted. It fixes Changes from B #19 (design B parsed `/sbin/mount` text on macOS).

**Context.** PRD §22: tell "directory exists" apart from "filesystem is mounted", using platform mount information.

**Decision.**
- `table::read()`: on Linux, `/proc/self/mountinfo` through `parse_mountinfo` (the fields after ` - `, `\NNN` octal
  unescaping, malformed lines skipped); on macOS, `libc::getmntinfo(MNT_NOWAIT)` under a static mutex, because it
  returns one per-process buffer. `find` returns the topmost entry at a path.
- Every decision reads it: mount readiness, `inspect` (`Missing` when the entry is gone or, on Linux, doesn't carry our
  marker), `unmount_path` (Ok when the path is no longer in the table, whatever the helper's exit code), adoption, and
  every `remove_dir` (only when the table says the path isn't a mountpoint).
- Reading it never touches a FUSE filesystem (hung-FUSE guard 1).

**Alternatives rejected.**
- Path existence or `stat`: an empty directory looks the same, and a dead FUSE mount can hang the call.
- Parsing `mount` output (design B): fragile.
- A mountinfo crate: the parser needs three fields (§15 #4).

**Consequences and limits.** The macOS path is compiled and unit-tested in CI but has no E2E (§15 #25). Tests:
`mountinfo_octal_escapes`, `mountinfo_optional_fields`, `mountinfo_malformed_skipped`,
`find_takes_topmost_and_read_sees_root`, `unmount_idempotent_when_absent`.

**Where in code.** `crates/bifrost-mount/src/table.rs` (`read`, `parse_mountinfo`, `find`);
`crates/bifrost-mount/src/lib.rs` (`unmount_path`, `mount_with`, `inspect_path`, `adopt`).

**Source.** PRD §22; contract §6; Changes from B #19; §15 #4.

<a id="sshfs-foreground-child"></a>
### sshfs runs in the foreground as our supervised child

**Status:** accepted. The contract's recovery path for a killed sshfs (`auto_unmount` → `Missing`) was superseded by
the behaviour verified in S1 sign-off 4 (ENOTCONN → `Stale`).

**Context.** PRD §8.1: spawn the mount, track the child process, recover from stale mounts.

**Decision.**
- sshfs runs with `-f` (foreground). It is spawned by absolute path with `Command::args`, never `sh -c`, with stdin
  `/dev/null`, `process_group(0)` (a terminal Ctrl-C or the daemon's death never signals it), `kill_on_drop(false)`,
  and the inherited environment (`SSH_AUTH_SOCK` included).
- stdout and stderr go to `<state>/logs/<id>.log` (0600, truncated on each spawn, first line
  `# bifrost exec: <argv>`), never a pipe: a full pipe stalls the FUSE server, and a dead reader would SIGPIPE the
  orphan after a daemon crash.
- The mount is ready when the table shows our marker at the path (polled every 100 ms up to `mount_timeout`). Then a
  supervisor task owns the `Child`, `wait()`s on it (reaping it) and calls `on_exit`, which sends `Msg::ChildExited`,
  which triggers an immediate inspect.
- Options: `fsname=<marker>,reconnect,idmap=user,transform_symlinks`, the `SSH_OPTS`, then `auto_unmount` on Linux,
  `volname=<id>,noappledouble` on macFUSE, `volname=<id>` on FUSE-T, `ro`, `-p`, `-F`, and the validated positionals
  last.
- Verified on this project's host (S1 sign-off 4, sshfs 3.7.3 / fuse3 3.14.0): after `kill -9 sshfs`, `auto_unmount`
  does **not** unmount. The mount stays and returns ENOTCONN, so recovery is `Stale` → row 10 (lazy detach) → row 9.
  Contract §6 "Stale-mount recovery" still describes the unverified `Missing` path; the comment in `sshfs_argv` states
  the verified behaviour.

**Alternatives rejected.** Letting sshfs daemonize itself: there would be no child to supervise and no exit
notification. Pipes for output (stall, SIGPIPE).

**Consequences and limits.** Children survive daemon restarts and are adopted. The log is truncated on each spawn and
never capped (§15 #13). An adopted mount has no `Child`; its pid (from state.json) is informational.

**Where in code.** `crates/bifrost-mount/src/lib.rs` (`mount_with`); `crates/bifrost-mount/src/sshfs.rs`
(`SshfsDriver`, `sshfs_argv`); `crates/bifrost-core/src/model.rs` (`OnExit`); `crates/bifrost-daemon/src/actor.rs`
(`start_mount`, `Msg::ChildExited` in `handle`).

**Source.** PRD §8.1; contract §6 ("Spawn", "mount(req)", "sshfs argv"); A8; S1 sign-off 4; 1e02973, 2b040d0.

<a id="ssh-preflight"></a>
### An ssh preflight before every spawn

**Status:** accepted. The contract made it conditional ("if ssh doesn't exit 0 on a working server, drop it"); S1
sign-off 4 verified it and kept it. Hardened in e914166 and r1 (2e09dce).

**Context.** PRD §8.1: "validate SSH connectivity". rclone swallows ssh's stderr, so a host-key or authentication
failure would otherwise show up only as a mount timeout.

**Decision.** Before each spawn, for both drivers, run

```
<ssh> -a -x -o ClearAllForwardings=yes -o PermitLocalCommand=no
      -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3
      -o ControlMaster=no -o ControlPath=none [-F <ssh_config>] [-p <port>] [-l <user>] -s -- <host> sftp
```

with stdin and stdout on `/dev/null`, stderr read up to 64 KiB, one 15 s timeout, and `kill_on_drop`. Exit 0 means
the host key, authentication and the sftp subsystem work. Anything else becomes `Failed(tail(stderr, 512))`, for
example `Host key verification failed.`, and that is what `last_error` shows.

**Alternatives rejected.** Relying on the child's log tail: rclone doesn't pass ssh's stderr through.

**Consequences and limits.** One extra ssh connection per mount attempt. The error text is actionable. Tests:
`preflight_argv_golden`, `preflight_stdout_ignored_stderr_capped`, and the ignored docker test
`preflight_exit0_and_hostkey_failure`. E2E psec asserts the host-key message for sshfs, p09 for rclone.

**Where in code.** `crates/bifrost-mount/src/check.rs` (`ssh_preflight`); `crates/bifrost-mount/src/lib.rs`
(`preflight_argv`, `mount_with` step 4).

**Source.** PRD §8.1; contract §6 "SSH preflight"; S1 sign-off 4; e914166 (review SEC-2), 2e09dce (r1-5).

<a id="host-keys-never-weakened"></a>
### Host-key checking is never weakened

**Status:** accepted. It fixes Changes from B #3. r1 added `SSH_CLI_HARDENING` (2e09dce).

**Context.** PRD §23.4: do not silently weaken host verification. Design B had a `HostKeyChecking::AcceptNew`
(trust-on-first-use) knob.

**Decision.**
- The constants `SSH_OPTS` (`BatchMode=yes`, `ConnectTimeout=10`, `ServerAliveInterval=15`, `ServerAliveCountMax=3`,
  `ControlMaster=no`, `ControlPath=none`) and `SSH_CLI_HARDENING` (`-a -x -o ClearAllForwardings=yes -o
  PermitLocalCommand=no`) are the only ssh options Bifröst passes. There is no config knob.
- No argv ever contains `StrictHostKeyChecking`, `UserKnownHostsFile`, `GlobalKnownHostsFile`, `ProxyCommand`,
  `IdentityFile`, or sshfs's `ssh_command`, `directport`, `passive` or `sftp_server`.
- The user's ssh_config (or `mount.ssh_config`, passed as `-F`) decides trust. `BatchMode=yes` turns "ask" into
  "fail". `ControlPath=none` ties the mount's lifetime to our process instead of a user's multiplexing master.
- `SSH_CLI_HARDENING` leads the preflight and rclone's `--sftp-ssh`, so a `ForwardAgent`, `ForwardX11` or port forward
  in ssh_config never reaches a mounted host. sshfs passes `-x -a -oClearAllForwardings=yes` itself, and its `-o`
  passthrough rejects these flags, so they stay out of `SSH_OPTS`.
- rclone's internal SSH library, which skips host-key checks unless `known_hosts_file` is set, is never used.

**Alternatives rejected.** A trust-on-first-use knob (Changes from B #3).

**Consequences and limits.** A new host must be in `known_hosts` (for example through `ssh-keyscan`) before it can
mount. Tests: `argv_never_weakens_host_keys` (nine forbidden substrings across every argv builder, every flavour, with
and without `-F`), `ssh_never_forwards_agent_x11_or_ports`, `rclone_never_uses_internal_ssh`. E2E: `psec_hostkey.sh`
(sshfs) and p09's `unknown-key-rc` (rclone).

**Where in code.** `crates/bifrost-mount/src/lib.rs` (`SSH_OPTS`, `SSH_CLI_HARDENING`, `preflight_argv`);
`crates/bifrost-mount/src/sshfs.rs` (`sshfs_argv`); `crates/bifrost-mount/src/rclone.rs` (`rclone_argv`).

**Source.** PRD §23.4; contract §6 "Host keys are never weakened", §11 #4; Changes from B #3; B13; S4a sign-off 6
(r1); 2e09dce.

<a id="rclone-via-system-ssh"></a>
### rclone uses the system OpenSSH through `--sftp-ssh`

**Status:** accepted (S3-J, d354ab2; review 785e351). A23 and B13 shaped it.

**Context.** PRD §8.2: use rclone's SFTP backend. rclone's internal SSH library skips host-key checks unless
`known_hosts_file` is set, and doesn't share the user's agent, ProxyJump or Tailscale SSH setup.

**Decision.** `rclone mount|nfsmount :sftp:<sftp_path> <local_path>` with, always in `--flag=value` form:

| Flag | Why |
|---|---|
| `--config=/dev/null` | never read the user's rclone.conf |
| `--sftp-host=<host>` | defensive; harmless next to `--sftp-ssh` |
| `--sftp-ssh=<ssh> <SSH_CLI_HARDENING> <SSH_OPTS> [-F "<cfg>"] [-p <port>] [-l <user>] <host>` | OpenSSH does the connection, with the same known_hosts and agent as sshfs. rclone splits the value on spaces (with `"…"` quoting) and appends `-s sftp`, so there is no `--` (it would turn `-s sftp` into a remote command) |
| `--sftp-shell-type=none`, `--sftp-disable-hashcheck` | rclone never runs remote shell commands |
| `--devname=bifrost:<id>@<fp16>` | the adoption marker |
| `--cache-dir=<state>/rclone/<id>` | A23: one VFS cache per mount, never shared across hosts; pending writes resume on the next mount |
| `--vfs-cache-mode=<mode>` | from config; `nfsmount` forces at least `writes` (it is read-only below that) |
| `--dir-cache-time=15s`, `--log-level=NOTICE` | the liveness probe's cache ceiling (§15 #6) |
| `--read-only`, macOS FUSE `--volname=<id>` | from the spec |

`sftp_ssh_check` refuses a spec whose ssh path, host or user contains whitespace, `"` or a control character, or
whose ssh_config path contains `"` or a control character. The probe checks binaries and flags only, never the
settings (S2 sign-off 3): `rclone version`, `--sftp-ssh` in `rclone help flags sftp` (feature detection), ssh, and
FUSE (or `/sbin/mount_nfs` for `rclone-nfs`, which is macOS only).

**Alternatives rejected.** rclone's internal SSH (no host-key check). A generated rclone config (the user's config is
never read, and none is written).

**Consequences and limits.**
- rclone logs a NOTICE "No host key validation is being performed". It refers to the unused internal library.
- Data-safety notes recorded in 785e351 as `ponytail:` comments: a lazily detached rclone can still use `--cache-dir`
  while its replacement starts; the VFS cache is keyed inside `--cache-dir` by a hash of the `--sftp-*` flags; a
  graceful unmount doesn't wait for VFS write-back; `nfsmount` serves on an unauthenticated 127.0.0.1 port. See
  [simplifications.md](simplifications.md) and [security.md](security.md#residual-risks).

**Where in code.** `crates/bifrost-mount/src/rclone.rs` (`RcloneDriver`, `rclone_argv`, `sftp_ssh_check`,
`probe_with`).

**Source.** PRD §8.2; contract §6 "rclone argv"; A23; B13; S2 sign-off 3; S3 sign-off 1; d354ab2, 785e351.

<a id="no-pid-signalling-lazy-detach"></a>
### No recorded or adopted pid is ever signalled; force means lazy detach

**Status:** accepted. It fixes Changes from B #1 and #2. Recorded as §15 #15.

**Context.** Design B's orphan reaper killed every same-uid process carrying a `bifrost:` marker, including the user's
real daemon during E2E. Its force unmount detached lazily and then killed the process group, losing writes in flight.

**Decision.**
- No adopted pid is ever signalled, and the only mount process Bifröst ever kills is its own spawn that did not
  become ready within `mount_timeout` (`start_kill()` and `wait()`, then a lazy detach only if our marker is at the
  path). Timed-out helper commands are a separate case (next bullets).
- Force unmount is `fusermount3 -u -z` on Linux, and `diskutil unmount force` falling back to `umount -f` on macOS.
  Nothing is killed: open files keep working, and the child exits when the last reference closes.
- Pids from state.json and `MountHandle.pid` are informational.
- Helper commands (`fusermount3`, `umount`, `diskutil`, the preflight, `tailscale`, the probes) run under a timeout with
  `kill_on_drop(true)`. Mount children use `kill_on_drop(false)` and their own process group.

**Alternatives rejected.** An orphan reaper (Changes from B #1). Killing after a lazy detach (Changes from B #2).

**Consequences and limits.** A process stuck connecting lives until `ConnectTimeout` or the `ServerAlive` limit. A
lazily detached sshfs lingers until its last reference closes. Tests and the E2E harness kill only by the recorded pid
(`mpid`), never `pkill -f fsname=`, which would also kill the `fusermount3` auto_unmount helper (A13).

**Where in code.** `crates/bifrost-mount/src/lib.rs` (`mount_with`, the timeout branch; `unmount_path`);
`crates/bifrost-mount/src/check.rs` (`run`).

**Source.** contract §6; Changes from B #1, #2; §15 #15; A13; 2b040d0.

<a id="busy-unmount-never-forced"></a>
### A busy mount is never force-unmounted automatically

**Status:** accepted. C4 fixed the wording. r2 fixed its backoff (d719eaa).

**Context.** PRD §12 lets Bifröst retain, remount or unmount an offline machine's mount. An unmount forced while files
are open risks losing data.

**Decision.**
- A graceful unmount (`fusermount3 -u`, or `umount` on macOS) whose stderr says "resource busy" returns `Err(Busy)`.
  The match is on the errno text, not on the word "busy", which a path may contain (e914166).
- `Busy` puts the mount back to `Mounted` as `Degraded("unmount blocked: busy (files open)")` (the one busy string,
  C4), increments `failures` and sets `unmount_retry_at`. Rows 3 and 5 wait out that backoff; rows 10–12 show the error.
- Force is used only when the mount is `Stale` (dead process or ENOTCONN; rows 3 and 10), has been `Degraded` past
  `offline_grace_period` (row 12), needs a spec-change remount while its health is `Degraded` (row 11,
  `Remount { force: matches!(h, Health::Degraded(_)), why: SpecChanged }`, not gated on the grace period), or the
  user asked (`unmount --force` sets `force_requested`). Force is always a lazy detach; nothing is killed.
- A busy result does not change `health`: the `Degraded("unmount blocked: …")` shown is `gate_show` displaying
  `last_error`. So a spec change on a busy mount whose probes stay `Healthy` remains graceful: it shows `Degraded` and
  retries. Only if its probe also reads `Degraded` (for example `unresponsive`) does row 11 force it.
- r2: `health(Healthy)` no longer resets `failures` while an unmount retry is pending. Before, every health tick reset
  the backoff of a long-up busy mount to its first step, so it retried about 13 times a minute forever. `POST …/unmount`
  now clears `unmount_retry_at`, so `unmount --force` acts at once.

**Alternatives rejected.** Forcing after N busy retries: that is exactly the data loss this avoids.

**Consequences and limits.**
- A busy mount that is no longer wanted stays until its files close or the user forces it.
- One `failures` counter serves mount and unmount (§15 #26). After a long busy period, the next mount's first failure
  backs off at up to `retry_max` (accepted in r2).
- Tests: `api_unmount_busy_stays_degraded`, `busy_unmount_backoff_survives_healthy_probe`,
  `api_force_unmount_skips_busy_backoff`, `busy_matches_the_errno_text_not_the_path`, `gate_u_unmount_backoff`.

**Where in code.** `crates/bifrost-mount/src/lib.rs` (`unmount_path`, `busy`); `crates/bifrost-core/src/reconcile.rs`
(`decide` rows 3, 5, 10–12; `MountRuntime::unmount_done`, `health`); `crates/bifrost-daemon/src/actor.rs`
(`api_unmount`).

**Source.** PRD §12; contract §5, §6 "unmount"; C4; r2 sign-off 10; e914166, d719eaa.

<a id="hung-fuse-guards"></a>
### Hung-FUSE guards

**Status:** accepted. r2 changed guard 2's key from the path to the mount instance (S4a sign-off 11, 374fcb3).

**Context.** Any syscall on a hung FUSE mount can block forever and pin a thread. The daemon must stay responsive
(contract §14 risk 1).

**Decision.**
1. Mount-table reads (`/proc/self/mountinfo`, `getmntinfo(MNT_NOWAIT)`) never touch FUSE.
2. The only filesystem call on a live mountpoint runs inside `check::timed`: `spawn_blocking` under a 5 s timeout, with
   a process-wide in-flight key set. The key is `<path>/.pid-<pid>` (the plain path when the handle has no pid), so
   there is at most one leaked thread per hung instance, and while a probe is stuck the next one returns at once.
3. `prepare_mountpoint` and `remove_dir` run only after the table says the path is not a mountpoint.
4. Every helper command runs under a timeout with `kill_on_drop(true)`.
5. The root is canonicalized once, at startup.

The liveness probe looks up a unique nonexistent name, `<mnt>/.bifrost-probe-<16 hex>`, because a cached `stat` of
the root makes a dead remote look healthy (Changes from B #4). Any server reply (`Ok`, `NotFound`, `PermissionDenied`)
means `Healthy` (A17: an unsearchable remote root is not a fault). `NotConnected` (and ENXIO on macOS) means `Stale`.
A timeout means `Degraded("unresponsive")`. `main` ends with `std::process::exit` without dropping the runtime, so a
probe thread stuck on a hung mount can't hold up the exit.

**Alternatives rejected.**
- `stat` on the mount root (Changes from B #4): answered from caches.
- An in-flight key per path (the r2 bug): a lazy detach doesn't abort the FUSE connection, so a probe stuck on a
  detached instance held the path, and every later mount there read `Degraded("unresponsive")` and was force-detached
  each grace period.

**Consequences and limits.** One leaked blocking thread per hung instance. Two pid-less adoptions at one path share a
key (a `ponytail:` comment). The 5 s timeout is fixed (§15 #7). Code differs from contract §6 (the `check` module
signature and inspect step 2), which still shows `check::liveness(path)`: the code takes `liveness(path, key)`, the
key being the instance (S4a sign-off 11). Tests: `timed_guard_single_thread`, `liveness_keyed_per_instance`,
`liveness_local_dir_healthy`.

**Where in code.** `crates/bifrost-mount/src/check.rs` (`timed`, `liveness`, `INFLIGHT`); `crates/bifrost-mount/src/lib.rs`
(`inspect_path`, `prepare_mountpoint`); `crates/bifrost-daemon/src/main.rs` (`main`, `run`).

**Source.** contract §5 "Hung-FUSE guards", §14 risk 1; Changes from B #4; A17; S4a sign-off 11; 374fcb3.

<a id="adoption-by-marker-and-fingerprint"></a>
### Adoption by marker and spec fingerprint

**Status:** accepted. It fixes Changes from B #23. The sshfs `fsname=` override was verified (S1 sign-off 4), so the
A10 fallback (path + fstype) was never implemented.

**Context.** Mounts outlive the daemon (restarts, crashes). The daemon must recognise its own mounts without trusting
state.json, and notice a spec change that happened while it was down.

**Decision.**
- Every mount carries the marker `bifrost:<id>@<fp16>` as its source: sshfs `fsname=`, rclone `--devname=`. `fp16` is
  FNV-1a 64 over the whole spec (id, machine, host, port, user, remote, local path, driver selector, read-only),
  16 hex digits. The encoding is pinned by `fingerprint_stable_vector`, because it lives in kernel mount tables and
  state.json across upgrades.
- `adopt()` considers only entries directly under this daemon's canonical root, takes the topmost entry per path,
  parses the marker exactly (`agent-01` never matches `agent-01-x`), and requires the directory name to equal the id.
  The driver comes from the fstype, else the state record, else `sshfs`. The pid comes from the state record with the
  same `local_path`.
- On Linux an entry without a marker is foreign. On macOS it is adopted when state.json has a record for that path
  (FUSE-T and NFS may not show the marker).
- A fingerprint that differs from the candidate's leads to row 11, a graceful remount.
- `mount()` step 2: our marker with the same fingerprint → adopt it (the leftover of a dropped or timed-out attempt);
  our marker with an older fingerprint → lazy detach, then continue; anything else → `Refused("occupied by …")` (A9).

**Alternatives rejected.** Trusting state.json pids (PRD §21 says never trust state blindly). Design B's orphan scan.

**Consequences and limits.**
- A restart doesn't remount: E2E p13a asserts `adopted: true`, the same pid, no `MountStarted` and no extra sshfs
  process.
- The driver selector text is fingerprinted, so `auto` mounts stay put when probes flap. `vfs_cache_mode` and
  `ssh_config` are not fingerprinted (§15 #16).
- After a root change across a restart, mounts under the old root are left alone (§15 #19).
- Tests: `fingerprint_changes_on_every_field`, `fingerprint_stable_vector`, `marker_roundtrip_exact`,
  `adopt_marker_record_foreign_outside_root`, `mount_step2_own_marker_adopt_or_detach_foreign_refused`.

**Where in code.** `crates/bifrost-core/src/model.rs` (`MountSpec::fingerprint`, `marker`, `parse_marker`);
`crates/bifrost-mount/src/lib.rs` (`adopt`, `step2`, `mount_with`); `crates/bifrost-daemon/src/main.rs` (`run`, steps
6–7).

**Source.** PRD §21, §22; contract §6 "Adoption after a restart", mount() step 2; Changes from B #23; A9, A10, B6, B15;
S1 sign-off 4; §15 #16, #19, #23; 1e02973, e914166.

---

<a id="area-config"></a>
## Configuration

<a id="config-deterministic-validation"></a>
### Config validation: every error, sorted, byte-identical output

**Status:** accepted (S1-B, bf2f08e; review a665a12). S1 sign-off 6 accepted its details.

**Context.** PRD phase 2 acceptance: `bifrost config check` returns deterministic validation output.

**Decision.**
- `parse(text, path, env)` is pure except for one read-only `Path::exists()` on `mount.ssh_config`.
- A TOML syntax or shape error gives exactly one `ConfigError` whose path is `<file>:<line>:<col>`. Otherwise every
  semantic error is collected, sorted by (key path, message) and deduplicated.
- Every table is `deny_unknown_fields`, so a typo or a `password =` key is an error. `RawDiscovery` is one flat
  struct rather than a tagged enum, so unknown-field errors keep their line and column; per-type keys are checked in
  validation.
- `~`, `$VAR`, `${VAR}` and `$$` expand only in `mount.root`, `mount.ssh_config`, `discovery.url` and header values. An
  undefined or empty variable is an error, never an empty string.
- Every message goes through `clean(_, 512)`. A shape error drops a quoted string value (it may be a header secret),
  and URLs and header values are never echoed.
- `config check` needs no daemon and builds no providers.

**Alternatives rejected.** Stopping at the first error. A tagged `RawDiscovery` enum (loses line and column).

**Consequences and limits.** A URL or header value that `config check` accepts but reqwest rejects fails only when the
daemon builds the provider; the provider then keeps its frozen view (r2, B11). Tests: `errors_sorted_deterministic`,
`unknown_field_has_line_col`, `per_type_keys_enforced`, `config_check_output_deterministic` (CLI).

**Where in code.** `crates/bifrost-config/src/lib.rs` (`parse`, `load`, `V`, `expand`, `url_allowed`, `is_token`);
`crates/bifrost-config/src/raw.rs`; `crates/bifrost-cli/src/main.rs` (`config_check`).

**Source.** PRD §13, phase 2; contract §3; S1 sign-off 6; r2 sign-off 9; bf2f08e, a665a12.

<a id="config-polling-not-notify"></a>
### Config reload by polling on a plain thread

**Status:** accepted (S4-M, ee027b3). B10 and §15 #3. S4a sign-off 1 made SIGHUP wake the poller; r1 stopped the
poller from applying a 0-byte file (3b903ac).

**Context.** PRD §27: watch the config file, reload transactionally, and never let an invalid config destroy the
working one.

**Decision.**
- `reload::spawn_poller` runs on a `std::thread`. Every 2 s it calls `std::fs::read` (which follows symlinks). It acts
  once two consecutive reads agree and differ from the bytes it last acted on (a debounce for half-written saves).
  It parses the bytes and sends `Msg::Config { reply: None }`.
- A given set of bad bytes is reported once. A missing, unreadable or empty (0-byte) file keeps the active config with
  one warning.
- SIGHUP is installed only if the thread started. It wakes the thread for an immediate read that skips the debounce,
  sends a read error as an error, and applies an empty file.
- `POST /v1/config/reload` runs `spawn_blocking(load)` and sends `Msg::Config { reply: Some(..) }`.
- The actor applies startup and every reload through one path (A4), and rejects a `mount.root` change (A22).

**Alternatives rejected.**
- The notify crate (inotify, FSEvents). Polling handles rename-on-save editors, dotfile-manager symlink swaps and
  config files on NFS or sshfs homes (contract §8).
- Polling inside tokio: a read on a hung NFS or sshfs home would block a runtime worker (B10).

**Consequences and limits.** An edit applies 2–4 s later. SIGHUP and the API are immediate. A `>` redirect truncates
before its writer fills the file, so the poller never applies an empty file (it would drop every machine and unmount
every idle mount). Tests: `poller_applies_after_two_identical_reads`, `poller_ignores_half_written_change`,
`poller_reports_bad_bytes_once`, `poller_missing_file_keeps_active_and_warns_once`,
`poller_empty_file_keeps_active_and_warns_once`, `poller_follows_symlink_swap`, `sighup_skips_the_debounce`,
`reload_invalid_keeps_old`.

**Where in code.** `crates/bifrost-daemon/src/reload.rs` (`spawn_poller`, `Poller::poll`);
`crates/bifrost-daemon/src/api.rs` (`reload`); `crates/bifrost-daemon/src/actor.rs` (`apply`).

**Source.** PRD §27; contract §8 "Config hot reload"; A4, B10; §15 #3; S4a sign-offs 1, 8; ee027b3, de28fe8, 3b903ac.

<a id="missing-config-empty-default"></a>
### A missing config runs the empty default; changing the root needs a restart

**Status:** accepted. It fixes Changes from B #14. A6 and A22 refined it; r1 treats a 0-byte file as missing.

**Context.** PRD §31: a plain `bifrostd` must start. Design B exited when the config was missing. The actor does no I/O,
so it can't create or canonicalize a new root on reload (A22).

**Decision.**
- At startup, a missing or 0-byte config file → `parse("")` (root `~/machines`, no machines) plus a warning. The
  config doesn't count as loaded, so `ready` stays false and nothing adopted is unmounted. (A 0-byte file applied
  later by SIGHUP or the API does count as loaded; see [warm-up-readiness](#warm-up-readiness).)
- An invalid config at startup → print the sorted errors and exit 2. The daemon never starts with an empty config that
  would unmount everything.
- A reload whose expanded `mount.root` differs from the active one is rejected with "mount.root change requires
  restart", and the old config is kept. After a missing-config start, a file that appears later is applied only if its
  root is `~/machines`.

**Alternatives rejected.** Exiting on a missing config (Changes from B #14). Applying a root change live (A22: `mount()`
would refuse every path, since the new parent was never canonicalized).

**Consequences and limits.** A typo in `BIFROST_CONFIG` runs with no machines, logged only at warn (§15 #27). Tests:
`missing_config_never_unmounts_adopted`, `empty_config_file_not_loaded`, `reload_root_change_rejected`.

**Where in code.** `crates/bifrost-daemon/src/main.rs` (`config`, `run`); `crates/bifrost-daemon/src/actor.rs`
(`apply`).

**Source.** PRD §31; contract §8 "Startup" step 4; Changes from B #14; A6, A22; §15 #27; S4a sign-off 8 (r1).

---

<a id="area-clients"></a>
## Clients

<a id="cli-exit-codes"></a>
### CLI exit codes 0 / 1 / 2 / 3

**Status:** accepted (S2-F, 1512326). S2 sign-off 3 settled the edge cases.

**Context.** The CLI is used by people and scripts. A script needs to tell "the daemon isn't running" from "the daemon
refused" and from "you typed it wrong". The contract defines the codes without stating a reason; that one is an
inference.

**Decision.**

| Code | `bifrost` | `bifrostd` | `bifrost-tui` |
|---|---|---|---|
| 0 | success | clean shutdown | quit |
| 1 | the operation failed: an API error, a mount that ended Failed or Offline, a busy unmount, an invalid config in `config check` / `config reload`, a `doctor` ✗ in Config or Drivers, a 60 s settle timeout, an unknown id in `machines show` | startup failure (already running, directories, socket, root) | terminal or runtime failure |
| 2 | usage (clap), including a target that isn't a valid `Name` | usage, or an invalid config | usage |
| 3 | the daemon isn't reachable (`ClientError::NotRunning`, or `Io` such as EACCES on the socket); `daemon status` when it isn't running | — | — |

`doctor` exits 1 only on a Config ✗, when no driver is usable, or when a mount carries the macOS permission hint. One
unavailable driver alone doesn't fail it (S2 sign-off 3), and neither does a default that is still `probing` (r2).

**Alternatives rejected.** Not recorded.

**Consequences and limits.** `ClientError::Io` counts as "not reachable" (3), not "failed" (1). Tests:
`exit3_when_daemon_absent`, `doctor_config_drivers_and_macos_hint`, `config_check_output_deterministic`.

**Where in code.** `crates/bifrost-cli/src/main.rs` (`run`, `daemon`, `settle`, `name`); `crates/bifrost-cli/src/doctor.rs`
(`run`); `crates/bifrost-daemon/src/main.rs` (`main`, `run`); `crates/bifrost-tui/src/main.rs` (`main`, `fail`).

**Source.** contract §8 "Invocation", §9 "Exit codes"; S2 sign-off 3; r2 sign-off 9; 1512326, f548564.

<a id="tui-inline-polling"></a>
### TUI: a synchronous loop that polls inline

**Status:** accepted (S3-L, 911a90e). E6 and C1. Review fixes in fdc5e00 and a5d7c4c.

**Context.** PRD §19 and phase 11: every primary operation available in the TUI, which talks only to the daemon API.

**Decision.**
- `main` is synchronous and owns a tokio multi-thread runtime with one worker. There is no poller task and no channel.
- Each loop iteration: `GET /v1/status` once a second through `rt.block_on(timeout(500ms, …))`; the Logs view's
  `GET /v1/mounts/{id}/log` every 2 s while it is visible; `ratatui::crossterm::event::poll(100ms)` (C1: crossterm is
  not a direct dependency); then draw.
- Commands run the same way with a 5 s bound, and their result becomes the status line.
- `App` is pure data with `on_key(KeyEvent) -> Option<Command>`, and `ui::render(&App, &mut Frame)` draws it. If the
  daemon is unreachable, a red banner appears and the last snapshot stays on screen.
- `m`, `u` and `U` on a machine row post each of its mounts (`App::mounts_of`), because the daemon resolves an id
  that is also a mount id to that mount alone (B15, fdc5e00).
- `NO_COLOR` removes every style; the glyphs still show the state.

**Alternatives rejected.** A poller task and a channel (critique E6: more code for the same one-second latency). An SSE
consumer (§15 #12).

**Consequences and limits.** Updates show up to 1 s late. A wedged daemon costs at most 500 ms per poll. Views are
tested with ratatui's `TestBackend` and keys as pure `on_key` calls. Truecolor only (§15 #24).

**Where in code.** `crates/bifrost-tui/src/main.rs` (`run`, `exec`, `call`); `crates/bifrost-tui/src/app.rs` (`App`);
`crates/bifrost-tui/src/ui.rs` (`render`).

**Source.** PRD §19, phase 11; contract §10; C1, E6; S3 sign-off 1; 911a90e, fdc5e00, a5d7c4c.

---

<a id="area-release"></a>
## Testing, release and docs

<a id="e2e-docker-harness"></a>
### E2E: a bash harness against a docker sshd

**Status:** accepted (S2-G, bfdc736). S4a sign-off 5 made a missing phase file fatal, replacing the contract's "skip
it" rule.

**Context.** PRD §28 asks for real integration tests with an OpenSSH container, sshfs, rclone and a DNS server. The
tests must never touch `~/.ssh` or `~/machines`.

**Decision.**
- `tests/e2e/run.sh [m1|all|<phase>…]` takes `flock -n /tmp/bf-e2e.lock` (one run per host; exit 2 if it is held).
  Everything lives under `T=$(mktemp -d)`: the socket, state, config, mount root and keys.
- It builds the workspace, copies the three binaries into `$T/bin`, and starts an Alpine sshd container
  (`tests/e2e/sshd/Dockerfile`: host keys generated at build time, password locked, one authorized key) published on
  127.0.0.1:2222 and 127.0.0.2:2222. The ssh_config it writes uses `StrictHostKeyChecking yes` with its own
  known_hosts.
- p08 runs CoreDNS; p10 runs `inventory.py`. Phases are listed explicitly, not globbed (C7: collation could reorder
  `p13_hardening` and `p13a_adopt`). Each phase file defines `setup_<p>` and `config_<p>` (optional) and `check_<p>`
  (required). Only p08 defines `[policy.*]` (B5).
- Processes are killed only through `mpid` (A13). The daemon starts with `9>&-`, so the lock doesn't leak into sshfs
  children. Cleanup lazily detaches whatever is mounted under `$T/machines` (found in mountinfo) and keeps the scratch
  directory (an `rm -rf` could recurse into a mount that failed to detach).
- p07 (tailscale) is opt-in with `E2E_TAILSCALE=1`, discovery only, and asserts zero mounts.

**Alternatives rejected.** Globbing phase files (C7). Skipping a listed phase file that doesn't exist (the S2 rule,
made fatal by S4a sign-off 5 once p12 and p13 existed).

**Consequences and limits.**
- Linux only (FUSE, docker). Run it with `TMPDIR` unset, so `$T/bf.sock` stays within 103 bytes.
- Scratch directories accumulate in `/tmp`, and p07's holds real tailnet names (S3 sign-off 3).
- CI doesn't run it: `ci.yml` runs `scripts/check.sh` and `scripts/test-install.sh`. The ignored docker tests in
  `bifrost-mount` reuse the same sshd through `BIFROST_E2E_SSH` (`tests/e2e/lib.sh`).
- See [e2e-harness.md](e2e-harness.md).

**Where in code.** `tests/e2e/run.sh`, `lib.sh`, `p*.sh`, `sshd/Dockerfile`, `dns/`, `inventory.py`,
`config.tmpl.toml`.

**Source.** PRD §28; contract §12 "E2E harness"; A11, A12, A13, B4, B5, B6, C7; S2 sign-off 4; S3 sign-off 3; S4a
sign-off 5; bfdc736, 7f3fe76, 280d909, 8d0cd4f.

<a id="release-static-musl-and-darwin"></a>
### Release: static musl builds for Linux, macOS builds on an arm64 runner, SHA256SUMS

**Status:** accepted (e30f0aa, 12044f4). It supersedes §15 #29 ("no CI yaml until a git remote exists"). The CI macOS
job also makes the §15 #31 comment obsolete (see [simplifications.md](simplifications.md)).

**Context.** The project needed prebuilt binaries for Linux and macOS on x86_64 and aarch64, and a CI gate once it had
a remote.

**Decision.**
- `release.yml` runs on `v*` tags. Its matrix builds `x86_64-unknown-linux-musl` (ubuntu-24.04),
  `aarch64-unknown-linux-musl` (ubuntu-24.04-arm), and `x86_64-apple-darwin` and `aarch64-apple-darwin` (both on
  macos-15; the CI job log shows the image is `macos-15-arm64`, so the x86_64 build is a cross-compile). The musl
  builds install `musl-tools` and set `CC_<target>=musl-gcc` for ring's C code.
- Each build runs `cargo build --release --locked --target <t> -p bifrost-cli -p bifrost-daemon -p bifrost-tui` and
  packages `bifrost-<target>.tar.gz` (the three binaries, the README and both licences).
- The release job writes `SHA256SUMS` and runs `gh release create --verify-tag`. `workflow_dispatch` with `dry_run`
  builds the artifacts only.
- `ci.yml`: on Linux, `scripts/check.sh`, a workspace build and `scripts/test-install.sh`; on macos-15, a workspace
  build (all targets) and `cargo test --workspace`.
- Why musl is not recorded. An inference: a static binary runs on any Linux distribution whatever its glibc version,
  and the dependency tree has no system library to link (reqwest uses rustls, not OpenSSL).

**Alternatives rejected.** Not recorded.

**Consequences and limits.** The target names are a contract with `install.sh` and the README.
`aarch64-apple-darwin` is built natively and unit-tested in CI (the `macos` job runs `cargo test --workspace` for the
host target only). `x86_64-apple-darwin` is cross-compiled on the arm64 runner and never tested. Neither has an E2E
("built but not field-tested", 087efa1; §15 #25).

**Where in code.** `.github/workflows/release.yml`, `.github/workflows/ci.yml`; `install.sh` (target detection).

**Source.** e30f0aa, 12044f4, 087efa1; §15 #25, #29, #31; README "Install". See [release-and-ci.md](release-and-ci.md).

<a id="install-sh-verify-always"></a>
### install.sh: always verify, never sudo

**Status:** accepted (7b1088d; review fixes d9ecdf8).

**Context.** The README's `curl … | sh` one-liner puts the installer in front of every new user.

**Decision.**
- POSIX sh (dash and bash). All the work happens in `main()`, called on the last line, so a truncated download runs
  nothing.
- It detects the target (a shell under Rosetta on Apple Silicon gets the native arm64 build) and validates
  `BIFROST_VERSION` against `[A-Za-z0-9._-]`.
- It downloads `SHA256SUMS` and the tarball, hashes the tarball through stdin (an odd `TMPDIR` path can't alter the
  output), and refuses to install when no hasher exists, when the asset has no entry, or on a mismatch ("nothing was
  installed").
- It installs only the three binaries into `BIFROST_INSTALL_DIR` (default `~/.local/bin`, resolved to an absolute
  path), staged as `.<name>.new` and then renamed, which is atomic and works while `bifrostd` runs.
- It never uses sudo. It reports missing runtime dependencies (with the package manager command to fix them) but never
  installs them, and it prints PATH advice.
- Why no sudo is not recorded beyond the script's header ("Never uses sudo and installs nothing but the three
  binaries"). An inference: the daemon runs as the user and needs nothing root-owned, and system packages are the
  user's call.

**Alternatives rejected.** Installing into `/usr/local/bin` with sudo. Installing unverified when no hasher is found.

**Consequences and limits.** The checksums come from the same release as the tarball. That protects against corruption
and truncation, not against a compromised release: nothing is signed (see [security.md](security.md#residual-risks)).
`scripts/test-install.sh` covers the happy path under dash and bash (from a file and piped), a corrupted tarball, a
missing `SHA256SUMS`, a hostile `BIFROST_VERSION` and an unwritable directory. The Pages workflow serves `install.sh`
at the site root.

**Where in code.** `install.sh`; `scripts/test-install.sh`; `.github/workflows/pages.yml`.

**Source.** 7b1088d, d9ecdf8, e30f0aa, 3d40b33.

<a id="docs-starlight-on-pages"></a>
### User docs: an Astro Starlight site on GitHub Pages

**Status:** accepted (3d40b33).

**Context.** The README alone couldn't hold the guides, the reference and the examples.

**Decision.** `site/` is an Astro Starlight site (base `/bifrost`, published at
https://samishal1998.github.io/bifrost/) with the brand palette on Starlight's variables, a custom `Hero` and
`SiteTitle`. `pages.yml` builds it on pushes to `main` that touch `site/`, `install.sh`, `brand/` or the workflow, copies
the repo-root `install.sh` into `site/public/` (where it is git-ignored), and deploys to Pages. The pages were checked
against the built binaries and the contract (3d40b33, 833b236). Engineering documentation lives in `docs/dev-guides/`
(this set), and design history in `docs/design/`.

**Alternatives rejected.** Not recorded.

**Consequences and limits.** The curl one-liner is served from the docs site. See [docs-site.md](docs-site.md).

**Where in code.** `site/`; `.github/workflows/pages.yml`.

**Source.** 3d40b33, 98588d8, 087efa1, dcc2438, 833b236.

---

<a id="adding-a-decision"></a>
## Adding a decision

Give it a new slug and an `<a id="…"></a>` line of its own before the heading. Never rename an existing slug: other
guides link to them. Fill in every field, cite the commit, and add it to the table of contents. When a decision is
replaced, keep its entry, set **Status** to superseded and name what replaced it.
