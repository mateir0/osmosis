//! Screen 0 — Preflight (`osmosis preflight`). Windows-only verb: the Run
//! button is disabled on other platforms (the screen stays visible so the
//! journey reads the same everywhere). After a run, the bucket counts and
//! the Will-NOT-Work list are shown on the screen itself.

use crate::gui::{jobs, state::App, widgets};
use crate::preflight::PreflightReport;

/// Flattened view of a finished preflight report for on-screen display.
pub struct PreflightView {
    pub counts: [(&'static str, usize); 5],
    pub will_not_work: Vec<String>,
    pub games_blocked: Vec<String>,
    pub blocking_warnings: Vec<String>,
}

impl PreflightView {
    pub fn from_report(r: &PreflightReport) -> PreflightView {
        let b = &r.buckets;
        PreflightView {
            counts: [
                ("native", b.native.len()),
                ("equivalent", b.equivalent.len()),
                ("web only", b.web_only.len()),
                ("will NOT work", b.will_not_work.len()),
                ("unknown", b.unknown.len()),
            ],
            will_not_work: b
                .will_not_work
                .iter()
                .map(|a| format!("{} — {}", a.name, a.reason))
                .collect(),
            games_blocked: r
                .games
                .iter()
                .map(|g| format!("{} ({}) — {}", g.title, g.matched_in, g.reason))
                .collect(),
            blocking_warnings: r.blocking_warnings.clone(),
        }
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    widgets::path_picker(
        ui,
        "manifest.json from the scan, optional (--manifest)",
        &mut app.preflight_manifest,
        false,
    );
    widgets::path_picker(ui, "Output folder (--out)", &mut app.preflight_out, true);
    ui.add_space(6.0);
    let enabled = cfg!(windows);
    if !enabled {
        ui.colored_label(
            egui::Color32::YELLOW,
            "Preflight reads the Windows registry/WMI — it only runs on Windows.",
        );
    }
    ui.add_enabled_ui(enabled, |ui| {
        if widgets::run_button(ui, app.running(), "Run preflight") {
            match jobs::start_preflight(
                app.preflight_manifest.clone(),
                app.preflight_out.clone(),
                app.preflight_shared.clone(),
            ) {
                Ok(handle) => app.start_job(handle),
                Err(e) => app.error = Some(e),
            }
        }
    });
    ui.label("Writes only preflight-report.json + preflight-report.txt inside the output folder.");

    if let Ok(g) = app.preflight_shared.lock() {
        if let Some(report) = g.as_ref() {
            let view = PreflightView::from_report(report);
            ui.add_space(8.0);
            ui.heading("Preflight result");
            for (label, n) in view.counts {
                ui.label(format!("{}: {}", label, n));
            }
            if !view.will_not_work.is_empty() {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::RED, "WILL NOT WORK ON LINUX:");
                for line in &view.will_not_work {
                    ui.colored_label(egui::Color32::RED, format!("  - {}", line));
                }
            }
            if !view.games_blocked.is_empty() {
                ui.add_space(4.0);
                ui.colored_label(egui::Color32::RED, "GAMES BLOCKED BY ANTI-CHEAT:");
                for line in &view.games_blocked {
                    ui.colored_label(egui::Color32::RED, format!("  - {}", line));
                }
            }
            for w in &view.blocking_warnings {
                ui.colored_label(egui::Color32::GOLD, format!("  ! {}", w));
            }
        }
    }

    widgets::log_view(ui, &app.events);
    widgets::result_view(ui, &app.result, &app.error);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preflight::{AppItem, Buckets, GameItem, Hardware, Inventory, ScanCrossRef};

    fn item(name: &str) -> AppItem {
        AppItem {
            name: name.to_string(),
            verdict: "will_not_work".to_string(),
            linux_name: String::new(),
            note: String::new(),
            reason: "no Linux version".to_string(),
        }
    }

    #[test]
    fn view_lists_will_not_work_and_counts_buckets() {
        let mut buckets = Buckets::default();
        buckets.will_not_work.push(item("Visual Studio 2022"));
        buckets.native.push(AppItem {
            verdict: "native".to_string(),
            ..item("Steam")
        });
        let report = PreflightReport {
            schema_version: 1,
            created_at: "2026-11-25T00:00:00Z".to_string(),
            inventory: Inventory::Ok(vec![]),
            buckets,
            games: vec![GameItem {
                title: "Valorant".to_string(),
                matched_in: "app inventory".to_string(),
                reason: "Vanguard kernel anti-cheat does not run on Linux".to_string(),
            }],
            hardware: Hardware::default(),
            hardware_notes: vec![],
            blocking_warnings: vec!["BitLocker is ON for C:.".to_string()],
            scan: Some(ScanCrossRef {
                placeholder_count: 0,
                placeholder_bytes: 0,
                repos: 0,
                games_detected: vec![],
            }),
        };
        let v = PreflightView::from_report(&report);
        assert_eq!(v.counts[0], ("native", 1));
        assert_eq!(v.counts[3], ("will NOT work", 1));
        assert_eq!(v.will_not_work.len(), 1);
        assert!(v.will_not_work[0].contains("Visual Studio 2022"));
        assert_eq!(v.games_blocked.len(), 1);
        assert!(v.games_blocked[0].contains("Valorant"));
        assert_eq!(v.blocking_warnings.len(), 1);
    }
}
