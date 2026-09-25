//! Configuration: TOML → validated `Config` (contract §3). STUB (S0.3): S1-B implements the bodies.

pub mod paths;
mod raw;

use bifrost_core::policy::Policy;
use bifrost_core::reconcile::{MountTemplate, StaticMount};
use bifrost_core::registry::Source;
use bifrost_core::{DriverSelector, Host, MachineId, MachineObservation, User};
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
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
            trust: u8::MAX, // STUB (S1-B): rank from TRUST
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

fn not_implemented() -> Vec<ConfigError> {
    vec![ConfigError {
        path: String::new(),
        message: "not implemented".into(),
    }]
}

/// Pure. HOME is looked up through `env("HOME")`. TOML syntax/shape error ⇒ exactly one ConfigError (with line:col).
/// Otherwise returns ALL semantic errors, sorted. parse("", …) == the default config.
pub fn parse(
    _text: &str,
    _path: &Path,
    _env: &dyn Fn(&str) -> Option<String>,
) -> Result<Config, Vec<ConfigError>> {
    Err(not_implemented()) // STUB (S1-B)
}

/// read + parse with the process env
pub fn load(_path: &Path) -> Result<Config, Vec<ConfigError>> {
    Err(not_implemented()) // STUB (S1-B)
}

impl Config {
    /// addresses=[host], hints.user=user, online None, ttl None
    pub fn static_observations(&self) -> Vec<MachineObservation> {
        vec![] // STUB (S1-B)
    }
    pub fn static_mounts(&self) -> BTreeMap<MachineId, Vec<StaticMount>> {
        BTreeMap::new() // STUB (S1-B)
    }
    /// keyed by provider name
    pub fn templates(&self) -> BTreeMap<String, MountTemplate> {
        BTreeMap::new() // STUB (S1-B)
    }
}
