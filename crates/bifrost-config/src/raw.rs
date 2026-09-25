//! serde mirror of the TOML (contract §3): every struct `#[derive(Deserialize)] #[serde(deny_unknown_fields)]`,
//! fields Option/default; `RawDiscovery` is ONE flat struct so unknown-field errors keep toml line/col.
//! Written by S1-B.

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RawConfig {
    pub version: Option<u32>,
    pub mount: RawMount,
    pub daemon: RawDaemon,
    pub reconciliation: RawRecon,
    pub policy: RawPolicy,
    pub discovery: Vec<RawDiscovery>,
    pub machines: Vec<RawMachine>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RawMount {
    pub root: Option<String>,
    pub default_driver: Option<String>,
    pub ssh_config: Option<String>,
    pub vfs_cache_mode: Option<String>,
    pub auto_order: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RawDaemon {
    pub discovery_interval: Option<String>,
    pub health_interval: Option<String>,
    pub reconcile_interval: Option<String>,
    pub mount_timeout: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RawRecon {
    pub offline_grace_period: Option<String>,
    pub retry_initial: Option<String>,
    pub retry_max: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RawPolicy {
    pub allow: RawMatch,
    pub deny: RawMatch,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RawMatch {
    pub ids: Vec<String>,
    pub names: Vec<String>,
    pub cidrs: Vec<String>,
    pub tags: Vec<String>,
    pub providers: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

/// Flat on purpose (not a tagged enum): per-type key checks happen in validation.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawDiscovery {
    pub r#type: String,
    pub name: Option<String>,
    pub interval: Option<String>,
    pub domain: Option<String>,
    #[serde(default)]
    pub nameservers: Vec<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub filter: RawFilter,
    #[serde(default)]
    pub mount: RawTemplate,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RawFilter {
    pub include_ids: Vec<String>,
    pub include_names: Vec<String>,
    pub include_cidrs: Vec<String>,
    pub include_tags: Vec<String>,
    pub include_metadata: BTreeMap<String, String>,
    pub exclude_ids: Vec<String>,
    pub exclude_names: Vec<String>,
    pub exclude_cidrs: Vec<String>,
    pub exclude_tags: Vec<String>,
    pub exclude_metadata: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields, default)]
pub(crate) struct RawTemplate {
    pub user: Option<String>,
    pub remote: Option<String>,
    pub driver: Option<String>,
    pub read_only: Option<bool>,
    pub honor_hints: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMachine {
    pub name: String,
    pub host: String,
    pub port: Option<i64>,
    pub user: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
    pub remote: Option<String>,
    pub driver: Option<String>,
    pub read_only: Option<bool>,
    #[serde(default)]
    pub mounts: Vec<RawMount1>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawMount1 {
    pub remote: String,
    pub local: Option<String>,
    pub driver: Option<String>,
    pub read_only: Option<bool>,
}
