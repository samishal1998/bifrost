//! DNS TXT `bf1` provider (contract §7). The index `_bifrost.<domain>.` lists node labels in `nodes=`; each
//! node publishes one `_bifrost.<node>.<domain>.` record. Every TXT byte is untrusted: it goes through
//! `parse_bf1` (and the core validators) before it can reach an observation, and a bad record is skipped
//! whole, never partially applied.

use bifrost_core::validate::{clean, native_id, tag};
use bifrost_core::{
    BoxFuture, DiscoveryError, DiscoveryProvider, Host, Invalid, MachineObservation, Metadata,
    MountHints, Name, RemotePath, User,
};
use hickory_resolver::config::{NameServerConfig, ResolverConfig};
use hickory_resolver::lookup::Lookup;
use hickory_resolver::net::NetError;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use hickory_resolver::proto::rr::RData;
use hickory_resolver::{Resolver, TokioResolver};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tracing::warn;

/// `nodes=` entries across all index RRs
const MAX_NODES: usize = 256;

pub struct DnsProvider {
    name: String,
    domain: Host,
    resolver: TokioResolver,
}

impl DnsProvider {
    pub fn new(name: String, domain: Host, nameservers: Vec<SocketAddr>) -> Result<Self, String> {
        let b = if nameservers.is_empty() {
            TokioResolver::builder_tokio().map_err(|e| e.to_string())? // C8: NetError → String
        } else {
            let ns = nameservers
                .iter()
                .map(|sa| {
                    let mut n = NameServerConfig::udp_and_tcp(sa.ip());
                    for c in &mut n.connections {
                        c.port = sa.port();
                    }
                    n
                })
                .collect();
            Resolver::builder_with_config(
                ResolverConfig::from_name_servers(ns),
                TokioRuntimeProvider::default(),
            )
        };
        // no options_mut() lines (D1): ResolverOpts::default() is already 5s timeout × 2 attempts.
        // The cache stays on and honours the record TTLs.
        let resolver = b.build().map_err(|e| e.to_string())?;
        Ok(Self {
            name,
            domain,
            resolver,
        })
    }
}

/// `fqdn` ends in '.', so resolv.conf search domains never apply. `from_ascii`, not IDNA: every label is
/// already validated ASCII (a domain label may contain '_').
async fn txt(r: &TokioResolver, fqdn: &str) -> Result<Lookup, NetError> {
    r.txt_lookup(hickory_resolver::proto::rr::Name::from_ascii(fqdn)?)
        .await
}

impl DiscoveryProvider for DnsProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>> {
        Box::pin(async move {
            let (provider, d) = (self.name.as_str(), self.domain.as_str());
            let q = format!("_bifrost.{d}.");
            let index = match txt(&self.resolver, &q).await {
                Ok(l) => l,
                // an empty view, not a failure: with the expiry registry this causes no churn
                Err(e) if e.is_no_records_found() => {
                    warn!(provider, record = q, "no bf1 index record");
                    return Ok(vec![]);
                }
                Err(e) => return Err(DiscoveryError::Failed(clean(&e.to_string(), 512))),
            };
            let nodes = index_nodes(provider, &index);
            if nodes.is_empty() {
                warn!(provider, record = q, "no valid bf1 index record");
                return Ok(vec![]);
            }
            let mut set = JoinSet::new();
            for n in nodes {
                let (r, q) = (self.resolver.clone(), format!("_bifrost.{n}.{d}."));
                set.spawn(async move {
                    let l = txt(&r, &q).await.map_err(|e| e.to_string());
                    (n, q, l)
                });
            }
            let mut out = Vec::new();
            while let Some(j) = set.join_next().await {
                let (n, q, l) = j.map_err(|e| DiscoveryError::Failed(e.to_string()))?;
                let o =
                    l.and_then(|l| node(&n, &self.domain, index.valid_until(), &l, Instant::now()));
                match o {
                    Ok(o) => out.push(o),
                    // a skipped node is not refreshed and ages out
                    // ponytail: skipped records are only logged, invisible from the CLI and TUI; `warnings` in ProviderDto
                    Err(e) => warn!(
                        provider,
                        record = q,
                        reason = clean(&e, 512),
                        "bf1 node skipped"
                    ),
                }
            }
            out.sort_by(|a, b| a.id.cmp(&b.id)); // ids are distinct: one per deduplicated label
            Ok(out)
        })
    }
}

/// no driver (E2)
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bf1 {
    pub nodes: Vec<String>,
    pub host: Option<Host>,
    pub port: Option<u16>,
    pub user: Option<User>,
    pub tags: Vec<String>,
    pub path: Option<RemotePath>,
    pub id: Option<String>,
}

/// ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$ (no dots). Lives here, not in core (B8).
/// No lowercasing: the label is the machine id verbatim.
pub fn dns_label(s: &str) -> Result<String, Invalid> {
    let b = s.as_bytes();
    let an = |c: &u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    let ok = b.len() <= 63
        && b.first().is_some_and(an)
        && b.last().is_some_and(an)
        && b.iter().all(|c| an(c) || *c == b'-');
    ok.then(|| s.to_string()).ok_or_else(|| Invalid {
        what: "dns label",
        value: s.to_string(),
        why: "must match [a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?",
    })
}

/// Ok(None) = not a bf1 record (ignore silently)
///
/// ```text
/// S := concat(character-strings), ≤ 2048 bytes;  record := token *(1*SP token);  token := key "=" value
/// key := 1*32 [a-z0-9_-];  value := 1*256 (%x21-7E except '"');  first token "v=bf1" else Ok(None)
/// ```
/// Missing '=', a bad key or value, a duplicate key or a known key failing validation → Err (record invalid).
/// Unknown keys are ignored, `driver=` included (E2).
pub fn parse_bf1(txt: &str) -> Result<Option<Bf1>, String> {
    let mut toks = txt.split(' ');
    if toks.next() != Some("v=bf1") {
        return Ok(None); // SPF, a future v=bf2, …
    }
    if txt.len() > 2048 {
        return Err(format!("{} bytes (max 2048)", txt.len()));
    }
    if txt.ends_with(' ') {
        return Err("trailing space".into());
    }
    let inv = |e: Invalid| e.to_string();
    let mut seen = BTreeSet::from(["v"]);
    let mut r = Bf1::default();
    for t in toks.filter(|t| !t.is_empty()) {
        let (k, v) = t
            .split_once('=')
            .ok_or_else(|| format!("token {t:?} has no '='"))?;
        let key_ok = k.len() <= 32
            && !k.is_empty()
            && k.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-');
        let val_ok =
            v.len() <= 256 && !v.is_empty() && v.bytes().all(|c| c.is_ascii_graphic() && c != b'"');
        if !key_ok || !val_ok {
            return Err(format!("bad token {t:?}"));
        }
        if !seen.insert(k) {
            return Err(format!("duplicate key {k:?}"));
        }
        match k {
            "nodes" => {
                r.nodes = v
                    .split(',')
                    .map(dns_label)
                    .collect::<Result<_, _>>()
                    .map_err(inv)?
            }
            "host" => r.host = Some(Host::parse(v).map_err(inv)?),
            "port" => {
                let p = v.parse().ok().filter(|p| *p > 0 && v.len() <= 5);
                let digits = v.bytes().all(|c| c.is_ascii_digit()); // u16::from_str takes "+22"
                r.port = Some(
                    p.filter(|_| digits)
                        .ok_or_else(|| format!("bad port {v:?}"))?,
                );
            }
            "user" => r.user = Some(User::parse(v).map_err(inv)?),
            "path" => r.path = Some(RemotePath::parse(v).map_err(inv)?),
            "tags" => {
                r.tags = v
                    .split(',')
                    .map(tag)
                    .collect::<Result<_, _>>()
                    .map_err(inv)?;
                if r.tags.len() > 32 {
                    return Err(format!("{} tags (max 32)", r.tags.len()));
                }
            }
            "id" => r.id = Some(native_id(v).map_err(inv)?),
            _ => {} // unknown, driver= included (E2)
        }
    }
    Ok(Some(r))
}

/// pure. Identity is always the node label; `id=` is only the native id.
pub fn node_observation(
    label: &str,
    domain: &Host,
    r: &Bf1,
    ttl: Duration,
) -> Result<MachineObservation, String> {
    let inv = |e: Invalid| e.to_string();
    let id = Name::parse(&dns_label(label).map_err(inv)?).map_err(inv)?;
    let host = match &r.host {
        Some(h) => h.clone(),
        None => Host::parse(&format!("{label}.{}", domain.as_str())).map_err(inv)?,
    };
    Ok(MachineObservation {
        id,
        name: label.to_string(),
        native_id: r.id.clone(),
        addresses: vec![host],
        port: r.port,
        online: None,
        metadata: Metadata {
            tags: r.tags.iter().cloned().collect(),
            values: Default::default(),
        },
        hints: MountHints {
            user: r.user.clone(),
            path: r.path.clone(),
        },
        ttl: Some(ttl),
    })
}

/// Every TXT RR of `l`, its character-strings concatenated. Lossy UTF-8: a bad byte then fails the bf1
/// value grammar, and a non-bf1 record stays ignored.
fn txts(l: &Lookup) -> Vec<String> {
    let s = |r: &RData| match r {
        RData::TXT(t) => Some(String::from_utf8_lossy(&t.txt_data.concat()).into_owned()),
        _ => None,
    };
    l.answers().iter().filter_map(|r| s(&r.data)).collect()
}

/// The union of `nodes=` over the valid bf1 index RRs: sorted, deduplicated, at most MAX_NODES.
/// An invalid index RR is skipped with a warning; the others still count.
// ponytail: inline root TXT machine records (PRD §6.3 first form) and DNSSEC are not supported, publishers must use index + node records; parse `host=` at the index
fn index_nodes(provider: &str, index: &Lookup) -> Vec<String> {
    let record = index.query().name().to_string();
    let mut nodes = BTreeSet::new();
    for s in txts(index) {
        match parse_bf1(&s) {
            Ok(r) => nodes.extend(r.into_iter().flat_map(|r| r.nodes)),
            Err(e) => warn!(
                provider,
                record,
                reason = clean(&e, 512),
                "bf1 index record skipped"
            ),
        }
    }
    if nodes.len() > MAX_NODES {
        warn!(
            provider,
            record,
            count = nodes.len(),
            "more than {MAX_NODES} bf1 nodes, the rest ignored"
        );
    }
    nodes.into_iter().take(MAX_NODES).collect()
}

/// One node's TXT lookup → its observation. Err = skip the whole node: zero bf1 RRs, more than one distinct
/// bf1 RR (ambiguous), or ANY bf1 RR failing validation. ttl = min(index, node) validity from `now`.
fn node(
    label: &str,
    domain: &Host,
    index_until: Instant,
    l: &Lookup,
    now: Instant,
) -> Result<MachineObservation, String> {
    let mut found: Vec<Bf1> = Vec::new();
    for s in txts(l) {
        if let Some(r) = parse_bf1(&s)?
            && !found.contains(&r)
        {
            found.push(r);
        }
    }
    let [r] = &found[..] else {
        return Err(match found.len() {
            0 => "no bf1 record".into(),
            n => format!("ambiguous: {n} distinct bf1 records"),
        });
    };
    let ttl = index_until
        .min(l.valid_until())
        .saturating_duration_since(now);
    node_observation(label, domain, r, ttl)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hickory_resolver::proto::op::Query;
    use hickory_resolver::proto::rr::rdata::TXT;
    use hickory_resolver::proto::rr::{Name, RData, Record, RecordType};

    fn h(s: &str) -> Host {
        Host::parse(s).unwrap()
    }
    fn bf1(s: &str) -> Bf1 {
        parse_bf1(s).unwrap().unwrap()
    }
    /// a TXT answer set: one RR per inner slice, each inner &str one character-string
    fn lk(rrs: &[&[&str]], until: Instant) -> Lookup {
        let n = Name::from_ascii("_bifrost.x.test.bifrost.").unwrap();
        let recs = rrs.iter().map(|cs| {
            let t = TXT::new(cs.iter().map(|s| s.to_string()).collect());
            Record::from_rdata(n.clone(), 5, RData::TXT(t))
        });
        Lookup::new_with_deadline(Query::query(n.clone(), RecordType::TXT), recs, until)
    }

    #[test]
    fn bf1_v_first_required() {
        assert_eq!(parse_bf1("host=h v=bf1"), Ok(None));
        assert_eq!(parse_bf1("v=bf1x host=h"), Ok(None));
        assert_eq!(parse_bf1("v=bf2 host=h"), Ok(None));
        assert_eq!(parse_bf1(" v=bf1 host=h"), Ok(None)); // a record starts with a token
        assert_eq!(parse_bf1("v=bf1"), Ok(Some(Bf1::default())));
        assert_eq!(bf1("v=bf1  host=h").host, Some(h("h"))); // 1*SP
        assert!(parse_bf1("v=bf1 host=h ").is_err()); // no trailing SP
    }

    #[test]
    fn bf1_other_txt_ignored() {
        assert_eq!(parse_bf1(""), Ok(None));
        assert_eq!(parse_bf1("v=spf1 include:_spf.example.com ~all"), Ok(None));
        assert_eq!(parse_bf1("google-site-verification=abc"), Ok(None));
        // not ours even when it is huge or not valid key=value
        assert_eq!(parse_bf1(&"x".repeat(5000)), Ok(None));
    }

    #[test]
    fn bf1_unknown_keys_ignored() {
        let r = bf1("v=bf1 driver=rclone future_key=x-1 host=h");
        assert_eq!(
            r,
            Bf1 {
                host: Some(h("h")),
                ..Bf1::default()
            }
        );
        // unknown keys still obey the token grammar
        assert!(parse_bf1("v=bf1 Driver=x").is_err());
        assert!(parse_bf1("v=bf1 x=\"q\"").is_err());
        assert!(parse_bf1("v=bf1 x=").is_err());
        assert!(parse_bf1("v=bf1 x").is_err());
        assert!(parse_bf1(&format!("v=bf1 {}=1", "k".repeat(33))).is_err());
        assert!(parse_bf1(&format!("v=bf1 k={}", "v".repeat(257))).is_err());
        assert!(parse_bf1(&format!("v=bf1 {}=1", "k".repeat(32))).is_ok());
        assert!(parse_bf1(&format!("v=bf1 k={}", "v".repeat(256))).is_ok());
        assert!(parse_bf1("v=bf1 k=a\tb").is_err());
        assert!(parse_bf1("v=bf1 k=\u{e9}").is_err());
        // split at the first '=' only
        assert!(parse_bf1("v=bf1 k=a=b").is_ok());
    }

    #[test]
    fn bf1_duplicate_key_invalid() {
        assert!(parse_bf1("v=bf1 host=a host=b").is_err());
        assert!(parse_bf1("v=bf1 host=a host=a").is_err());
        assert!(parse_bf1("v=bf1 v=bf1").is_err());
        assert!(parse_bf1("v=bf1 zz=1 zz=2").is_err());
    }

    #[test]
    fn bf1_host_injection_rejected() {
        assert!(parse_bf1("v=bf1 host=-oProxyCommand=touch${IFS}/tmp/pwned").is_err());
        assert!(parse_bf1("v=bf1 host=a,b").is_err());
        assert!(parse_bf1("v=bf1 host=u@h").is_err());
        assert!(parse_bf1("v=bf1 user=-oX").is_err());
        assert!(parse_bf1("v=bf1 path=/a/../b").is_err());
        assert!(parse_bf1("v=bf1 path=rel").is_err());
        for p in ["0", "65536", "+22", "022222", "x"] {
            assert!(parse_bf1(&format!("v=bf1 port={p}")).is_err(), "{p}");
        }
        let r = bf1("v=bf1 host=127.0.0.1 port=2222 user=bf tags=dev,Agent path=/home/bf/data");
        assert_eq!(r.host, Some(h("127.0.0.1")));
        assert_eq!(r.port, Some(2222));
        assert_eq!(r.user, Some(User::parse("bf").unwrap()));
        assert_eq!(r.tags, ["dev", "agent"]);
        assert_eq!(r.path, Some(RemotePath::parse("/home/bf/data").unwrap()));
        assert_eq!(bf1("v=bf1 port=65535").port, Some(65535));
        let tags = (0..33).map(|i| format!("t{i}")).collect::<Vec<_>>();
        assert!(parse_bf1(&format!("v=bf1 tags={}", tags[..32].join(","))).is_ok());
        assert!(parse_bf1(&format!("v=bf1 tags={}", tags.join(","))).is_err());
        assert!(parse_bf1("v=bf1 tags=a,,b").is_err());
    }

    #[test]
    fn bf1_bad_id_rejects_node() {
        assert!(parse_bf1("v=bf1 host=127.0.0.1 id=../../etc").is_err());
        assert_eq!(
            bf1("v=bf1 id=nABC:12-x.y").id.as_deref(),
            Some("nABC:12-x.y")
        );
        let now = Instant::now();
        let until = now + Duration::from_secs(5);
        let l = lk(&[&["v=bf1 host=127.0.0.1 id=../../etc"]], until);
        assert!(node("evil", &h("test.bifrost"), until, &l, now).is_err());
    }

    #[test]
    fn bf1_nodes_no_dots_capped() {
        assert_eq!(
            bf1("v=bf1 nodes=agent-01,b,9").nodes,
            ["agent-01", "b", "9"]
        );
        for bad in ["a.b", "a,,b", "_x", "-a", "a-", "A", &"a".repeat(64)] {
            assert!(parse_bf1(&format!("v=bf1 nodes={bad}")).is_err(), "{bad}");
        }
        assert_eq!(dns_label(&"a".repeat(63)), Ok("a".repeat(63)));
        assert!(dns_label("").is_err());
        // union over the index RRs: deduplicated, sorted, capped at 256
        let names = (0..350).map(|i| format!("n{i:03}")).collect::<Vec<_>>();
        let vals: Vec<[String; 1]> = names
            .chunks(50)
            .map(|c| [format!("v=bf1 nodes={}", c.join(","))])
            .collect();
        let owned: Vec<Vec<&str>> = vals.iter().map(|[v]| vec![v.as_str()]).collect();
        let mut rrs: Vec<&[&str]> = owned.iter().map(|v| &v[..]).collect();
        rrs.push(&["v=bf1 nodes=n000,n001"]); // duplicates
        rrs.push(&["v=bf1 nodes=zz.evil.com"]); // invalid RR: skipped
        rrs.push(&["v=spf1 -all"]);
        let got = index_nodes("dns", &lk(&rrs, Instant::now()));
        assert_eq!(got, names[..256]);
        // no valid index RR → no nodes
        let l = lk(&[&["v=bf1 nodes=a.b"], &["v=spf1"]], Instant::now());
        assert!(index_nodes("dns", &l).is_empty());
        // nodes from every valid RR; host= at the index is ignored
        let l = lk(
            &[&["v=bf1 nodes=b,a"], &["v=bf1 nodes=c host=h"]],
            Instant::now(),
        );
        assert_eq!(index_nodes("dns", &l), ["a", "b", "c"]);
    }

    #[test]
    fn bf1_char_strings_concatenated() {
        let l = lk(&[&["v=bf1 host=127.0", ".0.1 po", "rt=22"]], Instant::now());
        assert_eq!(txts(&l), ["v=bf1 host=127.0.0.1 port=22"]);
        let r = bf1(&txts(&l)[0]);
        assert_eq!((r.host, r.port), (Some(h("127.0.0.1")), Some(22)));
    }

    #[test]
    fn bf1_size_limit() {
        let mut s = "v=bf1".to_string();
        for i in 0..8 {
            s += &format!(" k{i}={}", "a".repeat(250));
        }
        s += &format!(" z={}", "a".repeat(8));
        assert_eq!(s.len(), 2048);
        assert!(parse_bf1(&s).is_ok());
        s.push('a');
        assert!(parse_bf1(&s).is_err());
    }

    #[test]
    fn node_default_host() {
        let d = h("test.bifrost");
        let ttl = Duration::from_secs(5);
        let r = bf1("v=bf1 port=2222 user=bf tags=dev path=/home/bf/data");
        let o = node_observation("agent-dns", &d, &r, ttl).unwrap();
        assert_eq!(o.addresses, [h("agent-dns.test.bifrost")]);
        assert_eq!(o.port, Some(2222));
        assert_eq!(o.hints.user, r.user);
        assert_eq!(o.hints.path, r.path);
        assert!(o.metadata.tags.contains("dev"));
        assert_eq!((o.online, o.ttl), (None, Some(ttl)));
        let o = node_observation("agent-dns", &d, &bf1("v=bf1 host=10.0.0.9"), ttl).unwrap();
        assert_eq!(o.addresses, [h("10.0.0.9")]);
        // a default host over 253 bytes is not a host: the node is skipped
        let long = h(&format!("{0}.{0}.{0}", "d".repeat(63)));
        assert!(node_observation(&"a".repeat(63), &long, &Bf1::default(), ttl).is_err());
    }

    #[test]
    fn node_id_pinned_to_label() {
        let d = h("test.bifrost");
        let r = bf1("v=bf1 host=127.0.0.1 id=nodekey-123");
        let o = node_observation("agent-dns", &d, &r, Duration::ZERO).unwrap();
        assert_eq!(o.id.as_str(), "agent-dns");
        assert_eq!(o.name, "agent-dns");
        assert_eq!(o.native_id.as_deref(), Some("nodekey-123"));
        for bad in ["a.b", "../x", "A", "_x", ""] {
            assert!(
                node_observation(bad, &d, &r, Duration::ZERO).is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn ambiguous_node_skipped() {
        let (d, now) = (h("test.bifrost"), Instant::now());
        let u = now + Duration::from_secs(5);
        let one = |rrs: &[&[&str]]| node("n1", &d, u, &lk(rrs, u), now);
        let e = one(&[&["v=bf1 host=10.0.0.1"], &["v=bf1 host=10.0.0.2"]]).unwrap_err();
        assert!(e.contains("ambiguous"), "{e}");
        // the same record twice is not ambiguous; other TXT is ignored
        let o = one(&[
            &["v=bf1 host=10.0.0.1"],
            &["v=bf1  host=10.0.0.1"],
            &["v=spf1"],
        ])
        .unwrap();
        assert_eq!(o.addresses, [h("10.0.0.1")]);
        // one invalid bf1 RR poisons the node; zero bf1 RRs skip it
        assert!(one(&[&["v=bf1 host=10.0.0.1"], &["v=bf1 host=-x"]]).is_err());
        assert!(one(&[&["v=spf1"]]).is_err());
        assert!(one(&[]).is_err());
    }

    #[test]
    fn ttl_min_of_index_and_node() {
        let (d, now) = (h("test.bifrost"), Instant::now());
        let s = Duration::from_secs;
        let rec: &[&[&str]] = &[&["v=bf1"]];
        let ttl = |iu, nu| node("n1", &d, iu, &lk(rec, nu), now).unwrap().ttl;
        assert_eq!(ttl(now + s(10), now + s(4)), Some(s(4)));
        assert_eq!(ttl(now + s(3), now + s(8)), Some(s(3)));
        assert_eq!(ttl(now, now + s(8)), Some(Duration::ZERO));
    }

    /// Against the p08 zone: `T=$(mktemp -d); source tests/e2e/lib.sh; source tests/e2e/p08_dns.sh;
    /// setup_p08`, then `cargo test -p bifrost-discovery -- --ignored coredns_discovery`, then
    /// `docker rm -f bf-e2e-dns`. BIFROST_E2E_DNS overrides the 127.0.0.1:5353 nameserver.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the p08 coredns container (tests/e2e/p08_dns.sh setup_p08)"]
    async fn coredns_discovery() {
        let ns: SocketAddr = std::env::var("BIFROST_E2E_DNS")
            .unwrap_or("127.0.0.1:5353".into())
            .parse()
            .unwrap();
        let p = DnsProvider::new("dns".into(), h("test.bifrost"), vec![ns]).unwrap();
        let obs = p.discover().await.unwrap();
        let ids: Vec<&str> = obs.iter().map(|o| o.id.as_str()).collect();
        assert_eq!(ids, ["agent-dns", "other-01"]); // bad-node and evil skipped
        let a = &obs[0];
        assert_eq!(
            (&a.addresses[..], a.port),
            (&[h("127.0.0.1")][..], Some(2222))
        );
        assert_eq!(a.hints.user, Some(User::parse("bf").unwrap()));
        assert_eq!(
            a.hints.path,
            Some(RemotePath::parse("/home/bf/data").unwrap())
        );
        assert!(a.metadata.tags.contains("dev") && a.native_id.is_none());
        assert!(a.ttl.unwrap() <= Duration::from_secs(5));
        assert!(obs[1].metadata.tags.contains("misc"));
        // NXDOMAIN on the index: an empty view, not an error
        let p = DnsProvider::new("dns".into(), h("none.test.bifrost"), vec![ns]).unwrap();
        assert_eq!(p.discover().await, Ok(vec![]));
        // a zone the server doesn't serve (REFUSED): the source couldn't be read → Failed (freeze)
        let p = DnsProvider::new("dns".into(), h("example.org"), vec![ns]).unwrap();
        assert!(matches!(p.discover().await, Err(DiscoveryError::Failed(_))));
    }
}
