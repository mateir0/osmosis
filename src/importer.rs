//! The importer (slice 3): `osmosis import` unpacks a staged tree
//! (`<stage>/` containing `manifest.json`) into the user's Linux home,
//! fixes permissions, re-verifies every hash, and issues a receipt.
//!
//! WRITE-PATH RULE: the stage is opened read-only, always — the importer never
//! moves, deletes, or modifies anything under `--stage`. The only write targets
//! are files under the canonicalized `--home` (each proven contained by the same
//! guard stage uses) and the receipt directory `<home>/osmosis-import/`.
//! Nothing outside the target home is ever written, and no root/sudo is needed.

use crate::classify::normalize_rel;
use crate::model::Manifest;
use crate::report::human;
use crate::stage::{
    abs_lexical, copy_file_verified, hash_file, resolve_existing, CategoryTotals, CopyFail,
    FileCopy, ProblemFile,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs::File;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;

pub const IMPORT_DIR_NAME: &str = "osmosis-import";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportReport {
    pub schema_version: u32,
    pub created_at: String,
    pub manifest: String,
    pub stage_root: String,
    pub home: String,
    pub dry_run: bool,
    pub total_files: u64,
    pub total_bytes: u64,
    pub placed_verified: CategoryTotals,
    pub already_present: CategoryTotals,
    pub conflicts: CategoryTotals,
    pub failed: CategoryTotals,
    pub conflict_files: Vec<ProblemFile>,
    pub failed_files: Vec<ProblemFile>,
}

#[derive(Debug)]
pub struct ImportOutcome {
    pub report: ImportReport,
    pub summary: String,
    /// 0 = clean; 2 = completed with conflicts or failures. Hard refusals are
    /// returned as `Err` by `run_import` and exit 1 at the CLI.
    pub exit_code: u8,
}

/// Map a manifest `linux_dest` ("~/Documents/report.pdf") to the home-relative
/// path ("Documents/report.pdf"). Any component that is not a normal path
/// component (`..`, root, prefix, cur-dir) is a hard error: the destination
/// would escape the target home.
pub fn rel_from_dest(linux_dest: &str) -> Result<String, String> {
    let rel = linux_dest.strip_prefix("~/").unwrap_or(linux_dest);
    if rel.is_empty() {
        return Err(format!(
            "linux_dest escapes the target home (empty destination): {}",
            linux_dest
        ));
    }
    let p = Path::new(rel);
    if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!(
            "linux_dest escapes the target home: {}",
            linux_dest
        ));
    }
    Ok(rel.replace('\\', "/"))
}

/// Pure permission plan: home-relative path + sensitivity -> unix mode.
/// Directories are 0755 (the `.ssh` directory itself 0700). Files are 0644,
/// except: `.ssh` contents 0600 (`*.pub` 0644), sensitive entries 0600, and
/// scripts (`*.sh`, anything under a `bin` directory) 0755.
pub fn permission_plan(rel: &str, sensitive: bool, is_dir: bool) -> u32 {
    const DIR: u32 = 0o755;
    const FILE: u32 = 0o644;
    const PRIVATE: u32 = 0o600;
    const EXEC: u32 = 0o755;

    let comps: Vec<&str> = rel.split('/').collect();
    if is_dir {
        return if comps.len() == 1 && comps[0] == ".ssh" {
            0o700
        } else {
            DIR
        };
    }
    let name = comps.last().copied().unwrap_or("");
    if comps.len() >= 2 && comps[0] == ".ssh" {
        return if name.ends_with(".pub") {
            FILE
        } else {
            PRIVATE
        };
    }
    if sensitive {
        return PRIVATE;
    }
    if name.ends_with(".sh") {
        return EXEC;
    }
    if comps[..comps.len() - 1].contains(&"bin") {
        return EXEC;
    }
    FILE
}

#[cfg(unix)]
fn apply_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn apply_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

/// Join a validated home-relative path under the canonical home, re-proving
/// containment by canonicalizing the deepest existing ancestor. Any escape is a
/// hard error — never a write.
fn safe_join_under_home(home_canon: &Path, rel: &str) -> Result<PathBuf, String> {
    let candidate = home_canon.join(rel);
    let resolved = resolve_existing(&candidate);
    if resolved.starts_with(home_canon) {
        Ok(resolved)
    } else {
        Err(format!(
            "target path escapes the home directory {}: {}",
            home_canon.display(),
            rel
        ))
    }
}

/// Insert every ancestor directory of a file's home-relative path.
fn push_ancestor_dirs(rel: &str, out: &mut BTreeSet<String>) {
    let comps: Vec<&str> = rel.split('/').collect();
    let mut cur = String::new();
    for c in &comps[..comps.len().saturating_sub(1)] {
        if !cur.is_empty() {
            cur.push('/');
        }
        cur.push_str(c);
        out.insert(cur.clone());
    }
}

struct PlanItem {
    /// Home-relative target path (forward slashes), also used in the receipt.
    rel: String,
    /// Staged source file (read-only).
    src: PathBuf,
    /// Bytes accounted for this file (manifest size for entries, staged size for
    /// files inside a `keep_dirs` subtree).
    size: u64,
    manifest_hash: Option<String>,
    sensitive: bool,
}

struct Plan {
    items: Vec<PlanItem>,
    /// Staged `keep_dirs` subtrees missing entirely: (rel, file_count, bytes).
    missing_dirs: Vec<(String, u64, u64)>,
}

fn build_plan(stage_root: &Path, manifest: &Manifest) -> Result<Plan, String> {
    let mut items = Vec::new();
    for e in &manifest.entries {
        let rel = rel_from_dest(&e.linux_dest)?;
        items.push(PlanItem {
            src: stage_root.join(&rel),
            rel,
            size: e.size_bytes,
            manifest_hash: e.hash.clone(),
            sensitive: e.sensitive,
        });
    }
    let mut missing_dirs = Vec::new();
    for d in &manifest.keep_dirs {
        let rel = rel_from_dest(&d.linux_dest)?;
        let src_dir = stage_root.join(&rel);
        if !src_dir.is_dir() {
            missing_dirs.push((rel, d.file_count, d.size_bytes));
            continue;
        }
        for res in WalkDir::new(&src_dir).follow_links(false) {
            let entry = match res {
                Ok(e) => e,
                Err(_) => continue,
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let inner = normalize_rel(entry.path(), &src_dir);
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            items.push(PlanItem {
                rel: format!("{}/{}", rel, inner),
                src: entry.path().to_path_buf(),
                size,
                manifest_hash: None,
                sensitive: false,
            });
        }
    }
    Ok(Plan {
        items,
        missing_dirs,
    })
}

struct Acc {
    files_done: u64,
    bytes_done: u64,
    total_files: u64,
    total_bytes: u64,
    placed: CategoryTotals,
    already: CategoryTotals,
    conflicts: CategoryTotals,
    failed: CategoryTotals,
    conflict_files: Vec<ProblemFile>,
    failed_files: Vec<ProblemFile>,
    /// (target path, mode) for files this run placed or confirmed in place.
    file_modes: Vec<(PathBuf, u32)>,
    /// Home-relative directories to chmod after placement.
    dirs: BTreeSet<String>,
}

impl Acc {
    fn new(total_files: u64, total_bytes: u64) -> Acc {
        Acc {
            files_done: 0,
            bytes_done: 0,
            total_files,
            total_bytes,
            placed: CategoryTotals::default(),
            already: CategoryTotals::default(),
            conflicts: CategoryTotals::default(),
            failed: CategoryTotals::default(),
            conflict_files: Vec::new(),
            failed_files: Vec::new(),
            file_modes: Vec::new(),
            dirs: BTreeSet::new(),
        }
    }

    fn progress(&mut self, bytes: u64) {
        self.files_done += 1;
        self.bytes_done += bytes;
        if self.files_done.is_multiple_of(500) {
            eprintln!(
                "osmosis: progress - {} / {} files ({} / {})",
                self.files_done,
                self.total_files,
                human(self.bytes_done),
                human(self.total_bytes)
            );
        }
    }

    fn record_conflict(&mut self, path: &str, bytes: u64, reason: String) {
        self.conflicts.count += 1;
        self.conflicts.bytes += bytes;
        self.conflict_files.push(ProblemFile {
            path: path.to_string(),
            reason,
        });
    }

    fn record_failed(&mut self, path: &str, bytes: u64, reason: String) {
        self.failed.count += 1;
        self.failed.bytes += bytes;
        self.failed_files.push(ProblemFile {
            path: path.to_string(),
            reason,
        });
    }
}

/// True when an existing target already matches the staged content: against the
/// manifest hash when present, otherwise against a fresh staged-source hash.
fn target_matches(item: &PlanItem, target: &Path) -> std::io::Result<bool> {
    let placed_hash = hash_file(target)?;
    match &item.manifest_hash {
        Some(mh) => Ok(placed_hash == *mh),
        None => Ok(hash_file(&item.src)? == placed_hash),
    }
}

/// Dry-run: classify one planned file without writing anything.
fn classify_dry(item: &PlanItem, home_canon: &Path, acc: &mut Acc) -> Result<(), String> {
    let target = safe_join_under_home(home_canon, &item.rel)?;
    push_ancestor_dirs(&item.rel, &mut acc.dirs);
    if std::fs::metadata(&item.src).is_err() {
        acc.record_failed(&item.rel, item.size, "staged source is missing".to_string());
        acc.progress(item.size);
        return Ok(());
    }
    if target.exists() {
        match target_matches(item, &target) {
            Ok(true) => {
                acc.already.count += 1;
                acc.already.bytes += item.size;
            }
            Ok(false) => acc.record_conflict(
                &item.rel,
                item.size,
                "target already exists with different content (hash mismatch)".to_string(),
            ),
            Err(err) => acc.record_failed(
                &item.rel,
                item.size,
                format!("cannot hash existing target: {}", err),
            ),
        }
    } else {
        // "to place" — labelled to_place in the dry-run report
        acc.placed.count += 1;
        acc.placed.bytes += item.size;
    }
    acc.progress(item.size);
    Ok(())
}

/// Real run: place one planned file (or classify an existing target) and verify.
fn import_one(item: &PlanItem, home_canon: &Path, acc: &mut Acc) -> Result<(), String> {
    let target = safe_join_under_home(home_canon, &item.rel)?;
    push_ancestor_dirs(&item.rel, &mut acc.dirs);

    if std::fs::metadata(&item.src).is_err() {
        acc.record_failed(
            &item.rel,
            item.size,
            "staged source is missing or unreadable".to_string(),
        );
        acc.progress(item.size);
        return Ok(());
    }

    if target.exists() {
        match target_matches(item, &target) {
            Ok(true) => {
                acc.already.count += 1;
                acc.already.bytes += item.size;
                acc.file_modes
                    .push((target, permission_plan(&item.rel, item.sensitive, false)));
            }
            Ok(false) => acc.record_conflict(
                &item.rel,
                item.size,
                "target already exists with different content (hash mismatch); not overwritten"
                    .to_string(),
            ),
            Err(err) => acc.record_failed(
                &item.rel,
                item.size,
                format!("cannot hash existing target: {}", err),
            ),
        }
        acc.progress(item.size);
        return Ok(());
    }

    match copy_file_verified(&item.src, &target, item.manifest_hash.as_deref()) {
        Ok(FileCopy::Verified { .. }) => {
            acc.placed.count += 1;
            acc.placed.bytes += item.size;
            acc.file_modes
                .push((target, permission_plan(&item.rel, item.sensitive, false)));
        }
        Ok(FileCopy::Mismatch { expected, actual }) => {
            acc.record_failed(
                &item.rel,
                item.size,
                format!("hash mismatch: expected {}, placed {}", expected, actual),
            );
        }
        Err(CopyFail::SourceOpen(err)) => {
            acc.record_failed(
                &item.rel,
                item.size,
                format!("cannot open staged source: {}", err),
            );
        }
        Err(CopyFail::Other(msg)) => {
            acc.record_failed(&item.rel, item.size, msg);
        }
    }
    acc.progress(item.size);
    Ok(())
}

/// Apply the permission plan after placement. Files this run placed or
/// confirmed get their planned file mode; every directory that received
/// content gets its planned directory mode. Never touches anything outside the
/// home (all paths come from the validated plan).
fn apply_permissions(home_canon: &Path, acc: &Acc) -> Result<(), String> {
    for (path, mode) in &acc.file_modes {
        if let Err(e) = apply_mode(path, *mode) {
            eprintln!(
                "osmosis: warning: cannot set mode {:04o} on {}: {}",
                mode,
                path.display(),
                e
            );
        }
    }
    for rel in &acc.dirs {
        let dir = safe_join_under_home(home_canon, rel)?;
        if dir.is_dir() {
            if let Err(e) = apply_mode(&dir, permission_plan(rel, false, true)) {
                eprintln!(
                    "osmosis: warning: cannot set mode on {}: {}",
                    dir.display(),
                    e
                );
            }
        }
    }
    Ok(())
}

fn now_string() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn assemble_report(
    manifest_path: &Path,
    stage_root: &Path,
    home_canon: &Path,
    dry_run: bool,
    total_files: u64,
    total_bytes: u64,
    acc: Acc,
) -> ImportReport {
    ImportReport {
        schema_version: 1,
        created_at: now_string(),
        manifest: manifest_path.to_string_lossy().into_owned(),
        stage_root: stage_root.to_string_lossy().into_owned(),
        home: home_canon.to_string_lossy().into_owned(),
        dry_run,
        total_files,
        total_bytes,
        placed_verified: acc.placed,
        already_present: acc.already,
        conflicts: acc.conflicts,
        failed: acc.failed,
        conflict_files: acc.conflict_files,
        failed_files: acc.failed_files,
    }
}

fn write_receipt_files(dir: &Path, report: &ImportReport, summary: &str) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| format!("cannot serialize import report: {}", e))?;
    let jp = dir.join("import-report.json");
    let mut f = File::create(&jp).map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    f.write_all(json.as_bytes())
        .and_then(|_| f.write_all(b"\n"))
        .map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    let tp = dir.join("import-report.txt");
    let mut f = File::create(&tp).map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;
    f.write_all(summary.as_bytes())
        .map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;
    Ok(())
}

fn exit_code_of(report: &ImportReport) -> u8 {
    if report.failed.count == 0 && report.conflicts.count == 0 {
        0
    } else {
        2
    }
}

/// `osmosis import --stage <dir> --home <dir> [--dry-run]`: copy every KEEP
/// entry from the staged tree into the target home, hash-verify the placed
/// files, fix permissions, and write `<home>/osmosis-import/import-report.*`.
/// The stage is read-only; nothing outside the home is written.
pub fn run_import(stage_dir: &Path, home: &Path, dry_run: bool) -> Result<ImportOutcome, String> {
    let manifest_path = stage_dir.join("manifest.json");
    if !manifest_path.is_file() {
        return Err(format!(
            "stage directory {} does not contain manifest.json — pass the osmosis-stage folder as --stage",
            stage_dir.display()
        ));
    }
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("cannot read manifest {}: {}", manifest_path.display(), e))?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse manifest {}: {}", manifest_path.display(), e))?;
    if manifest.schema_version != 1 {
        return Err(format!(
            "unsupported manifest schema_version {} (expected 1)",
            manifest.schema_version
        ));
    }

    let stage_root = stage_dir.canonicalize().map_err(|e| {
        format!(
            "cannot canonicalize stage dir {}: {}",
            stage_dir.display(),
            e
        )
    })?;
    if !home.is_dir() {
        return Err(format!(
            "target home is not a directory: {}",
            home.display()
        ));
    }
    let home_c = home
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize target home {}: {}", home.display(), e))?;

    // refusal: the stage and the home must not contain one another
    let stage_l = abs_lexical(&stage_root);
    let home_l = abs_lexical(&home_c);
    if stage_root == home_c || stage_l.starts_with(&home_l) || home_l.starts_with(&stage_l) {
        return Err(format!(
            "refusing to import: stage dir {} overlaps the target home {}",
            stage_dir.display(),
            home.display()
        ));
    }

    // escape guard on EVERY destination, before anything is created
    for e in &manifest.entries {
        rel_from_dest(&e.linux_dest)?;
    }
    for d in &manifest.keep_dirs {
        rel_from_dest(&d.linux_dest)?;
    }

    let plan = build_plan(&stage_root, &manifest)?;
    let total_files = manifest.entries.len() as u64
        + manifest.keep_dirs.iter().map(|d| d.file_count).sum::<u64>();
    let total_bytes = manifest.totals.keep_bytes;

    let mut acc = Acc::new(total_files, total_bytes);
    for (rel, count, bytes) in &plan.missing_dirs {
        acc.failed.count += count;
        acc.failed.bytes += bytes;
        acc.failed_files.push(ProblemFile {
            path: rel.clone(),
            reason: "staged directory is missing".to_string(),
        });
        acc.files_done += count;
        acc.bytes_done += bytes;
    }

    if dry_run {
        for item in &plan.items {
            classify_dry(item, &home_c, &mut acc)?;
        }
        let report = assemble_report(
            &manifest_path,
            &stage_root,
            &home_c,
            true,
            total_files,
            total_bytes,
            acc,
        );
        let summary = build_import_report_txt(&report);
        return Ok(ImportOutcome {
            exit_code: exit_code_of(&report),
            report,
            summary,
        });
    }

    for item in &plan.items {
        import_one(item, &home_c, &mut acc)?;
    }
    apply_permissions(&home_c, &acc)?;

    // the receipt is the only other thing this module writes, always under home
    let receipt_dir = safe_join_under_home(&home_c, IMPORT_DIR_NAME)?;
    std::fs::create_dir_all(&receipt_dir)
        .map_err(|e| format!("cannot create {}: {}", receipt_dir.display(), e))?;

    let report = assemble_report(
        &manifest_path,
        &stage_root,
        &home_c,
        false,
        total_files,
        total_bytes,
        acc,
    );
    let summary = build_import_report_txt(&report);
    write_receipt_files(&receipt_dir, &report, &summary)?;
    Ok(ImportOutcome {
        exit_code: exit_code_of(&report),
        report,
        summary,
    })
}

/// Human-readable import report, same style as copy-report.txt.
pub fn build_import_report_txt(r: &ImportReport) -> String {
    let mut s = String::new();
    s.push_str("OSmosis import report\n");
    s.push_str("=====================\n");
    s.push_str(&format!("manifest: {}\n", r.manifest));
    s.push_str(&format!("stage_root: {}\n", r.stage_root));
    s.push_str(&format!("home: {}\n", r.home));
    s.push_str(&format!("created_at: {}\n", r.created_at));
    s.push_str(&format!("dry_run: {}\n\n", r.dry_run));

    s.push_str("Totals\n");
    s.push_str("------\n");
    s.push_str(&format!(
        "total:             {} ({})\n",
        r.total_files,
        human(r.total_bytes)
    ));
    let place_label = if r.dry_run {
        "to_place:          "
    } else {
        "placed_verified:   "
    };
    s.push_str(&format!(
        "{}{} ({})\n",
        place_label,
        r.placed_verified.count,
        human(r.placed_verified.bytes)
    ));
    s.push_str(&format!(
        "already_present:   {} ({})\n",
        r.already_present.count,
        human(r.already_present.bytes)
    ));
    s.push_str(&format!(
        "conflicts:         {} ({})\n",
        r.conflicts.count,
        human(r.conflicts.bytes)
    ));
    s.push_str(&format!(
        "failed:            {} ({})\n\n",
        r.failed.count,
        human(r.failed.bytes)
    ));

    s.push_str("CONFLICTS\n");
    s.push_str("---------\n");
    if r.conflict_files.is_empty() {
        s.push_str("(none)\n");
    }
    for p in &r.conflict_files {
        s.push_str(&format!("- {}: {}\n", p.path, p.reason));
    }
    s.push('\n');
    s.push_str("FAILED\n");
    s.push_str("------\n");
    if r.failed_files.is_empty() {
        s.push_str("(none)\n");
    }
    for p in &r.failed_files {
        s.push_str(&format!("- {}: {}\n", p.path, p.reason));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rel_strips_home_and_accepts_normal_paths() {
        assert_eq!(
            rel_from_dest("~/Documents/report.pdf").unwrap(),
            "Documents/report.pdf"
        );
        assert_eq!(
            rel_from_dest("~/.ssh/id_ed25519").unwrap(),
            ".ssh/id_ed25519"
        );
        assert_eq!(
            rel_from_dest("~/.local/bin/tool").unwrap(),
            ".local/bin/tool"
        );
    }

    #[test]
    fn rel_rejects_escapes() {
        assert!(rel_from_dest("~/../evil.txt").is_err());
        assert!(rel_from_dest("~/a/../../evil.txt").is_err());
        assert!(rel_from_dest("~//etc/passwd").is_err());
        assert!(rel_from_dest("~/").is_err());
        assert!(rel_from_dest("/etc/passwd").is_err());
    }

    #[test]
    fn permission_plan_ssh_private_key_is_0600() {
        assert_eq!(permission_plan(".ssh/id_ed25519", true, false), 0o600);
        assert_eq!(permission_plan(".ssh/known_hosts", false, false), 0o600);
    }

    #[test]
    fn permission_plan_ssh_pub_is_0644() {
        assert_eq!(permission_plan(".ssh/id_ed25519.pub", false, false), 0o644);
    }

    #[test]
    fn permission_plan_sensitive_entry_is_0600() {
        assert_eq!(
            permission_plan(".config/google-chrome/Default/Cookies", true, false),
            0o600
        );
    }

    #[test]
    fn permission_plan_normal_document_is_0644() {
        assert_eq!(permission_plan("Documents/report.pdf", false, false), 0o644);
    }

    #[test]
    fn permission_plan_scripts_and_bin_are_0755() {
        assert_eq!(permission_plan("Documents/deploy.sh", false, false), 0o755);
        assert_eq!(permission_plan(".local/bin/tool", false, false), 0o755);
    }

    #[test]
    fn permission_plan_directories() {
        assert_eq!(permission_plan(".ssh", false, true), 0o700);
        assert_eq!(permission_plan("Documents", false, true), 0o755);
        assert_eq!(permission_plan(".ssh/keys", false, true), 0o755);
    }

    #[test]
    fn report_txt_mirrors_totals() {
        let r = ImportReport {
            schema_version: 1,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            manifest: "/s/manifest.json".to_string(),
            stage_root: "/s".to_string(),
            home: "/h".to_string(),
            dry_run: false,
            total_files: 3,
            total_bytes: 30,
            placed_verified: CategoryTotals {
                count: 2,
                bytes: 20,
            },
            already_present: CategoryTotals {
                count: 1,
                bytes: 10,
            },
            conflicts: CategoryTotals::default(),
            failed: CategoryTotals::default(),
            conflict_files: Vec::new(),
            failed_files: Vec::new(),
        };
        let txt = build_import_report_txt(&r);
        assert!(txt.contains("placed_verified:   2 (20 B)"));
        assert!(txt.contains("already_present:   1 (10 B)"));
        assert!(txt.contains("CONFLICTS\n---------\n(none)"));
        assert!(txt.contains("FAILED\n------\n(none)"));
    }
}
