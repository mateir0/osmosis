//! The read-only scanner (sections 1.2-1.4 of the slice spec).
//!
//! READ-PATH RULE: every filesystem error is recorded in `errors` and the scan
//! continues. Nothing in this module unwraps or panics on a filesystem entry.

use crate::classify::{classify_file, is_recreate_dir, normalize_rel, Class};
use crate::config::Config;
use crate::model::*;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;
use xxhash_rust::xxh64::Xxh64;

pub struct ScanResult {
    pub manifest: Manifest,
    pub summary: String,
}

/// Aggregate a pruned directory: total bytes + file count of its subtree.
/// Read errors inside the subtree are collected, never fatal.
fn aggregate_dir(dir: &Path, root: &Path, errors: &mut Vec<ScanError>) -> (u64, u64) {
    let mut bytes = 0u64;
    let mut count = 0u64;
    for res in WalkDir::new(dir).follow_links(false) {
        match res {
            Ok(e) => {
                if e.file_type().is_file() {
                    match e.metadata() {
                        Ok(md) => {
                            bytes += md.len();
                            count += 1;
                        }
                        Err(err) => errors.push(ScanError {
                            path: e.path().to_string_lossy().into_owned(),
                            message: err.to_string(),
                        }),
                    }
                }
            }
            Err(e) => errors.push(ScanError {
                path: e
                    .path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| normalize_rel(dir, root)),
                message: e.to_string(),
            }),
        }
    }
    (bytes, count)
}

/// xxh64 of a file's contents, streamed in chunks. Never fails the scan.
fn hash_file(path: &Path, errors: &mut Vec<ScanError>) -> Option<String> {
    let mut f = match File::open(path) {
        Ok(f) => f,
        Err(e) => {
            errors.push(ScanError {
                path: path.to_string_lossy().into_owned(),
                message: format!("hash open failed: {}", e),
            });
            return None;
        }
    };
    let mut h = Xxh64::new(0);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(e) => {
                errors.push(ScanError {
                    path: path.to_string_lossy().into_owned(),
                    message: format!("hash read failed: {}", e),
                });
                return None;
            }
        }
    }
    Some(format!("{:016x}", h.digest()))
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn run_git(repo_path: &Path, args: &[&str]) -> io::Result<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(repo_path)
        .args(args)
        .output()?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(io::Error::other(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ))
    }
}

/// Git detection/checks are suppressed under `AppData` (any subtree) and under
/// any `Temp` directory: caches there contain fake `.git` entries that only
/// produce noise errors and pollute the repos list (rule 1.1.4).
fn is_git_check_exempt(rel: &str, cfg: &Config) -> bool {
    let lower = rel.to_lowercase();
    let first = lower.split('/').next().unwrap_or("");
    if first == cfg.appdata_root.to_lowercase() {
        return true;
    }
    lower.split('/').any(|c| c == "temp")
}

/// Inspect one detected repo with git. A missing upstream is NOT an error.
/// An unborn HEAD (zero commits) is NOT an error either: it sets `no_commits`.
fn inspect_repo(
    repo_path: &Path,
    rel: &str,
    git_dir_bytes: u64,
    git_ok: bool,
    errors: &mut Vec<ScanError>,
) -> Repo {
    let mut repo = Repo {
        path: rel.to_string(),
        git_dir_bytes,
        branch: None,
        dirty_files: 0,
        unpushed: None,
        no_upstream: false,
        no_commits: false,
    };
    if !git_ok {
        return repo;
    }
    match run_git(repo_path, &["status", "--porcelain"]) {
        Ok(out) => repo.dirty_files = out.lines().filter(|l| !l.trim().is_empty()).count() as u64,
        Err(e) => errors.push(ScanError {
            path: repo_path.to_string_lossy().into_owned(),
            message: format!("git status failed: {}", e),
        }),
    }
    match run_git(repo_path, &["rev-parse", "--abbrev-ref", "HEAD"]) {
        Ok(out) => repo.branch = Some(out.trim().to_string()),
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("ambiguous argument") && msg.contains("HEAD") {
                // unborn HEAD: repo has zero commits, not an error
                repo.no_commits = true;
            } else {
                errors.push(ScanError {
                    path: repo_path.to_string_lossy().into_owned(),
                    message: format!("git rev-parse failed: {}", msg),
                });
            }
        }
    }
    match run_git(repo_path, &["log", "@{u}..HEAD", "--oneline"]) {
        Ok(out) => {
            repo.unpushed = Some(out.lines().filter(|l| !l.trim().is_empty()).count() as u64)
        }
        Err(_) => {
            // no upstream configured: not an error
            repo.unpushed = None;
            repo.no_upstream = true;
        }
    }
    repo
}

/// Run the full scan of `root`, write manifest.json + report.txt into `out`
/// (the ONLY files this program ever writes), and return the manifest + summary.
pub fn run_scan(
    root: &Path,
    cfg: &Config,
    out: &Path,
    staging: Option<&Path>,
) -> Result<ScanResult, String> {
    if !root.is_dir() {
        return Err(format!("scan root is not a directory: {}", root.display()));
    }
    let root = root
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize root: {}", e))?;

    let git_ok = git_available();
    let git_check = if git_ok { "ok" } else { "git_not_found" }.to_string();

    let mut totals = Totals {
        keep_files: 0,
        keep_bytes: 0,
        recreate_entries: 0,
        recreate_bytes: 0,
        skip_files: 0,
        skip_bytes: 0,
        error_count: 0,
    };
    let mut entries: Vec<Entry> = Vec::new();
    let mut recreate: Vec<RecreateEntry> = Vec::new();
    let mut keep_dirs: Vec<KeepDirEntry> = Vec::new();
    let mut repos: Vec<Repo> = Vec::new();
    let mut errors: Vec<ScanError> = Vec::new();
    let mut pending_repos: Vec<(PathBuf, String)> = Vec::new(); // (abs path, rel path of repo root)
    let mut git_bytes: std::collections::HashMap<String, u64> = std::collections::HashMap::new(); // rel path of .git -> bytes

    let mut it = WalkDir::new(&root).follow_links(false).into_iter();
    let mut scanned: u64 = 0;
    while let Some(res) = it.next() {
        let entry = match res {
            Ok(e) => e,
            Err(e) => {
                errors.push(ScanError {
                    path: e
                        .path()
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.to_string_lossy().into_owned()),
                    message: e.to_string(),
                });
                continue;
            }
        };
        scanned += 1;
        if scanned.is_multiple_of(5000) {
            eprintln!("osmosis: progress - {} entries scanned so far", scanned);
        }

        let path = entry.path();
        let rel = normalize_rel(path, &root);
        let ft = entry.file_type();

        if ft.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.eq_ignore_ascii_case(".git") {
                // rule 1.1.3: .git is always KEEP — one aggregate counted in the
                // KEEP totals, internals never listed, never RECREATE/SKIP
                let (bytes, count) = aggregate_dir(path, &root, &mut errors);
                keep_dirs.push(KeepDirEntry {
                    path: rel.clone(),
                    kind: "git_dir".to_string(),
                    size_bytes: bytes,
                    file_count: count,
                    linux_dest: format!("~/{}", rel),
                });
                git_bytes.insert(rel.clone(), bytes);
                totals.keep_files += count;
                totals.keep_bytes += bytes;
                it.skip_current_dir();
                continue;
            }
            if is_recreate_dir(&name, cfg) {
                // rule (2): prune the whole subtree as ONE aggregate
                let (bytes, count) = aggregate_dir(path, &root, &mut errors);
                recreate.push(RecreateEntry {
                    path: rel.clone(),
                    kind: "recreate_dir".to_string(),
                    size_bytes: bytes,
                    file_count: count,
                });
                totals.recreate_entries += 1;
                totals.recreate_bytes += bytes;
                it.skip_current_dir();
                continue;
            }
            // repo detection: this directory contains a .git entry; suppressed
            // under AppData / Temp (fake .git entries in caches, rule 1.1.4)
            if !is_git_check_exempt(&rel, cfg) {
                let git_child = path.join(".git");
                if std::fs::symlink_metadata(&git_child).is_ok() {
                    pending_repos.push((path.to_path_buf(), rel.clone()));
                }
            }
            continue;
        }

        if !ft.is_file() {
            continue; // symlinks etc. are not followed and not listed
        }

        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        match classify_file(&rel, cfg) {
            Class::Keep {
                linux_dest,
                sensitive,
            } => {
                let (hash, hash_status) = if size <= cfg.hash_max_bytes {
                    match hash_file(path, &mut errors) {
                        Some(h) => (Some(h), "ok".to_string()),
                        None => (None, "error".to_string()),
                    }
                } else {
                    (None, "deferred_large_file".to_string())
                };
                entries.push(Entry {
                    relative_path: rel.clone(),
                    size_bytes: size,
                    linux_dest,
                    sensitive,
                    hash,
                    hash_status,
                });
                totals.keep_files += 1;
                totals.keep_bytes += size;
            }
            Class::Skip => {
                totals.skip_files += 1;
                totals.skip_bytes += size;
            }
        }
    }

    // inspect detected repos (git metadata is read-only); .git size was captured during the walk
    for (abs, rel) in &pending_repos {
        let git_rel = if rel.is_empty() {
            ".git".to_string()
        } else {
            format!("{}/.git", rel)
        };
        let bytes = git_bytes.get(&git_rel).copied().unwrap_or(0);
        let repo = inspect_repo(abs, rel, bytes, git_ok, &mut errors);
        repos.push(repo);
    }

    totals.error_count = errors.len() as u64;

    let manifest = Manifest {
        schema_version: 1,
        created_at: chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
        root: root.to_string_lossy().into_owned(),
        git_check,
        totals,
        entries,
        recreate,
        keep_dirs,
        repos,
        errors,
    };

    let summary = crate::report::build_summary(&manifest, staging);

    // write the ONLY two files this program may ever write, inside --out
    std::fs::create_dir_all(out)
        .map_err(|e| format!("cannot create output dir {}: {}", out.display(), e))?;
    let manifest_path = out.join("manifest.json");
    let mut f = File::create(&manifest_path)
        .map_err(|e| format!("cannot write {}: {}", manifest_path.display(), e))?;
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|e| format!("cannot serialize manifest: {}", e))?;
    f.write_all(json.as_bytes())
        .and_then(|_| f.write_all(b"\n"))
        .map_err(|e| format!("cannot write {}: {}", manifest_path.display(), e))?;
    let report_path = out.join("report.txt");
    let mut f = File::create(&report_path)
        .map_err(|e| format!("cannot write {}: {}", report_path.display(), e))?;
    f.write_all(summary.as_bytes())
        .map_err(|e| format!("cannot write {}: {}", report_path.display(), e))?;

    Ok(ScanResult { manifest, summary })
}
