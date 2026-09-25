//! Core is provider- and driver-agnostic (B8): no ProviderKind enum, no DRIVER_NAMES, no default_auto_order here.
//! Provider kinds are plain strings with a numeric trust rank (registry::Source); driver names, the auto order and
//! the trust ranks live in bifrost-config (§3).
//! MountSpec::{fingerprint, source}, marker, parse_marker and DriverSelector: TryFrom<String> are implemented
//! and tested in S0 (A2, S0.5), because S1 agents B and C depend on them.

use crate::validate::{
    Host, Invalid, MachineId, MountId, Name, RemotePath, User, fnv64, name_grammar,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::Duration;

/// tags via tag(), keys via meta_key(), values clean(v, 256)
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub tags: BTreeSet<String>,
    pub values: BTreeMap<String, String>,
}

/// Untrusted, pre-validated. user/path are used only with `honor_hints`. There is no driver hint (E2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountHints {
    pub user: Option<User>,
    pub path: Option<RemotePath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MachineObservation {
    pub id: MachineId,
    /// display, clean(name, 128)
    pub name: String,
    /// tailscale ID / TXT id= / HTTP id — matched only by the OWNING provider's
    /// include_ids/exclude_ids; global `ids` match the machine id only (A19)
    pub native_id: Option<String>,
    /// ≥1; [0] = connect target
    pub addresses: Vec<Host>,
    pub port: Option<u16>,
    /// Some only when the provider knows (tailscale, http)
    pub online: Option<bool>,
    pub metadata: Metadata,
    /// static: user = config user (trusted)
    pub hints: MountHints,
    /// DNS only; registry floors it
    pub ttl: Option<Duration>,
}

/// "auto" → Auto; anything else must match ^[a-z0-9][a-z0-9._-]{0,62}$ (no lowercasing) → Named.
/// Core checks only this grammar; membership in DRIVER_NAMES is checked by bifrost-config (B8).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum DriverSelector {
    Auto,
    Named(String),
}

impl TryFrom<String> for DriverSelector {
    type Error = Invalid;
    fn try_from(s: String) -> Result<Self, Invalid> {
        if s == "auto" {
            Ok(Self::Auto)
        } else if name_grammar(&s) {
            Ok(Self::Named(s))
        } else {
            Err(Invalid {
                what: "driver",
                value: s,
                why: "must be \"auto\" or match [a-z0-9][a-z0-9._-]{0,62}",
            })
        }
    }
}

impl From<DriverSelector> for String {
    fn from(d: DriverSelector) -> String {
        match d {
            DriverSelector::Auto => "auto".into(),
            DriverSelector::Named(n) => n,
        }
    }
}

/// PRD §5 (machine = resolved id; options = read_only).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountSpec {
    pub id: MountId,
    pub machine: MachineId,
    pub host: Host,
    pub port: Option<u16>,
    pub user: Option<User>,
    pub remote: RemotePath,
    /// canonical_root.join(id) — built only in reconcile::desired
    pub local_path: PathBuf,
    /// the SELECTOR text is fingerprinted ⇒ "auto" is sticky across probe flaps
    pub driver: DriverSelector,
    pub read_only: bool,
}

impl MountSpec {
    /// 16 lowercase hex = fnv64("id\0machine\0host\0port\0user\0remote\0local\0driver\0ro").
    /// None port/user → "", ro → "0"/"1". Pinned by `fingerprint_stable_vector`: it lives in kernel mount
    /// tables and state.json across upgrades, so never change the encoding.
    // ponytail: vfs_cache_mode and ssh_config are not fingerprinted, changes apply to new mounts only; include them in the fingerprint
    pub fn fingerprint(&self) -> String {
        let s = format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}\0{}",
            self.id.as_str(),
            self.machine.as_str(),
            self.host.as_str(),
            self.port.map(|p| p.to_string()).unwrap_or_default(),
            self.user.as_ref().map_or("", |u| u.as_str()),
            self.remote.as_str(),
            self.local_path.to_string_lossy(),
            String::from(self.driver.clone()),
            u8::from(self.read_only),
        );
        format!("{:016x}", fnv64(s.as_bytes()))
    }
    /// "[user@]<host.for_colon()>:<remote.sftp_path()>"
    pub fn source(&self) -> String {
        let user = self
            .user
            .as_ref()
            .map_or(String::new(), |u| format!("{}@", u.as_str()));
        format!(
            "{user}{}:{}",
            self.host.for_colon(),
            self.remote.sftp_path()
        )
    }
}

/// "bifrost:<id>@<fp16>"
pub fn marker(id: &MountId, fingerprint: &str) -> String {
    format!("bifrost:{}@{fingerprint}", id.as_str())
}

/// exact grammar; "agent-01" never matches "agent-01-x"
pub fn parse_marker(source: &str) -> Option<(MountId, String)> {
    let (id, fp) = source.strip_prefix("bifrost:")?.split_once('@')?;
    let hex = fp.len() == 16 && fp.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    if !hex || !name_grammar(id) {
        return None; // name_grammar, not Name::parse: an upper-case id is not ours
    }
    Some((Name::parse(id).ok()?, fp.to_string()))
}

/// called once, with an exit description
pub type OnExit = Box<dyn FnOnce(String) + Send + 'static>;

pub struct MountRequest {
    pub spec: MountSpec,
    pub log_path: PathBuf,
    pub on_exit: OnExit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountHandle {
    pub id: MountId,
    pub driver: String,
    pub local_path: PathBuf,
    pub fingerprint: String,
    /// informational, never signalled
    pub pid: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountState {
    Missing,
    Healthy,
    Degraded(String),
    Stale(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DriverAvailability {
    Available { binary: PathBuf, detail: String },
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DiscoveryError {
    /// binary missing, backend stopped, not implemented
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// transport / protocol / timeout / whole response unusable
    #[error("{0}")]
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MountError {
    #[error("driver unavailable: {0}")]
    Unavailable(String),
    /// the one busy string everywhere (C4)
    #[error("unmount blocked: busy (files open)")]
    Busy,
    /// occupied path, symlink, not a dir, not empty, invalid request
    #[error("refused: {0}")]
    Refused(String),
    /// preflight / driver log tail (tail(…, 512)), timeout
    #[error("{0}")]
    Failed(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::de::value::{Error, StrDeserializer};
    use serde::de::{DeserializeOwned, IntoDeserializer};

    fn spec() -> MountSpec {
        MountSpec {
            id: Name::parse("agent-01-home").unwrap(),
            machine: Name::parse("agent-01").unwrap(),
            host: Host::parse("100.64.0.1").unwrap(),
            port: None,
            user: None,
            remote: RemotePath::parse("~").unwrap(),
            local_path: PathBuf::from("/home/sami/machines/agent-01-home"),
            driver: DriverSelector::Auto,
            read_only: false,
        }
    }

    /// serde's own value deserializer runs the same derive path as serde_json, with no extra dev-dep (§1).
    fn de<T: DeserializeOwned>(s: &str) -> Result<T, Error> {
        let d: StrDeserializer<Error> = s.into_deserializer();
        T::deserialize(d)
    }

    #[test]
    fn fingerprint_changes_on_every_field() {
        let muts: [fn(&mut MountSpec); 9] = [
            |s| s.id = Name::parse("x").unwrap(),
            |s| s.machine = Name::parse("x").unwrap(),
            |s| s.host = Host::parse("100.64.0.2").unwrap(),
            |s| s.port = Some(22),
            |s| s.user = Some(User::parse("sami").unwrap()),
            |s| s.remote = RemotePath::parse("/").unwrap(),
            |s| s.local_path = PathBuf::from("/other"),
            |s| s.driver = DriverSelector::Named("sshfs".into()),
            |s| s.read_only = true,
        ];
        let mut seen = BTreeSet::from([spec().fingerprint()]);
        for m in muts {
            let mut s = spec();
            m(&mut s);
            assert!(seen.insert(s.fingerprint()), "{s:?}");
        }
        assert_eq!(spec().fingerprint(), spec().fingerprint());
    }

    #[test]
    fn fingerprint_stable_vector() {
        let fp = spec().fingerprint();
        assert!(fp.len() == 16 && fp.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')));
        assert_eq!(fp, "952d3037aea39f48"); // cross-checked with an independent FNV-1a
    }

    #[test]
    fn marker_roundtrip_exact() {
        let fp = "0123456789abcdef";
        let a = Name::parse("agent-01").unwrap();
        let ah = Name::parse("agent-01-home").unwrap();
        assert_eq!(marker(&a, fp), "bifrost:agent-01@0123456789abcdef");
        assert_eq!(
            parse_marker(&marker(&a, fp)),
            Some((a.clone(), fp.to_string()))
        );
        assert_eq!(parse_marker(&marker(&ah, fp)).unwrap().0, ah);
        assert_ne!(parse_marker(&marker(&ah, fp)).unwrap().0, a);
        for s in [
            "bifrost:agent-01@0123456789abcdefx",
            "bifrost:agent-01@0123456789abcde",
            "bifrost:agent-01@0123456789ABCDEF",
            "bifrost:Agent-01@0123456789abcdef",
            "xbifrost:agent-01@0123456789abcdef",
            "bifrost:agent-01",
            "bifrost:@0123456789abcdef",
            "bifrost:a@b@0123456789abcdef",
            "bifrost:../x@0123456789abcdef",
            "agent-01@0123456789abcdef",
            "",
        ] {
            assert_eq!(parse_marker(s), None, "{s:?}");
        }
    }

    #[test]
    fn source_format() {
        let mut s = spec();
        assert_eq!(s.source(), "100.64.0.1:");
        s.user = Some(User::parse("sami").unwrap());
        s.remote = RemotePath::parse("/srv/data").unwrap();
        assert_eq!(s.source(), "sami@100.64.0.1:/srv/data");
        s.host = Host::parse("fd7a::1").unwrap();
        s.remote = RemotePath::parse("~/proj").unwrap();
        assert_eq!(s.source(), "sami@[fd7a::1]:proj");
        s.remote = RemotePath::parse("/").unwrap();
        assert_eq!(s.source(), "sami@[fd7a::1]:/");
    }

    #[test]
    fn driver_selector_serde_rejects_bad_grammar() {
        assert_eq!(de::<DriverSelector>("auto").unwrap(), DriverSelector::Auto);
        assert_eq!(
            de::<DriverSelector>("rclone").unwrap(),
            DriverSelector::Named("rclone".into())
        );
        // grammar only (B8): an unknown but well-formed name is bifrost-config's problem
        assert!(de::<DriverSelector>("nfs-x.2").is_ok());
        for s in [
            "",
            "Auto",
            "SSHFS",
            "-x",
            ".x",
            "a b",
            "../x",
            "a/b",
            &"a".repeat(64),
        ] {
            assert!(de::<DriverSelector>(s).is_err(), "{s:?}");
            assert!(DriverSelector::try_from(s.to_string()).is_err(), "{s:?}");
        }
        assert_eq!(String::from(DriverSelector::Auto), "auto");
        assert_eq!(String::from(DriverSelector::Named("sshfs".into())), "sshfs");
    }

    #[test]
    fn serde_newtypes_validate_on_deserialize() {
        assert!(de::<Name>("..").is_err());
        assert!(de::<Name>("a/b").is_err());
        assert_eq!(de::<Name>("Agent-01").unwrap().as_str(), "agent-01");
        assert!(de::<Host>("-oProxyCommand=x").is_err());
        assert!(de::<Host>("fe80::1%eth0").is_err());
        assert_eq!(de::<Host>("10.0.0.1").unwrap().as_str(), "10.0.0.1");
        assert!(de::<User>("-x").is_err());
        assert!(de::<RemotePath>("a/b").is_err());
        assert_eq!(de::<RemotePath>("/").unwrap().as_str(), "/");
    }
}
