//! DNS TXT `bf1` provider (contract §7). The root `_bifrost.<domain>.` holds inline node records (`node=`)
//! and index values listing node labels in `nodes=`; each index node publishes one `_bifrost.<node>.<domain>.`
//! record. Every TXT byte is untrusted: it goes through
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
use std::collections::{BTreeMap, BTreeSet};
use std::net::SocketAddr;
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use tracing::warn;

/// labels across the root RRset: inline `node=` values plus `nodes=` entries
const MAX_NODES: usize = 256;

pub struct DnsProvider {
    name: String,
    domain: Host,
    /// Some for explicit `nameservers` only. None = the system resolver, whose config (resolv.conf, or
    /// SCDynamicStore on macOS) is read afresh on every refresh: a snapshot taken at start would keep querying
    /// the old servers after a network change, and a daemon started offline would never get one.
    resolver: Option<TokioResolver>,
}

impl DnsProvider {
    pub fn new(name: String, domain: Host, nameservers: Vec<SocketAddr>) -> Result<Self, String> {
        if nameservers.is_empty() {
            return Ok(Self {
                name,
                domain,
                resolver: None,
            });
        }
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
        // no options_mut() lines (D1): ResolverOpts::default() is already 5s timeout × 2 attempts.
        // The cache stays on and honours the record TTLs.
        let b = Resolver::builder_with_config(
            ResolverConfig::from_name_servers(ns),
            TokioRuntimeProvider::default(),
        );
        let resolver = Some(b.build().map_err(|e| e.to_string())?); // C8: NetError → String
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
            // ponytail: the system resolver is rebuilt every refresh, so hickory's cache never outlives one (the
            // registry does expiry; the stub or upstream still caches); reuse it while the config is unchanged if
            // the extra queries ever matter
            let resolver = match &self.resolver {
                Some(r) => r.clone(),
                None => TokioResolver::builder_tokio()
                    .and_then(|b| b.build())
                    .map_err(|e| DiscoveryError::Failed(clean(&e.to_string(), 512)))?,
            };
            let q = format!("_bifrost.{d}.");
            let index = match txt(&resolver, &q).await {
                Ok(l) => l,
                // an empty view, not a failure: with the expiry registry this causes no churn
                Err(e) if e.is_no_records_found() => {
                    warn!(provider, record = q, "no bf1 root record");
                    return Ok(vec![]);
                }
                Err(e) => return Err(DiscoveryError::Failed(clean(&e.to_string(), 512))),
            };
            let (nodes, mut out) = root(provider, &self.domain, &index, Instant::now());
            if nodes.is_empty() && out.is_empty() {
                warn!(provider, record = q, "no valid bf1 root record");
                return Ok(vec![]);
            }
            let mut set = JoinSet::new();
            for n in nodes {
                let (r, q) = (resolver.clone(), format!("_bifrost.{n}.{d}."));
                set.spawn(async move {
                    let l = txt(&r, &q).await.map_err(|e| e.to_string());
                    (n, q, l)
                });
            }
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
            out.sort_by(|a, b| a.id.cmp(&b.id)); // ids are distinct: inline and index labels are disjoint
            Ok(out)
        })
    }
}

/// no driver (E2)
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bf1 {
    /// root values only: an inline node record, identity = this label
    pub node: Option<String>,
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
/// Missing '=', a bad key or value, a duplicate key, a known key failing validation, or `node=` with
/// `nodes=` → Err (record invalid). Unknown keys are ignored, `driver=` included (E2).
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
            "node" => r.node = Some(dns_label(v).map_err(inv)?),
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
    if r.node.is_some() && !r.nodes.is_empty() {
        return Err("node= and nodes= in one value".into());
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

/// The root RRset → (index nodes to look up, inline node observations). A `v=bf1` value with a `node=` token
/// is an inline node record, grouped by that label (not a DNS label → skipped, no cap slot) before parsing so
/// an invalid value still rejects its node;
/// any other valid bf1 value is an index value whose `nodes=` count (an invalid one is skipped with a warning).
/// At most MAX_NODES labels of the sorted union; a label both inline and in `nodes=` is ambiguous and skipped.
/// Inline ttl = the root's validity.
// ponytail: DNSSEC is not validated, a spoofed or on-path answer is trusted; hickory's dnssec feature + `validate`
fn root(
    provider: &str,
    domain: &Host,
    l: &Lookup,
    now: Instant,
) -> (Vec<String>, Vec<MachineObservation>) {
    let record = l.query().name().to_string();
    let (mut index, mut inline) = (BTreeSet::new(), BTreeMap::<String, Vec<String>>::new());
    for s in txts(l) {
        let label = s
            .strip_prefix("v=bf1 ")
            .and_then(|t| t.split(' ').find_map(|t| t.strip_prefix("node=")));
        if let Some(n) = label {
            match dns_label(n) {
                Ok(n) => inline.entry(n).or_default().push(s),
                Err(e) => warn!(
                    provider,
                    record,
                    node = clean(n, 128),
                    reason = clean(&e.to_string(), 512),
                    "bf1 node skipped"
                ),
            }
            continue;
        }
        match parse_bf1(&s) {
            Ok(r) => index.extend(r.into_iter().flat_map(|r| r.nodes)),
            Err(e) => warn!(
                provider,
                record,
                reason = clean(&e, 512),
                "bf1 index record skipped"
            ),
        }
    }
    let labels: BTreeSet<&String> = index.iter().chain(inline.keys()).collect();
    if labels.len() > MAX_NODES {
        warn!(
            provider,
            record,
            count = labels.len(),
            "more than {MAX_NODES} bf1 nodes, the rest ignored"
        );
    }
    let ttl = l.valid_until().saturating_duration_since(now);
    let (mut look, mut out) = (Vec::new(), Vec::new());
    for n in labels.into_iter().take(MAX_NODES) {
        let o = match inline.get(n) {
            None => {
                look.push(n.clone());
                continue;
            }
            Some(_) if index.contains(n) => Err("ambiguous: inline and in nodes=".into()),
            Some(v) => one(v).and_then(|r| node_observation(n, domain, &r, ttl)),
        };
        match o {
            Ok(o) => out.push(o),
            Err(e) => warn!(
                provider,
                record,
                node = clean(n, 128),
                reason = clean(&e, 512),
                "bf1 node skipped"
            ),
        }
    }
    (look, out)
}

/// A node's bf1 values → its one record. Err: zero bf1 values, more than one distinct one (ambiguous), or ANY
/// failing validation. Other TXT is ignored.
fn one(vals: &[String]) -> Result<Bf1, String> {
    let mut found: Vec<Bf1> = Vec::new();
    for s in vals {
        if let Some(r) = parse_bf1(s)?
            && !found.contains(&r)
        {
            found.push(r);
        }
    }
    if found.len() > 1 {
        return Err(format!("ambiguous: {} distinct bf1 records", found.len()));
    }
    found.pop().ok_or_else(|| "no bf1 record".into())
}

/// One index node's TXT lookup → its observation. Err = skip the whole node: `one()` failed, or its record
/// carries `node=` (root values only). ttl = min(index, node) validity from `now`.
fn node(
    label: &str,
    domain: &Host,
    index_until: Instant,
    l: &Lookup,
    now: Instant,
) -> Result<MachineObservation, String> {
    let r = one(&txts(l))?;
    if r.node.is_some() {
        return Err("node= in a node record (root values only)".into());
    }
    let ttl = index_until
        .min(l.valid_until())
        .saturating_duration_since(now);
    node_observation(label, domain, &r, ttl)
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
        let (d, now) = (h("test.bifrost"), Instant::now());
        let index_nodes = |l: &Lookup| root("dns", &d, l, now).0;
        let got = index_nodes(&lk(&rrs, now));
        assert_eq!(got, names[..256]);
        // no valid index RR → no nodes
        let l = lk(&[&["v=bf1 nodes=a.b"], &["v=spf1"]], now);
        assert!(index_nodes(&l).is_empty());
        // nodes from every valid RR; host= at the index is ignored
        let l = lk(&[&["v=bf1 nodes=b,a"], &["v=bf1 nodes=c host=h"]], now);
        assert_eq!(index_nodes(&l), ["a", "b", "c"]);
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

    /// root() of a root RRset with one RR per value, valid for 5s
    fn rt(vals: &[&str]) -> (Vec<String>, Vec<MachineObservation>) {
        let rrs: Vec<[&str; 1]> = vals.iter().map(|v| [*v]).collect();
        let rrs: Vec<&[&str]> = rrs.iter().map(|r| &r[..]).collect();
        let now = Instant::now();
        let l = lk(&rrs, now + Duration::from_secs(5));
        root("dns", &h("test.bifrost"), &l, now)
    }
    fn ids(o: &[MachineObservation]) -> Vec<&str> {
        o.iter().map(|o| o.id.as_str()).collect()
    }

    #[test]
    fn inline_node_records_discovered() {
        let (look, obs) = rt(&[
            "v=bf1 node=agent-01 host=10.0.0.5 user=sami tags=dev,agent",
            "v=bf1 node=agent-02 tags=dev",
            "v=bf1 node=agent-03 port=2222 path=/srv/data id=nkey-3",
            "v=spf1 node=x -all",
        ]);
        assert!(look.is_empty(), "{look:?}");
        assert_eq!(ids(&obs), ["agent-01", "agent-02", "agent-03"]);
        assert_eq!(obs[0].addresses, [h("10.0.0.5")]);
        assert_eq!(obs[0].hints.user, Some(User::parse("sami").unwrap()));
        assert!(obs[0].metadata.tags.contains("dev") && obs[0].metadata.tags.contains("agent"));
        assert_eq!(obs[1].addresses, [h("agent-02.test.bifrost")]); // <node>.<domain>
        assert_eq!(obs[2].port, Some(2222));
        assert_eq!(obs[2].native_id.as_deref(), Some("nkey-3"));
        assert_eq!(
            obs[2].hints.path,
            Some(RemotePath::parse("/srv/data").unwrap())
        );
    }

    #[test]
    fn inline_and_index_mixed() {
        let (look, obs) = rt(&[
            "v=bf1 nodes=idx-02,idx-01",
            "v=bf1 node=inl-01 tags=dev",
            "v=bf1 nodes=idx-03",
        ]);
        assert_eq!(look, ["idx-01", "idx-02", "idx-03"]); // looked up at _bifrost.<node>.<domain>.
        assert_eq!(ids(&obs), ["inl-01"]); // never looked up
    }

    #[test]
    fn inline_node_also_in_index_is_ambiguous() {
        let (look, obs) = rt(&["v=bf1 nodes=a,b", "v=bf1 node=a host=10.0.0.1"]);
        assert_eq!(look, ["b"]);
        assert!(obs.is_empty());
    }

    #[test]
    fn two_distinct_inline_values_same_node_ambiguous() {
        let (_, obs) = rt(&[
            "v=bf1 node=a host=10.0.0.1",
            "v=bf1 node=a host=10.0.0.2",
            "v=bf1 node=b",
        ]);
        assert_eq!(ids(&obs), ["b"]);
    }

    #[test]
    fn identical_inline_duplicates_ok() {
        let (_, obs) = rt(&["v=bf1 node=a host=10.0.0.1", "v=bf1  node=a host=10.0.0.1"]);
        assert_eq!(ids(&obs), ["a"]);
        assert_eq!(obs[0].addresses, [h("10.0.0.1")]);
    }

    #[test]
    fn value_with_node_and_nodes_rejected() {
        assert!(parse_bf1("v=bf1 node=a nodes=b").is_err());
        assert!(parse_bf1("v=bf1 nodes=b node=a").is_err());
        let (look, obs) = rt(&["v=bf1 node=a nodes=b", "v=bf1 nodes=c"]);
        assert_eq!(look, ["c"]);
        assert!(obs.is_empty());
    }

    #[test]
    fn inline_invalid_key_rejects_whole_node() {
        for bad in [
            "v=bf1 node=a host=-oProxyCommand=x",
            "v=bf1 node=a id=../x",
            "v=bf1 node=a tags=dev port=0",
        ] {
            let (look, obs) = rt(&[bad, "v=bf1 node=b"]);
            assert!(look.is_empty(), "{bad}");
            assert_eq!(ids(&obs), ["b"], "{bad}");
            // next to a valid value for the same node: still the whole node, never partial
            let (_, obs) = rt(&[bad, "v=bf1 node=a host=10.0.0.1"]);
            assert!(obs.is_empty(), "{bad}");
        }
        // not a label: no node, and never a query into another domain
        for bad in ["a.evil.com", "A", "-a", "../x"] {
            let (look, obs) = rt(&[&format!("v=bf1 node={bad}")]);
            assert!(look.is_empty() && obs.is_empty(), "{bad}");
        }
    }

    #[test]
    fn inline_ttl_is_root_validity() {
        let now = Instant::now();
        let l = lk(&[&["v=bf1 node=a"]], now + Duration::from_secs(7));
        let (_, obs) = root("dns", &h("test.bifrost"), &l, now);
        assert_eq!(obs[0].ttl, Some(Duration::from_secs(7)));
    }

    #[test]
    fn max_nodes_caps_inline_plus_index() {
        // 200 index + 100 inline: each under 256, the union over it → the first 256 labels
        let names: Vec<String> = (0..300).map(|i| format!("n{i:03}")).collect();
        let mut vals: Vec<String> = names[..200]
            .chunks(50)
            .map(|c| format!("v=bf1 nodes={}", c.join(",")))
            .collect();
        vals.extend(names[200..].iter().map(|n| format!("v=bf1 node={n}")));
        vals.push("v=bf1 node=A".into()); // not a label: takes no slot (sorts first)
        let (look, obs) = rt(&vals.iter().map(String::as_str).collect::<Vec<_>>());
        assert_eq!(look, names[..200]);
        assert_eq!(ids(&obs), names[200..256]);
    }

    #[test]
    fn node_key_only_valid_at_root() {
        // stricter than ignoring it: a per-node record carrying node= (its own label too) rejects that node
        let (d, now) = (h("test.bifrost"), Instant::now());
        let u = now + Duration::from_secs(5);
        for v in ["v=bf1 node=n1 host=10.0.0.1", "v=bf1 node=other"] {
            let e = node("n1", &d, u, &lk(&[&[v]], u), now).unwrap_err();
            assert!(e.contains("node="), "{e}");
        }
    }

    /// hickory retries a truncated UDP answer over TCP only on a server with a TCP connection. A big inline
    /// root RRset outgrows a UDP reply (1232 bytes with EDNS; 512 on unix unless resolv.conf has
    /// `options edns0`), so the system resolver's servers must have one too, like the explicit `udp_and_tcp` ones.
    #[cfg(target_os = "linux")]
    #[test]
    fn system_resolver_has_tcp_fallback() {
        use hickory_resolver::config::ProtocolConfig;
        let (c, _) =
            hickory_resolver::system_conf::parse_resolv_conf("nameserver 192.0.2.1\n").unwrap();
        let conns = &c.name_servers()[0].connections;
        assert!(
            conns
                .iter()
                .any(|c| matches!(c.protocol, ProtocolConfig::Tcp))
        );
    }

    #[tokio::test]
    async fn system_resolver_read_per_refresh() {
        // nothing read at build time: an offline start (no nameserver yet) or a later network change can't pin
        // the provider to a dead or stale resolver; discover() reads the system config each refresh
        let p = DnsProvider::new("dns".into(), h("test.bifrost"), vec![]).unwrap();
        assert!(p.resolver.is_none());
        let ns = "127.0.0.1:5353".parse().unwrap();
        let p = DnsProvider::new("dns".into(), h("test.bifrost"), vec![ns]).unwrap();
        assert!(p.resolver.is_some()); // explicit nameservers: built once, cache kept
    }

    /// Against the p08 zone: `T=$(mktemp -d); source tests/e2e/lib.sh; source tests/e2e/p08_dns.sh;
    /// setup_p08`, then `cargo test -p bifrost-discovery -- --ignored coredns_discovery`, then
    /// `docker rm -f bf-e2e-dns`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs the p08 coredns container (tests/e2e/p08_dns.sh setup_p08)"]
    async fn coredns_discovery() {
        let ns: SocketAddr = "127.0.0.1:5353".parse().unwrap();
        let p = DnsProvider::new("dns".into(), h("test.bifrost"), vec![ns]).unwrap();
        let obs = p.discover().await.unwrap();
        let ids: Vec<&str> = obs.iter().map(|o| o.id.as_str()).collect();
        // bad-node and evil skipped; the root answer only fits over TCP (a 1232-byte UDP one is truncated)
        let mut want = vec!["agent-dns".to_string()];
        want.extend((1..=20).map(|i| format!("bulk-{i:02}")));
        want.push("other-01".into());
        assert_eq!(ids, want);
        assert_eq!(obs[20].addresses, [h("192.0.2.20")]);
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
        assert!(obs[21].metadata.tags.contains("misc")); // other-01: index + its own record
        // NXDOMAIN on the index: an empty view, not an error
        let p = DnsProvider::new("dns".into(), h("none.test.bifrost"), vec![ns]).unwrap();
        assert_eq!(p.discover().await, Ok(vec![]));
        // a zone the server doesn't serve (REFUSED): the source couldn't be read → Failed (freeze)
        let p = DnsProvider::new("dns".into(), h("example.org"), vec![ns]).unwrap();
        assert!(matches!(p.discover().await, Err(DiscoveryError::Failed(_))));
    }
}
