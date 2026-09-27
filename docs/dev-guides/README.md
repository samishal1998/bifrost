# Bifröst developer guides

This directory is Bifröst's internal engineering documentation. It is for a developer who has to understand,
maintain and extend the code: how the system is built, why each design choice was made and what it costs, what every
crate does, and how to change each part safely. This page is the index. It explains which documentation lives where,
gives a reading order and a one-line map of every guide, answers "where do I change X", and lists the conventions
every change follows.

## Contents

- [What lives where](#what-lives-where)
- [Reading order](#reading-order)
- [The guides](#the-guides)
- [The crates](#the-crates)
- [Where do I change X](#where-do-i-change-x)
- [Conventions](#conventions)
- [Citation codes](#citation-codes)
- [Keeping these guides true](#keeping-these-guides-true)

## What lives where

| Source | Audience | What it holds |
|---|---|---|
| `crates/`, `tests/`, `scripts/`, `.github/`, `install.sh` | everyone | The behaviour. **The source of truth.** |
| `docs/dev-guides/` (here) | maintainers | How it is built and why, per crate and across crates. Every claim was checked against the code |
| [`site/`](../../site/), published at https://samishal1998.github.io/bifrost/ | users | Install, concepts, guides, examples, reference. How it is maintained: [docs-site.md](docs-site.md) |
| [README.md](../../README.md) | users arriving on GitHub | Overview, install, the full config example, policy, providers, drivers, CLI, TUI, the security summary, a Development section |
| [bifrost_prd_and_implementation_plan.md](../../bifrost_prd_and_implementation_plan.md) | history | The PRD: product intent, cited as "PRD §n" |
| [docs/design/contract.md](../design/contract.md) | history | The implementation contract agreed during the build (§1–§15). The "Amendments (applied)" table and the "Orchestrator sign-offs" sections override earlier text |
| [docs/design/critique.md](../design/critique.md) | history | The completeness critique of the contract (sections A–E) that produced the amendments and the E1–E6 cuts |
| `git log` | history | Why each change was made: review findings, sign-offs, fixes. Decision entries cite commit hashes |

When they disagree, the code wins, then these guides, then the contract (amendments and sign-offs before the text
they amend), then the PRD. A guide that contradicts the code is a doc bug: fix the guide. Where the code departs from
the contract on purpose, the guide says so in its "Where the code differs from the contract" section, for example
[architecture.md#contract-vs-code](architecture.md#contract-vs-code).

## Reading order

For a developer new to the project:

1. **Build, and run the gate**, so you know the tree is green before you change it: `cargo build --workspace`, then
   `scripts/check.sh` ([testing.md#gate](testing.md#gate)).
2. [architecture.md](architecture.md): the whole system in one pass. Keep [glossary.md](glossary.md) open next to it.
3. [decisions.md](decisions.md): skim its contents table, then read the entries for the area you will touch.
4. [security.md](security.md): before you touch argv, discovery parsing, file handling or logging.
5. The [crate guides](#the-crates) in pipeline order (core, config, discovery, mount, daemon, client, cli, tui), or
   only the one you are changing.
6. [testing.md](testing.md), then [e2e-harness.md](e2e-harness.md).
7. [extending.md](extending.md) before your first change, and [simplifications.md](simplifications.md) so you can
   tell a deliberate ceiling from a bug.
8. [release-and-ci.md](release-and-ci.md) and [docs-site.md](docs-site.md) when you ship, or change something users
   can see.

## The guides

**System**

| Guide | What it covers |
|---|---|
| [architecture.md](architecture.md) | The end-to-end system: goals, the pipeline, crate boundaries, the process model and message flow, the data model and state machines, the lifecycle (startup, adoption, warm-up, reload, shutdown, crash), the local API, runtime files, the platform matrix, where the code differs from the contract |
| [decisions.md](decisions.md) | One anchored entry per design choice: status, context, decision, alternatives rejected, consequences, where in code, source |
| [glossary.md](glossary.md) | Domain terms, and the tags used in code comments and design documents |
| [simplifications.md](simplifications.md) | The ponytail ledger: every deliberate corner, its ceiling and its upgrade path, reconciled against contract §15 |
| [security.md](security.md) | Assets and attackers, trust boundaries and what enforces them, argv and host-key rules, filesystem permissions, secrets, cleaning of untrusted text, the log allowlist, residual risks, review history, rules for changes |

**Testing, release and docs**

| Guide | What it covers |
|---|---|
| [testing.md](testing.md) | The test layers, how to run each, the conventions every test follows, `scripts/check.sh`, and which test pins which risk |
| [e2e-harness.md](e2e-harness.md) | `tests/e2e`: `run.sh` modes, the lock, the scratch directory, fixtures, `lib.sh`, every phase, adding a phase, debugging |
| [release-and-ci.md](release-and-ci.md) | `ci.yml`, `release.yml`, `pages.yml`, the packaging contract, `install.sh` and its test, versioning, the release procedure |
| [docs-site.md](docs-site.md) | The Astro Starlight user docs in `site/`: layout, overrides, branding, which page documents which code, deployment, which docs a change touches |
| [extending.md](extending.md) | Recipes: a provider, a driver, a config key, an API route with its CLI command and TUI key, an event, an E2E phase, the `bf1` grammar, the fingerprint |

**Crates**

| Guide | What it covers |
|---|---|
| [crates/core.md](crates/core.md) | `bifrost-core`: validated types, model, policy, registry, the pure reconciler, events, DTOs, fakes |
| [crates/config.md](crates/config.md) | `bifrost-config`: TOML to a validated `Config`, deterministic errors, expansion, default paths per OS |
| [crates/discovery.md](crates/discovery.md) | `bifrost-discovery`: the `tailscale`, `dns` (`bf1`) and `http` providers |
| [crates/mount.md](crates/mount.md) | `bifrost-mount`: the drivers, the mount table, hung-FUSE checks, the ssh preflight, adoption |
| [crates/daemon.md](crates/daemon.md) | `bifrostd`: startup steps and exit codes, the actor, API routes, state.json, the config poller |
| [crates/client.md](crates/client.md) | `bifrost-client`: the HTTP/1 client over the Unix socket, and its error mapping |
| [crates/cli.md](crates/cli.md) | `bifrost`: commands, waiting for results, rendering, `doctor`, exit codes |
| [crates/tui.md](crates/tui.md) | `bifrost-tui`: a pure `App` and renderer, the inline polling loop, views, keys |

## The crates

The workspace is `members = ["crates/*"]`, edition 2024, `rust-version = "1.89"` (for `File::try_lock`). Why eight
crates: [decisions.md#rust-and-8-crate-workspace](decisions.md#rust-and-8-crate-workspace).

| Crate | Binary | Workspace deps | Holds |
|---|---|---|---|
| [bifrost-core](crates/core.md) | | none (only `serde`, `thiserror`) | Everything pure. No I/O ([decisions.md#core-no-io](decisions.md#core-no-io)) |
| [bifrost-config](crates/config.md) | | core | `parse`, `paths`, and the names of concrete providers and drivers (`TRUST`, `DRIVER_NAMES`, `default_auto_order`) |
| [bifrost-discovery](crates/discovery.md) | | core | `TailscaleProvider`, `DnsProvider`, `HttpProvider` |
| [bifrost-mount](crates/mount.md) | | core | All process, FUSE and OS-specific code |
| [bifrost-daemon](crates/daemon.md) | `bifrostd` | core, config, discovery, mount (dev: client) | `main.rs`, `actor.rs`, `api.rs`, `state.rs`, `reload.rs` |
| [bifrost-client](crates/client.md) | | none | `Client::get`, `Client::post` |
| [bifrost-cli](crates/cli.md) | `bifrost` | core, config, client, mount | mount is used by `doctor` and the daemon-less `drivers` |
| [bifrost-tui](crates/tui.md) | `bifrost-tui` | core, config, client | config supplies the default socket path |

## Where do I change X

"Start here" is the guide section to read first. "Also" lists what else moves and the decision behind the current
design; read the decision before you change the behaviour it records.

**Configuration, discovery and policy**

| To change… | Code | Start here | Also |
|---|---|---|---|
| Add a config key | `crates/bifrost-config/src/raw.rs`, `lib.rs` | [extending.md#config-key](extending.md#config-key) | [crates/config.md#changing-this-crate](crates/config.md#changing-this-crate), [decisions.md#config-deterministic-validation](decisions.md#config-deterministic-validation) |
| A default value or a fixed timing | `crates/bifrost-config/src/lib.rs`, or a constant at its use | [crates/config.md#defaults](crates/config.md#defaults) | Most timings are constants on purpose: [simplifications.md#ledger](simplifications.md#ledger) |
| Default config, state or socket paths | `crates/bifrost-config/src/paths.rs` | [crates/config.md#paths-default-locations](crates/config.md#paths-default-locations) | [architecture.md#filesystem-layout](architecture.md#filesystem-layout) |
| How a config edit is picked up | `crates/bifrost-daemon/src/reload.rs` | [crates/daemon.md#reload](crates/daemon.md#reload) | [decisions.md#config-polling-not-notify](decisions.md#config-polling-not-notify) |
| Add a discovery provider | `crates/bifrost-discovery/src/`, `bifrost-config`, `bifrost-daemon/src/main.rs` | [extending.md#provider](extending.md#provider) | [crates/discovery.md#changing](crates/discovery.md#changing), [decisions.md#discover-is-not-mount](decisions.md#discover-is-not-mount) |
| The `bf1` DNS record format | `crates/bifrost-discovery/src/dns.rs` | [extending.md#bf1](extending.md#bf1) | [crates/discovery.md#bf1-grammar](crates/discovery.md#bf1-grammar), [security.md#hostile-records](security.md#hostile-records), [decisions.md#bf1-dns-format-inline-and-index](decisions.md#bf1-dns-format-inline-and-index) |
| Tailscale or HTTP inventory parsing | `crates/bifrost-discovery/src/tailscale.rs`, `http.rs` | [crates/discovery.md#tailscale](crates/discovery.md#tailscale), [crates/discovery.md#http](crates/discovery.md#http) | [decisions.md#tailscale-via-cli-json](decisions.md#tailscale-via-cli-json), [decisions.md#http-inventory-limits](decisions.md#http-inventory-limits) |
| Policy rules or trust order | `crates/bifrost-core/src/policy.rs` (`evaluate`), `registry.rs`; `TRUST` in `bifrost-config` | [crates/core.md#policyrs-who-may-be-mounted](crates/core.md#policyrs-who-may-be-mounted) | [decisions.md#policy-semantics](decisions.md#policy-semantics), [decisions.md#winner-takes-all-trust](decisions.md#winner-takes-all-trust) |
| Accept a new untrusted string | `crates/bifrost-core/src/validate.rs` | [crates/core.md#validaters-the-trust-boundary](crates/core.md#validaters-the-trust-boundary) | [security.md#trust-boundaries](security.md#trust-boundaries), [decisions.md#validated-newtypes-at-trust-boundary](decisions.md#validated-newtypes-at-trust-boundary) |

**Mounting and the daemon**

| To change… | Code | Start here | Also |
|---|---|---|---|
| Add a mount driver | `crates/bifrost-mount/src/<name>.rs`, `lib.rs`; `DRIVER_NAMES` in `bifrost-config` | [extending.md#driver](extending.md#driver) | [crates/mount.md#changing](crates/mount.md#changing), [crates/mount.md#driver-contract](crates/mount.md#driver-contract) |
| An ssh option passed to ssh, sshfs or rclone | `crates/bifrost-mount/src/lib.rs` (`SSH_OPTS`, `SSH_CLI_HARDENING`, `preflight_argv`) | [crates/mount.md#ssh-options](crates/mount.md#ssh-options) | [security.md#argv-rules](security.md#argv-rules), [security.md#host-keys](security.md#host-keys), [decisions.md#host-keys-never-weakened](decisions.md#host-keys-never-weakened) |
| What happens next for a mount (decision table, retries, backoff) | `crates/bifrost-core/src/reconcile.rs` (`decide`) | [crates/core.md#reconcilers-the-pure-reconciler](crates/core.md#reconcilers-the-pure-reconciler) | [architecture.md#decision-table](architecture.md#decision-table), [decisions.md#pure-planner-decision-table](decisions.md#pure-planner-decision-table) |
| How the actor handles a message or runs a pass | `crates/bifrost-daemon/src/actor.rs` (`Msg`, `pass`) | [crates/daemon.md#messages](crates/daemon.md#messages), [crates/daemon.md#pass](crates/daemon.md#pass) | [decisions.md#single-actor-daemon](decisions.md#single-actor-daemon), [decisions.md#generations-for-stale-results](decisions.md#generations-for-stale-results) |
| A timeout on a filesystem call | `crates/bifrost-mount/src/check.rs` | [crates/mount.md#timed](crates/mount.md#timed) | [decisions.md#hung-fuse-guards](decisions.md#hung-fuse-guards) |
| Unmount behaviour | `crates/bifrost-mount/src/lib.rs` | [crates/mount.md#unmount](crates/mount.md#unmount) | [decisions.md#busy-unmount-never-forced](decisions.md#busy-unmount-never-forced), [decisions.md#no-pid-signalling-lazy-detach](decisions.md#no-pid-signalling-lazy-detach) |
| The fingerprint or adoption | `crates/bifrost-core/src/model.rs` (`fingerprint`, `marker`), `crates/bifrost-mount/src/lib.rs` (`adopt`) | [extending.md#fingerprint](extending.md#fingerprint) | [crates/mount.md#adopt](crates/mount.md#adopt), [decisions.md#adoption-by-marker-and-fingerprint](decisions.md#adoption-by-marker-and-fingerprint) |
| The state.json format | `crates/bifrost-daemon/src/state.rs` | [crates/daemon.md#state-json](crates/daemon.md#state-json) | [decisions.md#state-json-is-a-hint](decisions.md#state-json-is-a-hint) |
| Daemon startup or its exit codes | `crates/bifrost-daemon/src/main.rs` | [crates/daemon.md#startup-steps](crates/daemon.md#startup-steps) | [architecture.md#startup](architecture.md#startup) |
| Which log targets are shown | `crates/bifrost-daemon/src/main.rs` (`log_filter`) | [crates/daemon.md#tracing](crates/daemon.md#tracing) | [security.md#log-allowlist](security.md#log-allowlist), [decisions.md#log-filter-allowlist](decisions.md#log-filter-allowlist) |
| macOS-specific behaviour | `crates/bifrost-mount/src/` | [crates/mount.md#macos](crates/mount.md#macos) | [architecture.md#platforms](architecture.md#platforms) |

**API and clients**

| To change… | Code | Start here | Also |
|---|---|---|---|
| Add an API route, CLI command and TUI key | `crates/bifrost-daemon/src/api.rs` (`router`), `crates/bifrost-cli/src/main.rs`, `crates/bifrost-tui/src/` | [extending.md#api-cli-tui](extending.md#api-cli-tui) | [crates/daemon.md#routes](crates/daemon.md#routes), [crates/cli.md#adding-or-changing-a-command](crates/cli.md#adding-or-changing-a-command), [crates/tui.md#changing-the-tui](crates/tui.md#changing-the-tui) |
| A field on the wire | `crates/bifrost-core/src/api.rs` | [crates/core.md#apirs-wire-dtos](crates/core.md#apirs-wire-dtos) | [crates/core.md#changing-core-what-else-moves](crates/core.md#changing-core-what-else-moves), [decisions.md#http-over-unix-socket](decisions.md#http-over-unix-socket) |
| Add an event | `crates/bifrost-core/src/events.rs` | [extending.md#event](extending.md#event) | [crates/daemon.md#events](crates/daemon.md#events), [decisions.md#sse-events-and-polling-clients](decisions.md#sse-events-and-polling-clients) |
| CLI exit codes or output | `crates/bifrost-cli/src/main.rs` (`run`), `output.rs` | [crates/cli.md#exit-codes](crates/cli.md#exit-codes) | [crates/client.md#error-mapping](crates/client.md#error-mapping), [decisions.md#cli-exit-codes](decisions.md#cli-exit-codes) |
| A TUI view, key or colour | `crates/bifrost-tui/src/app.rs` (`App`, `Command`), `ui.rs` (`render`) | [crates/tui.md#views](crates/tui.md#views), [crates/tui.md#keys](crates/tui.md#keys) | [decisions.md#tui-inline-polling](decisions.md#tui-inline-polling) |

**Tests, release, docs and process**

| To change… | Code | Start here | Also |
|---|---|---|---|
| Add a test (which layer?) | per crate, `tests/e2e/` | [testing.md#choosing](testing.md#choosing) | [testing.md#conventions](testing.md#conventions) |
| Add an E2E phase | `tests/e2e/pNN_<what>.sh`, `run.sh` | [extending.md#e2e-phase](extending.md#e2e-phase) | [e2e-harness.md#adding](e2e-harness.md#adding), [decisions.md#e2e-docker-harness](decisions.md#e2e-docker-harness) |
| CI | `.github/workflows/ci.yml`, `scripts/check.sh` | [release-and-ci.md#ci](release-and-ci.md#ci) | [testing.md#gate](testing.md#gate) |
| Release targets or packaging | `.github/workflows/release.yml` | [release-and-ci.md#matrix](release-and-ci.md#matrix), [release-and-ci.md#packaging](release-and-ci.md#packaging) | [decisions.md#release-static-musl-and-darwin](decisions.md#release-static-musl-and-darwin) |
| Cut a release | workspace `Cargo.toml` version, a `v*` tag | [release-and-ci.md#procedure](release-and-ci.md#procedure) | [release-and-ci.md#tags](release-and-ci.md#tags) |
| The installer | `install.sh`, `scripts/test-install.sh` | [release-and-ci.md#install](release-and-ci.md#install) | [release-and-ci.md#test-install](release-and-ci.md#test-install), [decisions.md#install-sh-verify-always](decisions.md#install-sh-verify-always) |
| A user docs page | `site/src/content/docs/` | [docs-site.md#which-docs](docs-site.md#which-docs) | [docs-site.md#content-map](docs-site.md#content-map), [decisions.md#docs-starlight-on-pages](decisions.md#docs-starlight-on-pages) |
| Cut a corner, or lift a ceiling | a `ponytail:` comment at the site | [simplifications.md](simplifications.md) | [decisions.md#ponytail-style](decisions.md#ponytail-style) |
| Record a design decision | [decisions.md](decisions.md) | [decisions.md#adding-a-decision](decisions.md#adding-a-decision) | [decisions.md#citations](decisions.md#citations) |

## Conventions

- **Ponytail.** Build the simplest thing that works. A constant stays a constant until someone needs a knob. Every
  deliberate corner with a known ceiling gets a `// ponytail: <ceiling>; <upgrade path>` comment where it lives and a
  row in [simplifications.md](simplifications.md). Lifting a ceiling deletes the comment and updates the ledger. See
  [decisions.md#ponytail-style](decisions.md#ponytail-style).
- **The gate before every commit.** `scripts/check.sh` runs `cargo fmt --all --check`,
  `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`. CI's Linux job runs the same
  script; the macOS job builds and runs the tests only ([release-and-ci.md#ci](release-and-ci.md#ci)). Behaviour
  that crosses processes or needs a real tool (FUSE mounts, adoption, signals, reload by file edit, DNS over TCP) is
  proven only by `tests/e2e/run.sh`, so a change there also runs the relevant phases
  ([testing.md#choosing](testing.md#choosing)).
- **Tests** come first and are named for the behaviour they pin. They use fakes instead of mocks, never call
  `set_var`, and pass the binary search path in explicitly ([testing.md#conventions](testing.md#conventions)).
- **Security rules** for argv, file handling, logging and discovery parsing are listed in
  [security.md#rules-for-changes](security.md#rules-for-changes).
- **Commit messages.** Subjects are `type(scope): summary` with type `feat`, `fix`, `docs`, `test`, `chore` or `ci`
  (for example `fix(dns): …`), `merge <branch>[: <summary>]` for merges, and `release: vX.Y.Z`. Some early commits
  use a bare scope (`core: …`). The body says why, usually as short bullets, and names the review finding or
  sign-off it answers. Commits written with an AI agent end with its attribution trailers (`Co-Authored-By: …`, and
  `Claude-Session: …` when the session provides one).
- **History is append-only.** Fix a mistake with a new commit, never by rewriting pushed history: no force-push to
  `main`. A pushed release tag is never deleted, moved or force-pushed, even when the release is broken
  ([release-and-ci.md#tags](release-and-ci.md#tags)).
- **Parallel work** uses git worktrees that share one `CARGO_TARGET_DIR`, so only one cargo build runs at a time,
  and branches merge one at a time, with `scripts/check.sh` after each merge
  ([decisions.md#shared-target-dir-and-worktrees](decisions.md#shared-target-dir-and-worktrees)).
- **Code comments** cite the design history with short codes (`A15`, `B8`, `§5`, `S4a`), decoded in
  [glossary.md#tags](glossary.md#tags).

## Citation codes

Codes such as A9, B2, E6, S4a.11, r2 and §15 #n point into the design history. They are decoded in
[decisions.md#citations](decisions.md#citations) and [glossary.md#tags](glossary.md#tags).

## Keeping these guides true

When you change code, update in the same change:

- the crate guide for that code, and [architecture.md](architecture.md) if the change crosses crates;
- the decision entry, if the change alters a decision. Add a new slug for a new decision and never rename an
  existing one, because other guides link to it ([decisions.md#adding-a-decision](decisions.md#adding-a-decision));
- the ledger in [simplifications.md](simplifications.md) when you add or lift a `ponytail:` corner;
- the recipe in [extending.md](extending.md) if the steps to add something changed.

Cite code as a repo-relative path plus an item name (a function, type or constant), never a line number, because
line numbers drift. Keep relative links and `#anchors` working. For which user-facing docs a change touches, see
[docs-site.md#which-docs](docs-site.md#which-docs).
