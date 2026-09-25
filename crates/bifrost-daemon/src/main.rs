//! `bifrostd` (contract §8): startup, socket, lock, signals, graceful shutdown. The wiring (A3, A5) is final.

mod actor;
mod api;
mod reload;
mod state;

use actor::Deps;
use bifrost_config::paths;
use bifrost_config::{Config, ConfigError, ProviderConfig, ProviderSpec};
use bifrost_core::DiscoveryProvider;
use bifrost_discovery::{dns::DnsProvider, http::HttpProvider, tailscale::TailscaleProvider};
use bifrost_mount::DriverSettings;
use std::ffi::OsString;
use std::fs::{File, Permissions, TryLockError};
use std::io::{self, ErrorKind, IsTerminal};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::net::UnixListener;
use tracing::{error, info, warn};

fn build_provider(pc: &ProviderConfig) -> Result<Arc<dyn DiscoveryProvider>, String> {
    Ok(match &pc.spec {
        ProviderSpec::Tailscale => Arc::new(TailscaleProvider::new(pc.name.clone(), None)),
        ProviderSpec::Dns {
            domain,
            nameservers,
        } => Arc::new(DnsProvider::new(
            pc.name.clone(),
            domain.clone(),
            nameservers.clone(),
        )?),
        ProviderSpec::Http { url, headers } => Arc::new(HttpProvider::new(
            pc.name.clone(),
            url.clone(),
            headers
                .iter()
                .map(|(k, v)| (k.clone(), v.0.clone()))
                .collect(),
        )?),
    })
}

// ponytail: bifrostd and bifrost-discovery are never darwin-checked (reqwest pulls ring, whose C build needs an Apple toolchain; B9); upgrade: a macOS runner, or retry SDKROOT=/ CC_aarch64_apple_darwin=true AR_aarch64_apple_darwin=true cargo check --target aarch64-apple-darwin --workspace
fn main() {
    let usage = "usage: bifrostd [--version | --help]\n\
                 configured by BIFROST_CONFIG, BIFROST_SOCKET, BIFROST_STATE_DIR and BIFROST_LOG \
                 (error|warn|info|debug|trace)";
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match args.as_slice() {
        [] => {}
        [a] if a == "--version" => return println!("bifrostd {}", env!("CARGO_PKG_VERSION")),
        [a] if a == "--help" => return println!("{usage}"),
        _ => {
            eprintln!("{usage}");
            std::process::exit(2);
        }
    }
    // 1. logging: ANSI only on a terminal
    let level = std::env::var("BIFROST_LOG")
        .ok()
        .and_then(|l| l.parse().ok());
    tracing_subscriber::fmt()
        .with_max_level(level.unwrap_or(tracing::Level::INFO))
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
    let deps = Deps {
        drivers: Arc::new(|s: &DriverSettings| bifrost_mount::drivers(s)),
        build_provider: Arc::new(build_provider),
    };
    let o = Opts {
        config: paths::config_path(),
        state_dir: paths::state_dir(),
        socket: paths::socket_path(),
    };
    let code = match tokio::runtime::Runtime::new() {
        Ok(rt) => rt.block_on(run(o, deps, signal())),
        Err(e) => {
            eprintln!("bifrostd: runtime: {e}");
            1
        }
    };
    // exit without dropping the runtime: a probe thread stuck on a hung FUSE mount must not hold up the exit
    std::process::exit(code);
}

/// SIGTERM / SIGINT.
async fn signal() {
    use tokio::signal::unix::{SignalKind, signal};
    match signal(SignalKind::terminate()) {
        Ok(mut term) => {
            tokio::select! {
                _ = term.recv() => {}
                _ = tokio::signal::ctrl_c() => {}
            }
        }
        Err(e) => {
            warn!("no SIGTERM handler: {e}");
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    info!("shutting down; mounts stay and are adopted on the next start");
}

struct Opts {
    config: PathBuf,
    state_dir: PathBuf,
    socket: PathBuf,
}

/// Startup steps 2–10 (§8), serve until `stop`, then the graceful shutdown. Returns the exit code.
// ponytail: graceful shutdown doesn't unmount (stopping the daemon leaves mounts unsupervised until the restart adopts them); `bifrost unmount <all>` before stop if wanted
async fn run(o: Opts, deps: Deps, stop: impl Future<Output = ()> + Send + 'static) -> i32 {
    let fail = |code: i32, msg: String| {
        eprintln!("bifrostd: {msg}");
        code
    };
    // 2. state dir and <state>/logs (B11), 0700 when created (A21)
    let dirs =
        create_dirs(&o.state_dir).and_then(|e| Ok((e, create_dirs(&o.state_dir.join("logs"))?)));
    let state_existed = match dirs {
        Ok((e, _)) => e,
        Err(e) => return fail(1, format!("{}: {e}", o.state_dir.display())),
    };
    // 3. one daemon per state dir; then the ownership check
    let (lock, uid) = match lock(&o.state_dir) {
        Ok(l) => l,
        Err(e) => return fail(1, e),
    };
    if state_existed && owner(&o.state_dir) != Some(uid) {
        return fail(
            1,
            format!("{} is not owned by this user", o.state_dir.display()),
        );
    }
    // 4. config: invalid ⇒ exit 2, never a daemon that would unmount everything
    let (cfg, loaded) = match config(&o.config) {
        Ok(c) => c,
        Err(errs) => {
            errs.iter().for_each(|e| eprintln!("{e}"));
            return 2;
        }
    };
    // 5. the root: canonicalized once (hung-FUSE guard 5)
    let root = match create_dirs(&cfg.root).and_then(|_| cfg.root.canonicalize()) {
        Ok(r) => r,
        Err(e) => return fail(1, format!("mount.root {}: {e}", cfg.root.display())),
    };
    // 6–7. state.json, then adoption of our marker mounts directly under the root
    let st = state::read(&o.state_dir);
    let table = bifrost_mount::table::read().unwrap_or_else(|e| {
        warn!("mount table: {e}; adopting nothing");
        vec![]
    });
    let adopted = bifrost_mount::adopt(&table, &root, &st.mounts);
    // 9. the socket
    let listener = match bind_socket(&o.socket, uid) {
        Ok(l) => l,
        Err(e) => return fail(1, e),
    };
    // 8, 10. the actor (it builds and probes the drivers, applies the config, spawns providers and tickers)
    let (tx, snapshot, events, _actor) = actor::spawn(
        cfg,
        loaded,
        root,
        o.state_dir.clone(),
        o.socket.clone(),
        st.held,
        adopted,
        deps,
    );
    reload::spawn_poller(o.config.clone(), tx.clone());
    let app = api::AppState {
        snapshot,
        tx: tx.clone(),
        events,
        state_dir: o.state_dir.clone(),
        config_path: o.config.clone(),
    };
    info!(socket = %o.socket.display(), "bifrostd {} listening", env!("CARGO_PKG_VERSION"));
    let stopping = Arc::new(tokio::sync::Notify::new());
    let s = stopping.clone();
    let server = axum::serve(listener, api::router(app)).with_graceful_shutdown(async move {
        stop.await;
        s.notify_one();
    });
    // 1. stop the API; open SSE streams never end on their own, so the graceful shutdown gets 2s
    tokio::select! {
        r = server.into_future() => if let Err(e) = r { error!("api: {e}") },
        _ = async { stopping.notified().await; tokio::time::sleep(Duration::from_secs(2)).await } => {}
    }
    // 2. the actor aborts providers, waits ≤5s for executors, writes state.json. 3. remove the socket, exit 0
    let (reply, done) = tokio::sync::oneshot::channel();
    if tx.send(actor::Msg::Shutdown(reply)).is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(7), done).await;
    }
    let _ = std::fs::remove_file(&o.socket);
    drop(lock);
    0
}

/// Step 4: a missing file ⇒ the empty default, not loaded (A6); anything else goes through `load`.
// ponytail: a missing config runs the empty default (root ~/machines), so a typo in BIFROST_CONFIG silently runs with no machines (warned), and a file that appears later with another mount.root is rejected (A22) and needs a restart; an opt-in --require-config
fn config(path: &Path) -> Result<(Config, bool), Vec<ConfigError>> {
    if matches!(std::fs::metadata(path), Err(e) if e.kind() == ErrorKind::NotFound) {
        warn!(
            "no config at {}: running the empty default until it appears",
            path.display()
        );
        let env = |k: &str| std::env::var(k).ok();
        return bifrost_config::parse("", path, &env).map(|c| (c, false));
    }
    bifrost_config::load(path).map(|c| (c, true))
}

/// Creates `p` and its missing parents, mode 0700, and chmods ONLY the directories created here (A21): a
/// pre-existing one is never touched. Returns whether `p` already existed.
fn create_dirs(p: &Path) -> io::Result<bool> {
    let mut existed = true;
    let mut chain: Vec<&Path> = p
        .ancestors()
        .filter(|a| !a.as_os_str().is_empty())
        .collect();
    chain.reverse();
    for d in chain {
        existed = match std::fs::DirBuilder::new().mode(0o700).create(d) {
            Ok(()) => {
                std::fs::set_permissions(d, Permissions::from_mode(0o700))?; // whatever the umask did
                false
            }
            // macOS mkdir("/") is EISDIR, not EEXIST; std's create_dir_all uses the same is_dir fallback
            Err(e) if e.kind() == ErrorKind::AlreadyExists || d.is_dir() => true,
            Err(e) => return Err(e),
        };
    }
    Ok(existed)
}

fn owner(p: &Path) -> Option<u32> {
    std::fs::metadata(p).ok().map(|m| m.uid())
}

/// Step 3: one daemon per state dir; the lock goes when the process does (a crash included). Returns the lock
/// and its owner uid, the reference for the ownership checks (§8 Directories).
fn lock(state_dir: &Path) -> Result<(File, u32), String> {
    let p = state_dir.join("bifrostd.lock");
    let at = |e: io::Error| format!("{}: {e}", p.display());
    let f = (File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600))
    .open(&p)
    .map_err(at)?;
    match f.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => {
            return Err(format!("already running (lock {})", p.display()));
        }
        Err(TryLockError::Error(e)) => return Err(at(e)),
    }
    let m = f.metadata().map_err(at)?;
    // opened for writing and not group/other-writable ⇒ this user owns it, so it is a valid §8 uid reference
    // (a hostile pre-created 0666 lock would otherwise make an attacker's state dir pass the ownership check)
    if m.mode() & 0o022 != 0 {
        return Err(format!("{} is writable by group or others", p.display()));
    }
    Ok((f, m.uid()))
}

/// Step 9: the parent as in `create_dirs` (ownership-checked when it pre-existed), ≤103 bytes (sun_path), a stale
/// socket removed (only when nothing answers on it), bound, then 0600.
fn bind_socket(path: &Path, uid: u32) -> Result<UnixListener, String> {
    let n = path.as_os_str().len();
    if n > 103 {
        return Err(format!(
            "socket path is {n} bytes, over the 103-byte limit: {}",
            path.display()
        ));
    }
    let at = |e: io::Error| format!("{}: {e}", path.display());
    let parent = (path.parent())
        .filter(|p| path.is_absolute() && !p.as_os_str().is_empty())
        .ok_or_else(|| format!("socket path must be absolute: {}", path.display()))?;
    if create_dirs(parent).map_err(at)? && owner(parent) != Some(uid) {
        return Err(format!("{} is not owned by this user", parent.display()));
    }
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_socket() => {
            // the lock is per state dir, the socket per BIFROST_SOCKET: a live one belongs to another bifrostd
            if std::os::unix::net::UnixStream::connect(path).is_ok() {
                return Err(format!("{} is in use by another bifrostd", path.display()));
            }
            std::fs::remove_file(path).map_err(at)?
        }
        Ok(_) => return Err(format!("{} exists and is not a socket", path.display())),
        Err(e) if e.kind() == ErrorKind::NotFound => {}
        Err(e) => return Err(at(e)),
    }
    let l = UnixListener::bind(path).map_err(at)?;
    std::fs::set_permissions(path, Permissions::from_mode(0o600)).map_err(at)?;
    Ok(l)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actor::tests::{RECON, Rig, is, machine, rig};
    use bifrost_client::Client;
    use bifrost_core::api::StatusDto;
    use bifrost_core::reconcile::Availability;
    use tokio::sync::oneshot;

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn second_instance_lock_refused() {
        let r = rig("lock");
        let first = lock(&r.dir).unwrap();
        let e = lock(&r.dir).err().unwrap();
        assert!(e.contains("already running"), "{e}");
        drop(first);
        assert!(lock(&r.dir).is_ok());
        // a pre-created writable-by-others lock is no uid reference (§8): it could be anyone's
        std::fs::set_permissions(r.dir.join("bifrostd.lock"), Permissions::from_mode(0o666))
            .unwrap();
        assert!(lock(&r.dir).is_err());
    }

    #[tokio::test]
    async fn preexisting_parent_dirs_not_chmodded() {
        let r = rig("dirs");
        let (_lock, uid) = lock(&r.dir).unwrap();
        let home = r.dir.join("home");
        std::fs::create_dir(&home).unwrap();
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
        // BIFROST_SOCKET=$HOME/b.sock never chmods $HOME (A21); the socket itself is always 0600
        let _l = bind_socket(&home.join("b.sock"), uid).unwrap();
        assert_eq!((mode(&home), mode(&home.join("b.sock"))), (0o755, 0o600));
        // only the components created here become 0700
        let _l = bind_socket(&home.join("run/bifrost/b.sock"), uid).unwrap();
        assert_eq!(mode(&home), 0o755);
        assert_eq!(
            (mode(&home.join("run")), mode(&home.join("run/bifrost"))),
            (0o700, 0o700)
        );
        assert!(!create_dirs(&home.join("st/ate")).unwrap());
        assert_eq!(
            (mode(&home.join("st")), mode(&home.join("st/ate"))),
            (0o700, 0o700)
        );
        assert!(create_dirs(&home).unwrap());
        assert_eq!(mode(&home), 0o755);
        // a non-socket in the way is never removed
        std::fs::write(home.join("file"), "x").unwrap();
        assert!(bind_socket(&home.join("file"), uid).is_err());
        assert_eq!(std::fs::read_to_string(home.join("file")).unwrap(), "x");
        // nor is a live socket: the lock is per state dir, so it can be another bifrostd's
        let live = home.join("live.sock");
        let _other = std::os::unix::net::UnixListener::bind(&live).unwrap();
        assert!(bind_socket(&live, uid).err().unwrap().contains("in use"));
        assert!(live.exists());
        // sun_path limit
        let long = home.join("x".repeat(120));
        assert!(bind_socket(&long, uid).err().unwrap().contains("103"));
    }

    /// Runs the whole daemon (startup §8 steps 2–10) on the rig's config, state dir and `sock`.
    async fn daemon(
        r: &Rig,
        sock: &Path,
    ) -> (oneshot::Sender<()>, tokio::task::JoinHandle<i32>, StatusDto) {
        let config = r.dir.join("config.toml");
        std::fs::write(&config, r.text(&r.root, RECON, &machine("a"))).unwrap();
        let o = Opts {
            config,
            state_dir: r.dir.join("state"),
            socket: sock.to_path_buf(),
        };
        let (stop, stopped) = oneshot::channel::<()>();
        let d = tokio::spawn(run(o, r.deps(), async move {
            let _ = stopped.await;
        }));
        let c = Client::new(sock.to_path_buf());
        for _ in 0..100 {
            if let Ok(s) = c.get::<StatusDto>("/v1/status").await
                && is(&s, "a", Availability::Mounted)
            {
                return (stop, d, s);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("daemon never mounted a");
    }

    #[tokio::test]
    async fn socket_0600_stale_replaced() {
        let r = rig("sock");
        let sock = r.dir.join("run/b.sock");
        std::fs::create_dir(r.dir.join("run")).unwrap();
        // a crashed daemon's socket file: nothing listens on it
        drop(std::os::unix::net::UnixListener::bind(&sock).unwrap());
        let (stop, d, s) = daemon(&r, &sock).await;
        assert_eq!(mode(&sock), 0o600);
        assert_eq!(s.socket, sock.display().to_string());
        stop.send(()).unwrap();
        assert_eq!(d.await.unwrap(), 0);
        assert!(!sock.exists());
    }

    #[tokio::test]
    async fn shutdown_writes_state_and_leaves_mounts() {
        let r = rig("shutdown");
        let sock = r.dir.join("s.sock");
        let (stop, d, _) = daemon(&r, &sock).await;
        stop.send(()).unwrap();
        assert_eq!(d.await.unwrap(), 0);
        // no unmount (§8): the mount stays and its handle is recorded for adoption
        assert_eq!(r.calls(), ["mount a"]);
        let st = state::read(&r.dir.join("state"));
        let h = &st.mounts[&bifrost_core::Name::parse("a").unwrap()];
        assert_eq!(
            (h.driver.as_str(), h.local_path.clone()),
            ("sshfs", r.root.join("a"))
        );
        assert!(!sock.exists());
    }
}
