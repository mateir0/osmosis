# OSmosis

**Move from Windows to Linux, without losing anything.**

OSmosis is an open-source (GPL-3.0) Windows-to-Linux migration tool written in Rust.
It helps a user move **all** their files from Windows to Linux Mint Cinnamon in three
stages:

1. **Scanner (Windows side)** — walks the user profile, classifies every file
   (KEEP / RECREATE / SKIP), hashes keepers, detects git repos, and writes a
   `manifest.json` + human-readable `report.txt`.
2. **Copier (later slice)** — stages the keepers to an external drive.
3. **Importer (later slice, Linux side)** — restores them to the right places.

Status: Slice 1 — scanner

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

## Development

```
cargo build
cargo test
cargo clippy -- -D warnings
cargo fmt --check
```

## License

GPL-3.0-or-later — see [LICENSE](LICENSE).
