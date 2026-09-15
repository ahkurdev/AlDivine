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
    pub async fn run(&self) {
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
        let (_tx, rx) = tokio::sync::watch::channel(false);
        Arc::new(ServerState::new(cfg, rx))
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
}
