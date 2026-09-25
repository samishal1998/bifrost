//! HTTP-over-UDS client for bifrostd (contract §9). STUB (S0.3): S1-D implements it.
//!
//! Per request: UnixStream::connect → hyper::client::conn::http1::handshake(TokioIo::new(s)) → tokio::spawn(conn)
//! → send_request(Request{Host: bifrost, Content-Type: application/json, Full<hyper::body::Bytes>})
//! → BodyExt::collect → serde_json::from_slice.
//! Every POST sends a JSON body (Content-Type is always application/json, so `&()` → `null` would be rejected):
//! the CLI always sends UnmountReq{force} for unmount and `{}` for the body-less POSTs (C6).

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::PathBuf;

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

impl Client {
    pub fn new(socket: PathBuf) -> Self {
        Self { socket }
    }
    pub async fn get<T: DeserializeOwned>(&self, _path: &str) -> Result<T, ClientError> {
        Err(ClientError::NotRunning(self.socket.clone())) // STUB (S1-D)
    }
    pub async fn post<T: DeserializeOwned>(
        &self,
        _path: &str,
        _body: &impl Serialize,
    ) -> Result<T, ClientError> {
        Err(ClientError::NotRunning(self.socket.clone())) // STUB (S1-D)
    }
}
