//! `tailscale status --json` provider (contract §7 Tailscale).

use bifrost_core::validate::{clean, native_id, tag, tail};
use bifrost_core::{
    BoxFuture, DiscoveryError, DiscoveryProvider, Host, MachineObservation, Metadata, MountHints,
    Name,
};
use serde::Deserialize;
use std::collections::{BTreeMap, btree_map::Entry};
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tracing::warn;

const CAP: usize = 16 << 20;
const MACOS_APP: &str = "/Applications/Tailscale.app/Contents/MacOS/Tailscale";

pub struct TailscaleProvider {
    name: String,
    binary: Option<PathBuf>,
}

impl TailscaleProvider {
    /// `binary`: tests only
    pub fn new(name: String, binary: Option<PathBuf>) -> Self {
        Self { name, binary }
    }
}

impl DiscoveryProvider for TailscaleProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>> {
        Box::pin(async move {
            // looked up on every refresh, so a tailscale installed after start is found
            let bin =
                self.binary.clone().or_else(which).ok_or_else(|| {
                    DiscoveryError::Unavailable("tailscale binary not found".into())
                })?;
            let out = tokio::time::timeout(Duration::from_secs(10), status_json(&bin))
                .await
                .map_err(|_| {
                    DiscoveryError::Failed("tailscale status: timed out after 10s".into())
                })??;
            parse_status(&out)
        })
    }
}

/// "tailscale" in an absolute $PATH entry, else the macOS app bundle (a runtime cfg!, so Linux compiles it).
fn which() -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let exe = |p: &PathBuf| {
        std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
        .filter(|d| d.is_absolute())
        .map(|d| d.join("tailscale"))
        .find(exe)
        .or_else(|| Some(PathBuf::from(MACOS_APP)).filter(|p| cfg!(target_os = "macos") && exe(p)))
}

/// stdout (≤ 16 MiB) of `<bin> status --json`; a non-zero exit is Failed(tail(stderr, 512)).
async fn status_json(bin: &Path) -> Result<Vec<u8>, DiscoveryError> {
    let fail = |e: std::io::Error| DiscoveryError::Failed(format!("tailscale status: {e}"));
    let mut child = tokio::process::Command::new(bin)
        .args(["status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => {
                DiscoveryError::Unavailable(format!("{}: not found", bin.display()))
            }
            _ => fail(e),
        })?;
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    // each reader owns its pipe and drops it at the cap: the child gets EPIPE instead of filling our heap
    let out = async {
        let mut b = Vec::new();
        if let Some(o) = stdout {
            o.take(CAP as u64 + 1)
                .read_to_end(&mut b)
                .await
                .map_err(fail)?;
        }
        if b.len() > CAP {
            return Err(DiscoveryError::Failed(
                "tailscale status: output exceeds 16 MiB".into(),
            ));
        }
        Ok(b)
    };
    let err = async {
        let mut b = Vec::new();
        if let Some(e) = stderr {
            let _ = e.take(64 << 10).read_to_end(&mut b).await;
        }
        Ok(b)
    };
    let (out, err, st) = tokio::try_join!(out, err, async { child.wait().await.map_err(fail) })?;
    if !st.success() {
        let t = tail(&String::from_utf8_lossy(&err), 512);
        return Err(DiscoveryError::Failed(if t.is_empty() {
            format!("tailscale status: {st}")
        } else {
            t
        }));
    }
    Ok(out)
}

// Field names are the JSON keys verbatim; `Self` is not declared, so we never see (or mount) ourselves.
#[allow(non_snake_case)]
#[derive(Deserialize)]
struct Status {
    BackendState: String,
    CurrentTailnet: Option<Tailnet>,
    /// Values, not `Peer`s: one malformed peer is skipped alone instead of failing the whole view.
    Peer: Option<BTreeMap<String, serde_json::Value>>,
}

#[allow(non_snake_case)]
#[derive(Deserialize)]
struct Tailnet {
    MagicDNSEnabled: bool,
    #[serde(default)]
    MagicDNSSuffix: String,
}

#[allow(non_snake_case)]
#[derive(Deserialize)]
struct Peer {
    ID: String,
    HostName: String,
    DNSName: String,
    #[serde(default)]
    OS: String,
    TailscaleIPs: Option<Vec<IpAddr>>,
    Tags: Option<Vec<String>>,
    Online: bool,
}

/// pure
pub fn parse_status(json: &[u8]) -> Result<Vec<MachineObservation>, DiscoveryError> {
    let s: Status = serde_json::from_slice(json)
        .map_err(|e| DiscoveryError::Failed(clean(&format!("tailscale status: {e}"), 512)))?;
    if s.BackendState != "Running" {
        let state = clean(&s.BackendState, 64);
        return Err(DiscoveryError::Unavailable(format!(
            "backend state {state}"
        )));
    }
    let (magic, own) = match s.CurrentTailnet {
        Some(t) => (
            t.MagicDNSEnabled,
            format!(".{}.", t.MagicDNSSuffix.trim_end_matches('.')),
        ),
        None => (false, String::new()),
    };
    // ponytail: skipped peers only warn! into the log (§15 #22); upgrade: `warnings` in ProviderDto
    let skip = |record: &str, reason: &str| {
        warn!(record = %clean(record, 128), reason = %clean(reason, 512), "tailscale peer skipped");
    };
    let mut peers = Vec::new();
    for (key, v) in s.Peer.unwrap_or_default() {
        match serde_json::from_value::<Peer>(v) {
            Ok(p) => peers.push(p),
            Err(e) => skip(&key, &e.to_string()),
        }
    }
    // decides which duplicate id wins: our tailnet's control-assigned names, then shared-in (foreign suffix),
    // then HostName-only (DNSName ""), so a lower-trust identity never evicts one of our own nodes.
    // No CurrentTailnet → own = "" (every name matches); empty suffix → ".." (none does): plain DNSName order.
    peers.sort_by_cached_key(|p| {
        (
            p.DNSName.is_empty(),
            !p.DNSName.ends_with(&own),
            p.DNSName.clone(),
        )
    });
    let mut out = BTreeMap::new();
    for p in peers {
        let record = if p.DNSName.is_empty() {
            p.HostName.clone()
        } else {
            p.DNSName.clone()
        };
        match observation(p, magic) {
            Err(why) => skip(&record, &why),
            Ok(o) => match out.entry(o.id.clone()) {
                Entry::Occupied(_) => skip(
                    &record,
                    "duplicate id (own tailnet first, then shared-in, then HostName-only)",
                ),
                Entry::Vacant(e) => {
                    e.insert(o);
                }
            },
        }
    }
    Ok(out.into_values().collect())
}

/// Every untrusted field is validated; any invalid identity or address field skips the whole peer.
fn observation(p: Peer, magic: bool) -> Result<MachineObservation, String> {
    let dns = match p.DNSName.as_str() {
        "" => None,
        d => Some(Host::parse(d).map_err(|e| e.to_string())?), // strips the trailing dot
    };
    let label = dns.as_ref().and_then(|h| h.as_str().split('.').next());
    let id = match label.map(Name::parse) {
        Some(Ok(id)) => id,
        _ => Name::parse(&p.HostName).map_err(|e| e.to_string())?, // lowercases: "MacBook" is kept
    };
    let ips = (p.TailscaleIPs.unwrap_or_default().iter())
        .map(|ip| Host::parse(&ip.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    let mut addresses: Vec<Host> = match &dns {
        Some(h) if magic => vec![h.clone()],
        _ => ips
            .iter()
            .find(|h| h.ip().is_some_and(|ip| ip.is_ipv4()))
            .cloned()
            .into_iter()
            .collect(),
    };
    for h in ips {
        if !addresses.contains(&h) {
            addresses.push(h);
        }
    }
    if addresses.is_empty() {
        return Err("no address".into());
    }
    let mut tags = std::collections::BTreeSet::new();
    for t in p.Tags.unwrap_or_default() {
        match tag(t.strip_prefix("tag:").unwrap_or(&t)) {
            Ok(t) => {
                tags.insert(t);
            }
            Err(e) => {
                warn!(record = %id.as_str(), reason = %clean(&e.to_string(), 512), "tailscale tag dropped")
            }
        }
    }
    let dns_name = dns.as_ref().map_or("", Host::as_str);
    let values = [
        ("os", p.OS.as_str()),
        ("hostname", &p.HostName),
        ("dns_name", dns_name),
    ]
    .into_iter()
    .filter(|(_, v)| !v.is_empty())
    .map(|(k, v)| (k.to_string(), clean(v, 256)))
    .collect();
    Ok(MachineObservation {
        id,
        name: clean(&p.HostName, 128),
        native_id: native_id(&p.ID).ok(),
        addresses,
        port: None,
        online: Some(p.Online),
        metadata: Metadata { tags, values },
        hints: MountHints::default(),
        ttl: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::{Host, Metadata, MountHints, Name};
    use serde_json::{Value, json};
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use std::time::{Duration, Instant};

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/tailscale_status.json");
    const PHONE: &str = "nodekey:0000000000000000000000000000000000000000000000000000000000000004";

    fn fixture() -> Value {
        serde_json::from_slice(FIXTURE).unwrap()
    }
    fn parse(v: &Value) -> Result<Vec<MachineObservation>, DiscoveryError> {
        parse_status(v.to_string().as_bytes())
    }
    fn ids(o: &[MachineObservation]) -> Vec<&str> {
        o.iter().map(|o| o.id.as_str()).collect()
    }
    fn addrs(o: &MachineObservation) -> Vec<&str> {
        o.addresses.iter().map(Host::as_str).collect()
    }
    fn get<'a>(o: &'a [MachineObservation], id: &str) -> &'a MachineObservation {
        o.iter().find(|o| o.id.as_str() == id).unwrap()
    }
    fn tmp() -> PathBuf {
        let r = bifrost_core::validate::random_u64();
        let d = std::env::temp_dir().join(format!("bifrost-ts-{r:016x}"));
        std::fs::create_dir(&d).unwrap();
        d
    }
    fn fake(d: &Path, name: &str, body: &str) -> PathBuf {
        let p = d.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }
    async fn run(bin: &Path) -> Result<Vec<MachineObservation>, DiscoveryError> {
        TailscaleProvider::new("tailscale".into(), Some(bin.into()))
            .discover()
            .await
    }

    #[test]
    fn tailscale_fixture_parse() {
        let o = parse_status(FIXTURE).unwrap();
        // Self excluded; the shared-in devbox.other.ts.net. repeats the id "devbox" and loses to our own
        // tailnet's devbox (own suffix first); the capitalised HostName fallback is kept (lowercased)
        assert_eq!(ids(&o), ["devbox", "devbox-1", "fixture-macbook", "phone"]);
        let tags = ["dev", "server"].map(String::from); // "tag:" stripped, lowercased, "bad tag!" dropped
        let values = [
            ("dns_name", "devbox.example.ts.net"), // trailing dot trimmed
            ("hostname", "devbox"),
            ("os", "linux"),
        ];
        let want = MachineObservation {
            id: Name::parse("devbox").unwrap(),
            name: "devbox".into(),
            native_id: Some("nDevbox0001CNTRL".into()),
            addresses: ["devbox.example.ts.net", "100.64.0.10", "fd7a:115c:a1e0::a"]
                .map(|h| Host::parse(h).unwrap())
                .into(),
            port: None,
            online: Some(true),
            metadata: Metadata {
                tags: tags.into(),
                values: values.map(|(k, v)| (k.into(), v.into())).into(),
            },
            hints: MountHints::default(),
            ttl: None,
        };
        assert_eq!(get(&o, "devbox"), &want);
        // duplicate HostName resolved via DNSName; Tags absent → no tags
        let d1 = get(&o, "devbox-1");
        assert_eq!((d1.name.as_str(), d1.online), ("devbox", Some(false)));
        assert!(d1.metadata.tags.is_empty());
        assert_eq!(addrs(d1)[0], "devbox-1.example.ts.net");
        // DNSName "" → id from HostName; address = the first IPv4 although v6 is listed first
        let m = get(&o, "fixture-macbook");
        assert_eq!(m.name, "Fixture-MacBook");
        assert_eq!(addrs(m), ["100.64.0.12", "fd7a:115c:a1e0::c"]);
        assert!(!m.metadata.values.contains_key("dns_name"));
        assert_eq!(get(&o, "phone").name, "localhost");

        // a shared-in node whose suffix sorts first and a HostName-only "devbox" never take our node's id
        let mut v = fixture();
        v["Peer"]["nodekey:0000000000000000000000000000000000000000000000000000000000000005"]["DNSName"] =
            json!("devbox.aaa.ts.net.");
        v["Peer"]["k06"] = json!({"ID": "nX", "HostName": "devbox", "DNSName": "", "TailscaleIPs": ["100.64.0.30"], "Online": true});
        let own = |v: &Value| get(&parse(v).unwrap(), "devbox").native_id.clone();
        assert_eq!(own(&v).as_deref(), Some("nDevbox0001CNTRL"));
        v["CurrentTailnet"] = Value::Null; // no suffix known: plain DNSName order, HostName-only last
        assert_eq!(own(&v).as_deref(), Some("nShared0005CNTRL"));

        // MagicDNS off (or no CurrentTailnet) → first IPv4, then every IP
        let mut v = fixture();
        v["CurrentTailnet"]["MagicDNSEnabled"] = json!(false);
        assert_eq!(
            addrs(get(&parse(&v).unwrap(), "devbox")),
            ["100.64.0.10", "fd7a:115c:a1e0::a"]
        );
        v["CurrentTailnet"] = Value::Null;
        assert_eq!(
            addrs(get(&parse(&v).unwrap(), "devbox-1")),
            ["100.64.0.11", "fd7a:115c:a1e0::b"]
        );
        // Peer: null, or absent → an empty (complete) view
        v["Peer"] = Value::Null;
        assert_eq!(parse(&v), Ok(vec![]));
        v.as_object_mut().unwrap().remove("Peer");
        assert_eq!(parse(&v), Ok(vec![]));
    }

    #[test]
    fn tailscale_backend_stopped_unavailable() {
        for state in ["Stopped", "NeedsLogin", "Starting"] {
            let mut v = fixture();
            v["BackendState"] = json!(state);
            let want = DiscoveryError::Unavailable(format!("backend state {state}"));
            assert_eq!(parse(&v), Err(want));
        }
        assert!(matches!(
            parse_status(b"not json"),
            Err(DiscoveryError::Failed(_))
        ));
    }

    #[test]
    fn hostile_peer_fields_rejected() {
        let peer = |host: &str, dns: &str, ips: Value| json!({"ID": "nX", "HostName": host, "DNSName": dns, "TailscaleIPs": ips, "Online": true});
        let ip = || json!(["100.64.0.20"]);
        let mut v = fixture();
        v["Peer"] = json!({
            "k01": peer("good", "good.example.ts.net.", ip()),
            "k02": peer("x", "-oProxyCommand=x.example.ts.net.", ip()),
            "k03": peer("-oProxyCommand=x", "", ip()),
            "k04": peer("x", "../x.", ip()),
            "k05": peer("../x", "", ip()),
            "k06": peer("x", "a\u{7}b.example.ts.net.", ip()),
            "k07": peer("c\u{1b}[31mtrl\u{202e}", "ctrl.example.ts.net.", ip()),
            "k08": peer("x", "ip1.example.ts.net.", json!(["100.64.0"])),
            "k09": peer("x", "ip2.example.ts.net.", json!(["0.0.0.0"])),
            "k10": peer("x", "ip3.example.ts.net.", json!(["fe80::1%eth0"])),
            "k11": peer("x", "ip4.example.ts.net.", json!(["-oProxyCommand=x"])),
            "k12": peer("noaddr", "", json!([])),
            "k13": peer("x", "idbad.example.ts.net.", ip()),
            "k14": {"ID": "nX", "HostName": "x", "DNSName": "online.example.ts.net.", "Online": "yes"},
        });
        v["Peer"]["k13"]["ID"] = json!("../../etc");
        let o = parse(&v).unwrap(); // bad peers are skipped one by one; the document still parses
        assert_eq!(ids(&o), ["ctrl", "good", "idbad"]);
        let c = get(&o, "ctrl"); // display-only text is cleaned, never trusted
        assert_eq!(c.name, "c?[31mtrl?");
        assert_eq!(c.metadata.values["hostname"], "c?[31mtrl?");
        assert_eq!(get(&o, "idbad").native_id, None);
        for m in &o {
            assert!(m.addresses.iter().all(|h| !h.as_str().starts_with('-')));
        }
    }

    #[tokio::test]
    async fn tailscale_new_peer_appears() {
        let d = tmp();
        let data = d.join("status.json");
        // anything but `status --json` exits 9
        let body = format!(
            "[ \"$*\" = 'status --json' ] || exit 9\nexec cat '{}'",
            data.display()
        );
        let bin = fake(&d, "tailscale", &body);
        std::thread::sleep(Duration::from_millis(200)); // ETXTBSY: a sibling test's fork may hold the write fd
        let mut v = fixture();
        let phone = v["Peer"].as_object_mut().unwrap().remove(PHONE).unwrap();
        std::fs::write(&data, v.to_string()).unwrap();
        let p = TailscaleProvider::new("tailscale".into(), Some(bin));
        let first = p.discover().await.unwrap();
        assert_eq!(ids(&first), ["devbox", "devbox-1", "fixture-macbook"]);
        v["Peer"][PHONE] = phone;
        std::fs::write(&data, v.to_string()).unwrap();
        let second = p.discover().await.unwrap();
        assert_eq!(
            ids(&second),
            ["devbox", "devbox-1", "fixture-macbook", "phone"]
        );
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[tokio::test]
    async fn tailscale_invocation_errors() {
        let missing = run(Path::new("/nonexistent/tailscale")).await;
        assert!(
            matches!(missing, Err(DiscoveryError::Unavailable(_))),
            "{missing:?}"
        );
        let d = tmp();
        let fail = fake(
            &d,
            "fail",
            "echo 'failed to connect to local tailscaled' >&2; exit 1",
        );
        let flood = fake(&d, "flood", "exec yes");
        std::thread::sleep(Duration::from_millis(200)); // ETXTBSY, as above
        let want = DiscoveryError::Failed("failed to connect to local tailscaled".into());
        assert_eq!(run(&fail).await, Err(want));
        let t0 = Instant::now();
        let r = run(&flood).await; // stdout capped at 16 MiB, well before the 10s timeout
        assert!(
            matches!(&r, Err(DiscoveryError::Failed(e)) if e.contains("16 MiB")),
            "{r:?}"
        );
        assert!(t0.elapsed() < Duration::from_secs(5));
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// Runs the real binary. Prints counts only: tailnet data never lands in logs or commits.
    #[tokio::test]
    #[ignore = "runs the real tailscale binary"]
    async fn tailscale_live_status() {
        let o = TailscaleProvider::new("tailscale".into(), None)
            .discover()
            .await
            .unwrap();
        for m in &o {
            assert_eq!(Name::parse(m.id.as_str()).as_ref(), Ok(&m.id));
            assert!(!m.addresses.is_empty());
        }
        let online = o.iter().filter(|m| m.online == Some(true)).count();
        println!("tailscale_live_status: {} peers, {online} online", o.len());
    }
}
