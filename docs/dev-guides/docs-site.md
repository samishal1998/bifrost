# Docs site

This guide is for whoever maintains the user documentation at https://samishal1998.github.io/bifrost/: how `site/`
is built (Astro Starlight), why two Starlight components are overridden, how the brand palette maps onto Starlight's
theme variables, which page documents which part of the code, how the examples were checked against real binaries
and how to re-check them, and how the site is deployed. The site is the user-facing documentation; this directory
(`docs/dev-guides/`) is the internal engineering documentation, and `docs/design/` holds the design history. The
decision to use Starlight on Pages is in [decisions.md#docs-starlight-on-pages](decisions.md#docs-starlight-on-pages).

## Contents

- [Layout](#layout)
- [astro.config.mjs](#config)
- [Component overrides](#overrides)
- [Links and the /bifrost base](#links)
- [Branding](#branding)
- [Content map](#content-map)
- [Keeping the docs true](#verify)
- [Build and preview](#build)
- [Deployment](#deploy)
- [Which docs to update for a change](#which-docs)

<a id="layout"></a>
## Layout

```text
site/
├── astro.config.mjs            Starlight config: title, logos, sidebar, overrides
├── package.json                astro ^7.2.10, @astrojs/starlight ^0.42.4, sharp ^0.35.3 (image processing)
├── package-lock.json           npm ci installs exactly this
├── tsconfig.json               extends astro/tsconfigs/strict
├── .gitignore                  dist/, .astro/, node_modules/, public/install.sh, …
├── public/
│   └── favicon.png             64×64, from the app icon (copied verbatim to the site root)
└── src/
    ├── content.config.ts       the "docs" collection (Starlight's docsLoader + docsSchema)
    ├── content/docs/           one .mdx per page; the path is the URL
    ├── components/
    │   ├── Hero.astro          landing hero (override)
    │   └── SiteTitle.astro     header title + "Docs" link (override)
    ├── assets/                 logos, hero image and pillar icons, derived from brand/
    └── styles/custom.css       brand palette on Starlight variables, header line, pillars
```

`public/install.sh` is not in git: the pages workflow copies the repository's `install.sh` there at build time.

<a id="config"></a>
## astro.config.mjs

| Setting | Value | Why |
|---|---|---|
| `site`, `base` | `https://samishal1998.github.io`, `/bifrost` | a GitHub project page lives under `/<repo>/` |
| `title` | `Bifröst` | also the `<title>` suffix |
| `logo` | `light: logo-ink-dark.png`, `dark: logo-ink-light.png`, `replacesTitle: true` | dark ink on the light theme, light ink on the dark theme; the logo is the wordmark, so the text title is hidden |
| `favicon` | `/favicon.png` | from `public/` |
| `social` | GitHub link | |
| `editLink.baseUrl` | `…/edit/main/site/` | "Edit page" opens the `.mdx` on GitHub |
| `customCss` | `./src/styles/custom.css` | the palette |
| `components` | `Hero`, `SiteTitle` | [overrides](#overrides) |
| `sidebar` | explicit groups: Getting started, Guides, Examples, Reference, Concepts, then Contributing | the order is editorial; **a new page doesn't appear in the sidebar until it is added here** |

<a id="overrides"></a>
## Component overrides

### Hero (`src/components/Hero.astro`)

Starlight's default hero renders its image as a 400 × 400 square, which crops the 16:9 aurora-bridge scene. The
override lays the landing page out like the approved brand board (`brand/reference/approved_brand_board.png`):

- the aurora image (`hero-aurora.webp`, 1672 × 941) beside the copy on wide screens (`7fr 5fr` grid from 50rem),
  above it on narrow ones; `astro:assets` `<Image>` generates responsive widths 640–1672 and loads it eagerly with
  high fetch priority (it is the largest element on the page);
- the colour lockup in the `<h1>`, one image per theme (`logo-color-hero.png` hidden in dark mode,
  `logo-light-hero.png` hidden in light mode, via Starlight's `dark:sl-hidden`/`light:sl-hidden` classes);
- the tagline, the page `description` as the pitch, and the frontmatter `hero.actions` as `LinkButton`s.

It reads the frontmatter from `Astro.locals.starlightRoute.entry`, so `index.mdx` keeps using Starlight's normal
`hero:` fields. Action links that start with `/` get the base path prepended (`withBase`), so the frontmatter can say
`/installation/`.

### SiteTitle (`src/components/SiteTitle.astro`)

Wraps Starlight's own `SiteTitle` and adds a **Docs** link to `/bifrost/installation/` next to the logo. The landing
page uses the `splash` template, which has no sidebar; on a phone there was no way from the landing page into the
docs except the hero button. The header row is the one element visible at every width on every page (98588d8).

The link carries `aria-current="true"` on pages that have a sidebar (`starlightRoute.hasSidebar`), which draws the
Aurora Teal underline. It is `"true"` (current item in a set), not `"page"`: the link always points at the
installation page, and `"page"` made screen readers announce "current page" on every other page (e9e48e1). Hover is
Glacial Blue (`--bf-nav-hover`), focus is an inset outline.

<a id="links"></a>
## Links and the /bifrost base

- Links inside `.mdx` content are written **with** the base: `[architecture](/bifrost/concepts/architecture/)`.
  Every internal content link in the site follows this; a link without `/bifrost` is a 404 on Pages.
- Links in frontmatter `hero.actions` are written **without** it (`link: /installation/`); `Hero.astro` adds it.
- `SiteTitle.astro` builds its link from `import.meta.env.BASE_URL`.
- Sidebar entries are slugs (`'guides/policy'`), resolved by Starlight.

<a id="branding"></a>
## Branding

The palette (`brand/README.md`): Nordic Night `#0B1220`, Aurora Teal `#2DD4BF`, Glacial Blue `#60A5FA`, Frost Glass
`#E5F0FF`, Stone `#94A3B8`. The TUI uses the same five plus amber `#FBBF24` and rose `#F87171`
(`crates/bifrost-tui/src/ui.rs`), and the site reuses those two for caution and danger, so a state looks the same in
the terminal and in the docs.

`src/styles/custom.css` maps them onto Starlight's variables. The dark theme is the default (`:root`); the light
theme (`:root[data-theme='light']`) swaps in darker shades so text stays readable. The file's header states that every
text/background pair is at least 4.5:1 (WCAG AA); re-check contrast when changing a colour.

| Role | Starlight variable | Dark theme | Light theme |
|---|---|---|---|
| page background | `--sl-color-black` | Nordic Night `#0b1220` | `#f8fbff` |
| strongest text | `--sl-color-white` | `#f4f8ff` | Nordic Night `#0b1220` |
| body text | `--sl-color-gray-1` / `-2` | Frost Glass `#e5f0ff` / `#c4d0e2` | `#1e293b` / `#334155` |
| muted text | `--sl-color-gray-3` | Stone `#94a3b8` | `#475569` |
| panels, borders | `--sl-color-gray-5` / `-6` | `#1c2840` / `#111a2e` | `#cbd5e1` / Frost Glass `#e5f0ff` |
| accent (links, current item) | `--sl-color-accent` | Aurora Teal `#2dd4bf` | `#0f766e` (teal dark enough for AA on white) |
| note aside | `--sl-color-blue` | Glacial Blue `#60a5fa` | `#3b82f6` |
| tip aside | `--sl-color-purple` | Aurora Teal `#2dd4bf` | `#14b8a6` |
| caution aside | `--sl-color-orange` | amber `#fbbf24` | `#f59e0b` |
| danger aside | `--sl-color-red` | rose `#f87171` | `#ef4444` |
| header Docs hover | `--bf-nav-hover` | Glacial Blue `#60a5fa` | `#1d4ed8` |

Other brand touches in the CSS: a single 1 px "aurora" line under the header (teal into glacial blue, "never a
rainbow"), and the four landing-page pillars, whose line icons are drawn with a CSS `mask` filled with
`--sl-color-accent`, so they take the theme's accent colour instead of the PNG's own.

### Assets

The site never reads `brand/` at build time. Its assets are derivatives committed under `site/src/assets/`: cropped
to the lockup (without the tagline), background made transparent, resized. No file is byte-identical to a brand file,
the tool used is not recorded, and the source column below is by appearance:

| Site asset | Size | Derived from (`brand/`) | Used by |
|---|---|---|---|
| `public/favicon.png` | 64 × 64 | `03_app_icons/` | browser tab |
| `assets/logo-ink-dark.png` | 148 × 80 | `01_logo/primary_logo_mono_dark.png` | header logo, light theme |
| `assets/logo-ink-light.png` | 147 × 80 | `01_logo/primary_logo_mono_light.png` | header logo, dark theme |
| `assets/logo-color-hero.png` | 762 × 400 | `01_logo/primary_logo_color.png` | hero lockup, light theme |
| `assets/logo-light-hero.png` | 733 × 400 | `01_logo/primary_logo_mono_light.png` | hero lockup, dark theme |
| `assets/hero-aurora.webp` | 1672 × 941 | `05_hero/hero_aurora_bridge_16x9.png` | hero image |
| `assets/icons/{discovery,modular,mount,bridges}.png` | 160 × 160 | `04_ui_icons/{discovery,modular_layers,mount_cube,bridges}.png` | landing pillars (as masks) |

To change an asset, regenerate the derivative from `brand/` with any image tool, keep the size (or update the
`width`/`widths` in `Hero.astro`), and commit it under `site/src/assets/`. Editing `brand/` alone changes nothing on
the site, although it does trigger a rebuild.

<a id="content-map"></a>
## Content map

Each page and the code it describes. When that code changes, the page is what goes stale.

| Page (`src/content/docs/…`) | Covers | Source of truth |
|---|---|---|
| `index.mdx` | landing: pillars, one-line install, the pipeline, next steps | `install.sh` URL |
| `installation.mdx` | one-liner and knobs, release tarballs, from source, runtime tools, uninstall | `install.sh`, `release.yml` targets, README "Install" |
| `quickstart.mdx` | a four-line config to a first mount | `bifrost-config` shorthand, `bifrost status` output (`crates/bifrost-cli/src/output.rs`) |
| `guides/configuration.mdx` | config location, machines → mounts, reload | `crates/bifrost-config/src/{lib,paths}.rs`, `crates/bifrost-daemon/src/reload.rs` |
| `guides/policy.mdx` | allow/deny/filters, trust, verdict strings | `crates/bifrost-core/src/{policy,registry}.rs` |
| `guides/discovery.mdx` | static, Tailscale, DNS TXT, HTTP | `crates/bifrost-discovery/src/*`, config `ProviderSpec` |
| `guides/mount-drivers.mdx` | sshfs, rclone, rclone-nfs, `auto`, unmount semantics | `crates/bifrost-mount/src/*`, `DRIVER_NAMES`/`default_auto_order` |
| `guides/service.mdx` | systemd user service, `KillMode=process`, `SSH_AUTH_SOCK` | `crates/bifrost-daemon/src/main.rs` (signals, shutdown), README |
| `guides/troubleshooting.mdx` | doctor, host keys, busy unmounts, stuck mounts, logs | `crates/bifrost-cli/src/doctor.rs`, error strings in `bifrost-mount` |
| `examples/discovery.mdx` | ready-made configs and the verdicts they produce | `bifrost config check`, verdict `Display` in `bifrost-core` |
| `examples/dns-records.mdx` | a complete bf1 zone for several DNS servers, checks, rejected records and their logged reasons, the index form | `crates/bifrost-discovery/src/dns.rs` (`parse_bf1`, `root`, `node`, the warn messages) |
| `reference/cli.mdx` | every command, flag, output and exit code | `crates/bifrost-cli/src/{main,output}.rs` |
| `reference/tui.mdx` | views, keys, glyphs | `crates/bifrost-tui/src/{app,ui}.rs` |
| `reference/configuration.mdx` | every key, default and rule; a full example | `crates/bifrost-config/src/{raw,lib}.rs` |
| `reference/api.mdx` | routes, DTOs, status codes, SSE | `crates/bifrost-daemon/src/api.rs`, `crates/bifrost-core/src/{api,events}.rs` |
| `reference/files.mdx` | config, state, logs, socket, mounts | `crates/bifrost-config/src/paths.rs`, `crates/bifrost-daemon/src/{main,state}.rs` |
| `concepts/architecture.mdx` | the pipeline and the crates | [architecture.md](architecture.md) |
| `concepts/reconciliation.mdx` | machine and mount states, the rules | `crates/bifrost-core/src/reconcile.rs` (`decide`, `Availability`) |
| `concepts/security.mdx` | each principle and its enforcement | contract §11, [security.md](security.md) |
| `contributing.mdx` | build, gate, E2E, releasing, the site | `scripts/check.sh`, `tests/e2e/run.sh`, [release-and-ci.md](release-and-ci.md) |

<a id="verify"></a>
## Keeping the docs true

The pages were written against the built binaries, not from the contract alone (3d40b33), and the DNS pages were
re-verified live for v0.1.1: "every zone, the dnsmasq snippet and the dig/discover/machines outputs and logged
reasons come from real runs (CoreDNS 1.11.3, dnsmasq on alpine 3.20)" (833b236). A review pass (087efa1) still found
nine wrong facts, among them a `500ms` duration in an example that the parser rejects (every duration is at least
1 s). The lesson: run it, don't recall it. Three techniques make that cheap.

### 1. Check every TOML block with `bifrost config check`

```bash
B=$CARGO_TARGET_DIR/debug                                  # or target/debug
S=$(mktemp -d); mkdir -p "$S/.config/bifrost"; touch "$S/.config/bifrost/ssh_config"
for f in site/src/content/docs/examples/*.mdx site/src/content/docs/reference/configuration.mdx; do
  awk -v out="$S/$(basename "$f" .mdx)" '
    /^[ \t]*```toml/ { n++; file = sprintf("%s-%02d.toml", out, n); inb = 1; next }
    inb && /^[ \t]*```/ { inb = 0; next }
    inb { sub(/^\t+/, ""); print > file }' "$f"
done
for t in "$S"/*.toml; do
  HOME=$S BIFROST_INVENTORY_TOKEN=x "$B/bifrost" config check "$t" >/dev/null || echo "FAIL: $t"
done
rm -r "$S"
```

A fake `HOME` makes `~` expand somewhere harmless and gives `mount.ssh_config = "~/.config/bifrost/ssh_config"`
something to exist; set any `${VAR}` a block references. Blocks that are deliberately partial (one table shown out of
context) fail and have to be judged by eye.

### 2. An isolated daemon that can't mount anything

Run a scratch daemon with its own config, state, root and socket, and make every allowed mount fail its driver
selection by forcing `rclone-nfs`, which probes as unavailable ("macOS only") on Linux. Discovery, policy, verdicts,
`machines`, `status`, `discover` and the logged reasons are all real; nothing is ever mounted and no ssh connection is
made.

```bash
B=$CARGO_TARGET_DIR/debug
S=$(mktemp -d)                                  # keep it short (TMPDIR unset): the socket must fit in 103 bytes
cat >"$S/config.toml" <<EOF
[mount]
root = "$S/machines"
default_driver = "rclone-nfs"                   # Linux: unavailable, so nothing mounts

[[machines]]
name = "demo"
host = "192.0.2.10"
user = "sami"
remote = "/srv"
EOF
export BIFROST_CONFIG=$S/config.toml BIFROST_STATE_DIR=$S/state BIFROST_SOCKET=$S/bf.sock
"$B/bifrostd" 2>"$S/d.log" &
sleep 2                    # the first driver probe
"$B/bifrost" mounts        # demo … failed … rclone-nfs unavailable: macOS only
```

This was re-run on 2026-09-27 with the v0.1.1 binaries: right after start the mount shows `eligible` with
`probing drivers`, and once the probe finishes it shows `failed` with detail and action
`waiting (no driver: rclone-nfs unavailable: macOS only)`. The configured default driver also applies to every
discovery provider's template unless the provider sets `mount.driver`.

What it can't show: a mounted state. The `STATE`/`MOUNTED` columns in the DNS examples come from a real mounted run,
which the blocked driver can't produce (833b236). Use the E2E fixtures, or a real host, for those. Stop the daemon
with `kill %1` when done (after technique 3 if you use it).

### 3. CoreDNS on 10053 for DNS examples

Continuing in the same shell as technique 2 (`$B`, `$S` and the exported `BIFROST_*`):

```bash
mkdir -p "$S/dns" && chmod 755 "$S/dns"
printf 'example.test:53 {\n    file /zones/db\n}\n' >"$S/dns/Corefile"
cat >"$S/dns/db" <<'EOF'
$ORIGIN example.test.
$TTL 5
@          IN SOA ns admin 1 60 60 3600 5
@          IN NS  ns
ns         IN A   127.0.0.1
_bifrost   IN TXT "v=bf1 node=agent-01 host=192.0.2.21 user=sami tags=dev"
_bifrost   IN TXT "v=bf1 node=build-01 host=192.0.2.22 tags=ci"
EOF
chmod 644 "$S/dns/"*
docker run -d --name bf-docs-dns -p 127.0.0.1:10053:53/udp -p 127.0.0.1:10053:53/tcp \
  -v "$S/dns:/zones:ro" coredns/coredns:1.11.3 -conf /zones/Corefile
dig @127.0.0.1 -p 10053 +short TXT _bifrost.example.test
cat >>"$S/config.toml" <<'EOF'

[[discovery]]
type = "dns"
domain = "example.test"
nameservers = ["127.0.0.1:10053"]

[discovery.filter]
include_tags = ["dev"]
EOF
"$B/bifrost" config reload && "$B/bifrost" discover && "$B/bifrost" machines
# agent-01  dns  192.0.2.21  failed      no     (allowed (dns.filter.include), blocked driver)
# build-01  dns  192.0.2.22  discovered  no     (discover-only)
docker rm -f bf-docs-dns
kill %1; rm -r "$S"          # stop the scratch daemon (SIGTERM; nothing was mounted), remove the scratch
```

- **10053, not 5353.** 5353 is the mDNS port; where avahi (Linux desktops) or mDNSResponder (macOS) holds
  `0.0.0.0:5353`, `docker run` fails with "address already in use" (25af2e2). 10053 is unassigned. (The E2E harness
  still uses 5353; see [e2e-harness.md](e2e-harness.md#fixtures).)
- The image runs as a non-root user: the directory must be 0755 and the files 0644.
- Bump the SOA serial after each edit; without a `reload` interval the `file` plugin picks the change up within a
  minute (the E2E Corefile sets `reload 1s`).
- Name the container something other than `bf-e2e-*`, so an E2E run's cleanup doesn't remove it and the two don't
  collide.
- For the size-related claims (a big root RRset truncated over UDP and retried over TCP; 512-byte queries without
  EDNS), `dig +notcp +ignore +bufsize=…` shows the `tc` flag, as p08 does.

<a id="build"></a>
## Build and preview

```bash
cd site
npm ci                 # exactly package-lock.json
npm run dev            # http://localhost:4321/bifrost/ with live reload
npm run build          # static site in site/dist; fails on broken frontmatter or imports
npm run preview        # serve site/dist
```

CI uses Node 22. `npm run build` does not check internal links; click through changed pages in `npm run dev`, and
remember the [`/bifrost` prefix](#links).

<a id="deploy"></a>
## Deployment

`.github/workflows/pages.yml` ([release-and-ci.md](release-and-ci.md#pages)) runs on pushes to `main` that touch
`site/**`, `install.sh`, `brand/**` or the workflow, and on manual dispatch. It runs `npm ci`, copies the repository's
`install.sh` into `site/public/`, builds, and deploys `site/dist` with `actions/deploy-pages` (the repository's Pages
source is "GitHub Actions"). Consequences:

- The site always matches `main`, not the latest release. Document a feature when it is merged, and say which version
  introduced it if users on the previous release would be confused.
- `https://samishal1998.github.io/bifrost/install.sh` is the `install.sh` on `main`, so an installer change reaches
  users as soon as it merges, before any release. Keep `install.sh` compatible with the **published** releases'
  packaging.
- There are no preview deployments for pull requests; build locally.

<a id="which-docs"></a>
## Which docs to update for a change

| Change | Site | README | dev-guides |
|---|---|---|---|
| a config key | `reference/configuration.mdx`, the relevant guide | "Configuration" section | [crates/config.md](crates/config.md) |
| a CLI command or output | `reference/cli.mdx` | if it is in the quickstart | [crates/cli.md](crates/cli.md) |
| a TUI key | `reference/tui.mdx` | | [crates/tui.md](crates/tui.md) |
| an API route or event | `reference/api.mdx` | | [crates/daemon.md](crates/daemon.md) |
| a provider or the bf1 format | `guides/discovery.mdx`, `examples/*` | provider section | [crates/discovery.md](crates/discovery.md) |
| a driver | `guides/mount-drivers.mdx`, `installation.mdx` (runtime tools) | drivers section | [crates/mount.md](crates/mount.md) |
| install or release | `installation.mdx`, `contributing.mdx` | "Install" | [release-and-ci.md](release-and-ci.md) |
| a design decision | `concepts/*` if user-visible | | [decisions.md](decisions.md) |

[extending.md](extending.md) repeats the relevant rows at the end of each recipe.
