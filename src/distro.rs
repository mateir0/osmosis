//! Slice 8 — distro detection from /etc/os-release and the hard refusal for
//! unsupported distros. The per-distro package-manager behaviour lives in
//! `distro::profiles` as pure data; the setup/games engines read it through
//! `DistroProfile` and never sprout per-distro if-chains of their own.

use std::path::Path;

pub mod profiles;

pub use profiles::{profile_for, DistroProfile};

/// The distros OSmosis knows how to drive. `Unsupported` carries the raw
/// `ID=` value so the refusal can name what was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Distro {
    Mint,
    Ubuntu,
    Debian,
    Fedora,
    Arch,
    Unsupported(String),
}

/// Human-readable list used in the refusal message.
pub const SUPPORTED_LIST: &str = "Linux Mint, Ubuntu, Debian, Fedora, Arch Linux";

impl Distro {
    /// The lowercase `ID=` value this distro maps to (used in reports).
    pub fn id(&self) -> &str {
        match self {
            Distro::Mint => "linuxmint",
            Distro::Ubuntu => "ubuntu",
            Distro::Debian => "debian",
            Distro::Fedora => "fedora",
            Distro::Arch => "arch",
            Distro::Unsupported(id) => id.as_str(),
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            Distro::Mint => "Linux Mint".to_string(),
            Distro::Ubuntu => "Ubuntu".to_string(),
            Distro::Debian => "Debian".to_string(),
            Distro::Fedora => "Fedora".to_string(),
            Distro::Arch => "Arch Linux".to_string(),
            Distro::Unsupported(id) => format!("unsupported distro ({})", id),
        }
    }

    pub fn is_supported(&self) -> bool {
        !matches!(self, Distro::Unsupported(_))
    }
}

/// Extract one `KEY=VALUE` field from an os-release line, stripping a
/// matching pair of double or single quotes.
fn field<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (k, v) = line.split_once('=')?;
    if k.trim() != key {
        return None;
    }
    let v = v.trim();
    for q in ['"', '\''] {
        if let Some(inner) = v.strip_prefix(q).and_then(|s| s.strip_suffix(q)) {
            return Some(inner);
        }
    }
    Some(v)
}

/// Map raw os-release contents to a `Distro`. Exact `ID=` matches win;
/// otherwise `ID_LIKE` is scanned in priority order (ubuntu, debian, arch,
/// fedora/rhel). Anything else is `Unsupported`.
pub fn parse_os_release(contents: &str) -> Distro {
    let mut id: Option<String> = None;
    let mut id_like = String::new();
    for line in contents.lines() {
        if let Some(v) = field(line, "ID") {
            id = Some(v.to_ascii_lowercase());
        }
        if let Some(v) = field(line, "ID_LIKE") {
            id_like = v.to_ascii_lowercase();
        }
    }
    match id.as_deref() {
        Some("linuxmint") => return Distro::Mint,
        Some("ubuntu") => return Distro::Ubuntu,
        Some("debian") => return Distro::Debian,
        Some("fedora") => return Distro::Fedora,
        Some("arch") => return Distro::Arch,
        _ => {}
    }
    let like = id_like.as_str();
    if like.contains("ubuntu") {
        Distro::Ubuntu
    } else if like.contains("debian") {
        Distro::Debian
    } else if like.contains("arch") {
        Distro::Arch
    } else if like.contains("fedora") || like.contains("rhel") {
        Distro::Fedora
    } else {
        Distro::Unsupported(id.unwrap_or_else(|| "unknown".to_string()))
    }
}

/// Read and parse an os-release file (path-taking so tests can feed
/// fixtures without touching /etc).
pub fn detect_from_path(path: &Path) -> std::io::Result<Distro> {
    let contents = std::fs::read_to_string(path)?;
    Ok(parse_os_release(&contents))
}

/// Detect the host distro from /etc/os-release.
pub fn detect() -> Result<Distro, String> {
    detect_from_path(Path::new("/etc/os-release"))
        .map_err(|e| format!("cannot read /etc/os-release: {}", e))
}

/// The hard-refusal error for an unsupported distro. Returned BEFORE any
/// Runner command is issued, so a fake runner must record zero calls.
pub fn refusal(id: &str) -> String {
    format!(
        "unsupported distro '{}' — OSmosis supports {} only; refusing to run any package-manager command on this host",
        id, SUPPORTED_LIST
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_ids() {
        let cases: &[(&str, &str, Distro)] = &[
            ("linuxmint", "", Distro::Mint),
            ("ubuntu", "", Distro::Ubuntu),
            ("debian", "", Distro::Debian),
            ("fedora", "", Distro::Fedora),
            ("arch", "", Distro::Arch),
        ];
        for (id, like, want) in cases {
            let contents = format!("NAME=x\nID={}\nID_LIKE={}\n", id, like);
            assert_eq!(&parse_os_release(&contents), want, "id={}", id);
        }
    }

    #[test]
    fn quoted_values_and_id_like_fallbacks() {
        assert_eq!(
            parse_os_release("ID=\"pop\"\nID_LIKE=\"ubuntu debian\"\n"),
            Distro::Ubuntu
        );
        assert_eq!(parse_os_release("ID=manjaro\nID_LIKE=arch\n"), Distro::Arch);
        assert_eq!(
            parse_os_release("ID=rocky\nID_LIKE='rhel centos fedora'\n"),
            Distro::Fedora
        );
        assert_eq!(
            parse_os_release("ID=opensuse\nID_LIKE=suse\n"),
            Distro::Unsupported("opensuse".to_string())
        );
        assert_eq!(
            parse_os_release("PRETTY_NAME=\"No ID here\"\n"),
            Distro::Unsupported("unknown".to_string())
        );
    }

    #[test]
    fn refusal_names_the_detected_id_and_supported_list() {
        let msg = refusal("gentoo");
        assert!(msg.contains("gentoo"), "{}", msg);
        assert!(msg.contains(SUPPORTED_LIST), "{}", msg);
        assert!(msg.contains("refusing"), "{}", msg);
    }
}
