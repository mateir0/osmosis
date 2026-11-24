//! One module per screen. Each screen renders its inputs, launches its job
//! through `crate::gui::jobs`, and shows the shared log/result panes.

pub mod games;
pub mod import;
pub mod install;
pub mod preflight;
pub mod scan;
pub mod setup;
pub mod stage;
pub mod summary;

use crate::gui::app::phase_header;
use crate::gui::nav::Screen;
use crate::gui::state::App;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    phase_header(ui, app.screen);
    match app.screen {
        Screen::Preflight => preflight::show(app, ui),
        Screen::Scan => scan::show(app, ui),
        Screen::Stage => stage::show(app, ui),
        Screen::Install => install::show(app, ui),
        Screen::Import => import::show(app, ui),
        Screen::Setup => setup::show(app, ui),
        Screen::Games => games::show(app, ui),
        Screen::Summary => summary::show(app, ui),
    }
}
