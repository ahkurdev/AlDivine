//! Aldivine Server Runtime.
//!
//! Headless, cross-platform (Linux first-class, Windows supported).
//! Owns: config validation, AstraNet listener, resource lifecycle, scheduler,
//! console, structured logging, metrics, graceful shutdown.
//!
//! Design note: LifecycleManager is owned by one dedicated task and mutated
//! only through a command channel. This keeps resource state transitions
//! single-writer (no mutex-held across await points) while consoles, Aegis,
//! and the hot-reload path can all request transitions safely.

use std::sync::Arc;
use std::time::Instant;

use ald_config::Config;
use anyhow::Result;
use tokio::sync::{mpsc, watch};

use crate::lifecycle::LifecycleSupervisor;

pub mod console;
pub mod lifecycle;
pub mod state;

/// Commands accepted by the lifecycle supervisor task.
#[derive(Debug, Clone)]
pub enum LifecycleCommand {
    Start(String),
    Stop(String),
    Restart(String),
    Reload(String),
}

/// Server runtime handle.
pub struct Server {
    state: Arc<state::ServerState>,
    shutdown_tx: watch::Sender<bool>,
    lifecycle_tx: mpsc::UnboundedSender<LifecycleCommand>,
}

impl Server {
    /// Validate configuration, initialize subsystems, and bind the listener.
    /// Startup fails on invalid security values — never silently ignored.
    pub async fn start(config: Config) -> Result<Self> {
        config.validate()?;

        tracing::info!(
            name = %config.server.name,
            max_players = config.server.max_players,
            bind = %config.network.bind,
            "starting Aldivine server runtime"
        );

        let started = Instant::now();
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        let state = Arc::new(state::ServerState::new(config, shutdown_tx.clone()));

        // Resource lifecycle supervisor (single writer for LifecycleManager).
        let (lifecycle_tx, lifecycle_rx) = mpsc::unbounded_channel::<LifecycleCommand>();
        let supervisor = LifecycleSupervisor::new(Arc::clone(&state));
        tokio::spawn(async move { supervisor.run(lifecycle_rx).await });

        // AstraNet listener.
        let listener = ald_network::UdpTransport::bind(&state.config().network.bind).await?;
        tracing::info!(addr = %state.config().network.bind, "AstraNet listening");

        // Telemetry + console tasks.
        let telemetry_state = Arc::clone(&state);
        tokio::spawn(async move { telemetry_state.telemetry_loop().await });
        let console_state = Arc::clone(&state);
        tokio::spawn(async move {
            let console = console::Console::new(console_state);
            console.run().await;
        });

        // Network receive loop.
        let net_state = Arc::clone(&state);
        tokio::spawn(async move {
            net_state.network_loop(&listener).await;
        });

        // Auto-discover and queue resource startup
        let search_dirs =
            [std::path::PathBuf::from(&state.config().resources.directory), std::path::PathBuf::from("base-resources")];
        for dir in &search_dirs {
            if dir.is_dir() {
                if let Ok(entries) = std::fs::read_dir(dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() && path.join("ald_manifest.toml").exists() {
                            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                                tracing::info!(resource = %name, "queueing resource startup");
                                let _ = lifecycle_tx.send(LifecycleCommand::Start(name.to_string()));
                            }
                        }
                    }
                }
            }
        }

        tracing::info!(elapsed_ms = started.elapsed().as_millis() as u64, "server ready");
        Ok(Server { state, shutdown_tx, lifecycle_tx })
    }

    /// Block until a shutdown signal is received (Ctrl-C / SIGTERM / stop cmd).
    pub async fn run_until_stopped(self) -> Result<()> {
        let mut rx = self.state.shutdown_rx();
        loop {
            tokio::select! {
                _ = shutdown_signal() => break,
                _ = rx.changed() => {
                    if *rx.borrow() { break; }
                }
            }
        }
        self.shutdown().await
    }

    /// Graceful shutdown: signal tasks, stop resources, close listener.
    pub async fn shutdown(&self) -> Result<()> {
        tracing::info!("graceful shutdown requested");
        let _ = self.shutdown_tx.send(true);
        // Ask the supervisor to stop every running resource.
        let names = {
            let l = self.state.lifecycle().lock().await;
            l.resource_names()
        };
        for name in names {
            let _ = self.lifecycle_tx.send(LifecycleCommand::Stop(name));
        }
        // Give resources a bounded moment to drain before returning.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        tracing::info!("shutdown complete");
        Ok(())
    }

    /// Live server state for Aegis / console / tests.
    pub fn state(&self) -> &Arc<state::ServerState> {
        &self.state
    }

    /// Request a resource lifecycle transition (async, fire-and-forget).
    pub fn lifecycle_tx(&self) -> &mpsc::UnboundedSender<LifecycleCommand> {
        &self.lifecycle_tx
    }
}

/// Wait for a platform shutdown signal. Unix SIGTERM where available,
/// otherwise Ctrl-C only.
async fn shutdown_signal() {
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = unix_terminate() => {}
    }
}

#[cfg(unix)]
async fn unix_terminate() {
    use tokio::signal::unix::{signal, SignalKind};
    match signal(SignalKind::terminate()) {
        Ok(mut s) => {
            s.recv().await;
        }
        // Cannot install handler: fall back to never signalling here; the
        // ctrl_c branch in shutdown_signal still drives shutdown.
        Err(_) => std::future::pending::<()>().await,
    }
}

#[cfg(not(unix))]
async fn unix_terminate() {
    std::future::pending::<()>().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn start_with_default_config_succeeds() {
        // Bind to an ephemeral port to avoid clashing with anything.
        let mut cfg = Config::default();
        cfg.network.bind = "127.0.0.1:0".to_string();
        let server = Server::start(cfg).await.expect("server must start");
        assert!(server.state().config().validate().is_ok());
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn shutdown_flag_propagates() {
        let mut cfg = Config::default();
        cfg.network.bind = "127.0.0.1:0".to_string();
        let server = Server::start(cfg).await.unwrap();
        assert!(!*server.state().shutdown_rx().borrow());
        server.shutdown().await.unwrap();
        assert!(*server.state().shutdown_rx().borrow());
    }
}
