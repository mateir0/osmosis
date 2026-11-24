//! Screen 1 — Scan (`osmosis scan`). Windows side of the journey.

use crate::gui::{jobs, state::App, widgets};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    widgets::path_picker(ui, "Profile folder (--root)", &mut app.scan_root, true);
    widgets::path_picker(ui, "Output folder (--out)", &mut app.scan_out, true);
    widgets::path_picker(
        ui,
        "Staging drive, optional fit check (--staging)",
        &mut app.scan_staging,
        true,
    );
    widgets::path_picker(
        ui,
        "Config TOML, optional (--config)",
        &mut app.scan_config,
        false,
    );
    ui.add_space(6.0);
    if widgets::run_button(ui, app.running(), "Run scan") {
        match jobs::start_scan(
            app.scan_root.clone(),
            app.scan_out.clone(),
            app.scan_staging.clone(),
            app.scan_config.clone(),
        ) {
            Ok(handle) => app.start_job(handle),
            Err(e) => app.error = Some(e),
        }
    }
    ui.label("Writes only manifest.json + report.txt inside the output folder.");
    widgets::log_view(ui, &app.events);
    widgets::result_view(ui, &app.result, &app.error);
}
