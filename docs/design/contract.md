# Bifröst V1: final implementation contract

This contract keeps the structure of design B and fixes every defect the judges found in it. It takes specific ideas from design A (minimal) and design C (correctness) where they make the result simpler or more correct. "ponytail" is the house engineering style: the laziest solution that actually works, with every deliberate corner cut recorded. Each item in §15 becomes a `// ponytail:` comment at the named location.

## Changes from B

| # | Judge defect in B | Fix | § |
|---|---|---|---|
| 1 | The orphan reaper kills every same-uid process with a `bifrost:` marker, including the user's real daemon during E2E. | Reaper deleted. **No pid is ever signalled** except a child this daemon spawned whose mount readiness timed out. Adoption only looks under this daemon's canonical root. | 6, 8 |
| 2 | Force unmount does lazy detach and then kills the process group, losing writes that are in flight. | Force is `fusermount3 -u -z` (Linux) or `diskutil unmount force` (macOS), **with no kill**. After a lazy detach the child keeps serving open files and exits when the last reference closes. | 6 |
| 3 | The `HostKeyChecking::AcceptNew` trust-on-first-use (TOFU) knob weakens host verification. | Enum and config key deleted. The constant `SSH_OPTS` never contains StrictHostKeyChecking, UserKnownHostsFile or GlobalKnownHostsFile. Test `argv_never_weakens_host_keys`. | 3, 6, 11 |
| 4 | `stat` on the mount root is answered from the kernel and sshfs caches, so a dead remote looks healthy. | The probe looks up a unique nonexistent name, `<mnt>/.bifrost-probe-<nonce>`. rclone gets `--dir-cache-time=15s` as its ceiling. | 6 |
| 5 | Warm-up treats an `Err` as "reported", so adopted tailscale mounts get unmounted. | `ready` requires a loaded config file and one **non-empty Ok** from every network provider, or `offline_grace_period` since the first successful config load (A6, A18). | 5, 8 |
| 6 | Row 11 force-unmounts `Missing` mounts, and fusermount exits non-zero, which loops. | `Missing` is a state transition (to Absent with backoff), not an action. `unmount` is idempotent: if the path is not in the mount table afterwards, the result is Ok. | 5, 6 |
| 7 | `generation` only bumps on spawn, so a late health result can land after an unmount. | `begin()` bumps `generation` for both mount and unmount. Provider results carry `task_gen`. | 5 |
| 8 | A persistent `Override::Mounted` can mount a DiscoverOnly machine that has an untrusted address. | Removed. Only `held` (manual unmount) is persisted. `POST …/mount` clears a hold and returns **403** for anything that isn't a candidate. | 4, 8 |
| 9 | A DNS NXDOMAIN makes the index `Ok(empty)`, so every machine flaps to Lost. | The registry never removes on absence. Absent observations age out at `expires_at`, and expiry is frozen while a provider is failing. | 2, 7 |
| 10 | The hickory `build()` call was missing a `?`. | `TokioResolver::builder_tokio()?.build()?` (verified). | 7 |
| 11 | Merging by provider config order lets untrusted DNS supply the connect host. | **Winner takes all by trust**: `Source.trust` ranks static 0 < tailscale 1 < http 2 < dns 3 (assigned by bifrost-config, B8). The selected observation's host, port, hints and metadata are the only ones used. | 4 |
| 12 | Any provider's exclude or deny, or a DNS-published tag, could deny a static machine. | A provider `exclude` drops only that provider's observation. Global deny is evaluated against the selected (most trusted) observation, so a lower-trust source can't redirect or deny a machine that a more trusted source reports. | 4 |
| 13 | `#[serde(transparent)]` newtypes bypass validators when state.json or DTOs are read. | `#[serde(try_from = "String", into = "String")]` on Name, Host, User, RemotePath and DriverSelector. | 2 |
| 14 | A missing config makes the daemon exit, so PRD §31's plain `bifrostd` can't start. | Missing config means the empty default config (root `~/machines`) plus a warning, and the poller adopts the file once it appears, provided its `mount.root` equals that default root; any other root is rejected like every root change and needs a restart (A22). `ready` stays false until a config file has loaded (A6). An invalid config means exit 2. | 8 |
| 15 | Knob and feature sprawl. | See §15 for everything cut: ssh timeouts, stat timeout, dir-cache time, http timeout, `auto_mount`, clap in the daemon, `/v1/snapshot`, the `events` CLI, FailureKind, the context parameters, and extra events. | 15 |
| 16 | tokio types in core (`watch::Receiver` inside MountHandle), and HostKeyChecking in core. | Core depends only on serde and thiserror. Child exit reaches the daemon through an `OnExit` callback in `MountRequest`. | 2 |
| 17 | S0 `todo!()` stubs panic in the M1 daemon, because drivers are probed at startup. | Safe-stub rule: stub providers return `Err(Unavailable)`, and stub drivers return `Unavailable` from `probe` and `Err(Unavailable)` from `mount`. | 13 |
| 18 | Client API had no signatures. | Frozen in §9. | 9 |
| 19 | The macOS mount table came from parsing `/sbin/mount` text. | `libc::getmntinfo(MNT_NOWAIT)` behind a static Mutex, as PRD §22 says. | 6 |
| 20 | 10 crates; plus nix, bytes, async-trait and clap in the daemon. | 8 crates, which is the PRD §14 practical layout. A `BoxFuture` alias replaces async-trait. No nix. libc only for macOS `getmntinfo`. | 1 |
| 21 | Bug the judges missed: a tailscale peer whose id falls back to a capitalised HostName is silently skipped. | `Name::parse` lowercases first. Config names must already be lowercase, and uppercase gets a "use lowercase" hint. | 2, 3 |
| 22 | Bug the judges missed: the only form for the remote login directory was `""`. | RemotePath is `~`, `~/rel` or `/abs`, which map to sftp paths `""`, `"rel"` and `"/abs"`. | 2 |
| 23 | A spec change on a remount wasn't detectable after a restart. | The fsname/devname marker is `bifrost:<id>@<fp16>`. The fingerprint covers the whole spec, including host, so adoption compares specs using the kernel mount table. | 5, 6 |

## Verified facts (this host, 2026-09-25)

Each fact was read from `~/.cargo/registry/src`, from crates.io `.crate` tarballs, or from the installed tools.

| Item | Fact |
|---|---|
| axum 0.8.9 | `impl Listener for tokio::net::UnixListener` (`#[cfg(unix)]`). `Router<()>: Service<IncomingStream<'_, L>>` for any `L`. `Sse::new` needs `TryStream<Ok=Event>`. `Sse::keep_alive` and `KeepAlive` need feature `tokio`. `Event::{id,event,data,comment}` exist. `Json` implements `OptionalFromRequest`. Route parameters use `{id}`. |
| hyper 1.11.1 / hyper-util 0.1.20 / http-body-util 0.1.5 | `hyper::client::conn::http1::handshake(io) -> (SendRequest, Connection)` and `send_request`. `hyper_util::rt::TokioIo` needs feature `tokio`. `BodyExt::collect` exists. `hyper::body::Bytes` is re-exported. |
| hickory-resolver 0.26.3 | Default features are `system-config` and `tokio` (no TLS). `TokioResolver::builder_tokio() -> Result<ResolverBuilder,_>`. `ResolverBuilder::{options_mut, build() -> Result<Resolver,_>}`. `Resolver::builder_with_config(ResolverConfig::from_name_servers(Vec<NameServerConfig>), hickory_resolver::net::runtime::TokioRuntimeProvider::default())`. `NameServerConfig::udp_and_tcp(ip)` has pub `connections: Vec<ConnectionConfig{pub port}>`. `ResolverOpts{pub timeout, pub attempts}`. `txt_lookup(name) -> Result<Lookup, NetError>`. `Lookup::{answers() -> &[Record], valid_until() -> Instant}`. `Record{pub ttl, pub data: RData}`. `RData::TXT(TXT{pub txt_data: Box<[Box<[u8]>]>})`. `NetError::is_no_records_found()` exists. |
| reqwest 0.12.28 | `default-features=false, features=["rustls-tls-native-roots"]` means rustls on **ring**, which compiles C, so it blocks the darwin check. `ClientBuilder::{timeout, redirect, user_agent}`. `Response::chunk()`. |
| ratatui 0.30.2 | Default feature `crossterm` pulls ratatui-crossterm 0.1.2, whose default feature `crossterm_0_29` gives crossterm 0.29. Exposes `ratatui::crossterm`, `ratatui::{init, restore}` and `ratatui::backend::TestBackend`. |
| tokio 1.53.1 | `process::Command::{process_group (unix), kill_on_drop}` and `Child::{id, start_kill, wait, try_wait}`. |
| toml 1.1.5 | `from_str` needs features `parse` and `serde`. Its errors carry line and column. |
| libc 0.2.189 (apple) | `getmntinfo(*mut *mut statfs, c_int)`, `MNT_NOWAIT`, `statfs{f_fstypename[16], f_mntonname[1024], f_mntfromname[1024], f_owner}`. |
| std (rustc 1.98) | `File::try_lock` (since 1.89), `Waker::noop` (since 1.85), `RandomState` and `hash_one`, `DirBuilderExt::mode`, `OpenOptionsExt::mode`. |
| sshfs 3.7.3 / fuse 3.14 | Has `-f`, `-F`, `-p`, `-o reconnect`, `idmap=`, `transform_symlinks`, `auto_unmount`, and forwards ssh `-o` options. Dangerous options that exist and are never used: `ssh_command`, `directport`, `passive`. |
| rclone 1.75.1 | `--sftp-ssh` is a SpaceSepList with double-quote quoting; rclone appends `-s sftp` and ignores internal config when it is set. Also verified: `--sftp-host`, `--sftp-shell-type`, `--sftp-disable-hashcheck`, `--config`, `--devname`, `--volname`, `--dir-cache-time`, `--vfs-cache-mode`, `--read-only`, and `nfsmount`. |
| tailscale `status --json` (live) | Top-level `BackendState`, `CurrentTailnet.MagicDNSEnabled`, `Peer` (a map), `Self`. Peer: `ID`, `HostName` (**duplicates seen live**), `DNSName` (trailing dot), `OS`, `TailscaleIPs[]`, `Online`. `Tags` is **absent** when the peer has none. |
| tools | fusermount3, ssh (OpenSSH 9.6), ssh-keyscan, docker, dig, jq, python3 and cc are present. Only Linux rust targets are installed. |

**Not verifiable here, flagged in the text:**
- the exit-0 semantics of the SSH preflight (§6);
- whether rclone accepts `--sftp-host` next to `--sftp-ssh`; it is passed anyway (§6);
- whether sshfs honours a user-supplied `fsname=` (§6, E2E p04). `strings /usr/bin/sshfs` shows it inserts `-osubtype=sshfs,fsname=%s` at argv[1], so a later user `-o fsname=` should win; S1 agent C verifies (A10).

---

## 1. Crate layout, dependencies, parallel build

Eight crates, following the PRD §14 practical layout. Crates are split only for PRD layering or for dependencies. Parallel agents get isolation from **git worktrees**, not from crate boundaries.

```
bifrost/
├── Cargo.toml  Cargo.lock  scripts/check.sh  README.md        # no rustfmt.toml: rustfmt defaults (P1)
├── crates/
│   ├── bifrost-core/src/{lib,validate,model,policy,registry,reconcile,events,api,fake}.rs
│   ├── bifrost-config/src/{lib,raw,paths}.rs
│   ├── bifrost-discovery/src/{lib,tailscale,dns,http}.rs   tests/fixtures/{tailscale_status.json,inventory.json}
│   ├── bifrost-mount/src/{lib,table,check,sshfs,rclone}.rs
│   ├── bifrost-client/src/lib.rs
│   ├── bifrost-daemon/src/{main,actor,api,state,reload}.rs      # bin "bifrostd"
│   ├── bifrost-cli/src/{main,output,doctor}.rs                  # bin "bifrost"
│   └── bifrost-tui/src/{main,app,ui}.rs                         # bin "bifrost-tui"
└── tests/e2e/{run.sh,lib.sh,config.tmpl.toml,sshd/Dockerfile,dns/Corefile,dns/zone.tmpl,inventory.py,
               p04_sshfs.sh,p05_api.sh,p06_recovery.sh,psec_hostkey.sh,p13a_adopt.sh,
               p07_tailscale.sh,p08_dns.sh,p09_rclone.sh,p10_http.sh,p12_reload.sh,p13_hardening.sh}
```

```toml
# /Cargo.toml — written in S0, never edited afterwards
[workspace]
resolver = "3"
members = ["crates/*"]

[workspace.package]
version = "0.1.0"
edition = "2024"
rust-version = "1.89"            # File::try_lock
license = "MIT OR Apache-2.0"

[workspace.dependencies]
bifrost-core      = { path = "crates/bifrost-core" }
bifrost-config    = { path = "crates/bifrost-config" }
bifrost-discovery = { path = "crates/bifrost-discovery" }
bifrost-mount     = { path = "crates/bifrost-mount" }
bifrost-client    = { path = "crates/bifrost-client" }
tokio = { version = "1.53.1", default-features = false }
serde = { version = "1.0.228", features = ["derive"] }
serde_json = "1.0.151"
toml = { version = "1.1.5", default-features = false, features = ["std", "serde", "parse"] }
thiserror = "2.0.20"
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", default-features = false, features = ["fmt", "std", "ansi"] }
clap = { version = "4.6.6", features = ["derive", "env"] }
ratatui = "0.30.2"
axum = { version = "0.8.9", default-features = false, features = ["http1", "json", "tokio"] }
hyper = { version = "1.11.1", features = ["client", "http1"] }
hyper-util = { version = "0.1.20", features = ["tokio"] }
http-body-util = "0.1.5"
futures-util = { version = "0.3.34", default-features = false, features = ["std"] }
hickory-resolver = "0.26.3"
reqwest = { version = "0.12.28", default-features = false, features = ["rustls-tls-native-roots"] }
libc = "0.2.189"

[profile.release]
lto = "thin"
codegen-units = 1
strip = true
# not panic = "abort": a panicking task must not kill the daemon
```

| crate | deps | dev-deps |
|---|---|---|
| bifrost-core | serde, thiserror | — |
| bifrost-config | core, serde, toml | — |
| bifrost-discovery | core, serde, serde_json, tokio{process,time,rt,macros,io-util}, hickory-resolver, reqwest, tracing | tokio{rt-multi-thread,net} |
| bifrost-mount | core, tokio{process,time,rt,sync,fs,macros,io-util}, tracing; `[target.'cfg(target_os="macos")'.dependencies] libc` | tokio{rt-multi-thread} |
| bifrost-client | hyper, hyper-util, http-body-util, serde, serde_json, thiserror, tokio{net,rt} | axum, tokio{rt-multi-thread,macros} |
| bifrost-daemon | core, config, discovery, mount, axum, futures-util, serde, serde_json, tokio{rt-multi-thread,macros,net,process,signal,sync,time,fs,io-util}, tracing, tracing-subscriber | bifrost-client |
| bifrost-cli | core, config, client, mount, clap, serde_json, tokio{rt,macros,time} | — |
| bifrost-tui | core, config, client, ratatui, tokio{rt-multi-thread,time} | — |

What this adds and removes:
- **Added** relative to the PRD §30 list: axum, hyper, hyper-util, http-body-util and futures-util. All of them are already compiled as dependencies of axum or reqwest, and all are required by the pre-committed HTTP-over-UDS decision. libc is also already compiled, since tokio depends on it.
- **Removed:** async-trait, notify, globset, nix and a direct crossterm dependency.

**Rules that keep parallel builds safe:**
1. S0 writes every manifest in final form, runs `cargo generate-lockfile && cargo build --workspace --all-targets`, and commits `Cargo.lock`. After S0, no agent edits any `Cargo.toml` or the lockfile.
2. S0 writes every public signature in §2, §3, §5 (`Msg`, `Tick` in `daemon/src/actor.rs`), §6, §7, §8 (`ApiCmd` in `daemon/src/api.rs`, `Deps` in `actor.rs`) and §9 as compiling stubs that follow the **safe-stub rule** (§13) (A3). S0 also fully implements and tests `core/validate.rs`, the `core/model.rs` helpers `MountSpec::{fingerprint, source}`, `marker`, `parse_marker` and `DriverSelector: TryFrom<String>` (A2), and `core/fake.rs`. Every `lib.rs` declares its modules in S0, so later agents only fill in files.
3. Each agent works in its own worktree (`git worktree add ../bf-<agent> -b <agent>`), edits only the files it owns (§13), and builds with `cargo test -p <crate>`. **All agents share `CARGO_TARGET_DIR=/home/samimishal/projects/rust/bifrost-target`** (P2): with 4 CPUs and about 2 GB free RAM, compiling the dependencies once beats per-worktree builds, and the workflow cap is 2 concurrent agents anyway.
4. The orchestrator merges one branch at a time and runs `scripts/check.sh` after each merge (`cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`). A public signature changes only with the orchestrator's sign-off.

**macOS compile check.** All OS-specific code lives in `bifrost-mount`, `bifrost-config::paths` and `bifrost-config::default_auto_order` (B8). `bifrost-discovery` and `bifrost-daemon` contain no macOS code except a runtime `cfg!()` in tailscale, which compiles on Linux. They are excluded because reqwest pulls in ring, and ring needs an Apple C toolchain (§15 row; the ring cross-compile experiment is deferred, B9).

```
rustup target add aarch64-apple-darwin
cargo check --target aarch64-apple-darwin -p bifrost-core -p bifrost-config -p bifrost-mount \
            -p bifrost-client -p bifrost-cli -p bifrost-tui
```

---

## 2. `bifrost-core` public API

Core does no I/O: no filesystem, network, process or clock access. Time comes in as `now: Instant` and randomness as `rand: u64`. Its dependencies are only serde and thiserror. The two traits are the plugin boundary that the PRD requires.

```rust
// ===== lib.rs =====
pub mod api; pub mod events; pub mod fake; pub mod model; pub mod policy;
pub mod reconcile; pub mod registry; pub mod validate;
pub use model::*;
pub use validate::{Host, Invalid, MachineId, MountId, Name, RemotePath, User};

/// Dyn-compatible async without async-trait: impls write `Box::pin(async move { .. })`.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// PRD §5 minus `ctx` (nothing to carry).
/// Ok  = complete current view; invalid records are skipped with tracing::warn!, never Err.
/// Err = "could not look": the registry freezes this provider's observations until its next Ok.
pub trait DiscoveryProvider: Send + Sync {
    fn name(&self) -> &str;
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>>;
}

/// PRD §5 minus `ctx`. Contract:
/// - mount:   Ok only once the target is in the OS mount table. Err ⇒ nothing mounted, no process left.
/// - inspect: returns within ~5 s even on a hung FUSE mount (errors fold into Degraded).
/// - unmount: idempotent (path absent from mount table afterwards ⇒ Ok).
///            force=false never detaches a busy mount; force=true never kills a process.
pub trait MountDriver: Send + Sync {
    fn name(&self) -> &str;
    fn probe(&self) -> BoxFuture<'_, DriverAvailability>;
    fn mount(&self, req: MountRequest) -> BoxFuture<'_, Result<MountHandle, MountError>>;
    fn inspect<'a>(&'a self, h: &'a MountHandle) -> BoxFuture<'a, MountState>;
    fn unmount<'a>(&'a self, h: &'a MountHandle, force: bool) -> BoxFuture<'a, Result<(), MountError>>;
}
```

```rust
// ===== validate.rs — the trust boundary. FULLY implemented and tested in S0. =====
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {what} {value:?}: {why}")]
pub struct Invalid { pub what: &'static str, pub value: String, pub why: &'static str }

// Every newtype below: #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
//                     #[serde(try_from = "String", into = "String")]   — never `transparent`.

/// Identity key AND a single path component. ASCII-lowercases, then `^[a-z0-9][a-z0-9._-]{0,62}$`.
/// ⇒ never "", ".", "..", "-x", ".x"; no '/', NUL or whitespace; case-insensitive filesystems can't collide.
pub struct Name(String);
pub type MachineId = Name;
pub type MountId = Name;            // == directory name under mount.root
impl Name { pub fn parse(s: &str) -> Result<Self, Invalid>; pub fn as_str(&self) -> &str; }

/// Either a std `IpAddr` literal (no brackets, no zone id, not unspecified), or a hostname: one trailing '.'
/// stripped, ≤253 bytes, labels 1..=63 of [A-Za-z0-9_-], no label starting with '-'. Stored lowercase.
/// IP literals are stored canonical (v4-mapped v6 → v4, v6 compressed), so a v4 cidr can't be dodged.
/// ⇒ never starts with '-'; never contains whitespace, '"', '\'', ',', '@', '/', '=', '%', or ':'
///   (':' only inside a v6 literal).
pub struct Host(String);
impl Host {
    pub fn parse(s: &str) -> Result<Self, Invalid>;
    pub fn as_str(&self) -> &str;
    pub fn ip(&self) -> Option<std::net::IpAddr>;
    pub fn for_colon(&self) -> String;             // "[fd7a::1]" for v6, else as_str()
}

/// `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}$`
pub struct User(String);
impl User { pub fn parse(s: &str) -> Result<Self, Invalid>; pub fn as_str(&self) -> &str; }

/// "~" | "~/<rel>" | "/" | "/<abs>"; ≤1024 bytes; no control chars (<0x20, 0x7f); no ':' ; no ".." component.
/// "/" parses (PRD §6.1 root mounts, B2).
/// Spaces are allowed: always a single argv element, never placed inside --sftp-ssh.
pub struct RemotePath(String);
impl RemotePath {
    pub fn parse(s: &str) -> Result<Self, Invalid>;
    pub fn as_str(&self) -> &str;
    pub fn sftp_path(&self) -> &str;               // "~"→"", "~/rel"→"rel", "/"→"/", "/abs"→"/abs"
}

pub fn tag(s: &str) -> Result<String, Invalid>;       // lowercased; ^[a-z0-9][a-z0-9_.:-]{0,62}$
pub fn meta_key(s: &str) -> Result<String, Invalid>;  // ^[a-z0-9_.-]{1,64}$
pub fn native_id(s: &str) -> Result<String, Invalid>; // ^[A-Za-z0-9._:-]{1,128}$
/// Display-only text: control chars and bidi/zero-width format chars → '?', truncated at `max` chars.
/// Callers pass 128 for display names, 256 for metadata values (the config rule) and 512 for errors and
/// log lines (A8).
pub fn clean(s: &str, max: usize) -> String;
/// First line of every driver log (§6 spawn): `LOG_HEADER` + the argv.
pub const LOG_HEADER: &str = "# bifrost exec: ";
/// Error text from a log or stderr: split on '\n' (trailing '\r' trimmed), skip blank (whitespace-only) lines
/// and lines starting with LOG_HEADER, keep the LAST lines that fit in `max` chars, each clean(line, max)ed,
/// joined with " | " (A8).
pub fn tail(s: &str, max: usize) -> String;
pub fn parse_duration(s: &str) -> Result<std::time::Duration, Invalid>; // ^[0-9]+(ms|s|m|h)$, > 0, ≤ 366d (so Instant + 3·d can't overflow), checked overflow

/// `[a-z0-9*?._-]{1,63}`; '*' = any run, '?' = one char; iterative two-pointer matcher.
#[derive(Clone, Debug, PartialEq, Eq)] pub struct Glob(String);
impl Glob { pub fn parse(s: &str) -> Result<Self, Invalid>; pub fn matches(&self, s: &str) -> bool; }

/// "10.0.0.0/8" | "fd7a::/48" | bare IP (= /32 or /128). Address families never cross-match.
/// IPv4-mapped v6 is rejected (write the v4 form): Host stores it as v4, so it could never match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Cidr { pub addr: std::net::IpAddr, pub prefix: u8 }
impl Cidr { pub fn parse(s: &str) -> Result<Self, Invalid>; pub fn contains(&self, ip: std::net::IpAddr) -> bool; }

pub fn fnv64(bytes: &[u8]) -> u64;   // FNV-1a 64: stable across Rust versions (used for fingerprints)
pub fn random_u64() -> u64;          // RandomState::new().build_hasher().finish() — std only
/// base = min(max, initial·2^min(failures.saturating_sub(1), 32)); returns base/2 + rand % (base/2 + 1ms)
/// ("equal jitter", ∈ [base/2, base]); failures = 0 behaves like 1, never underflows (C3).
pub fn backoff(failures: u32, initial: Duration, max: Duration, rand: u64) -> Duration;
```

```rust
// ===== model.rs =====
// Core is provider- and driver-agnostic (B8): no ProviderKind enum, no DRIVER_NAMES, no default_auto_order here.
// Provider kinds are plain strings with a numeric trust rank (registry::Source); driver names, the auto order and
// the trust ranks live in bifrost-config (§3).
// MountSpec::{fingerprint, source}, marker, parse_marker and DriverSelector: TryFrom<String> are implemented
// and tested in S0 (A2), because S1 agents B and C depend on them.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata { pub tags: BTreeSet<String>, pub values: BTreeMap<String, String> }   // tags via tag(), keys via meta_key(), values clean(v, 256)

/// Untrusted, pre-validated. user/path are used only with `honor_hints`. There is no driver hint (E2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountHints { pub user: Option<User>, pub path: Option<RemotePath> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineObservation {
    pub id: MachineId,
    pub name: String,                 // display, clean(name, 128)
    pub native_id: Option<String>,    // tailscale ID / TXT id= / HTTP id — matched only by the OWNING provider's
                                      // include_ids/exclude_ids; global `ids` match the machine id only (A19)
    pub addresses: Vec<Host>,         // ≥1; [0] = connect target
    pub port: Option<u16>,
    pub online: Option<bool>,         // Some only when the provider knows (tailscale, http)
    pub metadata: Metadata,
    pub hints: MountHints,            // static: user = config user (trusted)
    pub ttl: Option<Duration>,        // DNS only; registry floors it
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum DriverSelector { Auto, Named(String) }
// "auto" → Auto; anything else must match ^[a-z0-9][a-z0-9._-]{0,62}$ (no lowercasing) → Named.
// Core checks only this grammar; membership in DRIVER_NAMES is checked by bifrost-config (B8).

/// PRD §5 (machine = resolved id; options = read_only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    pub id: MountId, pub machine: MachineId,
    pub host: Host, pub port: Option<u16>, pub user: Option<User>,
    pub remote: RemotePath,
    pub local_path: PathBuf,          // canonical_root.join(id) — built only in reconcile::desired
    pub driver: DriverSelector,       // the SELECTOR text is fingerprinted ⇒ "auto" is sticky across probe flaps
    pub read_only: bool,
}
impl MountSpec {
    /// 16 lowercase hex = fnv64("id\0machine\0host\0port\0user\0remote\0local\0driver\0ro").
    pub fn fingerprint(&self) -> String;
    pub fn source(&self) -> String;                // "[user@]<host.for_colon()>:<remote.sftp_path()>"
}
pub fn marker(id: &MountId, fingerprint: &str) -> String;           // "bifrost:<id>@<fp16>"
pub fn parse_marker(source: &str) -> Option<(MountId, String)>;     // exact grammar; "agent-01" never matches "agent-01-x"

pub type OnExit = Box<dyn FnOnce(String) + Send + 'static>;         // called once, with an exit description
pub struct MountRequest { pub spec: MountSpec, pub log_path: PathBuf, pub on_exit: OnExit }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountHandle { pub id: MountId, pub driver: String, pub local_path: PathBuf,
                         pub fingerprint: String, pub pid: Option<u32> /* informational, never signalled */ }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountState { Missing, Healthy, Degraded(String), Stale(String) }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriverAvailability { Available { binary: PathBuf, detail: String }, Unavailable(String) }

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    #[error("unavailable: {0}")] Unavailable(String),     // binary missing, backend stopped, not implemented
    #[error("{0}")] Failed(String),                       // transport / protocol / timeout / whole response unusable
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountError {
    #[error("driver unavailable: {0}")] Unavailable(String),
    #[error("unmount blocked: busy (files open)")] Busy,   // the one busy string everywhere (C4)
    #[error("refused: {0}")] Refused(String),   // occupied path, symlink, not a dir, not empty, invalid request
    #[error("{0}")] Failed(String),             // preflight / driver log tail (tail(…, 512)), timeout
}
```

```rust
// ===== policy.rs =====
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    pub ids: Vec<String>, pub names: Vec<Glob>, pub cidrs: Vec<Cidr>,
    pub tags: Vec<String>, pub providers: Vec<String>, pub metadata: BTreeMap<String, String>,
}
impl Match {
    pub fn is_empty(&self) -> bool;
    /// include/allow: every NON-EMPTY kind matches (AND), any entry within a kind (OR); metadata: all pairs equal.
    /// `own` = this Match is `src.provider`'s own filter: only then do `ids` also match `o.native_id` (A19).
    pub fn all(&self, src: &Source, o: &MachineObservation, own: bool) -> bool;
    /// exclude/deny: any single primitive matches → Some("names=prod-*").
    /// cidrs fail CLOSED here: a non-static observation with no IP-literal address matches any cidr entry.
    pub fn any(&self, src: &Source, o: &MachineObservation, own: bool) -> Option<String>;
}
#[derive(Clone, Debug, Default, PartialEq)] pub struct ProviderFilter { pub include: Match, pub exclude: Match }
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy { pub allow: Match, pub deny: Match, pub filters: BTreeMap<String /*provider*/, ProviderFilter> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict { Allowed { by: String }, DiscoverOnly, Denied { by: String } }
impl Verdict { pub fn is_allowed(&self) -> bool; }
impl std::fmt::Display for Verdict {} // "allowed (tailscale.filter.include)" | "discover-only" | "denied (policy.deny names=prod-*)"

/// §4. `observed` is in trust order. Returns the verdict and the index of the selected observation.
pub fn evaluate(p: &Policy, observed: &[Observed]) -> (Verdict, usize);

// ===== registry.rs =====
/// Ord == trust order: `trust` (lower = more trusted; 0 = static), then kind, then provider name.
/// Ranks are assigned by bifrost-config (static 0, tailscale 1, http 2, dns 3); core only compares numbers (B8).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Source { pub trust: u8, pub kind: String, pub provider: String }
#[derive(Clone, Debug, PartialEq)]
pub struct Observed { pub source: Source, pub obs: MachineObservation, pub expires_at: Option<Instant> }
#[derive(Clone, Debug, PartialEq)]
pub struct Machine { pub id: MachineId, pub observed: Vec<Observed> /*trust order*/, pub selected: usize, pub verdict: Verdict }
impl Machine {
    pub fn obs(&self) -> &MachineObservation;
    pub fn source(&self) -> &Source;
    pub fn shadowed(&self) -> Vec<String>;
}

#[derive(Default)]
pub struct MachineRegistry { /* BTreeMap<MachineId, BTreeMap<Source, (MachineObservation, Option<Instant>)>>, failing: BTreeSet<String> */ }
impl MachineRegistry {
    /// Successful network refresh. Upserts each obs (a duplicate id within `obs`: the first wins, the rest are
    /// dropped; providers already warn, §7) with expires_at = now + max(obs.ttl.unwrap_or(ZERO), floor). Ids this
    /// provider reported earlier but not now are NOT removed; they age out. Clears `failing`. Returns ids that are
    /// new to the registry.
    pub fn apply_ok(&mut self, src: &Source, obs: Vec<MachineObservation>, now: Instant, floor: Duration) -> Vec<MachineId>;
    /// Failed refresh: expire() skips this provider until its next apply_ok (freeze, not drop).
    pub fn mark_failed(&mut self, provider: &str);
    /// Authoritative, no expiry: static observations on every config apply. Returns (new ids, gone ids).
    pub fn replace(&mut self, src: &Source, obs: Vec<MachineObservation>) -> (Vec<MachineId>, Vec<MachineId>);
    pub fn remove_provider(&mut self, provider: &str) -> Vec<MachineId>;   // gone ids
    pub fn expire(&mut self, now: Instant) -> Vec<MachineId>;             // gone ids (no next_expiry: E3)
    pub fn machines(&self, p: &Policy) -> Vec<Machine>;                   // sorted by id
}
```

```rust
// ===== reconcile.rs =====
#[derive(Clone, Debug, PartialEq)]
pub struct MountTemplate { pub user: Option<User>, pub remote: RemotePath, pub driver: DriverSelector,
                           pub read_only: bool, pub honor_hints: bool }
#[derive(Clone, Debug, PartialEq)]
pub struct StaticMount { pub local: MountId, pub remote: RemotePath, pub driver: DriverSelector, pub read_only: bool }

pub struct DesiredInput<'a> {
    pub root: &'a Path,                                          // canonical
    pub machines: &'a [Machine],
    pub static_mounts: &'a BTreeMap<MachineId, Vec<StaticMount>>,
    pub templates: &'a BTreeMap<String /*provider*/, MountTemplate>,
    pub held: &'a BTreeSet<MountId>,
    pub auto_order: &'a [String],
    pub probes: &'a BTreeMap<String, DriverAvailability>,
}
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate { pub spec: MountSpec, pub driver: Result<String, String>,   // fingerprint = spec.fingerprint() (E5)
                       pub held: bool, pub online: Option<bool> }
pub struct Desired { pub candidates: BTreeMap<MountId, Candidate>, pub conflicts: Vec<String> }
/// Candidates only for verdict = Allowed. `held` is carried; desired = !held.
pub fn desired(i: &DesiredInput) -> Desired;
/// Named: Ok(n) iff n is Available, else Err("<n> unavailable: <reason>") — never substitutes.
/// Auto: first Available entry in auto_order, else Err("no available driver (tried …)").
pub fn select_driver(sel: &DriverSelector, auto_order: &[String],
                     probes: &BTreeMap<String, DriverAvailability>) -> Result<String, String>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)] pub enum Phase { #[default] Absent, Mounting, Mounted, Unmounting }
#[derive(Clone, Debug, PartialEq, Eq, Default)] pub enum Health { #[default] Unknown, Healthy, Degraded(String), Stale(String) }
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)] pub enum Reason { NotDesired, Manual, Stale, SpecChanged, OfflineGrace }
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Timing { pub grace: Duration, pub retry_initial: Duration, pub retry_max: Duration }

#[derive(Clone, Debug, Default)]
pub struct MountRuntime {
    pub phase: Phase, pub handle: Option<MountHandle>, pub health: Health,
    pub degraded_since: Option<Instant>, pub generation: u64, pub failures: u32,
    pub mounted_at: Option<Instant>,  // set by mount_done(Ok); cleared when the mount goes Absent (A15)
    pub mount_retry_at: Option<Instant>, pub unmount_retry_at: Option<Instant>,
    pub last_error: Option<String>,
    pub offline: bool,            // set by an OfflineGrace unmount; cleared by a successful mount
    pub force_requested: bool,    // API `unmount --force`
    pub adopted: bool, pub probing: bool,
}
impl MountRuntime {
    pub fn adopted(h: MountHandle) -> Self;                                      // Mounted, health Unknown, adopted=true, mounted_at None
    pub fn begin(&mut self, phase: Phase) -> u64;                                // generation += 1 (BOTH ops); phase = Mounting|Unmounting
    pub fn mount_done(&mut self, generation: u64, r: Result<MountHandle, MountError>, now: Instant, t: &Timing, rand: u64) -> Option<Event>;
    pub fn unmount_done(&mut self, generation: u64, why: Reason, r: Result<(), MountError>, now: Instant, t: &Timing, rand: u64) -> Option<Event>;
    pub fn health(&mut self, generation: u64, s: MountState, now: Instant, t: &Timing, rand: u64) -> Option<Event>;
}   // every transition ignores a stale generation and returns an Event only on a state change (§5).
    // `gen` is a reserved keyword in edition 2024, hence `generation` (A1); `task_gen` is fine.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitReason { InFlight, WarmingUp, NoDriver(String), MachineOffline, Backoff(Duration) }  // remaining time, computed in decide (C2)
/// PRD §10 action set. Only Mount / Unmount / Remount have side effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    NoOp,
    Mount { driver: String },
    Unmount { force: bool, why: Reason },
    Remount { force: bool, why: Reason },   // executed as an unmount; the Mount follows via row 9 on a later pass
    Degraded(String),
    Waiting(WaitReason),
}
impl Action { pub fn has_side_effect(&self) -> bool; pub fn kind(&self) -> &'static str; }
impl std::fmt::Display for Action {}          // "noop" | "mount (sshfs)" | "waiting (backoff 3s)" | …

pub struct PlanInput<'a> { pub now: Instant, pub ready: bool, pub grace: Duration,
    pub candidates: &'a BTreeMap<MountId, Candidate>, pub runtimes: &'a BTreeMap<MountId, MountRuntime> }
pub fn decide(c: Option<&Candidate>, rt: &MountRuntime, now: Instant, ready: bool, grace: Duration) -> Action; // §5 table
pub fn plan(i: &PlanInput) -> Vec<(MountId, Action)>;            // ids = candidates ∪ runtimes, sorted; pure
pub fn next_wakeup(runtimes: &BTreeMap<MountId, MountRuntime>, grace: Duration, now: Instant) -> Option<Instant>; // only deadlines > now (S1 sign-off)

/// PRD §12 states. Ord == display severity (used when aggregating a machine's mounts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability { Unknown, Discovered, Eligible, Mounted, Connecting, Unmounting, Offline, Degraded, Failed }
pub fn mount_availability(rt: Option<&MountRuntime>, c: Option<&Candidate>) -> Availability;   // desired + driver Err ⇒ Failed (B15)
pub fn machine_availability(m: &Machine, mounts: &[Availability]) -> Availability;             // Allowed, no mounts ⇒ Eligible (B15)
```

```rust
// ===== events.rs — PRD §20 + MountDegraded =====
// MachineEligible and DriverUnavailable are emitted by the actor on transitions (B1): it keeps the previous
// verdicts and probe results and emits on a change to Allowed, or on Available → Unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    MachineDiscovered { machine: String, provider: String },
    MachineLost { machine: String },
    MachineEligible { machine: String, via: String },
    MountRequested { mount: String, driver: String },
    MountStarted { mount: String, driver: String, pid: Option<u32> },
    MountHealthy { mount: String },
    MountDegraded { mount: String, reason: String },
    MountFailed { mount: String, error: String, attempt: u32, retry_in_ms: Option<u64> },
    UnmountStarted { mount: String, reason: String },
    UnmountComplete { mount: String },
    DriverUnavailable { driver: String, reason: String },
    ConfigurationReloaded { ok: bool, errors: Vec<String> },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRecord { pub seq: u64, pub ts_unix_ms: u64, pub event: Event }

// ===== api.rs — wire DTOs (all #[derive(Clone, Debug, Serialize, Deserialize)]) =====
pub struct StatusDto {                    // GET /v1/status = full snapshot (the TUI polls this)
    pub version: String, pub pid: u32, pub uptime_secs: u64, pub socket: String,
    pub config_path: String, pub config_errors: Vec<String>,   // non-empty ⇒ running on the previous config
    pub mount_root: String, pub ready: bool, pub ssh_agent: bool,
    pub providers: Vec<ProviderDto>, pub drivers: Vec<DriverDto>,
    pub auto_driver: Option<String>,      // select_driver(&Auto, active auto_order, probes).ok(); what status, doctor and the TUI print (E5)
    pub machines: Vec<MachineDto>, pub mounts: Vec<MountDto>,
    pub conflicts: Vec<String>, pub events: Vec<EventRecord>,   // last 200
}
pub struct ProviderDto { pub name: String, pub kind: String /* B8 */, pub machines: usize, pub refreshes: u64,
                         pub last_ok_secs_ago: Option<u64>, pub last_error: Option<String> }
pub struct DriverDto { pub name: String, pub available: bool, pub binary: Option<String>, pub detail: String }  // no auto_rank (E5)
pub struct MachineDto { pub id: String, pub name: String, pub source: String, pub shadowed: Vec<String>,
    pub address: String, pub port: Option<u16>, pub online: Option<bool>, pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>, pub verdict: String,   // no `eligible`: the verdict says it (E5)
    pub state: Availability, pub mounts: Vec<String> }
pub struct MountDto { pub id: String, pub machine: String, pub driver: Option<String>, pub local_path: String,
    pub remote: String /* MountSpec::source() */, pub state: Availability, pub detail: String,
    pub desired: bool, pub held: bool, pub adopted: bool, pub pid: Option<u32>, pub failures: u32,
    pub retry_in_secs: Option<u64>, pub last_error: Option<String>, pub action: String }
pub struct ActionDto { pub mount: String, pub action: String }
pub struct LogDto { pub mount: String, pub path: String, pub lines: Vec<String> }
pub struct ReloadDto { pub ok: bool, pub errors: Vec<String> }
pub struct UnmountReq { #[serde(default)] pub force: bool }
pub struct ErrorDto { pub error: String }

// ===== fake.rs — always compiled, #[doc(hidden)], std only (~100 lines); complete in S0 (S0.6) =====
pub struct FakeDiscovery { pub name: String, pub result: Mutex<Result<Vec<MachineObservation>, DiscoveryError>> }
pub struct FakeDriver {
    pub name: String,
    pub mounted: Mutex<BTreeMap<MountId, MountHandle>>,
    pub states: Mutex<BTreeMap<MountId, MountState>>,
    pub fail_next: Mutex<BTreeSet<MountId>>,
    pub calls: Mutex<Vec<String>>,
    pub exits: Mutex<BTreeMap<MountId, OnExit>>,   // the on_exit of each successful mount (B12)
    pub busy: Mutex<BTreeSet<MountId>>,            // graceful unmount of these → Err(Busy) (B12)
}
impl FakeDriver {
    pub fn new(name: &str) -> Self;
    pub fn set_state(&self, id: &str, s: MountState);
    pub fn fail_next(&self, id: &str);
    pub fn exit(&self, id: &str, detail: &str);    // removes and calls that mount's on_exit(detail) (B12)
}
pub fn obs(id: &str, addr: &str) -> MachineObservation;
pub fn block_on<F: Future>(f: F) -> F::Output;   // one poll with Waker::noop(); fakes never pend
```

---

## 3. Configuration

**Location:** `$BIFROST_CONFIG`, otherwise `~/.config/bifrost/config.toml`, on both OSes. Every table uses `#[serde(deny_unknown_fields)]`, so a typo or a `password = …` key is an error.

```toml
version = 1                                   # optional; only 1 accepted

[mount]
root = "~/machines"                           # ~ and $VAR expanded; absolute; not "/", not $HOME, no ".."
default_driver = "auto"                       # auto | sshfs | rclone | rclone-nfs
auto_order = ["sshfs", "rclone"]              # default: linux [sshfs, rclone]; macos [rclone-nfs, rclone, sshfs]
ssh_config = "~/.config/bifrost/ssh_config"   # optional → `-F` for ssh/sshfs/rclone-ssh; must exist; no '"'
vfs_cache_mode = "writes"                     # rclone: off | minimal | writes | full

[daemon]
discovery_interval = "30s"                    # default per-provider refresh
health_interval = "15s"
reconcile_interval = "60s"                    # slow fallback (the PRD §13 sample "30s" is accepted)
mount_timeout = "30s"

[reconciliation]
offline_grace_period = "5m"                   # also caps startup warm-up
retry_initial = "2s"
retry_max = "1m"

[policy.allow]                                # global explicit allow: AND across kinds, OR within a kind
ids = ["agent-07"]

[policy.deny]                                 # global deny: any primitive; beats everything, static included
names = ["prod-*"]
cidrs = ["10.99.0.0/16"]
tags = ["prod"]
providers = []
metadata = { env = "production" }

[[discovery]]
type = "tailscale"                            # tailscale | dns | http
# name = "tailscale"                          # default = type; Name grammar; unique; "static" reserved
interval = "30s"                              # default daemon.discovery_interval

[discovery.filter]
include_tags = ["dev", "agent"]               # provider strips "tag:"
exclude_names = ["*-old"]

[discovery.mount]                             # template for Allowed machines of THIS provider
user = "sami"                                 # default: none (ssh_config / local user decides)
remote = "~"                                  # default "~" (remote login dir)
driver = "auto"                               # default mount.default_driver
read_only = false
honor_hints = false                           # true ⇒ record user=/path= win (validated); there is no driver hint (E2)

[[discovery]]
type = "dns"
name = "infra"
domain = "infra.example.com"
nameservers = ["10.0.0.2", "127.0.0.1:5353"]  # optional "ip" | "ip:port"; default: system resolver

[discovery.filter]
include_names = ["agent-*", "build-*"]

[discovery.mount]
user = "ubuntu"
honor_hints = true

[[discovery]]
type = "http"
name = "inventory"
url = "https://inventory.example.com/bifrost/v1/machines"  # https, or http:// only to 127.0.0.1 / [::1] / localhost
headers = { Authorization = "Bearer ${BIFROST_INVENTORY_TOKEN}" }
interval = "60s"

[discovery.filter]
include_cidrs = ["10.20.0.0/16"]
include_metadata = { env = "dev" }

[[machines]]                                  # PRD §13 form
name = "build"
host = "10.0.0.18"
user = "sami"
port = 22
tags = ["ci"]
metadata = { env = "ci" }

[[machines.mounts]]
remote = "/home/sami"
local = "build"
driver = "sshfs"

[[machines.mounts]]
remote = "/srv/artifacts"
local = "build-artifacts"
driver = "rclone"
read_only = true

[[machines]]                                  # PRD §31 shorthand: one mount, local = name
name = "agent-01"
host = "agent-01"
user = "sami"
remote = "/home/sami"                         # optional: driver, read_only
```

```rust
// bifrost-config/src/raw.rs — serde mirror; every struct #[derive(Deserialize)] #[serde(deny_unknown_fields)], fields Option/default.
// RawDiscovery is ONE flat struct (not a tagged enum), so unknown-field errors keep toml line/col;
// per-type key checks are part of validation.
struct RawConfig { version: Option<u32>, mount: RawMount, daemon: RawDaemon, reconciliation: RawRecon,
                   policy: RawPolicy, discovery: Vec<RawDiscovery>, machines: Vec<RawMachine> }
struct RawMount { root, default_driver, ssh_config, vfs_cache_mode: Option<String>, auto_order: Option<Vec<String>> }
struct RawDaemon { discovery_interval, health_interval, reconcile_interval, mount_timeout: Option<String> }
struct RawRecon { offline_grace_period, retry_initial, retry_max: Option<String> }
struct RawPolicy { allow: RawMatch, deny: RawMatch }
struct RawMatch { ids, names, cidrs, tags, providers: Vec<String>, metadata: BTreeMap<String, String> }
struct RawDiscovery { r#type: String, name: Option<String>, interval: Option<String>, domain: Option<String>,
    nameservers: Vec<String>, url: Option<String>, headers: BTreeMap<String, String>,
    filter: RawFilter, mount: RawTemplate }
struct RawFilter { include_ids, include_names, include_cidrs, include_tags: Vec<String>, include_metadata: BTreeMap<String, String>,
                   exclude_ids, exclude_names, exclude_cidrs, exclude_tags: Vec<String>, exclude_metadata: BTreeMap<String, String> }
struct RawTemplate { user: Option<String>, remote: Option<String>, driver: Option<String>, read_only: Option<bool>, honor_hints: Option<bool> }
struct RawMachine { name: String, host: String, port: Option<i64>, user: Option<String>, tags: Vec<String>,
    metadata: BTreeMap<String, String>, remote: Option<String>, driver: Option<String>, read_only: Option<bool>,
    mounts: Vec<RawMount1> }
struct RawMount1 { remote: String, local: Option<String>, driver: Option<String>, read_only: Option<bool> }
```

```rust
// bifrost-config/src/lib.rs — validated output (all #[derive(Clone, Debug, PartialEq)])
pub struct Config {
    pub path: PathBuf,
    pub root: PathBuf,                     // expanded + absolute (daemon canonicalizes)
    pub default_driver: DriverSelector,
    pub auto_order: Vec<String>,
    pub ssh_config: Option<PathBuf>,
    pub vfs_cache_mode: String,
    pub timings: Timings,
    pub policy: Policy,                    // core
    pub providers: Vec<ProviderConfig>,
    pub machines: Vec<StaticMachine>,
}
pub struct Timings { pub discovery_interval: Duration, pub health_interval: Duration, pub reconcile_interval: Duration,
    pub mount_timeout: Duration, pub offline_grace_period: Duration, pub retry_initial: Duration, pub retry_max: Duration }
pub struct ProviderConfig { pub name: String, pub kind: String /* "tailscale" | "dns" | "http" */, pub interval: Duration,
                            pub template: MountTemplate, pub spec: ProviderSpec }
impl ProviderConfig { pub fn source(&self) -> Source; }  // Source{trust: rank of kind, kind, provider: name}

// Owned here, not in core (B8): adding a provider or a driver never edits bifrost-core.
/// Trust ranks, lower = more trusted. Core only compares the numbers; `trust == 0` means static.
pub const TRUST: [(&str, u8); 4] = [("static", 0), ("tailscale", 1), ("http", 2), ("dns", 3)];
pub fn static_source() -> Source;                          // Source{trust: 0, kind: "static", provider: "static"}
pub const DRIVER_NAMES: [&str; 3] = ["sshfs", "rclone", "rclone-nfs"];
pub fn default_auto_order() -> Vec<String>;  // cfg!(target_os="macos") ? [rclone-nfs, rclone, sshfs] : [sshfs, rclone]
pub enum ProviderSpec { Tailscale, Dns { domain: Host, nameservers: Vec<SocketAddr> },
                        Http { url: String, headers: Vec<(String, Secret)> } }
#[derive(Clone, PartialEq)] pub struct Secret(pub String);              // Debug prints "***"
pub struct StaticMachine { pub id: MachineId, pub host: Host, pub port: Option<u16>, pub user: Option<User>,
    pub tags: BTreeSet<String>, pub metadata: BTreeMap<String, String>, pub mounts: Vec<StaticMount> }
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConfigError { pub path: String, pub message: String }         // Display "error: {path}: {message}"

/// Pure. HOME is looked up through `env("HOME")`. TOML syntax/shape error ⇒ exactly one ConfigError (with line:col).
/// Otherwise returns ALL semantic errors, sorted. parse("", …) == the default config.
pub fn parse(text: &str, path: &Path, env: &dyn Fn(&str) -> Option<String>) -> Result<Config, Vec<ConfigError>>;
pub fn load(path: &Path) -> Result<Config, Vec<ConfigError>>;           // read + parse with the process env
impl Config {
    pub fn static_observations(&self) -> Vec<MachineObservation>;       // addresses=[host], hints.user=user, online None, ttl None
    pub fn static_mounts(&self) -> BTreeMap<MachineId, Vec<StaticMount>>;
    pub fn templates(&self) -> BTreeMap<String, MountTemplate>;
}
pub mod paths {
    pub fn config_path() -> PathBuf;   // $BIFROST_CONFIG | ~/.config/bifrost/config.toml
    pub fn state_dir() -> PathBuf;     // $BIFROST_STATE_DIR | $XDG_STATE_HOME/bifrost | ~/.local/state/bifrost
    pub fn socket_path() -> PathBuf;   // $BIFROST_SOCKET | macos ~/Library/Caches/bifrost/bifrost.sock
                                       //  | linux $XDG_RUNTIME_DIR/bifrost/bifrost.sock | ~/.cache/bifrost/bifrost.sock
}
```

**Validation rules.** Every error is collected, then sorted by `(path, message)`, so `bifrost config check` prints the same bytes every time.

| path | rule |
|---|---|
| `version` | absent or 1 |
| any key | unknown keys are rejected (`deny_unknown_fields`) |
| durations | `parse_duration`; every interval ≥ 1s; `retry_initial ≤ retry_max`; `mount_timeout ≤ 5m` |
| `mount.root` | expanded; absolute; not `/`; not `$HOME`; no `..` component |
| `default_driver`, every `driver` | `auto` or one of this crate's `DRIVER_NAMES` (OS-agnostic; unavailable on this OS ⇒ runtime `Waiting(NoDriver)`). Core's `DriverSelector` checks only the grammar (B8) |
| `auto_order` | non-empty, subset of `DRIVER_NAMES`, no duplicates |
| `mount.ssh_config` | expanded; absolute; exists; contains no `"` (it is quoted inside `--sftp-ssh`) |
| `vfs_cache_mode` | off \| minimal \| writes \| full |
| `discovery[i].type` | tailscale \| dns \| http |
| `discovery[i].name` | defaults to the type; `Name::parse`; unique (a second instance of a type needs a `name`); `static` is reserved |
| per-type keys | `domain`/`nameservers` only for dns (domain required, `Host`, not an IP); `url`/`headers` only for http (url required) |
| `nameservers` | `IpAddr` (port 53) or `SocketAddr` |
| `url` | `https://`, or `http://` only for 127.0.0.1, [::1] or localhost |
| `headers` | name is an RFC 7230 token; value after expansion has no CR, LF or NUL |
| filter / policy | `policy.*.ids` pass `Name::parse` (they match the machine id only, A19); filter `*_ids` pass `native_id` (or `Name`); names pass `Glob::parse`; cidrs pass `Cidr::parse`; tags pass `tag`; metadata keys pass `meta_key` and values are ≤256 chars with no control characters; `policy.*.providers` name an existing provider or kind |
| `discovery[i].mount` | user passes `User::parse`; remote passes `RemotePath::parse`; driver as above |
| `machines[i].name` | must already be lowercase (hint "use lowercase"), then `Name::parse`; unique |
| `machines[i]` | host `Host::parse`; port 1..=65535; user `User::parse`; tags `tag`; exactly one of `remote` and `[[mounts]]` ("has no mounts" / "both remote and mounts"); `driver`/`read_only` only together with `remote` |
| `mounts[j]` | remote `RemotePath::parse`; `local` defaults to the machine name only when the machine has exactly one mount; otherwise it is required and passes `Name::parse` |
| locals | unique across all static mounts, shorthand included |

**Expansion.**
- A leading `~` or `~/` becomes `$HOME`. `$NAME` and `${NAME}` come from the environment. `$$` is a literal `$`.
- An undefined variable is an **error**, never an empty string.
- Expansion applies **only** to `mount.root`, `mount.ssh_config`, `discovery.url` and header values.
- Names, hosts and remote paths are never expanded. In a remote path, `~` means the remote home.

**How an Allowed machine becomes mount specs** (`reconcile::desired`). `s` is the selected observation; `P` is its provider's template.

```
selected source has trust 0 (static):
    one spec per StaticMount: host = s.addresses[0], port = s.port, user = s.hints.user (config),
    remote/driver/read_only from the StaticMount
otherwise:
    one spec:  id = machine id;  host = s.addresses[0];  port = s.port   (address data from the selected obs)
    user   = if P.honor_hints { s.hints.user.or(P.user) } else { P.user }   // None ⇒ ssh decides
    remote = if P.honor_hints { s.hints.path.or(P.remote) } else { P.remote }
    driver = P.driver;  read_only = P.read_only                             // there is no driver hint (E2)
local_path = root.join(id)                                                  // the only place local paths are built
collision:  static locals are claimed first → a discovered id equal to a claimed local is skipped
            and reported in Desired.conflicts ("agent-01: local name taken by static machine build")
```

---

## 4. Policy semantics

| primitive | config keys | include / allow matches when | exclude / deny matches when |
|---|---|---|---|
| exact id | `ids` (global) | the machine id equals an entry; `native_id` is **never** matched here (A19) | same |
| exact id | `include_ids` / `exclude_ids` (provider filter) | the machine id, or the `native_id` of this provider's own observation, equals an entry | same |
| name glob | `names` / `*_names` | the id glob-matches an entry | same |
| CIDR | `cidrs` / `*_cidrs` | an IP-literal address is inside an entry (hostnames are never resolved) | the same, **or** a non-static observation has no IP literal (fail closed) |
| tag | `tags` / `*_tags` | any listed tag is in `metadata.tags` | same |
| provider | `providers` (global only) | the provider name or kind name is listed | same |
| metadata | `metadata` / `*_metadata` | **all** pairs equal | **any** pair equal |

Include and allow combine as AND across non-empty kinds and OR within a kind. Exclude and deny fire on any single primitive.

**`evaluate(policy, observed)`.** `observed` is in trust order: `Source.trust` ascending (bifrost-config assigns static 0 < tailscale 1 < http 2 < dns 3), then provider name. The first match wins. A provider filter is evaluated with `own = true` against its own provider's observation; global allow/deny with `own = false` (A19).

```
1. live = observations whose provider filter exclude does NOT match   // an exclude drops only that observation
   live empty                                   → Denied{by: "<provider>.filter.exclude <prim>"}, selected = 0
2. sel = live[0]                                // WINNER TAKES ALL: the most trusted remaining observation
3. policy.deny.any(sel)                         → Denied{by: "policy.deny <prim>"}          // deny wins, static included
4. sel.source.trust == 0                        → Allowed{by: sel.source.kind} ("static")   // static = explicit allow (B8)
5. filter(sel.provider).include non-empty ∧ .all(sel) → Allowed{by: "<provider>.filter.include"}
6. policy.allow non-empty ∧ policy.allow.all(sel)     → Allowed{by: "policy.allow"}
7. otherwise                                    → DiscoverOnly                              // PRD §7: discover yes, mount no
```

What this rule guarantees:
- **Untrusted data can't redirect a trusted machine.** A lower-trust source (DNS/HTTP) can never set the connect host, port, user, tags or metadata of a machine that a more trusted source reports, even when that source's verdict is DiscoverOnly. Shadowed sources are listed in `MachineDto.shadowed`.
- **Native ids are scoped to their provider (A19).** `native_id` is matched only by the owning provider's `include_ids`/`exclude_ids`; global `ids` match the machine id only. So a DNS or HTTP record publishing `id=<a tailscale node ID>` can't satisfy a global `allow ids=[…]`.
- **Global allow rules can be satisfied by any provider.** A global allow on id, name, tag or metadata matches whichever provider supplies the selected observation, including DNS/HTTP, which control their own names, tags and metadata. Scope such rules with `providers=[…]`, or use a provider-level include instead.
- **Untrusted data can't deny a trusted machine.** A DNS record publishing `tags=prod` can't deny a static or tailscale machine, because global deny sees only the selected observation.
- **The trade-off is deliberate (§15).** A DNS include can't mount an id that tailscale also reports without allowing it. To hand such a machine to DNS, add `exclude_names=[…]` to the tailscale filter, or allow it globally.
- **Manual mount never overrides policy.** `bifrost mount X` on a DiscoverOnly or Denied machine returns 403 with the verdict. It only clears a manual hold.
- **Deny rules on source-controlled attributes are advisory.** A deny on tags, metadata or names can only deny what that same (untrusted) source controls. Real protection is the allow rules plus SSH host-key verification.

---

## 5. Reconciliation

**One pass, pure, recomputed every time.**

```
registry.expire(now)
machines  = registry.machines(&cfg.policy)
desired   = reconcile::desired(&DesiredInput{ root, machines, static_mounts, templates, held, auto_order, probes })
plan      = reconcile::plan(&PlanInput{ now, ready, grace, candidates: &desired.candidates, runtimes })
execute side-effect actions (spawn tasks), publish snapshot, persist state.json if dirty
```

**Decision table** (`decide`; first match wins). Terms:
- `D` means a candidate exists and is not held.
- `H` is `rt.health`.
- ⊳U means: if `unmount_retry_at > now`, the result becomes `Waiting(Backoff(remaining))` for rows 3 and 5, and `Degraded(last_error)` for rows 10–12.
- `why` in rows 3 and 5 is `if c.is_some_and(|c| c.held) { Manual } else { NotDesired }` (A14).

| # | D | phase | condition | Action |
|---|---|---|---|---|
| 1 | * | Mounting / Unmounting | — | `Waiting(InFlight)` |
| 2 | no | Absent | — | `NoOp` (runtime is dropped once it has no pending retry) |
| 3 | no | Mounted | H = Stale ∨ force_requested | `Unmount{force: true, why}` ⊳U |
| 4 | no | Mounted | `!ready` ∧ not held | `Waiting(WarmingUp)` |
| 5 | no | Mounted | — | `Unmount{force: false, why}` ⊳U (busy → backoff, never auto-forced) |
| 6 | yes | Absent | `driver` is Err | `Waiting(NoDriver)` |
| 7 | yes | Absent | `online == Some(false)` | `Waiting(MachineOffline)` |
| 8 | yes | Absent | `mount_retry_at > now` | `Waiting(Backoff(mount_retry_at − now))` |
| 9 | yes | Absent | — | `Mount{driver}` |
| 10 | yes | Mounted | H = Stale | `Remount{force: true, Stale}` ⊳U |
| 11 | yes | Mounted | `handle.fingerprint ≠ cand.spec.fingerprint()` ∧ `cand.driver.is_ok()` ∧ `cand.online != Some(false)` (A16) | `Remount{force: H = Degraded, SpecChanged}` ⊳U |
| 12 | yes | Mounted | H = Degraded ∧ `now − degraded_since ≥ grace` | `Unmount{force: true, OfflineGrace}` ⊳U |
| 13 | yes | Mounted | H = Degraded ∨ `handle.fingerprint ≠ cand.spec.fingerprint()` | `Degraded(reason)`: H's reason when H = Degraded (sshfs `reconnect` + ServerAlive is handling it), else `"change pending: <driver error \| machine offline>"` (A16) |
| 14 | yes | Mounted | — | `NoOp` (Healthy, Unknown, or offline-but-Healthy: a working mount is never removed) |

Notes on the table:
- `Remount` is executed exactly like `Unmount`. The new mount comes from row 9 on a later pass, so the offline and backoff gates still apply.
- Stale cleanup is gated only by `unmount_retry_at`, never by mount backoff.
- A spec change includes a host change such as a DNS IP move, because the fingerprint covers the host. It remounts gracefully. If the mount is busy, it stays `Degraded("unmount blocked: busy (files open)")` and retries (C4).
- Row 11 never drops a working mount for a spec it can't mount: while the new spec's driver is unavailable or the machine is offline, row 11 doesn't match and the pass falls through. A hung mount still reaches row 12 after grace, a Degraded one shows its real reason (row 13), and a working one stays and shows `Degraded("change pending: …")` (row 13, A16).

**Runtime transitions** (pure, in `MountRuntime`). A message with the wrong `generation` is ignored. `bo` means `now + backoff(failures, retry_initial, retry_max, rand)`.

| input | effect | event |
|---|---|---|
| `begin(Mounting \| Unmounting)` | `generation += 1`; set phase | MountRequested / UnmountStarted (emitted by the actor) |
| `mount_done(Ok h)` | Mounted; `handle = h`; H = Unknown; `mounted_at = Some(now)`; `offline = false`; `last_error = None`; `mount_retry_at = None` (**failures not reset**) | MountStarted |
| `mount_done(Err e)` | Absent; `failures += 1`; `mount_retry_at = bo`; `last_error = e` | MountFailed |
| `unmount_done(Ok)` | Absent; `handle = None`; H = Unknown; clear `mounted_at`, `degraded_since`, `unmount_retry_at`, `force_requested`. If why = OfflineGrace: `offline = true`, `failures += 1`, `mount_retry_at = bo` | UnmountComplete |
| `unmount_done(Err e)` | back to Mounted; `failures += 1`; `unmount_retry_at = bo`; `last_error = e` | MountDegraded(`e`): Busy reads "unmount blocked: busy (files open)" (C4), anything else "unmount failed: …" |
| `health(Healthy)` | H = Healthy; clear `degraded_since`; `last_error = None`; `failures = 0` **only if** `mounted_at.is_some_and(\|t\| now ≥ t + retry_max)` (A15) | MountHealthy (on change) |
| `health(Degraded r)` | H = Degraded; `degraded_since.get_or_insert(now)` | MountDegraded (on entry) |
| `health(Stale r)` | H = Stale; `failures += 1`; `mount_retry_at = bo` | MountDegraded("stale: …") |
| `health(Missing)` | Absent; `handle = None`; `mounted_at = None`; `failures += 1`; `mount_retry_at = bo`; `last_error = "mount disappeared"` | MountFailed |
| child exit (`on_exit`) | no state change; the actor probes that mount immediately (ignored while Unmounting) | — |

`failures` resets only on a Healthy inspect once the mount has been up for at least `retry_max` (`now ≥ mounted_at + retry_max`, A15), so a mount that dies right after mounting keeps backing off instead of looping. `POST …/mount` resets `failures`, both retry timers and `offline`.

**Availability** (PRD §12; derived, never stored):
- **Mount level:**
  - Mounting → Connecting.
  - Unmounting → Unmounting.
  - Mounted with H = Degraded or Stale → Degraded; otherwise Mounted.
  - Absent and desired: `offline` or `online == Some(false)` → Offline; candidate `driver` is Err → Failed, with the driver error as detail (B15); `last_error` set → Failed; otherwise Eligible.
  - Held → Eligible, with `held: true` in the DTO.
- **Machine level:** a verdict that isn't Allowed → Discovered. An Allowed machine with no mounts → Eligible (B15). Otherwise the maximum over its mounts, where the order is Failed > Degraded > Offline > Unmounting > Connecting > Mounted > Eligible.
- **Unknown** is used only for an adopted mount whose machine isn't in the registry yet.
- **The PRD chain in practice:** Mounted → Degraded (the probe fails) → Offline (grace elapsed: lazily detached, still desired, retrying).

**Backoff.** Uses `validate::backoff`, with `rand = random_u64()` passed in by the actor.

**Idempotency argument.**
1. `plan` is a pure function of (candidates, runtimes, now, ready, grace), and its output is sorted by id.
2. Every side-effect action calls `begin()` before its task is spawned, so a re-plan during execution hits row 1.
3. Each side-effect row changes the predicate that selected it: Absent becomes Mounted, a fingerprint mismatch becomes Absent and then a matching fingerprint, Stale becomes Absent.
4. Failures are rate-limited by `mount_retry_at` / `unmount_retry_at`.
5. So once the world converges, every id sits in a no-side-effect row (2, 4, 6, 7, 8, 13, 14). Only `now` crossing a deadline, which is an input change, can produce a new action.
6. Tests: `plan_twice_second_all_noop` and `plan_is_deterministic`.

**Triggers.**

| trigger | mechanism |
|---|---|
| startup | the first pass runs right after adoption; adopted mounts are inspected at t = 0 |
| discovery | one task per network provider: `loop { timeout(30s, discover()) → Msg; select!{ sleep(interval), notify.notified() } }`: discover **first**, so the first result doesn't wait an interval (A7); static observations are replaced on every config apply |
| health | ticker every `health_interval` → inspect every Mounted runtime with a handle and `!probing` |
| fallback | ticker every `reconcile_interval` → re-probe drivers, then run a pass |
| precise deadlines | the actor sleeps until the earliest of `next_wakeup()` and (while `!ready` and once a config file has loaded) `cfg_loaded_at + grace`. There is no expiry deadline (E3): a pass runs on every provider result and `expires_at ≥ 3 × interval`, so expiry is accurate to one interval |
| child exit | the driver's `on_exit` → `Msg::ChildExited{id, generation}` → immediate inspect |
| config change | poller / SIGHUP (`reload.rs`) / `POST /v1/config/reload` (`spawn_blocking(load)`) → `Msg::Config`; the actor applies it with the same config-apply path it uses at startup (A4) |
| API | `Msg::Api(cmd)` with a oneshot reply (`ApiCmd::Discover` has none, E4) |

**Warm-up.** `ready` is false until a config file has actually loaded (A6): with no config file, no healthy, un-held mount is unmounted (row 3 still clears a Stale or force-requested mount, and a held unmount goes through), and the grace period does not run. Once a config file has loaded, `ready` becomes true when every network provider has returned a **non-empty Ok** at least once (A18), or `offline_grace_period` has passed since the first successful config load (`cfg_loaded_at`: the daemon start when the file exists at startup, else the moment the first `Msg::Config` with Ok is applied, whether it comes from the poller, SIGHUP or the API). Measuring from the load, not the start, keeps a late-appearing config from being ready before any provider has reported. A static-only config is therefore ready immediately. An `Ok(vec![])` (for example a DNS NXDOMAIN on the index) and an `Err` never count. A provider whose `build_provider` failed never reports, so it waits for the grace period (B11). Once true, `ready` stays true. Before that, only rows 4/5 are blocked, and a held (manual) unmount goes through. Removal decisions that depend on discovery wait for discovery to have actually worked. A provider that is down at login delays removals by at most the grace period.

**Concurrency model.** One actor task owns all mutable state: `Arc<Config>`, the registry, runtimes, `held`, probe results, the previous verdicts, provider status and the event ring. There are no locks on domain state.

```rust
// bifrost-daemon/src/actor.rs — frozen in S0 (A3), together with ApiCmd (api.rs) and Deps (§8)
pub enum Msg {
    Discovery { provider: String, task_gen: u64, result: Result<Vec<MachineObservation>, DiscoveryError> },
    MountDone { id: MountId, generation: u64, result: Result<MountHandle, MountError> },
    UnmountDone { id: MountId, generation: u64, why: Reason, result: Result<(), MountError> },
    Health { id: MountId, generation: u64, state: MountState },
    ChildExited { id: MountId, generation: u64, detail: String },
    Probed(BTreeMap<String, DriverAvailability>),
    Config { result: Result<Box<Config>, Vec<ConfigError>>, reply: Option<oneshot::Sender<ReloadDto>> },
    Api(ApiCmd),
    Tick(Tick),
    Shutdown(oneshot::Sender<()>),              // abort provider tasks, wait ≤5s for executors, write state.json, reply
}
pub enum Tick { Health, Fallback }
```

- **Inbox:** `mpsc::unbounded_channel`. `on_exit` is a sync callback and must never block; producers are naturally rate-limited.
- **Loop:** `select!` over the inbox, `sleep_until(deadline)` and shutdown. It drains `try_recv`, then runs one pass. It publishes `watch::Sender<Arc<StatusDto>>` and pushes events to `broadcast::Sender<EventRecord>` (capacity 256) plus a ring of 200 kept in the snapshot.
- **Events on transitions (B1):** after each pass the actor compares verdicts and probe results with the previous ones and emits `MachineEligible` when a machine becomes Allowed, and `DriverUnavailable` when a driver goes Available → Unavailable.
- **The actor never awaits I/O and never touches a mount path.**
- **Executor tasks** run each driver call under a timeout: mount gets `mount_timeout + 60s` (A9; it covers the 15s preflight, `mount_timeout` and the 10s lazy unmount inside the driver), unmount gets 30s. They report through `Msg`. If the outer timeout drops a mount that was still in progress, the next `mount()` adopts or detaches our own marker entry (in the A10 fallback, our same-fstype entry) at step 2 (§6), so the path never stays Refused (A9).

**Hung-FUSE guards.**
1. Mount-table reads (`/proc/self/mountinfo`, `getmntinfo(MNT_NOWAIT)`) never touch FUSE.
2. The only filesystem calls on a live mountpoint run inside `check::timed` (§6). It is `spawn_blocking` under a 5s timeout. A per-path in-flight set means at most one leaked thread per hung mount; while a probe is stuck, the next one returns immediately.
3. `prepare_mountpoint` and `remove_dir` run only after the mount table says the path is **not** a mountpoint.
4. Every helper command (fusermount3, umount, diskutil, ssh preflight, tailscale, probes) runs under `tokio::time::timeout` with `kill_on_drop(true)`. Mount children are the exception: they use `kill_on_drop(false)`.
5. The root is canonicalized once, at startup.

---

## 6. Mount drivers (`bifrost-mount`)

```rust
pub struct DriverSettings { pub ssh_config: Option<PathBuf>, pub vfs_cache_mode: String, pub mount_timeout: Duration,
                            pub state_dir: PathBuf /* rclone --cache-dir=<state>/rclone/<id> (A23) */ }
pub struct SshfsDriver;  impl SshfsDriver  { pub fn new(s: DriverSettings) -> Self; }
pub struct RcloneDriver; impl RcloneDriver { pub fn new(s: DriverSettings, nfs: bool) -> Self; }   // "rclone" / "rclone-nfs"
/// Every OS gets all three; rclone-nfs probes Unavailable("macOS only") on Linux.
pub fn drivers(s: &DriverSettings) -> Vec<Arc<dyn MountDriver>>;

pub mod table {
    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct MountEntry { pub mount_point: PathBuf, pub fstype: String, pub source: String }
    pub fn read() -> std::io::Result<Vec<MountEntry>>;     // linux: /proc/self/mountinfo; macos: getmntinfo(MNT_NOWAIT) under a static Mutex
    pub fn parse_mountinfo(bytes: &[u8]) -> Vec<MountEntry>; // pure: fields after " - "; \NNN octal unescape (\040 \011 \012 \134); malformed lines skipped
    pub fn find<'a>(e: &'a [MountEntry], p: &Path) -> Option<&'a MountEntry>;
}
pub mod check {
    /// None = timed out, or already in flight for `key` (no new thread is spawned).
    pub async fn timed<T: Send + 'static>(key: &Path, dur: Duration,
        f: impl FnOnce() -> std::io::Result<T> + Send + 'static) -> Option<std::io::Result<T>>;
    /// symlink_metadata(path/".bifrost-probe-<16hex nonce>") under timed(5s):
    /// any server reply — Ok, NotFound, PermissionDenied — → Healthy (A17: an unsearchable remote root is not a fault);
    /// NotConnected (macOS also raw errno 6 ENXIO) → Stale; other → Degraded(e); None → Degraded("unresponsive").
    pub async fn liveness(path: &Path) -> MountState;
    /// Searches ONLY `path` (a PATH-style list); is_file ∧ mode & 0o111. The fixed fallback dirs live in `which()`.
    /// Tests pass their own `path` and never call `set_var` (unsafe in edition 2024, racy across test threads) (B14).
    pub fn which_in(name: &str, path: &OsStr) -> Option<PathBuf>;
    pub fn which(name: &str) -> Option<PathBuf>;           // which_in(name, $PATH + /usr/local/bin:/usr/bin:/bin [+ macos /opt/homebrew/bin])
    pub async fn run(bin: &Path, args: &[OsString], t: Duration) -> std::io::Result<std::process::Output>; // stdin null, kill_on_drop(true)
    pub async fn ssh_preflight(ssh: &Path, spec: &MountSpec, ssh_config: Option<&Path>) -> Result<(), MountError>;
}
pub const SSH_OPTS: [&str; 6] = ["BatchMode=yes", "ConnectTimeout=10", "ServerAliveInterval=15",
                                 "ServerAliveCountMax=3", "ControlMaster=no", "ControlPath=none"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Flavor { Linux, MacFuse, FuseT }
pub fn sshfs_argv(spec: &MountSpec, ssh_config: Option<&Path>, f: Flavor) -> Vec<OsString>;             // pure
pub fn rclone_argv(spec: &MountSpec, ssh: &Path, ssh_config: Option<&Path>, vfs: &str, cache_dir: &Path,
                   nfs: bool, f: Flavor) -> Vec<OsString>;  // pure; the S0 stub returns vec![] (B13)
pub fn preflight_argv(spec: &MountSpec, ssh_config: Option<&Path>) -> Vec<OsString>;                   // pure
pub async fn unmount_path(path: &Path, force: bool) -> Result<(), MountError>;
/// Pure. Only entries whose parent == root. Marker source ⇒ adopt (driver from fstype, else record, else "sshfs");
/// no marker ⇒ adopt only when state.json has a record with the same local_path (macOS NFS fallback); else foreign.
/// `pid` comes from the state record with the same local_path, else None (B15).
pub fn adopt(entries: &[table::MountEntry], root: &Path, records: &BTreeMap<MountId, MountHandle>) -> Vec<MountHandle>;
```

**Host keys are never weakened.** `SSH_OPTS` is the only set of ssh options Bifröst ever passes. It never contains StrictHostKeyChecking, UserKnownHostsFile, GlobalKnownHostsFile, ProxyCommand or IdentityFile, and no argv ever carries sshfs's `sftp_server` (it takes a remote command, B13). The user's ssh_config (or the `-F` file) decides trust, and `BatchMode=yes` turns "ask" into "fail". `ControlPath=none` ties the mount's lifetime to our process rather than to a user's multiplexing master. Test: `argv_never_weakens_host_keys`, a scan of every argv builder for those substrings plus `sftp_server`. S1 agent C writes it while `rclone_argv` is still the `vec![]` stub; S3 agent J extends it to real rclone argv (B13).

**SSH preflight**, run before every spawn for both drivers (PRD §8.1 "validate SSH connectivity"):

```
<ssh> -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3
      -o ControlMaster=no -o ControlPath=none [-F <ssh_config>] [-p <port>] [-l <user>] -s -- <host> sftp
stdin=/dev/null, stdout=/dev/null, stderr piped; timeout 15s
exit 0 ⇒ host key + auth + sftp subsystem OK;  else Err(Failed(tail(stderr, 512)))   (A8)
         e.g. "Host key verification failed." / "Permission denied (publickey)."
```

This matters because rclone swallows ssh's stderr. **Unverified here:** that ssh exits 0 when sftp-server sees EOF on stdin. S1 agent C verifies this against the docker sshd. If it doesn't hold, drop the preflight and rely on the log tail.

**Spawn** (shared by both drivers):
- `tokio::process::Command::new(<absolute binary from which()>).args(argv)`. Never `sh -c`.
- `stdin(null)`; stdout and stderr both go to `log_path` (`<state>/logs/<id>.log`, opened create+truncate+write with mode 0600, argv written first as one header line starting with `LOG_HEADER`, which `tail()` skips (A8)). **Never a pipe**: a full pipe can stall the FUSE server, and a pipe would SIGPIPE orphaned children after a daemon crash.
- `process_group(0)`, so a terminal Ctrl-C or the daemon's death never signals the child.
- `kill_on_drop(false)`.
- The environment is inherited, including `SSH_AUTH_SOCK`.
- After a successful mount, a supervisor task owns the `Child`: `tokio::spawn(async move { let s = child.wait().await; (req.on_exit)(describe(s)) })`. It reaps the child. There is no kill channel.

**`mount(req)`:**
1. Check the request:
   - `local_path` is absolute, and `local_path.parent()` canonicalizes to itself;
   - `file_name == id`;
   - the binary resolves; otherwise `Unavailable`.
2. `table::read()`. If an entry is at `local_path` (A9):
   - our marker (`parse_marker(source)` gives this `id`) with the same fingerprint → `Ok(MountHandle{pid: None})`: a mount left behind by a dropped or timed-out attempt is adopted, with no supervisor;
   - our marker with a different fingerprint, or (A10 fallback only) an entry whose fstype is this driver's own, whose fingerprint can't be read → lazy unmount (`unmount_path(local_path, true)`), then continue. The fallback entry is never adopted here: a leftover of an older spec would silently serve the wrong host;
   - anything else (foreign) → `Refused("occupied by <fstype> <source>")`.
3. `prepare_mountpoint`:
   - `symlink_metadata` NotFound → `create_dir` with mode 0700;
   - a symlink → Refused;
   - not a directory → Refused;
   - `read_dir` non-empty → `Refused("not empty")`. fuse3 would mount over it and hide the files.
4. `ssh_preflight`.
5. Spawn.
6. Every 100 ms until `mount_timeout`:
   - the mount table has `local_path` (on Linux also `source == marker(id, fp)`; if S1 finds that sshfs ignores a user `fsname=`, path + fstype instead, A10) → `Ok(MountHandle{pid: child.id()})` and start the supervisor;
   - the child has exited → `Err(Failed(status + tail(last 2 KiB of the log, 512)))` (A8);
   - the deadline passed → `start_kill()` and `wait()` (this is the only kill, and it hits our own child that never finished mounting), then lazily unmount **only if** the entry at the path carries our marker, or in the A10 fallback has this driver's fstype (A9), then `Err(Failed("timed out after 30s: <tail>"))`.
   - macOS permission hint (B7): when the log tail matches `kernel extension|System Extension|not permitted`, the error gets the suffix " (macOS: allow the macFUSE system extension in System Settings → Privacy & Security)". This lives in the shared spawn/readiness code in `mount/src/lib.rs` (S1-C), so rclone gets it too; `bifrost doctor` shows the same hint (S2-F) and the README documents it (S4-N).

**sshfs argv.** Options come first and the validated positionals last. Neither positional can start with `-`: the host grammar forbids it and the local path is absolute.

```
<sshfs> -f
  -o fsname=bifrost:<id>@<fp16>,reconnect,idmap=user,transform_symlinks
  -o BatchMode=yes,ConnectTimeout=10,ServerAliveInterval=15,ServerAliveCountMax=3,ControlMaster=no,ControlPath=none
  [Linux]   -o auto_unmount                      # a SIGKILLed sshfs is unmounted by the fusermount3 helper → Missing, not ENOTCONN
  [MacFuse] -o volname=<id>,noappledouble        [FuseT] -o volname=<id>
  [-o ro] [-p <port>] [-F <ssh_config>]
  [<user>@]<host|[v6]>:<sftp_path>  <local_path>
```

Example: `sshfs -f -o fsname=bifrost:static1@9f1c2e0a7b3d4c55,reconnect,idmap=user,transform_symlinks -o BatchMode=yes,ConnectTimeout=10,ServerAliveInterval=15,ServerAliveCountMax=3,ControlMaster=no,ControlPath=none -o auto_unmount -p 2222 -F /tmp/e2e/ssh_config bf@127.0.0.1:/home/bf /tmp/e2e/machines/static1`.

**Unverified here:** that a user `fsname=` overrides sshfs's own. `strings /usr/bin/sshfs` shows sshfs inserts `-osubtype=sshfs,fsname=%s` at argv[1], so a later user `-o fsname=` should win. S1 agent C verifies it, and E2E p04 asserts `bifrost:static1@` appears in mountinfo. If it doesn't hold (A10): readiness (mount step 6) and `inspect` fall back to path + fstype, and adoption falls back to state.json records, the same path macOS NFS uses. The same fstype test also marks an entry as ours in mount step 2 (lazy detach, never adopt) and in the step 6 timeout detach, so a dropped or timed-out attempt never leaves the path Refused (A9). `MountRequest` carries no state records, so the driver can't use them there. If S1-C implements this fallback, it adds `// ponytail: a same-fstype entry at <root>/<id> counts as ours and a good leftover costs one extra mount cycle; upgrade: pass state records in MountRequest` at the step 2 check.

**rclone argv.** Always `--flag=value` form.

```
<rclone> mount|nfsmount :sftp:<sftp_path> <local_path>
  --config=/dev/null                             # never read the user's rclone.conf
  --sftp-host=<host>                             # harmless with --sftp-ssh; avoids a possible "host not set"
  "--sftp-ssh=<ssh> -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3
               -o ControlMaster=no -o ControlPath=none [-F \"<ssh_config>\"] [-p <port>] [-l <user>] <host>"
  --sftp-shell-type=none --sftp-disable-hashcheck   # rclone never runs remote shell commands
  --devname=bifrost:<id>@<fp16>
  --cache-dir=<state>/rclone/<id>                # per mount: no VFS cache shared across hosts; pending writes resume (A23)
  --vfs-cache-mode=<mode>                        # nfsmount: at least "writes" (read-only otherwise)
  --dir-cache-time=15s --log-level=NOTICE
  [--read-only]   [macOS mount: --volname=<id>]
```

Notes:
- **Host-key verification** is kept because rclone's internal SSH library is **never** used; it skips host-key checks unless `known_hosts_file` is set. `--sftp-ssh` hands everything to OpenSSH, which uses the same known_hosts, agent, ProxyJump and Tailscale SSH setup as sshfs.
- **Tokenisation:** rclone splits the `--sftp-ssh` value on spaces and appends `-s sftp`. Every token is a constant or a validated Host, User or port, all free of whitespace and quotes. A runtime check returns `Refused` if one slips through. The ssh binary path and the config path are checked for whitespace and `"`.
- **No `--` here:** rclone appends `-s sftp`, and `--` would turn that into a remote command.
- rclone 1.75.1 logs a NOTICE "No host key validation is being performed". That refers to its unused internal library; `argv_never_weakens_host_keys` and the E2E negative test cover the real path.

**probe():**

| driver | Linux: Available iff | macOS: Available iff | detail |
|---|---|---|---|
| sshfs | `which sshfs` ∧ `sshfs --version` (5s) contains "SSHFS version" ∧ (`fusermount3` or `fusermount`) ∧ `/dev/fuse` exists ∧ `which ssh` | sshfs ∧ ssh ∧ (`/Library/Filesystems/macfuse.fs` → MacFuse, or `/Library/Application Support/fuse-t` or `/usr/local/lib/libfuse-t.dylib` → FuseT) | "SSHFS version 3.7.3, fusermount3" |
| rclone | `which rclone` ∧ `rclone version` parses ∧ `rclone help flags sftp` contains `--sftp-ssh` (feature detection) ∧ ssh ∧ FUSE as above | rclone checks ∧ macFUSE or FUSE-T | "v1.75.1, fuse3" |
| rclone-nfs | Unavailable("macOS only") | rclone checks ∧ `/sbin/mount_nfs` | "nfsmount" |

Each driver's `probe()` is a crate-internal `probe_with(path: &OsStr)` called with `$PATH`; tests call `probe_with(<temp dir>)` so `probe_fake_sshfs_in_path` never mutates the environment (B14).

Probes run at startup, on reload, on every fallback tick and on `POST /v1/reconcile`, so installing sshfs later heals itself. Because the spec fingerprint covers the selector text, `auto` mounts are sticky and a probe flap never remounts them.

**`inspect(h)`:**
1. `table::read()`: no entry at `h.local_path`, or an entry that doesn't carry our marker (Linux; path + fstype instead if sshfs ignores `fsname=`, A10) → `Missing`.
2. Otherwise `check::liveness(h.local_path)`.

Probing a unique nonexistent name forces a real round trip: the kernel has no negative dentry for it and sshfs has no cache entry, so a dead remote can't look healthy from cache. For rclone, a negative lookup can be answered from its directory cache for at most 15 seconds (`--dir-cache-time=15s`).

**`unmount(h, force)`**, via the shared `unmount_path`:

| | Linux | macOS | result |
|---|---|---|---|
| graceful | `fusermount3 -u <p>` (falls back to `fusermount`), 10s | `/sbin/umount <p>`, 10s | stderr contains "busy" / "Resource busy" → `Err(Busy)` (nothing killed) |
| force | `fusermount3 -u -z <p>`: **lazy detach, no kill**; open files keep working and the child exits when the last reference closes | `/usr/sbin/diskutil unmount force <p>`, falling back to `/sbin/umount -f <p>` | — |
| both | "not mounted", "not found", "entry for … not found", or the path no longer in `table::read()` → **Ok** (idempotent) | same | still listed → `Err(Failed("still mounted"))` |

After `Ok` with why = NotDesired or Manual, the executor task calls `std::fs::remove_dir(local)` in `spawn_blocking`, having checked first that the path is not a mountpoint. It removes only empty directories and ignores errors. Nothing under the root is ever deleted recursively.

**Force is used only when:**
- the mount is Stale: the process is dead, or ENOTCONN;
- the mount has been Degraded past the grace period: the remote is unreachable, and on Linux open handles survive the lazy detach;
- the user asked with `unmount --force`.

A live, busy mount that is merely not desired is **never** forced. It stays Degraded ("unmount blocked: busy (files open)", C4) and retries with backoff.

**Stale-mount recovery:**
- sshfs is killed → `auto_unmount` removes the mount → Missing → Absent + backoff → row 9.
- rclone is killed, or a dead adopted mount is found → ENOTCONN → Stale → row 10 (lazy detach) → row 9.
- Hung → Degraded → row 12 after grace.

**Adoption after a restart:**
- `adopt()` runs over the mount table and considers only entries **directly under this daemon's canonical root**.
- Adopted mounts get `MountRuntime::adopted(handle)` with the fingerprint taken from the marker and the pid from the state record with the same `local_path`, else None (B15). A fingerprint that differs from the candidate's goes through row 11 (graceful remount, or Degraded if busy).
- A non-marker entry at a desired path is foreign (in the A10 fallback, only one whose fstype isn't the driver's own; see mount step 2). It is never touched, and `mount()` returns Refused, visible as Failed.
- State records whose path is not mounted are dropped. **No pid is ever signalled**: a recorded process may be a lazily-detached sshfs still serving open files. Stuck processes end on their own through ConnectTimeout or ServerAlive.

---

## 7. Discovery providers (`bifrost-discovery`)

```rust
pub mod tailscale {
    pub struct TailscaleProvider;
    impl TailscaleProvider { pub fn new(name: String, binary: Option<PathBuf> /* tests only */) -> Self; }
    pub fn parse_status(json: &[u8]) -> Result<Vec<MachineObservation>, DiscoveryError>;   // pure
}
pub mod dns {
    pub struct DnsProvider;
    impl DnsProvider { pub fn new(name: String, domain: Host, nameservers: Vec<SocketAddr>) -> Result<Self, String>; }
    #[derive(Clone, Debug, PartialEq, Eq, Default)]
    pub struct Bf1 { pub nodes: Vec<String>, pub host: Option<Host>, pub port: Option<u16>, pub user: Option<User>,
                     pub tags: Vec<String>, pub path: Option<RemotePath>, pub id: Option<String> }   // no driver (E2)
    /// ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$ (no dots). Lives here, not in core (B8).
    pub fn dns_label(s: &str) -> Result<String, Invalid>;
    pub fn parse_bf1(txt: &str) -> Result<Option<Bf1>, String>;   // Ok(None) = not a bf1 record (ignore silently)
    pub fn node_observation(label: &str, domain: &Host, r: &Bf1, ttl: Duration) -> Result<MachineObservation, String>; // pure
}
pub mod http {
    pub struct HttpProvider;
    impl HttpProvider { pub fn new(name: String, url: String, headers: Vec<(String, String)>) -> Result<Self, String>; }
    pub fn parse_inventory(body: &[u8]) -> Result<Vec<MachineObservation>, DiscoveryError>;  // pure
}
```

**Rules for every provider:**
- Every untrusted field goes through a `core::validate` function (or `dns::dns_label`) before it can reach an observation.
- A bad record is skipped with `warn!(provider, record, reason = clean(value, 512))`, so records are isolated from each other.
- Output is sorted by id, and a duplicate id is skipped with a warning (first wins).
- `Err` is returned only when the source as a whole couldn't be read. The registry then freezes that provider's observations.
- The daemon wraps each call in `timeout(30s)`.
- Removal hysteresis: `expires_at = now + max(ttl, 3 × interval)`, applied in `apply_ok`.

**Tailscale.**

Invocation and errors:
- `tailscale status --json`, with a 10s timeout and stdout capped at 16 MiB.
- Binary lookup: the constructor's `binary`, else `"tailscale"` via PATH, else on macOS `/Applications/Tailscale.app/Contents/MacOS/Tailscale`. Not found anywhere → `Unavailable`.
- Non-zero exit → `Failed(tail(stderr, 512))`. `BackendState != "Running"` → `Unavailable("backend state X")`.

Parsed JSON:
- Top level: `{BackendState, CurrentTailnet?{MagicDNSEnabled}, Peer: Option<BTreeMap<String, Peer>>}`. `Self` is ignored, so we never mount ourselves.
- `Peer{ID, HostName, DNSName, OS (default ""), TailscaleIPs: Option<Vec<IpAddr>>, Tags: Option<Vec<String>>, Online}`.
- Peers are processed sorted by DNSName.

| observation field | source | if invalid |
|---|---|---|
| id | `Name::parse(first label of DNSName)`, else `Name::parse(HostName)` (lowercases first; HostName can repeat) | peer skipped + warning |
| name | `clean(HostName, 128)` | — |
| native_id | `native_id(ID)` | dropped |
| addresses | `[DNSName without the trailing dot]` if MagicDNS is enabled and DNSName is set, else first IPv4; then every IP | peer skipped if none |
| online / tags | `Some(Online)` / `Tags` with `tag:` stripped, each passed through `tag()` | a bad tag is dropped |
| metadata.values | `os`, `hostname` (cleaned), `dns_name` (no `tailscale_id`: it duplicates `native_id`, E5) | — |

New peers appear on the next refresh without a restart.

**DNS TXT `bf1`** (hickory 0.26.3).

Resolver setup:
```rust
let b = if nameservers.is_empty() {
    TokioResolver::builder_tokio().map_err(|e| e.to_string())?      // C8: NetError → String
} else {
    let ns = nameservers.iter().map(|sa| {
        let mut n = NameServerConfig::udp_and_tcp(sa.ip());
        for c in &mut n.connections { c.port = sa.port(); }
        n
    }).collect();
    Resolver::builder_with_config(ResolverConfig::from_name_servers(ns), TokioRuntimeProvider::default())
};
// no options_mut() lines (D1): ResolverOpts::default() is already 5s timeout × 2 attempts
let resolver = b.build().map_err(|e| e.to_string())?;   // the cache stays on and honours TTL for lookups
```

All queries are absolute names ending in `.`, so resolv.conf search domains never apply.

```
S       := concat(all character-strings of one TXT RR)      ; UTF-8 ASCII, ≤ 2048 bytes
record  := token *(1*SP token)
token   := key "=" value                                     ; split at the first '='
key     := 1*32 [a-z0-9_-]      value := 1*256 (%x21-7E except '"')
first token != "v=bf1"  → Ok(None): not ours (SPF, future v=bf2) — ignored silently
missing '=', empty value, over-long key or value, duplicate key → Err (record invalid)
unknown keys → ignored (including `driver=`, E2).  Known: v nodes host port user tags path id
```

| key | validation | goes to |
|---|---|---|
| nodes | comma list of `dns_label` (this module, B8; no dots, so an index can't make us query another domain); ≤256 in total across all index RRs; deduplicated | index |
| host | `Host::parse` (rejects `-oProxyCommand=…`); default `<node>.<domain>` | `addresses[0]` |
| port | 1..=65535, ≤5 digits | port |
| user / path | `User::parse` / `RemotePath::parse` | hints |
| tags | ≤32 entries, each `tag()` | metadata.tags |
| id | `native_id()`, so `../../etc` is rejected | native_id only; **identity is always the node label** |

Algorithm:
1. Look up `_bifrost.<domain>.`.
   - `is_no_records_found()`, or no valid bf1 index RR → `Ok(vec![])` plus a warning. With the expiry registry this causes no churn; entries age out after `3 × interval`. An empty Ok does not count toward warm-up `ready` (A18).
   - Any other error → `Err(Failed)`, which freezes the provider.
2. Nodes = the union of `nodes=` over the valid index RRs. Inline index records (`host=` at the root) are not supported (§15).
3. Look up every `_bifrost.<node>.<domain>.` concurrently with a `JoinSet`. For each node:
   - a lookup error → skip the node; its old observation is not refreshed and ages out;
   - zero bf1 RRs → skip;
   - more than one **distinct** valid bf1 RR → skip as "ambiguous";
   - **any** key failing validation → skip the whole node. Attacker data is never partially applied.
4. `ttl = min(index.valid_until(), node.valid_until()).saturating_duration_since(now)`. Record changes show up when the TTL expires (hickory cache); removals take `max(ttl, 3 × interval)`.

**HTTP.**
- Client: `reqwest::Client::builder().timeout(10s).redirect(Policy::none()).user_agent("bifrost/<ver>")`. No redirects means auth headers can't leak.
- `GET url` with the configured headers plus `Accept: application/json`.
- A non-2xx status → `Failed("HTTP 401")`. The body is read with `chunk()` and capped at **1 MiB** (more → `Failed`).

```json
{ "machines": [ {
    "name": "agent-01",                              // required, else "id"; → Name::parse (lowercased)
    "id": "i-0abc",                                  // optional → native_id
    "host": "agent-01.corp",                         // optional connect target; when present, addresses = [host] (A20)
    "addresses": ["10.20.0.4"],                      // used only without "host": each Host::parse; invalid ones dropped; ≥1 needed
    "port": 22, "online": true,
    "user": "sami", "path": "/home/sami",            // hints (validated; used only with honor_hints); "driver" is ignored (E2)
    "metadata": { "tags": ["dev", "agent"], "env": "dev", "rack": 4 }   // tags → tags; scalars → clean(v, 256) strings; nested ignored
} ] }
```

- The body is parsed as `struct Top { machines: Vec<serde_json::Value> }`. At most the first 1000 entries are used; the rest get one warning.
- Each entry is converted separately, and a failing entry is skipped with `warn!("machines[{i}]: …")`.
- When `host` is present, `addresses = [host]` and the `addresses` array is ignored (A20). CIDR rules then see exactly the connect target: an in-range IP in `addresses` can't carry a different `host` past `include_cidrs`.
- Refresh happens every `interval`. On failure, the last good view is served (frozen). On success, entries that are gone age out after `3 × interval`. That is the whole cache policy.

---

## 8. Daemon (`bifrostd`)

**Invocation.** `bifrostd [--version | --help]`. Any other argument prints usage and exits 2. Everything else comes from the environment: `BIFROST_CONFIG`, `BIFROST_SOCKET`, `BIFROST_STATE_DIR`, and `BIFROST_LOG` (error|warn|info|debug|trace, default info; C8). **Exit codes:** 0 clean shutdown; 1 startup failure (already running, directories, socket); 2 usage or invalid config.

**Directories (A21).** The daemon creates missing directories with `DirBuilder::new().recursive(true).mode(0o700)` and chmods to 0700 (`set_permissions`) **only the components it created**; a pre-existing directory is never chmodded (so `BIFROST_SOCKET=$HOME/b.sock` never touches `$HOME`). Ownership check: a pre-existing state or socket directory must have the same owner uid (`MetadataExt::uid`) as the lock file the daemon has just created; otherwise exit 1. Test: `preexisting_parent_dirs_not_chmodded`.

**Startup:**
1. `tracing_subscriber::fmt().with_max_level(level)`; ANSI only when stderr is a TTY.
2. `state_dir` and `<state>/logs` (B11): created as in **Directories** above (0700 when created).
3. `File::try_lock(<state>/bifrostd.lock)`. `WouldBlock` → "bifrostd already running", exit 1. The lock is released automatically on a crash. Then the ownership check on `state_dir`.
4. Config: if the file is missing → `parse("")` (the empty default, root `~/machines`) plus a warning, and the poller picks up the file later, but only if its `mount.root` equals that default root; a file with any other root is rejected with "mount.root change requires restart" (A22) and needs a restart. `ready` stays false until a config file has actually loaded, so no healthy, un-held adopted mount is unmounted meanwhile (row 3, a Stale or force-requested mount, still applies; A6, test `missing_config_never_unmounts_adopted`). If `load` returns Err → print the sorted errors and exit 2. **An invalid config never starts a daemon that would unmount everything.**
5. `root`: created as in **Directories** (0700 when created), then `canonicalize` once. On macOS that resolves `/private` paths.
6. Read `state.json`. If it doesn't parse, rename it to `state.json.corrupt-<unix>` and start empty. `held` is authoritative (user intent); mount records are only hints.
7. `adopt(table::read(), root, records)`. Adopted runtimes are inspected at t = 0.
8. Drivers are built inside the actor with `(deps.drivers)(&settings)` (A5) and probed concurrently as soon as it starts.
9. Socket:
   - create the parent directory as in **Directories** (0700 only when created, A21), and run the ownership check on it when it already existed;
   - reject paths longer than 103 bytes with a clear message;
   - the lock guarantees any existing socket file is stale, so remove it;
   - `tokio::net::UnixListener::bind`, then `set_permissions(0o600)` on the socket (always).
10. Spawn the actor, which applies the startup config through the same config-apply path as a reload (A4) and so spawns the provider tasks; then the tickers, `reload::spawn_poller(config_path, tx)` (which also installs the SIGHUP handler, A4) and the signal task (SIGTERM/SIGINT → shutdown). Run `axum::serve(listener, router(state)).with_graceful_shutdown(signal)`, raced against `shutdown + 2s` so open SSE streams can't block exit.

**Wiring**, frozen in S0 and never edited by later stages (A3: `Msg`/`Tick` (§5), `Deps` and `spawn` in `actor.rs`; `ApiCmd` and `AppState` in `api.rs`):

```rust
// main.rs
fn build_provider(pc: &ProviderConfig) -> Result<Arc<dyn DiscoveryProvider>, String> {
    Ok(match &pc.spec {
        ProviderSpec::Tailscale => Arc::new(TailscaleProvider::new(pc.name.clone(), None)),
        ProviderSpec::Dns { domain, nameservers } =>
            Arc::new(DnsProvider::new(pc.name.clone(), domain.clone(), nameservers.clone())?),
        ProviderSpec::Http { url, headers } =>
            Arc::new(HttpProvider::new(pc.name.clone(), url.clone(),
                                       headers.iter().map(|(k, v)| (k.clone(), v.0.clone())).collect())?),
    })
}
let deps = Deps { drivers: Arc::new(|s: &DriverSettings| bifrost_mount::drivers(s)), build_provider: Arc::new(build_provider) };

// actor.rs
pub struct Deps {
    /// Factory, not a Vec (A5): a reload that changes driver settings rebuilds through it, so daemon tests keep their FakeDrivers.
    pub drivers: Arc<dyn Fn(&DriverSettings) -> Vec<Arc<dyn MountDriver>> + Send + Sync>,
    pub build_provider: Arc<dyn Fn(&ProviderConfig) -> Result<Arc<dyn DiscoveryProvider>, String> + Send + Sync>,
}
/// `cfg_loaded`: false ⇒ `cfg` is the empty default because no config file exists yet; `ready` stays false until a
/// `Msg::Config` with Ok is applied (A6). The grace window runs from `cfg_loaded_at`, the first successful config
/// load (spawn time when `cfg_loaded`, else when that first `Msg::Config` applies), not from spawn. `socket` fills
/// `StatusDto.socket` (B11).
#[allow(clippy::too_many_arguments)]
pub fn spawn(cfg: Config, cfg_loaded: bool, root: PathBuf, state_dir: PathBuf, socket: PathBuf,
             held: BTreeSet<MountId>, adopted: Vec<MountHandle>, deps: Deps)
             -> (mpsc::UnboundedSender<Msg>, watch::Receiver<Arc<StatusDto>>, broadcast::Sender<EventRecord>, JoinHandle<()>);

// api.rs
pub struct ApiError { pub status: u16, pub error: String }
pub enum ApiCmd {
    Mount     { target: String, reply: oneshot::Sender<Result<Vec<String>, ApiError>> },
    Unmount   { target: String, force: bool, reply: oneshot::Sender<Result<Vec<String>, ApiError>> },
    Reconcile { reply: oneshot::Sender<Vec<ActionDto>> },
    Discover,                                  // no reply: the route returns 202 at once (E4)
}                                              // no Reload: the route sends Msg::Config itself (A4)
#[derive(Clone)]
pub struct AppState { pub snapshot: watch::Receiver<Arc<StatusDto>>, pub tx: mpsc::UnboundedSender<Msg>,
                      pub events: broadcast::Sender<EventRecord>, pub state_dir: PathBuf, pub config_path: PathBuf }
pub fn router(s: AppState) -> axum::Router;
```

**Routes** (axum 0.8 `{param}` syntax). Errors are `ErrorDto`. Every POST carries a JSON body: the client sends `{}` where there is nothing to say, and always sends `UnmountReq{force}` for unmount (C6). An API `target` that is both a mount id and another machine's id means the **mount** (B15).

| method + path | request | response |
|---|---|---|
| GET `/v1/status` | — | `StatusDto` (full snapshot) |
| GET `/v1/machines` | — | `Vec<MachineDto>` (there is no `/v1/machines/{id}`: `machines show` filters this list, E1) |
| GET `/v1/mounts` | — | `Vec<MountDto>` |
| GET `/v1/mounts/{id}/log` | — | `LogDto`: last 64 KiB of `<state>/logs/<id>.log`, `clean(line, 512)` per line; `id` goes through `Name::parse` so traversal is impossible |
| GET `/v1/drivers` | — | `Vec<DriverDto>` |
| POST `/v1/discover` | `{}` | 202 `{}`: sends `ApiCmd::Discover` (no reply, E4); the actor notifies every provider task |
| POST `/v1/reconcile` | `{}` | 200 `Vec<ActionDto>`: re-probe, run a pass, return the plan. It does **not** clear `mount_retry_at`, so a second call has no side effects (A11); `POST …/mount` is the "retry now" |
| POST `/v1/mounts/{target}/mount` | `{}` | 202 `Vec<String>` (ids). `target` is a mount id, or a machine id meaning all its mounts. Clears the hold and both retry timers. 404 unknown / **403** not a candidate (body carries the verdict) |
| POST `/v1/mounts/{target}/unmount` | `UnmountReq` | 202 `Vec<String>`: adds a persisted hold; `force` sets `force_requested`. 404 unknown |
| POST `/v1/config/reload` | `{}` | 200 `ReloadDto` (`ok: false` when the config is invalid): `spawn_blocking(load(config_path))`, then `Msg::Config{result, reply: Some(tx)}` and await the reply (A4) |
| GET `/v1/events` | — | SSE |

Mount and unmount return immediately; clients wait by polling `/v1/mounts`. GET status routes only read the `watch` channel; they never wait on the actor (the `/log` route reads a file) (C5).

Example `MountDto`:

```json
{"id":"agent-01","machine":"agent-01","driver":"sshfs","local_path":"/home/sami/machines/agent-01",
 "remote":"sami@agent-01.tail1234.ts.net:/home/sami","state":"mounted","detail":"","desired":true,"held":false,
 "adopted":false,"pid":4242,"failures":0,"retry_in_secs":null,"last_error":null,"action":"noop"}
```

**SSE.** `Sse::new(futures_util::stream::unfold(broadcast_rx, …)).keep_alive(KeepAlive::new().interval(15s))`. A lagged receiver gets `event: lagged` with `data: {"skipped":N}` and the stream continues. Frames look like:

```
id: 42
event: MountHealthy
data: {"seq":42,"ts_unix_ms":1790000000000,"event":{"type":"MountHealthy","mount":"agent-01"}}

```

**Config hot reload.** Split by file ownership (A4): `reload.rs` (S4 agent M) only produces `Msg::Config`; `actor.rs` (S2 agent E) owns applying it.

`reload.rs` is `pub fn spawn_poller(path: PathBuf, tx: mpsc::UnboundedSender<Msg>)`. The file is polled with std only on a plain `std::thread` (B10), so a config on an sshfs/NFS home can't hang a tokio worker; notify is not used.
1. Every 2s, `std::fs::read(path)` (this follows symlinks). The poller's baseline is its first read (the file at startup, or none).
2. Continue only if the bytes differ from the bytes it last sent (or the baseline) **and** are identical on two consecutive polls, which debounces half-written saves.
3. `config::parse` → `Msg::Config{result, reply: None}`.
4. A given set of bad bytes is reported once. A file that is deleted or unreadable at runtime keeps the active config and warns once (B10).
5. SIGHUP skips the debounce: `spawn_poller`, which is called inside the runtime, also installs a tokio SIGHUP listener that runs `spawn_blocking(load(path))` → `Msg::Config{reply: None}`. `POST /v1/config/reload` skips it too (see Routes).

This handles rename-on-save editors, dotfile-manager symlink swaps and NFS or sshfs home directories.

In the actor (`actor.rs`), one config-apply path serves startup and every `Msg::Config` (A4). The whole swap happens in one step:
- **Invalid config:** keep the old `Arc<Config>`, set `status.config_errors`, emit `ConfigurationReloaded{ok: false, errors}`.
- **`mount.root` changed:** rejected like an invalid config with the error "mount.root change requires restart"; the old config is kept (A22; the actor does no I/O, so it can't create or canonicalize a new root). The comparison is the new expanded `mount.root` against the active config's. This also covers the missing-config start: the default root `~/machines` is active, so a first file is applied only if its root equals it; otherwise `config_errors` shows the error and `ready` stays false until a restart. Test `reload_root_change_rejected`.
- **Valid config:**
  - `registry.replace(static_source(), new.static_observations())`;
  - diff providers by name: removed → abort the task and `remove_provider`; added → spawn; changed (`ProviderConfig !=`) → abort and respawn with `task_gen + 1` (observations are kept until they expire); unchanged → keep;
  - a `build_provider` Err sets that provider's `ProviderDto.last_error`, spawns no task, and leaves warm-up to the grace period (B11);
  - rebuild `policy` and templates;
  - rebuild drivers with `(deps.drivers)(&settings)` (A5) if `ssh_config`, `vfs_cache_mode` or `mount_timeout` changed, then re-probe;
  - swap the config, mark it loaded (A6), emit `ConfigurationReloaded{ok: true}`, clear `config_errors`, run a pass.
  - **At startup** the same path runs once on the initial config, with two differences: it marks the config loaded only when `cfg_loaded` is true (so an empty default from a missing file never starts the grace clock), and it emits no `ConfigurationReloaded`.
- Any other changed spec changes the fingerprint, which leads to a graceful remount through row 11.

**state.json.** Written tmp → `sync_all` → rename, with mode 0600, whenever `held` or the mount handles change.

```json
{ "version": 1, "held": ["build-artifacts"],
  "mounts": { "agent-01": { "id": "agent-01", "driver": "sshfs", "local_path": "/home/sami/machines/agent-01",
                            "fingerprint": "9f1c2e0a7b3d4c55", "pid": 12345 } } }
```

**Tracing spans:**
- executor tasks: `info_span!("mount_op", machine_id, mount_id, provider, driver, local_path, remote_path, attempt)`;
- provider tasks: `info_span!("discovery", provider, kind)`;
- health: `debug_span!("health", mount_id)`.

Every emitted `Event` is also logged at info with the same fields. Header values are never logged; `Secret` prints `***` in Debug.

**Graceful shutdown (SIGTERM/SIGINT) does not unmount.**
1. main stops the API (graceful shutdown of `axum::serve`, raced against 2s).
2. main sends `Msg::Shutdown(reply)`. The actor aborts its provider tasks, waits up to 5s for in-flight executor tasks, writes state.json and replies (main waits ≤7s for the reply).
3. main removes the socket and exits 0.

The actor owns the provider and executor tasks (A4), so shutdown goes through the inbox; `Msg::Shutdown` is part of the frozen `Msg` (A3).

The children run in their own process groups, so they keep serving and are adopted on the next start. A restart and a crash therefore take the same, tested path. Teardown is `bifrost unmount <target>`.

Under systemd, use `KillMode=process` to get adoption; the default `control-group` sends SIGTERM to the children and they unmount themselves. Do not set `ProtectHome` or `PrivateMounts`, because they would place the mounts in another namespace (README).

---

## 9. CLI (`bifrost`) and `bifrost-client`

```rust
// bifrost-client/src/lib.rs
pub struct Client { socket: PathBuf }
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("bifrostd is not running (socket {0})")] NotRunning(PathBuf),   // ENOENT / ECONNREFUSED on connect
    #[error("{status}: {error}")] Api { status: u16, error: String },      // non-2xx; body is ErrorDto
    #[error(transparent)] Io(#[from] std::io::Error),
    #[error("bad response: {0}")] Decode(String),
}
impl Client {
    pub fn new(socket: PathBuf) -> Self;
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError>;
    pub async fn post<T: DeserializeOwned>(&self, path: &str, body: &impl Serialize) -> Result<T, ClientError>;
}
// Per request: UnixStream::connect → hyper::client::conn::http1::handshake(TokioIo::new(s)) → tokio::spawn(conn)
// → send_request(Request{Host: bifrost, Content-Type: application/json, Full<hyper::body::Bytes>})
// → BodyExt::collect → serde_json::from_slice.
// Every POST sends a JSON body (Content-Type is always application/json, so `&()` → `null` would be rejected):
// the CLI always sends UnmountReq{force} for unmount and `{}` for the body-less POSTs (C6).
```

CLI global flags: `--json` (pretty-prints the DTO), `--socket PATH` (env `BIFROST_SOCKET`), `--config PATH` (env `BIFROST_CONFIG`). Tables are hand-aligned (column width is the widest cell), and every daemon string is passed through `clean(s, 512)` again.

| command | daemon | route | human output |
|---|---|---|---|
| `status` | yes | GET status | summary block (below) |
| `machines`, `machines list` | yes | GET machines | `NAME SOURCE ADDRESS STATE MOUNTED` (PRD §18) |
| `machines show <id>` | yes | GET machines, filtered by id (E1) | key/value lines: verdict, address:port, online, tags, metadata, shadowed, mounts |
| `mounts` | yes | GET mounts | `ID MACHINE DRIVER STATE LOCAL REMOTE` (+ `ERROR` when any row has one: `last_error`, else `detail`, B15) |
| `mount <target> [--no-wait]` | yes | POST …/mount, then poll GET mounts every 250 ms for up to 60 s | `agent-01  mounted  /home/sami/machines/agent-01 (sshfs, pid 4242)` or `agent-01  failed: <last_error, else detail>` (exit 1; a driver-selection failure has only `detail`, B15) |
| `unmount <target> [--force] [--no-wait]` | yes | POST …/unmount, then poll | `agent-01  unmounted (held; 'bifrost mount agent-01' to resume)` or `agent-01  unmount blocked: busy (files open); retry with --force` (exit 1, C4) |
| `discover` | yes | POST discover, then poll status until every provider's `refreshes` increases (≤ 30 s) | `PROVIDER KIND STATUS MACHINES LAST-OK` |
| `reconcile` | yes | POST reconcile | `MOUNT ACTION` |
| `drivers` | yes; local probe if the daemon is down | GET status (`drivers`, `auto_driver`) | `✓ sshfs  /usr/bin/sshfs  SSHFS version 3.7.3, fusermount3` … then `default (auto): sshfs` from `StatusDto.auto_driver` (`none` when it is None). With the daemon down: the first available driver of the local probe in `auto_order` from the CLI's own config load (`default_auto_order()` when there is no file). `DriverDto.auto_rank` is cut (E5). `--json` prints `StatusDto.drivers` (`Vec<DriverDto>`), the same shape as `GET /v1/drivers` |
| `doctor` | no (uses the daemon if it is up) | local + GET status | PRD §24 layout (below) |
| `config check [PATH]` | **no** | local `load` | `ok: <path> (3 machines, 2 providers, 4 mounts, root /home/sami/machines)`, or sorted `error: <path>: <msg>` lines, exit 1 |
| `config reload` | yes | POST config/reload | `reloaded`, or the error lines, exit 1 |
| `daemon status` | yes | GET status | `running pid 4242 up 3h12m socket …`, or `not running (socket …)` exit 3 |

```
$ bifrost status
bifrostd 0.1.0  pid 4242  up 3h12m  /run/user/1000/bifrost/bifrost.sock
config    /home/sami/.config/bifrost/config.toml (ok)
root      /home/sami/machines
machines  7 (3 eligible)   mounts 3/3 mounted, 0 degraded, 0 failed
providers static ok (2) · tailscale ok (5, 12s ago) · infra error: timed out (last ok 2m ago)
drivers   sshfs ✓ (default) · rclone ✓ · rclone-nfs ✗ macOS only

$ bifrost doctor
Config         ✓ /home/sami/.config/bifrost/config.toml
Daemon         ✓ running pid 4242
SSH            ✓ ssh /usr/bin/ssh   ✓ SSH_AUTH_SOCK visible to daemon
Discovery
  ✓ static      2 machines
  ✓ tailscale   5 machines
  ✗ infra       timed out
Mount Drivers
  ✓ sshfs       /usr/bin/sshfs (SSHFS version 3.7.3)
  ✓ rclone      /usr/bin/rclone (v1.75.1)
  ✗ rclone-nfs  macOS only
FUSE           ✓ /dev/fuse  /usr/bin/fusermount3
Selected default
  sshfs
```

The `(default)` mark in `status` and doctor's "Selected default" both come from `StatusDto.auto_driver`, the daemon's own `select_driver(&Auto, …)` over its active config (E5). When the daemon is down, doctor uses the same local derivation as `drivers`.

When a mount's `last_error` carries the macOS permission hint (§6 mount step 6, B7), `doctor` repeats it under Mount Drivers (S2-F).

**Exit codes:** 0 ok · 1 operation failed (API error, mount ended Failed or Offline, unmount busy, invalid config, doctor found a ✗ in Config or Drivers) · 2 usage (clap) · 3 daemon not reachable.

---

## 10. TUI (`bifrost-tui`)

**Architecture.**
- `App` is pure data: `status: Option<StatusDto>`, `view`, `selected`, `filter`, `popup`, `status_line`, `unreachable`, `no_color`, `log: Option<LogDto>`.
- `App::on_key(KeyEvent) -> Option<Command>`, where `Command = Mount(String) | Unmount(String, bool) | Reconcile | Discover | Reload | FetchLog(String) | Quit`.
- `ui::render(&App, &mut Frame)`.

**Runtime.**
- `main` is synchronous and owns a tokio multi-thread runtime with one worker.
- The UI loop polls inline, with no poller task and no channel (E6): once per second `rt.block_on(timeout(500ms, client.get("/v1/status")))`.
- The Logs view fetches `GET /v1/mounts/{id}/log` the same way every 2s while it is visible.
- Commands run the same way (`rt.block_on(timeout(…))`; the routes return at once), and their results become `status_line`.
- UI loop: `ratatui::init()`, then `ratatui::crossterm::event::poll(100ms)` (C1: crossterm is not a direct dependency), the due polls, `draw`, and `ratatui::restore()` on exit or panic.
- If the daemon is unreachable, a red banner reads "bifrostd not reachable at <socket> — retrying" and the last snapshot stays on screen.
- The TUI talks only to the daemon API.

**Views** (`1`–`7` or Tab / Shift-Tab):
1. Overview: the wordmark "Bifröst", the tagline "Remote worlds. Local files.", counts, provider health, config errors, the last 5 events.
2. Machines: `● agent-01  tailscale  mounted  sshfs`.
3. Mounts.
4. Discovery.
5. Drivers: one row per `DriverDto`, with the default marked from `StatusDto.auto_driver` (E5).
6. Events.
7. Logs.

**Keys:**

| key | action |
|---|---|
| ↑/↓, j/k | select |
| m | mount the selection (a machine means all its mounts) |
| u | unmount |
| U | force unmount (asks y/n) |
| r | reconcile |
| s | discover now |
| c | reload config |
| d / Enter | details popup (verdict, observations, `last_error`, else `detail` (B15), next retry) |
| l | jump to Logs for the selection |
| / | filter (substring; Enter applies, Esc clears) |
| ? | help |
| q, Ctrl-C | quit |

**Palette** (`Color::Rgb`):

| role | color |
|---|---|
| background | Nordic Night `#0B1220` |
| primary text | Frost Glass `#E5F0FF` |
| active tab, selected row (text on it is Nordic Night), ● mounted, wordmark | Aurora Teal `#2DD4BF` |
| focus border, headers, ◌ connecting/unmounting, key hints | Glacial Blue `#60A5FA` |
| borders, secondary text, timestamps, tagline, ○ discovered/eligible/offline | Stone `#94A3B8` |
| semantic additions, since the brand has no warning or error colour | ◐ degraded amber `#FBBF24`; ✕ failed rose `#F87171` |

`NO_COLOR` set disables every style; the glyphs still carry the state.

**Tests without a terminal:** `Terminal::new(TestBackend::new(100, 30))`, render a fixture `StatusDto`, and assert on buffer symbols and `fg` colours. Keys are tested as pure `on_key` calls.

---

## 11. Security checklist (PRD §23)

| # | principle | enforced by |
|---|---|---|
| 1 | Discovery ≠ trust | `policy::evaluate`: non-static observations default to DiscoverOnly. `reconcile::desired` produces candidates only for Allowed machines. The API returns 403 for non-candidates. Winner-takes-all by trust means DNS/HTTP can't redirect or deny a machine that a more trusted source reports. A `native_id` is matched only by its own provider's `include_ids`/`exclude_ids`, never by global `ids` (A19). |
| 2 | No SSH passwords | No password keys exist (`deny_unknown_fields`). `BatchMode=yes` in `SSH_OPTS`, used by sshfs, the preflight and `--sftp-ssh`. Child stdin is `/dev/null`. |
| 3 | Prefer agent / keys / Tailscale SSH | The system `ssh` is always used (sshfs, preflight, rclone `--sftp-ssh`). `SSH_AUTH_SOCK` is inherited. `-F` only when configured. `doctor` and `StatusDto.ssh_agent` report the agent. |
| 4 | Never weaken host verification | The constant `SSH_OPTS` has no host-key options. No config knob exists. rclone's internal SFTP library is never used. Test `argv_never_weakens_host_keys`. E2E `psec_hostkey.sh` checks an unknown key fails with "Host key verification failed". |
| 5 | Mount only explicit paths | Remote paths come from static config or the provider template. Hints are used only with `honor_hints`, and only after `RemotePath::parse` (no `..`, `:` or control characters). There is no driver hint (E2). |
| 6 | Never execute discovery-supplied commands | Discovery data can only become `Name`, `Host`, `User`, `RemotePath` or `u16`. argv is built from constants plus those, via `Command::args`, never `sh -c`. Positionals can't start with `-`. `--flag=value` for rclone. `--sftp-ssh` tokens are checked for whitespace and quotes. `--sftp-shell-type=none` and `--sftp-disable-hashcheck`. The dangerous sshfs options (`ssh_command`, `directport`, `passive`) are never emitted. |
| 7 | TXT/HTTP are untrusted | `parse_bf1`: 2 KiB, `v=bf1` first, a duplicate key invalidates the record, labels have no dots, ≤256 nodes, whole-node rejection, identity pinned to the label. HTTP: 1 MiB, 1000 entries, per-entry isolation, no redirects, credentials only over https or loopback, no CR/LF in headers. `clean(s, max)` on every displayed string: 128 for names, 256 for metadata values, 512 for errors and log lines (no terminal-escape injection, A8). E2E hostile records plus a canary file. |
| 8 | Local path traversal / collisions | `Name::parse` gives a single lowercase component. `local_path = root.join(id)` only in `desired`. Config enforces unique locals; at runtime static wins and the conflict is reported. `prepare_mountpoint` refuses symlinks, non-directories, non-empty directories and occupied paths. The root is canonicalized once and can't be `/` or `$HOME`. |
| + | Local API | The socket is always 0600. Directories the daemon creates are 0700; pre-existing directories are never chmodded (A21), and pre-existing state and socket directories must have the daemon's owner uid (ownership check, §8 Directories; the mount root is not checked). A lock file prevents a second daemon. The default socket path is never under `/tmp` (C5). |

---

## 12. Test plan

**Unit tests.** Every name is a `#[test]` or `#[tokio::test]`. No network, no root.

- **core::validate** (S0):
  - `name_rejects_traversal` (`""`, `.`, `..`, `a/b`, `../x`, `.x`, `-x`, `a b`, `a\0`, 64 chars), `name_lowercases`;
  - `host_rejects_option_injection` (`-oProxyCommand=x`, `a b`, `a,b`, `a"b`, `u@h`, `h:22`, `h;rm`, `fe80::1%eth0`, 254 chars, unspecified `0.0.0.0`/`::`/`0.0.0.0.`);
  - `host_accepts_ipv4_ipv6_fqdn_trailing_dot`, `host_for_colon_brackets_v6`;
  - `user_rejects_dash_at_space`;
  - `remote_path_rules` (`~`, `~/x`, `/`, `/a b` ok; `a/b`, `/a/../b`, `/a\nb`, `/a:b`, `""` rejected), `sftp_path_mapping` (including `sftp_path("/") == "/"`, B2);
  - `tag_meta_label_native_id_grammars` (tag, meta_key, native_id; `dns_label` now lives in discovery, B8), `clean_strips_escapes` (with `max`; bidi/zero-width → `?`), `tail_skips_header` (keeps the last lines within `max`, skips `LOG_HEADER` lines, A8);
  - `duration_units_rejects_zero_and_junk` (`500ms`, `30s`, `5m`, `1h` ok; `5`, `5 m`, `-1s`, `1d`, `8785h` (> 366d) rejected);
  - `glob_star_question`, `cidr_v4_v6_no_cross_family` (v4-mapped v6 rejected), `fnv64_known_vectors`;
  - `backoff_bounds` (failures 0..=64 × rand ∈ {0, u64::MAX}: within [base/2, base] and ≤ max; 0 behaves like 1, C3), `backoff_no_overflow`.
- **core::model** (S0, A2): `fingerprint_changes_on_every_field`, `fingerprint_stable_vector` (a hard-coded expected hex), `marker_roundtrip_exact` (`agent-01` vs `agent-01-home`), `source_format`, `driver_selector_serde_rejects_bad_grammar` (grammar only, B8), `serde_newtypes_validate_on_deserialize`.
- **core::policy:** `deny_wins_over_allow`, `global_deny_beats_static`, `provider_exclude_drops_only_that_observation`, `no_allow_rule_is_discover_only`, `static_allowed_by_default`, `include_kinds_anded_values_ored`, `include_metadata_all_pairs_deny_metadata_any`, `cidr_deny_fails_closed_without_ip`, `cidr_deny_static_hostname_not_matched`, `cidr_allow_requires_ip`, `providers_matches_name_or_kind`, **`global_ids_match_machine_id_only`**, **`native_id_scoped_to_owning_provider`** (A19: a DNS/HTTP record publishing `id=<a tailscale node ID>` doesn't satisfy a global `allow ids=[…]`).
- **core::registry:**
  - trust and merge: `trust_order_static_tailscale_http_dns` (Sources with trust 0–3, B8), `dns_cannot_redirect_tailscale_machine`, `dns_tags_cannot_deny_static_machine`, `dup_within_provider_first_wins`;
  - expiry and freezing: `absent_ages_out_not_removed`, `failed_provider_freezes_expiry`, `recovery_expires_long_absent`, `ttl_floor_three_intervals`;
  - bookkeeping: `replace_is_authoritative`, `remove_provider_returns_gone` (no `next_expiry`, E3).
- **core::reconcile:**
  - **`prd_fake_discovery_fake_driver`**: `FakeDiscovery` returns A + B, the policy has `include_names=["*"]` for `fake`, and `FakeDriver` has A mounted with a matching fingerprint and B absent. `plan` gives `[(a, NoOp), (b, Mount)]`.
  - **`plan_twice_second_all_noop`**: execute plan₁ through `FakeDriver` (`block_on`), set runtimes from `inspect`, then plan₂ is all NoOp. Also `plan_is_deterministic`.
  - One test per table row: `row01_inflight_waits`, `row02_absent_not_desired_noop`, `row03_stale_not_desired_force`, `row04_warmup_blocks_removal`, `row04_held_bypasses_warmup`, `row05_not_desired_graceful`, `row06_no_driver_waits`, `row07_offline_waits`, `row08_backoff_waits`, `row09_mount`, `row10_stale_remount_force`, `row11_spec_change_remount_graceful`, `row11_degraded_spec_change_lazy`, `row11_gated_on_driver_and_online` (A16: with the gate closed, a Healthy mount gives row 13 `Degraded("change pending: …")`, and one Degraded past grace still gives row 12), `row12_grace_elapsed_lazy_unmount`, `row13_degraded_within_grace`, `row14_offline_but_healthy_noop`, and `gate_u_unmount_backoff`.
  - Rows 3 and 5 use `why = Manual` for a held candidate, else `NotDesired` (A14); `row03_*`/`row05_*` assert both.
  - Also: `stale_cleanup_not_gated_by_mount_backoff`, `missing_goes_absent_with_backoff`, `stale_generation_ignored_for_health_and_exit` (A1), `failures_reset_only_after_stable_healthy` (A15), `grace_unmount_sets_offline`, `auto_driver_sticky_fingerprint`, `select_driver_auto_order_and_named_unavailable`, `hints_ignored_unless_honor_hints`, `static_local_collision_conflict`, `next_wakeup_is_earliest_deadline`, `availability_tables` (mount and machine; a desired mount with a driver error is Failed with detail, a machine with no mounts is Eligible, B15), `offline_chain_mounted_degraded_offline`.
- **config:**
  - examples parse: `parses_full_example` (the §3 TOML verbatim), `parses_prd_s6_s7_s13_s31_snippets` (§6.1 has `remote = "/"`, B2), `shorthand_equals_mounts_form`, `empty_text_is_default_config`;
  - determinism and schema errors: `errors_sorted_deterministic` (byte-identical output twice), `unknown_field_has_line_col`, `per_type_keys_enforced`;
  - machine and mount rules: `remote_xor_mounts`, `local_traversal_rejected`, `duplicate_local_rejected`, `duplicate_machine_rejected`, `uppercase_name_hint`;
  - value rules: `unknown_driver_rejected`, `bad_duration_rejected`, `retry_initial_gt_max_rejected`, `root_slash_home_relative_rejected`;
  - expansion: `tilde_and_vars_expanded`, `undefined_var_error`, `dollar_dollar_literal`;
  - discovery and policy: `http_plaintext_non_loopback_rejected`, `header_crlf_rejected`, `discovery_names_default_unique_static_reserved`, `policy_unknown_provider_rejected`, `ssh_config_with_quote_rejected`.
- **mount:**
  - mount table: `mountinfo_octal_escapes`, `mountinfo_optional_fields`, `mountinfo_malformed_skipped`;
  - argv golden tests: `sshfs_argv_linux_golden`, `sshfs_argv_macfuse_golden`, `sshfs_argv_fuset_golden`, `sshfs_argv_ipv6_home_port_ro_cfg`, `rclone_argv_mount_golden`, `rclone_argv_nfsmount_forces_writes`, `rclone_sftp_ssh_tokens_clean_cfg_quoted`, `preflight_argv_golden`;
  - argv safety: **`argv_never_weakens_host_keys`** (forbidden list includes `sftp_server`; written by S1-C against the `vec![]` rclone stub, extended by S3-J, B13), `positionals_never_start_with_dash`, `rclone_never_uses_internal_ssh`;
  - fs and probes: `adopt_marker_record_foreign_outside_root` (pid from the state record with the same `local_path`, else None, B15), `mount_step2_own_marker_adopt_or_detach_foreign_refused` (A9), `prepare_mountpoint_creates_rejects_symlink_file_nonempty`, `which_respects_exec_bit`, `timed_guard_single_thread` (the closure sleeps; a second call returns None immediately), `probe_fake_sshfs_in_path` (temp dir with fake `sshfs` printing "SSHFS version 3.7.3" plus fake `ssh` and `fusermount3`, found via `which_in`/`probe_with`, which search only the given dir; tests never call `set_var`, B14), `probe_missing_binary_unavailable`, `rclone_nfs_unavailable_on_linux`;
  - `#[ignore]` docker tests (needs `BIFROST_E2E_SSH=host:port:user:ssh_config` from `tests/e2e/lib.sh`): `sshfs_mount_inspect_unmount`, `sshfs_kill9_auto_unmount_missing`, `rclone_mount_write_roundtrip`, `rclone_kill9_stale_then_lazy`, `preflight_exit0_and_hostkey_failure`. The kill tests signal the child by its `MountHandle.pid`, never `pkill -f fsname=`, which would also kill the fusermount3 auto_unmount helper (A13).
- **discovery:**
  - tailscale: `tailscale_fixture_parse` (Self excluded, `tag:` stripped, `Tags` absent, trailing dot trimmed, `Peer: null`, MagicDNS off → IP, duplicate HostName resolved via DNSName, a capitalised HostName fallback kept), `tailscale_backend_stopped_unavailable`, `tailscale_new_peer_appears` (a fake `tailscale` script swaps its fixture between two `discover()` calls), `#[ignore] tailscale_live_status`;
  - bf1 parser: `bf1_v_first_required`, `bf1_other_txt_ignored`, `bf1_unknown_keys_ignored`, `bf1_duplicate_key_invalid`, `bf1_host_injection_rejected`, `bf1_bad_id_rejects_node`, `bf1_nodes_no_dots_capped`, `bf1_char_strings_concatenated`, `bf1_size_limit`;
  - DNS assembly: `node_default_host`, `node_id_pinned_to_label`, `ambiguous_node_skipped`, `ttl_min_of_index_and_node`, `#[ignore] coredns_discovery`;
  - HTTP: `inventory_prd_example`, `invalid_entries_isolated`, `name_falls_back_to_id`, `metadata_tags_merged_scalars_flattened`, and against a 15-line tokio TCP responder on 127.0.0.1:0: `body_cap_enforced`, `auth_header_sent_non2xx_failed`, `redirect_not_followed`.
- **client + API** (S1-D): `uds_roundtrip` (an axum dev-dependency server on a temp socket), `not_running_enoent_and_econnrefused`, `api_error_maps_status_and_body`; in `daemon/src/api.rs`: `sse_frame_format`, `log_route_rejects_traversal`.
- **daemon** (S2-E; in-process: `Deps` whose `drivers` closure returns `FakeDriver`s and whose `build_provider` returns `FakeDiscovery`, A5; temp state directory and socket):
  - startup and mounting: `mounts_desired_on_startup_and_idempotent`, `warmup_protects_adopted_until_ok`, **`missing_config_never_unmounts_adopted`** (A6: a healthy, un-held adopted mount stays while no config file exists; the grace clock starts only when a file loads);
  - child exits (via `FakeDriver::exit`, B12): `child_exit_triggers_probe_and_remount`, `exit_during_unmount_ignored`;
  - API: `api_mount_forbidden_for_discover_only`, `api_unmount_holds_across_restart`, `api_unmount_busy_stays_degraded` (via `FakeDriver.busy`, B12), **`reconcile_endpoint_twice_second_all_noop`** (A11);
  - socket, directories and lock: `socket_0600_stale_replaced`, **`preexisting_parent_dirs_not_chmodded`** (A21), `second_instance_lock_refused`;
  - state file: `state_json_atomic_corrupt_quarantined`;
  - reload: `reload_invalid_keeps_old`, `reload_root_change_rejected` (A22; also a first file with a non-default root after a missing-config start), `reload_changed_provider_respawns_keeps_observations`, `stale_task_gen_dropped`;
  - events: `events_eligible_and_driver_unavailable_on_transition` (B1).
- **cli:** `machines_table_golden`, `mounts_table_golden`, `status_block_golden`, `exit3_when_daemon_absent` (runs the binary against a bogus socket), `config_check_output_deterministic`.
- **tui:** `renders_machines_with_glyphs_and_teal`, `key_m_emits_mount_for_selection`, `U_asks_confirmation`, `filter_narrows_rows`, `unreachable_banner`, `no_color_disables_styles`.

**E2E harness.** `tests/e2e/run.sh [m1|all]`, using bash, docker, jq, python3, curl, dig (p08), pgrep from procps (p13a), fusermount3 (cleanup), ssh-keygen and ssh-keyscan (C7).

- Each phase file `pNN_*.sh` defines up to three functions: `setup_<p>` (fixtures), `config_<p>` (prints a TOML fragment) and `check_<p>` (assertions).
- `run.sh` sources `lib.sh` and the phase files **listed explicitly** for the chosen mode (no glob, so collation can't reorder `p13_hardening` and `p13a_adopt`; C7), then calls every `setup_*`, concatenates the `config_*` output after `config.tmpl.toml`, starts the daemon, and calls every `check_*` in list order.
- **S2-G writes both complete lists into `run.sh`**: m1 is `p04_sshfs p05_api p06_recovery psec_hostkey p13a_adopt`; `all` appends `p07_tailscale p08_dns p09_rclone p10_http p12_reload p13_hardening`. A listed phase file that doesn't exist yet is skipped with a `skip: <file> (not present)` line, so S3's `run.sh all` runs p04–p10 before S4 adds p12/p13. **Adding a phase edits only its own file**; no S3/S4 agent touches `run.sh` (owned by S2-G, C7). `run.sh` carries the bash comment `# ponytail: a missing listed phase file is skipped, not an error; upgrade: make a missing file fatal once S4 lands (the final gate requires no skip lines)`.
- Only `p08` defines `[policy.*]` (B5); a second `[policy.deny]` fragment would collide in the concatenated TOML.

```bash
set -euo pipefail; T=$(mktemp -d); export E2E=$T BIFROST_SOCKET=$T/bf.sock BIFROST_STATE_DIR=$T/state \
  BIFROST_CONFIG=$T/config.toml BIFROST_LOG=debug BF_TOKEN=s3cret
trap cleanup EXIT        # fusermount3 -uz $T/machines/*; kill bifrostd; docker rm -f bf-e2e-sshd bf-e2e-dns; kill inventory
cargo build --workspace; PATH=${CARGO_TARGET_DIR:-$PWD/target}/debug:$PATH   # shared target dir (P2)
ssh-keygen -t ed25519 -N '' -q -f $T/id
docker build -t bifrost-e2e-sshd tests/e2e/sshd    # alpine:3.20 + openssh-server + openssh-sftp-server; ssh-keygen -A at build
      # (host keys survive stop/start); adduser -D bf; echo 'bf:*' | chpasswd -e; /home/bf/hello.txt; /home/bf/data/w.txt;
      # CMD writes $PUBKEY to /home/bf/.ssh/authorized_keys (0600, bf) then exec /usr/sbin/sshd -D -e
docker run -d --name bf-e2e-sshd -p 127.0.0.1:2222:22 -p 127.0.0.2:2222:22 -e PUBKEY="$(cat $T/id.pub)" bifrost-e2e-sshd
wait_until 20 ssh-keyscan -p 2222 127.0.0.1; ssh-keyscan -p 2222 127.0.0.1 127.0.0.2 > $T/known_hosts
cat > $T/ssh_config <<EOF
Host *
  IdentityFile $T/id
  IdentitiesOnly yes
  UserKnownHostsFile $T/known_hosts
  GlobalKnownHostsFile /dev/null
  StrictHostKeyChecking yes
  CheckHostIP no
EOF
# config.tmpl.toml: root=$T/machines, ssh_config=$T/ssh_config; discovery 3s, health 2s, reconcile 5s,
# mount_timeout 15s, grace 20s, retry 1s..4s. static1 comes from p04's fragment (B4).
bifrostd > $T/d.log 2>&1 & DPID=$!; wait_until 10 bifrost daemon status
```

`lib.sh` provides `wait_until SECS CMD…` (polls every 0.5s), `mstate ID` (from `bifrost --json mounts | jq`), `mpid ID` (the only way tests pick a process to kill, A13), `is_mounted PATH` (grep in `/proc/self/mountinfo`), `start_daemon` and `stop_daemon`.

| file | set | acceptance checks |
|---|---|---|
| `p04_sshfs.sh` | m1 | Fragment: `static1` in the PRD §31 shorthand shape (B4): `[[machines]] name="static1" host="127.0.0.1" port=2222 user="bf" remote="/home/bf"`, mounted under `$T/machines`; the real `~/machines` is never touched. `static1` mounted within 20s. `test -f $T/machines/static1/hello.txt`. mountinfo shows `fuse.sshfs` with source `bifrost:static1@`. `bifrost unmount static1` → not in mountinfo and the directory is removed. `bifrost reconcile` → still unmounted (held). `bifrost mount static1` → mounted again. |
| `p05_api.sh` | m1 | `bifrost status/machines/mounts/drivers` exit 0 and `--json` parses. `stat -c %a $BIFROST_SOCKET` is 600. A second `bifrostd` exits non-zero with "already running". `BIFROST_SOCKET=$T/nope bifrost status` exits 3. SSE: `timeout 8 curl -sN --unix-socket $BIFROST_SOCKET http://bifrost/v1/events` shows an `event: UnmountStarted` frame during an unmount/mount of static1. |
| `p06_recovery.sh` | m1 | `kill -TERM $(mpid static1)` → mounted within 20s with a new pid. `kill -KILL $(mpid static1)` → mounted within 20s, `ls` works, no ENOTCONN. Never `pkill -f fsname=`: it would also kill the fusermount3 auto_unmount helper (A13). `bifrost --json reconcile` twice → the second is `noop` for static1, with the jq filtered to `.mount == "static1"` (psec's `unknown-key` fails permanently, A11). `docker stop` → `degraded` within 30s and `kill -0 $DPID`. `docker start` → `mounted` within 40s and `hello.txt` readable. `docker stop` again and wait → `offline` and not in mountinfo within 45s (grace 20s, lazy detach). `docker start` → `mounted` within 40s. |
| `psec_hostkey.sh` | m1 | Static `unknown-key` (host `localhost`, which is not in known_hosts) → state `failed`, `last_error` contains "Host key verification failed", never in mountinfo. The rclone half (`unknown-key-rc`) runs in p09, since rclone is still a stub at M1 (A12). |
| `p13a_adopt.sh` | m1 | `kill -9 $DPID` → mount still works. Restart → `adopted:true`, same pid, no `MountStarted` for static1 in `status --json .events`, and `pgrep -xc sshfs` unchanged (no duplicate process, B6). SIGTERM → restart → the same. |
| `p07_tailscale.sh` | all, opt-in `E2E_TAILSCALE=1` | Provider with **no filter**. `machines --json` has ≥1 machine with source tailscale, and every one is `discovered`. **Zero** tailscale mounts. Ids equal the distinct first labels of DNSName (`jq`). Never runs `tailscale up/down` and never mounts real peers. |
| `p08_dns.sh` | all | coredns/coredns:1.11.3 on 127.0.0.1:5353 udp+tcp; Corefile `test.bifrost:53 { file /zones/db { reload 1s } }`; `$TTL 5`. Index `nodes=agent-dns,other-01,bad-node,evil`: agent-dns `host=127.0.0.1 port=2222 user=bf tags=dev path=/home/bf/data`; other-01 `host=127.0.0.1 port=2222 tags=misc`; bad-node `host=-oProxyCommand=touch${IFS}$T/pwned`; evil `host=127.0.0.1 id=../../etc`. Provider: `honor_hints=true`, `include_tags=["dev"]`. Fragment also sets `[policy.deny] tags=["misc"]` (the only `[policy.*]` fragment, B5). Checks: `dig @127.0.0.1 -p 5353 TXT _bifrost.test.bifrost` answers. agent-dns mounted with `w.txt` visible. other-01's verdict is `denied (policy.deny tags=misc)`, never mounted. bad-node and evil absent, warnings in `$T/d.log`, `! test -e $T/pwned`. Remove the tag and bump the serial → unmounted within about 30s. |
| `p09_rclone.sh` | all | `static2-rc` (`driver="rclone"`): mountinfo `fuse.rclone` with source `bifrost:static2-rc@`; write and read round-trip. `bifrost drivers` shows sshfs ✓ and rclone ✓. `kill -KILL $(mpid static2-rc)` → Stale → recovered within 20s (A13). rclone host-key negative (moved from psec, A12): static `unknown-key-rc` (`driver="rclone"`, host `localhost`, not in known_hosts) → `failed`, `last_error` contains "Host key verification failed", never in mountinfo. |
| `p10_http.sh` | all | `inventory.py 127.0.0.1:18080 $T/inv.json` (401 unless `Authorization: Bearer s3cret`; re-reads the file per request). Provider `include_names=["inv-*"]`. inv.json: `inv-01` → 127.0.0.1:2222, plus entries with address `-oProxyCommand=x` and name `../x`. Checks: inv-01 mounted; invalid entries absent and warned. Rewrite the token to a wrong value → provider `last_error` "HTTP 401" and inv-01 **stays mounted** (served from cache). Restore → ok. |
| `p12_reload.sh` | all | Append static `box2` → mounted within 10s and `ConfigurationReloaded{ok:true}`. Write broken TOML → `status --json .config_errors` non-empty and every mount untouched. Restore → errors cleared. Swap `auto_order` to `["rclone","sshfs"]` → existing `auto` mounts are **not** remounted (sticky); a newly added auto machine mounts via rclone. |
| `p13_hardening.sh` | all | IP change: agent-dns `host=127.0.0.2` plus a serial bump → remounted with a new fingerprint within 30s. Duplicate discovery: `inv-01` also published via DNS → one machine, source `inventory`, shadowed `dns`. Rename: a node label changes → the old one unmounts after about 9–15s and the new one mounts. |

---

## 13. Staged execution plan

This section mirrors the approved plan's S0–S4 file ownership exactly; the B7 and E5 tags follow the Refinements list after the Amendments table (B7 moves off S3-J, which owns no shared spawn or doctor code). Every agent edits only the files it owns. Every cargo command runs with the shared `CARGO_TARGET_DIR` (P2).

**Safe-stub rule (S0):**
- Unimplemented providers return `Err(DiscoveryError::Unavailable("not implemented"))`.
- Unimplemented drivers return `Unavailable("not implemented")` from `probe` and `Err(MountError::Unavailable(..))` from `mount`.
- `rclone_argv` returns `vec![]`, so S1's `argv_never_weakens_host_keys` can scan it (B13).
- `reload::spawn_poller` is a no-op (so SIGHUP keeps its default action until S4 agent M installs the handler). There is no `reload::apply` (A4).
- `todo!()` is allowed only where M1 cannot reach.
- The daemon wiring (`build_provider`, the `Deps` factories) and the frozen types `Msg`, `Tick`, `ApiCmd`, `Deps` (A3) are final from S0 on, so no later stage edits them.

| stage | PRD phases | agents → files owned | implements / tests first | gate |
|---|---|---|---|---|
| **S0** (serial) | 0 | **S0.1 spec:** `git init`, initial commit of the PRD and `brand/`; `docs/design/{contract.md,critique.md}` with every amendment applied. **S0.2 verifier:** every amendment ID checked against `contract.md`; zero misses. **S0.3 skeleton:** `.gitignore` (`target/`), `Cargo.toml`, `crates/*/Cargo.toml` per §1 (with P1: no `rustfmt.toml`; P2), `cargo generate-lockfile` (needs network), every public signature of §2, §3, §5 (`Msg`/`Tick`), §6, §7, §8 (`ApiCmd`, `Deps`, `AppState`) and §9 as amended, as safe stubs. **S0.4** `core/validate.rs` fully implemented. **S0.5** `core/model.rs` helpers (A2). **S0.6** `core/fake.rs` complete (with B12); `bifrost-daemon/src/main.rs` wiring final. **S0.7** `scripts/check.sh`, the release profile, `rustup target add aarch64-apple-darwin` | S0.4: the §12 core::validate list (incl. `remote_path_rules` with `"/"`, `tail_skips_header`, `backoff_bounds` from 0). S0.5: the §12 core::model list | `cargo build --workspace --all-targets`; `scripts/check.sh` green; `cargo check --target aarch64-apple-darwin -p bifrost-core -p bifrost-config -p bifrost-mount -p bifrost-client -p bifrost-cli -p bifrost-tui` green; commit `chore: workspace skeleton + frozen contract`; `Cargo.toml` and `Cargo.lock` frozen |
| **S1** (4 parallel) | 1, 2, 4 | **A** core: `core/src/{model,policy,registry,reconcile,events,api}.rs` bodies. **B** config: `bifrost-config/src/*`; `config check` in `bifrost-cli/src/main.rs`. **C** mount + sshfs: `bifrost-mount/src/{lib,table,check,sshfs}.rs`; `tests/e2e/{lib.sh,sshd/Dockerfile}`. **D** client + API: `bifrost-client/src/lib.rs`; `bifrost-daemon/src/api.rs` | **A:** §2, §4, §5 with A6/A14–A16/A18/A19/B8/B15/C2/C4; policy suite incl. `global_ids_match_machine_id_only`, `native_id_scoped_to_owning_provider`; registry suite; `prd_fake_discovery_fake_driver`, `plan_twice_second_all_noop`, `plan_is_deterministic`, `row01…row14`, `gate_u_unmount_backoff`, `failures_reset_only_after_stable_healthy`, `row11_gated_on_driver_and_online`, `availability_tables`, `offline_chain_mounted_degraded_offline`. **B:** §3 with B2/B8 (trust ranks, `DRIVER_NAMES`, `default_auto_order`); `parses_full_example`, `parses_prd_s6_s7_s13_s31_snippets`, `shorthand_equals_mounts_form`, `empty_text_is_default_config`, `errors_sorted_deterministic`, `unknown_field_has_line_col` and the rest of the §12 config list. **C:** §6 (Linux + macOS sshfs argv, getmntinfo) with A9/A10/A13/A17/B7 (the `last_error` hint in the shared lib.rs spawn code)/B14/B15; `mountinfo_*`, `sshfs_argv_{linux,macfuse,fuset}_golden`, `argv_never_weakens_host_keys`, `positionals_never_start_with_dash`, `adopt_marker_record_foreign_outside_root`, `prepare_mountpoint_*`, `timed_guard_single_thread`, `probe_fake_sshfs_in_path` (via `which_in`), `mount_step2_own_marker_adopt_or_detach_foreign_refused`; `#[ignore]` docker `sshfs_mount_inspect_unmount`, `sshfs_kill9_auto_unmount_missing`, `preflight_exit0_and_hostkey_failure`; **records** whether the preflight exits 0 and whether the `fsname` override works. **D:** §8 routes (minus E1), §9 client, C6; `uds_roundtrip`, `not_running_enoent_and_econnrefused`, `api_error_maps_status_and_body`, `sse_frame_format`, `log_route_rejects_traversal` (router over a fixture snapshot with a stub actor task) | `scripts/check.sh` green; `cargo test -p bifrost-mount -- --ignored` mounts, reads, kills and unmounts against the docker sshd; `bifrost config check` byte-identical twice on the §3/§13/§31 examples; commit |
| **S2 = M1** (3 parallel) | 3, 5, 6 | **E** daemon: `bifrost-daemon/src/{actor,state,main}.rs`. **F** CLI: `bifrost-cli/src/{main,output,doctor}.rs`. **G** E2E m1: `tests/e2e/{run.sh,config.tmpl.toml,p04_sshfs.sh,p05_api.sh,p06_recovery.sh,psec_hostkey.sh,p13a_adopt.sh}` | **E:** §5 actor, executor, health, provider runner, warm-up; §8 startup, socket, lock, state.json, adoption, signals, shutdown; config apply + provider diffing (A4); A6, A7, A9 outer timeout, A11, A21, A22, B1, B11; the §12 daemon list. **F:** §9, every PRD §18 command, exit codes 0/1/2/3, `--json`, doctor per PRD §24 (with the B7 hint and `StatusDto.auto_driver`, E5), C6; `machines_table_golden`, `mounts_table_golden`, `status_block_golden`, `exit3_when_daemon_absent`, `config_check_output_deterministic`. **G:** the §12 harness with B4, B6, A11, A12, A13, C7 (both complete `run.sh` lists; missing phase files skipped) (the harness is the test) | **M1 gate:** `tests/e2e/run.sh m1` green: P4 mount, `ls`, unmount, held, remount (the §31 shorthand shape under `$T`, B4); P5 API, socket mode 600, second instance refused, exit 3, SSE frame; P6 `kill -TERM` and `kill -KILL` recovery, idempotent reconcile, degraded → restored, offline → restored; the host-key negative; kill -9 adoption with no duplicate process. `scripts/check.sh` green; commit `feat: milestone 1` |
| **S3** (5 parallel) | 7, 8, 9, 10, 11 | **H** `discovery/src/tailscale.rs`, `tests/fixtures/tailscale_status.json`, `tests/e2e/p07_tailscale.sh`. **I** `discovery/src/dns.rs` (+ `dns_label`), `tests/e2e/dns/*`, `p08_dns.sh`. **J** `mount/src/rclone.rs`, `p09_rclone.sh`. **K** `discovery/src/http.rs`, `tests/e2e/inventory.py`, `p10_http.sh`. **L** `bifrost-tui/src/{main,app,ui}.rs`. The daemon is not touched | **H:** §7 Tailscale (E5: no `tailscale_id`); `tailscale_fixture_parse`, `tailscale_backend_stopped_unavailable`, `tailscale_new_peer_appears`, `#[ignore] tailscale_live_status`; p07 opt-in, discovery only, zero mounts of real peers. **I:** §7 bf1 with B5, C8, D1, E2; `bf1_*`, `node_default_host`, `node_id_pinned_to_label`, `ambiguous_node_skipped`, `ttl_min_of_index_and_node`, `#[ignore] coredns_discovery`; p08 with hostile records, canary file and global deny. **J:** §6 rclone (Linux mount; macOS mount/nfsmount argv; probes) with A12, A23, B13 (B7 comes through the shared spawn code, S1-C); `rclone_argv_mount_golden`, `rclone_argv_nfsmount_forces_writes`, `rclone_sftp_ssh_tokens_clean_cfg_quoted`, `rclone_never_uses_internal_ssh`, `rclone_nfs_unavailable_on_linux`, extended `argv_never_weakens_host_keys`; `#[ignore] rclone_mount_write_roundtrip`, `rclone_kill9_stale_then_lazy`; p09 + the rclone host-key negative. **K:** §7 HTTP with A20, E2; `inventory_prd_example`, `invalid_entries_isolated`, `name_falls_back_to_id`, `metadata_tags_merged_scalars_flattened`, `body_cap_enforced`, `auth_header_sent_non2xx_failed`, `redirect_not_followed`; p10. **L:** §10 with C1, E6; brand palette; wordmark + tagline "Remote worlds. Local files."; the §12 tui list | `tests/e2e/run.sh all` passes p04–p10; `E2E_TAILSCALE=1 tests/e2e/run.sh all` passes p07 against the live tailnet, discovery only; the darwin check stays green; a manual TUI session against the E2E daemon covers mount, unmount, force, reconcile, discover, reload, details, logs, filter; commit |
| **S4** (2 parallel + final review) | 12, 13 | **M** `bifrost-daemon/src/reload.rs`, `p12_reload.sh`. **N** `p13_hardening.sh`, `README.md`, the macOS cfg audit, the `ponytail:` comment audit | **M:** `spawn_poller` on a `std::thread`: 2s poll, a two-read debounce, SIGHUP; B10. **N:** p13 checks IP change, duplicate discovery and rename. README covers the hero `brand/05_hero/hero_aurora_bridge_16x9.png` and logo; quickstart with the §31 config; the policy model; systemd `KillMode=process`, `SSH_AUTH_SOCK` import, `ssh-keyscan` hint; macOS permissions (B7). **Final whole-repo review**, looping until 2 consecutive dry rounds or at most 3 rounds: five finder lenses (PRD §35 DoD conformance, §23 security, reconciler/concurrency correctness, data loss and error handling, ponytail over-engineering), a 3-vote adversarial verify (a finding survives with ≥2 confirmations), a fixer, then `check.sh` and `run.sh all` | `tests/e2e/run.sh all` green **twice in a row**; `scripts/check.sh` green; the darwin check green; every §15 item has its `ponytail:` comment; commit `feat: bifrost v1` |

Per-task pipeline inside S1–S4: implementer in a worktree (TDD: the named tests first) → two reviewers (contract/amendment conformance + correctness; trust boundary + ponytail) → skeptic → fixer; then one merge agent merges branch by branch, running `scripts/check.sh` after each.

---

## 14. Top risks and how the design pre-empts them

| # | risk | pre-emption |
|---|---|---|
| 1 | A hung FUSE mount freezes the daemon | The actor does no I/O. Mount-table reads never touch FUSE. There is exactly one timed probe per path, with an in-flight guard. Every command has a timeout. After the grace period, a lazy detach frees the path without killing anything. |
| 2 | Data loss on unmount or remount | Graceful by default. Busy leads to backoff and Degraded, never automatic force. Force means lazy detach with **no kill**, so open files keep working. Mounting over a non-empty directory is refused. `remove_dir` only removes empty directories. state.json writes are atomic. |
| 3 | The startup race unmounts good adopted mounts | Warm-up requires a loaded config file (A6) and one **non-empty** Ok from every network provider, or the grace period measured from that first config load (A18). An invalid config exits instead of running empty. A missing config runs the empty default with `ready = false`, so no healthy, un-held mount adopted from the mount table is unmounted until a config file loads (test `missing_config_never_unmounts_adopted`). |
| 4 | Daemon death kills or corrupts children | `process_group(0)`, no `kill_on_drop`, and log output goes to a file rather than a pipe (no SIGPIPE). Adoption uses the fingerprint marker from the kernel mount table and is scoped to this daemon's root. No pid is ever signalled. |
| 5 | Remount storms | The fingerprint covers the selector text, so `auto` is sticky. Offline-but-Healthy is NoOp. Absent observations age out after `3 × interval`. A failing provider freezes. `failures` resets only on a Healthy probe once the mount has been up for `retry_max` (A15). Events fire on transitions only. |
| 6 | Fighting sshfs's own `reconnect` | Degraded or Unresponsive is left to `reconnect` and ServerAlive until the grace period. Bifröst acts immediately only on Stale (a dead process or ENOTCONN) or a spec change. |
| 7 | Untrusted discovery data escalates | Winner-takes-all by trust, per-observation filters, identity pinned to the DNS label, native ids never used as identity and matched only by their own provider's filter (A19), no driver hint at all (E2), HTTP `host` replacing `addresses` for CIDR checks (A20), `honor_hints` off by default, validators both at the provider and in the typed spec, hostile E2E records. |
| 8 | rclone `--sftp-ssh` edge cases | Validated tokens and a quoted config path. `--sftp-shell-type=none` and `--sftp-disable-hashcheck`. `--sftp-host` passed defensively. The flag is feature-detected in the probe. p09 proves the whole path. |
| 9 | The SSH environment differs under a service manager | `ssh_agent` appears in status and doctor, and the README documents `systemctl --user import-environment SSH_AUTH_SOCK`. A host-key failure is surfaced with the preflight's exact stderr plus a README hint to run `ssh-keyscan`. Bifröst never falls back to weaker checking. |
| 10 | macOS can't be run here | OS code lives only in mount, config paths and bifrost-config's `default_auto_order` (B8). The argv builders take a `Flavor` parameter, so macOS argv is golden-tested on Linux. `getmntinfo` compiles in the darwin check. Known unknowns: nfsmount may need root; the macFUSE kext may not be approved (shows as Failed with a permission hint in `last_error` and doctor, B7); whether FUSE-T honours `fsname` (state.json covers adoption). |
| 11 | Suspend and wake | `Instant` excludes suspend on Linux (CLOCK_MONOTONIC) and macOS (mach uptime), so grace timers don't count sleep and nothing is mass-unmounted. Tickers probe every mount within `health_interval` after wake. |
| 12 | Parallel agents break each other | S0 freezes manifests, the lockfile, signatures and daemon wiring. Worktrees are used per agent, with exclusive file ownership. Safe stubs keep M1 free of panics. E2E phases are separate files, listed explicitly in `run.sh` (C7). |
| 13 | Unverified tool behaviour | The preflight exit code and the sshfs `fsname` override are verified in S1 (agent C) before anything depends on them, and each has a stated fallback. |

---

## 15. Deliberate simplifications

Each item becomes a `// ponytail: <ceiling>; <upgrade>` comment at the named location.

| # | simplification | ceiling | upgrade path | location |
|---|---|---|---|---|
| 1 | `BoxFuture` alias instead of async-trait; the `ctx` parameters from PRD §5 dropped; `inspect` returns `MountState` (errors fold into Degraded) | impls write `Box::pin(async move {..})` | add `DiscoveryContext`/`DriverContext` when a plugin needs runtime context | core/lib.rs |
| 2 | Static discovery is `registry.replace(config.static_observations())`, with no provider object or task | static machines can only come from config | implement `DiscoveryProvider` if static ever needs another source | daemon/actor.rs |
| 3 | Config reload polls the file every 2s (std, on a plain `std::thread`, B10) instead of using notify | up to ~4s latency; SIGHUP and the API are instant | notify 8.x if latency matters | daemon/reload.rs |
| 4 | Hand-rolled glob (`*`, `?`), CIDR, duration, `~`/`$VAR` expansion, FNV-1a, jitter and mountinfo parser | no character classes or brace globs; durations use a single unit | globset if rules need classes | core/validate.rs, mount/table.rs |
| 5 | SSH timings are constants: ConnectTimeout 10, ServerAlive 15×3 | a dead link takes ≥45s to detect; the command line overrides ssh_config values | an `[ssh]` section if users need tuning | mount/lib.rs `SSH_OPTS` |
| 6 | rclone `--dir-cache-time=15s` constant | a dead rclone remote can look healthy for ≤15s; directory listings are refetched every 15s | make it a key if listing traffic matters | mount/rclone.rs |
| 7 | Probe timeout fixed at 5s | slow links under load can read as Degraded (harmless: no action until grace) | a key if false Degraded reports annoy | mount/check.rs |
| 8 | HTTP: 10s timeout, 1 MiB body, 1000 entries, no ETag or Cache-Control | expensive inventories get polled in full every interval | ETag/If-None-Match | discovery/http.rs |
| 9 | Removal hysteresis is `3 × interval`, with no grace tombstones | a machine absent from successful refreshes for 3 intervals is unmounted (gracefully) | tombstones for `offline_grace_period` if inventories flap for longer | core/registry.rs |
| 10 | Same-kind provider tie-break is by provider name, not config order | two DNS providers reporting the same id: the alphabetically first wins | carry a config rank in `Source` | core/registry.rs |
| 11 | Winner-takes-all merge | a DNS/HTTP include can't mount an id that a more trusted source reports without allowing it | a per-id `prefer = "<provider>"` rule | core/policy.rs |
| 12 | No client SSE consumer; the TUI polls `/v1/status` every 1s inline in its UI loop (no poller task or channel, E6) | event latency ≤1s; debugging SSE needs `curl --unix-socket` | `Client::events()` plus a `bifrost events` command | client, tui |
| 13 | Child log is truncated at each spawn; no rotation or size cap | a noisy long-lived rclone grows its log | cap on the health tick | mount/lib.rs |
| 14 | No explicit wake detector | after resume, probes and discovery happen within one interval | compare SystemTime vs Instant elapsed and force ticks | daemon/actor.rs |
| 15 | No process is ever signalled except our own timed-out spawn; no orphan scan | a process stuck in connect lives until ConnectTimeout; a lazily-detached sshfs lingers until its references close | none (deliberate: killing loses data) | mount/lib.rs |
| 16 | `vfs_cache_mode` and `ssh_config` are not in the fingerprint | changes apply to new mounts only | include them in the fingerprint | core/model.rs |
| 17 | Drivers are re-probed only at startup, reload, the fallback tick and `POST /v1/reconcile` | a freshly installed tool is seen within `reconcile_interval` | — | daemon/actor.rs |
| 18 | Graceful shutdown doesn't unmount | stopping the daemon leaves mounts unsupervised until restart | `bifrost unmount <all>` before stop if wanted | daemon/main.rs |
| 19 | Only mounts directly under the current root are adopted | after a root change across a restart, old-root mounts are left alone | unmount any marker mount outside the root at startup | mount/lib.rs `adopt` |
| 20 | No concurrency cap on mount operations | dozens of simultaneous auto-mounts each spawn ssh at once | a tokio Semaphore in the executor | daemon/actor.rs |
| 21 | Inline root TXT machine records (the first form in PRD §6.3) and DNSSEC are not supported | publishers must use index + node records | parse `host=` at the index | discovery/dns.rs |
| 22 | Provider warnings (skipped records) go only to the log | invisible from the CLI and TUI | `warnings` in `ProviderDto` | discovery/*, daemon |
| 23 | macOS adoption without a marker trusts a state.json record; the driver guess falls back to "sshfs" | inspect and unmount logic is shared, so the guess is harmless | read the NFS source to tell drivers apart | mount/lib.rs |
| 24 | TUI uses truecolor only | Terminal.app renders the RGB colours approximately | map to 256-colour indices when `COLORTERM` isn't truecolor | tui/ui.rs |
| 25 | macOS code is compile-checked only | runtime behaviour of macFUSE, FUSE-T and nfsmount is unproven | a macOS runner for the E2E | mount/* |
| 26 | Discovery intervals have no jitter; one `failures` counter covers both mount and unmount | synchronised provider polls | ±10% jitter | daemon/actor.rs |
| 27 | Missing config means the empty default config (root `~/machines`) | a typo in `BIFROST_CONFIG` silently runs with no machines (logged at warn); a file that appears later with a non-default `mount.root` is rejected (A22) and needs a restart | an opt-in `--require-config` | daemon/main.rs |
| 28 | A discovered machine's default template is `remote = "~"` (the remote login directory), not PRD §2's `~/machines/agent-01/home/sami/project` shape (B2) | discovered machines mount their home unless the provider template sets `remote` | a per-provider default in docs or a smarter template | bifrost-config/src/lib.rs |
| 29 | No CI yaml until a git remote exists; `scripts/check.sh` is the gate (PRD §29 P0 CI, B3) | checks run only when an agent or a human runs them | add a CI workflow calling `scripts/check.sh` once a remote exists | scripts/check.sh |
| 30 | A provider that fails permanently freezes its last view forever (B9) | machines it reported stay listed, and mounted, until it recovers or is removed from config | expire frozen observations after a long cap (e.g. `offline_grace_period`) | core/registry.rs |
| 31 | `bifrostd` and `bifrost-discovery` are never darwin-checked: reqwest pulls in ring, whose C build needs an Apple toolchain this host lacks (B9; the ring cross-compile experiment is deferred) | macOS compile errors in the daemon or providers show up only on a Mac | a macOS runner, or retry `SDKROOT=/ CC_aarch64_apple_darwin=true AR_aarch64_apple_darwin=true cargo check --target aarch64-apple-darwin --workspace` | daemon/main.rs |

### Critical Files for Implementation
- /home/samimishal/projects/rust/bifrost/crates/bifrost-core/src/reconcile.rs
- /home/samimishal/projects/rust/bifrost/crates/bifrost-core/src/validate.rs
- /home/samimishal/projects/rust/bifrost/crates/bifrost-core/src/policy.rs
- /home/samimishal/projects/rust/bifrost/crates/bifrost-mount/src/lib.rs
- /home/samimishal/projects/rust/bifrost/crates/bifrost-daemon/src/actor.rs

## Amendments (applied)

The critique triage from the approved plan, copied verbatim. IDs refer to `docs/design/critique.md` (A = defects, B = missing homes, C = inconsistencies, D = API claims, E = ponytail cuts; P = orchestrator additions). Every row is applied in place in the sections above; where any text still conflicts, the amendment wins, as refined by the list after the table.

| ID | Change (short) | Owner |
|---|---|---|
| A1 | Rename `gen` to `generation` everywhere; `gen` is reserved in edition 2024 | S0 |
| A2 | `MountSpec::{fingerprint, source}`, `marker`, `parse_marker` and `DriverSelector: TryFrom<String>` are implemented and tested in S0 | S0 |
| A3 | `Msg`, `Tick`, `ApiCmd` and `Deps` are frozen in S0 in `daemon/src/actor.rs` and `api.rs` | S0 |
| A4 | Config apply and provider diffing live in `actor.rs` (S2-E) and are used at startup too. `reload.rs` = `spawn_poller(path, tx)` only. `reload::apply` and `ApiCmd::Reload` are deleted. `POST /v1/config/reload` does `spawn_blocking(load)` then sends `Msg::Config{reply}` | S0/S2-E/S4-M |
| A5 | `Deps.drivers: Arc<dyn Fn(&DriverSettings) -> Vec<Arc<dyn MountDriver>> + Send + Sync>` | S0 |
| A6 | `ready = false` until a config file has actually loaded. Coherent with A18: static-only config → ready immediately; a network provider → waits for a non-empty Ok or the grace period | S1-A/S2-E |
| A7 | Provider loop: `discover` first, then `select!{sleep, notified}` | S2-E |
| A8 | `clean(s, max)` plus `tail(s, max)`: keep the last lines and skip the argv header; 512 chars for errors and log lines | S0 |
| A9 | Outer mount timeout = `mount_timeout + 60s`. `mount()` step 2: our marker with the same fingerprint → `Ok(handle{pid: None})`; our marker with a different fingerprint → lazy detach, then continue; foreign → Refused. On timeout, only detach an entry that carries our marker | S1-C/S2-E |
| A10 | If sshfs ignores `fsname=`, readiness and inspect fall back to path + fstype. S1-C verifies which case applies (strings evidence says a user `fsname` wins) | S1-C |
| A11 | `POST /v1/reconcile` does NOT clear `mount_retry_at`; `POST …/mount` is the "retry now". The p06 jq is filtered to `static1` | S2-E/S2-G |
| A12 | The rclone half of psec moves to p09 | S2-G/S3-J |
| A13 | E2E and tests kill by `$(mpid id)`, never with `pkill -f fsname=` (that would also kill the auto_unmount helper) | S1-C/S2-G |
| A14 | Rows 3 and 5: `why = if held { Manual } else { NotDesired }` | S1-A |
| A15 | `mounted_at: Option<Instant>`; failures reset on Healthy only when `now ≥ mounted_at + retry_max`. Test `failures_reset_only_after_stable_healthy` | S1-A |
| A16 | Row 11 is gated on `cand.driver.is_ok() && cand.online != Some(false)`, else `Degraded("change pending: …")` | S1-A |
| A17 | liveness: any server reply (Ok, NotFound, PermissionDenied) → Healthy | S1-C |
| A18 | `ready` needs a **non-empty** Ok from each network provider, or `offline_grace_period` since start | S1-A/S2-E |
| A19 | `native_id` is matched only by the owning provider's `include_ids`/`exclude_ids`; global `ids` match the machine id only. §4 reworded | S1-A |
| A20 | HTTP: when `host` is present, `addresses = [host]` | S3-K |
| A21 | chmod only directories the daemon created; the socket stays 0600 | S2-E |
| A22 | A `mount.root` change on reload is rejected ("mount.root change requires restart") and the old config is kept | S2-E |
| A23 | rclone `--cache-dir=<state>/rclone/<id>` per mount | S3-J |
| B1 | The actor emits `MachineEligible` and `DriverUnavailable` on transitions. Test `events_eligible_and_driver_unavailable_on_transition` | S2-E |
| B2 | `remote = "/"` parses and `sftp_path("/") == "/"`. §15 row: the discovered default is `remote="~"` | S0 |
| B3 | §15 row: no CI yaml until a git remote exists; `scripts/check.sh` is the gate | S0 |
| B4 | p04 uses the §31 shorthand (`name/host/user/remote`) with host 127.0.0.1, port 2222, under `$T`. The real `~/machines` is never touched | S2-G |
| B5 | p08 is the only fragment with `[policy.deny] tags=["misc"]`; it asserts other-01 shows `denied (policy.deny tags=misc)` | S3-I |
| B6 | p13a: after a restart, `pgrep -xc sshfs` is unchanged (no duplicate process) | S2-G |
| B7 | A log tail matching `kernel extension\|System Extension\|not permitted` adds a macOS permission hint to `last_error` and doctor; README note | S3-J/S4-N |
| B8 | Core is provider- and driver-agnostic:<br>• `Source{trust: u8, kind: String, provider: String}`, with trust ranks in bifrost-config (static 0, tailscale 1, http 2, dns 3);<br>• `evaluate` step 4 becomes `sel.source.trust == 0`;<br>• `ProviderDto.kind: String`;<br>• `DRIVER_NAMES` and `default_auto_order` move to bifrost-config, `dns_label` to `discovery/dns.rs`;<br>• core checks only the grammar of `DriverSelector::Named` | S0 |
| B9 | **Accept the §15 rows** (a permanently failing provider freezes its view; bifrostd and discovery are not darwin-checked). **Defer** the ring cross-compile experiment | S0 |
| B10 | The config poller runs on a `std::thread`. A deleted or unreadable file keeps the active config and warns once | S4-M |
| B11 | Create `<state>/logs` (0700) at startup. `actor::spawn` takes `socket: PathBuf`. A `build_provider` Err → `ProviderDto.last_error`, no task spawned, grace warm-up | S0/S2-E |
| B12 | `FakeDriver` gains `exits: Mutex<BTreeMap<MountId, OnExit>>`, `fn exit(id, detail)` and `busy: Mutex<BTreeSet<MountId>>` | S0 |
| B13 | The `rclone_argv` stub returns `vec![]`; S3-J extends `argv_never_weakens_host_keys`, whose forbidden list includes `sftp_server` | S0/S3-J |
| B14 | `check::which_in(name, path: &OsStr)` and `probe_with(path)`; tests never call `set_var` | S1-C |
| B15 | Pinned behaviour:<br>• a mount id wins over a machine id in API targets;<br>• `adopt` takes the pid from the state record with the same `local_path`, else None;<br>• a desired mount with a driver error shows Failed with detail;<br>• a machine with no mounts shows Eligible | S1-A/S1-C/S2-E |
| C1 | Use `ratatui::crossterm::event::poll` | S3-L |
| C2 | `WaitReason::Backoff(Duration)`, computed in `decide` | S0 |
| C3 | `backoff` uses `failures.saturating_sub(1)`; `backoff_bounds` covers 0 | S0 |
| C4 | One string for a busy unmount: `"unmount blocked: busy (files open)"` | S1-A |
| C5 | Wording: "GET status routes only read the watch channel"; "the default socket path is never under /tmp" | S0 |
| C6 | The CLI always sends `UnmountReq{force}`; body-less POSTs send `{}` | S2-F |
| C7 | `run.sh` lists phase files explicitly (no glob order); curl added to the tool list | S2-G |
| C8 | hickory errors `.map_err(\|e\| e.to_string())`; `BIFROST_LOG` accepts `error` | S0/S3-I |
| D1 | Delete the two hickory `options_mut()` lines, since the defaults already match | S3-I |
| E1 | Cut `GET /v1/machines/{id}`; `machines show` filters `GET /v1/machines` | S0 |
| E2 | Cut `MountHints.driver`, `Bf1.driver` and the HTTP `driver` field; `driver=` becomes an ignored key | S0 |
| E3 | Cut `MachineRegistry::next_expiry` and its actor deadline | S0 |
| E4 | `ApiCmd::Discover` has no reply oneshot (the route notifies and returns 202); `ApiCmd::Reload` is removed (A4) | S0 |
| E5 | Cut `Candidate.fingerprint`, `MachineDto.eligible`, `DriverDto.auto_rank` and tailscale `metadata.tailscale_id` | S0 |
| E6 | The TUI polls inline: `rt.block_on(timeout(500ms, get("/v1/status")))` once per second, with no poller task or channel | S3-L |
| P1 | No `rustfmt.toml`; rustfmt defaults | S0 |
| P2 | All agents share `CARGO_TARGET_DIR=/home/samimishal/projects/rust/bifrost-target`. With 4 CPUs and about 2 GB free RAM, compiling the dependencies once beats per-worktree builds; the workflow cap is 2 concurrent agents anyway. This replaces contract §1 rule 3 | all |

### Refinements (S0.2 verifier, round 1)

These refine the rows above. Where a verbatim row's wording differs, this list wins over that row.

- **A18 / A6:** "`offline_grace_period` since start" means since the first successful config load (`cfg_loaded_at`). That is the daemon start whenever a config file exists at startup (§5 Warm-up, §8 `spawn`).
- **A22 / A6:** A22 also covers the missing-config start. The default root `~/machines` is active, so a later file is applied only if its `mount.root` equals that root; otherwise a restart is needed (§8 Startup step 4, §15 #27).
- **A6:** "nothing is unmounted" means no healthy, un-held mount. Row 3 (Stale or force-requested) and a held unmount still go through.
- **A16:** The row 11 gate falls through instead of returning. Row 12 still detaches a hung mount after grace, and row 13 shows `"change pending: …"` for a working mount (§5 table).
- **A9 / A10:** In the A10 fallback, an entry with the driver's own fstype at `local_path` counts as ours for mount step 2 (lazy detach, never adopt) and for the timeout detach (§6).
- **B7:** The hint code is owned by S1-C (shared `mount/src/lib.rs` spawn and readiness code), S2-F (doctor) and S4-N (README). S3-J inherits it and owns no B7 code.
- **E5:** `StatusDto.auto_driver` (the daemon's `select_driver(&Auto, …)`) is the one source for the default shown by status, doctor, `drivers` and the TUI; S2-F owns the status, doctor and `drivers` printing. The per-driver `auto_rank` stays cut.
- **C7:** S2-G writes both complete `run.sh` lists; a listed phase file that doesn't exist yet is skipped. S3 and S4 agents never edit `run.sh`.
- **A8:** `clean` caps are 128 for names, 256 for metadata values and 512 for errors and log lines.

## Orchestrator sign-offs (after S1)

These are binding for S2 onward and win over earlier text where they differ.

1. `reconcile::next_wakeup(runtimes, grace, now)` takes `now` and returns only deadlines strictly after it. The actor never sleeps until an instant ≤ now.
2. `MountRuntime` stores no id, so `mount_done(Err)` returns `MountFailed` with an empty `mount`. **The actor stamps the id it keys the runtime by into every event before publishing it.**
3. `SshfsDriver { s: DriverSettings }` (private field) is accepted. It mirrors `RcloneDriver { nfs }`.
4. **Verified on this host** (sshfs 3.7.3 / fuse3 3.14.0 / OpenSSH 9.6):
   - the ssh `-s sftp` preflight exits 0 against a working server, so it stays;
   - a user `-o fsname=bifrost:<id>@<fp>` IS the fuse.sshfs mountinfo source, so the A10 fallback is not used;
   - after `kill -9 sshfs`, `auto_unmount` does NOT unmount. The mount stays and returns ENOTCONN, so recovery is Stale → row 10 (lazy detach) → row 9, driven by the health tick. p06's 20s budget relies on the E2E `health_interval = 2s`;
   - `fusermount3 -u` on a busy mount prints "Device or resource busy" and exits 1. On a path that isn't mounted it prints "entry for … not found" and exits 1.
5. `crates/bifrost-cli/tests/config_check.rs` (created by S1-B) is owned by S2-F from S2 on.
6. These S1-B choices are accepted:
   - `ConfigError.path` is the key path for semantic errors, `<file>:<line>:<col>` for TOML syntax/shape errors, and the file path for an unreadable file;
   - every duration is ≥ 1s;
   - `config check --json` prints `ReloadDto`;
   - `config check` reports on stdout;
   - `parse()` makes a single `Path::exists()` call for `ssh_config`;
   - `load()` on a missing file is Err, and the daemon decides the fallback (§8 step 4).
7. The S1-A Display strings for verdicts, actions, wait reasons and `Reason` are final. S2-F golden tests lock them in.
8. A graceful held unmount of an **adopted** mount that has no candidate waits for `ready`; `--force` still goes through row 3. Accepted.
9. `api.rs` keeps `#![allow(dead_code)]` until S2-E's `main.rs` serves `router()`. The S2 merge agent deletes that line, and the ones in `actor.rs`, once they are no longer needed.

## Orchestrator sign-offs (after S2 / Milestone 1)

M1 gate: `tests/e2e/run.sh m1` passed 63/63 on its first run (commit 359db31).

1. **Accepted S2-E choices:**
   - the tickers are spawned in the actor's config-apply path, so interval changes take effect on reload;
   - `POST /v1/reconcile` replies after the pass that follows the next driver re-probe;
   - the next deadline is measured from the last pass's `now`;
   - state.json is written synchronously on the actor;
   - generations are seeded per created runtime;
   - inspect has a 30s outer timeout.
2. **Open, for the S4 final review:** if an executor task panics, its runtime stays in flight (row 1) forever. Convert a `JoinError` into `MountDone`/`UnmountDone` Err (or Degraded) so the mount recovers.
3. **Accepted S2-F choices:**
   - doctor exits 1 on a Config ✗, on no usable driver, or on a B7 hint; one unavailable driver alone does not fail it;
   - `ClientError::Io` exits 3;
   - the NAME column prints the machine id;
   - the local probe uses placeholder `DriverSettings`. **So S3-J's rclone probe must look only at binaries and flags, never at the settings.**
4. **E2E harness rules:**
   - Run with `TMPDIR` unset, because `$T/bf.sock` must be ≤103 bytes, and with `CARGO_TARGET_DIR` exported.
   - One run at a time: `run.sh` takes `flock -n /tmp/bf-e2e.lock` and exits 2 if another run holds it. Wait and retry; never delete the lock.
   - The daemon is started with `9>&-`, so the lock can't leak into sshfs children.
   - `run.sh` copies the binaries into `$T/bin` after building, so parallel worktree builds can't swap them mid-run.
   - Phase-file convention (the `run.sh` header): `<p>` is the stem before the first `_`, `check_<p>` is required, `setup_<p>`/`config_<p>` are optional.
   - The template uses `$E2E` for paths.
   - Only ONE phase fragment may define `[policy.*]` tables, and that is p08 (B5).
5. The second-instance message no longer has a doubled prefix ("bifrostd: already running (lock …)").

## Orchestrator sign-offs (after S3)

S3 gate:
- `run.sh all` passed 116/116, with p12 and p13 not yet present;
- `E2E_TAILSCALE=1 run.sh p07_tailscale` passed 5/5, with zero mounts;
- the darwin check is clean;
- the scripted tmux TUI session passed 81/81;
- 230 unit tests pass (commit e8306d1).

1. **Accepted interpretations:**
   - **Tailscale:** a peer whose DNSName or IP is invalid is skipped whole, and empty metadata values are omitted.
   - **bf1:** a trailing space is an error; the 2 KiB limit is checked after the `v=bf1` test; `dns_label` does not lowercase; a node lookup `JoinError` freezes the provider.
   - **rclone:** the `--sftp-ssh` config path is quoted (spaces allowed, `"` and control characters rejected), and `--sftp-host` is still passed.
   - **HTTP:** any invalid field skips the whole entry; `name: ""` is skipped; p10 flips the token on the server side.
   - **TUI:** uses `rt.block_on(async { timeout(..).await })` and `ratatui::try_init`, and shows "U force" only in the help popup.
2. **Carry-overs for S4 (task O)** — each one must be fixed with a test:
   - (a) If an executor task panics, the runtime is stuck in flight (the open S2 item). Map a `JoinError` to MountDone/UnmountDone `Err` so the mount recovers.
   - (b) The driver list is empty after daemon start until the first probe returns. Either probe synchronously before the first snapshot, or show "probing".
   - (c) With `BIFROST_LOG=debug`, hickory's debug logging writes raw TXT answers (untrusted, possibly containing terminal escapes) into the daemon log. The subscriber must cap third-party targets (hickory*, reqwest, hyper, rustls) at `warn` whatever `BIFROST_LOG` says, without adding a dependency.
   - (d) Tailscale `which()` must also search `/usr/local/bin:/usr/bin:/bin` (plus the macOS app path) like `bifrost-mount::check::which`, so it still works under a minimal systemd PATH.
   - (e) `flavor()` and the docker test helpers are duplicated between `sshfs.rs` and `rclone.rs`. Move them into `lib.rs` as `pub(crate)` and delete the copies.
3. **For the S4 hardening/README pass:**
   - E2E scratch dirs are kept, one per run, in /tmp, and a p07 scratch dir holds real tailnet names. The README says so.
   - `run.sh` must treat a missing listed phase as fatal once p12 and p13 exist.

## Orchestrator sign-offs (after S4a)

S4a gate: `run.sh all` passed 164/164 twice in a row, with only p07's opt-in skip. `check.sh` is green with 240 tests, and the darwin check is green (commit b46b8d8).

1. **Accepted: SIGHUP wakes the poller thread**, which reads, parses and sends immediately. So one component owns the poll state, and the same bytes are never sent twice.
2. **Accepted: the log filter is an allowlist.** `BIFROST_LOG` applies only to `bifrost*` targets. Every other target is capped at `min(level, WARN)`, so tokio and axum debug logs no longer appear under `BIFROST_LOG=debug`.
3. **Accepted: driver, probe, inspect and provider panics are caught** and turned into errors or Degraded, so nothing stays in flight.
4. **Accepted: drivers show `probing` in the snapshot** until the first probe result arrives.
5. **Replaces §12's skip rule:** `run.sh` now treats a missing listed phase file as FATAL. The only skip left is p07's opt-in.
