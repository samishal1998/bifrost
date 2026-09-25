//! `bifrost doctor` (PRD §24 layout, contract §9). Written by S2-F.

use crate::output::{c, table};
use bifrost_client::Client;
use bifrost_config::Config;
use bifrost_core::api::{DriverDto, StatusDto};
use bifrost_core::reconcile::select_driver;
use bifrost_core::{DriverAvailability, DriverSelector};
use bifrost_mount::DriverSettings;
use bifrost_mount::check::which;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The B7 suffix bifrost-mount appends to a macOS FUSE permission failure.
const MACOS_HINT: &str = "macOS: allow the macFUSE system extension";

/// Probes every driver here, as the daemon would (for `drivers` and `doctor` when the daemon is down).
/// The default is the first available driver in the config's auto_order, else `default_auto_order()`.
pub async fn local_drivers(cfg: Option<&Config>) -> (Vec<DriverDto>, Option<String>) {
    // a probe looks at binaries only, never at these settings
    let s = DriverSettings {
        ssh_config: None,
        vfs_cache_mode: String::new(),
        mount_timeout: Duration::ZERO,
        state_dir: PathBuf::new(),
    };
    let (mut ds, mut probes) = (Vec::new(), BTreeMap::new());
    for d in bifrost_mount::drivers(&s) {
        let a = d.probe().await;
        let name = d.name().to_string();
        ds.push(match &a {
            DriverAvailability::Available { binary, detail } => DriverDto {
                name: name.clone(),
                available: true,
                binary: Some(binary.display().to_string()),
                detail: detail.clone(),
            },
            DriverAvailability::Unavailable(why) => DriverDto {
                name: name.clone(),
                available: false,
                binary: None,
                detail: why.clone(),
            },
        });
        probes.insert(name, a);
    }
    let order = cfg.map_or_else(bifrost_config::default_auto_order, |c| c.auto_order.clone());
    let auto = select_driver(&DriverSelector::Auto, &order, &probes).ok();
    (ds, auto)
}

fn kv(label: &str, text: &str) -> String {
    format!("{label:<15}{text}\n")
}

fn indent(t: &str) -> String {
    t.lines().map(|l| format!("  {l}\n")).collect()
}

/// Exit 1 on a ✗ in Config or Drivers. A driver that is merely unavailable (rclone-nfs on Linux) is shown ✗
/// but informational; Drivers fails when no driver is usable (Selected default: none) or a mount carries
/// the macOS permission hint (B7).
pub async fn run(client: &Client, config: Option<PathBuf>) -> i32 {
    let daemon = client.get::<StatusDto>("/v1/status").await;
    let st = daemon.as_ref().ok();
    // --config (or $BIFROST_CONFIG, its env form; empty = unset), else the file the daemon runs on, else the default
    let path = config
        .or_else(|| {
            std::env::var_os("BIFROST_CONFIG")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        })
        .or_else(|| st.map(|s| PathBuf::from(&s.config_path)))
        .unwrap_or_else(bifrost_config::paths::config_path);
    let shown = c(&path.display().to_string()); // may be the daemon's string (§9)
    let cfg = bifrost_config::load(&path);
    let mut bad = false;
    let mut out = String::new();

    match &cfg {
        Ok(_) => out += &kv("Config", &format!("✓ {shown}")),
        Err(es) => {
            bad = true;
            out += &kv("Config", &format!("✗ {shown}"));
            es.iter()
                .for_each(|e| out += &format!("  {}\n", c(&e.to_string())));
        }
    }
    out += &kv(
        "Daemon",
        &match &daemon {
            Ok(s) => format!("✓ running pid {}", s.pid),
            Err(e) => format!("✗ {}", c(&e.to_string())),
        },
    );
    let ssh = which("ssh").map_or("✗ ssh not found".into(), |p| {
        format!("✓ ssh {}", p.display())
    });
    let agent = match st {
        Some(s) if s.ssh_agent => "✓ SSH_AUTH_SOCK visible to daemon",
        Some(_) => "✗ SSH_AUTH_SOCK not visible to daemon",
        None if std::env::var_os("SSH_AUTH_SOCK").is_some() => "✓ SSH_AUTH_SOCK set",
        None => "✗ SSH_AUTH_SOCK not set",
    };
    out += &kv("SSH", &format!("{ssh}   {agent}"));

    out += "Discovery\n";
    let rows = match (st, &cfg) {
        (Some(s), _) => s
            .providers
            .iter()
            .map(|p| match &p.last_error {
                None => vec![format!("✓ {}", p.name), format!("{} machines", p.machines)],
                Some(e) => vec![format!("✗ {}", p.name), e.clone()],
            })
            .collect(),
        (None, Ok(cfg)) => {
            let n = cfg.machines.len();
            let mut rows = vec![vec!["✓ static".into(), format!("{n} machines")]];
            rows.extend(
                cfg.providers
                    .iter()
                    .map(|p| vec![format!("- {}", p.name), "daemon not running".into()]),
            );
            rows
        }
        (None, Err(_)) => vec![],
    };
    out += &indent(&table(rows));

    out += "Mount Drivers\n";
    let (ds, auto) = match st {
        Some(s) => (s.drivers.clone(), s.auto_driver.clone()),
        None => local_drivers(cfg.as_ref().ok()).await,
    };
    let mut rows: Vec<Vec<String>> = ds
        .iter()
        .map(|d| match &d.binary {
            Some(b) if d.available => vec![format!("✓ {}", d.name), format!("{b} ({})", d.detail)],
            _ => vec![format!("✗ {}", d.name), d.detail.clone()],
        })
        .collect();
    for m in st.map_or(&[][..], |s| &s.mounts) {
        if let Some(e) = m.last_error.as_ref().filter(|e| e.contains(MACOS_HINT)) {
            bad = true;
            rows.push(vec![format!("✗ {}", m.id), e.clone()]);
        }
    }
    out += &indent(&table(rows));

    out += &kv("FUSE", &fuse());
    out += "Selected default\n";
    match auto {
        Some(a) => out += &format!("  {}\n", c(&a)),
        // the daemon's first probe hasn't answered yet (drivers "probing"): nothing is known missing
        None if ds.iter().any(|d| d.detail == "probing") => out += "  probing\n",
        None => {
            bad = true;
            out += "  ✗ none (no available driver)\n";
        }
    }
    print!("{out}");
    i32::from(bad)
}

/// Linux: /dev/fuse + the fusermount helper. macOS: macFUSE or FUSE-T (the paths sshfs's flavor() checks).
fn fuse() -> String {
    let is = |p: &str| Path::new(p).exists();
    if cfg!(target_os = "macos") {
        return if is("/Library/Filesystems/macfuse.fs") {
            "✓ macFUSE".into()
        } else if is("/Library/Application Support/fuse-t") || is("/usr/local/lib/libfuse-t.dylib")
        {
            "✓ FUSE-T".into()
        } else {
            "✗ neither macFUSE nor FUSE-T is installed".into()
        };
    }
    match (
        is("/dev/fuse"),
        which("fusermount3").or_else(|| which("fusermount")),
    ) {
        (true, Some(h)) => format!("✓ /dev/fuse  {}", h.display()),
        (false, _) => "✗ /dev/fuse missing".into(),
        (true, None) => "✗ fusermount3 not found".into(),
    }
}
