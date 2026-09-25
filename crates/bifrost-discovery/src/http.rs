//! HTTP inventory provider. STUB (S0.3): S3-K implements it.

use bifrost_core::{BoxFuture, DiscoveryError, DiscoveryProvider, MachineObservation};

pub struct HttpProvider {
    name: String,
}

impl HttpProvider {
    pub fn new(
        name: String,
        _url: String,
        _headers: Vec<(String, String)>,
    ) -> Result<Self, String> {
        Ok(Self { name }) // STUB (S3-K)
    }
}

impl DiscoveryProvider for HttpProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>> {
        Box::pin(async { Err(DiscoveryError::Unavailable("not implemented".into())) })
    }
}

/// pure
pub fn parse_inventory(_body: &[u8]) -> Result<Vec<MachineObservation>, DiscoveryError> {
    Err(DiscoveryError::Unavailable("not implemented".into())) // STUB (S3-K)
}
