//! GTA V installation detection.
//!
//! Detects the distribution (Rockstar / Steam / Epic / Unknown) from
//! legitimate read-only signals, then confirms the installation is real by
//! checking the expected file structure.
//!
//! IMPORTANT: detection is deliberately separate from entitlement.
//! A found installation is reported as INSTALLATION_DETECTED, never as
//! ownership. See the top-level module docs.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Which distribution a detected installation came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameDistribution {
    Rockstar,
    Steam,
    EpicGames,
    Unknown,
}

impl GameDistribution {
    pub fn as_str(self) -> &'static str {
        match self {
            GameDistribution::Rockstar => "ROCKSTAR",
            GameDistribution::Steam => "STEAM",
            GameDistribution::EpicGames => "EPIC_GAMES",
            GameDistribution::Unknown => "UNKNOWN",
        }
    }
}

/// Result of searching for a GTA V installation. Detection only — this is
/// never an entitlement verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallationResult {
    pub distribution: GameDistribution,
    pub install_path: Option<PathBuf>,
    /// Distributions we found evidence for but could not confirm via file
    /// structure. Useful for Aegis diagnostics, not for the player.
    pub candidates: Vec<DistributionCandidate>,
    pub status: DetectionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DetectionStatus {
    /// Files confirmed on disk.
    InstallationDetected,
    /// Nothing found.
    NotDetected,
}

/// A distribution we found evidence for, before file-structure confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistributionCandidate {
    pub distribution: GameDistribution,
    pub evidence: String,
    pub path: PathBuf,
    /// Did the expected game files actually exist there?
    pub files_confirmed: bool,
}

/// Files whose presence distinguishes a real GTA V installation from an
/// empty or partial directory.
const GTA_V_FILES: &[&str] = &["GTA5.exe", "bink2w64.dll", "x64a.rpf"];

impl InstallationResult {
    fn not_detected() -> Self {
        InstallationResult {
            distribution: GameDistribution::Unknown,
            install_path: None,
            candidates: Vec::new(),
            status: DetectionStatus::NotDetected,
        }
    }
}

/// Search for a GTA V installation.
///
/// `extra_roots` lets tests (or a user-configured path list) inject search
/// locations without touching the real filesystem.
pub fn detect_gta_installation(extra_roots: &[PathBuf]) -> InstallationResult {
    let mut candidates: Vec<DistributionCandidate> = Vec::new();

    // 1. Registry-derived install directories (Windows, publisher-written).
    for path in registry_install_dirs() {
        candidates.push(probe(GameDistribution::Rockstar, "registry uninstall key", path));
    }
    // 2. Steam library folders.
    for path in steam_library_install_dirs() {
        candidates.push(probe(GameDistribution::Steam, "steamapps library", path));
    }
    // 3. Epic manifests.
    for path in epic_install_dirs() {
        candidates.push(probe(GameDistribution::EpicGames, "epic manifest", path));
    }
    // 4. User/configured roots — distribution unknown unless confirmed.
    for path in extra_roots {
        candidates.push(probe(GameDistribution::Unknown, "configured root", path.clone()));
    }

    // Prefer a confirmed installation; distribution order is irrelevant once
    // the files exist, but a confirmed Rockstar/Steam/Epic candidate wins
    // over a bare Unknown root with the same files.
    let confirmed = candidates
        .iter()
        .find(|c| c.files_confirmed && c.distribution != GameDistribution::Unknown)
        .or_else(|| candidates.iter().find(|c| c.files_confirmed));

    match confirmed {
        Some(c) => InstallationResult {
            distribution: c.distribution,
            install_path: Some(c.path.clone()),
            candidates,
            status: DetectionStatus::InstallationDetected,
        },
        None => {
            let mut res = InstallationResult::not_detected();
            res.candidates = candidates;
            res
        }
    }
}

fn probe(distribution: GameDistribution, evidence: &str, path: PathBuf) -> DistributionCandidate {
    let confirmed = confirm_gta_files(&path);
    DistributionCandidate { distribution, evidence: evidence.to_string(), path, files_confirmed: confirmed }
}

/// Decisive check: the expected executables/archives must exist here.
pub fn confirm_gta_files(path: &Path) -> bool {
    GTA_V_FILES.iter().all(|f| path.join(f).is_file())
}

#[cfg(windows)]
fn registry_install_dirs() -> Vec<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;

    let mut out = Vec::new();
    // Rockstar and Steam both write standard uninstall metadata. We only read
    // the display-name-matching entries, never credentials or tokens.
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        let uninstall =
            match RegKey::predef(hive).open_subkey("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall") {
                Ok(k) => k,
                Err(_) => continue,
            };
        for key in uninstall.enum_keys().flatten() {
            let sub = match uninstall.open_subkey(&key) {
                Ok(s) => s,
                Err(_) => continue,
            };
            let name: String = sub.get_value("DisplayName").unwrap_or_default();
            let location: String = sub.get_value("InstallLocation").unwrap_or_default();
            if is_gta_display_name(&name) && !location.trim().is_empty() {
                out.push(PathBuf::from(location));
            }
        }
    }
    out
}

#[cfg(not(windows))]
fn registry_install_dirs() -> Vec<PathBuf> {
    Vec::new()
}

fn is_gta_display_name(name: &str) -> bool {
    let n = name.to_lowercase();
    // Match the installers' display names without depending on exact strings.
    n.contains("grand theft auto v") || n.contains("gta v") || n == "gta5"
}

/// Steam library folders, parsed from steamapps/libraryfolders.vdf.
///
/// Only the library paths are read — no login tokens, no user metadata.
pub fn steam_library_install_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for lib in steam_library_roots() {
        let manifest = lib.join("steamapps").join("libraryfolders.vdf");
        let Ok(text) = std::fs::read_to_string(&manifest) else {
            continue;
        };
        // The VDF "path" lines contain quoted absolute paths.
        for line in text.lines() {
            if let Some(p) = extract_quoted(line, "\"path\"") {
                let candidate = PathBuf::from(p).join("steamapps").join("common").join("Grand Theft Auto V");
                out.push(candidate);
            }
        }
    }
    out
}

fn steam_library_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".steam").join("steam"));
    }
    if cfg!(windows) {
        if let Some(pf) = std::env::var_os("ProgramFiles(x86)") {
            roots.push(PathBuf::from(pf).join("Steam"));
        }
        if let Some(pf) = std::env::var_os("ProgramFiles") {
            roots.push(PathBuf::from(pf).join("Steam"));
        }
    }
    roots
}

fn epic_install_dirs() -> Vec<PathBuf> {
    // Epic stores install manifests under %ProgramData%; read-only lookup of
    // the game install location only.
    let mut out = Vec::new();
    let Some(pd) = std::env::var_os("ProgramData") else {
        return out;
    };
    let manifests = PathBuf::from(pd).join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests");
    let Ok(entries) = std::fs::read_dir(&manifests) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("item") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        // Look for the GTA install location in the manifest.
        if text.to_lowercase().contains("grand theft auto") || text.to_lowercase().contains("gta") {
            if let Some(p) = extract_quoted(&text, "\"InstallLocation\"") {
                out.push(PathBuf::from(p));
            }
        }
    }
    out
}

/// Extract the value following `key` in a quoted VDF/JSON-ish line.
fn extract_quoted(line: &str, key: &str) -> Option<String> {
    let idx = line.find(key)?;
    let rest = &line[idx + key.len()..];
    let start = rest.find('"')? + 1;
    let end = rest[start..].find('"')? + start;
    Some(rest[start..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fake_install(dir: &std::path::Path) {
        fs::create_dir_all(dir).unwrap();
        for f in GTA_V_FILES {
            fs::write(dir.join(f), b"stub").unwrap();
        }
    }

    #[test]
    fn injected_root_confirms_files() {
        // Machine-independent: only our injected root can be confirmed here.
        let tmp = std::env::temp_dir().join(format!("ald-novagate-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let game = tmp.join("gta");
        fake_install(&game);
        let res = detect_gta_installation(&[game.clone()]);
        assert!(res.candidates.iter().any(|c| c.files_confirmed && c.path == game));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn partial_files_do_not_confirm() {
        let tmp = std::env::temp_dir().join(format!("ald-novagate-empty-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        // Only one of the required files.
        fs::write(tmp.join("GTA5.exe"), b"stub").unwrap();
        assert!(!confirm_gta_files(&tmp));
        let res = detect_gta_installation(&[tmp.clone()]);
        // Our injected candidate must not report a confirmed install.
        assert!(res.candidates.iter().find(|c| c.path == tmp).map(|c| !c.files_confirmed).unwrap_or(true));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detection_result_carries_no_entitlement_claim() {
        // Machine-independent invariant: the result type carries no ownership
        // verdict anywhere, only a files-on-disk status. This test exists to
        // document and guard that separation.
        let res = detect_gta_installation(&[]);
        assert!(matches!(res.status, DetectionStatus::InstallationDetected | DetectionStatus::NotDetected));
        // Regardless of what was found, no field on InstallationResult
        // expresses entitlement — verified by construction.
    }

    #[test]
    fn file_check_requires_all_three_files() {
        let tmp = std::env::temp_dir().join(format!("ald-novagate-partial-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(&tmp).unwrap();
        fs::write(tmp.join("GTA5.exe"), b"stub").unwrap();
        fs::write(tmp.join("bink2w64.dll"), b"stub").unwrap();
        // x64a.rpf missing
        assert!(!confirm_gta_files(&tmp));
        fs::write(tmp.join("x64a.rpf"), b"stub").unwrap();
        assert!(confirm_gta_files(&tmp));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detection_status_is_never_entitlement() {
        // The type system enforces this: InstallationResult carries no
        // ownership claim anywhere. This test documents the invariant.
        let tmp = std::env::temp_dir().join(format!("ald-novagate-status-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        fake_install(&tmp);
        let res = detect_gta_installation(&[tmp.clone()]);
        assert_eq!(res.status, DetectionStatus::InstallationDetected);
        // No entitlement field exists on this type, by construction.
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn distribution_codes() {
        assert_eq!(GameDistribution::Rockstar.as_str(), "ROCKSTAR");
        assert_eq!(GameDistribution::Steam.as_str(), "STEAM");
        assert_eq!(GameDistribution::EpicGames.as_str(), "EPIC_GAMES");
        assert_eq!(GameDistribution::Unknown.as_str(), "UNKNOWN");
    }

    #[test]
    fn display_name_matching() {
        assert!(is_gta_display_name("Grand Theft Auto V"));
        assert!(is_gta_display_name("Grand Theft Auto V: Premium Edition"));
        assert!(!is_gta_display_name("Microsoft Visual C++ Redistributable"));
    }

    #[test]
    fn extract_quoted_from_vdf() {
        let line = "\t\"path\"\t\t\"D:\\\\SteamLibrary\"";
        assert_eq!(extract_quoted(line, "\"path\"").unwrap(), "D:\\\\SteamLibrary");
    }

    #[test]
    fn extract_quoted_missing_key() {
        assert!(extract_quoted("\"other\" \"x\"", "\"path\"").is_none());
    }

    #[test]
    fn candidates_recorded_for_diagnostics() {
        let tmp = std::env::temp_dir().join(format!("ald-novagate-cand-{}", std::process::id()));
        let _ = fs::remove_dir_all(&tmp);
        let game = tmp.join("gta");
        fake_install(&game);
        let res = detect_gta_installation(&[game]);
        assert!(!res.candidates.is_empty());
        let _ = fs::remove_dir_all(&tmp);
    }
}
