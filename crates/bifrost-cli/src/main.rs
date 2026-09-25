//! `bifrost` CLI (contract §9). S1-B: `config check`; S2-F adds the rest.

mod doctor;
mod output;

use bifrost_client::{Client, ClientError};
use bifrost_config::paths;
use bifrost_core::api::{
    ActionDto, MachineDto, MountDto, ProviderDto, ReloadDto, StatusDto, UnmountReq,
};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// `--json`: the DTO, pretty-printed.
macro_rules! pretty {
    ($v:expr) => {
        println!(
            "{}",
            serde_json::to_string_pretty(&$v).expect("DTOs always serialize")
        )
    };
}

#[derive(Parser)]
#[command(
    name = "bifrost",
    version,
    about = "Bifröst: remote machines as local folders"
)]
struct Cli {
    /// Config file [default: $BIFROST_CONFIG, else ~/.config/bifrost/config.toml]
    // no clap `env`: it rejects an empty value; paths::config_path() reads it, empty = unset
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    /// Daemon socket [default: $BIFROST_SOCKET, else $XDG_RUNTIME_DIR/bifrost/bifrost.sock; macOS ~/Library/Caches/bifrost/bifrost.sock]
    // no clap `env`, as for --config: paths::socket_path() reads it, empty = unset
    #[arg(long, global = true)]
    socket: Option<PathBuf>,
    /// Print JSON instead of text
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Daemon, config, machines, mounts, providers and drivers at a glance
    Status,
    /// Discovered machines (`machines show <id>` for one)
    Machines {
        #[command(subcommand)]
        cmd: Option<MachinesCmd>,
    },
    /// Mounts and their state
    Mounts,
    /// Mount a mount id, or every mount of a machine id, and wait for the result
    Mount {
        #[arg(value_parser = name)]
        target: String,
        /// Return once the daemon has accepted the request
        #[arg(long)]
        no_wait: bool,
    },
    /// Unmount and hold until `bifrost mount`
    Unmount {
        #[arg(value_parser = name)]
        target: String,
        /// Detach lazily even when busy (never kills a process)
        #[arg(long)]
        force: bool,
        /// Return once the daemon has accepted the request
        #[arg(long)]
        no_wait: bool,
    },
    /// Refresh every discovery provider now
    Discover,
    /// Run a reconcile pass now and print its plan
    Reconcile,
    /// Mount driver probes (probed here when the daemon is down)
    Drivers,
    /// Check config, daemon, ssh, discovery, drivers and FUSE
    Doctor,
    /// Configuration
    #[command(subcommand)]
    Config(ConfigCmd),
    /// The daemon
    #[command(subcommand)]
    Daemon(DaemonCmd),
}

#[derive(Subcommand)]
enum MachinesCmd {
    /// Every machine (the default)
    List,
    /// One machine's details
    Show {
        #[arg(value_parser = name)]
        id: String,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Validate a config file (no daemon needed)
    Check {
        /// defaults to --config
        path: Option<PathBuf>,
    },
    /// Make the daemon reload its config file now
    Reload,
}

#[derive(Subcommand)]
enum DaemonCmd {
    /// Is bifrostd running? (exit 3 when not)
    Status,
}

/// A mount or machine id: `Name` grammar, lowercased. The target becomes a URL path segment, so nothing
/// else ever reaches the socket (a bad one is a usage error, exit 2).
fn name(s: &str) -> Result<String, String> {
    bifrost_core::Name::parse(s)
        .map(|n| n.as_str().to_string())
        .map_err(|e| e.to_string())
}

fn main() {
    let cli = Cli::parse();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");
    std::process::exit(rt.block_on(run(cli)));
}

/// Exit codes (contract §9): 0 ok · 1 operation failed · 2 usage (clap) · 3 daemon not reachable.
async fn run(cli: Cli) -> i32 {
    if let Cmd::Config(ConfigCmd::Check { path }) = &cli.cmd {
        let path = path
            .clone()
            .or(cli.config)
            .unwrap_or_else(paths::config_path);
        return config_check(&path, cli.json);
    }
    let c = Client::new(cli.socket.clone().unwrap_or_else(paths::socket_path));
    match daemon(&cli, &c).await {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {}", output::c(&e.to_string()));
            // Io = the connection itself failed (e.g. EACCES on the socket): not reachable either
            match e {
                ClientError::NotRunning(_) | ClientError::Io(_) => 3,
                ClientError::Api { .. } | ClientError::Decode(_) => 1,
            }
        }
    }
}

async fn daemon(cli: &Cli, c: &Client) -> Result<i32, ClientError> {
    let json = cli.json;
    let empty = serde_json::json!({}); // every body-less POST sends `{}` (C6)
    match &cli.cmd {
        Cmd::Status => {
            let s: StatusDto = c.get("/v1/status").await?;
            if json {
                pretty!(s)
            } else {
                print!("{}", output::status(&s))
            }
        }
        Cmd::Machines {
            cmd: None | Some(MachinesCmd::List),
        } => {
            let ms: Vec<MachineDto> = c.get("/v1/machines").await?;
            if json {
                pretty!(ms)
            } else {
                print!("{}", output::machines(&ms))
            }
        }
        Cmd::Machines {
            cmd: Some(MachinesCmd::Show { id }),
        } => {
            // no /v1/machines/{id} (E1)
            let ms: Vec<MachineDto> = c.get("/v1/machines").await?;
            let Some(m) = ms.iter().find(|m| m.id == *id) else {
                eprintln!("error: unknown machine {id}");
                return Ok(1);
            };
            if json {
                pretty!(m)
            } else {
                print!("{}", output::machine(m))
            }
        }
        Cmd::Mounts => {
            let ms: Vec<MountDto> = c.get("/v1/mounts").await?;
            if json {
                pretty!(ms)
            } else {
                print!("{}", output::mounts(&ms))
            }
        }
        Cmd::Mount { target, no_wait } => {
            let ids: Vec<String> = c
                .post(&format!("/v1/mounts/{target}/mount"), &empty)
                .await?;
            if *no_wait {
                return Ok(requested(&ids, "mount", json));
            }
            return settle(c, &ids, json, output::mount_line).await;
        }
        Cmd::Unmount {
            target,
            force,
            no_wait,
        } => {
            let force = *force;
            let req = UnmountReq { force }; // always sent, force or not (C6)
            let ids: Vec<String> = c
                .post(&format!("/v1/mounts/{target}/unmount"), &req)
                .await?;
            if *no_wait {
                return Ok(requested(&ids, "unmount", json));
            }
            return settle(c, &ids, json, |id, m| output::unmount_line(id, m, force)).await;
        }
        Cmd::Discover => discover(c, json).await?,
        Cmd::Reconcile => {
            let a: Vec<ActionDto> = c.post("/v1/reconcile", &empty).await?;
            if json {
                pretty!(a)
            } else {
                print!("{}", output::actions(&a))
            }
        }
        Cmd::Drivers => {
            let (ds, auto) = match c.get::<StatusDto>("/v1/status").await {
                Ok(s) => (s.drivers, s.auto_driver),
                // not reachable, as run() counts it (Io: e.g. EACCES on the socket)
                Err(e @ (ClientError::NotRunning(_) | ClientError::Io(_))) => {
                    eprintln!("{}; probing locally", output::c(&e.to_string()));
                    let path = cli.config.clone().unwrap_or_else(paths::config_path);
                    let cfg = bifrost_config::load(&path).ok();
                    doctor::local_drivers(cfg.as_ref()).await
                }
                Err(e) => return Err(e),
            };
            if json {
                pretty!(ds)
            } else {
                print!("{}", output::drivers(&ds, auto.as_deref()))
            }
        }
        Cmd::Doctor => return Ok(doctor::run(c, cli.config.clone()).await),
        Cmd::Config(ConfigCmd::Reload) => {
            let r: ReloadDto = c.post("/v1/config/reload", &empty).await?;
            if json {
                pretty!(r)
            } else if r.ok {
                println!("reloaded")
            } else {
                r.errors.iter().for_each(|e| println!("{}", output::c(e)))
            }
            return Ok(i32::from(!r.ok));
        }
        Cmd::Config(ConfigCmd::Check { .. }) => {
            unreachable!("handled before the socket is resolved")
        }
        Cmd::Daemon(DaemonCmd::Status) => match c.get::<StatusDto>("/v1/status").await {
            Ok(s) if json => pretty!(s),
            Ok(s) => println!(
                "running pid {} up {} socket {}",
                s.pid,
                output::dur(s.uptime_secs),
                output::c(&s.socket)
            ),
            Err(ClientError::NotRunning(p)) => {
                println!("not running (socket {})", p.display());
                return Ok(3);
            }
            Err(e) => return Err(e),
        },
    }
    Ok(0)
}

/// `--no-wait`: the ids the daemon accepted.
fn requested(ids: &[String], what: &str, json: bool) -> i32 {
    if json {
        pretty!(ids)
    } else {
        ids.iter()
            .for_each(|id| println!("{}  {what} requested", output::c(id)))
    }
    0
}

/// Polls GET /v1/mounts every 250 ms, for up to 60 s, until `line` settles every id; prints one line per id.
/// The first poll waits 250 ms too, so it never reads the snapshot from before the POST.
async fn settle(
    c: &Client,
    ids: &[String],
    json: bool,
    line: impl Fn(&str, Option<&MountDto>) -> Option<(String, bool)>,
) -> Result<i32, ClientError> {
    let deadline = Instant::now() + Duration::from_secs(60);
    let find = |ms: &[MountDto], id: &str| ms.iter().find(|m| m.id == id).cloned();
    let ms = loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let ms: Vec<MountDto> = c.get("/v1/mounts").await?;
        let settled = ids
            .iter()
            .all(|id| line(id, find(&ms, id).as_ref()).is_some());
        if settled || Instant::now() >= deadline {
            break ms;
        }
    };
    let mine: Vec<Option<MountDto>> = ids.iter().map(|id| find(&ms, id)).collect();
    if json {
        pretty!(mine.iter().flatten().collect::<Vec<_>>())
    }
    let mut code = 0;
    for (id, m) in ids.iter().zip(&mine) {
        let (l, ok) = line(id, m.as_ref()).unwrap_or_else(|| {
            let state = m.as_ref().map_or("absent".into(), |m| output::st(m.state));
            (
                format!("{}  timed out after 60s ({state})", output::c(id)),
                false,
            )
        });
        if !json {
            println!("{l}")
        }
        if !ok {
            code = 1
        }
    }
    Ok(code)
}

/// POST discover (202 at once, E4), then poll status until every provider's `refreshes` increases (≤ 30 s).
async fn discover(c: &Client, json: bool) -> Result<(), ClientError> {
    // static has no task, and a provider whose build failed (B11) has none either: neither ever refreshes
    let waits =
        |p: &&ProviderDto| p.kind != "static" && !(p.refreshes == 0 && p.last_error.is_some());
    let before: StatusDto = c.get("/v1/status").await?;
    let _: serde_json::Value = c.post("/v1/discover", &serde_json::json!({})).await?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let s = loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        let s: StatusDto = c.get("/v1/status").await?;
        let stuck = |p: &&ProviderDto| {
            s.providers
                .iter()
                .any(|q| q.name == p.name && q.refreshes <= p.refreshes)
        };
        let pending: Vec<&str> = before
            .providers
            .iter()
            .filter(waits)
            .filter(stuck)
            .map(|p| p.name.as_str())
            .collect();
        if pending.is_empty() {
            break s;
        }
        if Instant::now() >= deadline {
            let pending = output::c(&pending.join(", "));
            eprintln!("warning: no refresh within 30s from: {pending}");
            break s;
        }
    };
    if json {
        pretty!(s.providers)
    } else {
        print!("{}", output::providers(&s.providers))
    }
    Ok(())
}

/// "ok: <path> (N machines, N providers, N mounts, root <root>)" or the sorted error lines; exit 1 on error.
/// `--json` prints a `ReloadDto` (the `config reload` shape).
fn config_check(path: &Path, json: bool) -> i32 {
    let result = bifrost_config::load(path);
    if json {
        let dto = bifrost_core::api::ReloadDto {
            ok: result.is_ok(),
            errors: result
                .as_ref()
                .err()
                .into_iter()
                .flatten()
                .map(|e| e.to_string())
                .collect(),
        };
        println!(
            "{}",
            serde_json::to_string_pretty(&dto).expect("plain struct")
        );
        return i32::from(!dto.ok);
    }
    match result {
        Ok(c) => {
            let mounts: usize = c.machines.iter().map(|m| m.mounts.len()).sum();
            println!(
                "ok: {} ({} machines, {} providers, {mounts} mounts, root {})",
                path.display(),
                c.machines.len(),
                c.providers.len(),
                bifrost_core::validate::clean(&c.root.display().to_string(), 512)
            );
            0
        }
        Err(errors) => {
            for e in errors {
                println!("{e}");
            }
            1
        }
    }
}
