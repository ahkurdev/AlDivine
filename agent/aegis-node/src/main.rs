use aegis_node::{AegisNodeSupervisor, InstanceConfig};
use std::env;

fn print_banner() {
    println!("=====================================================");
    println!("           AEGIS NODE AGENT — SUPERVISOR             ");
    println!("        Aldivine Server Process Control              ");
    println!("=====================================================");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("aegis-node v0.1.0-RC1");
        return;
    }

    print_banner();

    let mut supervisor = AegisNodeSupervisor::new(1000);
    let config = InstanceConfig::default();
    println!("Registering instance: {}", config.instance_id);
    println!("Target binary: {}", config.binary_path);
    println!("Config path: {}", config.config_path);
    println!("Watchdog timeout: {} ms", config.watchdog_timeout_ms);

    if let Err(e) = supervisor.register_instance(config) {
        eprintln!("Failed to register instance: {}", e);
        return;
    }

    println!("Starting instance under supervisor...");
    match supervisor.start_instance("default-inst", 0) {
        Ok(()) => {
            println!("Instance 'default-inst' successfully registered and started.");
            println!("Watchdog active. Status: Running.");
        }
        Err(e) => {
            eprintln!("Failed to start instance: {}", e);
        }
    }

    println!();
    println!("Supervisor listening on 127.0.0.1:40120 (Aegis Control Plane).");
    println!("Press Ctrl+C to terminate supervisor and stop child instances.");
}
