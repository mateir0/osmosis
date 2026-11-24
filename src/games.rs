//! Slice 5: `osmosis games` — turn hashir's cracked standalone games on the
//! external NTFS drive into ready-to-click Lutris entries.
//!
//! The games drive is READ-ONLY for this tool: nothing is ever written there.
//! Everything this slice creates lives on the home drive:
//!   ~/.local/share/osmosis/prefixes/<slug>     (one Wine prefix per game)
//!   ~/.config/lutris/games/<slug>-osmosis.yml  (per-game Lutris config)
//!   ~/.local/share/lutris/pga.db               (only rows whose slug ends
//!                                               in "-osmosis" are touched)
//!
//! All external commands go through the slice-4 `Runner` as fixed argv arrays.
//! Prefix environment (WINEPREFIX / WINEARCH / WINE) is passed with the fixed
//! `env` binary, never through a shell.
//!
//! HARD RULES enforced here:
//!   * the winetricks verb "sandbox" never appears in any verb list;
//!   * the prefix Documents symlink is never removed or replaced (Wine maps it
//!     to ~/Documents, which is how FIFA 22 and RDR1 find their saves);
//!   * pga.db rows we do not own (slug not ending in "-osmosis") are never
//!     deleted or modified;
//!   * nothing is launched — registration only.

use crate::setup::{DownloadRecord, Runner, SystemRunner, SETUP_DIR_NAME};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub const PREFIX_ROOT: &str = ".local/share/osmosis/prefixes";
const LUTRIS_GAMES_DIR: &str = ".config/lutris/games";
const PGA_DB: &str = ".local/share/lutris/pga.db";
const RUNNERS_DIR: &str = ".local/share/lutris/runners/wine";
const TLAUNCHER_URL: &str = "https://tlauncher.org/jar";
const TLAUNCHER_JAR: &str = "Games/minecraft/TLauncher.jar";
const SLUG_SUFFIX: &str = "-osmosis";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameKind {
    /// Plain .exe folder on the games drive.
    Wine,
    /// FitGirl-style installer repack in ~/Downloads.
    Installer,
    /// Native application (no Wine).
    Native,
}

#[derive(Debug, Clone, Copy)]
pub struct GameDef {
    pub slug: &'static str,
    pub name: &'static str,
    pub kind: GameKind,
    pub folders: &'static [&'static str],
    pub exes: &'static [&'static str],
    pub verbs: &'static [&'static str],
}

/// The game table (data-driven so a later slice can externalise it).
pub const GAMES: [GameDef; 5] = [
    GameDef {
        slug: "fifa-22",
        name: "FIFA 22",
        kind: GameKind::Wine,
        folders: &["FIFA 22"],
        exes: &["FIFA22.exe"],
        verbs: &["vcrun2019", "d3dcompiler_47"],
    },
    GameDef {
        slug: "rdr1",
        name: "Red Dead Redemption",
        kind: GameKind::Wine,
        folders: &["Red Dead Redemption", "RDR1", "Red Dead Redemption 1"],
        exes: &["RDR.exe", "PlayRDR.exe"],
        verbs: &["vcrun2019", "d3dcompiler_47", "d3dx11_43"],
    },
    GameDef {
        slug: "watch-dogs",
        name: "Watch Dogs",
        kind: GameKind::Wine,
        folders: &["Watch Dogs", "Watch_Dogs", "Watch Dogs 1"],
        exes: &["WatchDogs.exe"],
        verbs: &["vcrun2010", "vcrun2012", "d3dcompiler_47"],
    },
    GameDef {
        slug: "batman-arkham-asylum",
        name: "Batman: Arkham Asylum",
        kind: GameKind::Installer,
        folders: &[],
        exes: &[],
        verbs: &["physx", "d3dx9_43", "vcrun2008"],
    },
    GameDef {
        slug: "minecraft",
        name: "Minecraft",
        kind: GameKind::Native,
        folders: &[],
        exes: &[],
        verbs: &[],
    },
];

pub fn games_table() -> &'static [GameDef] {
    &GAMES
}

/// The one verb that must never be used: it removes the prefix Documents
/// symlink and would hide imported saves from FIFA 22 / RDR1.
fn verb_is_forbidden(verb: &str) -> bool {
    verb == "sandbox"
}

fn check_verb_table() -> Result<(), String> {
    for def in &GAMES {
        if let Some(v) = def.verbs.iter().find(|v| verb_is_forbidden(v)) {
            return Err(format!(
                "internal error: game {} uses the forbidden winetricks verb '{}'",
                def.slug, v
            ));
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ report --

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameReport {
    pub slug: String,
    pub name: String,
    /// ready | installer-pending | degraded | not-found | deferred-registration
    /// | planned (dry-run) | failed
    pub status: String,
    pub detail: String,
    pub exe: Option<String>,
    pub folder: Option<String>,
    pub prefix: Option<String>,
    pub runner: String,
    pub failed_verbs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GamesReport {
    pub schema_version: u32,
    pub created_at: String,
    pub home: String,
    pub games_root: String,
    pub os: String,
    pub dry_run: bool,
    pub runner: String,
    pub wine: String,
    pub total_games: u64,
    pub ready: u64,
    pub installer_pending: u64,
    pub degraded: u64,
    pub not_found: u64,
    pub deferred: u64,
    pub planned: u64,
    pub failed: u64,
    pub games: Vec<GameReport>,
    pub downloads: Vec<DownloadRecord>,
    pub manual_followups: Vec<String>,
}

#[derive(Debug)]
pub struct GamesOutcome {
    pub report: GamesReport,
    pub summary: String,
    /// 0 = every found game is ready/installer-pending; 2 = degraded/failed.
    pub exit_code: u8,
}

// ------------------------------------------------------------------- engine --

struct Ctx<'a> {
    home: &'a Path,
    games_root: &'a Path,
    work: &'a Path,
    wine: &'a Path,
    runner_name: &'a str,
    downloads: Vec<DownloadRecord>,
    followups: Vec<String>,
    registration_deferred: bool,
    any_wine_game: bool,
}

struct Found<'a> {
    exe: Option<&'a Path>,
    folder: Option<&'a Path>,
    prefix: Option<&'a Path>,
}

fn game_report(
    def: &GameDef,
    ctx: &Ctx,
    status: &str,
    detail: String,
    found: Found,
    failed_verbs: Vec<String>,
) -> GameReport {
    GameReport {
        slug: def.slug.to_string(),
        name: def.name.to_string(),
        status: status.to_string(),
        detail,
        exe: found.exe.map(|p| p.to_string_lossy().into_owned()),
        folder: found.folder.map(|p| p.to_string_lossy().into_owned()),
        prefix: found.prefix.map(|p| p.to_string_lossy().into_owned()),
        runner: ctx.runner_name.to_string(),
        failed_verbs,
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

fn version_of(r: &dyn Runner, argv: &[&str]) -> Option<String> {
    match r.output(argv) {
        Ok(o) if o.status == 0 => {
            let line = first_line(&o.stdout);
            if line.is_empty() {
                None
            } else {
                Some(line)
            }
        }
        _ => None,
    }
}

/// Save `url` to `dest` with curl and record its sha256 before use. Same
/// discipline as slice 4: no piping of remote content into a shell.
fn fetch(
    r: &dyn Runner,
    downloads: &mut Vec<DownloadRecord>,
    step: &str,
    url: &str,
    dest: &Path,
) -> Result<(), String> {
    let d = dest.to_string_lossy().into_owned();
    r.run(&["curl", "-fsSL", "--retry", "3", "-o", &d, url])?;
    let sum = r
        .output(&["sha256sum", &d])
        .map_err(|e| format!("cannot hash download {}: {}", url, e))?;
    let sha = sum
        .stdout
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if sha.len() != 64 {
        return Err(format!(
            "cannot record sha256 for {} (sha256sum output: {:?})",
            url,
            sum.stdout.trim()
        ));
    }
    downloads.push(DownloadRecord {
        step: step.to_string(),
        url: url.to_string(),
        sha256: sha,
        path: d,
    });
    Ok(())
}

// -------------------------------------------------------------- discovery ---

fn sorted_dirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

fn name_matches(path: &Path, candidates: &[&str]) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .map(|n| candidates.iter().any(|c| n.eq_ignore_ascii_case(c)))
        .unwrap_or(false)
}

/// Look for a game folder at the games root itself or up to two levels down,
/// case-insensitively.
fn find_game_folder(root: &Path, candidates: &[&str]) -> Option<PathBuf> {
    if name_matches(root, candidates) {
        return Some(root.to_path_buf());
    }
    for level1 in sorted_dirs(root) {
        if name_matches(&level1, candidates) {
            return Some(level1);
        }
        for level2 in sorted_dirs(&level1) {
            if name_matches(&level2, candidates) {
                return Some(level2);
            }
        }
    }
    None
}

/// Every regular file below `dir`, sorted shallowest-first then by path so the
/// choice is deterministic.
fn files_below(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    v.sort_by(|a, b| {
        a.components()
            .count()
            .cmp(&b.components().count())
            .then_with(|| a.cmp(b))
    });
    v
}

fn find_file_named(dir: &Path, name: &str) -> Option<PathBuf> {
    files_below(dir).into_iter().find(|p| {
        p.file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.eq_ignore_ascii_case(name))
            .unwrap_or(false)
    })
}

/// First exe candidate (in table order) that exists anywhere under the folder.
fn find_exe(folder: &Path, candidates: &[&str]) -> Option<PathBuf> {
    let files = files_below(folder);
    for cand in candidates {
        if let Some(hit) = files.iter().find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case(cand))
                .unwrap_or(false)
        }) {
            return Some(hit.clone());
        }
    }
    None
}

/// Batman is not installed: the FitGirl repack lives in the imported Downloads.
fn find_batman_setup(home: &Path) -> Option<PathBuf> {
    for dir in sorted_dirs(&home.join("Downloads")) {
        let lower = dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|n| n.to_ascii_lowercase())
            .unwrap_or_default();
        if lower.contains("batman") && lower.contains("arkham") {
            if let Some(exe) = find_file_named(&dir, "setup.exe") {
                return Some(exe);
            }
        }
    }
    None
}

fn find_installed_batman(prefix: &Path) -> Option<PathBuf> {
    find_file_named(&prefix.join("drive_c"), "ShippingPC-BmGame.exe")
}

// ------------------------------------------------------------------ prefix --

fn prefix_path(home: &Path, slug: &str) -> PathBuf {
    home.join(PREFIX_ROOT).join(slug)
}

fn sibling(wine: &Path, bin: &str) -> PathBuf {
    match wine.parent() {
        Some(p) => p.join(bin),
        None => PathBuf::from(bin),
    }
}

fn prefix_env(prefix: &Path) -> String {
    format!("WINEPREFIX={}", prefix.to_string_lossy())
}

/// Idempotent: an existing prefix with system.reg is left untouched. A partial
/// prefix (no system.reg) is retried in place — resumable state, never deleted.
fn ensure_prefix(r: &dyn Runner, prefix: &Path, wine: &Path) -> Result<(), String> {
    if prefix.join("system.reg").is_file() {
        return Ok(());
    }
    fs::create_dir_all(prefix)
        .map_err(|e| format!("cannot create prefix {}: {}", prefix.display(), e))?;
    let envp = prefix_env(prefix);
    let wineboot = sibling(wine, "wineboot").to_string_lossy().into_owned();
    r.run(&["env", &envp, "WINEARCH=win64", &wineboot, "--init"])?;
    let wineserver = sibling(wine, "wineserver").to_string_lossy().into_owned();
    r.run(&["env", &envp, &wineserver, "-w"])?;
    Ok(())
}

/// Install each winetricks verb into the prefix. Failures are collected so the
/// game is registered as degraded instead of aborting the run.
fn install_verbs(r: &dyn Runner, prefix: &Path, wine: &Path, verbs: &[&str]) -> Vec<String> {
    let mut failed = Vec::new();
    let envp = prefix_env(prefix);
    let wine_env = format!("WINE={}", wine.to_string_lossy());
    for verb in verbs {
        if verb_is_forbidden(verb) {
            failed.push(format!("{}: forbidden verb, skipped", verb));
            continue;
        }
        if let Err(e) = r.run(&["env", &envp, &wine_env, "winetricks", "-q", verb]) {
            failed.push(format!("{}: {}", verb, e));
        }
    }
    failed
}

// ------------------------------------------------------------ registration --

enum Reg {
    Registered,
    Deferred(String),
    Refused(String),
}

enum PgaError {
    Refused(String),
    Db(String),
}

fn yaml_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn render_yaml(def: &GameDef, exe: &Path, prefix: &Path, runner_name: &str) -> String {
    // Lutris reads exe/prefix/arch from the nested `game:` section; at the top
    // level they are ignored and the entry shows "no executable set".
    format!(
        "name: {name}\ngame_slug: {slug}\nrunner: wine\ngame:\n  exe: {exe}\n  prefix: {prefix}\n  arch: win64\nwine:\n  version: {version}\n  dxvk: true\n  esync: true\n",
        name = yaml_quote(def.name),
        slug = yaml_quote(&format!("{}{}", def.slug, SLUG_SUFFIX)),
        exe = yaml_quote(&exe.to_string_lossy()),
        prefix = yaml_quote(&prefix.to_string_lossy()),
        version = yaml_quote(runner_name),
    )
}

fn db_err(e: rusqlite::Error) -> PgaError {
    PgaError::Db(format!("pga.db: {}", e))
}

/// Insert or update our own row. Never touches a row whose slug does not end
/// in "-osmosis"; refuses cleanly on schema drift.
fn write_pga_row(
    db: &Path,
    def: &GameDef,
    exe: &Path,
    folder: &Path,
    config: &str,
) -> Result<(), PgaError> {
    let mut conn = Connection::open(db)
        .map_err(|e| PgaError::Db(format!("cannot open {}: {}", db.display(), e)))?;
    let columns: Vec<String> = {
        let mut stmt = conn.prepare("PRAGMA table_info(games)").map_err(db_err)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(db_err)?;
        rows.filter_map(Result::ok).collect()
    };
    let has = |name: &str| columns.iter().any(|c| c == name);
    let exe_col = if has("executable") {
        "executable"
    } else if has("exe") {
        "exe"
    } else {
        return Err(PgaError::Refused(
            "games table has neither an 'executable' nor an 'exe' column".to_string(),
        ));
    };
    for required in [
        "name",
        "slug",
        "platform",
        "runner",
        "directory",
        "installed",
        "configpath",
    ] {
        if !has(required) {
            return Err(PgaError::Refused(format!(
                "games table is missing the '{}' column",
                required
            )));
        }
    }

    let slug = format!("{}{}", def.slug, SLUG_SUFFIX);
    let exe_s = exe.to_string_lossy().into_owned();
    let dir_s = folder.to_string_lossy().into_owned();
    let tx = conn.transaction().map_err(db_err)?;
    let updated = tx
        .execute(
            &format!(
                "UPDATE games SET name = ?1, platform = ?2, runner = ?3, {} = ?4, \
                 directory = ?5, installed = 1, configpath = ?6 WHERE slug = ?7",
                exe_col
            ),
            rusqlite::params![def.name, "Windows", "wine", exe_s, dir_s, config, slug],
        )
        .map_err(db_err)?;
    if updated == 0 {
        tx.execute(
            &format!(
                "INSERT INTO games (name, slug, platform, runner, {}, directory, installed, configpath) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
                exe_col
            ),
            rusqlite::params![def.name, slug, "Windows", "wine", exe_s, dir_s, config],
        )
        .map_err(db_err)?;
    }
    tx.commit().map_err(db_err)?;
    Ok(())
}

fn register(
    ctx: &mut Ctx,
    def: &GameDef,
    exe: &Path,
    folder: &Path,
    prefix: &Path,
) -> Result<Reg, String> {
    let file = format!("{}{}.yml", def.slug, SLUG_SUFFIX);
    // Lutris appends ".yml" itself when resolving `configpath`, so the value
    // stored in pga.db must NOT carry the extension (else it looks for
    // "<slug>-osmosis.yml.yml" and launches an empty config).
    let config = format!("{}{}", def.slug, SLUG_SUFFIX);
    let games_dir = ctx.home.join(LUTRIS_GAMES_DIR);
    fs::create_dir_all(&games_dir)
        .map_err(|e| format!("cannot create {}: {}", games_dir.display(), e))?;
    let yaml = render_yaml(def, exe, prefix, ctx.runner_name);
    let yaml_path = games_dir.join(&file);
    fs::write(&yaml_path, yaml)
        .map_err(|e| format!("cannot write {}: {}", yaml_path.display(), e))?;

    let db = ctx.home.join(PGA_DB);
    if !db.is_file() {
        ctx.registration_deferred = true;
        return Ok(Reg::Deferred(
            "Lutris pga.db not found (Lutris has never been launched)".to_string(),
        ));
    }
    match write_pga_row(&db, def, exe, folder, &config) {
        Ok(()) => Ok(Reg::Registered),
        Err(PgaError::Refused(msg)) => {
            ctx.registration_deferred = true;
            Ok(Reg::Refused(msg))
        }
        Err(PgaError::Db(msg)) => Err(msg),
    }
}

// -------------------------------------------------------------------- games --

fn not_found(def: &GameDef, ctx: &Ctx, detail: String) -> GameReport {
    game_report(
        def,
        ctx,
        "not-found",
        detail,
        Found {
            exe: None,
            folder: None,
            prefix: None,
        },
        Vec::new(),
    )
}

fn process_wine_game(r: &dyn Runner, ctx: &mut Ctx, def: &GameDef, dry_run: bool) -> GameReport {
    let folder = match find_game_folder(ctx.games_root, def.folders) {
        Some(f) => f,
        None => {
            return not_found(
                def,
                ctx,
                format!(
                    "no folder matching {:?} within 2 levels of the games root",
                    def.folders
                ),
            )
        }
    };
    let exe = match find_exe(&folder, def.exes) {
        Some(e) => e,
        None => {
            return not_found(
                def,
                ctx,
                format!(
                    "folder {} found but none of {:?} exists inside it",
                    folder.display(),
                    def.exes
                ),
            )
        }
    };
    let prefix = prefix_path(ctx.home, def.slug);
    ctx.any_wine_game = true;

    if dry_run {
        let db_note = if ctx.home.join(PGA_DB).is_file() {
            "would register in Lutris"
        } else {
            "registration would be deferred (pga.db absent)"
        };
        return game_report(
            def,
            ctx,
            "planned",
            format!(
                "would build prefix {} and install {} requirement(s); {}",
                prefix.display(),
                def.verbs.len(),
                db_note
            ),
            Found {
                exe: Some(&exe),
                folder: Some(&folder),
                prefix: Some(&prefix),
            },
            Vec::new(),
        );
    }

    if let Err(e) = ensure_prefix(r, &prefix, ctx.wine) {
        return game_report(
            def,
            ctx,
            "failed",
            format!("prefix build failed: {}", e),
            Found {
                exe: Some(&exe),
                folder: Some(&folder),
                prefix: Some(&prefix),
            },
            Vec::new(),
        );
    }
    let failed_verbs = install_verbs(r, &prefix, ctx.wine, def.verbs);
    let reg = match register(ctx, def, &exe, &folder, &prefix) {
        Ok(reg) => reg,
        Err(e) => {
            return game_report(
                def,
                ctx,
                "failed",
                format!("prefix built but registration failed: {}", e),
                Found {
                    exe: Some(&exe),
                    folder: Some(&folder),
                    prefix: Some(&prefix),
                },
                failed_verbs,
            )
        }
    };

    let mut detail = if failed_verbs.is_empty() {
        format!("prefix ready; {} requirement(s) installed", def.verbs.len())
    } else {
        format!("degraded: {} requirement(s) failed", failed_verbs.len())
    };
    let status = if !failed_verbs.is_empty() {
        "degraded"
    } else {
        match &reg {
            Reg::Registered => "ready",
            Reg::Deferred(_) | Reg::Refused(_) => "deferred-registration",
        }
    };
    match reg {
        Reg::Registered => detail.push_str("; registered in Lutris"),
        Reg::Deferred(msg) => detail.push_str(&format!("; registration deferred: {}", msg)),
        Reg::Refused(msg) => detail.push_str(&format!("; registration refused: {}", msg)),
    }

    game_report(
        def,
        ctx,
        status,
        detail,
        Found {
            exe: Some(&exe),
            folder: Some(&folder),
            prefix: Some(&prefix),
        },
        failed_verbs,
    )
}

fn process_batman(r: &dyn Runner, ctx: &mut Ctx, def: &GameDef, dry_run: bool) -> GameReport {
    let setup = match find_batman_setup(ctx.home) {
        Some(p) => p,
        None => {
            return not_found(
                def,
                ctx,
                "no Batman FitGirl repack (setup.exe) found under ~/Downloads".to_string(),
            )
        }
    };
    let folder = setup.parent().map(Path::to_path_buf).unwrap_or_default();
    let prefix = prefix_path(ctx.home, def.slug);
    ctx.any_wine_game = true;

    if dry_run {
        return game_report(
            def,
            ctx,
            "planned",
            format!(
                "would build prefix {} and register the repack installer; {} requirement(s)",
                prefix.display(),
                def.verbs.len()
            ),
            Found {
                exe: Some(&setup),
                folder: Some(&folder),
                prefix: Some(&prefix),
            },
            Vec::new(),
        );
    }

    if let Err(e) = ensure_prefix(r, &prefix, ctx.wine) {
        return game_report(
            def,
            ctx,
            "failed",
            format!("prefix build failed: {}", e),
            Found {
                exe: Some(&setup),
                folder: Some(&folder),
                prefix: Some(&prefix),
            },
            Vec::new(),
        );
    }
    let failed_verbs = install_verbs(r, &prefix, ctx.wine, def.verbs);

    // Completion pass: the real game exe appears once the installer has run.
    let installed = find_installed_batman(&prefix);
    let (exe, pending) = match &installed {
        Some(exe) => (exe.clone(), false),
        None => (setup.clone(), true),
    };

    let reg = match register(ctx, def, &exe, &folder, &prefix) {
        Ok(reg) => reg,
        Err(e) => {
            return game_report(
                def,
                ctx,
                "failed",
                format!("prefix built but registration failed: {}", e),
                Found {
                    exe: Some(&exe),
                    folder: Some(&folder),
                    prefix: Some(&prefix),
                },
                failed_verbs,
            )
        }
    };

    let mut detail = if pending {
        format!(
            "installer pending: Lutris launches {} into the prefix",
            setup.display()
        )
    } else {
        format!("installed game detected at {}", exe.display())
    };
    if !failed_verbs.is_empty() {
        detail.push_str(&format!(
            "; degraded: {} requirement(s) failed",
            failed_verbs.len()
        ));
    }
    let status = if !failed_verbs.is_empty() {
        "degraded"
    } else {
        match &reg {
            Reg::Registered if pending => "installer-pending",
            Reg::Registered => "ready",
            Reg::Deferred(_) | Reg::Refused(_) => "deferred-registration",
        }
    };
    match reg {
        Reg::Registered => detail.push_str("; registered in Lutris"),
        Reg::Deferred(msg) => detail.push_str(&format!("; registration deferred: {}", msg)),
        Reg::Refused(msg) => detail.push_str(&format!("; registration refused: {}", msg)),
    }

    game_report(
        def,
        ctx,
        status,
        detail,
        Found {
            exe: Some(&exe),
            folder: Some(&folder),
            prefix: Some(&prefix),
        },
        failed_verbs,
    )
}

fn process_minecraft(r: &dyn Runner, ctx: &mut Ctx, def: &GameDef, dry_run: bool) -> GameReport {
    // `java -version` writes to stderr, so read both streams and trust status.
    let java = r
        .output(&["java", "-version"])
        .ok()
        .filter(|o| o.status == 0)
        .map(|o| first_line(&o.stderr))
        .filter(|s| !s.is_empty());
    let jar = ctx.home.join(TLAUNCHER_JAR);

    if dry_run {
        let java_note = match &java {
            Some(v) => format!("java present ({})", v),
            None => "would install openjdk-21-jre".to_string(),
        };
        return game_report(
            def,
            ctx,
            "planned",
            format!(
                "would download TLauncher and write a desktop entry; {}",
                java_note
            ),
            Found {
                exe: Some(&jar),
                folder: None,
                prefix: None,
            },
            Vec::new(),
        );
    }

    if java.is_none() {
        if let Err(e) = r.run(&crate::setup::sudo_apt_get(&[
            "install",
            "-y",
            "openjdk-21-jre",
        ])) {
            return game_report(
                def,
                ctx,
                "failed",
                format!("java missing and openjdk-21-jre install failed: {}", e),
                Found {
                    exe: Some(&jar),
                    folder: None,
                    prefix: None,
                },
                Vec::new(),
            );
        }
    }

    let tmp = ctx.work.join("TLauncher.jar");
    if let Err(e) = fetch(r, &mut ctx.downloads, "minecraft", TLAUNCHER_URL, &tmp) {
        ctx.followups.push(format!(
            "Minecraft: TLauncher download failed — re-run later ({})",
            TLAUNCHER_URL
        ));
        return game_report(
            def,
            ctx,
            "failed",
            format!("TLauncher download failed: {} ({})", e, TLAUNCHER_URL),
            Found {
                exe: Some(&jar),
                folder: None,
                prefix: None,
            },
            Vec::new(),
        );
    }
    if let Some(parent) = jar.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            return game_report(
                def,
                ctx,
                "failed",
                format!("cannot create {}: {}", parent.display(), e),
                Found {
                    exe: Some(&jar),
                    folder: None,
                    prefix: None,
                },
                Vec::new(),
            );
        }
    }
    if let Err(e) = fs::copy(&tmp, &jar) {
        return game_report(
            def,
            ctx,
            "failed",
            format!("cannot place {}: {}", jar.display(), e),
            Found {
                exe: Some(&jar),
                folder: None,
                prefix: None,
            },
            Vec::new(),
        );
    }
    let _ = fs::remove_file(&tmp);

    // Desktop entry (never touches ~/.minecraft or ~/.tlauncher).
    let apps = ctx.home.join(".local/share/applications");
    if let Err(e) = fs::create_dir_all(&apps) {
        return game_report(
            def,
            ctx,
            "failed",
            format!("cannot create {}: {}", apps.display(), e),
            Found {
                exe: Some(&jar),
                folder: None,
                prefix: None,
            },
            Vec::new(),
        );
    }
    let desktop = apps.join("minecraft-tlauncher.desktop");
    let body = format!(
        "[Desktop Entry]\nType=Application\nName=Minecraft (TLauncher)\nExec=java -jar {}\nTerminal=false\nCategories=Game;\n",
        jar.to_string_lossy()
    );
    if let Err(e) = fs::write(&desktop, body) {
        return game_report(
            def,
            ctx,
            "failed",
            format!("cannot write {}: {}", desktop.display(), e),
            Found {
                exe: Some(&jar),
                folder: None,
                prefix: None,
            },
            Vec::new(),
        );
    }

    let java_note = match &java {
        Some(v) => format!("java present ({})", v),
        None => "openjdk-21-jre installed".to_string(),
    };
    game_report(
        def,
        ctx,
        "ready",
        format!(
            "TLauncher placed at {}; desktop entry written; {}",
            jar.display(),
            java_note
        ),
        Found {
            exe: Some(&jar),
            folder: None,
            prefix: None,
        },
        Vec::new(),
    )
}

// -------------------------------------------------------------- preconditions --

/// The lexicographically latest runner directory that actually has bin/wine.
fn resolve_runner(r: &dyn Runner, home: &Path) -> Result<(String, PathBuf), String> {
    if version_of(r, &["lutris", "--version"]).is_none() {
        return Err("lutris is not installed — run osmosis setup first".to_string());
    }
    let runners = home.join(RUNNERS_DIR);
    let mut usable: Vec<(String, PathBuf)> = sorted_dirs(&runners)
        .into_iter()
        .filter_map(|dir| {
            let name = dir.file_name()?.to_str()?.to_string();
            let wine = dir.join("bin/wine");
            if wine.is_file() {
                Some((name, wine))
            } else {
                None
            }
        })
        .collect();
    usable.sort_by(|a, b| a.0.cmp(&b.0));
    usable.pop().ok_or_else(|| {
        "no Wine runner found under ~/.local/share/lutris/runners/wine — run osmosis setup first"
            .to_string()
    })
}

// ------------------------------------------------------------------- report --

fn now_string() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

struct ReportInput<'a> {
    home: &'a Path,
    games_root: &'a Path,
    dry_run: bool,
    runner: &'a str,
    wine: &'a Path,
    games: Vec<GameReport>,
    downloads: Vec<DownloadRecord>,
    followups: Vec<String>,
    registration_deferred: bool,
    any_wine_game: bool,
}

fn assemble_report(input: ReportInput) -> GamesReport {
    let ReportInput {
        home,
        games_root,
        dry_run,
        runner,
        wine,
        games,
        downloads,
        mut followups,
        registration_deferred,
        any_wine_game,
    } = input;
    let count = |s: &str| games.iter().filter(|g| g.status == s).count() as u64;
    if registration_deferred {
        followups.push("launch Lutris once, then re-run osmosis games".to_string());
    }
    let batman_pending = games
        .iter()
        .any(|g| g.slug == "batman-arkham-asylum" && g.status == "installer-pending");
    if batman_pending {
        followups.push("Batman: launch once to run its installer".to_string());
    }
    if any_wine_game {
        followups.push(
            "first launch of each game may take a few minutes while Lutris prepares DXVK"
                .to_string(),
        );
    }

    GamesReport {
        schema_version: 1,
        created_at: now_string(),
        home: home.to_string_lossy().into_owned(),
        games_root: games_root.to_string_lossy().into_owned(),
        os: std::env::consts::OS.to_string(),
        dry_run,
        runner: runner.to_string(),
        wine: wine.to_string_lossy().into_owned(),
        total_games: games.len() as u64,
        ready: count("ready"),
        installer_pending: count("installer-pending"),
        degraded: count("degraded"),
        not_found: count("not-found"),
        deferred: count("deferred-registration"),
        planned: count("planned"),
        failed: count("failed"),
        games,
        downloads,
        manual_followups: followups,
    }
}

pub fn build_games_report_txt(r: &GamesReport) -> String {
    let mut s = String::new();
    s.push_str("OSmosis games report\n");
    s.push_str("====================\n");
    s.push_str(&format!("home: {}\n", r.home));
    s.push_str(&format!("games_root: {}\n", r.games_root));
    s.push_str(&format!("os: {}\n", r.os));
    s.push_str(&format!("created_at: {}\n", r.created_at));
    s.push_str(&format!("dry_run: {}\n", r.dry_run));
    s.push_str(&format!("runner: {}\n", r.runner));
    s.push_str(&format!("wine: {}\n\n", r.wine));

    s.push_str("Totals\n");
    s.push_str("------\n");
    s.push_str(&format!("total_games:      {}\n", r.total_games));
    s.push_str(&format!("ready:            {}\n", r.ready));
    s.push_str(&format!("installer_pending:{}\n", r.installer_pending));
    s.push_str(&format!("degraded:         {}\n", r.degraded));
    s.push_str(&format!("not_found:        {}\n", r.not_found));
    s.push_str(&format!("deferred:         {}\n", r.deferred));
    if r.dry_run {
        s.push_str(&format!("planned:          {}\n", r.planned));
    }
    s.push_str(&format!("failed:           {}\n\n", r.failed));

    s.push_str("GAMES\n");
    s.push_str("-----\n");
    for g in &r.games {
        s.push_str(&format!(
            "[{}] {} — {}: {}\n",
            g.status, g.slug, g.name, g.detail
        ));
        if let Some(exe) = &g.exe {
            s.push_str(&format!("  exe:    {}\n", exe));
        }
        if let Some(prefix) = &g.prefix {
            s.push_str(&format!("  prefix: {}\n", prefix));
        }
        if !g.failed_verbs.is_empty() {
            s.push_str(&format!("  failed: {}\n", g.failed_verbs.join(", ")));
        }
    }
    s.push('\n');

    s.push_str("DOWNLOADS\n");
    s.push_str("---------\n");
    if r.downloads.is_empty() {
        s.push_str("(none)\n");
    }
    for d in &r.downloads {
        s.push_str(&format!(
            "- {} sha256={} (step {})\n",
            d.url, d.sha256, d.step
        ));
    }
    s.push('\n');

    s.push_str("MANUAL FOLLOW-UPS\n");
    s.push_str("-----------------\n");
    if r.manual_followups.is_empty() {
        s.push_str("(none)\n");
    }
    for f in &r.manual_followups {
        s.push_str(&format!("- {}\n", f));
    }
    s
}

fn write_report_files(dir: &Path, report: &GamesReport, summary: &str) -> Result<(), String> {
    use std::io::Write;
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| format!("cannot serialize games report: {}", e))?;
    let jp = dir.join("games-report.json");
    let mut f =
        fs::File::create(&jp).map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    f.write_all(json.as_bytes())
        .and_then(|_| f.write_all(b"\n"))
        .map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    let tp = dir.join("games-report.txt");
    let mut f =
        fs::File::create(&tp).map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;
    f.write_all(summary.as_bytes())
        .map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;
    Ok(())
}

// ------------------------------------------------------------------- entry --

/// Engine shared by the real command and tests. No OS gate here so the whole
/// decision tree stays testable on Windows.
pub fn run_games_with(
    runner: &dyn Runner,
    home: &Path,
    games_root: &Path,
    dry_run: bool,
    work: &Path,
) -> Result<GamesOutcome, String> {
    if !home.is_dir() {
        return Err(format!(
            "target home is not a directory: {}",
            home.display()
        ));
    }
    if !games_root.is_dir() {
        return Err(format!(
            "games root is not a directory: {}",
            games_root.display()
        ));
    }
    check_verb_table()?;
    let (runner_name, wine) = resolve_runner(runner, home)?;

    let report_dir = home.join(SETUP_DIR_NAME);
    fs::create_dir_all(&report_dir).map_err(|e| {
        format!(
            "cannot create {} (is the home writable?): {}",
            report_dir.display(),
            e
        )
    })?;
    fs::create_dir_all(work)
        .map_err(|e| format!("cannot create work dir {}: {}", work.display(), e))?;

    let mut ctx = Ctx {
        home,
        games_root,
        work,
        wine: &wine,
        runner_name: &runner_name,
        downloads: Vec::new(),
        followups: Vec::new(),
        registration_deferred: false,
        any_wine_game: false,
    };

    let mut games = Vec::new();
    for def in &GAMES {
        let report = match def.kind {
            GameKind::Wine => process_wine_game(runner, &mut ctx, def, dry_run),
            GameKind::Installer => process_batman(runner, &mut ctx, def, dry_run),
            GameKind::Native => process_minecraft(runner, &mut ctx, def, dry_run),
        };
        games.push(report);
    }

    let report = assemble_report(ReportInput {
        home,
        games_root,
        dry_run,
        runner: &runner_name,
        wine: &wine,
        games,
        downloads: ctx.downloads.clone(),
        followups: ctx.followups.clone(),
        registration_deferred: ctx.registration_deferred,
        any_wine_game: ctx.any_wine_game,
    });
    let summary = build_games_report_txt(&report);
    write_report_files(&report_dir, &report, &summary)?;
    let _ = fs::remove_dir_all(work);

    let exit_code = if report.degraded > 0 || report.failed > 0 {
        2
    } else {
        0
    };
    Ok(GamesOutcome {
        report,
        summary,
        exit_code,
    })
}

/// `osmosis games --games-root <dir> [--home <dir>] [--dry-run]`: Linux-only.
pub fn run_games(games_root: &Path, home: &Path, dry_run: bool) -> Result<GamesOutcome, String> {
    if !cfg!(target_os = "linux") {
        return Err(format!(
            "osmosis games is Linux-only; this host is {}",
            std::env::consts::OS
        ));
    }
    let runner = SystemRunner;
    let work = std::env::temp_dir().join(format!("osmosis-games-{}", std::process::id()));
    run_games_with(&runner, home, games_root, dry_run, &work)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_game_uses_the_sandbox_verb() {
        for def in games_table() {
            assert!(
                !def.verbs.contains(&"sandbox"),
                "{} must not use the sandbox verb",
                def.slug
            );
        }
        assert!(check_verb_table().is_ok());
        assert!(verb_is_forbidden("sandbox"));
        assert!(!verb_is_forbidden("vcrun2019"));
    }

    #[test]
    fn game_table_has_the_five_expected_slugs() {
        let slugs: Vec<&str> = games_table().iter().map(|g| g.slug).collect();
        assert_eq!(
            slugs,
            vec![
                "fifa-22",
                "rdr1",
                "watch-dogs",
                "batman-arkham-asylum",
                "minecraft"
            ]
        );
    }

    #[test]
    fn yaml_values_are_single_quoted_and_escaped() {
        assert_eq!(yaml_quote("FIFA 22"), "'FIFA 22'");
        assert_eq!(yaml_quote("it's"), "'it''s'");
    }

    #[test]
    fn rendered_yaml_contains_the_required_keys() {
        let def = &games_table()[0];
        let yaml = render_yaml(
            def,
            Path::new("/media/x/FIFA 22/FIFA22.exe"),
            Path::new("/home/u/.local/share/osmosis/prefixes/fifa-22"),
            "GE-Proton8-26",
        );
        for needle in [
            "name: 'FIFA 22'",
            "game_slug: 'fifa-22-osmosis'",
            "runner: wine",
            "  arch: win64",
            "  version: 'GE-Proton8-26'",
            "dxvk: true",
            "esync: true",
        ] {
            assert!(yaml.contains(needle), "missing {:?} in\n{}", needle, yaml);
        }
    }

    #[test]
    fn rendered_yaml_nests_exe_prefix_arch_under_the_game_section() {
        let def = &games_table()[0];
        let exe = Path::new("/media/x/FIFA 22/FIFA22.exe");
        let prefix = Path::new("/home/u/.local/share/osmosis/prefixes/fifa-22");
        let yaml = render_yaml(def, exe, prefix, "GE-Proton8-26");

        // parse the rendered YAML back line by line (no YAML crate in the
        // dependency tree): exe/prefix/arch must live ONLY inside game:
        for line in yaml.lines() {
            assert!(
                !line.starts_with("exe:")
                    && !line.starts_with("prefix:")
                    && !line.starts_with("arch:"),
                "top-level exe/prefix/arch line (Lutris ignores it): {:?}\n{}",
                line,
                yaml
            );
        }
        let game_block = format!(
            "game:\n  exe: {}\n  prefix: {}\n  arch: win64\n",
            yaml_quote(&exe.to_string_lossy()),
            yaml_quote(&prefix.to_string_lossy())
        );
        assert!(
            yaml.contains(&game_block),
            "missing game section {:?} in\n{}",
            game_block,
            yaml
        );
        // the surrounding top-level shape is unchanged
        assert!(yaml
            .starts_with("name: 'FIFA 22'\ngame_slug: 'fifa-22-osmosis'\nrunner: wine\ngame:\n"));
        assert!(yaml.contains("\nwine:\n  version: 'GE-Proton8-26'\n  dxvk: true\n  esync: true\n"));
    }
}
