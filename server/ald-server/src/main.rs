use ald_config::Config;
use ald_server::Server;
use anyhow::Result;
use std::env;
use std::fs;
use std::path::Path;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            env::var("RUST_LOG").unwrap_or_else(|_| "info,ald_server=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    println!("=====================================================");
    println!("           ALDIVINE DEDICATED SERVER                 ");
    println!("        Next-Generation GTA V Multiplayer            ");
    println!("=====================================================");

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
        println!("Loading configuration from: {}", config_path);
        let s = fs::read_to_string(config_path)?;
        Config::from_toml(&s)?
    } else {
        println!("Configuration file '{}' not found, using default configuration.", config_path);
        Config::default()
    };

    println!("Server Name: {}", config.server.name);
    println!("Max Players: {}", config.server.max_players);
    println!("AstraNet Binding: {}", config.network.bind);
    println!("Starting Aldivine Server Runtime...");

    let server = Server::start(config).await?;
    server.run_until_stopped().await?;

    println!("Aldivine Server terminated gracefully.");
    Ok(())
}
