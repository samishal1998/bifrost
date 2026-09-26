//! The pure reconciler (contract §5).

use crate::events::Event;
use crate::model::{
    DriverAvailability, DriverSelector, MountError, MountHandle, MountSpec, MountState,
};
use crate::registry::Machine;
use crate::validate::{MachineId, MountId, RemotePath, User, backoff};
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
/// Static locals are claimed first (whatever their machine's verdict): a discovered id equal to one is skipped and
/// reported in `conflicts`. Address data comes only from the selected observation (§4 winner takes all).
pub fn desired(i: &DesiredInput) -> Desired {
    let claimed: BTreeMap<&MountId, &MachineId> = (i.static_mounts.iter())
        .flat_map(|(m, v)| v.iter().map(move |s| (&s.local, m)))
        .collect();
    let (mut candidates, mut conflicts) = (BTreeMap::new(), vec![]);
    let mut add = |spec: MountSpec, online: Option<bool>| {
        let c = Candidate {
            driver: select_driver(&spec.driver, i.auto_order, i.probes),
            held: i.held.contains(&spec.id),
            online,
            spec,
        };
        candidates.insert(c.spec.id.clone(), c);
    };
    for m in i.machines.iter().filter(|m| m.verdict.is_allowed()) {
        let s = m.obs();
        let Some(host) = s.addresses.first() else {
            continue; // providers guarantee ≥1 address; never index blindly
        };
        let spec = |id: &MountId, user, remote, driver, read_only| MountSpec {
            id: id.clone(),
            machine: m.id.clone(),
            host: host.clone(),
            port: s.port,
            user,
            remote,
            local_path: i.root.join(id.as_str()),
            driver,
            read_only,
        };
        if m.source().trust == 0 {
            for sm in i.static_mounts.get(&m.id).into_iter().flatten() {
                let (r, d) = (sm.remote.clone(), sm.driver.clone());
                add(
                    spec(&sm.local, s.hints.user.clone(), r, d, sm.read_only),
                    s.online,
                );
            }
        } else if let Some(owner) = claimed.get(&m.id) {
            conflicts.push(format!(
                "{}: local name taken by static machine {}",
                m.id.as_str(),
                owner.as_str()
            ));
        } else if let Some(t) = i.templates.get(&m.source().provider) {
            let (user, remote) = if t.honor_hints {
                let user = s.hints.user.clone().or_else(|| t.user.clone());
                (
                    user,
                    s.hints.path.clone().unwrap_or_else(|| t.remote.clone()),
                )
            } else {
                (t.user.clone(), t.remote.clone())
            };
            add(
                spec(&m.id, user, remote, t.driver.clone(), t.read_only),
                s.online,
            );
        }
    }
    Desired {
        candidates,
        conflicts,
    }
}

/// Named: Ok(n) iff n is Available, else Err("<n> unavailable: <reason>") — never substitutes.
/// Auto: first Available entry in auto_order, else Err("no available driver (tried …)").
pub fn select_driver(
    sel: &DriverSelector,
    auto_order: &[String],
    probes: &BTreeMap<String, DriverAvailability>,
) -> Result<String, String> {
    let available = |n: &str| matches!(probes.get(n), Some(DriverAvailability::Available { .. }));
    match sel {
        DriverSelector::Named(n) => match probes.get(n) {
            Some(DriverAvailability::Available { .. }) => Ok(n.clone()),
            Some(DriverAvailability::Unavailable(why)) => Err(format!("{n} unavailable: {why}")),
            None => Err(format!("{n} unavailable: not probed")),
        },
        DriverSelector::Auto => (auto_order.iter().find(|n| available(n)).cloned())
            .ok_or_else(|| format!("no available driver (tried {})", auto_order.join(", "))),
    }
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
// A transition also applies only in the phase its generation was issued for (a result for Mounting is dropped
// once the runtime is Absent), so a duplicate result can't count a failure twice.
// Events name the mount from `handle`; MountRuntime carries no id, so a MountFailed from mount_done(Err) has an
// empty `mount` and the actor fills in the id it keys the runtime by.
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
    pub fn begin(&mut self, phase: Phase) -> u64 {
        self.generation += 1;
        self.phase = phase;
        self.generation
    }
    pub fn mount_done(
        &mut self,
        generation: u64,
        r: Result<MountHandle, MountError>,
        now: Instant,
        t: &Timing,
        rand: u64,
    ) -> Option<Event> {
        if generation != self.generation || self.phase != Phase::Mounting {
            return None;
        }
        match r {
            Ok(h) => {
                let ev = Event::MountStarted {
                    mount: h.id.as_str().into(),
                    driver: h.driver.clone(),
                    pid: h.pid,
                };
                // every other per-mount field was reset when the runtime went Absent; force_requested
                // (an `unmount --force` issued mid-mount) survives so row 3 still forces
                self.phase = Phase::Mounted;
                self.handle = Some(h);
                self.health = Health::Unknown;
                self.mounted_at = Some(now);
                self.offline = false;
                self.last_error = None;
                self.mount_retry_at = None; // failures are NOT reset here (A15)
                Some(ev)
            }
            Err(e) => {
                self.absent();
                let d = self.fail(t, rand);
                self.mount_retry_at = Some(now + d);
                self.last_error = Some(e.to_string());
                Some(self.failed(String::new(), e.to_string(), d))
            }
        }
    }
    pub fn unmount_done(
        &mut self,
        generation: u64,
        why: Reason,
        r: Result<(), MountError>,
        now: Instant,
        t: &Timing,
        rand: u64,
    ) -> Option<Event> {
        if generation != self.generation || self.phase != Phase::Unmounting {
            return None;
        }
        let mount = self.id();
        match r {
            Ok(()) => {
                self.absent();
                if why == Reason::OfflineGrace {
                    self.offline = true;
                    let d = self.fail(t, rand);
                    self.mount_retry_at = Some(now + d);
                }
                Some(Event::UnmountComplete { mount })
            }
            Err(e) => {
                self.phase = Phase::Mounted;
                let d = self.fail(t, rand);
                self.unmount_retry_at = Some(now + d);
                let reason = match e {
                    MountError::Busy => e.to_string(), // the one busy string (C4)
                    e => format!("unmount failed: {e}"),
                };
                self.last_error = Some(reason.clone());
                Some(Event::MountDegraded { mount, reason })
            }
        }
    }
    pub fn health(
        &mut self,
        generation: u64,
        s: MountState,
        now: Instant,
        t: &Timing,
        rand: u64,
    ) -> Option<Event> {
        if generation != self.generation || self.phase != Phase::Mounted {
            return None;
        }
        // a passed unmount retry gates nothing in decide; dropping it here keeps an unmount that is no longer
        // wanted (a busy remount whose spec reverted) from pinning last_error and next_wakeup in the past
        self.unmount_retry_at = self.unmount_retry_at.filter(|t| *t > now);
        let mount = self.id();
        match s {
            MountState::Healthy => {
                // A15: only a mount that stayed up for retry_max earns a clean slate, and not while a busy unmount
                // is still being retried (else every tick resets its backoff to bo(1))
                if self.unmount_retry_at.is_none()
                    && (self.mounted_at)
                        .is_some_and(|m| now.saturating_duration_since(m) >= t.retry_max)
                {
                    self.failures = 0;
                }
                self.degraded_since = None;
                // keep an unmount error while its retry is still ahead, so a busy remount keeps saying why (C4)
                if self.unmount_retry_at.is_none() {
                    self.last_error = None;
                }
                let changed = self.health != Health::Healthy;
                self.health = Health::Healthy;
                changed.then_some(Event::MountHealthy { mount })
            }
            MountState::Degraded(reason) => {
                self.degraded_since.get_or_insert(now);
                let entered = !matches!(self.health, Health::Degraded(_));
                self.health = Health::Degraded(reason.clone());
                entered.then_some(Event::MountDegraded { mount, reason })
            }
            MountState::Stale(r) => {
                let entered = !matches!(self.health, Health::Stale(_));
                self.health = Health::Stale(r.clone());
                if !entered {
                    return None; // the same dead mount, not a new failure
                }
                let d = self.fail(t, rand);
                self.mount_retry_at = Some(now + d);
                let reason = format!("stale: {r}");
                Some(Event::MountDegraded { mount, reason })
            }
            MountState::Missing => {
                self.absent();
                let d = self.fail(t, rand);
                self.mount_retry_at = Some(now + d);
                self.last_error = Some("mount disappeared".into());
                Some(self.failed(mount, "mount disappeared".into(), d))
            }
        }
    }

    /// Back to Absent: drops everything that belonged to the mount instance that is gone. Every transition
    /// into Absent goes through here, so a later mount_done(Ok) starts clean.
    fn absent(&mut self) {
        self.phase = Phase::Absent;
        self.handle = None;
        self.health = Health::Unknown;
        self.mounted_at = None;
        self.degraded_since = None;
        self.unmount_retry_at = None;
        self.last_error = None; // a stale unmount error; callers that fail set theirs after this
        self.force_requested = false;
        self.adopted = false;
    }
    /// One more failure; returns the backoff before the next attempt.
    fn fail(&mut self, t: &Timing, rand: u64) -> Duration {
        self.failures = self.failures.saturating_add(1);
        backoff(self.failures, t.retry_initial, t.retry_max, rand)
    }
    fn failed(&self, mount: String, error: String, retry: Duration) -> Event {
        Event::MountFailed {
            mount,
            error,
            attempt: self.failures,
            retry_in_ms: Some(retry.as_millis() as u64),
        }
    }
    fn id(&self) -> String {
        (self.handle.as_ref()).map_or(String::new(), |h| h.id.as_str().into())
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
        matches!(
            self,
            Self::Mount { .. } | Self::Unmount { .. } | Self::Remount { .. }
        )
    }
    pub fn kind(&self) -> &'static str {
        match self {
            Self::NoOp => "noop",
            Self::Mount { .. } => "mount",
            Self::Unmount { .. } => "unmount",
            Self::Remount { .. } => "remount",
            Self::Degraded(_) => "degraded",
            Self::Waiting(_) => "waiting",
        }
    }
}

/// "noop" | "mount (sshfs)" | "waiting (backoff 3s)" | "unmount (manual, force)" | "remount (spec changed)" | …
impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let k = self.kind();
        match self {
            Self::NoOp => f.write_str(k),
            Self::Mount { driver } => write!(f, "{k} ({driver})"),
            Self::Unmount { force, why } | Self::Remount { force, why } => {
                let why = match why {
                    Reason::NotDesired => "not desired",
                    Reason::Manual => "manual",
                    Reason::Stale => "stale",
                    Reason::SpecChanged => "spec changed",
                    Reason::OfflineGrace => "offline grace",
                };
                write!(f, "{k} ({why}{})", if *force { ", force" } else { "" })
            }
            Self::Degraded(r) => write!(f, "{k} ({r})"),
            Self::Waiting(w) => match w {
                WaitReason::InFlight => write!(f, "{k} (in flight)"),
                WaitReason::WarmingUp => write!(f, "{k} (warming up)"),
                WaitReason::NoDriver(e) => write!(f, "{k} (no driver: {e})"),
                WaitReason::MachineOffline => write!(f, "{k} (machine offline)"),
                // whole seconds, rounded up: "0s" would read as "now"
                WaitReason::Backoff(d) => {
                    write!(f, "{k} (backoff {}s)", d.as_millis().div_ceil(1000))
                }
            },
        }
    }
}

pub struct PlanInput<'a> {
    pub now: Instant,
    pub ready: bool,
    pub grace: Duration,
    pub candidates: &'a BTreeMap<MountId, Candidate>,
    pub runtimes: &'a BTreeMap<MountId, MountRuntime>,
}

/// Time left until `deadline`, when it is still in the future.
fn remaining(deadline: Option<Instant>, now: Instant) -> Option<Duration> {
    deadline
        .and_then(|t| t.checked_duration_since(now))
        .filter(|d| !d.is_zero())
}

/// §5 table; first match wins. Row numbers refer to the contract.
pub fn decide(
    c: Option<&Candidate>,
    rt: &MountRuntime,
    now: Instant,
    ready: bool,
    grace: Duration,
) -> Action {
    use Action::*;
    let held = c.is_some_and(|c| c.held);
    let d = c.filter(|c| !c.held);
    let unmount_wait = remaining(rt.unmount_retry_at, now);
    // ⊳U: rows 3/5 wait out the unmount backoff, rows 10–12 show the error that caused it
    let gate_wait = |a: Action| unmount_wait.map_or(a, |r| Waiting(WaitReason::Backoff(r)));
    let gate_show = |a: Action| match unmount_wait {
        Some(_) => Degraded(rt.last_error.clone().unwrap_or_default()),
        None => a,
    };
    match (rt.phase, d) {
        (Phase::Mounting | Phase::Unmounting, _) => Waiting(WaitReason::InFlight), // 1
        (Phase::Absent, None) => NoOp,                                             // 2
        (Phase::Mounted, None) => {
            let why = if held {
                Reason::Manual
            } else {
                Reason::NotDesired
            }; // A14
            if matches!(rt.health, Health::Stale(_)) || rt.force_requested {
                gate_wait(Unmount { force: true, why }) // 3
            } else if !ready && !held {
                Waiting(WaitReason::WarmingUp) // 4
            } else {
                gate_wait(Unmount { force: false, why }) // 5: busy → backoff, never auto-forced
            }
        }
        (Phase::Absent, Some(c)) => match &c.driver {
            Err(e) => Waiting(WaitReason::NoDriver(e.clone())), // 6
            Ok(_) if c.online == Some(false) => Waiting(WaitReason::MachineOffline), // 7
            Ok(driver) => match remaining(rt.mount_retry_at, now) {
                Some(r) => Waiting(WaitReason::Backoff(r)), // 8
                None => Mount {
                    driver: driver.clone(),
                }, // 9
            },
        },
        (Phase::Mounted, Some(c)) => {
            let changed =
                (rt.handle.as_ref()).is_some_and(|h| h.fingerprint != c.spec.fingerprint());
            // A16: never drop a working mount for a spec that can't mount now; r3-5: nor for a partial pre-ready view
            let can_remount = c.driver.is_ok() && c.online != Some(false) && ready;
            let past_grace =
                (rt.degraded_since).is_some_and(|t| now.saturating_duration_since(t) >= grace);
            match &rt.health {
                Health::Stale(_) => gate_show(Remount {
                    force: true,
                    why: Reason::Stale,
                }), // 10
                h if changed && can_remount => gate_show(Remount {
                    force: matches!(h, Health::Degraded(_)),
                    why: Reason::SpecChanged,
                }), // 11
                Health::Degraded(_) if past_grace => gate_show(Unmount {
                    force: true,
                    why: Reason::OfflineGrace,
                }), // 12
                Health::Degraded(r) => Degraded(r.clone()), // 13: sshfs reconnect is handling it
                _ if changed => {
                    let why = match &c.driver {
                        Err(e) => e.as_str(),
                        Ok(_) if c.online == Some(false) => "machine offline",
                        Ok(_) => "warming up",
                    };
                    Degraded(format!("change pending: {why}")) // 13 (A16)
                }
                _ => NoOp, // 14: Healthy, Unknown, or offline-but-Healthy
            }
        }
    }
}

/// ids = candidates ∪ runtimes, sorted; pure
pub fn plan(i: &PlanInput) -> Vec<(MountId, Action)> {
    let none = MountRuntime::default();
    let ids: BTreeSet<&MountId> = i.candidates.keys().chain(i.runtimes.keys()).collect();
    (ids.into_iter())
        .map(|id| {
            let rt = i.runtimes.get(id).unwrap_or(&none);
            (
                id.clone(),
                decide(i.candidates.get(id), rt, i.now, i.ready, i.grace),
            )
        })
        .collect()
}

/// Earliest deadline a pass may act on: `mount_retry_at` while Absent (row 8); while Mounted, `unmount_retry_at`
/// (⊳U gates every Mounted row that acts, row 12 included), else `degraded_since + grace` when Degraded (row 12).
/// Only deadlines strictly after `now` count: one that passed without its row acting (Absent but offline or without a
/// driver; Degraded past grace while not desired in warm-up; a no-longer-wanted unmount retry) must not make the actor
/// spin.
pub fn next_wakeup(
    runtimes: &BTreeMap<MountId, MountRuntime>,
    grace: Duration,
    now: Instant,
) -> Option<Instant> {
    (runtimes.values())
        .flat_map(|rt| match rt.phase {
            Phase::Absent => rt.mount_retry_at,
            Phase::Mounted => rt.unmount_retry_at.or_else(|| {
                (rt.degraded_since)
                    .filter(|_| matches!(rt.health, Health::Degraded(_)))
                    .and_then(|t| t.checked_add(grace))
            }),
            Phase::Mounting | Phase::Unmounting => None,
        })
        .filter(|t| *t > now)
        .min()
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
pub fn mount_availability(rt: Option<&MountRuntime>, c: Option<&Candidate>) -> Availability {
    use Availability as A;
    let none = MountRuntime::default();
    let rt = rt.unwrap_or(&none);
    match rt.phase {
        Phase::Mounting => A::Connecting,
        Phase::Unmounting => A::Unmounting,
        Phase::Mounted => match rt.health {
            Health::Degraded(_) | Health::Stale(_) => A::Degraded,
            // an adopted mount whose machine isn't in the registry (or allowed) yet
            _ if rt.adopted && c.is_none() => A::Unknown,
            _ => A::Mounted,
        },
        Phase::Absent => match c.filter(|c| !c.held) {
            None => A::Eligible, // held (the DTO says held: true), or not a candidate
            Some(c) if rt.offline || c.online == Some(false) => A::Offline,
            Some(c) if c.driver.is_err() || rt.last_error.is_some() => A::Failed,
            Some(_) => A::Eligible,
        },
    }
}

/// Allowed, no mounts ⇒ Eligible (B15)
pub fn machine_availability(m: &Machine, mounts: &[Availability]) -> Availability {
    if !m.verdict.is_allowed() {
        return Availability::Discovered;
    }
    // the enum's order is the severity order
    mounts
        .iter()
        .copied()
        .max()
        .unwrap_or(Availability::Eligible)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeDiscovery, FakeDriver, block_on, obs};
    use crate::model::MountRequest;
    use crate::policy::{Match, Policy, ProviderFilter, Verdict};
    use crate::registry::{MachineRegistry, Observed, Source};
    use crate::validate::{Glob, Host, Name, backoff};
    use crate::{DiscoveryProvider, MountDriver};
    use std::path::PathBuf;
    use std::sync::Mutex;

    const T: Timing = Timing {
        grace: Duration::from_secs(300),
        retry_initial: Duration::from_secs(2),
        retry_max: Duration::from_secs(60),
    };
    const GRACE: Duration = T.grace;
    const BUSY: &str = "unmount blocked: busy (files open)";

    fn id(s: &str) -> MountId {
        Name::parse(s).unwrap()
    }
    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }
    fn spec(i: &str) -> MountSpec {
        MountSpec {
            id: id(i),
            machine: id(i),
            host: Host::parse("10.0.0.1").unwrap(),
            port: None,
            user: None,
            remote: RemotePath::parse("~").unwrap(),
            local_path: PathBuf::from("/r").join(i),
            driver: DriverSelector::Auto,
            read_only: false,
        }
    }
    fn cand(i: &str) -> Candidate {
        Candidate {
            spec: spec(i),
            driver: Ok("sshfs".into()),
            held: false,
            online: None,
        }
    }
    fn held(i: &str) -> Candidate {
        Candidate {
            held: true,
            ..cand(i)
        }
    }
    /// the candidate with a changed spec (a DNS IP move): same id, different fingerprint
    fn moved(i: &str) -> Candidate {
        let mut c = cand(i);
        c.spec.host = Host::parse("10.0.0.2").unwrap();
        c
    }
    fn handle(c: &Candidate) -> MountHandle {
        MountHandle {
            id: c.spec.id.clone(),
            driver: "sshfs".into(),
            local_path: c.spec.local_path.clone(),
            fingerprint: c.spec.fingerprint(),
            pid: Some(42),
        }
    }
    fn mounted(c: &Candidate, h: Health) -> MountRuntime {
        MountRuntime {
            phase: Phase::Mounted,
            handle: Some(handle(c)),
            health: h,
            generation: 1,
            ..Default::default()
        }
    }
    fn degraded(c: &Candidate, since: Instant) -> MountRuntime {
        MountRuntime {
            degraded_since: Some(since),
            ..mounted(c, Health::Degraded("timeout".into()))
        }
    }
    fn stale() -> Health {
        Health::Stale("ENOTCONN".into())
    }
    fn dec(c: Option<&Candidate>, rt: &MountRuntime, now: Instant) -> Action {
        decide(c, rt, now, true, GRACE)
    }
    fn unmount(force: bool, why: Reason) -> Action {
        Action::Unmount { force, why }
    }
    fn remount(force: bool, why: Reason) -> Action {
        Action::Remount { force, why }
    }
    fn wait(w: WaitReason) -> Action {
        Action::Waiting(w)
    }
    fn bo(failures: u32) -> Duration {
        backoff(failures, T.retry_initial, T.retry_max, 0)
    }
    fn req(spec: &MountSpec) -> MountRequest {
        MountRequest {
            spec: spec.clone(),
            log_path: PathBuf::new(),
            on_exit: Box::new(|_| {}),
        }
    }
    fn avail() -> DriverAvailability {
        DriverAvailability::Available {
            binary: PathBuf::from("/usr/bin/x"),
            detail: String::new(),
        }
    }
    fn unavail(why: &str) -> DriverAvailability {
        DriverAvailability::Unavailable(why.into())
    }
    fn include_all(provider: &str) -> Policy {
        Policy {
            filters: BTreeMap::from([(
                provider.to_string(),
                ProviderFilter {
                    include: Match {
                        names: vec![Glob::parse("*").unwrap()],
                        ..Default::default()
                    },
                    ..Default::default()
                },
            )]),
            ..Default::default()
        }
    }
    fn tmpl(honor_hints: bool) -> MountTemplate {
        MountTemplate {
            user: Some(User::parse("sami").unwrap()),
            remote: RemotePath::parse("~/proj").unwrap(),
            driver: DriverSelector::Auto,
            read_only: false,
            honor_hints,
        }
    }

    /// Owned inputs for `desired`.
    #[derive(Default)]
    struct World {
        machines: Vec<Machine>,
        statics: BTreeMap<MachineId, Vec<StaticMount>>,
        templates: BTreeMap<String, MountTemplate>,
        held: BTreeSet<MountId>,
        auto_order: Vec<String>,
        probes: BTreeMap<String, DriverAvailability>,
    }
    impl World {
        fn desired(&self) -> Desired {
            desired(&DesiredInput {
                root: Path::new("/m"),
                machines: &self.machines,
                static_mounts: &self.statics,
                templates: &self.templates,
                held: &self.held,
                auto_order: &self.auto_order,
                probes: &self.probes,
            })
        }
    }

    /// Runs a plan's side effects through the fake the way the actor does: begin, call, feed the result back.
    fn execute(
        drv: &FakeDriver,
        plan: &[(MountId, Action)],
        cands: &BTreeMap<MountId, Candidate>,
        rts: &mut BTreeMap<MountId, MountRuntime>,
        now: Instant,
    ) {
        for (i, a) in plan {
            let rt = rts.entry(i.clone()).or_default();
            match a {
                Action::Mount { .. } => {
                    let g = rt.begin(Phase::Mounting);
                    let r = block_on(drv.mount(req(&cands[i].spec)));
                    rt.mount_done(g, r, now, &T, 0);
                    let h = rt.handle.clone().unwrap();
                    rt.health(g, block_on(drv.inspect(&h)), now, &T, 0);
                }
                Action::Unmount { force, why } | Action::Remount { force, why } => {
                    let h = rt.handle.clone().unwrap();
                    let g = rt.begin(Phase::Unmounting);
                    let r = block_on(drv.unmount(&h, *force));
                    rt.unmount_done(g, *why, r, now, &T, 0);
                }
                _ => {}
            }
        }
    }

    /// FakeDiscovery → registry → policy → desired, with a FakeDriver named "fake".
    fn fake_world(drv: &FakeDriver, now: Instant) -> Desired {
        let disc = FakeDiscovery {
            name: "fake".into(),
            result: Mutex::new(Ok(vec![obs("a", "10.0.0.1"), obs("b", "10.0.0.2")])),
        };
        let src = Source {
            trust: 2,
            kind: "fake".into(),
            provider: disc.name().into(),
        };
        let mut reg = MachineRegistry::default();
        reg.apply_ok(&src, block_on(disc.discover()).unwrap(), now, secs(90));
        let w = World {
            machines: reg.machines(&include_all("fake")),
            templates: BTreeMap::from([("fake".to_string(), tmpl(false))]),
            auto_order: vec!["fake".into()],
            probes: BTreeMap::from([("fake".to_string(), block_on(drv.probe()))]),
            ..Default::default()
        };
        w.desired()
    }

    fn run(
        now: Instant,
        c: &BTreeMap<MountId, Candidate>,
        r: &BTreeMap<MountId, MountRuntime>,
    ) -> Vec<(MountId, Action)> {
        plan(&PlanInput {
            now,
            ready: true,
            grace: GRACE,
            candidates: c,
            runtimes: r,
        })
    }

    #[test]
    fn prd_fake_discovery_fake_driver() {
        let now = Instant::now();
        let drv = FakeDriver::new("fake");
        let d = fake_world(&drv, now);
        assert!(d.conflicts.is_empty());
        let h = block_on(drv.mount(req(&d.candidates[&id("a")].spec))).unwrap();
        let rts = BTreeMap::from([(id("a"), MountRuntime::adopted(h))]);
        assert_eq!(
            run(now, &d.candidates, &rts),
            [
                (id("a"), Action::NoOp),
                (
                    id("b"),
                    Action::Mount {
                        driver: "fake".into()
                    }
                )
            ]
        );
    }

    #[test]
    fn plan_twice_second_all_noop() {
        let now = Instant::now();
        let drv = FakeDriver::new("fake");
        let d = fake_world(&drv, now);
        // "old" is mounted but no longer desired
        let old = block_on(drv.mount(req(&spec("old")))).unwrap();
        let mut rts = BTreeMap::from([(id("old"), MountRuntime::adopted(old))]);
        let p1 = run(now, &d.candidates, &rts);
        assert_eq!(
            p1.iter().map(|(_, a)| a.to_string()).collect::<Vec<_>>(),
            ["mount (fake)", "mount (fake)", "unmount (not desired)"]
        );
        execute(&drv, &p1, &d.candidates, &mut rts, now);
        let p2 = run(now, &d.candidates, &rts);
        assert!(p2.iter().all(|(_, a)| *a == Action::NoOp), "{p2:?}");
        assert!(!p2.iter().any(|(_, a)| a.has_side_effect()));
        assert_eq!(
            *drv.calls.lock().unwrap(),
            ["mount old", "mount a", "mount b", "unmount old force=false"]
        );
        assert_eq!(rts[&id("a")].health, Health::Healthy);
    }

    #[test]
    fn plan_is_deterministic() {
        let now = Instant::now();
        let (a, b, c) = (cand("a"), cand("b"), held("c"));
        let c1 = BTreeMap::from([
            (id("b"), b.clone()),
            (id("a"), a.clone()),
            (id("c"), c.clone()),
        ]);
        let c2 = BTreeMap::from([(id("c"), c.clone()), (id("a"), a), (id("b"), b.clone())]);
        let r1 = BTreeMap::from([
            (id("z"), mounted(&cand("z"), Health::Healthy)),
            (id("b"), mounted(&b, stale())),
            (id("c"), mounted(&c, Health::Healthy)),
        ]);
        let r2 = BTreeMap::from([
            (id("c"), mounted(&c, Health::Healthy)),
            (id("b"), mounted(&b, stale())),
            (id("z"), mounted(&cand("z"), Health::Healthy)),
        ]);
        let p = run(now, &c1, &r1);
        assert_eq!(p, run(now, &c2, &r2));
        assert_eq!(p, run(now, &c1, &r1));
        assert_eq!(
            p,
            [
                (
                    id("a"),
                    Action::Mount {
                        driver: "sshfs".into()
                    }
                ),
                (id("b"), remount(true, Reason::Stale)),
                (id("c"), unmount(false, Reason::Manual)),
                (id("z"), unmount(false, Reason::NotDesired)),
            ]
        );
    }

    #[test]
    fn row01_inflight_waits() {
        let now = Instant::now();
        for phase in [Phase::Mounting, Phase::Unmounting] {
            let rt = MountRuntime {
                phase,
                health: stale(),
                force_requested: true,
                ..Default::default()
            };
            for c in [None, Some(&cand("a")), Some(&held("a"))] {
                assert_eq!(dec(c, &rt, now), wait(WaitReason::InFlight));
            }
        }
        assert_eq!(
            wait(WaitReason::InFlight).to_string(),
            "waiting (in flight)"
        );
    }

    #[test]
    fn row02_absent_not_desired_noop() {
        let now = Instant::now();
        let rt = MountRuntime {
            mount_retry_at: Some(now + secs(9)),
            ..Default::default()
        };
        assert_eq!(dec(None, &rt, now), Action::NoOp);
        assert_eq!(dec(Some(&held("a")), &rt, now), Action::NoOp);
        assert_eq!(dec(None, &MountRuntime::default(), now), Action::NoOp);
        assert_eq!(Action::NoOp.to_string(), "noop");
        assert_eq!(Action::NoOp.kind(), "noop");
    }

    #[test]
    fn row03_stale_not_desired_force() {
        let now = Instant::now();
        let c = cand("a");
        let rt = mounted(&c, stale());
        assert_eq!(dec(None, &rt, now), unmount(true, Reason::NotDesired));
        assert_eq!(
            dec(Some(&held("a")), &rt, now),
            unmount(true, Reason::Manual)
        );
        // force_requested (API unmount --force) on a healthy mount; also during warm-up
        let rt = MountRuntime {
            force_requested: true,
            ..mounted(&c, Health::Healthy)
        };
        assert_eq!(
            decide(Some(&held("a")), &rt, now, false, GRACE),
            unmount(true, Reason::Manual)
        );
        assert_eq!(
            decide(None, &mounted(&c, stale()), now, false, GRACE),
            unmount(true, Reason::NotDesired)
        );
        // `unmount --force` while the mount was still in flight: it lands, then row 3 forces it off
        let mut rt = MountRuntime::default();
        let g = rt.begin(Phase::Mounting);
        rt.force_requested = true;
        rt.mount_done(g, Ok(handle(&c)), now, &T, 0);
        assert_eq!(
            dec(Some(&held("a")), &rt, now),
            unmount(true, Reason::Manual)
        );
        let a = unmount(true, Reason::Manual);
        assert!(a.has_side_effect());
        assert_eq!(
            (a.kind(), a.to_string().as_str()),
            ("unmount", "unmount (manual, force)")
        );
    }

    #[test]
    fn row04_warmup_blocks_removal() {
        let now = Instant::now();
        let c = cand("a");
        for h in [
            Health::Healthy,
            Health::Unknown,
            Health::Degraded("x".into()),
        ] {
            let rt = mounted(&c, h);
            assert_eq!(
                decide(None, &rt, now, false, GRACE),
                wait(WaitReason::WarmingUp)
            );
        }
        assert_eq!(
            wait(WaitReason::WarmingUp).to_string(),
            "waiting (warming up)"
        );
    }

    #[test]
    fn row04_held_bypasses_warmup() {
        let now = Instant::now();
        let rt = mounted(&cand("a"), Health::Healthy);
        assert_eq!(
            decide(Some(&held("a")), &rt, now, false, GRACE),
            unmount(false, Reason::Manual)
        );
    }

    #[test]
    fn row05_not_desired_graceful() {
        let now = Instant::now();
        let rt = mounted(&cand("a"), Health::Healthy);
        assert_eq!(dec(None, &rt, now), unmount(false, Reason::NotDesired));
        assert_eq!(
            dec(Some(&held("a")), &rt, now),
            unmount(false, Reason::Manual)
        );
        let rt = mounted(&cand("a"), Health::Degraded("x".into()));
        assert_eq!(dec(None, &rt, now), unmount(false, Reason::NotDesired));
        assert_eq!(
            unmount(false, Reason::NotDesired).to_string(),
            "unmount (not desired)"
        );
    }

    #[test]
    fn manual_reason_when_held() {
        // A14: rows 3 and 5 say Manual for a held candidate, NotDesired when there is no candidate
        let now = Instant::now();
        let c = cand("a");
        let reason = |a: Action| match a {
            Action::Unmount { why, .. } => why,
            a => panic!("{a:?}"),
        };
        for rt in [mounted(&c, stale()), mounted(&c, Health::Healthy)] {
            assert_eq!(reason(dec(Some(&held("a")), &rt, now)), Reason::Manual);
            assert_eq!(reason(dec(None, &rt, now)), Reason::NotDesired);
        }
    }

    #[test]
    fn row06_no_driver_waits() {
        let now = Instant::now();
        let c = Candidate {
            driver: Err("sshfs unavailable: not installed".into()),
            online: Some(false),
            ..cand("a")
        };
        let rt = MountRuntime {
            mount_retry_at: Some(now + secs(5)),
            ..Default::default()
        };
        let a = dec(Some(&c), &rt, now);
        assert_eq!(
            a,
            wait(WaitReason::NoDriver(
                "sshfs unavailable: not installed".into()
            ))
        );
        assert_eq!(
            a.to_string(),
            "waiting (no driver: sshfs unavailable: not installed)"
        );
    }

    #[test]
    fn row07_offline_waits() {
        let now = Instant::now();
        let c = Candidate {
            online: Some(false),
            ..cand("a")
        };
        let rt = MountRuntime {
            mount_retry_at: Some(now + secs(5)),
            ..Default::default()
        };
        let a = dec(Some(&c), &rt, now);
        assert_eq!(a, wait(WaitReason::MachineOffline));
        assert_eq!(a.to_string(), "waiting (machine offline)");
        assert!(!a.has_side_effect());
    }

    #[test]
    fn row08_backoff_waits() {
        let now = Instant::now();
        let rt = MountRuntime {
            mount_retry_at: Some(now + secs(5)),
            ..Default::default()
        };
        let c = cand("a");
        let a = dec(Some(&c), &rt, now);
        assert_eq!(a, wait(WaitReason::Backoff(secs(5))));
        assert_eq!(a.to_string(), "waiting (backoff 5s)");
        assert_eq!(a.kind(), "waiting");
        assert_eq!(
            dec(Some(&c), &rt, now + Duration::from_millis(2500)).to_string(),
            "waiting (backoff 3s)"
        );
        assert_eq!(
            dec(Some(&c), &rt, now + secs(5)),
            Action::Mount {
                driver: "sshfs".into()
            }
        );
    }

    #[test]
    fn row09_mount() {
        let now = Instant::now();
        let a = dec(Some(&cand("a")), &MountRuntime::default(), now);
        assert_eq!(
            a,
            Action::Mount {
                driver: "sshfs".into()
            }
        );
        assert_eq!(
            (a.kind(), a.to_string().as_str()),
            ("mount", "mount (sshfs)")
        );
        assert!(a.has_side_effect());
        // online == Some(true) and None both mount
        let c = Candidate {
            online: Some(true),
            ..cand("a")
        };
        assert!(dec(Some(&c), &MountRuntime::default(), now).has_side_effect());
    }

    #[test]
    fn row10_stale_remount_force() {
        let now = Instant::now();
        let c = cand("a");
        let a = dec(Some(&c), &mounted(&c, stale()), now);
        assert_eq!(a, remount(true, Reason::Stale));
        assert_eq!(
            (a.kind(), a.to_string().as_str()),
            ("remount", "remount (stale, force)")
        );
        // stale beats a spec change
        assert_eq!(
            dec(Some(&moved("a")), &mounted(&c, stale()), now),
            remount(true, Reason::Stale)
        );
    }

    #[test]
    fn row11_spec_change_remount_graceful() {
        let now = Instant::now();
        for h in [Health::Healthy, Health::Unknown] {
            let a = dec(Some(&moved("a")), &mounted(&cand("a"), h), now);
            assert_eq!(a, remount(false, Reason::SpecChanged));
            assert_eq!(a.to_string(), "remount (spec changed)");
        }
    }

    #[test]
    fn row11_degraded_spec_change_lazy() {
        let now = Instant::now();
        let rt = degraded(&cand("a"), now);
        assert_eq!(
            dec(Some(&moved("a")), &rt, now),
            remount(true, Reason::SpecChanged)
        );
    }

    #[test]
    fn row11_gated_on_driver_and_online() {
        let now = Instant::now();
        let no_driver = Candidate {
            driver: Err("rclone unavailable: not installed".into()),
            ..moved("a")
        };
        let offline = Candidate {
            online: Some(false),
            ..moved("a")
        };
        let healthy = mounted(&cand("a"), Health::Healthy);
        assert_eq!(
            dec(Some(&no_driver), &healthy, now),
            Action::Degraded("change pending: rclone unavailable: not installed".into())
        );
        assert_eq!(
            dec(Some(&offline), &healthy, now),
            Action::Degraded("change pending: machine offline".into())
        );
        // r3-5: before ready a partial discovery view (lower-trust provider first) must not remount an adopted mount
        assert_eq!(
            decide(Some(&moved("a")), &healthy, now, false, GRACE),
            Action::Degraded("change pending: warming up".into())
        );
        // a Degraded mount with the gate closed shows its own reason, and still reaches row 12 after grace
        let rt = degraded(&cand("a"), now);
        assert_eq!(
            dec(Some(&offline), &rt, now + secs(10)),
            Action::Degraded("timeout".into())
        );
        assert_eq!(
            dec(Some(&no_driver), &rt, now + GRACE),
            unmount(true, Reason::OfflineGrace)
        );
    }

    #[test]
    fn row12_grace_elapsed_lazy_unmount() {
        let now = Instant::now();
        let c = cand("a");
        let rt = degraded(&c, now);
        let a = dec(Some(&c), &rt, now + GRACE);
        assert_eq!(a, unmount(true, Reason::OfflineGrace));
        assert_eq!(a.to_string(), "unmount (offline grace, force)");
        assert_eq!(
            dec(Some(&c), &rt, now + GRACE - Duration::from_millis(1)),
            Action::Degraded("timeout".into())
        );
    }

    #[test]
    fn row13_degraded_within_grace() {
        let now = Instant::now();
        let c = cand("a");
        let a = dec(Some(&c), &degraded(&c, now), now + secs(10));
        assert_eq!(a, Action::Degraded("timeout".into()));
        assert_eq!(
            (a.kind(), a.to_string().as_str()),
            ("degraded", "degraded (timeout)")
        );
        assert!(!a.has_side_effect());
    }

    #[test]
    fn row14_offline_but_healthy_noop() {
        let now = Instant::now();
        let c = Candidate {
            online: Some(false),
            ..cand("a")
        };
        for h in [Health::Healthy, Health::Unknown] {
            assert_eq!(dec(Some(&c), &mounted(&c, h), now), Action::NoOp);
        }
        let rt = MountRuntime::adopted(handle(&c));
        assert_eq!(dec(Some(&c), &rt, now), Action::NoOp);
    }

    #[test]
    fn gate_u_unmount_backoff() {
        let now = Instant::now();
        let c = cand("a");
        let gate = |rt: MountRuntime| MountRuntime {
            unmount_retry_at: Some(now + secs(7)),
            last_error: Some(BUSY.into()),
            ..rt
        };
        let busy = Action::Degraded(BUSY.into());
        let backoff7 = wait(WaitReason::Backoff(secs(7)));
        // rows 3 and 5 wait out the unmount backoff
        assert_eq!(dec(None, &gate(mounted(&c, stale())), now), backoff7);
        assert_eq!(
            dec(Some(&held("a")), &gate(mounted(&c, Health::Healthy)), now),
            backoff7
        );
        // rows 10, 11, 12 show why
        assert_eq!(dec(Some(&c), &gate(mounted(&c, stale())), now), busy);
        assert_eq!(
            dec(Some(&moved("a")), &gate(mounted(&c, Health::Healthy)), now),
            busy
        );
        assert_eq!(dec(Some(&c), &gate(degraded(&c, now - GRACE)), now), busy);
        // once the deadline passes, each fires again
        let later = now + secs(7);
        assert_eq!(
            dec(Some(&held("a")), &gate(mounted(&c, Health::Healthy)), later),
            unmount(false, Reason::Manual)
        );
        assert_eq!(
            dec(
                Some(&moved("a")),
                &gate(mounted(&c, Health::Healthy)),
                later
            ),
            remount(false, Reason::SpecChanged)
        );
    }

    #[test]
    fn stale_cleanup_not_gated_by_mount_backoff() {
        let now = Instant::now();
        let c = cand("a");
        let mut rt = mounted(&c, Health::Healthy);
        let ev = rt.health(1, MountState::Stale("ENOTCONN".into()), now, &T, 0);
        assert_eq!(
            ev,
            Some(Event::MountDegraded {
                mount: "a".into(),
                reason: "stale: ENOTCONN".into()
            })
        );
        assert_eq!((rt.failures, rt.mount_retry_at), (1, Some(now + bo(1))));
        assert_eq!(dec(Some(&c), &rt, now), remount(true, Reason::Stale));
        assert_eq!(dec(None, &rt, now), unmount(true, Reason::NotDesired));
        // a repeated Stale is not a new failure
        assert_eq!(
            rt.health(1, MountState::Stale("x".into()), now, &T, 0),
            None
        );
        assert_eq!(rt.failures, 1);
    }

    #[test]
    fn missing_goes_absent_with_backoff() {
        let now = Instant::now();
        let c = cand("a");
        let mut rt = MountRuntime {
            mounted_at: Some(now),
            degraded_since: Some(now),
            force_requested: true,
            ..mounted(&c, Health::Degraded("x".into()))
        };
        let ev = rt.health(1, MountState::Missing, now, &T, 0);
        assert_eq!(
            ev,
            Some(Event::MountFailed {
                mount: "a".into(),
                error: "mount disappeared".into(),
                attempt: 1,
                retry_in_ms: Some(bo(1).as_millis() as u64)
            })
        );
        assert_eq!(rt.phase, Phase::Absent);
        assert_eq!(rt.handle, None);
        assert_eq!(rt.health, Health::Unknown);
        assert_eq!((rt.mounted_at, rt.degraded_since), (None, None));
        assert!(!rt.force_requested);
        assert_eq!(rt.last_error.as_deref(), Some("mount disappeared"));
        assert_eq!(rt.mount_retry_at, Some(now + bo(1)));
        assert_eq!(dec(Some(&c), &rt, now), wait(WaitReason::Backoff(bo(1))));
        assert_eq!(
            mount_availability(Some(&rt), Some(&c)),
            Availability::Failed
        );
        assert_eq!(
            dec(Some(&c), &rt, now + bo(1)),
            Action::Mount {
                driver: "sshfs".into()
            }
        );
    }

    #[test]
    fn stale_generation_ignored_for_health_and_exit() {
        let now = Instant::now();
        let c = cand("a");
        let mut rt = mounted(&c, Health::Healthy);
        // a probe started for generation 1 (by a tick or a child exit) lands after the unmount began
        let g = rt.begin(Phase::Unmounting);
        assert_eq!(g, 2);
        let before = format!("{rt:?}");
        assert_eq!(rt.health(1, MountState::Missing, now, &T, 0), None);
        assert_eq!(
            rt.health(1, MountState::Stale("x".into()), now, &T, 0),
            None
        );
        assert_eq!(rt.mount_done(1, Ok(handle(&c)), now, &T, 0), None);
        assert_eq!(rt.unmount_done(1, Reason::Manual, Ok(()), now, &T, 0), None);
        assert_eq!(format!("{rt:?}"), before);
        assert_eq!(
            rt.unmount_done(2, Reason::Manual, Ok(()), now, &T, 0),
            Some(Event::UnmountComplete { mount: "a".into() })
        );
        // the current generation still only applies in the phase it was issued for
        assert_eq!(rt.health(2, MountState::Missing, now, &T, 0), None);
        assert_eq!(rt.unmount_done(2, Reason::Manual, Ok(()), now, &T, 0), None);
        assert_eq!(rt.failures, 0);
        // begin bumps for both operations
        assert_eq!(rt.begin(Phase::Mounting), 3);
    }

    #[test]
    fn failures_reset_only_after_stable_healthy() {
        let now = Instant::now();
        let c = cand("a");
        let mut rt = MountRuntime {
            failures: 3,
            last_error: Some("boom".into()),
            mount_retry_at: Some(now),
            ..Default::default()
        };
        let g = rt.begin(Phase::Mounting);
        assert_eq!(
            rt.mount_done(g, Ok(handle(&c)), now, &T, 0),
            Some(Event::MountStarted {
                mount: "a".into(),
                driver: "sshfs".into(),
                pid: Some(42)
            })
        );
        assert_eq!(
            (rt.phase, rt.failures, rt.mounted_at),
            (Phase::Mounted, 3, Some(now))
        );
        assert_eq!((rt.last_error.as_deref(), rt.mount_retry_at), (None, None));
        assert_eq!(
            rt.health(g, MountState::Healthy, now + T.retry_max - secs(1), &T, 0),
            Some(Event::MountHealthy { mount: "a".into() })
        );
        assert_eq!(rt.failures, 3);
        assert_eq!(
            rt.health(g, MountState::Healthy, now + T.retry_max, &T, 0),
            None
        );
        assert_eq!(rt.failures, 0);
        // an adopted mount has no mounted_at: its count is never reset by a probe
        let mut ad = MountRuntime {
            failures: 2,
            ..MountRuntime::adopted(handle(&c))
        };
        ad.health(0, MountState::Healthy, now + secs(3600), &T, 0);
        assert_eq!(ad.failures, 2);
    }

    #[test]
    fn mount_error_backs_off() {
        let now = Instant::now();
        let mut rt = MountRuntime::default();
        let g = rt.begin(Phase::Mounting);
        let ev = rt.mount_done(
            g,
            Err(MountError::Failed("Host key verification failed".into())),
            now,
            &T,
            0,
        );
        assert_eq!(
            ev,
            Some(Event::MountFailed {
                mount: String::new(),
                error: "Host key verification failed".into(),
                attempt: 1,
                retry_in_ms: Some(bo(1).as_millis() as u64)
            })
        );
        assert_eq!((rt.phase, rt.failures), (Phase::Absent, 1));
        assert_eq!(rt.mount_retry_at, Some(now + bo(1)));
        assert_eq!(
            rt.last_error.as_deref(),
            Some("Host key verification failed")
        );
        let g = rt.begin(Phase::Mounting);
        rt.mount_done(
            g,
            Err(MountError::Failed("again".into())),
            now,
            &T,
            u64::MAX,
        );
        assert_eq!(rt.failures, 2);
        assert_eq!(
            rt.mount_retry_at,
            Some(now + backoff(2, T.retry_initial, T.retry_max, u64::MAX))
        );
    }

    #[test]
    fn unmount_error_keeps_mount_and_backs_off() {
        let now = Instant::now();
        let c = cand("a");
        let mut rt = mounted(&c, Health::Healthy);
        let g = rt.begin(Phase::Unmounting);
        assert_eq!(
            rt.unmount_done(g, Reason::SpecChanged, Err(MountError::Busy), now, &T, 0),
            Some(Event::MountDegraded {
                mount: "a".into(),
                reason: BUSY.into()
            })
        );
        assert_eq!((rt.phase, rt.failures), (Phase::Mounted, 1));
        assert_eq!(rt.unmount_retry_at, Some(now + bo(1)));
        assert_eq!(rt.last_error.as_deref(), Some(BUSY));
        // the busy reason survives a healthy probe while the retry is pending (C4)
        rt.health(g, MountState::Healthy, now, &T, 0);
        assert_eq!(
            dec(Some(&moved("a")), &rt, now),
            Action::Degraded(BUSY.into())
        );
        let g = rt.begin(Phase::Unmounting);
        assert_eq!(
            rt.unmount_done(
                g,
                Reason::Stale,
                Err(MountError::Failed("EPERM".into())),
                now,
                &T,
                0
            ),
            Some(Event::MountDegraded {
                mount: "a".into(),
                reason: "unmount failed: EPERM".into()
            })
        );
        assert_eq!(rt.failures, 2);
    }

    /// A15's clean slate waits for a pending unmount retry: a long-up mount held busy (a shell cwd) must keep
    /// backing off, not be reset to bo(1) by every health tick.
    #[test]
    fn busy_unmount_backoff_survives_healthy_probe() {
        let t0 = Instant::now();
        let now = t0 + 2 * T.retry_max;
        let c = cand("a");
        let mut rt = MountRuntime {
            mounted_at: Some(t0),
            ..mounted(&c, Health::Healthy)
        };
        let g = rt.begin(Phase::Unmounting);
        rt.unmount_done(g, Reason::SpecChanged, Err(MountError::Busy), now, &T, 0);
        rt.health(g, MountState::Healthy, now + bo(1) / 2, &T, 0);
        assert_eq!(rt.failures, 1);
        let g = rt.begin(Phase::Unmounting);
        rt.unmount_done(
            g,
            Reason::SpecChanged,
            Err(MountError::Busy),
            now + bo(1),
            &T,
            0,
        );
        assert_eq!(rt.failures, 2);
        assert_eq!(rt.unmount_retry_at, Some(now + bo(1) + bo(2)));
    }

    /// A busy remount whose spec then reverts (DNS flap) is no longer wanted: once its retry time passes, a
    /// healthy probe drops the timer and the busy text, so neither lingers nor keeps `next_wakeup` in the past.
    #[test]
    fn expired_unmount_retry_clears_busy_error() {
        let now = Instant::now();
        let c = cand("a");
        let mut rt = mounted(&c, Health::Healthy);
        let g = rt.begin(Phase::Unmounting);
        rt.unmount_done(g, Reason::SpecChanged, Err(MountError::Busy), now, &T, 0);
        assert_eq!(dec(Some(&c), &rt, now), Action::NoOp); // reverted: row 14
        rt.health(g, MountState::Healthy, now + bo(1), &T, 0);
        assert_eq!(
            (rt.last_error.as_deref(), rt.unmount_retry_at),
            (None, None)
        );
        assert_eq!(
            next_wakeup(&BTreeMap::from([(id("a"), rt.clone())]), GRACE, now),
            None
        );
        // a busy unmount that later succeeds leaves no stale error behind (would read Failed while Absent)
        let g = rt.begin(Phase::Unmounting);
        rt.unmount_done(g, Reason::SpecChanged, Err(MountError::Busy), now, &T, 0);
        let g = rt.begin(Phase::Unmounting);
        rt.unmount_done(g, Reason::SpecChanged, Ok(()), now + bo(2), &T, 0);
        assert_eq!(rt.last_error, None);
        assert_eq!(
            mount_availability(Some(&rt), Some(&c)),
            Availability::Eligible
        );
    }

    #[test]
    fn grace_unmount_sets_offline() {
        let now = Instant::now();
        let c = cand("a");
        let mut rt = degraded(&c, now);
        let g = rt.begin(Phase::Unmounting);
        assert_eq!(
            rt.unmount_done(g, Reason::OfflineGrace, Ok(()), now, &T, 0),
            Some(Event::UnmountComplete { mount: "a".into() })
        );
        assert!(rt.offline);
        assert_eq!((rt.phase, rt.failures), (Phase::Absent, 1));
        assert_eq!(rt.mount_retry_at, Some(now + bo(1)));
        assert_eq!((rt.handle.as_ref(), rt.degraded_since), (None, None));
        assert_eq!(
            mount_availability(Some(&rt), Some(&c)),
            Availability::Offline
        );
        let g = rt.begin(Phase::Mounting);
        rt.mount_done(g, Ok(handle(&c)), now, &T, 0);
        assert!(!rt.offline);
        // a graceful NotDesired unmount is not a failure and not offline
        let mut rt = mounted(&c, Health::Healthy);
        let g = rt.begin(Phase::Unmounting);
        rt.unmount_done(g, Reason::NotDesired, Ok(()), now, &T, 0);
        assert!(!rt.offline);
        assert_eq!((rt.failures, rt.mount_retry_at), (0, None));
    }

    #[test]
    fn offline_chain_mounted_degraded_offline() {
        use Availability as A;
        let now = Instant::now();
        let c = cand("a");
        let av = |rt: &MountRuntime| mount_availability(Some(rt), Some(&c));
        let mut rt = MountRuntime::default();
        assert_eq!(av(&rt), A::Eligible);
        let g = rt.begin(Phase::Mounting);
        assert_eq!(av(&rt), A::Connecting);
        rt.mount_done(g, Ok(handle(&c)), now, &T, 0);
        rt.health(g, MountState::Healthy, now, &T, 0);
        assert_eq!(av(&rt), A::Mounted);
        let t1 = now + secs(10);
        assert_eq!(
            rt.health(g, MountState::Degraded("timeout".into()), t1, &T, 0),
            Some(Event::MountDegraded {
                mount: "a".into(),
                reason: "timeout".into()
            })
        );
        // a second Degraded is not a new event and keeps the original start
        assert_eq!(
            rt.health(g, MountState::Degraded("t2".into()), t1 + secs(5), &T, 0),
            None
        );
        assert_eq!(rt.degraded_since, Some(t1));
        assert_eq!(av(&rt), A::Degraded);
        assert_eq!(
            dec(Some(&c), &rt, t1 + secs(15)),
            Action::Degraded("t2".into())
        );
        let t2 = t1 + GRACE;
        assert_eq!(dec(Some(&c), &rt, t2), unmount(true, Reason::OfflineGrace));
        let g = rt.begin(Phase::Unmounting);
        assert_eq!(av(&rt), A::Unmounting);
        rt.unmount_done(g, Reason::OfflineGrace, Ok(()), t2, &T, 0);
        assert_eq!(av(&rt), A::Offline);
        assert_eq!(dec(Some(&c), &rt, t2), wait(WaitReason::Backoff(bo(1))));
        // still desired and retrying; while the provider says offline it waits instead
        let off = Candidate {
            online: Some(false),
            ..c.clone()
        };
        assert_eq!(
            dec(Some(&off), &rt, t2 + secs(60)),
            wait(WaitReason::MachineOffline)
        );
        assert_eq!(mount_availability(Some(&rt), Some(&off)), A::Offline);
        assert_eq!(
            dec(Some(&c), &rt, t2 + secs(60)),
            Action::Mount {
                driver: "sshfs".into()
            }
        );
    }

    #[test]
    fn auto_driver_sticky_fingerprint() {
        let mut reg = MachineRegistry::default();
        let ts = Source {
            trust: 1,
            kind: "tailscale".into(),
            provider: "tailscale".into(),
        };
        reg.apply_ok(&ts, vec![obs("a", "100.64.0.1")], Instant::now(), secs(90));
        let mut w = World {
            machines: reg.machines(&include_all("tailscale")),
            templates: BTreeMap::from([("tailscale".to_string(), tmpl(false))]),
            auto_order: vec!["sshfs".into(), "rclone".into()],
            probes: BTreeMap::from([("sshfs".into(), avail()), ("rclone".into(), avail())]),
            ..Default::default()
        };
        let c1 = w.desired().candidates[&id("a")].clone();
        assert_eq!(c1.driver.as_deref(), Ok("sshfs"));
        w.probes.insert("sshfs".into(), unavail("gone"));
        let c2 = w.desired().candidates[&id("a")].clone();
        assert_eq!(c2.driver.as_deref(), Ok("rclone"));
        assert_eq!(c1.spec.fingerprint(), c2.spec.fingerprint());
        assert_eq!(
            dec(Some(&c2), &mounted(&c1, Health::Healthy), Instant::now()),
            Action::NoOp
        );
    }

    #[test]
    fn select_driver_auto_order_and_named_unavailable() {
        let order = |o: &[&str]| o.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let probes = BTreeMap::from([
            ("sshfs".to_string(), unavail("not installed")),
            ("rclone".to_string(), avail()),
            ("rclone-nfs".to_string(), avail()),
        ]);
        let auto = DriverSelector::Auto;
        assert_eq!(
            select_driver(&auto, &order(&["sshfs", "rclone"]), &probes),
            Ok("rclone".into())
        );
        assert_eq!(
            select_driver(&auto, &order(&["rclone-nfs", "rclone"]), &probes),
            Ok("rclone-nfs".into())
        );
        assert_eq!(
            select_driver(&auto, &order(&["sshfs", "nfs"]), &probes),
            Err("no available driver (tried sshfs, nfs)".into())
        );
        let named = |n: &str| DriverSelector::Named(n.into());
        // named never substitutes, even when another driver is available
        assert_eq!(
            select_driver(&named("sshfs"), &order(&["rclone"]), &probes),
            Err("sshfs unavailable: not installed".into())
        );
        assert_eq!(
            select_driver(&named("x"), &order(&["rclone"]), &probes),
            Err("x unavailable: not probed".into())
        );
        assert_eq!(
            select_driver(&named("rclone"), &[], &probes),
            Ok("rclone".into())
        );
    }

    #[test]
    fn hints_ignored_unless_honor_hints() {
        let dns = Source {
            trust: 3,
            kind: "dns".into(),
            provider: "infra".into(),
        };
        let mut o = obs("agent-01", "10.0.0.7");
        o.port = Some(2222);
        o.online = Some(true);
        o.hints.user = Some(User::parse("evil").unwrap());
        o.hints.path = Some(RemotePath::parse("/etc").unwrap());
        let mut bare = obs("agent-02", "10.0.0.8");
        bare.hints = Default::default();
        let mut reg = MachineRegistry::default();
        reg.apply_ok(&dns, vec![o, bare], Instant::now(), secs(90));
        let mut w = World {
            machines: reg.machines(&include_all("infra")),
            templates: BTreeMap::from([("infra".to_string(), tmpl(false))]),
            auto_order: vec!["sshfs".into()],
            probes: BTreeMap::from([("sshfs".into(), avail())]),
            ..Default::default()
        };
        let d = w.desired();
        let s = &d.candidates[&id("agent-01")].spec;
        assert_eq!(s.user.as_ref().map(User::as_str), Some("sami"));
        assert_eq!(s.remote.as_str(), "~/proj");
        assert_eq!((s.host.as_str(), s.port), ("10.0.0.7", Some(2222)));
        assert_eq!(s.local_path, Path::new("/m/agent-01"));
        assert_eq!(s.driver, DriverSelector::Auto);
        assert_eq!(d.candidates[&id("agent-01")].online, Some(true));
        w.templates.insert("infra".into(), tmpl(true));
        let d = w.desired();
        let s = &d.candidates[&id("agent-01")].spec;
        assert_eq!(s.user.as_ref().map(User::as_str), Some("evil"));
        assert_eq!(s.remote.as_str(), "/etc");
        // no hints: the template still applies
        let s = &d.candidates[&id("agent-02")].spec;
        assert_eq!(s.user.as_ref().map(User::as_str), Some("sami"));
        assert_eq!(s.remote.as_str(), "~/proj");
    }

    #[test]
    fn static_local_collision_conflict() {
        let stat = Source {
            trust: 0,
            kind: "static".into(),
            provider: "static".into(),
        };
        let ts = Source {
            trust: 1,
            kind: "tailscale".into(),
            provider: "tailscale".into(),
        };
        let mut build = obs("build", "10.0.0.18");
        build.port = Some(22);
        build.hints.user = Some(User::parse("sami").unwrap());
        let mut reg = MachineRegistry::default();
        reg.replace(&stat, vec![build]);
        let now = Instant::now();
        reg.apply_ok(
            &ts,
            vec![obs("agent-01", "100.64.0.1"), obs("other", "100.64.0.2")],
            now,
            secs(90),
        );
        let sm = |local: &str, remote: &str| StaticMount {
            local: id(local),
            remote: RemotePath::parse(remote).unwrap(),
            driver: DriverSelector::Named("sshfs".into()),
            read_only: true,
        };
        let mut policy = include_all("tailscale");
        policy.deny.names = vec![Glob::parse("other").unwrap()];
        let w = World {
            machines: reg.machines(&policy),
            statics: BTreeMap::from([(
                id("build"),
                vec![sm("build", "/home/sami"), sm("agent-01", "/srv")],
            )]),
            templates: BTreeMap::from([("tailscale".to_string(), tmpl(false))]),
            held: BTreeSet::from([id("build")]),
            auto_order: vec!["sshfs".into()],
            probes: BTreeMap::from([("sshfs".into(), unavail("no fuse"))]),
        };
        let d = w.desired();
        assert_eq!(
            d.conflicts,
            ["agent-01: local name taken by static machine build"]
        );
        assert_eq!(
            d.candidates.keys().map(Name::as_str).collect::<Vec<_>>(),
            ["agent-01", "build"]
        );
        let c = &d.candidates[&id("agent-01")];
        assert_eq!(c.spec.machine.as_str(), "build");
        assert_eq!(c.spec.remote.as_str(), "/srv");
        assert_eq!(c.spec.local_path, Path::new("/m/agent-01"));
        assert_eq!((c.spec.host.as_str(), c.spec.port), ("10.0.0.18", Some(22)));
        assert_eq!(c.spec.user.as_ref().map(User::as_str), Some("sami"));
        assert!(c.spec.read_only && !c.held);
        assert_eq!(c.driver, Err("sshfs unavailable: no fuse".into()));
        assert!(d.candidates[&id("build")].held);
    }

    #[test]
    fn next_wakeup_is_earliest_deadline() {
        let now = Instant::now();
        let c = cand("a");
        let absent = MountRuntime {
            mount_retry_at: Some(now + secs(30)),
            unmount_retry_at: Some(now + secs(1)), // not a deadline while Absent
            ..Default::default()
        };
        let busy = MountRuntime {
            unmount_retry_at: Some(now + secs(20)),
            mount_retry_at: Some(now + secs(2)), // Stale backoff: not a deadline while Mounted
            ..mounted(&c, Health::Healthy)
        };
        let grace = degraded(&c, now + secs(15) - GRACE); // row 12 fires at now + 15s
        let inflight = MountRuntime {
            phase: Phase::Mounting,
            mount_retry_at: Some(now + secs(3)),
            ..Default::default()
        };
        let mut rts = BTreeMap::from([
            (id("a"), absent),
            (id("b"), busy),
            (id("c"), grace),
            (id("d"), inflight),
        ]);
        assert_eq!(next_wakeup(&rts, GRACE, now), Some(now + secs(15)));
        // passed deadlines are ignored, so the actor never sleeps until an instant <= now
        assert_eq!(
            next_wakeup(&rts, GRACE, now + secs(16)),
            Some(now + secs(20))
        );
        rts.remove(&id("c"));
        assert_eq!(next_wakeup(&rts, GRACE, now), Some(now + secs(20)));
        rts.remove(&id("b"));
        assert_eq!(next_wakeup(&rts, GRACE, now), Some(now + secs(30)));
        rts.remove(&id("a"));
        assert_eq!(next_wakeup(&rts, GRACE, now), None);
        // past grace but row 12 waits out ⊳U: wake at the unmount retry, not the passed grace deadline
        let held_back = MountRuntime {
            unmount_retry_at: Some(now + secs(9)),
            ..degraded(&c, now - GRACE - secs(5))
        };
        let rts = BTreeMap::from([(id("e"), held_back)]);
        assert_eq!(next_wakeup(&rts, GRACE, now), Some(now + secs(9)));
    }

    #[test]
    fn availability_tables() {
        use Availability as A;
        let c = cand("a");
        let av = |rt: Option<&MountRuntime>, c: Option<&Candidate>| mount_availability(rt, c);
        // mount level
        assert_eq!(av(None, Some(&c)), A::Eligible);
        for (phase, want) in [
            (Phase::Mounting, A::Connecting),
            (Phase::Unmounting, A::Unmounting),
        ] {
            let rt = MountRuntime {
                phase,
                ..mounted(&c, stale())
            };
            assert_eq!(av(Some(&rt), Some(&c)), want);
        }
        assert_eq!(
            av(Some(&mounted(&c, Health::Healthy)), Some(&c)),
            A::Mounted
        );
        assert_eq!(
            av(Some(&mounted(&c, Health::Unknown)), Some(&c)),
            A::Mounted
        );
        assert_eq!(av(Some(&mounted(&c, stale())), Some(&c)), A::Degraded);
        assert_eq!(av(Some(&degraded(&c, Instant::now())), None), A::Degraded);
        // a held mount still mounted shows Mounted; once Absent it is Eligible (held: true in the DTO)
        assert_eq!(
            av(Some(&mounted(&c, Health::Healthy)), Some(&held("a"))),
            A::Mounted
        );
        let failed = MountRuntime {
            last_error: Some("boom".into()),
            offline: true,
            ..Default::default()
        };
        assert_eq!(av(Some(&failed), Some(&held("a"))), A::Eligible);
        // absent and desired: Offline, then driver error (B15), then last_error, else Eligible
        assert_eq!(av(Some(&failed), Some(&c)), A::Offline);
        let off = Candidate {
            online: Some(false),
            driver: Err("x".into()),
            ..c.clone()
        };
        assert_eq!(av(None, Some(&off)), A::Offline);
        let no_driver = Candidate {
            driver: Err("sshfs unavailable: x".into()),
            ..c.clone()
        };
        assert_eq!(av(None, Some(&no_driver)), A::Failed);
        let errored = MountRuntime {
            last_error: Some("boom".into()),
            ..Default::default()
        };
        assert_eq!(av(Some(&errored), Some(&c)), A::Failed);
        assert_eq!(av(Some(&MountRuntime::default()), Some(&c)), A::Eligible);
        // adopted, machine not (yet) in the registry
        let adopted = MountRuntime::adopted(handle(&c));
        assert_eq!(av(Some(&adopted), None), A::Unknown);
        assert_eq!(av(Some(&adopted), Some(&c)), A::Mounted);

        // machine level
        let m = |verdict: Verdict| Machine {
            id: id("a"),
            observed: vec![Observed {
                source: Source {
                    trust: 1,
                    kind: "tailscale".into(),
                    provider: "tailscale".into(),
                },
                obs: obs("a", "10.0.0.1"),
                expires_at: None,
            }],
            selected: 0,
            verdict,
        };
        let allowed = m(Verdict::Allowed {
            by: "static".into(),
        });
        assert_eq!(
            machine_availability(&m(Verdict::DiscoverOnly), &[A::Mounted]),
            A::Discovered
        );
        let denied = m(Verdict::Denied { by: "x".into() });
        assert_eq!(machine_availability(&denied, &[]), A::Discovered);
        assert_eq!(machine_availability(&allowed, &[]), A::Eligible);
        for (mounts, want) in [
            (&[A::Mounted, A::Failed, A::Eligible][..], A::Failed),
            (&[A::Mounted, A::Degraded, A::Offline], A::Degraded),
            (&[A::Offline, A::Unmounting], A::Offline),
            (&[A::Connecting, A::Unmounting], A::Unmounting),
            (&[A::Connecting, A::Mounted], A::Connecting),
            (&[A::Mounted, A::Eligible], A::Mounted),
            (&[A::Unknown], A::Unknown),
        ] {
            assert_eq!(machine_availability(&allowed, mounts), want, "{mounts:?}");
        }
    }
}
