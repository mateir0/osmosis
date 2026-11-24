//! Integration tests for slice 9.3–9.6: the preflight verb. A fake Runner
//! feeds recorded PowerShell JSON (apps inventory + hardware/BitLocker), so
//! the whole pipeline runs on any platform. Verdict tables are pinned here:
//! every verdict kind must appear at least once, and the anti-cheat titles
//! must match exactly (GTA V single-player must NOT match).

use osmosis::config::Config;
use osmosis::placeholder::{FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS, FILE_ATTRIBUTE_RECALL_ON_OPEN};
use osmosis::preflight::inventory::{PS_APPS_ARGV, PS_HW_ARGV};
use osmosis::preflight::verdicts::{app_verdict, game_row_for, Verdict, APP_ROWS, GAME_ROWS};
use osmosis::preflight::{run_preflight_with, Inventory};
use osmosis::scan::run_scan_with_attrs;
use osmosis::setup::{CommandOutput, Runner};
use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Fake {
    apps_json: RefCell<Result<String, String>>,
    hw_json: RefCell<Result<String, String>>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl Fake {
    fn new(apps: Result<String, String>, hw: Result<String, String>) -> Fake {
        Fake {
            apps_json: RefCell::new(apps),
            hw_json: RefCell::new(hw),
            calls: RefCell::new(Vec::new()),
        }
    }
}

impl Runner for Fake {
    fn output(&self, argv: &[&str]) -> Result<CommandOutput, String> {
        self.calls
            .borrow_mut()
            .push(argv.iter().map(|s| s.to_string()).collect());
        if argv == PS_APPS_ARGV {
            let r = self.apps_json.borrow_mut().clone();
            match r {
                Ok(j) => Ok(CommandOutput::ok(j)),
                Err(e) => Err(e),
            }
        } else if argv == PS_HW_ARGV {
            let r = self.hw_json.borrow_mut().clone();
            match r {
                Ok(j) => Ok(CommandOutput::ok(j)),
                Err(e) => Err(e),
            }
        } else {
            Err(format!("unexpected argv: {:?}", argv))
        }
    }
}

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Fixture {
        let dir = std::env::temp_dir().join(format!(
            "osmosis-preflight-fixture-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();
        Fixture(dir)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn apps_json(entries: &[(&str, &str, &str)]) -> String {
    let inner: Vec<String> = entries
        .iter()
        .map(|(n, v, p)| {
            format!(
                r#"{{"DisplayName":"{}","DisplayVersion":"{}","Publisher":"{}"}}"#,
                n, v, p
            )
        })
        .collect();
    format!("[{}]", inner.join(","))
}

fn hw_json(video: &[&str], net: &[&str], printers: &[&str], bl: &str, bld: &str) -> String {
    let arr = |a: &[&str]| {
        format!(
            "[{}]",
            a.iter()
                .map(|s| format!("\"{}\"", s))
                .collect::<Vec<_>>()
                .join(",")
        )
    };
    format!(
        r#"{{"Video":{},"Network":{},"Printers":{},"BitLocker":"{}","BitLockerDetail":"{}"}}"#,
        arr(video),
        arr(net),
        arr(printers),
        bl,
        bld
    )
}

const APPS: &[(&str, &str, &str)] = &[
    ("Visual Studio Code", "1.95", "Microsoft"),
    ("Visual Studio 2022", "17.11", "Microsoft"), // dup publisher, different app
    ("Microsoft 365", "16.0", "Microsoft"),
    ("Adobe Photoshop 2024", "25.0", "Adobe"),
    ("Adobe Premiere Pro 2024", "24.0", "Adobe"),
    ("Steam", "1.0", "Valve"),
    ("Epic Games Launcher", "1.0", "Epic"),
    ("Discord", "1.0", "Discord Inc."),
    ("Some Obscure Tool", "0.1", "Nobody"),
    // duplicate across registry locations: must be de-duplicated
    ("Steam", "1.0", "Valve"),
];

#[test]
fn preflight_buckets_and_report_files_written() {
    let fx = Fixture::new();
    let out = fx.0.join("out");
    let runner = Fake::new(
        Ok(apps_json(APPS)),
        Ok(hw_json(
            &["NVIDIA GeForce RTX 3060"],
            &["Intel Wi-Fi 6 AX201", "Broadcom 802.11ac WiFi Adapter"],
            &["HP LaserJet"],
            "Off",
            "FullyDecrypted",
        )),
    );
    let res = run_preflight_with(&runner, None, &out).expect("preflight failed");
    let r = &res.report;

    // buckets
    assert_eq!(r.buckets.native.len(), 3, "VS Code + Steam + Discord"); // steam deduped
    assert!(r
        .buckets
        .native
        .iter()
        .any(|a| a.name == "Visual Studio Code"));
    assert!(r.buckets.native.iter().any(|a| a.name == "Discord"));
    assert_eq!(r.buckets.equivalent.len(), 3, "M365, Photoshop, Epic");
    assert_eq!(
        r.buckets.will_not_work.len(),
        2,
        "Visual Studio 2022 + Premiere"
    );
    assert_eq!(r.buckets.unknown.len(), 1, "Some Obscure Tool");
    assert!(r
        .buckets
        .will_not_work
        .iter()
        .any(|a| a.name.contains("Visual Studio 2022")));
    assert!(r
        .buckets
        .equivalent
        .iter()
        .any(|a| a.name.contains("Photoshop") && a.note.contains("partial")));

    // de-dup: Steam appears once
    assert_eq!(
        r.buckets
            .native
            .iter()
            .filter(|a| a.name == "Steam")
            .count(),
        1
    );
    match &r.inventory {
        Inventory::Ok(apps) => assert_eq!(apps.len(), 9, "10 rows, Steam deduped"),
        Inventory::Failed(e) => panic!("inventory failed: {}", e),
    }

    // hardware notes: NVIDIA + Broadcom WiFi caution + BitLocker off
    assert!(r
        .hardware_notes
        .iter()
        .any(|n| n.contains("NVIDIA") && n.contains("setup step")));
    assert!(r
        .hardware_notes
        .iter()
        .any(|n| n.contains("Broadcom") && n.contains("extra driver work")));
    assert!(r
        .hardware_notes
        .iter()
        .any(|n| n.contains("BitLocker is off")));
    assert!(r.blocking_warnings.is_empty());
    assert_eq!(r.hardware.bitlocker, "off");

    // report files
    let jp = out.join("preflight-report.json");
    let tp = out.join("preflight-report.txt");
    assert!(jp.exists() && tp.exists());
    let json: serde_json::Value = serde_json::from_str(&fs::read_to_string(&jp).unwrap()).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["buckets"]["native"].as_array().unwrap().len(), 3);

    // txt: dealbreakers FIRST (before the summary and the bucket lists)
    let txt = fs::read_to_string(&tp).unwrap();
    let i_deal = txt.find("!!! WILL NOT WORK ON LINUX !!!").unwrap();
    let i_block = txt.find("BLOCKING WARNINGS").unwrap();
    let i_sum = txt.find("Summary\n").unwrap();
    let i_native = txt.find("READY ON LINUX (NATIVE)").unwrap();
    assert!(
        i_deal < i_sum && i_block < i_sum && i_sum < i_native,
        "dealbreakers first:\n{}",
        txt
    );
    assert!(txt.contains("Visual Studio 2022"));
    assert!(txt.contains("will NOT work:                2"));
    assert!(txt.contains("no --manifest given"));
    // both probes went through the Runner as fixed argv
    let calls = runner.calls.borrow();
    assert_eq!(calls.len(), 2);
    assert!(calls
        .iter()
        .all(|c| c[0] == "powershell" && c[1] == "-NoProfile"));
}

#[test]
fn preflight_bitlocker_on_is_a_blocking_warning_and_games_flagged() {
    let fx = Fixture::new();
    let out = fx.0.join("out");
    let mut apps: Vec<(&str, &str, &str)> = APPS.to_vec();
    apps.push(("VALORANT", "8.0", "Riot Games"));
    apps.push(("Call of Duty: Modern Warfare III", "1.0", "Activision"));
    let runner = Fake::new(
        Ok(apps_json(&apps)),
        Ok(hw_json(
            &["AMD Radeon RX 7600"],
            &["Realtek PCIe GbE"],
            &[],
            "On",
            "FullyEncrypted",
        )),
    );
    let res = run_preflight_with(&runner, None, &out).expect("preflight failed");
    let r = &res.report;

    assert_eq!(r.hardware.bitlocker, "on");
    assert!(r
        .blocking_warnings
        .iter()
        .any(|w| w.contains("BitLocker is ON") && w.contains("cannot read BitLocker")));
    // anti-cheat games matched from the inventory
    assert!(r
        .games
        .iter()
        .any(|g| g.title == "Valorant" && g.matched_in.contains("inventory")));
    assert!(r
        .games
        .iter()
        .any(|g| g.title == "Call of Duty" && g.reason.contains("Ricochet")));
    // GTA V single player is NOT flagged
    assert!(!r
        .games
        .iter()
        .any(|g| g.title.contains("GTA") && !g.title.contains("Online")));

    let txt = fs::read_to_string(out.join("preflight-report.txt")).unwrap();
    let i_games = txt.find("!!! GAMES BLOCKED BY ANTI-CHEAT !!!").unwrap();
    let i_sum = txt.find("Summary\n").unwrap();
    assert!(i_games < i_sum, "games section before summary:\n{}", txt);
    assert!(txt.contains("VALORANT"));
}

#[test]
fn preflight_failed_inventory_is_a_reported_failure_never_an_empty_list() {
    let fx = Fixture::new();
    let out = fx.0.join("out");
    let runner = Fake::new(
        Err("simulated: powershell not found".to_string()),
        Ok(hw_json(
            &[],
            &[],
            &[],
            "unknown",
            "Get-BitLockerVolume unavailable: no cmdlet",
        )),
    );
    let res = run_preflight_with(&runner, None, &out).expect("preflight failed");
    match &res.report.inventory {
        Inventory::Failed(e) => assert!(e.contains("simulated")),
        Inventory::Ok(_) => {
            panic!("READ-PATH RULE violated: failure must not become an empty Ok list")
        }
    }
    // all buckets empty, but the txt says the inventory FAILED loudly
    let txt = fs::read_to_string(out.join("preflight-report.txt")).unwrap();
    assert!(txt.contains("APP INVENTORY FAILED"), "txt: {}", txt);
    // bitlocker unknown => check-manually note, NOT a blocking "on" warning
    assert!(res
        .report
        .hardware_notes
        .iter()
        .any(|n| n.contains("BitLocker status unknown")));
    assert!(res.report.blocking_warnings.is_empty());
}

#[test]
fn preflight_manifest_crossref_adds_placeholder_blocking_warning() {
    let fx = Fixture::new();
    // scan a profile with one placeholder stub
    let root = fx.0.join("profile");
    fs::create_dir_all(root.join("OneDrive")).unwrap();
    fs::create_dir_all(root.join("Documents")).unwrap();
    fs::write(root.join("Documents/real.txt"), b"real").unwrap();
    fs::write(root.join("Documents/stub.txt"), b"stub").unwrap();
    fs::create_dir_all(root.join("Documents/Games/VALORANT")).unwrap();
    fs::write(root.join("Documents/Games/VALORANT/VALORANT.exe"), b"bin").unwrap();
    let stubs = Arc::new(HashMap::from([(
        "Documents/stub.txt".to_string(),
        FILE_ATTRIBUTE_RECALL_ON_OPEN | FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS,
    )]));
    let attrs = move |p: &Path| -> Option<u32> {
        let rel = p
            .to_string_lossy()
            .replace('\\', "/")
            .split("profile/")
            .nth(1)
            .unwrap_or("")
            .to_string();
        stubs.get(&rel).copied()
    };
    let scan_out = fx.0.join("scan-out");
    let scan = run_scan_with_attrs(&root, &Config::default_config(), &scan_out, None, &attrs)
        .expect("scan failed");
    assert_eq!(scan.manifest.placeholders.count, 1);

    let out = fx.0.join("out");
    let runner = Fake::new(
        Ok(apps_json(&[
            ("Steam", "1", "Valve"),
            ("Grand Theft Auto V Online", "1", "Rockstar"),
        ])),
        Ok(hw_json(
            &["NVIDIA GTX 1060"],
            &["Intel Wi-Fi"],
            &[],
            "Off",
            "",
        )),
    );
    let res = run_preflight_with(&runner, Some(&scan_out.join("manifest.json")), &out)
        .expect("preflight failed");
    let sc = res.report.scan.as_ref().expect("scan crossref");
    assert_eq!(sc.placeholder_count, 1);
    assert!(
        sc.games_detected.iter().any(|g| g == "Valorant"),
        "games_detected: {:?}",
        sc.games_detected
    );
    assert!(res.report.blocking_warnings.iter().any(
        |w| w.contains("online-only placeholders") && w.contains("Always keep on this device")
    ));
    // GTA Online matched in the app inventory too
    assert!(res
        .report
        .games
        .iter()
        .any(|g| g.title == "Grand Theft Auto V Online"));
}

#[test]
fn verdict_table_pins_every_kind_and_anticheat_titles() {
    // every verdict kind appears at least once in APP_ROWS
    let mut kinds = std::collections::HashSet::new();
    for row in APP_ROWS {
        kinds.insert(row.verdict.kind());
    }
    for k in [
        "native",
        "equivalent",
        "web_only",
        "will_not_work",
        "unknown",
    ] {
        // "unknown" is the fallback, not a row; the other four must have rows
        if k != "unknown" {
            assert!(kinds.contains(k), "APP_ROWS must pin verdict kind {}", k);
        }
    }
    // specific pins
    assert_eq!(app_verdict("Visual Studio Code"), Verdict::Native);
    assert!(matches!(
        app_verdict("Visual Studio 2022 Community"),
        Verdict::WillNotWork { .. }
    ));
    assert!(matches!(
        app_verdict("Microsoft Office 2021"),
        Verdict::Equivalent { .. }
    ));
    assert!(matches!(app_verdict("Microsoft Teams"), Verdict::WebOnly));
    assert!(matches!(
        app_verdict("AutoCAD 2024"),
        Verdict::WillNotWork { .. }
    ));
    assert_eq!(app_verdict("Dropbox"), Verdict::Native);
    assert_eq!(app_verdict("Steam"), Verdict::Native);
    assert_eq!(app_verdict("Discord"), Verdict::Native);
    assert_eq!(app_verdict("Spotify"), Verdict::Native);
    assert_eq!(app_verdict("Zoom"), Verdict::Native);
    assert!(matches!(
        app_verdict("Adobe Photoshop 2024"),
        Verdict::Equivalent { .. }
    ));
    assert!(matches!(
        app_verdict("Adobe Premiere Pro"),
        Verdict::WillNotWork { .. }
    ));
    assert_eq!(
        app_verdict("Something Completely Unknown"),
        Verdict::Unknown
    );

    // anti-cheat: exact titles match; variants via prefix + separator; GTA V SP does not
    for t in [
        "Valorant",
        "League of Legends",
        "Fortnite",
        "Apex Legends",
        "Destiny 2",
        "PUBG: Battlegrounds",
        "Rainbow Six Siege",
    ] {
        assert!(
            game_row_for(t).is_some(),
            "anti-cheat title must match: {}",
            t
        );
    }
    assert!(game_row_for("Call of Duty: Modern Warfare III").is_some());
    assert!(game_row_for("Grand Theft Auto V Online").is_some());
    assert!(
        game_row_for("Grand Theft Auto V").is_none(),
        "GTA V single-player must NOT be flagged"
    );
    assert!(
        game_row_for("Minecraft").is_none(),
        "generic Steam game must NOT be flagged"
    );
    assert!(GAME_ROWS.iter().all(|r| r.title.len() > 3));
}
