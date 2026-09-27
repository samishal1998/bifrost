# Release and CI

This guide covers everything between a commit on `main` and a user running `curl … | sh`: the three GitHub Actions
workflows (`ci.yml`, `release.yml`, `pages.yml`), the release packaging contract that `install.sh` and the README rely
on, the design of `install.sh` and its test, how versions are numbered, and the release procedure step by step as it
was actually run for v0.1.0 and v0.1.1. The reasons for the release shape (static musl, SHA256SUMS, no sudo) are
recorded in [decisions.md#release-static-musl-and-darwin](decisions.md#release-static-musl-and-darwin) and
[decisions.md#install-sh-verify-always](decisions.md#install-sh-verify-always).

## Contents

- [Workflows at a glance](#workflows)
- [ci.yml](#ci)
- [release.yml](#release)
  - [Build matrix and toolchain](#matrix)
  - [Packaging contract](#packaging)
  - [The release job and dry runs](#release-job)
  - [Injection safety and permissions](#hardening)
- [pages.yml](#pages)
- [install.sh](#install)
- [scripts/test-install.sh](#test-install)
- [Versioning](#versioning)
- [Release procedure](#procedure)
- [Never move a pushed tag](#tags)
- [When something goes wrong](#troubleshooting)

<a id="workflows"></a>
## Workflows at a glance

| Workflow | Trigger | Jobs | Token permissions |
|---|---|---|---|
| `.github/workflows/ci.yml` | push to `main`, any pull request | `linux`, `macos` | `contents: read` |
| `.github/workflows/release.yml` | push of a `v*` tag; `workflow_dispatch` with input `dry_run` (default `true`) | `build` (4-target matrix), `release` | `contents: read`; `release` job `contents: write` |
| `.github/workflows/pages.yml` | push to `main` touching `site/**`, `install.sh`, `brand/**` or the workflow; `workflow_dispatch` | `build`, `deploy` | `contents: read`; `deploy` job `pages: write`, `id-token: write` |

All checkouts use `persist-credentials: false`, so the token is not left in `.git/config` for later steps. Actions are
pinned to major-version tags (`actions/checkout@v7`, `actions/cache@v6`, `actions/setup-node@v7`,
`actions/upload-artifact@v7`, `actions/download-artifact@v8`, `actions/upload-pages-artifact@v5`,
`actions/deploy-pages@v5`; bumped together in 12044f4), not to commit SHAs.

<a id="ci"></a>
## ci.yml

```yaml
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:
  CARGO_TERM_COLOR: always
  CARGO_INCREMENTAL: 0
```

| Job | Runner | Steps |
|---|---|---|
| `linux` | ubuntu-24.04 | checkout; cache `~/.cargo/registry` and `target` keyed on `hashFiles('Cargo.lock')`; `rustup show && rustup component add rustfmt clippy`; `scripts/check.sh`; `cargo build --workspace`; `bash scripts/test-install.sh` |
| `macos` | macos-15 | checkout; the same cache; `cargo build --workspace --all-targets`; `cargo test --workspace` |

- **Concurrency** cancels a superseded run on the same ref, so a quick second push doesn't queue behind the first.
- **`CARGO_INCREMENTAL: 0`**: CI builds write no incremental artifacts. No reason is recorded (e30f0aa); presumably
  because a fresh runner can't reuse them and they would only grow the cached `target`.
- **`CARGO_TARGET_DIR`** is pinned to `${{ github.workspace }}/target` in the Linux job so `scripts/check.sh`,
  `cargo build` and `scripts/test-install.sh` (which reads `${CARGO_TARGET_DIR:-target}/debug`) all use the cached
  directory.
- **macos-15**, not macos-14: macos-14 runners retire on 2026-11-02 with brownouts in October (12044f4). The macOS job
  caught the `/var` → `/private/var` symlink failure in the daemon test rig: the `macos` job of the "merge p-prep" run
  failed while `linux` passed, and 0c7f17a fixed it.
- **The Linux `cargo build --workspace` step is redundant.** `scripts/test-install.sh` builds `bifrost-cli`,
  `bifrost-daemon` and `bifrost-tui` itself when called without a `BIN_DIR` (d9ecdf8). The two fixes landed on
  parallel branches (p-install and p-workflows) and both were kept; whichever build runs second has nothing to do.

What CI does **not** do:

- run the `#[ignore]` tests or the E2E harness (Docker, FUSE, sshfs and rclone on the runner);
- run fmt or clippy on macOS, or the darwin cross-check from the README;
- test the MSRV: `rust-version = "1.89"` (for `File::try_lock`) is declared in `Cargo.toml`, but the jobs use the
  runner's default stable toolchain.

<a id="release"></a>
## release.yml

<a id="matrix"></a>
### Build matrix and toolchain

| Target | Runner | Notes |
|---|---|---|
| `x86_64-unknown-linux-musl` | ubuntu-24.04 | `musl-tools`; `CC_x86_64_unknown_linux_musl=musl-gcc` |
| `aarch64-unknown-linux-musl` | ubuntu-24.04-arm | native arm64 runner; `CC_aarch64_unknown_linux_musl=musl-gcc` |
| `x86_64-apple-darwin` | macos-15 | cross-compiled on the Apple Silicon runner by Apple's toolchain |
| `aarch64-apple-darwin` | macos-15 | native |

The Toolchain step:

```bash
rustup target add "$TARGET"
if [[ $TARGET == *-linux-musl ]]; then
  sudo apt-get update -qq && sudo apt-get install -y -qq musl-tools
  # cc-rs looks for aarch64-linux-musl-gcc; Ubuntu's musl-tools only ships musl-gcc
  echo "CC_${TARGET//-/_}=musl-gcc" >> "$GITHUB_ENV"
fi
```

C code is compiled because reqwest uses rustls on `ring`, which builds C (contract "Verified facts"). The `CC_<target>`
variable, with dashes turned into underscores, is how `cc-rs` is told which compiler to use for that target.

The build:

```bash
cargo build --release --locked --target "$TARGET" -p bifrost-cli -p bifrost-daemon -p bifrost-tui
```

- `--locked` fails the build if `Cargo.lock` doesn't match `Cargo.toml`. That is why the version bump commit must
  include the regenerated lockfile ([procedure](#procedure)).
- Only the three binary crates are named; the libraries come in as dependencies.
- The release profile (`Cargo.toml`): `lto = "thin"`, `codegen-units = 1`, `strip = true`, and deliberately **not**
  `panic = "abort"`: a panicking task must not kill the daemon
  ([decisions.md#panics-caught-at-driver-boundary](decisions.md#panics-caught-at-driver-boundary)).

`defaults.run.shell: bash` makes every step run under bash with `-eo pipefail` on all runners, which the `[[ … ]]`,
`${TARGET//-/_}` and brace expansion in these steps rely on.

<a id="packaging"></a>
### Packaging contract

```bash
d="bifrost-$TARGET"
mkdir "$d"
cp "target/$TARGET/release/"{bifrost,bifrostd,bifrost-tui} README.md LICENSE-MIT LICENSE-APACHE "$d/"
tar -czf "$d.tar.gz" "$d"
```

with `COPYFILE_DISABLE: 1` in the step's environment, which stops macOS `tar` from adding `._*` AppleDouble entries.

| Item | Contract | Relied on by |
|---|---|---|
| asset name | `bifrost-<target>.tar.gz`, **no version in the name** | `install.sh` (`asset=bifrost-$target.tar.gz`), the README's manual install, `scripts/test-install.sh` |
| archive layout | one top-level directory `bifrost-<target>/` holding `bifrost`, `bifrostd`, `bifrost-tui`, `README.md`, `LICENSE-MIT`, `LICENSE-APACHE` | `install.sh` (`src=$tmp/bifrost-$target`, checks each binary exists) |
| targets | the four above | `install.sh`'s `uname` mapping, the README |
| checksums | one `SHA256SUMS` per release, `sha256sum` text format (`<hex>  <asset>`) | `install.sh` (accepts `<asset>` and `*<asset>` in field 2), the README's `sha256sum -c` recipe |

Because asset names carry no version, `https://github.com/samishal1998/bifrost/releases/latest/download/<asset>`
always resolves to the newest published release; that URL is `install.sh`'s default. Renaming an asset, adding a
target or changing the layout is a breaking change for every installer already on users' machines and for the
README: change `install.sh`, `scripts/test-install.sh`, the README and `site/src/content/docs/installation.mdx` in
the same release.

Each build uploads its tarball as the workflow artifact `bifrost-<target>` (`if-no-files-found: error`).

<a id="release-job"></a>
### The release job and dry runs

```yaml
release:
  needs: build
  if: startsWith(github.ref, 'refs/tags/v') && (github.event_name == 'push' || !inputs.dry_run)
```

| How it was started | Ref | `release` job |
|---|---|---|
| `git push origin vX.Y.Z` | the tag | runs: publishes |
| Actions → release → Run workflow on `main` (either checkbox state) | a branch | skipped: artifacts only |
| Run workflow on a `v*` tag with `dry_run` checked (the default) | the tag | skipped: artifacts only |
| Run workflow on a `v*` tag with `dry_run` unchecked | the tag | runs: publishes |

So a dispatch on `main` never publishes, whatever the checkbox says; the checkbox only matters on a tag ref. The
dry run used before each release is a dispatch on `main`: it builds all four targets (the only other place the
musl and darwin release builds run is the tag push itself) and leaves the tarballs as downloadable artifacts.

When it runs, the job:

1. downloads every `bifrost-*` artifact into `dist/` (`merge-multiple: true`);
2. in `dist/`, runs `sha256sum bifrost-*.tar.gz > SHA256SUMS` and prints it;
3. runs `gh release create "$GITHUB_REF_NAME" --repo "$GITHUB_REPOSITORY" --verify-tag --title "Bifröst $GITHUB_REF_NAME" --generate-notes dist/*`.

`--verify-tag` refuses to create the release if the tag doesn't exist on the remote, so `gh` can never create a tag
itself. `--generate-notes` builds the notes from the pull requests merged since the previous release; this repository
merges without PRs, so the generated notes are just the "Full Changelog" compare link (v0.1.0...v0.1.1 for v0.1.1). The checksums are computed in the same job from the same files
that are uploaded.

<a id="hardening"></a>
### Injection safety and permissions

- **No `${{ }}` inside `run:` scripts for values that vary.** `${{ matrix.target }}` enters as `env: TARGET` and the
  scripts use `"$TARGET"`; the release step uses the runner's `$GITHUB_REF_NAME` and `$GITHUB_REPOSITORY`. A `${{ }}`
  expression is pasted into the script text before bash parses it, so a crafted value (a tag name, for instance)
  could inject shell code; an environment variable is only ever data. Keep new steps to this rule.
- **Least privilege.** The workflow default is `contents: read`. Only the `release` job gets `contents: write` (it
  creates the release), and it runs only after every build succeeded. The `gh` CLI gets the token through
  `GH_TOKEN: ${{ github.token }}` in that step's environment only.
- **`persist-credentials: false`** on checkout: nothing in the build needs to push.

<a id="pages"></a>
## pages.yml

Builds the Starlight site and deploys it to GitHub Pages (Pages is configured with build type "workflow", i.e.
GitHub Actions as the source).

```bash
cd site
npm ci
mkdir -p public && cp ../install.sh public/install.sh     # the one-liner URL serves install.sh from main
npm run build                                             # → site/dist, uploaded with upload-pages-artifact
```

- Node 22 (`actions/setup-node`).
- `concurrency` group `pages` with `cancel-in-progress: false`: a deploy in progress is never cancelled half-way; the
  next one waits.
- The `deploy` job runs in the `github-pages` environment with `pages: write` and `id-token: write` (the OIDC token
  `actions/deploy-pages` needs).
- `install.sh` is in the trigger paths because the site serves it: `https://samishal1998.github.io/bifrost/install.sh`
  is the README's install URL. A change to `install.sh` on `main` is live for new installs after this workflow runs.
- `brand/**` is in the trigger paths, but the build never reads `brand/`: the site uses derived copies in
  `site/src/assets/`. A `brand/` change alone rebuilds an unchanged site.

Site structure, theming and content: [docs-site.md](docs-site.md).

<a id="install"></a>
## install.sh

A POSIX `sh` script (it runs under dash and bash) at the repository root, served from Pages and from
`raw.githubusercontent.com/samishal1998/bifrost/main/install.sh`.

### Structure

| Section | Behaviour | Why |
|---|---|---|
| `set -eu`; everything inside `main()`, called on the last line (`main "$@"`) | a truncated download defines at most part of a function and runs nothing | `curl … \| sh` executes the script as it streams in |
| colours | only when stdout is a terminal and `NO_COLOR` is empty | logs and pipes stay clean (the test asserts no escapes) |
| target | `uname -s` → `unknown-linux-musl` or `apple-darwin`; `uname -m` → `x86_64` (also `amd64`) or `aarch64` (also `arm64`); anything else is an error | the [packaging contract](#packaging) targets |
| Rosetta | on macOS, `x86_64` with `sysctl -n hw.optional.arm64` = 1 installs `aarch64` | a shell under Rosetta reports x86_64 on Apple Silicon (d9ecdf8) |
| version | `BIFROST_VERSION` (default `latest`) must match `[A-Za-z0-9._-]+`; a leading digit gets a `v` | it becomes part of a URL; `v1;id` is refused |
| base URL | `BIFROST_DOWNLOAD_URL` if set, else `…/releases/latest/download` or `…/releases/download/<version>` | mirrors and the test's fake release |
| install dir | checked **before any download**: `BIFROST_INSTALL_DIR`, else `$HOME/.local/bin` (an unset `HOME` without the override is an error); `mkdir -p`; must be writable; resolved to an absolute path with `CDPATH='' cd … && pwd` | fail fast; the PATH advice and cleanup need an absolute path without a trailing slash (d9ecdf8) |
| temp dir | `mktemp -d "${TMPDIR:-/tmp}/bifrost.XXXXXX"`; `trap` on EXIT removes it and any staged `.<name>.new`; HUP/INT/TERM exit 1 so the EXIT trap runs | no leftovers on Ctrl-C |
| download | `SHA256SUMS` first, then the tarball, with `curl -fsSL` or `wget -qO` | a release without checksums fails before the big download |
| verify | hash with `sha256sum` or `shasum -a 256`, reading the tarball **on stdin**; the expected hash is the line whose field 2 is exactly `<asset>` or `*<asset>`; no hasher, no entry or a mismatch → error, "nothing was installed" | a checksum is mandatory, never skipped; hashing stdin means an odd `TMPDIR` path can't change `sha256sum`'s output format (d9ecdf8) |
| install | extract; for each binary check `bifrost-<target>/<b>` exists, copy to `<dir>/.<b>.new`, `chmod 755`; only then `mv -f` all three into place | a failed copy leaves the old install intact; a rename is atomic and works while `bifrostd` is running |
| smoke test | runs `<dir>/bifrost --version`; failure → "does not run on this machine" | catches a wrong-architecture binary |
| PATH | warns with the exact `export PATH="<dir>:$PATH"` line when `<dir>` isn't on `PATH` | |
| runtime dependencies | reports ✓/✗ for ssh, sshfs, fusermount3 (or fusermount), `/dev/fuse`, rclone (optional) on Linux, with the `apt`/`dnf`/`pacman` package names; on macOS ssh, macFUSE or FUSE-T, sshfs, rclone with Homebrew commands; **never installs anything** | no sudo, no package manager side effects; `bifrost doctor` re-checks later |

### Knobs

| Variable | Default | Meaning |
|---|---|---|
| `BIFROST_VERSION` | `latest` | tag to install (`v0.1.1` or `0.1.1`) |
| `BIFROST_INSTALL_DIR` | `$HOME/.local/bin` | where the three binaries go |
| `BIFROST_DOWNLOAD_URL` | GitHub Releases | base URL; the script fetches `<base>/SHA256SUMS` and `<base>/<asset>` |
| `NO_COLOR` | unset | any non-empty value disables colour |
| `TMPDIR` | `/tmp` | where the download is staged |

The variables go on `sh`, not on `curl`: `curl -fsSL …/install.sh | BIFROST_VERSION=v0.1.0 sh`.

Limits: the checksums come from the same release as the tarballs, so they catch corruption and truncation, not a
compromised release; nothing is signed ([security.md](security.md)). There is no uninstaller: the site's installation
page ("Uninstall") says to unmount, stop `bifrostd` and delete the three binaries.

<a id="test-install"></a>
## scripts/test-install.sh

`scripts/test-install.sh [BIN_DIR]` (Linux; `BIN_DIR` defaults to `${CARGO_TARGET_DIR:-target}/debug`, and without
an argument it first builds the three binary crates, because `cargo test` doesn't build `bifrostd` or
`bifrost-tui`).

1. Syntax: `sh -n`, `dash -n`, `bash -n`; `shellcheck install.sh` when shellcheck is installed.
2. A fake release under a temp dir, served by `python3 -m http.server` on a random `127.0.0.1` port:
   `good/` (a real tarball in the contract layout, named for `$(uname -m)-unknown-linux-musl`, plus `SHA256SUMS`),
   `bad/` (the tarball truncated to 100 000 bytes, good sums) and `nosums/` (the tarball only). The host's debug
   binaries stand in for the musl ones; only the names matter.
3. The happy path four times, dash and bash × from the file and piped: all three binaries executable (mode 755 asserted
   on `bifrost`), `bifrost --version` and `bifrostd --version` output, `bifrost-tui --help`, the "sha256 verified" line, the PATH warning, no colour escapes without a
   TTY, no staged `.*.new` left.
4. A relative `BIFROST_INSTALL_DIR` with a trailing slash and a `TMPDIR` containing a backslash: the PATH advice is
   absolute and the hash still matches.
5. Refusals, each with nothing installed: unset `HOME` ("HOME is not set"), corrupted tarball ("checksum mismatch"),
   missing `SHA256SUMS` ("download failed: …/SHA256SUMS"), `BIFROST_VERSION='v1;id'` ("invalid BIFROST_VERSION"), and
   an unwritable directory ("set BIFROST_INSTALL_DIR"; skipped as root).

Any change to `install.sh` needs this script green, and a new behaviour needs a case here.

<a id="versioning"></a>
## Versioning

- **One version for everything.** `[workspace.package] version` in the root `Cargo.toml`; every crate inherits it
  with `version.workspace = true`. It reaches users as `bifrost --version` (clap), `bifrostd --version` and
  `StatusDto.version` (all `env!("CARGO_PKG_VERSION")`).
- **Tags are `v<version>`**, annotated: `v0.1.0` ("Bifröst v0.1.0") and `v0.1.1` ("Bifröst v0.1.1 — inline DNS node
  records"). The tag push is what builds and publishes a release.
- **0.x semantics.** v0.1.1 added a backwards-compatible DNS record form (inline `node=` values) on top of v0.1.0. The
  things users depend on across versions are the config format, the CLI's output and exit codes, the API, the bf1
  record format, the packaging contract and the mount marker/fingerprint (a change there remounts every mount; see
  [extending.md](extending.md#fingerprint)). Changing any of them deserves a minor bump and a release-note line.
- The crates are not published to crates.io.

<a id="procedure"></a>
## Release procedure

This is the sequence that produced v0.1.1 (visible in `gh run list`: push `main` → ci and pages → a `release`
dispatch on `main` → the tag push → the `release` run).

1. **Start from a green `main`.** `git pull`; the latest `ci` run on `main` passed. Run `scripts/check.sh` and, for
   behaviour changes, `tests/e2e/run.sh all` locally (CI doesn't run it).
2. **Bump the version** in the root `Cargo.toml`:

   ```toml
   [workspace.package]
   version = "0.1.2"
   ```

3. **Regenerate the lockfile** so the eight workspace crates carry the new version (the release build uses
   `--locked`):

   ```bash
   cargo build        # or: cargo update --workspace
   git diff --stat    # Cargo.toml and Cargo.lock only (v0.1.1: Cargo.toml | 2, Cargo.lock | 16)
   ```

4. **Update what names versions**, if anything: README examples (`BIFROST_VERSION=v0.1.0` is only an example and need
   not follow), the site if a page shows a version.
5. **Commit and push** to `main` with the subject `release: vX.Y.Z` (the convention of 389f0eb):

   ```bash
   git commit -am "release: v0.1.2" && git push origin main
   ```

   Wait for `ci` (and `pages`, if the site or `install.sh` changed) to pass.
6. **Dry run** the release build on `main`:

   ```bash
   gh workflow run release.yml --ref main -f dry_run=true
   # once the run shows up in `gh run list --workflow release.yml`:
   gh run watch "$(gh run list --workflow release.yml --limit 1 --json databaseId --jq '.[0].databaseId')"
   ```

   All four `build` jobs must pass; the `release` job shows as skipped. Optionally download an artifact
   (`gh run download <id> -n bifrost-x86_64-unknown-linux-musl`) and run the binary.
7. **Tag the release commit** with an annotated tag and push it:

   ```bash
   git tag -a v0.1.2 -m "Bifröst v0.1.2 — <one-line headline>"
   git push origin v0.1.2
   ```

8. **Watch the release run** (`gh run watch`). It rebuilds the four targets from the tag, writes `SHA256SUMS` and
   publishes the release with generated notes.
9. **Verify** what users get:

   ```bash
   gh release view v0.1.2                     # 4 × bifrost-<target>.tar.gz + SHA256SUMS
   d=$(mktemp -d)
   curl -fsSL https://samishal1998.github.io/bifrost/install.sh | BIFROST_INSTALL_DIR="$d" sh
   "$d/bifrost" --version                     # bifrost 0.1.2 (latest now resolves to the new release)
   rm -r "$d"
   ```

10. **Edit the notes** if the generated ones need a headline (`gh release edit v0.1.2 --notes-file …`).

<a id="tags"></a>
## Never move a pushed tag

Once `vX.Y.Z` is pushed, it is never deleted, re-pointed or force-pushed, even if the release turns out broken.

- The tag push already built and published binaries from the old commit. Re-pointing the tag leaves a GitHub
  release whose tarballs and `SHA256SUMS` don't match the source the tag now names.
- Anyone who installed with `BIFROST_VERSION=vX.Y.Z`, mirrored the assets or recorded the checksums would get
  different bits under the same name, which is exactly what the checksum is meant to rule out.
- Clones that already have the tag keep the old one: `git fetch` refuses to clobber an existing tag unless forced.
- Re-running the workflow for a moved tag fails anyway: `gh release create` refuses a tag that already has a release.

Fix forward instead: fix on `main`, bump the patch version and release `vX.Y.(Z+1)`. If the broken release must not
be installed, mark it as a pre-release or edit its notes in the GitHub UI (a pre-release is not what
`releases/latest` resolves to), but leave the tag alone.

<a id="troubleshooting"></a>
## When something goes wrong

| Symptom | Cause | Action |
|---|---|---|
| release `build` fails with "the lock file … needs to be updated but --locked was passed" | the version bump didn't include `Cargo.lock` | commit the lockfile; if the tag is already pushed, release the next patch version |
| a musl build fails compiling `ring` | the `CC_<target>` line or `musl-tools` step broke | reproduce with the dry run on `main`; never debug on a tag |
| a darwin build fails | macOS-only code that the Linux darwin `cargo check` doesn't cover (`bifrostd` and `bifrost-discovery` are not darwin-checked locally, §15 #31) | the CI `macos` job usually catches it first |
| the `release` job failed after the builds passed (network, API) | transient | re-run only the failed job: `gh run rerun <run-id> --failed`. `--verify-tag` and the existing artifacts make that safe as long as no release was created |
| `gh release create` says the release exists | a previous attempt created it | delete the **release** (not the tag) in the UI or with `gh release delete vX.Y.Z` only if it has no assets users could have fetched; otherwise fix forward |
| `install.sh` fails with "download failed: …/bifrost-<target>.tar.gz" | the release has no asset for that target (a target added to `install.sh` before a release produced it) | keep `install.sh`'s target mapping in step with the release matrix |
| CI `linux` fails in `test-install.sh` only | an `install.sh` change broke a refusal or a message the test greps for | run `scripts/test-install.sh` locally |
