//! Shared server state: config, resource lifecycle, telemetry, shutdown.

use std::sync::Arc;
use std::time::Instant;

use ald_config::Config;
use ald_resource::LifecycleManager;
use ald_telemetry::MetricStore;
use tokio::sync::watch;

use crate::console::ConsoleCommand;

/// Snapshot of live server state. Shared via Arc across tasks.
pub struct ServerState {
    config: Config,
    started_at: Instant,
    /// Resource lifecycle states. Guarded: mutated only through the
    /// lifecycle command channel to keep state transitions single-writer.
    lifecycle: Arc<tokio::sync::Mutex<LifecycleManager>>,
    metrics: Arc<MetricStore>,
    shutdown_tx: watch::Sender<bool>,
    shutdown_rx: watch::Receiver<bool>,
    console_tx: tokio::sync::mpsc::UnboundedSender<ConsoleCommand>,
    /// Held here (not dropped) so Aegis/operator tooling can inject commands
    /// via `console_tx()`; the console task takes it once via
    /// `take_console_rx()` and serves it alongside stdin.
    console_rx: std::sync::Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<ConsoleCommand>>>,
}

impl ServerState {
    pub fn new(config: Config, shutdown_tx: watch::Sender<bool>) -> Self {
        let shutdown_rx = shutdown_tx.subscribe();
        let (console_tx, console_rx) = tokio::sync::mpsc::unbounded_channel();
        ServerState {
            config,
            started_at: Instant::now(),
            lifecycle: Arc::new(tokio::sync::Mutex::new(LifecycleManager::new())),
            metrics: Arc::new(MetricStore::new()),
            shutdown_tx,
            shutdown_rx,
            console_tx,
            console_rx: std::sync::Mutex::new(Some(console_rx)),
        }
    }

    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn metrics(&self) -> &Arc<MetricStore> {
        &self.metrics
    }
    pub fn uptime(&self) -> std::time::Duration {
        self.started_at.elapsed()
    }
    pub fn shutdown_rx(&self) -> watch::Receiver<bool> {
        self.shutdown_rx.clone()
    }
    /// Signal shutdown to every task watching the flag. Idempotent best
    /// effort: silently ignored if no task is watching yet.
    pub fn request_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
    pub fn console_tx(&self) -> tokio::sync::mpsc::UnboundedSender<ConsoleCommand> {
        self.console_tx.clone()
    }
    /// Take the remote-command receiver. `None` if the console task (or a
    /// previous caller) already took it — there is exactly one server.
    pub fn take_console_rx(&self) -> Option<tokio::sync::mpsc::UnboundedReceiver<ConsoleCommand>> {
        self.console_rx.lock().ok()?.take()
    }
    pub fn lifecycle(&self) -> &Arc<tokio::sync::Mutex<LifecycleManager>> {
        &self.lifecycle
    }

    /// Number of resources currently in the Running state.
    pub async fn running_count(&self) -> usize {
        let l = self.lifecycle.lock().await;
        l.running_count()
    }

    /// Periodic telemetry sampling. Bounded: exits once shutdown is signalled.
    pub async fn telemetry_loop(self: Arc<Self>) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
        interval.tick().await; // discard immediate first tick
        loop {
            interval.tick().await;
            if *self.shutdown_rx.borrow() {
                break;
            }
            self.metrics.gauge("server.uptime_s", self.uptime().as_secs() as f64);
            self.metrics.gauge("resources.running", self.running_count().await as f64);
        }
    }

    /// Network receive loop: decode, firewall-check, dispatch.
    /// Errors are logged and counted, never fatal — a single bad peer must
    /// not take the server down.
    pub async fn network_loop(self: Arc<Self>, listener: &ald_network::UdpTransport) {
        let mut rx = self.shutdown_rx();
        loop {
            if *rx.borrow() {
                break;
            }
            tokio::select! {
                res = listener.recv() => {
                    match res {
                        Ok((packet, peer)) => {
                            self.metrics.incr("network.packets", 1);
                            self.metrics.incr("network.bytes", packet.payload.len() as u64);
                            tracing::debug!(%peer, ch = ?packet.header.channel, "packet received");
                        }
                        Err(e) => {
                            self.metrics.incr("network.errors", 1);
                            tracing::warn!(error = %e, "udp recv error");
                        }
                    }
                }
                _ = rx.changed() => break,
            }
        }
    }
}
