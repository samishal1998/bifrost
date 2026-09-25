//! `bifrostd` (contract §8). STUB (S0.3): S2-E writes the startup; the wiring below is final (A3, A5).

mod actor;
mod api;
mod reload;
mod state;

use actor::Deps;
use bifrost_config::{ProviderConfig, ProviderSpec};
use bifrost_core::DiscoveryProvider;
use bifrost_discovery::{dns::DnsProvider, http::HttpProvider, tailscale::TailscaleProvider};
use bifrost_mount::DriverSettings;
use std::sync::Arc;

fn build_provider(pc: &ProviderConfig) -> Result<Arc<dyn DiscoveryProvider>, String> {
    Ok(match &pc.spec {
        ProviderSpec::Tailscale => Arc::new(TailscaleProvider::new(pc.name.clone(), None)),
        ProviderSpec::Dns {
            domain,
            nameservers,
        } => Arc::new(DnsProvider::new(
            pc.name.clone(),
            domain.clone(),
            nameservers.clone(),
        )?),
        ProviderSpec::Http { url, headers } => Arc::new(HttpProvider::new(
            pc.name.clone(),
            url.clone(),
            headers
                .iter()
                .map(|(k, v)| (k.clone(), v.0.clone()))
                .collect(),
        )?),
    })
}

fn main() {
    let _deps = Deps {
        drivers: Arc::new(|s: &DriverSettings| bifrost_mount::drivers(s)),
        build_provider: Arc::new(build_provider),
    };
    eprintln!("bifrostd: not implemented yet"); // STUB (S2-E): startup per contract §8
    std::process::exit(1);
}
