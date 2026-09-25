# Bifröst V1: final implementation contract

This contract keeps the structure of design B and fixes every defect the judges found in it. It takes specific ideas from design A (minimal) and design C (correctness) where they make the result simpler or more correct. "ponytail" is the house engineering style: the laziest solution that actually works, with every deliberate corner cut recorded. Each item in §15 becomes a `// ponytail:` comment at the named location.

## Changes from B

| # | Judge defect in B | Fix | § |
|---|---|---|---|
| 1 | The orphan reaper kills every same-uid process with a `bifrost:` marker, including the user's real daemon during E2E. | Reaper deleted. **No pid is ever signalled** except a child this daemon spawned whose mount readiness timed out. Adoption only looks under this daemon's canonical root. | 6, 8 |
| 2 | Force unmount does lazy detach and then kills the process group, losing writes that are in flight. | Force is `fusermount3 -u -z` (Linux) or `diskutil unmount force` (macOS), **with no kill**. After a lazy detach the child keeps serving open files and exits when the last reference closes. | 6 |
| 3 | The `HostKeyChecking::AcceptNew` trust-on-first-use (TOFU) knob weakens host verification. | Enum and config key deleted. The constant `SSH_OPTS` never contains StrictHostKeyChecking, UserKnownHostsFile or GlobalKnownHostsFile. Test `argv_never_weakens_host_keys`. | 3, 6, 11 |
| 4 | `stat` on the mount root is answered from the kernel and sshfs caches, so a dead remote looks healthy. | The probe looks up a unique nonexistent name, `<mnt>/.bifrost-probe-<nonce>`. rclone gets `--dir-cache-time=15s` as its ceiling. | 6 |
| 5 | Warm-up treats an `Err` as "reported", so adopted tailscale mounts get unmounted. | `ready` requires one **Ok** from every network provider, or `offline_grace_period` since start. | 5, 8 |
| 6 | Row 11 force-unmounts `Missing` mounts, and fusermount exits non-zero, which loops. | `Missing` is a state transition (to Absent with backoff), not an action. `unmount` is idempotent: if the path is not in the mount table afterwards, the result is Ok. | 5, 6 |
| 7 | `generation` only bumps on spawn, so a late health result can land after an unmount. | `begin()` bumps `gen` for both mount and unmount. Provider results carry `task_gen`. | 5 |
| 8 | A persistent `Override::Mounted` can mount a DiscoverOnly machine that has an untrusted address. | Removed. Only `held` (manual unmount) is persisted. `POST …/mount` clears a hold and returns **403** for anything that isn't a candidate. | 4, 8 |
| 9 | A DNS NXDOMAIN makes the index `Ok(empty)`, so every machine flaps to Lost. | The registry never removes on absence. Absent observations age out at `expires_at`, and expiry is frozen while a provider is failing. | 2, 7 |
| 10 | The hickory `build()` call was missing a `?`. | `TokioResolver::builder_tokio()?.build()?` (verified). | 7 |
| 11 | Merging by provider config order lets untrusted DNS supply the connect host. | **Winner takes all by trust**: `Static < Tailscale < Http < Dns`. The selected observation's host, port, hints and metadata are the only ones used. | 4 |
| 12 | Any provider's exclude or deny, or a DNS-published tag, could deny a static machine. | A provider `exclude` drops only that provider's observation. Global deny is evaluated against the selected (most trusted) observation, so a lower-trust source can't redirect or deny a machine that a more trusted source reports. | 4 |
| 13 | `#[serde(transparent)]` newtypes bypass validators when state.json or DTOs are read. | `#[serde(try_from = "String", into = "String")]` on Name, Host, User, RemotePath and DriverSelector. | 2 |
| 14 | A missing config makes the daemon exit, so PRD §31's plain `bifrostd` can't start. | Missing config means the empty default config plus a warning, and the poller adopts the file once it appears. An invalid config means exit 2. | 8 |
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
- whether sshfs honours a user-supplied `fsname=` (§6, E2E p04).

---

## 1. Crate layout, dependencies, parallel build

Eight crates, following the PRD §14 practical layout. Crates are split only for PRD layering or for dependencies. Parallel agents get isolation from **git worktrees**, not from crate boundaries.

```
bifrost/
├── Cargo.toml  Cargo.lock  rustfmt.toml  scripts/check.sh  README.md
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
2. S0 writes every public signature in §2, §3, §6, §7, §8 and §9 as compiling stubs that follow the **safe-stub rule** (§13). Every `lib.rs` declares its modules in S0, so later agents only fill in files.
3. Each agent works in its own worktree (`git worktree add ../bf-<agent> -b <agent>`), edits only the files it owns (§13), and builds with `cargo test -p <crate>`. Keep the default per-worktree target directory; sharing one would serialise builds on cargo's lock.
4. The orchestrator merges one branch at a time and runs `scripts/check.sh` after each merge (`cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`). A public signature changes only with the orchestrator's sign-off.

**macOS compile check.** All OS-specific code lives in `bifrost-mount`, `bifrost-config::paths` and `core::model::default_auto_order`. `bifrost-discovery` and `bifrost-daemon` contain no macOS code except a runtime `cfg!()` in tailscale, which compiles on Linux. They are excluded because reqwest pulls in ring, and ring needs an Apple C toolchain.

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

/// Either a std `IpAddr` literal (no brackets, no zone id), or a hostname: one trailing '.' stripped,
/// ≤253 bytes, labels 1..=63 of [A-Za-z0-9_-], no label starting with '-'. Stored lowercase.
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

/// "~" | "~/<rel>" | "/<abs>"; ≤1024 bytes; no control chars (<0x20, 0x7f); no ':' ; no ".." component.
/// Spaces are allowed: always a single argv element, never placed inside --sftp-ssh.
pub struct RemotePath(String);
impl RemotePath {
    pub fn parse(s: &str) -> Result<Self, Invalid>;
    pub fn as_str(&self) -> &str;
    pub fn sftp_path(&self) -> &str;               // "~"→"", "~/rel"→"rel", "/abs"→"/abs"
}

pub fn tag(s: &str) -> Result<String, Invalid>;       // lowercased; ^[a-z0-9][a-z0-9_.:-]{0,62}$
pub fn meta_key(s: &str) -> Result<String, Invalid>;  // ^[a-z0-9_.-]{1,64}$
pub fn dns_label(s: &str) -> Result<String, Invalid>; // ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$  (no dots)
pub fn native_id(s: &str) -> Result<String, Invalid>; // ^[A-Za-z0-9._:-]{1,128}$
pub fn clean(s: &str) -> String;                      // display-only text: control chars → '?', truncate at 128 chars
pub fn parse_duration(s: &str) -> Result<std::time::Duration, Invalid>; // ^[0-9]+(ms|s|m|h)$, >0, checked overflow

/// `[a-z0-9*?._-]{1,63}`; '*' = any run, '?' = one char; iterative two-pointer matcher.
#[derive(Clone, Debug, PartialEq, Eq)] pub struct Glob(String);
impl Glob { pub fn parse(s: &str) -> Result<Self, Invalid>; pub fn matches(&self, s: &str) -> bool; }

/// "10.0.0.0/8" | "fd7a::/48" | bare IP (= /32 or /128). Address families never cross-match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub struct Cidr { pub addr: std::net::IpAddr, pub prefix: u8 }
impl Cidr { pub fn parse(s: &str) -> Result<Self, Invalid>; pub fn contains(&self, ip: std::net::IpAddr) -> bool; }

pub fn fnv64(bytes: &[u8]) -> u64;   // FNV-1a 64: stable across Rust versions (used for fingerprints)
pub fn random_u64() -> u64;          // RandomState::new().build_hasher().finish() — std only
/// base = min(max, initial·2^min(failures−1, 32)); returns base/2 + rand % (base/2 + 1ms)   ("equal jitter", ∈ [base/2, base])
pub fn backoff(failures: u32, initial: Duration, max: Duration, rand: u64) -> Duration;
```

```rust
// ===== model.rs =====
pub const DRIVER_NAMES: [&str; 3] = ["sshfs", "rclone", "rclone-nfs"];
pub fn default_auto_order() -> Vec<String>;  // cfg!(target_os="macos") ? [rclone-nfs, rclone, sshfs] : [sshfs, rclone]

/// Declaration order == trust order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind { Static, Tailscale, Http, Dns }

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata { pub tags: BTreeSet<String>, pub values: BTreeMap<String, String> }   // validated / clean()ed

/// Untrusted, pre-validated. user/path are used only with `honor_hints`; driver is display-only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountHints { pub user: Option<User>, pub path: Option<RemotePath>, pub driver: Option<String> }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineObservation {
    pub id: MachineId,
    pub name: String,                 // display, clean()ed
    pub native_id: Option<String>,    // tailscale ID / TXT id= / HTTP id — matched only by the `ids` primitive
    pub addresses: Vec<Host>,         // ≥1; [0] = connect target
    pub port: Option<u16>,
    pub online: Option<bool>,         // Some only when the provider knows (tailscale, http)
    pub metadata: Metadata,
    pub hints: MountHints,            // static: user = config user (trusted)
    pub ttl: Option<Duration>,        // DNS only; registry floors it
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum DriverSelector { Auto, Named(String) }    // "auto" | one of DRIVER_NAMES

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
    #[error("busy: files are open under the mountpoint")] Busy,
    #[error("refused: {0}")] Refused(String),   // occupied path, symlink, not a dir, not empty, invalid request
    #[error("{0}")] Failed(String),             // preflight / driver log tail (clean()ed), timeout
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
    pub fn all(&self, src: &Source, o: &MachineObservation) -> bool;
    /// exclude/deny: any single primitive matches → Some("names=prod-*").
    /// cidrs fail CLOSED here: a non-static observation with no IP-literal address matches any cidr entry.
    pub fn any(&self, src: &Source, o: &MachineObservation) -> Option<String>;
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
/// Ord == trust order: kind, then provider name.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Source { pub kind: ProviderKind, pub provider: String }
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
    /// Successful network refresh. Upserts each obs (a duplicate id within `obs`: the first wins, the rest are warned)
    /// with expires_at = now + max(obs.ttl.unwrap_or(ZERO), floor). Ids this provider reported earlier but not
    /// now are NOT removed; they age out. Clears `failing`. Returns ids that are new to the registry.
    pub fn apply_ok(&mut self, src: &Source, obs: Vec<MachineObservation>, now: Instant, floor: Duration) -> Vec<MachineId>;
    /// Failed refresh: expire() skips this provider until its next apply_ok (freeze, not drop).
    pub fn mark_failed(&mut self, provider: &str);
    /// Authoritative, no expiry: static observations on every config apply. Returns (new ids, gone ids).
    pub fn replace(&mut self, src: &Source, obs: Vec<MachineObservation>) -> (Vec<MachineId>, Vec<MachineId>);
    pub fn remove_provider(&mut self, provider: &str) -> Vec<MachineId>;   // gone ids
    pub fn expire(&mut self, now: Instant) -> Vec<MachineId>;             // gone ids
    pub fn next_expiry(&self) -> Option<Instant>;
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
pub struct Candidate { pub spec: MountSpec, pub fingerprint: String, pub driver: Result<String, String>,
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
    pub degraded_since: Option<Instant>, pub gen: u64, pub failures: u32,
    pub mount_retry_at: Option<Instant>, pub unmount_retry_at: Option<Instant>,
    pub last_error: Option<String>,
    pub offline: bool,            // set by an OfflineGrace unmount; cleared by a successful mount
    pub force_requested: bool,    // API `unmount --force`
    pub adopted: bool, pub probing: bool,
}
impl MountRuntime {
    pub fn adopted(h: MountHandle) -> Self;                                      // Mounted, health Unknown, adopted=true
    pub fn begin(&mut self, phase: Phase) -> u64;                                // gen += 1 (BOTH ops); phase = Mounting|Unmounting
    pub fn mount_done(&mut self, gen: u64, r: Result<MountHandle, MountError>, now: Instant, t: &Timing, rand: u64) -> Option<Event>;
    pub fn unmount_done(&mut self, gen: u64, why: Reason, r: Result<(), MountError>, now: Instant, t: &Timing, rand: u64) -> Option<Event>;
    pub fn health(&mut self, gen: u64, s: MountState, now: Instant, t: &Timing, rand: u64) -> Option<Event>;
}   // every transition ignores stale gen and returns an Event only on a state change (§5)

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitReason { InFlight, WarmingUp, NoDriver(String), MachineOffline, Backoff(Instant) }
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
pub fn next_wakeup(runtimes: &BTreeMap<MountId, MountRuntime>, grace: Duration) -> Option<Instant>;

/// PRD §12 states. Ord == display severity (used when aggregating a machine's mounts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability { Unknown, Discovered, Eligible, Mounted, Connecting, Unmounting, Offline, Degraded, Failed }
pub fn mount_availability(rt: Option<&MountRuntime>, c: Option<&Candidate>) -> Availability;
pub fn machine_availability(m: &Machine, mounts: &[Availability]) -> Availability;
```

```rust
// ===== events.rs — PRD §20 + MountDegraded =====
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
    pub machines: Vec<MachineDto>, pub mounts: Vec<MountDto>,
    pub conflicts: Vec<String>, pub events: Vec<EventRecord>,   // last 200
}
pub struct ProviderDto { pub name: String, pub kind: ProviderKind, pub machines: usize, pub refreshes: u64,
                         pub last_ok_secs_ago: Option<u64>, pub last_error: Option<String> }
pub struct DriverDto { pub name: String, pub available: bool, pub binary: Option<String>, pub detail: String,
                       pub auto_rank: Option<usize> }
pub struct MachineDto { pub id: String, pub name: String, pub source: String, pub shadowed: Vec<String>,
    pub address: String, pub port: Option<u16>, pub online: Option<bool>, pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>, pub verdict: String, pub eligible: bool,
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

// ===== fake.rs — always compiled, #[doc(hidden)], std only (~90 lines) =====
pub struct FakeDiscovery { pub name: String, pub result: Mutex<Result<Vec<MachineObservation>, DiscoveryError>> }
pub struct FakeDriver {
    pub name: String,
    pub mounted: Mutex<BTreeMap<MountId, MountHandle>>,
    pub states: Mutex<BTreeMap<MountId, MountState>>,
    pub fail_next: Mutex<BTreeSet<MountId>>,
    pub calls: Mutex<Vec<String>>,
}
impl FakeDriver {
    pub fn new(name: &str) -> Self;
    pub fn set_state(&self, id: &str, s: MountState);
    pub fn fail_next(&self, id: &str);
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
honor_hints = false                           # true ⇒ record user=/path= win (validated); driver= is never used

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
pub struct ProviderConfig { pub name: String, pub kind: ProviderKind, pub interval: Duration,
                            pub template: MountTemplate, pub spec: ProviderSpec }
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
| `default_driver`, every `driver` | `auto` or one of `DRIVER_NAMES` (OS-agnostic; unavailable on this OS ⇒ runtime `Waiting(NoDriver)`) |
| `auto_order` | non-empty, subset of `DRIVER_NAMES`, no duplicates |
| `mount.ssh_config` | expanded; absolute; exists; contains no `"` (it is quoted inside `--sftp-ssh`) |
| `vfs_cache_mode` | off \| minimal \| writes \| full |
| `discovery[i].type` | tailscale \| dns \| http |
| `discovery[i].name` | defaults to the type; `Name::parse`; unique (a second instance of a type needs a `name`); `static` is reserved |
| per-type keys | `domain`/`nameservers` only for dns (domain required, `Host`, not an IP); `url`/`headers` only for http (url required) |
| `nameservers` | `IpAddr` (port 53) or `SocketAddr` |
| `url` | `https://`, or `http://` only for 127.0.0.1, [::1] or localhost |
| `headers` | name is an RFC 7230 token; value after expansion has no CR, LF or NUL |
| filter / policy | ids pass `native_id` (or `Name`); names pass `Glob::parse`; cidrs pass `Cidr::parse`; tags pass `tag`; metadata keys pass `meta_key` and values are ≤256 chars with no control characters; `policy.*.providers` name an existing provider or kind |
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
selected source is Static:
    one spec per StaticMount: host = s.addresses[0], port = s.port, user = s.hints.user (config),
    remote/driver/read_only from the StaticMount
otherwise:
    one spec:  id = machine id;  host = s.addresses[0];  port = s.port   (address data from the selected obs)
    user   = if P.honor_hints { s.hints.user.or(P.user) } else { P.user }   // None ⇒ ssh decides
    remote = if P.honor_hints { s.hints.path.or(P.remote) } else { P.remote }
    driver = P.driver;  read_only = P.read_only                             // the driver hint is never used
local_path = root.join(id)                                                  // the only place local paths are built
collision:  static locals are claimed first → a discovered id equal to a claimed local is skipped
            and reported in Desired.conflicts ("agent-01: local name taken by static machine build")
```

---

## 4. Policy semantics

| primitive | config keys | include / allow matches when | exclude / deny matches when |
|---|---|---|---|
| exact id | `ids` / `include_ids` / `exclude_ids` | id or `native_id` equals an entry | same |
| name glob | `names` / `*_names` | the id glob-matches an entry | same |
| CIDR | `cidrs` / `*_cidrs` | an IP-literal address is inside an entry (hostnames are never resolved) | the same, **or** a non-static observation has no IP literal (fail closed) |
| tag | `tags` / `*_tags` | any listed tag is in `metadata.tags` | same |
| provider | `providers` (global only) | the provider name or kind name is listed | same |
| metadata | `metadata` / `*_metadata` | **all** pairs equal | **any** pair equal |

Include and allow combine as AND across non-empty kinds and OR within a kind. Exclude and deny fire on any single primitive.

**`evaluate(policy, observed)`.** `observed` is in trust order (Static < Tailscale < Http < Dns, then provider name). The first match wins.

```
1. live = observations whose provider filter exclude does NOT match   // an exclude drops only that observation
   live empty                                   → Denied{by: "<provider>.filter.exclude <prim>"}, selected = 0
2. sel = live[0]                                // WINNER TAKES ALL: the most trusted remaining observation
3. policy.deny.any(sel)                         → Denied{by: "policy.deny <prim>"}          // deny wins, static included
4. sel.kind == Static                           → Allowed{by: "static"}                      // static = explicit allow
5. filter(sel.provider).include non-empty ∧ .all(sel) → Allowed{by: "<provider>.filter.include"}
6. policy.allow non-empty ∧ policy.allow.all(sel)     → Allowed{by: "policy.allow"}
7. otherwise                                    → DiscoverOnly                              // PRD §7: discover yes, mount no
```

What this rule guarantees:
- **Untrusted data can't widen trust.** A lower-trust source (DNS/HTTP) can never set the connect host, port, user, tags or metadata of a machine that a more trusted source reports, even when that source's verdict is DiscoverOnly. Shadowed sources are listed in `MachineDto.shadowed`.
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
- ⊳U means: if `unmount_retry_at > now`, the result becomes `Waiting(Backoff)` for rows 3 and 5, and `Degraded(last_error)` for rows 10–12.

| # | D | phase | condition | Action |
|---|---|---|---|---|
| 1 | * | Mounting / Unmounting | — | `Waiting(InFlight)` |
| 2 | no | Absent | — | `NoOp` (runtime is dropped once it has no pending retry) |
| 3 | no | Mounted | H = Stale ∨ force_requested | `Unmount{force: true, NotDesired}` ⊳U |
| 4 | no | Mounted | `!ready` ∧ not held | `Waiting(WarmingUp)` |
| 5 | no | Mounted | — | `Unmount{force: false, NotDesired}` ⊳U (busy → backoff, never auto-forced) |
| 6 | yes | Absent | `driver` is Err | `Waiting(NoDriver)` |
| 7 | yes | Absent | `online == Some(false)` | `Waiting(MachineOffline)` |
| 8 | yes | Absent | `mount_retry_at > now` | `Waiting(Backoff)` |
| 9 | yes | Absent | — | `Mount{driver}` |
| 10 | yes | Mounted | H = Stale | `Remount{force: true, Stale}` ⊳U |
| 11 | yes | Mounted | `handle.fingerprint ≠ cand.fingerprint` | `Remount{force: H = Degraded, SpecChanged}` ⊳U |
| 12 | yes | Mounted | H = Degraded ∧ `now − degraded_since ≥ grace` | `Unmount{force: true, OfflineGrace}` ⊳U |
| 13 | yes | Mounted | H = Degraded | `Degraded(reason)` (sshfs `reconnect` + ServerAlive is handling it) |
| 14 | yes | Mounted | — | `NoOp` (Healthy, Unknown, or offline-but-Healthy: a working mount is never removed) |

Notes on the table:
- `Remount` is executed exactly like `Unmount`. The new mount comes from row 9 on a later pass, so the offline and backoff gates still apply.
- Stale cleanup is gated only by `unmount_retry_at`, never by mount backoff.
- A spec change includes a host change such as a DNS IP move, because the fingerprint covers the host. It remounts gracefully. If the mount is busy, it stays `Degraded("change pending: busy")` and retries.

**Runtime transitions** (pure, in `MountRuntime`). A message with the wrong `gen` is ignored. `bo` means `now + backoff(failures, retry_initial, retry_max, rand)`.

| input | effect | event |
|---|---|---|
| `begin(Mounting \| Unmounting)` | `gen += 1`; set phase | MountRequested / UnmountStarted (emitted by the actor) |
| `mount_done(Ok h)` | Mounted; `handle = h`; H = Unknown; `offline = false`; `last_error = None`; `mount_retry_at = None` (**failures not reset**) | MountStarted |
| `mount_done(Err e)` | Absent; `failures += 1`; `mount_retry_at = bo`; `last_error = e` | MountFailed |
| `unmount_done(Ok)` | Absent; `handle = None`; H = Unknown; clear `degraded_since`, `unmount_retry_at`, `force_requested`. If why = OfflineGrace: `offline = true`, `failures += 1`, `mount_retry_at = bo` | UnmountComplete |
| `unmount_done(Err e)` | back to Mounted; `failures += 1`; `unmount_retry_at = bo`; `last_error = e` | MountDegraded("unmount failed: …") |
| `health(Healthy)` | H = Healthy; clear `degraded_since`; `failures = 0`; `last_error = None` | MountHealthy (on change) |
| `health(Degraded r)` | H = Degraded; `degraded_since.get_or_insert(now)` | MountDegraded (on entry) |
| `health(Stale r)` | H = Stale; `failures += 1`; `mount_retry_at = bo` | MountDegraded("stale: …") |
| `health(Missing)` | Absent; `handle = None`; `failures += 1`; `mount_retry_at = bo`; `last_error = "mount disappeared"` | MountFailed |
| child exit (`on_exit`) | no state change; the actor probes that mount immediately (ignored while Unmounting) | — |

`failures` resets only on a **periodic** Healthy inspect, so a mount that dies right after mounting keeps backing off instead of looping. `POST …/mount` resets `failures`, both retry timers and `offline`.

**Availability** (PRD §12; derived, never stored):
- **Mount level:**
  - Mounting → Connecting.
  - Unmounting → Unmounting.
  - Mounted with H = Degraded or Stale → Degraded; otherwise Mounted.
  - Absent and desired: `offline` or `online == Some(false)` → Offline; `last_error` set → Failed; otherwise Eligible.
  - Held → Eligible, with `held: true` in the DTO.
- **Machine level:** a verdict that isn't Allowed → Discovered. Otherwise the maximum over its mounts, where the order is Failed > Degraded > Offline > Unmounting > Connecting > Mounted > Eligible.
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
| discovery | one task per network provider: `loop { select!{ sleep(interval), notify.notified() }; timeout(30s, discover()) → Msg }`; static observations are replaced on every config apply |
| health | ticker every `health_interval` → inspect every Mounted runtime with a handle and `!probing` |
| fallback | ticker every `reconcile_interval` → re-probe drivers, then run a pass |
| precise deadlines | the actor sleeps until the earliest of `next_wakeup()`, `registry.next_expiry()` and (while `!ready`) `start + grace` |
| child exit | the driver's `on_exit` → `Msg::ChildExited{id, gen}` → immediate inspect |
| config change | poller / SIGHUP / API → `Msg::Config` |
| API | `Msg::Api(cmd)` with a oneshot reply |

**Warm-up.** `ready` becomes true once every network provider has returned **Ok** at least once, or `offline_grace_period` has passed since start. Once true it stays true. Before that, only rows 4/5 are blocked, and a held (manual) unmount goes through. Removal decisions that depend on discovery wait for discovery to have actually worked. A provider that is down at login delays removals by at most the grace period.

**Concurrency model.** One actor task owns all mutable state: `Arc<Config>`, the registry, runtimes, `held`, probe results, provider status and the event ring. There are no locks on domain state.

```rust
pub enum Msg {
    Discovery { provider: String, task_gen: u64, result: Result<Vec<MachineObservation>, DiscoveryError> },
    MountDone { id: MountId, gen: u64, result: Result<MountHandle, MountError> },
    UnmountDone { id: MountId, gen: u64, why: Reason, result: Result<(), MountError> },
    Health { id: MountId, gen: u64, state: MountState },
    ChildExited { id: MountId, gen: u64, detail: String },
    Probed(BTreeMap<String, DriverAvailability>),
    Config { result: Result<Box<Config>, Vec<ConfigError>>, reply: Option<oneshot::Sender<ReloadDto>> },
    Api(ApiCmd),
    Tick(Tick),                                  // Health | Fallback
}
```

- **Inbox:** `mpsc::unbounded_channel`. `on_exit` is a sync callback and must never block; producers are naturally rate-limited.
- **Loop:** `select!` over the inbox, `sleep_until(deadline)` and shutdown. It drains `try_recv`, then runs one pass. It publishes `watch::Sender<Arc<StatusDto>>` and pushes events to `broadcast::Sender<EventRecord>` (capacity 256) plus a ring of 200 kept in the snapshot.
- **The actor never awaits I/O and never touches a mount path.**
- **Executor tasks** run each driver call under a timeout: mount gets `mount_timeout + 20s` (that covers the preflight), unmount gets 30s. They report through `Msg`.

**Hung-FUSE guards.**
1. Mount-table reads (`/proc/self/mountinfo`, `getmntinfo(MNT_NOWAIT)`) never touch FUSE.
2. The only filesystem calls on a live mountpoint run inside `check::timed` (§6). It is `spawn_blocking` under a 5s timeout. A per-path in-flight set means at most one leaked thread per hung mount; while a probe is stuck, the next one returns immediately.
3. `prepare_mountpoint` and `remove_dir` run only after the mount table says the path is **not** a mountpoint.
4. Every helper command (fusermount3, umount, diskutil, ssh preflight, tailscale, probes) runs under `tokio::time::timeout` with `kill_on_drop(true)`. Mount children are the exception: they use `kill_on_drop(false)`.
5. The root is canonicalized once, at startup.

---

## 6. Mount drivers (`bifrost-mount`)

```rust
pub struct DriverSettings { pub ssh_config: Option<PathBuf>, pub vfs_cache_mode: String, pub mount_timeout: Duration }
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
    /// Ok | NotFound → Healthy; NotConnected (macOS also raw errno 6 ENXIO) → Stale; other → Degraded(e); None → Degraded("unresponsive").
    pub async fn liveness(path: &Path) -> MountState;
    pub fn which(name: &str) -> Option<PathBuf>;           // $PATH + /usr/local/bin:/usr/bin:/bin (+ macos /opt/homebrew/bin); is_file ∧ mode & 0o111
    pub async fn run(bin: &Path, args: &[OsString], t: Duration) -> std::io::Result<std::process::Output>; // stdin null, kill_on_drop(true)
    pub async fn ssh_preflight(ssh: &Path, spec: &MountSpec, ssh_config: Option<&Path>) -> Result<(), MountError>;
}
pub const SSH_OPTS: [&str; 6] = ["BatchMode=yes", "ConnectTimeout=10", "ServerAliveInterval=15",
                                 "ServerAliveCountMax=3", "ControlMaster=no", "ControlPath=none"];
#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Flavor { Linux, MacFuse, FuseT }
pub fn sshfs_argv(spec: &MountSpec, ssh_config: Option<&Path>, f: Flavor) -> Vec<OsString>;             // pure
pub fn rclone_argv(spec: &MountSpec, ssh: &Path, ssh_config: Option<&Path>, vfs: &str, nfs: bool, f: Flavor) -> Vec<OsString>; // pure
pub fn preflight_argv(spec: &MountSpec, ssh_config: Option<&Path>) -> Vec<OsString>;                   // pure
pub async fn unmount_path(path: &Path, force: bool) -> Result<(), MountError>;
/// Pure. Only entries whose parent == root. Marker source ⇒ adopt (driver from fstype, else record, else "sshfs");
/// no marker ⇒ adopt only when state.json has a record with the same local_path (macOS NFS fallback); else foreign.
pub fn adopt(entries: &[table::MountEntry], root: &Path, records: &BTreeMap<MountId, MountHandle>) -> Vec<MountHandle>;
```

**Host keys are never weakened.** `SSH_OPTS` is the only set of ssh options Bifröst ever passes. It never contains StrictHostKeyChecking, UserKnownHostsFile, GlobalKnownHostsFile, ProxyCommand or IdentityFile. The user's ssh_config (or the `-F` file) decides trust, and `BatchMode=yes` turns "ask" into "fail". `ControlPath=none` ties the mount's lifetime to our process rather than to a user's multiplexing master. Test: `argv_never_weakens_host_keys`, a scan of every argv builder for those substrings.

**SSH preflight**, run before every spawn for both drivers (PRD §8.1 "validate SSH connectivity"):

```
<ssh> -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3
      -o ControlMaster=no -o ControlPath=none [-F <ssh_config>] [-p <port>] [-l <user>] -s -- <host> sftp
stdin=/dev/null, stdout=/dev/null, stderr piped; timeout 15s
exit 0 ⇒ host key + auth + sftp subsystem OK;  else Err(Failed(clean(stderr tail)))
         e.g. "Host key verification failed." / "Permission denied (publickey)."
```

This matters because rclone swallows ssh's stderr. **Unverified here:** that ssh exits 0 when sftp-server sees EOF on stdin. S1 agent C verifies this against the docker sshd. If it doesn't hold, drop the preflight and rely on the log tail.

**Spawn** (shared by both drivers):
- `tokio::process::Command::new(<absolute binary from which()>).args(argv)`. Never `sh -c`.
- `stdin(null)`; stdout and stderr both go to `log_path` (`<state>/logs/<id>.log`, opened create+truncate+write with mode 0600, argv written first as a header line). **Never a pipe**: a full pipe can stall the FUSE server, and a pipe would SIGPIPE orphaned children after a daemon crash.
- `process_group(0)`, so a terminal Ctrl-C or the daemon's death never signals the child.
- `kill_on_drop(false)`.
- The environment is inherited, including `SSH_AUTH_SOCK`.
- After a successful mount, a supervisor task owns the `Child`: `tokio::spawn(async move { let s = child.wait().await; (req.on_exit)(describe(s)) })`. It reaps the child. There is no kill channel.

**`mount(req)`:**
1. Check the request:
   - `local_path` is absolute, and `local_path.parent()` canonicalizes to itself;
   - `file_name == id`;
   - the binary resolves; otherwise `Unavailable`.
2. `table::read()`. If anything is at `local_path`, return `Refused("occupied by <fstype> <source>")`.
3. `prepare_mountpoint`:
   - `symlink_metadata` NotFound → `create_dir` with mode 0700;
   - a symlink → Refused;
   - not a directory → Refused;
   - `read_dir` non-empty → `Refused("not empty")`. fuse3 would mount over it and hide the files.
4. `ssh_preflight`.
5. Spawn.
6. Every 100 ms until `mount_timeout`:
   - the mount table has `local_path` (on Linux also `source == marker(id, fp)`) → `Ok(MountHandle{pid: child.id()})` and start the supervisor;
   - the child has exited → `Err(Failed(status + clean(last 2 KiB of the log)))`;
   - the deadline passed → `start_kill()` and `wait()` (this is the only kill, and it hits our own child that never finished mounting), then lazily unmount if the path is present, then `Err(Failed("timed out after 30s: <tail>"))`.

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

**Unverified here:** that a user `fsname=` overrides sshfs's own. E2E p04 asserts `bifrost:static1@` appears in mountinfo. If it doesn't, adoption falls back to state.json records, the same path macOS NFS uses.

**rclone argv.** Always `--flag=value` form.

```
<rclone> mount|nfsmount :sftp:<sftp_path> <local_path>
  --config=/dev/null                             # never read the user's rclone.conf
  --sftp-host=<host>                             # harmless with --sftp-ssh; avoids a possible "host not set"
  "--sftp-ssh=<ssh> -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3
               -o ControlMaster=no -o ControlPath=none [-F \"<ssh_config>\"] [-p <port>] [-l <user>] <host>"
  --sftp-shell-type=none --sftp-disable-hashcheck   # rclone never runs remote shell commands
  --devname=bifrost:<id>@<fp16>
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

Probes run at startup, on reload, on every fallback tick and on `POST /v1/reconcile`, so installing sshfs later heals itself. Because the spec fingerprint covers the selector text, `auto` mounts are sticky and a probe flap never remounts them.

**`inspect(h)`:**
1. `table::read()`: no entry at `h.local_path`, or an entry that doesn't carry our marker (Linux) → `Missing`.
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

A live, busy mount that is merely not desired is **never** forced. It stays Degraded ("unmount blocked: busy") and retries with backoff.

**Stale-mount recovery:**
- sshfs is killed → `auto_unmount` removes the mount → Missing → Absent + backoff → row 9.
- rclone is killed, or a dead adopted mount is found → ENOTCONN → Stale → row 10 (lazy detach) → row 9.
- Hung → Degraded → row 12 after grace.

**Adoption after a restart:**
- `adopt()` runs over the mount table and considers only entries **directly under this daemon's canonical root**.
- Adopted mounts get `MountRuntime::adopted(handle)` with the fingerprint taken from the marker. A fingerprint that differs from the candidate's goes through row 11 (graceful remount, or Degraded if busy).
- A non-marker entry at a desired path is foreign. It is never touched, and `mount()` returns Refused, visible as Failed.
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
                     pub tags: Vec<String>, pub driver: Option<String>, pub path: Option<RemotePath>, pub id: Option<String> }
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
- Every untrusted field goes through a `core::validate` function before it can reach an observation.
- A bad record is skipped with `warn!(provider, record, reason = clean(truncated value))`, so records are isolated from each other.
- Output is sorted by id, and a duplicate id is skipped with a warning (first wins).
- `Err` is returned only when the source as a whole couldn't be read. The registry then freezes that provider's observations.
- The daemon wraps each call in `timeout(30s)`.
- Removal hysteresis: `expires_at = now + max(ttl, 3 × interval)`, applied in `apply_ok`.

**Tailscale.**

Invocation and errors:
- `tailscale status --json`, with a 10s timeout and stdout capped at 16 MiB.
- Binary lookup: the constructor's `binary`, else `"tailscale"` via PATH, else on macOS `/Applications/Tailscale.app/Contents/MacOS/Tailscale`. Not found anywhere → `Unavailable`.
- Non-zero exit → `Failed(clean(stderr))`. `BackendState != "Running"` → `Unavailable("backend state X")`.

Parsed JSON:
- Top level: `{BackendState, CurrentTailnet?{MagicDNSEnabled}, Peer: Option<BTreeMap<String, Peer>>}`. `Self` is ignored, so we never mount ourselves.
- `Peer{ID, HostName, DNSName, OS (default ""), TailscaleIPs: Option<Vec<IpAddr>>, Tags: Option<Vec<String>>, Online}`.
- Peers are processed sorted by DNSName.

| observation field | source | if invalid |
|---|---|---|
| id | `Name::parse(first label of DNSName)`, else `Name::parse(HostName)` (lowercases first; HostName can repeat) | peer skipped + warning |
| name | `clean(HostName)` | — |
| native_id | `native_id(ID)` | dropped |
| addresses | `[DNSName without the trailing dot]` if MagicDNS is enabled and DNSName is set, else first IPv4; then every IP | peer skipped if none |
| online / tags | `Some(Online)` / `Tags` with `tag:` stripped, each passed through `tag()` | a bad tag is dropped |
| metadata.values | `os`, `tailscale_id`, `hostname` (cleaned), `dns_name` | — |

New peers appear on the next refresh without a restart.

**DNS TXT `bf1`** (hickory 0.26.3).

Resolver setup:
```rust
let mut b = if nameservers.is_empty() {
    TokioResolver::builder_tokio()?
} else {
    let ns = nameservers.iter().map(|sa| {
        let mut n = NameServerConfig::udp_and_tcp(sa.ip());
        for c in &mut n.connections { c.port = sa.port(); }
        n
    }).collect();
    Resolver::builder_with_config(ResolverConfig::from_name_servers(ns), TokioRuntimeProvider::default())
};
b.options_mut().timeout = Duration::from_secs(5);
b.options_mut().attempts = 2;
let resolver = b.build()?;          // the cache stays on and honours TTL for lookups
```

All queries are absolute names ending in `.`, so resolv.conf search domains never apply.

```
S       := concat(all character-strings of one TXT RR)      ; UTF-8 ASCII, ≤ 2048 bytes
record  := token *(1*SP token)
token   := key "=" value                                     ; split at the first '='
key     := 1*32 [a-z0-9_-]      value := 1*256 (%x21-7E except '"')
first token != "v=bf1"  → Ok(None): not ours (SPF, future v=bf2) — ignored silently
missing '=', empty value, over-long key or value, duplicate key → Err (record invalid)
unknown keys → ignored.  Known: v nodes host port user tags driver path id
```

| key | validation | goes to |
|---|---|---|
| nodes | comma list of `dns_label` (no dots, so an index can't make us query another domain); ≤256 in total across all index RRs; deduplicated | index |
| host | `Host::parse` (rejects `-oProxyCommand=…`); default `<node>.<domain>` | `addresses[0]` |
| port | 1..=65535, ≤5 digits | port |
| user / path | `User::parse` / `RemotePath::parse` | hints |
| tags | ≤32 entries, each `tag()` | metadata.tags |
| driver | `auto` or one of `DRIVER_NAMES` | hints.driver (display only) |
| id | `native_id()`, so `../../etc` is rejected | native_id only; **identity is always the node label** |

Algorithm:
1. Look up `_bifrost.<domain>.`.
   - `is_no_records_found()`, or no valid bf1 index RR → `Ok(vec![])` plus a warning. With the expiry registry this causes no churn; entries age out after `3 × interval`.
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
    "host": "agent-01.corp",                         // optional connect target; else addresses[0]
    "addresses": ["10.20.0.4"],                      // each Host::parse; invalid ones dropped; ≥1 needed in total
    "port": 22, "online": true,
    "user": "sami", "path": "/home/sami", "driver": "auto",   // hints (validated; used only with honor_hints)
    "metadata": { "tags": ["dev", "agent"], "env": "dev", "rack": 4 }   // tags → tags; scalars → clean()ed strings; nested ignored
} ] }
```

- The body is parsed as `struct Top { machines: Vec<serde_json::Value> }`. At most the first 1000 entries are used; the rest get one warning.
- Each entry is converted separately, and a failing entry is skipped with `warn!("machines[{i}]: …")`.
- Refresh happens every `interval`. On failure, the last good view is served (frozen). On success, entries that are gone age out after `3 × interval`. That is the whole cache policy.

---

## 8. Daemon (`bifrostd`)

**Invocation.** `bifrostd [--version | --help]`. Any other argument prints usage and exits 2. Everything else comes from the environment: `BIFROST_CONFIG`, `BIFROST_SOCKET`, `BIFROST_STATE_DIR`, and `BIFROST_LOG` (trace|debug|info|warn, default info). **Exit codes:** 0 clean shutdown; 1 startup failure (already running, directories, socket); 2 usage or invalid config.

**Startup:**
1. `tracing_subscriber::fmt().with_max_level(level)`; ANSI only when stderr is a TTY.
2. `state_dir`: `DirBuilder::new().recursive(true).mode(0o700)`, then `set_permissions(0o700)`. That call fails with EPERM when we don't own the directory, which makes it the ownership check; exit 1.
3. `File::try_lock(<state>/bifrostd.lock)`. `WouldBlock` → "bifrostd already running", exit 1. The lock is released automatically on a crash.
4. Config: if the file is missing → `parse("")` (the empty default) plus a warning, and the poller picks up the file later. If `load` returns Err → print the sorted errors and exit 2. **An invalid config never starts a daemon that would unmount everything.**
5. `create_dir_all(root)` (0700), then `canonicalize` once. On macOS that resolves `/private` paths.
6. Read `state.json`. If it doesn't parse, rename it to `state.json.corrupt-<unix>` and start empty. `held` is authoritative (user intent); mount records are only hints.
7. `adopt(table::read(), root, records)`. Adopted runtimes are inspected at t = 0.
8. Build `drivers(&settings)` and probe them all concurrently.
9. Socket:
   - create the parent directory 0700 and `set_permissions(0o700)`;
   - reject paths longer than 103 bytes with a clear message;
   - the lock guarantees any existing socket file is stale, so remove it;
   - `tokio::net::UnixListener::bind`, then `set_permissions(0o600)`.
10. Spawn the actor, provider tasks, tickers, the config poller, and the signal task (SIGTERM/SIGINT → shutdown, SIGHUP → reload). Run `axum::serve(listener, router(state)).with_graceful_shutdown(signal)`, raced against `shutdown + 2s` so open SSE streams can't block exit.

**Wiring**, frozen in S0 and never edited by later stages:

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

// actor.rs
pub struct Deps {
    pub drivers: Vec<Arc<dyn MountDriver>>,
    pub build_provider: Arc<dyn Fn(&ProviderConfig) -> Result<Arc<dyn DiscoveryProvider>, String> + Send + Sync>,
}
pub fn spawn(cfg: Config, root: PathBuf, state_dir: PathBuf, held: BTreeSet<MountId>, adopted: Vec<MountHandle>,
             deps: Deps) -> (mpsc::UnboundedSender<Msg>, watch::Receiver<Arc<StatusDto>>,
                             broadcast::Sender<EventRecord>, JoinHandle<()>);

// api.rs
pub struct ApiError { pub status: u16, pub error: String }
pub enum ApiCmd {
    Mount     { target: String, reply: oneshot::Sender<Result<Vec<String>, ApiError>> },
    Unmount   { target: String, force: bool, reply: oneshot::Sender<Result<Vec<String>, ApiError>> },
    Reconcile { reply: oneshot::Sender<Vec<ActionDto>> },
    Discover  { reply: oneshot::Sender<()> },
    Reload    { reply: oneshot::Sender<ReloadDto> },
}
#[derive(Clone)]
pub struct AppState { pub snapshot: watch::Receiver<Arc<StatusDto>>, pub tx: mpsc::UnboundedSender<Msg>,
                      pub events: broadcast::Sender<EventRecord>, pub state_dir: PathBuf }
pub fn router(s: AppState) -> axum::Router;
```

**Routes** (axum 0.8 `{param}` syntax). Errors are `ErrorDto`.

| method + path | request | response |
|---|---|---|
| GET `/v1/status` | — | `StatusDto` (full snapshot) |
| GET `/v1/machines` | — | `Vec<MachineDto>` |
| GET `/v1/machines/{id}` | — | `MachineDto` / 404 |
| GET `/v1/mounts` | — | `Vec<MountDto>` |
| GET `/v1/mounts/{id}/log` | — | `LogDto`: last 64 KiB of `<state>/logs/<id>.log`, `clean()`ed per line; `id` goes through `Name::parse` so traversal is impossible |
| GET `/v1/drivers` | — | `Vec<DriverDto>` |
| POST `/v1/discover` | — | 202 `{}`: notifies every provider task |
| POST `/v1/reconcile` | — | 200 `Vec<ActionDto>`: re-probe, clear `mount_retry_at`, run a pass, return the plan |
| POST `/v1/mounts/{target}/mount` | — | 202 `Vec<String>` (ids). `target` is a mount id, or a machine id meaning all its mounts. Clears the hold and both retry timers. 404 unknown / **403** not a candidate (body carries the verdict) |
| POST `/v1/mounts/{target}/unmount` | optional `UnmountReq` | 202 `Vec<String>`: adds a persisted hold; `force` sets `force_requested`. 404 unknown |
| POST `/v1/config/reload` | — | 200 `ReloadDto` (`ok: false` when the config is invalid) |
| GET `/v1/events` | — | SSE |

Mount and unmount return immediately; clients wait by polling `/v1/mounts`. GET handlers only read the `watch` channel; they never wait on the actor.

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

**Config hot reload** (`reload.rs`). The config file is polled with std only; notify is not used.
1. Every 2s, `std::fs::read(path)` (this follows symlinks).
2. Continue only if the bytes differ from the active config **and** are identical on two consecutive polls, which debounces half-written saves.
3. `config::parse` → `Msg::Config`.
4. A given set of bad bytes is reported once. SIGHUP and `POST /v1/config/reload` skip the debounce.

This handles rename-on-save editors, dotfile-manager symlink swaps and NFS or sshfs home directories.

In the actor, the whole swap happens in one step:
- **Invalid config:** keep the old `Arc<Config>`, set `status.config_errors`, emit `ConfigurationReloaded{ok: false, errors}`.
- **Valid config:**
  - `registry.replace(static, new.static_observations())`;
  - diff providers by name: removed → abort the task and `remove_provider`; added → spawn; changed (`ProviderConfig !=`) → abort and respawn with `task_gen + 1` (observations are kept until they expire); unchanged → keep;
  - rebuild `policy` and templates;
  - rebuild `drivers(&settings)` if `ssh_config`, `vfs_cache_mode` or `mount_timeout` changed, then re-probe;
  - swap the config, emit `ConfigurationReloaded{ok: true}`, clear `config_errors`, run a pass.
- A changed spec (including `root`) changes the fingerprint, which leads to a graceful remount through row 11.

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
1. Stop the API and provider tasks.
2. Wait up to 5s for in-flight executor tasks.
3. Write state.json, remove the socket, exit 0.

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
```

CLI global flags: `--json` (pretty-prints the DTO), `--socket PATH` (env `BIFROST_SOCKET`), `--config PATH` (env `BIFROST_CONFIG`). Tables are hand-aligned (column width is the widest cell), and every daemon string is passed through `clean()` again.

| command | daemon | route | human output |
|---|---|---|---|
| `status` | yes | GET status | summary block (below) |
| `machines`, `machines list` | yes | GET machines | `NAME SOURCE ADDRESS STATE MOUNTED` (PRD §18) |
| `machines show <id>` | yes | GET machines/{id} | key/value lines: verdict, address:port, online, tags, metadata, shadowed, mounts |
| `mounts` | yes | GET mounts | `ID MACHINE DRIVER STATE LOCAL REMOTE` (+ `ERROR` when any is set) |
| `mount <target> [--no-wait]` | yes | POST …/mount, then poll GET mounts every 250 ms for up to 60 s | `agent-01  mounted  /home/sami/machines/agent-01 (sshfs, pid 4242)` or `agent-01  failed: <last_error>` (exit 1) |
| `unmount <target> [--force] [--no-wait]` | yes | POST …/unmount, then poll | `agent-01  unmounted (held; 'bifrost mount agent-01' to resume)` or `busy (files open); retry with --force` (exit 1) |
| `discover` | yes | POST discover, then poll status until every provider's `refreshes` increases (≤ 30 s) | `PROVIDER KIND STATUS MACHINES LAST-OK` |
| `reconcile` | yes | POST reconcile | `MOUNT ACTION` |
| `drivers` | yes; local probe if the daemon is down | GET drivers | `✓ sshfs  /usr/bin/sshfs  SSHFS version 3.7.3, fusermount3` … then `default (auto): sshfs` |
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

**Exit codes:** 0 ok · 1 operation failed (API error, mount ended Failed or Offline, unmount busy, invalid config, doctor found a ✗ in Config or Drivers) · 2 usage (clap) · 3 daemon not reachable.

---

## 10. TUI (`bifrost-tui`)

**Architecture.**
- `App` is pure data: `status: Option<StatusDto>`, `view`, `selected`, `filter`, `popup`, `status_line`, `unreachable`, `no_color`, `log: Option<LogDto>`.
- `App::on_key(KeyEvent) -> Option<Command>`, where `Command = Mount(String) | Unmount(String, bool) | Reconcile | Discover | Reload | FetchLog(String) | Quit`.
- `ui::render(&App, &mut Frame)`.

**Runtime.**
- `main` is synchronous and owns a tokio multi-thread runtime with one worker.
- A spawned poller does `GET /v1/status` every 1s and sends results through `std::sync::mpsc`.
- The Logs view fetches `GET /v1/mounts/{id}/log` every 2s while it is visible.
- Commands are spawned as tasks, and their results come back as `status_line`.
- UI loop: `ratatui::init()`, then `crossterm::event::poll(100ms)`, drain the channel, `draw`, and `ratatui::restore()` on exit or panic.
- If the daemon is unreachable, a red banner reads "bifrostd not reachable at <socket> — retrying" and the last snapshot stays on screen.
- The TUI talks only to the daemon API.

**Views** (`1`–`7` or Tab / Shift-Tab):
1. Overview: the wordmark "Bifröst", the tagline "Remote worlds. Local files.", counts, provider health, config errors, the last 5 events.
2. Machines: `● agent-01  tailscale  mounted  sshfs`.
3. Mounts.
4. Discovery.
5. Drivers.
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
| d / Enter | details popup (verdict, observations, last_error, next retry) |
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
| 1 | Discovery ≠ trust | `policy::evaluate`: non-static observations default to DiscoverOnly. `reconcile::desired` produces candidates only for Allowed machines. The API returns 403 for non-candidates. Winner-takes-all by trust means DNS/HTTP can't redirect or deny a machine that a more trusted source reports. |
| 2 | No SSH passwords | No password keys exist (`deny_unknown_fields`). `BatchMode=yes` in `SSH_OPTS`, used by sshfs, the preflight and `--sftp-ssh`. Child stdin is `/dev/null`. |
| 3 | Prefer agent / keys / Tailscale SSH | The system `ssh` is always used (sshfs, preflight, rclone `--sftp-ssh`). `SSH_AUTH_SOCK` is inherited. `-F` only when configured. `doctor` and `StatusDto.ssh_agent` report the agent. |
| 4 | Never weaken host verification | The constant `SSH_OPTS` has no host-key options. No config knob exists. rclone's internal SFTP library is never used. Test `argv_never_weakens_host_keys`. E2E `psec_hostkey.sh` checks an unknown key fails with "Host key verification failed". |
| 5 | Mount only explicit paths | Remote paths come from static config or the provider template. Hints are used only with `honor_hints`, and only after `RemotePath::parse` (no `..`, `:` or control characters). The driver hint is never used. |
| 6 | Never execute discovery-supplied commands | Discovery data can only become `Name`, `Host`, `User`, `RemotePath` or `u16`. argv is built from constants plus those, via `Command::args`, never `sh -c`. Positionals can't start with `-`. `--flag=value` for rclone. `--sftp-ssh` tokens are checked for whitespace and quotes. `--sftp-shell-type=none` and `--sftp-disable-hashcheck`. The dangerous sshfs options (`ssh_command`, `directport`, `passive`) are never emitted. |
| 7 | TXT/HTTP are untrusted | `parse_bf1`: 2 KiB, `v=bf1` first, a duplicate key invalidates the record, labels have no dots, ≤256 nodes, whole-node rejection, identity pinned to the label. HTTP: 1 MiB, 1000 entries, per-entry isolation, no redirects, credentials only over https or loopback, no CR/LF in headers. `clean()` on every displayed string (no terminal-escape injection). E2E hostile records plus a canary file. |
| 8 | Local path traversal / collisions | `Name::parse` gives a single lowercase component. `local_path = root.join(id)` only in `desired`. Config enforces unique locals; at runtime static wins and the conflict is reported. `prepare_mountpoint` refuses symlinks, non-directories, non-empty directories and occupied paths. The root is canonicalized once and can't be `/` or `$HOME`. |
| + | Local API | The socket is 0600 inside a 0700 directory. `set_permissions` doubles as the ownership check. A lock file prevents a second daemon. The socket path is never under `/tmp`. |

---

## 12. Test plan

**Unit tests.** Every name is a `#[test]` or `#[tokio::test]`. No network, no root.

- **core::validate** (S0):
  - `name_rejects_traversal` (`""`, `.`, `..`, `a/b`, `../x`, `.x`, `-x`, `a b`, `a\0`, 64 chars), `name_lowercases`;
  - `host_rejects_option_injection` (`-oProxyCommand=x`, `a b`, `a,b`, `a"b`, `u@h`, `h:22`, `h;rm`, `fe80::1%eth0`, 254 chars);
  - `host_accepts_ipv4_ipv6_fqdn_trailing_dot`, `host_for_colon_brackets_v6`;
  - `user_rejects_dash_at_space`;
  - `remote_path_rules` (`~`, `~/x`, `/a b` ok; `a/b`, `/a/../b`, `/a\nb`, `/a:b`, `""` rejected), `sftp_path_mapping`;
  - `tag_meta_label_native_id_grammars`, `clean_strips_escapes`;
  - `duration_units_rejects_zero_and_junk` (`500ms`, `30s`, `5m`, `1h` ok; `5`, `5 m`, `-1s`, `1d` rejected);
  - `glob_star_question`, `cidr_v4_v6_no_cross_family`, `fnv64_known_vectors`;
  - `backoff_bounds` (failures 1..=64 × rand ∈ {0, u64::MAX}: within [base/2, base] and ≤ max), `backoff_no_overflow`.
- **core::model:** `fingerprint_changes_on_every_field`, `fingerprint_stable_vector` (a hard-coded expected hex), `marker_roundtrip_exact` (`agent-01` vs `agent-01-home`), `source_format`, `driver_selector_serde_rejects_unknown`, `serde_newtypes_validate_on_deserialize`.
- **core::policy:** `deny_wins_over_allow`, `global_deny_beats_static`, `provider_exclude_drops_only_that_observation`, `no_allow_rule_is_discover_only`, `static_allowed_by_default`, `include_kinds_anded_values_ored`, `include_metadata_all_pairs_deny_metadata_any`, `cidr_deny_fails_closed_without_ip`, `cidr_deny_static_hostname_not_matched`, `cidr_allow_requires_ip`, `providers_matches_name_or_kind`, `ids_match_native_or_id`.
- **core::registry:**
  - trust and merge: `trust_order_static_tailscale_http_dns`, `dns_cannot_redirect_tailscale_machine`, `dns_tags_cannot_deny_static_machine`, `dup_within_provider_first_wins`;
  - expiry and freezing: `absent_ages_out_not_removed`, `failed_provider_freezes_expiry`, `recovery_expires_long_absent`, `ttl_floor_three_intervals`;
  - bookkeeping: `replace_is_authoritative`, `remove_provider_returns_gone`, `next_expiry_earliest`.
- **core::reconcile:**
  - **`prd_fake_discovery_fake_driver`**: `FakeDiscovery` returns A + B, the policy has `include_names=["*"]` for `fake`, and `FakeDriver` has A mounted with a matching fingerprint and B absent. `plan` gives `[(a, NoOp), (b, Mount)]`.
  - **`plan_twice_second_all_noop`**: execute plan₁ through `FakeDriver` (`block_on`), set runtimes from `inspect`, then plan₂ is all NoOp. Also `plan_is_deterministic`.
  - One test per table row: `row01_inflight_waits`, `row02_absent_not_desired_noop`, `row03_stale_not_desired_force`, `row04_warmup_blocks_removal`, `row04_held_bypasses_warmup`, `row05_not_desired_graceful`, `row06_no_driver_waits`, `row07_offline_waits`, `row08_backoff_waits`, `row09_mount`, `row10_stale_remount_force`, `row11_spec_change_remount_graceful`, `row11_degraded_spec_change_lazy`, `row12_grace_elapsed_lazy_unmount`, `row13_degraded_within_grace`, `row14_offline_but_healthy_noop`, and `gate_u_unmount_backoff`.
  - Also: `stale_cleanup_not_gated_by_mount_backoff`, `missing_goes_absent_with_backoff`, `stale_gen_ignored_for_health_and_exit`, `failures_reset_only_on_periodic_healthy`, `grace_unmount_sets_offline`, `auto_driver_sticky_fingerprint`, `select_driver_auto_order_and_named_unavailable`, `hints_ignored_unless_honor_hints`, `driver_hint_never_used`, `static_local_collision_conflict`, `next_wakeup_is_earliest_deadline`, `availability_tables` (mount and machine), `offline_chain_mounted_degraded_offline`.
- **config:**
  - examples parse: `parses_full_example` (the §3 TOML verbatim), `parses_prd_s6_s7_s13_s31_snippets`, `shorthand_equals_mounts_form`, `empty_text_is_default_config`;
  - determinism and schema errors: `errors_sorted_deterministic` (byte-identical output twice), `unknown_field_has_line_col`, `per_type_keys_enforced`;
  - machine and mount rules: `remote_xor_mounts`, `local_traversal_rejected`, `duplicate_local_rejected`, `duplicate_machine_rejected`, `uppercase_name_hint`;
  - value rules: `unknown_driver_rejected`, `bad_duration_rejected`, `retry_initial_gt_max_rejected`, `root_slash_home_relative_rejected`;
  - expansion: `tilde_and_vars_expanded`, `undefined_var_error`, `dollar_dollar_literal`;
  - discovery and policy: `http_plaintext_non_loopback_rejected`, `header_crlf_rejected`, `discovery_names_default_unique_static_reserved`, `policy_unknown_provider_rejected`, `ssh_config_with_quote_rejected`.
- **mount:**
  - mount table: `mountinfo_octal_escapes`, `mountinfo_optional_fields`, `mountinfo_malformed_skipped`;
  - argv golden tests: `sshfs_argv_linux_golden`, `sshfs_argv_macfuse_golden`, `sshfs_argv_fuset_golden`, `sshfs_argv_ipv6_home_port_ro_cfg`, `rclone_argv_mount_golden`, `rclone_argv_nfsmount_forces_writes`, `rclone_sftp_ssh_tokens_clean_cfg_quoted`, `preflight_argv_golden`;
  - argv safety: **`argv_never_weakens_host_keys`**, `positionals_never_start_with_dash`, `rclone_never_uses_internal_ssh`;
  - fs and probes: `adopt_marker_record_foreign_outside_root`, `prepare_mountpoint_creates_rejects_symlink_file_nonempty`, `which_respects_exec_bit`, `timed_guard_single_thread` (the closure sleeps; a second call returns None immediately), `probe_fake_sshfs_in_path` (a temp script printing "SSHFS version 3.7.3"), `probe_missing_binary_unavailable`, `rclone_nfs_unavailable_on_linux`;
  - `#[ignore]` docker tests (needs `BIFROST_E2E_SSH=host:port:user:ssh_config` from `tests/e2e/lib.sh`): `sshfs_mount_inspect_unmount`, `sshfs_kill9_auto_unmount_missing`, `rclone_mount_write_roundtrip`, `rclone_kill9_stale_then_lazy`, `preflight_exit0_and_hostkey_failure`.
- **discovery:**
  - tailscale: `tailscale_fixture_parse` (Self excluded, `tag:` stripped, `Tags` absent, trailing dot trimmed, `Peer: null`, MagicDNS off → IP, duplicate HostName resolved via DNSName, a capitalised HostName fallback kept), `tailscale_backend_stopped_unavailable`, `tailscale_new_peer_appears` (a fake `tailscale` script swaps its fixture between two `discover()` calls), `#[ignore] tailscale_live_status`;
  - bf1 parser: `bf1_v_first_required`, `bf1_other_txt_ignored`, `bf1_unknown_keys_ignored`, `bf1_duplicate_key_invalid`, `bf1_host_injection_rejected`, `bf1_bad_id_rejects_node`, `bf1_nodes_no_dots_capped`, `bf1_char_strings_concatenated`, `bf1_size_limit`;
  - DNS assembly: `node_default_host`, `node_id_pinned_to_label`, `ambiguous_node_skipped`, `ttl_min_of_index_and_node`, `#[ignore] coredns_discovery`;
  - HTTP: `inventory_prd_example`, `invalid_entries_isolated`, `name_falls_back_to_id`, `metadata_tags_merged_scalars_flattened`, and against a 15-line tokio TCP responder on 127.0.0.1:0: `body_cap_enforced`, `auth_header_sent_non2xx_failed`, `redirect_not_followed`.
- **client:** `uds_roundtrip` (an axum dev-dependency server on a temp socket), `not_running_enoent_and_econnrefused`, `api_error_maps_status_and_body`.
- **daemon** (in-process: `Deps` with `FakeDriver` and a closure returning `FakeDiscovery`; temp state directory and socket):
  - startup and mounting: `mounts_desired_on_startup_and_idempotent`, `warmup_protects_adopted_until_ok`;
  - child exits: `child_exit_triggers_probe_and_remount`, `exit_during_unmount_ignored`;
  - API: `api_mount_forbidden_for_discover_only`, `api_unmount_holds_across_restart`, `api_unmount_busy_stays_degraded`;
  - socket and lock: `socket_0600_stale_replaced`, `second_instance_lock_refused`;
  - state file: `state_json_atomic_corrupt_quarantined`;
  - reload: `reload_invalid_keeps_old`, `reload_changed_provider_respawns_keeps_observations`, `stale_task_gen_dropped`;
  - SSE and logs: `sse_frame_format`, `log_route_rejects_traversal`.
- **cli:** `machines_table_golden`, `mounts_table_golden`, `status_block_golden`, `exit3_when_daemon_absent` (runs the binary against a bogus socket), `config_check_output_deterministic`.
- **tui:** `renders_machines_with_glyphs_and_teal`, `key_m_emits_mount_for_selection`, `U_asks_confirmation`, `filter_narrows_rows`, `unreachable_banner`, `no_color_disables_styles`.

**E2E harness.** `tests/e2e/run.sh [m1|all]`, using bash, docker, jq, python3, ssh-keygen and ssh-keyscan.

- Each phase file `pNN_*.sh` defines up to three functions: `setup_<p>` (fixtures), `config_<p>` (prints a TOML fragment) and `check_<p>` (assertions).
- `run.sh` sources `lib.sh` and the phase files for the chosen mode, then calls every `setup_*`, concatenates the `config_*` output after `config.tmpl.toml`, starts the daemon, and calls every `check_*` in file order.
- **Adding a phase never edits a shared file.** The m1 set is `p04 p05 p06 psec p13a`.

```bash
set -euo pipefail; T=$(mktemp -d); export E2E=$T BIFROST_SOCKET=$T/bf.sock BIFROST_STATE_DIR=$T/state \
  BIFROST_CONFIG=$T/config.toml BIFROST_LOG=debug BF_TOKEN=s3cret
trap cleanup EXIT        # fusermount3 -uz $T/machines/*; kill bifrostd; docker rm -f bf-e2e-sshd bf-e2e-dns; kill inventory
cargo build --workspace; PATH=$PWD/target/debug:$PATH
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
# mount_timeout 15s, grace 20s, retry 1s..4s; static1 = bf@127.0.0.1:2222 remote "/home/bf"
bifrostd > $T/d.log 2>&1 & DPID=$!; wait_until 10 bifrost daemon status
```

`lib.sh` provides `wait_until SECS CMD…` (polls every 0.5s), `mstate ID` (from `bifrost --json mounts | jq`), `mpid ID`, `is_mounted PATH` (grep in `/proc/self/mountinfo`), `start_daemon` and `stop_daemon`.

| file | set | acceptance checks |
|---|---|---|
| `p04_sshfs.sh` | m1 | `static1` mounted within 20s. `test -f $T/machines/static1/hello.txt`. mountinfo shows `fuse.sshfs` with source `bifrost:static1@`. `bifrost unmount static1` → not in mountinfo and the directory is removed. `bifrost reconcile` → still unmounted (held). `bifrost mount static1` → mounted again. |
| `p05_api.sh` | m1 | `bifrost status/machines/mounts/drivers` exit 0 and `--json` parses. `stat -c %a $BIFROST_SOCKET` is 600. A second `bifrostd` exits non-zero with "already running". `BIFROST_SOCKET=$T/nope bifrost status` exits 3. SSE: `timeout 8 curl -sN --unix-socket $BIFROST_SOCKET http://bifrost/v1/events` shows an `event: UnmountStarted` frame during an unmount/mount of static1. |
| `p06_recovery.sh` | m1 | `pkill -TERM -f 'fsname=bifrost:static1@'` → mounted within 20s with a new pid. `pkill -KILL …` → mounted within 20s, `ls` works, no ENOTCONN. `bifrost --json reconcile` twice → the second is all `noop`. `docker stop` → `degraded` within 30s and `kill -0 $DPID`. `docker start` → `mounted` within 40s and `hello.txt` readable. `docker stop` again and wait → `offline` and not in mountinfo within 45s (grace 20s, lazy detach). `docker start` → `mounted` within 40s. |
| `psec_hostkey.sh` | m1 | Static `unknown-key` (host `localhost`, which is not in known_hosts) → state `failed`, `last_error` contains "Host key verification failed", never in mountinfo. Same for `unknown-key-rc` with `driver="rclone"`. |
| `p13a_adopt.sh` | m1 | `kill -9 $DPID` → mount still works. Restart → `adopted:true`, same pid, and no `MountStarted` for static1 in `status --json .events`. SIGTERM → restart → the same. |
| `p07_tailscale.sh` | all, opt-in `E2E_TAILSCALE=1` | Provider with **no filter**. `machines --json` has ≥1 machine with source tailscale, and every one is `discovered`. **Zero** tailscale mounts. Ids equal the distinct first labels of DNSName (`jq`). Never runs `tailscale up/down` and never mounts real peers. |
| `p08_dns.sh` | all | coredns/coredns:1.11.3 on 127.0.0.1:5353 udp+tcp; Corefile `test.bifrost:53 { file /zones/db { reload 1s } }`; `$TTL 5`. Index `nodes=agent-dns,other-01,bad-node,evil`: agent-dns `host=127.0.0.1 port=2222 user=bf tags=dev path=/home/bf/data`; other-01 `host=127.0.0.1 port=2222 tags=misc`; bad-node `host=-oProxyCommand=touch${IFS}$T/pwned`; evil `host=127.0.0.1 id=../../etc`. Provider: `honor_hints=true`, `include_tags=["dev"]`. Checks: `dig @127.0.0.1 -p 5353 TXT _bifrost.test.bifrost` answers. agent-dns mounted with `w.txt` visible. other-01 `discovered`, never mounted. bad-node and evil absent, warnings in `$T/d.log`, `! test -e $T/pwned`. Remove the tag and bump the serial → unmounted within about 30s. |
| `p09_rclone.sh` | all | `static2-rc` (`driver="rclone"`): mountinfo `fuse.rclone` with source `bifrost:static2-rc@`; write and read round-trip. `bifrost drivers` shows sshfs ✓ and rclone ✓. `pkill -KILL -f 'devname=bifrost:static2-rc@'` → Stale → recovered within 20s. |
| `p10_http.sh` | all | `inventory.py 127.0.0.1:18080 $T/inv.json` (401 unless `Authorization: Bearer s3cret`; re-reads the file per request). Provider `include_names=["inv-*"]`. inv.json: `inv-01` → 127.0.0.1:2222, plus entries with address `-oProxyCommand=x` and name `../x`. Checks: inv-01 mounted; invalid entries absent and warned. Rewrite the token to a wrong value → provider `last_error` "HTTP 401" and inv-01 **stays mounted** (served from cache). Restore → ok. |
| `p12_reload.sh` | all | Append static `box2` → mounted within 10s and `ConfigurationReloaded{ok:true}`. Write broken TOML → `status --json .config_errors` non-empty and every mount untouched. Restore → errors cleared. Swap `auto_order` to `["rclone","sshfs"]` → existing `auto` mounts are **not** remounted (sticky); a newly added auto machine mounts via rclone. |
| `p13_hardening.sh` | all | IP change: agent-dns `host=127.0.0.2` plus a serial bump → remounted with a new fingerprint within 30s. Duplicate discovery: `inv-01` also published via DNS → one machine, source `inventory`, shadowed `dns`. Rename: a node label changes → the old one unmounts after about 9–15s and the new one mounts. |

---

## 13. Staged execution plan

**Safe-stub rule (S0):**
- Unimplemented providers return `Err(DiscoveryError::Unavailable("not implemented"))`.
- Unimplemented drivers return `Unavailable("not implemented")` from `probe` and `Err(MountError::Unavailable(..))` from `mount`.
- `reload::spawn_poller` is a no-op, and `reload::apply` returns `ReloadDto{ok:false, errors:["not implemented"]}`.
- `todo!()` is allowed only where M1 cannot reach.
- The daemon wiring (`build_provider`, `drivers()`) is final from S0 on, so no later stage edits it.

| stage | PRD phases | parallel agents (worktree → files owned) | acceptance |
|---|---|---|---|
| **S0** | 0 | 1 agent, serial: `git init`; workspace plus all 8 manifests; `cargo generate-lockfile`; every §2/§3/§6/§7/§8/§9 signature as a safe stub; **`core/validate.rs` fully implemented and tested**; daemon `main.rs` wiring; `scripts/check.sh`; `rustfmt.toml`; release profile; `rustup target add aarch64-apple-darwin` | `cargo build --workspace --all-targets`, `check.sh` and the darwin check are green; validator tests pass; `Cargo.lock` committed and frozen |
| **S1** | 1, 2, 4 (driver) | **A** `core/{model,policy,registry,reconcile,events,api,fake}.rs` bodies and tests. **B** `bifrost-config/*` plus the `config check` subcommand in `bifrost-cli/src/main.rs`. **C** `bifrost-mount/{lib,table,check,sshfs}.rs` (Linux and macOS argv), `tests/e2e/{lib.sh,sshd/Dockerfile}`, docker `#[ignore]` tests; verifies the preflight exit-0 behaviour and the `fsname` override. **D** `bifrost-client` plus `bifrost-daemon/src/api.rs` (router over a fixture snapshot with a stub actor task in tests) | A: every core test including `prd_fake_discovery_fake_driver` and `plan_twice_second_all_noop`. B: `bifrost config check` is deterministic on the §3/§13/§31 examples. C: `cargo test -p bifrost-mount -- --ignored` mounts, reads, kills and unmounts against the docker sshd. D: `uds_roundtrip`, `sse_frame_format`, `log_route_rejects_traversal` |
| **S2 = M1** | 3, 5, 6 | **E** `bifrost-daemon/src/{actor,state,main}.rs` (actor, executor, health, provider runner, adoption, warm-up, signals, socket, lock). **F** `bifrost-cli/src/{main,output,doctor}.rs` (every command). **G** `tests/e2e/{run.sh,config.tmpl.toml,p04,p05,p06,psec,p13a}` | **M1 gate:** `tests/e2e/run.sh m1` is green (PRD P4 mount/ls/unmount, P5 API, P6 recovery / degraded / offline / idempotent, the host-key negative, kill -9 adoption); the daemon's fake-driven tests pass; `bifrostd` with the §31 config produces `~/machines/agent-01/` |
| **S3** | 7, 8, 9, 10, 11 | **H** `discovery/tailscale.rs` + fixture + `p07`. **I** `discovery/dns.rs` + `tests/e2e/dns/*` + `p08`. **J** `mount/rclone.rs` (Linux mount, macOS mount and nfsmount argv, probes) + `p09`. **K** `discovery/http.rs` + `inventory.py` + `p10`. **L** `bifrost-tui/*`. The daemon is not touched. | Each crate's tests pass. `E2E_TAILSCALE=1 run.sh all` passes p07 (run by hand here). `run.sh all` passes p08–p10. The TUI TestBackend tests pass, and a manual TUI session against the E2E daemon covers every operation. The darwin check stays green. |
| **S4** | 12, 13 | **M** `daemon/reload.rs` (poller, debounce, SIGHUP, POST, provider diffing) + `p12`. **N** `p13_hardening.sh`, the macOS cfg audit plus darwin check, `README.md` (hero `brand/05_hero/hero_aurora_bridge_16x9.png`, systemd `KillMode=process`, `ssh-keyscan`, `SSH_AUTH_SOCK` notes), and a `ponytail:` comment audit against §15 | `run.sh all` green twice in a row; `check.sh` green; darwin check green; every §15 item has its `ponytail:` comment |

---

## 14. Top risks and how the design pre-empts them

| # | risk | pre-emption |
|---|---|---|
| 1 | A hung FUSE mount freezes the daemon | The actor does no I/O. Mount-table reads never touch FUSE. There is exactly one timed probe per path, with an in-flight guard. Every command has a timeout. After the grace period, a lazy detach frees the path without killing anything. |
| 2 | Data loss on unmount or remount | Graceful by default. Busy leads to backoff and Degraded, never automatic force. Force means lazy detach with **no kill**, so open files keep working. Mounting over a non-empty directory is refused. `remove_dir` only removes empty directories. state.json writes are atomic. |
| 3 | The startup race unmounts good adopted mounts | Warm-up requires one Ok from every provider (or the grace period). An invalid config exits instead of running empty. A missing config is empty, but then nothing was adopted from a config anyway. |
| 4 | Daemon death kills or corrupts children | `process_group(0)`, no `kill_on_drop`, and log output goes to a file rather than a pipe (no SIGPIPE). Adoption uses the fingerprint marker from the kernel mount table and is scoped to this daemon's root. No pid is ever signalled. |
| 5 | Remount storms | The fingerprint covers the selector text, so `auto` is sticky. Offline-but-Healthy is NoOp. Absent observations age out after `3 × interval`. A failing provider freezes. `failures` resets only on a periodic Healthy probe. Events fire on transitions only. |
| 6 | Fighting sshfs's own `reconnect` | Degraded or Unresponsive is left to `reconnect` and ServerAlive until the grace period. Bifröst acts immediately only on Stale (a dead process or ENOTCONN) or a spec change. |
| 7 | Untrusted discovery data escalates | Winner-takes-all by trust, per-observation filters, identity pinned to the DNS label, native ids never used as identity, the driver hint ignored, `honor_hints` off by default, validators both at the provider and in the typed spec, hostile E2E records. |
| 8 | rclone `--sftp-ssh` edge cases | Validated tokens and a quoted config path. `--sftp-shell-type=none` and `--sftp-disable-hashcheck`. `--sftp-host` passed defensively. The flag is feature-detected in the probe. p09 proves the whole path. |
| 9 | The SSH environment differs under a service manager | `ssh_agent` appears in status and doctor, and the README documents `systemctl --user import-environment SSH_AUTH_SOCK`. A host-key failure is surfaced with the preflight's exact stderr plus a README hint to run `ssh-keyscan`. Bifröst never falls back to weaker checking. |
| 10 | macOS can't be run here | OS code lives only in mount, config paths and `default_auto_order`. The argv builders take a `Flavor` parameter, so macOS argv is golden-tested on Linux. `getmntinfo` compiles in the darwin check. Known unknowns: nfsmount may need root; the macFUSE kext may not be approved (shows as Failed plus doctor ✗); whether FUSE-T honours `fsname` (state.json covers adoption). |
| 11 | Suspend and wake | `Instant` excludes suspend on Linux (CLOCK_MONOTONIC) and macOS (mach uptime), so grace timers don't count sleep and nothing is mass-unmounted. Tickers probe every mount within `health_interval` after wake. |
| 12 | Parallel agents break each other | S0 freezes manifests, the lockfile, signatures and daemon wiring. Worktrees are used per agent, with exclusive file ownership. Safe stubs keep M1 free of panics. E2E phases are separate files sourced by glob. |
| 13 | Unverified tool behaviour | The preflight exit code and the sshfs `fsname` override are verified in S1 (agent C) before anything depends on them, and each has a stated fallback. |

---

## 15. Deliberate simplifications

Each item becomes a `// ponytail: <ceiling>; <upgrade>` comment at the named location.

| # | simplification | ceiling | upgrade path | location |
|---|---|---|---|---|
| 1 | `BoxFuture` alias instead of async-trait; the `ctx` parameters from PRD §5 dropped; `inspect` returns `MountState` (errors fold into Degraded) | impls write `Box::pin(async move {..})` | add `DiscoveryContext`/`DriverContext` when a plugin needs runtime context | core/lib.rs |
| 2 | Static discovery is `registry.replace(config.static_observations())`, with no provider object or task | static machines can only come from config | implement `DiscoveryProvider` if static ever needs another source | daemon/actor.rs |
| 3 | Config reload polls the file every 2s (std) instead of using notify | up to ~4s latency; SIGHUP and the API are instant | notify 8.x if latency matters | daemon/reload.rs |
| 4 | Hand-rolled glob (`*`, `?`), CIDR, duration, `~`/`$VAR` expansion, FNV-1a, jitter and mountinfo parser | no character classes or brace globs; durations use a single unit | globset if rules need classes | core/validate.rs, mount/table.rs |
| 5 | SSH timings are constants: ConnectTimeout 10, ServerAlive 15×3 | a dead link takes ≥45s to detect; the command line overrides ssh_config values | an `[ssh]` section if users need tuning | mount/lib.rs `SSH_OPTS` |
| 6 | rclone `--dir-cache-time=15s` constant | a dead rclone remote can look healthy for ≤15s; directory listings are refetched every 15s | make it a key if listing traffic matters | mount/rclone.rs |
| 7 | Probe timeout fixed at 5s | slow links under load can read as Degraded (harmless: no action until grace) | a key if false Degraded reports annoy | mount/check.rs |
| 8 | HTTP: 10s timeout, 1 MiB body, 1000 entries, no ETag or Cache-Control | expensive inventories get polled in full every interval | ETag/If-None-Match | discovery/http.rs |
| 9 | Removal hysteresis is `3 × interval`, with no grace tombstones | a machine absent from successful refreshes for 3 intervals is unmounted (gracefully) | tombstones for `offline_grace_period` if inventories flap for longer | core/registry.rs |
| 10 | Same-kind provider tie-break is by provider name, not config order | two DNS providers reporting the same id: the alphabetically first wins | carry a config rank in `Source` | core/registry.rs |
| 11 | Winner-takes-all merge | a DNS/HTTP include can't mount an id that a more trusted source reports without allowing it | a per-id `prefer = "<provider>"` rule | core/policy.rs |
| 12 | No client SSE consumer; the TUI polls `/v1/status` every 1s | event latency ≤1s; debugging SSE needs `curl --unix-socket` | `Client::events()` plus a `bifrost events` command | client, tui |
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
| 27 | Missing config means the empty default config | a typo in `BIFROST_CONFIG` silently runs with no machines (logged at warn) | an opt-in `--require-config` | daemon/main.rs |

### Critical Files for Implementation
- /home/samimishal/projects/rust/bifrost/crates/bifrost-core/src/reconcile.rs
- /home/samimishal/projects/rust/bifrost/crates/bifrost-core/src/validate.rs
- /home/samimishal/projects/rust/bifrost/crates/bifrost-core/src/policy.rs
- /home/samimishal/projects/rust/bifrost/crates/bifrost-mount/src/lib.rs
- /home/samimishal/projects/rust/bifrost/crates/bifrost-daemon/src/actor.rs