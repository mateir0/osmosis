//! Integration tests for slice 8 (multi-distro setup/games).
//!
//! Self-contained fake machine: a `World` of present binaries/packages that
//! mutates the way real installs would, and a `Fake` runner recording every
//! argv. Each supported distro gets read-path + write-payload assertions, and
//! unsupported distros must be refused before a single command runs.

use osmosis::distro::{detect_from_path, parse_os_release, Distro};
use osmosis::games::{run_games_with, GamesOutcome};
use osmosis::setup::{
    run_setup_with, CommandOutput, Runner, SetupOutcome, SetupReport, StepReport,
};
use std::cell::RefCell;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

const WINE_GE_JSON: &str = r#"{"tag_name":"GE-Proton8-26","assets":[{"name":"wine-lutris-GE-Proton8-26-x86_64.tar.xz","browser_download_url":"https://example.invalid/wine-ge.tar.xz"}]}"#;
const OPENCODE_DEB_JSON: &str = r#"{"tag_name":"v0.5.0","assets":[{"name":"opencode-desktop_0.5.0_amd64.deb","browser_download_url":"https://example.invalid/opencode.deb"}]}"#;
const OPENCODE_APPIMAGE_JSON: &str = r#"{"tag_name":"v0.5.0","assets":[{"name":"opencode-desktop_0.5.0_amd64.AppImage","browser_download_url":"https://example.invalid/opencode.AppImage"}]}"#;

const APT: [&str; 4] = ["sudo", "env", "DEBIAN_FRONTEND=noninteractive", "apt-get"];
const DNF: [&str; 3] = ["sudo", "dnf", "-y"];
const PACMAN: [&str; 5] = ["sudo", "pacman", "-S", "--needed", "--noconfirm"];

// ------------------------------------------------------------- fake machine --

#[derive(Debug, Clone)]
struct World {
    /// binaries that resolve with `--version`
    bins: HashSet<&'static str>,
    /// packages recorded as installed by the distro's package database
    pkgs: HashSet<String>,
    nvidia_gpu: bool,
    fedora_ver: String,
    /// package names whose install command fails
    fail_pkgs: HashSet<String>,
    /// serve the AppImage-only OpenCode release (non-apt distros)
    opencode_appimage_only: bool,
}

impl World {
    fn fresh() -> World {
        World {
            bins: HashSet::new(),
            pkgs: HashSet::new(),
            nvidia_gpu: false,
            fedora_ver: "41".to_string(),
            fail_pkgs: HashSet::new(),
            opencode_appimage_only: false,
        }
    }

    fn with_nvidia_gpu() -> World {
        let mut w = World::fresh();
        w.nvidia_gpu = true;
        w
    }

    fn with_lutris() -> World {
        let mut w = World::fresh();
        w.bins.insert("lutris");
        w
    }
}

fn ver(w: &World, bin: &str, text: &str) -> CommandOutput {
    if w.bins.contains(bin) {
        CommandOutput::ok(text)
    } else {
        CommandOutput::fail(127, "command not found")
    }
}

fn eq(a: &[&str], b: &[&str]) -> bool {
    a == b
}

/// Which binary a successfully installed package provides (for verify probes).
fn pkg_to_bin(pkg: &str) -> Option<&'static str> {
    Some(match pkg {
        "git" => "git",
        "curl" => "curl",
        "build-essential" | "gcc" | "base-devel" => "gcc",
        "gh" | "github-cli" => "gh",
        "code" => "code",
        "lutris" => "lutris",
        "chromium" => "chromium",
        "nvidia" | "nvidia-utils" | "akmod-nvidia" | "nvidia-driver" => "nvidia-smi",
        "openjdk-21-jre" | "java-17-openjdk" | "jre17-openjdk" => "java",
        _ => return None,
    })
}

/// Which binary an installed .deb/.rpm file provides.
fn path_arg_to_bin(arg: &str) -> Option<&'static str> {
    if arg.ends_with("vscode.deb") || arg.ends_with("vscode.rpm") {
        Some("code")
    } else if arg.ends_with("google-chrome.deb") || arg.ends_with("google-chrome.rpm") {
        Some("google-chrome")
    } else if arg.ends_with("opencode.deb") {
        Some("opencode")
    } else {
        None
    }
}

/// If `a` is a package-install argv, return its package items.
fn install_items<'a>(a: &'a [&'a str]) -> Option<Vec<&'a str>> {
    if a.starts_with(&APT) && a.get(4) == Some(&"install") {
        return Some(a[6..].to_vec());
    }
    if a.starts_with(&DNF) && a.get(3) == Some(&"install") {
        return Some(a[4..].to_vec());
    }
    if a.starts_with(&PACMAN) {
        return Some(a[5..].to_vec());
    }
    None
}

fn respond(w: &mut World, a: &[&str]) -> CommandOutput {
    // ---- version probes -------------------------------------------------
    for b in [
        "git",
        "curl",
        "gcc",
        "gh",
        "code",
        "google-chrome",
        "chromium",
        "opencode",
        "lutris",
        "rustc",
        "cargo",
    ] {
        if eq(a, &[b, "--version"]) {
            return ver(w, b, "1.0.0");
        }
    }
    if eq(a, &["java", "-version"]) {
        return if w.bins.contains("java") {
            CommandOutput {
                status: 0,
                stdout: String::new(),
                stderr: "openjdk version \"21.0.1\" 2023-10-17".to_string(),
            }
        } else {
            CommandOutput::fail(127, "java: command not found")
        };
    }

    // ---- bash-driven installers -----------------------------------------
    if a.first() == Some(&"bash") {
        let rest = &a[1..];
        if rest.len() == 2 && rest[0] == "-lc" {
            let bin = rest[1].split_whitespace().next().unwrap_or("");
            return ver(w, bin, "9.9.9");
        }
        if rest.len() == 2 && rest[0] == "-c" {
            for b in ["node", "npm", "pnpm"] {
                w.bins.insert(b);
            }
            return CommandOutput::ok("");
        }
        let script = rest.first().copied().unwrap_or("");
        if script.contains("nvm") {
            for b in ["node", "npm", "pnpm"] {
                w.bins.insert(b);
            }
        } else if script.contains("rustup") {
            w.bins.insert("rustc");
            w.bins.insert("cargo");
        } else if script.contains("opencode") {
            w.bins.insert("opencode");
        }
        return CommandOutput::ok("");
    }

    // ---- hardware ---------------------------------------------------------
    if eq(a, &["lspci"]) {
        return if w.nvidia_gpu {
            CommandOutput::ok("01:00.0 VGA compatible controller: NVIDIA Corporation GA104")
        } else {
            CommandOutput::ok("00:02.0 VGA compatible controller: Intel UHD Graphics")
        };
    }
    if eq(a, &["nvidia-smi"]) {
        return ver(w, "nvidia-smi", "NVIDIA-SMI 550.54.14");
    }
    if eq(a, &["which", "ubuntu-drivers"]) {
        return CommandOutput::ok("/usr/bin/ubuntu-drivers\n");
    }
    if eq(a, &["sudo", "ubuntu-drivers", "autoinstall"]) {
        w.bins.insert("nvidia-smi");
        return CommandOutput::ok("");
    }
    if eq(a, &["ubuntu-drivers", "devices"]) {
        return CommandOutput::ok(
            "driver   : nvidia-driver-550 - third-party non-free recommended\n",
        );
    }
    if eq(a, &["rpm", "-E", "%fedora"]) {
        return CommandOutput::ok(format!("{}\n", w.fedora_ver));
    }

    // ---- package database checks ------------------------------------------
    if a.first() == Some(&"dpkg-query") {
        let pkg = a.last().copied().unwrap_or("");
        return if w.pkgs.contains(pkg) {
            CommandOutput::ok("install ok installed")
        } else {
            CommandOutput::fail(1, "dpkg-query: no packages found matching")
        };
    }
    if a.starts_with(&["rpm", "-q"]) {
        let pkg = a.get(2).copied().unwrap_or("");
        return if w.pkgs.contains(pkg) {
            CommandOutput::ok(format!("{}-1\n", pkg))
        } else {
            CommandOutput::fail(1, "not installed")
        };
    }
    if a.starts_with(&["pacman", "-Q"]) {
        let pkg = a.get(2).copied().unwrap_or("");
        return if w.pkgs.contains(pkg) {
            CommandOutput::ok(format!("{} 1.0-1\n", pkg))
        } else {
            CommandOutput::fail(1, "not found")
        };
    }

    // ---- installs -----------------------------------------------------------
    if a.starts_with(&APT) && a.get(4) == Some(&"update") {
        return CommandOutput::ok("");
    }
    if let Some(items) = install_items(a) {
        if items.iter().any(|p| w.fail_pkgs.contains(*p)) {
            return CommandOutput::fail(100, "E: Unable to locate package");
        }
        for it in &items {
            w.pkgs.insert(it.to_string());
            if let Some(b) = pkg_to_bin(it) {
                w.bins.insert(b);
            }
            if it.contains('/') {
                if let Some(b) = path_arg_to_bin(it) {
                    w.bins.insert(b);
                }
            }
        }
        return CommandOutput::ok("");
    }
    if a.first() == Some(&"sudo") && a.get(1) == Some(&"apt-get") {
        return CommandOutput::fail(127, format!("REGRESSION: bare sudo apt-get {:?}", a));
    }
    if a.first() == Some(&"sudo") && a.get(1) == Some(&"dpkg") {
        return CommandOutput::fail(127, format!("REGRESSION: bare sudo dpkg {:?}", a));
    }
    if a.starts_with(&["sudo", "install", "-m"]) {
        return CommandOutput::ok("");
    }

    // ---- downloads ----------------------------------------------------------
    if a.first() == Some(&"curl") {
        let url = a.last().copied().unwrap_or("");
        if let Some(i) = a.iter().position(|x| *x == "-o") {
            if let Some(dest) = a.get(i + 1) {
                let _ = fs::write(dest, b"fixture-download");
            }
        }
        if url.contains("anomalyco/opencode") {
            return CommandOutput::ok(if w.opencode_appimage_only {
                OPENCODE_APPIMAGE_JSON
            } else {
                OPENCODE_DEB_JSON
            });
        }
        if url.contains("GloriousEggroll/wine-ge-custom") {
            return CommandOutput::ok(WINE_GE_JSON);
        }
        return CommandOutput::ok("");
    }
    if a.first() == Some(&"sha256sum") {
        return CommandOutput::ok(format!("{}  {}\n", "a".repeat(64), a.get(1).unwrap_or(&"")));
    }
    if a.first() == Some(&"tar") {
        // materialize bin/wine in the -C dest so the Lutris verify passes
        if let Some(i) = a.iter().position(|x| *x == "-C") {
            if let Some(d) = a.get(i + 1) {
                let bin = Path::new(d).join("bin");
                let _ = fs::create_dir_all(&bin);
                let _ = fs::write(bin.join("wine"), b"wine");
            }
        }
        return CommandOutput::ok("");
    }

    CommandOutput::fail(127, format!("fake runner: unhandled argv {:?}", a))
}

struct Fake {
    world: RefCell<World>,
    calls: RefCell<Vec<Vec<String>>>,
}

impl Fake {
    fn new(world: World) -> Fake {
        Fake {
            world: RefCell::new(world),
            calls: RefCell::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<Vec<String>> {
        self.calls.borrow().clone()
    }

    /// exact argv match
    fn ran(&self, argv: &[&str]) -> bool {
        self.calls
            .borrow()
            .iter()
            .any(|c| c.len() == argv.len() && c.iter().zip(argv).all(|(a, b)| a == b))
    }

    /// prefix match
    fn ran_any(&self, prefix: &[&str]) -> bool {
        self.calls
            .borrow()
            .iter()
            .any(|c| c.len() >= prefix.len() && c.iter().zip(prefix).all(|(a, b)| a == b))
    }

    /// any recorded argv containing `frag` as a substring of any element
    fn has(&self, frag: &str) -> bool {
        self.calls()
            .iter()
            .any(|c| c.iter().any(|x| x.contains(frag)))
    }

    /// index of the first argv containing `frag`
    fn index_of(&self, frag: &str) -> Option<usize> {
        self.calls()
            .iter()
            .position(|c| c.iter().any(|x| x.contains(frag)))
    }
}

impl Runner for Fake {
    fn output(&self, argv: &[&str]) -> Result<CommandOutput, String> {
        self.calls
            .borrow_mut()
            .push(argv.iter().map(|s| s.to_string()).collect());
        let mut w = self.world.borrow_mut();
        Ok(respond(&mut w, argv))
    }
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// temp HOME + games root + work dir
struct Fx {
    dir: PathBuf,
}

impl Fx {
    fn new() -> Fx {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "osmosis-distro-fixture-{}-{}",
            std::process::id(),
            n
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("home")).unwrap();
        fs::create_dir_all(dir.join("drive")).unwrap();
        fs::create_dir_all(dir.join("work")).unwrap();
        Fx { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    fn drive(&self) -> PathBuf {
        self.dir.join("drive")
    }

    fn work(&self) -> PathBuf {
        self.dir.join("work")
    }

    fn touch_home(&self, rel: &str) {
        let p = self.home().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn setup(fx: &Fx, w: World, d: Distro) -> (Fake, SetupOutcome) {
    let fake = Fake::new(w);
    let out = run_setup_with(&fake, &fx.home(), false, &fx.work(), d)
        .expect("setup runs in the fake world");
    (fake, out)
}

fn games(fx: &Fx, w: World, d: Distro) -> (Fake, GamesOutcome) {
    fx.touch_home(".local/share/lutris/runners/wine/GE-Proton8-26/bin/wine");
    let fake = Fake::new(w);
    let out = run_games_with(&fake, &fx.home(), &fx.drive(), false, &fx.work(), d)
        .expect("games runs in the fake world");
    (fake, out)
}

fn step<'a>(r: &'a SetupReport, id: &str) -> &'a StepReport {
    r.steps.iter().find(|s| s.id == id).unwrap_or_else(|| {
        panic!(
            "step {} missing: {:?}",
            id,
            r.steps.iter().map(|s| &s.id).collect::<Vec<_>>()
        )
    })
}

const APT_INSTALL: [&str; 6] = [
    "sudo",
    "env",
    "DEBIAN_FRONTEND=noninteractive",
    "apt-get",
    "install",
    "-y",
];

fn apt_install<'a>(pkgs: &'a [&'a str]) -> Vec<&'a str> {
    let mut v = APT_INSTALL.to_vec();
    v.extend_from_slice(pkgs);
    v
}

// ---------------------------------------------------------------- detection --

#[test]
fn os_release_parsing_maps_ids_and_id_like() {
    assert_eq!(parse_os_release("ID=linuxmint\n"), Distro::Mint);
    assert_eq!(parse_os_release("ID=ubuntu\n"), Distro::Ubuntu);
    assert_eq!(parse_os_release("ID=debian\n"), Distro::Debian);
    assert_eq!(parse_os_release("ID=fedora\n"), Distro::Fedora);
    assert_eq!(parse_os_release("ID=arch\n"), Distro::Arch);
    assert_eq!(
        parse_os_release("ID=\"pop\"\nID_LIKE=\"ubuntu debian\"\n"),
        Distro::Ubuntu
    );
    assert_eq!(parse_os_release("ID=manjaro\nID_LIKE=arch\n"), Distro::Arch);
    assert_eq!(
        parse_os_release("ID=opensuse-tumbleweed\n"),
        Distro::Unsupported("opensuse-tumbleweed".to_string())
    );
    assert_eq!(
        parse_os_release("NAME=NoId\n"),
        Distro::Unsupported("unknown".to_string())
    );

    let fx = Fx::new();
    let p = fx.dir.join("os-release");
    fs::write(&p, "ID=fedora\nPRETTY_NAME=\"Fedora Linux 41\"\n").unwrap();
    assert_eq!(detect_from_path(&p).unwrap(), Distro::Fedora);
}

// ----------------------------------------------------------------- refusal --

#[test]
fn unsupported_distro_setup_refused_before_any_command() {
    let fx = Fx::new();
    let fake = Fake::new(World::fresh());
    let err = run_setup_with(
        &fake,
        &fx.home(),
        false,
        &fx.work(),
        Distro::Unsupported("gentoo".to_string()),
    )
    .expect_err("unsupported distro must be refused");
    assert!(err.contains("gentoo"), "{}", err);
    assert!(
        err.contains("Linux Mint, Ubuntu, Debian, Fedora, Arch Linux"),
        "{}",
        err
    );
    assert!(err.contains("refusing"), "{}", err);
    assert!(
        fake.calls().is_empty(),
        "zero runner commands, got {:?}",
        fake.calls()
    );
}

#[test]
fn unsupported_distro_games_refused_before_any_command() {
    let fx = Fx::new();
    fx.touch_home(".local/share/lutris/runners/wine/GE-Proton8-26/bin/wine");
    let fake = Fake::new(World::with_lutris());
    let err = run_games_with(
        &fake,
        &fx.home(),
        &fx.drive(),
        false,
        &fx.work(),
        Distro::Unsupported("gentoo".to_string()),
    )
    .expect_err("unsupported distro must be refused");
    assert!(
        err.contains("gentoo") && err.contains("refusing"),
        "{}",
        err
    );
    assert!(
        fake.calls().is_empty(),
        "zero runner commands, got {:?}",
        fake.calls()
    );
}

// ------------------------------------------------------------- apt-family ---

#[test]
fn mint_base_argv_is_byte_identical_to_pre_slice_and_report_carries_distro() {
    let fx = Fx::new();
    let (fake, out) = setup(&fx, World::fresh(), Distro::Mint);
    let r = &out.report;
    assert_eq!(r.os, "linuxmint");
    assert!(fake.ran(&apt_install(&[
        "git",
        "curl",
        "build-essential",
        "ntfs-3g",
        "software-properties-common",
        "fonts-noto",
        "fonts-liberation",
    ])));
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "update",
    ]));
    assert!(fake.ran(&apt_install(&["mint-meta-codecs"])));
    // detect-phase dpkg-query probes are read-only; no install of the fallback
    assert!(
        !fake.ran(&apt_install(&["ubuntu-restricted-extras"])),
        "no fallback install on Mint success"
    );
    assert!(!fake.ran_any(&["sudo", "dnf"]), "no dnf on Mint");
    assert!(!fake.ran_any(&["sudo", "pacman"]), "no pacman on Mint");
    assert!(!fake.ran_any(&["rpm"]), "no rpm on Mint");
    assert!(!fake.ran_any(&["pacman", "-Q"]), "no pacman -Q on Mint");
    assert_eq!(step(r, "base").status, "done");
    assert!(
        step(r, "base")
            .detail
            .starts_with("base packages installed"),
        "detail was: {}",
        step(r, "base").detail
    );
}

#[test]
fn mint_falls_back_to_ubuntu_restricted_extras_when_mint_meta_codecs_fails() {
    let fx = Fx::new();
    let mut w = World::fresh();
    w.fail_pkgs.insert("mint-meta-codecs".to_string());
    let (fake, out) = setup(&fx, w, Distro::Mint);
    assert!(fake.ran(&apt_install(&["mint-meta-codecs"])));
    assert!(fake.ran(&apt_install(&["ubuntu-restricted-extras"])));
    assert_eq!(step(&out.report, "base").status, "done");
}

#[test]
fn ubuntu_uses_restricted_extras_never_mint_codecs() {
    let fx = Fx::new();
    let (fake, out) = setup(&fx, World::fresh(), Distro::Ubuntu);
    assert_eq!(out.report.os, "ubuntu");
    assert!(fake.ran(&apt_install(&["ubuntu-restricted-extras"])));
    assert!(!fake.has("mint-meta-codecs"));
    assert!(fake.ran(&apt_install(&[
        "git",
        "curl",
        "build-essential",
        "ntfs-3g",
        "software-properties-common",
        "fonts-noto",
        "fonts-liberation",
    ])));
}

#[test]
fn debian_codecs_failure_carries_the_nonfree_hint() {
    let fx = Fx::new();
    let mut w = World::fresh();
    w.fail_pkgs.insert("libavcodec-extra".to_string());
    let (fake, out) = setup(&fx, w, Distro::Debian);
    assert_eq!(out.report.os, "debian");
    assert!(fake.ran(&apt_install(&["libavcodec-extra"])));
    let base = step(&out.report, "base");
    assert_eq!(base.status, "failed");
    assert!(
        base.detail.contains("non-free"),
        "detail was: {}",
        base.detail
    );
}

// ------------------------------------------------------------ rpm / pacman --

#[test]
fn fedora_enables_rpm_fusion_before_base_and_uses_dnf_only() {
    let fx = Fx::new();
    let mut w = World::fresh();
    w.opencode_appimage_only = true;
    let (fake, out) = setup(&fx, w, Distro::Fedora);
    let r = &out.report;
    assert_eq!(r.os, "fedora");

    assert!(
        fake.has("rpmfusion-free-release-41.noarch.rpm"),
        "calls: {:?}",
        fake.calls()
    );
    assert!(fake.has("rpmfusion-nonfree-release-41.noarch.rpm"));
    assert!(fake.ran(&[
        "sudo",
        "dnf",
        "-y",
        "install",
        "git",
        "curl",
        "gcc",
        "gcc-c++",
        "make",
        "ntfs-3g",
        "google-noto-sans-fonts",
        "liberation-fonts"
    ]));
    assert!(fake.ran(&[
        "sudo",
        "dnf",
        "-y",
        "install",
        "ffmpeg",
        "gstreamer1-plugins-base",
        "gstreamer1-plugins-good",
        "gstreamer1-plugins-bad-free",
        "gstreamer1-plugins-ugly"
    ]));
    assert!(fake.ran(&["sudo", "dnf", "-y", "install", "gh"]));
    assert!(fake.has("linux-rpm-x64"), "vscode rpm url");
    assert!(
        fake.has("google-chrome-stable_current_x86_64.rpm"),
        "chrome rpm url"
    );
    assert!(fake.has("opencode.AppImage"), "AppImage path on non-apt");
    assert!(!fake.has("opencode.deb"), "no .deb install on Fedora");

    // fusion strictly before the base install
    let fusion = fake.index_of("rpmfusion-free-release").unwrap();
    let base = fake.index_of("gcc-c++").unwrap();
    assert!(
        fusion < base,
        "fusion {} must precede base {}",
        fusion,
        base
    );

    // absolutely no apt/dpkg/pacman on Fedora
    assert!(!fake.ran_any(&["sudo", "env"]));
    assert!(!fake.ran_any(&["sudo", "apt-get"]));
    assert!(!fake.ran_any(&["dpkg-query"]));
    assert!(!fake.ran_any(&["sudo", "pacman"]));
    assert!(!fake.ran_any(&["pacman", "-Q"]));

    assert_eq!(step(r, "base").status, "done");
    assert_eq!(step(r, "gh").status, "done");
    assert_eq!(step(r, "vscode").status, "done");
    assert_eq!(step(r, "chrome").status, "done");
}

#[test]
fn arch_uses_pacman_chromium_repo_and_aur_followup() {
    let fx = Fx::new();
    let (fake, out) = setup(&fx, World::fresh(), Distro::Arch);
    let r = &out.report;
    assert_eq!(r.os, "arch");

    assert!(fake.ran(&[
        "sudo",
        "pacman",
        "-S",
        "--needed",
        "--noconfirm",
        "git",
        "curl",
        "base-devel",
        "ntfs-3g",
        "noto-fonts",
        "ttf-liberation"
    ]));
    assert!(fake.ran(&[
        "sudo",
        "pacman",
        "-S",
        "--needed",
        "--noconfirm",
        "ffmpeg",
        "gst-plugins-base",
        "gst-plugins-good",
        "gst-plugins-bad",
        "gst-plugins-ugly",
        "libdvdcss"
    ]));
    assert!(fake.ran(&[
        "sudo",
        "pacman",
        "-S",
        "--needed",
        "--noconfirm",
        "github-cli"
    ]));
    assert!(fake.ran(&["sudo", "pacman", "-S", "--needed", "--noconfirm", "code"]));
    assert!(fake.ran(&[
        "sudo",
        "pacman",
        "-S",
        "--needed",
        "--noconfirm",
        "chromium"
    ]));
    assert!(fake.ran(&[
        "sudo",
        "pacman",
        "-S",
        "--needed",
        "--noconfirm",
        "lutris",
        "winetricks"
    ]));
    assert!(
        fake.ran(&["chromium", "--version"]),
        "chrome verify probes chromium"
    );
    assert!(!fake.ran_any(&["google-chrome", "--version"]));

    assert!(!fake.ran_any(&["sudo", "env"]));
    assert!(!fake.ran_any(&["sudo", "apt-get"]));
    assert!(!fake.ran_any(&["dpkg-query"]));
    assert!(!fake.ran_any(&["sudo", "dnf"]));
    assert!(!fake.ran_any(&["rpm"]));

    let chrome = step(r, "chrome");
    assert_eq!(chrome.status, "done");
    assert!(
        chrome.detail.contains("AUR"),
        "detail was: {}",
        chrome.detail
    );
    assert!(
        r.manual_followups
            .iter()
            .any(|f| f.contains("google-chrome")),
        "followups: {:?}",
        r.manual_followups
    );
    assert_eq!(step(r, "base").status, "done");
    assert_eq!(step(r, "gh").status, "done");
    assert_eq!(step(r, "vscode").status, "done");
    assert_eq!(step(r, "lutris").status, "done");
}

// ----------------------------------------------------------------- nvidia ---

#[test]
fn nvidia_mint_and_ubuntu_use_ubuntu_drivers_autoinstall() {
    for d in [Distro::Mint, Distro::Ubuntu] {
        let fx = Fx::new();
        let (fake, out) = setup(&fx, World::with_nvidia_gpu(), d.clone());
        assert!(
            fake.ran(&["sudo", "ubuntu-drivers", "autoinstall"]),
            "{:?} calls {:?}",
            d,
            fake.calls()
        );
        assert!(out.report.reboot_required, "reboot flag on {:?}", d);
        assert_eq!(step(&out.report, "nvidia").status, "done");
    }
}

#[test]
fn nvidia_debian_installs_nvidia_driver_package() {
    let fx = Fx::new();
    let (fake, out) = setup(&fx, World::with_nvidia_gpu(), Distro::Debian);
    assert!(fake.ran(&apt_install(&["nvidia-driver"])));
    assert!(out.report.reboot_required);
    assert_eq!(step(&out.report, "nvidia").status, "done");
}

#[test]
fn nvidia_debian_failure_carries_nonfree_hint() {
    let fx = Fx::new();
    let mut w = World::with_nvidia_gpu();
    w.fail_pkgs.insert("nvidia-driver".to_string());
    let (_, out) = setup(&fx, w, Distro::Debian);
    let n = step(&out.report, "nvidia");
    assert_eq!(n.status, "failed");
    assert!(n.detail.contains("non-free"), "detail was: {}", n.detail);
}

#[test]
fn nvidia_fedora_installs_akmod_and_marks_reboot() {
    let fx = Fx::new();
    let (fake, out) = setup(&fx, World::with_nvidia_gpu(), Distro::Fedora);
    assert!(fake.ran(&["sudo", "dnf", "-y", "install", "akmod-nvidia"]));
    assert!(out.report.reboot_required);
    let n = step(&out.report, "nvidia");
    assert_eq!(n.status, "done");
    assert!(
        n.detail.to_lowercase().contains("reboot"),
        "detail was: {}",
        n.detail
    );
}

#[test]
fn nvidia_arch_installs_nvidia_and_nvidia_utils_with_lts_followup() {
    let fx = Fx::new();
    let (fake, out) = setup(&fx, World::with_nvidia_gpu(), Distro::Arch);
    assert!(fake.ran(&[
        "sudo",
        "pacman",
        "-S",
        "--needed",
        "--noconfirm",
        "nvidia",
        "nvidia-utils"
    ]));
    assert_eq!(step(&out.report, "nvidia").status, "done");
    assert!(
        out.report
            .manual_followups
            .iter()
            .any(|f| f.contains("nvidia-lts")),
        "followups: {:?}",
        out.report.manual_followups
    );
}

// ------------------------------------------------------------------ games ---

#[test]
fn games_java_package_and_argv_per_distro() {
    // Fedora: dnf java-17-openjdk
    let fx = Fx::new();
    let (fake, out) = games(&fx, World::with_lutris(), Distro::Fedora);
    assert!(
        fake.ran(&["sudo", "dnf", "-y", "install", "java-17-openjdk"]),
        "calls: {:?}",
        fake.calls()
    );
    assert!(!fake.ran_any(&["sudo", "env"]));
    assert_eq!(out.report.os, "fedora");
    let mc = out
        .report
        .games
        .iter()
        .find(|g| g.slug == "minecraft")
        .unwrap();
    assert_eq!(mc.status, "ready", "detail: {}", mc.detail);

    // Arch: pacman jre17-openjdk
    let fx = Fx::new();
    let (fake, out) = games(&fx, World::with_lutris(), Distro::Arch);
    assert!(fake.ran(&[
        "sudo",
        "pacman",
        "-S",
        "--needed",
        "--noconfirm",
        "jre17-openjdk"
    ]));
    assert_eq!(out.report.os, "arch");
    let mc = out
        .report
        .games
        .iter()
        .find(|g| g.slug == "minecraft")
        .unwrap();
    assert_eq!(mc.status, "ready", "detail: {}", mc.detail);

    // Debian: apt openjdk-21-jre
    let fx = Fx::new();
    let (fake, out) = games(&fx, World::with_lutris(), Distro::Debian);
    assert!(fake.ran(&apt_install(&["openjdk-21-jre"])));
    assert_eq!(out.report.os, "debian");
    let mc = out
        .report
        .games
        .iter()
        .find(|g| g.slug == "minecraft")
        .unwrap();
    assert_eq!(mc.status, "ready", "detail: {}", mc.detail);
}

#[test]
fn games_on_unsupported_distro_never_reaches_the_package_manager() {
    let fx = Fx::new();
    fx.touch_home(".local/share/lutris/runners/wine/GE-Proton8-26/bin/wine");
    let fake = Fake::new(World::with_lutris());
    let err = run_games_with(
        &fake,
        &fx.home(),
        &fx.drive(),
        false,
        &fx.work(),
        Distro::Unsupported("opensuse".to_string()),
    )
    .expect_err("refused");
    assert!(
        err.contains("opensuse") && err.contains("refusing"),
        "{}",
        err
    );
    assert!(fake.calls().is_empty());
}
