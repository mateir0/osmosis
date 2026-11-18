//! OSmosis CLI entry point. Slice 1 exposes a single subcommand: `scan`.

use clap::{Parser, Subcommand};
use osmosis::config::Config;
use osmosis::scan::run_scan;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "osmosis",
    version,
    about = "Move from Windows to Linux, without losing anything."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan a directory tree and write manifest.json + report.txt into --out.
    Scan {
        /// Directory to scan. Defaults to the USERPROFILE environment variable.
        #[arg(long)]
        root: Option<PathBuf>,
        /// Optional TOML config overriding the embedded defaults.
        #[arg(long)]
        config: Option<PathBuf>,
        /// Output directory (the only place the program writes).
        #[arg(long, default_value = "./osmosis-output")]
        out: PathBuf,
        /// Optional staging path for a free-space fit check.
        #[arg(long)]
        staging: Option<PathBuf>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Commands::Scan {
            root,
            config,
            out,
            staging,
        } => {
            let root = match root {
                Some(r) => r,
                None => match std::env::var_os("USERPROFILE") {
                    Some(v) if !v.is_empty() => PathBuf::from(v),
                    _ => {
                        eprintln!(
                            "osmosis: error: no --root given and the USERPROFILE environment variable is not set."
                        );
                        eprintln!("osmosis: pass --root <dir> explicitly, e.g. `osmosis scan --root %USERPROFILE%`.");
                        return ExitCode::from(2);
                    }
                },
            };
            let cfg = match &config {
                Some(p) => match Config::load_override(p) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("osmosis: error: {}", e);
                        return ExitCode::from(2);
                    }
                },
                None => Config::default_config(),
            };
            match run_scan(&root, &cfg, &out, staging.as_deref()) {
                Ok(res) => {
                    // the same summary that was written to report.txt, on stdout
                    print!("{}", res.summary);
                    if res.manifest.totals.error_count > 0 {
                        eprintln!(
                            "osmosis: scan completed with {} read error(s) (listed in manifest.json)",
                            res.manifest.totals.error_count
                        );
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("osmosis: error: {}", e);
                    ExitCode::FAILURE
                }
            }
        }
    }
}
