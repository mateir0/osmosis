//! Integration test (section 1.5): builds a synthetic fixture tree in a temp
//! directory — and ONLY there — runs the scanner library against it, and
//! asserts categories, totals, and manifest validity. Never touches the real
//! user profile or any path outside its own temp dir.

use osmosis::config::Config;
use osmosis::model::Manifest;
use osmosis::scan::run_scan;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "osmosis-fixture-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        Fixture(dir)
    }

    fn write(&self, rel: &str, content: &[u8]) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn scanner_classifies_fixture_tree() {
    let fx = Fixture::new();
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();

    // KEEP via mapping
    fx.write("profile/Documents/a.txt", b"hello documents");
    // KEEP, sensitive
    fx.write("profile/.ssh/dummy_key", b"ssh-key-material");
    // KEEP: AppData mapping beats the AppData SKIP rule
    fx.write(
        "profile/AppData/Roaming/Code/User/settings.json",
        b"{\"editor.fontSize\": 14}",
    );
    // SKIP: rest of AppData
    fx.write("profile/AppData/Local/junk.bin", b"junkjunkjunk");
    // SKIP by filename / extension
    fx.write("profile/Documents/Thumbs.db", b"thumbs");
    fx.write("profile/Documents/scratch.tmp", b"tmp");
    // RECREATE aggregate inside a KEEP folder
    fx.write("profile/projects/app/node_modules/big.bin", &[7u8; 1000]);
    fx.write("profile/projects/app/src/main.rs", b"fn main() {}");
    // git repo (created only when git is available)
    let made_repo = if git_available() {
        let repo = root.join("repo");
        fs::create_dir_all(&repo).unwrap();
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
        };
        assert!(run(&["init"]).status.success());
        assert!(run(&["config", "user.email", "test@example.com"])
            .status
            .success());
        assert!(run(&["config", "user.name", "Test"]).status.success());
        fs::write(repo.join("tracked.txt"), b"v1").unwrap();
        assert!(run(&["add", "."]).status.success());
        assert!(run(&["commit", "-m", "init"]).status.success());
        // dirty the repo: an uncommitted change
        fs::write(repo.join("tracked.txt"), b"v2 dirty").unwrap();
        true
    } else {
        false
    };

    let out = fx.0.join("out");
    let res = run_scan(&root, &Config::default_config(), &out, None).expect("scan failed");
    let m: Manifest = serde_json::from_str(&fs::read_to_string(out.join("manifest.json")).unwrap())
        .expect("manifest.json is not valid JSON");

    // manifest shape
    assert_eq!(m.schema_version, 1);
    assert!(!m.created_at.is_empty());
    assert_eq!(m.totals.error_count as usize, m.errors.len());

    let keep_paths: Vec<&str> = m.entries.iter().map(|e| e.relative_path.as_str()).collect();
    assert!(
        keep_paths.contains(&"Documents/a.txt"),
        "keep: {:?}",
        keep_paths
    );
    assert!(keep_paths.contains(&".ssh/dummy_key"));
    assert!(keep_paths.contains(&"AppData/Roaming/Code/User/settings.json"));
    // non-mapped top-level folders are SKIP by the default rule
    assert!(!keep_paths.contains(&"projects/app/src/main.rs"));
    assert!(!keep_paths.iter().any(|p| p.starts_with("AppData/Local")));
    assert!(!keep_paths
        .iter()
        .any(|p| p.ends_with("Thumbs.db") || p.ends_with(".tmp")));
    assert!(!keep_paths.iter().any(|p| p.contains("node_modules")));

    // sensitive flag on .ssh
    let ssh = m
        .entries
        .iter()
        .find(|e| e.relative_path == ".ssh/dummy_key")
        .unwrap();
    assert!(ssh.sensitive);
    assert_eq!(ssh.linux_dest, "~/.ssh/dummy_key");
    assert_eq!(ssh.hash_status, "ok");
    assert!(ssh.hash.is_some());
    let doc = m
        .entries
        .iter()
        .find(|e| e.relative_path == "Documents/a.txt")
        .unwrap();
    assert_eq!(doc.linux_dest, "~/Documents/a.txt");
    assert!(!doc.sensitive);

    // RECREATE aggregate: node_modules pruned to ONE entry
    let nm: Vec<&osmosis::model::RecreateEntry> = m
        .recreate
        .iter()
        .filter(|r| r.path == "projects/app/node_modules")
        .collect();
    assert_eq!(
        nm.len(),
        1,
        "node_modules must be one aggregate, got {:?}",
        m.recreate
    );
    assert_eq!(nm[0].kind, "recreate_dir");
    assert_eq!(nm[0].file_count, 1);
    assert_eq!(nm[0].size_bytes, 1000);

    // totals consistency (.git subtrees are aggregated KEEP, counted in totals)
    let sum_keep: u64 = m.entries.iter().map(|e| e.size_bytes).sum::<u64>()
        + m.keep_dirs.iter().map(|d| d.size_bytes).sum::<u64>();
    assert_eq!(m.totals.keep_bytes, sum_keep);
    assert_eq!(
        m.totals.keep_files as usize,
        m.entries.len()
            + m.keep_dirs
                .iter()
                .map(|d| d.file_count as usize)
                .sum::<usize>()
    );
    assert!(
        m.totals.skip_files >= 4,
        "AppData junk, Thumbs.db, .tmp, projects/src => skip: {}",
        m.totals.skip_files
    );
    assert_eq!(m.totals.recreate_entries as usize, m.recreate.len());
    assert_eq!(
        m.totals.recreate_bytes,
        m.recreate.iter().map(|r| r.size_bytes).sum::<u64>()
    );

    // repos
    if made_repo {
        assert_eq!(m.git_check, "ok");
        let repo = m
            .repos
            .iter()
            .find(|r| r.path == "repo")
            .expect("repo not detected");
        assert!(
            repo.dirty_files >= 1,
            "uncommitted file must show dirty_files >= 1"
        );
        assert!(repo.no_upstream, "repo has no remote => no_upstream = true");
        assert_eq!(repo.unpushed, None);
        assert!(
            !repo.no_commits,
            "repo with a commit must have no_commits = false"
        );
        // .git pruned as one KEEP aggregate, internals not listed, never RECREATE
        let git_dir = m
            .keep_dirs
            .iter()
            .find(|d| d.path == "repo/.git" && d.kind == "git_dir")
            .expect("repo/.git must be one keep_dirs aggregate");
        assert!(git_dir.file_count > 0);
        assert!(!m.recreate.iter().any(|r| r.path.starts_with("repo/.git")));
        assert!(!m
            .entries
            .iter()
            .any(|e| e.relative_path.starts_with("repo/.git/")));
    }

    // report.txt exists and its totals match the manifest exactly
    let report = fs::read_to_string(out.join("report.txt")).unwrap();
    assert!(report.contains(&format!("keep_files:        {}", m.totals.keep_files)));
    assert!(report.contains(&format!("error_count:       {}", m.totals.error_count)));
    assert!(report.contains(&osmosis::report::human(m.totals.keep_bytes)));
    assert!(report.contains("Staging: not checked (no --staging path given)"));
    assert!(report.contains("REPOS REQUIRING ATTENTION"));

    // stdout summary equals report.txt content
    assert_eq!(res.summary, report);
}

#[test]
fn staging_fit_check_appears_when_requested() {
    let fx = Fixture::new();
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();
    fx.write("profile/Documents/a.txt", b"tiny");
    let out = fx.0.join("out");
    let staging = fx.0.join("staging");
    fs::create_dir_all(&staging).unwrap();
    let res = run_scan(&root, &Config::default_config(), &out, Some(&staging)).unwrap();
    assert!(res.summary.contains("Keepers:"), "summary: {}", res.summary);
    assert!(res.summary.contains("Staging free:"));
    assert!(res.summary.contains("-> FITS") || res.summary.contains("-> DOES NOT FIT"));
}

#[test]
fn scan_rejects_missing_root() {
    let fx = Fixture::new();
    let missing = fx.0.join("does-not-exist");
    let out = fx.0.join("out");
    let err = match run_scan(&missing, &Config::default_config(), &out, None) {
        Ok(_) => panic!("expected an error for a missing root"),
        Err(e) => e,
    };
    assert!(err.contains("not a directory"), "err: {}", err);
    // and it wrote nothing
    assert!(!Path::new(&out).exists());
}

/// 1.1.1 — Roaming KEEP mappings classify with the stated linux_dest / sensitive.
#[test]
fn roaming_keep_mappings_classify_correctly() {
    let fx = Fixture::new();
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();

    fx.write(
        "profile/AppData/Roaming/.minecraft/saves/world/level.dat",
        b"level",
    );
    fx.write(
        "profile/AppData/Roaming/Telegram Desktop/tdata/key_datas",
        b"tdata",
    );
    fx.write(
        "profile/AppData/Local/Google/Chrome/User Data/Default/Preferences",
        b"{}",
    );

    let out = fx.0.join("out");
    let res = run_scan(&root, &Config::default_config(), &out, None).unwrap();
    let m = &res.manifest;

    let mc = m
        .entries
        .iter()
        .find(|e| e.relative_path == "AppData/Roaming/.minecraft/saves/world/level.dat")
        .expect("roaming .minecraft file must be KEEP");
    assert_eq!(mc.linux_dest, "~/.minecraft/saves/world/level.dat");
    assert!(!mc.sensitive);

    let tg = m
        .entries
        .iter()
        .find(|e| e.relative_path == "AppData/Roaming/Telegram Desktop/tdata/key_datas")
        .expect("Telegram Desktop file must be KEEP");
    assert_eq!(
        tg.linux_dest,
        "~/.local/share/TelegramDesktop/tdata/key_datas"
    );
    assert!(tg.sensitive, "Telegram Desktop must be sensitive");

    let chrome = m
        .entries
        .iter()
        .find(|e| e.relative_path == "AppData/Local/Google/Chrome/User Data/Default/Preferences")
        .expect("Chrome Default profile must be KEEP");
    assert_eq!(
        chrome.linux_dest,
        "~/.config/google-chrome/Default/Preferences"
    );
    assert!(chrome.sensitive, "Chrome Default profile must be sensitive");

    assert_eq!(m.totals.error_count, 0);
}

/// 1.1.2 — `dist` (and `out`) directories aggregate as ONE RECREATE entry each.
#[test]
fn dist_and_out_dirs_are_single_recreate_aggregates() {
    let fx = Fixture::new();
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();

    fx.write("profile/projects/app/dist/bundle.js", &[1u8; 512]);
    fx.write("profile/projects/app/dist/assets/logo.svg", &[2u8; 128]);
    fx.write("profile/projects/app/out/main.exe", &[3u8; 256]);

    let out = fx.0.join("out");
    let res = run_scan(&root, &Config::default_config(), &out, None).unwrap();
    let m = &res.manifest;

    let dist: Vec<&osmosis::model::RecreateEntry> = m
        .recreate
        .iter()
        .filter(|r| r.path == "projects/app/dist")
        .collect();
    assert_eq!(
        dist.len(),
        1,
        "dist must be ONE aggregate: {:?}",
        m.recreate
    );
    assert_eq!(dist[0].kind, "recreate_dir");
    assert_eq!(dist[0].file_count, 2);
    assert_eq!(dist[0].size_bytes, 512 + 128);

    let outdir: Vec<&osmosis::model::RecreateEntry> = m
        .recreate
        .iter()
        .filter(|r| r.path == "projects/app/out")
        .collect();
    assert_eq!(
        outdir.len(),
        1,
        "out must be ONE aggregate: {:?}",
        m.recreate
    );
    assert_eq!(outdir[0].file_count, 1);

    // nothing under dist/ or out/ is listed as an individual entry
    assert!(!m
        .entries
        .iter()
        .any(|e| e.relative_path.contains("/dist/") || e.relative_path.contains("/out/")));
}

/// 1.1.3 — a `.git` containing a `node_modules`-named subfolder is KEEP only.
#[test]
fn git_dir_with_node_modules_subfolder_is_keep_only() {
    let fx = Fixture::new();
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();

    fx.write("profile/repo/src/main.rs", b"fn main() {}");
    // a real-ish .git; if git is available, init it so git checks do not error
    let repo = root.join("repo");
    if git_available() {
        let run = |args: &[&str]| {
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap()
        };
        assert!(run(&["init"]).status.success());
    }
    // the trap: a RECREATE-named subfolder inside .git must NOT be pruned as RECREATE
    fx.write("profile/repo/.git/node_modules/fake.bin", &[9u8; 100]);
    fx.write("profile/repo/.git/HEAD", b"ref: refs/heads/main\n");

    let out = fx.0.join("out");
    let res = run_scan(&root, &Config::default_config(), &out, None).unwrap();
    let m = &res.manifest;

    // the whole .git is ONE KEEP aggregate
    let git_dirs: Vec<&osmosis::model::KeepDirEntry> = m
        .keep_dirs
        .iter()
        .filter(|d| d.path == "repo/.git")
        .collect();
    assert_eq!(git_dirs.len(), 1, "keep_dirs: {:?}", m.keep_dirs);
    assert_eq!(git_dirs[0].kind, "git_dir");
    assert!(git_dirs[0].file_count >= 2);
    assert!(git_dirs[0].size_bytes >= 100);

    // zero RECREATE entries from inside .git
    assert!(
        !m.recreate.iter().any(|r| r.path.starts_with("repo/.git")),
        "recreate: {:?}",
        m.recreate
    );
    // zero individual KEEP entries from inside .git (aggregated, not listed)
    assert!(!m
        .entries
        .iter()
        .any(|e| e.relative_path.starts_with("repo/.git/")));
}

/// 1.1.4 — fake repos under AppData/Local/Temp and under a Temp dir produce
/// no repo entries and no errors.
#[test]
fn git_checks_suppressed_under_appdata_and_temp() {
    let fx = Fixture::new();
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();

    // fake ".git" in an AppData cache (a plain dir with garbage, like real caches)
    fx.write(
        "profile/AppData/Local/Temp/fakecache/.git/HEAD",
        b"this is not a real git dir\n",
    );
    fx.write(
        "profile/AppData/Local/Temp/fakecache/.git/config",
        b"garbage",
    );
    // fake repo under a top-level Temp dir
    fx.write("profile/Temp/othercache/repo/.git/HEAD", b"also garbage\n");

    let out = fx.0.join("out");
    let res = run_scan(&root, &Config::default_config(), &out, None).unwrap();
    let m = &res.manifest;

    assert!(
        m.repos.is_empty(),
        "no repos may be detected under AppData/Temp, got {:?}",
        m.repos
    );
    assert_eq!(
        m.totals.error_count, 0,
        "git-check noise must not produce errors: {:?}",
        m.errors
    );
}

/// 1.1.4 — a zero-commit repo is listed with no_commits = true, branch = null,
/// and error_count stays 0.
#[test]
fn zero_commit_repo_is_no_commits_not_an_error() {
    if !git_available() {
        return; // nothing to test without git
    }
    let fx = Fixture::new();
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();

    let repo = root.join("emptyrepo");
    fs::create_dir_all(&repo).unwrap();
    let run = |args: &[&str]| {
        Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap()
    };
    assert!(run(&["init"]).status.success());
    assert!(run(&["config", "user.email", "test@example.com"])
        .status
        .success());
    assert!(run(&["config", "user.name", "Test"]).status.success());
    // NO commit -> unborn HEAD

    let out = fx.0.join("out");
    let res = run_scan(&root, &Config::default_config(), &out, None).unwrap();
    let m = &res.manifest;

    let r = m
        .repos
        .iter()
        .find(|r| r.path == "emptyrepo")
        .expect("zero-commit repo must still be listed");
    assert!(r.no_commits, "no_commits must be true: {:?}", r);
    assert_eq!(r.branch, None);
    assert_eq!(
        m.totals.error_count, 0,
        "unborn HEAD must not be an error: {:?}",
        m.errors
    );

    // and the serialized manifest really carries no_commits: true
    let raw = fs::read_to_string(out.join("manifest.json")).unwrap();
    assert!(
        raw.contains("\"no_commits\": true"),
        "manifest json: {}",
        raw
    );
}
