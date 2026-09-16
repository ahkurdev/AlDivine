use ald_core::{install_crash_handler, DiagnosticLogger};
use novagate_core::{detect_gta_installation, DetectionStatus};
use std::env;

fn main() {
    install_crash_handler("logs", "novagate");
    let logger = DiagnosticLogger::new("logs", "launcher.log");

    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("NovaGate Launcher v0.1.0-RC1");
        return;
    }

    logger.info("launcher", "=====================================================");
    logger.info("launcher", "           NOVAGATE LAUNCHER — ALDIVINE              ");
    logger.info("launcher", "        Next-Generation GTA V Multiplayer            ");
    logger.info("launcher", "=====================================================");

    if let Some(pos) = args.iter().position(|a| a == "--connect") {
        if let Some(endpoint) = args.get(pos + 1) {
            logger.info("connect", &format!("Connecting to Aldivine Server: {}", endpoint));
            logger.info("detect", "Detecting local GTA V installation...");
            let result = detect_gta_installation(&[]);
            match result.status {
                DetectionStatus::InstallationDetected => {
                    let path_str = format!("{:?}", result.install_path.unwrap_or_default());
                    logger.info(
                        "detect",
                        &format!("GTA V found: {} (distribution: {})", path_str, result.distribution.as_str()),
                    );
                    logger.info("bootstrap", "Launching Astryn Client bootstrap -> AstraNet handshake...");
                }
                _ => {
                    logger.error("detect", "GTA V installation not detected on this machine.");
                    logger.error(
                        "detect",
                        "Please ensure GTA V is installed via Steam, Epic Games, or Rockstar Launcher.",
                    );
                }
            }
            return;
        }
    }

    logger.info("detect", "Scanning host for GTA V installation...");
    let result = detect_gta_installation(&[]);
    match result.status {
        DetectionStatus::InstallationDetected => {
            let path_str = format!("{:?}", result.install_path.unwrap_or_default());
            logger.info("detect", &format!("Status: [OK] GTA V installation detected at: {}", path_str));
            logger.info("detect", &format!("Distribution: {}", result.distribution.as_str()));
            logger.info("entitlement", "Entitlement check: Ready for authentication.");
            println!();
            println!("Usage:");
            println!("  novagate --connect <ip:port>   Connect directly to an Aldivine server");
            println!("  novagate --detect              Re-run GTA V detection check");
        }
        _ => {
            logger.warn("detect", "Status: [NOT FOUND] No legitimate GTA V installation found on this system.");
            logger.info("detect", "Supported distributions: Steam, Epic Games Store, Rockstar Games Launcher.");
            logger.info("detect", &format!("Candidates evaluated: {}", result.candidates.len()));
            for candidate in &result.candidates {
                logger.info(
                    "detect",
                    &format!(
                        "  Candidate: [{} - {}] path: {:?} confirmed: {}",
                        candidate.distribution.as_str(),
                        candidate.evidence,
                        candidate.path,
                        candidate.files_confirmed
                    ),
                );
            }
            println!();
            println!("Usage:");
            println!("  novagate --connect <ip:port>   Attempt connection when GTA V path is set");
        }
    }
}
