//! Timed filesystem checks, binary lookup and helper commands. STUB (S0.3): S1-C implements the bodies.

use bifrost_core::{MountError, MountSpec, MountState};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// None = timed out, or already in flight for `key` (no new thread is spawned).
pub async fn timed<T: Send + 'static>(
    _key: &Path,
    _dur: Duration,
    _f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Option<std::io::Result<T>> {
    None // STUB (S1-C)
}

/// symlink_metadata(path/".bifrost-probe-<16hex nonce>") under timed(5s):
/// any server reply — Ok, NotFound, PermissionDenied — → Healthy (A17: an unsearchable remote root is not a fault);
/// NotConnected (macOS also raw errno 6 ENXIO) → Stale; other → Degraded(e); None → Degraded("unresponsive").
pub async fn liveness(_path: &Path) -> MountState {
    MountState::Degraded("not implemented".into()) // STUB (S1-C)
}

/// `path` (a PATH-style list) + /usr/local/bin:/usr/bin:/bin (+ macos /opt/homebrew/bin); is_file ∧ mode & 0o111.
/// Tests pass their own `path` and never call `set_var` (unsafe in edition 2024, racy across test threads) (B14).
pub fn which_in(_name: &str, _path: &OsStr) -> Option<PathBuf> {
    None // STUB (S1-C)
}

/// which_in(name, $PATH)
pub fn which(_name: &str) -> Option<PathBuf> {
    None // STUB (S1-C)
}

/// stdin null, kill_on_drop(true)
pub async fn run(
    _bin: &Path,
    _args: &[OsString],
    _t: Duration,
) -> std::io::Result<std::process::Output> {
    Err(std::io::Error::other("not implemented")) // STUB (S1-C)
}

pub async fn ssh_preflight(
    _ssh: &Path,
    _spec: &MountSpec,
    _ssh_config: Option<&Path>,
) -> Result<(), MountError> {
    Err(MountError::Unavailable("not implemented".into())) // STUB (S1-C)
}
