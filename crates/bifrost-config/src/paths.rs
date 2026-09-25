//! Default locations (contract §3). An empty variable counts as unset; relative XDG values are ignored (XDG spec).

use std::path::PathBuf;

fn var(k: &str) -> Option<PathBuf> {
    std::env::var_os(k)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

fn xdg(k: &str) -> Option<PathBuf> {
    var(k).filter(|p| p.is_absolute())
}

/// Panics rather than resolve a default against the cwd (a daemon started in /tmp, a client in a hostile dir).
/// Only reached when the BIFROST_* override is unset.
fn home() -> PathBuf {
    std::env::home_dir()
        .filter(|h| h.is_absolute())
        .expect("HOME is not an absolute path")
}

/// $BIFROST_CONFIG | ~/.config/bifrost/config.toml
pub fn config_path() -> PathBuf {
    var("BIFROST_CONFIG").unwrap_or_else(|| home().join(".config/bifrost/config.toml"))
}

/// $BIFROST_STATE_DIR | $XDG_STATE_HOME/bifrost | ~/.local/state/bifrost
pub fn state_dir() -> PathBuf {
    var("BIFROST_STATE_DIR")
        .or_else(|| xdg("XDG_STATE_HOME").map(|d| d.join("bifrost")))
        .unwrap_or_else(|| home().join(".local/state/bifrost"))
}

/// $BIFROST_SOCKET | macos ~/Library/Caches/bifrost/bifrost.sock
///  | linux $XDG_RUNTIME_DIR/bifrost/bifrost.sock | ~/.cache/bifrost/bifrost.sock
pub fn socket_path() -> PathBuf {
    if let Some(s) = var("BIFROST_SOCKET") {
        return s;
    }
    if cfg!(target_os = "macos") {
        return home().join("Library/Caches/bifrost/bifrost.sock");
    }
    match xdg("XDG_RUNTIME_DIR") {
        Some(d) => d.join("bifrost/bifrost.sock"),
        None => home().join(".cache/bifrost/bifrost.sock"),
    }
}
