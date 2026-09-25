//! Table and status-block rendering (contract §9). Written by S2-F.
//! Every daemon string goes through `clean(s, 512)` again here, at render time.

use bifrost_core::api::{ActionDto, DriverDto, MachineDto, MountDto, ProviderDto, StatusDto};
use bifrost_core::reconcile::Availability;
use bifrost_core::validate::clean;

/// A daemon string, re-cleaned for the terminal (contract §9).
pub fn c(s: &str) -> String {
    clean(s, 512)
}

/// The wire name (`--json` prints the same).
pub fn st(a: Availability) -> String {
    serde_json::to_value(a)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
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

/// Hand-aligned: each column is as wide as its widest cell, two spaces apart, no trailing spaces.
/// Every cell is re-cleaned.
pub fn table(rows: Vec<Vec<String>>) -> String {
    let rows: Vec<Vec<String>> = rows
        .into_iter()
        .map(|r| r.iter().map(|x| c(x)).collect())
        .collect();
    let mut w = Vec::new();
    for r in &rows {
        for (i, x) in r.iter().enumerate() {
            let n = x.chars().count();
            match w.get_mut(i) {
                Some(m) => *m = n.max(*m),
                None => w.push(n),
            }
        }
    }
    let mut out = String::new();
    for r in &rows {
        let line: String = r
            .iter()
            .zip(&w)
            .map(|(x, &n)| format!("{x:<n$}  "))
            .collect();
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

fn row(cells: &[&str]) -> Vec<String> {
    cells.iter().map(|s| s.to_string()).collect()
}

fn list(v: &[String]) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        v.join(", ")
    }
}

/// `NAME SOURCE ADDRESS STATE MOUNTED` (PRD §18). NAME is the id: what `mount`/`machines show` take.
pub fn machines(ms: &[MachineDto]) -> String {
    let mut rows = vec![row(&["NAME", "SOURCE", "ADDRESS", "STATE", "MOUNTED"])];
    for m in ms {
        // the machine state is the worst of its mounts; Degraded is still in the mount table
        let mounted = matches!(m.state, Availability::Mounted | Availability::Degraded);
        let mounted = if mounted { "yes" } else { "no" };
        rows.push(row(&[&m.id, &m.source, &m.address, &st(m.state), mounted]));
    }
    table(rows)
}

/// `machines show`: key/value lines.
pub fn machine(m: &MachineDto) -> String {
    let address = match m.port {
        Some(p) if m.address.contains(':') => format!("[{}]:{p}", m.address),
        Some(p) => format!("{}:{p}", m.address),
        None => m.address.clone(),
    };
    let online = match m.online {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    };
    let mut rows = vec![
        row(&["id", &m.id]),
        row(&["name", &m.name]),
        row(&["source", &m.source]),
        row(&["verdict", &m.verdict]),
        row(&["address", &address]),
        row(&["online", online]),
        row(&["state", &st(m.state)]),
        row(&["tags", &list(&m.tags)]),
    ];
    let meta: Vec<String> = m.metadata.iter().map(|(k, v)| format!("{k}={v}")).collect();
    if meta.is_empty() {
        rows.push(row(&["metadata", "-"]));
    }
    for (i, kv) in meta.iter().enumerate() {
        rows.push(row(&[if i == 0 { "metadata" } else { "" }, kv]));
    }
    rows.push(row(&["shadowed", &list(&m.shadowed)]));
    rows.push(row(&["mounts", &list(&m.mounts)]));
    table(rows)
}

/// last_error, else detail (B15: a driver-selection failure has only detail)
fn why(m: &MountDto) -> &str {
    m.last_error.as_deref().unwrap_or(&m.detail)
}

/// `ID MACHINE DRIVER STATE LOCAL REMOTE` (+ `ERROR` when any row has one)
pub fn mounts(ms: &[MountDto]) -> String {
    let err = ms.iter().any(|m| !why(m).is_empty());
    let mut head = row(&["ID", "MACHINE", "DRIVER", "STATE", "LOCAL", "REMOTE"]);
    if err {
        head.push("ERROR".into());
    }
    let mut rows = vec![head];
    for m in ms {
        let driver = m.driver.as_deref().unwrap_or("-");
        let state = st(m.state);
        let mut r = row(&[&m.id, &m.machine, driver, &state, &m.local_path, &m.remote]);
        if err {
            r.push(why(m).into());
        }
        rows.push(r);
    }
    table(rows)
}

/// The `bifrost status` block (contract §9).
pub fn status(s: &StatusDto) -> String {
    use Availability as A;
    let mut out = format!(
        "bifrostd {}  pid {}  up {}  {}{}\n",
        c(&s.version),
        s.pid,
        dur(s.uptime_secs),
        c(&s.socket),
        if s.ready { "" } else { "  (warming up)" }
    );
    if s.config_errors.is_empty() {
        out += &format!("config    {} (ok)\n", c(&s.config_path));
    } else {
        out += &format!(
            "config    {} (invalid; running the previous config)\n",
            c(&s.config_path)
        );
        for e in &s.config_errors {
            out += &format!("          {}\n", c(e));
        }
    }
    out += &format!("root      {}\n", c(&s.mount_root));
    let eligible = s
        .machines
        .iter()
        .filter(|m| m.verdict.starts_with("allowed"));
    let count = |a: A| s.mounts.iter().filter(|m| m.state == a).count();
    out += &format!(
        "machines  {} ({} eligible)   mounts {}/{} mounted, {} degraded, {} failed\n",
        s.machines.len(),
        eligible.count(),
        count(A::Mounted),
        s.mounts.len(),
        count(A::Degraded),
        count(A::Failed)
    );
    let providers: Vec<String> = s
        .providers
        .iter()
        .map(|p| {
            let (name, n) = (c(&p.name), p.machines);
            match (&p.last_error, p.last_ok_secs_ago) {
                (None, None) => format!("{name} ok ({n})"),
                (None, Some(t)) => format!("{name} ok ({n}, {} ago)", dur(t)),
                (Some(e), None) => format!("{name} error: {} (never ok)", c(e)),
                (Some(e), Some(t)) => format!("{name} error: {} (last ok {} ago)", c(e), dur(t)),
            }
        })
        .collect();
    let providers = if providers.is_empty() {
        "none".into()
    } else {
        providers.join(" · ")
    };
    out += &format!("providers {providers}\n");
    let drivers: Vec<String> = s
        .drivers
        .iter()
        .map(|d| match d.available {
            true if s.auto_driver.as_ref() == Some(&d.name) => {
                format!("{} ✓ (default)", c(&d.name))
            }
            true => format!("{} ✓", c(&d.name)),
            false => format!("{} ✗ {}", c(&d.name), c(&d.detail)),
        })
        .collect();
    out += &format!("drivers   {}\n", drivers.join(" · "));
    if !s.conflicts.is_empty() {
        let cs: Vec<String> = s.conflicts.iter().map(|x| c(x)).collect();
        out += &format!("conflicts {}\n", cs.join(" · "));
    }
    out
}

/// `discover`: `PROVIDER KIND STATUS MACHINES LAST-OK`
pub fn providers(ps: &[ProviderDto]) -> String {
    let mut rows = vec![row(&["PROVIDER", "KIND", "STATUS", "MACHINES", "LAST-OK"])];
    for p in ps {
        let status = p
            .last_error
            .as_ref()
            .map_or("ok".into(), |e| format!("error: {e}"));
        let ago = p
            .last_ok_secs_ago
            .map_or("-".into(), |t| format!("{} ago", dur(t)));
        rows.push(vec![
            p.name.clone(),
            p.kind.clone(),
            status,
            p.machines.to_string(),
            ago,
        ]);
    }
    table(rows)
}

/// `reconcile`: `MOUNT ACTION`
pub fn actions(a: &[ActionDto]) -> String {
    let mut rows = vec![row(&["MOUNT", "ACTION"])];
    rows.extend(a.iter().map(|x| row(&[&x.mount, &x.action])));
    table(rows)
}

/// `✓ sshfs  /usr/bin/sshfs  SSHFS version 3.7.3, fusermount3` … then `default (auto): <auto_driver | none>`
pub fn drivers(ds: &[DriverDto], auto: Option<&str>) -> String {
    let rows = ds
        .iter()
        .map(|d| {
            let mark = if d.available { "✓" } else { "✗" };
            let bin = d.binary.clone().unwrap_or_default();
            vec![format!("{mark} {}", d.name), bin, d.detail.clone()]
        })
        .collect();
    format!(
        "{}default (auto): {}\n",
        table(rows),
        c(auto.unwrap_or("none"))
    )
}

/// `mount`'s result once `m` settles: (line, ok). None = still settling.
pub fn mount_line(id: &str, m: Option<&MountDto>) -> Option<(String, bool)> {
    use Availability as A;
    let m = m?;
    let id = c(id);
    let (line, ok) = match m.state {
        // Unknown = an adopted mount whose machine isn't in the registry yet: it is mounted
        A::Mounted | A::Unknown => {
            let driver = m.driver.as_deref().unwrap_or("?");
            let pid = m.pid.map_or(String::new(), |p| format!(", pid {p}"));
            let local = c(&m.local_path);
            (format!("{id}  mounted  {local} ({}{pid})", c(driver)), true)
        }
        // mounted; sshfs reconnect is working on it
        A::Degraded => (format!("{id}  degraded: {}", c(why(m))), true),
        A::Failed => (format!("{id}  failed: {}", c(why(m))), false),
        A::Offline if why(m).is_empty() => (format!("{id}  offline"), false),
        A::Offline => (format!("{id}  offline: {}", c(why(m))), false),
        _ => return None,
    };
    Some((line, ok))
}

/// `unmount`'s result once `m` settles: (line, ok). None = still settling.
/// A Busy failure is final unless `force` (the forced retry waits out the unmount backoff).
pub fn unmount_line(id: &str, m: Option<&MountDto>, force: bool) -> Option<(String, bool)> {
    use Availability as A;
    let busy = bifrost_core::MountError::Busy.to_string(); // the one busy string (C4)
    let id = c(id);
    match m.map(|m| (m.state, m.last_error.as_deref())) {
        Some((A::Mounted | A::Degraded | A::Unknown, Some(e))) if e == busy && !force => {
            Some((format!("{id}  {busy}; retry with --force"), false))
        }
        // core stores it as "unmount failed: …" (unmount_done)
        Some((A::Mounted | A::Degraded | A::Unknown, Some(e))) if e != busy => {
            Some((format!("{id}  {}", c(e)), false))
        }
        Some((A::Mounted | A::Degraded | A::Unknown | A::Unmounting | A::Connecting, _)) => None,
        _ => Some((
            format!("{id}  unmounted (held; 'bifrost mount {id}' to resume)"),
            true,
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Availability::*;
    use bifrost_core::policy::Verdict;
    use bifrost_core::reconcile::{Action, Reason, WaitReason};
    use std::time::Duration;

    // core's Display strings are final (sign-off 7): fixtures are built from them, goldens stay literal
    fn allowed(by: &str) -> Verdict {
        Verdict::Allowed { by: by.into() }
    }

    fn machine_dto(
        id: &str,
        source: &str,
        address: &str,
        verdict: Verdict,
        state: Availability,
    ) -> MachineDto {
        MachineDto {
            id: id.into(),
            name: id.into(),
            source: source.into(),
            shadowed: vec![],
            address: address.into(),
            port: None,
            online: Some(true),
            tags: vec![],
            metadata: Default::default(),
            verdict: verdict.to_string(),
            state,
            mounts: vec![id.into()],
        }
    }

    fn mount_dto(id: &str, state: Availability) -> MountDto {
        MountDto {
            id: id.into(),
            machine: id.into(),
            driver: Some("sshfs".into()),
            local_path: format!("/home/sami/machines/{id}"),
            remote: format!("sami@{id}.tail1234.ts.net:/home/sami"),
            state,
            detail: String::new(),
            desired: true,
            held: false,
            adopted: false,
            pid: Some(4242),
            failures: 0,
            retry_in_secs: None,
            last_error: None,
            action: "noop".into(),
        }
    }

    fn provider(
        name: &str,
        kind: &str,
        machines: usize,
        ago: Option<u64>,
        err: Option<&str>,
    ) -> ProviderDto {
        ProviderDto {
            name: name.into(),
            kind: kind.into(),
            machines,
            refreshes: 3,
            last_ok_secs_ago: ago,
            last_error: err.map(Into::into),
        }
    }

    fn driver(name: &str, bin: Option<&str>, detail: &str) -> DriverDto {
        DriverDto {
            name: name.into(),
            available: bin.is_some(),
            binary: bin.map(Into::into),
            detail: detail.into(),
        }
    }

    #[test]
    fn machines_table_golden() {
        let ms = [
            machine_dto(
                "agent-01",
                "tailscale",
                "100.80.1.4",
                allowed("tailscale"),
                Mounted,
            ),
            machine_dto("agent-02", "dns", "10.0.20.8", allowed("dns"), Degraded),
            machine_dto("build", "static", "10.0.0.18", allowed("static"), Eligible),
            // a hostile daemon string is re-cleaned
            machine_dto(
                "evil",
                "http",
                "\x1b[31m10.9.9.9",
                Verdict::DiscoverOnly,
                Discovered,
            ),
        ];
        assert_eq!(
            machines(&ms),
            "NAME      SOURCE     ADDRESS        STATE       MOUNTED\n\
             agent-01  tailscale  100.80.1.4     mounted     yes\n\
             agent-02  dns        10.0.20.8      degraded    yes\n\
             build     static     10.0.0.18      eligible    no\n\
             evil      http       ?[31m10.9.9.9  discovered  no\n"
        );
        assert_eq!(machines(&[]), "NAME  SOURCE  ADDRESS  STATE  MOUNTED\n");

        let mut m = machine_dto(
            "agent-01",
            "tailscale",
            "fd7a::1",
            allowed("tailscale"),
            Mounted,
        );
        m.port = Some(2222);
        m.tags = vec!["agent".into(), "dev".into()];
        m.metadata = [
            ("os".into(), "linux".into()),
            ("zone".into(), "eu\u{202e}x".into()),
        ]
        .into();
        m.shadowed = vec!["dns".into()];
        assert_eq!(
            machine(&m),
            "id        agent-01\n\
             name      agent-01\n\
             source    tailscale\n\
             verdict   allowed (tailscale)\n\
             address   [fd7a::1]:2222\n\
             online    yes\n\
             state     mounted\n\
             tags      agent, dev\n\
             metadata  os=linux\n          zone=eu?x\n\
             shadowed  dns\n\
             mounts    agent-01\n"
        );
        m.online = None;
        m.tags.clear();
        m.metadata.clear();
        m.port = None;
        m.address = "10.0.0.1".into();
        let out = machine(&m);
        assert!(
            out.contains("address   10.0.0.1\nonline    unknown\n"),
            "{out}"
        );
        assert!(out.contains("tags      -\nmetadata  -\n"), "{out}");
    }

    #[test]
    fn mounts_table_golden() {
        let ok = [mount_dto("agent-01", Mounted)];
        assert_eq!(
            mounts(&ok),
            "ID        MACHINE   DRIVER  STATE    LOCAL                         REMOTE\n\
             agent-01  agent-01  sshfs   mounted  /home/sami/machines/agent-01  sami@agent-01.tail1234.ts.net:/home/sami\n"
        );

        // ERROR appears when any row has one: last_error, else detail (B15)
        let mut nodrv = mount_dto("web", Failed);
        nodrv.driver = None;
        nodrv.pid = None;
        nodrv.detail = "no available driver (tried sshfs, rclone)".into();
        let mut key = mount_dto("key", Failed);
        key.detail = "backoff 4s".into();
        key.last_error = Some("ssh preflight: Host key verification failed.\x1b]0;x".into());
        let ms = [mount_dto("a", Mounted), key, nodrv];
        assert_eq!(
            mounts(&ms),
            "ID   MACHINE  DRIVER  STATE    LOCAL                    REMOTE                               ERROR\n\
             a    a        sshfs   mounted  /home/sami/machines/a    sami@a.tail1234.ts.net:/home/sami\n\
             key  key      sshfs   failed   /home/sami/machines/key  sami@key.tail1234.ts.net:/home/sami  ssh preflight: Host key verification failed.?]0;x\n\
             web  web      -       failed   /home/sami/machines/web  sami@web.tail1234.ts.net:/home/sami  no available driver (tried sshfs, rclone)\n"
        );
    }

    fn status_fixture() -> StatusDto {
        let ok = |id: &str| machine_dto(id, "static", "10.0.0.1", allowed("static"), Mounted);
        let seen = |id: &str| {
            let v = Verdict::DiscoverOnly;
            machine_dto(id, "tailscale", "100.64.0.1", v, Discovered)
        };
        // denied is not eligible: pins status()'s starts_with("allowed") on core's Display
        let denied = Verdict::Denied {
            by: "policy.deny tags=misc".into(),
        };
        StatusDto {
            version: "0.1.0".into(),
            pid: 4242,
            uptime_secs: 3 * 3600 + 12 * 60 + 5,
            socket: "/run/user/1000/bifrost/bifrost.sock".into(),
            config_path: "/home/sami/.config/bifrost/config.toml".into(),
            config_errors: vec![],
            mount_root: "/home/sami/machines".into(),
            ready: true,
            ssh_agent: true,
            providers: vec![
                provider("static", "static", 2, None, None),
                provider("tailscale", "tailscale", 5, Some(12), None),
                provider("infra", "http", 0, Some(125), Some("timed out")),
            ],
            drivers: vec![
                driver(
                    "sshfs",
                    Some("/usr/bin/sshfs"),
                    "SSHFS version 3.7.3, fusermount3",
                ),
                driver("rclone", Some("/usr/bin/rclone"), "rclone v1.75.1"),
                driver("rclone-nfs", None, "macOS only"),
            ],
            auto_driver: Some("sshfs".into()),
            machines: vec![
                ok("a"),
                ok("b"),
                ok("c"),
                seen("d"),
                seen("e"),
                seen("f"),
                machine_dto("g", "dns", "10.0.0.7", denied, Discovered),
            ],
            mounts: vec![
                mount_dto("a", Mounted),
                mount_dto("b", Mounted),
                mount_dto("c", Mounted),
            ],
            conflicts: vec![],
            events: vec![],
        }
    }

    #[test]
    fn status_block_golden() {
        // the contract §9 example, byte for byte
        let mut s = status_fixture();
        assert_eq!(
            status(&s),
            "bifrostd 0.1.0  pid 4242  up 3h12m  /run/user/1000/bifrost/bifrost.sock\n\
             config    /home/sami/.config/bifrost/config.toml (ok)\n\
             root      /home/sami/machines\n\
             machines  7 (3 eligible)   mounts 3/3 mounted, 0 degraded, 0 failed\n\
             providers static ok (2) · tailscale ok (5, 12s ago) · infra error: timed out (last ok 2m ago)\n\
             drivers   sshfs ✓ (default) · rclone ✓ · rclone-nfs ✗ macOS only\n"
        );

        s.ready = false;
        s.config_errors = vec!["error: machines[0].host: bad\x07".into()];
        s.providers = vec![provider(
            "dns",
            "dns",
            0,
            None,
            Some("unavailable: not implemented"),
        )];
        s.mounts[1].state = Degraded;
        s.mounts[2].state = Failed;
        s.auto_driver = None;
        s.conflicts = vec!["x: local path collides with y".into()];
        s.uptime_secs = 59;
        assert_eq!(
            status(&s),
            "bifrostd 0.1.0  pid 4242  up 59s  /run/user/1000/bifrost/bifrost.sock  (warming up)\n\
             config    /home/sami/.config/bifrost/config.toml (invalid; running the previous config)\n\
             \x20         error: machines[0].host: bad?\n\
             root      /home/sami/machines\n\
             machines  7 (3 eligible)   mounts 1/3 mounted, 1 degraded, 1 failed\n\
             providers dns error: unavailable: not implemented (never ok)\n\
             drivers   sshfs ✓ · rclone ✓ · rclone-nfs ✗ macOS only\n\
             conflicts x: local path collides with y\n"
        );
        s.providers.clear();
        assert!(status(&s).contains("\nproviders none\n"));
    }

    #[test]
    fn small_tables_and_durations() {
        assert_eq!(
            [0, 59, 60, 3599, 3600, 11_520, 86_399, 90_000].map(dur),
            ["0s", "59s", "1m", "59m", "1h0m", "3h12m", "23h59m", "1d1h"]
        );
        let s = status_fixture();
        assert_eq!(
            providers(&s.providers),
            "PROVIDER   KIND       STATUS            MACHINES  LAST-OK\n\
             static     static     ok                2         -\n\
             tailscale  tailscale  ok                5         12s ago\n\
             infra      http       error: timed out  0         2m ago\n"
        );
        let a = [
            Action::NoOp,
            Action::Mount {
                driver: "sshfs".into(),
            },
            Action::Unmount {
                force: true,
                why: Reason::Manual,
            },
            Action::Remount {
                force: false,
                why: Reason::SpecChanged,
            },
            Action::Degraded("change pending: x".into()),
            Action::Waiting(WaitReason::Backoff(Duration::from_millis(2500))),
            Action::Waiting(WaitReason::WarmingUp),
        ]
        .map(|a| ActionDto {
            mount: "agent-01".into(),
            action: a.to_string(),
        });
        assert_eq!(
            actions(&a),
            "MOUNT     ACTION\n\
             agent-01  noop\n\
             agent-01  mount (sshfs)\n\
             agent-01  unmount (manual, force)\n\
             agent-01  remount (spec changed)\n\
             agent-01  degraded (change pending: x)\n\
             agent-01  waiting (backoff 3s)\n\
             agent-01  waiting (warming up)\n"
        );
        assert_eq!(
            drivers(&s.drivers, s.auto_driver.as_deref()),
            "✓ sshfs       /usr/bin/sshfs   SSHFS version 3.7.3, fusermount3\n\
             ✓ rclone      /usr/bin/rclone  rclone v1.75.1\n\
             ✗ rclone-nfs                   macOS only\n\
             default (auto): sshfs\n"
        );
        assert_eq!(drivers(&[], None), "default (auto): none\n");
        assert_eq!(st(Discovered), "discovered");
    }

    #[test]
    fn mount_and_unmount_lines() {
        let mut m = mount_dto("agent-01", Mounted);
        assert_eq!(
            mount_line("agent-01", Some(&m)),
            Some((
                "agent-01  mounted  /home/sami/machines/agent-01 (sshfs, pid 4242)".into(),
                true
            ))
        );
        m.pid = None;
        assert_eq!(
            mount_line("agent-01", Some(&m)).unwrap().0,
            "agent-01  mounted  /home/sami/machines/agent-01 (sshfs)"
        );
        for s in [Connecting, Eligible, Unmounting] {
            m.state = s;
            assert_eq!(mount_line("agent-01", Some(&m)), None, "{s:?}");
        }
        assert_eq!(mount_line("agent-01", None), None);
        // failed: last_error, else detail (a driver-selection failure has only detail, B15)
        m.state = Failed;
        m.detail = "no available driver (tried sshfs, rclone)".into();
        assert_eq!(
            mount_line("agent-01", Some(&m)),
            Some((
                "agent-01  failed: no available driver (tried sshfs, rclone)".into(),
                false
            ))
        );
        m.last_error = Some("ssh preflight: Host key verification failed.".into());
        assert_eq!(
            mount_line("agent-01", Some(&m)).unwrap().0,
            "agent-01  failed: ssh preflight: Host key verification failed."
        );
        m.state = Offline;
        assert!(!mount_line("agent-01", Some(&m)).unwrap().1);

        // unmount: gone or absent is done; busy is C4's one string (exit 1) unless --force keeps waiting
        let done = Some((
            "agent-01  unmounted (held; 'bifrost mount agent-01' to resume)".to_string(),
            true,
        ));
        assert_eq!(unmount_line("agent-01", None, false), done);
        let mut m = mount_dto("agent-01", Eligible);
        m.held = true;
        assert_eq!(unmount_line("agent-01", Some(&m), false), done);
        for s in [Mounted, Unmounting, Connecting, Degraded] {
            m.state = s;
            assert_eq!(unmount_line("agent-01", Some(&m), false), None, "{s:?}");
        }
        m.state = Mounted;
        m.last_error = Some("unmount blocked: busy (files open)".into());
        assert_eq!(
            unmount_line("agent-01", Some(&m), false),
            Some((
                "agent-01  unmount blocked: busy (files open); retry with --force".into(),
                false
            ))
        );
        assert_eq!(unmount_line("agent-01", Some(&m), true), None);
        m.state = Unmounting; // a retry in flight still carries the old error
        assert_eq!(unmount_line("agent-01", Some(&m), false), None);
        m.state = Degraded;
        // core stores every non-busy unmount error already prefixed (unmount_done)
        m.last_error = Some("unmount failed: driver unavailable: fusermount3 not found".into());
        assert_eq!(
            unmount_line("agent-01", Some(&m), true),
            Some((
                "agent-01  unmount failed: driver unavailable: fusermount3 not found".into(),
                false
            ))
        );
    }
}
