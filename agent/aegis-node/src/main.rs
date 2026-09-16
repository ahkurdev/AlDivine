use aegis_node::{AegisNodeSupervisor, InstanceConfig};
use ald_core::{install_crash_handler, DiagnosticLogger};
use std::env;

fn main() {
    install_crash_handler("logs", "aegis-node");
    let logger = DiagnosticLogger::new("logs", "aegis-node.log");

    let args: Vec<String> = env::args().collect();

    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("aegis-node v0.1.0-RC1");
        return;
    }

    logger.info("aegis", "=====================================================");
    logger.info("aegis", "           AEGIS NODE AGENT — SUPERVISOR             ");
    logger.info("aegis", "        Aldivine Server Process Control              ");
    logger.info("aegis", "=====================================================");

    let mut supervisor = AegisNodeSupervisor::new(1000);
    let config = InstanceConfig::default();
    logger.info("config", &format!("Registering instance: {}", config.instance_id));
    logger.info("config", &format!("Target binary: {}", config.binary_path));
    logger.info("config", &format!("Config path: {}", config.config_path));
    logger.info("watchdog", &format!("Watchdog timeout: {} ms", config.watchdog_timeout_ms));

    if let Err(e) = supervisor.register_instance(config) {
        logger.error("config", &format!("Failed to register instance: {}", e));
        return;
    }

    logger.info("runtime", "Starting instance under supervisor...");
    match supervisor.start_instance("default-inst", 0) {
        Ok(()) => {
            logger.info("runtime", "Instance 'default-inst' successfully registered and started.");
            logger.info("runtime", "Watchdog active. Status: Running.");
        }
        Err(e) => {
            logger.error("runtime", &format!("Failed to start instance: {}", e));
        }
    }

    println!();
    println!("Supervisor listening on 127.0.0.1:40120 (Aegis Control Plane).");
    println!("Press Ctrl+C to terminate supervisor and stop child instances.");
}
