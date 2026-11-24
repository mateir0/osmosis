//! The setup step runner (slice 4): `osmosis setup` turns a fresh Linux Mint
//! (post-import) into the working machine. Linux-only, unprivileged: every
//! step escalates per-command through `sudo` in a fixed argv array.
//!
//! SAFETY RULE: every external command is a fixed argv array executed through
//! one `Runner` abstraction — no `sh -c` with interpolated strings anywhere
//! (the single `bash -c` call uses a compile-time constant). Downloads are
//! saved to a temp file first, their URL + sha256 recorded in the report, and
//! only then executed. Nothing imported by `osmosis import` is edited.
//! No reboot is ever executed — only flagged in the report.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const SETUP_DIR_NAME: &str = "osmosis-setup";

/// Prefix of every privileged apt/dpkg argv. `sudo` strips the caller's
/// environment, so `DEBIAN_FRONTEND=noninteractive` must be passed through
/// `env` explicitly — otherwise apt-get and .deb postinst scripts (e.g. the
/// VS Code Microsoft-repo prompt) never see it and can hang a non-interactive
/// run (observed live on 2026-11-20).
pub const SUDO_APT_PREFIX: &[&str] = &["sudo", "env", "DEBIAN_FRONTEND=noninteractive"];

/// `sudo env DEBIAN_FRONTEND=noninteractive apt-get <args...>` as a fixed argv.
pub fn sudo_apt_get<'a>(args: &[&'a str]) -> Vec<&'a str> {
    let mut v = SUDO_APT_PREFIX.to_vec();
    v.push("apt-get");
    v.extend_from_slice(args);
    v
}

const APT_BASE_PACKAGES: &[&str] = &[
    "git",
    "curl",
    "build-essential",
    "ntfs-3g",
    "software-properties-common",
    "fonts-noto",
    "fonts-liberation",
];

const NVM_VERSION: &str = "v0.40.1";
const NVM_URL: &str = "https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh";
const NVM_CMD: &str = "export NVM_DIR=\"$HOME/.nvm\"; . \"$NVM_DIR/nvm.sh\"; nvm install --lts; nvm alias default 'lts/*'; npm install -g pnpm";

const RUSTUP_URL: &str = "https://sh.rustup.rs";

const GH_KEYRING_URL: &str = "https://cli.github.com/packages/githubcli-archive-keyring.gpg";
const GH_SOURCES_ENTRY: &str = "deb [arch=amd64 signed-by=/usr/share/keyrings/githubcli-archive-keyring.gpg] https://cli.github.com/packages stable main";

const VSCODE_URL: &str = "https://code.visualstudio.com/sha/download?build=stable&os=linux-deb-x64";
const CHROME_URL: &str =
    "https://dl.google.com/linux/direct/google-chrome-stable_current_amd64.deb";

const OPENCODE_INSTALL_URL: &str = "https://opencode.ai/install";
const OPENCODE_API: &str = "https://api.github.com/repos/anomalyco/opencode/releases/latest";
const OPENCODE_RELEASES_URL: &str = "https://github.com/anomalyco/opencode/releases";

const LUTRIS_API: &str =
    "https://api.github.com/repos/GloriousEggroll/wine-ge-custom/releases/latest";
const WINE_GE_RELEASES_URL: &str = "https://github.com/GloriousEggroll/wine-ge-custom/releases";

/// Output of one external command.
#[derive(Debug, Clone, Default)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    pub fn ok(stdout: impl Into<String>) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    pub fn fail(status: i32, stderr: impl Into<String>) -> CommandOutput {
        CommandOutput {
            status,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

/// The single abstraction every external command goes through. Detect queries
/// use `output`; state-changing commands use the default `run` (which fails on
/// a non-zero exit). Tests supply a fake implementation.
pub trait Runner {
    fn output(&self, argv: &[&str]) -> Result<CommandOutput, String>;

    fn run(&self, argv: &[&str]) -> Result<(), String> {
        let out = self.output(argv)?;
        if out.status == 0 {
            Ok(())
        } else {
            let err = out.stderr.trim();
            if err.is_empty() {
                Err(format!(
                    "{} exited with status {}",
                    argv.join(" "),
                    out.status
                ))
            } else {
                Err(format!(
                    "{} exited with status {}: {}",
                    argv.join(" "),
                    out.status,
                    err
                ))
            }
        }
    }
}

/// The real runner: fixed argv, `DEBIAN_FRONTEND=noninteractive` for apt.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn output(&self, argv: &[&str]) -> Result<CommandOutput, String> {
        let (program, args) = argv
            .split_first()
            .ok_or_else(|| "empty command argv".to_string())?;
        let out = Command::new(program)
            .args(args)
            .env("DEBIAN_FRONTEND", "noninteractive")
            .output()
            .map_err(|e| format!("cannot run {}: {}", program, e))?;
        Ok(CommandOutput {
            status: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRecord {
    pub step: String,
    pub url: String,
    pub sha256: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepReport {
    pub id: String,
    pub title: String,
    /// done | skipped-already | not-applicable | failed | would-run (dry-run)
    pub status: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetupReport {
    pub schema_version: u32,
    pub created_at: String,
    pub home: String,
    pub os: String,
    pub dry_run: bool,
    pub reboot_required: bool,
    pub total_steps: u64,
    pub done: u64,
    pub skipped_already: u64,
    pub not_applicable: u64,
    pub failed: u64,
    pub would_run: u64,
    pub steps: Vec<StepReport>,
    pub downloads: Vec<DownloadRecord>,
    pub manual_followups: Vec<String>,
}

#[derive(Debug)]
pub struct SetupOutcome {
    pub report: SetupReport,
    pub summary: String,
    /// 0 = no step failed; 2 = at least one step failed. Hard refusals are
    /// returned as `Err` by `run_setup` and exit 1 at the CLI.
    pub exit_code: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepKind {
    Base,
    Nvidia,
    Gh,
    Node,
    Rust,
    VsCode,
    Chrome,
    OpenCode,
    Lutris,
}

struct StepDef {
    id: &'static str,
    title: &'static str,
    kind: StepKind,
}

const STEP_DEFS: [StepDef; 9] = [
    StepDef {
        id: "base",
        title: "Base system packages (apt)",
        kind: StepKind::Base,
    },
    StepDef {
        id: "nvidia",
        title: "NVIDIA driver",
        kind: StepKind::Nvidia,
    },
    StepDef {
        id: "gh",
        title: "GitHub CLI",
        kind: StepKind::Gh,
    },
    StepDef {
        id: "node",
        title: "Node via nvm + pnpm",
        kind: StepKind::Node,
    },
    StepDef {
        id: "rust",
        title: "Rust via rustup",
        kind: StepKind::Rust,
    },
    StepDef {
        id: "vscode",
        title: "VS Code",
        kind: StepKind::VsCode,
    },
    StepDef {
        id: "chrome",
        title: "Google Chrome",
        kind: StepKind::Chrome,
    },
    StepDef {
        id: "opencode",
        title: "OpenCode CLI + desktop",
        kind: StepKind::OpenCode,
    },
    StepDef {
        id: "lutris",
        title: "Lutris + Wine-GE groundwork",
        kind: StepKind::Lutris,
    },
];

enum Detect {
    Done(String),
    Needed(String),
    NotApplicable(String),
}

struct RunNote {
    detail: String,
    reboot: bool,
    followups: Vec<String>,
}

impl RunNote {
    fn new(detail: impl Into<String>) -> RunNote {
        RunNote {
            detail: detail.into(),
            reboot: false,
            followups: Vec::new(),
        }
    }
}

struct Ctx {
    home: PathBuf,
    work: PathBuf,
    downloads: Vec<DownloadRecord>,
    followups: Vec<String>,
    reboot_required: bool,
}

impl Ctx {
    fn new(home: &Path, work: &Path) -> Ctx {
        Ctx {
            home: home.to_path_buf(),
            work: work.to_path_buf(),
            downloads: Vec::new(),
            followups: Vec::new(),
            reboot_required: false,
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or("").trim().to_string()
}

fn version_of(r: &dyn Runner, argv: &[&str]) -> Option<String> {
    match r.output(argv) {
        Ok(o) if o.status == 0 => {
            let line = first_line(&o.stdout);
            if line.is_empty() {
                None
            } else {
                Some(line)
            }
        }
        _ => None,
    }
}

fn package_installed(r: &dyn Runner, pkg: &str) -> bool {
    match r.output(&["dpkg-query", "-W", "-f=${Status}", pkg]) {
        Ok(o) => o.status == 0 && o.stdout.contains("install ok installed"),
        Err(_) => false,
    }
}

fn runner_dir_has_wine(runners: &Path) -> bool {
    match fs::read_dir(runners) {
        Ok(rd) => rd
            .filter_map(Result::ok)
            .any(|e| e.path().join("bin/wine").is_file()),
        Err(_) => false,
    }
}

/// Save `url` to `dest` with curl, then record its sha256 (via `sha256sum`)
/// before the caller executes it. No piping of remote scripts.
fn fetch(r: &dyn Runner, ctx: &mut Ctx, step: &str, url: &str, dest: &Path) -> Result<(), String> {
    let d = dest.to_string_lossy().into_owned();
    r.run(&["curl", "-fsSL", "--retry", "3", "-o", &d, url])?;
    let sum = r
        .output(&["sha256sum", &d])
        .map_err(|e| format!("cannot hash download {}: {}", url, e))?;
    let sha = sum
        .stdout
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    if sha.len() != 64 {
        return Err(format!(
            "cannot record sha256 for {} (sha256sum output: {:?})",
            url,
            sum.stdout.trim()
        ));
    }
    ctx.downloads.push(DownloadRecord {
        step: step.to_string(),
        url: url.to_string(),
        sha256: sha,
        path: d,
    });
    Ok(())
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

fn resolve_release(r: &dyn Runner, api: &str) -> Result<GhRelease, String> {
    let out = r.output(&[
        "curl",
        "-fsSL",
        "--retry",
        "3",
        "-H",
        "Accept: application/vnd.github+json",
        api,
    ])?;
    if out.status != 0 {
        return Err(format!(
            "GitHub API request failed ({}): {}",
            out.status,
            out.stderr.trim()
        ));
    }
    serde_json::from_str(&out.stdout)
        .map_err(|e| format!("cannot parse GitHub API response: {}", e))
}

fn sanitize_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// Case-insensitive match for the Wine-GE Lutris release asset. The real
/// releases ship mixed-case names, e.g.
/// `wine-lutris-GE-Proton8-26-x86_64.tar.xz` (GE-Proton8-26), so the comparison
/// runs on a lowercased copy of the asset name.
fn is_wine_ge_tarball(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.contains("wine-lutris-ge") && lower.ends_with(".tar.xz")
}

#[cfg(unix)]
fn set_executable(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

fn detect_step(kind: StepKind, r: &dyn Runner, ctx: &Ctx) -> Detect {
    match kind {
        StepKind::Base => {
            let mut missing: Vec<&str> = Vec::new();
            for bin in ["git", "curl", "gcc"] {
                if version_of(r, &[bin, "--version"]).is_none() {
                    missing.push(bin);
                }
            }
            if !package_installed(r, "fonts-noto") {
                missing.push("fonts-noto");
            }
            if !(package_installed(r, "mint-meta-codecs")
                || package_installed(r, "ubuntu-restricted-extras"))
            {
                missing.push("codecs");
            }
            if missing.is_empty() {
                Detect::Done("git, curl, gcc and font/codec packages present".to_string())
            } else {
                Detect::Needed(format!("missing: {}", missing.join(", ")))
            }
        }
        StepKind::Nvidia => {
            let lspci = r
                .output(&["lspci"])
                .ok()
                .filter(|o| o.status == 0)
                .map(|o| o.stdout)
                .unwrap_or_default();
            if !lspci.to_lowercase().contains("nvidia") {
                return Detect::NotApplicable("no NVIDIA GPU detected (lspci)".to_string());
            }
            match version_of(r, &["nvidia-smi"]) {
                Some(v) => Detect::Done(format!("nvidia-smi responds: {}", v)),
                None => Detect::Needed("NVIDIA GPU present; driver not active".to_string()),
            }
        }
        StepKind::Gh => match version_of(r, &["gh", "--version"]) {
            Some(v) => Detect::Done(v),
            None => Detect::Needed("gh not found".to_string()),
        },
        StepKind::Node => {
            let nvm = ctx.home.join(".nvm/nvm.sh").is_file();
            match version_of(r, &["bash", "-lc", "node --version"]) {
                Some(v) if nvm => Detect::Done(v),
                Some(v) => Detect::Needed(format!("node {} present but ~/.nvm/nvm.sh missing", v)),
                None => Detect::Needed("node/nvm not found".to_string()),
            }
        }
        StepKind::Rust => {
            let cargo_bin = ctx.home.join(".cargo/bin/rustc").is_file();
            match version_of(r, &["rustc", "--version"]) {
                Some(v) if cargo_bin => Detect::Done(v),
                Some(v) => Detect::Needed(format!(
                    "rustc {} present but ~/.cargo/bin/rustc missing",
                    v
                )),
                None => Detect::Needed("rustup/cargo not found".to_string()),
            }
        }
        StepKind::VsCode => match version_of(r, &["code", "--version"]) {
            Some(v) => Detect::Done(v),
            None => Detect::Needed("code not found".to_string()),
        },
        StepKind::Chrome => match version_of(r, &["google-chrome", "--version"]) {
            Some(v) => Detect::Done(v),
            None => Detect::Needed("google-chrome not found".to_string()),
        },
        StepKind::OpenCode => {
            let bin = ctx.home.join(".opencode/bin/opencode").is_file();
            match version_of(r, &["opencode", "--version"]) {
                Some(v) if bin => Detect::Done(v),
                Some(v) => Detect::Needed(format!(
                    "opencode {} present but ~/.opencode/bin missing",
                    v
                )),
                None => Detect::Needed("opencode not found".to_string()),
            }
        }
        StepKind::Lutris => {
            let runners = ctx.home.join(".local/share/lutris/runners/wine");
            match version_of(r, &["lutris", "--version"]) {
                Some(v) if runner_dir_has_wine(&runners) => Detect::Done(v),
                Some(v) => Detect::Needed(format!("lutris {} present; Wine-GE runner missing", v)),
                None => Detect::Needed("lutris not found".to_string()),
            }
        }
    }
}

fn parse_recommended_driver(text: &str) -> Option<String> {
    for line in text.lines() {
        if !line.contains("recommended") {
            continue;
        }
        for tok in line.split_whitespace() {
            if tok.starts_with("nvidia-driver-") {
                return Some(tok.to_string());
            }
        }
    }
    None
}

fn run_step(kind: StepKind, r: &dyn Runner, ctx: &mut Ctx) -> Result<RunNote, String> {
    match kind {
        StepKind::Base => {
            r.run(&sudo_apt_get(&["update"]))?;
            let mut argv = sudo_apt_get(&["install", "-y"]);
            argv.extend_from_slice(APT_BASE_PACKAGES);
            r.run(&argv)?;
            if r.run(&sudo_apt_get(&["install", "-y", "mint-meta-codecs"]))
                .is_err()
            {
                r.run(&sudo_apt_get(&[
                    "install",
                    "-y",
                    "ubuntu-restricted-extras",
                ]))?;
            }
            Ok(RunNote::new("base packages installed"))
        }
        StepKind::Nvidia => {
            let present = r
                .output(&["which", "ubuntu-drivers"])
                .map(|o| o.status == 0)
                .unwrap_or(false);
            if !present {
                return Err(
                    "ubuntu-drivers is not available; install the NVIDIA driver manually (Driver Manager)"
                        .to_string(),
                );
            }
            if r.run(&["sudo", "ubuntu-drivers", "autoinstall"]).is_err() {
                // older ubuntu-drivers without `autoinstall`: parse the
                // recommended driver and install it explicitly.
                let dev = r.output(&["ubuntu-drivers", "devices"])?;
                if dev.status != 0 {
                    return Err(format!(
                        "ubuntu-drivers devices failed ({}): {}",
                        dev.status,
                        dev.stderr.trim()
                    ));
                }
                let driver = parse_recommended_driver(&dev.stdout).ok_or_else(|| {
                    "cannot determine the recommended NVIDIA driver from ubuntu-drivers devices"
                        .to_string()
                })?;
                r.run(&sudo_apt_get(&["install", "-y", &driver]))?;
            }
            let mut note = RunNote::new("NVIDIA driver installed");
            note.reboot = true;
            Ok(note)
        }
        StepKind::Gh => {
            let keyring = ctx.work.join("githubcli-archive-keyring.gpg");
            fetch(r, ctx, "gh", GH_KEYRING_URL, &keyring)?;
            let k = keyring.to_string_lossy().into_owned();
            r.run(&[
                "sudo",
                "install",
                "-m",
                "0644",
                &k,
                "/usr/share/keyrings/githubcli-archive-keyring.gpg",
            ])?;
            let sources = ctx.work.join("github-cli.list");
            fs::write(&sources, format!("{}\n", GH_SOURCES_ENTRY))
                .map_err(|e| format!("cannot write {}: {}", sources.display(), e))?;
            let s = sources.to_string_lossy().into_owned();
            r.run(&[
                "sudo",
                "install",
                "-m",
                "0644",
                &s,
                "/etc/apt/sources.list.d/github-cli.list",
            ])?;
            r.run(&sudo_apt_get(&["update"]))?;
            r.run(&sudo_apt_get(&["install", "-y", "gh"]))?;
            Ok(RunNote::new(
                "GitHub CLI installed from the official apt repo",
            ))
        }
        StepKind::Node => {
            let script = ctx.work.join("nvm-install.sh");
            fetch(r, ctx, "node", NVM_URL, &script)?;
            let p = script.to_string_lossy().into_owned();
            r.run(&["bash", &p])?;
            r.run(&["bash", "-c", NVM_CMD])?;
            Ok(RunNote::new(format!(
                "nvm {} installed; node LTS and pnpm set up",
                NVM_VERSION
            )))
        }
        StepKind::Rust => {
            let script = ctx.work.join("rustup-init.sh");
            fetch(r, ctx, "rust", RUSTUP_URL, &script)?;
            let p = script.to_string_lossy().into_owned();
            r.run(&["bash", &p, "-y", "--default-toolchain", "stable"])?;
            Ok(RunNote::new("rustup installed with the stable toolchain"))
        }
        StepKind::VsCode => {
            let deb = ctx.work.join("vscode.deb");
            fetch(r, ctx, "vscode", VSCODE_URL, &deb)?;
            let p = deb.to_string_lossy().into_owned();
            r.run(&sudo_apt_get(&["install", "-y", &p]))?;
            Ok(RunNote::new("VS Code .deb installed"))
        }
        StepKind::Chrome => {
            let deb = ctx.work.join("google-chrome.deb");
            fetch(r, ctx, "chrome", CHROME_URL, &deb)?;
            let p = deb.to_string_lossy().into_owned();
            r.run(&sudo_apt_get(&["install", "-y", &p]))?;
            Ok(RunNote::new("Google Chrome .deb installed"))
        }
        StepKind::OpenCode => {
            let script = ctx.work.join("opencode-install.sh");
            fetch(r, ctx, "opencode", OPENCODE_INSTALL_URL, &script)?;
            let p = script.to_string_lossy().into_owned();
            r.run(&["bash", &p])?;
            let mut note = RunNote::new("OpenCode CLI installed");
            if let Err(e) = install_opencode_desktop(r, ctx) {
                note.detail
                    .push_str(&format!("; desktop sub-step failed: {}", e));
                note.followups
                    .push(format!("OpenCode desktop: {}", OPENCODE_RELEASES_URL));
            } else {
                note.detail.push_str("; desktop installed");
            }
            Ok(note)
        }
        StepKind::Lutris => {
            r.run(&sudo_apt_get(&["install", "-y", "lutris", "winetricks"]))?;
            let rel = resolve_release(r, LUTRIS_API).map_err(|e| {
                format!(
                    "cannot resolve Wine-GE release: {} ({})",
                    e, WINE_GE_RELEASES_URL
                )
            })?;
            let asset = rel
                .assets
                .iter()
                .find(|a| is_wine_ge_tarball(&a.name))
                .ok_or_else(|| {
                    format!(
                        "no wine-lutris-ge .tar.xz asset in release {} ({})",
                        rel.tag_name, WINE_GE_RELEASES_URL
                    )
                })?;
            let archive = ctx.work.join(&asset.name);
            fetch(r, ctx, "lutris", &asset.browser_download_url, &archive)?;
            let runners = ctx.home.join(".local/share/lutris/runners/wine");
            fs::create_dir_all(&runners)
                .map_err(|e| format!("cannot create {}: {}", runners.display(), e))?;
            let target = runners.join(sanitize_name(&rel.tag_name));
            fs::create_dir_all(&target)
                .map_err(|e| format!("cannot create {}: {}", target.display(), e))?;
            let a = archive.to_string_lossy().into_owned();
            let t = target.to_string_lossy().into_owned();
            r.run(&["tar", "-xf", &a, "-C", &t, "--strip-components=1"])?;
            Ok(RunNote::new(format!(
                "lutris + winetricks installed; Wine-GE {} extracted",
                rel.tag_name
            )))
        }
    }
}

/// Desktop sub-step: resolve the latest OpenCode release and install a .deb,
/// or place an AppImage under ~/Applications with a .desktop entry.
fn install_opencode_desktop(r: &dyn Runner, ctx: &mut Ctx) -> Result<(), String> {
    let rel = resolve_release(r, OPENCODE_API)?;
    if let Some(asset) = rel
        .assets
        .iter()
        .find(|a| a.name.ends_with(".deb") && !a.name.ends_with(".AppImage"))
    {
        let deb = ctx.work.join("opencode.deb");
        fetch(r, ctx, "opencode", &asset.browser_download_url, &deb)?;
        let p = deb.to_string_lossy().into_owned();
        r.run(&sudo_apt_get(&["install", "-y", &p]))?;
        return Ok(());
    }
    if let Some(asset) = rel.assets.iter().find(|a| a.name.ends_with(".AppImage")) {
        let tmp = ctx.work.join(&asset.name);
        fetch(r, ctx, "opencode", &asset.browser_download_url, &tmp)?;
        let apps = ctx.home.join("Applications");
        fs::create_dir_all(&apps)
            .map_err(|e| format!("cannot create {}: {}", apps.display(), e))?;
        let dest = apps.join(&asset.name);
        fs::copy(&tmp, &dest)
            .map_err(|e| format!("cannot copy AppImage to {}: {}", dest.display(), e))?;
        set_executable(&dest)
            .map_err(|e| format!("cannot make {} executable: {}", dest.display(), e))?;
        let desktop = ctx.home.join(".local/share/applications/opencode.desktop");
        if let Some(parent) = desktop.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create {}: {}", parent.display(), e))?;
        }
        fs::write(
            &desktop,
            format!(
                "[Desktop Entry]\nName=OpenCode\nExec={}\nType=Application\nTerminal=false\nCategories=Development;\n",
                dest.display()
            ),
        )
        .map_err(|e| format!("cannot write {}: {}", desktop.display(), e))?;
        return Ok(());
    }
    Err(format!(
        "no Linux .deb or .AppImage asset in release {}",
        rel.tag_name
    ))
}

fn verify_step(kind: StepKind, r: &dyn Runner, ctx: &Ctx) -> Result<String, String> {
    match kind {
        StepKind::Base => {
            let mut parts = Vec::new();
            for bin in ["git", "curl", "gcc"] {
                match version_of(r, &[bin, "--version"]) {
                    Some(v) => parts.push(v),
                    None => return Err(format!("{} does not resolve on PATH", bin)),
                }
            }
            Ok(parts.join("; "))
        }
        StepKind::Nvidia => match version_of(r, &["nvidia-smi"]) {
            Some(v) => Ok(v),
            None => Ok("driver installed; becomes active after reboot".to_string()),
        },
        StepKind::Gh => version_of(r, &["gh", "--version"])
            .ok_or_else(|| "gh --version did not parse".to_string()),
        StepKind::Node => {
            let node = version_of(r, &["bash", "-lc", "node --version"])
                .ok_or_else(|| "node --version did not parse".to_string())?;
            let npm = version_of(r, &["bash", "-lc", "npm --version"])
                .ok_or_else(|| "npm --version did not parse".to_string())?;
            let pnpm = version_of(r, &["bash", "-lc", "pnpm --version"])
                .ok_or_else(|| "pnpm --version did not parse".to_string())?;
            Ok(format!("node {}; npm {}; pnpm {}", node, npm, pnpm))
        }
        StepKind::Rust => {
            // via a login shell: the launching terminal's PATH may be stale
            // (rustup installs into ~/.cargo/bin), a fresh shell re-reads it
            let rustc = version_of(r, &["bash", "-lc", "rustc --version"])
                .ok_or_else(|| "rustc --version did not parse".to_string())?;
            let cargo = version_of(r, &["bash", "-lc", "cargo --version"])
                .ok_or_else(|| "cargo --version did not parse".to_string())?;
            Ok(format!("{}; {}", rustc, cargo))
        }
        StepKind::VsCode => version_of(r, &["code", "--version"])
            .ok_or_else(|| "code --version did not parse".to_string()),
        StepKind::Chrome => version_of(r, &["google-chrome", "--version"])
            .ok_or_else(|| "google-chrome --version did not parse".to_string()),
        StepKind::OpenCode => version_of(r, &["bash", "-lc", "opencode --version"])
            .ok_or_else(|| "opencode --version did not parse".to_string()),
        StepKind::Lutris => {
            let v = version_of(r, &["lutris", "--version"])
                .ok_or_else(|| "lutris --version did not parse".to_string())?;
            let runners = ctx.home.join(".local/share/lutris/runners/wine");
            if !runner_dir_has_wine(&runners) {
                return Err(format!(
                    "Wine-GE runner directory {} has no bin/wine",
                    runners.display()
                ));
            }
            Ok(format!("{}; Wine-GE runner present", v))
        }
    }
}

fn execute_step(def: &StepDef, r: &dyn Runner, ctx: &mut Ctx, dry_run: bool) -> StepReport {
    let detect = detect_step(def.kind, r, ctx);
    let (status, detail) = match detect {
        Detect::Done(d) => ("skipped-already", d),
        Detect::NotApplicable(d) => ("not-applicable", d),
        Detect::Needed(d) => {
            if dry_run {
                ("would-run", d)
            } else {
                let start = ctx.downloads.len();
                let outcome = run_step(def.kind, r, ctx);
                let result = match outcome {
                    Ok(note) => {
                        if note.reboot {
                            ctx.reboot_required = true;
                        }
                        ctx.followups.extend(note.followups);
                        match verify_step(def.kind, r, ctx) {
                            Ok(v) => ("done", format!("{}; {}", note.detail, v)),
                            Err(e) => ("failed", format!("{}; verify failed: {}", note.detail, e)),
                        }
                    }
                    Err(e) => ("failed", e),
                };
                // temp downloads never outlive their step
                for d in &ctx.downloads[start..] {
                    let _ = fs::remove_file(&d.path);
                }
                result
            }
        }
    };
    StepReport {
        id: def.id.to_string(),
        title: def.title.to_string(),
        status: status.to_string(),
        detail,
    }
}

fn assemble_followups(
    steps: &[StepReport],
    reboot_required: bool,
    dynamic: Vec<String>,
) -> Vec<String> {
    let mut out = dynamic;
    let status = |id: &str| {
        steps
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.status.as_str())
            .unwrap_or("")
    };
    // A tool counts as "present after setup" whether it was just installed or
    // was already there.
    let present = |id: &str| matches!(status(id), "done" | "skipped-already");
    if present("gh") {
        out.push("run: gh auth login".to_string());
    }
    if present("vscode") {
        out.push("VS Code: sign into Settings Sync to restore extensions".to_string());
    }
    if present("chrome") {
        out.push(
            "Chrome: sign into Chrome Sync — passwords/cookies do not travel by file copy (DPAPI)"
                .to_string(),
        );
    }
    out.push(
        "Telegram/Firefox are already restored from imported profiles — just launch them"
            .to_string(),
    );
    if reboot_required {
        out.push("REBOOT required before the NVIDIA driver is active".to_string());
    }
    out
}

fn now_string() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

fn assemble_report(
    home: &Path,
    dry_run: bool,
    steps: Vec<StepReport>,
    downloads: Vec<DownloadRecord>,
    followups: Vec<String>,
    reboot_required: bool,
) -> SetupReport {
    let count = |s: &str| steps.iter().filter(|x| x.status == s).count() as u64;
    let followups = assemble_followups(&steps, reboot_required, followups);
    SetupReport {
        schema_version: 1,
        created_at: now_string(),
        home: home.to_string_lossy().into_owned(),
        os: std::env::consts::OS.to_string(),
        dry_run,
        reboot_required,
        total_steps: steps.len() as u64,
        done: count("done"),
        skipped_already: count("skipped-already"),
        not_applicable: count("not-applicable"),
        failed: count("failed"),
        would_run: count("would-run"),
        steps,
        downloads,
        manual_followups: followups,
    }
}

fn write_report_files(dir: &Path, report: &SetupReport, summary: &str) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|e| format!("cannot serialize setup report: {}", e))?;
    let jp = dir.join("setup-report.json");
    let mut f =
        fs::File::create(&jp).map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    use std::io::Write;
    f.write_all(json.as_bytes())
        .and_then(|_| f.write_all(b"\n"))
        .map_err(|e| format!("cannot write {}: {}", jp.display(), e))?;
    let tp = dir.join("setup-report.txt");
    let mut f =
        fs::File::create(&tp).map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;
    f.write_all(summary.as_bytes())
        .map_err(|e| format!("cannot write {}: {}", tp.display(), e))?;
    Ok(())
}

pub fn build_setup_report_txt(r: &SetupReport) -> String {
    let mut s = String::new();
    s.push_str("OSmosis setup report\n");
    s.push_str("====================\n");
    s.push_str(&format!("home: {}\n", r.home));
    s.push_str(&format!("os: {}\n", r.os));
    s.push_str(&format!("created_at: {}\n", r.created_at));
    s.push_str(&format!("dry_run: {}\n", r.dry_run));
    s.push_str(&format!("reboot_required: {}\n\n", r.reboot_required));

    s.push_str("Totals\n");
    s.push_str("------\n");
    s.push_str(&format!("total_steps:      {}\n", r.total_steps));
    s.push_str(&format!("done:             {}\n", r.done));
    s.push_str(&format!("skipped_already:  {}\n", r.skipped_already));
    s.push_str(&format!("not_applicable:   {}\n", r.not_applicable));
    s.push_str(&format!("failed:           {}\n", r.failed));
    if r.dry_run {
        s.push_str(&format!("would_run:        {}\n", r.would_run));
    }
    s.push('\n');

    s.push_str("STEPS\n");
    s.push_str("-----\n");
    for st in &r.steps {
        s.push_str(&format!(
            "[{}] {} — {}: {}\n",
            st.status, st.id, st.title, st.detail
        ));
    }
    s.push('\n');

    s.push_str("DOWNLOADS\n");
    s.push_str("---------\n");
    if r.downloads.is_empty() {
        s.push_str("(none)\n");
    }
    for d in &r.downloads {
        s.push_str(&format!(
            "- {} sha256={} (step {})\n",
            d.url, d.sha256, d.step
        ));
    }
    s.push('\n');

    s.push_str("MANUAL FOLLOW-UPS\n");
    s.push_str("-----------------\n");
    if r.manual_followups.is_empty() {
        s.push_str("(none)\n");
    }
    for f in &r.manual_followups {
        s.push_str(&format!("- {}\n", f));
    }
    s
}

/// Engine shared by the real command and tests. No OS gate here so the step
/// logic stays testable on Windows; `run_setup` adds the Linux-only refusal.
pub fn run_setup_with(
    runner: &dyn Runner,
    home: &Path,
    dry_run: bool,
    work: &Path,
) -> Result<SetupOutcome, String> {
    if !home.is_dir() {
        return Err(format!(
            "target home is not a directory: {}",
            home.display()
        ));
    }
    let report_dir = home.join(SETUP_DIR_NAME);
    fs::create_dir_all(&report_dir).map_err(|e| {
        format!(
            "cannot create {} (is the home writable?): {}",
            report_dir.display(),
            e
        )
    })?;
    fs::create_dir_all(work)
        .map_err(|e| format!("cannot create work dir {}: {}", work.display(), e))?;

    let mut ctx = Ctx::new(home, work);
    let mut steps = Vec::new();
    for def in &STEP_DEFS {
        steps.push(execute_step(def, runner, &mut ctx, dry_run));
    }

    let report = assemble_report(
        home,
        dry_run,
        steps,
        ctx.downloads.clone(),
        ctx.followups.clone(),
        ctx.reboot_required,
    );
    let summary = build_setup_report_txt(&report);

    write_report_files(&report_dir, &report, &summary)?;
    let _ = fs::remove_dir_all(work);

    let exit_code = if report.failed > 0 { 2 } else { 0 };
    Ok(SetupOutcome {
        report,
        summary,
        exit_code,
    })
}

/// `osmosis setup [--home <dir>] [--dry-run]`: Linux-only. Refuses (exit 1) on
/// any other OS and when the home is not writable.
pub fn run_setup(home: &Path, dry_run: bool) -> Result<SetupOutcome, String> {
    if !cfg!(target_os = "linux") {
        return Err(format!(
            "osmosis setup is Linux-only; this host is {}",
            std::env::consts::OS
        ));
    }
    let runner = SystemRunner;
    let work = std::env::temp_dir().join(format!("osmosis-setup-{}", std::process::id()));
    run_setup_with(&runner, home, dry_run, &work)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommended_driver_is_parsed_from_devices_output() {
        let text = "\
== /sys/devices/pci0000:00/0000:01:00.0 ==
modalias : pci:v000010DEd00002484sv00001043sd000087D3bc03sc00i00
vendor   : NVIDIA Corporation
driver   : nvidia-driver-550 - third-party non-free recommended
driver   : nvidia-driver-535 - third-party non-free
";
        assert_eq!(
            parse_recommended_driver(text).as_deref(),
            Some("nvidia-driver-550")
        );
    }

    #[test]
    fn recommended_driver_absent_is_none() {
        assert!(parse_recommended_driver("no drivers here\n").is_none());
    }

    #[test]
    fn release_tag_names_are_sanitized() {
        assert_eq!(sanitize_name("GE-Proton8-26"), "GE-Proton8-26");
        assert_eq!(sanitize_name("v8/26 beta"), "v8-26-beta");
        assert!(!sanitize_name("../etc").contains('/'));
    }

    #[test]
    fn wine_ge_matcher_selects_the_real_release_asset_case_insensitively() {
        // the exact asset name from the live GE-Proton8-26 release
        assert!(is_wine_ge_tarball(
            "wine-lutris-GE-Proton8-26-x86_64.tar.xz"
        ));
        // decoys must not match
        assert!(!is_wine_ge_tarball(
            "wine-lutris-GE-Proton8-26-x86_64.tar.zst"
        ));
        assert!(!is_wine_ge_tarball("lutris-0.5.17.tar.xz"));
        assert!(!is_wine_ge_tarball("wine-ge-custom-8.tar.xz"));
        // a selection over a mixed asset list picks only the real one
        let assets = [
            "wine-lutris-GE-Proton8-26-x86_64.tar.zst",
            "some-other-tool.tar.xz",
            "wine-lutris-GE-Proton8-26-x86_64.tar.xz",
        ];
        let picked: Vec<&str> = assets
            .iter()
            .copied()
            .filter(|n| is_wine_ge_tarball(n))
            .collect();
        assert_eq!(picked, vec!["wine-lutris-GE-Proton8-26-x86_64.tar.xz"]);
    }
}
