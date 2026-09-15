//! `ald migrate` — scan a legacy (FiveM/ESX/QBCore/Qbox) resource and report
//! compatibility without rewriting it.
//!
//! The report is deliberately conservative: an unknown global is reported as
//! unsupported, and no code is rewritten. Confidence is reported as a
//! difficulty estimate so a human can triage.

use std::fs;
use std::path::Path;
use std::process::exit;

/// One finding in the migration report.
#[derive(Debug, Clone)]
pub struct Finding {
    pub severity: Severity,
    /// Where in the resource it was found (file:line when known).
    pub location: String,
    pub detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// The call has a native equivalent; automatic at runtime.
    Supported,
    /// Works with documented caveats.
    Partial,
    /// Must be ported manually.
    Unsupported,
    /// FiveM-specific runtime behavior; no Aldivine equivalent planned.
    FivemOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    EsxCall,
    QbcoreCall,
    QboxCall,
    FivemGlobal,
    FivemNative,
    Database,
    Manifest,
    Ui,
}

impl Category {
    fn as_str(self) -> &'static str {
        match self {
            Category::EsxCall => "ESX call",
            Category::QbcoreCall => "QBCore call",
            Category::QboxCall => "Qbox call",
            Category::FivemGlobal => "FiveM global",
            Category::FivemNative => "FiveM native",
            Category::Database => "database dependency",
            Category::Manifest => "manifest",
            Category::Ui => "NUI/UI",
        }
    }
}

/// Full migration report for one resource.
#[derive(Debug)]
pub struct Report {
    pub resource_path: String,
    pub flavor: Option<String>,
    pub files_scanned: usize,
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn difficulty(&self) -> Difficulty {
        let unsupported = self
            .findings
            .iter()
            .filter(|f| f.severity == Severity::Unsupported || f.severity == Severity::FivemOnly)
            .count();
        let partial = self.findings.iter().filter(|f| f.severity == Severity::Partial).count();
        match (unsupported, partial) {
            (0, 0) => Difficulty::Trivial,
            (0, _) => Difficulty::Easy,
            (1..=5, _) => Difficulty::Moderate,
            (6..=20, _) => Difficulty::Hard,
            (_, _) => Difficulty::VeryHard,
        }
    }

    pub fn summary_lines(&self) -> Vec<String> {
        let mut out = Vec::new();
        out.push(format!("resource: {}", self.resource_path));
        out.push(format!("detected flavor: {}", self.flavor.as_deref().unwrap_or("unknown")));
        out.push(format!("files scanned: {}", self.files_scanned));
        let by_sev = severity_counts(&self.findings);
        out.push(format!(
            "supported: {}  partial: {}  unsupported: {}  fivem-only: {}",
            by_sev[0], by_sev[1], by_sev[2], by_sev[3]
        ));
        out.push(format!("estimated difficulty: {:?}", self.difficulty()));
        out.push(String::new());
        out.push("findings:".into());
        for f in &self.findings {
            out.push(format!("  [{}] {} — {}", severity_label(f.severity), f.location, f.detail));
        }
        out
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    Trivial,
    Easy,
    Moderate,
    Hard,
    VeryHard,
}

fn severity_counts(findings: &[Finding]) -> [usize; 4] {
    let mut c = [0usize; 4];
    for f in findings {
        match f.severity {
            Severity::Supported => c[0] += 1,
            Severity::Partial => c[1] += 1,
            Severity::Unsupported => c[2] += 1,
            Severity::FivemOnly => c[3] += 1,
        }
    }
    c
}

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Supported => "SUPPORTED",
        Severity::Partial => "PARTIAL",
        Severity::Unsupported => "UNSUPPORTED",
        Severity::FivemOnly => "FIVEM-ONLY",
    }
}

/// Patterns the scanner knows. Each is (needle, severity, category, advice).
/// Severities match the adapter coverage in docs/compatibility/FIVEM_API_MATRIX.md.
const PATTERNS: &[(&str, Severity, Category, &str)] = &[
    // ESX
    ("ESX.GetPlayerFromId", Severity::Supported, Category::EsxCall, "routed through acl-esx"),
    ("ESX.GetPlayerFromIdentifier", Severity::Supported, Category::EsxCall, "routed through acl-esx"),
    ("ESX.RegisterServerCallback", Severity::FivemOnly, Category::EsxCall, "use the native typed RPC system"),
    ("esx:registerUsableItem", Severity::FivemOnly, Category::EsxCall, "use the native item-use hook"),
    ("ESX.Trace", Severity::FivemOnly, Category::EsxCall, "no equivalent; use structured logging"),
    // QBCore
    ("QBCore.Functions.GetPlayer", Severity::Supported, Category::QbcoreCall, "routed through acl-qbcore"),
    ("QBCore.Functions.GetPlayerByCitizenId", Severity::Supported, Category::QbcoreCall, "routed through acl-qbcore"),
    ("QBCore.Functions.CreateUseableItem", Severity::FivemOnly, Category::QbcoreCall, "use the native item-use hook"),
    (
        "QBCore.Functions.CreateBill",
        Severity::Unsupported,
        Category::QbcoreCall,
        "no native billing equivalent yet; port manually",
    ),
    // Qbox
    ("Qbox.", Severity::Unsupported, Category::QboxCall, "qbox namespace; check acl-qbox matrix"),
    // Database
    ("oxmysql", Severity::Unsupported, Category::Database, "use the native Aldivine database API"),
    ("mysql-async", Severity::Unsupported, Category::Database, "use the native Aldivine database API"),
    ("MySQL.async", Severity::Unsupported, Category::Database, "use the native Aldivine database API"),
    // FiveM globals/natives with no native equivalent
    (
        "Citizen.CreateThread",
        Severity::FivemOnly,
        Category::FivemGlobal,
        "Aldivine has no thread primitive for scripts; use async handlers",
    ),
    ("Citizen.Wait", Severity::FivemOnly, Category::FivemGlobal, "no blocking wait; use timers/async"),
    (
        "RegisterNetEvent",
        Severity::Partial,
        Category::FivemGlobal,
        "event translation planned; register via Aldivine.Events",
    ),
    ("TriggerServerEvent", Severity::Partial, Category::FivemGlobal, "use Aldivine.Events.emit (server-authoritative)"),
    ("TriggerClientEvent", Severity::Partial, Category::FivemGlobal, "server-side emission only"),
    ("RegisterCommand", Severity::Partial, Category::FivemGlobal, "use the native command system with permissions"),
    ("PerformHttpRequest", Severity::Partial, Category::FivemGlobal, "network.http capability required"),
    (
        "GetGameTimer",
        Severity::FivemOnly,
        Category::FivemNative,
        "client-side game native; not available server-side on Aldivine",
    ),
    ("fxmanifest.lua", Severity::FivemOnly, Category::Manifest, "must be converted to ald_manifest.toml"),
    ("SendNUIMessage", Severity::Partial, Category::Ui, "NUI bridge is validated; permissions apply"),
];

pub fn run(args: &[String]) {
    if args.is_empty() {
        eprintln!("usage: ald migrate <resource-path>");
        exit(1);
    }
    let path = &args[0];
    let report = match scan(Path::new(path)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("cannot scan {path}: {e}");
            exit(1);
        }
    };
    for line in report.summary_lines() {
        println!("{line}");
    }
    if report.findings.iter().any(|f| f.severity == Severity::Unsupported || f.severity == Severity::FivemOnly) {
        exit(2);
    }
}

fn scan(root: &Path) -> std::io::Result<Report> {
    let mut findings = Vec::new();
    let mut files_scanned = 0usize;
    let mut flavor: Option<String> = None;

    let mut files: Vec<std::path::PathBuf> = Vec::new();
    collect_scripts(root, &mut files)?;

    for file in &files {
        let text = match fs::read_to_string(file) {
            Ok(t) => t,
            Err(_) => continue,
        };
        files_scanned += 1;
        let rel = file.strip_prefix(root).unwrap_or(file).display().to_string();

        for (needle, sev, cat, advice) in PATTERNS {
            for (idx, _occurrence) in text.match_indices(needle) {
                let line_no = text[..idx].matches('\n').count() + 1;
                findings.push(Finding {
                    severity: *sev,
                    location: format!("{rel}:{line_no}"),
                    detail: format!("{} '{}' — {}", cat.as_str(), needle, advice),
                });
            }
        }

        // Flavor detection: the first framework detected wins, but a mixed
        // resource reports both.
        let detected = detect_flavor(&text);
        if let Some(f) = detected {
            match &mut flavor {
                Some(existing) if !existing.contains(&f) => {
                    *existing = format!("{existing} + {f}");
                }
                None => flavor = Some(f),
                _ => {}
            }
        }
    }

    // A resource with no manifest is a finding of its own.
    if !root.join("ald_manifest.toml").exists() {
        findings.push(Finding {
            severity: if root.join("fxmanifest.lua").exists() { Severity::FivemOnly } else { Severity::Unsupported },
            location: ".".into(),
            detail: "no ald_manifest.toml; resource cannot be loaded by Aldivine".into(),
        });
    }

    // Stable order for reproducible reports.
    findings.sort_by(|a, b| a.location.cmp(&b.location).then(a.detail.cmp(&b.detail)));

    Ok(Report { resource_path: root.display().to_string(), flavor, files_scanned, findings })
}

fn collect_scripts(root: &Path, out: &mut Vec<std::path::PathBuf>) -> std::io::Result<()> {
    if root.is_file() {
        if is_script(root) {
            out.push(root.to_path_buf());
        }
        return Ok(());
    }
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let p = entry.path();
        if p.is_dir() {
            // Skip dependency vendoring; it would flood the report.
            if p.file_name().and_then(|n| n.to_str()) == Some("node_modules") {
                continue;
            }
            collect_scripts(&p, out)?;
        } else if is_script(&p) {
            out.push(p);
        }
    }
    Ok(())
}

fn is_script(p: &Path) -> bool {
    matches!(p.extension().and_then(|e| e.to_str()), Some("lua" | "js" | "ts" | "json"))
}

fn detect_flavor(text: &str) -> Option<String> {
    if text.contains("ESX.") || text.contains("esx:") {
        return Some("esx".into());
    }
    if text.contains("QBCore.") {
        return Some("qbcore".into());
    }
    if text.contains("Qbox.") || text.contains("qbox") {
        return Some("qbox".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn difficulty_scales_with_unsupported() {
        let r = Report { resource_path: "x".into(), flavor: None, files_scanned: 0, findings: vec![] };
        assert_eq!(r.difficulty(), Difficulty::Trivial);

        let mut r = r;
        r.findings = vec![finding(Severity::Partial)];
        assert_eq!(r.difficulty(), Difficulty::Easy);

        r.findings = (0..3).map(|_| finding(Severity::Unsupported)).collect();
        assert_eq!(r.difficulty(), Difficulty::Moderate);

        r.findings = (0..10).map(|_| finding(Severity::FivemOnly)).collect();
        assert_eq!(r.difficulty(), Difficulty::Hard);

        r.findings = (0..30).map(|_| finding(Severity::Unsupported)).collect();
        assert_eq!(r.difficulty(), Difficulty::VeryHard);
    }

    fn finding(s: Severity) -> Finding {
        Finding { severity: s, location: "f:1".into(), detail: "d".into() }
    }

    #[test]
    fn flavor_detection() {
        assert_eq!(detect_flavor("local x = ESX.GetPlayerFromId(1)"), Some("esx".into()));
        assert_eq!(detect_flavor("QBCore.Functions.GetPlayer(1)"), Some("qbcore".into()));
        assert_eq!(detect_flavor("print('hi')"), None);
    }

    #[test]
    fn script_filter() {
        assert!(is_script(Path::new("init.lua")));
        assert!(is_script(Path::new("init.ts")));
        assert!(!is_script(Path::new("readme.md")));
    }
}
