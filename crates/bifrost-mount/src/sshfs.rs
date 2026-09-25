//! sshfs driver. STUB (S0.3): S1-C implements it.

use crate::{DriverSettings, Flavor};
use bifrost_core::{
    BoxFuture, DriverAvailability, MountDriver, MountError, MountHandle, MountRequest, MountSpec,
    MountState,
};
use std::ffi::OsString;
use std::path::Path;

pub struct SshfsDriver;

impl SshfsDriver {
    pub fn new(_s: DriverSettings) -> Self {
        SshfsDriver // STUB (S1-C)
    }
}

impl MountDriver for SshfsDriver {
    fn name(&self) -> &str {
        "sshfs"
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

/// pure
pub fn sshfs_argv(_spec: &MountSpec, _ssh_config: Option<&Path>, _f: Flavor) -> Vec<OsString> {
    vec![] // STUB (S1-C)
}
