//! Preflight verdict tables (slice 9.4) — pure DATA, no if-chains: every row
//! is a real verdict, every row is pinned by a test (tests/preflight_fixture.rs).
//!
//! App matching: case-insensitive SUBSTRING on the installed app name, first
//! matching row wins (so "Visual Studio Code" must precede "Visual Studio").
//! Game matching: exact title match (case-insensitive), or the name starts
//! with the title followed by a non-alphanumeric separator ("Call of Duty:
//! ..."). GTA V single-player does NOT match the "... V Online" row.

/// The verdict for one app pattern.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// A native Linux version exists.
    Native,
    /// A Linux equivalent exists (possibly partial — see note).
    Equivalent {
        linux_name: &'static str,
        note: &'static str,
    },
    /// Only usable through a web browser on Linux.
    WebOnly,
    /// Will NOT work on Linux.
    WillNotWork { reason: &'static str },
    /// No row matched — listed, never silently dropped.
    Unknown,
}

impl Verdict {
    pub fn kind(&self) -> &'static str {
        match self {
            Verdict::Native => "native",
            Verdict::Equivalent { .. } => "equivalent",
            Verdict::WebOnly => "web_only",
            Verdict::WillNotWork { .. } => "will_not_work",
            Verdict::Unknown => "unknown",
        }
    }

    pub fn linux_name(&self) -> &'static str {
        match self {
            Verdict::Equivalent { linux_name, .. } => linux_name,
            _ => "",
        }
    }

    pub fn note(&self) -> &'static str {
        match self {
            Verdict::Equivalent { note, .. } => note,
            _ => "",
        }
    }

    pub fn reason(&self) -> &'static str {
        match self {
            Verdict::WillNotWork { reason } => reason,
            _ => "",
        }
    }
}

pub struct AppRow {
    pub pattern: &'static str,
    pub verdict: Verdict,
}

/// Known-app verdicts. FIRST MATCH WINS — order is significant.
pub const APP_ROWS: &[AppRow] = &[
    AppRow {
        pattern: "visual studio code",
        verdict: Verdict::Native,
    },
    AppRow {
        pattern: "visual studio",
        verdict: Verdict::WillNotWork {
            reason: "the full Visual Studio IDE has no Linux version; use VS Code (native) or \
                     Rider instead",
        },
    },
    AppRow {
        pattern: "microsoft office",
        verdict: Verdict::Equivalent {
            linux_name: "LibreOffice / OnlyOffice",
            note: "full office suite; OnlyOffice matches MS Office formatting most closely",
        },
    },
    AppRow {
        pattern: "microsoft 365",
        verdict: Verdict::Equivalent {
            linux_name: "LibreOffice / OnlyOffice",
            note: "the Microsoft 365 apps (Word/Excel/PowerPoint) map to LibreOffice/OnlyOffice; \
                   some advanced features differ",
        },
    },
    AppRow {
        pattern: "office 365",
        verdict: Verdict::Equivalent {
            linux_name: "LibreOffice / OnlyOffice",
            note: "same as Microsoft 365 apps",
        },
    },
    AppRow {
        pattern: "adobe photoshop",
        verdict: Verdict::Equivalent {
            linux_name: "GIMP / Krita",
            note: "partial replacement only — NOT a full Photoshop replacement; check your \
                   plugins and workflows before switching",
        },
    },
    AppRow {
        pattern: "adobe premiere",
        verdict: Verdict::WillNotWork {
            reason: "no Linux equivalent at parity (DaVinci Resolve is the closest, still not \
                     parity)",
        },
    },
    AppRow {
        pattern: "adobe illustrator",
        verdict: Verdict::Equivalent {
            linux_name: "Inkscape",
            note: "partial replacement for vector illustration",
        },
    },
    AppRow {
        pattern: "itunes",
        verdict: Verdict::Equivalent {
            linux_name: "Rhythmbox / Clementine",
            note: "library playback works; iPhone sync differs from iTunes",
        },
    },
    AppRow {
        pattern: "onedrive",
        verdict: Verdict::Equivalent {
            linux_name: "rclone / abraunegg OneDrive client",
            note: "sync works, but Files On-Demand placeholders must be fully downloaded first \
                   (see the scan report)",
        },
    },
    AppRow {
        pattern: "google drive",
        verdict: Verdict::Equivalent {
            linux_name: "rclone / Insync",
            note: "sync client, not the web UI",
        },
    },
    AppRow {
        pattern: "dropbox",
        verdict: Verdict::Native,
    },
    AppRow {
        pattern: "steam",
        verdict: Verdict::Native,
    },
    AppRow {
        pattern: "epic games",
        verdict: Verdict::Equivalent {
            linux_name: "Heroic Games Launcher",
            note: "covers the Epic Games Store library on Linux",
        },
    },
    AppRow {
        pattern: "discord",
        verdict: Verdict::Native,
    },
    AppRow {
        pattern: "spotify",
        verdict: Verdict::Native,
    },
    AppRow {
        pattern: "zoom",
        verdict: Verdict::Native,
    },
    AppRow {
        pattern: "microsoft teams",
        verdict: Verdict::WebOnly,
    },
    AppRow {
        pattern: "teams",
        verdict: Verdict::WebOnly,
    },
    AppRow {
        pattern: "autocad",
        verdict: Verdict::WillNotWork {
            reason: "no Linux AutoCAD; LibreCAD/FreeCAD are not equivalents",
        },
    },
    AppRow {
        pattern: "visio",
        verdict: Verdict::WebOnly,
    },
    AppRow {
        pattern: "microsoft project",
        verdict: Verdict::WebOnly,
    },
];

/// Verdict for an installed app name: first matching row wins, else Unknown.
pub fn app_verdict(name: &str) -> Verdict {
    let lower = name.to_lowercase();
    for row in APP_ROWS {
        if lower.contains(row.pattern) {
            return row.verdict.clone();
        }
    }
    Verdict::Unknown
}

pub struct GameRow {
    pub title: &'static str,
    pub reason: &'static str,
}

/// Games whose kernel-level anti-cheat does NOT run on Linux
/// (AreWeAntiCheatYet categories). Generic Steam games and single-player modes
/// are NOT flagged — only these exact titles.
pub const GAME_ROWS: &[GameRow] = &[
    GameRow {
        title: "Valorant",
        reason: "Vanguard kernel anti-cheat does not run on Linux",
    },
    GameRow {
        title: "League of Legends",
        reason: "Vanguard kernel anti-cheat does not run on Linux",
    },
    GameRow {
        title: "Fortnite",
        reason: "Easy Anti-Cheat kernel driver is not supported on Linux",
    },
    GameRow {
        title: "Apex Legends",
        reason: "Easy Anti-Cheat kernel driver is not supported on Linux",
    },
    GameRow {
        title: "Destiny 2",
        reason: "BattlEye kernel anti-cheat blocks Linux",
    },
    GameRow {
        title: "PUBG: Battlegrounds",
        reason: "kernel anti-cheat is not supported on Linux",
    },
    GameRow {
        title: "Rainbow Six Siege",
        reason: "BattlEye kernel anti-cheat blocks Linux",
    },
    GameRow {
        title: "Call of Duty",
        reason: "Ricochet kernel anti-cheat in recent titles blocks Linux",
    },
    GameRow {
        title: "Grand Theft Auto V Online",
        reason: "BattlEye blocks Linux in GTA Online (single-player is fine)",
    },
    GameRow {
        title: "GTA V Online",
        reason: "BattlEye blocks Linux in GTA Online (single-player is fine)",
    },
];

/// Exact-title game match: case-insensitive equality, or the candidate starts
/// with the title followed by a non-alphanumeric char ("Call of Duty: …").
/// "Grand Theft Auto V" (single player) does NOT match "… V Online".
pub fn game_row_for(name: &str) -> Option<&'static GameRow> {
    let lower = name.to_lowercase();
    GAME_ROWS.iter().find(|r| {
        let t = r.title.to_lowercase();
        if lower == t {
            return true;
        }
        lower.starts_with(&t)
            && lower[t.len()..]
                .chars()
                .next()
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(false)
    })
}

/// Built-in WiFi-adapter caution list: (needle, label). An adapter whose name
/// contains the needle (and is WiFi-classified) gets the "may need extra
/// driver work" note.
pub const WIFI_CAUTION_ROWS: &[(&str, &str)] = &[
    ("broadcom", "Broadcom WiFi"),
    ("realtek usb", "Realtek USB WiFi"),
];
