//! The trust boundary. Every untrusted string becomes one of these types or is rejected.
//!
//! STUB (S0.3): every parser below accepts all input and every helper returns a placeholder.
//! S0.4 implements this file fully; no untrusted input may run before it lands.

use serde::{Deserialize, Serialize};
use std::net::IpAddr;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {what} {value:?}: {why}")]
pub struct Invalid {
    pub what: &'static str,
    pub value: String,
    pub why: &'static str,
}

/// Identity key AND a single path component. ASCII-lowercases, then `^[a-z0-9][a-z0-9._-]{0,62}$`.
/// ⇒ never "", ".", "..", "-x", ".x"; no '/', NUL or whitespace; case-insensitive filesystems can't collide.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Name(String);
pub type MachineId = Name;
pub type MountId = Name; // == directory name under mount.root

impl Name {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        Ok(Self(s.to_string())) // STUB: accepts all input until S0.4
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Either a std `IpAddr` literal (no brackets, no zone id), or a hostname: one trailing '.' stripped,
/// ≤253 bytes, labels 1..=63 of [A-Za-z0-9_-], no label starting with '-'. Stored lowercase.
/// ⇒ never starts with '-'; never contains whitespace, '"', '\'', ',', '@', '/', '=', '%', or ':'
///   (':' only inside a v6 literal).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Host(String);

impl Host {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        Ok(Self(s.to_string())) // STUB: accepts all input until S0.4
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn ip(&self) -> Option<IpAddr> {
        None // STUB (S0.4)
    }
    /// "[fd7a::1]" for v6, else as_str()
    pub fn for_colon(&self) -> String {
        self.0.clone() // STUB (S0.4)
    }
}

/// `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}$`
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct User(String);

impl User {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        Ok(Self(s.to_string())) // STUB: accepts all input until S0.4
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// "~" | "~/<rel>" | "/" | "/<abs>"; ≤1024 bytes; no control chars (<0x20, 0x7f); no ':' ; no ".." component.
/// "/" parses (PRD §6.1 root mounts, B2).
/// Spaces are allowed: always a single argv element, never placed inside --sftp-ssh.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct RemotePath(String);

impl RemotePath {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        Ok(Self(s.to_string())) // STUB: accepts all input until S0.4
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// "~"→"", "~/rel"→"rel", "/"→"/", "/abs"→"/abs"
    pub fn sftp_path(&self) -> &str {
        &self.0 // STUB (S0.4)
    }
}

macro_rules! string_newtype {
    ($($t:ident),*) => {$(
        impl TryFrom<String> for $t {
            type Error = Invalid;
            fn try_from(s: String) -> Result<Self, Invalid> {
                Self::parse(&s)
            }
        }
        impl From<$t> for String {
            fn from(v: $t) -> String {
                v.0
            }
        }
    )*};
}
string_newtype!(Name, Host, User, RemotePath);

fn stub(what: &'static str, s: &str) -> Invalid {
    Invalid {
        what,
        value: s.to_string(),
        why: "not implemented",
    }
}

/// lowercased; `^[a-z0-9][a-z0-9_.:-]{0,62}$`
pub fn tag(s: &str) -> Result<String, Invalid> {
    Err(stub("tag", s)) // STUB (S0.4)
}
/// `^[a-z0-9_.-]{1,64}$`
pub fn meta_key(s: &str) -> Result<String, Invalid> {
    Err(stub("metadata key", s)) // STUB (S0.4)
}
/// `^[A-Za-z0-9._:-]{1,128}$`
pub fn native_id(s: &str) -> Result<String, Invalid> {
    Err(stub("native id", s)) // STUB (S0.4)
}
/// Display-only text: control chars → '?', truncated at `max` chars. Callers pass 128 for display names,
/// 256 for metadata values (the config rule) and 512 for errors and log lines (A8).
pub fn clean(_s: &str, _max: usize) -> String {
    String::new() // STUB (S0.4)
}
/// First line of every driver log (§6 spawn): `LOG_HEADER` + the argv.
pub const LOG_HEADER: &str = "# bifrost exec: ";
/// Error text from a log or stderr: split on '\n' (trailing '\r' trimmed), skip empty lines and lines starting
/// with LOG_HEADER, keep the LAST lines that fit in `max` chars, each clean(line, max)ed, joined with " | " (A8).
pub fn tail(_s: &str, _max: usize) -> String {
    String::new() // STUB (S0.4)
}
/// `^[0-9]+(ms|s|m|h)$`, >0, checked overflow
pub fn parse_duration(s: &str) -> Result<Duration, Invalid> {
    Err(stub("duration", s)) // STUB (S0.4)
}

/// `[a-z0-9*?._-]{1,63}`; '*' = any run, '?' = one char; iterative two-pointer matcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glob(String);

impl Glob {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        Err(stub("glob", s)) // STUB (S0.4)
    }
    pub fn matches(&self, _s: &str) -> bool {
        false // STUB (S0.4)
    }
}

/// "10.0.0.0/8" | "fd7a::/48" | bare IP (= /32 or /128). Address families never cross-match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        Err(stub("cidr", s)) // STUB (S0.4)
    }
    pub fn contains(&self, _ip: IpAddr) -> bool {
        false // STUB (S0.4)
    }
}

/// FNV-1a 64: stable across Rust versions (used for fingerprints)
pub fn fnv64(_bytes: &[u8]) -> u64 {
    0 // STUB (S0.4)
}
/// `RandomState::new().build_hasher().finish()` — std only
pub fn random_u64() -> u64 {
    0 // STUB (S0.4)
}
/// base = min(max, initial·2^min(failures.saturating_sub(1), 32)); returns base/2 + rand % (base/2 + 1ms)
/// ("equal jitter", ∈ [base/2, base]); failures = 0 behaves like 1, never underflows (C3).
pub fn backoff(_failures: u32, _initial: Duration, max: Duration, _rand: u64) -> Duration {
    max // STUB (S0.4)
}
