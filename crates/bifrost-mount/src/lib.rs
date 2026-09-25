//! Mount drivers (contract §6). STUB (S0.3): S1-C implements lib/table/check/sshfs, S3-J rclone.

pub mod check;
mod rclone;
mod sshfs;
pub mod table;

pub use rclone::{RcloneDriver, rclone_argv};
pub use sshfs::{SshfsDriver, sshfs_argv};

use bifrost_core::{MountDriver, MountError, MountHandle, MountId, MountSpec};
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverSettings {
    pub ssh_config: Option<PathBuf>,
    pub vfs_cache_mode: String,
    pub mount_timeout: Duration,
    /// rclone --cache-dir=<state>/rclone/<id> (A23)
    pub state_dir: PathBuf,
}

/// Every OS gets all three; rclone-nfs probes Unavailable("macOS only") on Linux.
pub fn drivers(s: &DriverSettings) -> Vec<Arc<dyn MountDriver>> {
    vec![
        Arc::new(SshfsDriver::new(s.clone())),
        Arc::new(RcloneDriver::new(s.clone(), false)),
        Arc::new(RcloneDriver::new(s.clone(), true)),
    ]
}

// ponytail: SSH timings are constants (a dead link takes >=45s to detect; the command line overrides ssh_config values); an [ssh] section if users need tuning
pub const SSH_OPTS: [&str; 6] = [
    "BatchMode=yes",
    "ConnectTimeout=10",
    "ServerAliveInterval=15",
    "ServerAliveCountMax=3",
    "ControlMaster=no",
    "ControlPath=none",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Linux,
    MacFuse,
    FuseT,
}

/// pure
pub fn preflight_argv(_spec: &MountSpec, _ssh_config: Option<&Path>) -> Vec<OsString> {
    vec![] // STUB (S1-C)
}

pub async fn unmount_path(_path: &Path, _force: bool) -> Result<(), MountError> {
    Ok(()) // STUB (S1-C)
}

/// Pure. Only entries whose parent == root. Marker source ⇒ adopt (driver from fstype, else record, else "sshfs");
/// no marker ⇒ adopt only when state.json has a record with the same local_path (macOS NFS fallback); else foreign.
/// `pid` comes from the state record with the same local_path, else None (B15).
pub fn adopt(
    _entries: &[table::MountEntry],
    _root: &Path,
    _records: &BTreeMap<MountId, MountHandle>,
) -> Vec<MountHandle> {
    vec![] // STUB (S1-C)
}
