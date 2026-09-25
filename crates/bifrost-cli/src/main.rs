//! `bifrost` CLI (contract §9). S1-B: `config check`; S2-F adds the rest.

mod doctor;
mod output;

use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

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
    /// Print JSON instead of text
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Configuration
    #[command(subcommand)]
    Config(ConfigCmd),
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Validate a config file (no daemon needed)
    Check {
        /// defaults to --config
        path: Option<PathBuf>,
    },
}

fn main() {
    let cli = Cli::parse();
    let code = match cli.cmd {
        Cmd::Config(ConfigCmd::Check { path }) => {
            let path = path
                .or(cli.config)
                .unwrap_or_else(bifrost_config::paths::config_path);
            config_check(&path, cli.json)
        }
    };
    std::process::exit(code);
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
