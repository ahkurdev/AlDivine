//! Aegis Control — backend core.
//!
//! Server operations and orchestration platform. This crate holds the
//! security-critical pieces that must be right before any admin can touch a
//! live server: password storage, session handling, TOTP second factors,
//! brute-force protection, and audit recording.
//!
//! Non-negotiable rules enforced here:
//! - Never store plaintext passwords. Passwords are hashed with a salted
//!   PBKDF2-style construction; verification never compares raw bytes.
//! - Rate-limit every authentication attempt.
//! - Every privileged action is written to an audit log with actor, target,
//!   and time.

pub mod audit;
pub mod auth;
pub mod passwords;
pub mod player_actions;
pub mod sessions;
pub mod totp;

pub use audit::{AuditAction, AuditEntry, AuditLog};
pub use auth::{AuthOutcome, AuthService, LoginAttempt};
pub use passwords::{hash_password, verify_password, PasswordError};
pub use player_actions::{ActionOutcome, BanDuration, BanRecord, PermissionResolver, PlayerActions};
pub use sessions::{Session, SessionStore};
pub use totp::{generate_totp_secret, totp_code, verify_totp, TotpError};
