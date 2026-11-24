//! Spawning engine jobs on worker threads. Each `start_*` mirrors one CLI
//! subcommand; the closure runs on a background thread and talks to the UI
//! only through the channel, so the egui frame never blocks.

use crate::config::Config;
use crate::gui::runner::{GuiRunner, JobKind, JobMsg, JobSummary};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::thread;

/// Handle to a running job: the receiving end of its message channel.
pub struct JobHandle {
    pub kind: JobKind,
    pub rx: Receiver<JobMsg>,
}

fn spawn(
    kind: JobKind,
    work: impl FnOnce(Sender<JobMsg>) -> Result<JobSummary, String> + Send + 'static,
) -> JobHandle {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let result = work(tx.clone());
        let _ = tx.send(JobMsg::Done(result));
    });
    JobHandle { kind, rx }
}

fn opt_path(s: &str) -> Option<PathBuf> {
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(PathBuf::from(t))
    }
}

/// `osmosis scan [--root] [--config] [--out] [--staging]`
pub fn start_scan(
    root: String,
    out: String,
    staging: String,
    config: String,
) -> Result<JobHandle, String> {
    let root = opt_path(&root).ok_or("choose the profile folder to scan")?;
    let out = opt_path(&out).ok_or("choose the output folder")?;
    let staging = opt_path(&staging);
    let config = opt_path(&config);
    Ok(spawn(JobKind::Scan, move |_tx| {
        let cfg = match &config {
            Some(p) => Config::load_override(p)?,
            None => Config::default_config(),
        };
        let res = crate::scan::run_scan(&root, &cfg, &out, staging.as_deref())?;
        let errs = res.manifest.totals.error_count;
        Ok(JobSummary {
            kind: JobKind::Scan,
            ok: true,
            exit_code: 0,
            text: format!(
                "{} ({} read error(s) recorded in manifest.json)",
                res.summary, errs
            ),
        })
    }))
}

/// `osmosis stage --manifest <p> --staging <p>`
pub fn start_stage(manifest: String, staging: String) -> Result<JobHandle, String> {
    let manifest = opt_path(&manifest).ok_or("choose manifest.json from the scan")?;
    let staging = opt_path(&staging).ok_or("choose the staging drive/folder")?;
    Ok(spawn(JobKind::Stage, move |_tx| {
        let out = crate::stage::run_stage(&manifest, &staging)?;
        Ok(JobSummary {
            kind: JobKind::Stage,
            ok: out.exit_code == 0,
            exit_code: out.exit_code,
            text: out.summary,
        })
    }))
}

/// `osmosis import --stage <dir> [--home] [--dry-run]`
pub fn start_import(stage: String, home: String, dry_run: bool) -> Result<JobHandle, String> {
    let stage = opt_path(&stage).ok_or("choose the osmosis-stage folder")?;
    let home = opt_path(&home).ok_or("choose the target home directory")?;
    Ok(spawn(JobKind::Import, move |_tx| {
        let out = crate::importer::run_import(&stage, &home, dry_run)?;
        Ok(JobSummary {
            kind: JobKind::Import,
            ok: out.exit_code == 0,
            exit_code: out.exit_code,
            text: out.summary,
        })
    }))
}

/// `osmosis setup [--home] [--dry-run]` — via `run_setup_with` so the
/// GuiRunner streams step/command events into the UI.
pub fn start_setup(home: String, dry_run: bool) -> Result<JobHandle, String> {
    let home = opt_path(&home).ok_or("choose the target home directory")?;
    Ok(spawn(JobKind::Setup, move |tx| {
        let runner = GuiRunner { tx: tx.clone() };
        let work = std::env::temp_dir().join(format!("osmosis-gui-setup-{}", std::process::id()));
        let out = crate::setup::run_setup_with(&runner, &home, dry_run, &work)?;
        Ok(JobSummary {
            kind: JobKind::Setup,
            ok: out.exit_code == 0,
            exit_code: out.exit_code,
            text: out.summary,
        })
    }))
}

/// `osmosis games --games-root <dir> [--home] [--dry-run]` — same streaming
/// runner as setup.
pub fn start_games(games_root: String, home: String, dry_run: bool) -> Result<JobHandle, String> {
    let games_root = opt_path(&games_root).ok_or("choose the mounted games drive")?;
    let home = opt_path(&home).ok_or("choose the target home directory")?;
    Ok(spawn(JobKind::Games, move |tx| {
        let runner = GuiRunner { tx: tx.clone() };
        let work = std::env::temp_dir().join(format!("osmosis-gui-games-{}", std::process::id()));
        let out = crate::games::run_games_with(&runner, &home, &games_root, dry_run, &work)?;
        Ok(JobSummary {
            kind: JobKind::Games,
            ok: out.exit_code == 0,
            exit_code: out.exit_code,
            text: out.summary,
        })
    }))
}
