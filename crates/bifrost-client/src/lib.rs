//! HTTP-over-UDS client for bifrostd (contract §9).
//!
//! Per request: UnixStream::connect → hyper::client::conn::http1::handshake(TokioIo::new(s)) → tokio::spawn(conn)
//! → send_request(Request{Host: bifrost, Content-Type: application/json, Full<hyper::body::Bytes>})
//! → BodyExt::collect → serde_json::from_slice.
//! Every POST sends a JSON body (Content-Type is always application/json, so `&()` → `null` would be rejected):
//! the CLI always sends UnmountReq{force} for unmount and `{}` for the body-less POSTs (C6).
// ponytail: no SSE consumer, the TUI polls /v1/status every 1s (E6), so event latency is ≤1s and debugging SSE needs curl --unix-socket; upgrade: Client::events() plus a `bifrost events` command

use http_body_util::{BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Method, Request, header};
use hyper_util::rt::TokioIo;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::{self, ErrorKind};
use std::path::PathBuf;
use tokio::net::UnixStream;

pub struct Client {
    socket: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// ENOENT / ECONNREFUSED on connect
    #[error("bifrostd is not running (socket {0})")]
    NotRunning(PathBuf),
    /// non-2xx; body is ErrorDto
    #[error("{status}: {error}")]
    Api { status: u16, error: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("bad response: {0}")]
    Decode(String),
}

/// core's `ErrorDto` (the client doesn't depend on bifrost-core)
#[derive(serde::Deserialize)]
struct ErrorDto {
    error: String,
}

impl Client {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
    pub async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ClientError> {
        self.send(Method::GET, path, Vec::new()).await
    }
    pub async fn post<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<T, ClientError> {
        let body = serde_json::to_vec(body).map_err(|e| ClientError::Decode(e.to_string()))?;
        self.send(Method::POST, path, body).await
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Vec<u8>,
    ) -> Result<T, ClientError> {
        let stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|e| match e.kind() {
                ErrorKind::NotFound | ErrorKind::ConnectionRefused => {
                    ClientError::NotRunning(self.socket.clone())
                }
                _ => ClientError::Io(e),
            })?;
        let (mut tx, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(io::Error::other)?;
        tokio::spawn(conn);
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header(header::HOST, "bifrost")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Full::new(Bytes::from(body)))
            .map_err(|e| io::Error::new(ErrorKind::InvalidInput, e))?;
        let resp = tx.send_request(req).await.map_err(io::Error::other)?;
        let status = resp.status();
        let bytes = resp
            .into_body()
            .collect()
            .await
            .map_err(io::Error::other)?
            .to_bytes();
        if !status.is_success() {
            // axum's own rejections (415, 405, an unknown route) carry a text or empty body, not an ErrorDto
            let error = serde_json::from_slice::<ErrorDto>(&bytes)
                .map(|e| e.error)
                .unwrap_or_else(|_| String::from_utf8_lossy(&bytes).into_owned());
            return Err(ClientError::Api {
                status: status.as_u16(),
                error,
            });
        }
        serde_json::from_slice(&bytes).map_err(|e| ClientError::Decode(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::{get, post};
    use serde_json::{Value, json};
    use std::path::Path;

    /// A fresh, unique socket path (tests run in parallel).
    fn sock(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("bf-client-{name}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn serve(p: &Path, app: axum::Router) {
        let l = tokio::net::UnixListener::bind(p).unwrap();
        tokio::spawn(async move { axum::serve(l, app).await });
    }

    #[tokio::test]
    async fn uds_roundtrip() {
        let p = sock("roundtrip");
        serve(
            &p,
            axum::Router::new()
                .route(
                    "/v1/status",
                    get(|h: HeaderMap| async move {
                        Json(json!({"host": h["host"].to_str().unwrap()}))
                    }),
                )
                // Json<Value> rejects a missing or wrong Content-Type with 415, so this also pins the header
                .route(
                    "/v1/echo",
                    post(|Json(v): Json<Value>| async move { Json(v) }),
                ),
        );
        let c = Client::new(p.clone());
        let v: Value = c.get("/v1/status").await.unwrap();
        assert_eq!(v, json!({"host": "bifrost"}));
        let v: Value = c.post("/v1/echo", &json!({"force": true})).await.unwrap();
        assert_eq!(v, json!({"force": true}));
        let v: Value = c.post("/v1/echo", &json!({})).await.unwrap();
        assert_eq!(v, json!({}));
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn not_running_enoent_and_econnrefused() {
        let p = sock("absent");
        let e = Client::new(p.clone()).get::<Value>("/v1/status").await;
        assert!(
            matches!(e, Err(ClientError::NotRunning(ref q)) if *q == p),
            "{e:?}"
        );

        // a stale socket file with no listener behind it: ECONNREFUSED
        let p = sock("stale");
        drop(std::os::unix::net::UnixListener::bind(&p).unwrap());
        assert!(p.exists());
        let e = Client::new(p.clone()).get::<Value>("/v1/status").await;
        assert!(
            matches!(e, Err(ClientError::NotRunning(ref q)) if *q == p),
            "{e:?}"
        );
        let _ = std::fs::remove_file(p);
    }

    #[tokio::test]
    async fn api_error_maps_status_and_body() {
        let p = sock("apierr");
        serve(
            &p,
            axum::Router::new()
                .route(
                    "/v1/mounts/{id}/mount",
                    post(|| async {
                        let e = json!({"error": "discover-only (tailscale)"});
                        (StatusCode::FORBIDDEN, Json(e))
                    }),
                )
                .route("/v1/garbage", get(|| async { "not json" })),
        );
        let c = Client::new(p.clone());
        match c.post::<Value>("/v1/mounts/x/mount", &json!({})).await {
            Err(ClientError::Api { status, error }) => {
                assert_eq!((status, error.as_str()), (403, "discover-only (tailscale)"))
            }
            other => panic!("{other:?}"),
        }
        // a non-ErrorDto error body (axum's empty 404) is still Api, carrying the raw body
        match c.get::<Value>("/v1/nope").await {
            Err(ClientError::Api { status, error }) => {
                assert_eq!((status, error.as_str()), (404, ""))
            }
            other => panic!("{other:?}"),
        }
        // a 2xx that doesn't decode
        let e = c.get::<Value>("/v1/garbage").await;
        assert!(matches!(e, Err(ClientError::Decode(_))), "{e:?}");
        let _ = std::fs::remove_file(p);
    }
}
