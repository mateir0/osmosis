# OSmosis

**Move from Windows to Linux, without losing anything.**

OSmosis is an open-source (GPL-3.0) tool written in Rust that walks you through
the whole switch: it scans your Windows profile, copies what matters to an
external drive (hash-verified), restores it onto a fresh Linux install,
turns that install into your working machine, and registers your standalone
games in Lutris so they are ready to click.

## Download

Grab the prebuilt binaries from the
[GitHub Releases page](https://github.com/mateir0/osmosis/releases) (the
`latest` prerelease, rebuilt from `master` on every push):

- **Windows** (for the scan + stage steps): `osmosis-windows-x86_64.exe`
- **Linux** (for the import + setup + games steps): `osmosis-linux-x86_64`
- **Desktop GUI** (no terminal needed — same steps as a click-through wizard):
  `osmosis-gui-windows-x86_64.exe` and `osmosis-gui-linux-x86_64`

Or build from source with `cargo build --release` (see Development below).
The GUI is a second binary, `osmosis-gui`, built from the same crate.

## The journey

Seven steps, in order. Preflight, scan, and stage run on Windows; the last
four run on the fresh Linux machine (Linux Mint, Ubuntu, Debian, Fedora, or
Arch Linux).

### 0. Preflight (Windows, optional but recommended)

```powershell
osmosis-windows-x86_64.exe preflight --out ./osmosis-output
osmosis-windows-x86_64.exe preflight --manifest ./osmosis-output/manifest.json --out ./osmosis-output
```

Advisory compatibility report written before anything is wiped:
`preflight-report.{json,txt}` in `--out`. It inventories installed apps via the
uninstall registry (one fixed PowerShell call, no network) and buckets them —
native on Linux / has an equivalent / web-only / will NOT work / unknown — plus
games blocked by kernel anti-cheat (Valorant, Fortnite, ...), hardware notes
(NVIDIA driver step, WiFi adapters that may need extra drivers), and BitLocker
status (a blocking warning when ON). Pass `--manifest` from a scan to also
cross-reference OneDrive placeholders and detected games. It never blocks the
journey and only ever runs on Windows.

### 1. Scan (Windows)

```powershell
osmosis-windows-x86_64.exe scan
osmosis-windows-x86_64.exe scan --root C:\Users\You --out ./osmosis-output --staging E:\ --config config.example.toml
```

Walks the user profile (default `--root` = `%USERPROFILE%`), classifies every
file (KEEP / RECREATE / SKIP), hashes the keepers with xxh64, detects git repos,
and writes `manifest.json` + a human-readable `report.txt` into `--out` (default
`./osmosis-output`). With `--staging`, the report adds a fit check against the
external drive's free space. Classification is: explicit KEEP mappings first
(e.g. `Documents` -> `~/Documents`, `.ssh` -> `~/.ssh`), then RECREATE directory
matches (`node_modules`, `target`, ...), then SKIP rules (most of `AppData`,
`Thumbs.db`, `*.tmp`, ...), and otherwise KEEP only inside mapped top-level
folders. OneDrive "Files On-Demand" placeholders (online-only stubs) are flagged
per file in `manifest.json` and called out loudly in `report.txt` — set them to
"Always keep on this device" and re-run the scan so they are real before
staging.

### 2. Stage to the external drive (Windows)

```powershell
osmosis-windows-x86_64.exe stage --manifest .\osmosis-output\manifest.json --staging E:\
```

Copies every keeper to `<staging>/osmosis-stage/`, hash-verifying each file as
it goes, and is resumable — re-running skips what is already staged and correct.
Files that are still online-only cloud placeholders at stage time are NEVER
staged as successes: they are listed under `SKIPPED (PLACEHOLDER ...)`, the
report ends with `STAGING IS INCOMPLETE`, and the exit code is 2.

### 3. Install Linux (Mint, Ubuntu, Debian, Fedora, or Arch)

This is the one step OSmosis cannot do for you: create a USB installer for
Linux Mint, Ubuntu, Debian, Fedora, or Arch Linux and install it on the target
machine, replacing Windows. OSmosis never repartitions or touches your disks.

### 4. Import (Linux)

```bash
osmosis-linux-x86_64 import --stage /media/<user>/<drive>/osmosis-stage --dry-run
osmosis-linux-x86_64 import --stage /media/<user>/<drive>/osmosis-stage
```

Unpacks the staged tree into your home directory (`--home`, default `$HOME`),
hash-verifying every file again, applies a permission plan, and writes an import
receipt + report. `--dry-run` prints the plan (to place / already present /
conflicts) and writes nothing.

### 5. Setup (Linux)

```bash
osmosis-linux-x86_64 setup --dry-run
osmosis-linux-x86_64 setup
```

Idempotent step runner that turns the fresh install into the working machine.
The distro is auto-detected from `/etc/os-release` (Linux Mint, Ubuntu,
Debian, Fedora, and Arch Linux are supported; anything else is refused before
any command runs) and every package command goes through that distro's
package manager: `apt` (Mint/Ubuntu/Debian), `dnf` + RPM Fusion (Fedora), or
`pacman` (Arch). It installs base packages + codecs, the NVIDIA driver (when
a GPU is present), GitHub CLI, Node via nvm + pnpm, Rust via rustup, VS Code,
Google Chrome (Chromium on Arch), OpenCode, and Lutris + Wine-GE. Runs
unprivileged and escalates per command with `sudo`
(`DEBIAN_FRONTEND=noninteractive` is passed through on apt so nothing can
block on an interactive prompt). `--dry-run` prints the plan (would-run /
already-done / not-applicable) and changes nothing. Report:
`~/osmosis-setup/setup-report.{json,txt}`.

### 6. Games (Linux)

```bash
osmosis-linux-x86_64 games --games-root /media/<user>/<drive> --dry-run
osmosis-linux-x86_64 games --games-root /media/<user>/<drive>
```

Finds the standalone games on the mounted external drive, builds one Wine
prefix per game under `~/.local/share/osmosis/prefixes/<slug>`, installs each
game's winetricks requirements, and registers ready-to-click entries in Lutris
(`~/.config/lutris/games/<slug>-osmosis.yml` plus a row in `pga.db`). `--dry-run`
discovers and prints the plan only. Report: `~/osmosis-setup/games-report.{json,txt}`.

## Safety promises

These are enforced in the code, not just promised:

- **The scanner is read-only.** It writes only its own output files
  (`manifest.json`, `report.txt`) inside `--out`. It never deletes, moves,
  renames, or modifies anything else, makes no network calls, and never
  requires elevation.
- **The stager hash-verifies every file** it copies (xxh64 from the manifest).
- **The importer never overwrites conflicts.** A target that already exists
  with different content is recorded as a conflict and left untouched.
- **Setup never reboots on its own.** A required reboot (NVIDIA driver) is only
  flagged in the report and the manual follow-ups list.
- **The games drive is read-only.** Nothing is ever written to it; prefixes,
  Lutris configs, and `pga.db` all live on the home drive.

## Status and honest limitations

OSmosis was built and proven on **one real Windows -> Linux Mint Cinnamon
switch** (65,288 files imported and hash-verified, games playable in Lutris).
Live verification: Mint only. Ubuntu/Debian/Fedora/Arch verified by fixture
tests only — NOT run on real installs of those distros.
The games step ships a fixed built-in table of 5 games and is the most
personal, least general part of the tool — extending it to arbitrary games is
future work. A desktop GUI (`osmosis-gui`, downloadable above) wraps the same
commands as a click-through wizard; the CLI remains the source of truth (see
`docs/UI_PLAN.md`). There is no dual-boot support and no automatic
partitioning. The preflight verdicts come from a curated built-in table, not a
live database: "unknown" means "check manually", not "safe".

## Development

```
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
