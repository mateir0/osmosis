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
}

/// One pruned aggregate: a RECREATE directory or a `.git` directory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecreateEntry {
    pub path: String,
    pub kind: String, // "recreate_dir" | "git_dir"
    pub size_bytes: u64,
    pub file_count: u64,
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
    pub repos: Vec<Repo>,
    pub errors: Vec<ScanError>,
}
