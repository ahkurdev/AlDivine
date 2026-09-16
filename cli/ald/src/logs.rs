//! `ald logs` command: inspect, summarize, and triage runtime logs and crash dumps.

use ald_core::inspect_log_file;
use std::fs;
use std::path::{Path, PathBuf};

pub fn run(args: &[String]) {
    let logs_dir = Path::new("logs");

    if !logs_dir.exists() {
        println!("No 'logs/' directory found. Run ald-server, novagate, or aegis-node first.");
        return;
    }

    let show_only_errors = args.iter().any(|a| a == "--errors" || a == "-e");
    let summary_only = args.iter().any(|a| a == "--summary" || a == "-s");
    let target = args.iter().find(|a| !a.starts_with('-')).map(|s| s.as_str()).unwrap_or("all");

    println!("=====================================================");
    println!("           ALDIVINE LOG DIAGNOSTICS                  ");
    println!("=====================================================");

    let log_files: Vec<PathBuf> = match target {
        "server" => vec![logs_dir.join("server.log")],
        "launcher" | "novagate" => vec![logs_dir.join("launcher.log")],
        "aegis" => vec![logs_dir.join("aegis-node.log")],
        "crash" => vec![logs_dir.join("crash.log")],
        _ => {
            // Find all .log files in logs/
            let mut list = Vec::new();
            if let Ok(entries) = fs::read_dir(logs_dir) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().map(|e| e == "log").unwrap_or(false) {
                        list.push(p);
                    }
                }
            }
            list.sort();
            list
        }
    };

    if log_files.is_empty() {
        println!("No matching log files found in 'logs/'.");
        return;
    }

    let mut total_errors = 0;
    let mut total_warnings = 0;

    for path in &log_files {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        if !path.exists() {
            println!("Log file '{}': Not yet created (no events).", name);
            continue;
        }

        match inspect_log_file(path) {
            Ok(summary) => {
                total_errors += summary.error_count + summary.fatal_count;
                total_warnings += summary.warn_count;

                println!(
                    "File: {:<18} | Lines: {:<5} | Info: {:<5} | Warn: {:<4} | Error: {:<4}",
                    name,
                    summary.total_lines,
                    summary.info_count,
                    summary.warn_count,
                    summary.error_count + summary.fatal_count
                );

                if (show_only_errors || !summary_only) && !summary.recent_errors.is_empty() {
                    println!("  --- Recent Errors in {} ---", name);
                    for err in &summary.recent_errors {
                        println!("  {}", err);
                    }
                }
            }
            Err(e) => {
                eprintln!("Error reading {}: {}", name, e);
            }
        }
    }

    println!("-----------------------------------------------------");
    if total_errors > 0 {
        println!("STATUS: [ISSUES DETECTED] Found {} error(s) and {} warning(s).", total_errors, total_warnings);
        println!("Tip: Run 'ald report create' to bundle full logs for technical support.");
    } else {
        println!("STATUS: [HEALTHY] 0 errors detected across scanned log files.");
    }
}
