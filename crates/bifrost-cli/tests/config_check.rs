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

    // missing file is an error, not the default config
    let o = check(&dir.join("missing.toml"), &dir);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8(o.stdout).unwrap().contains("cannot read"));
}
