//! Lifecycle supervisor — owns the LifecycleManager as a single writer.
//!
//! Resource state transitions are funneled through one task so that
//! LifecycleManager mutation never crosses an await point under a lock and
//! consoles/Aegis/hot-reload can request transitions concurrently.

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::state::ServerState;
use crate::LifecycleCommand;

macro_rules! fail_point {
    ($self:expr, $name:expr, $state:expr) => {{
        let mut l = $self.state.lifecycle().lock().await;
        l.set($name, $state);
    }};
}

pub struct LifecycleSupervisor {
    state: Arc<ServerState>,
}

impl LifecycleSupervisor {
    pub fn new(state: Arc<ServerState>) -> Self {
        LifecycleSupervisor { state }
    }

    /// Process lifecycle commands until the channel closes.
    pub async fn run(self, mut rx: mpsc::UnboundedReceiver<LifecycleCommand>) {
        while let Some(cmd) = rx.recv().await {
            if *self.state.shutdown_rx().borrow() {
                tracing::debug!("shutdown active; ignoring lifecycle command");
                continue;
            }
            match cmd {
                LifecycleCommand::Start(name) => self.transition_start(&name).await,
                LifecycleCommand::Stop(name) => self.transition_stop(&name).await,
                LifecycleCommand::Restart(name) => self.transition_restart(&name).await,
                LifecycleCommand::Reload(name) => self.transition_restart(&name).await,
            }
        }
    }

    async fn transition_start(&self, name: &str) {
        {
            let l = self.state.lifecycle().lock().await;
            if !l.can_start(name) {
                tracing::warn!(resource = name, "start rejected: not in Stopped/Failed");
                return;
            }
        }
        let resource_dir = self.state.config().resources.directory.clone();
        fail_point!(self, name, ald_resource::ResourceState::Validating);
        let manifest = match load_manifest(name, &resource_dir) {
            Ok(m) => m,
            Err(e) => return self.fail(name, &e).await,
        };
        fail_point!(self, name, ald_resource::ResourceState::DependencyResolution);
        if let Err(e) = check_dependencies(&manifest, &resource_dir) {
            return self.fail(name, &e).await;
        }
        {
            let mut l = self.state.lifecycle().lock().await;
            l.set(name, ald_resource::ResourceState::Starting);
        }
        fail_point!(self, name, ald_resource::ResourceState::ScriptHostStarting);
        let handles = match start_resource_scripts(name, &resource_dir) {
            Ok(h) => h,
            Err(e) => return self.fail(name, &e).await,
        };
        if !manifest.database_migrations.is_empty() {
            return self.fail(name, "database migrations declared but no migration runner is wired").await;
        }
        {
            let mut l = self.state.lifecycle().lock().await;
            l.set(name, ald_resource::ResourceState::MigrationsReady);
            l.set(name, ald_resource::ResourceState::HealthChecking);
            match l.mark_healthy(name, handles) {
                Ok(()) => {
                    self.state.metrics().incr("resources.started", 1);
                    tracing::info!(resource = name, handles, "resource healthy");
                }
                Err(e) => {
                    let msg = e.to_string();
                    drop(l);
                    self.fail(name, &msg).await;
                }
            }
        }
    }

    async fn fail(&self, name: &str, reason: &str) {
        let mut l = self.state.lifecycle().lock().await;
        l.set(name, ald_resource::ResourceState::Failed);
        self.state.metrics().incr("resources.start_failures", 1);
        tracing::error!(resource = name, reason, "resource failed to start");
    }

    async fn transition_stop(&self, name: &str) {
        let mut l = self.state.lifecycle().lock().await;
        l.set(name, ald_resource::ResourceState::Stopping);
        for _ in 0..4096 {
            l.release_handle(name);
        }
        match l.stop(name) {
            Ok(()) => {
                self.state.metrics().incr("resources.stopped", 1);
                tracing::info!(resource = name, "resource stopped");
            }
            Err(e) => {
                l.set(name, ald_resource::ResourceState::Failed);
                self.state.metrics().incr("resources.stop_failures", 1);
                tracing::error!(resource = name, error = %e, "resource leaked handles on stop");
            }
        }
    }

    async fn transition_restart(&self, name: &str) {
        self.transition_stop(name).await;
        self.transition_start(name).await;
    }
}

fn resource_root(name: &str, configured: &str) -> Option<std::path::PathBuf> {
    for base in [configured, "base-resources", "../../base-resources"] {
        let root = std::path::PathBuf::from(base).join(name);
        if root.join("ald_manifest.toml").is_file() {
            return Some(root);
        }
    }
    None
}

// The Lua script host is an isolated native boundary: it compiles only with
// `--features lua`, so the default `ald-server` closure stays free of C code
// and `ald dependencies audit-native` keeps passing on the default build.
#[cfg(feature = "lua")]
fn start_resource_scripts(name: &str, configured_dir: &str) -> Result<usize, String> {
    let root = resource_root(name, configured_dir).ok_or_else(|| format!("manifest not found for '{name}'"))?;
    let text = std::fs::read_to_string(root.join("ald_manifest.toml")).map_err(|e| format!("read manifest: {e}"))?;
    let manifest = ald_resource::Manifest::parse(&text).map_err(|e| format!("parse manifest: {e}"))?;
    if manifest.server_scripts.is_empty() {
        return Ok(0);
    }
    let rt = ald_script_lua::LuaRuntime::new().map_err(|e| format!("lua init: {e}"))?;
    rt.register_api_version("v1").map_err(|e| format!("lua api: {e}"))?;
    let lua = rt.lua();
    let globals = lua.globals();
    let events = lua.create_table().map_err(|e| format!("lua events table: {e}"))?;
    let on_fn = lua
        .create_function(|_, (_event, _cb): (String, mlua::Value)| Ok(()))
        .map_err(|e| format!("lua on bind: {e}"))?;
    let emit_fn = lua
        .create_function(|_, (_event, _payload): (String, mlua::Value)| Ok(()))
        .map_err(|e| format!("lua emit bind: {e}"))?;
    events.set("on", on_fn).map_err(|e| format!("lua events.on: {e}"))?;
    events.set("emit", emit_fn).map_err(|e| format!("lua events.emit: {e}"))?;
    let aldivine = lua.create_table().map_err(|e| format!("lua aldivine table: {e}"))?;
    aldivine.set("Events", events).map_err(|e| format!("lua aldivine.events: {e}"))?;
    globals.set("Aldivine", aldivine).map_err(|e| format!("lua global: {e}"))?;
    let mut loaded = 0usize;
    for script in &manifest.server_scripts {
        let path = root.join(script);
        let src = std::fs::read_to_string(&path).map_err(|e| format!("read {}: {e}", path.display()))?;
        rt.exec(&src).map_err(|e| format!("exec {}: {e}", path.display()))?;
        loaded += 1;
    }
    Ok(loaded)
}

#[cfg(not(feature = "lua"))]
fn start_resource_scripts(name: &str, _configured_dir: &str) -> Result<usize, String> {
    Err(format!("resource '{name}' needs the lua script host (rebuild with --features lua)"))
}

fn load_manifest(name: &str, configured_dir: &str) -> Result<ald_resource::Manifest, String> {
    let root = resource_root(name, configured_dir).ok_or_else(|| format!("manifest not found for '{name}'"))?;
    let text = std::fs::read_to_string(root.join("ald_manifest.toml")).map_err(|e| format!("read manifest: {e}"))?;
    ald_resource::Manifest::parse(&text).map_err(|e| format!("parse manifest: {e}"))
}

fn check_dependencies(manifest: &ald_resource::Manifest, configured_dir: &str) -> Result<(), String> {
    for dep in &manifest.dependencies {
        if resource_root(&dep.name, configured_dir).is_none() {
            return Err(format!("resource '{}' missing dependency '{}'", manifest.name, dep.name));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "lua")]
    use ald_config::Config;

    #[cfg(feature = "lua")]
    fn dummy() -> (Arc<ServerState>, mpsc::UnboundedSender<LifecycleCommand>, LifecycleSupervisor) {
        let (shutdown_tx, _rx) = tokio::sync::watch::channel(false);
        let state = Arc::new(ServerState::new(Config::default(), shutdown_tx));
        let (tx, rx2) = mpsc::unbounded_channel();
        let supervisor = LifecycleSupervisor::new(Arc::clone(&state));
        // Drop rx2 to keep the channel quiescent; supervisor gets its own.
        drop(rx2);
        (state, tx, supervisor)
    }

    #[tokio::test]
    #[cfg(feature = "lua")]
    async fn start_stop_roundtrip() {
        let (state, _tx, supervisor) = dummy();
        let (tx2, rx2) = mpsc::unbounded_channel();
        let handle = tokio::spawn(async move { supervisor.run(rx2).await });
        tx2.send(LifecycleCommand::Start("spawn".into())).unwrap();
        // Allow the supervisor task to process.
        for _ in 0..40 {
            if state.running_count().await == 1 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(state.running_count().await, 1);

        tx2.send(LifecycleCommand::Stop("spawn".into())).unwrap();
        for _ in 0..20 {
            if state.running_count().await == 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert_eq!(state.running_count().await, 0);
        drop(tx2);
        handle.await.unwrap();
    }

    #[test]
    #[cfg(feature = "lua")]
    fn spawn_scripts_load() {
        let n = start_resource_scripts("spawn", "resources").expect("spawn scripts must load");
        assert!(n >= 1);
    }

    #[test]
    #[cfg(not(feature = "lua"))]
    fn start_requires_lua_feature() {
        let err = start_resource_scripts("spawn", "resources").unwrap_err();
        assert!(err.contains("--features lua"));
    }

    #[test]
    fn missing_resource_fails_honestly() {
        assert!(start_resource_scripts("no-such-resource-xyz", "resources").is_err());
    }
}
