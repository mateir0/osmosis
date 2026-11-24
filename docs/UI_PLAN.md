# OSmosis desktop UI — plan

Status: **plan only. No UI code exists or is written yet.** This document is the
agreement for how the desktop app should be built. Everything below is a
proposal to ratify, not a spec of finished work.

## Goal

A single desktop app that walks a non-technical user through the whole
Windows -> Linux switch without ever opening a terminal: scan -> stage ->
"install Mint" -> import -> setup -> games. The app must feel like the CLI it
wraps, never more powerful: every action maps 1:1 onto an existing subcommand
and its arguments.

## Recommendation: egui as a second binary in the same crate

- Add `src/bin/osmosis-gui.rs` (or a `[[bin]]` entry) in the **same Cargo
  crate**, depending on `egui` + `eframe`. The GUI links against `osmosis` as a
  library and calls the exact same entry points the CLI uses:
  `run_scan`, `run_stage`, `run_import`, `run_setup_with`, `run_games_with`
  (all re-exported from `src/lib.rs`), with a custom `Runner`
  implementation that streams `CommandOutput`/step progress into the UI
  instead of running on a terminal.
- Why egui over Tauri:
  - Rust-native, no Node/webview toolchain, tiny incremental build cost.
  - One binary per platform, matching how the release workflow already ships
    `osmosis-windows-x86_64.exe` / `osmosis-linux-x86_64` — the GUI just becomes
    a second artifact (`osmosis-gui-windows-x86_64.exe`, ...).
  - The CLI and GUI can never drift apart, because they share one library.
- Tauri alternative (web frontend + Rust backend via `invoke`): nicer styling
  and animations, but adds a JS toolchain, a webview runtime on Linux (WebKitGTK
  dependency on Mint), and a much bigger release payload. Revisit only if the
  egui look becomes a blocker for adoption.

## Wizard screens mapped to the CLI commands

One screen per phase, Next enabled only when the phase reports success.
Every screen has a "what this does" line, a dry-run/preview mode, and an error
state that shows the failing step + command from the report JSON instead of a
bare error code.

| # | Screen | CLI equivalent | Shows | Calls | Error state |
|---|--------|----------------|-------|-------|-------------|
| 1 | Scan | `scan [--root] [--config] [--out] [--staging]` | root picker (default `%USERPROFILE%`), staging-drive picker + fit check, KEEP/RECREATE/SKIP counts from the report | `run_scan` | per-category failure list from `report.txt`/`manifest.json` |
| 2 | Stage | `stage --manifest <p> --staging <p>` | progress bar per file (hash-verified copies), resume notice | `run_stage` then `verify_stage` | `verify_failed` / `skipped_locked` categories, each file listed |
| 3 | Install Mint | (none — interstitial) | instructions + checklist ("create USB installer, install Mint, come back"), explicit statement that OSmosis never touches partitions | none | n/a; Next = "I've installed Mint" |
| 4 | Import | `import --stage <dir> [--home] [--dry-run]` | dry-run preview: to-place / already-present / conflicts (never overwritten) | `run_import` with `dry_run` toggle | conflict list + receipt path `~/osmosis-import/` |
| 5 | Setup | `setup [--home] [--dry-run]` | step checklist with live status (base, nvidia, gh, node, rust, vscode, chrome, opencode, lutris, wine-ge) | `run_setup_with` via progress-`Runner` | failing step + command output; reboot flag surfaced as "reboot recommended", never auto-reboot |
| 6 | Games | `games --games-root <dir> [--home] [--dry-run]` | discovered games table (`games_table()`), per-game prefix/winetricks progress, Lutris registration result | `run_games_with` | per-game `GameReport` failures, `pga.db` write status |

Final screen: summary of all reports (`scan/stage/import/setup/games` JSON+txt
paths) with "open folder" buttons.

## Cross-cutting behavior

- **Dry-run everywhere**: every mutating screen has a preview toggle wired to
  the existing `--dry-run` paths; the CLI flags stay the source of truth.
- **Reports as the UI's data**: screens render from the JSON reports the
  library already produces; the GUI never re-implements scan/stage/import/
  setup/games logic.
- **Read-only guarantees stay**: scanner and games drive stay read-only in the
  GUI too; no screen may offer delete/format/partition actions.
- **Linux vs Windows binary**: scan+stage screens only on the Windows build,
  import/setup/games only on the Linux build (the release workflow already
  cross-compiles both); the "Install Mint" interstitial bridges the two.

## Non-goals (for now)

- No tray icon, no background service, no auto-update (GitHub `latest`
  prerelease manual download is enough at this stage).
- No support for arbitrary games beyond the built-in `games_table()`; no
  dual-boot flow; no theming beyond light/dark.

## Open questions

1. Binary naming: `osmosis-gui-*` artifacts alongside the CLI ones, or replace
   the CLI binaries with the GUI at some milestone?
2. Windows SmartScreen: the unsigned `.exe` will warn on first run — add a
   README note + (later) signing, or accept the warning for now?
3. To stream step-level progress into egui, `run_setup_with`/`run_games_with`
   need a progress callback or event channel on the `Runner` trait — small
   additive refactor; confirm the trait shape before writing any UI code.
4. Should the GUI shell out to the CLI binary instead of linking the library
   (simpler, but loses streaming and type safety)? Recommendation: link the
   library.
