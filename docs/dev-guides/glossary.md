# Glossary

Every domain term used in the code, the design documents and these guides, with a one-line definition and a
link to where it is explained. Terms are grouped by area; within a group they follow the order in which they
build on each other. Identifiers in `code` font are the names used in the source. The last section decodes the
tags that appear in code comments (`A15`, `B8`, `§5`, `row 9`, `S4a`, `ponytail:`), which have no meaning
without the design history.

## Contents

- [Identity and names](#identity)
- [Discovery and trust](#discovery)
- [DNS bf1 records](#bf1)
- [Policy](#policy)
- [Planning](#planning)
- [Mount runtime](#runtime)
- [Drivers and the mount table](#drivers)
- [Availability states](#availability)
- [Daemon, API and state](#daemon)
- [Configuration](#configuration)
- [Tags in code comments and design documents](#tags)

<a id="identity"></a>
## Identity and names

| Term | Meaning | See |
|---|---|---|
| `Name` | The validated identity type: ASCII-lowercased, then `^[a-z0-9][a-z0-9._-]{0,62}$`. It is an identity key and a single path component at once (never `""`, `.`, `..`, `-x`, and never contains `/`) | [decisions.md#validated-newtypes-at-trust-boundary](decisions.md#validated-newtypes-at-trust-boundary), [crates/core.md](crates/core.md) |
| machine id (`MachineId`) | A `Name` that identifies a machine across providers; observations with equal ids are merged. Tailscale: first label of `DNSName`, else `HostName`; DNS: the node label; HTTP: `name`, else `id`; static: `name` | [architecture.md#data-model](architecture.md#data-model) |
| mount id (`MountId`) | A `Name` that identifies one mount and its directory `<root>/<id>`. A discovered machine's single mount uses the machine id; a static mount uses its `local` | [architecture.md#data-model](architecture.md#data-model) |
| local | The mount id of a static machine's mount (`[[machines.mounts]] local`); optional when the machine has one mount (it is then the machine name) | [crates/config.md](crates/config.md) |
| native id | A provider's own identifier (Tailscale node `ID`, bf1 `id=`, HTTP `id`), `^[A-Za-z0-9._:-]{1,128}$`. Never an identity; matched only by the owning provider's `include_ids`/`exclude_ids` (A19) | [decisions.md#policy-semantics](decisions.md#policy-semantics) |
| display name | `MachineObservation.name`: the provider's name for the machine, `clean`ed to 128 characters, shown only | [crates/core.md](crates/core.md) |
| `Host` | A validated connect target: a canonical IP literal (v4-mapped v6 stored as v4, never unspecified) or a hostname of `[A-Za-z0-9_-]` labels; never starts with `-`, stored lowercase | [decisions.md#validated-newtypes-at-trust-boundary](decisions.md#validated-newtypes-at-trust-boundary) |
| `User` | A validated ssh login name, `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}$` | [crates/core.md](crates/core.md) |
| `RemotePath` | A validated remote path: `~`, `~/rel`, `/` or `/abs`; at most 1024 bytes, no control characters, no `:`, no `..` component | [crates/core.md](crates/core.md) |
| sftp path | `RemotePath::sftp_path`: `~` → `""` (the login directory), `~/rel` → `rel`, `/abs` unchanged | [crates/core.md](crates/core.md) |
| `clean` | `validate::clean(s, max)`: replaces control, bidi and zero-width characters with `?` and truncates; 128 for names, 256 for metadata values, 512 for errors and log lines | [security.md](security.md) |
| `tail` | `validate::tail(s, max)`: the last lines of a log or stderr that fit in `max` characters, skipping blank lines and the `LOG_HEADER` line, joined with ` \| ` | [crates/core.md](crates/core.md) |

<a id="discovery"></a>
## Discovery and trust

| Term | Meaning | See |
|---|---|---|
| machine | A remote host known under one machine id. In code, `registry::Machine`: the id, every current observation in trust order, the selected one and the verdict | [architecture.md#data-model](architecture.md#data-model) |
| observation (`MachineObservation`) | One provider's validated report of one machine: addresses (`[0]` is the connect target), port, `online`, metadata, hints, TTL | [architecture.md#data-model](architecture.md#data-model) |
| provider | A discovery source: a `DiscoveryProvider` (tailscale, dns, http) running in its own task, or the built-in static source. It has a unique name (default: its kind) and a kind | [crates/discovery.md](crates/discovery.md) |
| provider kind | `static`, `tailscale`, `http` or `dns`; the kind decides the trust rank | [crates/config.md](crates/config.md) |
| static machine | A `[[machines]]` entry in the config. Its observations are replaced on every config apply, never expire and have trust 0, which allows them unless a global deny matches | [decisions.md#static-provider-is-config](decisions.md#static-provider-is-config) |
| `Source` | `{trust, kind, provider}` attached to each stored observation; its ordering is the trust order (then kind, then provider name) | [crates/core.md](crates/core.md) |
| trust rank | `bifrost_config::TRUST`: static 0, tailscale 1, http 2, dns 3; lower is more trusted. Core only compares the numbers | [decisions.md#winner-takes-all-trust](decisions.md#winner-takes-all-trust) |
| winner takes all | Of a machine's observations that its own provider filter does not exclude, the most trusted one is selected; its address, port, hints and metadata are the only ones used, and policy is evaluated against it alone | [decisions.md#winner-takes-all-trust](decisions.md#winner-takes-all-trust) |
| selected observation | `Machine::obs()` / `Machine::source()`: the winner | [architecture.md#pipeline](architecture.md#pipeline) |
| shadowed | Providers whose observations of a machine lost to the selected one (`MachineDto.shadowed`) | [crates/core.md](crates/core.md) |
| registry | `MachineRegistry`: per machine id, one observation and expiry per `Source` | [architecture.md#pipeline](architecture.md#pipeline) |
| complete view | A provider's `Ok` is everything it currently sees. A machine missing from it is not removed; it ages out | [crates/discovery.md](crates/discovery.md) |
| expiry, removal hysteresis | An observation lives until `now + max(ttl, 3 × interval)`; a machine absent from successful refreshes disappears after that | [architecture.md#steady-state](architecture.md#steady-state) |
| freeze, failing provider | After an `Err` (or a failed build), `mark_failed` stops that provider's observations from expiring until its next `Ok` | [architecture.md#steady-state](architecture.md#steady-state) |
| refresh | One `discover()` call of a provider task; counted in `ProviderDto.refreshes` | [architecture.md#process-model](architecture.md#process-model) |
| reported | A provider that has returned a non-empty `Ok` at least once (A18); every network provider must have reported, or the grace period must pass, for `ready` | [architecture.md#warm-up](architecture.md#warm-up) |
| `task_gen` | Per-provider generation carried by `Msg::Discovery`; a result from an aborted or replaced provider task is dropped | [decisions.md#generations-for-stale-results](decisions.md#generations-for-stale-results) |
| hints (`MountHints`) | A record's suggested `user` and `path`, validated like config values and used only when the provider's template sets `honor_hints`. There is no driver hint (E2) | [crates/core.md](crates/core.md) |
| `honor_hints` | Provider template switch, default false: when true, record hints override the template's user and remote | [crates/config.md](crates/config.md) |
| template (`MountTemplate`) | A provider's `[discovery.mount]`: user, remote (default `~`), driver, read_only, honor_hints; it builds the one mount of every allowed machine that provider supplies | [crates/config.md](crates/config.md) |
| MagicDNS | Tailscale's DNS names; when enabled, the tailscale provider connects to the peer's DNS name instead of its first IPv4 | [decisions.md#tailscale-via-cli-json](decisions.md#tailscale-via-cli-json) |
| sharee node | A Tailscale peer listed only because we shared a node with its (other) owner (`ShareeNode: true`); skipped silently, never ours to mount | [crates/discovery.md](crates/discovery.md) |
| HTTP inventory | The JSON document `{"machines": [...]}` fetched by an `http` provider | [decisions.md#http-inventory-limits](decisions.md#http-inventory-limits) |

<a id="bf1"></a>
## DNS bf1 records

| Term | Meaning | See |
|---|---|---|
| bf1 | Bifröst's TXT record format: a value starting with the token `v=bf1`, then `key=value` tokens separated by spaces; at most 2048 bytes; unknown keys ignored; a duplicate key invalidates the value | [decisions.md#bf1-dns-format-inline-and-index](decisions.md#bf1-dns-format-inline-and-index) |
| root record | The TXT RRset at `_bifrost.<domain>.`: inline node records and index values, mixed freely | [crates/discovery.md](crates/discovery.md) |
| inline node record | A root value carrying `node=<label>`: a whole machine in one value, identity = the label (v0.1.1) | [decisions.md#bf1-dns-format-inline-and-index](decisions.md#bf1-dns-format-inline-and-index) |
| index value | A root value carrying `nodes=<label>,<label>…`: labels whose own records are looked up | [crates/discovery.md](crates/discovery.md) |
| node record | The bf1 value at `_bifrost.<label>.<domain>.` for a label listed in the index | [crates/discovery.md](crates/discovery.md) |
| node label (`dns_label`) | `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`, no dots (so an index can't point queries at another domain); it is the machine id | [crates/discovery.md](crates/discovery.md) |
| ambiguous node | A label with two distinct bf1 values, or both inline and in `nodes=`; skipped | [crates/discovery.md](crates/discovery.md) |
| whole-node rejection | Any invalid key in a node's record skips the whole node; attacker data is never partially applied | [security.md](security.md) |
| `MAX_NODES` | 256: the cap on inline plus index labels read from one root RRset | [crates/discovery.md](crates/discovery.md) |
| TCP fallback | A truncated UDP answer (TC bit) is retried over TCP by hickory, on explicit and system resolvers alike (test `system_resolver_has_tcp_fallback`) | [decisions.md#dns-tcp-fallback](decisions.md#dns-tcp-fallback) |

<a id="policy"></a>
## Policy

| Term | Meaning | See |
|---|---|---|
| policy | `[policy.allow]`, `[policy.deny]` and every provider's `[discovery.filter]`, evaluated by `policy::evaluate` | [decisions.md#policy-semantics](decisions.md#policy-semantics) |
| verdict | `Allowed{by}`, `DiscoverOnly` or `Denied{by}`; displayed as e.g. `allowed (tailscale.filter.include)`, `discover-only`, `denied (policy.deny tags=prod)` | [crates/core.md](crates/core.md) |
| Allowed | The machine's mounts become candidates | [architecture.md#pipeline](architecture.md#pipeline) |
| discover-only (`DiscoverOnly`) | The default for a non-static machine that no rule allows: listed, never mounted; `POST …/mount` returns 403 with the verdict | [decisions.md#discover-is-not-mount](decisions.md#discover-is-not-mount) |
| Denied | A global deny matched the selected observation, or every observation was excluded by its own provider's filter | [decisions.md#policy-semantics](decisions.md#policy-semantics) |
| allow, deny | Global matchers. Allow (and include) needs every non-empty primitive kind to match (AND), any entry within a kind (OR); deny (and exclude) fires on any single primitive. Deny wins, static included | [decisions.md#policy-semantics](decisions.md#policy-semantics) |
| provider filter | A provider's `include_*` / `exclude_*` keys. An exclude drops only that provider's observation; an include allows the machine when that provider's observation is selected | [decisions.md#policy-semantics](decisions.md#policy-semantics) |
| own filter | The filter of the provider that made the observation; the only place where `ids` also match the `native_id` | [crates/core.md](crates/core.md) |
| primitive | One matcher kind: `ids`, `names` (globs with `*` and `?`), `cidrs`, `tags`, `providers` (global only), `metadata` | [crates/core.md](crates/core.md) |
| fail closed (cidrs) | In an exclude or deny, a cidr entry also matches a non-static observation that has no IP-literal address (hostnames are never resolved) | [crates/core.md](crates/core.md) |

<a id="planning"></a>
## Planning

| Term | Meaning | See |
|---|---|---|
| pass | One `Actor::pass`: expire, verdicts, desired, plan, execute side effects, persist | [architecture.md#message-flow](architecture.md#message-flow) |
| desired | A mount is desired when it is a candidate and not held; `reconcile::desired` computes the candidates | [architecture.md#pipeline](architecture.md#pipeline) |
| candidate (`Candidate`) | `{spec, driver, held, online}` for one mount id of an allowed machine, recomputed every pass | [architecture.md#data-model](architecture.md#data-model) |
| hold, held | A persisted manual unmount (`bifrost unmount`): the mount stays a candidate but is not desired until `bifrost mount`. Holds are the authoritative part of state.json | [architecture.md#local-api](architecture.md#local-api) |
| conflict | A discovered machine whose id equals a static mount's local: skipped and listed in `StatusDto.conflicts` | [crates/core.md](crates/core.md) |
| driver selector (`DriverSelector`) | `auto` or a driver name. The selector text, not the chosen driver, is fingerprinted, so `auto` is sticky across probe flaps | [crates/core.md](crates/core.md) |
| `auto_order` | `mount.auto_order`: driver preference for `auto` (Linux `sshfs, rclone`; macOS `rclone-nfs, rclone, sshfs`) | [architecture.md#platforms](architecture.md#platforms) |
| `select_driver` | A named driver is chosen only if it probed Available (never substituted); `auto` takes the first Available in `auto_order` | [crates/core.md](crates/core.md) |
| auto driver | `StatusDto.auto_driver`: `select_driver(auto)` over the active config and probes; the default shown by `status`, `doctor`, `drivers` and the TUI | [crates/cli.md](crates/cli.md) |
| plan, `decide` | The pure planner: `plan` runs `decide` for every id in candidates ∪ runtimes, sorted | [decisions.md#pure-planner-decision-table](decisions.md#pure-planner-decision-table) |
| action (`Action`) | `NoOp`, `Mount`, `Unmount`, `Remount`, `Degraded`, `Waiting`; only `Mount`, `Unmount` and `Remount` have side effects (`Action::has_side_effect`) | [architecture.md#decision-table](architecture.md#decision-table) |
| row N | A row of the 14-row decision table in `decide`; code comments end with the row number | [architecture.md#decision-table](architecture.md#decision-table) |
| ⊳U | The unmount-backoff gate: while `unmount_retry_at` is in the future, rows 3/5 wait and rows 10–12 show `Degraded(last_error)` | [architecture.md#decision-table](architecture.md#decision-table) |
| `WaitReason` | `InFlight`, `WarmingUp`, `NoDriver`, `MachineOffline`, `Backoff(remaining)` | [crates/core.md](crates/core.md) |
| `Reason` | Why an unmount runs: `NotDesired`, `Manual`, `Stale`, `SpecChanged`, `OfflineGrace` | [crates/core.md](crates/core.md) |
| remount | An unmount now; the mount follows through row 9 on a later pass, so offline and backoff gates still apply | [architecture.md#mount-phase](architecture.md#mount-phase) |
| spec change | The handle's fingerprint differs from the candidate's `spec.fingerprint()` (a host, port, user, path, driver selector or read-only change) | [decisions.md#adoption-by-marker-and-fingerprint](decisions.md#adoption-by-marker-and-fingerprint) |
| change pending | Row 13's `Degraded("change pending: …")` for a working mount whose spec changed but can't be remounted yet (driver unavailable, machine offline, warming up) | [architecture.md#decision-table](architecture.md#decision-table) |
| idempotent | A pass with unchanged inputs has no side effects (tests `plan_twice_second_all_noop`, `plan_is_deterministic`) | [decisions.md#pure-planner-decision-table](decisions.md#pure-planner-decision-table) |
| deadline, `next_wakeup` | The earliest future instant at which a row could act (a retry, a grace expiry, the end of warm-up); the actor sleeps until it | [architecture.md#steady-state](architecture.md#steady-state) |
| warm-up, `ready` | After a start, row 4 holds back row 5's removal of an un-held, not-desired mount, and spec-change remounts (row 11) are gated, until a config has loaded and every network provider has reported, or the grace period has passed; `ready` then latches | [decisions.md#warm-up-readiness](decisions.md#warm-up-readiness) |
| `cfg_loaded_at` | The first successful config load (not the empty default of a missing file); starts the warm-up grace clock | [architecture.md#warm-up](architecture.md#warm-up) |
| grace (`offline_grace_period`) | Default 5 m. The longest warm-up, and how long a mount may stay Degraded before row 12 lazily detaches it | [architecture.md#availability](architecture.md#availability) |
| backoff | `validate::backoff`: base = `min(retry_max, retry_initial · 2^(failures − 1))`, result uniformly in `[base/2, base]` ("equal jitter") | [architecture.md#decision-table](architecture.md#decision-table) |
| `failures` | Consecutive failures of a mount id (mount and unmount share it); reset only by a Healthy probe after the mount has been up for `retry_max` with no unmount retry pending, or by `POST …/mount` | [architecture.md#health](architecture.md#health) |
| `mount_retry_at`, `unmount_retry_at` | The instants before which row 8 (mount) or ⊳U (unmount) waits | [architecture.md#decision-table](architecture.md#decision-table) |

<a id="runtime"></a>
## Mount runtime

| Term | Meaning | See |
|---|---|---|
| `MountSpec` | Everything needed to mount: id, machine, host, port, user, remote, `local_path`, driver selector, read-only. Built only in `reconcile::desired` | [architecture.md#data-model](architecture.md#data-model) |
| fingerprint | `MountSpec::fingerprint()`: 16 lowercase hex of FNV-1a 64 over every spec field; its encoding must never change (it lives in kernel mount tables and state.json; test `fingerprint_stable_vector`) | [decisions.md#adoption-by-marker-and-fingerprint](decisions.md#adoption-by-marker-and-fingerprint) |
| marker | `bifrost:<id>@<fp16>`: sshfs `fsname=` or rclone `--devname=`, visible as the mount's source in the kernel mount table | [decisions.md#adoption-by-marker-and-fingerprint](decisions.md#adoption-by-marker-and-fingerprint) |
| spec source | `MountSpec::source()`: `[user@]host:sftp_path`, IPv6 in brackets; shown as `MountDto.remote` | [crates/core.md](crates/core.md) |
| `MountHandle` | What a mount returns: id, driver name, local path, fingerprint, pid (informational, never signalled). Persisted in state.json | [architecture.md#data-model](architecture.md#data-model) |
| `MountRuntime` | The actor's per-mount-id record: phase, handle, health, timers, generation, failures, flags | [architecture.md#data-model](architecture.md#data-model) |
| phase | `Absent`, `Mounting`, `Mounted`, `Unmounting` | [architecture.md#mount-phase](architecture.md#mount-phase) |
| generation | Incremented by `MountRuntime::begin` for every mount and unmount; a result whose generation or phase doesn't match is ignored | [decisions.md#generations-for-stale-results](decisions.md#generations-for-stale-results) |
| in flight | Phase `Mounting` or `Unmounting`: row 1 waits | [architecture.md#decision-table](architecture.md#decision-table) |
| health | `Unknown` (not inspected since mount or adoption), `Healthy`, `Degraded(r)`, `Stale(r)` | [architecture.md#health](architecture.md#health) |
| `MountState` | An inspect result: `Missing`, `Healthy`, `Degraded(r)`, `Stale(r)` | [architecture.md#health](architecture.md#health) |
| Missing | No entry at the path in the mount table (on Linux: none carrying our marker): the runtime goes Absent with backoff | [architecture.md#health](architecture.md#health) |
| Stale | The mount returns `ENOTCONN`: its process is gone. Rows 3 (not desired) and 10 (desired, then remounted) lazily detach it without waiting for the grace period | [architecture.md#availability](architecture.md#availability) |
| Degraded (health) | The mount is there but the probe failed or timed out; left to sshfs `reconnect` and ServerAlive until the grace period | [architecture.md#availability](architecture.md#availability) |
| `degraded_since` | When the current Degraded stretch began; row 12 compares it with the grace period | [architecture.md#decision-table](architecture.md#decision-table) |
| offline flag | `MountRuntime.offline`: set by an `OfflineGrace` unmount, cleared by a successful mount or by `POST …/mount` (`bifrost mount`); shows the mount as Offline meanwhile | [architecture.md#availability](architecture.md#availability) |
| `force_requested` | Set by `bifrost unmount --force`; row 3 then lazily detaches even a busy mount | [architecture.md#local-api](architecture.md#local-api) |
| `probing` | An inspect is in flight for this runtime; at most one at a time | [architecture.md#process-model](architecture.md#process-model) |
| adoption, adopted | Taking over, at startup, a marker mount already under the root (or on macOS one recorded in state.json) instead of remounting it; `MountDto.adopted` | [architecture.md#adoption](architecture.md#adoption) |
| foreign mount | Anything mounted at `<root>/<id>` that is not ours (no marker for that id); never touched, and mounting there is `Refused("occupied by …")` | [architecture.md#adoption](architecture.md#adoption) |
| executor task | A task that runs one driver `mount` or `unmount` under an outer timeout and reports back with `Msg::MountDone`/`UnmountDone` | [architecture.md#process-model](architecture.md#process-model) |
| supervisor | The task inside `mount_with` that owns a spawned child, reaps it and calls `on_exit`; adopted mounts have none | [architecture.md#process-model](architecture.md#process-model) |
| `on_exit`, child exit | The callback in `MountRequest`, called once with an exit description; it sends `Msg::ChildExited`, which triggers an immediate inspect | [architecture.md#message-flow](architecture.md#message-flow) |

<a id="drivers"></a>
## Drivers and the mount table

| Term | Meaning | See |
|---|---|---|
| driver | A `MountDriver`: `sshfs`, `rclone` (FUSE `rclone mount`) or `rclone-nfs` (`rclone nfsmount`, macOS only) | [crates/mount.md](crates/mount.md) |
| probe | A driver's availability check (binaries, version text, FUSE); run at startup, on every config apply, on the fallback tick and on `POST /v1/reconcile`. `probing` in status until the first answer | [crates/mount.md](crates/mount.md) |
| `DriverAvailability` | `Available{binary, detail}` or `Unavailable(reason)` | [crates/core.md](crates/core.md) |
| flavour (`Flavor`) | `Linux`, `MacFuse` or `FuseT`: selects the platform-specific sshfs and rclone options | [architecture.md#platforms](architecture.md#platforms) |
| `DriverSettings` | `ssh_config`, `vfs_cache_mode`, `mount_timeout`, `state_dir`; the drivers are rebuilt when these change | [architecture.md#reload](architecture.md#reload) |
| preflight | `ssh … -s -- <host> sftp` with stdin `/dev/null`, run before every spawn: exit 0 means host key, auth and the sftp subsystem work; otherwise its stderr becomes the error | [decisions.md#ssh-preflight](decisions.md#ssh-preflight) |
| `SSH_OPTS` | The connection options Bifröst passes to ssh: `BatchMode=yes`, `ConnectTimeout=10`, `ServerAliveInterval=15`, `ServerAliveCountMax=3`, `ControlMaster=no`, `ControlPath=none`. With `SSH_CLI_HARDENING` they are the only ssh options it ever passes; nothing about host keys | [decisions.md#host-keys-never-weakened](decisions.md#host-keys-never-weakened) |
| `SSH_CLI_HARDENING` | `-a -x -o ClearAllForwardings=yes -o PermitLocalCommand=no`, first in the preflight and `--sftp-ssh` (sshfs adds its own equivalent) | [security.md](security.md) |
| step 2 (`Step2`) | `mount()`'s check of what is already at the path: `Free`; `Adopt` (our marker, same fingerprint); `Detach` (our marker, older spec: lazy detach first); `Refused` (foreign) | [crates/mount.md](crates/mount.md) |
| `prepare_mountpoint` | Creates `<root>/<id>` (0700) or accepts an empty directory; refuses a symlink, a non-directory or a non-empty directory. Runs only when the table says the path is not a mountpoint | [crates/mount.md](crates/mount.md) |
| readiness | After spawning, polling the mount table every 100 ms until our entry appears (`Ok`), the child exits or `mount_timeout` passes | [crates/mount.md](crates/mount.md) |
| child log | `<state>/logs/<id>.log`: the child's stdout and stderr (never a pipe), truncated at every spawn; its tail becomes the error on failure | [architecture.md#filesystem-layout](architecture.md#filesystem-layout) |
| `LOG_HEADER` | `# bifrost exec: `, the first line of every child log, followed by the argv; `tail` skips it | [crates/core.md](crates/core.md) |
| mount table | The kernel's list of mounts (`/proc/self/mountinfo` on Linux, `getmntinfo` on macOS). The only truth about what is mounted; reading it never touches a FUSE filesystem | [decisions.md#mount-table-not-path-existence](decisions.md#mount-table-not-path-existence) |
| liveness probe | `check::liveness`: a timed `symlink_metadata` of `<mount>/.bifrost-probe-<16 hex nonce>`, a name that exists nowhere, so no cache can answer | [architecture.md#health](architecture.md#health) |
| `check::timed`, in-flight key | Runs a blocking filesystem call on the blocking pool under a timeout; the key (`<path>/.pid-<pid>`, the mount instance) allows one call in flight, so a hung mount leaks at most one thread | [decisions.md#hung-fuse-guards](decisions.md#hung-fuse-guards) |
| hung FUSE | A FUSE mount whose server stopped answering: any filesystem call on it blocks. The guards keep the daemon from ever blocking on one | [decisions.md#hung-fuse-guards](decisions.md#hung-fuse-guards) |
| graceful unmount | `fusermount3 -u` (Linux) or `/sbin/umount` (macOS); a busy mount stays mounted and returns `Busy` | [decisions.md#busy-unmount-never-forced](decisions.md#busy-unmount-never-forced) |
| lazy detach, force unmount | `fusermount3 -u -z` or `diskutil unmount force`: the path is freed now, open files keep working, the child exits when the last reference closes. Never a kill | [decisions.md#no-pid-signalling-lazy-detach](decisions.md#no-pid-signalling-lazy-detach) |
| busy | A graceful unmount blocked by open files; always the string `unmount blocked: busy (files open)` (C4); retried with backoff, never forced automatically | [decisions.md#busy-unmount-never-forced](decisions.md#busy-unmount-never-forced) |
| `MountError` | `Unavailable` (no driver binary or FUSE), `Busy`, `Refused` (occupied path, symlink, invalid request), `Failed` (preflight, child log tail, timeout) | [crates/core.md](crates/core.md) |
| VFS cache | rclone's local write cache: `--vfs-cache-mode` from config (default `writes`), `--cache-dir=<state>/rclone/<id>` per mount (A23) | [crates/mount.md](crates/mount.md) |

<a id="availability"></a>
## Availability states

The PRD §12 states (`reconcile::Availability`), derived for display, listed in severity order. Rules:
[architecture.md#availability](architecture.md#availability).

| State | Mount | Machine |
|---|---|---|
| Unknown | adopted, mounted and health `Healthy` or `Unknown`, with no candidate for this mount id (typically its machine is not in the registry or not allowed yet) | — |
| Discovered | — | its verdict is not Allowed (discover-only or denied) |
| Eligible | not mounted and nothing wrong: waiting, held, or not desired; also before the first driver probe answers (`probing drivers`) | allowed, and no mount is in a worse state (or it has no mounts) |
| Mounted | mounted, health Healthy or not yet inspected | worst of its mounts |
| Connecting | a mount is in flight | worst of its mounts |
| Unmounting | an unmount is in flight | worst of its mounts |
| Offline | not mounted because the machine is offline, or lazily detached after the grace period and waiting to retry | worst of its mounts |
| Degraded | mounted but the probe fails (Degraded or Stale health) | worst of its mounts |
| Failed | desired but not mounted: no usable driver, or the last attempt failed | worst of its mounts |

<a id="daemon"></a>
## Daemon, API and state

| Term | Meaning | See |
|---|---|---|
| `bifrostd` | The daemon binary (crate `bifrost-daemon`); configured through `BIFROST_CONFIG`, `BIFROST_SOCKET`, `BIFROST_STATE_DIR`, `BIFROST_LOG` (with `XDG_STATE_HOME`, `XDG_RUNTIME_DIR` and `HOME` for the defaults; config values may also expand `$NAME`) | [crates/daemon.md](crates/daemon.md) |
| actor | The one task that owns all daemon state and never awaits I/O; everything else messages it | [decisions.md#single-actor-daemon](decisions.md#single-actor-daemon) |
| `Msg` | The actor's inbox message type (`Discovery`, `MountDone`, `UnmountDone`, `Health`, `ChildExited`, `Probed`, `Config`, `Api`, `Tick`, `Shutdown`) | [architecture.md#message-flow](architecture.md#message-flow) |
| tick (`Tick`) | `Health` (every `health_interval`: inspect mounted runtimes) or `Fallback` (every `reconcile_interval`: rebuild failed providers, re-probe drivers) | [architecture.md#steady-state](architecture.md#steady-state) |
| `Deps` | The daemon's two factories, for drivers and providers; tests inject fakes through them | [crates/daemon.md](crates/daemon.md) |
| snapshot | The `StatusDto` the actor publishes on a `watch` channel after every pass; the GET routes only read it | [architecture.md#local-api](architecture.md#local-api) |
| event, event ring | An `Event` emitted on a state change (PRD §20 list plus `MountDegraded`), numbered by `seq`; the last 200 are kept in the snapshot | [architecture.md#process-model](architecture.md#process-model) |
| SSE | `GET /v1/events`: server-sent events from the broadcast channel; `event: lagged` when a subscriber fell behind | [decisions.md#sse-events-and-polling-clients](decisions.md#sse-events-and-polling-clients) |
| target | The `{target}` of `POST /v1/mounts/{target}/mount` or `…/unmount`: a mount id, or a machine id meaning all its mounts; a mount id wins | [architecture.md#local-api](architecture.md#local-api) |
| `ApiCmd` | The API's request to the actor (`Mount`, `Unmount`, `Reconcile`, `Discover`); `Mount`, `Unmount` and `Reconcile` carry a oneshot reply, `Discover` has none (the route answers 202 at once, E4) | [crates/daemon.md](crates/daemon.md) |
| 403 | `POST …/mount` on something that is not a candidate: the body carries the verdict; manual mount never overrides policy | [decisions.md#discover-is-not-mount](decisions.md#discover-is-not-mount) |
| 503 | The actor's inbox or reply channel is closed: "bifrostd is shutting down" | [architecture.md#local-api](architecture.md#local-api) |
| state.json | `<state>/state.json`: holds (authoritative) and mount handles (adoption hints), written atomically | [decisions.md#state-json-is-a-hint](decisions.md#state-json-is-a-hint) |
| quarantine | An unreadable or unparseable state.json is renamed `state.json.corrupt-<unix>` and the daemon starts empty, never overwriting it | [architecture.md#filesystem-layout](architecture.md#filesystem-layout) |
| lock | `<state>/bifrostd.lock`, held with `File::try_lock`: one daemon per state dir, released by the kernel on a crash | [architecture.md#startup](architecture.md#startup) |
| private directory | The startup check: owned by the lock's uid and not world-writable (`mode & 0o002 == 0`); applied to the state dir, `logs`, a pre-existing socket parent and the mount root | [security.md](security.md) |
| mount root, canonical root | `mount.root` (default `~/machines`), canonicalized once at startup; every mount is `<canonical root>/<mount id>` | [architecture.md#filesystem-layout](architecture.md#filesystem-layout) |
| doctor | `bifrost doctor`: config, daemon, ssh and agent, discovery, drivers, FUSE and the selected default, with or without a running daemon | [crates/cli.md](crates/cli.md) |
| exit codes | CLI: 0 ok, 1 operation failed, 2 usage, 3 daemon not reachable. `bifrostd`: 0 clean shutdown, 1 startup failure, 2 usage or invalid config | [decisions.md#cli-exit-codes](decisions.md#cli-exit-codes) |

<a id="configuration"></a>
## Configuration

| Term | Meaning | See |
|---|---|---|
| config apply | `Actor::apply`, the one path that installs a config, at startup and on every reload | [architecture.md#reload](architecture.md#reload) |
| empty default config | What the daemon runs when the config file is missing or 0 bytes: no machines, no providers, root `~/machines`; it does not count as loaded, so warm-up never ends on it | [architecture.md#startup](architecture.md#startup) |
| config poller | The `config-poller` thread that reads the file every 2 s and sends settled changes | [decisions.md#config-polling-not-notify](decisions.md#config-polling-not-notify) |
| debounce | The poller acts on new bytes only when two consecutive reads are identical (a half-written save is never parsed) | [architecture.md#reload](architecture.md#reload) |
| SIGHUP | Wakes the poller for an immediate reload that skips the debounce | [architecture.md#reload](architecture.md#reload) |
| `ConfigError` | `error: <path>: <message>`; `path` is a key path (`machines[0].mounts[1].local`) for semantic errors, `<file>:<line>:<col>` for TOML errors. All semantic errors are returned, sorted | [decisions.md#config-deterministic-validation](decisions.md#config-deterministic-validation) |
| `config_errors` | `StatusDto.config_errors`: non-empty means the daemon rejected the file and runs on the previous config | [architecture.md#reload](architecture.md#reload) |
| expansion | `~`, `~/…`, `$NAME`, `${NAME}` and `$$` in `mount.root`, `mount.ssh_config`, HTTP `url` and header values; an undefined or empty variable is an error | [crates/config.md](crates/config.md) |
| `Secret` | An HTTP header value; its `Debug` prints `***` and errors never echo it | [security.md](security.md) |
| timings | `daemon.*` intervals and `mount_timeout`, `reconciliation.*` grace and retry; all durations ≥ 1 s | [crates/config.md](crates/config.md) |

<a id="tags"></a>
## Tags in code comments and design documents

| Tag | Meaning | Where to look |
|---|---|---|
| `A1`–`A23` | Amendments for defects found in the contract (e.g. A15: `failures` reset only after a stable mount) | docs/design/critique.md §A; contract.md "Amendments (applied)" |
| `B1`–`B15` | Amendments for behaviour that had no home in the contract (e.g. B8: core is provider- and driver-agnostic) | critique §B; the same table |
| `C1`–`C8` | Amendments for inconsistencies (e.g. C4: the single busy string) | critique §C |
| `D1` | Amendment for a wrong API claim (the hickory options lines) | critique §D |
| `E1`–`E6` | Cuts made for simplicity (e.g. E2: no driver hint anywhere) | critique §E |
| `P1`, `P2` | Orchestrator additions (rustfmt defaults; the shared `CARGO_TARGET_DIR`) | [decisions.md#shared-target-dir-and-worktrees](decisions.md#shared-target-dir-and-worktrees) |
| `§N` | A section of docs/design/contract.md (§4 policy, §5 reconciliation, §6 drivers, §7 discovery, §8 daemon, §15 simplifications). The contract has only §1–§15, so a bare `§N` above 15 in code or tests (e.g. `§31` in crates/bifrost-config/src/lib.rs) is a PRD section | docs/design/contract.md |
| `§15 #N` | Deliberate simplification number N | [simplifications.md](simplifications.md) |
| `PRD §N` | A section of bifrost_prd_and_implementation_plan.md (§12 availability, §20 events, §23 security) | the PRD |
| `S0`–`S4`, `S4a` | `S0`–`S4` are the build stages of contract §13; `S1-C` is stage 1, agent C. `S4a` is not a §13 stage: it is S4's first gate (agents M and N done), before the final review | contract.md §13 and "Orchestrator sign-offs"; [decisions.md#shared-target-dir-and-worktrees](decisions.md#shared-target-dir-and-worktrees) |
| `M1` | Milestone 1 (stage S2): the first E2E gate, `tests/e2e/run.sh m1` | [e2e-harness.md](e2e-harness.md) |
| `r1`–`r3` | Final whole-repo review rounds; commits `fix(review-rN): …` and sign-offs "Final review rN" | `git log` |
| sign-off | A numbered, binding decision in contract.md's "Orchestrator sign-offs (after S<k>)" sections; it wins over earlier contract text. Forms: "sign-off S4a.11" or short `S1.4` = item 11 (4) of the list after S4a (S1); "S3 sign-off 2a" = item 2, sub-point (a), of the list after S3; a bare "Sign-off 2" in code = the list after S1 | [architecture.md#contract-vs-code](architecture.md#contract-vs-code) |
| `row N`, `⊳U` | The decision table and its unmount-backoff gate | [architecture.md#decision-table](architecture.md#decision-table) |
| `pNN_…`, `psec` | E2E phase files in `tests/e2e/` (e.g. `p04_sshfs.sh`, `psec_hostkey.sh`) | [e2e-harness.md](e2e-harness.md) |
| frozen (A3) | A type or signature fixed in stage S0 so parallel agents could build against it (`Msg`, `Tick`, `ApiCmd`, `Deps`, `AppState`, `actor::spawn`) | [crates/daemon.md](crates/daemon.md) |
| `ponytail:` | A deliberate simplification comment that names its ceiling and upgrade path | [decisions.md#ponytail-style](decisions.md#ponytail-style), [simplifications.md](simplifications.md) |
