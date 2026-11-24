//! Screen 4 — Import (`osmosis import`). Linux side; unpacks the staged tree
//! into the home directory, hash-verified, never overwriting conflicts.

use crate::gui::{jobs, state::App, widgets};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    widgets::path_picker(
        ui,
        "Staged folder (--stage), e.g. /media/<you>/<drive>/osmosis-stage",
        &mut app.import_stage,
        true,
    );
    widgets::path_picker(ui, "Target home (--home)", &mut app.import_home, true);
    ui.add_space(6.0);
    if widgets::run_button(ui, app.running(), "Run import") {
        match jobs::start_import(
            app.import_stage.clone(),
            app.import_home.clone(),
            app.dry_run,
        ) {
            Ok(handle) => app.start_job(handle),
            Err(e) => app.error = Some(e),
        }
    }
    ui.label("Writes only under the target home; the stage is read-only.");
    ui.label("Receipt + report: <home>/osmosis-import/import-report.{json,txt}");
    widgets::log_view(ui, &app.events);
    widgets::result_view(ui, &app.result, &app.error);
}
