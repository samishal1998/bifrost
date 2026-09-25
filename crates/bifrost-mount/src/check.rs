//! Timed filesystem checks, binary lookup and helper commands.

use bifrost_core::validate::{random_u64, tail};
use bifrost_core::{MountError, MountSpec, MountState};
use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

/// Keys with a blocking call in flight: at most one leaked thread per hung mount.
static INFLIGHT: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

fn inflight() -> std::sync::MutexGuard<'static, BTreeSet<PathBuf>> {
    INFLIGHT.lock().unwrap_or_else(|e| e.into_inner())
}

/// Drops the key when the blocking call returns (or panics), not when the caller gives up.
struct Release(PathBuf);

impl Drop for Release {
    fn drop(&mut self) {
        inflight().remove(&self.0);
    }
}

/// None = timed out, or already in flight for `key` (no new thread is spawned).
pub async fn timed<T: Send + 'static>(
    key: &Path,
    dur: Duration,
    f: impl FnOnce() -> std::io::Result<T> + Send + 'static,
) -> Option<std::io::Result<T>> {
    if !inflight().insert(key.to_path_buf()) {
        return None;
    }
    let release = Release(key.to_path_buf());
    let task = tokio::task::spawn_blocking(move || {
        let _release = release;
        f()
    });
    match tokio::time::timeout(dur, task).await {
        Ok(Ok(r)) => Some(r),
        Ok(Err(e)) => Some(Err(std::io::Error::other(e.to_string()))), // the closure panicked
        Err(_) => None,
    }
}

/// symlink_metadata(path/".bifrost-probe-<16hex nonce>") under timed(5s):
/// any server reply — Ok, NotFound, PermissionDenied — → Healthy (A17: an unsearchable remote root is not a fault);
/// NotConnected (macOS also raw errno 6 ENXIO) → Stale; other → Degraded(e); None → Degraded("unresponsive").
/// `key` names the mount instance for the in-flight guard: a probe stuck on a lazily detached one
/// must not hold the path against the next mount there.
// ponytail: probe timeout fixed at 5s, slow links under load can read as Degraded (no action until grace); a key if false Degraded reports annoy
pub async fn liveness(path: &Path, key: &Path) -> MountState {
    // a unique name: neither the kernel nor sshfs can answer it from a cache
    let probe = path.join(format!(".bifrost-probe-{:016x}", random_u64()));
    match timed(key, Duration::from_secs(5), move || {
        std::fs::symlink_metadata(probe)
    })
    .await
    {
        None => MountState::Degraded("unresponsive".into()),
        Some(Ok(_)) => MountState::Healthy,
        Some(Err(e)) => match e.kind() {
            ErrorKind::NotFound | ErrorKind::PermissionDenied => MountState::Healthy,
            ErrorKind::NotConnected => MountState::Stale(e.to_string()),
            _ if cfg!(target_os = "macos") && e.raw_os_error() == Some(6) => {
                MountState::Stale(e.to_string())
            }
            _ => MountState::Degraded(e.to_string()),
        },
    }
}

/// Searches ONLY `path` (a PATH-style list); is_file ∧ mode & 0o111. The fixed fallback dirs live in `which()`.
/// Tests pass their own `path` and never call `set_var` (unsafe in edition 2024, racy across test threads) (B14).
/// Relative entries ("", ".") are skipped: spawned binaries are always absolute.
pub fn which_in(name: &str, path: &OsStr) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .filter(|d| d.is_absolute())
        .map(|d| d.join(name))
        .find(|p| {
            std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// $PATH + the fixed fallback dirs: a daemon started by launchd/systemd may have a minimal PATH.
pub(crate) fn search_path() -> OsString {
    let mut p = std::env::var_os("PATH").unwrap_or_default();
    p.push(":/usr/local/bin:/usr/bin:/bin");
    if cfg!(target_os = "macos") {
        p.push(":/opt/homebrew/bin");
    }
    p
}

/// which_in(name, $PATH + /usr/local/bin:/usr/bin:/bin [+ macos /opt/homebrew/bin])
pub fn which(name: &str) -> Option<PathBuf> {
    which_in(name, &search_path())
}

/// stdin null, kill_on_drop(true)
pub async fn run(
    bin: &Path,
    args: &[OsString],
    t: Duration,
) -> std::io::Result<std::process::Output> {
    let child = tokio::process::Command::new(bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    tokio::time::timeout(t, child.wait_with_output())
        .await
        .map_err(|_| std::io::Error::new(ErrorKind::TimedOut, format!("timed out after {t:?}")))?
}

/// exit 0 ⇒ host key + auth + sftp subsystem OK (verified against OpenSSH 9.6 → Alpine sshd).
/// The channel data comes from the peer: stdout → /dev/null, stderr read up to 64 KiB (not `run`, which buffers all).
pub async fn ssh_preflight(
    ssh: &Path,
    spec: &MountSpec,
    ssh_config: Option<&Path>,
) -> Result<(), MountError> {
    use tokio::io::AsyncReadExt;
    let fail = |e: &dyn std::fmt::Display| MountError::Failed(format!("ssh preflight: {e}"));
    let mut child = tokio::process::Command::new(ssh)
        .args(crate::preflight_argv(spec, ssh_config))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| fail(&e))?;
    let stderr = child.stderr.take();
    // the reader is dropped at the cap: a flooding peer gets EPIPE instead of our heap
    let read = async move {
        let mut buf = Vec::new();
        if let Some(e) = stderr {
            let _ = e.take(64 * 1024).read_to_end(&mut buf).await;
        }
        buf
    };
    let (buf, status) = tokio::time::timeout(Duration::from_secs(15), async {
        tokio::join!(read, child.wait())
    })
    .await
    .map_err(|_| fail(&"timed out"))?;
    let status = status.map_err(|e| fail(&e))?;
    if status.success() {
        return Ok(());
    }
    let why = tail(&String::from_utf8_lossy(&buf), 512);
    Err(if why.is_empty() {
        fail(&status)
    } else {
        MountError::Failed(why)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::tmpdir;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    #[test]
    fn which_respects_exec_bit() {
        let d = tmpdir("which");
        let mode = |p: &Path, m| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m));
        std::fs::write(d.join("plain"), "x").unwrap();
        mode(&d.join("plain"), 0o644).unwrap();
        std::fs::write(d.join("tool"), "x").unwrap();
        mode(&d.join("tool"), 0o755).unwrap();
        std::fs::create_dir(d.join("dir")).unwrap();
        let other = tmpdir("which-other");
        std::fs::write(other.join("plain"), "x").unwrap();
        mode(&other.join("plain"), 0o700).unwrap();

        let path = std::env::join_paths([&*d, &*other]).unwrap();
        assert_eq!(which_in("tool", &path), Some(d.join("tool")));
        assert_eq!(which_in("dir", &path), None);
        // the non-executable first hit is skipped, the executable one later in the list wins
        assert_eq!(which_in("plain", &path), Some(other.join("plain")));
        assert_eq!(which_in("tool", d.as_os_str()), Some(d.join("tool")));
        assert_eq!(which_in("tool", other.as_os_str()), None);
        // relative PATH entries ("", ".") never resolve: the spawn rule needs an absolute binary
        assert_eq!(which_in("tool", OsStr::new("")), None);
        assert!(which("sh").is_some_and(|p| p.is_absolute()));
    }

    #[tokio::test]
    async fn timed_guard_single_thread() {
        let key = tmpdir("timed");
        let done = Arc::new(AtomicBool::new(false));
        let d = done.clone();
        let slow = timed(&key, Duration::from_millis(50), move || {
            std::thread::sleep(Duration::from_millis(600));
            d.store(true, Ordering::SeqCst);
            Ok(1)
        });
        assert!(slow.await.is_none(), "times out");
        // the first closure is still sleeping: a second call returns at once and never runs its closure
        let ran = Arc::new(AtomicBool::new(false));
        let r = ran.clone();
        let t0 = Instant::now();
        let second = timed(&key, Duration::from_secs(5), move || {
            r.store(true, Ordering::SeqCst);
            Ok(2)
        })
        .await;
        assert!(second.is_none());
        assert!(t0.elapsed() < Duration::from_millis(40));
        assert!(!ran.load(Ordering::SeqCst));
        // another key is independent
        let other = timed(&key.join("x"), Duration::from_secs(5), || Ok(3)).await;
        assert_eq!(other.unwrap().unwrap(), 3);
        // once the stuck closure finishes, the key is free again
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(done.load(Ordering::SeqCst));
        let again = timed(&key, Duration::from_secs(5), || Ok(4)).await;
        assert_eq!(again.unwrap().unwrap(), 4);
    }

    #[tokio::test]
    async fn liveness_local_dir_healthy() {
        let d = tmpdir("live");
        assert_eq!(liveness(&d, &d).await, MountState::Healthy);
        let gone = tmpdir("live-gone").join("nope");
        assert_eq!(liveness(&gone, &gone).await, MountState::Healthy); // NotFound is a reply (A17)
    }

    #[tokio::test]
    async fn liveness_keyed_per_instance() {
        // a probe stuck on a lazily detached instance must not mark the next mount at that path unresponsive
        let d = tmpdir("live-inst");
        let old = d.join(".pid-1");
        let stuck = timed(&old, Duration::from_millis(50), || {
            std::thread::sleep(Duration::from_millis(600));
            Ok(())
        });
        assert!(stuck.await.is_none());
        assert_eq!(
            liveness(&d, &old).await,
            MountState::Degraded("unresponsive".into())
        );
        assert_eq!(liveness(&d, &d.join(".pid-2")).await, MountState::Healthy);
    }

    #[tokio::test]
    async fn preflight_stdout_ignored_stderr_capped() {
        let d = tmpdir("preflight");
        let s = crate::tests::static1();
        let fake = |name: &str, body: &str| {
            let p = d.join(name);
            std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        };
        let ok = fake("ok", "echo noise; exit 0");
        let denied = fake(
            "denied",
            "echo 'bf@h: Permission denied (publickey).' >&2; exit 255",
        );
        let flood = fake("flood", "exec yes flood >&2");
        // ETXTBSY: a child forked by another test thread during a write holds the fd until it execs
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(ssh_preflight(&ok, &s, None).await, Ok(()));
        assert_eq!(
            ssh_preflight(&denied, &s, None).await,
            Err(MountError::Failed(
                "bf@h: Permission denied (publickey).".into()
            ))
        );
        // a peer flooding stderr: cut at 64 KiB, the writer gets EPIPE, no 15s of buffering
        let t0 = Instant::now();
        let Err(MountError::Failed(why)) = ssh_preflight(&flood, &s, None).await else {
            panic!("flood passed")
        };
        assert!(t0.elapsed() < Duration::from_secs(10), "{:?}", t0.elapsed());
        assert!(why.len() <= 512 && why.starts_with("flood"), "{why}");
    }

    #[tokio::test]
    async fn run_times_out_and_captures() {
        let o = run(
            &which("echo").unwrap(),
            &["hi".into()],
            Duration::from_secs(5),
        )
        .await;
        assert_eq!(o.unwrap().stdout, b"hi\n");
        let e = run(
            &which("sleep").unwrap(),
            &["5".into()],
            Duration::from_millis(100),
        )
        .await;
        assert_eq!(e.unwrap_err().kind(), std::io::ErrorKind::TimedOut);
    }
}
