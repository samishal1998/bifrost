//! Machine registry: merges observations by trust, ages them out. STUB (S0.3): S1-A implements the bodies.

use crate::model::MachineObservation;
use crate::policy::{Policy, Verdict};
use crate::validate::MachineId;
use std::time::{Duration, Instant};

/// Ord == trust order: `trust` (lower = more trusted; 0 = static), then kind, then provider name.
/// Ranks are assigned by bifrost-config (static 0, tailscale 1, http 2, dns 3); core only compares numbers (B8).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Source {
    pub trust: u8,
    pub kind: String,
    pub provider: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Observed {
    pub source: Source,
    pub obs: MachineObservation,
    pub expires_at: Option<Instant>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Machine {
    pub id: MachineId,
    /// trust order
    pub observed: Vec<Observed>,
    pub selected: usize,
    pub verdict: Verdict,
}

impl Machine {
    pub fn obs(&self) -> &MachineObservation {
        &self.observed[self.selected].obs
    }
    pub fn source(&self) -> &Source {
        &self.observed[self.selected].source
    }
    pub fn shadowed(&self) -> Vec<String> {
        vec![] // STUB (S1-A)
    }
}

/// S1-A adds: BTreeMap<MachineId, BTreeMap<Source, (MachineObservation, Option<Instant>)>>, failing: BTreeSet<String>
#[derive(Default)]
pub struct MachineRegistry {}

impl MachineRegistry {
    /// Successful network refresh. Upserts each obs (a duplicate id within `obs`: the first wins, the rest are
    /// dropped; providers already warn, §7) with expires_at = now + max(obs.ttl.unwrap_or(ZERO), floor). Ids this
    /// provider reported earlier but not now are NOT removed; they age out. Clears `failing`. Returns ids that are
    /// new to the registry.
    pub fn apply_ok(
        &mut self,
        _src: &Source,
        _obs: Vec<MachineObservation>,
        _now: Instant,
        _floor: Duration,
    ) -> Vec<MachineId> {
        vec![] // STUB (S1-A)
    }
    /// Failed refresh: expire() skips this provider until its next apply_ok (freeze, not drop).
    pub fn mark_failed(&mut self, _provider: &str) {} // STUB (S1-A)
    /// Authoritative, no expiry: static observations on every config apply. Returns (new ids, gone ids).
    pub fn replace(
        &mut self,
        _src: &Source,
        _obs: Vec<MachineObservation>,
    ) -> (Vec<MachineId>, Vec<MachineId>) {
        (vec![], vec![]) // STUB (S1-A)
    }
    /// gone ids
    pub fn remove_provider(&mut self, _provider: &str) -> Vec<MachineId> {
        vec![] // STUB (S1-A)
    }
    /// gone ids (no next_expiry: E3)
    pub fn expire(&mut self, _now: Instant) -> Vec<MachineId> {
        vec![] // STUB (S1-A)
    }
    /// sorted by id
    pub fn machines(&self, _p: &Policy) -> Vec<Machine> {
        vec![] // STUB (S1-A)
    }
}
