//! rclone driver (§6): "rclone" = `rclone mount` (FUSE), "rclone-nfs" = `rclone nfsmount` (macOS only).
//! rclone's internal SSH library (no host-key check) is never used: `--sftp-ssh` hands every connection to
//! OpenSSH, with the same SSH_OPTS, ssh_config and known_hosts as sshfs. Spawn, readiness, inspect and
//! unmount are the shared lib.rs code.

use crate::{DriverSettings, Flavor, SSH_OPTS, check};
use bifrost_core::{
    BoxFuture, DriverAvailability, MountDriver, MountError, MountHandle, MountRequest, MountSpec,
    MountState, marker,
};
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

const MACOS_ONLY: &str = "macOS only";

pub struct RcloneDriver {
    s: DriverSettings,
    nfs: bool,
}

impl RcloneDriver {
    /// "rclone" / "rclone-nfs"
    pub fn new(s: DriverSettings, nfs: bool) -> Self {
        Self { s, nfs }
    }

    /// A23: one VFS cache per mount, so pending writes resume on its next mount and never cross hosts.
    // ponytail: a lazily detached rclone with open files can outlive its mount while row 9 starts a new one on the same --cache-dir (rclone: "can potentially cause data corruption"); upgrade: when nothing is mounted at local_path, scan /proc/*/cmdline for this exact --cache-dir= element and return Failed("cache still used by rclone pid N") so backoff waits
    // ponytail: rclone keys the VFS cache inside --cache-dir by a hash of the --sftp-* flags (vfs/:sftp{HASH}), so pending writes resume only while the --sftp-ssh/--sftp-host strings are unchanged (ssh path, ssh_config path, host, port, user); a change leaves them un-uploaded under <state>/rclone/<id>; upgrade: warn when vfs/ holds another :sftp{...} dir
    fn argv(&self, spec: &MountSpec, ssh: &Path, f: Flavor) -> Vec<OsString> {
        let cache = self.s.state_dir.join("rclone").join(spec.id.as_str());
        let (cfg, vfs) = (self.s.ssh_config.as_deref(), &self.s.vfs_cache_mode);
        rclone_argv(spec, ssh, cfg, vfs, &cache, self.nfs, f)
    }
}

impl MountDriver for RcloneDriver {
    fn name(&self) -> &str {
        if self.nfs { "rclone-nfs" } else { "rclone" }
    }
    /// Binaries and flags only, never the settings (S2 sign-off 3: doctor probes with placeholder settings).
    fn probe(&self) -> BoxFuture<'_, DriverAvailability> {
        Box::pin(async { probe_with(&check::search_path(), self.nfs).await })
    }
    fn mount(&self, req: MountRequest) -> BoxFuture<'_, Result<MountHandle, MountError>> {
        Box::pin(async move {
            if self.nfs && !cfg!(target_os = "macos") {
                return Err(MountError::Unavailable(MACOS_ONLY.into()));
            }
            let (Some(bin), Some(ssh)) = (check::which("rclone"), check::which("ssh")) else {
                return Err(MountError::Unavailable("rclone or ssh not found".into()));
            };
            let f = match flavor() {
                Some(f) => f,
                None if self.nfs => Flavor::MacFuse, // nfsmount needs no FUSE; its argv ignores the flavour
                None => return Err(MountError::Unavailable("no macFUSE or FUSE-T".into())),
            };
            sftp_ssh_check(&req.spec, &ssh, self.s.ssh_config.as_deref())?;
            let argv = self.argv(&req.spec, &ssh, f);
            crate::mount_with(self.name(), &bin, &ssh, argv, req, &self.s).await
        })
    }
    fn inspect<'a>(&'a self, h: &'a MountHandle) -> BoxFuture<'a, MountState> {
        Box::pin(crate::inspect_path(h))
    }
    fn unmount<'a>(
        &'a self,
        h: &'a MountHandle,
        force: bool,
    ) -> BoxFuture<'a, Result<(), MountError>> {
        // ponytail: a graceful unmount doesn't wait for VFS write-back, so writes closed <5s earlier stay in <state>/rclone/<id> until this id mounts again with the same flags; upgrade: rclone rc vfs/stats over a 0600 unix socket and return Busy while uploads are pending
        Box::pin(crate::unmount_path(&h.local_path, force))
    }
}

/// Runtime guard for `--sftp-ssh`, which rclone splits on spaces with "…" quoting (verified, rclone 1.75.1).
/// Bare tokens (ssh path, host, user) carry no whitespace or '"'; the quoted ssh_config carries no '"'; none
/// carries a control character or non-UTF-8. Host, User and port are clean by grammar: this catches a slip.
pub(crate) fn sftp_ssh_check(
    spec: &MountSpec,
    ssh: &Path,
    cfg: Option<&Path>,
) -> Result<(), MountError> {
    let quoted = |s: &str| !s.contains(|c: char| c == '"' || c.is_control());
    let bare = |s: &str| !s.is_empty() && quoted(s) && !s.contains(char::is_whitespace);
    let ok = ssh.to_str().is_some_and(bare)
        && cfg.is_none_or(|c| c.to_str().is_some_and(quoted))
        && bare(spec.host.as_str())
        && spec.user.as_ref().is_none_or(|u| bare(u.as_str()));
    ok.then_some(()).ok_or_else(|| {
        MountError::Refused(
            "--sftp-ssh: whitespace, '\"' or a control character in the ssh, ssh_config, host or user".into(),
        )
    })
}

/// Linux, or the macOS FUSE flavour (None: neither macFUSE nor FUSE-T). The same check is private in sshfs.rs.
#[cfg(not(target_os = "macos"))]
fn flavor() -> Option<Flavor> {
    Some(Flavor::Linux)
}

#[cfg(target_os = "macos")]
fn flavor() -> Option<Flavor> {
    let is = |p: &str| Path::new(p).exists();
    if is("/Library/Filesystems/macfuse.fs") {
        Some(Flavor::MacFuse)
    } else if is("/Library/Application Support/fuse-t") || is("/usr/local/lib/libfuse-t.dylib") {
        Some(Flavor::FuseT)
    } else {
        None
    }
}

/// stdout + stderr of `rclone <args>` (5s).
async fn text(bin: &Path, args: &[&str]) -> Result<String, String> {
    let a: Vec<OsString> = args.iter().map(OsString::from).collect();
    match check::run(bin, &a, Duration::from_secs(5)).await {
        Ok(o) => Ok(String::from_utf8_lossy(&[o.stdout, o.stderr].concat()).into_owned()),
        Err(e) => Err(format!("rclone {}: {e}", args.join(" "))),
    }
}

/// probe() with a caller-chosen search path (B14): tests pass a temp dir, never `set_var`.
pub(crate) async fn probe_with(path: &OsStr, nfs: bool) -> DriverAvailability {
    let no = |w: &str| DriverAvailability::Unavailable(w.into());
    if nfs && !cfg!(target_os = "macos") {
        return no(MACOS_ONLY);
    }
    let Some(bin) = check::which_in("rclone", path) else {
        return no("rclone not found");
    };
    let version = match text(&bin, &["version"]).await {
        Ok(t) => t.lines().next().unwrap_or_default().trim().to_string(),
        Err(e) => return no(&e),
    };
    if !version.starts_with("rclone v") {
        return no("rclone version: not rclone");
    }
    // feature detection: without --sftp-ssh only rclone's internal SSH (no host-key check) is left
    match text(&bin, &["help", "flags", "sftp"]).await {
        Ok(t) if t.contains("--sftp-ssh ") => {}
        Ok(_) => return no(&format!("{version} has no --sftp-ssh")),
        Err(e) => return no(&e),
    }
    if check::which_in("ssh", path).is_none() {
        return no("ssh not found");
    }
    let what = if nfs {
        if !Path::new("/sbin/mount_nfs").exists() {
            return no("/sbin/mount_nfs missing");
        }
        "nfsmount"
    } else if cfg!(target_os = "macos") {
        match flavor() {
            Some(Flavor::FuseT) => "FUSE-T",
            Some(_) => "macFUSE",
            None => return no("neither macFUSE nor FUSE-T is installed"),
        }
    } else {
        let Some(h) = ["fusermount3", "fusermount"]
            .into_iter()
            .find(|h| check::which_in(h, path).is_some())
        else {
            return no("fusermount3 not found");
        };
        if !Path::new("/dev/fuse").exists() {
            return no("/dev/fuse missing");
        }
        h
    };
    DriverAvailability::Available {
        binary: bin,
        detail: format!("{version}, {what}"),
    }
}

/// pure. Always `--flag=value`; the positionals (`:sftp:<path>`, an absolute local path) never start with '-'.
pub fn rclone_argv(
    spec: &MountSpec,
    ssh: &Path,
    ssh_config: Option<&Path>,
    vfs: &str,
    cache_dir: &Path,
    nfs: bool,
    f: Flavor,
) -> Vec<OsString> {
    // one space-separated value; the mount driver refuses unclean tokens (sftp_ssh_check)
    let mut cmd = vec![ssh.to_string_lossy().into_owned()];
    cmd.extend(SSH_OPTS.iter().flat_map(|o| ["-o".into(), o.to_string()]));
    if let Some(c) = ssh_config {
        cmd.extend(["-F".into(), format!("\"{}\"", c.to_string_lossy())]);
    }
    if let Some(p) = spec.port {
        cmd.extend(["-p".into(), p.to_string()]);
    }
    if let Some(u) = &spec.user {
        cmd.extend(["-l".into(), u.as_str().into()]);
    }
    // no "--": rclone appends `-s sftp`, which "--" would turn into a remote command
    cmd.push(spec.host.as_str().into());
    // nfsmount is read-only below "writes"
    let vfs = match vfs {
        "off" | "minimal" if nfs => "writes",
        v => v,
    };
    let mut cache = OsString::from("--cache-dir=");
    cache.push(cache_dir);
    let mut a: Vec<OsString> = vec![
        // ponytail: rclone nfsmount serves the remote on an unauthenticated random 127.0.0.1 NFS port that any local user can reach; fine on single-user Macs; upgrade: put "rclone" (FUSE) before "rclone-nfs" in the macOS auto_order when macFUSE/FUSE-T is installed
        if nfs { "nfsmount" } else { "mount" }.into(),
        format!(":sftp:{}", spec.remote.sftp_path()).into(),
        spec.local_path.clone().into(),
        "--config=/dev/null".into(), // never the user's rclone.conf
        format!("--sftp-host={}", spec.host.as_str()).into(),
        format!("--sftp-ssh={}", cmd.join(" ")).into(),
        "--sftp-shell-type=none".into(), // rclone never runs remote shell commands
        "--sftp-disable-hashcheck".into(),
        format!("--devname={}", marker(&spec.id, &spec.fingerprint())).into(),
        cache,
        format!("--vfs-cache-mode={vfs}").into(),
        // ponytail: --dir-cache-time=15s is a constant, a dead rclone remote can look healthy for <=15s and listings are refetched every 15s; make it a key if listing traffic matters
        "--dir-cache-time=15s".into(),
        "--log-level=NOTICE".into(),
    ];
    if spec.read_only {
        a.push("--read-only".into());
    }
    if !nfs && f != Flavor::Linux {
        a.push(format!("--volname={}", spec.id.as_str()).into());
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{Tmp, fresh_dir, spec, static1, tmpdir};
    use bifrost_core::{DriverSelector, marker};
    use std::path::PathBuf;
    use std::time::Duration;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    const SSH: &str = "/usr/bin/ssh -o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 \
                       -o ServerAliveCountMax=3 -o ControlMaster=no -o ControlPath=none";
    const FLAVORS: [Flavor; 3] = [Flavor::Linux, Flavor::MacFuse, Flavor::FuseT];

    fn specs() -> Vec<MountSpec> {
        vec![
            static1(),
            spec("v6", "fd7a:115c:a1e0::1", None, None, "~", true),
            spec(
                "home",
                "agent-01.tailnet.ts.net",
                Some(22),
                Some("sami"),
                "~/src x",
                false,
            ),
            spec("root", "10.0.0.1", None, Some("_svc.x-1"), "/", false),
        ]
    }

    fn argv(s: &MountSpec, cfg: Option<&str>, vfs: &str, nfs: bool, f: Flavor) -> Vec<OsString> {
        let (ssh, cache) = (Path::new("/usr/bin/ssh"), Path::new("/st/rclone/x"));
        rclone_argv(s, ssh, cfg.map(Path::new), vfs, cache, nfs, f)
    }

    fn settings(state_dir: &Path) -> DriverSettings {
        DriverSettings {
            ssh_config: None,
            vfs_cache_mode: "writes".into(),
            mount_timeout: Duration::from_secs(20),
            state_dir: state_dir.into(),
        }
    }

    #[test]
    fn rclone_argv_mount_golden() {
        let s = static1();
        let dev = format!("--devname=bifrost:static1@{}", s.fingerprint());
        let ssh = format!("--sftp-ssh={SSH} -F \"/tmp/e2e/ssh_config\" -p 2222 -l bf 127.0.0.1");
        assert_eq!(
            rclone_argv(
                &s,
                Path::new("/usr/bin/ssh"),
                Some(Path::new("/tmp/e2e/ssh_config")),
                "writes",
                Path::new("/st/rclone/static1"),
                false,
                Flavor::Linux
            ),
            os(&[
                "mount",
                ":sftp:/home/bf",
                "/tmp/e2e/machines/static1",
                "--config=/dev/null",
                "--sftp-host=127.0.0.1",
                &ssh,
                "--sftp-shell-type=none",
                "--sftp-disable-hashcheck",
                &dev,
                "--cache-dir=/st/rclone/static1",
                "--vfs-cache-mode=writes",
                "--dir-cache-time=15s",
                "--log-level=NOTICE",
            ])
        );
        // macOS FUSE: --volname; a v6 host goes bare into --sftp-ssh; "~" is the login dir ("")
        let v6 = spec("v6", "fd7a:115c:a1e0::1", None, None, "~", true);
        let dev = format!("--devname=bifrost:v6@{}", v6.fingerprint());
        let ssh = format!("--sftp-ssh={SSH} fd7a:115c:a1e0::1");
        for f in [Flavor::MacFuse, Flavor::FuseT] {
            assert_eq!(
                rclone_argv(
                    &v6,
                    Path::new("/usr/bin/ssh"),
                    None,
                    "full",
                    Path::new("/c"),
                    false,
                    f
                ),
                os(&[
                    "mount",
                    ":sftp:",
                    "/tmp/e2e/machines/v6",
                    "--config=/dev/null",
                    "--sftp-host=fd7a:115c:a1e0::1",
                    &ssh,
                    "--sftp-shell-type=none",
                    "--sftp-disable-hashcheck",
                    &dev,
                    "--cache-dir=/c",
                    "--vfs-cache-mode=full",
                    "--dir-cache-time=15s",
                    "--log-level=NOTICE",
                    "--read-only",
                    "--volname=v6",
                ])
            );
        }
    }

    #[test]
    fn rclone_argv_nfsmount_forces_writes() {
        let s = static1();
        let has = |a: &[OsString], x: &str| a.contains(&OsString::from(x));
        for (vfs, want) in [
            ("off", "writes"),
            ("minimal", "writes"),
            ("writes", "writes"),
            ("full", "full"),
        ] {
            for f in FLAVORS {
                let a = argv(&s, None, vfs, true, f);
                assert_eq!(a[0], "nfsmount");
                assert!(has(&a, &format!("--vfs-cache-mode={want}")), "{vfs}: {a:?}");
                assert_eq!(
                    a.iter()
                        .filter(|x| x.to_string_lossy().starts_with("--vfs-cache-mode="))
                        .count(),
                    1
                );
                // --volname is for the macOS FUSE mount only
                assert!(
                    !a.iter()
                        .any(|x| x.to_string_lossy().starts_with("--volname"))
                );
            }
            // the FUSE mount passes the configured mode through
            let a = argv(&s, None, vfs, false, Flavor::Linux);
            assert_eq!(a[0], "mount");
            assert!(has(&a, &format!("--vfs-cache-mode={vfs}")), "{a:?}");
        }
    }

    /// The one --sftp-ssh= value of an argv.
    fn sftp_ssh(a: &[OsString]) -> String {
        let v: Vec<_> = a
            .iter()
            .filter_map(|x| x.to_str()?.strip_prefix("--sftp-ssh="))
            .collect();
        assert_eq!(v.len(), 1, "{a:?}");
        v[0].to_string()
    }

    #[test]
    fn rclone_sftp_ssh_tokens_clean_cfg_quoted() {
        // rclone splits --sftp-ssh on spaces and honours "…" (verified, rclone 1.75.1): the cfg may hold spaces
        let cfg = "/c/ssh config";
        for s in specs() {
            for c in [None, Some(cfg)] {
                let v = sftp_ssh(&argv(&s, c, "writes", false, Flavor::Linux));
                let v = match c {
                    Some(c) => {
                        let q = format!(" -F \"{c}\" ");
                        assert!(v.contains(&q), "{v}");
                        v.replacen(&q, " ", 1)
                    }
                    None => v,
                };
                let toks: Vec<&str> = v.split(' ').collect();
                for t in &toks {
                    let dirty = t.contains(|ch: char| ch == '"' || ch.is_whitespace());
                    assert!(!t.is_empty() && !dirty, "{t:?} in {v}");
                }
                // rclone appends "-s sftp" right after the host; a "--" would make that a remote command
                assert_eq!(toks.last(), Some(&s.host.as_str()), "{v}");
                assert!(!toks.contains(&"--"), "{v}");
                assert!(
                    toks[0] == "/usr/bin/ssh"
                        && !toks[1..].iter().any(|t| t.starts_with('/') && *t != cfg)
                );
            }
        }
        // the runtime guard in mount(): Refused, never a mangled command line
        let s = static1();
        let check =
            |ssh: &str, c: Option<&str>| sftp_ssh_check(&s, Path::new(ssh), c.map(Path::new));
        assert_eq!(check("/usr/bin/ssh", None), Ok(()));
        assert_eq!(check("/usr/bin/ssh", Some(cfg)), Ok(()));
        for (ssh, c) in [
            ("/opt/my bin/ssh", None),
            ("/x\"/ssh", None),
            ("/x/ssh\t", None),
            ("", None),
            ("/usr/bin/ssh", Some("/c/a\"b")),
            ("/usr/bin/ssh", Some("/c/a\nb")),
        ] {
            assert!(
                matches!(check(ssh, c), Err(MountError::Refused(_))),
                "{ssh:?} {c:?}"
            );
        }
        use std::os::unix::ffi::OsStrExt;
        let raw = Path::new(OsStr::from_bytes(b"/x/\xff/ssh"));
        assert!(matches!(
            sftp_ssh_check(&s, raw, None),
            Err(MountError::Refused(_))
        ));
    }

    #[test]
    fn rclone_never_uses_internal_ssh() {
        // rclone's own SSH library skips host-key checks: only these --sftp-* flags ever appear, never
        // --sftp-key-file/--sftp-pass/--sftp-known-hosts-file/--sftp-user/--sftp-port/...
        let allowed = [
            "--sftp-host=",
            "--sftp-ssh=",
            "--sftp-shell-type=none",
            "--sftp-disable-hashcheck",
        ];
        for s in specs() {
            for c in [None, Some("/tmp/e2e/ssh_config")] {
                for nfs in [false, true] {
                    for f in FLAVORS {
                        let a = argv(&s, c, "writes", nfs, f);
                        // positionals first: subcommand, :sftp:<path>, the absolute local path
                        assert_eq!(a[0], if nfs { "nfsmount" } else { "mount" });
                        assert_eq!(
                            a[1],
                            OsString::from(format!(":sftp:{}", s.remote.sftp_path()))
                        );
                        assert_eq!(a[2], s.local_path.as_os_str());
                        let flags: Vec<String> =
                            a[3..].iter().map(|x| x.to_string_lossy().into()).collect();
                        for x in &flags {
                            assert!(x.starts_with("--"), "{x}");
                            if x.starts_with("--sftp-") {
                                assert!(allowed.iter().any(|p| x.starts_with(p)), "{x}");
                            }
                        }
                        let n = |p: &str| flags.iter().filter(|x| x.starts_with(p)).count();
                        assert_eq!((n("--sftp-ssh="), n("--config=")), (1, 1), "{flags:?}");
                        assert!(flags.contains(&"--config=/dev/null".into()));
                    }
                }
            }
        }
    }

    #[test]
    fn rclone_cache_dir_per_mount() {
        // A23: one VFS cache per mount id, under the state dir
        let d = RcloneDriver::new(settings(Path::new("/st")), false);
        let cache = |s: &MountSpec| -> Vec<OsString> {
            (d.argv(s, Path::new("/usr/bin/ssh"), Flavor::Linux)
                .into_iter())
            .filter(|x| x.to_string_lossy().starts_with("--cache-dir="))
            .collect()
        };
        let a = spec("a", "127.0.0.1", Some(2222), Some("bf"), "/home/bf", false);
        let b = spec("b", "127.0.0.1", Some(2222), Some("bf"), "/home/bf", false);
        assert_eq!(cache(&a), os(&["--cache-dir=/st/rclone/a"]));
        assert_eq!(cache(&b), os(&["--cache-dir=/st/rclone/b"]));
    }

    #[cfg(target_os = "linux")]
    fn script(dir: &Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A fake rclone 1.75.1 (`version`, `help flags sftp`) plus ssh and fusermount3.
    #[cfg(target_os = "linux")]
    fn fake_tools(d: &Path) {
        script(
            d,
            "rclone",
            r#"case "$1" in
  version) echo 'rclone v1.75.1'; echo '- os/type: linux' ;;
  help) echo '      --sftp-ssh SpaceSepList   Path and arguments to external ssh binary' ;;
esac"#,
        );
        script(d, "ssh", "exit 0");
        script(d, "fusermount3", "exit 0");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn rclone_nfs_unavailable_on_linux() {
        let d = tmpdir("rclone-nfs");
        fake_tools(&d);
        let macos_only = DriverAvailability::Unavailable("macOS only".into());
        assert_eq!(probe_with(d.as_os_str(), true).await, macos_only);
        let drv = RcloneDriver::new(settings(&d), true);
        assert_eq!(drv.name(), "rclone-nfs");
        assert_eq!(drv.probe().await, macos_only);
        let r = MountRequest {
            spec: static1(),
            log_path: d.join("x.log"),
            on_exit: Box::new(|_| {}),
        };
        assert_eq!(
            drv.mount(r).await,
            Err(MountError::Unavailable("macOS only".into()))
        );
        // the FUSE driver with the very same tools is fine
        assert!(matches!(
            probe_with(d.as_os_str(), false).await,
            DriverAvailability::Available { .. }
        ));
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn probe_fake_rclone_in_path() {
        let d = tmpdir("rclone-probe");
        fake_tools(&d);
        assert_eq!(
            probe_with(d.as_os_str(), false).await,
            DriverAvailability::Available {
                binary: d.join("rclone"),
                detail: "rclone v1.75.1, fusermount3".into()
            },
            "needs /dev/fuse"
        );
        let unavailable = |a: DriverAvailability, want: &str| match a {
            DriverAvailability::Unavailable(why) => assert!(why.contains(want), "{why}"),
            a => panic!("{a:?}"),
        };
        // too old for --sftp-ssh: its internal SSH (no host-key check) would be the only way in
        script(&d, "rclone", "echo 'rclone v1.40.0'");
        unavailable(probe_with(d.as_os_str(), false).await, "--sftp-ssh");
        // not rclone at all
        script(&d, "rclone", "echo 'something else'");
        unavailable(probe_with(d.as_os_str(), false).await, "rclone version");
        // no ssh / no rclone
        fake_tools(&d);
        std::fs::remove_file(d.join("ssh")).unwrap();
        unavailable(probe_with(d.as_os_str(), false).await, "ssh");
        let e = tmpdir("rclone-probe-empty");
        unavailable(probe_with(e.as_os_str(), false).await, "rclone not found");
    }

    // ---- #[ignore] docker tests: BIFROST_E2E_SSH=host:port:user:ssh_config (tests/e2e/lib.sh start_sshd) ----
    // sshfs.rs's docker helpers are private to its test module; these are the rclone copies.

    /// The docker sshd as (driver, spec, mount root, state dir). The root is not a Tmp: a leftover mount must
    /// never meet remove_dir_all. The state dir is (rclone nests its cache there; nothing is mounted in it).
    fn e2e(id: &str) -> (RcloneDriver, MountSpec, PathBuf, Tmp) {
        let v = std::env::var("BIFROST_E2E_SSH").unwrap_or_default();
        let [host, port, user, cfg] = v.splitn(4, ':').collect::<Vec<_>>()[..] else {
            panic!("BIFROST_E2E_SSH=host:port:user:ssh_config")
        };
        let root = fresh_dir(&format!("e2e-{id}")).canonicalize().unwrap();
        let mut s = spec(
            id,
            host,
            Some(port.parse().unwrap()),
            Some(user),
            "/home/bf",
            false,
        );
        s.driver = DriverSelector::Named("rclone".into());
        s.local_path = root.join(id);
        let state = tmpdir(&format!("e2e-{id}-state"));
        let set = DriverSettings {
            ssh_config: Some(cfg.into()),
            ..settings(&state)
        };
        (RcloneDriver::new(set, false), s, root, state)
    }

    fn req(s: &MountSpec, root: &Path) -> (MountRequest, std::sync::mpsc::Receiver<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let r = MountRequest {
            spec: s.clone(),
            log_path: root.join(format!("{}.log", s.id.as_str())),
            on_exit: Box::new(move |d| tx.send(d).unwrap()),
        };
        (r, rx)
    }

    /// Non-recursive on purpose: it fails rather than ever deleting through a mount.
    fn tidy(root: &Path) {
        for e in std::fs::read_dir(root).unwrap() {
            let p = e.unwrap().path();
            let _ = std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir(&p));
        }
        std::fs::remove_dir(root).unwrap();
    }

    fn in_table(p: &Path) -> Option<crate::table::MountEntry> {
        crate::table::find(&crate::table::read().unwrap(), p).cloned()
    }

    async fn on_exit(rx: std::sync::mpsc::Receiver<String>) -> String {
        tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(10)))
            .await
            .unwrap()
            .expect("on_exit called")
    }

    /// What the sshd really stores: `cat` over our own ssh, not through the mount.
    async fn remote_cat(s: &MountSpec, cfg: &Path, file: &str) -> String {
        let port = s.port.unwrap().to_string();
        let user = s.user.as_ref().unwrap().as_str();
        let args: Vec<OsString> = [
            "-F",
            &cfg.to_string_lossy(),
            "-o",
            "BatchMode=yes",
            "-p",
            &port,
        ]
        .into_iter()
        .chain([
            "-l",
            user,
            s.host.as_str(),
            "cat",
            &format!("/home/bf/{file}"),
        ])
        .map(OsString::from)
        .collect();
        let ssh = crate::check::which("ssh").unwrap();
        let o = crate::check::run(&ssh, &args, Duration::from_secs(15))
            .await
            .unwrap();
        String::from_utf8_lossy(&o.stdout).into_owned()
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs docker sshd: BIFROST_E2E_SSH"]
    async fn rclone_mount_write_roundtrip() {
        let (d, s, root, state) = e2e("rc1");
        assert!(matches!(
            d.probe().await,
            DriverAvailability::Available { .. }
        ));
        let (r, rx) = req(&s, &root);
        let h = d.mount(r).await.unwrap();
        assert!(h.pid.is_some());
        assert_eq!(
            (h.driver.as_str(), &h.fingerprint),
            ("rclone", &s.fingerprint())
        );
        // --devname is the fuse.rclone mountinfo source: readiness and inspect need no fallback
        let e = in_table(&s.local_path).unwrap();
        assert_eq!(
            (e.fstype.as_str(), e.source),
            ("fuse.rclone", marker(&s.id, &s.fingerprint()))
        );
        assert_eq!(d.inspect(&h).await, MountState::Healthy);
        assert_eq!(
            std::fs::read_to_string(s.local_path.join("hello.txt")).unwrap(),
            "hello\n"
        );

        // write through the mount, read it back, then find it on the server (vfs write-back: 5s)
        let name = format!("rc-{:016x}.txt", bifrost_core::validate::random_u64());
        let f = s.local_path.join(&name);
        std::fs::write(&f, "round trip\n").unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "round trip\n");
        let cfg = d.s.ssh_config.clone().unwrap();
        let mut remote = String::new();
        for _ in 0..40 {
            remote = remote_cat(&s, &cfg, &name).await;
            if remote == "round trip\n" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        assert_eq!(remote, "round trip\n");
        // A23: this mount's own VFS cache, under the state dir
        assert!(state.join("rclone/rc1").is_dir());

        // a second mount() of the same spec adopts the live mount (A9 step 2), no new process
        let (r, _) = req(&s, &root);
        let again = d.mount(r).await.unwrap();
        assert_eq!(
            again,
            MountHandle {
                pid: None,
                ..h.clone()
            }
        );

        std::fs::remove_file(&f).unwrap();
        d.unmount(&h, false).await.unwrap();
        assert!(in_table(&s.local_path).is_none());
        assert_eq!(d.inspect(&h).await, MountState::Missing);
        // rclone exits by itself once unmounted; the supervisor reaps it and reports
        eprintln!("rclone exit after unmount: {}", on_exit(rx).await);
        tidy(&root);
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs docker sshd: BIFROST_E2E_SSH"]
    async fn rclone_kill9_stale_then_lazy() {
        let (d, s, root, _state) = e2e("rck9");
        let (r, rx) = req(&s, &root);
        let h = d.mount(r).await.unwrap();
        assert_eq!(d.inspect(&h).await, MountState::Healthy);
        // kill by the handle's pid, never `pkill -f` (A13)
        let pid = h.pid.unwrap().to_string();
        assert!(
            std::process::Command::new("kill")
                .args(["-KILL", &pid])
                .status()
                .unwrap()
                .success()
        );
        let why = on_exit(rx).await;
        assert!(why.contains('9') || why.contains("KILL"), "{why}");
        // rclone has no auto_unmount: the entry stays behind, ENOTCONN → Stale (row 10)
        let mut st = d.inspect(&h).await;
        for _ in 0..20 {
            if matches!(st, MountState::Stale(_)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            st = d.inspect(&h).await;
        }
        assert!(matches!(st, MountState::Stale(_)), "{st:?}");
        assert!(in_table(&s.local_path).is_some());
        // row 10: lazy detach, nothing killed
        d.unmount(&h, true).await.unwrap();
        assert_eq!(d.inspect(&h).await, MountState::Missing);
        assert!(in_table(&s.local_path).is_none());
        tidy(&root);
    }
}
