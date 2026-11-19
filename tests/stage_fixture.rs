//! Integration tests for slice 2 (`osmosis stage`): synthetic fixture trees in
//! temp directories only — never the real profile, never a path outside the
//! fixture's own temp dir. Sources are only ever read by the copier.

use osmosis::config::Config;
use osmosis::model::Manifest;
use osmosis::scan::run_scan;
use osmosis::stage::{run_stage, verify_stage, CopyReport, STAGE_DIR_NAME};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "osmosis-stage-fixture-{}-{}",
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

/// Build a small profile, scan it, and return (fixture, manifest path, manifest).
fn scan_fixture(fx: &Fixture) -> (PathBuf, Manifest) {
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();
    fx.write("profile/Documents/a.txt", b"hello documents");
    fx.write("profile/Documents/notes.md", b"# notes\nsecond line\n");
    fx.write("profile/.ssh/dummy_key", b"ssh-key-material");
    fx.write(
        "profile/AppData/Roaming/Code/User/settings.json",
        b"{\"editor.fontSize\": 14}",
    );
    fx.write("profile/AppData/Local/junk.bin", b"junkjunkjunk");
    fx.write("profile/Documents/Thumbs.db", b"thumbs");
    // a .git subtree (aggregated KEEP); init a real repo when git is available so
    // the repo checks do not produce scan errors
    let repo = root.join("repo");
    fs::create_dir_all(repo.join(".git/objects/ab")).unwrap();
    fx.write("profile/repo/.git/HEAD", b"ref: refs/heads/main\n");
    fx.write("profile/repo/.git/objects/ab/cd", &[3u8; 64]);
    fx.write("profile/repo/tracked.txt", b"v1");
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

    let out = fx.0.join("out");
    let res = run_scan(&root, &Config::default_config(), &out, None).expect("scan failed");
    let manifest_path = out.join("manifest.json");
    assert!(manifest_path.is_file());
    (manifest_path, res.manifest)
}

fn stage_root(staging: &Path) -> PathBuf {
    staging.join(STAGE_DIR_NAME)
}

fn read_report(staging: &Path) -> CopyReport {
    let p = stage_root(staging).join("copy-report.json");
    serde_json::from_str(&fs::read_to_string(p).unwrap()).expect("copy-report.json invalid")
}

#[test]
fn end_to_end_scan_stage_verify() {
    let fx = Fixture::new();
    let (manifest_path, manifest) = scan_fixture(&fx);
    let staging = fx.0.join("staging");

    let out = run_stage(&manifest_path, &staging).expect("stage failed");
    assert_eq!(out.exit_code, 0, "summary: {}", out.summary);

    let root = stage_root(&staging);
    // manifest itself is staged
    assert!(root.join("manifest.json").is_file());
    // linux_dest layout: leading ~/ stripped
    assert_eq!(
        fs::read(root.join("Documents/a.txt")).unwrap(),
        b"hello documents"
    );
    assert_eq!(
        fs::read(root.join(".ssh/dummy_key")).unwrap(),
        b"ssh-key-material"
    );
    assert_eq!(
        fs::read(root.join(".config/Code/User/settings.json")).unwrap(),
        b"{\"editor.fontSize\": 14}"
    );
    // keep_dirs (.git subtree) copied recursively
    assert_eq!(
        fs::read(root.join("repo/.git/HEAD")).unwrap(),
        b"ref: refs/heads/main\n"
    );
    assert_eq!(
        fs::read(root.join("repo/.git/objects/ab/cd")).unwrap(),
        vec![3u8; 64]
    );
    // SKIP files are not staged
    assert!(!root.join("AppData").exists());
    assert!(!root.join("Documents/Thumbs.db").exists());

    let r = read_report(&staging);
    assert_eq!(r.precheck, "ok");
    assert_eq!(r.verify_failed.count, 0);
    assert_eq!(r.skipped_locked.count, 0);
    assert_eq!(r.skipped_missing.count, 0);
    assert_eq!(r.already_verified.count, 0);
    assert_eq!(r.copied_verified.count, manifest.totals.keep_files);
    // reconciliation: every keep file is accounted for exactly once
    let sum = r.copied_verified.count
        + r.already_verified.count
        + r.verify_failed.count
        + r.skipped_locked.count
        + r.skipped_missing.count;
    assert_eq!(sum, r.total_files);
    assert_eq!(r.total_files, manifest.totals.keep_files);
    assert_eq!(
        r.copied_verified.bytes
            + r.already_verified.bytes
            + r.verify_failed.bytes
            + r.skipped_locked.bytes
            + r.skipped_missing.bytes,
        r.total_bytes
    );

    // staged hash of a hashed keeper equals the manifest hash
    let doc = manifest
        .entries
        .iter()
        .find(|e| e.relative_path == "Documents/a.txt")
        .unwrap();
    let staged_hash = {
        use xxhash_rust::xxh64::Xxh64;
        let data = fs::read(root.join("Documents/a.txt")).unwrap();
        let mut h = Xxh64::new(0);
        h.update(&data);
        format!("{:016x}", h.digest())
    };
    assert_eq!(Some(&staged_hash), doc.hash.as_ref());

    // txt report mirrors the json totals
    let txt = fs::read_to_string(root.join("copy-report.txt")).unwrap();
    assert!(txt.contains(&format!("copied_verified:   {}", r.copied_verified.count)));
    assert!(txt.contains("precheck: ok"));
}

#[test]
fn corruption_reports_exactly_that_file() {
    let fx = Fixture::new();
    let (manifest_path, manifest) = scan_fixture(&fx);
    let staging = fx.0.join("staging");
    let out = run_stage(&manifest_path, &staging).unwrap();
    assert_eq!(out.exit_code, 0);

    // flip one byte in one staged file (same size)
    let target = stage_root(&staging).join("Documents/a.txt");
    let mut data = fs::read(&target).unwrap();
    data[0] ^= 0xFF;
    fs::write(&target, &data).unwrap();

    // re-running verification reports exactly that file as a mismatch
    let r = verify_stage(&manifest, &stage_root(&staging)).unwrap();
    assert_eq!(
        r.verify_failed.count, 1,
        "report: {:?}",
        r.verify_failed_files
    );
    assert_eq!(r.verify_failed_files.len(), 1);
    assert_eq!(r.verify_failed_files[0].path, "Documents/a.txt");
    assert!(r.verify_failed_files[0].reason.contains("hash mismatch"));
    assert_eq!(r.already_verified.count, manifest.totals.keep_files - 1);
}

#[test]
fn missing_source_is_skipped_missing_and_run_completes() {
    let fx = Fixture::new();
    let (manifest_path, manifest) = scan_fixture(&fx);
    // delete one source between scan and stage
    fs::remove_file(fx.0.join("profile/Documents/a.txt")).unwrap();
    let staging = fx.0.join("staging");

    let out = run_stage(&manifest_path, &staging).expect("stage failed");
    assert_eq!(out.exit_code, 2, "summary: {}", out.summary);
    let r = read_report(&staging);
    assert_eq!(r.skipped_missing.count, 1);
    assert_eq!(r.skipped_missing_files.len(), 1);
    assert_eq!(r.skipped_missing_files[0].path, "Documents/a.txt");
    assert!(!r.skipped_missing_files[0].reason.is_empty());
    assert_eq!(r.copied_verified.count, manifest.totals.keep_files - 1);
    assert!(!stage_root(&staging).join("Documents/a.txt").exists());

    // restore the source and re-run: only that file is copied, rest already_verified
    fx.write("profile/Documents/a.txt", b"hello documents");
    let out2 = run_stage(&manifest_path, &staging).unwrap();
    assert_eq!(out2.exit_code, 0, "summary: {}", out2.summary);
    let r2 = read_report(&staging);
    assert_eq!(r2.copied_verified.count, 1);
    assert_eq!(r2.already_verified.count, manifest.totals.keep_files - 1);
    assert_eq!(r2.verify_failed.count, 0);
    assert_eq!(r2.skipped_missing.count, 0);
}

#[test]
fn resume_second_run_copies_nothing() {
    let fx = Fixture::new();
    let (manifest_path, manifest) = scan_fixture(&fx);
    let staging = fx.0.join("staging");

    let out1 = run_stage(&manifest_path, &staging).unwrap();
    assert_eq!(out1.exit_code, 0);
    let out2 = run_stage(&manifest_path, &staging).unwrap();
    assert_eq!(out2.exit_code, 0, "summary: {}", out2.summary);
    let r2 = read_report(&staging);
    assert_eq!(r2.copied_verified.count, 0, "second run must copy nothing");
    assert_eq!(r2.already_verified.count, manifest.totals.keep_files);
    assert_eq!(r2.verify_failed.count, 0);
}

#[test]
fn escape_guard_is_a_hard_error_and_writes_nothing() {
    let fx = Fixture::new();
    let (manifest_path, _manifest) = scan_fixture(&fx);
    // hand-edit the manifest: one linux_dest escapes the staging root
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    v["entries"][0]["linux_dest"] = serde_json::Value::String("~/../evil.txt".to_string());
    let bad_manifest = fx.0.join("out/manifest-bad.json");
    fs::write(&bad_manifest, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let staging = fx.0.join("staging");
    let err = run_stage(&bad_manifest, &staging).expect_err("escape must be a hard error");
    assert!(err.contains("escapes the staging root"), "err: {}", err);
    // nothing was written at all — not even the staging root dir
    assert!(!stage_root(&staging).exists());
    assert!(!staging.exists() || fs::read_dir(&staging).unwrap().next().is_none());
}

#[test]
fn staging_inside_scanned_root_is_refused() {
    let fx = Fixture::new();
    let (manifest_path, _manifest) = scan_fixture(&fx);
    let root = fx.0.join("profile");

    // staging root inside the scanned root
    let err = run_stage(&manifest_path, &root.join("backup"))
        .expect_err("staging inside the scanned root must be refused");
    assert!(err.contains("refusing to stage"), "err: {}", err);
    assert!(!root.join("backup").exists());

    // staging root equal to the scanned root
    let err2 = run_stage(&manifest_path, &root).expect_err("staging == root must be refused");
    assert!(err2.contains("refusing to stage"), "err: {}", err2);
}

#[test]
fn precheck_shortfall_writes_only_the_report_shell() {
    let fx = Fixture::new();
    let (manifest_path, _manifest) = scan_fixture(&fx);
    // inflate the keep total so the free-space precheck cannot pass
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    v["totals"]["keep_bytes"] = serde_json::Value::from(u64::MAX / 2);
    let big_manifest = fx.0.join("out/manifest-big.json");
    fs::write(&big_manifest, serde_json::to_string_pretty(&v).unwrap()).unwrap();

    let staging = fx.0.join("staging");
    let out = run_stage(&big_manifest, &staging).expect("precheck failure is not an Err");
    assert_eq!(out.exit_code, 1);
    assert!(
        out.summary.contains("insufficient_space"),
        "summary: {}",
        out.summary
    );

    let root = stage_root(&staging);
    let r = read_report(&staging);
    assert_eq!(r.precheck, "insufficient_space");
    assert!(r.shortfall_bytes > 0);
    assert_eq!(r.copied_verified.count, 0);
    // only the report shell exists: no staged manifest, no copied files
    assert!(root.join("copy-report.json").is_file());
    assert!(root.join("copy-report.txt").is_file());
    assert!(!root.join("manifest.json").exists());
    let entries: Vec<_> = fs::read_dir(&root).unwrap().collect();
    assert_eq!(entries.len(), 2, "only the two report files may exist");
}
