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

    // totals consistency
    let sum_keep: u64 = m.entries.iter().map(|e| e.size_bytes).sum();
    assert_eq!(m.totals.keep_bytes, sum_keep);
    assert_eq!(m.totals.keep_files as usize, m.entries.len());
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
        // .git pruned as one aggregate, internals not listed
        assert!(m
            .recreate
            .iter()
            .any(|r| r.path == "repo/.git" && r.kind == "git_dir"));
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
