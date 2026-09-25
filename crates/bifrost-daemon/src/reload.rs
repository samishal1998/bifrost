//! Config file poller (contract §8 Config hot reload, B10): produces `Msg::Config` only; applying it is the actor's (A4).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::time::Duration;

use bifrost_config::ConfigError;
use tokio::sync::mpsc;
use tracing::{info, warn};

use crate::actor::Msg;

const EVERY: Duration = Duration::from_secs(2);

/// Polls `path` on a plain std::thread (B10: a config on NFS/sshfs must never hang a tokio worker) and sends a
/// `Msg::Config{reply: None}` for every settled change. Called inside the runtime, it also installs the SIGHUP
/// handler, which wakes the thread for an immediate reload that skips the debounce.
// ponytail: polls every 2s instead of notify, so an edit applies after 2–4s (SIGHUP and POST /v1/config/reload are instant); notify 8.x if latency matters
pub fn spawn_poller(path: PathBuf, tx: mpsc::UnboundedSender<Msg>) {
    let (kick, kicked) = channel();
    if tokio::runtime::Handle::try_current().is_ok() {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::hangup()) {
            Ok(mut hup) => drop(tokio::spawn(async move {
                while hup.recv().await.is_some() && kick.send(()).is_ok() {}
            })),
            Err(e) => warn!("no SIGHUP handler: {e}"),
        }
    }
    let poller = std::thread::Builder::new().name("config-poller".into());
    let spawned = poller.spawn(move || {
        let mut p = Poller::new(path, tx);
        while !p.tx.is_closed() {
            let hup = match kicked.recv_timeout(EVERY) {
                Ok(()) => true,
                Err(RecvTimeoutError::Timeout) => false,
                Err(RecvTimeoutError::Disconnected) => {
                    std::thread::sleep(EVERY); // no SIGHUP handler: poll only
                    false
                }
            };
            p.poll(hup);
        }
    });
    if let Err(e) = spawned {
        warn!("config poller not started: {e}; reload with SIGHUP or `bifrost config reload`");
    }
}

/// One `std::fs::read` (it follows symlinks) as comparable state: the bytes, or why the file can't be read.
type Read = Result<Vec<u8>, String>;

fn read(path: &Path) -> Read {
    std::fs::read(path).map_err(|e| e.to_string())
}

struct Poller {
    path: PathBuf,
    tx: mpsc::UnboundedSender<Msg>,
    /// the previous poll's read: a change counts once two polls in a row read the same (half-written saves)
    prev: Read,
    /// the read last acted on (sent, or warned about); the baseline is the first read
    acted: Read,
}

impl Poller {
    fn new(path: PathBuf, tx: mpsc::UnboundedSender<Msg>) -> Self {
        let r = read(&path);
        Self {
            path,
            tx,
            prev: r.clone(),
            acted: r,
        }
    }

    /// One read. It acts on a settled change once: sends the parsed config (bad bytes too, as Err), or warns
    /// that the file is gone and the active config stays. `hup` skips the debounce and acts even without a change.
    /// Returns whether it acted.
    fn poll(&mut self, hup: bool) -> bool {
        let cur = read(&self.path);
        let settled = cur == self.prev;
        self.prev = cur.clone();
        if !hup && (!settled || cur == self.acted) {
            return false;
        }
        self.acted = cur.clone();
        let path = &self.path;
        match cur {
            Err(e) => warn!("config {}: {e}; keeping the active config", path.display()),
            Ok(bytes) => {
                let why = if hup { "SIGHUP" } else { "changed" };
                info!("config {} {why}: reloading", path.display());
                let result = match std::str::from_utf8(&bytes) {
                    Ok(text) => bifrost_config::parse(text, path, &|k| std::env::var(k).ok()),
                    Err(e) => Err(vec![ConfigError {
                        path: path.display().to_string(),
                        message: format!("not UTF-8: {e}"),
                    }]),
                };
                let _ = self.tx.send(Msg::Config {
                    result: result.map(Box::new),
                    reply: None,
                });
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "[[machines]]\nname = \"box1\"\nhost = \"127.0.0.1\"\nremote = \"~\"\n";
    const B: &str = "[[machines]]\nname = \"box2\"\nhost = \"127.0.0.1\"\nremote = \"~\"\n";

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("bf-reload-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// config.toml = A in a fresh dir, and a poller whose baseline is it
    fn start(name: &str) -> (PathBuf, Poller, mpsc::UnboundedReceiver<Msg>) {
        let path = dir(name).join("config.toml");
        std::fs::write(&path, A).unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        (path.clone(), Poller::new(path, tx), rx)
    }

    /// Every `Msg::Config{reply: None}` sent so far: Ok(machine ids) or Err(error strings).
    fn sent(rx: &mut mpsc::UnboundedReceiver<Msg>) -> Vec<Result<Vec<String>, Vec<String>>> {
        std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| match m {
                Msg::Config {
                    result,
                    reply: None,
                } => result
                    .map(|c| c.machines.iter().map(|m| m.id.as_str().into()).collect())
                    .map_err(|e| e.iter().map(ToString::to_string).collect()),
                _ => panic!("not a Msg::Config{{reply: None}}"),
            })
            .collect()
    }

    fn ok(ids: &[&str]) -> Result<Vec<String>, Vec<String>> {
        Ok(ids.iter().map(|s| s.to_string()).collect())
    }

    #[test]
    fn poller_applies_after_two_identical_reads() {
        let (path, mut p, mut rx) = start("apply");
        assert!(!p.poll(false), "the baseline is not a change");
        std::fs::write(&path, B).unwrap();
        assert!(
            !p.poll(false),
            "first sight of new bytes waits for a second read"
        );
        assert!(p.poll(false));
        assert_eq!(sent(&mut rx), [ok(&["box2"])]);
        assert!(!p.poll(false) && !p.poll(false), "applied once");
        assert!(sent(&mut rx).is_empty());
    }

    #[test]
    fn poller_ignores_half_written_change() {
        let (path, mut p, mut rx) = start("half");
        std::fs::write(&path, &B[..B.len() / 2]).unwrap();
        assert!(!p.poll(false));
        std::fs::write(&path, B).unwrap(); // the save completes between two polls
        assert!(
            !p.poll(false),
            "bytes changed since the last poll: not settled"
        );
        assert!(
            sent(&mut rx).is_empty(),
            "the half-written file is never parsed"
        );
        assert!(p.poll(false));
        assert_eq!(sent(&mut rx), [ok(&["box2"])]);
    }

    #[test]
    fn poller_reports_bad_bytes_once() {
        let (path, mut p, mut rx) = start("bad");
        std::fs::write(&path, "[[machines]\nname = ").unwrap();
        assert!(!p.poll(false) && p.poll(false));
        let errs = sent(&mut rx);
        assert!(matches!(&errs[..], [Err(e)] if !e.is_empty()), "{errs:?}");
        assert!(
            (0..4).all(|_| !p.poll(false)),
            "the same bad bytes are not re-sent"
        );
        assert!(sent(&mut rx).is_empty());
        // restoring the old text is a change again (it clears the actor's config_errors)
        std::fs::write(&path, A).unwrap();
        assert!(!p.poll(false) && p.poll(false));
        assert_eq!(sent(&mut rx), [ok(&["box1"])]);
        // non-UTF-8 is an invalid config, never lossily applied
        std::fs::write(&path, b"[[machines]]\nname = \"b\xffx\"\n").unwrap();
        assert!(!p.poll(false) && p.poll(false));
        assert!(matches!(&sent(&mut rx)[..], [Err(e)] if !e.is_empty()));
    }

    #[test]
    fn poller_missing_file_keeps_active_and_warns_once() {
        let (path, mut p, mut rx) = start("missing");
        std::fs::remove_file(&path).unwrap();
        assert!(
            !p.poll(false),
            "a single missing read may be a rename-on-save"
        );
        assert!(p.poll(false), "warned");
        assert!((0..4).all(|_| !p.poll(false)), "warned once");
        assert!(
            sent(&mut rx).is_empty(),
            "nothing sent: the active config stays"
        );
        std::fs::write(&path, A).unwrap();
        assert!(!p.poll(false) && p.poll(false));
        assert_eq!(sent(&mut rx), [ok(&["box1"])]);
    }

    #[test]
    fn poller_follows_symlink_swap() {
        let d = dir("symlink");
        std::fs::write(d.join("a.toml"), A).unwrap();
        std::fs::write(d.join("b.toml"), B).unwrap();
        std::os::unix::fs::symlink("a.toml", d.join("config.toml")).unwrap();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let mut p = Poller::new(d.join("config.toml"), tx);
        // a dotfile manager's atomic swap: a new link renamed over the old one
        std::os::unix::fs::symlink("b.toml", d.join("config.tmp")).unwrap();
        std::fs::rename(d.join("config.tmp"), d.join("config.toml")).unwrap();
        assert!(!p.poll(false) && p.poll(false));
        assert_eq!(sent(&mut rx), [ok(&["box2"])]);
    }

    #[test]
    fn sighup_skips_the_debounce() {
        let (path, mut p, mut rx) = start("hup");
        std::fs::write(&path, B).unwrap();
        assert!(p.poll(true), "applied on the first read");
        assert_eq!(sent(&mut rx), [ok(&["box2"])]);
        assert!(
            !p.poll(false) && !p.poll(false),
            "the poll doesn't apply it again"
        );
        assert!(
            p.poll(true),
            "an explicit SIGHUP reloads unchanged bytes too"
        );
        assert_eq!(sent(&mut rx), [ok(&["box2"])]);
    }
}
