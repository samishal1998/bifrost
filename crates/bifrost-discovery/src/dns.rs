//! DNS TXT `bf1` provider. STUB (S0.3): S3-I implements it.

use bifrost_core::{
    BoxFuture, DiscoveryError, DiscoveryProvider, Host, Invalid, MachineObservation, RemotePath,
    User,
};
use std::net::SocketAddr;
use std::time::Duration;

pub struct DnsProvider {
    name: String,
}

impl DnsProvider {
    pub fn new(name: String, _domain: Host, _nameservers: Vec<SocketAddr>) -> Result<Self, String> {
        Ok(Self { name }) // STUB (S3-I)
    }
}

impl DiscoveryProvider for DnsProvider {
    fn name(&self) -> &str {
        &self.name
    }
    fn discover(&self) -> BoxFuture<'_, Result<Vec<MachineObservation>, DiscoveryError>> {
        Box::pin(async { Err(DiscoveryError::Unavailable("not implemented".into())) })
    }
}

/// no driver (E2)
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Bf1 {
    pub nodes: Vec<String>,
    pub host: Option<Host>,
    pub port: Option<u16>,
    pub user: Option<User>,
    pub tags: Vec<String>,
    pub path: Option<RemotePath>,
    pub id: Option<String>,
}

/// ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$ (no dots). Lives here, not in core (B8).
pub fn dns_label(s: &str) -> Result<String, Invalid> {
    Err(Invalid {
        what: "dns label",
        value: s.to_string(),
        why: "not implemented",
    }) // STUB (S3-I)
}

/// Ok(None) = not a bf1 record (ignore silently)
pub fn parse_bf1(_txt: &str) -> Result<Option<Bf1>, String> {
    Ok(None) // STUB (S3-I)
}

/// pure
pub fn node_observation(
    _label: &str,
    _domain: &Host,
    _r: &Bf1,
    _ttl: Duration,
) -> Result<MachineObservation, String> {
    Err("not implemented".into()) // STUB (S3-I)
}
