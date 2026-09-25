//! The trust boundary. Every untrusted string becomes one of these types or is rejected.

use serde::{Deserialize, Serialize};
use std::hash::{BuildHasher, Hasher, RandomState};
use std::net::IpAddr;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {what} {value:?}: {why}")]
pub struct Invalid {
    pub what: &'static str,
    pub value: String,
    pub why: &'static str,
}

fn bad(what: &'static str, s: &str, why: &'static str) -> Invalid {
    Invalid {
        what,
        value: s.to_string(),
        why,
    }
}

/// `first` then `rest*`, 1..=max bytes. The classes are ASCII-only, so any non-ASCII byte fails.
fn gram(s: &str, max: usize, first: fn(u8) -> bool, rest: fn(u8) -> bool) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= max && first(b[0]) && b[1..].iter().all(|&c| rest(c))
}

/// `^[a-z0-9][a-z0-9._-]{0,62}$`, no lowercasing (Name after lowercasing, DriverSelector::Named, markers).
pub(crate) fn name_grammar(s: &str) -> bool {
    gram(
        s,
        63,
        |c| c.is_ascii_lowercase() || c.is_ascii_digit(),
        |c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'.' | b'_' | b'-'),
    )
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
        let l = s.to_ascii_lowercase();
        if name_grammar(&l) {
            Ok(Self(l))
        } else {
            Err(bad("name", s, "must match [a-z0-9][a-z0-9._-]{0,62}"))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Either a std `IpAddr` literal (no brackets, no zone id, not unspecified), or a hostname: one trailing '.'
/// stripped, ≤253 bytes, labels 1..=63 of [A-Za-z0-9_-], no label starting with '-'. Stored lowercase.
/// IP literals are stored canonical (v4-mapped v6 → v4, v6 compressed), so a v4 cidr can't be dodged.
/// ⇒ never starts with '-'; never contains whitespace, '"', '\'', ',', '@', '/', '=', '%', or ':'
///   (':' only inside a v6 literal).
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Host(String);

impl Host {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        let h = s.strip_suffix('.').unwrap_or(s); // before the IP parse, so "0.0.0.0." can't dodge the check
        if let Ok(ip) = h.parse::<IpAddr>() {
            // canonical + lowercase; ::ffff:a.b.c.d is stored as a.b.c.d so a v4 cidr deny can't be dodged
            let ip = ip.to_canonical();
            if ip.is_unspecified() {
                return Err(bad(
                    "host",
                    s,
                    "unspecified address (connects to this machine)",
                ));
            }
            return Ok(Self(ip.to_string()));
        }
        let label = |l: &str| {
            gram(
                l,
                63,
                |c| c.is_ascii_alphanumeric() || c == b'_',
                |c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'),
            )
        };
        if h.len() <= 253 && h.split('.').all(label) {
            Ok(Self(h.to_ascii_lowercase()))
        } else {
            Err(bad("host", s, "not an IP literal or hostname"))
        }
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn ip(&self) -> Option<IpAddr> {
        self.0.parse().ok()
    }
    /// "[fd7a::1]" for v6, else as_str()
    pub fn for_colon(&self) -> String {
        match self.ip() {
            Some(IpAddr::V6(_)) => format!("[{}]", self.0),
            _ => self.0.clone(),
        }
    }
}

/// `^[A-Za-z0-9_][A-Za-z0-9_.-]{0,31}$`
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct User(String);

impl User {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        let ok = gram(
            s,
            32,
            |c| c.is_ascii_alphanumeric() || c == b'_',
            |c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b'-'),
        );
        if ok {
            Ok(Self(s.to_string()))
        } else {
            Err(bad(
                "user",
                s,
                "must match [A-Za-z0-9_][A-Za-z0-9_.-]{0,31}",
            ))
        }
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
        let why = if !(s == "~" || s.starts_with("~/") || s.starts_with('/')) {
            "must be ~, ~/<rel>, / or /<abs>"
        } else if s.len() > 1024 {
            "longer than 1024 bytes"
        } else if s.contains(|c: char| c.is_control() || c == ':') {
            "contains a control character or ':'"
        } else if s.split('/').any(|c| c == "..") {
            "contains a '..' component"
        } else {
            return Ok(Self(s.to_string()));
        };
        Err(bad("remote path", s, why))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// "~"→"", "~/rel"→"rel", "/"→"/", "/abs"→"/abs"
    pub fn sftp_path(&self) -> &str {
        match self.0.strip_prefix('~') {
            Some(rel) => rel.trim_start_matches('/'), // "~//x" stays relative
            None => &self.0,
        }
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

/// lowercased; `^[a-z0-9][a-z0-9_.:-]{0,62}$`
pub fn tag(s: &str) -> Result<String, Invalid> {
    let l = s.to_ascii_lowercase();
    let ok = gram(
        &l,
        63,
        |c| c.is_ascii_lowercase() || c.is_ascii_digit(),
        |c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b':' | b'-'),
    );
    ok.then_some(l)
        .ok_or_else(|| bad("tag", s, "must match [a-z0-9][a-z0-9_.:-]{0,62}"))
}
/// `^[a-z0-9_.-]{1,64}$`
pub fn meta_key(s: &str) -> Result<String, Invalid> {
    let c = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, b'_' | b'.' | b'-');
    gram(s, 64, c, c)
        .then(|| s.to_string())
        .ok_or_else(|| bad("metadata key", s, "must match [a-z0-9_.-]{1,64}"))
}
/// `^[A-Za-z0-9._:-]{1,128}$`
pub fn native_id(s: &str) -> Result<String, Invalid> {
    let c = |c: u8| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b':' | b'-');
    gram(s, 128, c, c)
        .then(|| s.to_string())
        .ok_or_else(|| bad("native id", s, "must match [A-Za-z0-9._:-]{1,128}"))
}
/// Display-only text: control chars and bidi/zero-width format chars → '?', truncated at `max` chars.
/// Callers pass 128 for display names, 256 for metadata values (the config rule) and 512 for errors and
/// log lines (A8).
/// `is_control` also covers C1 (0x80–0x9f, incl. the 8-bit CSI 0x9b).
pub fn clean(s: &str, max: usize) -> String {
    s.chars()
        .take(max)
        .map(|c| {
            let fmt = matches!(c, '\u{200b}'..='\u{200f}' | '\u{2028}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
            if c.is_control() || fmt { '?' } else { c }
        })
        .collect()
}
/// First line of every driver log (§6 spawn): `LOG_HEADER` + the argv.
pub const LOG_HEADER: &str = "# bifrost exec: ";
/// Error text from a log or stderr: split on '\n' (trailing '\r' trimmed), skip blank (whitespace-only) lines
/// and lines starting with LOG_HEADER, keep the LAST lines that fit in `max` chars, each clean(line, max)ed,
/// joined with " | " (A8).
pub fn tail(s: &str, max: usize) -> String {
    let mut kept = Vec::new();
    let mut len = 0;
    for line in s.split('\n').rev() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim().is_empty() || line.starts_with(LOG_HEADER) {
            continue;
        }
        let c = clean(line, max);
        let n = c.chars().count() + if kept.is_empty() { 0 } else { 3 };
        if len + n > max {
            break;
        }
        len += n;
        kept.push(c);
    }
    kept.reverse();
    kept.join(" | ")
}
/// `^[0-9]+(ms|s|m|h)$`, > 0, ≤ 366d (so `Instant + 3·d` can't overflow), checked overflow
pub fn parse_duration(s: &str) -> Result<Duration, Invalid> {
    let i = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(i); // num is all digits, so u64::from_str never sees a '+'
    let n: Option<u64> = num.parse().ok();
    let d = match unit {
        "ms" => n.map(Duration::from_millis),
        "s" => n.map(Duration::from_secs),
        "m" => n.and_then(|n| n.checked_mul(60)).map(Duration::from_secs),
        "h" => n.and_then(|n| n.checked_mul(3600)).map(Duration::from_secs),
        _ => None,
    };
    d.filter(|d| !d.is_zero() && *d <= Duration::from_secs(366 * 86_400))
        .ok_or_else(|| bad("duration", s, "must be <n>ms|s|m|h, > 0 and <= 366d"))
}

// ponytail: hand-rolled glob (*, ?), CIDR, duration, FNV-1a and jitter; no character classes or brace globs, durations use a single unit; globset if rules need classes
/// `[a-z0-9*?._-]{1,63}`; '*' = any run, '?' = one char; iterative two-pointer matcher.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glob(String);

impl Glob {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        let c = |c: u8| {
            c.is_ascii_lowercase()
                || c.is_ascii_digit()
                || matches!(c, b'*' | b'?' | b'.' | b'_' | b'-')
        };
        if gram(s, 63, c, c) {
            Ok(Self(s.to_string()))
        } else {
            Err(bad("glob", s, "must match [a-z0-9*?._-]{1,63}"))
        }
    }
    /// Byte-wise: '?' is one byte, fine because matched ids are ASCII Names. O(|p|·|s|) worst case.
    pub fn matches(&self, s: &str) -> bool {
        let (p, s) = (self.0.as_bytes(), s.as_bytes());
        let (mut pi, mut si) = (0, 0);
        let mut star: Option<(usize, usize)> = None; // (pattern index of '*', s index it resumes at)
        while si < s.len() {
            if pi < p.len() && (p[pi] == b'?' || p[pi] == s[si]) {
                pi += 1;
                si += 1;
            } else if pi < p.len() && p[pi] == b'*' {
                star = Some((pi, si));
                pi += 1;
            } else if let Some((sp, ss)) = star {
                star = Some((sp, ss + 1));
                pi = sp + 1;
                si = ss + 1;
            } else {
                return false;
            }
        }
        p[pi..].iter().all(|&c| c == b'*')
    }
}

/// "10.0.0.0/8" | "fd7a::/48" | bare IP (= /32 or /128). Address families never cross-match.
/// IPv4-mapped v6 is rejected (write the v4 form): Host stores it as v4, so it could never match.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cidr {
    pub addr: IpAddr,
    pub prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Result<Self, Invalid> {
        let (a, p) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let addr: IpAddr = a.parse().map_err(|_| bad("cidr", s, "not an IP address"))?;
        if matches!(addr, IpAddr::V6(v) if v.to_ipv4_mapped().is_some()) {
            return Err(bad("cidr", s, "IPv4-mapped: write the IPv4 form"));
        }
        let full = if addr.is_ipv4() { 32 } else { 128 };
        let prefix = match p {
            None => full,
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|&n| n <= full && p.bytes().all(|b| b.is_ascii_digit()))
                .ok_or_else(|| bad("cidr", s, "bad prefix length"))?,
        };
        Ok(Self { addr, prefix })
    }
    pub fn contains(&self, ip: IpAddr) -> bool {
        let p = u32::from(self.prefix);
        match (self.addr, ip) {
            (IpAddr::V4(n), IpAddr::V4(i)) => {
                let m = u32::MAX.checked_shl(32u32.saturating_sub(p)).unwrap_or(0);
                u32::from(n) & m == u32::from(i) & m
            }
            (IpAddr::V6(n), IpAddr::V6(i)) => {
                let m = u128::MAX.checked_shl(128u32.saturating_sub(p)).unwrap_or(0);
                u128::from(n) & m == u128::from(i) & m
            }
            _ => false,
        }
    }
}

/// FNV-1a 64: stable across Rust versions (used for fingerprints)
pub fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
/// `RandomState::new().build_hasher().finish()` — std only
pub fn random_u64() -> u64 {
    RandomState::new().build_hasher().finish()
}
/// base = min(max, initial·2^min(failures.saturating_sub(1), 32)); returns base/2 + rand % (base/2 + 1ms)
/// ("equal jitter", ∈ [base/2, base]); failures = 0 behaves like 1, never underflows (C3).
/// The jitter is whole milliseconds, so the "+1ms" keeps the result ≤ base.
pub fn backoff(failures: u32, initial: Duration, max: Duration, rand: u64) -> Duration {
    let exp = failures.saturating_sub(1).min(32);
    let base = (0..exp).fold(initial, |d, _| d.saturating_mul(2)).min(max);
    let half = base / 2;
    let jitter = u128::from(rand) % (half.as_millis() + 1); // ≤ rand, so it fits in u64
    half + Duration::from_millis(jitter as u64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn name_rejects_traversal() {
        let long = "a".repeat(64);
        for s in [
            "", ".", "..", "a/b", "../x", ".x", "-x", "a b", "a\0", &long,
        ] {
            assert!(Name::parse(s).is_err(), "{s:?}");
        }
        assert!(Name::parse(&"a".repeat(63)).is_ok());
        assert!(Name::parse("agent-01.home_2").is_ok());
    }

    #[test]
    fn name_lowercases() {
        assert_eq!(Name::parse("Agent-01").unwrap().as_str(), "agent-01");
        assert!(Name::parse("Ä").is_err());
    }

    #[test]
    fn host_rejects_option_injection() {
        let long = format!("{0}.{0}.{0}.{1}", "a".repeat(63), "a".repeat(62));
        assert_eq!(long.len(), 254);
        for s in [
            "-oProxyCommand=x",
            "a b",
            "a,b",
            "a\"b",
            "u@h",
            "h:22",
            "h;rm",
            "fe80::1%eth0",
            &long,
            "",
            ".",
            "..",
            "a..b",
            "[fd7a::1]",
            "a/b",
            "a=b",
            "a'b",
            "x.-y",
            "0.0.0.0",
            "::",
            "::ffff:0.0.0.0",
            "0.0.0.0.",
        ] {
            assert!(Host::parse(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn host_accepts_ipv4_ipv6_fqdn_trailing_dot() {
        let h = |s| Host::parse(s).unwrap();
        assert_eq!(
            h("10.0.0.1").ip(),
            Some(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
        );
        assert_eq!(h("FD7A:115C::1").as_str(), "fd7a:115c::1");
        assert!(h("::1").ip().is_some());
        assert_eq!(h("::ffff:10.1.2.3").as_str(), "10.1.2.3");
        assert_eq!(h("10.0.0.1.").as_str(), "10.0.0.1");
        assert_eq!(
            h("Agent-01.tail1234.TS.net.").as_str(),
            "agent-01.tail1234.ts.net"
        );
        assert_eq!(h("host_name").ip(), None);
        let max = format!("{0}.{0}.{0}.{1}", "a".repeat(63), "a".repeat(61));
        assert_eq!(h(&format!("{max}.")).as_str(), max);
    }

    #[test]
    fn host_for_colon_brackets_v6() {
        let h = |s| Host::parse(s).unwrap().for_colon();
        assert_eq!(h("fd7a::1"), "[fd7a::1]");
        assert_eq!(h("10.0.0.1"), "10.0.0.1");
        assert_eq!(h("example.com"), "example.com");
    }

    #[test]
    fn user_rejects_dash_at_space() {
        let long = "a".repeat(33);
        for s in ["-x", "a@b", "a b", "", "a:b", "a/b", ".x", &long] {
            assert!(User::parse(s).is_err(), "{s:?}");
        }
        for s in ["sami", "_svc", "Sami.b-c", &"a".repeat(32)] {
            assert_eq!(User::parse(s).unwrap().as_str(), s);
        }
    }

    #[test]
    fn remote_path_rules() {
        for s in ["~", "~/x", "/", "/a b", "/a/./b", "~/a..b"] {
            assert_eq!(RemotePath::parse(s).unwrap().as_str(), s);
        }
        let long = format!("/{}", "a".repeat(1024));
        for s in [
            "a/b", "/a/../b", "/a\nb", "/a:b", "", "~user", "~/..", "/a/..", "-x", "/a\u{7f}",
            &long,
        ] {
            assert!(RemotePath::parse(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn sftp_path_mapping() {
        let p = |s| RemotePath::parse(s).unwrap();
        assert_eq!(p("~").sftp_path(), "");
        assert_eq!(p("~/proj/x").sftp_path(), "proj/x");
        assert_eq!(p("~//x").sftp_path(), "x");
        assert_eq!(p("/").sftp_path(), "/");
        assert_eq!(p("/srv/data").sftp_path(), "/srv/data");
    }

    #[test]
    fn tag_meta_label_native_id_grammars() {
        assert_eq!(tag("Dev").unwrap(), "dev");
        assert_eq!(tag("k8s:prod_1.a-b").unwrap(), "k8s:prod_1.a-b");
        assert!(tag(&"a".repeat(63)).is_ok());
        for s in ["", "-x", ":x", ".x", "a b", "a/b", &"a".repeat(64)] {
            assert!(tag(s).is_err(), "{s:?}");
        }
        assert_eq!(meta_key("a.b_c-d").unwrap(), "a.b_c-d");
        assert!(meta_key(&"a".repeat(64)).is_ok());
        for s in ["", "Env", "a:b", "a b", &"a".repeat(65)] {
            assert!(meta_key(s).is_err(), "{s:?}");
        }
        assert_eq!(native_id("nABC123CNTRL").unwrap(), "nABC123CNTRL");
        assert!(native_id("a:b.c-d_e").is_ok());
        assert!(native_id(&"a".repeat(128)).is_ok());
        for s in ["", "a b", "a/b", "../x", "a@b", &"a".repeat(129)] {
            assert!(native_id(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn clean_strips_escapes() {
        assert_eq!(clean("\x1b[31mred\x1b[0m", 512), "?[31mred?[0m");
        assert_eq!(clean("a\nb\rc\td\u{9b}e\u{7f}", 512), "a?b?c?d?e?");
        assert_eq!(clean("héllo", 3), "hél");
        assert_eq!(clean("abc", 0), "");
        assert_eq!(
            clean("a\u{202e}b\u{200b}c\u{2066}d\u{feff}e\u{2028}f", 512),
            "a?b?c?d?e?f"
        );
        assert_eq!(clean("héllo ✓ 日本", 512), "héllo ✓ 日本");
    }

    #[test]
    fn tail_skips_header() {
        let prev = "No ED25519 host key is known for [127.0.0.1]:2222";
        let last = "Host key verification failed.";
        let log =
            format!("{LOG_HEADER}sshfs -o x host: /r/a\n\nread: reset\r\n  \n{prev}\r\n{last}\r\n");
        assert_eq!(tail(&log, 512), format!("read: reset | {prev} | {last}"));
        let n = prev.len() + 3 + last.len();
        assert_eq!(tail(&log, n), format!("{prev} | {last}"));
        assert_eq!(tail(&log, n - 1), last);
        assert_eq!(tail("a\x1bb\n", 512), "a?b");
        assert_eq!(tail(&"x".repeat(600), 512).len(), 512);
        assert_eq!(tail("", 512), "");
        assert_eq!(tail(&format!("{LOG_HEADER}only\n"), 512), "");
    }

    #[test]
    fn duration_units_rejects_zero_and_junk() {
        let d = |s| parse_duration(s).unwrap();
        assert_eq!(d("500ms"), Duration::from_millis(500));
        assert_eq!(d("30s"), Duration::from_secs(30));
        assert_eq!(d("5m"), Duration::from_secs(300));
        assert_eq!(d("1h"), Duration::from_secs(3600));
        assert_eq!(d("8784h"), Duration::from_secs(366 * 86_400));
        for s in [
            "5",
            "5 m",
            "-1s",
            "1d",
            "0s",
            "0ms",
            "",
            "s",
            "+5s",
            " 5s",
            "5s ",
            "1.5s",
            "5S",
            "18446744073709551615h",
            "99999999999999999999s",
            "8785h",
            "31622400001ms",
            "18446744073709551615s",
        ] {
            assert!(parse_duration(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn glob_star_question() {
        let g = |p: &str, s: &str| Glob::parse(p).unwrap().matches(s);
        assert!(g("agent-*", "agent-01") && g("agent-*", "agent-"));
        assert!(!g("agent-*", "agents"));
        assert!(g("*", "") && g("*", "x") && g("**", "abc"));
        assert!(g("a?c", "abc") && !g("a?c", "ac") && !g("a?c", "abbc"));
        assert!(g("*-01", "agent-01") && !g("*-01", "agent-011"));
        assert!(g("a*b*c", "abxbc") && g("a*b*c", "abc") && !g("a*b*c", "acb"));
        assert!(g("exact", "exact") && !g("exact", "exactly"));
        assert!(!g(&format!("{}b", "*a".repeat(30)), &"a".repeat(1000)));
        for p in ["", "A*", "a/b", "[ab]", "a b", &"a".repeat(64)] {
            assert!(Glob::parse(p).is_err(), "{p:?}");
        }
        assert!(Glob::parse(&"a".repeat(63)).is_ok());
    }

    #[test]
    fn cidr_v4_v6_no_cross_family() {
        let c = |s| Cidr::parse(s).unwrap();
        let ip = |s: &str| s.parse::<IpAddr>().unwrap();
        assert!(c("10.0.0.0/8").contains(ip("10.255.0.1")));
        assert!(!c("10.0.0.0/8").contains(ip("11.0.0.1")));
        assert!(c("10.1.2.3").contains(ip("10.1.2.3")) && !c("10.1.2.3").contains(ip("10.1.2.4")));
        assert_eq!(c("::1").prefix, 128);
        assert!(c("0.0.0.0/0").contains(ip("1.2.3.4")) && !c("0.0.0.0/0").contains(ip("::1")));
        assert!(c("::/0").contains(ip("fd7a::1")) && !c("::/0").contains(ip("10.0.0.1")));
        let v6 = c("fd7a:115c:a1e0::/48");
        assert!(v6.contains(ip("fd7a:115c:a1e0:ab12::1")));
        assert!(!v6.contains(ip("fd7a:115c:a1e1::1")) && !v6.contains(ip("10.0.0.1")));
        assert!(!c("10.0.0.0/8").contains(ip("fd7a::1")));
        // v4-mapped literals are canonicalised by Host::parse, never here
        assert!(!c("10.0.0.0/8").contains(ip("::ffff:10.1.2.3")));
        for s in [
            "10.0.0.0/33",
            "fd7a::/129",
            "10.0.0.0/",
            "host/8",
            "",
            "10.0.0.0/8/9",
            "::ffff:0:0/96",
            "::ffff:10.0.0.0/104",
            "::ffff:10.1.2.3",
        ] {
            assert!(Cidr::parse(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn fnv64_known_vectors() {
        assert_eq!(fnv64(b""), 0xcbf29ce484222325);
        assert_eq!(fnv64(b"a"), 0xaf63dc4c8601ec8c);
        assert_eq!(fnv64(b"foobar"), 0x85944171f73967e8);
        assert_ne!(random_u64(), random_u64());
    }

    #[test]
    fn backoff_bounds() {
        let (initial, max) = (Duration::from_secs(2), Duration::from_secs(60));
        for f in 0..=64u32 {
            let base = initial
                .saturating_mul(2u32.saturating_pow(f.saturating_sub(1)))
                .min(max);
            for rand in [0, u64::MAX] {
                let b = backoff(f, initial, max, rand);
                assert!(
                    b >= base / 2 && b <= base && b <= max,
                    "f={f} rand={rand} {b:?}"
                );
            }
            assert_eq!(backoff(f, initial, max, 0), base / 2);
        }
        assert_eq!(backoff(0, initial, max, 7), backoff(1, initial, max, 7));
    }

    #[test]
    fn backoff_no_overflow() {
        for f in [0, 1, 32, 33, 64, u32::MAX] {
            for rand in [0, 1, u64::MAX] {
                assert!(backoff(f, Duration::MAX, Duration::MAX, rand) <= Duration::MAX);
                assert!(backoff(f, Duration::from_secs(1), Duration::MAX, rand) <= Duration::MAX);
                assert_eq!(
                    backoff(f, Duration::ZERO, Duration::MAX, rand),
                    Duration::ZERO
                );
            }
        }
        let b = backoff(u32::MAX, Duration::from_secs(1), Duration::MAX, 0);
        assert_eq!(b, Duration::from_secs(1 << 31));
    }
}
