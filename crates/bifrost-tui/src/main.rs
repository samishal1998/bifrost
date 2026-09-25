//! `bifrost-tui` (contract §10): a synchronous UI loop that talks only to the daemon API and polls it inline (E6).

mod app;
mod ui;

use app::{App, Command, c};
use bifrost_client::{Client, ClientError};
use bifrost_config::paths;
use bifrost_core::Name;
use bifrost_core::api::{ActionDto, LogDto, ReloadDto, StatusDto, UnmountReq};
use ratatui::crossterm::event::{self, Event};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::runtime::Runtime;
use tokio::time::timeout;

const USAGE: &str = "usage: bifrost-tui [--socket PATH]
  --socket PATH  daemon socket [default: $BIFROST_SOCKET, else $XDG_RUNTIME_DIR/bifrost/bifrost.sock]";

/// The routes return at once (§10); this only bounds a wedged daemon.
const CMD_TIMEOUT: Duration = Duration::from_secs(5);

fn main() {
    let socket = match socket_arg() {
        Ok(s) => s.unwrap_or_else(paths::socket_path),
        Err(e) => {
            eprintln!("bifrost-tui: {e}\n{USAGE}");
            std::process::exit(2);
        }
    };
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty());
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap_or_else(|e| fail(e));
    let client = Client::new(socket.clone());
    let mut app = App::new(&socket.display().to_string(), no_color);
    // installs a panic hook that restores the terminal first
    let mut term = ratatui::try_init().unwrap_or_else(|e| fail(e));
    let r = run(&mut term, &rt, &client, &mut app);
    ratatui::restore();
    if let Err(e) = r {
        fail(e);
    }
}

fn fail(e: impl std::fmt::Display) -> ! {
    eprintln!("bifrost-tui: {e}");
    std::process::exit(1);
}

/// `--socket PATH` | `--socket=PATH`; None = paths::socket_path() ($BIFROST_SOCKET, then the default).
fn socket_arg() -> Result<Option<PathBuf>, String> {
    let (mut args, mut socket) = (std::env::args_os().skip(1), None);
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--socket") => socket = Some(args.next().ok_or("--socket needs a path")?),
            Some(s) if s.starts_with("--socket=") => socket = Some(s["--socket=".len()..].into()),
            Some("-h" | "--help") => {
                println!("{USAGE}");
                std::process::exit(0);
            }
            _ => return Err(format!("unexpected argument {}", a.to_string_lossy())),
        }
    }
    Ok(socket.filter(|s| !s.is_empty()).map(PathBuf::from))
}

fn run(
    term: &mut ratatui::DefaultTerminal,
    rt: &Runtime,
    client: &Client,
    app: &mut App,
) -> std::io::Result<()> {
    let (mut status_at, mut log_at) = (Instant::now(), Instant::now());
    loop {
        let now = Instant::now();
        if now >= status_at {
            // ponytail: no SSE consumer, the TUI polls /v1/status every 1s inline (no poller task or channel, E6), so event latency is ≤1s; upgrade: Client::events() plus a `bifrost events` command
            let get = client.get::<StatusDto>("/v1/status");
            // timeout() builds its timer at once, so it must be created inside the runtime
            let r = rt.block_on(async { timeout(Duration::from_millis(500), get).await });
            app.on_status(r.ok().and_then(Result::ok));
            status_at = now + Duration::from_secs(1);
        }
        if now >= log_at {
            // only in the Logs view
            if let Some(cmd) = app.log_cmd() {
                exec(rt, client, app, cmd);
            }
            log_at = now + Duration::from_secs(2);
        }
        term.draw(|f| ui::render(app, f))?;
        if !event::poll(Duration::from_millis(100))? {
            continue;
        }
        let Event::Key(k) = event::read()? else {
            continue;
        };
        match app.on_key(k) {
            None => {}
            Some(Command::Quit) => return Ok(()),
            Some(cmd @ Command::FetchLog(_)) => {
                exec(rt, client, app, cmd);
                log_at = Instant::now() + Duration::from_secs(2);
            }
            Some(cmd) => {
                exec(rt, client, app, cmd);
                status_at = Instant::now(); // show the effect at once
            }
        }
    }
}

fn call<T>(rt: &Runtime, f: impl Future<Output = Result<T, ClientError>>) -> Result<T, String> {
    match rt.block_on(async { timeout(CMD_TIMEOUT, f).await }) {
        Ok(r) => r.map_err(|e| e.to_string()),
        Err(_) => Err(format!("timed out after {}s", CMD_TIMEOUT.as_secs())),
    }
}

/// Only a valid id reaches a route path.
fn id(s: &str) -> Result<String, String> {
    Name::parse(s)
        .map(|n| n.as_str().to_string())
        .map_err(|e| e.to_string())
}

/// Runs a command against the daemon; the result becomes the status line (a log becomes `app.log`).
fn exec(rt: &Runtime, client: &Client, app: &mut App, cmd: Command) {
    let empty = BTreeMap::<String, String>::new(); // `{}` for the body-less POSTs (C6)
    let line = match cmd {
        Command::Mount(t) => id(&t)
            .and_then(|t| {
                call(
                    rt,
                    client.post::<Vec<String>>(&format!("/v1/mounts/{t}/mount"), &empty),
                )
            })
            .map(|ids| format!("mount requested: {}", ids.join(", "))),
        Command::Unmount(t, force) => id(&t)
            .and_then(|t| {
                let path = format!("/v1/mounts/{t}/unmount");
                call(rt, client.post::<Vec<String>>(&path, &UnmountReq { force }))
            })
            .map(|ids| {
                let what = if force { "force unmount" } else { "unmount" };
                format!("{what} requested: {}", ids.join(", "))
            }),
        Command::Reconcile => {
            call(rt, client.post::<Vec<ActionDto>>("/v1/reconcile", &empty)).map(|actions| {
                let acts: Vec<String> = (actions.iter())
                    .filter(|a| a.action != "noop")
                    .map(|a| format!("{} {}", a.mount, a.action))
                    .collect();
                match acts.is_empty() {
                    true => "reconcile: nothing to do".into(),
                    false => format!("reconcile: {}", acts.join(", ")),
                }
            })
        }
        Command::Discover => call(
            rt,
            client.post::<BTreeMap<String, String>>("/v1/discover", &empty),
        )
        .map(|_| "discovery requested".into()),
        Command::Reload => {
            call(rt, client.post::<ReloadDto>("/v1/config/reload", &empty)).map(|r| match r.ok {
                true => "config reloaded".into(),
                false => format!("config reload failed: {}", r.errors.join("; ")),
            })
        }
        Command::FetchLog(m) => {
            let r =
                id(&m).and_then(|t| call(rt, client.get::<LogDto>(&format!("/v1/mounts/{t}/log"))));
            // an error (no log yet: 404) shows in the Logs pane, not every 2s in the status line
            app.log = Some(r.unwrap_or_else(|e| LogDto {
                mount: m,
                path: String::new(),
                lines: vec![e],
            }));
            return;
        }
        Command::Quit => return,
    };
    // every daemon string is re-cleaned
    app.status_line = c(&line.unwrap_or_else(|e| e));
}
