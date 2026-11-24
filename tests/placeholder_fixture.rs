//! Integration tests for slice 9.1/9.2: OneDrive placeholder detection in the
//! scan and the stage refusal. A stub attribute source is injected (the real
//! Windows attribute read is cfg-gated), so these run on any platform. Only
//! fixture temp dirs are used.

use osmosis::config::Config;
use osmosis::model::Manifest;
use osmosis::placeholder::{FILE_ATTRIBUTE_OFFLINE, FILE_ATTRIBUTE_RECALL_ON_OPEN};
use osmosis::scan::{run_scan, run_scan_with_attrs};
use osmosis::stage::{run_stage_with, STAGE_DIR_NAME};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "osmosis-placeholder-fixture-{}-{}",
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

/// Stub metadata source: paths in `stubs` report the given attribute bits,
/// everything else is a normal file.
fn stub_attrs(stubs: Arc<HashMap<String, u32>>) -> impl Fn(&Path) -> Option<u32> + 'static {
    move |p: &Path| {
        let rel = p
            .to_string_lossy()
            .replace('\\', "/")
            .split("profile/")
            .nth(1)
            .unwrap_or("")
            .to_string();
        stubs.get(&rel).copied()
    }
}

fn build_profile(fx: &Fixture) -> PathBuf {
    let root = fx.0.join("profile");
    fs::create_dir_all(root.join("OneDrive")).unwrap(); // top-level OneDrive dir => detection flag
    fx.write("profile/Documents/real.txt", b"real content");
    fx.write("profile/Documents/stub-a.txt", b"stub");
    fx.write("profile/Documents/stub-b.txt", b"stub2");
    fx.write("profile/Pictures/photo.png", &[0u8; 128]);
    root
}

#[test]
fn scan_flags_placeholders_and_report_warns_loudly() {
    let fx = Fixture::new();
    let root = build_profile(&fx);
    let stubs = Arc::new(HashMap::from([
        (
            "Documents/stub-a.txt".to_string(),
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
        ),
        ("Documents/stub-b.txt".to_string(), FILE_ATTRIBUTE_OFFLINE),
        (
            "Pictures/photo.png".to_string(),
            0x0040_0000 | 0x80, // RECALL_ON_DATA_ACCESS + NORMAL
        ),
    ]));
    let out = fx.0.join("out");
    let res = run_scan_with_attrs(
        &root,
        &Config::default_config(),
        &out,
        None,
        &stub_attrs(stubs),
    )
    .expect("scan failed");
    let m: Manifest = res.manifest;

    // manifest: placeholder aggregate + per-entry flags
    assert_eq!(m.placeholders.count, 3);
    assert!(m.placeholders.onedrive_detected, "OneDrive dir in profile");
    assert_eq!(m.placeholders.paths.len(), 3);
    assert!(m
        .placeholders
        .paths
        .contains(&"Documents/stub-a.txt".to_string()));
    let flagged: Vec<&str> = m
        .entries
        .iter()
        .filter(|e| e.placeholder)
        .map(|e| e.relative_path.as_str())
        .collect();
    assert_eq!(flagged.len(), 3);
    assert!(
        !m.entries
            .iter()
            .find(|e| e.relative_path == "Documents/real.txt")
            .unwrap()
            .placeholder
    );
    assert_eq!(
        m.placeholders.bytes,
        m.entries
            .iter()
            .filter(|e| e.placeholder)
            .map(|e| e.size_bytes)
            .sum::<u64>()
    );

    // report.txt: placeholders section, OneDrive named, plain-words warning
    let txt = fs::read_to_string(out.join("report.txt")).unwrap();
    assert!(
        txt.contains("PLACEHOLDERS (ONEDRIVE FILES ON-DEMAND)"),
        "txt: {}",
        txt
    );
    assert!(txt.contains("count: 3"), "txt: {}", txt);
    assert!(txt.contains("- Documents/stub-a.txt"));
    assert!(txt.contains("online-only placeholders"));
    assert!(txt.contains("Always keep on this device"));
    assert!(txt.contains("re-run the scan"));
    // manifest.json carries the full list
    let mj = fs::read_to_string(out.join("manifest.json")).unwrap();
    assert!(mj.contains("\"placeholders\""));
    assert!(mj.contains("stub-b.txt"));
}

#[test]
fn scan_without_placeholders_has_empty_section_and_no_warning() {
    let fx = Fixture::new();
    let root = build_profile(&fx);
    let out = fx.0.join("out");
    // no stubs: every file is real (non-Windows default read is None)
    let res = run_scan(&root, &Config::default_config(), &out, None).expect("scan failed");
    assert_eq!(res.manifest.placeholders.count, 0);
    assert!(res.manifest.placeholders.paths.is_empty());
    assert!(res.manifest.entries.iter().all(|e| !e.placeholder));
    let txt = fs::read_to_string(out.join("report.txt")).unwrap();
    assert!(
        !txt.contains("PLACEHOLDERS"),
        "section must be additive: {}",
        txt
    );
    // manifest field is always present and may be 0/empty
    let mj = fs::read_to_string(out.join("manifest.json")).unwrap();
    assert!(mj.contains("\"placeholders\""));
    assert!(mj.contains("\"count\": 0"));
}

#[test]
fn stage_refuses_still_placeholder_sources_and_reports_incomplete() {
    let fx = Fixture::new();
    let root = build_profile(&fx);
    let stubs = Arc::new(HashMap::from([
        (
            "Documents/stub-a.txt".to_string(),
            FILE_ATTRIBUTE_RECALL_ON_OPEN,
        ),
        ("Documents/stub-b.txt".to_string(), FILE_ATTRIBUTE_OFFLINE),
    ]));
    let out = fx.0.join("out");
    run_scan_with_attrs(
        &root,
        &Config::default_config(),
        &out,
        None,
        &stub_attrs(stubs.clone()),
    )
    .expect("scan failed");
    let manifest_path = out.join("manifest.json");

    let staging = fx.0.join("staging");
    let outcome =
        run_stage_with(&manifest_path, &staging, &stub_attrs(stubs)).expect("stage failed");
    // exit convention for skipped categories: 2 (run completed with skips)
    assert_eq!(outcome.exit_code, 2, "summary: {}", outcome.summary);
    assert!(
        outcome.summary.contains("INCOMPLETE"),
        "summary: {}",
        outcome.summary
    );
    assert!(outcome.summary.contains("online-only placeholders"));
    assert!(outcome.summary.contains("Always keep on this device"));
    let r = &outcome.report;
    assert_eq!(r.skipped_placeholder.count, 2);
    assert_eq!(r.skipped_placeholder_files.len(), 2);
    assert!(r
        .skipped_placeholder_files
        .iter()
        .any(|p| p.path == "Documents/stub-a.txt"));
    assert!(r
        .skipped_placeholder_files
        .iter()
        .all(|p| p.reason.contains("NOT on this machine")));
    assert_eq!(r.copied_verified.count, 2, "only the real files are copied");
    // the stubs were NOT copied
    let stage = staging.join(STAGE_DIR_NAME);
    assert!(!stage.join("Documents/stub-a.txt").exists());
    assert!(!stage.join("Documents/stub-b.txt").exists());
    assert!(stage.join("Documents/real.txt").exists());
    assert!(stage.join("Pictures/photo.png").exists());
    // txt report lists the category
    let txt = fs::read_to_string(stage.join("copy-report.txt")).unwrap();
    assert!(txt.contains("skipped_placeholder: 2"), "txt: {}", txt);
    assert!(txt.contains("SKIPPED (PLACEHOLDER"));
}

#[test]
fn stage_copies_flagged_but_now_real_sources_normally() {
    let fx = Fixture::new();
    let root = build_profile(&fx);
    // scan time: stub-a is a placeholder
    let stubs_at_scan = Arc::new(HashMap::from([(
        "Documents/stub-a.txt".to_string(),
        FILE_ATTRIBUTE_RECALL_ON_OPEN,
    )]));
    let out = fx.0.join("out");
    run_scan_with_attrs(
        &root,
        &Config::default_config(),
        &out,
        None,
        &stub_attrs(stubs_at_scan),
    )
    .expect("scan failed");
    let manifest_path = out.join("manifest.json");

    // stage time: the user downloaded it — the live read says it is real now
    let stubs_at_stage: Arc<HashMap<String, u32>> = Arc::new(HashMap::new());
    let staging = fx.0.join("staging");
    let outcome = run_stage_with(&manifest_path, &staging, &stub_attrs(stubs_at_stage))
        .expect("stage failed");
    assert_eq!(outcome.exit_code, 0, "summary: {}", outcome.summary);
    assert_eq!(outcome.report.skipped_placeholder.count, 0);
    assert!(!outcome.summary.contains("INCOMPLETE"));
    // flagged at scan but real at stage: copied + hash-verified normally
    assert_eq!(outcome.report.copied_verified.count, 4);
    let stage = staging.join(STAGE_DIR_NAME);
    assert_eq!(
        fs::read(stage.join("Documents/stub-a.txt")).unwrap(),
        b"stub"
    );
    // the manifest flag still decides the warning/note
    assert!(outcome
        .summary
        .contains("placeholders at scan time but real at stage time"));
    let m: Manifest = serde_json::from_str(&fs::read_to_string(&manifest_path).unwrap()).unwrap();
    assert_eq!(
        m.entries.iter().filter(|e| e.placeholder).count(),
        1,
        "manifest flag preserved"
    );
}
