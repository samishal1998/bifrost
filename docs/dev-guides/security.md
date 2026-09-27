# Security model

This guide is the internal security model of Bifröst: what it protects, who it defends against, where the trust
boundaries are and which function enforces each one, the rules every change to argv, file handling, logging or
discovery parsing must keep, what a hostile discovery source can and cannot achieve, the risks that remain on
purpose, and the review history that shaped all of it. The user-facing summary is the README's "Security model"
section. The design reasons are in [decisions.md](decisions.md), and the deliberate ceilings in
[simplifications.md](simplifications.md). Everything here was checked against the code; function and test names are
the ones in the tree.

## Contents

- [Assets and attackers](#assets)
- [Trust boundaries](#trust-boundaries)
- [PRD §23 principles and what enforces them](#prd-23)
- [argv construction rules](#argv-rules)
- [Host-key policy](#host-keys)
- [Filesystem permissions](#filesystem-permissions)
- [Secrets](#secrets)
- [Terminal-escape cleaning](#cleaning)
- [The log target allowlist](#log-allowlist)
- [What a hostile DNS or HTTP source can and cannot do](#hostile-records)
- [Other attackers](#other-attackers)
- [Residual risks](#residual-risks)
- [Review history](#review-history)
- [Rules for changes](#rules-for-changes)

<a id="assets"></a>
## Assets and attackers

**What is protected.**

| Asset | Why it matters |
|---|---|
| The user's SSH credentials (agent, keys) | every mount authenticates with them; a forwarded agent would let a mounted host use them |
| Local files the user owns | the daemon runs as the user and writes logs, state and mountpoints; a planted symlink could redirect a write |
| Files on the mounted hosts | a wrong host, user or path mounted under a trusted name is a confidentiality and integrity problem; a forced unmount can lose writes in flight |
| HTTP inventory credentials | configured header values (usually a bearer token) |
| The terminal and the log | untrusted strings are displayed in the CLI, the TUI and the daemon log |
| The daemon's availability | a hung FUSE mount or a hostile source must not freeze or crash it |

**Who the attackers are.**

| Attacker | Controls |
|---|---|
| A DNS publisher, or anyone on the path to the resolver (no DNSSEC) | every byte of the `bf1` TXT answers for the configured domain |
| An HTTP inventory operator | the inventory response body and status |
| The owner of a tailnet peer | that peer's `HostName` and `OS`, and through the hostname usually the first label of its `DNSName` (the coordination server derives the MagicDNS name from the hostname unless an admin renames the node or the name is taken; it assigns the IPs and `ID`; the tailnet admin assigns ACL tags) |
| A mounted remote host | the file contents it serves, its sshd's stderr, and how long it takes to answer |
| Another local user (another uid) | anything world-writable on the machine |

Out of scope: root, and other processes running as the same user. They have the user's privileges already, and the
local API trusts them by design (see [Other attackers](#other-attackers)).

<a id="trust-boundaries"></a>
## Trust boundaries

| Input | Controlled by | Trust | What it can influence | Enforced by |
|---|---|---|---|---|
| The config file (`$BIFROST_CONFIG` or `~/.config/bifrost/config.toml`) | the user | trusted, but fully validated | everything: machines, providers, policy, templates, paths | `bifrost_config::parse` (`deny_unknown_fields`, the core validators, `url_allowed`, `is_token`, `expand`) |
| Environment (`BIFROST_*`, `HOME`, `XDG_*`, `PATH`, `SSH_AUTH_SOCK`, variables named in `${VAR}`) | the user | trusted | default paths, expansion, binary lookup, ssh auth | `bifrost_config::paths` (`HOME` must be absolute or it panics rather than resolve against the cwd; relative `XDG_*` values are ignored); `check::which_in` and tailscale's `which_in` use only absolute `PATH` entries |
| Static machines | the config | trust 0 | connect target, user, mounts; allowed unless denied | `Config::static_observations`, `policy::evaluate` step 4 |
| `tailscale status --json` | tailscaled; per-peer fields as above | trust 1 | ids, addresses, tags, online state of tailnet peers | `tailscale::parse_status`, `observation` (each field through a validator; `ShareeNode` peers skipped; own tailnet sorts first) |
| HTTP inventory | the inventory server | trust 2 | ids, a connect target, port, hints, tags, metadata | `http::HttpProvider`, `parse_inventory`, `observation` |
| DNS TXT `bf1` | whoever answers the query | trust 3 | ids (labels), a connect target, port, hints, tags | `dns::parse_bf1`, `root`, `one`, `node`, `node_observation` |
| The kernel mount table | the kernel | trusted for "what is mounted where" | adoption, readiness, idempotent unmount | `table::read`; a foreign entry's source and fstype are only displayed, through `clean` (`step2`) |
| Child logs and ssh stderr | the mounted host and the network, partly | untrusted text | `last_error`, the log route | `validate::tail`, `clean`, 2 KiB / 64 KiB read windows |
| `state.json` | the daemon (0600, in an owner-checked directory) | holds: authoritative (user intent); mount records: a hint | which mounts stay unmounted; the informational adoption pid; macOS adoption of an unmarked entry | `state::read` (quarantine on any doubt), `mount::adopt` |
| API clients on the socket | any process of the same uid | trusted as the user | mount/unmount of candidates, holds, reload, logs | the socket's 0600 mode and its directory; `Name::parse` on every path segment; 403 for non-candidates |
| Files inside a mount | the remote host | untrusted | nothing in the daemon | the daemon never reads inside a mount; its only call there is `symlink_metadata` on a nonexistent probe name (`check::liveness`) |

Trust ranks decide which observation of a machine wins (lower is more trusted); see
[decisions.md](decisions.md#winner-takes-all-trust). A rank says nothing about whether a machine is mounted: every
non-static machine is discover-only until a rule allows it.

<a id="prd-23"></a>
## PRD §23 principles and what enforces them

| # | Principle | Enforced by | Unit tests | E2E |
|---|---|---|---|---|
| 1 | Discovery does not imply trust | `policy::evaluate` (non-static observations default to `DiscoverOnly`; winner takes all; global deny sees only the selected observation; `id_hit` matches `native_id` only in the owning provider's filter, A19); `reconcile::desired` (candidates only for `Allowed`); `Actor::api_mount` (403 with the verdict); `tailscale::parse_status` (`ShareeNode` skipped) | `no_allow_rule_is_discover_only`, `dns_cannot_redirect_tailscale_machine`, `dns_tags_cannot_deny_static_machine`, `global_ids_match_machine_id_only`, `native_id_scoped_to_owning_provider`, `api_mount_forbidden_for_discover_only` | p07 (zero tailscale mounts without a filter), p08 (`other-01` denied by `policy.deny tags=misc`), p13 (`inv-01` from DNS stays inventory-sourced, `dns` shadowed) |
| 2 | No SSH passwords | no password key exists (`deny_unknown_fields` in `raw.rs`); `BatchMode=yes` in `SSH_OPTS` for sshfs, the preflight and `--sftp-ssh`; child stdin is `/dev/null` (`mount_with`, `ssh_preflight`, `check::run`) | `unknown_field_has_line_col`, `sshfs_argv_linux_golden`, `preflight_argv_golden` | the E2E sshd has a locked password and one authorized key |
| 3 | Prefer the agent, keys, Tailscale SSH | the system `ssh` is always used (sshfs, preflight, rclone `--sftp-ssh`); the environment, `SSH_AUTH_SOCK` included, is inherited; `-F` only when `mount.ssh_config` is set; `StatusDto.ssh_agent` and `doctor` report whether the daemon sees an agent | `rclone_never_uses_internal_ssh` | all mount phases authenticate by key |
| 4 | Never weaken host verification | the constants `SSH_OPTS` and `SSH_CLI_HARDENING`; no config knob; rclone's internal SSH never used | `argv_never_weakens_host_keys`, `ssh_never_forwards_agent_x11_or_ports`, `rclone_never_uses_internal_ssh`; ignored docker test `preflight_exit0_and_hostkey_failure` | `psec_hostkey.sh` (sshfs) and p09's `unknown-key-rc` (rclone): state `failed`, `last_error` contains "Host key verification failed", never in mountinfo |
| 5 | Mount only explicit paths permitted by policy | remote paths come from the static mount or the provider template; record hints are used only with `honor_hints` and only after `RemotePath::parse`; there is no driver hint (E2) | `remote_path_rules`, `hints_ignored_unless_honor_hints` | p08 (`agent-dns` mounts its record's `path=` because its provider sets `honor_hints = true`) |
| 6 | Never execute discovery-supplied commands | discovery data can only become a `Name`, `Host`, `User`, `RemotePath` or `u16`; see [argv construction rules](#argv-rules) | `host_rejects_option_injection`, `positionals_never_start_with_dash`, `rclone_sftp_ssh_tokens_clean_cfg_quoted`, `bf1_host_injection_rejected` | p08 (`bad-node host=-oProxyCommand=touch${IFS}$T/pwned`: skipped, warned, canary never created); p10 (`-oProxyCommand=x` as an address and as a host: skipped, warned) |
| 7 | TXT and HTTP data are untrusted | bf1: 2048-byte values, `v=bf1` first, duplicate key invalid, labels without dots, ≤256 labels, whole-node rejection, identity pinned to the label; HTTP: 1 MiB body, 1000 entries, 32 addresses read, per-entry isolation, no redirects, credentials only over https or loopback; `clean` on every displayed string | `bf1_*`, `inline_*`, `ambiguous_node_skipped`, `node_id_pinned_to_label`, `max_nodes_caps_inline_plus_index`, `invalid_entries_isolated`, `entries_capped_at_1000`, `body_cap_enforced`, `redirect_not_followed`, `host_overrides_addresses`, `clean_strips_escapes` | p08 (hostile records on both the inline and the per-node path), p10 |
| 8 | Validate local mount paths against traversal and collisions | `Name::parse` (one lowercase path component); `local_path = root.join(id)` only in `reconcile::desired`; unique locals in config; static locals claimed first at runtime (`conflicts`); `mount_with` step 1 (parent canonicalizes to itself, file name equals the id); `prepare_mountpoint`; `mount.root` rules; `Name::parse` on the log route and every CLI/TUI target | `name_rejects_traversal`, `local_traversal_rejected`, `duplicate_local_rejected`, `static_local_collision_conflict`, `prepare_mountpoint_creates_rejects_symlink_file_nonempty`, `root_slash_home_relative_rejected`, `log_route_rejects_traversal` | p10 (name `../x` skipped and warned), p08 (`evil` with `id=../../etc` skipped) |
| + | The local API | a 0600 socket; directories created 0700 and never chmodded when they exist; owner and world-write checks; a lock against a second daemon; no default socket under `/tmp` | `socket_0600_stale_replaced`, `second_instance_lock_refused`, `preexisting_parent_dirs_not_chmodded`, `world_writable_root_refused`, `world_writable_state_dirs_refused` | p05 (socket mode 600, a second `bifrostd` refused with "already running", exit 3 on a bogus socket) |

<a id="argv-rules"></a>
## argv construction rules

Every external command is built by a pure function and spawned without a shell. A change to any argv builder must
keep all of these, and the tests named below must keep passing.

1. **No shell, absolute binaries.** `tokio::process::Command::new(<absolute path>).args(argv)`. Binaries come from
   `check::which` (absolute `PATH` entries, then `/usr/local/bin:/usr/bin:/bin`, plus `/opt/homebrew/bin` on macOS),
   or fixed paths (`/sbin/umount`, `/usr/sbin/diskutil`). Never `sh -c`.
2. **Only validated values.** Discovery data reaches argv only as `Host`, `User`, `RemotePath`, a port (`u16`) or the
   marker (`Name` plus 16 hex digits). A static machine's host, user, port and remote come from the config through
   the same types. The other config and environment inputs are:
   - the `ssh_config` path (absolute, existing, no `"`, no control character), after `-F`;
   - the validated `vfs_cache_mode`, inside `--vfs-cache-mode=`;
   - the local path `<canonical mount.root>/<id>` (`mount.root` checked by `V::root` and canonicalized in
     `main::run`; `<id>` is a `Name`), an absolute positional;
   - rclone's `--cache-dir=<state dir>/rclone/<id>`, where the state dir comes from `$BIFROST_STATE_DIR` or XDG
     (`paths::state_dir`) and is not validated beyond being used as a path;
   - on macOS FUSE, the id as sshfs's `volname=<id>` (inside an `-o` value) and rclone's `--volname=<id>`.

   Each of them is an absolute positional, the value that follows a flag (`-F`, `-p`, `-l`, `-o`), or part of a
   `--flag=value`, so none can become an option.
3. **No positional ever starts with `-`, and each builder keeps options and positionals apart.**
   - sshfs (`sshfs_argv`): options first, positionals last: `[user@]host:path` (the `Host` grammar forbids a leading
     `-`) and the absolute local path.
   - The preflight (`preflight_argv`): options first, then `-s -- <host> sftp`.
   - rclone (`rclone_argv`): positionals first, right after the subcommand (`mount` or `nfsmount`): `:sftp:<path>`
     (a constant prefix) and the absolute local path. Every option comes after them, and rule 4 keeps each one
     self-contained.

   Tests: `positionals_never_start_with_dash` (sshfs and the preflight); rclone's order is pinned by
   `rclone_argv_mount_golden`.
4. **rclone options that take a value are always `--flag=value`**, so no value is ever parsed as a flag. The others
   are constant switches (`--sftp-disable-hashcheck`, `--read-only`).
5. **`--sftp-ssh` is one space-separated string** that rclone splits (with `"…"` quoting) and to which it appends
   `-s sftp`. Every token is a constant or a validated host, user or port; the ssh_config path is quoted. There is no
   `--` in it, because `--` would turn `-s sftp` into a remote command. `sftp_ssh_check` refuses the mount when the ssh
   path, host or user contains whitespace, `"` or a control character, or the config path contains `"` or a control
   character. Test: `rclone_sftp_ssh_tokens_clean_cfg_quoted`.
6. **rclone never runs remote commands or reads user config:** `--sftp-shell-type=none`, `--sftp-disable-hashcheck`,
   `--config=/dev/null`, and only the `--sftp-host`, `--sftp-ssh`, `--sftp-shell-type` and `--sftp-disable-hashcheck`
   sftp flags. Test: `rclone_never_uses_internal_ssh`.
7. **Forbidden substrings.** No argv contains `StrictHostKeyChecking`, `UserKnownHostsFile`, `GlobalKnownHostsFile`,
   `ProxyCommand`, `IdentityFile`, or sshfs's `ssh_command`, `directport`, `passive` or `sftp_server` (which take a
   command or bypass ssh). Test: `argv_never_weakens_host_keys`, over every builder, flavour and `-F` combination.
8. **No secret in argv.** Header values never reach a command line (argv is visible in `ps`). The environment is
   inherited instead (see [Secrets](#secrets)).
9. **stdin is `/dev/null`** for every child, so nothing can prompt.
10. **Timeouts and kills.** Helper commands run under a timeout with `kill_on_drop(true)` (`check::run`,
    `ssh_preflight`, tailscale's `status_json`). Mount children use `kill_on_drop(false)` and `process_group(0)`, and
    are never signalled except the one spawn that timed out (see [decisions.md](decisions.md#no-pid-signalling-lazy-detach)).

<a id="host-keys"></a>
## Host-key policy

- Bifröst passes exactly these ssh options: `SSH_OPTS` = `BatchMode=yes ConnectTimeout=10 ServerAliveInterval=15
  ServerAliveCountMax=3 ControlMaster=no ControlPath=none`, and on the preflight and `--sftp-ssh` also
  `SSH_CLI_HARDENING` = `-a -x -o ClearAllForwardings=yes -o PermitLocalCommand=no`. sshfs passes
  `-x -a -oClearAllForwardings=yes` itself.
- The user's ssh_config (or `mount.ssh_config`, passed as `-F`) and known_hosts decide trust. `BatchMode=yes` turns
  any prompt, host-key confirmation included, into a failure.
- No agent, X11 or port forwarding reaches a mounted host, whatever ssh_config says. `SSH_CLI_HARDENING` covers the
  preflight and rclone's `--sftp-ssh`; sshfs's own `-x -a -oClearAllForwardings=yes` covers sshfs.
- `LocalCommand` (which runs on the local machine, not on the mounted host) is disabled only for the preflight and
  rclone (`PermitLocalCommand=no` in `SSH_CLI_HARDENING`). `sshfs_argv` passes only `SSH_OPTS`, so for the sshfs
  child's ssh the user's ssh_config still decides it, although sshfs would forward a `PermitLocalCommand` `-o` option
  (the string is in the sshfs 3.7.3 binary). See [residual risk 21](#residual-risks). Closing it would be a code
  change: add `PermitLocalCommand=no` to the sshfs `-o` list.
- `ControlPath=none` / `ControlMaster=no`: a mount never rides on (or creates) a user's multiplexing master.
- rclone never uses its internal SSH library, which skips host-key checks unless `known_hosts_file` is set. Its
  NOTICE "No host key validation is being performed" refers to that unused library.
- The preflight (`check::ssh_preflight`) runs before every spawn, so a host-key failure shows up as
  `Host key verification failed.` in `last_error` instead of a mount timeout.

The reasoning and history are in [decisions.md](decisions.md#host-keys-never-weakened).

<a id="filesystem-permissions"></a>
## Filesystem permissions

The reference uid is the owner of the lock file the daemon has just opened. "Private" below means: owned by that uid
and not world-writable (`mode & 0o002 == 0`, so a umask-002 0775 directory passes). `create_dirs` creates missing
components with mode 0700 and chmods only the directories it created; a pre-existing directory is never chmodded.
Why these rules, and why `0o022` and an ancestor walk were rejected:
[decisions.md#private-state-and-socket-dirs](decisions.md#private-state-and-socket-dirs).

| Path | Mode | Rule | Code |
|---|---|---|---|
| state dir (`$BIFROST_STATE_DIR`, `$XDG_STATE_HOME/bifrost`, `~/.local/state/bifrost`) | 0700 when created | must be private, else exit 1 | `main::run` step 2–3, `create_dirs`, `private` |
| `<state>/logs` | 0700 when created | must be private, else exit 1 (r3: otherwise another user could swap `logs/` and plant `<id>.log -> <a file of ours>`, which the create + truncate open would clobber) | `main::run` |
| `<state>/bifrostd.lock` | 0600 | `File::try_lock`; `WouldBlock` → "already running", exit 1; refused if group- or other-writable (a hostile pre-created 0666 lock would otherwise make an attacker's directory pass the ownership check) | `main::lock` |
| `<state>/state.json` | 0600 | written as `state.json.tmp` (a leftover is removed first; `create_new`, 0600) → `sync_all` → rename; unreadable or unparseable → renamed `state.json.corrupt-<unix>` | `state::write`, `state::read` |
| `<state>/logs/<id>.log` | 0600 | `<id>` is a `Name`; opened create + truncate on each spawn; no size cap (§15 #13) | `mount_with` |
| `<state>/rclone/<id>/` | created by rclone | the per-mount VFS cache (A23), inside the private state dir | `rclone_argv` (`--cache-dir`) |
| socket parent | 0700 when created | when it already existed it must be private, else exit 1 | `main::bind_socket` |
| socket | 0600 | absolute; ≤103 bytes; an existing socket is removed only when nothing answers on it (a live one belongs to another `bifrostd`); an existing non-socket is refused; bound, then `set_permissions(0o600)` | `main::bind_socket` |
| `mount.root` | 0700 when created | expanded; absolute; not `/`, not `$HOME`, no `..`, no control character (config); canonicalized once at startup; must be private, else exit 1 (r2: otherwise another user could swap `<root>/<id>` for a symlink or pre-mount it with our marker) | `V::root`, `main::run` step 5 |
| `<root>/<id>` | 0700 when created | only after the mount table says it is not a mountpoint: missing → created; a symlink, a non-directory or a non-empty directory → `Refused` (fuse3 would hide the files); anything already mounted there that isn't ours → `Refused("occupied by …")` | `prepare_mountpoint`, `step2`, `mount_with` |
| removal | — | `std::fs::remove_dir` only (empty directories), only when the table says it isn't a mountpoint: after a failed mount, and after an unmount for `NotDesired`, `Manual` or `OfflineGrace`. Nothing under the root is ever deleted recursively | `mount_with`, `Actor::start_unmount` |

Not checked (recorded as ponytail X2 in [simplifications.md](simplifications.md)): the ancestors of the root and of
the state and socket directories, and group-writable (`0o020`) directories shared with other users. The config file's
own mode is not checked either.

<a id="secrets"></a>
## Secrets

The only secrets Bifröst handles are HTTP header values (and possibly a token in an inventory URL). SSH credentials
stay in ssh and the agent; Bifröst never sees them.

- **Where they come from.** Header values and `discovery.url` support `${VAR}` expansion, so a token can live in the
  environment instead of the config file (`headers = { Authorization = "Bearer ${BIFROST_INVENTORY_TOKEN}" }`). An
  undefined or empty variable is an error, never an empty string (a665a12: `TOK=""` no longer yields `"Bearer "`).
- **Validation never echoes them.** `expand` errors never repeat the input. Header-value errors say only "header value
  contains CR, LF or NUL". A URL that fails `url_allowed` is not echoed. A TOML shape error drops a quoted string value
  from its message (`parse`). `HttpProvider::new` errors name at most the header key.
- **In memory.** Header values are `Secret`, whose `Debug` prints `***`, and reqwest `HeaderValue`s marked
  `set_sensitive(true)`. No code path logs a `Config` or a `ProviderSpec`.
- **On the wire.** https with the system's root certificates (reqwest with rustls and native roots), or `http://` only
  to `127.0.0.1`, `[::1]` or `localhost` (`url_allowed` rejects userinfo and suffix tricks such as
  `http://127.0.0.1@evil` and `localhost.evil`), and then with `no_proxy()`, so an `HTTP_PROXY` never receives the
  credentials. Redirects are never followed, so the headers can't be sent to another host.
- **In errors and logs.** Transport errors use `reqwest::Error::without_url()`, because the URL's query may hold a
  token (`transport_error_names_cause_not_url`). A non-2xx response becomes just `HTTP <code>`.
- **Not in argv, not in state.json, not in the API.** `ProviderDto` carries the provider name, kind and status only.

<a id="cleaning"></a>
## Terminal-escape cleaning

`bifrost_core::validate::clean(s, max)` makes untrusted text safe to display: every control character (C0, DEL and
C1, which includes the 8-bit CSI `0x9b`) and every bidi or zero-width format character (`U+200B–U+200F`,
`U+2028–U+202E`, `U+2066–U+2069`, `U+FEFF`) becomes `?`, and the result is cut at `max` characters. The caps are 128
for display names, 256 for metadata values, and 512 for errors and log lines (A8). `validate::tail(s, max)` builds
error text from a log or stderr: it drops blank lines and lines starting with `# bifrost exec: ` (the argv header),
keeps the last lines that fit in `max`, cleans each, and joins them with ` | `.

| Text | Cleaned where it is produced | Cleaned again when shown |
|---|---|---|
| machine display names | `clean(_, 128)` in tailscale and HTTP (a DNS name is its label, which the grammar already restricts) | CLI `output::c` / `table`, TUI `app::c` |
| metadata values | `clean(_, 256)` in tailscale and HTTP; config values must be ≤256 characters with no control character | same |
| provider errors (`last_error`) | `clean(_, 512)` or `tail(_, 512)` in each provider, and again in `Actor::discovery` | same |
| mount errors (`last_error`, events) | `tail(_, 512)` for ssh stderr and child logs (`ssh_preflight`, `log_tail`, `unmount_path`); `clean(_, 512)` for a foreign mount's fstype and source (`step2`) and for panic messages (`caught`) | same |
| provider warnings in the daemon log | every `warn!` for a skipped record or entry cleans the reason with `clean(_, 512)`; the record or node name is cleaned with `clean(_, 128)` or built from validated labels (DNS query names) | — |
| config errors | every `ConfigError` message through `clean(_, 512)` | `bifrost config check` prints them; `doctor` re-cleans the config path it shows (f548564) |
| the mount log route | each line `clean(line, 512)` (`api::log`) | TUI Logs view |
| API 404 messages | the target through `clean(_, 128)` (`Actor::targets`) | CLI |
| `Invalid` errors | the offending value is printed with `{:?}` (escaped) | then cleaned with the rest of the message |

Every string the CLI or TUI prints from the daemon goes through `clean(_, 512)` again at render time, so a daemon bug
can't reach the terminal unfiltered. The daemon log uses ANSI colour only when stderr is a terminal. Raw third-party
debug output is kept out of it by the [log target allowlist](#log-allowlist).

<a id="log-allowlist"></a>
## The log target allowlist

`BIFROST_LOG` (error|warn|info|debug|trace, default info) sets the level for bifrost's own targets only. Every other
target (hickory, reqwest, hyper, rustls, h2, axum, tokio, …) is capped at warn, or at error when `BIFROST_LOG=error`,
by `main::log_filter`:

```rust
Targets::new().with_default(level.min(Level::WARN)).with_target("bifrost", level)
```

The reason is security, not noise: at debug level hickory logged raw TXT answers, which are attacker-controlled and can
carry terminal escapes (S3 carry-over 2c). An allowlist also covers any dependency added later. The test
`third_party_debug_logs_capped` checks that a hickory debug line is dropped and a hickory warning kept, that
`BIFROST_LOG=error` keeps third-party crates at error, and that `bifrost_discovery::dns` still logs at trace. See
[decisions.md](decisions.md#log-filter-allowlist).

<a id="hostile-records"></a>
## What a hostile DNS or HTTP source can and cannot do

This covers anyone who controls the bf1 TXT answers (the zone owner, or anyone on the resolver path, since DNSSEC is
not validated) or the HTTP inventory response.

**It can:**

- **List machines.** Any id it likes (a DNS label or a valid `Name`), with display names, tags and metadata (cleaned,
  capped). Without an allow rule they stay `discover-only`.
- **Get a machine mounted when a rule allows it.** If a provider include or a global allow matches what it publishes
  (names, tags and metadata are its own choice; a CIDR rule needs an IP literal in range as the connect target), the
  machine is mounted at `<root>/<id>`. The source picks the connect host and port, and with `honor_hints = true` also
  the remote user and path. Host-key checking only proves that the host is the one named. So a hostile record can
  mount **any host the user's ssh already trusts** under an id of the attacker's choosing, possibly as another user
  with `honor_hints`, and local programs will read those files as if they came from the named machine. This is why
  contract §4 calls the allow rules plus host-key checking the real protection, and deny rules on source-controlled
  attributes advisory.
- **Take an id no more trusted source reports.** It can't displace one that a more trusted source reports.
- **Remove its own machines.** An NXDOMAIN or an empty inventory is an empty `Ok`. The machines age out and are
  unmounted gracefully (a busy mount stays). An HTTP entry carries no TTL, so it ages out after 3 × interval. A DNS
  observation lasts `max(record TTL, 3 × interval)` (`MachineRegistry::apply_ok`; the DNS provider takes the TTL from
  the answer's `valid_until()`), so a publisher can keep a removed machine listed, and mounted if allowed, for as
  long as its TTL. hickory clamps a TTL to one day (`MAX_TTL`, 86 400 s) by default, and D1 sets no resolver options.
- **Cause churn.** Moving `host=` changes the fingerprint and triggers a remount (graceful, unless the mount's health
  is `Degraded` at that moment; see below). Flapping records come and go on the same `max(TTL, 3 × interval)`
  hysteresis.
- **Delay warm-up** by never returning a non-empty answer (bounded by `offline_grace_period`), and freeze the provider
  by failing (its last view stays).
- **Cost bounded work and log lines:** up to 256 concurrent TXT lookups inside `<domain>` per refresh; 1 MiB and 1000
  entries per HTTP refresh; one warning per skipped record or entry per refresh.

**It cannot:**

- **Run a command or inject an option** into ssh, sshfs or rclone: every field goes through the `Host`, `User`,
  `RemotePath`, port or tag grammar, and argv follows the [rules above](#argv-rules). E2E p08 and p10 plant
  `-oProxyCommand=…` records and check that the canary file never appears.
- **Get past host-key checking.** An unknown host key fails during key exchange, before user authentication, with
  `BatchMode=yes` turning any prompt into a failure. Pointing `host=` at the attacker's own server yields a failed
  connection and no credentials, unless the user's ssh_config itself accepts new keys (see
  [Residual risks](#residual-risks)).
- **Reach the agent, X11 or forwarded ports** of a mounted host (`SSH_CLI_HARDENING`, sshfs's own `-x -a`).
- **Redirect, re-address or deny a machine that a more trusted source reports.** The most trusted observation alone
  supplies the host, port, user, path, tags and metadata, and the global deny sees only that one.
- **Satisfy a global `ids` allow with its `id=` / `id` field.** Native ids match only in the owning provider's own
  filter (A19). Identity is the label or name, never the native id.
- **Slip past a CIDR include** with an in-range address while connecting elsewhere. An HTTP `host` replaces
  `addresses`, and without `host` only the first valid address is kept (A20, d911f9a). DNS has one address, `host=` or
  `<node>.<domain>`.
- **Choose the driver** (no driver hint, E2), **the local path** (`<root>/<id>`, a single `Name` component),
  **escape the root**, or **take a static machine's local name** (static locals are claimed first and the clash is
  reported in `conflicts`).
- **Make the daemon query another domain.** Labels contain no dots, and every query is an absolute name under
  `_bifrost.<domain>.`.
- **Exhaust memory:** 2048 bytes per TXT value, 256 labels, a 1 MiB HTTP body, 1000 entries, 32 addresses read per
  entry, 16 MiB of tailscale output, 64 KiB of stderr.
- **Crash the actor with a hostile TTL.** `apply_ok` uses `checked_add` and falls back to the floor.
- **Inject terminal escapes** into the CLI, the TUI or the log ([cleaning](#cleaning), [allowlist](#log-allowlist)).
- **Obtain the HTTP credentials** through a redirect, a proxy or an error message ([Secrets](#secrets)).
- **Force-unmount a busy, healthy mount.** Removals are graceful; a busy mount stays `Degraded` and retries. The
  exception: a host change that lands while the mount's health probe reads `Degraded` makes row 11 remount it with
  force, which is a lazy detach that kills nothing (see
  [decisions.md](decisions.md#busy-unmount-never-forced)).

<a id="other-attackers"></a>
## Other attackers

- **A tailnet peer's owner** sets `HostName` and `OS`. The id comes from the first label of `DNSName`, and `HostName`
  is only the fallback (when `DNSName` is empty or its first label isn't a valid `Name`). That label is not verifiable
  from this repo, but by Tailscale's default behaviour it is derived from the peer's own hostname unless an admin
  renames the node or the name collides. Peers sort with our own tailnet first, then shared-in nodes, then those with
  an empty `DNSName`, and the first peer with an id wins, so a peer never evicts one of our own existing ids (aa98d2a).
  It can still pick an id that is not taken, for example one that matches a `names =` include. For tailscale, prefer
  `include_tags` (ACL tags) or `include_ids` (the stable node `ID`, matched as `native_id` in the owning provider's
  filter) over name globs. `ShareeNode` peers (another user's
  devices that see ours because we shared a node) are skipped entirely (r1): at trust 1 one could otherwise shadow a DNS
  or HTTP machine with the same id.
- **A mounted remote host** controls what local programs see inside the mount, which is out of Bifröst's hands. It
  can hang the FUSE mount; the [hung-FUSE guards](decisions.md#hung-fuse-guards) keep the daemon responsive, and after
  `offline_grace_period` the mount is lazily detached. Its sshd output reaches the child log, which has no size cap
  (§15 #13); the daemon reads only its last 2 KiB (errors) or 64 KiB (the log route).
- **Another local user** can't reach the socket (0600, in a 0700 or owner-checked directory), and the daemon refuses
  to start when the state dir, `logs/`, the socket's parent or the mount root is owned by someone else or
  world-writable.
- **A process of the same user** can do anything through the API that the user can: mount candidates, hold and
  force-unmount any mount, reload the config, read mount logs. It still gets 403 for a machine that isn't a candidate.
  This is the intended access model: the socket is the user's.

<a id="residual-risks"></a>
## Residual risks

Known and accepted. Each item names where it is recorded.

| # | Risk | Recorded in |
|---|---|---|
| 1 | Deny rules on attributes a source controls itself (names, tags and metadata from DNS or HTTP) are advisory: they deny only what that source chose to publish | contract §4 |
| 2 | An allowed discovered machine's connect host (and, with `honor_hints`, user and path) come from the source, so a hostile source that passes an include rule can mount any host the user's ssh already trusts, under an id it picks | contract §4 (implied); [above](#hostile-records) |
| 3 | A global allow on a name, tag or metadata can be satisfied by any provider, DNS and HTTP included; scope it with `providers = [...]` or use a provider include | contract §4 |
| 4 | The user's ssh_config decides host-key trust. `StrictHostKeyChecking accept-new` (or `no`) there is inherited, and then a record pointing at an attacker's server is accepted on first contact. The same file's `ProxyCommand`, `SendEnv` and so on apply too | contract §6 ("the user's ssh_config decides trust") |
| 5 | DNSSEC is not validated: anyone on the resolver path is a record publisher | §15 #21 |
| 6 | Unknown bf1 keys are ignored, so a typo fails silently. A root value with neither `node=` nor `nodes=` (for example `name=agent-01`) is a valid, empty index value and logs nothing; only when nothing valid is left does the provider warn "no valid bf1 root record". A typo in an optional key such as `hsot=` falls back to its default (`host` = `<node>.<domain>`) | 55a35a6; `dns::root`, `parse_bf1` |
| 7 | Two providers of the same kind reporting one id: the alphabetically first provider name wins, whatever the config order | §15 #10 |
| 8 | Skipped records and entries are visible only in the daemon log, not in the CLI or TUI | §15 #22 |
| 9 | A provider that keeps failing freezes its last view: a machine removed from an unreachable inventory stays listed, and mounted, until the inventory answers again or the provider is removed from the config | §15 #30 |
| 10 | Ancestors of the root, state and socket directories, and group-writable shared directories are not checked; the config file's mode is not checked | ponytail X2 ([simplifications.md](simplifications.md)) |
| 11 | `bind_socket` binds and then chmods 0600. For that instant the socket has the umask's mode. Connecting needs write on the socket and search on its parent, so the window is open to other users when the umask leaves the socket group- or other-writable (umask 002 or 000) and the parent already existed and is searchable by them (`private()` accepts a pre-existing 0755 parent; a created parent is 0700). A connection made in the window is queued and stays open after the chmod | code (`main::bind_socket`); not previously recorded |
| 12 | The daemon's whole environment is inherited by ssh, sshfs and rclone, including any variable used for header expansion. An ssh_config `SendEnv` pattern that matches it would send it to mounted hosts | code (`mount_with`, `check::run` don't clear the environment); not previously recorded |
| 13 | Any same-uid process controls the daemon through the API | by design: the socket's permissions are the only access control (contract §11, "Local API" row) |
| 14 | `rclone nfsmount` (the macOS default auto driver) serves the remote on an unauthenticated 127.0.0.1 NFS port that any local user can reach | ponytail X9 |
| 15 | rclone data safety: a lazily detached rclone can share `--cache-dir` with its replacement; pending VFS writes survive only while the `--sftp-*` flags are unchanged; a graceful unmount doesn't wait for write-back | ponytail X6–X8 |
| 16 | On macOS, an unmarked mount at `<root>/<id>` with a matching state.json record is adopted (the root and the state dir are owner-checked and not world-writable, so another user can't plant such an entry or record) | §15 #23 |
| 17 | The child log has no size cap | §15 #13 |
| 18 | No concurrency cap: an inventory that suddenly allows many machines starts that many ssh processes at once | §15 #20 |
| 19 | `install.sh` verifies the tarball against a `SHA256SUMS` from the same GitHub release. That catches corruption and truncation, not a compromised release: nothing is signed | [decisions.md](decisions.md#install-sh-verify-always) |
| 20 | HTTPS trusts the system's native root store, including any locally installed interception CA | reqwest `rustls-tls-native-roots` (`Cargo.toml`) |
| 21 | sshfs doesn't pass `PermitLocalCommand=no`, so a `PermitLocalCommand yes` plus `LocalCommand` in the user's ssh_config runs locally for every sshfs mount's ssh connection (the preflight and rclone disable it). The command is the user's own configuration; tokens such as `%h` and `%r` expand to the validated host and user | code (`sshfs_argv` passes only `SSH_OPTS`); not previously recorded |

<a id="review-history"></a>
## Review history

Every stage task went through an implementer, two reviewers (contract conformance and correctness; trust boundary and
ponytail), a skeptic and a fixer (contract §13). After S4 a whole-repo review looped with five finder lenses (PRD §35
conformance, PRD §23 security, reconciler and concurrency correctness, data loss and error handling, ponytail
over-engineering), a three-vote adversarial verification (a finding survived with two or more confirmations) and a
fixer, followed by `scripts/check.sh` and `tests/e2e/run.sh all`. It was to stop after two consecutive rounds with no
findings, or after three rounds. All three rounds produced fixes, so it stopped at the three-round cap.

**Final review.**

| Round | Commit | Area | Finding → fix | Test |
|---|---|---|---|---|
| r1 | 2e09dce | mount | ssh_config forwarding (`ForwardAgent`, `ForwardX11`, `RemoteForward`) reached mounted hosts through the preflight and `--sftp-ssh` → `SSH_CLI_HARDENING` (r1-5) | `ssh_never_forwards_agent_x11_or_ports` |
| r1 | 2e09dce | mount | a failed mount left `<root>/<id>` behind → removed once the table shows no mount there (r1-7) | `failed_mount_leaves_no_mountpoint` |
| r1 | e2284f5 | discovery | a tailscale `ShareeNode` (someone else's device) at trust 1 could shadow and redirect a DNS/HTTP machine → skipped (r1-6) | `tailscale_fixture_parse` (sharee peer in the fixture) |
| r1 | e2284f5 | discovery | the DNS provider snapshotted the system resolver at start (stale after a network change, dead after an offline start) → re-read every refresh (r1-9, r1-11) | `system_resolver_read_per_refresh`, `reload_retries_failed_provider_build` |
| r1 | 3b903ac | daemon | a provider whose build failed never recovered without a reload → rebuilt on every fallback tick (r1-8) | `fallback_tick_retries_failed_provider_build` |
| r1 | 3b903ac | daemon | the poller applied a 0-byte config mid-`>`-redirect, dropping every machine and unmounting idle mounts → never applied by the poller; a 0-byte file at startup counts as missing (r1-12) | `poller_empty_file_keeps_active_and_warns_once`, `empty_config_file_not_loaded` |
| r2 | fb3238e | daemon | an unbuildable changed provider's observations aged out and unmounted its machines → frozen (r2-1) | `reload_unbuildable_provider_freezes_observations` |
| r2 | fb3238e | daemon | false `Failed` before the first probe → `Eligible` / "probing drivers"; `doctor` prints "probing" (r2-2, r2-10) | `drivers_listed_in_first_snapshot`, `doctor_config_drivers_and_macos_hint` |
| r2 | fb3238e | daemon | an OfflineGrace unmount left `<root>/<id>` forever → removed (r2-3) | `offline_grace_unmount_removes_empty_dir` |
| r2 | fb3238e | daemon | another user who can write `mount.root` could swap `<root>/<id>` for a symlink or pre-mount our marker → root must be owned and not world-writable (r2-6) | `world_writable_root_refused` |
| r2 | d719eaa | core | a Healthy probe reset a busy unmount's backoff every tick (about 13 retries a minute forever) → no reset while an unmount retry is pending; `POST …/unmount` clears the pending backoff | `busy_unmount_backoff_survives_healthy_probe`, `api_force_unmount_skips_busy_backoff` |
| r2 | 374fcb3 | mount | a probe stuck on a lazily detached hung instance held the path, so every later mount there was force-detached each grace period → in-flight key per instance (r2-9, r2-11) | `liveness_keyed_per_instance` |
| r2 | 374fcb3 | mount | `log_tail` read the whole peer-sized child log into memory → reads only the last 2 KiB (r2-12) | `log_tail_reads_only_the_tail` |
| r2 | 33f4a85 | docs | the README didn't say a mount id wins over a machine id in `mount`/`unmount` targets | — |
| r3 | 6376c65 | daemon | another user who can write the state dir could plant `logs/<id>.log -> <a file of ours>` → state dir, `logs/` and socket parent must be owned and not world-writable; status also gained the `static` provider row | `world_writable_state_dirs_refused` |
| r3 | 6e54b57 | core | after a restart a lower-trust provider answering first remounted an adopted mount to its spec (and back) → row 11 waits for warm-up `ready` | `row11_gated_on_driver_and_online` |

**Earlier stage reviews with security findings.**

| Stage | Commit | Area | Finding → fix |
|---|---|---|---|
| S0 | 4f03aa4 | core validate | `parse_duration` capped at 366 days (so `Instant + 3·d` can't overflow); `Host` rejects unspecified addresses, with the trailing dot stripped first so `0.0.0.0.` can't dodge it; `Cidr` rejects IPv4-mapped v6; `clean` also replaces bidi and zero-width characters |
| S1-A | 1cd5eb1 | core | a no-longer-wanted busy unmount pinned `last_error` and `next_wakeup` (SEC-1) → a passed `unmount_retry_at` is dropped |
| S1-B | a665a12 | config | an empty variable counted as defined (`HOME=""` → `/machines`, `TOK=""` → `"Bearer "`) → error; shape errors could echo a secret → quoted strings dropped; every message cleaned; control characters refused in `mount.root` and `ssh_config`; a relative `HOME` refused |
| S1-C | e914166 | mount | busy detection matched the word in a path (R1) → errno text only; the `Refused` message carried an uncleaned fstype (R2, SEC-3) → cleaned; unmarked entries were adopted on Linux (SEC-1) → macOS only; the preflight buffered peer stdout without a cap (SEC-2) → stdout to `/dev/null`, stderr capped at 64 KiB, one 15 s timeout; the E2E `start_daemon` could touch real paths (SEC-4) → refuses unless `BIFROST_CONFIG`, `BIFROST_STATE_DIR` and `BIFROST_SOCKET` are set and under `$T` |
| S2-E | 6f0411e | daemon | a group- or other-writable lock file was accepted as the uid reference → refused; a live socket could be unlinked → never; an unreadable state.json would be overwritten → quarantined |
| S2-F | f548564 | cli | `doctor` printed the daemon's config path uncleaned (F3, SEC-1) → re-cleaned |
| S2-G | 7f3fe76 | e2e | `start_daemon` without a config would default to the real `~/machines` → refused |
| S3-H | aa98d2a | tailscale | a shared-in node or a peer choosing its own HostName could take one of our own nodes' ids (sec-S3H-SEC-1) → own tailnet sorts first, HostName-only last |
| S3-J | 785e351 | rclone | cache-dir sharing, VFS cache keyed by flags (sec-S3J-DATA-1), no write-back wait, unauthenticated nfsmount port (sec-S3J-SEC-3) → recorded as `ponytail:` notes (X6–X9), not fixed |
| S3-K | d911f9a | http | a trailing in-range address could carry another connect target past `include_cidrs` → first valid address only; address warnings bounded (32 read, one warning per entry); `http://` credentials could go through an environment proxy → `no_proxy()` |
| S4-O | 36dc831 | daemon | hickory debug logs wrote raw TXT into the log (S3 carry-over 2c) → log target allowlist; a driver panic left a mount in flight forever → panics caught |
| S4-M | de28fe8 | daemon | a SIGHUP handler without a running poller turned SIGHUP into a silent no-op (SEC-2) → installed only after the poller thread starts |
| v0.1.1 | 9ea7c87 | dns | an invalid inline `node=` label used one of the 256 slots → skipped before the cap (`max_nodes_caps_inline_plus_index`) |

<a id="rules-for-changes"></a>
## Rules for changes

- New untrusted input goes through a `bifrost_core::validate` type or function before it reaches an observation, and
  a bad record is skipped whole with a cleaned warning, never partially applied.
- New argv follows [the argv rules](#argv-rules) and is added to `argv_never_weakens_host_keys` and
  `positionals_never_start_with_dash`.
- Any string that can reach a terminal or the log from outside goes through `clean` or `tail` with the right cap.
- New files and directories under the state dir or the root follow [Filesystem permissions](#filesystem-permissions).
  Never `remove_dir_all` under the root, and never touch a path the mount table says is a mountpoint except through the
  guarded helpers.
- A new provider's observations get a trust rank in `bifrost_config::TRUST`; a new source is discover-only by default.
- Never add an ssh option that changes host-key handling, forwarding or the command that runs, and never add a config
  knob for one.
