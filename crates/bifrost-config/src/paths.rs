//! Default locations (contract §3). STUB (S0.3): S1-B implements the bodies.

use std::path::PathBuf;

/// $BIFROST_CONFIG | ~/.config/bifrost/config.toml
pub fn config_path() -> PathBuf {
    PathBuf::new() // STUB (S1-B)
}

/// $BIFROST_STATE_DIR | $XDG_STATE_HOME/bifrost | ~/.local/state/bifrost
pub fn state_dir() -> PathBuf {
    PathBuf::new() // STUB (S1-B)
}

/// $BIFROST_SOCKET | macos ~/Library/Caches/bifrost/bifrost.sock
///  | linux $XDG_RUNTIME_DIR/bifrost/bifrost.sock | ~/.cache/bifrost/bifrost.sock
pub fn socket_path() -> PathBuf {
    PathBuf::new() // STUB (S1-B)
}
