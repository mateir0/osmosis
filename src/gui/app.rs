//! The `eframe::App` implementation: sidebar navigation plus the current
//! screen in `ui`, and the per-frame job polling in `logic` (called before
//! every pass and whenever a worker thread requests a repaint).

use crate::gui::nav::{can_advance, next_screen, prev_screen, screens_for_platform, Screen};
use crate::gui::state::App;

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.set_ctx(ctx);
        self.poll_job();
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::Panel::left("osmosis-nav")
            .resizable(false)
            .show(ui, |ui| {
                ui.add_space(8.0);
                ui.heading("OSmosis");
                ui.label(if cfg!(windows) {
                    "Windows side: scan + stage"
                } else {
                    "Linux side: import, setup, games"
                });
                ui.separator();
                for &s in screens_for_platform() {
                    if ui.selectable_label(self.screen == s, s.title()).clicked() {
                        self.screen = s;
                    }
                }
                ui.separator();
                ui.checkbox(&mut self.dry_run, "Dry run (preview only, changes nothing)");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            prev_screen(self.screen).is_some(),
                            egui::Button::new("← Back"),
                        )
                        .clicked()
                    {
                        if let Some(p) = prev_screen(self.screen) {
                            self.screen = p;
                        }
                    }
                    let next = next_screen(self.screen);
                    let enabled = next.is_some() && can_advance(self.screen, self.last_ok());
                    if ui
                        .add_enabled(enabled, egui::Button::new("Next →"))
                        .clicked()
                    {
                        if let Some(n) = next {
                            self.screen = n;
                        }
                    }
                });
                if self.running() {
                    ui.label(egui::RichText::new("Working…").italics());
                }
            });

        egui::CentralPanel::default().show(ui, |ui| {
            crate::gui::screens::show(self, ui);
        });
    }
}

/// Header shared by every phase screen: title + what-this-does blurb.
pub fn phase_header(ui: &mut egui::Ui, screen: Screen) {
    ui.heading(screen.title());
    ui.label(screen.blurb());
    ui.separator();
}
