//! `bifrost config check` runs the built binary (contract §9, §12).

use std::path::Path;
use std::process::{Command, Output};

fn check(cfg: &Path, home: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_bifrost"))
        .args(["config", "check"])
        .arg(cfg)
        .env("HOME", home)
        .env_remove("BIFROST_CONFIG")
        .output()
        .unwrap()
}

#[test]
fn config_check_output_deterministic() {
    let dir = std::env::temp_dir().join(format!("bf-cli-config-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let bad = dir.join("bad.toml");
    std::fs::write(
        &bad,
        r#"
version = 2
[mount]
root = "/"
default_driver = "fuse"
[[discovery]]
type = "http"
url = "http://example.com/inv"
[[machines]]
name = "Zed"
host = "-oProxyCommand=touch /tmp/x"
remote = "rel"
[[machines]]
name = "a"
host = "a"
"#,
    )
    .unwrap();

    let (a, b) = (check(&bad, &dir), check(&bad, &dir));
    assert_eq!(a.status.code(), Some(1));
    assert_eq!(
        (&a.stdout, &a.stderr),
        (&b.stdout, &b.stderr),
        "byte-identical twice"
    );
    let out = String::from_utf8(a.stdout).unwrap();
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.len() >= 7, "{out}");
    assert!(lines.iter().all(|l| l.starts_with("error: ")), "{out}");
    let mut sorted = lines.clone();
    sorted.sort();
    assert_eq!(lines, sorted);
    assert!(
        out.contains("error: machines[0].name: \"Zed\": use lowercase"),
        "{out}"
    );

    // a TOML error is exactly one line with file:line:col
    let syntax = dir.join("syntax.toml");
    std::fs::write(&syntax, "[mount]\nroot = \"/x\"\npasword = 1\n").unwrap();
    let o = check(&syntax, &dir);
    assert_eq!(o.status.code(), Some(1));
    let out = String::from_utf8(o.stdout).unwrap();
    assert_eq!(out.lines().count(), 1, "{out}");
    assert!(
        out.starts_with(&format!("error: {}:3:1: ", syntax.display())),
        "{out}"
    );

    // ok line (§31 example)
    let good = dir.join("good.toml");
    std::fs::write(
        &good,
        "[mount]\nroot = \"~/machines\"\n\n[[machines]]\nname = \"agent-01\"\nhost = \"agent-01\"\nuser = \"sami\"\nremote = \"/home/sami\"\n",
    )
    .unwrap();
    let o = check(&good, &dir);
    assert_eq!(o.status.code(), Some(0));
    let want = format!(
        "ok: {} (1 machines, 0 providers, 1 mounts, root {})\n",
        good.display(),
        dir.join("machines").display()
    );
    assert_eq!(String::from_utf8(o.stdout).unwrap(), want);

    // an empty BIFROST_CONFIG means unset (like paths::config_path), not a clap usage error
    std::fs::create_dir_all(dir.join(".config/bifrost")).unwrap();
    std::fs::copy(&good, dir.join(".config/bifrost/config.toml")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_bifrost"))
        .args(["config", "check"])
        .env("HOME", &dir)
        .env("BIFROST_CONFIG", "")
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{o:?}");

    // a relative HOME never yields a cwd-relative default path
    std::fs::create_dir_all(dir.join("rel/.config/bifrost")).unwrap();
    std::fs::copy(&good, dir.join("rel/.config/bifrost/config.toml")).unwrap();
    let o = Command::new(env!("CARGO_BIN_EXE_bifrost"))
        .args(["config", "check"])
        .current_dir(&dir)
        .env("HOME", "rel")
        .env_remove("BIFROST_CONFIG")
        .output()
        .unwrap();
    assert!(!o.status.success(), "{o:?}");
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("HOME is not an absolute path"),
        "{o:?}"
    );

    // missing file is an error, not the default config
    let o = check(&dir.join("missing.toml"), &dir);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8(o.stdout).unwrap().contains("cannot read"));
}

// ---- daemon commands (S2-F): the binary against no daemon, or a std UnixListener stub ----

use bifrost_core::api::{DriverDto, MountDto, StatusDto};
use bifrost_core::reconcile::Availability;
use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::thread::JoinHandle;

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("bf-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// The binary with HOME = `home` and no BIFROST_* from the caller's environment.
fn bifrost(home: &Path) -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_bifrost"));
    c.env("HOME", home)
        .env_remove("BIFROST_SOCKET")
        .env_remove("BIFROST_CONFIG")
        .env_remove("BIFROST_STATE_DIR");
    c
}

/// Answers `n` HTTP/1.1 requests on `sock` with `reply(request line, body)`, then returns
/// every (request line, body) it saw.
fn stub(
    sock: &Path,
    n: usize,
    reply: impl Fn(&str, &str) -> (u16, String) + Send + 'static,
) -> JoinHandle<Vec<(String, String)>> {
    let l = UnixListener::bind(sock).unwrap();
    std::thread::spawn(move || {
        (0..n)
            .map(|_| {
                let (mut s, _) = l.accept().unwrap();
                let (mut buf, mut b) = (Vec::new(), [0u8; 4096]);
                let head = loop {
                    if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break i + 4;
                    }
                    let k = s.read(&mut b).unwrap();
                    assert!(k > 0, "eof in headers");
                    buf.extend_from_slice(&b[..k]);
                };
                let text = String::from_utf8_lossy(&buf[..head]).to_string();
                let len = text
                    .lines()
                    .find_map(|l| {
                        let l = l.to_ascii_lowercase();
                        Some(l.strip_prefix("content-length:")?.trim().parse().unwrap())
                    })
                    .unwrap_or(0);
                while buf.len() < head + len {
                    let k = s.read(&mut b).unwrap();
                    assert!(k > 0, "eof in body");
                    buf.extend_from_slice(&b[..k]);
                }
                let body = String::from_utf8(buf[head..head + len].to_vec()).unwrap();
                let line = text.lines().next().unwrap().to_string();
                let (code, resp) = reply(&line, &body);
                write!(
                    s,
                    "HTTP/1.1 {code} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{resp}",
                    resp.len()
                )
                .unwrap();
                (line, body)
            })
            .collect()
    })
}

fn mount_dto(id: &str, state: Availability) -> MountDto {
    MountDto {
        id: id.into(),
        machine: id.into(),
        driver: Some("sshfs".into()),
        local_path: format!("/r/{id}"),
        remote: format!("bf@127.0.0.1:/home/{id}"),
        state,
        detail: String::new(),
        desired: true,
        held: false,
        adopted: false,
        pid: Some(7),
        failures: 0,
        retry_in_secs: None,
        last_error: None,
        action: "noop".into(),
    }
}

fn status_dto() -> StatusDto {
    StatusDto {
        version: "0.1.0".into(),
        pid: 4242,
        uptime_secs: 5,
        socket: "/x.sock".into(),
        config_path: "/nowhere/config.toml".into(),
        config_errors: vec![],
        mount_root: "/r".into(),
        ready: true,
        ssh_agent: true,
        providers: vec![],
        drivers: vec![DriverDto {
            name: "sshfs".into(),
            available: true,
            binary: Some("/usr/bin/sshfs".into()),
            detail: "SSHFS version 3.7.3, fusermount3".into(),
        }],
        auto_driver: Some("sshfs".into()),
        machines: vec![],
        mounts: vec![],
        conflicts: vec![],
        events: vec![],
    }
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn exit3_when_daemon_absent() {
    let dir = tmp("exit3");
    let nope = dir.join("nope.sock");
    let cmds: [&[&str]; 6] = [
        &["status"],
        &["machines"],
        &["machines", "show", "a"],
        &["mounts"],
        &["reconcile"],
        &["daemon", "status"],
    ];
    for args in cmds {
        let o = bifrost(&dir)
            .arg("--socket")
            .arg(&nope)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(3), "{args:?} {o:?}");
    }
    let o = bifrost(&dir)
        .arg("--socket")
        .arg(&nope)
        .arg("status")
        .output()
        .unwrap();
    let err = String::from_utf8(o.stderr).unwrap();
    assert_eq!(
        err,
        format!(
            "error: bifrostd is not running (socket {})\n",
            nope.display()
        )
    );

    // BIFROST_SOCKET works like --socket; `daemon status` says it on stdout
    let o = bifrost(&dir)
        .env("BIFROST_SOCKET", &nope)
        .args(["daemon", "status"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(3), "{o:?}");
    assert_eq!(
        stdout(&o),
        format!("not running (socket {})\n", nope.display())
    );

    // usage errors are 2, before any socket is touched
    for args in [&["mount"][..], &["mount", "../x"], &["bogus"]] {
        let o = bifrost(&dir)
            .arg("--socket")
            .arg(&nope)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{args:?} {o:?}");
    }
}

#[test]
fn unmount_sends_force_body() {
    let dir = tmp("unmount");
    let sock = dir.join("s.sock");
    let t = stub(&sock, 3, |line, _| {
        if line.starts_with("GET /v1/mounts ") {
            let mut m = mount_dto("agent-01", Availability::Eligible);
            m.held = true;
            (200, serde_json::to_string(&[m]).unwrap())
        } else {
            (202, r#"["agent-01"]"#.into())
        }
    });
    let run = |args: &[&str]| {
        bifrost(&dir)
            .arg("--socket")
            .arg(&sock)
            .args(args)
            .output()
            .unwrap()
    };
    let o = run(&["unmount", "agent-01", "--force", "--no-wait"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    // an upper-case target is the same Name; without --no-wait it polls GET mounts
    let o = run(&["unmount", "AGENT-01"]);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert_eq!(
        stdout(&o),
        "agent-01  unmounted (held; 'bifrost mount agent-01' to resume)\n"
    );
    let seen = t.join().unwrap();
    let want = [
        (
            "POST /v1/mounts/agent-01/unmount HTTP/1.1",
            r#"{"force":true}"#,
        ),
        (
            "POST /v1/mounts/agent-01/unmount HTTP/1.1",
            r#"{"force":false}"#,
        ),
        ("GET /v1/mounts HTTP/1.1", ""),
    ];
    let seen: Vec<(&str, &str)> = seen.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    assert_eq!(seen, want);
}

#[test]
fn json_flag_prints_dto() {
    let dir = tmp("json");
    let sock = dir.join("s.sock");
    let mounts = vec![mount_dto("a", Availability::Mounted)];
    let body = serde_json::to_string(&mounts).unwrap();
    let t = stub(&sock, 2, move |line, _| {
        if line.starts_with("GET /v1/mounts ") {
            (200, body.clone())
        } else {
            (200, r#"[{"mount":"a","action":"noop"}]"#.into())
        }
    });
    let o = bifrost(&dir)
        .arg("--socket")
        .arg(&sock)
        .args(["--json", "mounts"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let out = stdout(&o);
    assert!(out.lines().count() > 3, "pretty-printed: {out}");
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v, serde_json::to_value(&mounts).unwrap());

    // --json is global (after the subcommand too); a body-less POST sends `{}` (C6)
    let o = bifrost(&dir)
        .arg("--socket")
        .arg(&sock)
        .args(["reconcile", "--json"])
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let v: serde_json::Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v, serde_json::json!([{"mount": "a", "action": "noop"}]));
    let seen = t.join().unwrap();
    assert_eq!(seen[1], ("POST /v1/reconcile HTTP/1.1".into(), "{}".into()));
}

#[test]
fn drivers_falls_back_to_local_probe_when_daemon_down() {
    let dir = tmp("drivers");
    let run = |json: bool| {
        let mut c = bifrost(&dir);
        c.arg("--socket").arg(dir.join("nope.sock"));
        c.arg("--config").arg(dir.join("missing.toml"));
        if json {
            c.arg("--json");
        }
        c.arg("drivers").output().unwrap()
    };
    let o = run(true);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let ds: Vec<DriverDto> = serde_json::from_slice(&o.stdout).unwrap();
    let names: Vec<&str> = ds.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["sshfs", "rclone", "rclone-nfs"]);
    assert!(!ds[2].available || cfg!(target_os = "macos"));

    let o = run(false);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines.len(), 4, "{out}");
    assert!(
        lines[2].starts_with("✗ rclone-nfs") || cfg!(target_os = "macos"),
        "{out}"
    );
    // the local default is the first available driver of default_auto_order() (no config file)
    let want = if lines[0].starts_with("✓ sshfs") {
        "sshfs"
    } else {
        "none"
    };
    assert_eq!(lines[3], format!("default (auto): {want}"), "{out}");
    assert!(
        String::from_utf8_lossy(&o.stderr).contains("not running"),
        "{o:?}"
    );
}

#[test]
fn doctor_config_drivers_and_macos_hint() {
    let dir = tmp("doctor");
    let cfg = dir.join("config.toml");
    std::fs::write(&cfg, "[mount]\nroot = \"/r\"\n").unwrap();
    let sock = dir.join("s.sock");
    let t = stub(&sock, 1, |_, _| {
        let mut s = status_dto();
        s.mounts = vec![mount_dto("a", Availability::Mounted)];
        (200, serde_json::to_string(&s).unwrap())
    });
    let doctor = |extra: &[&str]| {
        bifrost(&dir)
            .arg("--socket")
            .arg(&sock)
            .arg("--config")
            .arg(&cfg)
            .arg("doctor")
            .args(extra)
            .output()
            .unwrap()
    };
    let o = doctor(&[]);
    let out = stdout(&o);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(
        out.starts_with(&format!("Config         ✓ {}\n", cfg.display())),
        "{out}"
    );
    assert!(out.contains("Daemon         ✓ running pid 4242\n"), "{out}");
    assert!(out.contains("\nSelected default\n  sshfs\n"), "{out}");
    t.join().unwrap();

    // B7: a mount's macOS permission hint is repeated under Mount Drivers and fails doctor
    let hint = "mount failed: not permitted (macOS: allow the macFUSE system extension in \
                System Settings → Privacy & Security)";
    let t = stub(&sock_again(&sock), 1, move |_, _| {
        let mut s = status_dto();
        let mut m = mount_dto("a", Availability::Failed);
        m.last_error = Some(hint.into());
        s.mounts = vec![m];
        (200, serde_json::to_string(&s).unwrap())
    });
    let o = doctor(&[]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let out = stdout(&o);
    let drivers = out.split("Mount Drivers\n").nth(1).unwrap();
    let row = drivers.lines().find(|l| l.starts_with("  ✗ a ")).unwrap();
    assert!(row.ends_with(&format!(" {hint}")), "{out}");
    t.join().unwrap();

    // a bad config is a ✗ and exit 1, daemon or not
    std::fs::write(&cfg, "[mount]\nrot = 1\n").unwrap();
    let _ = std::fs::remove_file(&sock);
    let o = doctor(&[]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let out = stdout(&o);
    assert!(
        out.starts_with(&format!("Config         ✗ {}\n", cfg.display())),
        "{out}"
    );
    assert!(
        out.contains("Daemon         ✗ bifrostd is not running"),
        "{out}"
    );
}

/// A fresh listener on the same path.
fn sock_again(sock: &Path) -> PathBuf {
    let _ = std::fs::remove_file(sock);
    sock.to_path_buf()
}
