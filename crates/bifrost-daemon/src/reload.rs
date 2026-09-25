//! Config file poller (contract §8 Config hot reload): produces `Msg::Config` only; applying it is the actor's (A4).
#![allow(dead_code)] // S0 stub: not called yet; S4-M deletes this line

use std::path::PathBuf;
use tokio::sync::mpsc;

use crate::actor::Msg;

/// STUB (S4-M): no-op, so SIGHUP keeps its default action until S4-M installs the handler.
pub fn spawn_poller(_path: PathBuf, _tx: mpsc::UnboundedSender<Msg>) {}
