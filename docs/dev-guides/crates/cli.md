# bifrost-cli (`bifrost`)

`bifrost-cli` builds the `bifrost` command. It is a thin client of the daemon: almost every command is one or two HTTP calls over the Unix socket through `bifrost-client`, followed by rendering the returned DTO as text or, with `--json`, as pretty-printed JSON. Three things work without a daemon: `config check` (validates a file locally), `drivers` (probes the drivers locally when the daemon is unreachable) and `doctor` (checks what it can and reports the daemon as down). `mount` and `unmount` wait for the result by polling, and `discover` waits for every provider to refresh. The exit codes let scripts tell "operation failed" from "daemon not reachable". This guide covers the commands, the waiting logic, the renderers, `doctor`, the exit codes and the tests.

## Contents

- [At a glance](#at-a-glance)
- [Source map](#source-map)
- [Global flags and default paths](#global-flags-and-default-paths)
- [Command reference](#command-reference)
- [Targets are validated names](#targets-are-validated-names)
- [Waiting for results](#waiting-for-results)
- [Rendering: `output.rs`](#rendering-outputrs)
- [`--json`](#--json)
- [`doctor`](#doctor)
- [Exit codes](#exit-codes)
- [Runtime](#runtime)
- [Tests](#tests)
- [Where the code differs from the contract](#where-the-code-differs-from-the-contract)
- [Adding or changing a command](#adding-or-changing-a-command)

## At a glance

| | |
|---|---|
| Path | `crates/bifrost-cli/` (package `bifrost-cli`, binary `bifrost`) |
| Files | `src/main.rs` (clap tree, dispatch, waiting, `config check`), `src/output.rs` (text renderers), `src/doctor.rs` (`doctor` and the local driver probe), `tests/config_check.rs` (integration tests against the built binary) |
| Dependencies | `bifrost-core` (DTOs, `Name`, `clean`, `select_driver`), `bifrost-config` (`load`, `paths`, `default_auto_order`), `bifrost-client`, `bifrost-mount` (driver probes and `which` for the local checks), `clap` (derive), `serde_json`, `tokio` (`rt`, `macros`, `time`) |
| Tests | 5 golden unit tests in `src/output.rs`, 8 integration tests in `tests/config_check.rs`: `cargo test -p bifrost-cli` |
| Decisions | [http-over-unix-socket](../decisions.md#http-over-unix-socket), [cli-exit-codes](../decisions.md#cli-exit-codes), [sse-events-and-polling-clients](../decisions.md#sse-events-and-polling-clients), [busy-unmount-never-forced](../decisions.md#busy-unmount-never-forced), [config-deterministic-validation](../decisions.md#config-deterministic-validation), [validated-newtypes-at-trust-boundary](../decisions.md#validated-newtypes-at-trust-boundary) |

The CLI never touches mounts itself. PRD §17 and §33 ("CLI and TUI are clients of the daemon") make the daemon the only owner of mount lifecycle, so the CLI asks and then watches.

## Source map

| Item | File | Purpose |
|---|---|---|
| `Cli`, `Cmd`, `MachinesCmd`, `ConfigCmd`, `DaemonCmd` | `main.rs` | clap derive tree |
| `name` | `main.rs` | clap `value_parser` for every target: `Name::parse`, lowercased |
| `main` | `main.rs` | builds a current-thread tokio runtime and exits with `run`'s code |
| `run` | `main.rs` | handles `config check` first (no socket), then calls `daemon` and maps `ClientError` to an exit code |
| `daemon` | `main.rs` | one arm per command |
| `requested` | `main.rs` | `--no-wait` output |
| `settle` | `main.rs` | polls `GET /v1/mounts` until every id settles (mount and unmount) |
| `discover` | `main.rs` | posts a discover and polls status until every provider refreshed |
| `config_check` | `main.rs` | local `load` and the `ok:` line |
| `pretty!` | `main.rs` | `--json`: `serde_json::to_string_pretty` of a DTO |
| `c`, `st`, `dur`, `table` | `output.rs` | re-clean a daemon string, the wire name of a state, a compact duration, a hand-aligned table |
| `machines`, `machine`, `mounts`, `status`, `providers`, `actions`, `drivers` | `output.rs` | one renderer per output shape |
| `mount_line`, `unmount_line` | `output.rs` | decide when a polled mount has settled and what to print |
| `run`, `local_drivers`, `fuse` | `doctor.rs` | the doctor report, the daemon-less driver probe, the FUSE check |

## Global flags and default paths

| Flag | Default | Read by |
|---|---|---|
| `--socket PATH` | `paths::socket_path()`: `$BIFROST_SOCKET`, else `$XDG_RUNTIME_DIR/bifrost/bifrost.sock`, else `~/.cache/bifrost/bifrost.sock`; macOS `~/Library/Caches/bifrost/bifrost.sock` | every daemon command |
| `--config PATH` | `paths::config_path()`: `$BIFROST_CONFIG`, else `~/.config/bifrost/config.toml` | only `config check`, `doctor` and the daemon-less `drivers`. The daemon reads its own config; `--config` never changes what a running daemon uses |
| `--json` | off | every command except `doctor` |

All three are `global = true`, so they work before or after the subcommand (`bifrost reconcile --json`; test `json_flag_prints_dto`).

Neither `--socket` nor `--config` uses clap's `env` attribute. clap rejects an empty value, but an empty `BIFROST_CONFIG` or `BIFROST_SOCKET` should mean "unset". Commit a665a12 dropped `env` from `--config`; `--socket` followed the same rule when it was added in 1512326. The `paths` functions read the variables and treat empty as unset, so the flags default to `None` and fall back to them ([config.md](config.md#paths-default-locations)).

## Command reference

"Body" is the JSON the CLI posts. `unmount` always sends `{"force": …}`: its handler takes `Json<UnmountReq>`, which rejects an empty or `null` body. The body-less POSTs (`mount`, `reconcile`, `discover`, `config reload`) send `{}` by convention (critique C6); their handlers take no body extractor and ignore it (`crates/bifrost-daemon/src/api.rs`; [client.md](client.md)).

| Command | Requests | Text output (stdout) | `--json` | Exit |
|---|---|---|---|---|
| `status` | `GET /v1/status` | the status block | `StatusDto` | 0 |
| `machines`, `machines list` | `GET /v1/machines` | `NAME SOURCE ADDRESS STATE MOUNTED` | `[MachineDto]` | 0 |
| `machines show <id>` | `GET /v1/machines`, filtered here (there is no `/v1/machines/{id}`, E1) | key/value lines | `MachineDto` | 1 and `error: unknown machine <id>` on stderr when absent |
| `mounts` | `GET /v1/mounts` | `ID MACHINE DRIVER STATE LOCAL REMOTE` (+ `ERROR`) | `[MountDto]` | 0 |
| `mount <target>` | `POST /v1/mounts/<target>/mount` body `{}` (202, the accepted ids), then `GET /v1/mounts` every 250 ms for up to 60 s | one result line per id | `[MountDto]` of the settled ids | 1 if any id failed, went offline or timed out |
| `mount <target> --no-wait` | the POST only | `<id>  mount requested` per id | `["<id>", …]` | 0 |
| `unmount <target> [--force]` | `POST /v1/mounts/<target>/unmount` body `{"force": bool}`, then the same polling | one result line per id | `[MountDto]` of the ids still listed | 1 if busy (without `--force`), another unmount error, or a timeout |
| `unmount <target> [--force] --no-wait` | the POST only | `<id>  unmount requested` | `["<id>", …]` | 0 |
| `discover` | `GET /v1/status`, `POST /v1/discover` body `{}` (202), then `GET /v1/status` every 250 ms for up to 30 s | `PROVIDER KIND STATUS MACHINES LAST-OK` | `[ProviderDto]` | 0, also after the 30 s warning |
| `reconcile` | `POST /v1/reconcile` body `{}` | `MOUNT ACTION` | `[ActionDto]` | 0 |
| `drivers` | `GET /v1/status` (`drivers`, `auto_driver`); a local probe when the daemon is unreachable | `✓ sshfs  /usr/bin/sshfs  …` rows, then `default (auto): <name \| none>` | `[DriverDto]` | 0 |
| `doctor` | `GET /v1/status` when the daemon is up, plus local checks | the doctor report | (ignored) | 0 or 1, never 3 |
| `config check [PATH]` | none: local `bifrost_config::load` | `ok: <path> (N machines, N providers, N mounts, root <root>)` or the sorted `error: …` lines | `ReloadDto { ok, errors }` | 0 or 1 |
| `config reload` | `POST /v1/config/reload` body `{}` | `reloaded`, or the error lines | `ReloadDto` | 1 when `ok` is false |
| `daemon status` | `GET /v1/status` | `running pid 4242 up 3h12m socket <path>` or `not running (socket <path>)` | `StatusDto` | 3 when not running |

Error output: a failed request prints `error: <error>` on stderr, the error re-cleaned. For an API error that reads `error: 404: unknown target x` or `error: 403: agent-07: discover-only` (the daemon refuses to mount anything that isn't a candidate, [discover-is-not-mount](../decisions.md#discover-is-not-mount)).

Notes per command:
- `mount` and `unmount` accept a mount id or a machine id. The daemon resolves it: a mount id wins over a machine id, and a machine id means all its mounts (B15). The POST returns the ids it acted on, and those are what the CLI waits for.
- `mount` is the "retry now": it clears a hold, both retry timers and the failure count ([core.md](core.md)). `unmount` adds a persisted hold, so the mount stays down until `bifrost mount`.
- `unmount --force` asks for a lazy detach. It never kills a process; the driver keeps serving open files until they close ([no-pid-signalling-lazy-detach](../decisions.md#no-pid-signalling-lazy-detach)).
- `reconcile` returns the plan of the pass the daemon runs after its next driver re-probe (orchestrator sign-off after S2, item 1). It doesn't clear retry timers (A11).
- `config check` is handled in `run` before the socket is resolved, so it never touches the daemon. Its path is the positional `PATH`, else `--config`, else `paths::config_path()`. It reports on stdout, both success and errors (sign-off after S1, item 6); `--json` prints the same `ReloadDto` shape as `config reload`.
- `config reload` returns after the daemon has applied the file through its one config-apply path. On errors the daemon keeps its previous config.

## Targets are validated names

Every `<target>` and `<id>` goes through `name`, the clap `value_parser`: `bifrost_core::Name::parse`, which lowercases and then allows only `[a-z0-9][a-z0-9._-]{0,62}`. So:

- a bad target is a usage error (exit 2) before any socket is touched: `bifrost mount ../x` (test `exit3_when_daemon_absent`);
- the target is always one safe URL path segment, so nothing but a `Name` reaches the request line;
- `bifrost unmount AGENT-01` posts to `/v1/mounts/agent-01/unmount` (test `unmount_sends_force_body`).

This is the CLI's side of [validated-newtypes-at-trust-boundary](../decisions.md#validated-newtypes-at-trust-boundary).

## Waiting for results

The daemon's mount, unmount and discover routes return at once with 202 (discover: amendment E4). The work happens later in the actor. The CLI has no event-stream consumer, so it polls to learn the outcome ([sse-events-and-polling-clients](../decisions.md#sse-events-and-polling-clients), contract §15 #12).

### `settle`: mount and unmount

```text
POST …/mount | …/unmount  →  ids
loop:
    sleep 250 ms                         # also before the first poll
    GET /v1/mounts
    stop when line(id, that id's MountDto) is Some for every id, or 60 s have passed
print one line per id; exit 1 if any line is not ok or timed out
```

The first poll also waits 250 ms, so it never reads the snapshot taken before the POST was applied. On timeout the line is `<id>  timed out after 60s (<state>)`, with `absent` when the id isn't listed.

`mount_line(id, m)` decides when a mount is settled (`output.rs`):

| Mount state | Line | ok |
|---|---|---|
| absent from `/v1/mounts` | keep waiting | |
| `mounted`, or `unknown` (an adopted mount whose machine isn't in the registry yet) | `<id>  mounted  <local> (<driver>, pid <pid>)`; the pid part is left out when the daemon has none (an adopted mount without a state record, B15) | yes |
| `degraded` | `<id>  degraded: <why>` (still mounted; sshfs reconnect is working on it) | yes |
| `failed` | `<id>  failed: <why>` | no |
| `offline` | `<id>  offline` or `<id>  offline: <why>` | no |
| `eligible`, `connecting`, `unmounting`, `discovered` | keep waiting | |

`<why>` is `last_error`, else `detail`: a driver-selection failure has only a `detail` such as `no available driver (tried sshfs, rclone)` (B15).

`unmount_line(id, m, force)`:

| Mount state and `last_error` | Line | ok |
|---|---|---|
| `mounted`/`degraded`/`unknown` with the busy error, without `--force` | `<id>  unmount blocked: busy (files open); retry with --force` | no |
| the same with `--force` | keep waiting: the forced lazy detach follows | |
| `mounted`/`degraded`/`unknown` with any other error | `<id>  <error>` (core stores it as `unmount failed: …`) | no |
| `mounted`/`degraded`/`unknown` without an error, `unmounting`, `connecting` | keep waiting | |
| anything else, or absent | `<id>  unmounted (held; 'bifrost mount <id>' to resume)` | yes |

The busy string comes from `bifrost_core::MountError::Busy.to_string()`, the one busy string used everywhere (C4). A busy unmount is final for the CLI without `--force`: the daemon never forces on its own; it keeps the mount and retries the graceful unmount with backoff ([busy-unmount-never-forced](../decisions.md#busy-unmount-never-forced)). With `--force` the CLI keeps waiting, and the daemon skips the pending unmount backoff so the detach happens on the next pass rather than up to `retry_max` later (orchestrator sign-off after S4a, item 10). An `unmounting` row can still carry the previous attempt's error, so it always means "wait".

Ceilings: 60 s and 250 ms are constants. A mount whose `mount_timeout` is near its 5 m maximum can outlast the CLI's wait; the CLI then reports a timeout while the daemon carries on.

### `discover`

1. `GET /v1/status` and remember every provider's `refreshes`.
2. `POST /v1/discover` with `{}`. The route wakes every provider task and returns 202 with `{}` (E4).
3. Every 250 ms, `GET /v1/status` until every waited-for provider has a higher `refreshes` than before, or 30 s have passed.
4. Print the providers table from the last status.

Waited-for providers exclude two kinds that will never refresh on a discover:
- the `static` row, which has no task (the daemon adds it to `StatusDto.providers` for display; sign-off after S4a, item 12);
- a provider whose build failed (`refreshes == 0` and a `last_error`): no task exists; it is rebuilt only by a reload or the fallback tick (B11).

A provider that disappeared from the status meanwhile (a reload removed it) stops being waited for. On timeout the CLI prints `warning: no refresh within 30s from: <names>` on stderr and still exits 0: the table shows each provider's state, which is the answer.

## Rendering: `output.rs`

**Everything from the daemon is re-cleaned.** `c(s)` is `clean(s, 512)` from core. The daemon cleans what it produces, but the CLI does it again at render time: control characters and bidi or zero-width characters become `?`, and the text is cut at 512 characters. `table` re-cleans every cell, `status` every string, and `run` the printed error (contract §9, A8; [security.md](../security.md)). A hostile metadata value or an old daemon therefore can't inject terminal escapes. Goldens pin it: `\x1b[31m10.9.9.9` renders as `?[31m10.9.9.9`.

**Tables.** `table(rows)` is hand-aligned: each column is as wide as its widest cell (counted in chars), columns are two spaces apart, and trailing spaces are trimmed. No table crate. Width is by char count, so a double-width character in a cell misaligns its row.

| Renderer | Layout | Notes |
|---|---|---|
| `machines` | `NAME SOURCE ADDRESS STATE MOUNTED` (PRD §18) | NAME is the machine id, what `mount` and `machines show` take (sign-off after S2, item 3). MOUNTED is `yes` for `mounted` or `degraded`: a machine's state is the worst of its mounts, and a degraded mount is still in the mount table |
| `machine` | key/value: `id name source verdict address online state tags metadata shadowed mounts` | IPv6 with a port prints `[addr]:port`; `online` is `yes`, `no` or `unknown`; one `metadata` line per pair; empty lists print `-` |
| `mounts` | `ID MACHINE DRIVER STATE LOCAL REMOTE`, plus `ERROR` when any row has `last_error` or `detail` | driver `-` when none is selected |
| `status` | the block below | |
| `providers` | `PROVIDER KIND STATUS MACHINES LAST-OK` | `ok` or `error: <e>`; `12s ago` or `-` |
| `actions` | `MOUNT ACTION` | the action is core's `Action` `Display`, e.g. `mount (sshfs)`, `waiting (backoff 3s)` |
| `drivers` | `✓ name  binary  detail` rows, then `default (auto): <name \| none>` | the default is `StatusDto.auto_driver` |

```text
bifrostd 0.1.0  pid 4242  up 3h12m  /run/user/1000/bifrost/bifrost.sock
config    /home/sami/.config/bifrost/config.toml (ok)
root      /home/sami/machines
machines  7 (3 eligible)   mounts 3/3 mounted, 0 degraded, 0 failed
providers static ok (2) · tailscale ok (5, 12s ago) · infra error: timed out (last ok 2m ago)
drivers   sshfs ✓ (default) · rclone ✓ · rclone-nfs ✗ macOS only
```

Status block rules: `(warming up)` is appended while `ready` is false. With config errors the config line reads `(invalid; running the previous config)` and each error follows, indented. "Eligible" counts machines whose verdict string starts with `allowed`; the golden test builds verdicts from core's `Display`, so a change to core's strings breaks the test rather than the count. A provider reads `ok (N)`, `ok (N, 12s ago)`, `error: e (never ok)` or `error: e (last ok 2m ago)`. A `conflicts` line appears only when there are conflicts.

**Helpers.** `st(state)` returns the serde wire name (`mounted`, `discovered`, …), the same word `--json` prints. `dur(secs)` gives `45s`, `12m`, `3h12m`, `1d1h`: two units at most, truncated, not rounded.

## `--json`

`--json` prints the DTO the daemon returned, pretty-printed with `serde_json::to_string_pretty`. The output is the wire format from core's `api.rs` ([core.md](core.md)), so scripts get exactly what the daemon sent, without the re-cleaning the text output applies. Exceptions:

| Command | `--json` prints |
|---|---|
| `machines show <id>` | the one `MachineDto` |
| `mount`, `unmount` (waiting) | the settled `MountDto`s, found ids only; the exit code still follows the text rules |
| `mount`, `unmount` with `--no-wait` | the accepted ids |
| `discover` | `StatusDto.providers` after the wait |
| `drivers` | `StatusDto.drivers` (the same shape as `GET /v1/drivers`), or the local probe |
| `config check` | `ReloadDto { ok, errors }` with the `error: …` strings |
| `doctor` | nothing different: `--json` is ignored |
| `daemon status` when not running | the text line, exit 3 |

## `doctor`

`doctor::run` prints a report in the PRD §24 layout and returns the exit code itself, so `doctor` never exits 3: a stopped daemon is one of the things it reports.

```text
Config         ✓ /home/sami/.config/bifrost/config.toml
Daemon         ✓ running pid 4242
SSH            ✓ ssh /usr/bin/ssh   ✓ SSH_AUTH_SOCK visible to daemon
Discovery
  ✓ static      2 machines
  ✓ tailscale   5 machines
  ✗ infra       timed out
Mount Drivers
  ✓ sshfs       /usr/bin/sshfs (SSHFS version 3.7.3, fusermount3)
  ✓ rclone      /usr/bin/rclone (rclone v1.75.1, fusermount3)
  ✗ rclone-nfs  macOS only
FUSE           ✓ /dev/fuse  /usr/bin/fusermount3
Selected default
  sshfs
```

| Section | Daemon up | Daemon down |
|---|---|---|
| Config | `load` of the config path (below); errors listed, cleaned | same |
| Daemon | `✓ running pid N` | `✗ <client error>` |
| SSH | `ssh` found by `bifrost_mount::check::which` (`$PATH` plus `/usr/local/bin:/usr/bin:/bin`, and `/opt/homebrew/bin` on macOS); agent from `StatusDto.ssh_agent`: `SSH_AUTH_SOCK visible to daemon` | agent: whether this shell has `SSH_AUTH_SOCK` set |
| Discovery | one row per `StatusDto.providers` entry, static included | `✓ static  N machines` from the config, then `- <name>  daemon not running` per provider; no rows (only the header) if the config is invalid |
| Mount Drivers | `StatusDto.drivers`, plus a `✗ <mount>` row for each mount whose `last_error` carries the macOS permission hint (B7) | `local_drivers`: probes run here |
| FUSE | Linux: `/dev/fuse` and `fusermount3` (or `fusermount`). macOS: `/Library/Filesystems/macfuse.fs` for macFUSE, `/Library/Application Support/fuse-t` or `/usr/local/lib/libfuse-t.dylib` for FUSE-T | same |
| Selected default | `StatusDto.auto_driver` | the local probe's first available driver in the config's `auto_order`, else `default_auto_order()` |

**Config path precedence.** `--config`, then `$BIFROST_CONFIG` (empty = unset), then the daemon's `StatusDto.config_path`, then `paths::config_path()`. `$BIFROST_CONFIG` ranks with `--config` because it is the flag's environment form (commit f548564, F9; test `doctor_config_path_env_first_and_cleaned`). The daemon's path is used raw to load the file but re-cleaned for display, because it is a daemon string (F3).

**Exit rules.** Exit 1 when any of these holds, else 0 (orchestrator sign-off after S2, item 3):

| Fails `doctor` | Does not fail `doctor` |
|---|---|
| the config doesn't load (a missing file included) | the daemon is down |
| a mount carries the macOS permission hint (`macOS: allow the macFUSE system extension`, B7) | one driver is unavailable, like `rclone-nfs ✗ macOS only` on Linux |
| no driver is usable: `Selected default ✗ none (no available driver)` | a provider error, a missing `ssh`, no agent, a FUSE ✗ |
| | the daemon's first probe hasn't answered yet: any driver's detail is `probing`, printed as `Selected default  probing` (sign-off after S4a, item 4; review r2) |

Only Config and Drivers can fail it, because those are what stop every mount. The other rows are informational: a provider may be down for a while, and FUSE or ssh problems show up as mount errors with their own messages.

**`local_drivers(cfg)`.** Used by `doctor` and `drivers` when the daemon can't be reached. It calls `bifrost_mount::drivers(&settings)` with placeholder `DriverSettings` (no ssh_config, empty cache mode, zero timeout, empty state dir) and awaits each driver's `probe()`. A probe must therefore look only at binaries and flags, never at the settings (sign-off after S2, item 3). The default is `select_driver(&Auto, order, probes)` with the config's `auto_order`, or `default_auto_order()` when the config doesn't load: the same function the daemon uses for `StatusDto.auto_driver` (E5).

`drivers` falls back to this probe on `NotRunning` and on `Io` (for example `EACCES` or `ENOTDIR` on the socket; commit f548564, F8), printing `<error>; probing locally` on stderr. It loads `--config` or the default path, and an invalid config silently means the default order.

## Exit codes

| Code | Meaning | Produced by |
|---|---|---|
| 0 | success | |
| 1 | the operation failed | `ClientError::Api` (any non-2xx) or `Decode`; a mount that ended `failed` or `offline`; a busy or failed unmount; a 60 s timeout; `machines show` of an unknown id; an invalid config in `config check` or `config reload`; a `doctor` failure |
| 2 | usage error | clap: an unknown command or flag, a missing argument, an invalid target name. Also `bifrost-tui` for a bad argument |
| 3 | the daemon is not reachable | `ClientError::NotRunning` and `ClientError::Io`; `daemon status` when not running |
| 101 | panic: `HOME is not an absolute path` | a relative or empty `HOME` when a default path under it is needed (the socket for any daemon command with no `--socket`, `BIFROST_SOCKET` or, on Linux, `XDG_RUNTIME_DIR`; the config file for `config check` with no `PATH`, `--config` or `BIFROST_CONFIG`, and for the `drivers` fallback and `doctor` when no other path is known). Deliberate: a default is never resolved against the working directory ([config.md](config.md#paths-default-locations)) |

Why a separate code for "not reachable": a script can tell "bifrostd isn't up" from "bifrostd said no". The E2E harness waits for the daemon with `wait_until 10 bifrost daemon status` (`tests/e2e/lib.sh`), and `tests/e2e/p05_api.sh` asserts exit 3 against a missing socket. `Io` also exits 3 because it is the connection itself that failed, for instance a permission error on the socket (sign-off after S2, item 3). 2 is clap's own usage code, kept as is. See [cli-exit-codes](../decisions.md#cli-exit-codes).

`daemon status` prints `not running (socket <path>)` on stdout for `NotRunning`. Other connection errors reach `run`, which prints `error: …` on stderr, and exit 3 as well. Every other daemon command prints `error: bifrostd is not running (socket <path>)` on stderr, except `drivers` (prints the error followed by `; probing locally` on stderr, with no `error:` prefix, falls back to the local probe and exits 0) and `doctor` (reports it in the Daemon row on stdout).

## Runtime

`main` builds a `tokio` current-thread runtime (`new_current_thread().enable_all()`) and `block_on`s `run`. The crate enables only tokio's `rt`, `macros` and `time` features (contract §1): the CLI makes one request at a time, and `block_on` also drives the connection task that `bifrost-client` spawns per request ([client.md](client.md)). The CLI puts no timeout on any single request. The read routes answer at once, and `mount`, `unmount` and `discover` return 202 after at most one actor round trip. `reconcile` waits until the actor has re-probed the drivers and run a pass, and `config reload` waits for the file load and the actor's apply. The 60 s settle and the discover deadline bound only the polling loop, so a wedged daemon hangs whichever request is in flight.

## Tests

**Goldens, `src/output.rs`.** Byte-for-byte expected output. Fixtures build verdict and action strings from core's `Display` (sign-off after S1, item 7, "core's Display strings are final"), while the expected text stays literal, so a change in core's wording shows up here.

| Test | Pins |
|---|---|
| `machines_table_golden` | the machines table (including a hostile address re-cleaned), an empty table, `machines show` with IPv6 and port, metadata lines, `-` placeholders |
| `mounts_table_golden` | the mounts table with and without the ERROR column; `last_error` over `detail`; escape cleaning |
| `status_block_golden` | the contract §9 status example, byte for byte; warming up, config errors, degraded and failed counts, no default driver, conflicts, `providers none` |
| `small_tables_and_durations` | `dur` boundaries, the providers, actions and drivers tables, `st` |
| `mount_and_unmount_lines` | the settle rules above, except `mount_line` for `degraded`, `unknown` and `discovered` and `unmount_line` for `unknown`; for `offline` only the ok flag, not the line text |

**Integration, `tests/config_check.rs`.** They run the built binary (`CARGO_BIN_EXE_bifrost`) with `HOME` pointed at a temp dir and the `BIFROST_*` variables removed. Daemon commands talk to a hand-written HTTP/1.1 stub on a `std::os::unix::net::UnixListener` (`stub`) that answers N requests and returns every request line and body it saw, so tests assert the exact wire traffic.

| Test | Pins |
|---|---|
| `config_check_output_deterministic` | identical bytes on two runs, sorted `error:` lines, exit 1; a TOML error is one `file:line:col` line; the `ok:` line; empty `BIFROST_CONFIG` = unset; a relative `HOME` fails instead of using the working directory; a missing file is an error |
| `exit3_when_daemon_absent` | exit 3 for six daemon commands against a missing socket, and the exact stderr for `status`; `BIFROST_SOCKET` works like `--socket`; `daemon status` prints `not running` on stdout; usage errors are 2 before any socket is touched |
| `unmount_sends_force_body` | `{"force":true}` and `{"force":false}` on the wire; a target is lowercased; the waiting path polls `GET /v1/mounts` and prints the held line |
| `json_flag_prints_dto` | `--json` pretty-prints the DTO; `--json` after the subcommand; `reconcile` posts `{}` |
| `drivers_falls_back_to_local_probe_when_daemon_down` | local probe rows in `DRIVER_NAMES` order, `rclone-nfs` unavailable on Linux, the default from `default_auto_order()`, the stderr notice |
| `doctor_config_drivers_and_macos_hint` | the Config and Daemon rows, `Selected default`, the B7 hint row and exit 1, `probing` is not a failure, a bad config fails with the daemon down |
| `doctor_config_path_env_first_and_cleaned` | the daemon's `config_path` is re-cleaned; `$BIFROST_CONFIG` beats it |
| `drivers_probes_locally_when_the_socket_is_unreachable` | an `Io` connect error (`ENOTDIR`) also falls back to the local probe |

The local-probe tests use whatever `sshfs` and `rclone` the test host has, and assert only what holds on any host. End-to-end use of the CLI against a real daemon is covered by the E2E phases ([e2e-harness.md](../e2e-harness.md)).

## Where the code differs from the contract

| Contract §9 says | Code does | Why |
|---|---|---|
| `--socket` (env `BIFROST_SOCKET`), `--config` (env `BIFROST_CONFIG`) via clap | no clap `env`; the `paths` functions read the variables | empty = unset (commits a665a12, 1512326) |
| `drivers`: local probe when the daemon is down | also on `ClientError::Io` | an unreachable socket is "down" too (F8) |
| doctor exits 1 on a ✗ in Config or Drivers | an unavailable driver alone doesn't fail it; `probing` doesn't fail it | sign-offs after S2 (item 3) and S4a (item 4) |
| `daemon status`: "not running … exit 3" | only for `NotRunning`; other connection errors print `error: …` on stderr, still exit 3 | `run`'s generic mapping |
| `discover`: poll "until every provider's `refreshes` increases (≤ 30 s)" | skips `static` and failed-build providers; on timeout warns and exits 0 | neither can refresh on a discover |
| `providers` in `status`, `doctor`, `discover` | include a `static` row | daemon change in review r3 (sign-off after S4a, item 12) |

## Adding or changing a command

1. Add a variant to `Cmd` (or a sub-enum) with a doc comment: clap uses it as the help text. Give any id argument `value_parser = name`.
2. Add an arm in `daemon` (or handle it in `run` before the client is built if it needs no daemon, like `config check`). Use `c.get`/`c.post` with the DTO from `bifrost_core::api`; a body-less POST sends `&empty` (`{}`).
3. Print with `pretty!` for `--json`, and with a renderer in `output.rs` for text. Route every daemon string through `c()` or `table`.
4. Return the exit code the [table above](#exit-codes) implies; let `ClientError` propagate with `?` so `run` maps it.
5. Add a golden test for the renderer and, if it touches the wire, an integration test with `stub`.
6. Update the README and the user docs in `site/`.

If the command needs a new route, add it to the daemon first ([daemon.md](daemon.md), [extending.md](../extending.md)).
