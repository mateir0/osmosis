//! Screen 5 — Setup (`osmosis setup`). Linux side; runs through
//! `run_setup_with` with the GuiRunner so every step/command streams into the
//! progress log.

use crate::gui::{jobs, state::App, widgets};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    widgets::path_picker(ui, "Target home (--home)", &mut app.setup_home, true);
    ui.add_space(4.0);
    let detected = crate::distro::detect()
        .map(|d| d.display_name())
        .unwrap_or_else(|e| e);
    ui.label(format!("Detected distro: {}", detected));
    ui.add_space(6.0);
    if widgets::run_button(ui, app.running(), "Run setup") {
        match jobs::start_setup(app.setup_home.clone(), app.dry_run) {
            Ok(handle) => app.start_job(handle),
            Err(e) => app.error = Some(e),
        }
    }
    ui.label("Runs unprivileged, escalating per command with sudo. Never reboots by itself.");
    ui.label("Report: <home>/osmosis-setup/setup-report.{json,txt}");
    widgets::log_view(ui, &app.events);
    widgets::result_view(ui, &app.result, &app.error);
}
