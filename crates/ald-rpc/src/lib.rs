//! Typed request-response RPC with request IDs, timeouts, and metrics.
//! Pending requests are tracked so callbacks never stay unresolved.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RpcDirection {
    ClientToServer,
    ServerToClient,
    ResourceToResource,
}

#[derive(Debug, Clone)]
pub struct RpcRequest {
    pub id: u64,
    pub method: String,
    pub payload: Value,
    pub direction: RpcDirection,
    pub owner: String,
    pub created: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RpcResponse {
    pub id: u64,
    pub ok: bool,
    pub error: Option<String>,
    pub data: Option<Value>,
}

impl RpcResponse {
    pub fn success(id: u64, data: Value) -> Self {
        RpcResponse { id, ok: true, error: None, data: Some(data) }
    }
    pub fn failure(id: u64, error: impl Into<String>) -> Self {
        RpcResponse { id, ok: false, error: Some(error.into()), data: None }
    }
}

#[derive(Default)]
pub struct RpcRegistry {
    pending: Mutex<HashMap<u64, RpcRequest>>,
    next_id: Mutex<u64>,
    timeouts: Mutex<Vec<(Duration, u64)>>,
}

impl RpcRegistry {
    pub fn new() -> Self {
        RpcRegistry::default()
    }

    pub fn new_request(&self, method: &str, payload: Value, direction: RpcDirection, owner: &str) -> RpcRequest {
        let id = {
            let mut n = self.next_id.lock().unwrap();
            *n += 1;
            *n
        };
        RpcRequest {
            id,
            method: method.to_string(),
            payload,
            direction,
            owner: owner.to_string(),
            created: Instant::now(),
        }
    }

    pub fn track(&self, req: RpcRequest, timeout: Duration) {
        self.pending.lock().unwrap().insert(req.id, req.clone());
        self.timeouts.lock().unwrap().push((timeout, req.id));
    }

    /// Resolve a pending request. Returns the request if it was tracked.
    pub fn resolve(&self, resp: &RpcResponse) -> Option<RpcRequest> {
        self.pending.lock().unwrap().remove(&resp.id)
    }

    /// Expire timed-out requests, returning their ids for cleanup/timeout errors.
    pub fn expire(&self) -> Vec<u64> {
        let now = Instant::now();
        let mut timeouts = self.timeouts.lock().unwrap();
        let mut expired = Vec::new();
        timeouts.retain(|(dur, id)| {
            // We approximate by checking tracked pending age.
            if let Some(req) = self.pending.lock().unwrap().get(id) {
                if now.duration_since(req.created) >= *dur {
                    expired.push(*id);
                    false
                } else {
                    true
                }
            } else {
                false
            }
        });
        let mut pending = self.pending.lock().unwrap();
        for id in &expired {
            pending.remove(id);
        }
        expired
    }

    pub fn pending_count(&self) -> usize {
        self.pending.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_tracked_and_resolved() {
        let reg = RpcRegistry::new();
        let req = reg.new_request("ping", serde_json::json!({}), RpcDirection::ClientToServer, "core");
        reg.track(req.clone(), Duration::from_secs(5));
        assert_eq!(reg.pending_count(), 1);
        let resp = RpcResponse::success(req.id, serde_json::json!({"pong": true}));
        let taken = reg.resolve(&resp).unwrap();
        assert_eq!(taken.method, "ping");
        assert_eq!(reg.pending_count(), 0);
    }

    #[test]
    fn expire_returns_timed_out() {
        let reg = RpcRegistry::new();
        let req = reg.new_request("slow", serde_json::json!({}), RpcDirection::ServerToClient, "core");
        reg.track(req.clone(), Duration::from_millis(1));
        std::thread::sleep(Duration::from_millis(5));
        let expired = reg.expire();
        assert!(expired.contains(&req.id));
        assert_eq!(reg.pending_count(), 0);
    }
}
