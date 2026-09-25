# Bifröst — PRD & Implementation Plan

## 1. Product Summary

**Bifröst** is a cross-platform daemon that makes remote machines feel like extensions of the local filesystem.

It discovers eligible machines through configurable discovery providers, filters them through explicit policy, and mounts selected remote paths into a predictable local namespace using pluggable mount drivers.

The product is intentionally **not tied to Tailscale, SSHFS, rclone, or any single network/filesystem technology**.

The core architecture is:

```text
Discovery Providers
        ↓
Normalized Machine Registry
        ↓
Policy / Filtering
        ↓
Desired Mount Graph
        ↓
Mount Reconciler
        ↓
Mount Drivers
        ↓
Local Filesystem Namespace
```

Initial implementation language: **Rust**.

Primary deliverables:

- long-running daemon
- reusable core library
- CLI using the core library
- TUI using the core library
- modular discovery providers
- modular mount drivers
- deterministic reconciliation loop
- Linux + macOS support

---

# 2. Goals

## Primary goals

1. Make remote files feel local.

   Example:

   ```text
   ~/machines/
     agent-01/
     agent-02/
     build/
     staging/
   ```

   A user should be able to use:

   ```bash
   cd ~/machines/agent-01/home/sami/project
   nvim src/main.rs
   cp artifact.tar ~/machines/build/tmp/
   rg "foo" ~/machines/agent-02/workspace
   ```

2. Remove the normal friction of:

   - `scp`
   - `rsync host:path`
   - manually configured SFTP remotes
   - remembering addresses
   - manually mounting/unmounting machines
   - losing shell completion for remote paths

3. Support multiple discovery mechanisms.

4. Support multiple mounting mechanisms.

5. Treat discovery and mounting as independent subsystems.

6. Continuously reconcile actual state with desired state.

---

# 3. Non-goals

Bifröst V1 is **not**:

- a VPN
- a sync engine
- a distributed filesystem
- an rsync replacement
- a replacement for Tailscale
- a replacement for SSH
- a file versioning system
- a remote shell manager
- a cloud storage product
- a container orchestrator

Bifröst consumes connectivity that already exists.

---

# 4. Why Rust

Both Go and Rust are viable.

For Bifröst, Rust is preferred because the project benefits from:

- strong trait-based boundaries for discovery and mount drivers
- a single reusable core library shared by daemon, CLI, and TUI
- strong process/resource lifecycle handling
- good async/concurrency support through Tokio
- excellent TUI ecosystem through Ratatui/Crossterm
- straightforward static binaries
- stronger compile-time modeling of daemon state transitions

Go would be attractive if Bifröst were primarily a network service or if native filesystem implementation were the center of the product.

However, V1 should **not implement its own filesystem driver**. It should orchestrate mature external tools such as SSHFS and rclone. Therefore Go has no material advantage for the initial architecture.

### Important rule

Do **not** implement a native FUSE filesystem in V1.

The mount system is a process/lifecycle abstraction around established tools.

A native/private mount driver can be added later behind the same `MountDriver` interface.

---

# 5. Core Concepts

## Machine

A discovered or statically configured host.

```rust
pub struct Machine {
    pub id: MachineId,
    pub name: String,
    pub addresses: Vec<MachineAddress>,
    pub metadata: Metadata,
    pub source: DiscoverySource,
}
```

A machine may be reported by several discovery providers.

Those observations should be merged into a normalized machine record.

---

## Mount Specification

Describes what remote filesystem path should appear locally.

```rust
pub struct MountSpec {
    pub id: MountId,
    pub machine: MachineSelector,
    pub remote_path: RemotePath,
    pub local_path: PathBuf,
    pub driver: DriverSelector,
    pub options: MountOptions,
}
```

---

## Discovery Provider

Produces observations about machines.

Conceptually:

```rust
#[async_trait]
pub trait DiscoveryProvider: Send + Sync {
    fn name(&self) -> &str;

    async fn discover(
        &self,
        ctx: &DiscoveryContext,
    ) -> Result<Vec<MachineObservation>, DiscoveryError>;
}
```

Discovery providers **do not mount anything**.

---

## Mount Driver

Turns a resolved machine + mount specification into a mounted local filesystem.

Conceptually:

```rust
#[async_trait]
pub trait MountDriver: Send + Sync {
    fn name(&self) -> &str;

    async fn probe(
        &self,
        ctx: &DriverContext,
    ) -> DriverAvailability;

    async fn mount(
        &self,
        request: MountRequest,
    ) -> Result<MountHandle, MountError>;

    async fn inspect(
        &self,
        handle: &MountHandle,
    ) -> Result<MountState, MountError>;

    async fn unmount(
        &self,
        handle: MountHandle,
    ) -> Result<(), MountError>;
}
```

---

# 6. Discovery Architecture

V1 should support four discovery classes.

## 6.1 Static configuration

Example:

```toml
[[machines]]
name = "build"
host = "10.10.10.12"
user = "sami"

[[machines.mounts]]
remote = "/"
local = "build"
driver = "sshfs"
```

This provider should require no daemon-side network discovery.

---

## 6.2 Tailscale discovery

Use Tailscale's machine-readable local status interface.

The provider produces normalized observations containing:

- hostname
- DNS name
- Tailscale IPs
- online/offline state
- tags where available
- device identity metadata

The Tailscale provider is only a source of machine information.

It must not be coupled to any mount driver.

---

## 6.3 DNS TXT discovery

DNS TXT should be a first-class provider.

The intention is to support environments where machines publish themselves through DNS without requiring Tailscale.

Suggested format:

```dns
_bifrost.example.com TXT "v=bf1 host=agent-01.example.com user=sami path=/home/sami"
_bifrost.example.com TXT "v=bf1 host=agent-02.example.com user=sami path=/srv/work tags=dev,agent"
```

However, a single large TXT record becomes awkward quickly.

A better V1 format is index + per-node records:

```dns
_bifrost.example.com TXT "v=bf1 nodes=agent-01,agent-02"

_bifrost.agent-01.example.com TXT \
  "v=bf1 host=agent-01.example.com user=sami tags=dev,agent"

_bifrost.agent-02.example.com TXT \
  "v=bf1 host=agent-02.example.com user=ubuntu tags=dev"
```

Optional fields:

```text
v
host
port
user
tags
driver
path
id
```

### Important security behavior

DNS discovery is **discovery, not authorization**.

A TXT record must never automatically become trusted merely because it exists.

Every discovered machine still passes through Bifröst filtering / allow policy.

Optional DNSSEC validation can later be supported as an additional trust signal.

---

## 6.4 HTTP discovery provider

Generic endpoint:

```text
GET https://inventory.example.com/bifrost/v1/machines
```

Example response:

```json
{
  "machines": [
    {
      "id": "agent-01",
      "name": "agent-01",
      "addresses": ["10.20.0.4"],
      "metadata": {
        "tags": ["dev", "agent"]
      }
    }
  ]
}
```

This keeps Bifröst easy to integrate with:

- internal inventory
- cloud provisioning systems
- Launchbay
- Kubernetes
- custom infrastructure
- future Bedouin integrations

without making any of those systems dependencies.

---

# 7. Discovery Filtering

Auto-discovery must never imply auto-mount-everything.

Filtering happens after discovery and before reconciliation.

Example:

```toml
[[discovery]]
type = "tailscale"

[discovery.filter]
include_tags = ["dev", "agent"]
exclude_names = ["prod-*"]

[[discovery]]
type = "dns"
domain = "example.com"

[discovery.filter]
include_names = ["agent-*", "build-*"]
```

Supported filter primitives should include:

- exact machine ID
- hostname glob
- address / CIDR
- tag
- discovery provider
- metadata values
- explicit allow
- explicit deny

### Rule

**Deny wins over allow.**

If no allow filter exists:

- provider policy decides whether matching discovered machines are eligible

Recommended safe default for auto-discovery:

```text
discover = yes
mount = no
```

until an allow rule is configured.

---

# 8. Mount Drivers

## 8.1 SSHFS

Initial SSHFS driver.

Responsibilities:

- detect `sshfs`
- validate SSH connectivity
- resolve mount flags
- spawn mount
- track child/process state
- determine whether mount exists
- unmount cleanly
- recover from stale mount state

Example generated command:

```bash
sshfs \
  sami@agent-01:/home/sami \
  /Users/sami/.bifrost/machines/agent-01
```

Platform-specific options belong inside the driver.

---

## 8.2 rclone

Initial rclone driver.

Use rclone's SFTP backend.

Example conceptual invocation:

```bash
rclone mount \
  :sftp,host=agent-01,user=sami:/home/sami \
  ~/.bifrost/machines/agent-01
```

The rclone driver should support:

- SFTP remote
- VFS caching options
- daemon/foreground process handling
- mount health checks
- platform-specific mount mechanisms

On macOS, the driver should detect whether supported mounting is available.

Potential strategies:

```text
rclone nfsmount
rclone mount + FUSE-T
rclone mount + macFUSE
```

Do not hardcode one macOS mechanism into the core.

---

## 8.3 Future drivers

The architecture should allow:

```text
NFS
SMB
WebDAV
9P
native/private protocol
native FUSE implementation
cloud-backed mounts
custom enterprise driver
```

without touching discovery logic.

---

# 9. Driver Selection

A mount may specify:

```toml
driver = "sshfs"
```

or:

```toml
driver = "rclone"
```

or:

```toml
driver = "auto"
```

`auto` evaluates available drivers against the local system.

Suggested default preference:

### Linux

```text
sshfs
rclone
```

### macOS

```text
rclone nfsmount
rclone mount
sshfs
```

This should be configurable rather than permanently encoded as product policy.

---

# 10. Desired State Reconciliation

The daemon is fundamentally a reconciliation loop.

```text
Config
+
Discovery observations
        ↓
Normalized registry
        ↓
Policy evaluation
        ↓
Desired mounts
        ↓
Current mounts
        ↓
Diff
        ↓
Actions
```

Possible actions:

```text
Mount
Unmount
Remount
NoOp
Degraded
Waiting
```

Example:

```text
Desired:
agent-01 → mounted
agent-02 → mounted

Actual:
agent-01 → healthy
agent-02 → missing
old-agent → mounted

Actions:
agent-01 → NoOp
agent-02 → Mount
old-agent → Unmount
```

---

# 11. Reconciliation Rules

The daemon should be idempotent.

Repeated reconciliation with unchanged inputs must produce no side effects.

Suggested triggers:

- config change
- periodic discovery tick
- machine appeared
- machine disappeared
- driver process exited
- mount health check failed
- explicit CLI request
- daemon startup

Recommended initial periods:

```text
discovery refresh: 30s
mount health check: 15s
slow reconciliation fallback: 60s
```

Make them configurable.

---

# 12. Machine Availability

Remote disappearance must not destroy the daemon.

State model:

```text
Unknown
Discovered
Eligible
Connecting
Mounted
Degraded
Offline
Unmounting
Failed
```

A temporarily offline machine should enter:

```text
Mounted → Degraded → Offline
```

Depending on driver behavior, Bifröst may:

- retain mount and retry
- lazily remount
- unmount after configured timeout

Configuration example:

```toml
[reconciliation]
offline_grace_period = "5m"
retry_initial = "2s"
retry_max = "1m"
```

Use exponential backoff with jitter.

---

# 13. Configuration

Default locations:

### Linux

```text
~/.config/bifrost/config.toml
```

### macOS

```text
~/.config/bifrost/config.toml
```

Keep one cross-platform location initially.

Example:

```toml
version = 1

[mount]
root = "~/machines"
default_driver = "auto"

[daemon]
reconcile_interval = "30s"

[[discovery]]
type = "tailscale"

[discovery.filter]
include_tags = ["dev"]

[[discovery]]
type = "dns"
domain = "infra.example.com"

[discovery.filter]
include_names = ["agent-*"]

[[machines]]
name = "build"
host = "10.0.0.18"
user = "sami"

[[machines.mounts]]
remote = "/home/sami"
local = "build"
driver = "sshfs"
```

---

# 14. Core Crate Layout

Suggested Cargo workspace:

```text
bifrost/
├── Cargo.toml
├── crates/
│   ├── bifrost-core/
│   ├── bifrost-config/
│   ├── bifrost-discovery/
│   ├── bifrost-discovery-static/
│   ├── bifrost-discovery-tailscale/
│   ├── bifrost-discovery-dns/
│   ├── bifrost-discovery-http/
│   ├── bifrost-mount/
│   ├── bifrost-mount-sshfs/
│   ├── bifrost-mount-rclone/
│   ├── bifrost-daemon/
│   ├── bifrost-client/
│   ├── bifrost-cli/
│   └── bifrost-tui/
└── docs/
```

Do not over-fragment on day one.

A practical initial workspace can combine plugin implementations:

```text
bifrost-core
bifrost-config
bifrost-discovery
bifrost-mount
bifrost-daemon
bifrost-client
bifrost-cli
bifrost-tui
```

Split providers into separate crates only when their dependencies or release boundaries justify it.

---

# 15. Core Library

`bifrost-core` should contain only domain models and orchestration abstractions.

It should not know about:

- Tailscale CLI
- DNS resolver implementation
- SSHFS arguments
- rclone arguments
- terminal UI
- Unix sockets

Suggested modules:

```text
machine
discovery
policy
mount
reconcile
events
state
error
```

---

# 16. Daemon

Binary:

```text
bifrostd
```

Responsibilities:

- load config
- start discovery providers
- maintain machine registry
- run policy evaluation
- compute desired mounts
- reconcile mounts
- supervise driver processes
- expose local API
- emit structured events
- persist lightweight runtime state

---

# 17. Local Daemon API

CLI and TUI should not directly manipulate mounts while the daemon is running.

They should call the daemon.

Recommended transport:

### Unix

Unix domain socket:

```text
$XDG_RUNTIME_DIR/bifrost/bifrost.sock
```

### macOS

```text
~/Library/Caches/bifrost/bifrost.sock
```

Protocol can initially be:

```text
HTTP over Unix socket
```

or a lightweight framed JSON protocol.

HTTP over UDS is preferable because it keeps debugging trivial.

Possible API:

```text
GET  /v1/status
GET  /v1/machines
GET  /v1/mounts
GET  /v1/drivers
POST /v1/reconcile
POST /v1/mounts/:id/mount
POST /v1/mounts/:id/unmount
GET  /v1/events
```

Event streaming:

```text
SSE
```

is enough for V1.

No need for WebSockets initially.

---

# 18. CLI

Binary:

```text
bifrost
```

Suggested commands:

```bash
bifrost status

bifrost machines
bifrost machines list
bifrost machines show agent-01

bifrost mounts
bifrost mount agent-01
bifrost unmount agent-01

bifrost discover
bifrost reconcile

bifrost drivers
bifrost doctor

bifrost config check
bifrost config reload

bifrost daemon status
```

Useful UX:

```text
$ bifrost machines

NAME       SOURCE       ADDRESS       STATE       MOUNTED
agent-01   tailscale    100.80.1.4    online      yes
agent-02   dns          10.0.20.8     online      yes
build      static       10.0.0.18     online      no
```

---

# 19. TUI

Use:

```text
ratatui
crossterm
```

Suggested sections:

```text
Overview
Machines
Mounts
Discovery
Drivers
Events
Logs
```

Machine view:

```text
┌ Machines ──────────────────────────────────────┐
│ ● agent-01  tailscale   mounted    sshfs       │
│ ● agent-02  dns         mounted    rclone      │
│ ○ build     static      offline                │
└────────────────────────────────────────────────┘
```

Actions:

```text
m  mount
u  unmount
r  reconcile
d  details
l  logs
/  filter
```

---

# 20. Observability

Use structured tracing from the beginning.

Rust stack:

```text
tracing
tracing-subscriber
```

Every reconciliation action should carry fields such as:

```text
machine_id
mount_id
provider
driver
local_path
remote_path
attempt
```

Useful daemon events:

```text
MachineDiscovered
MachineLost
MachineEligible
MountRequested
MountStarted
MountHealthy
MountFailed
UnmountStarted
UnmountComplete
DriverUnavailable
ConfigurationReloaded
```

---

# 21. State Persistence

Do not make Bifröst depend on a database initially.

Persist only information needed for safe recovery.

Suggested runtime state:

```text
~/.local/state/bifrost/state.json
```

Potential contents:

```json
{
  "mounts": {
    "agent-01-home": {
      "driver": "sshfs",
      "local_path": "/Users/sami/machines/agent-01",
      "pid": 12345
    }
  }
}
```

On startup, never trust state blindly.

Inspect the actual operating system mount table before deciding what exists.

---

# 22. Mount Detection

The daemon must distinguish:

```text
directory exists
```

from:

```text
filesystem is actually mounted
```

Use platform mount information rather than path existence.

Linux:

```text
/proc/self/mountinfo
```

macOS:

```text
getmntinfo / mount table APIs
```

This should live behind a platform abstraction.

---

# 23. Security

Principles:

1. Discovery does not imply trust.
2. Bifröst does not store SSH passwords.
3. Prefer existing SSH agent / key / Tailscale SSH configuration.
4. Do not silently weaken host verification.
5. Mount only explicit paths permitted by policy.
6. Do not execute arbitrary commands returned by discovery providers.
7. Treat TXT / HTTP discovery metadata as untrusted input.
8. Validate local mount paths against traversal and collisions.

---

# 24. Driver Capability Detection

Each mount driver implements a probe.

Example:

```text
$ bifrost doctor

Discovery
  ✓ static
  ✓ tailscale
  ✓ dns
  ✓ http

Mount Drivers
  ✓ sshfs        /opt/homebrew/bin/sshfs
  ✓ rclone       /usr/local/bin/rclone
  ✓ rclone-nfs
  ✓ macFUSE

Selected default
  rclone-nfs
```

Probe results should be accessible through both CLI and TUI.

---

# 25. Plugin Model

## V1

Compile plugins into the Bifröst binary.

Trait-based modules provide sufficient modularity.

Avoid dynamic shared-library plugins initially.

Reasons:

- dramatically simpler compatibility story
- no Rust ABI stability problem
- easier distribution
- easier security model
- easier testing

## Future

If external plugins are needed, prefer a process-based plugin protocol.

Example:

```text
bifrost-plugin-foo
```

communicating over:

```text
stdin/stdout JSON RPC
```

or a local socket.

That preserves language independence and avoids Rust ABI coupling.

---

# 26. DNS TXT Discovery Specification

Proposed initial convention.

## Discovery root

```text
_bifrost.<domain>
```

Example:

```text
_bifrost.dev.example.com
```

Root record:

```dns
_bifrost.dev.example.com TXT "v=bf1 nodes=agent-01,agent-02"
```

Node records:

```dns
_bifrost.agent-01.dev.example.com TXT \
  "v=bf1 host=agent-01.dev.example.com user=sami tags=dev,agent"

_bifrost.agent-02.dev.example.com TXT \
  "v=bf1 host=agent-02.dev.example.com user=ubuntu tags=dev"
```

Optional mount hint:

```dns
_bifrost.agent-01.dev.example.com TXT \
  "v=bf1 host=agent-01.dev.example.com user=sami path=/home/sami driver=auto tags=dev"
```

### Parser rules

- unknown fields ignored
- `v` required
- invalid records skipped, not fatal
- duplicate machines merged
- TTL respected
- expiration removes observation, not necessarily the machine immediately
- machine disappears only after provider observation timeout / reconciliation policy

---

# 27. Config Reload

Daemon should watch the configuration file.

Flow:

```text
filesystem event
    ↓
parse candidate config
    ↓
validate
    ↓
calculate diff
    ↓
atomically replace active config
    ↓
trigger reconciliation
```

Invalid new config must **not** destroy the currently working configuration.

---

# 28. Testing Strategy

## Unit tests

Test:

- config parsing
- selectors
- allow/deny precedence
- machine merging
- driver selection
- reconciliation planning
- DNS TXT parsing
- retry behavior
- mount state transitions

## Integration tests

Create fake discovery providers and fake mount drivers.

Example:

```text
FakeDiscovery
  returns A + B

FakeDriver
  reports A mounted
  reports B absent

Expected planner:
  A → NoOp
  B → Mount
```

## Real integration tests

CI / dedicated runner:

- OpenSSH container
- SSHFS mount
- rclone SFTP mount
- DNS test server

macOS mount testing should run on actual macOS CI rather than assuming Linux behavior.

---

# 29. Implementation Plan

## Phase 0 — Repository skeleton

Deliver:

```text
Cargo workspace
CI
rustfmt
clippy
test harness
release profile
```

Create binaries:

```text
bifrost
bifrostd
bifrost-tui
```

---

## Phase 1 — Domain core

Implement:

```text
Machine
MachineObservation
MachineRegistry
MountSpec
MountState
DiscoveryProvider
MountDriver
Policy
ReconcilePlan
```

No external commands yet.

Acceptance:

- full reconciliation planner works entirely with fake providers/drivers

---

## Phase 2 — Configuration

Implement TOML configuration.

Deliver:

```text
config parser
schema validation
defaults
environment expansion
path expansion
config check CLI
```

Acceptance:

```bash
bifrost config check
```

returns deterministic validation output.

---

## Phase 3 — Static discovery

Implement static machine provider.

Acceptance:

- configured hosts appear in normalized registry
- filtering works
- duplicate machine identity handling works

---

## Phase 4 — SSHFS driver

Implement:

```text
probe
mount
inspect
unmount
process supervision
```

Linux first.

Then macOS/macFUSE.

Acceptance:

```bash
bifrost mount devbox
ls ~/machines/devbox
bifrost unmount devbox
```

---

## Phase 5 — Daemon + local API

Implement daemon lifecycle and Unix socket API.

Acceptance:

```bash
bifrostd
bifrost status
bifrost machines
bifrost mounts
```

CLI no longer owns mounting lifecycle.

---

## Phase 6 — Reconciliation loop

Implement:

```text
desired state
actual state
planner
executor
retry/backoff
health checks
offline handling
```

Acceptance:

- killing SSHFS causes recovery
- disappearing target causes degraded state
- reconnecting target restores mount
- repeated reconciliation is idempotent

---

## Phase 7 — Tailscale discovery

Implement local Tailscale provider.

Acceptance:

- reachable tailnet peers discovered
- metadata normalized
- filters enforced
- new peers can appear without daemon restart

---

## Phase 8 — DNS TXT discovery

Implement TXT provider and `bf1` parser.

Acceptance:

- nodes discovered from TXT records
- TTL changes respected
- malformed nodes isolated
- machine filters enforced
- DNS discovery alone never bypasses policy

---

## Phase 9 — rclone driver

Implement:

```text
SFTP mount
mount inspection
unmount
process supervision
VFS options
```

Then macOS variants:

```text
nfsmount
FUSE-T
macFUSE
```

Acceptance:

- same MountDriver contract works across SSHFS and rclone
- driver can be selected per mount
- auto-selection works

---

## Phase 10 — HTTP discovery

Implement generic JSON discovery endpoint.

Acceptance:

- arbitrary inventory service can publish nodes
- authentication headers configurable
- cache / refresh policy works

---

## Phase 11 — TUI

Implement Ratatui application.

Views:

```text
Overview
Machines
Mounts
Discovery
Drivers
Events
Logs
```

Acceptance:

- all primary operations available without shell commands
- TUI communicates only through daemon API

---

## Phase 12 — Configuration hot reload

Implement file watching + transactional reload.

Acceptance:

- valid changes trigger reconciliation
- invalid config keeps previous state active

---

## Phase 13 — Hardening

Focus:

```text
stale mounts
daemon crashes
orphan driver processes
machine rename
duplicate discovery
IP changes
sleep/wake
network transitions
laptop suspend
DNS expiration
Tailscale reconnect
macOS permission failures
```

---

# 30. Suggested Rust Dependencies

Keep dependency count conservative.

Likely:

```toml
tokio
async-trait
serde
serde_json
toml
thiserror
tracing
tracing-subscriber
clap
ratatui
crossterm
reqwest
trust-dns-resolver / hickory-resolver
notify
globset
nix
```

Prefer **Hickory DNS** for the DNS discovery implementation if its current API fits the requirements.

Do not add native FUSE crates unless implementing an actual native driver.

---

# 31. First Milestone

The first genuinely useful milestone should be deliberately small:

```text
Linux/macOS
     ↓
static config
     ↓
SSHFS
     ↓
daemon
     ↓
reconciliation
     ↓
CLI
```

Example:

```toml
[mount]
root = "~/machines"

[[machines]]
name = "agent-01"
host = "agent-01"
user = "sami"
remote = "/home/sami"
```

Then:

```bash
bifrostd
```

produces:

```text
~/machines/agent-01/
```

This proves the entire architecture before adding discovery complexity.

---

# 32. Second Milestone

Add:

```text
Tailscale discovery
DNS TXT discovery
filters
rclone driver
TUI
```

At that point Bifröst becomes the intended product:

> Remote machines discovered from multiple environments and presented as a coherent local filesystem namespace.

---

# 33. Design Principles

The implementation should continuously enforce these rules:

1. **Discovery and mounting are separate.**
2. **Discovery never equals authorization.**
3. **Drivers never know how machines were discovered.**
4. **Providers never know how filesystems are mounted.**
5. **The daemon owns lifecycle.**
6. **CLI and TUI are clients of the daemon.**
7. **Desired state is declarative.**
8. **Reconciliation is idempotent.**
9. **External tools are replaceable.**
10. **The core library must remain independent of SSHFS, rclone, Tailscale, and DNS.**
11. **A future private/native mount protocol must fit without redesigning the product.**
12. **An external discovery plugin must fit without redesigning the product.**

---

# 34. Brand Direction

Selected direction:

**Bifröst**

Visual concept:

```text
ice / glass bridge
+
aurora illumination
+
dark Nordic sky
+
cold cyan / blue / green spectral light
```

Avoid a literal rainbow.

The bridge should feel:

- crystalline
- translucent
- engineered
- slightly mythic
- modern rather than fantasy-heavy

The first generated brand direction is the preferred visual reference.

Possible product language:

```text
Remote worlds. Local files.

Different systems. Same filesystem.

A bridge between machines.

Make distance disappear.

Far systems. Near.
```

The brand metaphor maps naturally to the architecture:

```text
realm       → machine / environment
bridge      → mounted filesystem
aurora      → discovery / connectivity
gateway     → Bifröst daemon
```

---

# 35. Definition of Done for V1

Bifröst V1 is complete when:

- Linux and macOS are supported
- daemon survives disconnect/reconnect cycles
- static discovery works
- Tailscale discovery works
- DNS TXT discovery works
- HTTP discovery works
- allow/deny filtering works
- SSHFS driver works
- rclone driver works
- automatic driver probing works
- mounted machines appear under one predictable root
- CLI manages and inspects daemon state
- TUI manages and inspects daemon state
- configuration hot reload works
- stale mounts are recoverable
- machine disappearance does not destabilize the daemon
- discovery providers and mount drivers remain independently replaceable
