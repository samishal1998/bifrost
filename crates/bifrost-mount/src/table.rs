//! OS mount table. STUB (S0.3): S1-C implements the bodies.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountEntry {
    pub mount_point: PathBuf,
    pub fstype: String,
    pub source: String,
}

/// linux: /proc/self/mountinfo; macos: getmntinfo(MNT_NOWAIT) under a static Mutex
pub fn read() -> std::io::Result<Vec<MountEntry>> {
    Ok(vec![]) // STUB (S1-C)
}

/// pure: fields after " - "; \NNN octal unescape (\040 \011 \012 \134); malformed lines skipped
pub fn parse_mountinfo(_bytes: &[u8]) -> Vec<MountEntry> {
    vec![] // STUB (S1-C)
}

pub fn find<'a>(_e: &'a [MountEntry], _p: &Path) -> Option<&'a MountEntry> {
    None // STUB (S1-C)
}
