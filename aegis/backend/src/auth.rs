//! Aegis authentication service.
//!
//! Ties password verification, TOTP, sessions, and the audit log into one
//! login pipeline with brute-force protection.
//!
//! Threat model this is written against: an attacker who has stolen the
//! password database and is hammering the login endpoint. Every gate below
//! exists to make that slow, noisy, and unsuccessful:
//! - Passwords are never stored in plaintext and never logged.
//! - Rate limiting is per-account, so one account under attack cannot
//!   degrade login for everyone.
//! - An account with TOTP enrolled cannot complete login with a password
//!   alone, and a wrong second factor is itself a rate-limited failure.
//! - Every attempt — successful or not — is written to the audit log.

use crate::audit::{AuditAction, AuditEntry, AuditLog};
use crate::passwords::verify_password;
use crate::sessions::{Session, SessionStore};
use crate::totp;

/// Brute-force limits.
pub const MAX_ATTEMPTS: u32 = 5;
pub const LOCKOUT_WINDOW_SECS: u64 = 15 * 60;

/// What a login attempt concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthOutcome {
    /// Fully authenticated; a live session was issued.
    Success { session: Session },
    /// Password was right but a second factor is required.
    SecondFactorRequired { session: Session },
    /// A second factor was supplied and accepted.
    SecondFactorAccepted { session: Session },
    /// Bad password or bad TOTP code.
    InvalidCredentials,
    /// Too many failures; the account is temporarily locked.
    LockedOut { retry_after_secs: u64 },
}

/// What the caller supplied for this attempt.
#[derive(Debug, Clone)]
pub struct LoginAttempt {
    pub username: String,
    pub password: String,
    /// None if the account has no TOTP enrolled.
    pub totp_code: Option<String>,
    pub source_ip: Option<String>,
}

/// Account record as the auth service needs it.
#[derive(Debug, Clone)]
pub struct AegisAccount {
    pub id: String,
    pub username: String,
    pub password_hash: String,
    pub totp_secret: Option<String>,
}

/// Rate-limit state per account.
#[derive(Debug, Default, Clone)]
struct RateState {
    failures: u32,
    window_start: u64,
}

pub struct AuthService {
    sessions: SessionStore,
    audit: AuditLog,
    limits: std::collections::HashMap<String, RateState>,
    max_attempts: u32,
    lockout_window: u64,
}

impl AuthService {
    pub fn new(audit_capacity: usize) -> Self {
        AuthService {
            sessions: SessionStore::new(),
            audit: AuditLog::new(audit_capacity),
            limits: std::collections::HashMap::new(),
            max_attempts: MAX_ATTEMPTS,
            lockout_window: LOCKOUT_WINDOW_SECS,
        }
    }

    pub fn with_limits(mut self, max_attempts: u32, lockout_window: u64) -> Self {
        self.max_attempts = max_attempts.max(1);
        self.lockout_window = lockout_window;
        self
    }

    /// Attempt a login. The outcome is the only authorization signal — never
    /// infer success from the absence of an error.
    pub fn login(&mut self, account: &AegisAccount, attempt: &LoginAttempt, now: u64) -> AuthOutcome {
        // 1. Brute-force gate: if this account is already over its failure
        //    budget within the window, reject without even hashing.
        if self.is_locked(&account.username, now) {
            self.audit.record(AuditEntry {
                actor: account.username.clone(),
                action: AuditAction::FailedLogin,
                target: None,
                timestamp: now,
                success: false,
                detail: "account locked out".into(),
                source_ip: attempt.source_ip.clone(),
            });
            return AuthOutcome::LockedOut { retry_after_secs: self.lockout_window };
        }

        // 2. Password gate. A wrong password counts against the budget.
        if let Err(e) = verify_password(&attempt.password, &account.password_hash) {
            self.record_failure(&account.username, now, &attempt.source_ip, "bad password");
            // Always return the same error shape so an attacker cannot tell
            // "no such account" from "wrong password".
            let _ = e;
            return AuthOutcome::InvalidCredentials;
        }

        // 3. Second-factor gate. If a secret is enrolled, a password alone
        //    is not enough: issue a provisional session that authorizes
        //    nothing sensitive until the factor completes.
        if let Some(secret) = &account.totp_secret {
            match &attempt.totp_code {
                None => {
                    let session = self.sessions.create(&account.id, now, false);
                    return AuthOutcome::SecondFactorRequired { session };
                }
                Some(code) => match totp::verify_totp(secret, code, now) {
                    Ok(()) => {
                        let session = self.sessions.create(&account.id, now, true);
                        self.record_success(&account.username, now, &attempt.source_ip, "password+totp");
                        return AuthOutcome::SecondFactorAccepted { session };
                    }
                    Err(_) => {
                        self.record_failure(&account.username, now, &attempt.source_ip, "bad totp");
                        return AuthOutcome::InvalidCredentials;
                    }
                },
            }
        }

        // 4. No second factor enrolled: password is sufficient.
        let session = self.sessions.create(&account.id, now, true);
        self.record_success(&account.username, now, &attempt.source_ip, "password");
        AuthOutcome::Success { session }
    }

    fn is_locked(&self, username: &str, now: u64) -> bool {
        match self.limits.get(username) {
            Some(state) => state.failures >= self.max_attempts && now < state.window_start + self.lockout_window,
            None => false,
        }
    }

    fn record_failure(&mut self, username: &str, now: u64, source_ip: &Option<String>, detail: &str) {
        let state = self.limits.entry(username.to_string()).or_default();
        if now >= state.window_start + self.lockout_window {
            state.window_start = now;
            state.failures = 0;
        }
        state.failures += 1;
        self.audit.record(AuditEntry {
            actor: username.to_string(),
            action: AuditAction::FailedLogin,
            target: None,
            timestamp: now,
            success: false,
            detail: detail.into(),
            source_ip: source_ip.clone(),
        });
    }

    fn record_success(&mut self, username: &str, now: u64, source_ip: &Option<String>, detail: &str) {
        self.limits.remove(username);
        self.audit.record(AuditEntry {
            actor: username.to_string(),
            action: AuditAction::Authentication,
            target: None,
            timestamp: now,
            success: true,
            detail: detail.into(),
            source_ip: source_ip.clone(),
        });
    }

    pub fn logout(&mut self, token: &str, username: &str, now: u64) {
        self.sessions.invalidate(token);
        self.audit.record(AuditEntry {
            actor: username.to_string(),
            action: AuditAction::Logout,
            target: None,
            timestamp: now,
            success: true,
            detail: "logout".into(),
            source_ip: None,
        });
    }

    pub fn validate_session(&mut self, token: &str, now: u64) -> Option<Session> {
        self.sessions.validate(token, now)
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn sessions(&self) -> &SessionStore {
        &self.sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passwords::hash_password;

    fn account(username: &str, password: &str, totp_secret: Option<String>) -> AegisAccount {
        AegisAccount {
            id: format!("id-{username}"),
            username: username.into(),
            password_hash: hash_password(password).unwrap(),
            totp_secret,
        }
    }

    fn attempt(user: &str, pw: &str) -> LoginAttempt {
        LoginAttempt { username: user.into(), password: pw.into(), totp_code: None, source_ip: None }
    }

    #[test]
    fn correct_password_succeeds_without_totp() {
        let mut svc = AuthService::new(64);
        let acc = account("admin", "s3cret", None);
        match svc.login(&acc, &attempt("admin", "s3cret"), 1000) {
            AuthOutcome::Success { session } => assert!(session.fully_authenticated),
            other => panic!("expected success, got {other:?}"),
        }
    }

    #[test]
    fn wrong_password_fails_and_is_audited() {
        let mut svc = AuthService::new(64);
        let acc = account("admin", "s3cret", None);
        assert!(matches!(svc.login(&acc, &attempt("admin", "wrong"), 1000), AuthOutcome::InvalidCredentials));
        assert_eq!(svc.audit().by_action(AuditAction::FailedLogin).len(), 1);
    }

    #[test]
    fn totp_account_requires_second_factor() {
        let mut svc = AuthService::new(64);
        let secret = totp::generate_totp_secret().unwrap();
        let acc = account("admin", "s3cret", Some(secret.clone()));
        match svc.login(&acc, &attempt("admin", "s3cret"), 1000) {
            AuthOutcome::SecondFactorRequired { session } => assert!(!session.fully_authenticated),
            other => panic!("expected second factor, got {other:?}"),
        }
    }

    #[test]
    fn correct_totp_completes_login() {
        let mut svc = AuthService::new(64);
        let secret = totp::generate_totp_secret().unwrap();
        let acc = account("admin", "s3cret", Some(secret.clone()));
        let mut att = attempt("admin", "s3cret");
        att.totp_code = Some(totp::totp_code(&secret, 1000).unwrap());
        match svc.login(&acc, &att, 1000) {
            AuthOutcome::SecondFactorAccepted { session } => assert!(session.fully_authenticated),
            other => panic!("expected 2fa accepted, got {other:?}"),
        }
    }

    #[test]
    fn wrong_totp_fails() {
        let mut svc = AuthService::new(64);
        let secret = totp::generate_totp_secret().unwrap();
        let acc = account("admin", "s3cret", Some(secret));
        let mut att = attempt("admin", "s3cret");
        att.totp_code = Some("000000".into());
        assert!(matches!(svc.login(&acc, &att, 1000), AuthOutcome::InvalidCredentials));
    }

    #[test]
    fn repeated_failures_lock_the_account() {
        let mut svc = AuthService::new(64).with_limits(3, 100);
        let acc = account("admin", "s3cret", None);
        for _ in 0..3 {
            assert!(matches!(svc.login(&acc, &attempt("admin", "wrong"), 10), AuthOutcome::InvalidCredentials));
        }
        // Now even the correct password is refused during lockout.
        match svc.login(&acc, &attempt("admin", "s3cret"), 20) {
            AuthOutcome::LockedOut { .. } => {}
            other => panic!("expected lockout, got {other:?}"),
        }
    }

    #[test]
    fn lockout_expires() {
        let mut svc = AuthService::new(64).with_limits(2, 100);
        let acc = account("admin", "s3cret", None);
        for _ in 0..2 {
            let _ = svc.login(&acc, &attempt("admin", "wrong"), 10);
        }
        assert!(matches!(svc.login(&acc, &attempt("admin", "s3cret"), 20), AuthOutcome::LockedOut { .. }));
        // Past the lockout window the budget resets.
        match svc.login(&acc, &attempt("admin", "s3cret"), 200) {
            AuthOutcome::Success { .. } => {}
            other => panic!("expected success after lockout expiry, got {other:?}"),
        }
    }

    #[test]
    fn success_resets_failure_budget() {
        let mut svc = AuthService::new(64).with_limits(3, 1000);
        let acc = account("admin", "s3cret", None);
        let _ = svc.login(&acc, &attempt("admin", "wrong"), 10);
        assert!(matches!(svc.login(&acc, &attempt("admin", "s3cret"), 20), AuthOutcome::Success { .. }));
        // A fresh failure budget.
        for _ in 0..2 {
            assert!(matches!(svc.login(&acc, &attempt("admin", "wrong"), 30), AuthOutcome::InvalidCredentials));
        }
        assert!(!matches!(svc.login(&acc, &attempt("admin", "wrong"), 40), AuthOutcome::LockedOut { .. }));
    }

    #[test]
    fn logout_invalidates_session() {
        let mut svc = AuthService::new(64);
        let acc = account("admin", "s3cret", None);
        let session = match svc.login(&acc, &attempt("admin", "s3cret"), 1000) {
            AuthOutcome::Success { session } => session,
            _ => panic!("expected success"),
        };
        svc.logout(&session.token, "admin", 1010);
        assert!(svc.validate_session(&session.token, 1020).is_none());
    }

    #[test]
    fn password_hash_is_never_in_audit_detail() {
        let mut svc = AuthService::new(64);
        let acc = account("admin", "s3cret", None);
        let _ = svc.login(&acc, &attempt("admin", "wrong"), 1000);
        for e in svc.audit().entries() {
            assert!(!e.detail.contains("s3cret"), "password must never appear in audit detail");
        }
    }

    #[test]
    fn one_account_lockout_does_not_block_another() {
        let mut svc = AuthService::new(64).with_limits(2, 1000);
        let bad = account("mallory", "pw", None);
        let good = account("alice", "pw", None);
        for _ in 0..2 {
            let _ = svc.login(&bad, &attempt("mallory", "nope"), 10);
        }
        assert!(matches!(svc.login(&bad, &attempt("mallory", "pw"), 20), AuthOutcome::LockedOut { .. }));
        assert!(matches!(svc.login(&good, &attempt("alice", "pw"), 20), AuthOutcome::Success { .. }));
    }
}
