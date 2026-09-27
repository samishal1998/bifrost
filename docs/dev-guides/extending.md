# Extending Bifröst

This guide is a set of recipes for the changes a maintainer is most likely to make: a new discovery provider, a new
mount driver, a config key, an API route with its CLI command and TUI key, an event, an E2E phase, a change to the
bf1 DNS record format, and a change to the mount fingerprint. Each recipe lists the exact files to touch in order,
the tests to add or update, the rules the existing code relies on, and the docs to update. The code is organised so
that most extensions don't touch `bifrost-core` at all: provider kinds, trust ranks and driver names live in
`bifrost-config` (B8). Read [architecture.md](architecture.md) first for how the pieces fit, and
[testing.md](testing.md) for the test layers.

## Contents

- [Before you start](#before)
- [Add a discovery provider](#provider)
- [Add a mount driver](#driver)
- [Add a config key](#config-key)
- [Add an API route, a CLI command and a TUI key](#api-cli-tui)
- [Add an event](#event)
- [Add an E2E phase](#e2e-phase)
- [Change the bf1 grammar safely](#bf1)
- [Change the fingerprint](#fingerprint)

<a id="before"></a>
## Before you start

- **Tests first.** Name the tests that pin the new behaviour, write them, watch them fail, then write the code. The
  whole codebase was built this way ([testing.md](testing.md#conventions)).
- **The gate.** `scripts/check.sh` (fmt, clippy `-D warnings`, all tests) must pass; behaviour that crosses processes
  also needs `tests/e2e/run.sh all`, which CI doesn't run.
- **Trust boundaries.** Anything that comes from discovery, the network, the API or a file other users can write goes
  through a validating newtype or function (`Name`, `Host`, `User`, `RemotePath`; `tag`, `meta_key`, `native_id`, all
  in `crates/bifrost-core/src/validate.rs`) before it is stored, and through `clean(s, max)` before it is displayed
  ([decisions.md#validated-newtypes-at-trust-boundary](decisions.md#validated-newtypes-at-trust-boundary)).
- **The house style.** The laziest thing that works, with every deliberate corner recorded as a `// ponytail:`
  comment naming its ceiling and upgrade path ([decisions.md#ponytail-style](decisions.md#ponytail-style)). New
  dependencies need a reason; check whether an existing crate or a few lines of std already do it.
- **Frozen shapes are history.** `Msg`, `Tick`, `ApiCmd`, `Deps` and `AppState` were frozen in S0 so parallel agents
  could build against them (A3). That constraint ended with the build; they are now ordinary internal types. Change
  them deliberately and update every `match` on them, including the test harnesses (`stub_actor` in
  `crates/bifrost-daemon/src/api.rs`, the `Rig` in `actor.rs`).

<a id="provider"></a>
## Add a discovery provider

A provider turns some source (a CLI, an API, DNS) into `MachineObservation`s. It never decides whether a machine is
mounted: that is policy's job ([decisions.md#discover-is-not-mount](decisions.md#discover-is-not-mount)).

### 1. Choose the kind and its trust rank

The kind is a lowercase string (`"tailscale"`, `"dns"`, `"http"`); it becomes the default provider name and the
`type` in config. The trust rank decides which observation wins when two providers report the same machine id:
lower is more trusted, and the winner's host, port, hints and metadata are the only ones used
([decisions.md#winner-takes-all-trust](decisions.md#winner-takes-all-trust)). Current ranks: static 0, tailscale 1,
http 2, dns 3. Rank by how much you trust the *address* the source reports.

**Never give a network provider rank 0.** `policy::evaluate` treats `trust == 0` as static configuration: allowed
without any allow rule, and exempt from the CIDR fail-closed check (`crates/bifrost-core/src/policy.rs`). Ties within
a rank are broken by kind, then provider name (`Source`'s `Ord` in `crates/bifrost-core/src/registry.rs`).

### 2. Config (`crates/bifrost-config`)

| File | Change |
|---|---|
| `src/lib.rs` `TRUST` | add `("<kind>", N)`; the array length is part of its type |
| `src/lib.rs` `ProviderSpec` | a variant holding the provider's **validated** settings (`Host`, `SocketAddr`, `Secret`, …) |
| `src/raw.rs` `RawDiscovery` | the new keys as `Option<…>` (collections with `#[serde(default)]`). The struct is flat on purpose, not a tagged enum, so an unknown key still reports `line:col` |
| `src/lib.rs` `V::config` | (a) the `matches!(kind.as_str(), "tailscale" \| "dns" \| "http")` check and its message "expected tailscale, dns or http"; (b) the per-type key table `for (key, set, only) in [...]`, so the new keys are rejected on other types and other types' keys on yours; (c) an explicit arm in `let spec = match kind.as_str()` that validates the keys (`"required for type = …"` for required ones) |

Pitfall: the spec `match` ends in `_ => Some(ProviderSpec::Tailscale)`. Without an explicit arm, the new kind passes
the type check and silently becomes a Tailscale provider.

Credentials: store them as `Secret` (its `Debug` prints `***`), expand them with `self.expand` (whose errors never
echo the input), and never put a value in an error message (see the `headers` handling for `http`).

Tests in `src/lib.rs` `mod tests`:

- `trust_ranks_and_sources` asserts `TRUST` verbatim: update it.
- `per_type_keys_enforced` uses `type = "consul"` as its unknown type; extend it with your keys, and pick another
  unknown name if your kind is `consul`.
- New: a parse test for a valid block (`ok(…)` and the resulting `ProviderSpec`), and `assert_has` tests for each
  rejection.

`policy.allow.providers`/`policy.deny.providers` accept any kind in `TRUST`, so they pick the new kind up without a
change.

### 3. The provider (`crates/bifrost-discovery`)

Add `src/<kind>.rs` and `pub mod <kind>;` in `src/lib.rs`:

```rust
pub struct FooProvider { name: String, /* validated settings */ }

impl FooProvider {
    /// Err(String) = can't be built; it becomes ProviderDto.last_error, so it must never contain a secret.
    pub fn new(name: String, /* … */) -> Result<Self, String> { /* … */ }
}

impl DiscoveryProvider for FooProvider {
    fn name(&self) -> &str { &self.name }
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>> {
        Box::pin(async move { /* … */ })
    }
}
```

The contract (`crates/bifrost-core/src/lib.rs`, `DiscoveryProvider`) and what the daemon does with each result:

| Return | Meaning | Daemon behaviour |
|---|---|---|
| `Ok(vec)` | the complete current view; invalid records were skipped with `tracing::warn!` | observations replace this provider's previous ones; a machine missing from later views ages out once its expiry passes: `max(ttl, 3 × interval)` after it was last seen (removal hysteresis) |
| `Ok(vec![])` | looked, found nothing | as above, and it does **not** count toward warm-up readiness (A18) |
| `Err(Unavailable(..))` | the source isn't there (binary missing, backend stopped) | the provider's observations are **frozen** (never expire) until the next `Ok`; `last_error` is set |
| `Err(Failed(..))` | could not look (transport, protocol, timeout, unusable response) | same as above |

A single bad record is never an `Err`: skipping it keeps the rest of the view, and an `Err` would freeze stale data.

Rules the existing providers follow (read `tailscale.rs`, `dns.rs`, `http.rs` side by side):

- **Validate every field** from the source with the core validators: `Name::parse` for the id (it lowercases),
  `Host::parse` for addresses (rejects option injection like `-oProxyCommand=…`), `User::parse`, `RemotePath::parse`,
  `tag`, `meta_key`, `native_id`; `clean(s, 128)` for display names and `clean(v, 256)` for metadata values. An
  entry with any invalid field is skipped whole, never half-applied.
- **Identity is the `id`** (a `Name`, which is also the directory name under the root). A source's own identifier
  goes in `native_id`, which only this provider's `include_ids`/`exclude_ids` can match (A19).
- **Hints** (`MountHints { user, path }`) are used only when the provider's template has `honor_hints = true`. There
  is no driver hint (E2).
- `online` is `Some` only when the source really knows; `ttl` only when the source has one (the registry floors it at
  3 × interval).
- **Bound everything**: a timeout inside `discover()` (the daemon also wraps each call in 30 s), a size cap on
  anything read (HTTP: 1 MiB and 1000 entries; tailscale: 16 MiB of stdout), a count cap on records.
- **Binary lookup** must search `$PATH` plus `/usr/local/bin:/usr/bin:/bin` (and `/opt/homebrew/bin` on macOS),
  because systemd and launchd start the daemon with a minimal `PATH`, and must take the path as a parameter so tests
  never call `set_var` (see `which_in` in `tailscale.rs`; discovery can't depend on `bifrost-mount`'s copy).
- **Logging**: warn with the provider name and a `clean`ed reason; never log a credential. Other crates' logs are
  capped at warn by the daemon's filter ([decisions.md#log-filter-allowlist](decisions.md#log-filter-allowlist)).
- **Panics** are caught by the daemon (`caught()` in `actor.rs`) and become `Failed`, but don't rely on it.

Tests in the module: parsing from a fixture file (`crates/bifrost-discovery/tests/fixtures/`), hostile inputs (an option as the host, a
traversal as the name, oversized values), the caps, the `Unavailable`/`Failed` mapping, and transport against a fake
binary in a temp dir or a `TcpListener` on `127.0.0.1:0` (see `serve()` in `http.rs` tests). Add an `#[ignore]` test
for a real source only if one can be stood up locally.

### 4. The daemon (`crates/bifrost-daemon/src/main.rs`)

Add the arm to `build_provider`:

```rust
ProviderSpec::Foo { .. } => Arc::new(FooProvider::new(pc.name.clone(), /* … */)?),
```

That is all. The actor diffs providers by name on every config apply, spawns `discover_loop` (discover first, then
wait for the interval or a `POST /v1/discover`), freezes on `Err`, retries a failed build on the next reload and on
every fallback tick, and reports the provider in `StatusDto.providers` with `kind` as a plain string. `bifrost-core`,
the CLI, `doctor` and the TUI need no change: they show whatever `ProviderDto` says.

### 5. Around it

- A new dependency goes in the workspace `Cargo.toml` and the crate's. If it compiles C, the release musl builds need
  it to work with `musl-gcc` ([release-and-ci.md](release-and-ci.md#matrix)).
- E2E: a phase with a local fixture for the source, a hostile record, and a check that nothing is mounted without a
  filter or allow rule ([Add an E2E phase](#e2e-phase)).
- Docs: `site/src/content/docs/guides/discovery.mdx`, `reference/configuration.mdx`, an example in
  `examples/discovery.mdx`; the README's provider section; [crates/discovery.md](crates/discovery.md) and
  [crates/config.md](crates/config.md); a decision in [decisions.md](decisions.md) for the trust rank you chose.

<a id="driver"></a>
## Add a mount driver

A driver mounts a `MountSpec` at `<root>/<id>` and supervises the child. The shared code in
`crates/bifrost-mount/src/lib.rs` does most of the work; a driver mostly contributes an argv and a probe.

### 1. Config (`crates/bifrost-config/src/lib.rs`)

- Add the name to `DRIVER_NAMES` (array length in the type). `driver = "<name>"`, `default_driver` and `auto_order`
  are validated against it.
- Decide whether it belongs in `default_auto_order()` for each OS. A driver that is right only for some setups should
  be selectable by name but not in the default order.
- Update `trust_ranks_and_sources`, which asserts `DRIVER_NAMES` and `default_auto_order()` verbatim.

`bifrost-core` checks only the grammar of a driver name (`DriverSelector: TryFrom<String>`), so it needs no change.

### 2. The driver (`crates/bifrost-mount/src/<name>.rs`)

Model it on `sshfs.rs` (one binary) or `rclone.rs` (two drivers from one binary):

| Piece | Rule |
|---|---|
| `pub struct FooDriver { s: DriverSettings }` + `new` | settings come from the daemon; `DriverSettings` derives `PartialEq`, and the actor rebuilds drivers only when it changes |
| `name()` | exactly the `DRIVER_NAMES` entry |
| `probe()` | `Box::pin(async { probe_with(&check::search_path()).await })` |
| `probe_with(path: &OsStr)` | finds binaries with `check::which_in(name, path)` only; runs the tool's version command with `check::run` (5 s) (`sshfs --version`, `rclone version`); feature-detects any flag the argv depends on; checks the FUSE helper and `/dev/fuse` (Linux) or `flavor()` (macOS); returns `Unavailable("<why>")` or `Available { binary, detail }`. It must look only at binaries and flags, never at `self.s`: `bifrost doctor` probes with placeholder settings (S2 sign-off 3). An OS the driver doesn't support probes `Unavailable("<os> only")`, as `rclone-nfs` does on Linux |
| `mount(req)` | `check::which` for the binary and `ssh`, `flavor()`, build the argv with a pure function, then `crate::mount_with(self.name(), &bin, &ssh, argv, req, &self.s)` |
| `inspect(h)` | `Box::pin(crate::inspect_path(h))` |
| `unmount(h, force)` | `Box::pin(crate::unmount_path(&h.local_path, force))` |
| `pub fn foo_argv(spec, …, f: Flavor) -> Vec<OsString>` | **pure**, so it can be golden-tested for every `Flavor` on Linux |

What `mount_with` requires of the tool:

- **It runs in the foreground** and stays the mount's process: the supervisor task owns it, reaps it and calls
  `on_exit` ([decisions.md#sshfs-foreground-child](decisions.md#sshfs-foreground-child)). A tool that daemonizes
  can't be supervised.
- **It lets you set the mount's source name** to `marker(&spec.id, &spec.fingerprint())`
  (`bifrost:<id>@<fp16>`): sshfs uses `-o fsname=`, rclone `--devname=`. On Linux, readiness and `inspect_path`
  compare the mountinfo source with it exactly, and adoption after a restart depends on it
  ([decisions.md#adoption-by-marker-and-fingerprint](decisions.md#adoption-by-marker-and-fingerprint)).
- **SSH goes through the system `ssh`**, with `SSH_OPTS`, `SSH_CLI_HARDENING` where the tool invokes ssh itself, and
  `-F <ssh_config>` when configured. Never a host-key option, never a tool's built-in SSH implementation
  ([decisions.md#host-keys-never-weakened](decisions.md#host-keys-never-weakened),
  [decisions.md#rclone-via-system-ssh](decisions.md#rclone-via-system-ssh)). `mount_with` runs the ssh preflight
  before spawning, so a host-key or auth failure is reported with ssh's own words.
- **Every argv token is data.** `Command::args`, never `sh -c`; option values are separate validated tokens (`-o <opts>`, `-p <port>`,
  `-F <path>` in sshfs) or `--flag=value` (rclone), never a string a shell parses; positionals last and never
  starting with `-` (the host grammar and the absolute local path guarantee it).

Wire it in `src/lib.rs`: `mod <name>;`, `pub use <name>::{FooDriver, foo_argv};`, and add it to `drivers(s)` (every
OS gets every driver). In `adopt()`, map the tool's fstype to the driver name (`"fuse.sshfs" => "sshfs"`, …);
otherwise an adopted mount is attributed from `state.json` or guessed as `"sshfs"`.

If the driver needs a new setting, add it to `DriverSettings`, fill it in the actor's `apply`
(`crates/bifrost-daemon/src/actor.rs`) and in `doctor::local_drivers`' placeholder, and follow
[Add a config key](#config-key).

### 3. Tests

| Test | Where |
|---|---|
| `foo_argv_*_golden` for each `Flavor`, read-only, port, IPv6 host, `ssh_config` | `<name>.rs` |
| add the new argv to the loops of `argv_never_weakens_host_keys` (the `FORBIDDEN` list), `positionals_never_start_with_dash`, and `ssh_never_forwards_agent_x11_or_ports` if it passes an ssh command line | `src/lib.rs` tests |
| `probe_fake_foo_in_path` (fake scripts in a temp dir) and a missing-binary case | `<name>.rs` |
| a case for the fstype in `adopt_marker_record_foreign_outside_root` | `src/lib.rs` tests |
| `#[ignore]` `foo_mount_…` against the docker sshd: mount, marker in mountinfo, read/write, busy unmount, kill -9 by `h.pid` | `<name>.rs`, using `e2e()`, `req()`, `tidy()` |

### 4. Around it

- Nothing to add in the daemon (it builds drivers through `bifrost_mount::drivers`), in `doctor` (it lists
  `bifrost_mount::drivers` and prints each probe) or in the TUI. `doctor`'s FUSE line is FUSE-specific; add a line
  only if the driver needs a different kernel facility.
- `install.sh`'s runtime-dependency report: add the tool with its package names.
- E2E: a phase like p09 (a static machine with `driver = "<name>"`, round trip, kill -9 recovery, a host-key
  negative).
- Docs: `site/src/content/docs/guides/mount-drivers.mdx`, `installation.mdx` (runtime tools),
  `reference/configuration.mdx` (driver names); README; [crates/mount.md](crates/mount.md).

<a id="config-key"></a>
## Add a config key

1. **Pick the table** and its raw struct in `crates/bifrost-config/src/raw.rs`: `[mount]` `RawMount`, `[daemon]`
   `RawDaemon`, `[reconciliation]` `RawRecon`, `[[discovery]]` `RawDiscovery` (flat), `[discovery.filter]`
   `RawFilter`, `[discovery.mount]` `RawTemplate`, `[[machines]]` `RawMachine`, `[[machines.mounts]]` `RawMount1`,
   `[policy.allow]`/`[policy.deny]` `RawMatch`.
2. **Add the field** as `Option<T>` (or a collection with a default). Every raw struct has `deny_unknown_fields`, so
   an older `bifrostd` rejects a config that uses the new key (with `line:col`), and a typo of it is an error rather
   than silently ignored.
3. **Validate it in `V::config`** (`crates/bifrost-config/src/lib.rs`): report problems with
   `self.err("<table>.<key>", …)` using the key path users see (`machines[0].mounts[1].local`), apply the default,
   and collect every error rather than stopping at the first
   ([decisions.md#config-deterministic-validation](decisions.md#config-deterministic-validation)). Durations go
   through `self.dur` (at least 1 s), paths and URLs through `self.expand` (`~`, `$VAR`, `$$`). Never echo a secret.
4. **Carry it** in the validated type: `Config`, `Timings`, `ProviderConfig`, `StaticMachine`, or (for per-mount
   settings) `MountTemplate`/`StaticMount`, which live in `crates/bifrost-core/src/reconcile.rs`.
5. **Use it.** Every config, at startup and on reload, goes through the actor's one apply path (`Actor::apply`). The
   tickers restart only when `health_interval` or `reconcile_interval` change; drivers are rebuilt only when
   `DriverSettings` changes; a provider restarts only when its `ProviderConfig` changes (or its last build failed).
   A `mount.root` change is rejected on reload (A22).
6. **Decide whether existing mounts must follow it.** If a change to the key must remount existing mounts, it has to
   reach `MountSpec` and the fingerprint ([Change the fingerprint](#fingerprint)); if new mounts picking it up is
   enough, keep it out, as `vfs_cache_mode` and `ssh_config` are (§15 #16).

Tests: a valid parse, `assert_has` for each rejection, and the `FULL` example in the config tests (the reference
config, also printed in `site/src/content/docs/reference/configuration.mdx`) if the key belongs there. If runtime
behaviour changes, an actor test with the `Rig`. Add it to `tests/e2e/config.tmpl.toml` only if the harness needs it.

Docs: `reference/configuration.mdx` (table and full example), the relevant guide, the README's configuration section,
[crates/config.md](crates/config.md). Check every TOML block you touch with `bifrost config check`
([docs-site.md](docs-site.md#verify)).

<a id="api-cli-tui"></a>
## Add an API route, a CLI command and a TUI key

### 1. The DTO (`crates/bifrost-core/src/api.rs`)

Shared by the daemon, the CLI and the TUI; `#[derive(Clone, Debug, Serialize, Deserialize)]`. serde ignores unknown
fields, so an old CLI reading a new daemon is fine, but a **new** field makes a new CLI fail to decode an old daemon's
reply (exit 1, "bad response") unless the field has `#[serde(default)]`.

### 2. The route (`crates/bifrost-daemon/src/api.rs`, `router()`)

| Kind | Pattern |
|---|---|
| read-only | a `get` handler that reads `snap(&s)` (the watch channel). GET routes never talk to the actor (C5), so they answer even while it is busy. Put new data into `StatusDto` via `Actor::snapshot` |
| changes state | a new `ApiCmd` variant with a `oneshot` reply; the handler calls `ask(&s, \|reply\| ApiCmd::Foo { …, reply })`; the actor answers in `Actor::api` (`actor.rs`). The actor does no I/O: long work goes to an executor task that reports back with a `Msg` |
| no reply needed | send the `ApiCmd` and return 202 at once (like `ApiCmd::Discover`, E4) |

- Validate path parameters with `Name::parse` (400) before they touch a path (see `log`).
- Errors are `ApiError { status, error }`, rendered as `ErrorDto`; use 404 for an unknown target and 403 for a
  target policy doesn't allow.
- Body-less POSTs take no body extractor, so the clients' `{}` is accepted; a POST with a body uses `Json<Req>`.
- The client (`bifrost-client`) is generic (`get`/`post<T>`); it needs nothing.

Tests: extend `stub_actor` and `routes_roundtrip` in `api.rs`; an actor test through `H::api` for the command's
behaviour.

### 3. The CLI command (`crates/bifrost-cli/src/main.rs`)

- A `Cmd` variant (clap derive; its doc comment is the help text). Ids take `#[arg(value_parser = name)]`, so only a
  valid `Name` ever reaches a URL path (a bad one is a usage error, exit 2).
- An arm in `daemon()`: call the route; with `--json` print the DTO (`pretty!`), else a renderer in `output.rs` that
  passes every daemon string through `c()` (the terminal-escape cleaner).
- Exit codes are a contract ([decisions.md#cli-exit-codes](decisions.md#cli-exit-codes)): 0 ok, 1 the operation
  failed, 2 usage, 3 the daemon is unreachable (`run()` maps `ClientError`).

Tests: a golden in `output.rs`; a binary test in `crates/bifrost-cli/tests/config_check.rs` with `stub()` asserting
the request line, the body and the exit code.

### 4. The TUI key (`crates/bifrost-tui/src/`)

- `app.rs`: a `Command` variant and the key in `App::on_key`. Keys in use: `q`, `Esc`, `j`/`k`/arrows, `Tab`/`BackTab`,
  `1`–`7`, `r`, `s`, `c`, `?`, `/`, `m`, `u`, `U`, `d`/`Enter`, `l`, and Ctrl-C. Popups swallow keys. Anything
  destructive asks first (see `U` → `Popup::ConfirmForce`).
- `main.rs`: an arm in `exec()` using `call(rt, CMD_TIMEOUT, client.post(…))` with the `{}` body; its result becomes
  the status line (cleaned). Keys must never block: commands are bounded by `CMD_TIMEOUT` (5 s), polls by
  `POLL_TIMEOUT` (500 ms) ([decisions.md#tui-inline-polling](decisions.md#tui-inline-polling)).
- `ui.rs`: add the key to `HELP` (the `?` popup) and, if it is common, to the bottom hint bar in `render`.

Tests: an `on_key` test in `app.rs` (like `simple_keys`), and a render test in `ui.rs` if the screen changes.

Docs: `site/src/content/docs/reference/api.mdx`, `reference/cli.mdx`, `reference/tui.mdx`;
[crates/daemon.md](crates/daemon.md), [crates/cli.md](crates/cli.md), [crates/tui.md](crates/tui.md).

<a id="event"></a>
## Add an event

1. **The variant** in `crates/bifrost-core/src/events.rs`. `Event` is `#[serde(tag = "type")]`: the variant name is
   the JSON `type` and the SSE `event:` name, so renaming a variant breaks SSE consumers and the E2E `events` helper.
2. **Emit it on a transition, never on every pass**, or the 200-entry ring fills with noise (B1; contract §14 risk 5 lists it too).
   - Every event with a `mount` field goes through the actor's `emit_mount(id, …)`, which fills the field in through
     `stamp()` in `actor.rs`: whether a `MountRuntime` method in `crates/bifrost-core/src/reconcile.rs` returned it as
     `Option<Event>` with an empty `mount` (the runtime stores no id), or the actor built it (as `MountRequested` and
     `UnmountStarted` are, on the executor dispatch paths). **Add the new variant to `stamp()`'s match**: its
     `_ => {}` arm compiles silently otherwise, and the event is published with `mount: ""` (S1 sign-off 2).
   - Machine, driver and config events (no `mount` field) are emitted by the actor with `self.emit(…)`. Keep the previous state and emit
     only on a change, as `MachineEligible` (the `allowed` set) and `DriverUnavailable` (the previous `probes`) do (B1).
3. `emit` logs every event at info with its `Debug` form: no secrets in event fields, and `clean` any string that came
   from outside.
4. **The TUI** (`crates/bifrost-tui/src/app.rs`, `event_text`) matches every variant, so the compiler points you there;
   write the one-line text.

The event then reaches `StatusDto.events` (the last 200), `bifrost status --json`, the TUI's Events view and
`GET /v1/events` (a broadcast channel of 256; a lagging SSE client gets a `lagged` frame).

Tests: a core test asserting the event a runtime method returns, or an actor test like
`events_eligible_and_driver_unavailable_on_transition` and `event_ids_stamped`. Docs: the event list in
`site/src/content/docs/reference/api.mdx`.

<a id="e2e-phase"></a>
## Add an E2E phase

Write `tests/e2e/pNN_<what>.sh` with `check_pNN` (and optionally `setup_pNN`, `config_pNN`), add it to the `all` list
in `tests/e2e/run.sh`, run it alone, then run `all` twice. The rules (only `ok` for assertions, bounded waits, kill
only by `mpid`, background jobs outside `ok` with `9>&-`, no second `[policy.*]` table, restore any config you edit)
and the couplings between phases are in [e2e-harness.md](e2e-harness.md#adding).

<a id="bf1"></a>
## Change the bf1 grammar safely

The bf1 format is a public contract: users publish it in their DNS zones, and every deployed `bifrostd` parses it
([decisions.md#bf1-dns-format-inline-and-index](decisions.md#bf1-dns-format-inline-and-index)). The code is in
`crates/bifrost-discovery/src/dns.rs`.

| Function | Role |
|---|---|
| `parse_bf1` | the grammar: `v=bf1` first, ≤ 2048 bytes, no trailing space, `key=value` tokens, a duplicate key invalid, per-key validation, unknown keys ignored, `node=` with `nodes=` invalid |
| `root` | splits the root RRset into inline node records (grouped by their `node=` label) and index values (`nodes=`); caps the union at `MAX_NODES` (256); a label both inline and indexed is ambiguous |
| `one` | a node's values → exactly one distinct record, or skip the node |
| `node` | a per-node lookup; `node=` there is invalid |
| `node_observation` | a record → `MachineObservation`; identity is the DNS label, `id=` is only the native id |
| `dns_label` | the label grammar (no dots, no lowercasing) |

Rules for a change:

- **Adding an optional key is compatible.** Older daemons ignore unknown keys, so records with the new key still work
  there, minus the new key's effect. Make sure that degraded behaviour is safe.
- **Changing what an existing key means, or rejecting records that were valid, is not.** Deployed zones would change
  behaviour on upgrade. Such a change needs a new version tag: `parse_bf1` returns `Ok(None)` for any value that
  doesn't start with `v=bf1`, so a `v=bf2` value is silently ignored by old daemons, and a zone can publish both
  during a migration.
- **Keep the invariants**: identity is always the label; any invalid known key rejects the whole record (never a
  partial apply); every value goes through a core validator; no driver hint (E2); caps stay.
- **`root()` pre-scans for `node=` on its own** (`strip_prefix("v=bf1 ")`, then the first token starting with
  `node=`) to group values before `parse_bf1` runs. A change to tokenisation or to the `node` key must keep the two in
  agreement, or a value could be grouped under one label and parsed as another.
- Warnings are part of the user docs: the site lists the logged reason for each rejected record.

Tests: the `bf1_*` and `inline_*` tests in `dns.rs` (add one per new rule, including a hostile value), the ignored
`coredns_discovery` against the p08 zone, and the E2E fixtures: `tests/e2e/dns/zone.tmpl` (add a hostile record for
any key that could carry attack data) and `p13_zone` in `tests/e2e/p13_hardening.sh`. Re-run p08 and p13.

Docs: `site/src/content/docs/examples/dns-records.mdx` (key tables, the rejected-records list and their logged
reasons, re-verified live as in [docs-site.md](docs-site.md#verify)) and `guides/discovery.mdx`; the README;
[crates/discovery.md](crates/discovery.md).

<a id="fingerprint"></a>
## Change the fingerprint

`MountSpec::fingerprint` (`crates/bifrost-core/src/model.rs`) is 16 lowercase hex digits of FNV-1a 64 over
`id\0machine\0host\0port\0user\0remote\0local\0driver\0ro` (no port or user → empty, `ro` → `0`/`1`, `driver` is the
**selector text**, so `auto` stays sticky when probes change). It is written into places that outlive the daemon
binary:

| Where | How |
|---|---|
| the kernel mount table | the marker `bifrost:<id>@<fp16>` as the mount's source (sshfs `fsname=`, rclone `--devname=`) |
| `state.json` | each `MountHandle.fingerprint` |

And it is compared in three places:

| Code | Comparison | Effect of a mismatch |
|---|---|---|
| `reconcile::decide` row 11 | the running handle's fingerprint vs the candidate spec's | a graceful remount (`SpecChanged`), once the daemon is `ready` and the new spec can mount |
| `mount()` step 2 (`step2` in `crates/bifrost-mount/src/lib.rs`) | a leftover marker at the path | same fingerprint → adopt it; different → lazy detach, then mount |
| `adopt()` at startup | takes the fingerprint from the marker | the adopted handle carries the **old** value |

### What happens if you change the encoding or add a field

On the first start of the new version, every mount is adopted with its old fingerprint, every candidate spec computes
a new one, and row 11 remounts **every mount** as soon as warm-up ends: gracefully, or by lazy detach for a mount
that is already Degraded. A mount with open files hits
the busy path (backoff, `Degraded("unmount blocked: busy (files open)")`) until they close; nothing is forced. Users
see every mount drop and come back once. The rclone VFS cache survives (it is keyed by mount id and the `--sftp-*`
flags, not the fingerprint).

That one-time remount is the whole cost, and it is why `fingerprint_stable_vector` pins the value for a fixed spec
(`"952d3037aea39f48"`): a change to the encoding must be deliberate, not a side effect of a refactor.

To do it:

1. Decide whether the new field must remount existing mounts when it changes. If new mounts picking it up is enough,
   don't fingerprint it (§15 #16 keeps `vfs_cache_mode` and `ssh_config` out for this reason).
2. Change `fingerprint()`, update `fingerprint_stable_vector` to the new value (compute it independently, as the
   original was), and extend `fingerprint_changes_on_every_field`.
3. Run `tests/e2e/run.sh all`: p13a (adoption, same pid) must still pass within one version; the upgrade remount
   itself is not covered by any test.
4. Release it as a minor version and say in the notes that every mount remounts once after the upgrade, so users
   should close files on mounts first ([release-and-ci.md](release-and-ci.md#versioning)).

**Don't change the marker grammar** (`bifrost:<id>@<16 hex>`, parsed exactly by `parse_marker`). A mount with a marker
the new code can't parse is foreign: on Linux `adopt()` ignores it, so it stays mounted but unsupervised, and
`mount()` step 2 refuses the path ("occupied by …") on both OSes until the user unmounts it by hand. (On macOS
`adopt()` still takes it when `state.json` has a record at that path, with the recorded fingerprint.) That is worse
than a fingerprint change.
