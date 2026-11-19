//! Integration tests for slice 5 (`osmosis games`).
//!
//! A fake Runner records every argv; the Lutris layout, the games drive, and a
//! real `pga.db` are created inside temp fixture dirs, so the whole decision
//! tree runs on Windows without touching the real system.

use osmosis::games::{run_games_with, GameReport, GamesOutcome, GamesReport};
use osmosis::setup::{CommandOutput, Runner};
use rusqlite::Connection;
use std::cell::RefCell;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const TLAUNCHER_URL: &str = "https://tlauncher.org/jar";

// ---------------------------------------------------------------- fake world --

#[derive(Debug, Clone)]
struct World {
    java: bool,
    lutris: bool,
    fail_verbs: Vec<String>,
    curl_fail: bool,
    apt_ok: bool,
}

impl World {
    fn good() -> World {
        World {
            java: true,
            lutris: true,
            fail_verbs: Vec::new(),
            curl_fail: false,
            apt_ok: true,
        }
    }
}

fn eq(a: &[&str], b: &[&str]) -> bool {
    a == b
}

fn respond(w: &mut World, a: &[&str]) -> CommandOutput {
    if eq(a, &["lutris", "--version"]) {
        return if w.lutris {
            CommandOutput::ok("lutris 0.5.17")
        } else {
            CommandOutput::fail(127, "lutris: command not found")
        };
    }
    if eq(a, &["java", "-version"]) {
        return if w.java {
            CommandOutput {
                status: 0,
                stdout: String::new(),
                stderr: "openjdk version \"21.0.1\" 2023-10-17".to_string(),
            }
        } else {
            CommandOutput::fail(127, "java: command not found")
        };
    }
    if a.first() == Some(&"env") {
        let prefix = a
            .get(1)
            .and_then(|s| s.strip_prefix("WINEPREFIX="))
            .unwrap_or("")
            .to_string();
        if a.iter().any(|x| x.contains("wineboot")) {
            let _ = fs::create_dir_all(&prefix);
            let _ = fs::write(Path::new(&prefix).join("system.reg"), b"WINE REGISTRY");
            return CommandOutput::ok("");
        }
        if a.iter().any(|x| x.contains("wineserver")) {
            return CommandOutput::ok("");
        }
        if a.contains(&"winetricks") {
            let verb = a.last().unwrap_or(&"").to_string();
            if w.fail_verbs.iter().any(|v| v == &verb) {
                return CommandOutput::fail(1, format!("winetricks {} failed", verb));
            }
            return CommandOutput::ok("");
        }
        return CommandOutput::fail(127, format!("env: unhandled argv {:?}", a));
    }
    if a.first() == Some(&"curl") {
        if w.curl_fail {
            return CommandOutput::fail(22, "HTTP 404");
        }
        if let Some(i) = a.iter().position(|x| *x == "-o") {
            if let Some(dest) = a.get(i + 1) {
                let _ = fs::write(dest, b"TLAUNCHER JAR");
            }
        }
        return CommandOutput::ok("");
    }
    if a.first() == Some(&"sha256sum") {
        return CommandOutput::ok(format!(
            "{}  {}\n",
            "b".repeat(64),
            a.get(1).copied().unwrap_or("")
        ));
    }
    if a.starts_with(&["sudo", "apt-get", "install", "-y", "openjdk-21-jre"]) {
        return if w.apt_ok {
            CommandOutput::ok("")
        } else {
            CommandOutput::fail(100, "E: unable to install")
        };
    }
    CommandOutput::fail(127, format!("fake runner: unhandled argv {:?}", a))
}

struct Fake {
    world: RefCell<World>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl Fake {
    fn new(world: World) -> Fake {
        Fake {
            world: RefCell::new(world),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }

    fn ran(&self, argv: &[&str]) -> bool {
        self.calls
            .borrow()
            .iter()
            .any(|c| c.len() == argv.len() && c.iter().zip(argv).all(|(a, b)| a == b))
    }

    fn count_containing(&self, needle: &str) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|c| c.iter().any(|x| x.contains(needle)))
            .count()
    }

    fn verbs(&self) -> Vec<String> {
        self.calls
            .borrow()
            .iter()
            .filter(|c| c.iter().any(|x| x == "winetricks"))
            .map(|c| c.last().cloned().unwrap_or_default())
            .collect()
    }
}

impl Runner for Fake {
    fn output(&self, argv: &[&str]) -> Result<CommandOutput, String> {
        self.calls
            .borrow_mut()
            .push(argv.iter().map(|s| s.to_string()).collect());
        let mut w = self.world.borrow_mut();
        Ok(respond(&mut w, argv))
    }
}

// ------------------------------------------------------------------ fixture --

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "osmosis-games-fixture-{}-{}",
            std::process::id(),
            n
        ));
        fs::create_dir_all(dir.join("home")).unwrap();
        fs::create_dir_all(dir.join("drive")).unwrap();
        Fixture { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    fn drive(&self) -> PathBuf {
        self.dir.join("drive")
    }

    fn work(&self) -> PathBuf {
        self.dir.join("work")
    }

    fn write(&self, path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn touch_home(&self, rel: &str) {
        self.write(&self.home().join(rel), b"fixture");
    }

    fn make_db(&self) {
        let db = self.home().join(".local/share/lutris/pga.db");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE games (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT, slug TEXT, platform TEXT, runner TEXT,
                 executable TEXT, directory TEXT, installed INTEGER,
                 configpath TEXT
             );",
        )
        .unwrap();
    }

    fn db(&self) -> PathBuf {
        self.home().join(".local/share/lutris/pga.db")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn standard_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.write(&fx.drive().join("FIFA 22/FIFA22.exe"), b"MZ fifa");
    fx.write(&fx.drive().join("RDR1/PlayRDR.exe"), b"MZ play");
    // nested deeper, but the first candidate must still win
    fx.write(&fx.drive().join("RDR1/iso/RDR.exe"), b"MZ rdr");
    // the folder is two levels down; the exe is nested inside it
    fx.write(
        &fx.drive().join("Games/Watch Dogs/data/bin/WatchDogs.exe"),
        b"MZ watch",
    );
    fx.write(
        &fx.home()
            .join("Downloads/Batman - Arkham Asylum GotY [FitGirl Repack]/setup.exe"),
        b"MZ setup",
    );
    fx.touch_home(".local/share/lutris/runners/wine/GE-Proton8-26/bin/wine");
    fx.touch_home(".local/share/lutris/runners/wine/wine-9.0/bin/wine");
    fx.touch_home(".minecraft/saves/world/level.dat");
    fx.make_db();
    fx
}

fn run(fx: &Fixture, world: World, dry_run: bool) -> (Fake, GamesOutcome) {
    run_at(fx, &fx.drive(), world, dry_run)
}

fn run_at(fx: &Fixture, root: &Path, world: World, dry_run: bool) -> (Fake, GamesOutcome) {
    let fake = Fake::new(world);
    let out = run_games_with(&fake, &fx.home(), root, dry_run, &fx.work())
        .expect("games must not hard-fail on a valid fixture");
    (fake, out)
}

fn game<'a>(report: &'a GamesReport, slug: &str) -> &'a GameReport {
    report
        .games
        .iter()
        .find(|g| g.slug == slug)
        .unwrap_or_else(|| panic!("missing game {}", slug))
}

fn file_name(p: &Option<String>) -> String {
    p.as_deref()
        .map(|s| {
            Path::new(s)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .unwrap_or_default()
}

#[derive(Debug)]
struct Row {
    name: String,
    slug: String,
    platform: String,
    runner: String,
    exe: String,
    dir: String,
    installed: i64,
    configpath: String,
}

fn read_rows(db: &Path) -> Vec<Row> {
    let conn = Connection::open(db).unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT name, slug, platform, runner, executable, directory, installed, configpath \
             FROM games ORDER BY slug",
        )
        .unwrap();
    let rows = stmt
        .query_map([], |r| {
            Ok(Row {
                name: r.get(0)?,
                slug: r.get(1)?,
                platform: r.get(2)?,
                runner: r.get(3)?,
                exe: r.get(4)?,
                dir: r.get(5)?,
                installed: r.get(6)?,
                configpath: r.get(7)?,
            })
        })
        .unwrap();
    rows.map(Result::unwrap).collect()
}

fn tree(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .map(|e| e.into_path())
        .collect();
    v.sort();
    v
}

// ---------------------------------------------------------------- 5.1 -------

#[test]
fn discovers_every_game_and_picks_the_right_exe() {
    let fx = standard_fixture();
    let (_, out) = run(&fx, World::good(), false);
    let r = &out.report;

    assert_eq!(r.total_games, 5);
    assert_eq!(r.games.len(), 5);
    assert_eq!(
        r.ready
            + r.installer_pending
            + r.degraded
            + r.not_found
            + r.deferred
            + r.planned
            + r.failed,
        r.total_games
    );
    assert_eq!(out.exit_code, 0);

    let fifa = game(r, "fifa-22");
    assert_eq!(fifa.status, "ready");
    assert_eq!(file_name(&fifa.exe), "FIFA22.exe");

    // first candidate wins even when it sits in a deeper folder
    let rdr = game(r, "rdr1");
    assert_eq!(rdr.status, "ready");
    assert_eq!(file_name(&rdr.exe), "RDR.exe");

    // folder found two levels down, exe found recursively inside it
    let watch = game(r, "watch-dogs");
    assert_eq!(watch.status, "ready");
    assert_eq!(file_name(&watch.exe), "WatchDogs.exe");

    assert_eq!(game(r, "batman-arkham-asylum").status, "installer-pending");
    assert_eq!(game(r, "minecraft").status, "ready");

    // lexicographically latest runner wins
    assert_eq!(r.runner, "wine-9.0");
    assert!(r.wine.replace('\\', "/").ends_with("wine-9.0/bin/wine"));
}

#[test]
fn missing_game_is_not_found_and_the_run_continues() {
    let fx = standard_fixture();
    fs::remove_dir_all(fx.drive().join("FIFA 22")).unwrap();
    let (_, out) = run(&fx, World::good(), false);
    let r = &out.report;

    let fifa = game(r, "fifa-22");
    assert_eq!(fifa.status, "not-found");
    assert!(fifa.exe.is_none());
    assert_eq!(game(r, "rdr1").status, "ready");
    assert_eq!(r.not_found, 1);
    assert_eq!(out.exit_code, 0);
}

#[test]
fn game_folder_can_be_the_games_root_itself() {
    let fx = Fixture::new();
    // the games root itself is the game folder
    let root = fx.drive().join("FIFA 22");
    fx.write(&root.join("FIFA22.exe"), b"MZ");
    fx.touch_home(".local/share/lutris/runners/wine/wine-9.0/bin/wine");
    fx.make_db();

    let (_, out) = run_at(&fx, &root, World::good(), false);
    assert_eq!(game(&out.report, "fifa-22").status, "ready");
    assert_eq!(file_name(&game(&out.report, "fifa-22").exe), "FIFA22.exe");
}

// ---------------------------------------------------------------- 5.2 -------

#[test]
fn prefixes_are_built_once_and_reused_on_a_second_run() {
    let fx = standard_fixture();
    let (fake, out) = run(&fx, World::good(), false);
    assert_eq!(out.report.ready, 4);
    // four Wine games, no Minecraft prefix
    assert_eq!(fake.count_containing("wineboot"), 4);

    let (fake2, _) = run(&fx, World::good(), false);
    assert_eq!(
        fake2.count_containing("wineboot"),
        0,
        "existing system.reg must skip creation"
    );
    assert!(fx
        .home()
        .join(".local/share/osmosis/prefixes/fifa-22/system.reg")
        .is_file());
}

#[test]
fn verb_argv_sequence_matches_the_game_table() {
    let fx = standard_fixture();
    let (fake, _) = run(&fx, World::good(), false);

    let verbs = fake.verbs();
    assert_eq!(
        &verbs[0..2],
        &["vcrun2019".to_string(), "d3dcompiler_47".to_string()]
    );
    assert_eq!(
        &verbs[2..5],
        &[
            "vcrun2019".to_string(),
            "d3dcompiler_47".to_string(),
            "d3dx11_43".to_string()
        ]
    );
    assert_eq!(
        &verbs[5..8],
        &[
            "vcrun2010".to_string(),
            "vcrun2012".to_string(),
            "d3dcompiler_47".to_string()
        ]
    );
    assert_eq!(
        &verbs[8..11],
        &[
            "physx".to_string(),
            "d3dx9_43".to_string(),
            "vcrun2008".to_string()
        ]
    );

    // exact argv for the first verb, including the env wrapper
    let prefix = fx
        .home()
        .join(".local/share/osmosis/prefixes")
        .join("fifa-22")
        .to_string_lossy()
        .into_owned();
    let wine = fx
        .home()
        .join(".local/share/lutris/runners/wine")
        .join("wine-9.0")
        .join("bin/wine")
        .to_string_lossy()
        .into_owned();
    assert!(
        fake.ran(&[
            "env",
            &format!("WINEPREFIX={}", prefix),
            &format!("WINE={}", wine),
            "winetricks",
            "-q",
            "vcrun2019"
        ]),
        "calls: {:#?}",
        fake.calls()
    );
}

#[test]
fn a_failed_verb_degrades_only_that_game() {
    let fx = standard_fixture();
    let mut w = World::good();
    w.fail_verbs = vec!["d3dx9_43".to_string()];
    let (_, out) = run(&fx, w, false);
    let r = &out.report;

    let batman = game(r, "batman-arkham-asylum");
    assert_eq!(batman.status, "degraded");
    assert_eq!(batman.failed_verbs.len(), 1);
    assert!(batman.failed_verbs[0].starts_with("d3dx9_43"));
    assert_eq!(game(r, "fifa-22").status, "ready");
    assert_eq!(r.degraded, 1);
    assert_eq!(out.exit_code, 2);
    // still registered despite the degraded requirement
    assert!(read_rows(&fx.db())
        .iter()
        .any(|row| row.slug == "batman-arkham-asylum-osmosis"));
}

#[test]
fn the_sandbox_verb_and_documents_are_never_touched() {
    let fx = standard_fixture();
    let (fake, _) = run(&fx, World::good(), false);
    for call in fake.calls() {
        for arg in &call {
            let lower = arg.to_ascii_lowercase();
            assert!(!lower.contains("sandbox"), "forbidden verb in {:?}", call);
            assert!(
                !arg.contains("Documents"),
                "Documents touched in {:?}",
                call
            );
            assert!(
                !matches!(lower.as_str(), "rm" | "ln" | "unlink" | "rmdir"),
                "destructive command in {:?}",
                call
            );
        }
    }
}

#[test]
fn the_games_drive_is_never_written() {
    let fx = standard_fixture();
    let before = tree(&fx.drive());
    run(&fx, World::good(), false);
    let after = tree(&fx.drive());
    assert_eq!(before, after, "the games drive must stay byte-identical");
}

// ---------------------------------------------------------------- 5.3 -------

#[test]
fn registration_writes_yaml_and_exactly_one_row_per_game() {
    let fx = standard_fixture();
    let (_, out) = run(&fx, World::good(), false);
    assert_eq!(out.exit_code, 0);

    let rows = read_rows(&fx.db());
    assert_eq!(rows.len(), 4, "four Wine games, Minecraft is native");
    for row in &rows {
        assert!(row.slug.ends_with("-osmosis"));
        assert_eq!(row.platform, "Windows");
        assert_eq!(row.runner, "wine");
        assert_eq!(row.installed, 1);
        assert!(row.configpath.ends_with("-osmosis.yml"));
        assert!(Path::new(&row.exe).is_absolute());
        assert!(Path::new(&row.dir).is_absolute());
    }
    let fifa = rows.iter().find(|r| r.slug == "fifa-22-osmosis").unwrap();
    assert_eq!(fifa.name, "FIFA 22");
    assert!(fifa.exe.replace('\\', "/").ends_with("FIFA 22/FIFA22.exe"));
    assert_eq!(fifa.configpath, "fifa-22-osmosis.yml");

    let yaml = fx.home().join(".config/lutris/games/fifa-22-osmosis.yml");
    let text = fs::read_to_string(&yaml).unwrap();
    for needle in [
        "name: 'FIFA 22'",
        "game_slug: 'fifa-22-osmosis'",
        "runner: wine",
        "arch: win64",
        "version: 'wine-9.0'",
        "dxvk: true",
        "esync: true",
    ] {
        assert!(text.contains(needle), "missing {:?} in\n{}", needle, text);
    }
}

#[test]
fn a_second_run_updates_rows_in_place() {
    let fx = standard_fixture();
    run(&fx, World::good(), false);
    let first = read_rows(&fx.db());
    run(&fx, World::good(), false);
    let second = read_rows(&fx.db());

    assert_eq!(first.len(), second.len());
    assert_eq!(first.len(), 4);
}

#[test]
fn rows_we_do_not_own_are_left_alone() {
    let fx = standard_fixture();
    {
        let conn = Connection::open(fx.db()).unwrap();
        conn.execute(
            "INSERT INTO games (name, slug, platform, runner, executable, directory, installed, configpath)
             VALUES ('Foreign', 'some-game', 'Windows', 'wine', '/x/foreign.exe', '/x', 1, 'foreign.yml')",
            [],
        )
        .unwrap();
    }
    run(&fx, World::good(), false);

    let rows = read_rows(&fx.db());
    let foreign = rows.iter().find(|r| r.slug == "some-game").unwrap();
    assert_eq!(foreign.name, "Foreign");
    assert_eq!(foreign.exe, "/x/foreign.exe");
    assert_eq!(foreign.configpath, "foreign.yml");
    assert_eq!(rows.len(), 5);
}

#[test]
fn missing_pga_db_defers_registration_but_keeps_prefixes_and_yamls() {
    let fx = standard_fixture();
    fs::remove_file(fx.db()).unwrap();
    let (_, out) = run(&fx, World::good(), false);
    let r = &out.report;

    assert_eq!(r.deferred, 4);
    assert_eq!(r.ready, 1); // Minecraft
    assert_eq!(out.exit_code, 0);
    assert!(game(r, "fifa-22").detail.contains("registration deferred"));
    assert!(fx
        .home()
        .join(".config/lutris/games/fifa-22-osmosis.yml")
        .is_file());
    assert!(fx
        .home()
        .join(".local/share/osmosis/prefixes/fifa-22/system.reg")
        .is_file());
    assert!(r
        .manual_followups
        .iter()
        .any(|f| f.contains("launch Lutris once")));
}

#[test]
fn schema_drift_defers_cleanly() {
    let fx = standard_fixture();
    {
        let conn = Connection::open(fx.db()).unwrap();
        conn.execute_batch(
            "DROP TABLE games;
             CREATE TABLE games (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 name TEXT, slug TEXT, platform TEXT, runner TEXT,
                 executable TEXT, directory TEXT, installed INTEGER
             );",
        )
        .unwrap();
    }
    let (_, out) = run(&fx, World::good(), false);
    let r = &out.report;

    assert_eq!(r.deferred, 4);
    assert_eq!(out.exit_code, 0);
    assert!(game(r, "fifa-22").detail.contains("registration refused"));
    assert!(game(r, "fifa-22").detail.contains("configpath"));

    let conn = Connection::open(fx.db()).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM games", [], |row| row.get(0))
        .unwrap();
    assert_eq!(count, 0, "nothing may be inserted on schema drift");
}

// ---------------------------------------------------------------- 5.4 -------

#[test]
fn batman_registers_the_installer_before_the_game_is_installed() {
    let fx = standard_fixture();
    let (_, out) = run(&fx, World::good(), false);
    let batman = game(&out.report, "batman-arkham-asylum");

    assert_eq!(batman.status, "installer-pending");
    assert_eq!(file_name(&batman.exe), "setup.exe");
    assert!(out
        .report
        .manual_followups
        .iter()
        .any(|f| f == "Batman: launch once to run its installer"));

    let row = read_rows(&fx.db())
        .into_iter()
        .find(|r| r.slug == "batman-arkham-asylum-osmosis")
        .unwrap();
    assert!(row.exe.replace('\\', "/").ends_with("setup.exe"));
}

#[test]
fn batman_completion_pass_flips_to_the_installed_exe() {
    let fx = standard_fixture();
    let installed = fx
        .home()
        .join(".local/share/osmosis/prefixes/batman-arkham-asylum/drive_c/Games/Batman/ShippingPC-BmGame.exe");
    fx.write(&installed, b"MZ game");
    // an existing prefix is reused
    fx.touch_home(".local/share/osmosis/prefixes/batman-arkham-asylum/system.reg");

    let (fake, out) = run(&fx, World::good(), false);
    let batman = game(&out.report, "batman-arkham-asylum");

    assert_eq!(batman.status, "ready");
    assert_eq!(file_name(&batman.exe), "ShippingPC-BmGame.exe");
    assert!(batman.detail.contains("installed game detected"));
    assert_eq!(fake.count_containing("wineboot"), 3, "batman prefix reused");

    let row = read_rows(&fx.db())
        .into_iter()
        .find(|r| r.slug == "batman-arkham-asylum-osmosis")
        .unwrap();
    assert!(row
        .exe
        .replace('\\', "/")
        .ends_with("ShippingPC-BmGame.exe"));
}

// ---------------------------------------------------------------- 5.5 -------

#[test]
fn minecraft_writes_a_desktop_entry_and_skips_apt_when_java_exists() {
    let fx = standard_fixture();
    let (fake, out) = run(&fx, World::good(), false);
    let mc = game(&out.report, "minecraft");

    assert_eq!(mc.status, "ready");
    assert!(!fake.ran(&["sudo", "apt-get", "install", "-y", "openjdk-21-jre"]));

    let jar = fx.home().join("Games/minecraft/TLauncher.jar");
    assert!(jar.is_file());
    assert_eq!(fs::read(&jar).unwrap(), b"TLAUNCHER JAR");

    let desktop = fx
        .home()
        .join(".local/share/applications/minecraft-tlauncher.desktop");
    let text = fs::read_to_string(&desktop).unwrap();
    assert!(text.contains("Exec=java -jar "));
    assert!(text.contains(&jar.to_string_lossy().into_owned()));

    let dl = out
        .report
        .downloads
        .iter()
        .find(|d| d.url == TLAUNCHER_URL)
        .expect("TLauncher download must be recorded");
    assert_eq!(dl.sha256.len(), 64);

    // imported world data is untouched
    assert_eq!(
        fs::read(fx.home().join(".minecraft/saves/world/level.dat")).unwrap(),
        b"fixture"
    );
}

#[test]
fn minecraft_installs_java_when_it_is_missing() {
    let fx = standard_fixture();
    let mut w = World::good();
    w.java = false;
    let (fake, out) = run(&fx, w, false);

    assert!(fake.ran(&["sudo", "apt-get", "install", "-y", "openjdk-21-jre"]));
    assert_eq!(game(&out.report, "minecraft").status, "ready");
    assert!(game(&out.report, "minecraft")
        .detail
        .contains("openjdk-21-jre installed"));
}

#[test]
fn minecraft_download_failure_records_the_url() {
    let fx = standard_fixture();
    let mut w = World::good();
    w.curl_fail = true;
    let (_, out) = run(&fx, w, false);
    let mc = game(&out.report, "minecraft");

    assert_eq!(mc.status, "failed");
    assert!(mc.detail.contains(TLAUNCHER_URL));
    assert!(out
        .report
        .manual_followups
        .iter()
        .any(|f| f.contains(TLAUNCHER_URL)));
    assert_eq!(out.exit_code, 2);
}

// ---------------------------------------------------------------- 5.6 -------

#[test]
fn dry_run_creates_no_prefixes_rows_or_downloads() {
    let fx = standard_fixture();
    let (fake, out) = run(&fx, World::good(), true);
    let r = &out.report;

    assert!(r.dry_run);
    assert_eq!(r.total_games, 5);
    assert_eq!(r.planned, 5);
    assert_eq!(out.exit_code, 0);

    assert!(!fx.home().join(".local/share/osmosis").exists());
    assert!(!fx.home().join(".config/lutris/games").exists());
    assert!(!fx.home().join("Games").exists());
    assert!(read_rows(&fx.db()).is_empty());

    assert_eq!(fake.count_containing("wineboot"), 0);
    assert_eq!(fake.count_containing("winetricks"), 0);
    assert_eq!(fake.count_containing("curl"), 0);
    assert!(!fake.ran(&["sudo", "apt-get", "install", "-y", "openjdk-21-jre"]));

    // the plan itself is written
    assert!(fx.home().join("osmosis-setup/games-report.json").is_file());
    assert!(fx.home().join("osmosis-setup/games-report.txt").is_file());
}

#[test]
fn report_counts_reconcile_with_the_game_table() {
    let fx = standard_fixture();
    let (_, out) = run(&fx, World::good(), false);
    let r = &out.report;
    assert_eq!(r.total_games, 5);
    assert_eq!(
        r.ready
            + r.installer_pending
            + r.degraded
            + r.not_found
            + r.deferred
            + r.planned
            + r.failed,
        5
    );
    assert_eq!(r.ready, 4);
    assert_eq!(r.installer_pending, 1);
    assert_eq!(r.degraded, 0);
    assert_eq!(r.not_found, 0);
    assert!(r.manual_followups.iter().any(|f| f.contains("DXVK")));
}

// ------------------------------------------------------------ preconditions --

#[test]
fn refuses_without_lutris() {
    let fx = standard_fixture();
    let mut w = World::good();
    w.lutris = false;
    let fake = Fake::new(w);
    let err = run_games_with(&fake, &fx.home(), &fx.drive(), false, &fx.work())
        .expect_err("must refuse without lutris");
    assert!(err.contains("run osmosis setup first"), "{}", err);
}

#[test]
fn refuses_without_a_wine_runner() {
    let fx = Fixture::new();
    fx.make_db();
    let fake = Fake::new(World::good());
    let err = run_games_with(&fake, &fx.home(), &fx.drive(), false, &fx.work())
        .expect_err("must refuse without a runner");
    assert!(err.contains("run osmosis setup first"), "{}", err);
}

#[test]
fn missing_games_root_is_a_hard_refusal() {
    let fx = standard_fixture();
    let fake = Fake::new(World::good());
    let err = run_games_with(
        &fake,
        &fx.home(),
        &fx.dir.join("not-mounted"),
        false,
        &fx.work(),
    )
    .expect_err("must refuse a missing games root");
    assert!(err.contains("not a directory"), "{}", err);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn games_refuses_on_non_linux() {
    let fx = standard_fixture();
    let err = osmosis::games::run_games(&fx.drive(), &fx.home(), true)
        .expect_err("games must refuse on non-Linux hosts");
    assert!(err.contains("Linux-only"), "{}", err);
}
