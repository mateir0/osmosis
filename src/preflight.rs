//! Slice 9 — the `preflight` verb: a compatibility report run on Windows
//! BEFORE the wipe. What they have, what will work on Linux, what has an
//! equivalent, and what will NOT work.
//!
//! Windows-only: the inventory and hardware probes read the Windows registry
//! and WMI through ONE constant-script PowerShell invocation each, executed
//! as a fixed argv array through the `Runner` abstraction (no interpolated
//! values in the scripts, ever). Verdicts come from built-in data tables
//! (preflight/verdicts.rs) — preflight makes NO network calls, keeping the
//! scanner's no-network promise.
//!
//! READ-PATH RULE: a failed inventory/hardware command is recorded as a
//! failure in the report, never as a silently empty list.

pub mod inventory;
pub mod render;
pub mod verdicts;

use crate::model::Manifest;
use crate::setup::{Runner, SystemRunner};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledApp {
    pub name: String,
    pub version: String,
    pub publisher: String,
}

/// The installed-app inventory: either a de-duplicated list, or a recorded
/// failure (never an empty list on failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Inventory {
    Ok(Vec<InstalledApp>),
    Failed(String),
}

impl Inventory {
    pub fn apps(&self) -> &[InstalledApp] {
        match self {
            Inventory::Ok(a) => a,
            Inventory::Failed(_) => &[],
        }
    }
}

/// Hardware + BitLocker snapshot. `bitlocker` is "on" | "off" | "unknown" —
/// never guessed when the cmdlet is unavailable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hardware {
    pub video: Vec<String>,
    pub network: Vec<String>,
    pub printers: Vec<String>,
    pub bitlocker: String,
    pub bitlocker_detail: String,
}

impl Default for Hardware {
    fn default() -> Self {
        Hardware {
            video: Vec::new(),
            network: Vec::new(),
            printers: Vec::new(),
            bitlocker: "unknown".to_string(),
            bitlocker_detail: String::new(),
        }
    }
}

/// One classified app: name + verdict + the table's explanation fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppItem {
    pub name: String,
    pub verdict: String,
    pub linux_name: String,
    pub note: String,
    pub reason: String,
}

/// The five report buckets.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Buckets {
    pub native: Vec<AppItem>,
    pub equivalent: Vec<AppItem>,
    pub web_only: Vec<AppItem>,
    pub will_not_work: Vec<AppItem>,
    pub unknown: Vec<AppItem>,
}

/// One flagged game/anti-cheat title.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameItem {
    pub title: String,
    pub matched_in: String,
    pub reason: String,
}

/// Cross-reference of the scan manifest (when --manifest was given).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanCrossRef {
    pub placeholder_count: u64,
    pub placeholder_bytes: u64,
    pub repos: usize,
    pub games_detected: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreflightReport {
    pub schema_version: u32,
    pub created_at: String,
    pub inventory: Inventory,
    pub buckets: Buckets,
    pub games: Vec<GameItem>,
    pub hardware: Hardware,
    pub hardware_notes: Vec<String>,
    pub blocking_warnings: Vec<String>,
    /// None when no --manifest was given (report says a scan is needed).
    pub scan: Option<ScanCrossRef>,
}

pub struct PreflightOutcome {
    pub report: PreflightReport,
    pub summary: String,
}

/// `osmosis preflight [--manifest <path>] [--out <dir>]` — Windows-only verb.
pub fn run_preflight(manifest: Option<&Path>, out: &Path) -> Result<PreflightOutcome, String> {
    if !cfg!(windows) {
        return Err(
            "preflight is a Windows-side verb (it reads the Windows registry and WMI); this \
             host is not Windows — refusing to run the Windows inventory here"
                .to_string(),
        );
    }
    run_preflight_with(&SystemRunner, manifest, out)
}

/// Testable core: the runner is injected (fake runners feed recorded
/// PowerShell JSON), so the whole pipeline runs on any platform.
pub fn run_preflight_with(
    runner: &dyn Runner,
    manifest: Option<&Path>,
    out: &Path,
) -> Result<PreflightOutcome, String> {
    let inventory = inventory::collect_apps(runner);
    let (hardware, hw_error) = inventory::collect_hardware(runner);
    let scan = match manifest {
        Some(p) => Some(load_crossref(p)?),
        None => None,
    };
    let report = build_report(inventory, hardware, hw_error, scan);
    let summary = render::build_report_txt(&report);

    std::fs::create_dir_all(out)
        .map_err(|e| format!("cannot create output dir {}: {}", out.display(), e))?;
    let jp = out.join("preflight-report.json");
    let json = serde_json::to_string_pretty(&report)
        .map_err(|e| format!("cannot serialize preflight report: {}", e))?;
    std::fs::write(&jp, format!("{}\n", json))
        .map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    let tp = out.join("preflight-report.txt");
    std::fs::write(&tp, &summary).map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;

    Ok(PreflightOutcome { report, summary })
}

fn load_crossref(manifest_path: &Path) -> Result<ScanCrossRef, String> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("cannot read manifest {}: {}", manifest_path.display(), e))?;
    let m: Manifest = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse manifest {}: {}", manifest_path.display(), e))?;
    let mut games_detected: Vec<String> = Vec::new();
    for row in verdicts::GAME_ROWS {
        let t = row.title.to_lowercase();
        if m.entries
            .iter()
            .any(|e| e.relative_path.to_lowercase().contains(&t))
        {
            games_detected.push(row.title.to_string());
        }
    }
    Ok(ScanCrossRef {
        placeholder_count: m.placeholders.count,
        placeholder_bytes: m.placeholders.bytes,
        repos: m.repos.len(),
        games_detected,
    })
}

fn build_report(
    inventory: Inventory,
    hardware: Hardware,
    hw_error: Option<String>,
    scan: Option<ScanCrossRef>,
) -> PreflightReport {
    let mut buckets = Buckets::default();
    for app in inventory.apps() {
        let v = verdicts::app_verdict(&app.name);
        let item = AppItem {
            name: app.name.clone(),
            verdict: v.kind().to_string(),
            linux_name: v.linux_name().to_string(),
            note: v.note().to_string(),
            reason: v.reason().to_string(),
        };
        match v {
            verdicts::Verdict::Native => buckets.native.push(item),
            verdicts::Verdict::Equivalent { .. } => buckets.equivalent.push(item),
            verdicts::Verdict::WebOnly => buckets.web_only.push(item),
            verdicts::Verdict::WillNotWork { .. } => buckets.will_not_work.push(item),
            verdicts::Verdict::Unknown => buckets.unknown.push(item),
        }
    }

    let mut games: Vec<GameItem> = Vec::new();
    for row in verdicts::GAME_ROWS {
        let mut matched_in = Vec::new();
        if inventory
            .apps()
            .iter()
            .any(|a| verdicts::game_row_for(&a.name).map(|r| r.title) == Some(row.title))
        {
            matched_in.push("app inventory");
        }
        if scan
            .as_ref()
            .map(|s| s.games_detected.iter().any(|g| g == row.title))
            .unwrap_or(false)
        {
            matched_in.push("scan manifest");
        }
        if !matched_in.is_empty() {
            games.push(GameItem {
                title: row.title.to_string(),
                matched_in: matched_in.join(" + "),
                reason: row.reason.to_string(),
            });
        }
    }

    let mut hardware_notes: Vec<String> = Vec::new();
    let mut blocking_warnings: Vec<String> = Vec::new();
    for v in &hardware.video {
        if v.to_lowercase().contains("nvidia") {
            hardware_notes.push(format!(
                "NVIDIA GPU detected ({}): the Linux setup step installs the NVIDIA driver and \
                 flags the required reboot there — it never reboots by itself.",
                v
            ));
        }
    }
    for n in &hardware.network {
        let lower = n.to_lowercase();
        let is_wifi = ["wi-fi", "wireless", "802.11", "wlan"]
            .iter()
            .any(|k| lower.contains(k));
        if is_wifi {
            if let Some((_, label)) = verdicts::WIFI_CAUTION_ROWS
                .iter()
                .find(|(needle, _)| lower.contains(needle))
            {
                hardware_notes.push(format!(
                    "WiFi adapter \"{}\" ({}) may need extra driver work on Linux — check the \
                     distro's driver manager after install.",
                    n, label
                ));
            }
        }
    }
    match hardware.bitlocker.as_str() {
        "on" => blocking_warnings.push(
            "BitLocker is ON for the system drive C:. Turn BitLocker OFF and let decryption \
             finish BEFORE staging/wipe — Linux cannot read BitLocker volumes."
                .to_string(),
        ),
        "off" => hardware_notes.push("BitLocker is off for C: — OK to proceed.".to_string()),
        _ => hardware_notes.push(format!(
            "BitLocker status unknown — check manually before the wipe \
             (Get-BitLockerVolume -MountPoint C:){}: Linux cannot read BitLocker volumes.",
            if hardware.bitlocker_detail.is_empty() {
                String::new()
            } else {
                format!(" ({})", hardware.bitlocker_detail)
            }
        )),
    }
    if let Some(err) = hw_error {
        hardware_notes.push(format!(
            "Hardware inventory failed: {} — check video/network/BitLocker manually.",
            err
        ));
    }
    if let Some(s) = &scan {
        if s.placeholder_count > 0 {
            blocking_warnings.push(crate::placeholder::placeholder_warning(
                s.placeholder_count,
                &crate::report::human(s.placeholder_bytes),
            ));
        }
    }

    PreflightReport {
        schema_version: 1,
        created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        inventory,
        buckets,
        games,
        hardware,
        hardware_notes,
        blocking_warnings,
        scan,
    }
}
