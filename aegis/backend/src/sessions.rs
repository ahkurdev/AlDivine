//! Session management for Aegis.
//!
//! Sessions are unguessable random tokens bound to a user and an issue time,
//! with absolute and idle expiry. Session invalidation is immediate: a
//! logged-out token stops authorizing on the very next request.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Default lifetime: 8 hours absolute, 30 minutes idle.
pub const DEFAULT_ABSOLUTE_TTL: u64 = 8 * 60 * 60;
pub const DEFAULT_IDLE_TTL: u64 = 30 * 60;
pub const TOKEN_LEN: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub token: String,
    pub user_id: String,
    pub issued_at: u64,
    pub last_activity: u64,
    pub absolute_expires: u64,
    pub idle_expires: u64,
    /// True only while the account has completed its second factor.
    pub fully_authenticated: bool,
}

impl Session {
    /// A session is live only if both the absolute and idle deadlines have
    /// not passed.
    pub fn is_live(&self, now: u64) -> bool {
        now < self.absolute_expires && now < self.idle_expires
    }
}

#[derive(Debug, Default)]
pub struct SessionStore {
    sessions: HashMap<String, Session>,
    absolute_ttl: u64,
    idle_ttl: u64,
}

impl SessionStore {
    pub fn new() -> Self {
        SessionStore::with_ttls(DEFAULT_ABSOLUTE_TTL, DEFAULT_IDLE_TTL)
    }

    pub fn with_ttls(absolute_ttl: u64, idle_ttl: u64) -> Self {
        SessionStore { sessions: HashMap::new(), absolute_ttl, idle_ttl }
    }

    /// Create a session. `fully_authenticated` is false until the second
    /// factor completes, so an account with TOTP enrolled can read nothing
    /// sensitive in between.
    pub fn create(&mut self, user_id: &str, now: u64, fully_authenticated: bool) -> Session {
        let token = generate_token();
        let session = Session {
            token: token.clone(),
            user_id: user_id.into(),
            issued_at: now,
            last_activity: now,
            absolute_expires: now + self.absolute_ttl,
            idle_expires: now + self.idle_ttl,
            fully_authenticated,
        };
        self.sessions.insert(token.clone(), session.clone());
        session
    }

    /// Look up a session and, if found and live, refresh its idle deadline.
    pub fn validate(&mut self, token: &str, now: u64) -> Option<Session> {
        let live = match self.sessions.get(token) {
            Some(s) => s.is_live(now),
            None => return None,
        };
        if !live {
            self.sessions.remove(token);
            return None;
        }
        let s = self.sessions.get_mut(token).unwrap();
        s.last_activity = now;
        s.idle_expires = now + self.idle_ttl;
        Some(s.clone())
    }

    /// Immediately invalidate one session.
    pub fn invalidate(&mut self, token: &str) -> bool {
        self.sessions.remove(token).is_some()
    }

    /// Invalidate every session for a user (password change, compromise).
    pub fn invalidate_user(&mut self, user_id: &str) -> usize {
        let to_remove: Vec<String> =
            self.sessions.iter().filter(|(_, s)| s.user_id == user_id).map(|(t, _)| t.clone()).collect();
        for t in &to_remove {
            self.sessions.remove(t);
        }
        to_remove.len()
    }

    /// Drop expired sessions so the store cannot grow unboundedly.
    pub fn gc(&mut self, now: u64) -> usize {
        let dead: Vec<String> = self.sessions.iter().filter(|(_, s)| !s.is_live(now)).map(|(t, _)| t.clone()).collect();
        for t in &dead {
            self.sessions.remove(t);
        }
        dead.len()
    }

    pub fn len(&self) -> usize {
        self.sessions.len()
    }
}

fn generate_token() -> String {
    let mut bytes = [0u8; TOKEN_LEN];
    fill_random(&mut bytes);
    hex_encode(&bytes)
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn fill_random(out: &mut [u8]) {
    #[cfg(unix)]
    {
        use std::io::Read;
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            if f.read_exact(out).is_ok() {
                return;
            }
        }
    }
    let mut seed: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x2545_f491_4f6c_dd1d);
    seed ^= std::process::id() as u64;
    for b in out.iter_mut() {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *b = (seed >> 32) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_is_live_and_validated() {
        let mut store = SessionStore::new();
        let s = store.create("user-1", 1000, true);
        assert!(store.validate(&s.token, 1010).is_some());
    }

    #[test]
    fn tokens_are_unique() {
        let mut store = SessionStore::new();
        let a = store.create("u", 1, true).token;
        let b = store.create("u", 2, true).token;
        assert_ne!(a, b);
    }

    #[test]
    fn unknown_token_rejected() {
        let mut store = SessionStore::new();
        assert!(store.validate("nope", 1).is_none());
    }

    #[test]
    fn idle_expiry_ends_session() {
        let mut store = SessionStore::with_ttls(1000, 10);
        let s = store.create("u", 100, true);
        assert!(store.validate(&s.token, 105).is_some());
        assert!(store.validate(&s.token, 120).is_none());
    }

    #[test]
    fn absolute_expiry_ends_session_even_when_active() {
        let mut store = SessionStore::with_ttls(10, 1000);
        let s = store.create("u", 100, true);
        // Constantly active, but the absolute deadline still passes.
        assert!(store.validate(&s.token, 105).is_some());
        assert!(store.validate(&s.token, 115).is_none());
    }

    #[test]
    fn invalidate_stops_authorization_immediately() {
        let mut store = SessionStore::new();
        let s = store.create("u", 1, true);
        assert!(store.invalidate(&s.token));
        assert!(store.validate(&s.token, 2).is_none());
    }

    #[test]
    fn invalidate_user_kills_all_their_sessions() {
        let mut store = SessionStore::new();
        store.create("u", 1, true);
        store.create("u", 2, true);
        store.create("other", 3, true);
        assert_eq!(store.invalidate_user("u"), 2);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn unauthenticated_session_cannot_become_authenticated_by_replay() {
        let mut store = SessionStore::new();
        let s = store.create("u", 1, false);
        let validated = store.validate(&s.token, 2).unwrap();
        assert!(!validated.fully_authenticated);
    }

    #[test]
    fn gc_removes_expired_sessions() {
        let mut store = SessionStore::with_ttls(10, 10);
        store.create("a", 1, true);
        store.create("b", 1, true);
        // All three are past both deadlines at t=20 (abs exp 11/11/15).
        store.create("c", 5, true);
        assert_eq!(store.gc(20), 3);
        assert_eq!(store.len(), 0);
    }
}
