//! rclone driver ("rclone" = FUSE mount, "rclone-nfs" = macOS nfsmount). STUB (S0.3): S3-J implements it.

use crate::{DriverSettings, Flavor};
use bifrost_core::{
    BoxFuture, DriverAvailability, MountDriver, MountError, MountHandle, MountRequest, MountSpec,
    MountState,
};
use std::ffi::OsString;
use std::path::Path;

pub struct RcloneDriver {
    nfs: bool,
}

impl RcloneDriver {
    /// "rclone" / "rclone-nfs"
    pub fn new(_s: DriverSettings, nfs: bool) -> Self {
        Self { nfs } // STUB (S3-J)
    }
}

impl MountDriver for RcloneDriver {
    fn name(&self) -> &str {
        if self.nfs { "rclone-nfs" } else { "rclone" }
    }
    fn probe(&self) -> BoxFuture<'_, DriverAvailability> {
        Box::pin(async { DriverAvailability::Unavailable("not implemented".into()) })
    }
    fn mount(&self, _req: MountRequest) -> BoxFuture<'_, Result<MountHandle, MountError>> {
        Box::pin(async { Err(MountError::Unavailable("not implemented".into())) })
    }
    fn inspect<'a>(&'a self, _h: &'a MountHandle) -> BoxFuture<'a, MountState> {
        Box::pin(async { MountState::Missing })
    }
    fn unmount<'a>(
        &'a self,
        _h: &'a MountHandle,
        _force: bool,
    ) -> BoxFuture<'a, Result<(), MountError>> {
        Box::pin(async { Ok(()) })
    }
}

/// pure; the S0 stub returns vec![] (B13)
pub fn rclone_argv(
    _spec: &MountSpec,
    _ssh: &Path,
    _ssh_config: Option<&Path>,
    _vfs: &str,
    _cache_dir: &Path,
    _nfs: bool,
    _f: Flavor,
) -> Vec<OsString> {
    vec![] // STUB (S3-J)
}
