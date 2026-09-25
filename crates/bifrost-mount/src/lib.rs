//! Mount drivers (contract §6). S1-C: lib/table/check/sshfs; S3-J: rclone (reuses `mount_with`).

pub mod check;
mod rclone;
mod sshfs;
pub mod table;

pub use rclone::{RcloneDriver, rclone_argv};
pub use sshfs::{SshfsDriver, sshfs_argv};

use bifrost_core::validate::{LOG_HEADER, clean, tail};
use bifrost_core::{
    MountDriver, MountError, MountHandle, MountId, MountRequest, MountSpec, marker, parse_marker,
};
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverSettings {
    pub ssh_config: Option<PathBuf>,
    pub vfs_cache_mode: String,
    pub mount_timeout: Duration,
    /// rclone --cache-dir=<state>/rclone/<id> (A23)
    pub state_dir: PathBuf,
}

/// Every OS gets all three; rclone-nfs probes Unavailable("macOS only") on Linux.
pub fn drivers(s: &DriverSettings) -> Vec<Arc<dyn MountDriver>> {
    vec![
        Arc::new(SshfsDriver::new(s.clone())),
        Arc::new(RcloneDriver::new(s.clone(), false)),
        Arc::new(RcloneDriver::new(s.clone(), true)),
    ]
}

// ponytail: SSH timings are constants (a dead link takes >=45s to detect; the command line overrides ssh_config values); an [ssh] section if users need tuning
pub const SSH_OPTS: [&str; 6] = [
    "BatchMode=yes",
    "ConnectTimeout=10",
    "ServerAliveInterval=15",
    "ServerAliveCountMax=3",
    "ControlMaster=no",
    "ControlPath=none",
];

/// ssh's own flags for the preflight and `--sftp-ssh` (sshfs adds `-x -a -oClearAllForwardings=yes` itself, and its `-o`
/// passthrough rejects these): no agent, X11 or port forwarding from ssh_config ever reaches a mounted host, as sftp/scp do.
pub const SSH_CLI_HARDENING: [&str; 6] = [
    "-a",
    "-x",
    "-o",
    "ClearAllForwardings=yes",
    "-o",
    "PermitLocalCommand=no",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    Linux,
    MacFuse,
    FuseT,
}

/// Linux, or the macOS FUSE flavour (None: neither macFUSE nor FUSE-T).
#[cfg(not(target_os = "macos"))]
pub(crate) fn flavor() -> Option<Flavor> {
    Some(Flavor::Linux)
}

#[cfg(target_os = "macos")]
pub(crate) fn flavor() -> Option<Flavor> {
    let is = |p: &str| Path::new(p).exists();
    if is("/Library/Filesystems/macfuse.fs") {
        Some(Flavor::MacFuse)
    } else if is("/Library/Application Support/fuse-t") || is("/usr/local/lib/libfuse-t.dylib") {
        Some(Flavor::FuseT)
    } else {
        None
    }
}

/// pure
pub fn preflight_argv(spec: &MountSpec, ssh_config: Option<&Path>) -> Vec<OsString> {
    let mut a: Vec<OsString> = (SSH_CLI_HARDENING.iter().copied())
        .chain(SSH_OPTS.iter().flat_map(|o| ["-o", o]))
        .map(Into::into)
        .collect();
    if let Some(c) = ssh_config {
        a.extend(["-F".into(), c.into()]);
    }
    if let Some(p) = spec.port {
        a.extend(["-p".into(), p.to_string().into()]);
    }
    if let Some(u) = &spec.user {
        a.extend(["-l".into(), u.as_str().into()]);
    }
    a.extend(["-s", "--", spec.host.as_str(), "sftp"].map(OsString::from));
    a
}

/// Graceful: fusermount3 -u / umount (busy ⇒ Busy, nothing killed). Force: lazy detach, never a kill.
/// Idempotent: the mount table decides, not the helper's exit code.
pub async fn unmount_path(path: &Path, force: bool) -> Result<(), MountError> {
    if !path.is_absolute() {
        return Err(MountError::Refused(format!(
            "not absolute: {}",
            path.display()
        )));
    }
    let mounted = || {
        table::read()
            .map(|t| table::find(&t, path).is_some())
            .map_err(|e| MountError::Failed(format!("mount table: {e}")))
    };
    if !mounted()? {
        return Ok(());
    }
    let p = path.as_os_str().to_os_string();
    #[cfg(not(target_os = "macos"))]
    let cmds = {
        let bin = check::which("fusermount3")
            .or_else(|| check::which("fusermount"))
            .ok_or_else(|| MountError::Unavailable("fusermount3 not found".into()))?;
        let args = if force {
            vec!["-u".into(), "-z".into(), p]
        } else {
            vec!["-u".into(), p]
        };
        vec![(bin, args)]
    };
    #[cfg(target_os = "macos")]
    let cmds = if force {
        vec![
            (
                PathBuf::from("/usr/sbin/diskutil"),
                vec!["unmount".into(), "force".into(), p.clone()],
            ),
            (PathBuf::from("/sbin/umount"), vec!["-f".into(), p]),
        ]
    } else {
        vec![(PathBuf::from("/sbin/umount"), vec![p])]
    };
    let mut why = String::new();
    for (bin, args) in cmds {
        why = match check::run(&bin, &args, Duration::from_secs(10)).await {
            Ok(o) => tail(&String::from_utf8_lossy(&o.stderr), 512),
            Err(e) => e.to_string(),
        };
        if !mounted()? {
            return Ok(());
        }
    }
    if !force && busy(&why) {
        return Err(MountError::Busy);
    }
    Err(MountError::Failed(format!("still mounted: {why}")))
}

/// The errno text, never the bare word: stderr carries the path, and a root or id may contain "busy".
fn busy(why: &str) -> bool {
    // "Device or resource busy" (fusermount3), "Resource busy" (macOS umount)
    why.to_ascii_lowercase().contains("resource busy")
}

/// Pure. Only entries whose parent == root. Marker source ⇒ adopt (driver from fstype, else record, else "sshfs");
/// no marker ⇒ on macOS only, adopt when state.json has a record with the same local_path (NFS fallback); else foreign.
/// `pid` comes from the state record with the same local_path, else None (B15).
// ponytail: only mounts directly under the current root are adopted, old-root mounts are left alone after a root change; unmount any marker mount outside the root at startup
// ponytail: on macOS only, an unmarked entry is trusted on a state.json record (Linux fsname markers always work, so there it is foreign), and an unknown fstype guesses "sshfs" (inspect/unmount are shared, so harmless); read the NFS source to tell drivers apart
pub fn adopt(
    entries: &[table::MountEntry],
    root: &Path,
    records: &BTreeMap<MountId, MountHandle>,
) -> Vec<MountHandle> {
    let mut top = BTreeMap::new(); // the topmost entry per mount point
    for e in entries
        .iter()
        .filter(|e| e.mount_point.parent() == Some(root))
    {
        top.insert(&e.mount_point, e);
    }
    top.into_values()
        .filter_map(|e| {
            let rec = records.values().find(|r| r.local_path == e.mount_point);
            let Some((id, fingerprint)) = parse_marker(&e.source) else {
                return rec.filter(|_| cfg!(target_os = "macos")).cloned();
            };
            let driver = match e.fstype.as_str() {
                "fuse.sshfs" => "sshfs".into(),
                "fuse.rclone" => "rclone".into(),
                _ => rec.map_or_else(|| "sshfs".into(), |r| r.driver.clone()),
            };
            (e.mount_point.file_name() == Some(OsStr::new(id.as_str()))).then(|| MountHandle {
                id,
                driver,
                local_path: e.mount_point.clone(),
                fingerprint,
                pid: rec.and_then(|r| r.pid),
            })
        })
        .collect()
}

/// mount() step 2 (A9): what to do with whatever is mounted at `local_path` right now.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Step2 {
    Free,
    /// our marker, same fingerprint: a leftover of a dropped or timed-out attempt
    Adopt,
    /// our marker, older spec: lazy detach, then mount
    Detach,
    Refused(String),
}

pub(crate) fn step2(entries: &[table::MountEntry], spec: &MountSpec) -> Step2 {
    let Some(e) = table::find(entries, &spec.local_path) else {
        return Step2::Free;
    };
    match parse_marker(&e.source) {
        Some((id, fp)) if id == spec.id && fp == spec.fingerprint() => Step2::Adopt,
        Some((id, _)) if id == spec.id => Step2::Detach,
        // the fstype subtype is chosen by whoever mounted it: clean it with the source
        _ => Step2::Refused(clean(
            &format!("occupied by {} {}", e.fstype, e.source),
            512,
        )),
    }
}

/// Step 3. Runs only once the mount table says `p` is not a mountpoint (hung-FUSE guard 3).
pub(crate) fn prepare_mountpoint(p: &Path) -> Result<(), MountError> {
    use std::os::unix::fs::DirBuilderExt;
    let refuse = |why: &str| Err(MountError::Refused(format!("{} {why}", p.display())));
    let failed = |e: std::io::Error| MountError::Failed(format!("{}: {e}", p.display()));
    match std::fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => std::fs::DirBuilder::new()
            .mode(0o700)
            .create(p)
            .map_err(failed),
        Err(e) => Err(failed(e)),
        Ok(m) if m.file_type().is_symlink() => refuse("is a symlink"),
        Ok(m) if !m.is_dir() => refuse("is not a directory"),
        // fuse3 would mount over the files and hide them
        Ok(_) if std::fs::read_dir(p).map_err(failed)?.next().is_some() => refuse("is not empty"),
        Ok(_) => Ok(()),
    }
}

/// B7: a macOS FUSE permission failure gets a pointer to the fix.
fn with_hint(t: String) -> String {
    let hit = ["kernel extension", "System Extension", "not permitted"]
        .iter()
        .any(|k| t.contains(k));
    if cfg!(target_os = "macos") && hit {
        t + " (macOS: allow the macFUSE system extension in System Settings → Privacy & Security)"
    } else {
        t
    }
}

/// tail(last 2 KiB of the child log, 512) (A8) + the B7 hint.
fn log_tail(p: &Path) -> String {
    let b = std::fs::read(p).unwrap_or_default();
    with_hint(tail(
        &String::from_utf8_lossy(&b[b.len().saturating_sub(2048)..]),
        512,
    ))
}

/// mount() steps 1–6 for any driver (§6): `bin` and `ssh` are absolute, `argv` is the driver's own.
/// Ok only once the mount table shows our entry; then a supervisor task owns the child.
pub(crate) async fn mount_with(
    driver: &str,
    bin: &Path,
    ssh: &Path,
    argv: Vec<OsString>,
    req: MountRequest,
    s: &DriverSettings,
) -> Result<MountHandle, MountError> {
    let MountRequest {
        spec,
        log_path,
        on_exit,
    } = req;
    let spec = &spec;
    let local = &spec.local_path;
    let fp = spec.fingerprint();
    let handle = |pid| MountHandle {
        id: spec.id.clone(),
        driver: driver.into(),
        local_path: local.clone(),
        fingerprint: fp.clone(),
        pid,
    };
    // 1. the request: <canonical root>/<id>
    let parent_ok = local.is_absolute()
        && (local.parent()).is_some_and(|p| std::fs::canonicalize(p).is_ok_and(|c| c == p));
    if !parent_ok || local.file_name() != Some(OsStr::new(spec.id.as_str())) {
        return Err(MountError::Refused(format!(
            "bad local path {}",
            local.display()
        )));
    }
    // 2. whatever is mounted there already (A9)
    let table = || table::read().map_err(|e| MountError::Failed(format!("mount table: {e}")));
    match step2(&table()?, spec) {
        Step2::Free => {}
        Step2::Adopt => return Ok(handle(None)),
        Step2::Detach => unmount_path(local, true).await?,
        Step2::Refused(why) => return Err(MountError::Refused(why)),
    }
    // 3–6; a failed attempt removes the mountpoint again, only once the table says it is none (§6)
    // ponytail: an attempt dropped whole by the actor's outer timeout keeps its empty dir until the next attempt for that id; upgrade: a Drop guard once a dropped attempt's child can no longer mount there
    prepare_mountpoint(local)?;
    let r: Result<MountHandle, MountError> = async {
        check::ssh_preflight(ssh, spec, s.ssh_config.as_deref()).await?;
        // 5. spawn: never a pipe (a full pipe stalls FUSE; a dead reader SIGPIPEs the orphan)
        // ponytail: the child log is truncated at each spawn, no rotation or size cap; cap it on the health tick
        let log = {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&log_path)
                .map_err(|e| MountError::Failed(format!("log {}: {e}", log_path.display())))?;
            let cmd: Vec<_> = std::iter::once(bin.as_os_str())
                .chain(argv.iter().map(OsString::as_os_str))
                .map(OsStr::to_string_lossy)
                .collect();
            writeln!(f, "{LOG_HEADER}{}", cmd.join(" "))
                .map_err(|e| MountError::Failed(format!("log: {e}")))?;
            f
        };
        let io = |e: std::io::Error| MountError::Failed(format!("spawn {}: {e}", bin.display()));
        let mut child = tokio::process::Command::new(bin)
            .args(&argv)
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(io)?)
            .stderr(log)
            .process_group(0) // a terminal ^C or the daemon's death never signals it
            .kill_on_drop(false)
            .spawn()
            .map_err(io)?;
        // 6. readiness
        let mark = marker(&spec.id, &fp);
        let ours = || {
            let t = table::read().unwrap_or_default();
            // macOS: FUSE-T/NFS may not show the marker; step 2 made sure nothing else was there
            table::find(&t, local).is_some_and(|e| !cfg!(target_os = "linux") || e.source == mark)
        };
        let deadline = tokio::time::Instant::now() + s.mount_timeout;
        loop {
            if ours() {
                let pid = child.id();
                tokio::spawn(async move {
                    let st = child.wait().await; // reaps it; no kill channel
                    on_exit(st.map_or_else(|e| e.to_string(), |s| s.to_string()));
                });
                return Ok(handle(pid));
            }
            if let Ok(Some(st)) = child.try_wait() {
                return Err(MountError::Failed(format!(
                    "{driver} {st}: {}",
                    log_tail(&log_path)
                )));
            }
            if tokio::time::Instant::now() >= deadline {
                // ponytail: this timed-out spawn of ours is the only process ever signalled, no orphan scan (one stuck in connect lives until ConnectTimeout; a lazily detached sshfs lingers until its last reference closes); none (deliberate: killing loses data)
                let _ = child.start_kill();
                let _ = child.wait().await;
                if ours() {
                    let _ = unmount_path(local, true).await;
                }
                return Err(MountError::Failed(format!(
                    "timed out after {}s: {}",
                    s.mount_timeout.as_secs(),
                    log_tail(&log_path)
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
    .await;
    if r.is_err() && table::read().is_ok_and(|t| table::find(&t, local).is_none()) {
        let _ = std::fs::remove_dir(local);
    }
    r
}

/// inspect() for any driver: our entry at the path (Linux: carrying the marker) → liveness, else Missing.
pub(crate) async fn inspect_path(h: &MountHandle) -> bifrost_core::MountState {
    let t = match table::read() {
        Ok(t) => t,
        Err(e) => return bifrost_core::MountState::Degraded(format!("mount table: {e}")),
    };
    let mark = marker(&h.id, &h.fingerprint);
    match table::find(&t, &h.local_path) {
        Some(e) if !cfg!(target_os = "linux") || e.source == mark => {
            check::liveness(&h.local_path).await
        }
        _ => bifrost_core::MountState::Missing,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bifrost_core::{DriverSelector, Host, Name, RemotePath, User, marker};
    use table::MountEntry;

    /// A fresh dir under $TMPDIR (never an existing one); removed on drop. Never used for mounts.
    pub(crate) struct Tmp(PathBuf);

    impl std::ops::Deref for Tmp {
        type Target = PathBuf;
        fn deref(&self) -> &PathBuf {
            &self.0
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    pub(crate) fn fresh_dir(name: &str) -> PathBuf {
        let rand = bifrost_core::validate::random_u64();
        let d = std::env::temp_dir().join(format!("bifrost-mount-{name}-{rand:016x}"));
        std::fs::create_dir(&d).unwrap();
        d
    }

    pub(crate) fn tmpdir(name: &str) -> Tmp {
        Tmp(fresh_dir(name))
    }

    pub(crate) fn spec(
        id: &str,
        host: &str,
        port: Option<u16>,
        user: Option<&str>,
        remote: &str,
        ro: bool,
    ) -> MountSpec {
        MountSpec {
            id: Name::parse(id).unwrap(),
            machine: Name::parse(id).unwrap(),
            host: Host::parse(host).unwrap(),
            port,
            user: user.map(|u| User::parse(u).unwrap()),
            remote: RemotePath::parse(remote).unwrap(),
            local_path: PathBuf::from("/tmp/e2e/machines").join(id),
            driver: DriverSelector::Auto,
            read_only: ro,
        }
    }

    pub(crate) fn static1() -> MountSpec {
        spec(
            "static1",
            "127.0.0.1",
            Some(2222),
            Some("bf"),
            "/home/bf",
            false,
        )
    }

    // ---- #[ignore] docker helpers: BIFROST_E2E_SSH=host:port:user:ssh_config (tests/e2e/lib.sh start_sshd) ----

    /// A spec for the docker sshd (`host` overrides its host), mounted under a fresh root that is also the
    /// settings' state dir. The root is not a Tmp: a leftover mount must never meet remove_dir_all.
    pub(crate) fn e2e(id: &str, host: Option<&str>) -> (MountSpec, DriverSettings, PathBuf) {
        let v = std::env::var("BIFROST_E2E_SSH").unwrap_or_default();
        let [h, port, user, cfg] = v.splitn(4, ':').collect::<Vec<_>>()[..] else {
            panic!("BIFROST_E2E_SSH=host:port:user:ssh_config")
        };
        let root = fresh_dir(&format!("e2e-{id}")).canonicalize().unwrap();
        let port = Some(port.parse().unwrap());
        let mut s = spec(id, host.unwrap_or(h), port, Some(user), "/home/bf", false);
        s.local_path = root.join(id);
        let set = DriverSettings {
            ssh_config: Some(cfg.into()),
            vfs_cache_mode: "writes".into(),
            mount_timeout: Duration::from_secs(20),
            state_dir: root.clone(),
        };
        (s, set, root)
    }

    pub(crate) fn req(
        s: &MountSpec,
        root: &Path,
    ) -> (MountRequest, std::sync::mpsc::Receiver<String>) {
        let (tx, rx) = std::sync::mpsc::channel();
        let r = MountRequest {
            spec: s.clone(),
            log_path: root.join(format!("{}.log", s.id.as_str())),
            on_exit: Box::new(move |d| tx.send(d).unwrap()),
        };
        (r, rx)
    }

    /// Non-recursive on purpose: it fails rather than ever deleting through a mount.
    pub(crate) fn tidy(root: &Path) {
        for e in std::fs::read_dir(root).unwrap() {
            let p = e.unwrap().path();
            let _ = std::fs::remove_file(&p).or_else(|_| std::fs::remove_dir(&p));
        }
        std::fs::remove_dir(root).unwrap();
    }

    pub(crate) fn in_table(p: &Path) -> Option<MountEntry> {
        table::find(&table::read().unwrap(), p).cloned()
    }

    fn os(v: &[&str]) -> Vec<OsString> {
        v.iter().map(OsString::from).collect()
    }

    fn entry(mp: &Path, fstype: &str, source: &str) -> MountEntry {
        MountEntry {
            mount_point: mp.to_path_buf(),
            fstype: fstype.into(),
            source: source.into(),
        }
    }

    fn all_specs() -> Vec<MountSpec> {
        vec![
            static1(),
            spec("v6", "fd7a:115c:a1e0::1", None, None, "~", true),
            spec(
                "home",
                "agent-01.tailnet.ts.net",
                Some(22),
                Some("sami"),
                "~/src",
                false,
            ),
            spec("root", "10.0.0.1", None, Some("_svc.x-1"), "/", false),
        ]
    }

    #[test]
    fn preflight_argv_golden() {
        let cfg = Path::new("/tmp/e2e/ssh_config");
        assert_eq!(
            preflight_argv(&static1(), Some(cfg)),
            os(&[
                "-a",
                "-x",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "PermitLocalCommand=no",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ControlMaster=no",
                "-o",
                "ControlPath=none",
                "-F",
                "/tmp/e2e/ssh_config",
                "-p",
                "2222",
                "-l",
                "bf",
                "-s",
                "--",
                "127.0.0.1",
                "sftp",
            ])
        );
        let v6 = spec("v6", "fd7a:115c:a1e0::1", None, None, "~", false);
        assert_eq!(
            preflight_argv(&v6, None),
            os(&[
                "-a",
                "-x",
                "-o",
                "ClearAllForwardings=yes",
                "-o",
                "PermitLocalCommand=no",
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=10",
                "-o",
                "ServerAliveInterval=15",
                "-o",
                "ServerAliveCountMax=3",
                "-o",
                "ControlMaster=no",
                "-o",
                "ControlPath=none",
                "-s",
                "--",
                "fd7a:115c:a1e0::1",
                "sftp",
            ])
        );
    }

    #[test]
    fn argv_never_weakens_host_keys() {
        const FORBIDDEN: [&str; 9] = [
            "stricthostkeychecking",
            "userknownhostsfile",
            "globalknownhostsfile",
            "proxycommand",
            "identityfile",
            "ssh_command",
            "directport",
            "passive",
            "sftp_server",
        ];
        let cfg = Path::new("/tmp/e2e/ssh_config");
        let mut argvs = vec![os(&SSH_OPTS)];
        for s in all_specs() {
            for c in [None, Some(cfg)] {
                argvs.push(preflight_argv(&s, c));
                for f in [Flavor::Linux, Flavor::MacFuse, Flavor::FuseT] {
                    argvs.push(sshfs_argv(&s, c, f));
                    for nfs in [false, true] {
                        let ssh = Path::new("/usr/bin/ssh");
                        let a = rclone_argv(&s, ssh, c, "writes", Path::new("/c"), nfs, f);
                        // B13 (S3-J): the real rclone argv, not the S0 vec![] stub
                        let real = a
                            .iter()
                            .any(|x| x.to_string_lossy().starts_with("--sftp-ssh="));
                        assert!(real, "{a:?}");
                        argvs.push(a);
                    }
                }
            }
        }
        for a in argvs {
            let joined = a
                .join(OsStr::new(" "))
                .to_string_lossy()
                .to_ascii_lowercase();
            for bad in FORBIDDEN {
                assert!(!joined.contains(bad), "{bad} in {joined}");
            }
        }
    }

    #[test]
    fn ssh_never_forwards_agent_x11_or_ports() {
        // sshfs passes -x -a -oClearAllForwardings=yes itself; the preflight and --sftp-ssh must too
        let cfg = Path::new("/tmp/e2e/ssh_config");
        for s in all_specs() {
            for c in [None, Some(cfg)] {
                let str = |a: Vec<OsString>| -> Vec<String> {
                    a.iter().map(|x| x.to_string_lossy().into_owned()).collect()
                };
                let mut argvs = vec![str(preflight_argv(&s, c))];
                for nfs in [false, true] {
                    let ssh = Path::new("/usr/bin/ssh");
                    let a = str(rclone_argv(
                        &s,
                        ssh,
                        c,
                        "writes",
                        Path::new("/c"),
                        nfs,
                        Flavor::Linux,
                    ));
                    let v = a
                        .iter()
                        .find_map(|x| x.strip_prefix("--sftp-ssh="))
                        .unwrap();
                    argvs.push(v.split(' ').map(String::from).collect());
                }
                for a in argvs {
                    for t in [
                        "-a",
                        "-x",
                        "ClearAllForwardings=yes",
                        "PermitLocalCommand=no",
                    ] {
                        assert!(a.iter().any(|x| x == t), "{t} missing: {a:?}");
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn failed_mount_leaves_no_mountpoint() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("failed-mount");
        let root = d.canonicalize().unwrap();
        let fake = |name: &str, body: &str| {
            let p = root.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let denied = fake(
            "denied",
            "echo 'Host key verification failed.' >&2; exit 255",
        );
        let ok = fake("ok", "exit 0");
        // ETXTBSY: a child forked by another test thread during a write holds the fd until it execs
        std::thread::sleep(Duration::from_millis(200));
        let mut s = static1();
        s.local_path = root.join("static1");
        let set = DriverSettings {
            ssh_config: None,
            vfs_cache_mode: "writes".into(),
            mount_timeout: Duration::from_secs(5),
            state_dir: root.clone(),
        };
        let bin = check::which("false").unwrap();
        // the preflight fails, then the child exits before mounting
        for ssh in [denied, ok] {
            let (r, _rx) = req(&s, &root);
            let e = mount_with("sshfs", &bin, &ssh, vec![], r, &set).await;
            assert!(matches!(e, Err(MountError::Failed(_))), "{e:?}");
            assert!(!s.local_path.exists(), "{ssh:?}");
        }
    }

    #[test]
    fn positionals_never_start_with_dash() {
        for s in all_specs() {
            for f in [Flavor::Linux, Flavor::MacFuse, Flavor::FuseT] {
                let a = sshfs_argv(&s, Some(Path::new("/x/cfg")), f);
                let [.., src, local] = &a[..] else {
                    panic!("short argv")
                };
                assert_eq!(src, &OsString::from(s.source()));
                assert_eq!(local, s.local_path.as_os_str());
                for p in [src, local] {
                    assert!(!p.to_string_lossy().starts_with('-'), "{p:?}");
                }
            }
            let a = preflight_argv(&s, None);
            let [.., dd, host, sftp] = &a[..] else {
                panic!("short argv")
            };
            assert_eq!((dd.to_str(), sftp.to_str()), (Some("--"), Some("sftp")));
            assert_eq!(host.to_str(), Some(s.host.as_str()));
            assert!(!host.to_string_lossy().starts_with('-'));
        }
    }

    #[test]
    fn adopt_marker_record_foreign_outside_root() {
        let root = Path::new("/r/machines");
        let h = |id: &str, driver: &str, fp: &str, pid| MountHandle {
            id: Name::parse(id).unwrap(),
            driver: driver.into(),
            local_path: root.join(id),
            fingerprint: fp.into(),
            pid,
        };
        let (fa, fb) = ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb");
        let records: BTreeMap<MountId, MountHandle> = [
            h("a", "sshfs", "1111111111111111", Some(42)), // same path as marker a: pid only
            h("c", "rclone-nfs", "cccccccccccccccc", Some(7)), // unmarked but recorded
            h("gone", "sshfs", "dddddddddddddddd", Some(9)), // not mounted: dropped
        ]
        .into_iter()
        .map(|r| (r.id.clone(), r))
        .collect();
        let entries = vec![
            entry(Path::new("/"), "ext4", "/dev/sda1"),
            entry(&root.join("a"), "fuse.sshfs", &format!("bifrost:a@{fa}")),
            entry(&root.join("b"), "fuse.rclone", &format!("bifrost:b@{fb}")),
            entry(&root.join("c"), "nfs", "localhost:/xyz"),
            entry(&root.join("d"), "fuse.sshfs", "bf@h:/foreign"), // foreign
            entry(&root.join("e"), "macfuse", &format!("bifrost:e@{fa}")), // unknown fstype → "sshfs"
            entry(&root.join("f"), "fuse.sshfs", &format!("bifrost:g@{fa}")), // marker id ≠ dir name
            entry(
                Path::new("/elsewhere/x"),
                "fuse.sshfs",
                &format!("bifrost:x@{fa}"),
            ),
            entry(
                &root.join("a/nested"),
                "fuse.sshfs",
                &format!("bifrost:nested@{fa}"),
            ),
            entry(root, "fuse.sshfs", &format!("bifrost:machines@{fa}")),
        ];
        assert_eq!(
            adopt(&entries, root, &records),
            vec![
                h("a", "sshfs", fa, Some(42)),
                h("b", "rclone", fb, None),
                h("c", "rclone-nfs", "cccccccccccccccc", Some(7)),
                h("e", "sshfs", fa, None),
            ]
            .into_iter()
            // an unmarked entry is ours only on macOS (NFS has no fsname marker); elsewhere it is foreign
            .filter(|x| cfg!(target_os = "macos") || x.id.as_str() != "c")
            .collect::<Vec<_>>()
        );
        // driver falls back to the record when the fstype says nothing
        let mut r2 = BTreeMap::new();
        r2.insert(Name::parse("e").unwrap(), h("e", "rclone", fb, Some(5)));
        assert_eq!(
            adopt(&entries[5..6], root, &r2),
            vec![h("e", "rclone", fa, Some(5))]
        );
        // a foreign overmount hides our marker entry underneath
        let stacked = [
            entries[1].clone(),
            entry(&root.join("a"), "ext4", "/dev/sdb"),
        ];
        assert_eq!(adopt(&stacked, root, &BTreeMap::new()), vec![]);
    }

    #[test]
    fn mount_step2_own_marker_adopt_or_detach_foreign_refused() {
        let s = static1();
        let p = &s.local_path;
        let ours = marker(&s.id, &s.fingerprint());
        let root = entry(Path::new("/"), "ext4", "/dev/sda1");
        // nothing there
        assert_eq!(step2(std::slice::from_ref(&root), &s), Step2::Free);
        // a leftover of a dropped/timed-out attempt with this very spec: adopt (A9)
        let same = entry(p, "fuse.sshfs", &ours);
        assert_eq!(step2(&[root.clone(), same.clone()], &s), Step2::Adopt);
        // our marker, older spec: lazy detach, then mount
        let old = entry(p, "fuse.sshfs", &marker(&s.id, "0000000000000000"));
        assert_eq!(step2(std::slice::from_ref(&old), &s), Step2::Detach);
        assert_eq!(step2(&[entry(p, "fuse.rclone", &ours)], &s), Step2::Adopt);
        // anything else is foreign
        let foreign = [
            entry(p, "fuse.sshfs", "bf@127.0.0.1:/home/bf"),
            entry(p, "ext4", "/dev/sdb1"),
            entry(
                p,
                "fuse.sshfs",
                &marker(&Name::parse("other").unwrap(), &s.fingerprint()),
            ),
            entry(p, "fuse.sshfs", &format!("{ours}x")),
        ];
        for f in foreign {
            let Step2::Refused(why) = step2(std::slice::from_ref(&f), &s) else {
                panic!("{f:?}")
            };
            assert!(why.contains(&f.fstype) && why.contains(&f.source), "{why}");
        }
        // the fstype subtype is chosen by whoever mounted it: cleaned like the source (§11 #7)
        let Step2::Refused(why) = step2(&[entry(p, "fuse.\u{1b}[31m", "x\u{9b}y")], &s) else {
            panic!("not refused")
        };
        assert!(!why.chars().any(|c| c.is_control()), "{why:?}");
        // only the topmost entry counts
        assert_eq!(
            step2(&[entry(p, "ext4", "/dev/sdb1"), same], &s),
            Step2::Adopt
        );
        assert!(matches!(
            step2(&[old, entry(p, "ext4", "x")], &s),
            Step2::Refused(_)
        ));
    }

    #[test]
    fn prepare_mountpoint_creates_rejects_symlink_file_nonempty() {
        use std::os::unix::fs::PermissionsExt;
        let d = tmpdir("prepare");
        let new = d.join("new");
        prepare_mountpoint(&new).unwrap();
        let m = std::fs::symlink_metadata(&new).unwrap();
        assert!(m.is_dir());
        assert_eq!(m.permissions().mode() & 0o777, 0o700);
        prepare_mountpoint(&new).unwrap(); // existing empty dir is fine

        std::os::unix::fs::symlink(&new, d.join("link")).unwrap();
        std::fs::write(d.join("file"), "x").unwrap();
        std::fs::create_dir(d.join("full")).unwrap();
        std::fs::write(d.join("full/.hidden"), "x").unwrap();
        for (p, why) in [
            ("link", "symlink"),
            ("file", "not a directory"),
            ("full", "not empty"),
        ] {
            match prepare_mountpoint(&d.join(p)) {
                Err(MountError::Refused(m)) => assert!(m.contains(why), "{p}: {m}"),
                r => panic!("{p}: {r:?}"),
            }
        }
        assert!(d.join("full/.hidden").exists());
    }

    #[test]
    fn busy_matches_the_errno_text_not_the_path() {
        assert!(busy(
            "fusermount3: failed to unmount /r/machines/x: Device or resource busy"
        ));
        assert!(busy(
            "umount(/r/machines/x): Resource busy -- try 'diskutil unmount'"
        ));
        assert!(!busy(
            "fusermount3: entry for /home/busybee/machines/busy-box not found in /etc/mtab"
        ));
        assert!(!busy(
            "fusermount3: failed to unmount /home/busybee/machines/x: Invalid argument"
        ));
    }

    #[tokio::test]
    async fn unmount_idempotent_when_absent() {
        let d = tmpdir("unmount");
        for force in [false, true] {
            assert_eq!(unmount_path(&d, force).await, Ok(()));
            assert_eq!(unmount_path(&d.join("never-existed"), force).await, Ok(()));
        }
        assert!(matches!(
            unmount_path(Path::new("rel/x"), true).await,
            Err(MountError::Refused(_))
        ));
    }
}
