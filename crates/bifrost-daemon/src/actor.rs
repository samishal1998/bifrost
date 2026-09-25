//! The single actor task that owns all daemon state (contract §5, §8). `Msg`, `Tick`, `Deps` and the
//! `spawn` signature are frozen (A3); S2-E writes the bodies.
#![allow(dead_code)] // S0 stub: nothing constructs these yet; S2-E deletes this line

use bifrost_config::{Config, ConfigError, ProviderConfig};
use bifrost_core::api::{ReloadDto, StatusDto};
use bifrost_core::events::EventRecord;
use bifrost_core::reconcile::Reason;
use bifrost_core::{
    DiscoveryError, DiscoveryProvider, DriverAvailability, MachineObservation, MountDriver,
    MountError, MountHandle, MountId, MountState,
};
use bifrost_mount::DriverSettings;
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio::task::JoinHandle;

use crate::api::ApiCmd;

pub enum Msg {
    Discovery {
        provider: String,
        task_gen: u64,
        result: Result<Vec<MachineObservation>, DiscoveryError>,
    },
    MountDone {
        id: MountId,
        generation: u64,
        result: Result<MountHandle, MountError>,
    },
    UnmountDone {
        id: MountId,
        generation: u64,
        why: Reason,
        result: Result<(), MountError>,
    },
    Health {
        id: MountId,
        generation: u64,
        state: MountState,
    },
    ChildExited {
        id: MountId,
        generation: u64,
        detail: String,
    },
    Probed(BTreeMap<String, DriverAvailability>),
    Config {
        result: Result<Box<Config>, Vec<ConfigError>>,
        reply: Option<oneshot::Sender<ReloadDto>>,
    },
    Api(ApiCmd),
    Tick(Tick),
    /// Abort provider tasks, wait ≤5s for in-flight executors, write state.json, then reply (contract §8 shutdown).
    Shutdown(oneshot::Sender<()>),
}

pub enum Tick {
    Health,
    Fallback,
}

#[allow(clippy::type_complexity)] // frozen shape (A5)
pub struct Deps {
    /// Factory, not a Vec (A5): a reload that changes driver settings rebuilds through it, so daemon tests keep their FakeDrivers.
    pub drivers: Arc<dyn Fn(&DriverSettings) -> Vec<Arc<dyn MountDriver>> + Send + Sync>,
    pub build_provider:
        Arc<dyn Fn(&ProviderConfig) -> Result<Arc<dyn DiscoveryProvider>, String> + Send + Sync>,
}

/// `cfg_loaded`: false ⇒ `cfg` is the empty default because no config file exists yet; `ready` stays false until a
/// `Msg::Config` with Ok is applied (A6). The grace window runs from `cfg_loaded_at`, the first successful config
/// load (spawn time when `cfg_loaded`, else when that first `Msg::Config` applies), not from spawn. `socket` fills
/// `StatusDto.socket` (B11).
#[allow(clippy::too_many_arguments)]
pub fn spawn(
    _cfg: Config,
    _cfg_loaded: bool,
    _root: PathBuf,
    _state_dir: PathBuf,
    _socket: PathBuf,
    _held: BTreeSet<MountId>,
    _adopted: Vec<MountHandle>,
    _deps: Deps,
) -> (
    mpsc::UnboundedSender<Msg>,
    watch::Receiver<Arc<StatusDto>>,
    broadcast::Sender<EventRecord>,
    JoinHandle<()>,
) {
    todo!("S2-E: unreachable until the S2-E startup calls it")
}
