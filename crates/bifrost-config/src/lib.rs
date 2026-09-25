//! Configuration: TOML → validated `Config` (contract §3).

pub mod paths;
mod raw;

use bifrost_core::policy::{Match, Policy, ProviderFilter};
use bifrost_core::reconcile::{MountTemplate, StaticMount};
use bifrost_core::registry::Source;
use bifrost_core::validate::{Cidr, Glob, RemotePath, meta_key, native_id, parse_duration, tag};
use bifrost_core::{
    DriverSelector, Host, MachineId, MachineObservation, Metadata, MountHints, Name, User,
};
use raw::{RawConfig, RawMatch, RawMount1};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Display;
use std::net::{IpAddr, SocketAddr};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub path: PathBuf,
    /// expanded + absolute (daemon canonicalizes)
    pub root: PathBuf,
    pub default_driver: DriverSelector,
    pub auto_order: Vec<String>,
    pub ssh_config: Option<PathBuf>,
    pub vfs_cache_mode: String,
    pub timings: Timings,
    /// core
    pub policy: Policy,
    pub providers: Vec<ProviderConfig>,
    pub machines: Vec<StaticMachine>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Timings {
    pub discovery_interval: Duration,
    pub health_interval: Duration,
    pub reconcile_interval: Duration,
    pub mount_timeout: Duration,
    pub offline_grace_period: Duration,
    pub retry_initial: Duration,
    pub retry_max: Duration,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProviderConfig {
    pub name: String,
    /// "tailscale" | "dns" | "http"
    pub kind: String,
    pub interval: Duration,
    pub template: MountTemplate,
    pub spec: ProviderSpec,
}

impl ProviderConfig {
    /// Source{trust: rank of kind, kind, provider: name}
    pub fn source(&self) -> Source {
        Source {
            trust: TRUST
                .iter()
                .find(|(k, _)| *k == self.kind)
                .map_or(u8::MAX, |&(_, t)| t), // kind is validated; MAX never happens
            kind: self.kind.clone(),
            provider: self.name.clone(),
        }
    }
}

// Owned here, not in core (B8): adding a provider or a driver never edits bifrost-core.
/// Trust ranks, lower = more trusted. Core only compares the numbers; `trust == 0` means static.
pub const TRUST: [(&str, u8); 4] = [("static", 0), ("tailscale", 1), ("http", 2), ("dns", 3)];

/// Source{trust: 0, kind: "static", provider: "static"}
pub fn static_source() -> Source {
    Source {
        trust: 0,
        kind: "static".into(),
        provider: "static".into(),
    }
}

pub const DRIVER_NAMES: [&str; 3] = ["sshfs", "rclone", "rclone-nfs"];

/// macOS [rclone-nfs, rclone, sshfs]; elsewhere [sshfs, rclone]
pub fn default_auto_order() -> Vec<String> {
    let order: &[&str] = if cfg!(target_os = "macos") {
        &["rclone-nfs", "rclone", "sshfs"]
    } else {
        &["sshfs", "rclone"]
    };
    order.iter().map(|s| s.to_string()).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub enum ProviderSpec {
    Tailscale,
    Dns {
        domain: Host,
        nameservers: Vec<SocketAddr>,
    },
    Http {
        url: String,
        headers: Vec<(String, Secret)>,
    },
}

/// Debug prints "***"
#[derive(Clone, PartialEq)]
pub struct Secret(pub String);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("***")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StaticMachine {
    pub id: MachineId,
    pub host: Host,
    pub port: Option<u16>,
    pub user: Option<User>,
    pub tags: BTreeSet<String>,
    pub metadata: BTreeMap<String, String>,
    pub mounts: Vec<StaticMount>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ConfigError {
    pub path: String,
    pub message: String,
}

/// "error: {path}: {message}"
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "error: {}: {}", self.path, self.message)
    }
}

/// Pure. HOME is looked up through `env("HOME")`. TOML syntax/shape error ⇒ exactly one ConfigError (with line:col).
/// Otherwise returns ALL semantic errors, sorted. parse("", …) == the default config.
///
/// Error paths: a TOML error is `<file>:<line>:<col>`; a semantic error is the key path (`machines[0].mounts[1].local`).
/// The one I/O is the read-only `mount.ssh_config` existence check (the table requires it, and the reload
/// poller calls `parse` directly).
pub fn parse(
    text: &str,
    path: &Path,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Config, Vec<ConfigError>> {
    let raw: RawConfig = toml::from_str(text).map_err(|e| {
        let at = e.span().map_or(String::new(), |s| {
            let before = text.get(..s.start).unwrap_or(text);
            let line_start = before.rfind('\n').map_or(0, |i| i + 1);
            let (line, col) = (
                before.matches('\n').count() + 1,
                before[line_start..].chars().count() + 1,
            );
            format!(":{line}:{col}")
        });
        vec![ConfigError {
            path: format!("{}{at}", path.display()),
            message: e.message().trim().replace('\n', " "),
        }]
    })?;
    let mut v = V {
        env,
        errs: Vec::new(),
    };
    let cfg = v.config(raw, path);
    if v.errs.is_empty() {
        Ok(cfg)
    } else {
        v.errs.sort();
        v.errs.dedup();
        Err(v.errs)
    }
}

/// read + parse with the process env
pub fn load(path: &Path) -> Result<Config, Vec<ConfigError>> {
    let text = std::fs::read_to_string(path).map_err(|e| {
        vec![ConfigError {
            path: path.display().to_string(),
            message: format!("cannot read: {e}"),
        }]
    })?;
    parse(&text, path, &|k| std::env::var(k).ok())
}

impl Config {
    /// addresses=[host], hints.user=user, online None, ttl None
    pub fn static_observations(&self) -> Vec<MachineObservation> {
        self.machines
            .iter()
            .map(|m| MachineObservation {
                id: m.id.clone(),
                name: m.id.as_str().into(),
                native_id: None,
                addresses: vec![m.host.clone()],
                port: m.port,
                online: None,
                metadata: Metadata {
                    tags: m.tags.clone(),
                    values: m.metadata.clone(),
                },
                hints: MountHints {
                    user: m.user.clone(),
                    path: None,
                },
                ttl: None,
            })
            .collect()
    }
    pub fn static_mounts(&self) -> BTreeMap<MachineId, Vec<StaticMount>> {
        self.machines
            .iter()
            .map(|m| (m.id.clone(), m.mounts.clone()))
            .collect()
    }
    /// keyed by provider name
    pub fn templates(&self) -> BTreeMap<String, MountTemplate> {
        self.providers
            .iter()
            .map(|p| (p.name.clone(), p.template.clone()))
            .collect()
    }
}

type Env<'a> = &'a dyn Fn(&str) -> Option<String>;

/// Collects every error. A value that fails is dropped (None); any error discards the whole Config.
struct V<'a> {
    env: Env<'a>,
    errs: Vec<ConfigError>,
}

impl V<'_> {
    fn err(&mut self, path: &str, message: impl Display) {
        self.errs.push(ConfigError {
            path: path.into(),
            message: message.to_string(),
        });
    }

    fn check<T, E: Display>(&mut self, path: &str, r: Result<T, E>) -> Option<T> {
        r.map_err(|e| self.err(path, e)).ok()
    }

    fn expand(&mut self, path: &str, s: &str) -> Option<String> {
        let r = expand(s, self.env);
        self.check(path, r)
    }

    /// "every interval ≥ 1s": applied to every duration, nothing sensible is shorter
    fn dur(&mut self, path: &str, s: &str) -> Duration {
        match parse_duration(s) {
            Ok(d) if d < Duration::from_secs(1) => {
                self.err(path, format!("{s:?} is shorter than 1s"));
                d
            }
            r => self.check(path, r).unwrap_or_default(),
        }
    }

    /// Absent ⇒ `default`. Core checks only the grammar (B8); membership is checked here.
    fn driver(
        &mut self,
        path: &str,
        s: Option<&str>,
        default: &DriverSelector,
    ) -> Option<DriverSelector> {
        match s {
            None => Some(default.clone()),
            Some("auto") => Some(DriverSelector::Auto),
            Some(d) if DRIVER_NAMES.contains(&d) => Some(DriverSelector::Named(d.into())),
            Some(d) => {
                self.err(
                    path,
                    format!("unknown driver {d:?} (expected \"auto\" or one of {DRIVER_NAMES:?})"),
                );
                None
            }
        }
    }

    fn tags(&mut self, path: &str, tags: &[String]) -> Vec<String> {
        tags.iter()
            .enumerate()
            .filter_map(|(j, t)| self.check(&format!("{path}[{j}]"), tag(t)))
            .collect()
    }

    fn meta(&mut self, path: &str, m: BTreeMap<String, String>) -> BTreeMap<String, String> {
        let mut out = BTreeMap::new();
        for (k, val) in m {
            let kp = format!("{path}.{k:?}");
            let Some(key) = self.check(&kp, meta_key(&k)) else {
                continue;
            };
            if val.chars().count() > 256 || val.chars().any(char::is_control) {
                self.err(
                    &kp,
                    "value must be at most 256 characters with no control characters",
                );
            } else {
                out.insert(key, val);
            }
        }
        out
    }

    /// `p` is "policy.allow." or "discovery[0].filter.include_". `ids` are Names (the machine id only, A19),
    /// except in a provider filter (`native`), where they may be the provider's own native ids.
    /// `providers` is copied as-is; `config` checks it once every provider name is known.
    fn matcher(&mut self, p: &str, m: RawMatch, native: bool) -> Match {
        let mut ids = Vec::new();
        for (j, s) in m.ids.iter().enumerate() {
            let path = format!("{p}ids[{j}]");
            let id = if native {
                self.check(&path, native_id(s))
            } else {
                self.check(&path, Name::parse(s))
                    .map(|n| n.as_str().to_string())
            };
            ids.extend(id);
        }
        let mut names = Vec::new();
        for (j, s) in m.names.iter().enumerate() {
            names.extend(self.check(&format!("{p}names[{j}]"), Glob::parse(s)));
        }
        let mut cidrs = Vec::new();
        for (j, s) in m.cidrs.iter().enumerate() {
            cidrs.extend(self.check(&format!("{p}cidrs[{j}]"), Cidr::parse(s)));
        }
        Match {
            ids,
            names,
            cidrs,
            tags: self.tags(&format!("{p}tags"), &m.tags),
            providers: m.providers,
            metadata: self.meta(&format!("{p}metadata"), m.metadata),
        }
    }

    fn root(&mut self, s: &str) -> Option<PathBuf> {
        let r = PathBuf::from(self.expand("mount.root", s)?);
        let why = if !r.is_absolute() {
            "must be an absolute path"
        } else if r.components().any(|c| c == Component::ParentDir) {
            "must not contain a '..' component"
        } else if r.parent().is_none() {
            "must not be /"
        } else if (self.env)("HOME").is_some_and(|h| Path::new(&h) == r) {
            "must not be $HOME"
        } else {
            return Some(r);
        };
        self.err("mount.root", format!("{r:?} {why}"));
        None
    }

    fn ssh_config(&mut self, s: &str) -> Option<PathBuf> {
        let p = self.expand("mount.ssh_config", s)?;
        let why = if p.contains('"') {
            "must not contain '\"' (it is quoted inside --sftp-ssh)"
        } else if !Path::new(&p).is_absolute() {
            "must be an absolute path"
        } else if !Path::new(&p).exists() {
            "does not exist"
        } else {
            return Some(p.into());
        };
        self.err("mount.ssh_config", format!("{p:?} {why}"));
        None
    }

    fn config(&mut self, raw: RawConfig, path: &Path) -> Config {
        if let Some(n) = raw.version.filter(|&n| n != 1) {
            self.err(
                "version",
                format!("unsupported version {n} (only 1 is accepted)"),
            );
        }

        let m = raw.mount;
        let root = self.root(m.root.as_deref().unwrap_or("~/machines"));
        let default_driver = self
            .driver(
                "mount.default_driver",
                m.default_driver.as_deref(),
                &DriverSelector::Auto,
            )
            .unwrap_or(DriverSelector::Auto);
        let auto_order = m.auto_order.unwrap_or_else(default_auto_order);
        if auto_order.is_empty() {
            self.err("mount.auto_order", "must not be empty");
        }
        for (i, d) in auto_order.iter().enumerate() {
            let p = format!("mount.auto_order[{i}]");
            if !DRIVER_NAMES.contains(&d.as_str()) {
                self.err(
                    &p,
                    format!("unknown driver {d:?} (expected one of {DRIVER_NAMES:?})"),
                );
            } else if auto_order[..i].contains(d) {
                self.err(&p, format!("duplicate driver {d:?}"));
            }
        }
        let ssh_config = m.ssh_config.and_then(|s| self.ssh_config(&s));
        let vfs_cache_mode = m.vfs_cache_mode.unwrap_or_else(|| "writes".into());
        if !["off", "minimal", "writes", "full"].contains(&vfs_cache_mode.as_str()) {
            let msg = format!("{vfs_cache_mode:?} is not off, minimal, writes or full");
            self.err("mount.vfs_cache_mode", msg);
        }

        let (d, r) = (raw.daemon, raw.reconciliation);
        let mut dur = |path: &str, s: Option<String>, default: &str| {
            self.dur(path, s.as_deref().unwrap_or(default))
        };
        let timings = Timings {
            discovery_interval: dur("daemon.discovery_interval", d.discovery_interval, "30s"),
            health_interval: dur("daemon.health_interval", d.health_interval, "15s"),
            reconcile_interval: dur("daemon.reconcile_interval", d.reconcile_interval, "60s"),
            mount_timeout: dur("daemon.mount_timeout", d.mount_timeout, "30s"),
            offline_grace_period: dur(
                "reconciliation.offline_grace_period",
                r.offline_grace_period,
                "5m",
            ),
            retry_initial: dur("reconciliation.retry_initial", r.retry_initial, "2s"),
            retry_max: dur("reconciliation.retry_max", r.retry_max, "1m"),
        };
        if timings.mount_timeout > Duration::from_secs(300) {
            self.err("daemon.mount_timeout", "must be at most 5m");
        }
        // an unparsable retry_max is ZERO and already reported
        if !timings.retry_max.is_zero() && timings.retry_initial > timings.retry_max {
            self.err(
                "reconciliation.retry_initial",
                "must be <= reconciliation.retry_max",
            );
        }

        let mut policy = Policy {
            allow: self.matcher("policy.allow.", raw.policy.allow, false),
            deny: self.matcher("policy.deny.", raw.policy.deny, false),
            filters: BTreeMap::new(),
        };

        let mut names = BTreeSet::new();
        let mut providers = Vec::new();
        for (i, d) in raw.discovery.into_iter().enumerate() {
            let p = format!("discovery[{i}]");
            let kind = d.r#type;
            if !matches!(kind.as_str(), "tailscale" | "dns" | "http") {
                self.err(
                    &format!("{p}.type"),
                    format!("unknown type {kind:?} (expected tailscale, dns or http)"),
                );
            }
            let name = match &d.name {
                Some(n) => self
                    .check(&format!("{p}.name"), Name::parse(n))
                    .map(|n| n.as_str().to_string()),
                None => Some(kind.clone()),
            };
            if let Some(n) = &name {
                if n == "static" {
                    self.err(&format!("{p}.name"), "\"static\" is reserved");
                } else if !names.insert(n.clone()) {
                    let msg = format!(
                        "duplicate provider name {n:?} (a second provider of a type needs a `name`)"
                    );
                    self.err(&format!("{p}.name"), msg);
                }
            }
            let interval = match &d.interval {
                Some(s) => self.dur(&format!("{p}.interval"), s),
                None => timings.discovery_interval,
            };
            for (key, set, only) in [
                ("domain", d.domain.is_some(), "dns"),
                ("nameservers", !d.nameservers.is_empty(), "dns"),
                ("url", d.url.is_some(), "http"),
                ("headers", !d.headers.is_empty(), "http"),
            ] {
                if set && kind != only {
                    self.err(
                        &format!("{p}.{key}"),
                        format!("only allowed for type = {only:?}"),
                    );
                }
            }
            let spec = match kind.as_str() {
                "dns" => {
                    let domain = match &d.domain {
                        None => {
                            self.err(&format!("{p}.domain"), "required for type = \"dns\"");
                            None
                        }
                        Some(s) => match self.check(&format!("{p}.domain"), Host::parse(s)) {
                            Some(h) if h.ip().is_some() => {
                                self.err(
                                    &format!("{p}.domain"),
                                    format!("{s:?} is an IP address, not a domain"),
                                );
                                None
                            }
                            h => h,
                        },
                    };
                    let mut nameservers = Vec::new();
                    for (j, s) in d.nameservers.iter().enumerate() {
                        let r = s
                            .parse::<SocketAddr>()
                            .or_else(|_| s.parse::<IpAddr>().map(|ip| SocketAddr::new(ip, 53)))
                            .map_err(|_| format!("{s:?} is not an IP or IP:port"));
                        nameservers.extend(self.check(&format!("{p}.nameservers[{j}]"), r));
                    }
                    domain.map(|domain| ProviderSpec::Dns {
                        domain,
                        nameservers,
                    })
                }
                "http" => {
                    let url = match &d.url {
                        None => {
                            self.err(&format!("{p}.url"), "required for type = \"http\"");
                            None
                        }
                        Some(u) => self.expand(&format!("{p}.url"), u),
                    };
                    // never echo the url: it may carry a token
                    let url = url.filter(|u| {
                        let ok = url_allowed(u);
                        if !ok {
                            let msg = "must be https://, or http:// only to 127.0.0.1, [::1] or localhost";
                            self.err(&format!("{p}.url"), msg);
                        }
                        ok
                    });
                    let mut headers = Vec::new();
                    for (k, val) in d.headers {
                        let hp = format!("{p}.headers.{k:?}");
                        if !is_token(&k) {
                            self.err(&hp, "header name must be an RFC 7230 token");
                        } else if let Some(val) = self.expand(&hp, &val) {
                            // never echo the value: it is a secret
                            if val.contains(['\r', '\n', '\0']) {
                                self.err(&hp, "header value contains CR, LF or NUL");
                            } else {
                                headers.push((k, Secret(val)));
                            }
                        }
                    }
                    url.map(|url| ProviderSpec::Http { url, headers })
                }
                _ => Some(ProviderSpec::Tailscale),
            };

            let f = d.filter;
            let include = RawMatch {
                ids: f.include_ids,
                names: f.include_names,
                cidrs: f.include_cidrs,
                tags: f.include_tags,
                providers: vec![],
                metadata: f.include_metadata,
            };
            let exclude = RawMatch {
                ids: f.exclude_ids,
                names: f.exclude_names,
                cidrs: f.exclude_cidrs,
                tags: f.exclude_tags,
                providers: vec![],
                metadata: f.exclude_metadata,
            };
            let filter = ProviderFilter {
                include: self.matcher(&format!("{p}.filter.include_"), include, true),
                exclude: self.matcher(&format!("{p}.filter.exclude_"), exclude, true),
            };

            let t = d.mount;
            let user = match &t.user {
                None => Some(None),
                Some(u) => self
                    .check(&format!("{p}.mount.user"), User::parse(u))
                    .map(Some),
            };
            // ponytail: a discovered machine mounts its remote login dir (remote = "~", B2), not PRD §2's
            // ~/machines/agent-01/home/sami/project shape; upgrade: a per-provider default in docs or a smarter template
            let remote = RemotePath::parse(t.remote.as_deref().unwrap_or("~"));
            let remote = self.check(&format!("{p}.mount.remote"), remote);
            let driver = self.driver(
                &format!("{p}.mount.driver"),
                t.driver.as_deref(),
                &default_driver,
            );
            let (Some(name), Some(spec), Some(user), Some(remote), Some(driver)) =
                (name, spec, user, remote, driver)
            else {
                continue;
            };
            policy.filters.insert(name.clone(), filter);
            providers.push(ProviderConfig {
                name,
                kind,
                interval,
                template: MountTemplate {
                    user,
                    remote,
                    driver,
                    read_only: t.read_only.unwrap_or(false),
                    honor_hints: t.honor_hints.unwrap_or(false),
                },
                spec,
            });
        }

        // a configured provider name, or any kind (naming an unconfigured kind is harmless, not a typo)
        for (which, m) in [("allow", &policy.allow), ("deny", &policy.deny)] {
            for (j, pr) in m.providers.iter().enumerate() {
                if !names.contains(pr) && !TRUST.iter().any(|(k, _)| k == pr) {
                    let msg = format!(
                        "unknown provider {pr:?} (not a configured provider name or a kind)"
                    );
                    self.err(&format!("policy.{which}.providers[{j}]"), msg);
                }
            }
        }

        let mut ids = BTreeSet::new();
        let mut locals = BTreeSet::new();
        let mut machines = Vec::new();
        for (i, m) in raw.machines.into_iter().enumerate() {
            let p = format!("machines[{i}]");
            let np = format!("{p}.name");
            let id = self.check(&np, Name::parse(&m.name)).filter(|id| {
                let lower = id.as_str() == m.name;
                if !lower {
                    self.err(
                        &np,
                        format!("{:?}: use lowercase ({:?})", m.name, id.as_str()),
                    );
                }
                lower
            });
            if let Some(id) = &id
                && !ids.insert(id.clone())
            {
                self.err(&np, format!("duplicate machine {:?}", id.as_str()));
            }
            let host = self.check(&format!("{p}.host"), Host::parse(&m.host));
            let port = match m.port {
                None => Some(None),
                Some(n) => match u16::try_from(n) {
                    Ok(n) if n > 0 => Some(Some(n)),
                    _ => {
                        self.err(&format!("{p}.port"), format!("{n} is not in 1..=65535"));
                        None
                    }
                },
            };
            let user = match &m.user {
                None => Some(None),
                Some(u) => self.check(&format!("{p}.user"), User::parse(u)).map(Some),
            };
            let tags = self
                .tags(&format!("{p}.tags"), &m.tags)
                .into_iter()
                .collect();
            let metadata = self.meta(&format!("{p}.metadata"), m.metadata);

            // (key-path prefix, mount): the §31 shorthand reports its keys on the machine itself
            let mut entries: Vec<(String, RawMount1)> = m
                .mounts
                .into_iter()
                .enumerate()
                .map(|(j, mnt)| (format!("{p}.mounts[{j}]"), mnt))
                .collect();
            match m.remote {
                Some(_) if !entries.is_empty() => self.err(&p, "has both remote and mounts"),
                Some(remote) => entries.push((
                    p.clone(),
                    RawMount1 {
                        remote,
                        local: None,
                        driver: m.driver,
                        read_only: m.read_only,
                    },
                )),
                None => {
                    if entries.is_empty() {
                        self.err(&p, "has no mounts (set remote, or add [[machines.mounts]])");
                    }
                    if m.driver.is_some() {
                        self.err(&format!("{p}.driver"), "only allowed together with remote");
                    }
                    if m.read_only.is_some() {
                        self.err(
                            &format!("{p}.read_only"),
                            "only allowed together with remote",
                        );
                    }
                }
            }
            let single = entries.len() == 1;
            let mut mounts = Vec::new();
            for (mp, mnt) in entries {
                let lp = format!("{mp}.local");
                let local = match &mnt.local {
                    Some(l) => self.check(&lp, Name::parse(l)),
                    None if single => id.clone(),
                    None => {
                        self.err(&lp, "required when a machine has more than one mount");
                        None
                    }
                };
                if let Some(l) = &local
                    && !locals.insert(l.clone())
                {
                    self.err(&lp, format!("duplicate local {:?}", l.as_str()));
                }
                let remote = self.check(&format!("{mp}.remote"), RemotePath::parse(&mnt.remote));
                let driver = self.driver(
                    &format!("{mp}.driver"),
                    mnt.driver.as_deref(),
                    &default_driver,
                );
                if let (Some(local), Some(remote), Some(driver)) = (local, remote, driver) {
                    let read_only = mnt.read_only.unwrap_or(false);
                    mounts.push(StaticMount {
                        local,
                        remote,
                        driver,
                        read_only,
                    });
                }
            }
            if let (Some(id), Some(host), Some(port), Some(user)) = (id, host, port, user) {
                machines.push(StaticMachine {
                    id,
                    host,
                    port,
                    user,
                    tags,
                    metadata,
                    mounts,
                });
            }
        }

        Config {
            path: path.into(),
            root: root.unwrap_or_default(),
            default_driver,
            auto_order,
            ssh_config,
            vfs_cache_mode,
            timings,
            policy,
            providers,
            machines,
        }
    }
}

/// `~` / `~/` → $HOME; `$NAME` / `${NAME}` → env; `$$` → `$`. Undefined ⇒ error, never "".
/// Errors never echo the input: header values are secrets.
// ponytail: hand-rolled ~/$VAR expansion (§15 #4): no ~user, no ${VAR:-default}; add them if configs need them
fn expand(s: &str, env: Env) -> Result<String, String> {
    let var = |k: &str| env(k).ok_or_else(|| format!("undefined variable ${k}"));
    let (mut out, mut rest) = match s.strip_prefix('~') {
        Some(r) if r.is_empty() || r.starts_with('/') => (var("HOME")?, r),
        _ => (String::new(), s),
    };
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        rest = &rest[i + 1..];
        if let Some(r) = rest.strip_prefix('$') {
            out.push('$');
            rest = r;
            continue;
        }
        let (name, after) = match rest.strip_prefix('{') {
            Some(r) => r.split_once('}').unwrap_or(("", "")), // unterminated ⇒ "" ⇒ error below
            None => rest.split_at(
                rest.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .unwrap_or(rest.len()),
            ),
        };
        let valid = name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            return Err("bad $ reference: use $NAME, ${NAME}, or $$ for a literal $".into());
        }
        out.push_str(&var(name)?);
        rest = after;
    }
    out.push_str(rest);
    Ok(out)
}

/// https://, or http:// only to 127.0.0.1, [::1] or localhost. The http authority must be exactly one of those,
/// optionally with a digits-only port: no userinfo (`http://127.0.0.1@evil`), no suffix (`localhost.evil`).
fn url_allowed(u: &str) -> bool {
    let u = u.to_ascii_lowercase();
    if u.starts_with("https://") {
        return true;
    }
    let Some(rest) = u.strip_prefix("http://") else {
        return false;
    };
    let auth = rest.split(['/', '?', '#']).next().unwrap_or_default();
    ["127.0.0.1", "[::1]", "localhost"].iter().any(|h| {
        auth.strip_prefix(h).is_some_and(|p| {
            p.is_empty()
                || p.strip_prefix(':')
                    .is_some_and(|d| d.bytes().all(|b| b.is_ascii_digit()))
        })
    })
}

/// RFC 7230 token: 1*tchar
fn is_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::policy::{Match, ProviderFilter};
    use bifrost_core::validate::{Cidr, Glob, RemotePath};
    use bifrost_core::{Metadata, MountHints, Name};

    const HOME: &str = "/home/t";

    fn env<'a>(
        home: &'a str,
        vars: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> Option<String> + 'a {
        move |k| match k {
            "HOME" => Some(home.to_string()),
            _ => vars
                .iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| v.to_string()),
        }
    }
    fn p(text: &str) -> Result<Config, Vec<ConfigError>> {
        parse(
            text,
            Path::new("/c.toml"),
            &env(HOME, &[("XROOT", "/data")]),
        )
    }
    fn ok(text: &str) -> Config {
        p(text).unwrap_or_else(|e| panic!("{text}\n=> {e:#?}"))
    }
    /// Display lines
    fn errs(text: &str) -> Vec<String> {
        match p(text) {
            Ok(_) => panic!("expected errors for:\n{text}"),
            Err(e) => e.iter().map(|e| e.to_string()).collect(),
        }
    }
    fn has(errs: &[String], path: &str, needle: &str) -> bool {
        let pre = format!("error: {path}: ");
        errs.iter()
            .any(|e| e.starts_with(&pre) && e.contains(needle))
    }
    #[track_caller]
    fn assert_has(errs: &[String], path: &str, needle: &str) {
        assert!(
            has(errs, path, needle),
            "no `{path}: …{needle}…` in {errs:#?}"
        );
    }
    /// a temp HOME with .config/bifrost/ssh_config in it
    fn temp_home(test: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bf-config-{test}-{}", std::process::id()));
        std::fs::create_dir_all(d.join(".config/bifrost")).unwrap();
        std::fs::write(d.join(".config/bifrost/ssh_config"), "").unwrap();
        d
    }
    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }
    fn name(s: &str) -> Name {
        Name::parse(s).unwrap()
    }
    fn rp(s: &str) -> RemotePath {
        RemotePath::parse(s).unwrap()
    }
    fn named(s: &str) -> DriverSelector {
        DriverSelector::Named(s.into())
    }
    fn strs(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn machine<'a>(c: &'a Config, id: &str) -> &'a StaticMachine {
        c.machines.iter().find(|m| m.id.as_str() == id).unwrap()
    }

    const FULL: &str = r#"
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
honor_hints = false                           # true ⇒ record user=/path= win (validated); there is no driver hint (E2)

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
"#;

    #[test]
    fn parses_full_example() {
        let home = temp_home("full");
        let h = home.to_str().unwrap();
        let vars = [("BIFROST_INVENTORY_TOKEN", "s3cret")];
        let c = parse(FULL, Path::new("/c.toml"), &env(h, &vars)).unwrap();

        assert_eq!(c.path, Path::new("/c.toml"));
        assert_eq!(c.root, home.join("machines"));
        assert_eq!(c.default_driver, DriverSelector::Auto);
        assert_eq!(c.auto_order, strs(&["sshfs", "rclone"]));
        assert_eq!(c.ssh_config, Some(home.join(".config/bifrost/ssh_config")));
        assert_eq!(c.vfs_cache_mode, "writes");
        let t = &c.timings;
        assert_eq!(
            [
                t.discovery_interval,
                t.health_interval,
                t.reconcile_interval,
                t.mount_timeout
            ],
            [secs(30), secs(15), secs(60), secs(30)]
        );
        assert_eq!(
            [t.offline_grace_period, t.retry_initial, t.retry_max],
            [secs(300), secs(2), secs(60)]
        );

        assert_eq!(c.policy.allow.ids, strs(&["agent-07"]));
        let deny = &c.policy.deny;
        assert_eq!(deny.names, vec![Glob::parse("prod-*").unwrap()]);
        assert_eq!(deny.cidrs, vec![Cidr::parse("10.99.0.0/16").unwrap()]);
        assert_eq!(deny.tags, strs(&["prod"]));
        assert!(deny.providers.is_empty());
        assert_eq!(deny.metadata["env"], "production");

        let names: Vec<_> = c
            .providers
            .iter()
            .map(|p| (p.name.as_str(), p.kind.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("tailscale", "tailscale"),
                ("infra", "dns"),
                ("inventory", "http")
            ]
        );
        let (ts, infra, inv) = (&c.providers[0], &c.providers[1], &c.providers[2]);
        assert_eq!(ts.spec, ProviderSpec::Tailscale);
        assert_eq!(ts.interval, secs(30));
        assert_eq!(
            ts.template,
            MountTemplate {
                user: Some(User::parse("sami").unwrap()),
                remote: rp("~"),
                driver: DriverSelector::Auto,
                read_only: false,
                honor_hints: false,
            }
        );
        assert_eq!(
            infra.spec,
            ProviderSpec::Dns {
                domain: Host::parse("infra.example.com").unwrap(),
                nameservers: vec![
                    "10.0.0.2:53".parse().unwrap(),
                    "127.0.0.1:5353".parse().unwrap()
                ],
            }
        );
        assert_eq!(
            infra.interval,
            secs(30),
            "defaults to daemon.discovery_interval"
        );
        assert_eq!(infra.template.user, Some(User::parse("ubuntu").unwrap()));
        assert_eq!(infra.template.remote, rp("~"));
        assert!(infra.template.honor_hints);
        assert_eq!(
            inv.spec,
            ProviderSpec::Http {
                url: "https://inventory.example.com/bifrost/v1/machines".into(),
                headers: vec![("Authorization".into(), Secret("Bearer s3cret".into()))],
            }
        );
        assert_eq!(inv.interval, secs(60));
        assert!(
            !format!("{inv:?}").contains("s3cret"),
            "Secret must not Debug-print"
        );

        let f = &c.policy.filters;
        assert_eq!(f.len(), 3);
        assert_eq!(f["tailscale"].include.tags, strs(&["dev", "agent"]));
        assert_eq!(
            f["tailscale"].exclude.names,
            vec![Glob::parse("*-old").unwrap()]
        );
        assert_eq!(f["infra"].include.names.len(), 2);
        assert_eq!(
            f["inventory"].include.cidrs,
            vec![Cidr::parse("10.20.0.0/16").unwrap()]
        );
        assert_eq!(f["inventory"].include.metadata["env"], "dev");

        let build = machine(&c, "build");
        assert_eq!(build.host, Host::parse("10.0.0.18").unwrap());
        assert_eq!(build.port, Some(22));
        assert_eq!(build.tags, BTreeSet::from(["ci".to_string()]));
        assert_eq!(build.metadata["env"], "ci");
        assert_eq!(
            build.mounts,
            vec![
                StaticMount {
                    local: name("build"),
                    remote: rp("/home/sami"),
                    driver: named("sshfs"),
                    read_only: false
                },
                StaticMount {
                    local: name("build-artifacts"),
                    remote: rp("/srv/artifacts"),
                    driver: named("rclone"),
                    read_only: true,
                },
            ]
        );
        let agent = machine(&c, "agent-01");
        assert_eq!(
            agent.mounts,
            vec![StaticMount {
                local: name("agent-01"),
                remote: rp("/home/sami"),
                driver: DriverSelector::Auto,
                read_only: false
            }]
        );

        // how allowed machines become mount-spec inputs
        let obs = c.static_observations();
        assert_eq!(obs.len(), 2);
        assert_eq!(
            obs[0],
            MachineObservation {
                id: name("build"),
                name: "build".into(),
                native_id: None,
                addresses: vec![Host::parse("10.0.0.18").unwrap()],
                port: Some(22),
                online: None,
                metadata: Metadata {
                    tags: BTreeSet::from(["ci".to_string()]),
                    values: BTreeMap::from([("env".to_string(), "ci".to_string())]),
                },
                hints: MountHints {
                    user: Some(User::parse("sami").unwrap()),
                    path: None
                },
                ttl: None,
            }
        );
        let sm = c.static_mounts();
        assert_eq!(sm.len(), 2);
        assert_eq!(sm[&name("build")], build.mounts);
        let tpl = c.templates();
        assert_eq!(
            tpl.keys().collect::<Vec<_>>(),
            ["infra", "inventory", "tailscale"]
        );
        assert_eq!(tpl["tailscale"], ts.template);
    }

    #[test]
    fn parses_prd_s6_s7_s13_s31_snippets() {
        // PRD §6.1 (remote = "/", B2)
        let c = ok(r#"
[[machines]]
name = "build"
host = "10.10.10.12"
user = "sami"

[[machines.mounts]]
remote = "/"
local = "build"
driver = "sshfs"
"#);
        assert_eq!(c.machines[0].mounts[0].remote, rp("/"));
        assert_eq!(c.root, Path::new("/home/t/machines"));

        // PRD §7
        let c = ok(r#"
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
"#);
        assert_eq!(c.providers.len(), 2);
        assert_eq!(c.providers[1].name, "dns");
        assert!(
            c.providers[1].template.remote == rp("~") && c.providers[1].template.user.is_none()
        );

        // PRD §13 (reconcile_interval = "30s" is accepted)
        let c = ok(r#"
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
"#);
        assert_eq!(c.timings.reconcile_interval, secs(30));
        assert_eq!((c.providers.len(), c.machines.len()), (2, 1));

        // PRD §31
        let c = ok(r#"
[mount]
root = "~/machines"

[[machines]]
name = "agent-01"
host = "agent-01"
user = "sami"
remote = "/home/sami"
"#);
        assert_eq!(c.machines[0].mounts[0].local, name("agent-01"));
    }

    #[test]
    fn shorthand_equals_mounts_form() {
        let short = ok(r#"
[[machines]]
name = "agent-01"
host = "agent-01"
user = "sami"
remote = "/home/sami"
driver = "rclone"
read_only = true
"#);
        let long = ok(r#"
[[machines]]
name = "agent-01"
host = "agent-01"
user = "sami"

[[machines.mounts]]
remote = "/home/sami"
driver = "rclone"
read_only = true
"#);
        assert_eq!(short, long);
        assert_eq!(short.machines[0].mounts[0].local, name("agent-01"));
    }

    #[test]
    fn empty_text_is_default_config() {
        let want = Config {
            path: "/c.toml".into(),
            root: "/home/t/machines".into(),
            default_driver: DriverSelector::Auto,
            auto_order: default_auto_order(),
            ssh_config: None,
            vfs_cache_mode: "writes".into(),
            timings: Timings {
                discovery_interval: secs(30),
                health_interval: secs(15),
                reconcile_interval: secs(60),
                mount_timeout: secs(30),
                offline_grace_period: secs(300),
                retry_initial: secs(2),
                retry_max: secs(60),
            },
            policy: Policy::default(),
            providers: vec![],
            machines: vec![],
        };
        assert_eq!(ok(""), want);
        assert_eq!(ok("# only a comment\n"), want);
    }

    const MANY_ERRORS: &str = r#"
version = 2
[mount]
root = "/"
default_driver = "fuse"
vfs_cache_mode = "most"
[daemon]
health_interval = "5"
[[machines]]
name = "Zed"
host = "-oProxyCommand=x"
remote = "rel"
[[machines]]
name = "a"
host = "a"
"#;

    #[test]
    fn errors_sorted_deterministic() {
        let a = errs(MANY_ERRORS);
        let b = errs(MANY_ERRORS);
        assert_eq!(a.join("\n").into_bytes(), b.join("\n").into_bytes());
        assert!(a.len() >= 8, "{a:#?}");
        let raw = p(MANY_ERRORS).unwrap_err();
        let mut sorted = raw.clone();
        sorted.sort();
        assert_eq!(raw, sorted);
        assert_has(&a, "version", "1");
        assert_has(&a, "machines[1]", "has no mounts");
    }

    #[test]
    fn unknown_field_has_line_col() {
        let e = p("[mount]\nroot = \"/x\"\npasword = \"hunter2\"\n").unwrap_err();
        assert_eq!(e.len(), 1, "{e:#?}");
        assert_eq!(e[0].path, "/c.toml:3:1");
        assert!(e[0].message.contains("pasword"), "{e:#?}");
        assert!(!e[0].message.contains('\n'));
        // syntax error, one error only
        let e = p("[mount\nroot = 1\n").unwrap_err();
        assert_eq!(e.len(), 1);
        assert!(e[0].path.starts_with("/c.toml:1:"), "{e:#?}");
        // unknown key inside a [[discovery]] entry keeps line/col (flat RawDiscovery)
        let e = p("[[discovery]]\ntype = \"dns\"\ndomain = \"x.com\"\nzone = \"x\"\n").unwrap_err();
        assert_eq!(e[0].path, "/c.toml:4:1", "{e:#?}");
        // wrong type
        let e = p("[[machines]]\nname = \"a\"\nhost = \"a\"\nport = \"22\"\nremote = \"~\"\n")
            .unwrap_err();
        assert!(e[0].path.starts_with("/c.toml:4:"), "{e:#?}");
    }

    #[test]
    fn per_type_keys_enforced() {
        let e = errs(
            r#"
[[discovery]]
type = "tailscale"
domain = "x.com"
url = "https://x"
[[discovery]]
type = "dns"
name = "d1"
[[discovery]]
type = "dns"
name = "d2"
domain = "10.0.0.1"
headers = { A = "b" }
[[discovery]]
type = "http"
nameservers = ["10.0.0.2"]
[[discovery]]
type = "dns"
name = "d3"
domain = "-bad"
nameservers = ["not-an-ip", "::1", "[::1]:53"]
[[discovery]]
type = "consul"
"#,
        );
        assert_has(&e, "discovery[0].domain", "dns");
        assert_has(&e, "discovery[0].url", "http");
        assert_has(&e, "discovery[1].domain", "required");
        assert_has(&e, "discovery[2].domain", "IP");
        assert_has(&e, "discovery[2].headers", "http");
        assert_has(&e, "discovery[3].url", "required");
        assert_has(&e, "discovery[3].nameservers", "dns");
        assert_has(&e, "discovery[4].domain", "host");
        assert_has(&e, "discovery[4].nameservers[0]", "not-an-ip");
        assert!(
            !e.iter()
                .any(|l| l.contains("nameservers[1]") || l.contains("nameservers[2]")),
            "{e:#?}"
        );
        assert_has(&e, "discovery[5].type", "consul");
    }

    #[test]
    fn remote_xor_mounts() {
        let e = errs(
            r#"
[[machines]]
name = "none"
host = "h"
[[machines]]
name = "both"
host = "h"
remote = "~"
[[machines.mounts]]
remote = "/x"
[[machines]]
name = "loose"
host = "h"
driver = "sshfs"
read_only = true
[[machines.mounts]]
remote = "/x"
local = "loose"
"#,
        );
        assert_has(&e, "machines[0]", "has no mounts");
        assert_has(&e, "machines[1]", "both remote and mounts");
        assert_has(&e, "machines[2].driver", "remote");
        assert_has(&e, "machines[2].read_only", "remote");
    }

    #[test]
    fn local_traversal_rejected() {
        for bad in ["../x", "a/b", ".", "..", ".x", "-x", "a b"] {
            let e = errs(&format!(
                "[[machines]]\nname = \"m\"\nhost = \"h\"\n[[machines.mounts]]\nremote = \"~\"\nlocal = {bad:?}\n"
            ));
            assert_has(&e, "machines[0].mounts[0].local", "");
        }
        // two mounts: local is required
        let e = errs(
            "[[machines]]\nname = \"m\"\nhost = \"h\"\n[[machines.mounts]]\nremote = \"~\"\n[[machines.mounts]]\nremote = \"/x\"\nlocal = \"m-x\"\n",
        );
        assert_has(&e, "machines[0].mounts[0].local", "required");
    }

    #[test]
    fn duplicate_local_rejected() {
        let e = errs(
            r#"
[[machines]]
name = "a"
host = "h"
remote = "~"
[[machines]]
name = "b"
host = "h"
[[machines.mounts]]
remote = "/x"
local = "a"
[[machines.mounts]]
remote = "/y"
local = "b"
"#,
        );
        assert_eq!(
            e,
            ["error: machines[1].mounts[0].local: duplicate local \"a\""]
        );
    }

    #[test]
    fn duplicate_machine_rejected() {
        let e = errs(
            "[[machines]]\nname = \"a\"\nhost = \"h\"\nremote = \"~\"\n[[machines]]\nname = \"a\"\nhost = \"h2\"\n[[machines.mounts]]\nremote = \"/x\"\nlocal = \"a2\"\n",
        );
        assert_has(&e, "machines[1].name", "duplicate machine");
    }

    #[test]
    fn uppercase_name_hint() {
        let e = errs("[[machines]]\nname = \"Agent-01\"\nhost = \"h\"\nremote = \"~\"\n");
        assert_has(&e, "machines[0].name", "use lowercase");
    }

    #[test]
    fn unknown_driver_rejected() {
        let e = errs(
            r#"
[mount]
default_driver = "fuse"
[[discovery]]
type = "tailscale"
[discovery.mount]
driver = "nfs"
[[machines]]
name = "a"
host = "h"
remote = "~"
driver = "SSHFS"
[[machines]]
name = "b"
host = "h"
[[machines.mounts]]
remote = "~"
driver = "rclone-nfs2"
"#,
        );
        assert_has(&e, "mount.default_driver", "fuse");
        assert_has(&e, "discovery[0].mount.driver", "nfs");
        assert_has(&e, "machines[0].driver", "SSHFS");
        assert_has(&e, "machines[1].mounts[0].driver", "rclone-nfs2");

        let e = errs("[mount]\nauto_order = []\n");
        assert_has(&e, "mount.auto_order", "empty");
        let e = errs("[mount]\nauto_order = [\"sshfs\", \"nope\", \"sshfs\"]\n");
        assert_has(&e, "mount.auto_order[1]", "nope");
        assert_has(&e, "mount.auto_order[2]", "duplicate");
        // every known name is fine, whatever the OS
        let c = ok(
            "[mount]\ndefault_driver = \"rclone-nfs\"\nauto_order = [\"rclone-nfs\", \"rclone\", \"sshfs\"]\n",
        );
        assert_eq!(c.default_driver, named("rclone-nfs"));
    }

    #[test]
    fn bad_duration_rejected() {
        let e = errs(
            r#"
[daemon]
discovery_interval = "5"
health_interval = "1d"
reconcile_interval = "500ms"
mount_timeout = "6m"
[reconciliation]
offline_grace_period = "5 m"
[[discovery]]
type = "tailscale"
interval = "0s"
"#,
        );
        assert_has(&e, "daemon.discovery_interval", "\"5\"");
        assert_has(&e, "daemon.health_interval", "\"1d\"");
        assert_has(&e, "daemon.reconcile_interval", "1s");
        assert_has(&e, "daemon.mount_timeout", "5m");
        assert_has(&e, "reconciliation.offline_grace_period", "\"5 m\"");
        assert_has(&e, "discovery[0].interval", "\"0s\"");
        assert_eq!(
            ok("[daemon]\nmount_timeout = \"5m\"\n")
                .timings
                .mount_timeout,
            secs(300)
        );
    }

    #[test]
    fn retry_initial_gt_max_rejected() {
        let e = errs("[reconciliation]\nretry_initial = \"2m\"\nretry_max = \"1m\"\n");
        assert_has(&e, "reconciliation.retry_initial", "retry_max");
        // the default retry_max (1m) applies too
        let e = errs("[reconciliation]\nretry_initial = \"90s\"\n");
        assert_has(&e, "reconciliation.retry_initial", "retry_max");
        ok("[reconciliation]\nretry_initial = \"1m\"\nretry_max = \"1m\"\n");
    }

    #[test]
    fn root_slash_home_relative_rejected() {
        for (root, why) in [
            ("/", "/"),
            ("//", "/"),
            ("~", "HOME"),
            ("~/", "HOME"),
            ("$HOME", "HOME"),
            ("/home/t/.", "HOME"),
            ("machines", "absolute"),
            ("./m", "absolute"),
            ("~user/m", "absolute"),
            ("/tmp/../etc", ".."),
            ("~/a/../b", ".."),
        ] {
            let e = errs(&format!("[mount]\nroot = {root:?}\n"));
            assert_has(&e, "mount.root", why);
        }
        assert_eq!(
            ok("[mount]\nroot = \"/home/t/m\"\n").root,
            Path::new("/home/t/m")
        );
    }

    #[test]
    fn tilde_and_vars_expanded() {
        let c = ok("[mount]\nroot = \"~/m\"\n");
        assert_eq!(c.root, Path::new("/home/t/m"));
        let c = ok("[mount]\nroot = \"$XROOT/m\"\n");
        assert_eq!(c.root, Path::new("/data/m"));
        let c = ok("[mount]\nroot = \"${XROOT}x/m\"\n");
        assert_eq!(c.root, Path::new("/datax/m"));
        let c = ok(
            "[[discovery]]\ntype = \"http\"\nurl = \"https://$XROOT.example/${XROOT}\"\nheaders = { X-Root = \"~/${XROOT}\" }\n",
        );
        match &c.providers[0].spec {
            ProviderSpec::Http { url, headers } => {
                assert_eq!(url, "https:///data.example//data");
                assert_eq!(
                    headers,
                    &vec![("X-Root".to_string(), Secret("/home/t//data".into()))]
                );
            }
            s => panic!("{s:?}"),
        }
        // names, hosts and remote paths are never expanded; remote "~" stays the remote home
        let c = ok(
            "[[machines]]\nname = \"m\"\nhost = \"h\"\nremote = \"~/x\"\n[[discovery]]\ntype = \"tailscale\"\n[discovery.mount]\nremote = \"~/y\"\n",
        );
        assert_eq!(c.machines[0].mounts[0].remote, rp("~/x"));
        assert_eq!(c.providers[0].template.remote, rp("~/y"));
        let e = errs("[[machines]]\nname = \"m\"\nhost = \"$XROOT\"\nremote = \"$XROOT\"\n");
        assert_has(&e, "machines[0].host", "$XROOT");
        assert_has(&e, "machines[0].remote", "$XROOT");
    }

    #[test]
    fn undefined_var_error() {
        let e = errs("[mount]\nroot = \"$NOPE/x\"\n");
        assert_has(&e, "mount.root", "undefined variable $NOPE");
        let e = errs("[mount]\nroot = \"/x/${NOPE}\"\n");
        assert_has(&e, "mount.root", "undefined variable $NOPE");
        // no HOME ⇒ "~" is an error, never ""
        let noenv = |_: &str| None;
        let e = parse("", Path::new("/c.toml"), &noenv).unwrap_err();
        assert_eq!(e[0].path, "mount.root");
        assert!(e[0].message.contains("undefined variable $HOME"), "{e:#?}");
        // malformed references
        for bad in ["/x/$", "/x/${", "/x/${}", "/x/${A-B}", "/x/$-"] {
            let e = errs(&format!("[mount]\nroot = {bad:?}\n"));
            assert_has(&e, "mount.root", "$$");
        }
        // a secret header value is never echoed in an error
        let e = errs(
            "[[discovery]]\ntype = \"http\"\nurl = \"https://x\"\nheaders = { A = \"tok3n$\" }\n",
        );
        assert!(!e.join("\n").contains("tok3n"), "{e:#?}");
    }

    #[test]
    fn dollar_dollar_literal() {
        assert_eq!(
            ok("[mount]\nroot = \"/x/a$$b\"\n").root,
            Path::new("/x/a$b")
        );
        assert_eq!(
            ok("[mount]\nroot = \"/x/$$XROOT\"\n").root,
            Path::new("/x/$XROOT")
        );
        assert_eq!(
            ok("[mount]\nroot = \"/x/$$$XROOT\"\n").root,
            Path::new("/x/$/data")
        );
    }

    #[test]
    fn http_plaintext_non_loopback_rejected() {
        let url = |u: &str| p(&format!("[[discovery]]\ntype = \"http\"\nurl = {u:?}\n"));
        for bad in [
            "http://example.com/x",
            "http://10.0.0.1/x",
            "http://127.0.0.1@evil.com/",
            "http://localhost:80@evil.com/",
            "http://localhost.evil.com/",
            "http://127.0.0.1.evil.com/",
            "http://127.0.0.2/",
            "http:///evil.com/",
            "http://evil.com\\@127.0.0.1/",
            "http://localhost:8o/",
            "ftp://127.0.0.1/",
            "inventory.example.com",
            " http://localhost/",
        ] {
            let e = url(bad).unwrap_err();
            assert!(
                e.iter().any(|e| e.path == "discovery[0].url"),
                "{bad}: {e:#?}"
            );
        }
        for good in [
            "https://inventory.example.com/bifrost/v1/machines",
            "HTTPS://Inventory.example.com",
            "http://127.0.0.1:18080/inv",
            "http://[::1]/",
            "http://[::1]:8080",
            "http://localhost:18080?x=1",
            "http://LOCALHOST#frag",
        ] {
            assert!(url(good).is_ok(), "{good}: {:#?}", url(good));
        }
    }

    #[test]
    fn header_crlf_rejected() {
        let text = |h: &str| {
            format!("[[discovery]]\ntype = \"http\"\nurl = \"https://x\"\nheaders = {{ {h} }}\n")
        };
        let e = p(&text(r#"X-A = "a\r\nX-Evil: 1""#)).unwrap_err();
        assert_eq!(e.len(), 1, "{e:#?}");
        assert!(
            e[0].path.starts_with("discovery[0].headers") && e[0].message.contains("CR"),
            "{e:#?}"
        );
        let e = p(&text(r#"X-A = "a\u0000""#)).unwrap_err();
        assert!(e[0].message.contains("NUL"), "{e:#?}");
        // after expansion: a newline smuggled in through the environment
        let vars = [("TOK", "x\ny")];
        let e = parse(
            &text(r#"X-A = "$TOK""#),
            Path::new("/c.toml"),
            &env(HOME, &vars),
        )
        .unwrap_err();
        assert!(e[0].message.contains("LF"), "{e:#?}");
        assert!(!e[0].message.contains("x\ny"));
        // header name must be an RFC 7230 token
        for bad in [
            r#""Bad Name" = "v""#,
            r#""X:Y" = "v""#,
            r#""" = "v""#,
            r#""X\n" = "v""#,
        ] {
            let e = p(&text(bad)).unwrap_err();
            assert!(
                e[0].path.starts_with("discovery[0].headers") && e[0].message.contains("token"),
                "{bad}: {e:#?}"
            );
        }
        ok(&text(
            r#""X-Weird!#$%&'*+.^_`|~" = "v", Authorization = "Bearer a b""#,
        ));
    }

    #[test]
    fn discovery_names_default_unique_static_reserved() {
        let c = ok(
            "[[discovery]]\ntype = \"tailscale\"\n[[discovery]]\ntype = \"dns\"\ndomain = \"a.com\"\n[[discovery]]\ntype = \"dns\"\nname = \"dns2\"\ndomain = \"b.com\"\n",
        );
        let names: Vec<_> = c.providers.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["tailscale", "dns", "dns2"]);
        let e = errs(
            "[[discovery]]\ntype = \"dns\"\ndomain = \"a.com\"\n[[discovery]]\ntype = \"dns\"\ndomain = \"b.com\"\n",
        );
        assert_has(&e, "discovery[1].name", "duplicate provider name \"dns\"");
        let e = errs("[[discovery]]\ntype = \"tailscale\"\nname = \"static\"\n");
        assert_has(&e, "discovery[0].name", "reserved");
        let e = errs("[[discovery]]\ntype = \"tailscale\"\nname = \"../x\"\n");
        assert_has(&e, "discovery[0].name", "");
        let e = errs(
            "[[discovery]]\ntype = \"tailscale\"\n[[discovery]]\ntype = \"http\"\nname = \"tailscale\"\nurl = \"https://x\"\n",
        );
        assert_has(&e, "discovery[1].name", "duplicate");
    }

    #[test]
    fn policy_unknown_provider_rejected() {
        let e = errs(
            "[policy.deny]\nproviders = [\"tailscale\", \"nope\"]\n[policy.allow]\nproviders = [\"infra\"]\n",
        );
        assert_has(&e, "policy.deny.providers[1]", "nope");
        assert_has(&e, "policy.allow.providers[0]", "infra");
        assert!(
            !has(&e, "policy.deny.providers[0]", ""),
            "a kind name is always known: {e:#?}"
        );
        let c = ok(r#"
[policy.allow]
providers = ["infra", "static", "dns", "http"]
[[discovery]]
type = "dns"
name = "infra"
domain = "infra.example.com"
"#);
        assert_eq!(
            c.policy.allow.providers,
            strs(&["infra", "static", "dns", "http"])
        );
        // the rest of the filter/policy grammar
        let e = errs(
            r#"
[policy.allow]
ids = ["../x", "Agent-07"]
names = ["Prod-*"]
cidrs = ["10.0.0.0/33", "::ffff:10.0.0.1"]
tags = ["-x"]
metadata = { "Bad Key" = "v", env = "a\u0007b" }
[[discovery]]
type = "tailscale"
[discovery.filter]
include_ids = ["nABC:1", "a b"]
exclude_metadata = { k = "x" }
"#,
        );
        assert_has(&e, "policy.allow.ids[0]", "..");
        assert_has(&e, "policy.allow.names[0]", "Prod-*");
        assert_has(&e, "policy.allow.cidrs[0]", "");
        assert_has(&e, "policy.allow.cidrs[1]", "");
        assert_has(&e, "policy.allow.tags[0]", "");
        assert_has(&e, "policy.allow.metadata.\"Bad Key\"", "");
        assert_has(&e, "policy.allow.metadata.\"env\"", "control");
        assert_has(&e, "discovery[0].filter.include_ids[1]", "");
        assert!(
            !has(&e, "discovery[0].filter.include_ids[0]", ""),
            "native ids are allowed: {e:#?}"
        );
        assert!(!has(&e, "policy.allow.ids[1]", ""), "{e:#?}");
    }

    #[test]
    fn ssh_config_with_quote_rejected() {
        let home = temp_home("sshcfg");
        std::fs::write(home.join("bad\"cfg"), "").unwrap();
        let h = home.to_str().unwrap();
        let cfg = |s: &str| {
            parse(
                &format!("[mount]\nssh_config = {s:?}\n"),
                Path::new("/c.toml"),
                &env(h, &[]),
            )
        };
        let e = cfg("~/bad\"cfg").unwrap_err();
        assert!(
            e[0].path == "mount.ssh_config" && e[0].message.contains('"'),
            "{e:#?}"
        );
        let e = cfg("~/missing").unwrap_err();
        assert!(e[0].message.contains("exist"), "{e:#?}");
        let e = cfg("relative/cfg").unwrap_err();
        assert!(e[0].message.contains("absolute"), "{e:#?}");
        assert_eq!(
            cfg("~/.config/bifrost/ssh_config").unwrap().ssh_config,
            Some(home.join(".config/bifrost/ssh_config"))
        );
        assert_eq!(
            cfg("/dev/null").unwrap().ssh_config,
            Some("/dev/null".into())
        );
    }

    #[test]
    fn remote_slash_root_ok() {
        let c = ok(
            "[[machines]]\nname = \"m\"\nhost = \"h\"\nremote = \"/\"\n[[discovery]]\ntype = \"tailscale\"\n[discovery.mount]\nremote = \"/\"\n",
        );
        assert_eq!(c.machines[0].mounts[0].remote.sftp_path(), "/");
        assert_eq!(c.providers[0].template.remote, rp("/"));
        let e = errs(
            "[[machines]]\nname = \"m\"\nhost = \"h\"\nremote = \"/a/../b\"\n[[discovery]]\ntype = \"tailscale\"\n[discovery.mount]\nremote = \"rel\"\nuser = \"-oops\"\n",
        );
        assert_has(&e, "machines[0].remote", "..");
        assert_has(&e, "discovery[0].mount.remote", "rel");
        assert_has(&e, "discovery[0].mount.user", "-oops");
    }

    #[test]
    fn trust_ranks_and_sources() {
        assert_eq!(
            TRUST,
            [("static", 0), ("tailscale", 1), ("http", 2), ("dns", 3)]
        );
        assert_eq!(
            static_source(),
            Source {
                trust: 0,
                kind: "static".into(),
                provider: "static".into()
            }
        );
        let c = ok(
            "[[discovery]]\ntype = \"dns\"\nname = \"infra\"\ndomain = \"a.com\"\n[[discovery]]\ntype = \"http\"\nurl = \"https://x\"\n[[discovery]]\ntype = \"tailscale\"\n",
        );
        let src: Vec<_> = c.providers.iter().map(|p| p.source()).collect();
        assert_eq!(
            src,
            [
                Source {
                    trust: 3,
                    kind: "dns".into(),
                    provider: "infra".into()
                },
                Source {
                    trust: 2,
                    kind: "http".into(),
                    provider: "http".into()
                },
                Source {
                    trust: 1,
                    kind: "tailscale".into(),
                    provider: "tailscale".into()
                },
            ]
        );
        let mut sorted = src.clone();
        sorted.push(static_source());
        sorted.sort();
        assert_eq!(
            sorted.iter().map(|s| s.trust).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert_eq!(DRIVER_NAMES, ["sshfs", "rclone", "rclone-nfs"]);
        let want: &[&str] = if cfg!(target_os = "macos") {
            &["rclone-nfs", "rclone", "sshfs"]
        } else {
            &["sshfs", "rclone"]
        };
        assert_eq!(default_auto_order(), strs(want));
        // a default filter entry exists per provider
        assert_eq!(
            c.policy.filters["infra"],
            ProviderFilter {
                include: Match::default(),
                exclude: Match::default()
            }
        );
    }

    #[test]
    fn machine_fields_validated() {
        let e = errs(
            r#"
[[machines]]
name = "a"
host = "u@h"
port = 0
user = "-root"
tags = ["ok", "Bad Tag"]
metadata = { "K" = "v" }
remote = "~"
[[machines]]
name = "b"
host = "h"
port = 65536
remote = "~"
"#,
        );
        assert_has(&e, "machines[0].host", "u@h");
        assert_has(&e, "machines[0].port", "1..=65535");
        assert_has(&e, "machines[0].user", "-root");
        assert_has(&e, "machines[0].tags[1]", "Bad Tag");
        assert_has(&e, "machines[0].metadata.\"K\"", "");
        assert_has(&e, "machines[1].port", "65536");
        let c = ok(
            "[[machines]]\nname = \"a\"\nhost = \"h\"\nport = 65535\ntags = [\"CI\"]\nremote = \"~\"\n",
        );
        assert_eq!(c.machines[0].port, Some(65535));
        assert_eq!(c.machines[0].tags, BTreeSet::from(["ci".to_string()]));
        let e = errs("[mount]\nvfs_cache_mode = \"most\"\n");
        assert_has(&e, "mount.vfs_cache_mode", "most");
        let e = errs("version = 2\n");
        assert_has(&e, "version", "2");
    }

    #[test]
    fn load_reads_file_and_reports_missing() {
        let dir = temp_home("load");
        let f = dir.join("config.toml");
        std::fs::write(&f, "[mount]\nroot = \"/tmp/bf-m\"\n").unwrap();
        assert_eq!(load(&f).unwrap().root, Path::new("/tmp/bf-m"));
        let e = load(&dir.join("nope.toml")).unwrap_err();
        assert_eq!(e.len(), 1);
        assert!(
            e[0].path.ends_with("nope.toml") && e[0].message.contains("cannot read"),
            "{e:#?}"
        );
    }
}
