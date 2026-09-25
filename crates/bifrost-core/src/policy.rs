//! Policy semantics (contract §4).

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
        self.ids.is_empty()
            && self.names.is_empty()
            && self.cidrs.is_empty()
            && self.tags.is_empty()
            && self.providers.is_empty()
            && self.metadata.is_empty()
    }
    /// include/allow: every NON-EMPTY kind matches (AND), any entry within a kind (OR); metadata: all pairs equal.
    /// `own` = this Match is `src.provider`'s own filter: only then do `ids` also match `o.native_id` (A19).
    pub fn all(&self, src: &Source, o: &MachineObservation, own: bool) -> bool {
        (self.ids.is_empty() || self.ids.iter().any(|i| id_hit(i, o, own)))
            && (self.names.is_empty() || self.names.iter().any(|g| g.matches(o.id.as_str())))
            && (self.cidrs.is_empty() || self.cidrs.iter().any(|c| cidr_hit(c, o)))
            && (self.tags.is_empty() || self.tags.iter().any(|t| o.metadata.tags.contains(t)))
            && (self.providers.is_empty() || self.providers.iter().any(|p| provider_hit(p, src)))
            && (self.metadata.iter()).all(|(k, v)| o.metadata.values.get(k) == Some(v))
    }
    /// exclude/deny: any single primitive matches → Some("names=prod-*").
    /// cidrs fail CLOSED here: a non-static observation with no IP-literal address matches any cidr entry.
    pub fn any(&self, src: &Source, o: &MachineObservation, own: bool) -> Option<String> {
        let cidr = |c: &Cidr| format!("cidrs={}/{}", c.addr, c.prefix);
        let no_ip = src.trust != 0 && o.addresses.iter().all(|a| a.ip().is_none());
        (self.ids.iter().find(|i| id_hit(i, o, own)))
            .map(|i| format!("ids={i}"))
            .or_else(|| {
                let g = self.names.iter().find(|g| g.matches(o.id.as_str()))?;
                Some(format!("names={g}"))
            })
            .or_else(|| match self.cidrs.first() {
                Some(c) if no_ip => Some(format!("{} (no IP address)", cidr(c))),
                _ => self.cidrs.iter().find(|c| cidr_hit(c, o)).map(cidr),
            })
            .or_else(|| {
                let t = self.tags.iter().find(|t| o.metadata.tags.contains(*t))?;
                Some(format!("tags={t}"))
            })
            .or_else(|| {
                let p = self.providers.iter().find(|p| provider_hit(p, src))?;
                Some(format!("providers={p}"))
            })
            .or_else(|| {
                let (k, v) =
                    (self.metadata.iter()).find(|(k, v)| o.metadata.values.get(*k) == Some(v))?;
                Some(format!("metadata.{k}={v}"))
            })
    }
}

/// Global `ids` see the machine id only; a provider's own filter also sees its `native_id` (A19).
fn id_hit(i: &str, o: &MachineObservation, own: bool) -> bool {
    i == o.id.as_str() || (own && o.native_id.as_deref() == Some(i))
}

/// Hostnames are never resolved: only IP-literal addresses can be inside a cidr.
fn cidr_hit(c: &Cidr, o: &MachineObservation) -> bool {
    o.addresses
        .iter()
        .filter_map(|a| a.ip())
        .any(|ip| c.contains(ip))
}

fn provider_hit(p: &str, src: &Source) -> bool {
    p == src.provider || p == src.kind
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
        matches!(self, Self::Allowed { .. })
    }
}

/// "allowed (tailscale.filter.include)" | "discover-only" | "denied (policy.deny names=prod-*)"
impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Allowed { by } => write!(f, "allowed ({by})"),
            Self::DiscoverOnly => f.write_str("discover-only"),
            Self::Denied { by } => write!(f, "denied ({by})"),
        }
    }
}

/// §4. `observed` is in trust order. Returns the verdict and the index of the selected observation.
// ponytail: winner-takes-all merge, a DNS/HTTP include can't mount an id that a more trusted source reports without allowing it; a per-id `prefer = "<provider>"` rule
pub fn evaluate(p: &Policy, observed: &[Observed]) -> (Verdict, usize) {
    // a provider filter sees only its own provider's observation (own = true)
    let excluded = |o: &Observed| {
        let f = p.filters.get(&o.source.provider)?;
        f.exclude.any(&o.source, &o.obs, true)
    };
    // 1. an exclude drops only that observation; 2. the most trusted remaining one decides alone
    let Some(i) = observed.iter().position(|o| excluded(o).is_none()) else {
        let Some(o) = observed.first() else {
            return (Verdict::DiscoverOnly, 0); // no observation: nothing to allow
        };
        let by = format!(
            "{}.filter.exclude {}",
            o.source.provider,
            excluded(o).unwrap_or_default()
        );
        return (Verdict::Denied { by }, 0);
    };
    let (src, o) = (&observed[i].source, &observed[i].obs);
    let v = if let Some(prim) = p.deny.any(src, o, false) {
        Verdict::Denied {
            by: format!("policy.deny {prim}"),
        }
    } else if src.trust == 0 {
        Verdict::Allowed {
            by: src.kind.clone(),
        }
    } else if p
        .filters
        .get(&src.provider)
        .is_some_and(|f| !f.include.is_empty() && f.include.all(src, o, true))
    {
        Verdict::Allowed {
            by: format!("{}.filter.include", src.provider),
        }
    } else if !p.allow.is_empty() && p.allow.all(src, o, false) {
        Verdict::Allowed {
            by: "policy.allow".into(),
        }
    } else {
        Verdict::DiscoverOnly
    };
    (v, i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::obs;

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
    fn dns() -> Source {
        src(3, "dns", "infra")
    }
    fn o(s: Source, obs: MachineObservation) -> Observed {
        Observed {
            source: s,
            obs,
            expires_at: None,
        }
    }
    fn tagged(id: &str, addr: &str, tags: &[&str]) -> MachineObservation {
        let mut m = obs(id, addr);
        m.metadata.tags = tags.iter().map(|t| t.to_string()).collect();
        m
    }
    fn native(id: &str, nid: &str) -> MachineObservation {
        let mut m = obs(id, "10.0.0.1");
        m.native_id = Some(nid.into());
        m
    }
    fn globs(g: &[&str]) -> Vec<Glob> {
        g.iter().map(|g| Glob::parse(g).unwrap()).collect()
    }
    fn cidrs(c: &[&str]) -> Vec<Cidr> {
        c.iter().map(|c| Cidr::parse(c).unwrap()).collect()
    }
    fn strs(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }
    fn include(provider: &str, m: Match) -> BTreeMap<String, ProviderFilter> {
        BTreeMap::from([(
            provider.to_string(),
            ProviderFilter {
                include: m,
                ..Default::default()
            },
        )])
    }
    fn eval(p: &Policy, obs: Vec<Observed>) -> String {
        evaluate(p, &obs).0.to_string()
    }

    #[test]
    fn deny_wins_over_allow() {
        let p = Policy {
            allow: Match {
                names: globs(&["agent-*"]),
                ..Default::default()
            },
            deny: Match {
                tags: strs(&["prod"]),
                ..Default::default()
            },
            filters: include(
                "tailscale",
                Match {
                    names: globs(&["*"]),
                    ..Default::default()
                },
            ),
        };
        let v = evaluate(&p, &[o(ts(), tagged("agent-01", "10.0.0.1", &["prod"]))]);
        assert_eq!(
            v,
            (
                Verdict::Denied {
                    by: "policy.deny tags=prod".into()
                },
                0
            )
        );
        assert!(!v.0.is_allowed());
        assert_eq!(v.0.to_string(), "denied (policy.deny tags=prod)");
        let v = evaluate(&p, &[o(ts(), tagged("agent-01", "10.0.0.1", &["dev"]))]).0;
        assert!(v.is_allowed());
        assert_eq!(v.to_string(), "allowed (tailscale.filter.include)");
    }

    #[test]
    fn global_deny_beats_static() {
        let p = Policy {
            deny: Match {
                names: globs(&["prod-*"]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(stat(), obs("prod-db", "db.lan"))]),
            "denied (policy.deny names=prod-*)"
        );
        assert_eq!(
            eval(&p, vec![o(stat(), obs("build", "db.lan"))]),
            "allowed (static)"
        );
    }

    #[test]
    fn provider_exclude_drops_only_that_observation() {
        let mut filters = include(
            "infra",
            Match {
                names: globs(&["*"]),
                ..Default::default()
            },
        );
        filters.insert(
            "tailscale".into(),
            ProviderFilter {
                exclude: Match {
                    names: globs(&["x"]),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let p = Policy {
            filters,
            ..Default::default()
        };
        // tailscale's own exclude drops only its observation; the DNS one is selected and judged on its own
        let both = [
            o(ts(), obs("x", "100.64.0.1")),
            o(dns(), obs("x", "10.0.0.9")),
        ];
        assert_eq!(
            evaluate(&p, &both),
            (
                Verdict::Allowed {
                    by: "infra.filter.include".into()
                },
                1
            )
        );
        assert_eq!(
            evaluate(&p, &both[..1]),
            (
                Verdict::Denied {
                    by: "tailscale.filter.exclude names=x".into()
                },
                0
            )
        );
        // the tailscale exclude never applies to the DNS observation
        assert_eq!(
            evaluate(&p, &both[1..]).0.to_string(),
            "allowed (infra.filter.include)"
        );
    }

    #[test]
    fn no_allow_rule_is_discover_only() {
        let p = Policy::default();
        let v = evaluate(&p, &[o(ts(), obs("agent-01", "100.64.0.1"))]);
        assert_eq!(v, (Verdict::DiscoverOnly, 0));
        assert_eq!(v.0.to_string(), "discover-only");
        assert!(!v.0.is_allowed());
        // an empty include never allows
        let p = Policy {
            filters: include("tailscale", Match::default()),
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(ts(), obs("agent-01", "100.64.0.1"))]),
            "discover-only"
        );
        assert_eq!(evaluate(&p, &[]).0, Verdict::DiscoverOnly);
    }

    #[test]
    fn static_allowed_by_default() {
        let v = evaluate(&Policy::default(), &[o(stat(), obs("build", "10.0.0.18"))]);
        assert_eq!(
            v,
            (
                Verdict::Allowed {
                    by: "static".into()
                },
                0
            )
        );
        assert_eq!(v.0.to_string(), "allowed (static)");
    }

    #[test]
    fn include_kinds_anded_values_ored() {
        let p = Policy {
            filters: include(
                "tailscale",
                Match {
                    names: globs(&["a-*", "b-*"]),
                    tags: strs(&["dev", "ci"]),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        let e = |id: &str, tags: &[&str]| eval(&p, vec![o(ts(), tagged(id, "10.0.0.1", tags))]);
        assert_eq!(e("a-1", &["dev"]), "allowed (tailscale.filter.include)");
        assert_eq!(e("b-1", &["ci", "x"]), "allowed (tailscale.filter.include)");
        assert_eq!(e("a-1", &[]), "discover-only");
        assert_eq!(e("a-1", &["x"]), "discover-only");
        assert_eq!(e("c-1", &["dev"]), "discover-only");
    }

    #[test]
    fn include_metadata_all_pairs_deny_metadata_any() {
        let pairs = |p: &[(&str, &str)]| -> BTreeMap<String, String> {
            p.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        };
        let with = |p: &[(&str, &str)]| {
            let mut m = obs("agent-01", "10.0.0.1");
            m.metadata.values = pairs(p);
            m
        };
        let p = Policy {
            deny: Match {
                metadata: pairs(&[("env", "prod"), ("team", "y")]),
                ..Default::default()
            },
            filters: include(
                "tailscale",
                Match {
                    metadata: pairs(&[("env", "dev"), ("team", "x")]),
                    ..Default::default()
                },
            ),
            ..Default::default()
        };
        let e = |m| eval(&p, vec![o(ts(), m)]);
        assert_eq!(
            e(with(&[("env", "dev"), ("team", "x"), ("z", "1")])),
            "allowed (tailscale.filter.include)"
        );
        assert_eq!(e(with(&[("env", "dev")])), "discover-only");
        assert_eq!(e(with(&[("env", "dev"), ("team", "z")])), "discover-only");
        assert_eq!(
            e(with(&[("env", "dev"), ("team", "y")])),
            "denied (policy.deny metadata.team=y)"
        );
        assert_eq!(
            e(with(&[("env", "prod")])),
            "denied (policy.deny metadata.env=prod)"
        );
    }

    #[test]
    fn cidr_deny_fails_closed_without_ip() {
        let p = Policy {
            deny: Match {
                cidrs: cidrs(&["10.0.0.0/8"]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(dns(), obs("agent-01", "agent-01.example.com"))]),
            "denied (policy.deny cidrs=10.0.0.0/8 (no IP address))"
        );
        assert_eq!(
            eval(&p, vec![o(ts(), obs("agent-01", "10.1.2.3"))]),
            "denied (policy.deny cidrs=10.0.0.0/8)"
        );
        assert_eq!(
            eval(&p, vec![o(ts(), obs("agent-01", "192.168.1.1"))]),
            "discover-only"
        );
        // any IP-literal address is checked, not only the connect target
        let mut m = obs("agent-01", "agent-01.ts.net");
        m.addresses.push(crate::Host::parse("10.9.9.9").unwrap());
        assert_eq!(
            eval(&p, vec![o(ts(), m)]),
            "denied (policy.deny cidrs=10.0.0.0/8)"
        );
    }

    #[test]
    fn cidr_deny_static_hostname_not_matched() {
        let p = Policy {
            deny: Match {
                cidrs: cidrs(&["10.0.0.0/8"]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(stat(), obs("build", "build.lan"))]),
            "allowed (static)"
        );
        assert_eq!(
            eval(&p, vec![o(stat(), obs("build", "10.0.0.18"))]),
            "denied (policy.deny cidrs=10.0.0.0/8)"
        );
    }

    #[test]
    fn cidr_allow_requires_ip() {
        let p = Policy {
            allow: Match {
                cidrs: cidrs(&["100.64.0.0/10"]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(ts(), obs("a", "a.tail.ts.net"))]),
            "discover-only"
        );
        assert_eq!(
            eval(&p, vec![o(ts(), obs("a", "100.64.0.5"))]),
            "allowed (policy.allow)"
        );
        let mut m = obs("a", "a.tail.ts.net");
        m.addresses.push(crate::Host::parse("100.64.0.5").unwrap());
        assert_eq!(eval(&p, vec![o(ts(), m)]), "allowed (policy.allow)");
        assert_eq!(
            eval(&p, vec![o(ts(), obs("a", "fd7a::1"))]),
            "discover-only"
        );
    }

    #[test]
    fn providers_matches_name_or_kind() {
        let m = obs("agent-01", "10.0.0.1");
        let by_kind = Match {
            providers: strs(&["dns"]),
            ..Default::default()
        };
        let by_name = Match {
            providers: strs(&["infra"]),
            ..Default::default()
        };
        assert!(by_kind.all(&dns(), &m, false));
        assert!(by_name.all(&dns(), &m, false));
        assert!(!by_kind.all(&ts(), &m, false));
        assert_eq!(
            by_kind.any(&dns(), &m, false).as_deref(),
            Some("providers=dns")
        );
        assert_eq!(
            by_name.any(&dns(), &m, false).as_deref(),
            Some("providers=infra")
        );
        assert_eq!(by_name.any(&ts(), &m, false), None);
        let p = Policy {
            allow: by_kind,
            deny: Match {
                providers: strs(&["tailscale"]),
                ..Default::default()
            },
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(dns(), m.clone())]),
            "allowed (policy.allow)"
        );
        assert_eq!(
            eval(&p, vec![o(ts(), m)]),
            "denied (policy.deny providers=tailscale)"
        );
    }

    #[test]
    fn global_ids_match_machine_id_only() {
        let ids = Match {
            ids: strs(&["agent-07"]),
            ..Default::default()
        };
        let p = Policy {
            allow: ids.clone(),
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(ts(), obs("agent-07", "10.0.0.1"))]),
            "allowed (policy.allow)"
        );
        // a DNS record publishing native id=agent-07 does not satisfy the global allow (A19)
        assert_eq!(
            eval(&p, vec![o(dns(), native("evil", "agent-07"))]),
            "discover-only"
        );
        assert!(!ids.all(&dns(), &native("evil", "agent-07"), false));
        let p = Policy {
            deny: ids,
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(stat(), native("build", "agent-07"))]),
            "allowed (static)"
        );
        assert_eq!(
            eval(&p, vec![o(stat(), obs("agent-07", "10.0.0.1"))]),
            "denied (policy.deny ids=agent-07)"
        );
    }

    #[test]
    fn native_id_scoped_to_owning_provider() {
        let nid = Match {
            ids: strs(&["nTS123CNTRL"]),
            ..Default::default()
        };
        let mut filters = include("tailscale", nid.clone());
        filters.insert(
            "infra".into(),
            ProviderFilter {
                exclude: nid.clone(),
                ..Default::default()
            },
        );
        let p = Policy {
            filters,
            ..Default::default()
        };
        assert_eq!(
            eval(&p, vec![o(ts(), native("agent-01", "nTS123CNTRL"))]),
            "allowed (tailscale.filter.include)"
        );
        // a DNS record publishing the tailscale node id is not matched by tailscale's include (A19),
        // only by its own provider's exclude
        let hostile = [o(dns(), native("evil", "nTS123CNTRL"))];
        assert_eq!(
            evaluate(&p, &hostile),
            (
                Verdict::Denied {
                    by: "infra.filter.exclude ids=nTS123CNTRL".into()
                },
                0
            )
        );
        let p = Policy {
            filters: include("tailscale", nid),
            ..Default::default()
        };
        assert_eq!(evaluate(&p, &hostile).0, Verdict::DiscoverOnly);
    }
}
