//! Screen 2 — Stage (`osmosis stage`). Windows side; copies the scanned
//! keepers to the external drive, hash-verified and resumable.

use crate::gui::{jobs, state::App, widgets};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    widgets::path_picker(
        ui,
        "manifest.json from the scan (--manifest)",
        &mut app.stage_manifest,
        false,
    );
    widgets::path_picker(
        ui,
        "Staging drive (--staging), e.g. E:\\",
        &mut app.stage_staging,
        true,
    );
    ui.add_space(6.0);
    if widgets::run_button(ui, app.running(), "Run stage") {
        match jobs::start_stage(app.stage_manifest.clone(), app.stage_staging.clone()) {
            Ok(handle) => app.start_job(handle),
            Err(e) => app.error = Some(e),
        }
    }
    ui.label("Everything lands under <staging>/osmosis-stage/; re-running resumes.");
    widgets::log_view(ui, &app.events);
    widgets::result_view(ui, &app.result, &app.error);
}
