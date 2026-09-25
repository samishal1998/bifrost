//! HTTP API over the Unix socket (contract §8 Routes). `ApiCmd` and `AppState` are frozen (A3); S1-D writes
//! `router` and the routes.
#![allow(dead_code)] // router() is unused until S2-E's main.rs serves it; the S2 merge agent deletes this line

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bifrost_core::Name;
use bifrost_core::api::{ActionDto, ErrorDto, LogDto, ReloadDto, StatusDto, UnmountReq};
use bifrost_core::events::EventRecord;
use bifrost_core::validate::clean;
use futures_util::Stream;
use std::convert::Infallible;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use crate::actor::Msg;

pub struct ApiError {
    pub status: u16,
    pub error: String,
}

pub enum ApiCmd {
    Mount {
        target: String,
        reply: oneshot::Sender<Result<Vec<String>, ApiError>>,
    },
    Unmount {
        target: String,
        force: bool,
        reply: oneshot::Sender<Result<Vec<String>, ApiError>>,
    },
    Reconcile {
        reply: oneshot::Sender<Vec<ActionDto>>,
    },
    /// no reply: the route returns 202 at once (E4)
    Discover,
} // no Reload: the route sends Msg::Config itself (A4)

#[derive(Clone)]
pub struct AppState {
    pub snapshot: watch::Receiver<Arc<StatusDto>>,
    pub tx: mpsc::UnboundedSender<Msg>,
    pub events: broadcast::Sender<EventRecord>,
    pub state_dir: PathBuf,
    pub config_path: PathBuf,
}

/// Every error body is an `ErrorDto`.
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let code = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (code, Json(ErrorDto { error: self.error })).into_response()
    }
}

fn err(status: u16, error: impl ToString) -> ApiError {
    ApiError {
        status,
        error: error.to_string(),
    }
}

/// The actor's inbox or reply channel is closed: it is shutting down.
fn gone<E>(_: E) -> ApiError {
    err(503, "bifrostd is shutting down")
}

pub fn router(s: AppState) -> axum::Router {
    // GET status routes only read the watch channel, never the actor (C5). No /v1/machines/{id} (E1).
    axum::Router::new()
        .route(
            "/v1/status",
            get(|State(s): State<AppState>| async move { Json(StatusDto::clone(&snap(&s))) }),
        )
        .route(
            "/v1/machines",
            get(|State(s): State<AppState>| async move { Json(snap(&s).machines.clone()) }),
        )
        .route(
            "/v1/mounts",
            get(|State(s): State<AppState>| async move { Json(snap(&s).mounts.clone()) }),
        )
        .route(
            "/v1/drivers",
            get(|State(s): State<AppState>| async move { Json(snap(&s).drivers.clone()) }),
        )
        .route("/v1/mounts/{id}/log", get(log))
        .route("/v1/mounts/{target}/mount", post(mount))
        .route("/v1/mounts/{target}/unmount", post(unmount))
        .route("/v1/discover", post(discover))
        .route("/v1/reconcile", post(reconcile))
        .route("/v1/config/reload", post(reload))
        .route("/v1/events", get(events))
        .with_state(s)
}

fn snap(s: &AppState) -> Arc<StatusDto> {
    s.snapshot.borrow().clone()
}

/// Sends `cmd` to the actor and waits for its reply.
async fn ask<T>(
    s: &AppState,
    cmd: impl FnOnce(oneshot::Sender<T>) -> ApiCmd,
) -> Result<T, ApiError> {
    let (tx, rx) = oneshot::channel();
    s.tx.send(Msg::Api(cmd(tx))).map_err(gone)?;
    rx.await.map_err(gone)
}

// Body-less POSTs take no body extractor, so `{}` (the client's C6 body) is accepted and ignored.
async fn mount(
    State(s): State<AppState>,
    Path(target): Path<String>,
) -> Result<(StatusCode, Json<Vec<String>>), ApiError> {
    let ids = ask(&s, |reply| ApiCmd::Mount { target, reply }).await??;
    Ok((StatusCode::ACCEPTED, Json(ids)))
}

async fn unmount(
    State(s): State<AppState>,
    Path(target): Path<String>,
    Json(req): Json<UnmountReq>,
) -> Result<(StatusCode, Json<Vec<String>>), ApiError> {
    let force = req.force;
    let ids = ask(&s, |reply| ApiCmd::Unmount {
        target,
        force,
        reply,
    })
    .await??;
    Ok((StatusCode::ACCEPTED, Json(ids)))
}

async fn reconcile(State(s): State<AppState>) -> Result<Json<Vec<ActionDto>>, ApiError> {
    ask(&s, |reply| ApiCmd::Reconcile { reply }).await.map(Json)
}

/// Notify and return at once (E4); the CLI polls status for the providers' `refreshes`.
async fn discover(
    State(s): State<AppState>,
) -> Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    s.tx.send(Msg::Api(ApiCmd::Discover)).map_err(gone)?;
    Ok((StatusCode::ACCEPTED, Json(serde_json::json!({}))))
}

/// A4: load off the runtime, then the actor applies it through its one config-apply path and replies.
async fn reload(State(s): State<AppState>) -> Result<Json<ReloadDto>, ApiError> {
    let path = s.config_path.clone();
    let result = tokio::task::spawn_blocking(move || bifrost_config::load(&path))
        .await
        .map_err(|e| err(500, e))?;
    let (tx, rx) = oneshot::channel();
    let msg = Msg::Config {
        result: result.map(Box::new),
        reply: Some(tx),
    };
    s.tx.send(msg).map_err(gone)?;
    rx.await.map(Json).map_err(gone)
}

const LOG_TAIL: u64 = 64 * 1024;

/// Last 64 KiB of `<state>/logs/<id>.log`. `Name::parse` makes `id` one plain path component (no traversal).
async fn log(State(s): State<AppState>, Path(id): Path<String>) -> Result<Json<LogDto>, ApiError> {
    let id = Name::parse(&id).map_err(|e| err(400, e))?;
    let path = s
        .state_dir
        .join("logs")
        .join(format!("{}.log", id.as_str()));
    let p = path.clone();
    let text = tokio::task::spawn_blocking(move || {
        let mut f = std::fs::File::open(p)?;
        let start = f.metadata()?.len().saturating_sub(LOG_TAIL + 1);
        f.seek(SeekFrom::Start(start))?;
        let mut buf = Vec::new();
        f.take(LOG_TAIL + 1).read_to_end(&mut buf)?;
        if start > 0 {
            // buf[0] is the byte before the 64 KiB window: drop through the first '\n' (just that byte if it is
            // one); no '\n' at all means one huge line, so keep the fragment (clean() caps it at 512)
            let cut = buf.iter().position(|&b| b == b'\n').map_or(0, |i| i + 1);
            buf.drain(..cut);
        }
        std::io::Result::Ok(String::from_utf8_lossy(&buf).into_owned())
    })
    .await
    .map_err(|e| err(500, e))?
    .map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => err(404, format!("no log for mount {}", id.as_str())),
        _ => err(500, format!("{}: {e}", path.display())),
    })?;
    Ok(Json(LogDto {
        mount: id.as_str().to_string(),
        path: path.display().to_string(),
        lines: text.lines().map(|l| clean(l, 512)).collect(),
    }))
}

async fn events(State(s): State<AppState>) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let stream = futures_util::stream::unfold(s.events.subscribe(), |mut rx| async move {
        let ev = match rx.recv().await {
            Ok(r) => {
                // the enum's serde tag is the SSE event name
                let kind = serde_json::to_value(&r.event).ok();
                let kind = kind
                    .as_ref()
                    .and_then(|v| v["type"].as_str())
                    .unwrap_or("event");
                let data = serde_json::to_string(&r).unwrap_or_default();
                Event::default()
                    .id(r.seq.to_string())
                    .event(kind)
                    .data(data)
            }
            Err(RecvError::Lagged(n)) => Event::default()
                .event("lagged")
                .data(format!("{{\"skipped\":{n}}}")),
            Err(RecvError::Closed) => return None,
        };
        Some((Ok(ev), rx))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::State;
    use axum::response::IntoResponse;
    use bifrost_client::{Client, ClientError};
    use bifrost_core::api::{DriverDto, LogDto, MachineDto, MountDto, ProviderDto, ReloadDto};
    use bifrost_core::events::Event;
    use bifrost_core::reconcile::Availability;
    use serde_json::{Value, json};
    use std::path::Path;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn fixture() -> StatusDto {
        StatusDto {
            version: "0.1.0".into(),
            pid: 4242,
            uptime_secs: 7,
            socket: "/run/user/1000/bifrost/bifrost.sock".into(),
            config_path: "/home/sami/.config/bifrost/config.toml".into(),
            config_errors: vec![],
            mount_root: "/home/sami/machines".into(),
            ready: true,
            ssh_agent: true,
            providers: vec![ProviderDto {
                name: "tailscale".into(),
                kind: "tailscale".into(),
                machines: 1,
                refreshes: 3,
                last_ok_secs_ago: Some(12),
                last_error: None,
            }],
            drivers: vec![DriverDto {
                name: "sshfs".into(),
                available: true,
                binary: Some("/usr/bin/sshfs".into()),
                detail: "SSHFS version 3.7.3".into(),
            }],
            auto_driver: Some("sshfs".into()),
            machines: vec![MachineDto {
                id: "agent-01".into(),
                name: "agent-01".into(),
                source: "static".into(),
                shadowed: vec![],
                address: "agent-01.tail1234.ts.net".into(),
                port: None,
                online: Some(true),
                tags: vec!["agent".into()],
                metadata: Default::default(),
                verdict: "allowed (static)".into(),
                state: Availability::Mounted,
                mounts: vec!["agent-01".into()],
            }],
            mounts: vec![MountDto {
                id: "agent-01".into(),
                machine: "agent-01".into(),
                driver: Some("sshfs".into()),
                local_path: "/home/sami/machines/agent-01".into(),
                remote: "sami@agent-01.tail1234.ts.net:/home/sami".into(),
                state: Availability::Mounted,
                detail: String::new(),
                desired: true,
                held: false,
                adopted: false,
                pid: Some(4242),
                failures: 0,
                retry_in_secs: None,
                last_error: None,
                action: "noop".into(),
            }],
            conflicts: vec![],
            events: vec![],
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("bf-api-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        dir
    }

    /// AppState over the fixture snapshot; the test owns the actor's inbox.
    fn state(dir: &Path) -> (AppState, mpsc::UnboundedReceiver<Msg>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let s = AppState {
            snapshot: watch::channel(Arc::new(fixture())).1,
            tx,
            events: broadcast::channel(16).0,
            state_dir: dir.to_path_buf(),
            config_path: dir.join("config.toml"),
        };
        (s, rx)
    }

    /// Serves router() on <dir>/s.sock.
    fn serve(dir: &Path) -> mpsc::UnboundedReceiver<Msg> {
        let (s, rx) = state(dir);
        let l = tokio::net::UnixListener::bind(dir.join("s.sock")).unwrap();
        tokio::spawn(async move { axum::serve(l, router(s)).await });
        rx
    }

    /// Raw HTTP/1.1 with `Connection: close`, so a test sees the exact status code.
    async fn raw(dir: &Path, method: &str, path: &str, body: &str) -> (u16, String) {
        let mut s = tokio::net::UnixStream::connect(dir.join("s.sock"))
            .await
            .unwrap();
        let n = body.len();
        let req = format!(
            "{method} {path} HTTP/1.1\r\nHost: bifrost\r\nConnection: close\r\n\
             Content-Type: application/json\r\nContent-Length: {n}\r\n\r\n{body}"
        );
        s.write_all(req.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        let (head, body) = out.split_once("\r\n\r\n").unwrap();
        (head[9..12].parse().unwrap(), body.to_string())
    }

    /// Stub actor: answers ApiCmd replies the way the real actor would for the fixture.
    fn stub_actor(mut rx: mpsc::UnboundedReceiver<Msg>) {
        tokio::spawn(async move {
            while let Some(m) = rx.recv().await {
                match m {
                    Msg::Api(ApiCmd::Mount { target, reply }) => {
                        let _ = reply.send(match target.as_str() {
                            "agent-01" => Ok(vec![target]),
                            "tail-01" => Err(ApiError {
                                status: 403,
                                error: "tail-01: discover-only (tailscale)".into(),
                            }),
                            _ => Err(ApiError {
                                status: 404,
                                error: format!("unknown target {target}"),
                            }),
                        });
                    }
                    Msg::Api(ApiCmd::Unmount {
                        target,
                        force,
                        reply,
                    }) => {
                        let _ = reply.send(Ok(vec![format!("{target} force={force}")]));
                    }
                    Msg::Api(ApiCmd::Reconcile { reply }) => {
                        let _ = reply.send(vec![ActionDto {
                            mount: "agent-01".into(),
                            action: "noop".into(),
                        }]);
                    }
                    // echoes load's result, so the reload test sees what the route really loaded
                    Msg::Config {
                        result,
                        reply: Some(reply),
                    } => {
                        let _ = reply.send(ReloadDto {
                            ok: result.is_ok(),
                            errors: result
                                .err()
                                .unwrap_or_default()
                                .iter()
                                .map(ToString::to_string)
                                .collect(),
                        });
                    }
                    _ => {}
                }
            }
        });
    }

    fn val(v: impl serde::Serialize) -> Value {
        serde_json::to_value(v).unwrap()
    }

    #[tokio::test]
    async fn routes_roundtrip() {
        let dir = tmp("routes");
        stub_actor(serve(&dir));
        let c = Client::new(dir.join("s.sock"));
        let f = fixture();
        assert_eq!(c.get::<Value>("/v1/status").await.unwrap(), val(&f));
        assert_eq!(
            c.get::<Value>("/v1/machines").await.unwrap(),
            val(&f.machines)
        );
        assert_eq!(c.get::<Value>("/v1/mounts").await.unwrap(), val(&f.mounts));
        assert_eq!(
            c.get::<Value>("/v1/drivers").await.unwrap(),
            val(&f.drivers)
        );
        // E1: no per-machine route
        let (code, _) = raw(&dir, "GET", "/v1/machines/agent-01", "").await;
        assert_eq!(code, 404);

        let (code, body) = raw(&dir, "POST", "/v1/mounts/agent-01/mount", "{}").await;
        assert_eq!((code, body.as_str()), (202, r#"["agent-01"]"#));
        let (code, body) = raw(
            &dir,
            "POST",
            "/v1/mounts/agent-01/unmount",
            r#"{"force":true}"#,
        )
        .await;
        assert_eq!((code, body.as_str()), (202, r#"["agent-01 force=true"]"#));
        let (code, body) = raw(&dir, "POST", "/v1/mounts/agent-01/unmount", "{}").await;
        assert_eq!((code, body.as_str()), (202, r#"["agent-01 force=false"]"#));
        let (code, body) = raw(&dir, "POST", "/v1/reconcile", "{}").await;
        assert_eq!(
            (code, body.as_str()),
            (200, r#"[{"mount":"agent-01","action":"noop"}]"#)
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn mount_forbidden_maps_403() {
        let dir = tmp("forbidden");
        stub_actor(serve(&dir));
        let c = Client::new(dir.join("s.sock"));
        match c
            .post::<Vec<String>>("/v1/mounts/tail-01/mount", &json!({}))
            .await
        {
            Err(ClientError::Api { status, error }) => {
                assert_eq!(
                    (status, error.as_str()),
                    (403, "tail-01: discover-only (tailscale)")
                )
            }
            other => panic!("{other:?}"),
        }
        match c
            .post::<Vec<String>>("/v1/mounts/ghost/mount", &json!({}))
            .await
        {
            Err(ClientError::Api { status, error }) => {
                assert_eq!((status, error.as_str()), (404, "unknown target ghost"))
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn discover_returns_202_without_waiting() {
        let dir = tmp("discover");
        let mut rx = serve(&dir); // nobody reads the inbox
        let (code, body) = raw(&dir, "POST", "/v1/discover", "{}").await;
        assert_eq!((code, body.as_str()), (202, "{}"));
        assert!(matches!(rx.try_recv(), Ok(Msg::Api(ApiCmd::Discover))));
        // actor gone: 503 with an ErrorDto instead of a hang or a panic
        drop(rx);
        let (code, body) = raw(&dir, "POST", "/v1/discover", "{}").await;
        assert_eq!(code, 503);
        assert!(serde_json::from_str::<Value>(&body).unwrap()["error"].is_string());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn reload_route_sends_config_msg() {
        let dir = tmp("reload");
        stub_actor(serve(&dir));
        std::fs::write(dir.join("config.toml"), "[[[").unwrap();
        let c = Client::new(dir.join("s.sock"));
        // the stub echoes Msg::Config's result, so errors back prove the route awaited a real load(config_path)
        let dto: ReloadDto = c.post("/v1/config/reload", &json!({})).await.unwrap();
        assert!(!dto.ok && !dto.errors.is_empty(), "{dto:?}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn log_route_rejects_traversal() {
        let dir = tmp("log");
        serve(&dir);
        std::fs::write(dir.join("secret.log"), "TOPSECRET\n").unwrap();
        for id in ["..%2Fsecret", "..", "%2E%2E%2Fsecret", ".hidden", "a%2Fb"] {
            let (code, body) = raw(&dir, "GET", &format!("/v1/mounts/{id}/log"), "").await;
            assert_eq!(code, 400, "{id}: {body}");
            assert!(!body.contains("TOPSECRET"));
        }
        let (code, _) = raw(&dir, "GET", "/v1/mounts/ghost/log", "").await;
        assert_eq!(code, 404);

        // last 64 KiB only (the cut partial line is dropped), clean()ed per line, id lowercased by Name::parse
        let log = format!(
            "# bifrost exec: sshfs\n{}\n\x1b[31mred\r\nlast\n",
            "x".repeat(70_000)
        );
        std::fs::write(dir.join("logs/agent-01.log"), log).unwrap();
        let dto: LogDto = Client::new(dir.join("s.sock"))
            .get("/v1/mounts/AGENT-01/log")
            .await
            .unwrap();
        assert_eq!(dto.mount, "agent-01");
        assert_eq!(dto.lines, ["?[31mred", "last"]);
        assert!(dto.path.ends_with("logs/agent-01.log"));

        // the cut lands exactly on a line start: that line is kept whole, not dropped
        std::fs::write(
            dir.join("logs/b.log"),
            format!("first\n{}\n", "y".repeat(65535)),
        )
        .unwrap();
        let (code, body) = raw(&dir, "GET", "/v1/mounts/b/log", "").await;
        let lines = serde_json::from_str::<Value>(&body).unwrap()["lines"].clone();
        assert_eq!((code, lines.as_array().map(Vec::len)), (200, Some(1)));
        // no '\n' in the window (one huge line still being written): its fragment is kept, not dropped
        std::fs::write(dir.join("logs/c.log"), "z".repeat(70_000)).unwrap();
        let (code, body) = raw(&dir, "GET", "/v1/mounts/c/log", "").await;
        let lines = serde_json::from_str::<Value>(&body).unwrap()["lines"].clone();
        assert_eq!((code, lines.as_array().map(Vec::len)), (200, Some(1)));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn sse_frame_format() {
        let dir = tmp("sse");
        let (s, _rx) = state(&dir);
        let tx = s.events.clone();
        let resp = events(State(s)).await.into_response();
        assert_eq!(resp.headers()["content-type"], "text/event-stream");
        // 18 records into the 16-slot channel before the first recv: the receiver lags by 2
        for seq in 1..=18 {
            let event = Event::MountHealthy {
                mount: "agent-01".into(),
            };
            tx.send(EventRecord {
                seq,
                ts_unix_ms: 1_790_000_000_000,
                event,
            })
            .unwrap();
        }
        drop(tx); // closes the channel, so the stream (and the body) ends after draining
        let body = axum::body::to_bytes(resp.into_body(), 1 << 20)
            .await
            .unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        let frame = |seq: u64| {
            format!(
                "id: {seq}\nevent: MountHealthy\ndata: {{\"seq\":{seq},\"ts_unix_ms\":1790000000000,\
                 \"event\":{{\"type\":\"MountHealthy\",\"mount\":\"agent-01\"}}}}\n\n"
            )
        };
        let want = format!(
            "event: lagged\ndata: {{\"skipped\":2}}\n\n{}",
            (3..=18).map(frame).collect::<String>()
        );
        assert_eq!(body, want);
        let _ = std::fs::remove_dir_all(dir);
    }
}
