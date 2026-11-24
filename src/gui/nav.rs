//! Screen enumeration and navigation. One screen per CLI phase; which screens
//! appear depends on the platform build, matching docs/UI_PLAN.md:
//! scan + stage on the Windows binary, import + setup + games on the Linux
//! binary, with the "Install Linux" interstitial bridging the two.

use crate::gui::runner::JobKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Scan,
    Stage,
    Install,
    Import,
    Setup,
    Games,
    Summary,
}

impl Screen {
    pub fn title(self) -> &'static str {
        match self {
            Screen::Scan => "1. Scan",
            Screen::Stage => "2. Stage",
            Screen::Install => "3. Install Linux",
            Screen::Import => "4. Import",
            Screen::Setup => "5. Setup",
            Screen::Games => "6. Games",
            Screen::Summary => "Summary",
        }
    }

    /// The "what this does" line shown at the top of every screen.
    pub fn blurb(self) -> &'static str {
        match self {
            Screen::Scan => {
                "Reads your Windows profile (read-only), classifies every file, \
                             and writes manifest.json + report.txt into the output folder."
            }
            Screen::Stage => {
                "Copies every keeper to <staging>/osmosis-stage/, hash-verifying \
                              each file. Resumable: re-running skips what is already correct."
            }
            Screen::Install => {
                "OSmosis never touches partitions. Create a USB installer for a \
                                 supported distro (Linux Mint, Ubuntu, Debian, Fedora, Arch \
                                 Linux), install it, then come back here."
            }
            Screen::Import => {
                "Unpacks the staged tree into your home directory, hash-verifying \
                               every file. Conflicts are never overwritten."
            }
            Screen::Setup => {
                "Turns the fresh install into the working machine (packages, drivers, \
                              dev tools, Lutris + Wine-GE). Idempotent; never reboots by itself."
            }
            Screen::Games => {
                "Builds one Wine prefix per game and registers it in Lutris. The \
                              games drive is only ever read."
            }
            Screen::Summary => "Where every report lives after a run.",
        }
    }

    /// The engine job this screen launches, if any.
    pub fn job_kind(self) -> Option<JobKind> {
        match self {
            Screen::Scan => Some(JobKind::Scan),
            Screen::Stage => Some(JobKind::Stage),
            Screen::Import => Some(JobKind::Import),
            Screen::Setup => Some(JobKind::Setup),
            Screen::Games => Some(JobKind::Games),
            Screen::Install | Screen::Summary => None,
        }
    }
}

/// Screens offered by this platform build.
pub fn screens_for_platform() -> &'static [Screen] {
    if cfg!(windows) {
        &[
            Screen::Scan,
            Screen::Stage,
            Screen::Install,
            Screen::Summary,
        ]
    } else {
        &[
            Screen::Install,
            Screen::Import,
            Screen::Setup,
            Screen::Games,
            Screen::Summary,
        ]
    }
}

pub fn next_screen(s: Screen) -> Option<Screen> {
    let list = screens_for_platform();
    list.iter()
        .position(|x| *x == s)
        .and_then(|i| list.get(i + 1).copied())
}

pub fn prev_screen(s: Screen) -> Option<Screen> {
    let list = screens_for_platform();
    list.iter()
        .position(|x| *x == s)
        .filter(|i| *i > 0)
        .and_then(|i| list.get(i - 1).copied())
}

/// Next is enabled for the interstitial/summary always; for a phase screen
/// only after that phase's job finished successfully.
pub fn can_advance(s: Screen, last_ok: Option<(JobKind, bool)>) -> bool {
    match s.job_kind() {
        None => true,
        Some(kind) => last_ok == Some((kind, true)),
    }
}
