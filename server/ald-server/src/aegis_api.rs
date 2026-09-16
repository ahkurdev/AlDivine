use std::sync::Arc;

use axum::{extract::Path, extract::State, http::StatusCode, routing::get, Json, Router};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::state::ServerState;
use crate::LifecycleCommand;

#[derive(Clone)]
pub struct AegisState {
    pub state: Arc<ServerState>,
    pub lifecycle_tx: mpsc::UnboundedSender<LifecycleCommand>,
}

#[derive(Serialize)]
pub struct Health {
    pub status: &'static str,
    pub version: &'static str,
    pub uptime_s: u64,
}

#[derive(Serialize)]
pub struct ServerStatus {
    pub name: String,
    pub max_players: u32,
    pub uptime_s: u64,
    pub running_resources: usize,
    pub known_resources: usize,
    pub sessions: usize,
    pub framework_players_online: usize,
}

#[derive(Serialize)]
pub struct ResourceInfo {
    pub name: String,
    pub state: String,
}

#[derive(Serialize)]
pub struct ActionResult {
    pub accepted: bool,
    pub resource: String,
    pub action: String,
}

#[derive(Serialize)]
pub struct SetupStatus {
    pub first_run: bool,
}

#[derive(Debug, Deserialize)]
pub struct SetupRequest {
    pub hostname: String,
    pub max_players: u32,
    pub bind: String,
}

#[derive(Serialize)]
pub struct SetupDone {
    pub written: String,
}

fn config_dir() -> std::path::PathBuf {
    std::env::var("ALDIVINE_CONFIG_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from("."))
}

fn setup_status_in(dir: &std::path::Path) -> bool {
    !dir.join("server.cfg").exists() && !dir.join("server.toml").exists()
}

fn build_server_cfg_text(req: &SetupRequest) -> Result<String, String> {
    if req.hostname.trim().is_empty() {
        return Err("hostname required".into());
    }
    if req.max_players == 0 || req.max_players > 4096 {
        return Err("max_players out of range 1..=4096".into());
    }
    if req.bind.trim().is_empty() {
        return Err("bind required".into());
    }
    let text = format!(
        "sv_hostname \"{}\"\nsv_maxclients {}\nendpoint_add_udp \"{}\"\n",
        req.hostname.replace('"', ""),
        req.max_players,
        req.bind
    );
    let opts = ald_servercfg::parse::ParseOptions::default();
    ald_servercfg::NormalizedServerConfig::from_root("setup", &text, &opts)
        .map_err(|e| format!("generated config invalid: {e}"))?;
    Ok(text)
}

fn complete_setup_in(dir: &std::path::Path, req: &SetupRequest) -> Result<std::path::PathBuf, String> {
    let text = build_server_cfg_text(req)?;
    let path = dir.join("server.cfg");
    std::fs::write(&path, text).map_err(|e| format!("write server.cfg: {e}"))?;
    Ok(path)
}

async fn setup_status() -> Json<SetupStatus> {
    Json(SetupStatus { first_run: setup_status_in(&config_dir()) })
}

async fn setup_complete(Json(req): Json<SetupRequest>) -> Result<Json<SetupDone>, (StatusCode, String)> {
    let path = complete_setup_in(&config_dir(), &req).map_err(|e| (StatusCode::BAD_REQUEST, e))?;
    Ok(Json(SetupDone { written: path.display().to_string() }))
}

pub fn router(aegis: AegisState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/resources", get(resources))
        .route("/resources/:name/:action", axum::routing::post(resource_action))
        .route("/setup/status", get(setup_status))
        .route("/setup/complete", axum::routing::post(setup_complete))
        .with_state(aegis)
}

async fn health(State(a): State<AegisState>) -> Json<Health> {
    Json(Health { status: "ok", version: env!("CARGO_PKG_VERSION"), uptime_s: a.state.uptime().as_secs() })
}

async fn status(State(a): State<AegisState>) -> Json<ServerStatus> {
    let (running, known) = {
        let l = a.state.lifecycle().lock().await;
        (l.running_count(), l.resource_names().len())
    };
    let sessions = { a.state.admission().lock().await.session_count() };
    let online = { a.state.framework_players().lock().await.online_count() };
    Json(ServerStatus {
        name: a.state.config().server.name.clone(),
        max_players: a.state.config().server.max_players,
        uptime_s: a.state.uptime().as_secs(),
        running_resources: running,
        known_resources: known,
        sessions,
        framework_players_online: online,
    })
}

async fn resources(State(a): State<AegisState>) -> Json<Vec<ResourceInfo>> {
    let l = a.state.lifecycle().lock().await;
    Json(
        l.resource_names().into_iter().map(|n| ResourceInfo { state: format!("{:?}", l.state(&n)), name: n }).collect(),
    )
}

async fn resource_action(
    State(a): State<AegisState>,
    Path((name, action)): Path<(String, String)>,
) -> Result<Json<ActionResult>, StatusCode> {
    let cmd = match action.as_str() {
        "start" => LifecycleCommand::Start(name.clone()),
        "stop" => LifecycleCommand::Stop(name.clone()),
        "restart" | "reload" => LifecycleCommand::Restart(name.clone()),
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    a.lifecycle_tx.send(cmd).map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    Ok(Json(ActionResult { accepted: true, resource: name, action }))
}

/// Serve the Aegis management API. Binds localhost only by default; passing a
/// non-loopback address is refused to avoid accidental public exposure.
pub async fn serve(
    state: Arc<ServerState>,
    lifecycle_tx: mpsc::UnboundedSender<LifecycleCommand>,
    bind: &str,
) -> anyhow::Result<()> {
    let host = bind.rsplit_once(':').map(|(h, _)| h).unwrap_or(bind);
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let ip: std::net::IpAddr = host.parse().map_err(|_| anyhow::anyhow!("bad aegis bind {bind}"))?;
    if !ip.is_loopback() {
        anyhow::bail!("aegis api refuses non-loopback bind {bind}");
    }
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, router(AegisState { state, lifecycle_tx })).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_config::Config;

    fn aegis() -> (AegisState, tokio::sync::watch::Sender<bool>, mpsc::UnboundedReceiver<LifecycleCommand>) {
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let state = Arc::new(ServerState::new(Config::default(), tx.clone()));
        let (ltx, lrx) = mpsc::unbounded_channel();
        (AegisState { state, lifecycle_tx: ltx }, tx, lrx)
    }

    #[tokio::test]
    async fn health_ok() {
        let (a, _t, _lrx) = aegis();
        let h = health(State(a)).await;
        assert_eq!(h.status, "ok");
    }

    #[tokio::test]
    async fn status_reflects_config() {
        let (a, _t, _lrx) = aegis();
        let s = status(State(a)).await;
        assert_eq!(s.max_players, Config::default().server.max_players);
        assert_eq!(s.running_resources, 0);
    }

    #[tokio::test]
    async fn bad_action_rejected() {
        let (a, _t, _lrx) = aegis();
        let r = resource_action(State(a), Path(("spawn".into(), "explode".into()))).await;
        assert!(matches!(r, Err(StatusCode::BAD_REQUEST)));
    }

    #[tokio::test]
    async fn start_action_accepted() {
        let (a, _t, _lrx) = aegis();
        let r = resource_action(State(a), Path(("spawn".into(), "start".into()))).await.unwrap();
        assert!(r.accepted);
    }

    #[tokio::test]
    async fn public_bind_refused() {
        let (a, _t, _lrx) = aegis();
        let res = serve(a.state, a.lifecycle_tx, "0.0.0.0:40120").await;
        assert!(res.is_err());
    }

    #[test]
    fn setup_builder_roundtrips() {
        let req = SetupRequest { hostname: "My Server".into(), max_players: 32, bind: "0.0.0.0:30120".into() };
        let text = build_server_cfg_text(&req).unwrap();
        assert!(text.contains("sv_maxclients 32"));
    }

    #[test]
    fn setup_builder_rejects_bad_input() {
        assert!(
            build_server_cfg_text(&SetupRequest { hostname: "".into(), max_players: 10, bind: "x".into() }).is_err()
        );
        assert!(
            build_server_cfg_text(&SetupRequest { hostname: "h".into(), max_players: 0, bind: "x".into() }).is_err()
        );
    }

    #[test]
    fn setup_status_and_complete_use_disk() {
        let dir = std::env::temp_dir().join(format!("ald-setup-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(setup_status_in(&dir));
        let req = SetupRequest { hostname: "First".into(), max_players: 16, bind: "127.0.0.1:30120".into() };
        let path = complete_setup_in(&dir, &req).unwrap();
        assert!(path.exists());
        assert!(!setup_status_in(&dir));
        let back = std::fs::read_to_string(&path).unwrap();
        assert!(back.contains("First"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
