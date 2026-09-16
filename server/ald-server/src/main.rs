use ald_config::Config;
use ald_core::{install_crash_handler, DiagnosticLogger};
use ald_server::Server;
use anyhow::Result;
use std::env;
use std::fs;
use std::path::Path;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    // Install panic hook to write stack traces to logs/ald-server_crash.log
    install_crash_handler("logs", "ald-server");
    let logger = DiagnosticLogger::new("logs", "server.log");

    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            env::var("RUST_LOG").unwrap_or_else(|_| "info,ald_server=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    logger.info("runtime", "=====================================================");
    logger.info("runtime", "           ALDIVINE DEDICATED SERVER                 ");
    logger.info("runtime", "        Next-Generation GTA V Multiplayer            ");
    logger.info("runtime", "=====================================================");

    let args: Vec<String> = env::args().collect();
    if args.iter().any(|a| a == "--version" || a == "-v") {
        println!("ald-server v0.1.0-RC1");
        return Ok(());
    }

    let config_path = args
        .iter()
        .position(|a| a == "--config" || a == "-c")
        .and_then(|p| args.get(p + 1))
        .map(|s| s.as_str())
        .unwrap_or("server.toml");

    let config = if Path::new(config_path).exists() {
        logger.info("config", &format!("Loading configuration from: {}", config_path));
        let s = fs::read_to_string(config_path)?;
        Config::from_toml(&s)?
    } else {
        logger.warn("config", &format!("Configuration file '{}' not found, using default configuration.", config_path));
        Config::default()
    };

    logger.info("config", &format!("Server Name: {}", config.server.name));
    logger.info("config", &format!("Max Players: {}", config.server.max_players));
    logger.info("network", &format!("AstraNet Binding: {}", config.network.bind));
    logger.info("runtime", "Starting Aldivine Server Runtime...");

    let server = Server::start(config).await?;
    logger.info("runtime", "Server loop active. Waiting for incoming AstraNet traffic...");
    server.run_until_stopped().await?;

    logger.info("runtime", "Aldivine Server terminated gracefully.");
    Ok(())
}
