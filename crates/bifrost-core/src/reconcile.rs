//! The pure reconciler (contract §5). STUB (S0.3): S1-A implements the bodies.

use crate::events::Event;
use crate::model::{
    DriverAvailability, DriverSelector, MountError, MountHandle, MountSpec, MountState,
};
use crate::registry::Machine;
use crate::validate::{MachineId, MountId, RemotePath, User};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct MountTemplate {
    pub user: Option<User>,
    pub remote: RemotePath,
    pub driver: DriverSelector,
    pub read_only: bool,
    pub honor_hints: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StaticMount {
    pub local: MountId,
    pub remote: RemotePath,
    pub driver: DriverSelector,
    pub read_only: bool,
}

pub struct DesiredInput<'a> {
    /// canonical
    pub root: &'a Path,
    pub machines: &'a [Machine],
    pub static_mounts: &'a BTreeMap<MachineId, Vec<StaticMount>>,
    /// keyed by provider name
    pub templates: &'a BTreeMap<String, MountTemplate>,
    pub held: &'a BTreeSet<MountId>,
    pub auto_order: &'a [String],
    pub probes: &'a BTreeMap<String, DriverAvailability>,
}

/// fingerprint = spec.fingerprint() (E5)
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub spec: MountSpec,
    pub driver: Result<String, String>,
    pub held: bool,
    pub online: Option<bool>,
}

pub struct Desired {
    pub candidates: BTreeMap<MountId, Candidate>,
    pub conflicts: Vec<String>,
}

/// Candidates only for verdict = Allowed. `held` is carried; desired = !held.
pub fn desired(_i: &DesiredInput) -> Desired {
    Desired {
        candidates: BTreeMap::new(),
        conflicts: vec![],
    } // STUB (S1-A)
}

/// Named: Ok(n) iff n is Available, else Err("<n> unavailable: <reason>") — never substitutes.
/// Auto: first Available entry in auto_order, else Err("no available driver (tried …)").
pub fn select_driver(
    _sel: &DriverSelector,
    _auto_order: &[String],
    _probes: &BTreeMap<String, DriverAvailability>,
) -> Result<String, String> {
    Err("not implemented".into()) // STUB (S1-A)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Absent,
    Mounting,
    Mounted,
    Unmounting,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Health {
    #[default]
    Unknown,
    Healthy,
    Degraded(String),
    Stale(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Reason {
    NotDesired,
    Manual,
    Stale,
    SpecChanged,
    OfflineGrace,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    pub grace: Duration,
    pub retry_initial: Duration,
    pub retry_max: Duration,
}

#[derive(Clone, Debug, Default)]
pub struct MountRuntime {
    pub phase: Phase,
    pub handle: Option<MountHandle>,
    pub health: Health,
    pub degraded_since: Option<Instant>,
    pub generation: u64,
    pub failures: u32,
    /// set by mount_done(Ok); cleared when the mount goes Absent (A15)
    pub mounted_at: Option<Instant>,
    pub mount_retry_at: Option<Instant>,
    pub unmount_retry_at: Option<Instant>,
    pub last_error: Option<String>,
    /// set by an OfflineGrace unmount; cleared by a successful mount
    pub offline: bool,
    /// API `unmount --force`
    pub force_requested: bool,
    pub adopted: bool,
    pub probing: bool,
}

// Every transition ignores a stale generation and returns an Event only on a state change (§5).
// `gen` is a reserved keyword in edition 2024, hence `generation` (A1); `task_gen` is fine.
impl MountRuntime {
    /// Mounted, health Unknown, adopted=true, mounted_at None
    pub fn adopted(h: MountHandle) -> Self {
        Self {
            phase: Phase::Mounted,
            handle: Some(h),
            adopted: true,
            ..Self::default()
        }
    }
    /// generation += 1 (BOTH ops); phase = Mounting|Unmounting
    pub fn begin(&mut self, _phase: Phase) -> u64 {
        self.generation // STUB (S1-A)
    }
    pub fn mount_done(
        &mut self,
        _generation: u64,
        _r: Result<MountHandle, MountError>,
        _now: Instant,
        _t: &Timing,
        _rand: u64,
    ) -> Option<Event> {
        None // STUB (S1-A)
    }
    pub fn unmount_done(
        &mut self,
        _generation: u64,
        _why: Reason,
        _r: Result<(), MountError>,
        _now: Instant,
        _t: &Timing,
        _rand: u64,
    ) -> Option<Event> {
        None // STUB (S1-A)
    }
    pub fn health(
        &mut self,
        _generation: u64,
        _s: MountState,
        _now: Instant,
        _t: &Timing,
        _rand: u64,
    ) -> Option<Event> {
        None // STUB (S1-A)
    }
}

/// remaining time, computed in decide (C2)
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WaitReason {
    InFlight,
    WarmingUp,
    NoDriver(String),
    MachineOffline,
    Backoff(Duration),
}

/// PRD §10 action set. Only Mount / Unmount / Remount have side effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    NoOp,
    Mount {
        driver: String,
    },
    Unmount {
        force: bool,
        why: Reason,
    },
    /// executed as an unmount; the Mount follows via row 9 on a later pass
    Remount {
        force: bool,
        why: Reason,
    },
    Degraded(String),
    Waiting(WaitReason),
}

impl Action {
    pub fn has_side_effect(&self) -> bool {
        false // STUB (S1-A)
    }
    pub fn kind(&self) -> &'static str {
        "noop" // STUB (S1-A)
    }
}

/// "noop" | "mount (sshfs)" | "waiting (backoff 3s)" | …
impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}") // STUB (S1-A)
    }
}

pub struct PlanInput<'a> {
    pub now: Instant,
    pub ready: bool,
    pub grace: Duration,
    pub candidates: &'a BTreeMap<MountId, Candidate>,
    pub runtimes: &'a BTreeMap<MountId, MountRuntime>,
}

/// §5 table
pub fn decide(
    _c: Option<&Candidate>,
    _rt: &MountRuntime,
    _now: Instant,
    _ready: bool,
    _grace: Duration,
) -> Action {
    Action::NoOp // STUB (S1-A)
}

/// ids = candidates ∪ runtimes, sorted; pure
pub fn plan(_i: &PlanInput) -> Vec<(MountId, Action)> {
    vec![] // STUB (S1-A)
}

pub fn next_wakeup(
    _runtimes: &BTreeMap<MountId, MountRuntime>,
    _grace: Duration,
) -> Option<Instant> {
    None // STUB (S1-A)
}

/// PRD §12 states. Ord == display severity (used when aggregating a machine's mounts).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    Unknown,
    Discovered,
    Eligible,
    Mounted,
    Connecting,
    Unmounting,
    Offline,
    Degraded,
    Failed,
}

/// desired + driver Err ⇒ Failed (B15)
pub fn mount_availability(_rt: Option<&MountRuntime>, _c: Option<&Candidate>) -> Availability {
    Availability::Unknown // STUB (S1-A)
}

/// Allowed, no mounts ⇒ Eligible (B15)
pub fn machine_availability(_m: &Machine, _mounts: &[Availability]) -> Availability {
    Availability::Unknown // STUB (S1-A)
}
