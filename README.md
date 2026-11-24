# OSmosis

**Move from Windows to Linux, without losing anything.**

OSmosis is an open-source (GPL-3.0) tool written in Rust that walks you through
the whole switch: it scans your Windows profile, copies what matters to an
external drive (hash-verified), restores it onto a fresh Linux Mint install,
turns that install into your working machine, and registers your standalone
games in Lutris so they are ready to click.

## Download

Grab the prebuilt binaries from the
[GitHub Releases page](https://github.com/mateir0/osmosis/releases) (the
`latest` prerelease, rebuilt from `master` on every push):

- **Windows** (for the scan + stage steps): `osmosis-windows-x86_64.exe`
- **Linux** (for the import + setup + games steps): `osmosis-linux-x86_64`

Or build from source with `cargo build --release` (see Development below).

## The journey

Six steps, in order. The first two run on Windows, the last four on the fresh
Linux Mint machine.

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
folders.

### 2. Stage to the external drive (Windows)

```powershell
osmosis-windows-x86_64.exe stage --manifest .\osmosis-output\manifest.json --staging E:\
```

Copies every keeper to `<staging>/osmosis-stage/`, hash-verifying each file as
it goes, and is resumable — re-running skips what is already staged and correct.

### 3. Install Linux Mint

This is the one step OSmosis cannot do for you: create a Linux Mint Cinnamon
USB installer and install it on the target machine, replacing Windows. OSmosis
never repartitions or touches your disks.

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

Idempotent step runner that turns the fresh install into the working machine:
base apt packages + codecs, NVIDIA driver (when a GPU is present), GitHub CLI,
Node via nvm + pnpm, Rust via rustup, VS Code, Google Chrome, OpenCode, and
Lutris + Wine-GE. Runs unprivileged and escalates per command with `sudo`
(`DEBIAN_FRONTEND=noninteractive` is passed through so nothing can block on an
interactive prompt). `--dry-run` prints the plan (would-run / already-done /
not-applicable) and changes nothing. Report:
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
The target distribution is Linux Mint Cinnamon; other distros are untested.
The games step ships a fixed built-in table of 5 games and is the most
personal, least general part of the tool — extending it to arbitrary games is
future work. There is no GUI yet (see `docs/UI_PLAN.md` for the plan), no
dual-boot support, and no automatic partitioning.

## Development

```
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
