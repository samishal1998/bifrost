//! Policy semantics (contract §4). STUB (S0.3): S1-A implements the bodies.

use crate::model::MachineObservation;
use crate::registry::{Observed, Source};
use crate::validate::{Cidr, Glob};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Match {
    pub ids: Vec<String>,
    pub names: Vec<Glob>,
    pub cidrs: Vec<Cidr>,
    pub tags: Vec<String>,
    pub providers: Vec<String>,
    pub metadata: BTreeMap<String, String>,
}

impl Match {
    pub fn is_empty(&self) -> bool {
        true // STUB (S1-A)
    }
    /// include/allow: every NON-EMPTY kind matches (AND), any entry within a kind (OR); metadata: all pairs equal.
    /// `own` = this Match is `src.provider`'s own filter: only then do `ids` also match `o.native_id` (A19).
    pub fn all(&self, _src: &Source, _o: &MachineObservation, _own: bool) -> bool {
        false // STUB (S1-A)
    }
    /// exclude/deny: any single primitive matches → Some("names=prod-*").
    /// cidrs fail CLOSED here: a non-static observation with no IP-literal address matches any cidr entry.
    pub fn any(&self, _src: &Source, _o: &MachineObservation, _own: bool) -> Option<String> {
        None // STUB (S1-A)
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProviderFilter {
    pub include: Match,
    pub exclude: Match,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy {
    pub allow: Match,
    pub deny: Match,
    /// keyed by provider name
    pub filters: BTreeMap<String, ProviderFilter>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    Allowed { by: String },
    DiscoverOnly,
    Denied { by: String },
}

impl Verdict {
    pub fn is_allowed(&self) -> bool {
        false // STUB (S1-A)
    }
}

/// "allowed (tailscale.filter.include)" | "discover-only" | "denied (policy.deny names=prod-*)"
impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}") // STUB (S1-A)
    }
}

/// §4. `observed` is in trust order. Returns the verdict and the index of the selected observation.
pub fn evaluate(_p: &Policy, _observed: &[Observed]) -> (Verdict, usize) {
    (Verdict::DiscoverOnly, 0) // STUB (S1-A): fails closed
}
