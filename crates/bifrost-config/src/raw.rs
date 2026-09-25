//! serde mirror of the TOML (contract §3): every struct `#[derive(Deserialize)] #[serde(deny_unknown_fields)]`,
//! fields Option/default; `RawDiscovery` is ONE flat struct so unknown-field errors keep toml line/col.
//! Written by S1-B.
