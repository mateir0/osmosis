//! Data model for the OSmosis scan manifest.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Totals {
    pub keep_files: u64,
    pub keep_bytes: u64,
    pub recreate_entries: u64,
    pub recreate_bytes: u64,
    pub skip_files: u64,
    pub skip_bytes: u64,
    pub error_count: u64,
}

/// One individually listed KEEP file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub relative_path: String,
    pub size_bytes: u64,
    pub linux_dest: String,
    pub sensitive: bool,
    pub hash: Option<String>,
    pub hash_status: String,
    /// True when the file is an online-only cloud placeholder (OneDrive Files
    /// On-Demand stub) at scan time. Additive field: defaults to false for
    /// manifests written before this field existed.
    #[serde(default)]
    pub placeholder: bool,
}

/// Aggregate of every placeholder found during the scan. Always present in
/// the manifest (may be 0/empty); the human report section appears only when
/// count > 0.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Placeholders {
    pub count: u64,
    pub bytes: u64,
    pub onedrive_detected: bool,
    pub paths: Vec<String>,
}

/// One pruned aggregate: a RECREATE directory or a `.git` directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecreateEntry {
    pub path: String,
    pub kind: String, // "recreate_dir" | "git_dir"
    pub size_bytes: u64,
    pub file_count: u64,
}

/// One aggregated KEEP directory (currently: a `.git` directory). The subtree
/// is pruned from the walk and counted as KEEP in `totals`, never RECREATE/SKIP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeepDirEntry {
    pub path: String,
    pub kind: String, // "git_dir"
    pub size_bytes: u64,
    pub file_count: u64,
    pub linux_dest: String,
}

/// One detected git repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub path: String,
    pub git_dir_bytes: u64,
    pub branch: Option<String>,
    pub dirty_files: u64,
    pub unpushed: Option<u64>,
    pub no_upstream: bool,
    /// True when HEAD is unborn (zero commits). Additive field: omitted from
    /// serialized output for normal repos, defaults to false when absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub no_commits: bool,
}

/// One read error encountered during the scan. The scan always continues past these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanError {
    pub path: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u32,
    pub created_at: String,
    pub root: String,
    pub git_check: String, // "ok" | "git_not_found"
    pub totals: Totals,
    pub entries: Vec<Entry>,
    pub recreate: Vec<RecreateEntry>,
    pub keep_dirs: Vec<KeepDirEntry>,
    pub repos: Vec<Repo>,
    pub errors: Vec<ScanError>,
    /// Placeholder (cloud stub) aggregate; always present, may be 0/empty.
    #[serde(default)]
    pub placeholders: Placeholders,
}
