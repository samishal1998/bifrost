//! state.json: atomic write (tmp → sync_all → rename, 0600), corrupt-file quarantine (contract §8).
//! `held` is authoritative (user intent); mount records are only adoption hints.

use bifrost_core::{MountHandle, MountId};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub held: BTreeSet<MountId>,
    pub mounts: BTreeMap<MountId, MountHandle>,
}

/// Missing → empty. Unreadable, unparseable (or another version) → renamed to `state.json.corrupt-<unix>`, then empty.
pub fn read(dir: &Path) -> State {
    let p = dir.join("state.json");
    // unreadable is quarantined like unparseable: the next write must not replace the holds without a copy
    let bytes = match std::fs::read(&p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return State::default(),
        r => r.map_err(|e| e.to_string()),
    };
    match bytes.and_then(|b| serde_json::from_slice::<State>(&b).map_err(|e| e.to_string())) {
        Ok(s) if s.version == 1 => s,
        r => {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            let q = dir.join(format!("state.json.corrupt-{secs}"));
            let why = r.err().unwrap_or_else(|| "unknown version".into());
            tracing::warn!("{}: {why}; moved to {}", p.display(), q.display());
            let _ = std::fs::rename(&p, &q);
            State::default()
        }
    }
}

/// tmp (0600) → sync_all → rename: a crash leaves the old file or the new one, never half of one.
pub fn write(dir: &Path, s: &State) -> std::io::Result<()> {
    let tmp = dir.join("state.json.tmp");
    let _ = std::fs::remove_file(&tmp); // a leftover could carry another mode
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    f.write_all(&serde_json::to_vec_pretty(s)?)?;
    f.sync_all()?;
    std::fs::rename(&tmp, dir.join("state.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bifrost_core::Name;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    #[test]
    fn state_json_atomic_corrupt_quarantined() {
        let dir = std::env::temp_dir().join(format!("bf-state-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read(&dir), State::default()); // missing file

        let id = Name::parse("agent-01").unwrap();
        let s = State {
            version: 1,
            held: [id.clone()].into(),
            mounts: [(
                id.clone(),
                MountHandle {
                    id,
                    driver: "sshfs".into(),
                    local_path: PathBuf::from("/r/agent-01"),
                    fingerprint: "0123456789abcdef".into(),
                    pid: Some(7),
                },
            )]
            .into(),
        };
        write(&dir, &s).unwrap();
        let mode = std::fs::metadata(dir.join("state.json"))
            .unwrap()
            .permissions();
        assert_eq!(mode.mode() & 0o777, 0o600);
        assert!(!dir.join("state.json.tmp").exists());
        assert_eq!(read(&dir), s);

        // a validating newtype rejects a traversal id: the whole file is quarantined, never half-applied
        let bad = r#"{"version":1,"held":["../x"],"mounts":{}}"#;
        std::fs::write(dir.join("state.json"), bad).unwrap();
        assert_eq!(read(&dir), State::default());
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(!names.contains(&"state.json".to_string()), "{names:?}");
        let q = names.iter().find(|n| n.starts_with("state.json.corrupt-"));
        assert_eq!(std::fs::read_to_string(dir.join(q.unwrap())).unwrap(), bad);

        // unreadable (here EISDIR) is quarantined too, never overwritten by the next write
        std::fs::remove_file(dir.join(q.unwrap())).unwrap(); // the quarantine name is per second
        std::fs::create_dir_all(dir.join("state.json/x")).unwrap();
        assert_eq!(read(&dir), State::default());
        assert!(!dir.join("state.json").exists());
        let q = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path());
        let q = q.filter(|p| p.to_string_lossy().contains("state.json.corrupt-"));
        assert!(q.collect::<Vec<_>>()[0].join("x").is_dir());
        let _ = std::fs::remove_dir_all(dir);
    }
}
