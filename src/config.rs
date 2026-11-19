//! Scanner configuration: embedded defaults plus an optional override file.
//!
//! The override file is parsed by a small hand-rolled parser for the restricted
//! TOML subset documented in `config.example.toml` (sections, string pairs,
//! integers, string arrays). This keeps Slice 1 inside its dependency allowlist
//! (clap, serde, serde_json, walkdir, xxhash-rust, fs2, chrono) instead of adding
//! a `toml` crate.

use std::collections::BTreeMap;
use std::path::Path;

/// A normalized KEEP mapping: `prefix_lower` is the lowercase, forward-slash
/// form of the windows-relative prefix; `prefix` keeps the original casing so the
/// destination path can be rebuilt from the original relative path.
#[derive(Debug, Clone)]
pub struct KeepMapping {
    pub prefix: String,
    pub prefix_lower: String,
    pub dest: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub keep_mappings: Vec<KeepMapping>, // sorted by prefix length, longest first
    pub sensitive_prefixes: Vec<String>, // lowercase prefixes
    pub recreate_dirs: Vec<String>,      // lowercase names
    pub skip_names: Vec<String>,         // lowercase exact file names
    pub skip_prefix_names: Vec<String>,  // lowercase file-name prefixes
    pub skip_extensions: Vec<String>,    // lowercase, with leading dot
    pub appdata_root: String,            // compared case-insensitively
    pub hash_max_bytes: u64,
}

impl Config {
    /// The embedded default classification config (section 1.2 of the slice spec).
    pub fn default_config() -> Config {
        let mappings: Vec<(&str, &str)> = vec![
            ("Documents", "~/Documents"),
            ("Desktop", "~/Desktop"),
            ("Pictures", "~/Pictures"),
            ("Videos", "~/Videos"),
            ("Music", "~/Music"),
            ("Downloads", "~/Downloads"),
            (".ssh", "~/.ssh"),
            (".gitconfig", "~/.gitconfig"),
            ("AppData/Roaming/.minecraft", "~/.minecraft"),
            ("AppData/Roaming/.tlauncher", "~/.tlauncher"),
            ("AppData/Roaming/.fastclient", "~/.fastclient"),
            ("AppData/Roaming/FastClient", "~/.config/FastClient"),
            (
                "AppData/Roaming/Telegram Desktop",
                "~/.local/share/TelegramDesktop",
            ),
            ("AppData/Roaming/Mozilla", "~/.mozilla"),
            (
                "AppData/Roaming/ai.opencode.desktop",
                "~/.local/share/ai.opencode.desktop",
            ),
            (
                "AppData/Roaming/Antigravity IDE/User",
                "~/.config/Antigravity IDE/User",
            ),
            ("AppData/Roaming/Code/User", "~/.config/Code/User"),
            (
                "AppData/Local/Google/Chrome/User Data/Default",
                "~/.config/google-chrome/Default",
            ),
        ];
        Config {
            keep_mappings: sorted_mappings(&mappings),
            sensitive_prefixes: vec![
                ".ssh".to_string(),
                "appdata/roaming/telegram desktop".to_string(),
                "appdata/roaming/mozilla".to_string(),
                "appdata/local/google/chrome/user data/default".to_string(),
            ],
            recreate_dirs: vec![
                "node_modules",
                ".next",
                ".turbo",
                ".cache",
                "target",
                "__pycache__",
                ".venv",
                "dist",
                "out",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            skip_names: vec!["ntuser.ini", "thumbs.db", "desktop.ini"]
                .into_iter()
                .map(String::from)
                .collect(),
            skip_prefix_names: vec!["ntuser.dat".to_string()],
            skip_extensions: vec![".tmp".to_string(), ".sys".to_string()],
            appdata_root: "AppData".to_string(),
            hash_max_bytes: 256 * 1024 * 1024, // 256 MiB
        }
    }

    /// Load an override file on top of the embedded defaults.
    /// Any key absent from the file keeps its default value.
    pub fn load_override(path: &Path) -> Result<Config, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read config file {}: {}", path.display(), e))?;
        parse_override(&text)
    }
}

fn sorted_mappings(mappings: &[(&str, &str)]) -> Vec<KeepMapping> {
    sorted_mappings_owned(
        &mappings
            .iter()
            .map(|(p, d)| (p.to_string(), d.to_string()))
            .collect::<Vec<_>>(),
    )
}

fn sorted_mappings_owned(mappings: &[(String, String)]) -> Vec<KeepMapping> {
    let mut v: Vec<KeepMapping> = mappings
        .iter()
        .map(|(p, d)| KeepMapping {
            prefix: p.clone(),
            prefix_lower: p.to_lowercase(),
            dest: d.clone(),
        })
        .collect();
    // Longest prefix first so "AppData/Local/Google/Chrome/User Data/Default"
    // beats a hypothetical shorter "AppData/Local" mapping.
    v.sort_by_key(|m| std::cmp::Reverse(m.prefix_lower.len()));
    v
}

fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    let mut prev = '\0';
    for (i, c) in line.char_indices() {
        if c == '"' && prev != '\\' {
            in_str = !in_str;
        }
        if c == '#' && !in_str {
            return &line[..i];
        }
        prev = c;
    }
    line
}

fn parse_string(s: &str) -> Result<String, String> {
    let t = s.trim();
    if t.len() >= 2 && t.starts_with('"') && t.ends_with('"') {
        Ok(t[1..t.len() - 1].to_string())
    } else {
        Err(format!("expected a quoted string, got: {}", t))
    }
}

fn parse_string_array(s: &str) -> Result<Vec<String>, String> {
    let t = s.trim();
    if !(t.starts_with('[') && t.ends_with(']')) {
        return Err(format!("expected an array, got: {}", t));
    }
    let inner = &t[1..t.len() - 1];
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    let mut prev = '\0';
    for c in inner.chars() {
        if c == '"' && prev != '\\' {
            in_str = !in_str;
            cur.push(c);
        } else if c == ',' && !in_str {
            if !cur.trim().is_empty() {
                out.push(parse_string(&cur)?);
            }
            cur.clear();
        } else {
            cur.push(c);
        }
        prev = c;
    }
    if !cur.trim().is_empty() {
        out.push(parse_string(&cur)?);
    }
    Ok(out)
}

fn parse_override(text: &str) -> Result<Config, String> {
    let mut cfg = Config::default_config();
    let mut section = String::new();
    let mut keep: Vec<(String, String)> = Vec::new();
    let mut have_keep = false;

    for (lineno, raw) in text.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        let eq = line
            .find('=')
            .ok_or_else(|| format!("line {}: expected `key = value`, got: {}", lineno + 1, line))?;
        let key = line[..eq].trim().trim_matches('"').to_string();
        let value = &line[eq + 1..];
        match section.as_str() {
            "" => {
                if key == "hash_max_bytes" {
                    cfg.hash_max_bytes = value
                        .trim()
                        .parse::<u64>()
                        .map_err(|e| format!("line {}: bad hash_max_bytes: {}", lineno + 1, e))?;
                }
            }
            "keep_mappings" => {
                have_keep = true;
                let k = parse_string(line[..eq].trim())?.replace('\\', "/");
                let v = parse_string(value)?;
                keep.push((k, v));
            }
            "sensitive_mappings" if key == "prefixes" => {
                cfg.sensitive_prefixes = parse_string_array(value)?
                    .into_iter()
                    .map(|s| s.to_lowercase())
                    .collect();
            }
            "recreate_dirs" if key == "names" => {
                cfg.recreate_dirs = parse_string_array(value)?
                    .into_iter()
                    .map(|s| s.to_lowercase())
                    .collect();
            }
            "skip" => match key.as_str() {
                "appdata_root" => cfg.appdata_root = parse_string(value)?,
                "names" => {
                    cfg.skip_names = parse_string_array(value)?
                        .into_iter()
                        .map(|s| s.to_lowercase())
                        .collect()
                }
                "prefix_names" => {
                    cfg.skip_prefix_names = parse_string_array(value)?
                        .into_iter()
                        .map(|s| s.to_lowercase())
                        .collect()
                }
                "extensions" => {
                    cfg.skip_extensions = parse_string_array(value)?
                        .into_iter()
                        .map(|s| {
                            let s = s.to_lowercase();
                            if s.starts_with('.') {
                                s
                            } else {
                                format!(".{}", s)
                            }
                        })
                        .collect()
                }
                _ => {}
            },
            _ => {}
        }
    }
    if have_keep {
        cfg.keep_mappings = sorted_mappings_owned(&keep);
    }
    Ok(cfg)
}

/// Keep a BTreeMap import used by callers that want keyed lookups.
pub fn mapping_index(cfg: &Config) -> BTreeMap<String, String> {
    cfg.keep_mappings
        .iter()
        .map(|m| (m.prefix.clone(), m.dest.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = Config::default_config();
        assert_eq!(cfg.hash_max_bytes, 256 * 1024 * 1024);
        assert!(cfg
            .keep_mappings
            .iter()
            .any(|m| m.prefix == "AppData/Roaming/Code/User"));
        assert!(cfg
            .keep_mappings
            .iter()
            .any(|m| m.prefix == "AppData/Roaming/.minecraft" && m.dest == "~/.minecraft"));
        assert!(!cfg
            .keep_mappings
            .iter()
            .any(|m| m.prefix == ".minecraft/saves"));
        assert!(cfg.recreate_dirs.contains(&"node_modules".to_string()));
        assert!(cfg.recreate_dirs.contains(&"dist".to_string()));
        assert!(cfg.recreate_dirs.contains(&"out".to_string()));
    }

    #[test]
    fn override_parses_example_shape() {
        let text = r#"
# comment
hash_max_bytes = 1024

[keep_mappings]
"Documents" = "~/Documents"
".ssh" = "~/.ssh"

[sensitive_mappings]
prefixes = [".ssh"]

[recreate_dirs]
names = ["node_modules", "target"]

[skip]
appdata_root = "AppData"
names = ["desktop.ini"]
prefix_names = ["ntuser.dat"]
extensions = [".tmp", "sys"]
"#;
        let cfg = parse_override(text).unwrap();
        assert_eq!(cfg.hash_max_bytes, 1024);
        assert_eq!(cfg.keep_mappings.len(), 2);
        assert_eq!(
            cfg.skip_extensions,
            vec![".tmp".to_string(), ".sys".to_string()]
        );
    }
}
