//! Final screen — where every report lives, plus the last run's outcome.

use crate::gui::{state::App, widgets};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.label("Every step writes a JSON + human-readable report you can open or share:");
    ui.monospace(
        "scan:    <out>/manifest.json, <out>/report.txt\n\
         stage:   <staging>/osmosis-stage/copy-report.{json,txt}\n\
         import:  <home>/osmosis-import/import-report.{json,txt}\n\
         setup:   <home>/osmosis-setup/setup-report.{json,txt}\n\
         games:   <home>/osmosis-setup/games-report.{json,txt}",
    );
    ui.add_space(8.0);
    if let Some(home) = [
        app.import_home.as_str(),
        app.setup_home.as_str(),
        app.games_home.as_str(),
    ]
    .iter()
    .find(|h| !h.trim().is_empty())
    {
        if ui
            .button(format!("Open report folder ({})", home))
            .clicked()
        {
            let _ = std::process::Command::new("xdg-open")
                .arg(format!("{}/osmosis-setup", home.trim()))
                .spawn();
        }
    }
    widgets::log_view(ui, &app.events);
    widgets::result_view(ui, &app.result, &app.error);
}
