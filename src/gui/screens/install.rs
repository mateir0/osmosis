//! Screen 3 — Install Linux interstitial. No engine call: OSmosis never
//! touches partitions. This screen only bridges the Windows and Linux sides.

use crate::gui::state::App;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.label(
        "This is the one step OSmosis cannot do for you. Create a USB installer \
         for one of the supported distros — Linux Mint, Ubuntu, Debian, Fedora, \
         or Arch Linux (official ISO + Rufus/balenaEtcher/Ventoy) — boot it, and \
         install it on the target machine, replacing Windows.",
    );
    ui.add_space(8.0);
    ui.label("Checklist before you continue:");
    for item in [
        "your chosen distro installed and booting",
        "the external drive still plugged in (you will need it for Import)",
        "you are logged in as the normal user whose home will receive the files",
    ] {
        ui.label(format!("  ☐ {}", item));
    }
    ui.add_space(8.0);
    ui.separator();
    ui.label(
        "OSmosis never repartitions, formats, or deletes anything on any disk. \
         It only reads your drives and writes into the folders you pick.",
    );
    let _ = app;
}
