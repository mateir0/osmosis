//! File/directory classification rules (section 1.2 of the slice spec).
//!
//! Precedence:
//!   (1) RECREATE directory matches (applied at walk time, see scan.rs)
//!   (2) filename SKIP rules — universal: they apply even inside KEEP-mapped
//!       folders (NTUSER.DAT*, ntuser.ini, Thumbs.db, desktop.ini, *.tmp, *.sys)
//!   (3) explicit KEEP mappings
//!   (4) SKIP for everything under AppData except the KEEP mapping
//!   (5) default: KEEP only if under a mapped top-level folder, else SKIP

use crate::config::Config;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Class {
    Keep { linux_dest: String, sensitive: bool },
    Skip,
}

/// Normalize a path relative to the scan root into a forward-slash,
/// windows-style relative string ("Documents/a.txt").
pub fn normalize_rel(path: &Path, root: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.to_string_lossy().replace('\\', "/")
}

/// Case-insensitive RECREATE directory-name check (rule 2).
pub fn is_recreate_dir(name: &str, cfg: &Config) -> bool {
    let lower = name.to_lowercase();
    cfg.recreate_dirs.iter().any(|d| d == &lower)
}

/// Classify a single file given its root-relative path (rules 1-5).
pub fn classify_file(rel: &str, cfg: &Config) -> Class {
    let lower = rel.to_lowercase();

    let first = lower.split('/').next().unwrap_or("");
    let name = lower.rsplit('/').next().unwrap_or("");

    // (2) filename SKIP rules — universal, even inside KEEP-mapped folders
    if cfg.skip_names.iter().any(|n| n == name) {
        return Class::Skip;
    }
    if cfg
        .skip_prefix_names
        .iter()
        .any(|p| name.starts_with(p.as_str()))
    {
        return Class::Skip;
    }
    if let Some(ext) = Path::new(name).extension().and_then(|e| e.to_str()) {
        let ext = format!(".{}", ext.to_lowercase());
        if cfg.skip_extensions.iter().any(|e| e == &ext) {
            return Class::Skip;
        }
    }

    // (3) explicit KEEP mappings, on component boundaries
    for m in &cfg.keep_mappings {
        let prefix_l = &m.prefix_lower;
        if lower == *prefix_l || lower.starts_with(&format!("{}/", prefix_l)) {
            let rem = &rel[m.prefix.len()..];
            let linux_dest = if rem.is_empty() {
                m.dest.clone()
            } else {
                format!("{}/{}", m.dest, rem.trim_start_matches('/'))
            };
            let sensitive = cfg.sensitive_prefixes.iter().any(|s| s == prefix_l);
            return Class::Keep {
                linux_dest,
                sensitive,
            };
        }
    }

    // (4) SKIP rules for folders: everything under AppData except the KEEP mapping above
    if first == cfg.appdata_root.to_lowercase() {
        return Class::Skip;
    }

    // (5) default: KEEP only if under a mapped top-level folder
    let top_levels: Vec<String> = cfg
        .keep_mappings
        .iter()
        .map(|m| m.prefix.split('/').next().unwrap_or("").to_lowercase())
        .collect();
    if top_levels.iter().any(|t| t == first) {
        let sensitive = cfg
            .sensitive_prefixes
            .iter()
            .any(|s| s.split('/').next().unwrap_or("") == first);
        return Class::Keep {
            linux_dest: format!("~/{}", rel),
            sensitive,
        };
    }
    Class::Skip
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> Config {
        Config::default_config()
    }

    #[test]
    fn keep_mapping_documents() {
        match classify_file("Documents/report.pdf", &cfg()) {
            Class::Keep {
                linux_dest,
                sensitive,
            } => {
                assert_eq!(linux_dest, "~/Documents/report.pdf");
                assert!(!sensitive);
            }
            other => panic!("expected Keep, got {:?}", other),
        }
    }

    #[test]
    fn appdata_keep_beats_appdata_skip() {
        match classify_file("AppData/Roaming/Code/User/settings.json", &cfg()) {
            Class::Keep { linux_dest, .. } => {
                assert_eq!(linux_dest, "~/.config/Code/User/settings.json")
            }
            other => panic!("expected Keep, got {:?}", other),
        }
        // ...while the rest of AppData is skipped
        assert_eq!(classify_file("AppData/Local/junk.bin", &cfg()), Class::Skip);
        assert_eq!(
            classify_file("appdata/local/whatever", &cfg()),
            Class::Skip,
            "AppData match must be case-insensitive"
        );
    }

    #[test]
    fn recreate_dir_match_is_case_insensitive() {
        let c = cfg();
        assert!(is_recreate_dir("node_modules", &c));
        assert!(is_recreate_dir("Node_Modules", &c));
        assert!(is_recreate_dir("TARGET", &c));
        assert!(is_recreate_dir("__PYCACHE__", &c));
        assert!(!is_recreate_dir("src", &c));
    }

    #[test]
    fn skip_filename_rules() {
        let c = cfg();
        assert_eq!(classify_file("Documents/NTUSER.DAT", &c), Class::Skip);
        assert_eq!(classify_file("Documents/ntuser.dat.LOG1", &c), Class::Skip);
        assert_eq!(classify_file("Documents/ntuser.ini", &c), Class::Skip);
        assert_eq!(classify_file("Documents/Thumbs.db", &c), Class::Skip);
        assert_eq!(classify_file("Documents/desktop.ini", &c), Class::Skip);
        assert_eq!(classify_file("Documents/scratch.tmp", &c), Class::Skip);
        assert_eq!(classify_file("Documents/pagefile.sys", &c), Class::Skip);
    }

    #[test]
    fn sensitive_flag_on_ssh() {
        match classify_file(".ssh/id_ed25519", &cfg()) {
            Class::Keep {
                sensitive,
                linux_dest,
            } => {
                assert!(sensitive);
                assert_eq!(linux_dest, "~/.ssh/id_ed25519");
            }
            other => panic!("expected Keep, got {:?}", other),
        }
    }

    #[test]
    fn default_rule_outside_mapped_folders_is_skip() {
        assert_eq!(classify_file("RandomFolder/x.bin", &cfg()), Class::Skip);
        // under a mapped top-level folder but not a full mapping prefix -> KEEP at ~/...
        match classify_file(".minecraft/options.txt", &cfg()) {
            Class::Keep { linux_dest, .. } => assert_eq!(linux_dest, "~/.minecraft/options.txt"),
            other => panic!("expected Keep, got {:?}", other),
        }
    }

    #[test]
    fn mapping_is_component_boundary_not_substring() {
        // "DocumentsOld" must NOT match the "Documents" mapping
        assert_eq!(classify_file("DocumentsOld/x.txt", &cfg()), Class::Skip);
    }
}
