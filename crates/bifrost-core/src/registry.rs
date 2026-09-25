//! Machine registry: merges observations by trust, ages them out.

use crate::model::MachineObservation;
use crate::policy::{Policy, Verdict, evaluate};
use crate::validate::MachineId;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// Ord == trust order: `trust` (lower = more trusted; 0 = static), then kind, then provider name.
/// Ranks are assigned by bifrost-config (static 0, tailscale 1, http 2, dns 3); core only compares numbers (B8).
// ponytail: same-kind providers tie-break by provider name, not config order (two DNS providers reporting one id: the alphabetically first wins); carry a config rank in Source
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
    /// Provider names of the observations that lost to the selected one, in trust order.
    pub fn shadowed(&self) -> Vec<String> {
        (self.observed.iter().enumerate())
            .filter(|(i, _)| *i != self.selected)
            .map(|(_, o)| o.source.provider.clone())
            .collect()
    }
}

type Machines = BTreeMap<MachineId, BTreeMap<Source, (MachineObservation, Option<Instant>)>>;

#[derive(Default)]
pub struct MachineRegistry {
    machines: Machines,
    /// providers whose last refresh failed: their observations don't expire (freeze, not drop)
    failing: BTreeSet<String>,
}

impl MachineRegistry {
    /// Successful network refresh. Upserts each obs (a duplicate id within `obs`: the first wins, the rest are
    /// dropped; providers already warn, §7) with expires_at = now + max(obs.ttl.unwrap_or(ZERO), floor). Ids this
    /// provider reported earlier but not now are NOT removed; they age out. Clears `failing`. Returns ids that are
    /// new to the registry.
    pub fn apply_ok(
        &mut self,
        src: &Source,
        obs: Vec<MachineObservation>,
        now: Instant,
        floor: Duration,
    ) -> Vec<MachineId> {
        self.failing.remove(&src.provider);
        let before = self.ids();
        self.upsert(src, obs, |o| {
            // a hostile TTL can't overflow Instant and panic the actor
            let d = o.ttl.unwrap_or_default().max(floor);
            Some(now.checked_add(d).unwrap_or(now + floor))
        });
        self.ids().difference(&before).cloned().collect()
    }
    /// Failed refresh: expire() skips this provider until its next apply_ok (freeze, not drop).
    // ponytail: a provider that fails permanently freezes its last view forever (its machines stay listed and mounted until it recovers or leaves the config); expire frozen observations after a long cap such as offline_grace_period
    pub fn mark_failed(&mut self, provider: &str) {
        self.failing.insert(provider.to_string());
    }
    /// Authoritative, no expiry: static observations on every config apply. Returns (new ids, gone ids).
    pub fn replace(
        &mut self,
        src: &Source,
        obs: Vec<MachineObservation>,
    ) -> (Vec<MachineId>, Vec<MachineId>) {
        let before = self.ids();
        retain(&mut self.machines, |s, _| s.provider != src.provider);
        self.upsert(src, obs, |_| None);
        let after = self.ids();
        (
            after.difference(&before).cloned().collect(),
            before.difference(&after).cloned().collect(),
        )
    }
    /// gone ids
    pub fn remove_provider(&mut self, provider: &str) -> Vec<MachineId> {
        self.failing.remove(provider);
        retain(&mut self.machines, |s, _| s.provider != provider)
    }
    /// gone ids (no next_expiry: E3)
    // ponytail: removal hysteresis is the 3 x interval floor with no grace tombstones (a machine absent from successful refreshes for 3 intervals is unmounted gracefully); tombstones for offline_grace_period if inventories flap longer
    pub fn expire(&mut self, now: Instant) -> Vec<MachineId> {
        let failing = &self.failing;
        retain(&mut self.machines, |s, exp| {
            failing.contains(&s.provider) || exp.is_none_or(|e| e > now)
        })
    }
    /// sorted by id
    pub fn machines(&self, p: &Policy) -> Vec<Machine> {
        (self.machines.iter())
            .map(|(id, srcs)| {
                let observed: Vec<Observed> = (srcs.iter())
                    .map(|(s, (o, e))| Observed {
                        source: s.clone(),
                        obs: o.clone(),
                        expires_at: *e,
                    })
                    .collect();
                let (verdict, selected) = evaluate(p, &observed);
                Machine {
                    id: id.clone(),
                    observed,
                    selected,
                    verdict,
                }
            })
            .collect()
    }

    fn ids(&self) -> BTreeSet<MachineId> {
        self.machines.keys().cloned().collect()
    }

    fn upsert(
        &mut self,
        src: &Source,
        obs: Vec<MachineObservation>,
        expires: impl Fn(&MachineObservation) -> Option<Instant>,
    ) {
        let mut seen = BTreeSet::new();
        for o in obs.into_iter().filter(|o| seen.insert(o.id.clone())) {
            let e = expires(&o);
            let srcs = self.machines.entry(o.id.clone()).or_default();
            srcs.insert(src.clone(), (o, e));
        }
    }
}

/// Keeps the observations `keep` accepts; returns (sorted) the ids of machines left with none, which are dropped.
fn retain(m: &mut Machines, keep: impl Fn(&Source, &Option<Instant>) -> bool) -> Vec<MachineId> {
    let mut gone = vec![];
    m.retain(|id, srcs| {
        srcs.retain(|s, (_, e)| keep(s, e));
        if srcs.is_empty() {
            gone.push(id.clone());
        }
        !srcs.is_empty()
    });
    gone
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::obs;
    use crate::policy::{Match, ProviderFilter};
    use crate::validate::Glob;
    use std::collections::BTreeMap;

    fn src(trust: u8, kind: &str, provider: &str) -> Source {
        Source {
            trust,
            kind: kind.into(),
            provider: provider.into(),
        }
    }
    fn stat() -> Source {
        src(0, "static", "static")
    }
    fn ts() -> Source {
        src(1, "tailscale", "tailscale")
    }
    fn http() -> Source {
        src(2, "http", "inventory")
    }
    fn dns() -> Source {
        src(3, "dns", "dns")
    }
    fn ids(v: &[&str]) -> Vec<MachineId> {
        v.iter().map(|s| MachineId::parse(s).unwrap()).collect()
    }
    fn names(r: &MachineRegistry) -> Vec<String> {
        r.machines(&Policy::default())
            .iter()
            .map(|m| m.id.as_str().to_string())
            .collect()
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
    const FLOOR: Duration = Duration::from_secs(90);

    #[test]
    fn trust_order_static_tailscale_http_dns() {
        let now = Instant::now();
        let mut r = MachineRegistry::default();
        assert_eq!(
            r.apply_ok(&dns(), vec![obs("x", "10.0.0.4")], now, FLOOR),
            ids(&["x"])
        );
        assert_eq!(
            r.apply_ok(&http(), vec![obs("x", "10.0.0.3")], now, FLOOR),
            ids(&[])
        );
        r.apply_ok(&ts(), vec![obs("x", "10.0.0.2")], now, FLOOR);
        assert_eq!(
            r.replace(&stat(), vec![obs("x", "10.0.0.1")]),
            (ids(&[]), ids(&[]))
        );
        let m = &r.machines(&Policy::default())[0];
        let order: Vec<_> = m.observed.iter().map(|o| o.source.trust).collect();
        assert_eq!(order, [0, 1, 2, 3]);
        assert_eq!((m.selected, m.source()), (0, &stat()));
        assert_eq!(m.obs().addresses[0].as_str(), "10.0.0.1");
        assert_eq!(
            m.verdict,
            Verdict::Allowed {
                by: "static".into()
            }
        );
        assert_eq!(m.shadowed(), ["tailscale", "inventory", "dns"]);
    }

    #[test]
    fn dns_cannot_redirect_tailscale_machine() {
        let now = Instant::now();
        let mut r = MachineRegistry::default();
        r.apply_ok(&ts(), vec![obs("agent-01", "100.64.0.1")], now, FLOOR);
        let mut evil = obs("agent-01", "10.0.0.66");
        evil.port = Some(2222);
        r.apply_ok(&dns(), vec![evil], now, FLOOR);
        // DNS's include matches, yet tailscale's observation is selected and decides (DiscoverOnly)
        let m = &r.machines(&include_all("dns"))[0];
        assert_eq!(m.source(), &ts());
        assert_eq!(m.obs().addresses[0].as_str(), "100.64.0.1");
        assert_eq!(m.obs().port, None);
        assert_eq!(m.verdict, Verdict::DiscoverOnly);
        assert_eq!(m.shadowed(), ["dns"]);
    }

    #[test]
    fn dns_tags_cannot_deny_static_machine() {
        let mut r = MachineRegistry::default();
        r.replace(&stat(), vec![obs("build", "10.0.0.18")]);
        let mut tagged = obs("build", "10.0.0.18");
        tagged.metadata.tags.insert("prod".into());
        r.apply_ok(&dns(), vec![tagged], Instant::now(), FLOOR);
        let p = Policy {
            deny: Match {
                tags: vec!["prod".into()],
                ..Default::default()
            },
            ..Default::default()
        };
        let m = &r.machines(&p)[0];
        assert_eq!(
            m.verdict,
            Verdict::Allowed {
                by: "static".into()
            }
        );
        assert!(m.obs().metadata.tags.is_empty());
    }

    #[test]
    fn dup_within_provider_first_wins() {
        let mut r = MachineRegistry::default();
        let new = r.apply_ok(
            &ts(),
            vec![
                obs("a", "10.0.0.1"),
                obs("a", "10.0.0.2"),
                obs("b", "10.0.0.3"),
            ],
            Instant::now(),
            FLOOR,
        );
        assert_eq!(new, ids(&["a", "b"]));
        let ms = r.machines(&Policy::default());
        assert_eq!(ms[0].observed.len(), 1);
        assert_eq!(ms[0].obs().addresses[0].as_str(), "10.0.0.1");
    }

    #[test]
    fn absent_ages_out_not_removed() {
        let t0 = Instant::now();
        let mut r = MachineRegistry::default();
        r.apply_ok(
            &ts(),
            vec![obs("a", "10.0.0.1"), obs("b", "10.0.0.2")],
            t0,
            FLOOR,
        );
        let t1 = t0 + Duration::from_secs(30);
        assert_eq!(
            r.apply_ok(&ts(), vec![obs("a", "10.0.0.1")], t1, FLOOR),
            ids(&[])
        );
        assert_eq!(names(&r), ["a", "b"]);
        assert_eq!(r.expire(t0 + Duration::from_secs(89)), ids(&[]));
        assert_eq!(r.expire(t0 + FLOOR), ids(&["b"]));
        assert_eq!(names(&r), ["a"]);
        assert_eq!(r.expire(t1 + FLOOR), ids(&["a"]));
        assert!(names(&r).is_empty());
    }

    #[test]
    fn failed_provider_freezes_expiry() {
        let t0 = Instant::now();
        let mut r = MachineRegistry::default();
        r.apply_ok(&ts(), vec![obs("a", "10.0.0.1")], t0, FLOOR);
        r.apply_ok(&dns(), vec![obs("d", "10.0.0.4")], t0, FLOOR);
        r.mark_failed("tailscale");
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(r.expire(later), ids(&["d"]));
        assert_eq!(names(&r), ["a"]);
    }

    #[test]
    fn recovery_expires_long_absent() {
        let t0 = Instant::now();
        let mut r = MachineRegistry::default();
        r.apply_ok(
            &ts(),
            vec![obs("a", "10.0.0.1"), obs("b", "10.0.0.2")],
            t0,
            FLOOR,
        );
        r.mark_failed("tailscale");
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(r.expire(later), ids(&[]));
        // recovery: a is back and refreshed, b has been absent far longer than the floor
        r.apply_ok(&ts(), vec![obs("a", "10.0.0.1")], later, FLOOR);
        assert_eq!(r.expire(later), ids(&["b"]));
        assert_eq!(names(&r), ["a"]);
    }

    #[test]
    fn ttl_floor_three_intervals() {
        let t0 = Instant::now();
        let floor = 3 * Duration::from_secs(30);
        let mut r = MachineRegistry::default();
        let with_ttl = |id: &str, ttl: Option<u64>| {
            let mut o = obs(id, "10.0.0.1");
            o.ttl = ttl.map(Duration::from_secs);
            o
        };
        r.apply_ok(
            &dns(),
            vec![
                with_ttl("a", Some(5)),
                with_ttl("b", Some(300)),
                with_ttl("c", None),
            ],
            t0,
            floor,
        );
        let exp: Vec<_> = r
            .machines(&Policy::default())
            .iter()
            .map(|m| m.observed[0].expires_at)
            .collect();
        assert_eq!(
            exp,
            [
                Some(t0 + floor),
                Some(t0 + Duration::from_secs(300)),
                Some(t0 + floor)
            ]
        );
    }

    #[test]
    fn replace_is_authoritative() {
        let mut r = MachineRegistry::default();
        assert_eq!(
            r.replace(&stat(), vec![obs("a", "10.0.0.1"), obs("b", "10.0.0.2")]),
            (ids(&["a", "b"]), ids(&[]))
        );
        let t0 = Instant::now();
        r.apply_ok(&ts(), vec![obs("a", "100.64.0.1")], t0, FLOOR);
        assert_eq!(
            r.replace(&stat(), vec![obs("b", "10.0.0.2"), obs("c", "10.0.0.3")]),
            (ids(&["c"]), ids(&[]))
        );
        let ms = r.machines(&Policy::default());
        assert_eq!(ms[0].source(), &ts()); // a is still reported by tailscale
        assert_eq!(r.replace(&stat(), vec![]), (ids(&[]), ids(&["b", "c"])));
        // static observations never expire
        r.replace(&stat(), vec![obs("s", "10.0.0.9")]);
        assert_eq!(r.expire(t0 + Duration::from_secs(1 << 30)), ids(&["a"]));
        assert_eq!(names(&r), ["s"]);
    }

    #[test]
    fn remove_provider_returns_gone() {
        let t0 = Instant::now();
        let mut r = MachineRegistry::default();
        r.apply_ok(
            &ts(),
            vec![obs("a", "10.0.0.1"), obs("b", "10.0.0.2")],
            t0,
            FLOOR,
        );
        r.apply_ok(&dns(), vec![obs("b", "10.0.0.2")], t0, FLOOR);
        r.mark_failed("tailscale");
        assert_eq!(r.remove_provider("tailscale"), ids(&["a"]));
        assert_eq!(names(&r), ["b"]);
        assert_eq!(r.machines(&Policy::default())[0].source(), &dns());
        assert_eq!(r.remove_provider("nope"), ids(&[]));
        // a re-added provider of the same name is not frozen by the old failure
        r.apply_ok(&ts(), vec![obs("z", "10.0.0.5")], t0, FLOOR);
        assert_eq!(r.expire(t0 + FLOOR), ids(&["b", "z"]));
    }
}
