//! Small shared widgets: path pickers, the run button, and the log/result
//! panes every phase screen reuses.

use crate::gui::runner::JobSummary;
use std::path::PathBuf;

/// A labeled text field with a Browse button (folder or file picker via rfd).
/// Returns true when the value changed.
pub fn path_picker(ui: &mut egui::Ui, label: &str, value: &mut String, folder: bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed |= ui.text_edit_singleline(value).changed();
        if ui.button("Browse…").clicked() {
            let picked: Option<PathBuf> = if folder {
                rfd::FileDialog::new().set_title(label).pick_folder()
            } else {
                rfd::FileDialog::new().set_title(label).pick_file()
            };
            if let Some(p) = picked {
                *value = p.to_string_lossy().into_owned();
                changed = true;
            }
        }
    });
    changed
}

/// The primary action button; disabled while a job is running.
pub fn run_button(ui: &mut egui::Ui, running: bool, label: &str) -> bool {
    ui.add_enabled(!running, egui::Button::new(label)).clicked()
}

/// Scrollable live log of job events.
pub fn log_view(ui: &mut egui::Ui, events: &[String]) {
    if events.is_empty() {
        return;
    }
    ui.separator();
    ui.label(egui::RichText::new("Progress").strong());
    egui::ScrollArea::vertical()
        .max_height(180.0)
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for line in events {
                ui.monospace(line);
            }
        });
}

/// The outcome of the last finished job: report summary or the error.
pub fn result_view(ui: &mut egui::Ui, result: &Option<JobSummary>, error: &Option<String>) {
    ui.separator();
    if let Some(err) = error {
        ui.colored_label(egui::Color32::LIGHT_RED, format!("Error: {}", err));
    }
    if let Some(res) = result {
        let color = if res.ok {
            egui::Color32::LIGHT_GREEN
        } else {
            egui::Color32::GOLD
        };
        ui.colored_label(
            color,
            egui::RichText::new(format!(
                "{} finished (exit {}) — {}",
                res.kind.label(),
                res.exit_code,
                if res.ok {
                    "success"
                } else {
                    "with problems, see report"
                }
            ))
            .strong(),
        );
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                ui.monospace(&res.text);
            });
    }
}
