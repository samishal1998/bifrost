//! `tailscale status --json` provider. STUB (S0.3): S3-H implements it.

use bifrost_core::{BoxFuture, DiscoveryError, DiscoveryProvider, MachineObservation};
use std::path::PathBuf;

pub struct TailscaleProvider {
    name: String,
}

impl TailscaleProvider {
    /// `binary`: tests only
    pub fn new(name: String, _binary: Option<PathBuf>) -> Self {
        Self { name } // STUB (S3-H)
    }
}

impl DiscoveryProvider for TailscaleProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>> {
        Box::pin(async { Err(DiscoveryError::Unavailable("not implemented".into())) })
    }
}

/// pure
pub fn parse_status(_json: &[u8]) -> Result<Vec<MachineObservation>, DiscoveryError> {
    Err(DiscoveryError::Unavailable("not implemented".into())) // STUB (S3-H)
}
