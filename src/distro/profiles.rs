//! Per-distro profile table (data) plus the fixed-argv builders the setup and
//! games engines call. Every privileged command is still a fixed argv array
//! through the `Runner` abstraction — the profile only chooses WHICH fixed
//! array. Nothing here interpolates into a shell.

use super::Distro;
use crate::setup::{version_of, Runner};

// ------------------------------------------------------------- fixed argv --

/// `sudo env DEBIAN_FRONTEND=noninteractive apt-get` (must stay identical to
/// `crate::setup::SUDO_APT_PREFIX` + "apt-get" so Mint argv is unchanged).
pub const APT_PREFIX: &[&str] = &["sudo", "env", "DEBIAN_FRONTEND=noninteractive", "apt-get"];
pub const APT_VERB: &[&str] = &["install", "-y"];
pub const APT_UPDATE: &[&str] = &["update"];
pub const DNF_PREFIX: &[&str] = &["sudo", "dnf", "-y"];
pub const DNF_VERB: &[&str] = &["install"];
pub const PACMAN_PREFIX: &[&str] = &["sudo", "pacman", "-S", "--needed", "--noconfirm"];
pub const PACMAN_VERB: &[&str] = &[];

/// Fedora-only: RPM Fusion release RPM URL templates, `{ver}` is the output
/// of `rpm -E %fedora` with the trailing newline stripped.
pub const RPM_FUSION_FREE: &str =
    "https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-{ver}.noarch.rpm";
pub const RPM_FUSION_NONFREE: &str =
    "https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-{ver}.noarch.rpm";

/// Fedora-only download URLs for the RPM builds of VS Code and Chrome.
pub const VSCODE_RPM_URL: &str =
    "https://code.visualstudio.com/sha/download?build=stable&os=linux-rpm-x64";
pub const CHROME_RPM_URL: &str =
    "https://dl.google.com/linux/direct/google-chrome-stable_current_x86_64.rpm";

/// Debian codec failure hint: enabling non-free components is a manual
/// sources.list edit — OSmosis never edits apt sources itself.
pub const DEBIAN_NONFREE_HINT: &str = "enable the contrib, non-free and non-free-firmware components in /etc/apt/sources.list (add 'contrib non-free non-free-firmware' to the component list of your Debian entries), run the apt update, and re-run this step — OSmosis never edits apt sources itself";

// ------------------------------------------------------------------ plans --

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Apt,
    Dnf,
    Pacman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NvidiaPlan {
    /// `ubuntu-drivers autoinstall`, falling back to the recommended
    /// nvidia-driver-* via apt (Mint/Ubuntu, unchanged from pre-slice).
    UbuntuDrivers,
    /// `apt install <package>`; on failure and when `nonfree_hint` is set,
    /// return the manual non-free hint instead of the raw error (Debian).
    AptPackage {
        package: &'static str,
        nonfree_hint: bool,
    },
    /// `dnf install akmod-nvidia`; kernel module builds at first boot, so the
    /// report flags a reboot (never executed) (Fedora).
    Akmod,
    /// `pacman -S nvidia nvidia-utils` plus a linux-lts follow-up (Arch).
    Pacman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhPlan {
    /// Official cli.github.com apt repo (Mint/Ubuntu/Debian).
    AptRepo,
    /// `dnf install gh` (Fedora).
    Dnf,
    /// `pacman -S github-cli` (Arch).
    Pacman,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VsCodePlan {
    Deb,
    Rpm,
    /// `pacman -S code` — the open-source build from the Arch repos.
    Repo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromePlan {
    Deb,
    Rpm,
    /// `pacman -S chromium`; Chrome proper is AUR-only and is never fetched
    /// through an AUR helper.
    Chromium,
}

// ---------------------------------------------------------------- profile --

pub struct DistroProfile {
    pub distro: Distro,
    pub manager: PackageManager,
    /// Package-manager update command (apt only); None means "no update step".
    pub update: Option<&'static [&'static str]>,
    pub base_packages: &'static [&'static str],
    /// Package whose presence marks "fonts installed" during detect.
    pub fonts_package: &'static str,
    /// Codec install attempts, tried in order; the first success wins. On the
    /// last failure the raw error is returned, unless `codecs_nonfree_hint`
    /// is set (Debian), which returns the manual non-free hint.
    pub codecs_tries: &'static [&'static [&'static str]],
    pub codecs_nonfree_hint: bool,
    pub nvidia: NvidiaPlan,
    pub gh: GhPlan,
    pub vscode: VsCodePlan,
    pub chrome: ChromePlan,
    /// Binary probed with `--version` for the Chrome step ("google-chrome",
    /// or "chromium" on Arch).
    pub chrome_binary: &'static str,
    pub lutris_packages: &'static [&'static str],
    /// Java runtime package installed when `java -version` is missing.
    pub java_package: &'static str,
    /// Fedora only: RPM Fusion must be enabled before other packages.
    pub rpm_fusion: bool,
    /// True for the apt family — the only one that may install .deb files.
    pub uses_apt: bool,
}

impl DistroProfile {
    pub fn install_prefix(&self) -> &'static [&'static str] {
        match self.manager {
            PackageManager::Apt => APT_PREFIX,
            PackageManager::Dnf => DNF_PREFIX,
            PackageManager::Pacman => PACMAN_PREFIX,
        }
    }

    pub fn install_verb(&self) -> &'static [&'static str] {
        match self.manager {
            PackageManager::Apt => APT_VERB,
            PackageManager::Dnf => DNF_VERB,
            PackageManager::Pacman => PACMAN_VERB,
        }
    }

    /// `<prefix> <verb> <items...>` as one fixed argv.
    pub fn install_argv<'a>(&self, items: &[&'a str]) -> Vec<&'a str> {
        let mut v = self.install_prefix().to_vec();
        v.extend_from_slice(self.install_verb());
        v.extend_from_slice(items);
        v
    }

    pub fn update_argv(&self) -> Option<Vec<&'static str>> {
        self.update.map(|u| {
            let mut v = self.install_prefix().to_vec();
            v.extend_from_slice(u);
            v
        })
    }

    /// Is `pkg` recorded as installed by this distro's package database?
    pub fn check_argv<'a>(&self, pkg: &'a str) -> Vec<&'a str> {
        match self.manager {
            PackageManager::Apt => vec!["dpkg-query", "-W", "-f=${Status}", pkg],
            PackageManager::Dnf => vec!["rpm", "-q", pkg],
            PackageManager::Pacman => vec!["pacman", "-Q", pkg],
        }
    }

    pub fn package_installed(&self, r: &dyn Runner, pkg: &str) -> bool {
        match r.output(&self.check_argv(pkg)) {
            Ok(o) if o.status == 0 => match self.manager {
                PackageManager::Apt => o.stdout.contains("install ok installed"),
                PackageManager::Dnf | PackageManager::Pacman => true,
            },
            _ => false,
        }
    }

    /// RPM Fusion install argv (Fedora only), with `{ver}` resolved from
    /// `rpm -E %fedora`. Empty for every other distro.
    pub fn rpm_fusion_install_argv(&self, r: &dyn Runner) -> Result<Vec<Vec<String>>, String> {
        if !self.rpm_fusion {
            return Ok(Vec::new());
        }
        let ver = version_of(r, &["rpm", "-E", "%fedora"])
            .ok_or_else(|| "cannot determine the Fedora version (rpm -E %fedora)".to_string())?;
        Ok([RPM_FUSION_FREE, RPM_FUSION_NONFREE]
            .iter()
            .map(|tpl| {
                let url = tpl.replace("{ver}", &ver);
                self.install_argv(&[&url])
                    .iter()
                    .map(|s| s.to_string())
                    .collect()
            })
            .collect())
    }
}

// ------------------------------------------------------------ profile rows --

pub static MINT: DistroProfile = DistroProfile {
    distro: Distro::Mint,
    manager: PackageManager::Apt,
    update: Some(APT_UPDATE),
    base_packages: &[
        "git",
        "curl",
        "build-essential",
        "ntfs-3g",
        "software-properties-common",
        "fonts-noto",
        "fonts-liberation",
    ],
    fonts_package: "fonts-noto",
    codecs_tries: &[&["mint-meta-codecs"], &["ubuntu-restricted-extras"]],
    codecs_nonfree_hint: false,
    nvidia: NvidiaPlan::UbuntuDrivers,
    gh: GhPlan::AptRepo,
    vscode: VsCodePlan::Deb,
    chrome: ChromePlan::Deb,
    chrome_binary: "google-chrome",
    lutris_packages: &["lutris", "winetricks"],
    java_package: "openjdk-21-jre",
    rpm_fusion: false,
    uses_apt: true,
};

pub static UBUNTU: DistroProfile = DistroProfile {
    distro: Distro::Ubuntu,
    manager: PackageManager::Apt,
    update: Some(APT_UPDATE),
    base_packages: MINT.base_packages,
    fonts_package: "fonts-noto",
    codecs_tries: &[&["ubuntu-restricted-extras"]],
    codecs_nonfree_hint: false,
    nvidia: NvidiaPlan::UbuntuDrivers,
    gh: GhPlan::AptRepo,
    vscode: VsCodePlan::Deb,
    chrome: ChromePlan::Deb,
    chrome_binary: "google-chrome",
    lutris_packages: &["lutris", "winetricks"],
    java_package: "openjdk-21-jre",
    rpm_fusion: false,
    uses_apt: true,
};

pub static DEBIAN: DistroProfile = DistroProfile {
    distro: Distro::Debian,
    manager: PackageManager::Apt,
    update: Some(APT_UPDATE),
    base_packages: MINT.base_packages,
    fonts_package: "fonts-noto",
    codecs_tries: &[&["libavcodec-extra"]],
    codecs_nonfree_hint: true,
    nvidia: NvidiaPlan::AptPackage {
        package: "nvidia-driver",
        nonfree_hint: true,
    },
    gh: GhPlan::AptRepo,
    vscode: VsCodePlan::Deb,
    chrome: ChromePlan::Deb,
    chrome_binary: "google-chrome",
    lutris_packages: &["lutris", "winetricks"],
    java_package: "openjdk-21-jre",
    rpm_fusion: false,
    uses_apt: true,
};

pub static FEDORA: DistroProfile = DistroProfile {
    distro: Distro::Fedora,
    manager: PackageManager::Dnf,
    update: None,
    base_packages: &[
        "git",
        "curl",
        "gcc",
        "gcc-c++",
        "make",
        "ntfs-3g",
        "google-noto-sans-fonts",
        "liberation-fonts",
    ],
    fonts_package: "google-noto-sans-fonts",
    codecs_tries: &[&[
        "ffmpeg",
        "gstreamer1-plugins-base",
        "gstreamer1-plugins-good",
        "gstreamer1-plugins-bad-free",
        "gstreamer1-plugins-ugly",
    ]],
    codecs_nonfree_hint: false,
    nvidia: NvidiaPlan::Akmod,
    gh: GhPlan::Dnf,
    vscode: VsCodePlan::Rpm,
    chrome: ChromePlan::Rpm,
    chrome_binary: "google-chrome",
    lutris_packages: &["lutris", "winetricks"],
    java_package: "java-17-openjdk",
    rpm_fusion: true,
    uses_apt: false,
};

pub static ARCH: DistroProfile = DistroProfile {
    distro: Distro::Arch,
    manager: PackageManager::Pacman,
    update: None,
    base_packages: &[
        "git",
        "curl",
        "base-devel",
        "ntfs-3g",
        "noto-fonts",
        "ttf-liberation",
    ],
    fonts_package: "noto-fonts",
    codecs_tries: &[&[
        "ffmpeg",
        "gst-plugins-base",
        "gst-plugins-good",
        "gst-plugins-bad",
        "gst-plugins-ugly",
        "libdvdcss",
    ]],
    codecs_nonfree_hint: false,
    nvidia: NvidiaPlan::Pacman,
    gh: GhPlan::Pacman,
    vscode: VsCodePlan::Repo,
    chrome: ChromePlan::Chromium,
    chrome_binary: "chromium",
    lutris_packages: &["lutris", "winetricks"],
    java_package: "jre17-openjdk",
    rpm_fusion: false,
    uses_apt: false,
};

pub fn profile_for(d: &Distro) -> Option<&'static DistroProfile> {
    match d {
        Distro::Mint => Some(&MINT),
        Distro::Ubuntu => Some(&UBUNTU),
        Distro::Debian => Some(&DEBIAN),
        Distro::Fedora => Some(&FEDORA),
        Distro::Arch => Some(&ARCH),
        Distro::Unsupported(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::setup::sudo_apt_get;

    #[test]
    fn apt_install_argv_matches_the_pre_slice_sudo_apt_get() {
        // Mint argv must be byte-for-byte identical to the old hard-coded form.
        let items = ["install", "-y", "lutris", "winetricks"];
        assert_eq!(MINT.install_argv(&items[2..]), sudo_apt_get(&items));
        assert_eq!(
            MINT.install_argv(&["mint-meta-codecs"]),
            sudo_apt_get(&["install", "-y", "mint-meta-codecs"])
        );
        assert_eq!(MINT.update_argv().unwrap(), sudo_apt_get(&["update"]));
    }

    #[test]
    fn fixed_prefixes_per_manager() {
        assert_eq!(
            FEDORA.install_argv(&["gh"]),
            vec!["sudo", "dnf", "-y", "install", "gh"]
        );
        assert_eq!(
            ARCH.install_argv(&["code"]),
            vec!["sudo", "pacman", "-S", "--needed", "--noconfirm", "code"]
        );
        assert!(FEDORA.update_argv().is_none());
        assert!(ARCH.update_argv().is_none());
    }

    #[test]
    fn check_argv_per_manager() {
        assert_eq!(
            MINT.check_argv("fonts-noto"),
            vec!["dpkg-query", "-W", "-f=${Status}", "fonts-noto"]
        );
        assert_eq!(FEDORA.check_argv("git"), vec!["rpm", "-q", "git"]);
        assert_eq!(ARCH.check_argv("git"), vec!["pacman", "-Q", "git"]);
    }

    #[test]
    fn profile_for_covers_exactly_the_five_supported_distros() {
        for d in [
            Distro::Mint,
            Distro::Ubuntu,
            Distro::Debian,
            Distro::Fedora,
            Distro::Arch,
        ] {
            assert!(profile_for(&d).is_some());
        }
        assert!(profile_for(&Distro::Unsupported("gentoo".to_string())).is_none());
    }
}
