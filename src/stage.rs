//! The copier (slice 2): reads a manifest produced by `osmosis scan` and stages
//! every keeper onto an external drive, hash-verified and resumable.
//!
//! WRITE-PATH RULE: sources are opened read-only, always. The ONLY directory
//! tree this module ever writes is `<staging>/osmosis-stage/`. Every destination
//! path is proven to live under the canonicalized staging root before any byte
//! is written; a path that escapes the staging root is a hard error, never a
//! write. Nothing is ever deleted, moved, or renamed — neither on the source
//! side nor inside the staging root.

use crate::classify::normalize_rel;
use crate::model::Manifest;
use crate::report::human;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};
use walkdir::WalkDir;
use xxhash_rust::xxh64::Xxh64;

pub const STAGE_DIR_NAME: &str = "osmosis-stage";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CategoryTotals {
    pub count: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProblemFile {
    pub path: String,
    pub reason: String,
}

/// Source + staged hashes recorded for KEEP files that had no manifest hash
/// (files above the scanner's hash threshold).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileHashes {
    pub path: String,
    pub source_hash: String,
    pub staged_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyReport {
    pub schema_version: u32,
    pub created_at: String,
    pub manifest: String,
    pub stage_root: String,
    /// "ok" | "insufficient_space" (precheck failed: report shell only, nothing copied)
    pub precheck: String,
    pub shortfall_bytes: u64,
    pub total_files: u64,
    pub total_bytes: u64,
    pub copied_verified: CategoryTotals,
    pub already_verified: CategoryTotals,
    pub verify_failed: CategoryTotals,
    pub skipped_locked: CategoryTotals,
    pub skipped_missing: CategoryTotals,
    pub copied_unhashed_hashes: Vec<FileHashes>,
    pub verify_failed_files: Vec<ProblemFile>,
    pub skipped_locked_files: Vec<ProblemFile>,
    pub skipped_missing_files: Vec<ProblemFile>,
}

#[derive(Debug)]
pub struct StageOutcome {
    pub report: CopyReport,
    pub summary: String,
    /// 0 = everything verified; 1 = hard refusal / precheck failure; 2 = run
    /// completed but verify_failed or skipped_* > 0 (rerun to finish).
    pub exit_code: u8,
}

enum CopyFail {
    SourceOpen(io::Error),
    Other(String),
}

enum FileCopy {
    Verified {
        source_hash: Option<String>,
        staged_hash: String,
    },
    Mismatch {
        expected: String,
        actual: String,
    },
}

/// xxh64 of a file's contents, streamed. Same encoding as the scanner (`{:016x}`).
fn hash_file(path: &Path) -> io::Result<String> {
    let mut f = File::open(path)?;
    let mut h = Xxh64::new(0);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:016x}", h.digest()))
}

/// Map a manifest `linux_dest` ("~/Documents/report.pdf") to the staging-relative
/// path ("Documents/report.pdf"). Any component that is not a normal path
/// component (`..`, root, prefix, cur-dir) is a hard error: the destination would
/// escape the staging root.
fn stage_rel_from_dest(linux_dest: &str) -> Result<String, String> {
    let rel = linux_dest.strip_prefix("~/").unwrap_or(linux_dest);
    if rel.is_empty() {
        return Err(format!(
            "linux_dest escapes the staging root (empty destination): {}",
            linux_dest
        ));
    }
    let p = Path::new(rel);
    if p.components().any(|c| !matches!(c, Component::Normal(_))) {
        return Err(format!(
            "linux_dest escapes the staging root: {}",
            linux_dest
        ));
    }
    Ok(rel.replace('\\', "/"))
}

/// Lexically absolutize a path (resolve `.`/`..` without touching the fs).
fn abs_lexical(p: &Path) -> PathBuf {
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(p)
    };
    let mut out = PathBuf::new();
    for comp in abs.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Canonicalize the deepest existing ancestor of `p` and append the remaining
/// components, so the result is in the same (canonical) form as a canonicalized
/// existing directory.
fn resolve_existing(p: &Path) -> PathBuf {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut anc = p;
    loop {
        if anc.symlink_metadata().is_ok() {
            break;
        }
        match anc.file_name() {
            Some(n) => {
                tail.push(n.to_os_string());
                match anc.parent() {
                    Some(par) => anc = par,
                    None => return abs_lexical(p),
                }
            }
            None => return abs_lexical(p),
        }
    }
    match anc.canonicalize() {
        Ok(c) => {
            let mut out = c;
            for t in tail.iter().rev() {
                out.push(t);
            }
            out
        }
        Err(_) => abs_lexical(p),
    }
}

/// Join `rel` (already validated by `stage_rel_from_dest`) under the canonical
/// staging root, re-proving containment by canonicalizing the deepest existing
/// ancestor. Any escape is a hard error — never a write.
fn safe_join_under(stage_root_canon: &Path, rel: &str) -> Result<PathBuf, String> {
    let candidate = stage_root_canon.join(rel);
    let resolved = resolve_existing(&candidate);
    if resolved.starts_with(stage_root_canon) {
        Ok(resolved)
    } else {
        Err(format!(
            "staged path escapes the staging root {}: {}",
            stage_root_canon.display(),
            rel
        ))
    }
}

/// Stream-copy `src` to `dest` (dest already proven under the staging root),
/// hashing the source stream on the way. Then hash the staged file and compare:
/// against the manifest hash when one exists, otherwise against the source
/// stream hash. Sets the staged mtime to the source mtime where the platform
/// allows. Contents only — no ACLs.
fn copy_file_verified(
    src: &Path,
    dest: &Path,
    manifest_hash: Option<&str>,
) -> Result<FileCopy, CopyFail> {
    let mut f = File::open(src).map_err(CopyFail::SourceOpen)?;
    if let Some(parent) = dest.parent() {
        fs_create_dir_all(parent)?;
    }
    let mut out = File::create(dest)
        .map_err(|e| CopyFail::Other(format!("create {}: {}", dest.display(), e)))?;
    let mut h_src = Xxh64::new(0);
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| CopyFail::Other(format!("read {}: {}", src.display(), e)))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n])
            .map_err(|e| CopyFail::Other(format!("write {}: {}", dest.display(), e)))?;
        h_src.update(&buf[..n]);
    }
    out.flush()
        .map_err(|e| CopyFail::Other(format!("flush {}: {}", dest.display(), e)))?;
    if let Ok(md) = f.metadata() {
        if let Ok(mt) = md.modified() {
            let _ = out.set_modified(mt);
        }
    }
    drop(out);
    let staged_hash = hash_file(dest)
        .map_err(|e| CopyFail::Other(format!("hash staged {}: {}", dest.display(), e)))?;
    let src_hash = format!("{:016x}", h_src.digest());
    match manifest_hash {
        Some(mh) => {
            if staged_hash == mh {
                Ok(FileCopy::Verified {
                    source_hash: None,
                    staged_hash,
                })
            } else {
                Ok(FileCopy::Mismatch {
                    expected: mh.to_string(),
                    actual: staged_hash,
                })
            }
        }
        None => {
            if staged_hash == src_hash {
                Ok(FileCopy::Verified {
                    source_hash: Some(src_hash),
                    staged_hash,
                })
            } else {
                Ok(FileCopy::Mismatch {
                    expected: src_hash,
                    actual: staged_hash,
                })
            }
        }
    }
}

fn fs_create_dir_all(p: &Path) -> Result<(), CopyFail> {
    std::fs::create_dir_all(p)
        .map_err(|e| CopyFail::Other(format!("create dir {}: {}", p.display(), e)))
}

/// Classify a source-open failure: NotFound => missing, anything else
/// (sharing/lock violation os error 32/33, permissions, ...) => locked.
fn classify_open_error(e: &io::Error) -> (&'static str, String) {
    if e.kind() == io::ErrorKind::NotFound {
        ("missing", e.to_string())
    } else {
        ("locked", e.to_string())
    }
}

struct Acc {
    files_done: u64,
    bytes_done: u64,
    total_files: u64,
    total_bytes: u64,
    copied: CategoryTotals,
    already: CategoryTotals,
    failed: CategoryTotals,
    locked: CategoryTotals,
    missing: CategoryTotals,
    failed_files: Vec<ProblemFile>,
    locked_files: Vec<ProblemFile>,
    missing_files: Vec<ProblemFile>,
    unhashed_hashes: Vec<FileHashes>,
}

impl Acc {
    fn new(total_files: u64, total_bytes: u64) -> Acc {
        Acc {
            files_done: 0,
            bytes_done: 0,
            total_files,
            total_bytes,
            copied: CategoryTotals::default(),
            already: CategoryTotals::default(),
            failed: CategoryTotals::default(),
            locked: CategoryTotals::default(),
            missing: CategoryTotals::default(),
            failed_files: Vec::new(),
            locked_files: Vec::new(),
            missing_files: Vec::new(),
            unhashed_hashes: Vec::new(),
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

    fn record_locked(&mut self, path: &str, bytes: u64, reason: String) {
        self.locked.count += 1;
        self.locked.bytes += bytes;
        self.locked_files.push(ProblemFile {
            path: path.to_string(),
            reason,
        });
    }

    fn record_missing(&mut self, path: &str, bytes: u64, reason: String) {
        self.missing.count += 1;
        self.missing.bytes += bytes;
        self.missing_files.push(ProblemFile {
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

fn now_string() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn write_report_files(stage_root: &Path, report: &CopyReport, summary: &str) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| format!("cannot serialize copy report: {}", e))?;
    let jp = stage_root.join("copy-report.json");
    let mut f = File::create(&jp).map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    f.write_all(json.as_bytes())
        .and_then(|_| f.write_all(b"\n"))
        .map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    let tp = stage_root.join("copy-report.txt");
    let mut f = File::create(&tp).map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;
    f.write_all(summary.as_bytes())
        .map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;
    Ok(())
}

/// Handle one KEEP file entry (resume check + copy + verify). Returns the bytes
/// accounted for this file.
fn stage_one_entry(
    e: &crate::model::Entry,
    src: &Path,
    stage_root: &Path,
    acc: &mut Acc,
) -> Result<(), String> {
    let rel = stage_rel_from_dest(&e.linux_dest)?;
    let dest = safe_join_under(stage_root, &rel)?;
    let disp = &e.relative_path;

    let src_md = match std::fs::metadata(src) {
        Ok(m) => m,
        Err(err) => {
            let (kind, reason) = classify_open_error(&err);
            if kind == "missing" {
                acc.record_missing(disp, e.size_bytes, reason);
            } else {
                acc.record_locked(disp, e.size_bytes, reason);
            }
            acc.progress(e.size_bytes);
            return Ok(());
        }
    };
    let src_size = src_md.len();

    // resume: staged file already exists with the same size and, where a
    // manifest hash exists, a matching hash => already_verified, no copy
    if let Ok(dmd) = std::fs::metadata(&dest) {
        if dmd.len() == src_size {
            let hash_ok = match &e.hash {
                Some(mh) => hash_file(&dest).ok().as_deref() == Some(mh.as_str()),
                None => true, // not checkable (> hash threshold): size match suffices
            };
            if hash_ok {
                acc.already.count += 1;
                acc.already.bytes += src_size;
                acc.progress(src_size);
                return Ok(());
            }
        }
    }

    match copy_file_verified(src, &dest, e.hash.as_deref()) {
        Ok(FileCopy::Verified {
            source_hash,
            staged_hash,
        }) => {
            acc.copied.count += 1;
            acc.copied.bytes += src_size;
            if let Some(sh) = source_hash {
                acc.unhashed_hashes.push(FileHashes {
                    path: disp.clone(),
                    source_hash: sh,
                    staged_hash,
                });
            }
        }
        Ok(FileCopy::Mismatch { expected, actual }) => {
            acc.record_failed(
                disp,
                src_size,
                format!("hash mismatch: expected {}, staged {}", expected, actual),
            );
        }
        Err(CopyFail::SourceOpen(err)) => {
            let (kind, reason) = classify_open_error(&err);
            if kind == "missing" {
                acc.record_missing(disp, src_size, reason);
            } else {
                acc.record_locked(disp, src_size, reason);
            }
        }
        Err(CopyFail::Other(msg)) => {
            acc.record_failed(disp, src_size, msg);
        }
    }
    acc.progress(src_size);
    Ok(())
}

/// Copy one `keep_dirs` aggregate (a `.git` subtree) recursively. Every file
/// inside is stream-copied and verified (source-stream hash vs staged hash —
/// these files have no manifest hashes). The entry is verified only when every
/// file inside matches.
fn stage_one_keep_dir(
    d: &crate::model::KeepDirEntry,
    root: &Path,
    stage_root: &Path,
    acc: &mut Acc,
) -> Result<(), String> {
    let src_dir = root.join(&d.path);
    let rel = stage_rel_from_dest(&d.linux_dest)?;
    let mut seen: u64 = 0;
    for res in WalkDir::new(&src_dir).follow_links(false) {
        let entry = match res {
            Ok(e) => e,
            Err(err) => {
                let p = err
                    .path()
                    .map(|p| normalize_rel(p, root))
                    .unwrap_or_else(|| d.path.clone());
                acc.record_missing(&p, 0, err.to_string());
                continue;
            }
        };
        if !entry.file_type().is_file() {
            continue;
        }
        seen += 1;
        let inner = normalize_rel(entry.path(), &src_dir);
        let disp = format!("{}/{}", d.path, inner);
        let dest_rel = format!("{}/{}", rel, inner);
        let dest = safe_join_under(stage_root, &dest_rel)?;
        let src = entry.path();
        let src_md = match std::fs::metadata(src) {
            Ok(m) => m,
            Err(err) => {
                let (kind, reason) = classify_open_error(&err);
                if kind == "missing" {
                    acc.record_missing(&disp, 0, reason);
                } else {
                    acc.record_locked(&disp, 0, reason);
                }
                acc.progress(0);
                continue;
            }
        };
        let src_size = src_md.len();
        // resume: same size, no checkable hash for these files
        if let Ok(dmd) = std::fs::metadata(&dest) {
            if dmd.len() == src_size {
                acc.already.count += 1;
                acc.already.bytes += src_size;
                acc.progress(src_size);
                continue;
            }
        }
        match copy_file_verified(src, &dest, None) {
            Ok(FileCopy::Verified {
                source_hash,
                staged_hash,
            }) => {
                acc.copied.count += 1;
                acc.copied.bytes += src_size;
                if let Some(sh) = source_hash {
                    acc.unhashed_hashes.push(FileHashes {
                        path: disp.clone(),
                        source_hash: sh,
                        staged_hash,
                    });
                }
            }
            Ok(FileCopy::Mismatch { expected, actual }) => {
                acc.record_failed(
                    &disp,
                    src_size,
                    format!("hash mismatch: expected {}, staged {}", expected, actual),
                );
            }
            Err(CopyFail::SourceOpen(err)) => {
                let (kind, reason) = classify_open_error(&err);
                if kind == "missing" {
                    acc.record_missing(&disp, src_size, reason);
                } else {
                    acc.record_locked(&disp, src_size, reason);
                }
            }
            Err(CopyFail::Other(msg)) => {
                acc.record_failed(&disp, src_size, msg);
            }
        }
        acc.progress(src_size);
    }
    // files that vanished from the subtree since the scan are unaccounted for
    // otherwise — record the shortfall as skipped_missing on the aggregate
    if seen < d.file_count {
        acc.record_missing(
            &d.path,
            0,
            format!(
                "{} of {} file(s) missing from this subtree since the scan",
                d.file_count - seen,
                d.file_count
            ),
        );
    }
    Ok(())
}

/// `osmosis stage --manifest <m> --staging <dir>`: stage every keeper listed in
/// the manifest under `<staging>/osmosis-stage/`, hash-verified, resumable.
/// Writes only inside `<staging>/osmosis-stage/`; never touches a source file.
pub fn run_stage(manifest_path: &Path, staging: &Path) -> Result<StageOutcome, String> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("cannot read manifest {}: {}", manifest_path.display(), e))?;
    let manifest: Manifest = serde_json::from_str(&text)
        .map_err(|e| format!("cannot parse manifest {}: {}", manifest_path.display(), e))?;
    if manifest.schema_version != 1 {
        return Err(format!(
            "unsupported manifest schema_version {} (expected 1)",
            manifest.schema_version
        ));
    }
    let root = PathBuf::from(&manifest.root);
    if !root.is_dir() {
        return Err(format!(
            "manifest root is not a directory: {}",
            root.display()
        ));
    }

    // (1) escape guard on EVERY destination, before anything is created
    for e in &manifest.entries {
        stage_rel_from_dest(&e.linux_dest)?;
    }
    for d in &manifest.keep_dirs {
        stage_rel_from_dest(&d.linux_dest)?;
    }

    // (2) refusal: staging root inside / equal to / overlapping the scanned
    // root or any source path
    let stage_dir = staging.join(STAGE_DIR_NAME);
    let root_c = resolve_existing(&root);
    let stage_c = resolve_existing(&stage_dir);
    if stage_c == root_c || stage_c.starts_with(&root_c) || root_c.starts_with(&stage_c) {
        return Err(format!(
            "refusing to stage: staging root {} overlaps the scanned root {}",
            stage_dir.display(),
            root.display()
        ));
    }
    let stage_l = abs_lexical(&stage_dir);
    let root_l = abs_lexical(&root);
    for e in &manifest.entries {
        let src = root_l.join(&e.relative_path);
        if src.starts_with(&stage_l) || stage_l.starts_with(&src) {
            return Err(format!(
                "refusing to stage: staging root {} overlaps source {}",
                stage_dir.display(),
                e.relative_path
            ));
        }
    }
    for d in &manifest.keep_dirs {
        let src = root_l.join(&d.path);
        if src.starts_with(&stage_l) || stage_l.starts_with(&src) {
            return Err(format!(
                "refusing to stage: staging root {} overlaps source {}",
                stage_dir.display(),
                d.path
            ));
        }
    }

    // (3) create the one directory tree we are allowed to write
    std::fs::create_dir_all(&stage_dir)
        .map_err(|e| format!("cannot create staging root {}: {}", stage_dir.display(), e))?;
    let stage_root = stage_dir.canonicalize().map_err(|e| {
        format!(
            "cannot canonicalize staging root {}: {}",
            stage_dir.display(),
            e
        )
    })?;

    let total_files = manifest.entries.len() as u64
        + manifest.keep_dirs.iter().map(|d| d.file_count).sum::<u64>();
    let total_bytes = manifest.totals.keep_bytes;

    // (4) free-space precheck: on failure write ONLY the report shell and stop
    let free = fs2::available_space(&stage_root)
        .map_err(|e| format!("cannot check free space at {}: {}", stage_root.display(), e))?;
    if total_bytes > free {
        let report = CopyReport {
            schema_version: 1,
            created_at: now_string(),
            manifest: manifest_path.to_string_lossy().into_owned(),
            stage_root: stage_root.to_string_lossy().into_owned(),
            precheck: "insufficient_space".to_string(),
            shortfall_bytes: total_bytes - free,
            total_files,
            total_bytes,
            copied_verified: CategoryTotals::default(),
            already_verified: CategoryTotals::default(),
            verify_failed: CategoryTotals::default(),
            skipped_locked: CategoryTotals::default(),
            skipped_missing: CategoryTotals::default(),
            copied_unhashed_hashes: Vec::new(),
            verify_failed_files: Vec::new(),
            skipped_locked_files: Vec::new(),
            skipped_missing_files: Vec::new(),
        };
        let summary = build_copy_report_txt(&report);
        write_report_files(&stage_root, &report, &summary)?;
        eprintln!(
            "osmosis: error: staging does not fit: need {}, free {}, shortfall {}",
            human(total_bytes),
            human(free),
            human(total_bytes - free)
        );
        return Ok(StageOutcome {
            report,
            summary,
            exit_code: 1,
        });
    }

    // (5) stage the manifest itself
    std::fs::copy(manifest_path, stage_root.join("manifest.json"))
        .map_err(|e| format!("cannot copy manifest into {}: {}", stage_root.display(), e))?;

    // (6) copy + verify every keeper
    let mut acc = Acc::new(total_files, total_bytes);
    for e in &manifest.entries {
        stage_one_entry(e, &root.join(&e.relative_path), &stage_root, &mut acc)?;
    }
    for d in &manifest.keep_dirs {
        stage_one_keep_dir(d, &root, &stage_root, &mut acc)?;
    }

    // (7) copy report (json + txt) and exit code
    let report = CopyReport {
        schema_version: 1,
        created_at: now_string(),
        manifest: manifest_path.to_string_lossy().into_owned(),
        stage_root: stage_root.to_string_lossy().into_owned(),
        precheck: "ok".to_string(),
        shortfall_bytes: 0,
        total_files,
        total_bytes,
        copied_verified: acc.copied,
        already_verified: acc.already,
        verify_failed: acc.failed,
        skipped_locked: acc.locked,
        skipped_missing: acc.missing,
        copied_unhashed_hashes: acc.unhashed_hashes,
        verify_failed_files: acc.failed_files,
        skipped_locked_files: acc.locked_files,
        skipped_missing_files: acc.missing_files,
    };
    let summary = build_copy_report_txt(&report);
    write_report_files(&stage_root, &report, &summary)?;
    let exit_code = if report.verify_failed.count == 0
        && report.skipped_locked.count == 0
        && report.skipped_missing.count == 0
    {
        0
    } else {
        2
    };
    Ok(StageOutcome {
        report,
        summary,
        exit_code,
    })
}

/// Verify-only pass (no copying): re-hash every staged file and compare against
/// the manifest hash (or, for files without one, against a fresh source hash).
/// Used to re-check a staged tree without touching it.
pub fn verify_stage(manifest: &Manifest, stage_root: &Path) -> Result<CopyReport, String> {
    let stage_root = stage_root
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize staging root: {}", e))?;
    let root = PathBuf::from(&manifest.root);
    let total_files = manifest.entries.len() as u64
        + manifest.keep_dirs.iter().map(|d| d.file_count).sum::<u64>();
    let total_bytes = manifest.totals.keep_bytes;
    let mut acc = Acc::new(total_files, total_bytes);

    for e in &manifest.entries {
        let rel = stage_rel_from_dest(&e.linux_dest)?;
        let dest = safe_join_under(&stage_root, &rel)?;
        let src = root.join(&e.relative_path);
        let disp = &e.relative_path;
        match std::fs::metadata(&dest) {
            Err(err) => {
                acc.record_missing(disp, e.size_bytes, err.to_string());
            }
            Ok(dmd) => {
                let staged = hash_file(&dest)
                    .map_err(|err| format!("hash staged {}: {}", dest.display(), err))?;
                let ok = match &e.hash {
                    Some(mh) => staged == *mh,
                    None => match hash_file(&src) {
                        Ok(sh) => staged == sh,
                        Err(err) => {
                            acc.record_missing(disp, dmd.len(), err.to_string());
                            acc.progress(dmd.len());
                            continue;
                        }
                    },
                };
                if ok {
                    acc.already.count += 1;
                    acc.already.bytes += dmd.len();
                } else {
                    let expected = e
                        .hash
                        .clone()
                        .unwrap_or_else(|| "<source hash>".to_string());
                    acc.record_failed(
                        disp,
                        dmd.len(),
                        format!("hash mismatch: expected {}, staged {}", expected, staged),
                    );
                }
            }
        }
        acc.progress(e.size_bytes);
    }

    for d in &manifest.keep_dirs {
        let src_dir = root.join(&d.path);
        let rel = stage_rel_from_dest(&d.linux_dest)?;
        for res in WalkDir::new(&src_dir).follow_links(false) {
            let entry = match res {
                Ok(e) => e,
                Err(err) => {
                    let p = err
                        .path()
                        .map(|p| normalize_rel(p, &root))
                        .unwrap_or_else(|| d.path.clone());
                    acc.record_missing(&p, 0, err.to_string());
                    continue;
                }
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let inner = normalize_rel(entry.path(), &src_dir);
            let disp = format!("{}/{}", d.path, inner);
            let dest = safe_join_under(&stage_root, &format!("{}/{}", rel, inner))?;
            let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
            match std::fs::metadata(&dest) {
                Err(err) => acc.record_missing(&disp, size, err.to_string()),
                Ok(dmd) => {
                    let staged = hash_file(&dest)
                        .map_err(|e| format!("hash staged {}: {}", dest.display(), e))?;
                    let src_h = hash_file(entry.path())
                        .map_err(|e| format!("hash source {}: {}", entry.path().display(), e))?;
                    if staged == src_h {
                        acc.already.count += 1;
                        acc.already.bytes += dmd.len();
                    } else {
                        acc.record_failed(
                            &disp,
                            dmd.len(),
                            format!("hash mismatch: expected {}, staged {}", src_h, staged),
                        );
                    }
                }
            }
            acc.progress(size);
        }
    }

    Ok(CopyReport {
        schema_version: 1,
        created_at: now_string(),
        manifest: manifest.root.clone(),
        stage_root: stage_root.to_string_lossy().into_owned(),
        precheck: "ok".to_string(),
        shortfall_bytes: 0,
        total_files,
        total_bytes,
        copied_verified: CategoryTotals::default(),
        already_verified: acc.already,
        verify_failed: acc.failed,
        skipped_locked: acc.locked,
        skipped_missing: acc.missing,
        copied_unhashed_hashes: Vec::new(),
        verify_failed_files: acc.failed_files,
        skipped_locked_files: acc.locked_files,
        skipped_missing_files: acc.missing_files,
    })
}

/// Human-readable copy report, same style as report.txt.
pub fn build_copy_report_txt(r: &CopyReport) -> String {
    let mut s = String::new();
    s.push_str("OSmosis copy report\n");
    s.push_str("====================\n");
    s.push_str(&format!("manifest: {}\n", r.manifest));
    s.push_str(&format!("stage_root: {}\n", r.stage_root));
    s.push_str(&format!("created_at: {}\n", r.created_at));
    s.push_str(&format!("precheck: {}\n", r.precheck));
    if r.precheck != "ok" {
        s.push_str(&format!(
            "shortfall: {} (nothing was copied)\n",
            human(r.shortfall_bytes)
        ));
    }
    s.push('\n');
    s.push_str("Totals\n");
    s.push_str("------\n");
    s.push_str(&format!(
        "total:             {} ({})\n",
        r.total_files,
        human(r.total_bytes)
    ));
    s.push_str(&format!(
        "copied_verified:   {} ({})\n",
        r.copied_verified.count,
        human(r.copied_verified.bytes)
    ));
    s.push_str(&format!(
        "already_verified:  {} ({})\n",
        r.already_verified.count,
        human(r.already_verified.bytes)
    ));
    s.push_str(&format!(
        "verify_failed:     {} ({})\n",
        r.verify_failed.count,
        human(r.verify_failed.bytes)
    ));
    s.push_str(&format!(
        "skipped_locked:    {} ({})\n",
        r.skipped_locked.count,
        human(r.skipped_locked.bytes)
    ));
    s.push_str(&format!(
        "skipped_missing:   {} ({})\n\n",
        r.skipped_missing.count,
        human(r.skipped_missing.bytes)
    ));

    s.push_str("VERIFY FAILURES\n");
    s.push_str("---------------\n");
    if r.verify_failed_files.is_empty() {
        s.push_str("(none)\n");
    }
    for p in &r.verify_failed_files {
        s.push_str(&format!("- {}: {}\n", p.path, p.reason));
    }
    s.push('\n');
    s.push_str("SKIPPED (LOCKED)\n");
    s.push_str("----------------\n");
    if r.skipped_locked_files.is_empty() {
        s.push_str("(none)\n");
    }
    for p in &r.skipped_locked_files {
        s.push_str(&format!("- {}: {}\n", p.path, p.reason));
    }
    s.push('\n');
    s.push_str("SKIPPED (MISSING)\n");
    s.push_str("-----------------\n");
    if r.skipped_missing_files.is_empty() {
        s.push_str("(none)\n");
    }
    for p in &r.skipped_missing_files {
        s.push_str(&format!("- {}: {}\n", p.path, p.reason));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_rel_strips_home_and_accepts_normal_paths() {
        assert_eq!(
            stage_rel_from_dest("~/Documents/report.pdf").unwrap(),
            "Documents/report.pdf"
        );
        assert_eq!(
            stage_rel_from_dest("~/.ssh/id_ed25519").unwrap(),
            ".ssh/id_ed25519"
        );
    }

    #[test]
    fn stage_rel_rejects_escapes() {
        assert!(stage_rel_from_dest("~/../evil.txt").is_err());
        assert!(stage_rel_from_dest("~/a/../../evil.txt").is_err());
        assert!(stage_rel_from_dest("~//etc/passwd").is_err());
        assert!(stage_rel_from_dest("~/").is_err());
    }

    #[test]
    fn abs_lexical_resolves_dotdot() {
        let p = abs_lexical(Path::new("C:\\a\\b\\..\\c"));
        assert_eq!(p, PathBuf::from("C:\\a\\c"));
    }

    #[test]
    fn classify_open_error_buckets() {
        let nf = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(classify_open_error(&nf).0, "missing");
        let lock = io::Error::from_raw_os_error(32);
        assert_eq!(classify_open_error(&lock).0, "locked");
        let lock33 = io::Error::from_raw_os_error(33);
        assert_eq!(classify_open_error(&lock33).0, "locked");
    }
}
