<p align="center">
  <img src="brand/05_hero/hero_aurora_bridge_16x9.png" alt="An aurora bridge between two worlds" width="100%">
</p>

<p align="center">
  <img src="brand/01_logo/primary_logo_color.png" alt="Bifröst" width="360">
</p>

<p align="center"><strong>Remote worlds. Local files.</strong></p>

<p align="center">
  <a href="https://github.com/samishal1998/bifrost/actions/workflows/ci.yml"><img src="https://github.com/samishal1998/bifrost/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/samishal1998/bifrost/releases/latest"><img src="https://img.shields.io/github/v/release/samishal1998/bifrost" alt="Latest release"></a>
  <a href="https://samishal1998.github.io/bifrost/"><img src="https://github.com/samishal1998/bifrost/actions/workflows/pages.yml/badge.svg" alt="Docs"></a>
  <a href="#license"><img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-2DD4BF" alt="License: MIT OR Apache-2.0"></a>
</p>

<p align="center"><a href="https://samishal1998.github.io/bifrost/"><strong>Documentation</strong></a> · <a href="https://github.com/samishal1998/bifrost/releases">Releases</a></p>

# Bifröst

**What it is.** Bifröst is a daemon (`bifrostd`) that makes remote machines look like local folders. It finds
machines through discovery providers (static config, Tailscale, DNS TXT, HTTP). It filters them with explicit
allow/deny policy and keeps the selected remote paths mounted under one root (`~/machines/<id>`) with SSHFS or
rclone, reconciling continuously. A CLI (`bifrost`) and a TUI (`bifrost-tui`) talk to it over a local socket.

**What it isn't.** It is not a VPN, a sync engine, a distributed filesystem, an rsync replacement or a
replacement for Tailscale or SSH. It consumes the connectivity and the SSH trust you already have.

```text
discovery providers → machine registry → policy → desired mounts → reconciler → mount drivers → ~/machines/
```

Design documents: the PRD (`bifrost_prd_and_implementation_plan.md`) and the implementation contract
(`docs/design/contract.md`, authoritative where they differ).

## Install

```bash
curl -fsSL https://samishal1998.github.io/bifrost/install.sh | sh
```

If GitHub Pages is unreachable, the same script is served from the repository:

```bash
curl -fsSL https://raw.githubusercontent.com/samishal1998/bifrost/main/install.sh | sh
```

The installer picks the prebuilt release for your platform (Linux x86_64/aarch64, static musl; macOS
x86_64/arm64), checks it against the release's `SHA256SUMS` and refuses to install on a mismatch. It installs
`bifrost`, `bifrostd` and `bifrost-tui` into `~/.local/bin` without sudo, then reports which runtime
dependencies are missing. It never installs them itself.

| variable | default | meaning |
|---|---|---|
| `BIFROST_VERSION` | latest | release tag to install, e.g. `v0.1.0` |
| `BIFROST_INSTALL_DIR` | `$HOME/.local/bin` | where the three binaries go |
| `BIFROST_DOWNLOAD_URL` | GitHub Releases | base URL for mirrors or tests; the script fetches `<base>/<asset>` |

Put the variables on `sh`, not on `curl`, since they are read by the script:

```bash
curl -fsSL https://samishal1998.github.io/bifrost/install.sh | BIFROST_VERSION=v0.1.0 BIFROST_INSTALL_DIR="$HOME/bin" sh
```

To install by hand, download `bifrost-<target>.tar.gz` and `SHA256SUMS` from the
[latest release](https://github.com/samishal1998/bifrost/releases/latest), verify it with
`grep ' bifrost-<target>.tar.gz$' SHA256SUMS | sha256sum -c` (`shasum -a 256 -c` on macOS), and copy the three
binaries from `bifrost-<target>/` onto your `PATH`. The targets are `x86_64-unknown-linux-musl`,
`aarch64-unknown-linux-musl`, `x86_64-apple-darwin` and `aarch64-apple-darwin`.

### Build from source

```bash
cargo build --release          # Rust ≥ 1.89 (edition 2024)
mkdir -p ~/.local/bin && install -m 755 target/release/{bifrost,bifrostd,bifrost-tui} ~/.local/bin/
```

With `CARGO_TARGET_DIR` set, the binaries land in `$CARGO_TARGET_DIR/release/` instead. There are three:

| binary | role |
|---|---|
| `bifrostd` | the daemon: discovery, policy, reconciliation, the local API |
| `bifrost` | the CLI |
| `bifrost-tui` | the terminal UI |

### Runtime dependencies

They are found through `$PATH` plus `/usr/local/bin:/usr/bin:/bin` (and `/opt/homebrew/bin` on macOS), so a
minimal service PATH still works:

- `ssh` (OpenSSH client): always. Every driver goes through the system ssh.
- Linux: `sshfs` and/or `rclone`, plus FUSE 3 (`/dev/fuse` and `fusermount3`, package `fuse3`).
- macOS: macFUSE or FUSE-T for `sshfs`/`rclone`; `rclone` alone for `rclone-nfs`.
- `tailscale`: only for the Tailscale provider.

`bifrost doctor` checks all of this (the `tailscale` binary through the running daemon's provider status).

## Quickstart

The PRD §31 milestone: one static machine over SSHFS. Write `~/.config/bifrost/config.toml`:

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
bifrost config check            # ok: … (1 machines, 0 providers, 1 mounts, root /home/sami/machines)
bifrostd &                      # or as a systemd user service (below)
bifrost status                  # daemon, config, machines, mounts, providers, drivers
bifrost mounts                  # agent-01 … mounted
ls ~/machines/agent-01/
bifrost unmount agent-01        # unmounts and holds it down, even across daemon restarts
bifrost mount agent-01          # clears the hold and mounts again
```

`ssh sami@agent-01` has to work non-interactively first (key or agent, and a known host key). Bifröst runs ssh with
`BatchMode=yes` and never prompts. `bifrost mount` and `bifrost unmount` wait for the result (up to 60s) unless
you pass `--no-wait`.

## Configuration

**Where:**
- config: `$BIFROST_CONFIG`, else `~/.config/bifrost/config.toml`;
- state: `$BIFROST_STATE_DIR`, else `$XDG_STATE_HOME/bifrost`, else `~/.local/state/bifrost`. It holds `state.json`, the
  lock and the per-mount logs in `logs/<id>.log`;
- socket: `$BIFROST_SOCKET`, else `$XDG_RUNTIME_DIR/bifrost/bifrost.sock`, else `~/.cache/bifrost/bifrost.sock`. On
  macOS it is `~/Library/Caches/bifrost/bifrost.sock`. The socket is always mode 0600;
- log level: `BIFROST_LOG=error|warn|info|debug|trace` (default `info`), written to stderr.

**Rules:**
- A missing or empty (0-byte) config file runs an empty default (root `~/machines`, no machines), with a warning.
- An invalid config stops `bifrostd` from starting (exit 2), so it never unmounts everything.
- Edits are picked up automatically (polled every 2s; applied once two reads agree, so within 2–4s), or at once
  with `bifrost config reload` or SIGHUP. An invalid edit keeps the running config and shows in `bifrost status`.
  A file emptied at runtime (a `>` redirect still being written) keeps the running config too; only an explicit
  reload applies an empty file. Changing `mount.root` needs a restart.
- Unknown keys are errors, so a typo or a `password = …` line is rejected. `bifrost config check [PATH]` prints
  every error sorted, the same bytes every time.
- `~`, `$VAR` and `${VAR}` are expanded only in `mount.root`, `mount.ssh_config`, `discovery.url` and header
  values. An undefined variable is an error.

The full reference is [contract §3](docs/design/contract.md#3-configuration) (validation rules, expansion, how
machines become mounts). Its complete example follows. To run it as-is, `~/.config/bifrost/ssh_config` must exist
and `BIFROST_INVENTORY_TOKEN` must be set.

```toml
version = 1                                   # optional; only 1 accepted

[mount]
root = "~/machines"                           # ~ and $VAR expanded; absolute; not "/", not $HOME, no ".."; yours, not world-writable
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
nameservers = ["10.0.0.2", "127.0.0.1:5353"]  # optional "ip" | "ip:port"; default: system resolver (re-read each refresh)

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
local = "build"                               # mount id = local; it shadows the machine id for bifrost mount/unmount
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

Every mount lives at `<root>/<id>`. The id is the machine name, or a static mount's `local`. A discovered machine
mounts its remote login directory (`remote = "~"`) unless its provider's `[discovery.mount]` template says
otherwise.

## Policy model

**Discovering a machine is not permission to mount it.** Every machine gets a verdict, shown by
`bifrost machines show <id>`:
- `allowed (<rule>)`: it is mounted;
- `discover-only`: listed, never mounted;
- `denied (<rule>)`: listed, never mounted.

`bifrost mount` on a machine that is not allowed returns the verdict (HTTP 403). It never overrides policy.

The rules, evaluated in this order:

1. **Provider exclude.** `exclude_*` in a provider's `[discovery.filter]` drops only that provider's
   observation. If no observation is left, the machine is denied.
2. **Winner takes all by trust.** When several providers report the same id, only the most trusted remaining
   observation counts: static < tailscale < http < dns (lower is more trusted). It alone decides address, user,
   tags, metadata and the verdict. The losers are listed as `shadowed`. So a DNS or HTTP record can never
   redirect, re-tag or deny a machine that config or Tailscale also reports. To hand such a machine to DNS,
   exclude it in the more trusted provider's filter (for a static machine, remove it from `[[machines]]`).
3. **Deny wins.** A global `[policy.deny]` match denies, static machines included.
4. **Static = explicit allow.** Machines in `[[machines]]` are allowed unless denied.
5. **Provider include.** If a provider's `include_*` rules all match, the machine is allowed.
6. **Global allow.** If every `[policy.allow]` rule matches, the machine is allowed. It is satisfied by whichever
   provider supplied the observation, so scope it with `providers = [...]` when that matters.
7. Otherwise the machine is **discover-only**. A provider with no filter only lists its machines.

Rules within a table AND across kinds (`ids`, `names`, `cidrs`, `tags`, `providers`, `metadata`) and OR within a
kind:
- names are globs (`*`, `?`);
- CIDRs never resolve hostnames. A non-static machine without an IP literal fails a CIDR deny closed;
- metadata includes need every pair, metadata denies fire on any pair;
- global `ids` match the machine id only. A provider's `include_ids`/`exclude_ids` also match that provider's own
  native id (a Tailscale node ID, a bf1 `id=`, an inventory `id`).

A deny on tags, names or metadata published by DNS or HTTP is advisory: that source controls those values. The
real protection is the allow rules plus SSH host-key verification.

`honor_hints` (per provider, default `false`) lets a record's `user=` and `path=` override the provider's
template, after validation. Records can never pick a driver, a command or an ssh option.

## Discovery providers

Each provider refreshes every `interval`. Up to that many seconds pass before a new machine appears (`bifrost discover`
refreshes now).

A machine missing from successful refreshes is dropped after `max(TTL, 3 × interval)`, and its mount is unmounted
gracefully. A provider that fails keeps serving its last good view, frozen until it recovers; one that could not
start at all is retried every `reconcile_interval` and on the next `bifrost config reload` or SIGHUP. Invalid
records are skipped one by one with a warning in the daemon log.

**Tailscale** (`type = "tailscale"`, trust 1):
- runs `tailscale status --json` (10s timeout); it never runs `tailscale up`, `down` or `set`;
- the binary comes from `$PATH` and the fixed dirs above, else on macOS
  `/Applications/Tailscale.app/Contents/MacOS/Tailscale`;
- the id is the first label of the MagicDNS name, else the lowercased hostname. The address is the MagicDNS name
  when MagicDNS is on, else the first Tailscale IPv4 address (the first Tailscale IP if there is none);
- tags lose their `tag:` prefix; `Self` is never listed, and neither are other users' devices that appear only
  because you shared this machine with them (`ShareeNode`; `tailscale status` hides them too);
- with no filter and no global allow, every peer is discover-only.

**DNS TXT, `bf1` format** (`type = "dns"`, trust 3). One TXT record set at `_bifrost.<domain>`, one value per
machine:

```dns
_bifrost.example.com. TXT "v=bf1 node=agent-01 host=10.0.0.5 port=22 user=sami tags=dev,agent path=/home/sami"
_bifrost.example.com. TXT "v=bf1 node=agent-02 tags=dev"
```

Beyond about a dozen machines, list node labels in an index value and give each node a record of its own. Both
forms mix in one record set:

```dns
_bifrost.example.com.          TXT "v=bf1 nodes=build-01,build-02"
_bifrost.build-01.example.com. TXT "v=bf1 host=10.0.0.18 tags=ci"
_bifrost.build-02.example.com. TXT "v=bf1 host=10.0.0.19 tags=ci"
```

- Keys: `v` (must come first); root values only: `node` (an inline node record) or `nodes` (an index), never
  both; node keys: `host` (default `<node>.<domain>`), `port`, `user`, `tags`, `path` and `id` (a native id only).
  Unknown keys, `driver=` included, are ignored. Other TXT records (SPF…) are ignored.
- **Identity is the node label**, never `id=`. Labels have no dots, so an index can't send queries to another
  domain. There are at most 256 nodes, inline and indexed together, and every name is queried absolute (no
  search domains).
- A value over 2 KiB, a duplicate key, or any key that fails validation (e.g. `host=-oProxyCommand=…`) rejects
  the **whole node**. Two different values for one node, or a node both inline and in `nodes=`, are ambiguous,
  and the node is skipped.
- A truncated UDP answer is retried over TCP: a record set bigger than one UDP reply (about a dozen values)
  needs a server that answers on TCP port 53.
- Changes show up on the first refresh after the record TTL (resolvers cache). Without `nameservers`, the system
  resolver config is re-read on every refresh, so a network or VPN change is picked up. DNSSEC is not checked.

**HTTP JSON** (`type = "http"`, trust 2): `GET url` with your `headers`, which may use `${VAR}` for secrets.

```json
{ "machines": [
  { "name": "agent-01", "id": "i-0abc", "host": "agent-01.corp", "addresses": ["10.20.0.4"],
    "port": 22, "online": true, "user": "sami", "path": "/home/sami",
    "metadata": { "tags": ["dev", "agent"], "env": "dev" } } ] }
```

- `name` is required (else `id`). With `host` present, `addresses` is ignored, so CIDR rules see the real connect
  target. `driver` is ignored. Metadata scalars become strings; nested values are dropped.
- The URL must be `https://`, or `http://` only to `127.0.0.1`, `[::1]` or `localhost`.
- Redirects are never followed, so your auth header can't leak.
- Limits: a 10s timeout, a 1 MiB body, the first 1000 entries. A non-2xx status marks the provider failed
  (`HTTP 401`) and keeps its last good view. Header values are never logged.

## Mount drivers

| driver | Linux | macOS |
|---|---|---|
| `sshfs` | sshfs 3 + fuse3 (`fusermount3`) | sshfs with macFUSE or FUSE-T |
| `rclone` | `rclone mount` + fuse3 | `rclone mount` with macFUSE or FUSE-T |
| `rclone-nfs` | unavailable ("macOS only") | `rclone nfsmount` (needs `/sbin/mount_nfs`, no FUSE) |

`driver = "auto"` (the default) takes the first available driver in `auto_order`:

| OS | default `auto_order` |
|---|---|
| Linux | `sshfs`, `rclone` |
| macOS | `rclone-nfs`, `rclone`, `sshfs` |

`auto` is sticky: a mount keeps its driver until its spec changes, so a driver that comes and goes never remounts
anything. `bifrost drivers` shows what each probe found and which driver `auto` picks.

rclone always goes through OpenSSH via `--sftp-ssh` and never uses its internal SSH library, which skips host-key
checks. It runs with `--config=/dev/null` and a per-mount VFS cache in `<state>/rclone/<id>`. rclone logs "No host
key validation is being performed". That refers to the unused internal library.

Unmounting is graceful by default. A busy mount stays mounted and is retried later. `bifrost unmount --force`,
dead mounts and mounts hung past `offline_grace_period` get a lazy detach (`fusermount3 -uz`; `diskutil unmount force` on macOS). Bifröst never kills
a process holding your files. The only process it ever signals is its own mount child that never finished
mounting.

## CLI

Global flags: `--json` (print the API's JSON), `--socket PATH`, `--config PATH` (read only by `config check`,
`doctor` and a daemon-less `drivers`; the daemon's own config comes from `$BIFROST_CONFIG`, else the default path).

| command | what it does |
|---|---|
| `bifrost status` | daemon, config, root, machine and mount counts, providers, drivers |
| `bifrost machines` / `machines list` | `NAME SOURCE ADDRESS STATE MOUNTED` |
| `bifrost machines show <id>` | verdict, address, tags, metadata, shadowed providers, mounts |
| `bifrost mounts` | `ID MACHINE DRIVER STATE LOCAL REMOTE` (+ `ERROR`) |
| `bifrost mount <target> [--no-wait]` | a mount id, or a machine id (all its mounts; a mount id wins, see below): clear the hold, retry now, wait |
| `bifrost unmount <target> [--force] [--no-wait]` | same targets as `mount`; unmount and hold down until `bifrost mount`; `--force` = lazy detach |
| `bifrost discover` | refresh every provider now |
| `bifrost reconcile` | re-probe drivers, run one pass, print its plan |
| `bifrost drivers` | driver probes and the `auto` choice (probed locally when the daemon is down) |
| `bifrost doctor` | config, daemon, ssh + agent, discovery, drivers, FUSE (works without the daemon) |
| `bifrost config check [PATH]` | validate a config file, no daemon needed |
| `bifrost config reload` | make the daemon reload now |
| `bifrost daemon status` | running or not |

An id that is also a mount's id means that mount only, even when it is a machine id too. With the example config
above, `bifrost unmount build` unmounts `build` and leaves `build-artifacts` mounted; name the others as well
(`bifrost unmount build-artifacts`). The TUI's `m`/`u`/`U` on a machine row always cover all its mounts.

Exit codes: `0` ok · `1` failed (API error, mount failed, unmount busy, invalid config; for `doctor`: a missing or
invalid config file, no usable driver, or the macOS permission hint) · `2` usage · `3` daemon not reachable.

`bifrostd` takes only `--version` and `--help`. Everything else comes from the `BIFROST_*` environment variables.
Live events stream over SSE: `curl -sN --unix-socket "$sock" http://bifrost/v1/events`.

## TUI

`bifrost-tui [--socket PATH]` polls the daemon every second. Views: Overview, Machines, Mounts, Discovery,
Drivers, Events, Logs.

| key | action |
|---|---|
| `1`–`7`, Tab, Shift-Tab | switch view |
| ↑/↓, `j`/`k` | select |
| `m` | mount the selection (a machine means all its mounts) |
| `u` | unmount |
| `U` | force unmount: lazy detach, never kills (asks y/n) |
| `r` | reconcile now |
| `s` | discover now |
| `c` | reload config |
| `d`, Enter | details (verdict, observations, last error, next retry) |
| `l` | logs of the selection |
| `/` | filter (Enter applies, Esc clears) |
| `?` | help |
| `q`, Ctrl-C | quit |

Set `NO_COLOR` for a colourless UI: the glyphs (● ◐ ◌ ○ ✕) still show the state.

## Security model

| PRD §23 principle | how Bifröst enforces it |
|---|---|
| Discovery does not imply trust | Non-static machines are discover-only until a rule allows them. Winner-takes-all by trust. Native ids are matched only by their own provider's filter |
| No SSH passwords | No password key exists (unknown keys are errors). `BatchMode=yes` everywhere, child stdin is `/dev/null` |
| Prefer agent / keys / Tailscale SSH | Always the system `ssh`. `SSH_AUTH_SOCK` is inherited, and `doctor` reports whether the daemon sees it |
| Never weaken host verification | See below |
| Mount only explicit paths | Remote paths come from config or the provider template. Record hints need `honor_hints` and validation. There is no driver hint |
| Never execute discovery-supplied commands | Discovery data can only become a validated name, host, user, path or port. argv is built without a shell, and positionals never start with `-`. rclone runs with `--sftp-shell-type=none` |
| TXT / HTTP data is untrusted | Size and count limits, per-record isolation, whole-node rejection, and escape-stripping on every displayed string (no terminal injection) |
| Local paths: no traversal or collisions | Ids are single lowercase path components. Static locals win and conflicts are reported. Symlinks, files, non-empty or occupied mount points are refused. The root can't be `/` or `$HOME`, and must be yours and not world-writable |
| Local API | A 0600 Unix socket, 0700 directories it creates, a lock against a second daemon, never under `/tmp` by default |

**Host keys are never weakened.** Bifröst passes these ssh options, to sshfs, to rclone's `--sftp-ssh`
and to the preflight:

```
BatchMode=yes ConnectTimeout=10 ServerAliveInterval=15 ServerAliveCountMax=3 ControlMaster=no ControlPath=none
```

Nothing in that list touches `StrictHostKeyChecking`, `UserKnownHostsFile` or `ProxyCommand`, and no config knob
can. Your `~/.ssh/config`, or `mount.ssh_config` passed as `-F`, decides trust. `BatchMode=yes` turns "ask" into
"fail". The preflight and `--sftp-ssh` also pass `-a -x -o ClearAllForwardings=yes -o PermitLocalCommand=no` (sshfs
does the same by itself), so a `ForwardAgent`, `ForwardX11` or port forward in your ssh_config never reaches a mounted
host: the agent is only used to log in.

Before every mount an `ssh -s <host> sftp` preflight runs. Its stderr is shown as the mount's `last_error`.

**"Host key verification failed"** means the host isn't in your `known_hosts` yet, or its key changed. Bifröst
will not accept it for you. Check the key out of band, then add it:

```bash
ssh-keyscan -p 22 agent-01 | ssh-keygen -lf -       # compare these fingerprints with the host's real ones
ssh-keyscan -p 22 agent-01 >> ~/.ssh/known_hosts    # then add it (or just `ssh agent-01` once and answer yes)
bifrost mount agent-01                              # retry now instead of waiting for the backoff
```

## Running under systemd (user service)

`~/.config/systemd/user/bifrost.service`:

```ini
[Unit]
Description=Bifröst: remote machines as local folders

[Service]
ExecStart=%h/.local/bin/bifrostd
ExecReload=/bin/kill -HUP $MAINPID
Environment=BIFROST_LOG=info
Restart=on-failure
# the mount helpers live in the unit's cgroup: only stop bifrostd, so they keep serving and are adopted on restart
KillMode=process

[Install]
WantedBy=default.target
```

```bash
systemctl --user import-environment SSH_AUTH_SOCK   # let the daemon's ssh use your agent (e.g. in your login profile)
systemctl --user daemon-reload
systemctl --user enable --now bifrost
journalctl --user -u bifrost -f
```

- `KillMode=process` is what makes restarts seamless. With the default `control-group`, systemd SIGTERMs the
  sshfs/rclone children too and every mount drops. A stopped daemon leaves its mounts in place. The next start
  adopts them (same process, no remount), and `bifrost unmount <target>` is the way to tear one down.
- Do **not** set `ProtectHome=`, `PrivateMounts=` or other namespace sandboxing. The mounts would land in a
  private namespace that your shell can't see.
- If `bifrost doctor` shows `✗ SSH_AUTH_SOCK not visible to daemon`, import the variable as above and restart the
  service.

## macOS notes

macOS support is **built but not field-tested**:
- the release binaries for `x86_64-apple-darwin` and `aarch64-apple-darwin` are built on GitHub's macOS
  runners, and CI builds and unit-tests the workspace there;
- mounting on a real Mac (macFUSE, FUSE-T, `rclone nfsmount`) has not been exercised;
- from Linux, `cargo check`/`clippy` for `aarch64-apple-darwin` cover core, config, mount, client, cli and tui
  (see Development); `bifrostd` and the discovery crate need an Apple toolchain (reqwest's `ring`).

What the macOS code does:
- FUSE: macFUSE is detected at `/Library/Filesystems/macfuse.fs`, FUSE-T at `/Library/Application Support/fuse-t`
  or `/usr/local/lib/libfuse-t.dylib`. sshfs gets `volname=<id>` (plus `noappledouble` on macFUSE).
- `rclone-nfs` (`rclone nfsmount`) is first in the default `auto_order` because it needs no kernel extension. It
  serves the mount on an unauthenticated 127.0.0.1 NFS port that other local users could reach. On a shared Mac,
  put `rclone` or `sshfs` first. `nfsmount` may also need root. With `vfs_cache_mode` below `writes` it would be
  read-only, so Bifröst raises it to `writes`.
- Mounts are read with `getmntinfo`, unmounted with `/sbin/umount`, and forced with `diskutil unmount force`.
- The socket lives in `~/Library/Caches/bifrost/`.

**Permissions.** macFUSE needs its system extension approved. If a mount's log mentions `kernel extension`,
`System Extension` or `not permitted`, its error gets the hint *(macOS: allow the macFUSE system extension in
System Settings → Privacy & Security)*, and `bifrost doctor` repeats it. Approve it there (and reboot if asked).
Or use FUSE-T or `rclone-nfs`, which need no kernel extension.

## Development

```bash
scripts/check.sh                          # the gate: cargo fmt --check, clippy -D warnings, cargo test --workspace
tests/e2e/run.sh m1                       # E2E milestone 1: p04 p05 p06 psec p13a
tests/e2e/run.sh all                      # + p07 p08 p09 p10 p12 p13
tests/e2e/run.sh p08_dns p10_http p13_hardening   # listed phases only, in that order
```

- **Needs:** docker (a throwaway OpenSSH container on 127.0.0.1/127.0.0.2:2222, CoreDNS on 127.0.0.1:5353), jq,
  curl, pgrep, sshfs, rclone, fusermount3, ssh-keygen, ssh-keyscan, plus python3 and dig for `all`.
- **Isolation:** everything runs under a `mktemp -d` scratch dir: its own keys, known_hosts, socket, state and
  mount root. `~/.ssh` and `~/machines` are never touched.
- **`TMPDIR` must be unset:** the socket path must fit in 103 bytes.
- **One run at a time**, host-wide (`/tmp/bf-e2e.lock`). A second run exits 2 with "another e2e run is active".
  Wait and retry; don't delete the lock.
- **Scratch dirs are kept**, one `/tmp/tmp.*` per run, never `rm -rf`'d (that could recurse into a mount that
  failed to detach). Delete them yourself once nothing is mounted there.
- **p07 is opt-in** (`E2E_TAILSCALE=1`). It runs discovery only against your live tailnet and asserts zero mounts.
  Its scratch dir then holds **real tailnet names**: delete it and don't commit anything from it.
- **Phase files** (`tests/e2e/pNN_*.sh`) define `check_<p>` and optional `setup_<p>` / `config_<p>` (a TOML
  fragment). `run.sh` lists them explicitly; a listed file that is missing is an error. Only p08 may define
  `[policy.*]` tables.
- **Docker mount tests:** `cargo test -p bifrost-mount -- --ignored` runs the real mount tests against that
  container (`BIFROST_E2E_SSH=host:port:user:ssh_config`, exported by `tests/e2e/lib.sh`'s `start_sshd`).
- **macOS check:**

```bash
rustup target add aarch64-apple-darwin
cargo check --target aarch64-apple-darwin -p bifrost-core -p bifrost-config -p bifrost-mount -p bifrost-client -p bifrost-cli -p bifrost-tui
cargo clippy --target aarch64-apple-darwin -p bifrost-core -p bifrost-config -p bifrost-mount -p bifrost-client -p bifrost-cli -p bifrost-tui -- -D warnings
```

Crates: `bifrost-core` (models, policy, registry, planner; no I/O), `bifrost-config`, `bifrost-discovery`,
`bifrost-mount`, `bifrost-client`, `bifrost-daemon` (`bifrostd`), `bifrost-cli` (`bifrost`), `bifrost-tui`.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in Bifröst by you,
as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
