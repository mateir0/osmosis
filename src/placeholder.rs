//! OneDrive "Files On-Demand" placeholder detection (slice 9.1).
//!
//! A cloud placeholder is a stub file: its real content lives online only.
//! Hashing or staging the stub would report success while the real bytes are
//! lost at the wipe, so placeholders must be detected, reported loudly, and
//! never staged as successes.
//!
//! The bit test is a pure function (`is_placeholder_attributes`); only the
//! metadata READ is cfg-gated to Windows. On non-Windows targets the read
//! returns `None` and every file is treated as real.

use std::path::Path;

/// FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS — content is fetched on first access.
pub const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;
/// FILE_ATTRIBUTE_RECALL_ON_OPEN — content is fetched when the file is opened.
pub const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x0008_0000;
/// FILE_ATTRIBUTE_OFFLINE — the file data is not available locally.
pub const FILE_ATTRIBUTE_OFFLINE: u32 = 0x0000_1000;

const PLACEHOLDER_MASK: u32 =
    FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_OFFLINE;

/// Pure predicate: does this Windows `dwFileAttributes` value mark the file as
/// an online-only placeholder? Any of the three recall/offline bits set => true.
pub fn is_placeholder_attributes(attrs: u32) -> bool {
    attrs & PLACEHOLDER_MASK != 0
}

/// Read the Windows file attributes of `path`. On Windows this is
/// `MetadataExt::file_attributes()`; on every other target there is no such
/// concept and the read is cfg-gated away, returning `None`.
#[cfg(windows)]
pub fn file_attributes(path: &Path) -> Option<u32> {
    use std::os::windows::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|md| md.file_attributes())
}

/// Non-Windows: no placeholder attributes exist; always `None`.
#[cfg(not(windows))]
pub fn file_attributes(_path: &Path) -> Option<u32> {
    None
}

/// Is `path` a placeholder right now, per the live attribute read?
pub fn file_is_placeholder(path: &Path) -> bool {
    file_attributes(path)
        .map(is_placeholder_attributes)
        .unwrap_or(false)
}

/// Does the scanned tree include a OneDrive folder? True when the profile has
/// a top-level "OneDrive" / "OneDrive - *" directory, or when the %OneDrive%
/// environment path falls inside the scan root.
pub fn onedrive_in_scan(root: &Path) -> bool {
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name == "onedrive" || name.starts_with("onedrive - ") {
                return true;
            }
        }
    }
    if let Some(od) = std::env::var_os("OneDrive") {
        let od = Path::new(&od);
        if od.is_absolute() {
            let root_c = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
            let od_c = od.canonicalize().unwrap_or_else(|_| od.to_path_buf());
            if od_c.starts_with(&root_c) {
                return true;
            }
        }
    }
    false
}

/// The plain-words reason placeholders must never be staged as successes.
pub const PLACEHOLDER_REASON: &str =
    "online-only cloud placeholder (OneDrive Files On-Demand stub): \
     the real content is NOT on this machine";

/// The plain-words warning shown when placeholders were found: download fully
/// (right-click the folder in Explorer -> \"Always keep on this device\") and
/// re-run the scan before staging.
pub fn placeholder_warning(count: u64, human_bytes: &str) -> String {
    format!(
        "WARNING: {} file(s) ({}) are online-only placeholders — their real content is NOT on \
         this machine. Fully download them (right-click the folder in Explorer -> \"Always keep \
         on this device\") and re-run the scan BEFORE staging, or their content will be lost at \
         the wipe.",
        count, human_bytes
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_placeholder_bit_alone_is_flagged() {
        assert!(is_placeholder_attributes(
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
        ));
        assert!(is_placeholder_attributes(FILE_ATTRIBUTE_RECALL_ON_OPEN));
        assert!(is_placeholder_attributes(FILE_ATTRIBUTE_OFFLINE));
    }

    #[test]
    fn combined_placeholder_bits_are_flagged() {
        assert!(is_placeholder_attributes(
            FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS | FILE_ATTRIBUTE_RECALL_ON_OPEN
        ));
        assert!(is_placeholder_attributes(
            FILE_ATTRIBUTE_OFFLINE | FILE_ATTRIBUTE_RECALL_ON_OPEN | 0x80
        ));
    }

    #[test]
    fn normal_file_is_not_a_placeholder() {
        assert!(!is_placeholder_attributes(0x80)); // FILE_ATTRIBUTE_NORMAL
        assert!(!is_placeholder_attributes(0x100)); // FILE_ATTRIBUTE_DIRECTORY
        assert!(!is_placeholder_attributes(0x20 | 0x80)); // archive+normal
        assert!(!is_placeholder_attributes(0));
    }

    #[test]
    fn onedrive_top_level_dir_is_detected() {
        let dir = std::env::temp_dir().join(format!(
            "osmosis-od-test-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(dir.join("OneDrive - Personal")).unwrap();
        std::fs::create_dir_all(dir.join("Documents")).unwrap();
        assert!(onedrive_in_scan(&dir));
        let dir2 = dir.join("Documents");
        assert!(!onedrive_in_scan(&dir2));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
