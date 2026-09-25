//! HTTP API over the Unix socket (contract §8 Routes). `ApiCmd` and `AppState` are frozen (A3); S1-D writes
//! `router` and the routes.
#![allow(dead_code)] // router() is unused until S2-E's main.rs serves it; the S2 merge agent deletes this line

use bifrost_core::api::{ActionDto, StatusDto};
use bifrost_core::events::EventRecord;
use std::path::PathBuf;
use std::sync::Arc;
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

pub fn router(_s: AppState) -> axum::Router {
    axum::Router::new() // STUB (S1-D)
}
