//! sshfs driver (§6).

use crate::{DriverSettings, Flavor, SSH_OPTS, check};
use bifrost_core::{
    BoxFuture, DriverAvailability, MountDriver, MountError, MountHandle, MountRequest, MountSpec,
    MountState, marker,
};
use std::ffi::{OsStr, OsString};
use std::path::Path;
use std::time::Duration;

pub struct SshfsDriver {
    s: DriverSettings,
}

impl SshfsDriver {
    pub fn new(s: DriverSettings) -> Self {
        Self { s }
    }
}

impl MountDriver for SshfsDriver {
    fn name(&self) -> &str {
        "sshfs"
    }
    fn probe(&self) -> BoxFuture<'_, DriverAvailability> {
        Box::pin(async { probe_with(&check::search_path()).await })
    }
    fn mount(&self, req: MountRequest) -> BoxFuture<'_, Result<MountHandle, MountError>> {
        Box::pin(async move {
            let (Some(bin), Some(ssh)) = (check::which("sshfs"), check::which("ssh")) else {
                return Err(MountError::Unavailable("sshfs or ssh not found".into()));
            };
            let f =
                flavor().ok_or_else(|| MountError::Unavailable("no macFUSE or FUSE-T".into()))?;
            let argv = sshfs_argv(&req.spec, self.s.ssh_config.as_deref(), f);
            crate::mount_with("sshfs", &bin, &ssh, argv, req, &self.s).await
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
        Box::pin(crate::unmount_path(&h.local_path, force))
    }
}

/// pure
pub fn sshfs_argv(spec: &MountSpec, ssh_config: Option<&Path>, f: Flavor) -> Vec<OsString> {
    let id = spec.id.as_str();
    let fsname = marker(&spec.id, &spec.fingerprint());
    let mut a: Vec<OsString> = vec![
        "-f".into(),
        "-o".into(),
        format!("fsname={fsname},reconnect,idmap=user,transform_symlinks").into(),
        "-o".into(),
        SSH_OPTS.join(",").into(),
        "-o".into(),
        match f {
            // meant to unmount a SIGKILLed sshfs; fuse3 3.14 (verified) leaves it ENOTCONN → Stale → lazy detach
            Flavor::Linux => "auto_unmount".into(),
            Flavor::MacFuse => format!("volname={id},noappledouble").into(),
            Flavor::FuseT => format!("volname={id}").into(),
        },
    ];
    if spec.read_only {
        a.extend(["-o".into(), "ro".into()]);
    }
    if let Some(p) = spec.port {
        a.extend(["-p".into(), p.to_string().into()]);
    }
    if let Some(c) = ssh_config {
        a.extend(["-F".into(), c.into()]);
    }
    // validated positionals last: the host grammar forbids a leading '-', the local path is absolute
    a.extend([spec.source().into(), spec.local_path.clone().into()]);
    a
}

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

/// probe() with a caller-chosen search path (B14): tests pass a temp dir, never `set_var`.
pub(crate) async fn probe_with(path: &OsStr) -> DriverAvailability {
    let no = |w: &str| DriverAvailability::Unavailable(w.into());
    let Some(bin) = check::which_in("sshfs", path) else {
        return no("sshfs not found");
    };
    let out = check::run(&bin, &["--version".into()], Duration::from_secs(5)).await;
    let text = match out {
        Ok(o) => [o.stdout, o.stderr].concat(),
        Err(e) => return no(&format!("sshfs --version: {e}")),
    };
    let text = String::from_utf8_lossy(&text);
    let Some(version) = text.lines().find(|l| l.contains("SSHFS version")) else {
        return no("sshfs --version: not SSHFS");
    };
    if check::which_in("ssh", path).is_none() {
        return no("ssh not found");
    }
    let fuse = if cfg!(target_os = "macos") {
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
        detail: format!("{}, {fuse}", version.trim()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{fresh_dir, spec, static1, tmpdir};
    use bifrost_core::{DriverSelector, marker};
    use std::path::PathBuf;

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    const SSH: &str = "BatchMode=yes,ConnectTimeout=10,ServerAliveInterval=15,ServerAliveCountMax=3,ControlMaster=no,ControlPath=none";

    #[test]
    fn sshfs_argv_linux_golden() {
        let s = static1();
        let fsname = format!(
            "fsname=bifrost:static1@{},reconnect,idmap=user,transform_symlinks",
            s.fingerprint()
        );
        assert_eq!(
            sshfs_argv(&s, Some(Path::new("/tmp/e2e/ssh_config")), Flavor::Linux),
            os(&[
                "-f",
                "-o",
                &fsname,
                "-o",
                SSH,
                "-o",
                "auto_unmount",
                "-p",
                "2222",
                "-F",
                "/tmp/e2e/ssh_config",
                "bf@127.0.0.1:/home/bf",
                "/tmp/e2e/machines/static1",
            ])
        );
    }

    #[test]
    fn sshfs_argv_macfuse_golden() {
        let s = static1();
        let fsname = format!(
            "fsname=bifrost:static1@{},reconnect,idmap=user,transform_symlinks",
            s.fingerprint()
        );
        assert_eq!(
            sshfs_argv(&s, None, Flavor::MacFuse),
            os(&[
                "-f",
                "-o",
                &fsname,
                "-o",
                SSH,
                "-o",
                "volname=static1,noappledouble",
                "-p",
                "2222",
                "bf@127.0.0.1:/home/bf",
                "/tmp/e2e/machines/static1",
            ])
        );
    }

    #[test]
    fn sshfs_argv_fuset_golden() {
        let s = static1();
        let fsname = format!(
            "fsname=bifrost:static1@{},reconnect,idmap=user,transform_symlinks",
            s.fingerprint()
        );
        assert_eq!(
            sshfs_argv(&s, None, Flavor::FuseT),
            os(&[
                "-f",
                "-o",
                &fsname,
                "-o",
                SSH,
                "-o",
                "volname=static1",
                "-p",
                "2222",
                "bf@127.0.0.1:/home/bf",
                "/tmp/e2e/machines/static1",
            ])
        );
    }

    #[test]
    fn sshfs_argv_ipv6_home_port_ro_cfg() {
        let mut s = spec("v6", "fd7a:115c:a1e0::1", Some(2200), None, "~", true);
        s.driver = DriverSelector::Named("sshfs".into());
        let fsname = format!(
            "fsname={},reconnect,idmap=user,transform_symlinks",
            marker(&s.id, &s.fingerprint())
        );
        assert_eq!(
            sshfs_argv(&s, Some(Path::new("/c/ssh config")), Flavor::Linux),
            os(&[
                "-f",
                "-o",
                &fsname,
                "-o",
                SSH,
                "-o",
                "auto_unmount",
                "-o",
                "ro",
                "-p",
                "2200",
                "-F",
                "/c/ssh config",
                "[fd7a:115c:a1e0::1]:",
                "/tmp/e2e/machines/v6",
            ])
        );
        let s = spec("sub", "h.example", None, Some("u"), "~/src/x y", false);
        let a = sshfs_argv(&s, None, Flavor::Linux);
        assert_eq!(
            a[a.len() - 2..],
            os(&["u@h.example:src/x y", "/tmp/e2e/machines/sub"])
        );
    }

    fn script(dir: &Path, name: &str, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn probe_fake_sshfs_in_path() {
        let d = tmpdir("probe-fake");
        script(
            &d,
            "sshfs",
            "echo 'SSHFS version 3.7.3'; echo 'FUSE library version 3.14.0'",
        );
        script(&d, "ssh", "exit 0");
        script(&d, "fusermount3", "exit 0");
        match probe_with(d.as_os_str()).await {
            DriverAvailability::Available { binary, detail } => {
                assert_eq!(binary, d.join("sshfs"));
                assert_eq!(detail, "SSHFS version 3.7.3, fusermount3");
            }
            u => panic!("{u:?} (needs /dev/fuse)"),
        }
        // a binary that is not sshfs
        let d = tmpdir("probe-imposter");
        script(&d, "sshfs", "echo 'something else'");
        script(&d, "ssh", "exit 0");
        script(&d, "fusermount", "exit 0");
        assert!(matches!(
            probe_with(d.as_os_str()).await,
            DriverAvailability::Unavailable(_)
        ));
    }

    #[tokio::test]
    async fn probe_missing_binary_unavailable() {
        let d = tmpdir("probe-empty");
        let DriverAvailability::Unavailable(why) = probe_with(d.as_os_str()).await else {
            panic!("available with an empty PATH")
        };
        assert!(why.contains("sshfs"), "{why}");
        // sshfs present, ssh missing
        script(&d, "sshfs", "echo 'SSHFS version 3.7.3'");
        script(&d, "fusermount3", "exit 0");
        let DriverAvailability::Unavailable(why) = probe_with(d.as_os_str()).await else {
            panic!("available without ssh")
        };
        assert!(why.contains("ssh"), "{why}");
        // sshfs that fails to run (EACCES: exec bit for group only) is reported as such, not as "not SSHFS"
        use std::os::unix::fs::PermissionsExt;
        script(&d, "ssh", "exit 0");
        std::fs::set_permissions(d.join("sshfs"), std::fs::Permissions::from_mode(0o654)).unwrap();
        let DriverAvailability::Unavailable(why) = probe_with(d.as_os_str()).await else {
            panic!("available with an unrunnable sshfs")
        };
        assert!(
            why.starts_with("sshfs --version: ") && !why.contains("not SSHFS"),
            "{why}"
        );
    }

    // ---- #[ignore] docker tests: BIFROST_E2E_SSH=host:port:user:ssh_config (tests/e2e/lib.sh start_sshd) ----

    struct E2e {
        host: String,
        port: u16,
        user: String,
        cfg: PathBuf,
    }

    fn e2e() -> E2e {
        let v =
            std::env::var("BIFROST_E2E_SSH").expect("BIFROST_E2E_SSH=host:port:user:ssh_config");
        let mut it = v.splitn(4, ':');
        let mut next = || {
            it.next()
                .expect("BIFROST_E2E_SSH=host:port:user:ssh_config")
                .to_string()
        };
        E2e {
            host: next(),
            port: next().parse().unwrap(),
            user: next(),
            cfg: next().into(),
        }
    }

    /// A spec for the docker sshd, mounted under a fresh temp root.
    fn e2e_spec(id: &str, host: Option<&str>) -> (MountSpec, DriverSettings, PathBuf) {
        let e = e2e();
        // not a Tmp: a leftover mount must never meet remove_dir_all
        let root = fresh_dir(&format!("e2e-{id}")).canonicalize().unwrap();
        let mut s = spec(
            id,
            host.unwrap_or(&e.host),
            Some(e.port),
            Some(&e.user),
            "/home/bf",
            false,
        );
        s.local_path = root.join(id);
        let set = DriverSettings {
            ssh_config: Some(e.cfg),
            vfs_cache_mode: "writes".into(),
            mount_timeout: Duration::from_secs(15),
            state_dir: root.clone(),
        };
        (s, set, root)
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

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs docker sshd: BIFROST_E2E_SSH"]
    async fn preflight_exit0_and_hostkey_failure() {
        let (s, set, r1) = e2e_spec("pre", None);
        let ssh = crate::check::which("ssh").unwrap();
        let cfg = set.ssh_config.as_deref();
        assert_eq!(crate::check::ssh_preflight(&ssh, &s, cfg).await, Ok(()));
        // "localhost" is not in the scratch known_hosts: StrictHostKeyChecking yes + BatchMode → fail
        let (bad, _, r2) = e2e_spec("pre-bad", Some("localhost"));
        match crate::check::ssh_preflight(&ssh, &bad, cfg).await {
            Err(MountError::Failed(m)) => {
                assert!(m.contains("Host key verification failed"), "{m}")
            }
            r => panic!("{r:?}"),
        }
        // and the driver refuses to spawn sshfs for it: nothing mounted, error surfaced
        let (bad, set, root) = e2e_spec("hk", Some("localhost"));
        let (r, _) = req(&bad, &root);
        match SshfsDriver::new(set).mount(r).await {
            Err(MountError::Failed(m)) => {
                assert!(m.contains("Host key verification failed"), "{m}")
            }
            r => panic!("{r:?}"),
        }
        assert!(in_table(&bad.local_path).is_none());
        for r in [r1, r2, root] {
            tidy(&r);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs docker sshd: BIFROST_E2E_SSH"]
    async fn sshfs_mount_inspect_unmount() {
        let (s, set, root) = e2e_spec("m1", None);
        let d = SshfsDriver::new(set);
        assert!(matches!(
            d.probe().await,
            DriverAvailability::Available { .. }
        ));
        let (r, _rx) = req(&s, &root);
        let h = d.mount(r).await.unwrap();
        assert!(h.pid.is_some());
        assert_eq!(
            (h.driver.as_str(), &h.fingerprint),
            ("sshfs", &s.fingerprint())
        );
        // A10 check: the user fsname is the mountinfo source
        let e = in_table(&s.local_path).unwrap();
        assert_eq!(
            (e.fstype.as_str(), e.source),
            ("fuse.sshfs", marker(&s.id, &s.fingerprint()))
        );
        assert_eq!(
            std::fs::read_to_string(s.local_path.join("hello.txt")).unwrap(),
            "hello\n"
        );
        assert_eq!(d.inspect(&h).await, MountState::Healthy);
        let log = std::fs::read_to_string(root.join("m1.log")).unwrap();
        assert!(log.starts_with(bifrost_core::validate::LOG_HEADER), "{log}");
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.join("m1.log"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

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

        // busy: graceful refuses, nothing detached
        let f = std::fs::File::open(s.local_path.join("hello.txt")).unwrap();
        assert_eq!(d.unmount(&h, false).await, Err(MountError::Busy));
        assert!(in_table(&s.local_path).is_some());
        drop(f);

        d.unmount(&h, false).await.unwrap();
        assert!(in_table(&s.local_path).is_none());
        assert_eq!(d.inspect(&h).await, MountState::Missing);
        d.unmount(&h, false).await.unwrap(); // idempotent
        d.unmount(&h, true).await.unwrap();
        tidy(&root);
    }

    /// fuse3 3.14.0 (this host): the setuid auto_unmount helper does NOT unmount after SIGKILL (its
    /// open(mnt) as root gets EACCES, not ENOTCONN), so the mount stays behind as ENOTCONN → Stale →
    /// row 10 lazy detach. Newer fuse3 unmounts → Missing. Either way the supervisor reports the exit.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs docker sshd: BIFROST_E2E_SSH"]
    async fn sshfs_kill9_auto_unmount_missing() {
        let (s, set, root) = e2e_spec("k9", None);
        let d = SshfsDriver::new(set);
        let (r, rx) = req(&s, &root);
        let h = d.mount(r).await.unwrap();
        assert_eq!(d.inspect(&h).await, MountState::Healthy);
        // kill by the handle's pid, never `pkill -f fsname=` (it would also hit the fusermount3 helper, A13)
        let pid = h.pid.unwrap().to_string();
        assert!(
            std::process::Command::new("kill")
                .args(["-KILL", &pid])
                .status()
                .unwrap()
                .success()
        );
        let why = tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(10)))
            .await
            .unwrap()
            .expect("on_exit called");
        assert!(why.contains("9") || why.contains("KILL"), "{why}");
        let mut st = d.inspect(&h).await;
        for _ in 0..20 {
            if matches!(st, MountState::Missing | MountState::Stale(_)) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
            st = d.inspect(&h).await;
        }
        assert!(
            matches!(st, MountState::Missing | MountState::Stale(_)),
            "{st:?}"
        );
        eprintln!("after SIGKILL: {st:?}");
        d.unmount(&h, true).await.unwrap(); // Stale → lazy detach; Missing → idempotent Ok
        assert_eq!(d.inspect(&h).await, MountState::Missing);
        assert!(in_table(&s.local_path).is_none());
        tidy(&root);
    }
}
