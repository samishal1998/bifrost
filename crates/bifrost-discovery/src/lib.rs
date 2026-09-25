//! Network discovery providers (contract §7). Static discovery lives in the daemon (config observations).

pub mod dns;
pub mod http;
pub mod tailscale;
