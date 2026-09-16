use std::sync::Arc;

use axum::{extract::Path, extract::State, http::StatusCode, routing::get, Json, Router};
use serde::Serialize;
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

pub fn router(aegis: AegisState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/status", get(status))
        .route("/resources", get(resources))
        .route("/resources/:name/:action", axum::routing::post(resource_action))
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
}
