//! Core is provider- and driver-agnostic (B8): no ProviderKind enum, no DRIVER_NAMES, no default_auto_order here.
//! Provider kinds are plain strings with a numeric trust rank (registry::Source); driver names, the auto order and
//! the trust ranks live in bifrost-config (§3).
//! MountSpec::{fingerprint, source}, marker, parse_marker and DriverSelector: TryFrom<String> are implemented
//! and tested in S0 (A2, S0.5), because S1 agents B and C depend on them.

use crate::validate::{Host, Invalid, MachineId, MountId, RemotePath, User};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

/// tags via tag(), keys via meta_key(), values clean(v, 256)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub tags: BTreeSet<String>,
    pub values: BTreeMap<String, String>,
}

/// Untrusted, pre-validated. user/path are used only with `honor_hints`. There is no driver hint (E2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountHints {
    pub user: Option<User>,
    pub path: Option<RemotePath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineObservation {
    pub id: MachineId,
    /// display, clean(name, 128)
    pub name: String,
    /// tailscale ID / TXT id= / HTTP id — matched only by the OWNING provider's
    /// include_ids/exclude_ids; global `ids` match the machine id only (A19)
    pub native_id: Option<String>,
    /// ≥1; [0] = connect target
    pub addresses: Vec<Host>,
    pub port: Option<u16>,
    /// Some only when the provider knows (tailscale, http)
    pub online: Option<bool>,
    pub metadata: Metadata,
    /// static: user = config user (trusted)
    pub hints: MountHints,
    /// DNS only; registry floors it
    pub ttl: Option<Duration>,
}

/// "auto" → Auto; anything else must match ^[a-z0-9][a-z0-9._-]{0,62}$ (no lowercasing) → Named.
/// Core checks only this grammar; membership in DRIVER_NAMES is checked by bifrost-config (B8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum DriverSelector {
    Auto,
    Named(String),
}

impl TryFrom<String> for DriverSelector {
    type Error = Invalid;
    fn try_from(s: String) -> Result<Self, Invalid> {
        // STUB (S0.5): grammar check missing; accepts all input until then
        Ok(if s == "auto" {
            Self::Auto
        } else {
            Self::Named(s)
        })
    }
}

impl From<DriverSelector> for String {
    fn from(d: DriverSelector) -> String {
        match d {
            DriverSelector::Auto => "auto".into(),
            DriverSelector::Named(n) => n,
        }
    }
}

/// PRD §5 (machine = resolved id; options = read_only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    pub id: MountId,
    pub machine: MachineId,
    pub host: Host,
    pub port: Option<u16>,
    pub user: Option<User>,
    pub remote: RemotePath,
    /// canonical_root.join(id) — built only in reconcile::desired
    pub local_path: PathBuf,
    /// the SELECTOR text is fingerprinted ⇒ "auto" is sticky across probe flaps
    pub driver: DriverSelector,
    pub read_only: bool,
}

impl MountSpec {
    /// 16 lowercase hex = fnv64("id\0machine\0host\0port\0user\0remote\0local\0driver\0ro").
    pub fn fingerprint(&self) -> String {
        String::new() // STUB (S0.5)
    }
    /// "[user@]<host.for_colon()>:<remote.sftp_path()>"
    pub fn source(&self) -> String {
        String::new() // STUB (S0.5)
    }
}

/// "bifrost:<id>@<fp16>"
pub fn marker(_id: &MountId, _fingerprint: &str) -> String {
    String::new() // STUB (S0.5)
}

/// exact grammar; "agent-01" never matches "agent-01-x"
pub fn parse_marker(_source: &str) -> Option<(MountId, String)> {
    None // STUB (S0.5)
}

/// called once, with an exit description
pub type OnExit = Box<dyn FnOnce(String) + Send + 'static>;

pub struct MountRequest {
    pub spec: MountSpec,
    pub log_path: PathBuf,
    pub on_exit: OnExit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountHandle {
    pub id: MountId,
    pub driver: String,
    pub local_path: PathBuf,
    pub fingerprint: String,
    /// informational, never signalled
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountState {
    Missing,
    Healthy,
    Degraded(String),
    Stale(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriverAvailability {
    Available { binary: PathBuf, detail: String },
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    /// binary missing, backend stopped, not implemented
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// transport / protocol / timeout / whole response unusable
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountError {
    #[error("driver unavailable: {0}")]
    Unavailable(String),
    /// the one busy string everywhere (C4)
    #[error("unmount blocked: busy (files open)")]
    Busy,
    /// occupied path, symlink, not a dir, not empty, invalid request
    #[error("refused: {0}")]
    Refused(String),
    /// preflight / driver log tail (tail(…, 512)), timeout
    #[error("{0}")]
    Failed(String),
}
