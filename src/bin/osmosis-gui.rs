//! OSmosis desktop GUI entry point (slice 7). A thin egui shell over the same
//! library the CLI uses — see `src/gui/` and `docs/UI_PLAN.md`.

fn main() -> eframe::Result<()> {
    osmosis::gui::run()
}
