//! Structured console: live stream, filters, command execution with audit.

use std::sync::Arc;

use crate::state::ServerState;

/// Console commands accepted from stdin or Aegis. Every execution is audited.
#[derive(Debug, Clone)]
pub enum ConsoleCommand {
    Status,
    Stop,
    ResourceList,
    Raw(String),
}

/// Interactive console reading stdin lines.
pub struct Console {
    state: Arc<ServerState>,
}

impl Console {
    pub fn new(state: Arc<ServerState>) -> Self {
        Console { state }
    }

    /// Run the console until shutdown. stdin reads happen in a
    /// spawn_blocking task so the async runtime is never blocked.
    /// Remote commands injected via `ServerState::console_tx()` (Aegis,
    /// operator tooling) are served by a companion task.
    pub async fn run(&self) {
        if let Some(rx) = self.state.take_console_rx() {
            let remote = Arc::clone(&self.state);
            tokio::spawn(async move { serve_remote(remote, rx).await });
        }
        loop {
            if *self.state.shutdown_rx().borrow() {
                break;
            }
            // Blocking stdin read off the async runtime.
            let line = tokio::task::spawn_blocking(read_line).await;
            match line {
                Ok(Some(input)) => self.execute(&parse(input)).await,
                Ok(None) => break, // EOF
                Err(e) => {
                    tracing::warn!(error = %e, "console reader task failed");
                    break;
                }
            }
        }
    }

    /// Execute a command against server state. Every execution is logged
    /// with enough detail for the audit log (who/when/what).
    pub async fn execute(&self, cmd: &ConsoleCommand) {
        match cmd {
            ConsoleCommand::Status => {
                let lifecycle = self.state.lifecycle().lock().await;
                tracing::info!(
                    uptime_s = self.state.uptime().as_secs(),
                    running = lifecycle.running_count(),
                    known = lifecycle.resource_names().len(),
                    "console: status"
                );
            }
            ConsoleCommand::Stop => {
                tracing::info!("console: stop requested (operator)");
                self.state.request_shutdown();
            }
            ConsoleCommand::ResourceList => {
                let lifecycle = self.state.lifecycle().lock().await;
                for name in lifecycle.resource_names() {
                    tracing::info!(resource = %name, state = ?lifecycle.state(&name), "console: resource");
                }
            }
            ConsoleCommand::Raw(raw) => {
                // Audited but otherwise unhandled unknown commands.
                tracing::info!(command = %raw, "console: unknown command (audited)");
            }
        }
    }
}

/// Serve commands injected remotely (Aegis / operator tooling) until the
/// channel closes or shutdown fires. Stdin serving continues independently.
async fn serve_remote(state: Arc<ServerState>, mut rx: tokio::sync::mpsc::UnboundedReceiver<ConsoleCommand>) {
    let console = Console { state: Arc::clone(&state) };
    loop {
        tokio::select! {
            cmd = rx.recv() => match cmd {
                Some(c) => console.execute(&c).await,
                None => break,
            },
            _ = wait_shutdown(&state) => break,
        }
    }
}

/// Resolve once the shutdown flag is set (or the sender is dropped).
async fn wait_shutdown(state: &ServerState) {
    let mut rx = state.shutdown_rx();
    loop {
        if *rx.borrow() {
            break;
        }
        if rx.changed().await.is_err() {
            break;
        }
    }
}

fn parse(input: String) -> ConsoleCommand {
    match input.trim() {
        "status" => ConsoleCommand::Status,
        "stop" | "quit" | "exit" => ConsoleCommand::Stop,
        "resources" | "res" => ConsoleCommand::ResourceList,
        other => ConsoleCommand::Raw(other.to_string()),
    }
}

fn read_line() -> Option<String> {
    use std::io::BufRead;
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let mut buf = String::new();
    match reader.read_line(&mut buf) {
        Ok(0) => None,
        Ok(_) => Some(buf),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_config::Config;

    fn dummy_state() -> Arc<ServerState> {
        let cfg = Config::default();
        let (shutdown_tx, _rx) = tokio::sync::watch::channel(false);
        Arc::new(ServerState::new(cfg, shutdown_tx))
    }

    #[test]
    fn parse_commands() {
        assert!(matches!(parse("status\n".into()), ConsoleCommand::Status));
        assert!(matches!(parse("stop".into()), ConsoleCommand::Stop));
        assert!(matches!(parse("quit".into()), ConsoleCommand::Stop));
        assert!(matches!(parse("res".into()), ConsoleCommand::ResourceList));
        assert!(matches!(parse("hello".into()), ConsoleCommand::Raw(_)));
    }

    #[tokio::test]
    async fn execute_commands_no_panic() {
        let console = Console::new(dummy_state());
        console.execute(&ConsoleCommand::Status).await;
        console.execute(&ConsoleCommand::ResourceList).await;
        console.execute(&ConsoleCommand::Raw("say hi".into())).await;
        console.execute(&ConsoleCommand::Stop).await;
    }

    #[tokio::test]
    async fn running_count_starts_zero() {
        let state = dummy_state();
        assert_eq!(state.running_count().await, 0);
        assert_eq!(state.lifecycle().lock().await.resource_names(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn stop_command_requests_shutdown() {
        let state = dummy_state();
        assert!(!*state.shutdown_rx().borrow());
        Console::new(Arc::clone(&state)).execute(&ConsoleCommand::Stop).await;
        assert!(*state.shutdown_rx().borrow());
    }

    #[tokio::test]
    async fn remote_channel_serves_then_exits_on_shutdown() {
        let state = dummy_state();
        let tx = state.console_tx();
        let rx = state.take_console_rx().expect("receiver present");
        // Second take must fail: exactly one server of the channel.
        assert!(state.take_console_rx().is_none());
        let remote = Arc::clone(&state);
        let handle = tokio::spawn(async move { serve_remote(remote, rx).await });
        tx.send(ConsoleCommand::Status).unwrap();
        tx.send(ConsoleCommand::Raw("say hi".into())).unwrap();
        // Give the task a chance to drain; then shutdown must end it.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        assert!(!handle.is_finished());
        state.request_shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("serve_remote must exit on shutdown")
            .unwrap();
    }

    #[tokio::test]
    async fn remote_stop_command_shuts_down_and_ends_serve() {
        let state = dummy_state();
        let tx = state.console_tx();
        let rx = state.take_console_rx().expect("receiver present");
        let remote = Arc::clone(&state);
        let handle = tokio::spawn(async move { serve_remote(remote, rx).await });
        tx.send(ConsoleCommand::Stop).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), handle)
            .await
            .expect("serve_remote must exit after remote Stop")
            .unwrap();
        assert!(*state.shutdown_rx().borrow());
    }
}
