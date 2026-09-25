//! Wire DTOs shared by the daemon, CLI and TUI.

use crate::events::EventRecord;
use crate::reconcile::Availability;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// GET /v1/status = full snapshot (the TUI polls this)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusDto {
    pub version: String,
    pub pid: u32,
    pub uptime_secs: u64,
    pub socket: String,
    pub config_path: String,
    /// non-empty ⇒ running on the previous config
    pub config_errors: Vec<String>,
    pub mount_root: String,
    pub ready: bool,
    pub ssh_agent: bool,
    pub providers: Vec<ProviderDto>,
    pub drivers: Vec<DriverDto>,
    /// select_driver(&Auto, active auto_order, probes).ok(); what status, doctor and the TUI print (E5)
    pub auto_driver: Option<String>,
    pub machines: Vec<MachineDto>,
    pub mounts: Vec<MountDto>,
    pub conflicts: Vec<String>,
    /// last 200
    pub events: Vec<EventRecord>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderDto {
    pub name: String,
    /// B8
    pub kind: String,
    pub machines: usize,
    pub refreshes: u64,
    pub last_ok_secs_ago: Option<u64>,
    pub last_error: Option<String>,
}

/// no auto_rank (E5)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DriverDto {
    pub name: String,
    pub available: bool,
    pub binary: Option<String>,
    pub detail: String,
}

/// no `eligible`: the verdict says it (E5)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MachineDto {
    pub id: String,
    pub name: String,
    pub source: String,
    pub shadowed: Vec<String>,
    pub address: String,
    pub port: Option<u16>,
    pub online: Option<bool>,
    pub tags: Vec<String>,
    pub metadata: BTreeMap<String, String>,
    pub verdict: String,
    pub state: Availability,
    pub mounts: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MountDto {
    pub id: String,
    pub machine: String,
    pub driver: Option<String>,
    pub local_path: String,
    /// MountSpec::source()
    pub remote: String,
    pub state: Availability,
    pub detail: String,
    pub desired: bool,
    pub held: bool,
    pub adopted: bool,
    pub pid: Option<u32>,
    pub failures: u32,
    pub retry_in_secs: Option<u64>,
    pub last_error: Option<String>,
    pub action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionDto {
    pub mount: String,
    pub action: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LogDto {
    pub mount: String,
    pub path: String,
    pub lines: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReloadDto {
    pub ok: bool,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UnmountReq {
    #[serde(default)]
    pub force: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ErrorDto {
    pub error: String,
}
