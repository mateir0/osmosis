//! Integration tests for slice 4 (`osmosis setup`).
//!
//! A fake `Runner` returns canned command output and records every argv, so the
//! whole step engine (detect -> decide -> run -> verify) runs on Windows
//! without touching the real system. Only fixture temp dirs are written.

use osmosis::distro::Distro;
use osmosis::setup::{
    run_setup_with, CommandOutput, Runner, SetupOutcome, SetupReport, StepReport,
};
use std::cell::RefCell;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const APT_BASE: [&str; 7] = [
    "git",
    "curl",
    "build-essential",
    "ntfs-3g",
    "software-properties-common",
    "fonts-noto",
    "fonts-liberation",
];

const NVM_URL: &str = "https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.1/install.sh";
const RUSTUP_URL: &str = "https://sh.rustup.rs";
const GH_KEYRING_URL: &str = "https://cli.github.com/packages/githubcli-archive-keyring.gpg";
const OPENCODE_API: &str = "https://api.github.com/repos/anomalyco/opencode/releases/latest";
const OPENCODE_RELEASES_URL: &str = "https://github.com/anomalyco/opencode/releases";
const LUTRIS_API: &str =
    "https://api.github.com/repos/GloriousEggroll/wine-ge-custom/releases/latest";
const WINE_GE_RELEASES_URL: &str = "https://github.com/GloriousEggroll/wine-ge-custom/releases";

const OPENCODE_DEB_JSON: &str = r#"{"tag_name":"v0.5.0","assets":[{"name":"opencode-desktop_0.5.0_amd64.deb","browser_download_url":"https://example.invalid/opencode.deb"}]}"#;
const OPENCODE_APPIMAGE_JSON: &str = r#"{"tag_name":"v0.5.0","assets":[{"name":"opencode-desktop_0.5.0_amd64.AppImage","browser_download_url":"https://example.invalid/opencode.AppImage"}]}"#;
const WINE_GE_JSON: &str = r#"{"tag_name":"GE-Proton8-26","assets":[{"name":"wine-lutris-GE-Proton8-26-x86_64.tar.xz","browser_download_url":"https://example.invalid/wine-ge.tar.xz"}]}"#;

/// The privileged apt argv prefix every state-changing apt call must use:
/// `sudo` strips the caller's env, so DEBIAN_FRONTEND goes through `env`.
const SUDO_APT: [&str; 5] = [
    "sudo",
    "env",
    "DEBIAN_FRONTEND=noninteractive",
    "apt-get",
    "install",
];

/// What the fake machine "has" before setup runs. `run` commands mutate it the
/// way a real install would, so detect/verify behave like a live system.
#[derive(Debug, Clone)]
struct World {
    git: bool,
    curl: bool,
    gcc: bool,
    fonts: bool,
    codecs: bool,
    gh: bool,
    node: bool,
    npm: bool,
    pnpm: bool,
    rustc: bool,
    cargo: bool,
    code: bool,
    chrome: bool,
    opencode: bool,
    lutris: bool,
    nvidia_gpu: bool,
    nvidia_smi: bool,
    ubuntu_drivers_present: bool,
    autoinstall_ok: bool,
    mint_codecs_ok: bool,
    apt_update_ok: bool,
    apt_install_ok: bool,
    opencode_api_ok: bool,
    opencode_deb: bool,
    opencode_appimage: bool,
    lutris_api_ok: bool,
}

impl World {
    fn nothing() -> World {
        World {
            git: false,
            curl: false,
            gcc: false,
            fonts: false,
            codecs: false,
            gh: false,
            node: false,
            npm: false,
            pnpm: false,
            rustc: false,
            cargo: false,
            code: false,
            chrome: false,
            opencode: false,
            lutris: false,
            nvidia_gpu: false,
            nvidia_smi: false,
            ubuntu_drivers_present: true,
            autoinstall_ok: true,
            mint_codecs_ok: true,
            apt_update_ok: true,
            apt_install_ok: true,
            opencode_api_ok: true,
            opencode_deb: true,
            opencode_appimage: false,
            lutris_api_ok: true,
        }
    }

    fn everything() -> World {
        World {
            git: true,
            curl: true,
            gcc: true,
            fonts: true,
            codecs: true,
            gh: true,
            node: true,
            npm: true,
            pnpm: true,
            rustc: true,
            cargo: true,
            code: true,
            chrome: true,
            opencode: true,
            lutris: true,
            nvidia_gpu: true,
            nvidia_smi: true,
            ..World::nothing()
        }
    }
}

fn ver(present: bool, text: &str) -> CommandOutput {
    if present {
        CommandOutput::ok(text)
    } else {
        CommandOutput::fail(127, "command not found")
    }
}

fn eq(a: &[&str], b: &[&str]) -> bool {
    a == b
}

fn install_arg(w: &mut World, arg: &str) {
    match arg {
        "git" => w.git = true,
        "curl" => w.curl = true,
        "build-essential" => w.gcc = true,
        "fonts-noto" => w.fonts = true,
        "mint-meta-codecs" | "ubuntu-restricted-extras" => w.codecs = true,
        "gh" => w.gh = true,
        "lutris" => w.lutris = true,
        "winetricks" => {}
        "nvidia-driver-550" => {}
        other if other.ends_with("vscode.deb") => w.code = true,
        other if other.ends_with("google-chrome.deb") => w.chrome = true,
        other if other.ends_with("opencode.deb") => w.opencode = true,
        _ => {}
    }
}

fn respond(w: &mut World, a: &[&str]) -> CommandOutput {
    // ---- version probes -------------------------------------------------
    if eq(a, &["git", "--version"]) {
        return ver(w.git, "git version 2.43.0");
    }
    if eq(a, &["curl", "--version"]) {
        return ver(w.curl, "curl 8.5.0");
    }
    if eq(a, &["gcc", "--version"]) {
        return ver(w.gcc, "gcc (Ubuntu 13.2.0-23ubuntu4) 13.2.0");
    }
    if eq(a, &["gh", "--version"]) {
        return ver(w.gh, "gh version 2.40.1 (2024-01-08)");
    }
    if eq(a, &["code", "--version"]) {
        return ver(w.code, "1.86.0");
    }
    if eq(a, &["google-chrome", "--version"]) {
        return ver(w.chrome, "Google Chrome 122.0.6261.94");
    }
    if eq(a, &["opencode", "--version"]) {
        return ver(w.opencode, "1.0.0");
    }
    if eq(a, &["lutris", "--version"]) {
        return ver(w.lutris, "lutris 0.5.17");
    }
    if eq(a, &["rustc", "--version"]) {
        return ver(w.rustc, "rustc 1.75.0 (82e1608df 2023-12-21)");
    }
    if eq(a, &["cargo", "--version"]) {
        return ver(w.cargo, "cargo 1.75.0 (1d8b05cdd 2023-11-20)");
    }

    // ---- apt ------------------------------------------------------------
    if a.starts_with(&["sudo", "env", "DEBIAN_FRONTEND=noninteractive", "apt-get"]) {
        if a.get(4) == Some(&"update") {
            return if w.apt_update_ok {
                CommandOutput::ok("")
            } else {
                CommandOutput::fail(100, "E: Could not get lock /var/lib/apt/lists/lock")
            };
        }
        if a.get(4) == Some(&"install") {
            if !w.apt_install_ok {
                return CommandOutput::fail(100, "E: Unable to install packages");
            }
            if a.contains(&"mint-meta-codecs") && !w.mint_codecs_ok {
                return CommandOutput::fail(100, "E: Unable to locate package mint-meta-codecs");
            }
            for arg in a.iter().skip(6) {
                install_arg(w, arg);
            }
            return CommandOutput::ok("");
        }
        return CommandOutput::fail(127, format!("fake runner: unhandled sudo apt argv {:?}", a));
    }
    // a bare `sudo apt-get ...` (no env passthrough) must never be produced
    if a.first() == Some(&"sudo") && (a.get(1) == Some(&"apt-get") || a.get(1) == Some(&"dpkg")) {
        return CommandOutput::fail(
            127,
            format!(
                "REGRESSION: bare sudo apt/dpkg without env passthrough: {:?}",
                a
            ),
        );
    }
    if a.starts_with(&["sudo", "install", "-m"]) {
        return CommandOutput::ok("");
    }
    if a.first() == Some(&"dpkg-query") {
        let pkg = a.last().copied().unwrap_or("");
        let installed = match pkg {
            "fonts-noto" => w.fonts,
            "mint-meta-codecs" | "ubuntu-restricted-extras" => w.codecs,
            _ => false,
        };
        return if installed {
            CommandOutput::ok("install ok installed")
        } else {
            CommandOutput::fail(1, "dpkg-query: no packages found matching")
        };
    }

    // ---- hardware -------------------------------------------------------
    if eq(a, &["which", "ubuntu-drivers"]) {
        return ver(w.ubuntu_drivers_present, "/usr/bin/ubuntu-drivers\n");
    }
    if eq(a, &["lspci"]) {
        return if w.nvidia_gpu {
            CommandOutput::ok(
                "01:00.0 VGA compatible controller: NVIDIA Corporation GA104 [GeForce RTX 3070]",
            )
        } else {
            CommandOutput::ok("00:02.0 VGA compatible controller: Intel Corporation UHD Graphics")
        };
    }
    if eq(a, &["nvidia-smi"]) {
        return if w.nvidia_smi {
            CommandOutput::ok("NVIDIA-SMI 550.54.14   Driver Version: 550.54.14")
        } else {
            CommandOutput::fail(9, "NVIDIA-SMI has failed because it couldn't communicate")
        };
    }
    if eq(a, &["sudo", "ubuntu-drivers", "autoinstall"]) {
        return if w.autoinstall_ok {
            CommandOutput::ok("")
        } else {
            CommandOutput::fail(1, "autoinstall unsupported")
        };
    }
    if eq(a, &["ubuntu-drivers", "devices"]) {
        return CommandOutput::ok(
            "driver   : nvidia-driver-550 - third-party non-free recommended\n",
        );
    }

    // ---- bash-driven installers ----------------------------------------
    if a.first() == Some(&"bash") {
        let rest = &a[1..];
        if rest.len() == 2 && rest[0] == "-lc" {
            return match rest[1] {
                "node --version" => ver(w.node, "v20.11.0"),
                "npm --version" => ver(w.npm, "10.2.4"),
                "pnpm --version" => ver(w.pnpm, "8.15.1"),
                "rustc --version" => ver(w.rustc, "rustc 1.75.0 (82e1608df 2023-12-21)"),
                "cargo --version" => ver(w.cargo, "cargo 1.75.0 (1d8b05cdd 2023-11-20)"),
                "opencode --version" => ver(w.opencode, "1.0.0"),
                other => CommandOutput::fail(127, format!("bash: {}: command not found", other)),
            };
        }
        if rest.len() == 2 && rest[0] == "-c" {
            // the fixed NVM_CMD string
            w.node = true;
            w.npm = true;
            w.pnpm = true;
            return CommandOutput::ok("");
        }
        let script = rest.first().copied().unwrap_or("");
        if script.contains("nvm") {
            w.node = true;
            w.npm = true;
            w.pnpm = true;
        } else if script.contains("rustup") {
            w.rustc = true;
            w.cargo = true;
        } else if script.contains("opencode") {
            w.opencode = true;
        }
        return CommandOutput::ok("");
    }

    // ---- downloads ------------------------------------------------------
    if a.first() == Some(&"curl") {
        let url = a.last().copied().unwrap_or("");
        if let Some(i) = a.iter().position(|x| *x == "-o") {
            if let Some(dest) = a.get(i + 1) {
                let _ = fs::write(dest, b"fixture-download");
            }
        }
        if url.contains("anomalyco/opencode") {
            if !w.opencode_api_ok {
                return CommandOutput::fail(22, "HTTP 403");
            }
            return CommandOutput::ok(if w.opencode_appimage && !w.opencode_deb {
                OPENCODE_APPIMAGE_JSON
            } else {
                OPENCODE_DEB_JSON
            });
        }
        if url.contains("GloriousEggroll/wine-ge-custom") {
            return if w.lutris_api_ok {
                CommandOutput::ok(WINE_GE_JSON)
            } else {
                CommandOutput::fail(22, "HTTP 403")
            };
        }
        return CommandOutput::ok("");
    }
    if a.first() == Some(&"sha256sum") {
        let path = a.get(1).copied().unwrap_or("");
        return CommandOutput::ok(format!("{}  {}\n", "a".repeat(64), path));
    }
    if a.first() == Some(&"tar") {
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

    fn ran(&self, argv: &[&str]) -> bool {
        self.calls
            .borrow()
            .iter()
            .any(|c| c.len() == argv.len() && c.iter().zip(argv).all(|(a, b)| a == b))
    }

    fn ran_any(&self, prefixes: &[&str]) -> bool {
        self.calls
            .borrow()
            .iter()
            .any(|c| c.len() >= prefixes.len() && c.iter().zip(prefixes).all(|(a, b)| a == b))
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

struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "osmosis-setup-fixture-{}-{}",
            std::process::id(),
            n
        ));
        fs::create_dir_all(dir.join("home")).unwrap();
        Fixture { dir }
    }

    fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    fn work(&self) -> PathBuf {
        self.dir.join("work")
    }

    fn touch(&self, rel: &str) {
        let p = self.home().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"fixture").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn run(fx: &Fixture, world: World, dry_run: bool) -> (Fake, SetupOutcome) {
    let fake = Fake::new(world);
    let out = run_setup_with(&fake, &fx.home(), dry_run, &fx.work(), Distro::Mint)
        .expect("setup must not hard-fail on a valid fixture");
    (fake, out)
}

fn step<'a>(report: &'a SetupReport, id: &str) -> &'a StepReport {
    report
        .steps
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("missing step {}", id))
}

fn all_present_fixture() -> Fixture {
    let fx = Fixture::new();
    fx.touch(".nvm/nvm.sh");
    fx.touch(".cargo/bin/rustc");
    fx.touch(".opencode/bin/opencode");
    fx.touch(".local/share/lutris/runners/wine/GE-Proton8-26/bin/wine");
    fx
}

// ---------------------------------------------------------------- 4.1 ----

#[test]
fn all_present_skips_every_step_and_counts_reconcile() {
    let fx = all_present_fixture();
    let (fake, out) = run(&fx, World::everything(), false);
    let r = &out.report;

    assert_eq!(r.total_steps, 9);
    assert_eq!(r.steps.len(), 9);
    assert_eq!(r.skipped_already, 9);
    assert_eq!(r.done, 0);
    assert_eq!(r.failed, 0);
    assert_eq!(r.not_applicable, 0);
    assert_eq!(r.would_run, 0);
    assert_eq!(
        r.done + r.skipped_already + r.not_applicable + r.failed + r.would_run,
        r.total_steps
    );
    assert_eq!(out.exit_code, 0);
    assert!(!r.reboot_required);
    // nothing was installed: no privileged command ran
    assert!(!fake.ran_any(&["sudo"]));
    // report files exist and the work dir was cleaned up
    assert!(fx.home().join("osmosis-setup/setup-report.json").is_file());
    assert!(fx.home().join("osmosis-setup/setup-report.txt").is_file());
    assert!(!fx.work().exists());
}

#[test]
fn dry_run_plans_without_running_installers() {
    let fx = Fixture::new();
    let (fake, out) = run(&fx, World::nothing(), true);
    let r = &out.report;

    assert!(r.dry_run);
    assert_eq!(r.total_steps, 9);
    assert_eq!(r.would_run, 8);
    assert_eq!(r.not_applicable, 1); // no NVIDIA GPU
    assert_eq!(r.done, 0);
    assert_eq!(r.failed, 0);
    assert_eq!(out.exit_code, 0);
    assert_eq!(step(r, "nvidia").status, "not-applicable");

    // zero state-changing commands: only detect queries
    assert!(!fake.ran_any(&["sudo"]));
    // the only curl call is the read-only `curl --version` probe
    assert!(!fake.calls().iter().any(|c| {
        c.first().map(String::as_str) == Some("curl")
            && c.get(1).map(String::as_str) != Some("--version")
    }));
    assert!(!fake.ran_any(&["tar"]));
    // the only bash call is the read-only `bash -lc node --version` probe
    assert!(!fake.calls().iter().any(|c| {
        c.first().map(String::as_str) == Some("bash") && c.get(1).map(String::as_str) != Some("-lc")
    }));
    assert!(fake.ran(&["git", "--version"]));

    // the plan is still recorded
    assert!(fx.home().join("osmosis-setup/setup-report.json").is_file());
    assert!(!fx.work().exists());
}

#[test]
fn one_failed_step_still_runs_later_steps_and_exits_2() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.git = false;
    w.apt_update_ok = false;

    let (fake, out) = run(&fx, w, false);
    let r = &out.report;

    assert_eq!(step(r, "base").status, "failed");
    assert!(step(r, "base").detail.contains("apt-get update"));
    assert_eq!(r.failed, 1);
    assert_eq!(out.exit_code, 2);
    // later steps were still evaluated
    assert_eq!(r.steps.len(), 9);
    assert_eq!(step(r, "lutris").status, "skipped-already");
    assert!(fake.ran(&["lutris", "--version"]));
}

#[test]
fn missing_home_is_a_hard_refusal() {
    let fx = Fixture::new();
    let fake = Fake::new(World::nothing());
    let err = run_setup_with(
        &fake,
        &fx.dir.join("does-not-exist"),
        false,
        &fx.work(),
        Distro::Mint,
    )
    .expect_err("a missing home must be refused");
    assert!(err.contains("not a directory"), "{}", err);
}

#[cfg(not(target_os = "linux"))]
#[test]
fn setup_refuses_on_non_linux() {
    let fx = Fixture::new();
    let err = osmosis::setup::run_setup(&fx.home(), true)
        .expect_err("setup must refuse on non-Linux hosts");
    assert!(err.contains("Linux-only"), "{}", err);
}

// ---------------------------------------------------------------- 4.2 ----

#[test]
fn base_step_runs_the_exact_apt_argv_when_git_is_missing() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.git = false;

    let (fake, out) = run(&fx, w, false);

    assert_eq!(step(&out.report, "base").status, "done");
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "update"
    ]));
    let mut expected: Vec<&str> = SUDO_APT.to_vec();
    expected.push("-y");
    expected.extend_from_slice(&APT_BASE);
    assert!(fake.ran(&expected), "expected {:?}", expected);
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y",
        "mint-meta-codecs"
    ]));
    assert_eq!(out.exit_code, 0);
}

#[test]
fn base_step_falls_back_to_ubuntu_restricted_extras() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.codecs = false;
    w.mint_codecs_ok = false;

    let (fake, out) = run(&fx, w, false);

    assert_eq!(step(&out.report, "base").status, "done");
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y",
        "mint-meta-codecs"
    ]));
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y",
        "ubuntu-restricted-extras"
    ]));
}

// ---------------------------------------------------------------- 4.3 ----

#[test]
fn nvidia_not_applicable_without_a_gpu() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.nvidia_gpu = false;

    let (fake, out) = run(&fx, w, false);

    assert_eq!(step(&out.report, "nvidia").status, "not-applicable");
    assert!(!fake.ran(&["nvidia-smi"]));
    assert!(!fake.ran_any(&["sudo", "ubuntu-drivers"]));
    assert!(!out.report.reboot_required);
}

#[test]
fn nvidia_install_sets_reboot_required() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.nvidia_smi = false;

    let (fake, out) = run(&fx, w, false);
    let r = &out.report;

    assert_eq!(step(r, "nvidia").status, "done");
    assert!(fake.ran(&["sudo", "ubuntu-drivers", "autoinstall"]));
    assert!(r.reboot_required);
    assert!(r
        .manual_followups
        .iter()
        .any(|f| f.contains("REBOOT required before the NVIDIA driver is active")));
}

#[test]
fn nvidia_falls_back_to_recommended_driver_package() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.nvidia_smi = false;
    w.autoinstall_ok = false;

    let (fake, out) = run(&fx, w, false);

    assert_eq!(step(&out.report, "nvidia").status, "done");
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y",
        "nvidia-driver-550"
    ]));
    assert!(out.report.reboot_required);
}

// ---------------------------------------------------------------- 4.4 ----

#[test]
fn gh_installs_from_the_official_apt_repo() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.gh = false;

    let (fake, out) = run(&fx, w, false);
    let r = &out.report;

    assert_eq!(step(r, "gh").status, "done");
    let keyring = fx
        .work()
        .join("githubcli-archive-keyring.gpg")
        .to_string_lossy()
        .into_owned();
    assert!(fake.ran(&[
        "curl",
        "-fsSL",
        "--retry",
        "3",
        "-o",
        keyring.as_str(),
        GH_KEYRING_URL
    ]));
    assert!(fake.ran_any(&["sudo", "install", "-m", "0644",]));
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "update"
    ]));
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y",
        "gh"
    ]));

    let dl = r
        .downloads
        .iter()
        .find(|d| d.url == GH_KEYRING_URL)
        .expect("keyring download must be recorded");
    assert_eq!(dl.sha256.len(), 64);
    assert!(r.manual_followups.iter().any(|f| f == "run: gh auth login"));
}

// ---------------------------------------------------------------- 4.5 ----

#[test]
fn node_skips_when_nvm_and_node_are_present() {
    let fx = all_present_fixture();
    let (fake, out) = run(&fx, World::everything(), false);

    let s = step(&out.report, "node");
    assert_eq!(s.status, "skipped-already");
    assert!(s.detail.contains("v20.11.0"), "{}", s.detail);
    assert!(!fake.ran_any(&["bash", "-c"]));
}

#[test]
fn node_install_records_the_nvm_download() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.node = false;

    let (_, out) = run(&fx, w, false);
    let r = &out.report;

    assert_eq!(step(r, "node").status, "done");
    let dl = r
        .downloads
        .iter()
        .find(|d| d.url == NVM_URL)
        .expect("nvm download must be recorded");
    assert_eq!(dl.sha256.len(), 64);
    assert!(step(r, "node").detail.contains("node v20.11.0"));
}

// ---------------------------------------------------------------- 4.6 ----

#[test]
fn rust_skips_when_rustup_is_present() {
    let fx = all_present_fixture();
    let (_, out) = run(&fx, World::everything(), false);

    let s = step(&out.report, "rust");
    assert_eq!(s.status, "skipped-already");
    assert!(s.detail.contains("rustc 1.75.0"), "{}", s.detail);
}

#[test]
fn rust_install_records_the_rustup_download() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.rustc = false;
    w.cargo = false;

    let (fake, out) = run(&fx, w, false);
    let r = &out.report;

    assert_eq!(step(r, "rust").status, "done");
    assert!(r
        .downloads
        .iter()
        .any(|d| d.url == RUSTUP_URL && d.sha256.len() == 64));
    let script = fx
        .work()
        .join("rustup-init.sh")
        .to_string_lossy()
        .into_owned();
    assert!(fake.ran_any(&[
        "bash",
        script.as_str(),
        "-y",
        "--default-toolchain",
        "stable"
    ]));
}

// ---------------------------------------------------------------- 4.7 ----

#[test]
fn vscode_and_chrome_skip_independently_when_versions_parse() {
    let fx = all_present_fixture();
    let (fake, out) = run(&fx, World::everything(), false);

    assert_eq!(step(&out.report, "vscode").status, "skipped-already");
    assert_eq!(step(&out.report, "chrome").status, "skipped-already");
    assert!(!fake.ran_any(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y"
    ]));

    let mut w = World::everything();
    w.code = false;
    let fx2 = all_present_fixture();
    let (_, out2) = run(&fx2, w, false);
    assert_eq!(step(&out2.report, "vscode").status, "done");
    assert_eq!(step(&out2.report, "chrome").status, "skipped-already");
    assert!(out2
        .report
        .manual_followups
        .iter()
        .any(|f| f.contains("Settings Sync")));
}

// ---------------------------------------------------------------- 4.8 ----

#[test]
fn opencode_desktop_api_failure_is_recorded_not_fatal() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.opencode = false;
    w.opencode_api_ok = false;

    let (fake, out) = run(&fx, w, false);
    let r = &out.report;
    let s = step(r, "opencode");

    assert_eq!(s.status, "done");
    assert!(s.detail.contains("desktop sub-step failed"), "{}", s.detail);
    assert!(r
        .manual_followups
        .iter()
        .any(|f| f.contains(OPENCODE_RELEASES_URL)));
    assert!(fake.ran(&[
        "curl",
        "-fsSL",
        "--retry",
        "3",
        "-H",
        "Accept: application/vnd.github+json",
        OPENCODE_API
    ]));
    assert!(r.downloads.iter().any(|d| d.url.contains("opencode")));
    assert_eq!(out.exit_code, 0);
}

#[test]
fn opencode_appimage_fallback_writes_a_desktop_entry() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.opencode = false;
    w.opencode_deb = false;
    w.opencode_appimage = true;

    let (_, out) = run(&fx, w, false);

    assert_eq!(step(&out.report, "opencode").status, "done");
    assert!(step(&out.report, "opencode")
        .detail
        .contains("desktop installed"));
    assert!(fx
        .home()
        .join(".local/share/applications/opencode.desktop")
        .is_file());
}

#[test]
fn lutris_api_failure_marks_the_step_failed_with_the_releases_url() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.lutris = false;
    w.lutris_api_ok = false;

    let (fake, out) = run(&fx, w, false);
    let r = &out.report;
    let s = step(r, "lutris");

    assert_eq!(s.status, "failed");
    assert!(s.detail.contains(WINE_GE_RELEASES_URL), "{}", s.detail);
    assert!(fake.ran(&[
        "curl",
        "-fsSL",
        "--retry",
        "3",
        "-H",
        "Accept: application/vnd.github+json",
        LUTRIS_API
    ]));
    assert_eq!(out.exit_code, 2);
}

#[test]
fn lutris_installs_and_extracts_wine_ge() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.lutris = false;

    let (fake, out) = run(&fx, w, false);
    let r = &out.report;

    assert_eq!(step(r, "lutris").status, "done");
    assert!(fake.ran(&[
        "sudo",
        "env",
        "DEBIAN_FRONTEND=noninteractive",
        "apt-get",
        "install",
        "-y",
        "lutris",
        "winetricks"
    ]));
    assert!(fake.ran_any(&["tar", "-xf"]));
    assert!(r
        .downloads
        .iter()
        .any(|d| d.url.contains("wine-ge") && d.sha256.len() == 64));
    assert_eq!(step(r, "lutris").detail, "lutris + winetricks installed; Wine-GE GE-Proton8-26 extracted; lutris 0.5.17; Wine-GE runner present");
}

// ---------------------------------------------------------------- 4.9 ----

#[test]
fn followups_match_the_steps_that_ran() {
    let fx = Fixture::new();
    let (_, out) = run(&fx, World::nothing(), false);
    let r = &out.report;
    let has = |needle: &str| r.manual_followups.iter().any(|f| f.contains(needle));

    // every tool was installed, so every sign-in follow-up applies
    assert!(has("run: gh auth login"));
    assert!(has("Settings Sync"));
    assert!(has("Chrome Sync"));
    assert!(has("Telegram/Firefox"));
    // no GPU in this world, so no reboot line
    assert!(!has("REBOOT required"));
}

// ------------------------------------------------- 6.1.1 / 6.1.5 regressions --

#[test]
fn every_sudo_apt_or_dpkg_argv_passes_debian_frontend_through_env() {
    // World::nothing() forces every applicable step to run its privileged
    // commands, so the recorded argv is the full set of sudo apt calls.
    let fx = Fixture::new();
    let (fake, _) = run(&fx, World::nothing(), false);

    let mut privileged_apt_calls = 0;
    for c in fake.calls() {
        if c.first().map(String::as_str) != Some("sudo") {
            continue;
        }
        match c.get(1).map(String::as_str) {
            // bare `sudo apt-get ...` / `sudo dpkg ...`: sudo strips the
            // caller's env, so apt never sees DEBIAN_FRONTEND -> regression
            Some("apt-get") | Some("dpkg") => {
                panic!("bare sudo {:?} without env passthrough: {:?}", c[1], c)
            }
            Some("env") => {
                assert_eq!(
                    c.get(2).map(String::as_str),
                    Some("DEBIAN_FRONTEND=noninteractive")
                );
                match c.get(3).map(String::as_str) {
                    Some("apt-get") | Some("dpkg") => privileged_apt_calls += 1,
                    other => panic!(
                        "sudo env used for a non-apt/dpkg program: {:?} ({:?})",
                        other, c
                    ),
                }
            }
            _ => {} // sudo install / sudo ubuntu-drivers are not apt/dpkg
        }
    }
    assert!(
        privileged_apt_calls > 0,
        "the run must have issued at least one privileged apt call"
    );
}

#[test]
fn rust_and_opencode_verify_run_through_a_login_shell() {
    let fx = all_present_fixture();
    let mut w = World::everything();
    w.rustc = false;
    w.cargo = false;
    w.opencode = false;

    let (fake, out) = run(&fx, w, false);
    let r = &out.report;

    assert_eq!(step(r, "rust").status, "done");
    assert_eq!(step(r, "opencode").status, "done");
    // verify must go through `bash -lc` so a stale PATH in the launching
    // terminal cannot produce a false "did not parse" failure
    assert!(fake.ran(&["bash", "-lc", "rustc --version"]));
    assert!(fake.ran(&["bash", "-lc", "cargo --version"]));
    assert!(fake.ran(&["bash", "-lc", "opencode --version"]));
    // node verify already used the login-shell form
    assert!(fake.ran(&["bash", "-lc", "node --version"]));
}
