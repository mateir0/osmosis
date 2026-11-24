//! Slice 7 — the OSmosis desktop GUI (egui/eframe).
//!
//! This is a thin shell over the library: every screen maps 1:1 onto an
//! existing entry point (`run_scan`, `run_stage`, `run_import`,
//! `run_setup_with`, `run_games_with`) and renders the JSON/text reports the
//! library already produces. The GUI never re-implements engine logic, and
//! the read-only guarantees of the scanner and games drive hold here too.

pub mod app;
pub mod jobs;
pub mod nav;
pub mod runner;
pub mod screens;
pub mod state;
pub mod widgets;

/// Launch the desktop app. Called from `src/bin/osmosis-gui.rs`.
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([920.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "OSmosis",
        options,
        Box::new(|_cc| Ok(Box::new(state::App::new()))),
    )
}
