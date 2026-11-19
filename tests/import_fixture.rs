//! Integration tests for slice 3 (`osmosis import`): synthetic fixture trees in
//! temp directories only — never the real profile, never a path outside the
//! fixture's own temp dir. The stage is only ever read by the importer.

use osmosis::config::Config;
use osmosis::importer::{run_import, ImportReport};
use osmosis::model::Manifest;
use osmosis::scan::run_scan;
use osmosis::stage::{run_stage, STAGE_DIR_NAME};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "osmosis-import-fixture-{}-{}",
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

/// Build a small profile, scan it, stage it, and return the staged root plus the
/// manifest. `stage_root` is the `osmosis-stage` folder (contains manifest.json).
fn build_stage(fx: &Fixture) -> (PathBuf, Manifest) {
    let root = fx.0.join("profile");
    fs::create_dir_all(&root).unwrap();
    fx.write("profile/Documents/a.txt", b"hello documents");
    fx.write("profile/Documents/notes.md", b"# notes\nsecond line\n");
    fx.write("profile/Documents/deploy.sh", b"#!/bin/sh\necho hi\n");
    fx.write("profile/.ssh/dummy_key", b"ssh-key-material");
    fx.write("profile/.ssh/dummy_key.pub", b"ssh-rsa AAAA test");
    fx.write(
        "profile/AppData/Roaming/Code/User/settings.json",
        b"{\"editor.fontSize\": 14}",
    );
    fx.write("profile/AppData/Local/junk.bin", b"junkjunkjunk");
    fx.write("profile/Documents/Thumbs.db", b"thumbs");
    // a .git subtree (aggregated KEEP)
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
    let staging = fx.0.join("staging");
    let st = run_stage(&manifest_path, &staging).expect("stage failed");
    assert_eq!(st.exit_code, 0, "stage summary: {}", st.summary);
    (staging.join(STAGE_DIR_NAME), res.manifest)
}

fn read_report(home: &Path) -> ImportReport {
    let p = home.join("osmosis-import/import-report.json");
    serde_json::from_str(&fs::read_to_string(p).unwrap()).expect("import-report.json invalid")
}

fn new_home(fx: &Fixture) -> PathBuf {
    let home = fx.0.join("home");
    fs::create_dir_all(&home).unwrap();
    home
}

#[test]
fn import_places_exact_layout_and_receipt_reconciles() {
    let fx = Fixture::new();
    let (stage, manifest) = build_stage(&fx);
    let home = new_home(&fx);

    let out = run_import(&stage, &home, false).expect("import failed");
    assert_eq!(out.exit_code, 0, "summary: {}", out.summary);

    // exactly the home layout the manifest describes (leading ~/ stripped)
    assert_eq!(
        fs::read(home.join("Documents/a.txt")).unwrap(),
        b"hello documents"
    );
    assert_eq!(
        fs::read(home.join(".ssh/dummy_key")).unwrap(),
        b"ssh-key-material"
    );
    assert_eq!(
        fs::read(home.join(".config/Code/User/settings.json")).unwrap(),
        b"{\"editor.fontSize\": 14}"
    );
    // keep_dirs (.git subtree) copied recursively
    assert_eq!(
        fs::read(home.join("repo/.git/HEAD")).unwrap(),
        b"ref: refs/heads/main\n"
    );
    assert_eq!(
        fs::read(home.join("repo/.git/objects/ab/cd")).unwrap(),
        vec![3u8; 64]
    );
    // SKIP files are never imported
    assert!(!home.join("AppData").exists());
    assert!(!home.join("Documents/Thumbs.db").exists());

    let r = read_report(&home);
    assert!(!r.dry_run);
    assert_eq!(r.placed_verified.count, manifest.totals.keep_files);
    assert_eq!(r.already_present.count, 0);
    assert_eq!(r.conflicts.count, 0);
    assert_eq!(r.failed.count, 0);
    assert_eq!(r.total_files, manifest.totals.keep_files);
    assert_eq!(r.total_bytes, manifest.totals.keep_bytes);
    let count_sum =
        r.placed_verified.count + r.already_present.count + r.conflicts.count + r.failed.count;
    let byte_sum =
        r.placed_verified.bytes + r.already_present.bytes + r.conflicts.bytes + r.failed.bytes;
    assert_eq!(count_sum, r.total_files);
    assert_eq!(byte_sum, r.total_bytes);

    let txt = fs::read_to_string(home.join("osmosis-import/import-report.txt")).unwrap();
    assert!(txt.contains(&format!("placed_verified:   {}", r.placed_verified.count)));
    assert!(txt.contains("CONFLICTS\n---------\n(none)"));
    assert!(txt.contains("FAILED\n------\n(none)"));
}

#[test]
fn corrupted_staged_file_is_failed_not_accepted() {
    let fx = Fixture::new();
    let (stage, manifest) = build_stage(&fx);
    // flip one byte in one staged file (same size)
    let target = stage.join("Documents/a.txt");
    let mut data = fs::read(&target).unwrap();
    data[0] ^= 0xFF;
    fs::write(&target, &data).unwrap();

    let home = new_home(&fx);
    let out = run_import(&stage, &home, false).expect("import completed");
    assert_eq!(out.exit_code, 2, "summary: {}", out.summary);

    let r = read_report(&home);
    assert_eq!(r.failed.count, 1, "failed: {:?}", r.failed_files);
    assert_eq!(r.failed_files.len(), 1);
    assert_eq!(r.failed_files[0].path, "Documents/a.txt");
    assert!(r.failed_files[0].reason.contains("hash mismatch"));
    assert_eq!(r.conflicts.count, 0);
    assert_eq!(r.placed_verified.count, manifest.totals.keep_files - 1);
}

#[test]
fn conflicting_target_is_recorded_and_untouched() {
    let fx = Fixture::new();
    let (stage, manifest) = build_stage(&fx);
    let home = new_home(&fx);
    fs::create_dir_all(home.join("Documents")).unwrap();
    fs::write(home.join("Documents/a.txt"), b"different content").unwrap();

    let out = run_import(&stage, &home, false).expect("import completed");
    assert_eq!(out.exit_code, 2, "summary: {}", out.summary);

    let r = read_report(&home);
    assert_eq!(r.conflicts.count, 1, "conflicts: {:?}", r.conflict_files);
    assert_eq!(r.conflict_files.len(), 1);
    assert_eq!(r.conflict_files[0].path, "Documents/a.txt");
    assert_eq!(r.failed.count, 0);
    assert_eq!(r.placed_verified.count, manifest.totals.keep_files - 1);
    // never overwritten
    assert_eq!(
        fs::read(home.join("Documents/a.txt")).unwrap(),
        b"different content"
    );
}

#[test]
fn dry_run_computes_the_plan_and_writes_nothing() {
    let fx = Fixture::new();
    let (stage, manifest) = build_stage(&fx);
    let home = new_home(&fx);

    let out = run_import(&stage, &home, true).expect("dry-run failed");
    assert_eq!(out.exit_code, 0, "summary: {}", out.summary);
    assert!(out.summary.contains("dry_run: true"));
    assert!(out.summary.contains(&format!(
        "to_place:          {}",
        manifest.totals.keep_files
    )));
    // nothing written anywhere: home stays empty, no receipt dir
    assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
    assert!(!home.join("osmosis-import").exists());
    // and the stage is untouched
    assert_eq!(
        fs::read(stage.join("Documents/a.txt")).unwrap(),
        b"hello documents"
    );

    // dry-run also reports a conflict without writing
    fs::create_dir_all(home.join("Documents")).unwrap();
    fs::write(home.join("Documents/a.txt"), b"different").unwrap();
    let out2 = run_import(&stage, &home, true).expect("dry-run failed");
    assert_eq!(out2.exit_code, 2);
    assert!(out2.summary.contains("conflicts:         1"));
    assert_eq!(
        fs::read(home.join("Documents/a.txt")).unwrap(),
        b"different"
    );
    assert!(!home.join("osmosis-import").exists());
}

#[test]
fn second_import_is_idempotent() {
    let fx = Fixture::new();
    let (stage, manifest) = build_stage(&fx);
    let home = new_home(&fx);

    let out1 = run_import(&stage, &home, false).unwrap();
    assert_eq!(out1.exit_code, 0);
    let out2 = run_import(&stage, &home, false).unwrap();
    assert_eq!(out2.exit_code, 0, "summary: {}", out2.summary);

    let r = read_report(&home);
    assert_eq!(r.placed_verified.count, 0, "second run must place nothing");
    assert_eq!(r.already_present.count, manifest.totals.keep_files);
    assert_eq!(r.conflicts.count, 0);
    assert_eq!(r.failed.count, 0);
}

#[test]
fn stage_without_manifest_is_refused_and_writes_nothing() {
    let fx = Fixture::new();
    let stage = fx.0.join("empty-stage");
    fs::create_dir_all(&stage).unwrap();
    let home = new_home(&fx);

    let err = run_import(&stage, &home, false).expect_err("missing manifest must be refused");
    assert!(err.contains("manifest.json"), "err: {}", err);
    assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
}

#[test]
fn stage_inside_home_is_refused() {
    let fx = Fixture::new();
    let (stage, _manifest) = build_stage(&fx);
    // the staging parent contains osmosis-stage, so the stage lives inside it
    let home = fx.0.join("staging");

    let err = run_import(&stage, &home, false).expect_err("overlap must be refused");
    assert!(err.contains("overlaps"), "err: {}", err);
}

#[test]
fn home_inside_stage_is_refused() {
    let fx = Fixture::new();
    let (stage, _manifest) = build_stage(&fx);
    let home = stage.join("subhome");
    fs::create_dir_all(&home).unwrap();

    let err = run_import(&stage, &home, false).expect_err("overlap must be refused");
    assert!(err.contains("overlaps"), "err: {}", err);
}

#[test]
fn escaping_destination_is_a_hard_error_and_writes_nothing() {
    let fx = Fixture::new();
    let (stage, _manifest) = build_stage(&fx);
    // hand-edit a COPY of the staged manifest in a separate fake stage dir
    let fake = fx.0.join("fake-stage");
    fs::create_dir_all(&fake).unwrap();
    let mut v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(stage.join("manifest.json")).unwrap()).unwrap();
    v["entries"][0]["linux_dest"] = serde_json::Value::String("~/../evil.txt".to_string());
    fs::write(
        fake.join("manifest.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();

    let home = new_home(&fx);
    let err = run_import(&fake, &home, false).expect_err("escape must be a hard error");
    assert!(err.contains("escapes the target home"), "err: {}", err);
    assert_eq!(fs::read_dir(&home).unwrap().count(), 0);
}

#[cfg(unix)]
#[test]
fn unix_permissions_follow_the_plan() {
    use std::os::unix::fs::PermissionsExt;

    let fx = Fixture::new();
    let (stage, _manifest) = build_stage(&fx);
    let home = new_home(&fx);
    let out = run_import(&stage, &home, false).unwrap();
    assert_eq!(out.exit_code, 0, "summary: {}", out.summary);

    let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode(&home.join(".ssh")), 0o700);
    assert_eq!(mode(&home.join(".ssh/dummy_key")), 0o600);
    assert_eq!(mode(&home.join(".ssh/dummy_key.pub")), 0o644);
    assert_eq!(mode(&home.join("Documents/a.txt")), 0o644);
    assert_eq!(mode(&home.join("Documents/deploy.sh")), 0o755);
    assert_eq!(mode(&home.join("Documents")), 0o755);
}
