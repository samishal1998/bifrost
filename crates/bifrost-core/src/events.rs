//! PRD §20 events + MountDegraded.
//! MachineEligible and DriverUnavailable are emitted by the actor on transitions (B1): it keeps the previous
//! verdicts and probe results and emits on a change to Allowed, or on Available → Unavailable.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    MachineDiscovered {
        machine: String,
        provider: String,
    },
    MachineLost {
        machine: String,
    },
    MachineEligible {
        machine: String,
        via: String,
    },
    MountRequested {
        mount: String,
        driver: String,
    },
    MountStarted {
        mount: String,
        driver: String,
        pid: Option<u32>,
    },
    MountHealthy {
        mount: String,
    },
    MountDegraded {
        mount: String,
        reason: String,
    },
    MountFailed {
        mount: String,
        error: String,
        attempt: u32,
        retry_in_ms: Option<u64>,
    },
    UnmountStarted {
        mount: String,
        reason: String,
    },
    UnmountComplete {
        mount: String,
    },
    DriverUnavailable {
        driver: String,
        reason: String,
    },
    ConfigurationReloaded {
        ok: bool,
        errors: Vec<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventRecord {
    pub seq: u64,
    pub ts_unix_ms: u64,
    pub event: Event,
}
