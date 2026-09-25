//! OS mount table. Reading it never touches a FUSE filesystem (hung-FUSE guard 1).

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MountEntry {
    pub mount_point: PathBuf,
    pub fstype: String,
    pub source: String,
}

/// linux: /proc/self/mountinfo; macos: getmntinfo(MNT_NOWAIT) under a static Mutex
#[cfg(not(target_os = "macos"))]
pub fn read() -> std::io::Result<Vec<MountEntry>> {
    Ok(parse_mountinfo(&std::fs::read("/proc/self/mountinfo")?))
}

/// linux: /proc/self/mountinfo; macos: getmntinfo(MNT_NOWAIT) under a static Mutex
// ponytail: macOS code is compile-checked only, runtime behaviour of macFUSE/FUSE-T/nfsmount is unproven; a macOS runner for the E2E
#[cfg(target_os = "macos")]
pub fn read() -> std::io::Result<Vec<MountEntry>> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;
    // getmntinfo returns one per-process buffer that the next call overwrites
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut p: *mut libc::statfs = std::ptr::null_mut();
    // SAFETY: getmntinfo points `p` at `n` records in its own buffer; we copy them out under the lock.
    let n = unsafe { libc::getmntinfo(&mut p, libc::MNT_NOWAIT) };
    if n <= 0 || p.is_null() {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `p` points at `n` initialised records (checked above).
    let all = unsafe { std::slice::from_raw_parts(p, n as usize) };
    // SAFETY: the kernel NUL-terminates these fixed-size name arrays.
    let s = |a: &[libc::c_char]| unsafe { CStr::from_ptr(a.as_ptr()) }.to_bytes().to_vec();
    Ok(all
        .iter()
        .map(|f| MountEntry {
            mount_point: PathBuf::from(OsStr::from_bytes(&s(&f.f_mntonname))),
            fstype: String::from_utf8_lossy(&s(&f.f_fstypename)).into_owned(),
            source: String::from_utf8_lossy(&s(&f.f_mntfromname)).into_owned(),
        })
        .collect())
}

/// pure: fields after " - "; \NNN octal unescape (\040 \011 \012 \134); malformed lines skipped
// ponytail: hand-rolled mountinfo parser, reads only mount point, fstype and source; a crate if more fields are ever needed
pub fn parse_mountinfo(bytes: &[u8]) -> Vec<MountEntry> {
    use std::os::unix::ffi::OsStringExt;
    let text = |b: &[u8]| String::from_utf8_lossy(&unescape(b)).into_owned();
    bytes
        .split(|&b| b == b'\n')
        .filter_map(|line| {
            let f: Vec<&[u8]> = line.split(|&b| b == b' ').collect();
            // 6 fixed fields, optional fields, then a lone "-", fstype, source, super options
            let sep = f.iter().skip(6).position(|x| *x == b"-")? + 6;
            let (fstype, source) = (f.get(sep + 1)?, f.get(sep + 2)?);
            Some(MountEntry {
                mount_point: std::ffi::OsString::from_vec(unescape(f[4])).into(),
                fstype: text(fstype),
                source: text(source),
            })
        })
        .collect()
}

/// `\NNN` (three octal digits, ≤ \377) → that byte; anything else is kept as is.
fn unescape(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let d = s.get(i + 1..i + 4).filter(|d| {
            s[i] == b'\\' && d[0] <= b'3' && d.iter().all(|c| (b'0'..=b'7').contains(c))
        });
        if let Some(d) = d {
            out.push(d.iter().fold(0, |n, c| n * 8 + (c - b'0')));
            i += 4;
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

/// The topmost mount at `p` (an overmount is listed after the mount it hides).
pub fn find<'a>(e: &'a [MountEntry], p: &Path) -> Option<&'a MountEntry> {
    e.iter().rev().find(|x| x.mount_point == p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(mp: &str, fstype: &str, source: &str) -> MountEntry {
        MountEntry {
            mount_point: mp.into(),
            fstype: fstype.into(),
            source: source.into(),
        }
    }

    #[test]
    fn mountinfo_octal_escapes() {
        let t = b"135 31 0:65 / /home/sami/my\\040machines/a\\011b\\012c\\134d rw,nosuid shared:367 - fuse.sshfs bifrost:a@0123456789abcdef rw,user_id=1000\n\
                  136 31 0:66 / /x\\0401 rw - fuse.sshfs bf@h:/a\\040b rw\n\
                  137 31 0:67 / /n\\9x\\04 rw - fuse.sshfs \\400 rw\n";
        assert_eq!(
            parse_mountinfo(t),
            vec![
                e(
                    "/home/sami/my machines/a\tb\nc\\d",
                    "fuse.sshfs",
                    "bifrost:a@0123456789abcdef"
                ),
                e("/x 1", "fuse.sshfs", "bf@h:/a b"),
                // not escapes (non-octal, short, > 0o377): kept byte for byte
                e("/n\\9x\\04", "fuse.sshfs", "\\400"),
            ]
        );
    }

    #[test]
    fn mountinfo_optional_fields() {
        let t =
            b"25 31 0:23 / /sys rw,nosuid shared:7 master:1 propagate_from:2 - sysfs sysfs rw\n\
                  26 31 0:24 / /proc rw - proc proc rw\n\
                  27 31 0:5 / /dev rw shared:2 - devtmpfs udev rw,size=3946340k\n";
        assert_eq!(
            parse_mountinfo(t),
            vec![
                e("/sys", "sysfs", "sysfs"),
                e("/proc", "proc", "proc"),
                e("/dev", "devtmpfs", "udev"),
            ]
        );
    }

    #[test]
    fn mountinfo_malformed_skipped() {
        let t = b"garbage\n\
                  \n\
                  1 2 0:1 / /nosep rw fuse.sshfs src rw\n\
                  1 2 0:1 / /short - fuse.sshfs\n\
                  1 2 / - fuse.sshfs src rw\n\
                  26 31 0:24 / /proc rw - proc proc rw\n\
                  27 31 0:5 / /dev rw shared:2 - devtmpfs udev";
        assert_eq!(
            parse_mountinfo(t),
            vec![e("/proc", "proc", "proc"), e("/dev", "devtmpfs", "udev")]
        );
    }

    #[test]
    fn find_takes_topmost_and_read_sees_root() {
        let v = vec![
            e("/a", "ext4", "x"),
            e("/b", "ext4", "y"),
            e("/a", "fuse.sshfs", "z"),
        ];
        assert_eq!(find(&v, Path::new("/a")).unwrap().source, "z");
        assert!(find(&v, Path::new("/c")).is_none());
        assert!(
            read()
                .unwrap()
                .iter()
                .any(|m| m.mount_point == Path::new("/"))
        );
    }
}
