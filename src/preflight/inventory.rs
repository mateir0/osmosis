//! Preflight probes (slice 9.3 + 9.5): the installed-app inventory and the
//! hardware/BitLocker snapshot. Each probe is ONE PowerShell invocation whose
//! script is a compile-time constant string, run as a fixed argv array through
//! the `Runner` abstraction — no interpolated values in the scripts, ever.
//!
//! READ-PATH RULE: a failed or unparseable probe is a recorded failure
//! (`Inventory::Failed` / hardware error note), never a silently empty list.

use super::{Hardware, InstalledApp, Inventory};
use crate::setup::Runner;
use std::collections::HashSet;

/// Reads the three standard uninstall registry locations and emits JSON
/// (DisplayName, DisplayVersion, Publisher). Constant script — no variables
/// are interpolated into it from Rust.
pub const PS_APPS_SCRIPT: &str = r#"$ErrorActionPreference = 'SilentlyContinue'; $apps = @(); $paths = @('HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKLM:\SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\*', 'HKCU:\SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\*'); foreach ($p in $paths) { Get-ItemProperty $p | ForEach-Object { if ($_.DisplayName) { $apps += [PSCustomObject]@{ DisplayName = [string]$_.DisplayName; DisplayVersion = [string]$_.DisplayVersion; Publisher = [string]$_.Publisher } } } }; if ($apps.Count -eq 0) { '[]' } elseif ($apps.Count -eq 1) { $apps[0] | ConvertTo-Json -Compress } else { $apps | ConvertTo-Json -Compress }"#;

/// Video controllers, physical network adapters, printers, and the BitLocker
/// status of C:. `Get-BitLockerVolume` is wrapped in try/catch: when the cmdlet
/// is unavailable the script emits "unknown" — never a guessed status.
pub const PS_HW_SCRIPT: &str = r#"$ErrorActionPreference = 'SilentlyContinue'; $video = @(Get-CimInstance Win32_VideoController | ForEach-Object { [string]$_.Name }); $net = @(Get-CimInstance Win32_NetworkAdapter | Where-Object { $_.PhysicalAdapter -eq $true } | ForEach-Object { [string]$_.Name }); $printers = @(Get-CimInstance Win32_Printer | ForEach-Object { [string]$_.Name }); $bl = 'unknown'; $bld = ''; try { $v = Get-BitLockerVolume -MountPoint 'C:' -ErrorAction Stop; $bl = [string]$v.ProtectionStatus; $bld = [string]$v.VolumeStatus } catch { $bl = 'unknown'; $bld = 'Get-BitLockerVolume unavailable: ' + $_.Exception.Message }; [PSCustomObject]@{ Video = $video; Network = $net; Printers = $printers; BitLocker = $bl; BitLockerDetail = $bld } | ConvertTo-Json -Compress"#;

/// Fixed argv arrays: program + flags + the constant script. No interpolation.
pub const PS_APPS_ARGV: &[&str] = &[
    "powershell",
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    PS_APPS_SCRIPT,
];
pub const PS_HW_ARGV: &[&str] = &[
    "powershell",
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    PS_HW_SCRIPT,
];

/// Run the app-inventory probe. Any failure is recorded as `Inventory::Failed`.
pub fn collect_apps(runner: &dyn Runner) -> Inventory {
    match runner.output(PS_APPS_ARGV) {
        Ok(out) if out.status == 0 => match parse_apps(&out.stdout) {
            Ok(apps) => Inventory::Ok(apps),
            Err(e) => Inventory::Failed(format!(
                "app inventory: PowerShell returned unparseable output ({}); stderr: {}",
                e,
                out.stderr.trim()
            )),
        },
        Ok(out) => Inventory::Failed(format!(
            "app inventory: powershell exited with status {}; stderr: {}",
            out.status,
            out.stderr.trim()
        )),
        Err(e) => Inventory::Failed(format!("app inventory: cannot run powershell: {}", e)),
    }
}

/// Parse the inventory JSON. De-duplicates by name (case-insensitive) across
/// the three registry locations. A single object (one app) is accepted, since
/// ConvertTo-Json does not wrap a one-element pipeline in an array.
pub fn parse_apps(json: &str) -> Result<Vec<InstalledApp>, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid JSON: {}", e))?;
    let items: Vec<serde_json::Value> = match &v {
        serde_json::Value::Array(a) => a.clone(),
        serde_json::Value::Object(_) => vec![v.clone()],
        _ => return Err("expected a JSON array (or single object)".to_string()),
    };
    let mut apps = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for item in items {
        let name = item
            .get("DisplayName")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if name.is_empty() || !seen.insert(name.to_lowercase()) {
            continue; // empty or duplicate across registry locations
        }
        let str_field = |k: &str| -> String {
            item.get(k)
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string()
        };
        apps.push(InstalledApp {
            name,
            version: str_field("DisplayVersion"),
            publisher: str_field("Publisher"),
        });
    }
    Ok(apps)
}

/// Run the hardware/BitLocker probe. On failure the hardware defaults to
/// empty + bitlocker "unknown" and the error is returned for the report notes.
pub fn collect_hardware(runner: &dyn Runner) -> (Hardware, Option<String>) {
    match runner.output(PS_HW_ARGV) {
        Ok(out) if out.status == 0 => match parse_hardware(&out.stdout) {
            Ok(h) => (h, None),
            Err(e) => (
                Hardware::default(),
                Some(format!(
                    "hardware probe returned unparseable output ({}); stderr: {}",
                    e,
                    out.stderr.trim()
                )),
            ),
        },
        Ok(out) => (
            Hardware::default(),
            Some(format!(
                "hardware probe: powershell exited with status {}; stderr: {}",
                out.status,
                out.stderr.trim()
            )),
        ),
        Err(e) => (
            Hardware::default(),
            Some(format!("hardware probe: cannot run powershell: {}", e)),
        ),
    }
}

/// Parse the hardware JSON: string arrays for video/network/printers plus the
/// BitLocker status. "On"/"Off" are normalized to lowercase; anything else
/// (including the cmdlet-unavailable "unknown") stays "unknown".
pub fn parse_hardware(json: &str) -> Result<Hardware, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid JSON: {}", e))?;
    let arr = |k: &str| -> Vec<String> {
        v.get(k)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default()
    };
    let bitlocker_raw = v
        .get("BitLocker")
        .and_then(|x| x.as_str())
        .unwrap_or("unknown")
        .to_lowercase();
    let bitlocker = match bitlocker_raw.as_str() {
        "on" => "on".to_string(),
        "off" => "off".to_string(),
        _ => "unknown".to_string(),
    };
    Ok(Hardware {
        video: arr("Video"),
        network: arr("Network"),
        printers: arr("Printers"),
        bitlocker,
        bitlocker_detail: v
            .get("BitLockerDetail")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
    })
}
