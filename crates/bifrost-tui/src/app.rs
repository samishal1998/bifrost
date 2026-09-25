//! Pure UI state: `App`, `App::on_key`, `Command` (contract §10). Written by S3-L.

use bifrost_core::api::{LogDto, StatusDto};
use bifrost_core::events::Event;
use bifrost_core::reconcile::Availability;
use bifrost_core::validate::clean;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Overview,
    Machines,
    Mounts,
    Discovery,
    Drivers,
    Events,
    Logs,
}

pub const VIEWS: [View; 7] = [
    View::Overview,
    View::Machines,
    View::Mounts,
    View::Discovery,
    View::Drivers,
    View::Events,
    View::Logs,
];

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Mount(String),
    Unmount(String, bool),
    Reconcile,
    Discover,
    Reload,
    FetchLog(String),
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Popup {
    Help,
    /// a machine id (Machines view) or a mount id (Mounts view)
    Details(String),
    /// force unmount of this target, asked y/n
    ConfirmForce(String),
    /// the filter being typed; Enter applies it, Esc clears it
    Filter(String),
}

pub struct App {
    pub status: Option<StatusDto>,
    pub view: View,
    /// index into `rows()`; the Logs view selects among the mounts
    pub selected: usize,
    pub filter: String,
    pub popup: Option<Popup>,
    pub status_line: String,
    /// the last poll failed; `status` is the last snapshot
    pub unreachable: bool,
    pub no_color: bool,
    pub log: Option<LogDto>,
    pub socket: String,
}

/// A daemon string, re-cleaned for the terminal.
pub fn c(s: &str) -> String {
    clean(s, 512)
}

pub fn state_name(a: Availability) -> &'static str {
    use Availability::*;
    match a {
        Unknown => "unknown",
        Discovered => "discovered",
        Eligible => "eligible",
        Mounted => "mounted",
        Connecting => "connecting",
        Unmounting => "unmounting",
        Offline => "offline",
        Degraded => "degraded",
        Failed => "failed",
    }
}

pub fn glyph(a: Availability) -> &'static str {
    use Availability::*;
    match a {
        Mounted => "●",
        Connecting | Unmounting => "◌",
        Degraded => "◐",
        Failed => "✕",
        Unknown | Discovered | Eligible | Offline => "○",
    }
}

/// 45s, 12m, 3h12m, 1d1h
pub fn dur(s: u64) -> String {
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86400 => format!("{}h{}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d{}h", s / 86400, s % 86400 / 3600),
    }
}

/// HH:MM:SS, UTC
pub fn hms(ts_unix_ms: u64) -> String {
    let s = ts_unix_ms / 1000 % 86400;
    format!("{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

pub fn event_text(e: &Event) -> String {
    use Event::*;
    let s = match e {
        MachineDiscovered { machine, provider } => format!("discovered {machine} via {provider}"),
        MachineLost { machine } => format!("lost {machine}"),
        MachineEligible { machine, via } => format!("eligible {machine} ({via})"),
        MountRequested { mount, driver } => format!("mount requested {mount} ({driver})"),
        MountStarted { mount, driver, pid } => match pid {
            Some(p) => format!("mount started {mount} ({driver}, pid {p})"),
            None => format!("mount started {mount} ({driver})"),
        },
        MountHealthy { mount } => format!("healthy {mount}"),
        MountDegraded { mount, reason } => format!("degraded {mount}: {reason}"),
        MountFailed {
            mount,
            error,
            attempt,
            retry_in_ms,
        } => match retry_in_ms {
            Some(ms) => format!(
                "failed {mount} (attempt {attempt}, retry in {}): {error}",
                dur(ms.div_ceil(1000))
            ),
            None => format!("failed {mount} (attempt {attempt}): {error}"),
        },
        UnmountStarted { mount, reason } => format!("unmounting {mount} ({reason})"),
        UnmountComplete { mount } => format!("unmounted {mount}"),
        DriverUnavailable { driver, reason } => format!("driver {driver} unavailable: {reason}"),
        ConfigurationReloaded { ok: true, .. } => "config reloaded".into(),
        ConfigurationReloaded { errors, .. } => {
            format!("config reload failed: {}", errors.join("; "))
        }
    };
    c(&s)
}

impl App {
    pub fn new(socket: &str, no_color: bool) -> Self {
        Self {
            status: None,
            view: View::Overview,
            selected: 0,
            filter: String::new(),
            popup: None,
            status_line: String::new(),
            unreachable: false,
            no_color,
            log: None,
            socket: c(socket),
        }
    }

    /// (id, cells) of the current view, filter applied. Every daemon string is re-cleaned here, so render never
    /// sees a raw one. Cell 0 of Machines/Mounts/Logs is the state glyph, of Discovery/Drivers a ✓/✗.
    pub fn rows(&self) -> Vec<(String, Vec<String>)> {
        let Some(s) = &self.status else {
            return vec![];
        };
        let ok = |b: bool| if b { "✓" } else { "✗" }.to_string();
        let rows: Vec<(String, Vec<String>)> = match self.view {
            View::Overview => vec![],
            View::Machines => (s.machines.iter())
                .map(|m| {
                    let mut drivers: Vec<String> = (m.mounts.iter())
                        .filter_map(|id| s.mounts.iter().find(|x| &x.id == id)?.driver.as_deref())
                        .map(c)
                        .collect();
                    drivers.sort();
                    drivers.dedup();
                    let cells = vec![
                        glyph(m.state).into(),
                        c(&m.id),
                        c(&m.source),
                        state_name(m.state).into(),
                        drivers.join(","),
                    ];
                    (m.id.clone(), cells)
                })
                .collect(),
            View::Mounts | View::Logs => (s.mounts.iter())
                .map(|m| {
                    let err = m.last_error.as_deref().unwrap_or(&m.detail); // B15
                    let cells = vec![
                        glyph(m.state).into(),
                        c(&m.id),
                        c(&m.machine),
                        c(m.driver.as_deref().unwrap_or("-")),
                        state_name(m.state).into(),
                        c(&m.local_path),
                        c(&m.remote),
                        c(err),
                    ];
                    (m.id.clone(), cells)
                })
                .collect(),
            View::Discovery => (s.providers.iter())
                .map(|p| {
                    let health = match &p.last_error {
                        None => "ok".into(),
                        Some(e) => format!("error: {}", c(e)),
                    };
                    let last_ok = p
                        .last_ok_secs_ago
                        .map_or("-".into(), |t| format!("{} ago", dur(t)));
                    let cells = vec![
                        ok(p.last_error.is_none()),
                        c(&p.name),
                        c(&p.kind),
                        health,
                        p.machines.to_string(),
                        last_ok,
                    ];
                    (p.name.clone(), cells)
                })
                .collect(),
            View::Drivers => (s.drivers.iter())
                .map(|d| {
                    let default = s.auto_driver.as_ref() == Some(&d.name); // E5
                    let cells = vec![
                        ok(d.available),
                        c(&d.name),
                        c(d.binary.as_deref().unwrap_or("-")),
                        c(&d.detail),
                        if default { "(default)" } else { "" }.into(),
                    ];
                    (d.name.clone(), cells)
                })
                .collect(),
            View::Events => (s.events.iter().rev())
                .map(|r| {
                    (
                        r.seq.to_string(),
                        vec![hms(r.ts_unix_ms), event_text(&r.event)],
                    )
                })
                .collect(),
        };
        let keep = |r: &(String, Vec<String>)| r.1.join(" ").contains(&self.filter);
        rows.into_iter().filter(keep).collect()
    }

    /// A poll result: None (unreachable, timed out, undecodable) keeps the last snapshot. The selection stays on
    /// the same id when rows are inserted or removed around it, so a key never acts on a row the user didn't pick.
    pub fn on_status(&mut self, s: Option<StatusDto>) {
        self.unreachable = s.is_none();
        if s.is_some() {
            let keep = self.rows().get(self.selected).map(|r| r.0.clone());
            self.status = s;
            let rows = self.rows();
            let at = keep.and_then(|id| rows.iter().position(|r| r.0 == id));
            self.selected = at
                .unwrap_or(self.selected)
                .min(rows.len().saturating_sub(1));
        }
    }

    pub fn on_key(&mut self, k: KeyEvent) -> Option<Command> {
        use KeyCode::*;
        if k.kind != KeyEventKind::Press {
            return None;
        }
        if k.code == Char('c') && k.modifiers.contains(KeyModifiers::CONTROL) {
            return Some(Command::Quit);
        }
        match self.popup.take() {
            None => {}
            Some(Popup::Filter(mut f)) => {
                match k.code {
                    Enter => self.set_filter(f),
                    Esc => self.set_filter(String::new()),
                    Backspace => {
                        f.pop();
                        self.popup = Some(Popup::Filter(f));
                    }
                    Char(ch) => {
                        f.push(ch);
                        self.popup = Some(Popup::Filter(f));
                    }
                    _ => self.popup = Some(Popup::Filter(f)),
                }
                return None;
            }
            // anything but y cancels
            Some(Popup::ConfirmForce(t)) => {
                return (k.code == Char('y')).then_some(Command::Unmount(t, true));
            }
            Some(p) => {
                if !matches!(k.code, Esc | Enter | Char('q' | 'd' | '?')) {
                    self.popup = Some(p);
                }
                return None;
            }
        }
        let n = self.rows().len();
        match k.code {
            Char('q') => Some(Command::Quit),
            Esc => {
                self.set_filter(String::new());
                None
            }
            Down | Char('j') => {
                self.selected = (self.selected + 1).min(n.saturating_sub(1));
                self.log_cmd()
            }
            Up | Char('k') => {
                self.selected = self.selected.saturating_sub(1);
                self.log_cmd()
            }
            Tab => self.goto(VIEWS[(self.view as usize + 1) % 7]),
            BackTab => self.goto(VIEWS[(self.view as usize + 6) % 7]),
            Char(d @ '1'..='7') => self.goto(VIEWS[d as usize - '1' as usize]),
            Char('r') => Some(Command::Reconcile),
            Char('s') => Some(Command::Discover),
            Char('c') => Some(Command::Reload),
            Char('?') => {
                self.popup = Some(Popup::Help);
                None
            }
            Char('/') => {
                self.popup = Some(Popup::Filter(self.filter.clone()));
                None
            }
            Char('m') => self.target().map(Command::Mount),
            Char('u') => self.target().map(|t| Command::Unmount(t, false)),
            Char('U') => {
                self.popup = self.target().map(Popup::ConfirmForce);
                None
            }
            Char('d') | Enter => {
                self.popup = self.target().map(Popup::Details);
                None
            }
            Char('l') => self.jump_to_log(),
            _ => None,
        }
    }

    fn set_filter(&mut self, f: String) {
        self.filter = f;
        self.selected = 0;
    }

    fn goto(&mut self, v: View) -> Option<Command> {
        self.view = v;
        self.selected = 0;
        self.log_cmd()
    }

    /// In the Logs view, fetch the selected mount's log now (the loop refetches it every 2s).
    pub fn log_cmd(&self) -> Option<Command> {
        if self.view != View::Logs {
            return None;
        }
        let id = self.rows().get(self.selected)?.0.clone();
        Some(Command::FetchLog(id))
    }

    /// A machine row means all its mounts (§10); the daemon resolves an id that is also a mount id to that mount
    /// alone (B15), so the TUI expands it.
    pub fn mounts_of(&self, t: &str) -> Vec<String> {
        let m = (self.status.as_ref())
            .filter(|_| self.view == View::Machines)
            .and_then(|s| s.machines.iter().find(|m| m.id == t));
        match m {
            Some(m) if !m.mounts.is_empty() => m.mounts.clone(),
            _ => vec![t.to_string()],
        }
    }

    /// The selected machine or mount id; anywhere else, a hint in the status line.
    fn target(&mut self) -> Option<String> {
        let t = matches!(self.view, View::Machines | View::Mounts)
            .then(|| self.rows().get(self.selected).map(|r| r.0.clone()))
            .flatten();
        if t.is_none() {
            self.status_line = "select a machine (view 2) or a mount (view 3) first".into();
        }
        t
    }

    /// `l`: the Logs view on the selected mount, or on the selected machine's first mount.
    fn jump_to_log(&mut self) -> Option<Command> {
        let t = self.target()?;
        let mount = match self.view {
            View::Machines => (self.status.iter())
                .flat_map(|s| &s.machines)
                .find(|m| m.id == t)
                .and_then(|m| m.mounts.first().cloned()),
            _ => Some(t.clone()),
        };
        let Some(mount) = mount else {
            self.status_line = format!("{} has no mounts", c(&t));
            return None;
        };
        self.view = View::Logs;
        self.filter.clear();
        self.selected = self.rows().iter().position(|r| r.0 == mount).unwrap_or(0);
        Some(Command::FetchLog(mount))
    }
}

#[cfg(test)]
pub fn fixture() -> StatusDto {
    use bifrost_core::api::{DriverDto, MachineDto, MountDto, ProviderDto};
    use bifrost_core::events::{Event, EventRecord};
    use bifrost_core::reconcile::Availability as A;
    let machine = |id: &str, source: &str, state| MachineDto {
        id: id.into(),
        name: id.into(),
        source: source.into(),
        shadowed: vec![],
        address: "192.0.2.10".into(),
        port: Some(22),
        online: Some(true),
        tags: vec!["agents".into()],
        metadata: Default::default(),
        verdict: "allowed (static)".into(),
        state,
        mounts: vec![id.into()],
    };
    let mount = |id: &str, driver: Option<&str>, state, err: Option<&str>| MountDto {
        id: id.into(),
        machine: id.into(),
        driver: driver.map(Into::into),
        local_path: format!("/home/u/machines/{id}"),
        remote: format!("u@{id}:/srv"),
        state,
        detail: String::new(),
        desired: true,
        held: false,
        adopted: false,
        pid: None,
        failures: 0,
        retry_in_secs: err.map(|_| 4),
        last_error: err.map(Into::into),
        action: "noop".into(),
    };
    let ev = |seq, event| EventRecord {
        seq,
        ts_unix_ms: 1_700_000_000_000 + seq * 1000,
        event,
    };
    StatusDto {
        version: "0.1.0".into(),
        pid: 4242,
        uptime_secs: 3720,
        socket: "/run/user/1000/bifrost/bifrost.sock".into(),
        config_path: "/home/u/.config/bifrost/config.toml".into(),
        config_errors: vec![],
        mount_root: "/home/u/machines".into(),
        ready: true,
        ssh_agent: true,
        providers: vec![ProviderDto {
            name: "tailscale".into(),
            kind: "tailscale".into(),
            machines: 1,
            refreshes: 3,
            last_ok_secs_ago: Some(12),
            last_error: None,
        }],
        drivers: vec![
            DriverDto {
                name: "sshfs".into(),
                available: true,
                binary: Some("/usr/bin/sshfs".into()),
                detail: "SSHFS version 3.7.3".into(),
            },
            DriverDto {
                name: "rclone".into(),
                available: false,
                binary: None,
                detail: "not found".into(),
            },
        ],
        auto_driver: Some("sshfs".into()),
        machines: vec![
            machine("agent-01", "tailscale", A::Mounted),
            MachineDto {
                mounts: vec!["build".into(), "build-artifacts".into()],
                ..machine("build", "static", A::Failed)
            },
        ],
        mounts: vec![
            mount("agent-01", Some("sshfs"), A::Mounted, None),
            mount("build", None, A::Failed, Some("connection refused")),
            MountDto {
                machine: "build".into(),
                ..mount("build-artifacts", None, A::Failed, None)
            },
        ],
        conflicts: vec![],
        events: vec![
            ev(
                1,
                Event::MachineDiscovered {
                    machine: "agent-01".into(),
                    provider: "tailscale".into(),
                },
            ),
            ev(
                2,
                Event::MountHealthy {
                    mount: "agent-01".into(),
                },
            ),
            ev(
                3,
                Event::MountFailed {
                    mount: "build".into(),
                    error: "connection refused".into(),
                    attempt: 1,
                    retry_in_ms: Some(4000),
                },
            ),
        ],
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use bifrost_core::api::MachineDto;
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};

    pub fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }
    pub fn ch(c: char) -> KeyEvent {
        key(KeyCode::Char(c))
    }
    pub fn app(view: View) -> App {
        let mut a = App::new("/run/x.sock", false);
        a.on_status(Some(fixture()));
        a.view = view;
        a
    }

    #[test]
    fn key_m_emits_mount_for_selection() {
        let mut a = app(View::Machines);
        assert_eq!(a.on_key(ch('m')), Some(Command::Mount("agent-01".into())));
        a.on_key(ch('j'));
        assert_eq!(a.on_key(ch('m')), Some(Command::Mount("build".into())));
        assert_eq!(
            a.on_key(ch('u')),
            Some(Command::Unmount("build".into(), false))
        );
        // clamped at the end
        a.on_key(key(KeyCode::Down));
        assert_eq!(a.selected, 1);
        // the Mounts view targets mount ids
        let mut a = app(View::Mounts);
        a.on_key(key(KeyCode::Down));
        assert_eq!(a.on_key(ch('m')), Some(Command::Mount("build".into())));
        // nothing to target outside Machines/Mounts: no command, a hint instead
        let mut a = app(View::Drivers);
        assert_eq!(a.on_key(ch('m')), None);
        assert!(!a.status_line.is_empty());
    }

    #[test]
    fn shift_u_asks_confirmation() {
        let mut a = app(View::Machines);
        let shift_u = KeyEvent::new(KeyCode::Char('U'), KeyModifiers::SHIFT);
        assert_eq!(a.on_key(shift_u), None);
        assert_eq!(a.popup, Some(Popup::ConfirmForce("agent-01".into())));
        assert_eq!(a.on_key(ch('n')), None);
        assert_eq!(a.popup, None);
        a.on_key(shift_u);
        assert_eq!(
            a.on_key(ch('y')),
            Some(Command::Unmount("agent-01".into(), true))
        );
        assert_eq!(a.popup, None);
    }

    #[test]
    fn filter_narrows_rows() {
        let mut a = app(View::Machines);
        assert_eq!(a.rows().len(), 2);
        for k in "/buiq".chars() {
            assert_eq!(a.on_key(ch(k)), None, "typing never runs a command");
        }
        a.on_key(key(KeyCode::Backspace));
        a.on_key(ch('l'));
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.filter, "buil");
        let ids: Vec<_> = a.rows().into_iter().map(|r| r.0).collect();
        assert_eq!(ids, ["build"]);
        // the selection is the filtered row
        assert_eq!(a.on_key(ch('m')), Some(Command::Mount("build".into())));
        a.on_key(key(KeyCode::Esc));
        assert_eq!((a.filter.as_str(), a.rows().len()), ("", 2));
        // Esc while typing drops the filter too
        for k in "/zzz".chars() {
            a.on_key(ch(k));
        }
        a.on_key(key(KeyCode::Esc));
        assert_eq!((a.filter.as_str(), a.rows().len()), ("", 2));
    }

    #[test]
    fn tab_cycles_views() {
        use View::*;
        let mut a = app(Overview);
        let mut seen = vec![a.view];
        for _ in 0..7 {
            a.on_key(key(KeyCode::Tab));
            seen.push(a.view);
        }
        assert_eq!(
            seen,
            [
                Overview, Machines, Mounts, Discovery, Drivers, Events, Logs, Overview
            ]
        );
        a.on_key(key(KeyCode::BackTab));
        assert_eq!(a.view, Logs);
        a.on_key(ch('3'));
        assert_eq!(a.view, Mounts);
        a.on_key(ch('1'));
        assert_eq!(a.view, Overview);
    }

    #[test]
    fn simple_keys() {
        let mut a = app(View::Overview);
        assert_eq!(a.on_key(ch('r')), Some(Command::Reconcile));
        assert_eq!(a.on_key(ch('s')), Some(Command::Discover));
        assert_eq!(a.on_key(ch('c')), Some(Command::Reload));
        assert_eq!(a.on_key(ch('q')), Some(Command::Quit));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(a.on_key(ctrl_c), Some(Command::Quit));
        a.on_key(ch('?'));
        assert_eq!(a.popup, Some(Popup::Help));
        assert_eq!(a.on_key(ch('r')), None, "a popup swallows keys");
        assert_eq!(a.popup, Some(Popup::Help));
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.popup, None);
    }

    #[test]
    fn details_and_logs_follow_selection() {
        let mut a = app(View::Machines);
        a.on_key(ch('j'));
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.popup, Some(Popup::Details("build".into())));
        a.on_key(key(KeyCode::Esc));
        // l on a machine jumps to its first mount's log
        assert_eq!(a.on_key(ch('l')), Some(Command::FetchLog("build".into())));
        assert_eq!(a.view, View::Logs);
        assert_eq!(a.rows()[a.selected].0, "build");
        // j/k in Logs switches mounts and fetches at once
        assert_eq!(
            a.on_key(ch('k')),
            Some(Command::FetchLog("agent-01".into()))
        );
    }

    #[test]
    fn machine_row_means_all_its_mounts() {
        let a = app(View::Machines);
        assert_eq!(a.mounts_of("build"), ["build", "build-artifacts"]);
        // the Mounts view targets the mount alone
        let a = app(View::Mounts);
        assert_eq!(a.mounts_of("build"), ["build"]);
    }

    #[test]
    fn selection_follows_id_across_snapshots() {
        let mut a = app(View::Machines);
        a.on_key(ch('j'));
        let mut s = fixture();
        let first = MachineDto {
            id: "aaa".into(),
            ..s.machines[0].clone()
        };
        s.machines.insert(0, first);
        a.on_status(Some(s));
        assert_eq!(a.on_key(ch('m')), Some(Command::Mount("build".into())));
    }

    #[test]
    fn unreachable_keeps_last_snapshot() {
        let mut a = app(View::Machines);
        a.on_status(None);
        assert!(a.unreachable);
        assert_eq!(a.rows().len(), 2);
        a.on_status(Some(fixture()));
        assert!(!a.unreachable);
    }
}
