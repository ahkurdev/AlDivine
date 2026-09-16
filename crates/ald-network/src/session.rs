//! AstraNet session management.
//! A session is a cryptographically-random 64-bit id issued after AUTH.
//! Sessions track reliability state and rate-limit counters per peer.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rand::RngCore;

use crate::reliability::ReliabilityState;

/// A connected peer session.
pub struct Session {
    pub id: u64,
    pub addr: SocketAddr,
    pub created: Instant,
    pub last_seen: Mutex<Instant>,
    pub reliability: Mutex<ReliabilityState>,
}

impl Session {
    fn new(id: u64, addr: SocketAddr) -> Self {
        let now = Instant::now();
        Session { id, addr, created: now, last_seen: Mutex::new(now), reliability: Mutex::new(ReliabilityState::new()) }
    }

    pub fn touch(&self) {
        let _ = self.last_seen.lock().map(|mut g| *g = Instant::now());
    }

    pub fn idle_for(&self) -> Duration {
        match self.last_seen.lock() {
            Ok(g) => Instant::now().duration_since(*g),
            Err(_) => Duration::ZERO,
        }
    }
}

/// Manages sessions keyed by session id and by peer address.
#[derive(Default)]
pub struct SessionManager {
    by_id: HashMap<u64, Arc<Session>>,
    by_addr: HashMap<SocketAddr, u64>,
}

impl SessionManager {
    pub fn new() -> Self {
        SessionManager::default()
    }

    /// Create and register a new session for `addr`.
    pub fn create(&mut self, addr: SocketAddr) -> Arc<Session> {
        let id = random_session_id();
        let s = Arc::new(Session::new(id, addr));
        self.by_addr.insert(addr, id);
        self.by_id.insert(id, s.clone());
        s
    }

    pub fn get(&self, id: u64) -> Option<&Arc<Session>> {
        self.by_id.get(&id)
    }

    pub fn get_by_addr(&self, addr: &SocketAddr) -> Option<&Arc<Session>> {
        self.by_addr.get(addr).and_then(|id| self.by_id.get(id))
    }

    /// Drop a session. Returns true if it existed.
    pub fn remove(&mut self, id: u64) -> bool {
        if let Some(s) = self.by_id.remove(&id) {
            self.by_addr.remove(&s.addr);
            true
        } else {
            false
        }
    }

    /// Expire sessions idle longer than `timeout`. Returns the removed ids.
    pub fn expire_idle(&mut self, timeout: Duration) -> Vec<u64> {
        let dead: Vec<u64> = self.by_id.iter().filter(|(_, s)| s.idle_for() > timeout).map(|(id, _)| *id).collect();
        for id in &dead {
            self.remove(*id);
        }
        dead
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

fn random_session_id() -> u64 {
    let mut buf = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut buf);
    u64::from_le_bytes(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, SocketAddrV4};

    fn addr(p: u16) -> SocketAddr {
        SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), p))
    }

    #[test]
    fn create_and_lookup() {
        let mut sm = SessionManager::new();
        let s = sm.create(addr(1000));
        assert_eq!(sm.get(s.id).map(|x| x.id), Some(s.id));
        assert_eq!(sm.get_by_addr(&addr(1000)).map(|x| x.id), Some(s.id));
    }

    #[test]
    fn remove_session() {
        let mut sm = SessionManager::new();
        let s = sm.create(addr(1001));
        assert!(sm.remove(s.id));
        assert!(sm.get(s.id).is_none());
        assert!(sm.get_by_addr(&addr(1001)).is_none());
    }

    #[test]
    fn expire_idle_removes_old() {
        let mut sm = SessionManager::new();
        let s = sm.create(addr(1002));
        std::thread::sleep(Duration::from_millis(30));
        let removed = sm.expire_idle(Duration::from_millis(10));
        assert_eq!(removed, vec![s.id]);
        assert_eq!(sm.len(), 0);
    }
}
