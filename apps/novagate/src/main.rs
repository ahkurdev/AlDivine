use novagate_core::{detect_gta_installation, DetectionStatus};
use std::env;

fn print_banner() {
    println!("=====================================================");
    println!("           NOVAGATE LAUNCHER — ALDIVINE              ");
    println!("        Next-Generation GTA V Multiplayer            ");
    println!("=====================================================");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("NovaGate Launcher v0.1.0-RC1");
        return;
    }

    print_banner();

    if let Some(pos) = args.iter().position(|a| a == "--connect") {
        if let Some(endpoint) = args.get(pos + 1) {
            println!("Connecting to Aldivine Server: {}", endpoint);
            println!("Detecting local GTA V installation...");
            let result = detect_gta_installation(&[]);
            match result.status {
                DetectionStatus::InstallationDetected => {
                    println!(
                        "GTA V found: {:?} (distribution: {})",
                        result.install_path.unwrap_or_default(),
                        result.distribution.as_str()
                    );
                    println!("Launching Astryn Client bootstrap -> AstraNet handshake...");
                }
                _ => {
                    println!("Error: GTA V installation not detected on this machine.");
                    println!("Please ensure GTA V is installed via Steam, Epic Games, or Rockstar Launcher.");
                }
            }
            return;
        }
    }

    println!("Scanning host for GTA V installation...");
    let result = detect_gta_installation(&[]);
    match result.status {
        DetectionStatus::InstallationDetected => {
            println!("Status: [OK] GTA V installation detected at: {:?}", result.install_path.unwrap_or_default());
            println!("Distribution: {}", result.distribution.as_str());
            println!("Entitlement check: Ready for authentication.");
            println!();
            println!("Usage:");
            println!("  novagate --connect <ip:port>   Connect directly to an Aldivine server");
            println!("  novagate --detect              Re-run GTA V detection check");
        }
        _ => {
            println!("Status: [NOT FOUND] No legitimate GTA V installation found on this system.");
            println!("Supported distributions: Steam, Epic Games Store, Rockstar Games Launcher.");
            println!("Candidates evaluated: {}", result.candidates.len());
            println!();
            println!("Usage:");
            println!("  novagate --connect <ip:port>   Attempt connection when GTA V path is set");
        }
    }
}
