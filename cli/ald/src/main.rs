//! Aldivine developer CLI.
//!
//! Commands:
//!   ald new <name> <path> [--lua|--js|--ts]   Scaffold a resource
//!   ald resource validate <manifest>         Validate an ald_manifest.toml
//!   ald config validate <server.toml>        Validate a server config
//!   ald migrate <resource-path>              Migration report for a legacy resource
//!   ald dependencies audit-native            Native-dependency audit gate (workspace root)
//!   ald help

use std::fs;
use std::path::PathBuf;
use std::process::exit;

use ald_config::Config;
use ald_resource::Manifest;

mod audit;
mod logs;
mod migrate;
mod new;
mod report;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        print_help();
        exit(0);
    }
    let cmd = args[1].as_str();
    let rest = &args[2..];
    match cmd {
        "new" => new::run(rest),
        "migrate" => migrate::run(rest),
        "dependencies" => audit::run(rest),
        "report" => report::run(rest),
        "logs" => logs::run(rest),
        "resource" => cmd_resource(rest),
        "config" => cmd_config(rest),
        "help" | "--help" | "-h" => print_help(),
        other => {
            eprintln!("unknown command: {other}");
            print_help();
            exit(1);
        }
    }
}

fn print_help() {
    println!(
        "ald - Aldivine developer CLI\n\n\
Usage:\n\
  ald new <name> <path> [--lua|--js|--ts]  Scaffold a new resource\n\
  ald resource validate <manifest.toml>   Validate an ald_manifest.toml\n\
  ald config validate <server.toml>       Validate a server config\n\
  ald migrate <resource-path>             Migration report (no rewrite)\n\
  ald report create [root-dir] [--out <f>] Generate sanitized diagnostic bundle\n\
  ald logs [target] [--errors] [--summary] Inspect, filter, and summarize runtime logs\n\
  ald dependencies audit-native         Native-dep audit (workspace root)\n\
  ald help                                Show this help"
    );
}

fn cmd_resource(args: &[String]) {
    if args.first().map(|s| s.as_str()) != Some("validate") {
        eprintln!("usage: ald resource validate <manifest.toml>");
        exit(1);
    }
    let path = args.get(1).expect("manifest path required");
    let p = PathBuf::from(path);
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {}: {e}", p.display());
            exit(1);
        }
    };
    match Manifest::parse(&text) {
        Ok(m) => {
            println!("OK: resource '{}' v{} valid", m.name, m.version);
            if !m.capabilities.is_empty() {
                println!("capabilities: {}", m.capabilities.join(", "));
            }
        }
        Err(e) => {
            eprintln!("INVALID manifest: {e}");
            exit(1);
        }
    }
}

fn cmd_config(args: &[String]) {
    if args.first().map(|s| s.as_str()) != Some("validate") {
        eprintln!("usage: ald config validate <server.toml>");
        exit(1);
    }
    let path = args.get(1).expect("config path required");
    let p = PathBuf::from(path);
    let text = match fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("cannot read {}: {e}", p.display());
            exit(1);
        }
    };
    match Config::from_toml(&text) {
        Ok(_) => println!("OK: config '{}' valid", p.display()),
        Err(e) => {
            eprintln!("INVALID config: {e}");
            exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    // The CLI is exercised end-to-end by examples/ and the integration test
    // in tests/cli_e2e.rs; this module only guards module compilation.
}
