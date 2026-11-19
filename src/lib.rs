//! OSmosis Slice 1 — read-only Windows-profile scanner.
//!
//! The scanner reads files and writes ONLY `manifest.json` and `report.txt`
//! inside the `--out` directory. It never modifies, moves, or deletes anything
//! else, makes no network calls, and never panics on filesystem errors.

pub mod classify;
pub mod config;
pub mod games;
pub mod importer;
pub mod model;
pub mod report;
pub mod scan;
pub mod setup;
pub mod stage;

pub use config::Config;
pub use games::{
    games_table, run_games, run_games_with, GameDef, GameKind, GameReport, GamesOutcome,
    GamesReport,
};
pub use importer::{run_import, ImportOutcome, ImportReport};
pub use scan::{run_scan, ScanResult};
pub use setup::{run_setup, run_setup_with, CommandOutput, Runner, SetupOutcome, SetupReport};
pub use stage::{run_stage, verify_stage, CopyReport, StageOutcome};
