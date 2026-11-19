# OSmosis

**Move from Windows to Linux, without losing anything.**

OSmosis is an open-source (GPL-3.0) Windows-to-Linux migration tool written in Rust.
It helps a user move **all** their files from Windows to Linux Mint Cinnamon, then
turn that fresh install into a working machine:

1. **Scanner (Windows side)** — walks the user profile, classifies every file
   (KEEP / RECREATE / SKIP), hashes keepers, detects git repos, and writes a
   `manifest.json` + human-readable `report.txt`.
2. **Stager** — hash-verified, resumable copy of the keepers to the external drive.
3. **Importer (Linux side)** — restores them to the right places with a permission plan.
4. **Setup (Linux side)** — idempotent step runner: apt base + codecs, NVIDIA driver,
   GitHub CLI, nvm/pnpm, rustup, VS Code, Chrome, OpenCode, Lutris + Wine-GE.
5. **Games (Linux side)** — finds the standalone games on the external drive, builds
   one Wine prefix per game, installs its winetricks requirements, and registers
   ready-to-click entries in Lutris. The games drive is read-only.

Status: Slices 1–5 implemented

## What Slice 1 does

The scanner is **read-only and safe above everything**: it reads files and writes
**only** its own output files (`manifest.json`, `report.txt`) inside the `--out`
directory. It never deletes, moves, renames, or modifies anything else, makes no
network calls, and never requires elevation.

Classification precedence (per file):

1. Explicit KEEP mappings (e.g. `Documents` -> `~/Documents`, `.ssh` -> `~/.ssh`).
2. RECREATE directory matches (`node_modules`, `.next`, `target`, ...) — pruned as
   one aggregate entry, never descended into.
3. SKIP rules (everything under `AppData` except the explicit KEEP mapping,
   `NTUSER.DAT*`, `ntuser.ini`, `Thumbs.db`, `desktop.ini`, `*.tmp`, `*.sys`).
4. Default: KEEP only if under a mapped top-level folder, otherwise SKIP.

Git repos (any directory containing `.git`) are recorded with branch, dirty file
count, and unpushed-commit count. KEEP files up to 256 MB are hashed with xxh64.

## How to run the scanner

```powershell
# scan the current user's profile (default --root = %USERPROFILE%)
cargo run --release -- scan

# explicit options
cargo run --release -- scan --root C:\Users\You --out ./osmosis-output --staging E:\ --config config.example.toml
```

CLI shape:

```
osmosis scan [--root <dir>] [--config <file>] [--out <dir>] [--staging <path>]
```

- `--root` — directory to scan. Defaults to the `USERPROFILE` environment variable;
  the command errors out clearly if neither is available.
- `--config` — optional TOML config overriding the embedded defaults
  (see `config.example.toml`).
- `--out` — output directory (default `./osmosis-output`). The only place the
  program ever writes: `manifest.json` and `report.txt`.
- `--staging` — optional path on the external drive; when given, the report adds a
  fit check ("Keepers: X GB / Staging free: Y GB -> FITS / DOES NOT FIT").

## How to run setup (Linux)

```bash
# plan only: prints every step, changes nothing
cargo run --release -- setup --dry-run

# real run on the fresh Mint machine (no sudo up front; escalates per command)
cargo run --release -- setup
```

Report: `~/osmosis-setup/setup-report.{json,txt}`.

## How to run games (Linux)

```bash
# discover the games on the mounted drive and print the plan
cargo run --release -- games --games-root /media/<user>/<drive> --dry-run

# build prefixes, install requirements, register in Lutris
cargo run --release -- games --games-root /media/<user>/<drive>
```

Report: `~/osmosis-setup/games-report.{json,txt}`. Prefixes live in
`~/.local/share/osmosis/prefixes/<slug>`; Lutris entries are
`~/.config/lutris/games/<slug>-osmosis.yml` plus rows in `pga.db` whose slug ends
in `-osmosis`. Nothing is ever written to the games drive.

## Development

```
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
