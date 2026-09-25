//! HTTP inventory provider (contract §7 HTTP, A20, E2).

use bifrost_core::validate::{clean, meta_key, native_id, tag};
use bifrost_core::{
    BoxFuture, DiscoveryError, DiscoveryProvider, Host, MachineObservation, Metadata, MountHints,
    Name, RemotePath, User,
};
use reqwest::header::{ACCEPT, HeaderMap, HeaderName, HeaderValue};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::{BTreeMap, btree_map::Entry};
use std::error::Error;
use std::num::NonZeroU16;
use std::time::Duration;
use tracing::warn;

// ponytail: 10s timeout, 1 MiB body, 1000 entries, no ETag or Cache-Control (§15 #8): expensive inventories get polled in full every interval; ETag/If-None-Match
const BODY_CAP: usize = 1 << 20;
const MAX_ENTRIES: usize = 1000;

pub struct HttpProvider {
    name: String,
    url: reqwest::Url,
    client: reqwest::Client,
}

impl HttpProvider {
    /// Errors name a header key at most, never its value (values are secrets; this text reaches `last_error`).
    pub fn new(name: String, url: String, headers: Vec<(String, String)>) -> Result<Self, String> {
        let url = reqwest::Url::parse(&url).map_err(|e| format!("url: {e}"))?;
        let mut h = HeaderMap::new();
        h.insert(ACCEPT, HeaderValue::from_static("application/json"));
        for (k, v) in headers {
            let n = HeaderName::from_bytes(k.as_bytes())
                .map_err(|_| format!("header {k:?}: invalid name"))?;
            let mut v =
                HeaderValue::from_str(&v).map_err(|_| format!("header {k:?}: invalid value"))?;
            v.set_sensitive(true);
            h.insert(n, v); // a configured Accept replaces ours
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none()) // auth headers never follow a redirect elsewhere
            .user_agent(concat!("bifrost/", env!("CARGO_PKG_VERSION")))
            .default_headers(h)
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self { name, url, client })
    }
}

impl DiscoveryProvider for HttpProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>> {
        Box::pin(async move {
            let mut r = self
                .client
                .get(self.url.clone())
                .send()
                .await
                .map_err(failed)?;
            if !r.status().is_success() {
                let code = r.status().as_u16();
                return Err(DiscoveryError::Failed(format!("HTTP {code}")));
            }
            let mut body = Vec::new();
            while let Some(c) = r.chunk().await.map_err(failed)? {
                if body.len() + c.len() > BODY_CAP {
                    return Err(DiscoveryError::Failed(
                        "inventory body exceeds 1 MiB".into(),
                    ));
                }
                body.extend_from_slice(&c);
            }
            parse_inventory(&body)
        })
    }
}

/// reqwest's Display names the URL (a query may carry a token) and hides the cause: drop the one, chain the other.
fn failed(e: reqwest::Error) -> DiscoveryError {
    let e = e.without_url();
    let mut s = e.to_string();
    let mut src = e.source();
    while let Some(c) = src {
        s = format!("{s}: {c}");
        src = c.source();
    }
    DiscoveryError::Failed(clean(&s, 512))
}

#[derive(Deserialize)]
struct Top {
    machines: Vec<Value>,
}

/// One `machines[]` entry. Unknown keys are ignored, `driver` included (E2).
#[derive(Deserialize)]
struct Item {
    name: Option<String>,
    id: Option<String>,
    host: Option<String>,
    /// a Value: ignored entirely when `host` is present (A20)
    addresses: Option<Value>,
    port: Option<NonZeroU16>,
    online: Option<bool>,
    user: Option<String>,
    path: Option<String>,
    metadata: Option<Map<String, Value>>,
}

/// pure
pub fn parse_inventory(body: &[u8]) -> Result<Vec<MachineObservation>, DiscoveryError> {
    let top: Top = serde_json::from_slice(body)
        .map_err(|e| DiscoveryError::Failed(clean(&format!("inventory: {e}"), 512)))?;
    let n = top.machines.len();
    if n > MAX_ENTRIES {
        warn!("inventory lists {n} machines; only the first {MAX_ENTRIES} are used");
    }
    // ponytail: skipped entries only warn! into the log (§15 #22); upgrade: `warnings` in ProviderDto
    let mut out = BTreeMap::new();
    for (i, v) in top.machines.into_iter().take(MAX_ENTRIES).enumerate() {
        match serde_json::from_value(v)
            .map_err(Into::into)
            .and_then(|m| observation(i, m))
        {
            Err(e) => warn!("machines[{i}]: skipped: {}", clean(&e.to_string(), 512)),
            Ok(o) => match out.entry(o.id.clone()) {
                Entry::Occupied(_) => {
                    warn!(
                        "machines[{i}]: skipped: duplicate id {} (first wins)",
                        o.id.as_str()
                    )
                }
                Entry::Vacant(e) => {
                    e.insert(o);
                }
            },
        }
    }
    Ok(out.into_values().collect())
}

/// Every field is validated; any invalid one skips the whole entry. The one exception is the contract's:
/// without `host`, invalid `addresses` are dropped one by one and ≥1 valid one is needed.
fn observation(i: usize, m: Item) -> Result<MachineObservation, Box<dyn Error>> {
    let native = m.id.as_deref().map(native_id).transpose()?;
    let name = m.name.or(m.id).ok_or("no name and no id")?;
    let id = Name::parse(&name)?;
    let addresses = match (m.host, m.addresses) {
        (Some(h), _) => vec![Host::parse(&h)?], // A20: CIDR rules see exactly the connect target
        (None, Some(Value::Array(a))) => {
            let mut v = Vec::new();
            for a in a {
                match a.as_str().map(Host::parse) {
                    Some(Ok(h)) => v.push(h),
                    Some(Err(e)) => warn!(
                        "machines[{i}]: address dropped: {}",
                        clean(&e.to_string(), 512)
                    ),
                    None => warn!("machines[{i}]: address dropped: not a string"),
                }
            }
            v
        }
        (None, Some(_)) => return Err("addresses: not an array".into()),
        (None, None) => vec![],
    };
    if addresses.is_empty() {
        return Err("no valid host or address".into());
    }
    let mut metadata = Metadata::default();
    for (k, v) in m.metadata.unwrap_or_default() {
        if k == "tags" {
            let Value::Array(tags) = v else {
                return Err("metadata.tags: not an array".into());
            };
            for t in tags {
                metadata
                    .tags
                    .insert(tag(t.as_str().ok_or("metadata.tags: not a string")?)?);
            }
            continue;
        }
        let v = match v {
            Value::String(s) => clean(&s, 256),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null | Value::Array(_) | Value::Object(_) => continue, // nested: ignored
        };
        metadata.values.insert(meta_key(&k)?, v);
    }
    Ok(MachineObservation {
        id,
        name: clean(&name, 128),
        native_id: native,
        addresses,
        port: m.port.map(NonZeroU16::get),
        online: m.online,
        metadata,
        hints: MountHints {
            user: m.user.as_deref().map(User::parse).transpose()?,
            path: m.path.as_deref().map(RemotePath::parse).transpose()?,
        },
        ttl: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::{Host, Name};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn parse(v: serde_json::Value) -> Vec<MachineObservation> {
        parse_inventory(v.to_string().as_bytes()).unwrap()
    }
    fn ids(obs: &[MachineObservation]) -> Vec<&str> {
        obs.iter().map(|o| o.id.as_str()).collect()
    }
    fn hosts(o: &MachineObservation) -> Vec<&str> {
        o.addresses.iter().map(Host::as_str).collect()
    }

    #[test]
    fn inventory_prd_example() {
        // PRD §6.4, verbatim
        let obs = parse(json!({"machines": [{
            "id": "agent-01", "name": "agent-01", "addresses": ["10.20.0.4"],
            "metadata": {"tags": ["dev", "agent"]}
        }]}));
        assert_eq!(obs.len(), 1);
        let o = &obs[0];
        assert_eq!(o.id, Name::parse("agent-01").unwrap());
        assert_eq!(o.name, "agent-01");
        assert_eq!(o.native_id.as_deref(), Some("agent-01"));
        assert_eq!(hosts(o), ["10.20.0.4"]);
        assert_eq!((o.port, o.online, o.ttl), (None, None, None));
        assert_eq!(o.metadata.tags, ["agent", "dev"].map(String::from).into());
        assert!(o.metadata.values.is_empty());
        assert_eq!((&o.hints.user, &o.hints.path), (&None, &None));

        // contract §7 example: host wins (A20), hints validated, "driver" ignored (E2)
        let obs = parse(json!({"machines": [{
            "name": "Agent-01", "id": "i-0abc", "host": "agent-01.corp", "addresses": ["10.20.0.4"],
            "port": 22, "online": true, "user": "sami", "path": "/home/sami", "driver": "rclone",
            "metadata": {"tags": ["dev", "agent"], "env": "dev", "rack": 4}
        }]}));
        let o = &obs[0];
        assert_eq!((o.id.as_str(), o.name.as_str()), ("agent-01", "Agent-01"));
        assert_eq!(o.native_id.as_deref(), Some("i-0abc"));
        assert_eq!(hosts(o), ["agent-01.corp"]);
        assert_eq!((o.port, o.online), (Some(22), Some(true)));
        assert_eq!(o.hints.user.as_ref().map(|u| u.as_str()), Some("sami"));
        assert_eq!(
            o.hints.path.as_ref().map(|p| p.as_str()),
            Some("/home/sami")
        );
    }

    #[test]
    fn invalid_entries_isolated() {
        let a = json!(["10.0.0.1"]);
        let obs = parse(json!({"machines": [
            {"name": "ok-1", "addresses": a},
            42,
            {"name": "../x", "addresses": a},
            {"name": "bad-addr", "addresses": ["-oProxyCommand=x"]},
            {"name": "bad-host", "host": "-oProxyCommand=x", "addresses": a}, // A20: no fallback
            {"name": "no-addr"},
            {"name": "empty-addr", "addresses": []},
            {"name": "bad-port", "port": 70000, "addresses": a},
            {"name": "zero-port", "port": 0, "addresses": a},
            {"name": "bad-user", "user": "-oProxyCommand=x", "addresses": a},
            {"name": "bad-path", "path": "/a/../b", "addresses": a},
            {"name": "bad-id", "id": "../../etc", "addresses": a},
            {"name": "bad-online", "online": "yes", "addresses": a},
            {"name": "bad-tag", "addresses": a, "metadata": {"tags": ["ok", "no way"]}},
            {"name": "bad-key", "addresses": a, "metadata": {"Bad Key": "v"}},
            {"name": 7, "addresses": a},
            {"id": "ok-1", "addresses": ["10.0.0.9"]}, // duplicate id: first wins
            {"name": "ok-2", "addresses": ["-oProxyCommand=x", "h;rm", "10.0.0.2"]},
        ]}));
        assert_eq!(ids(&obs), ["ok-1", "ok-2"]);
        assert_eq!(hosts(&obs[0]), ["10.0.0.1"]);
        // without "host", invalid addresses are dropped one by one; ≥1 valid one is needed
        assert_eq!(hosts(&obs[1]), ["10.0.0.2"]);

        // the body as a whole unusable → Err (the registry freezes the last view)
        for body in [&b"not json"[..], b"{}", b"{\"machines\": 1}", b"[]"] {
            assert!(matches!(
                parse_inventory(body),
                Err(DiscoveryError::Failed(_))
            ));
        }
        assert!(parse(json!({"machines": []})).is_empty());
    }

    #[test]
    fn entries_capped_at_1000() {
        let m: Vec<_> = (0..1001)
            .map(|i| json!({"name": format!("m{i:04}"), "addresses": ["10.0.0.1"]}))
            .collect();
        let obs = parse(json!({ "machines": m }));
        assert_eq!(obs.len(), 1000);
        assert_eq!(obs.last().unwrap().id.as_str(), "m0999");
    }

    #[test]
    fn name_falls_back_to_id() {
        let obs = parse(json!({"machines": [
            {"id": "I-0ABC", "addresses": ["10.0.0.1"]},
            {"id": "i:colon", "addresses": ["10.0.0.1"]}, // a valid native id, not a valid Name
            {"addresses": ["10.0.0.1"]},
            {"name": "", "id": "fallback-not-used", "addresses": ["10.0.0.1"]},
        ]}));
        assert_eq!(ids(&obs), ["i-0abc"]);
        assert_eq!(obs[0].name, "I-0ABC");
        assert_eq!(obs[0].native_id.as_deref(), Some("I-0ABC"));
    }

    #[test]
    fn metadata_tags_merged_scalars_flattened() {
        let obs = parse(json!({"machines": [{
            "name": "m", "addresses": ["10.0.0.1"],
            "metadata": {
                "tags": ["dev", "Dev", "agent"], "env": "dev", "rack": 4, "gpu": true, "cost": 1.5,
                "note": "a\u{1b}[2Jb", "nested": {"a": 1}, "list": [1, 2], "none": null
            }
        }]}));
        let m = &obs[0].metadata;
        assert_eq!(m.tags, ["agent", "dev"].map(String::from).into());
        let v: Vec<(&str, &str)> = m.values.iter().map(|(k, v)| (&**k, &**v)).collect();
        assert_eq!(
            v,
            [
                ("cost", "1.5"),
                ("env", "dev"),
                ("gpu", "true"),
                ("note", "a?[2Jb"),
                ("rack", "4")
            ]
        );
        let long = "x".repeat(300);
        let obs = parse(
            json!({"machines": [{"name": "m", "addresses": ["10.0.0.1"], "metadata": {"k": long}}]}),
        );
        assert_eq!(obs[0].metadata.values["k"].len(), 256);
    }

    #[test]
    fn host_overrides_addresses() {
        // A20: include_cidrs sees exactly the connect target, so an in-range IP can't smuggle a host past it
        let obs = parse(json!({"machines": [
            {"name": "a", "host": "a.corp.", "addresses": ["10.20.0.4"]},
            {"name": "b", "host": "10.0.0.5", "addresses": "garbage, ignored"},
        ]}));
        assert_eq!(hosts(&obs[0]), ["a.corp"]);
        assert_eq!(hosts(&obs[1]), ["10.0.0.5"]);
    }

    /// One-shot HTTP/1.1 responder on 127.0.0.1:0: sends `resp` to the first request and yields the request head.
    async fn serve(resp: Vec<u8>) -> (String, tokio::task::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/inv", l.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let (mut req, mut buf) = (Vec::new(), [0; 4096]);
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = s.read(&mut buf).await.unwrap();
                assert!(n > 0, "client hung up before the request head ended");
                req.extend_from_slice(&buf[..n]);
            }
            let _ = s.write_all(&resp).await; // EPIPE is expected when the client stops at the cap
            String::from_utf8_lossy(&req).into_owned()
        });
        (url, task)
    }
    fn ok_json(body: &[u8]) -> Vec<u8> {
        let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len());
        [head.as_bytes(), body].concat()
    }
    fn provider(url: String, headers: &[(&str, &str)]) -> HttpProvider {
        let h = headers
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        HttpProvider::new("inventory".into(), url, h).unwrap()
    }

    #[tokio::test]
    async fn body_cap_enforced() {
        // exactly 1 MiB still parses
        let mut body = br#"{"machines":[{"name":"m","addresses":["10.0.0.1"]}]}"#.to_vec();
        body.resize(1 << 20, b' ');
        let (url, srv) = serve(ok_json(&body)).await;
        let obs = provider(url, &[]).discover().await.unwrap();
        assert_eq!(ids(&obs), ["m"]);
        srv.await.unwrap();

        // one byte more is Failed, read through chunk() (no Content-Length shortcut)
        body.push(b' ');
        let (url, srv) = serve(ok_json(&body)).await;
        let e = provider(url, &[]).discover().await.unwrap_err();
        assert!(
            matches!(&e, DiscoveryError::Failed(m) if m.contains("1 MiB")),
            "{e:?}"
        );
        srv.await.unwrap();
    }

    #[tokio::test]
    async fn auth_header_sent_non2xx_failed() {
        let (url, srv) =
            serve(b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n".to_vec()).await;
        let p = provider(
            url,
            &[("Authorization", "Bearer s3cret"), ("X-Team", "infra")],
        );
        assert_eq!(
            p.discover().await,
            Err(DiscoveryError::Failed("HTTP 401".into()))
        );
        let req = srv.await.unwrap().to_ascii_lowercase();
        assert!(req.starts_with("get /inv http/1.1\r\n"), "{req}");
        for h in [
            "\r\nauthorization: bearer s3cret\r\n",
            "\r\nx-team: infra\r\n",
            "\r\naccept: application/json\r\n",
            concat!(
                "\r\nuser-agent: bifrost/",
                env!("CARGO_PKG_VERSION"),
                "\r\n"
            ),
        ] {
            assert!(req.contains(h), "{h:?} missing from {req}");
        }

        // a bad header value is refused without echoing it (header values are secrets)
        let e = HttpProvider::new(
            "i".into(),
            "http://127.0.0.1/".into(),
            vec![("A".into(), "x\ny".into())],
        )
        .err()
        .unwrap();
        assert!(!e.contains("x\ny") && e.contains("\"A\""), "{e}");
    }

    #[tokio::test]
    async fn transport_error_names_cause_not_url() {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/inv?token=s3cret", l.local_addr().unwrap());
        drop(l); // nothing listens any more
        let Err(DiscoveryError::Failed(m)) = provider(url, &[]).discover().await else {
            panic!("expected Failed");
        };
        assert!(
            m.contains("onnection refused") && !m.contains("s3cret"),
            "{m}"
        );
    }

    #[tokio::test]
    async fn redirect_not_followed() {
        // the redirect target would serve a valid inventory: following it would turn this into Ok
        let (target, srv2) = serve(ok_json(br#"{"machines":[]}"#)).await;
        let resp = format!("HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\n\r\n");
        let (url, srv) = serve(resp.into_bytes()).await;
        let p = provider(url, &[("Authorization", "Bearer s3cret")]);
        assert_eq!(
            p.discover().await,
            Err(DiscoveryError::Failed("HTTP 302".into()))
        );
        srv.await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!srv2.is_finished(), "the redirect target was contacted");
        srv2.abort();
    }
}
