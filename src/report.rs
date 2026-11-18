//! Human-readable summary (section 1.4): printed to stdout AND written to report.txt.

use crate::model::Manifest;
use std::collections::BTreeMap;
use std::path::Path;

/// Bytes -> "X.XX GB" / "X.XX MB" / "X.XX KB" / "N B" with 2 decimals.
pub fn human(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

pub fn build_summary(manifest: &Manifest, staging: Option<&Path>) -> String {
    let t = &manifest.totals;
    let mut s = String::new();
    s.push_str("OSmosis scan report\n");
    s.push_str("===================\n");
    s.push_str(&format!("root: {}\n", manifest.root));
    s.push_str(&format!("created_at: {}\n", manifest.created_at));
    s.push_str(&format!("git_check: {}\n\n", manifest.git_check));

    s.push_str("Totals\n");
    s.push_str("------\n");
    s.push_str(&format!(
        "keep_files:        {} ({})\n",
        t.keep_files,
        human(t.keep_bytes)
    ));
    s.push_str(&format!(
        "recreate_entries:  {} ({})\n",
        t.recreate_entries,
        human(t.recreate_bytes)
    ));
    s.push_str(&format!(
        "skip_files:        {} ({})\n",
        t.skip_files,
        human(t.skip_bytes)
    ));
    s.push_str(&format!("error_count:       {}\n\n", t.error_count));

    // top 10 largest KEEP directories by total size
    let mut by_dir: BTreeMap<String, u64> = BTreeMap::new();
    for e in &manifest.entries {
        let dir = match e.relative_path.rfind('/') {
            Some(i) => e.relative_path[..i].to_string(),
            None => ".".to_string(),
        };
        *by_dir.entry(dir).or_insert(0) += e.size_bytes;
    }
    let mut dirs: Vec<(String, u64)> = by_dir.into_iter().collect();
    dirs.sort_by_key(|d| std::cmp::Reverse(d.1));
    s.push_str("Top 10 largest KEEP directories\n");
    s.push_str("-------------------------------\n");
    if dirs.is_empty() {
        s.push_str("(none)\n");
    }
    for (dir, bytes) in dirs.iter().take(10) {
        s.push_str(&format!("{:>12}  {}\n", human(*bytes), dir));
    }
    s.push('\n');

    s.push_str(&format!(
        "RECREATE total: {} entries, {}\n\n",
        t.recreate_entries,
        human(t.recreate_bytes)
    ));

    s.push_str("REPOS REQUIRING ATTENTION\n");
    s.push_str("-------------------------\n");
    let attention: Vec<&crate::model::Repo> = manifest
        .repos
        .iter()
        .filter(|r| r.dirty_files > 0 || r.unpushed.unwrap_or(0) > 0 || r.no_upstream)
        .collect();
    if attention.is_empty() {
        s.push_str("(none)\n");
    }
    for r in attention {
        s.push_str(&format!(
            "- {} (branch={}, dirty_files={}, unpushed={}, no_upstream={})\n",
            r.path,
            r.branch.as_deref().unwrap_or("?"),
            r.dirty_files,
            r.unpushed
                .map(|u| u.to_string())
                .unwrap_or_else(|| "null".into()),
            r.no_upstream
        ));
    }
    s.push('\n');

    s.push_str(&format!(
        "ERRORS: {} read error(s) during scan (see manifest.json errors[])\n",
        t.error_count
    ));
    for e in manifest.errors.iter().take(20) {
        s.push_str(&format!("  - {}: {}\n", e.path, e.message));
    }
    if t.error_count > 20 {
        s.push_str(&format!("  ... and {} more\n", t.error_count - 20));
    }
    s.push('\n');

    match staging {
        Some(p) => match fs2::available_space(p) {
            Ok(free) => {
                let fits = t.keep_bytes <= free;
                s.push_str(&format!(
                    "Keepers: {} / Staging free: {} -> {}\n",
                    human(t.keep_bytes),
                    human(free),
                    if fits { "FITS" } else { "DOES NOT FIT" }
                ));
            }
            Err(e) => s.push_str(&format!(
                "Staging: could not check {} ({})\n",
                p.display(),
                e
            )),
        },
        None => s.push_str("Staging: not checked (no --staging path given)\n"),
    }

    s
}
