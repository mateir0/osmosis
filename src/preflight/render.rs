//! Preflight report rendering (slice 9.6): the human-readable
//! preflight-report.txt. Dealbreakers come FIRST — the "Will NOT work" and
//! "Blocking warnings" sections lead the report, never buried at the bottom.

use super::PreflightReport;
use crate::report::human;

pub fn build_report_txt(r: &PreflightReport) -> String {
    let mut s = String::new();
    s.push_str("OSmosis preflight report\n");
    s.push_str("========================\n");
    s.push_str(&format!("created_at: {}\n", r.created_at));
    match &r.inventory {
        super::Inventory::Ok(apps) => {
            s.push_str(&format!("installed apps inventoried: {}\n", apps.len()))
        }
        super::Inventory::Failed(e) => s.push_str(&format!("APP INVENTORY FAILED: {}\n", e)),
    }
    s.push('\n');

    // ---- dealbreakers first ----
    s.push_str("!!! WILL NOT WORK ON LINUX !!!\n");
    s.push_str("------------------------------\n");
    if r.buckets.will_not_work.is_empty() {
        s.push_str("(none)\n");
    }
    for a in &r.buckets.will_not_work {
        s.push_str(&format!("- {}: {}\n", a.name, a.reason));
    }
    s.push('\n');
    if !r.games.is_empty() {
        s.push_str("!!! GAMES BLOCKED BY ANTI-CHEAT !!!\n");
        s.push_str("----------------------------------\n");
        for g in &r.games {
            s.push_str(&format!(
                "- {} (found in {}) — {}\n",
                g.title, g.matched_in, g.reason
            ));
        }
        s.push('\n');
    }
    s.push_str("BLOCKING WARNINGS\n");
    s.push_str("-----------------\n");
    if r.blocking_warnings.is_empty() {
        s.push_str("(none)\n");
    }
    for w in &r.blocking_warnings {
        s.push_str(&format!("- {}\n", w));
    }
    s.push('\n');

    // ---- summary counts ----
    let b = &r.buckets;
    s.push_str("Summary\n");
    s.push_str("-------\n");
    s.push_str(&format!(
        "ready on Linux (native):      {}\n",
        b.native.len()
    ));
    s.push_str(&format!(
        "has a Linux equivalent:       {}\n",
        b.equivalent.len()
    ));
    s.push_str(&format!(
        "web only:                     {}\n",
        b.web_only.len()
    ));
    s.push_str(&format!(
        "will NOT work:                {}\n",
        b.will_not_work.len()
    ));
    s.push_str(&format!(
        "unknown (review manually):    {}\n",
        b.unknown.len()
    ));
    s.push('\n');

    // ---- buckets ----
    push_bucket(&mut s, "READY ON LINUX (NATIVE)", &b.native, |a| {
        a.name.clone()
    });
    push_bucket(&mut s, "HAS A LINUX EQUIVALENT", &b.equivalent, |a| {
        format!("{} -> {} ({})", a.name, a.linux_name, a.note)
    });
    push_bucket(&mut s, "WEB ONLY", &b.web_only, |a| a.name.clone());
    push_bucket(&mut s, "WILL NOT WORK", &b.will_not_work, |a| {
        format!("{} — {}", a.name, a.reason)
    });
    push_bucket(&mut s, "UNKNOWN (REVIEW MANUALLY)", &b.unknown, |a| {
        a.name.clone()
    });

    // ---- hardware ----
    s.push_str("HARDWARE NOTES\n");
    s.push_str("--------------\n");
    s.push_str(&format!(
        "video: {}\n",
        if r.hardware.video.is_empty() {
            "(none reported)".to_string()
        } else {
            r.hardware.video.join(", ")
        }
    ));
    s.push_str(&format!(
        "network adapters: {}\n",
        if r.hardware.network.is_empty() {
            "(none reported)".to_string()
        } else {
            r.hardware.network.join(", ")
        }
    ));
    s.push_str(&format!(
        "printers: {}\n",
        if r.hardware.printers.is_empty() {
            "(none reported)".to_string()
        } else {
            r.hardware.printers.join(", ")
        }
    ));
    s.push_str(&format!("bitlocker (C:): {}\n", r.hardware.bitlocker));
    if r.hardware_notes.is_empty() {
        s.push_str("(no hardware notes)\n");
    }
    for n in &r.hardware_notes {
        s.push_str(&format!("- {}\n", n));
    }
    s.push('\n');

    // ---- scan cross-reference ----
    match &r.scan {
        Some(sc) => {
            s.push_str("SCAN CROSS-REFERENCE (--manifest)\n");
            s.push_str("---------------------------------\n");
            s.push_str(&format!(
                "placeholders (online-only stubs): {} ({})\n",
                sc.placeholder_count,
                human(sc.placeholder_bytes)
            ));
            s.push_str(&format!("git repos detected: {}\n", sc.repos));
            if sc.games_detected.is_empty() {
                s.push_str(
                    "games detected in the manifest: (none of the known anti-cheat titles)\n",
                );
            } else {
                s.push_str(&format!(
                    "games detected in the manifest: {}\n",
                    sc.games_detected.join(", ")
                ));
            }
        }
        None => {
            s.push_str("SCAN CROSS-REFERENCE\n");
            s.push_str("--------------------\n");
            s.push_str(
                "no --manifest given: run `osmosis scan` first and pass --manifest to \
                 cross-reference OneDrive placeholders and detected games here.\n",
            );
        }
    }

    s
}

fn push_bucket(
    s: &mut String,
    title: &str,
    items: &[super::AppItem],
    line: impl Fn(&super::AppItem) -> String,
) {
    s.push_str(&format!("{}\n", title));
    s.push_str(&format!("{}\n", "-".repeat(title.len())));
    if items.is_empty() {
        s.push_str("(none)\n");
    }
    for a in items {
        s.push_str(&format!("- {}\n", line(a)));
    }
    s.push('\n');
}
