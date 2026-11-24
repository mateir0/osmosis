//! Screen 6 — Games (`osmosis games`). Linux side; discovers the built-in
//! game table on the mounted drive, builds Wine prefixes, registers in
//! Lutris. The games drive is read-only.

use crate::games;
use crate::gui::{jobs, state::App, widgets};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    widgets::path_picker(
        ui,
        "Mounted games drive (--games-root), e.g. /media/<you>/<drive>",
        &mut app.games_root,
        true,
    );
    widgets::path_picker(ui, "Target home (--home)", &mut app.games_home, true);
    ui.add_space(6.0);
    ui.label(egui::RichText::new("Built-in game table:").strong());
    for def in games::games_table() {
        ui.label(format!("  • {} ({})", def.name, def.slug));
    }
    ui.add_space(6.0);
    if widgets::run_button(ui, app.running(), "Run games") {
        match jobs::start_games(app.games_root.clone(), app.games_home.clone(), app.dry_run) {
            Ok(handle) => app.start_job(handle),
            Err(e) => app.error = Some(e),
        }
    }
    ui.label("Prefixes + Lutris entries live under the home drive, never on the games drive.");
    ui.label("Report: <home>/osmosis-setup/games-report.{json,txt}");
    widgets::log_view(ui, &app.events);
    widgets::result_view(ui, &app.result, &app.error);
}
