//! NUI groundwork: secure origins, message channels, callbacks, focus, CSP.
//!
//! The embedded browser process (CEF/Chromium) is an allowed native boundary
//! that is NOT vendored here — no browser hookup is claimed. What this crate
//! proves is everything around it that is pure policy:
//!
//! - [`NuiOrigin`]: resource origins of the form `aldnui://<resource>/<path>`
//!   (Aldivine's own scheme, defined here — not a claim about anyone else's
//!   URL space). Resource names are strict (`[a-z0-9_-]`, bounded); paths
//!   reject traversal, NUL bytes, and absolute escapes.
//! - [`MessageQueue`]: bounded server→UI message channel. Oversized payloads
//!   are rejected, never truncated into corrupt JSON.
//! - [`CallbackRegistry`]: `RegisterNUICallback` table. Names are strict;
//!   invocation carries a request id; each request answers exactly once —
//!   double responses and unknown callbacks are errors, not silent drops.
//! - [`FocusState`]: `SetNuiFocus` / `SetNuiFocusKeepInput` tracking. One
//!   focused resource at a time; focus without UI open is refused.
//! - [`CspPolicy`]: Content-Security-Policy header builder. Default denies
//!   everything; WASM and origins open only when explicitly allowed.
//!
//! WASM note: `allow_wasm` gates `'wasm-unsafe-eval'` in the built header.
//! Whether the embedded browser honors it is the browser's business, proven
//! at integration — this crate proves the header says what the operator set.

use std::collections::{HashMap, VecDeque};

use thiserror::Error;

/// Longest accepted resource name / callback name / message payload.
pub const MAX_NAME_LEN: usize = 64;
pub const MAX_MESSAGE_LEN: usize = 1_048_576; // 1 MiB

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum NuiError {
    #[error("bad NUI origin '{0}'")]
    BadOrigin(String),
    #[error("bad resource name '{0}'")]
    BadResource(String),
    #[error("bad NUI path '{0}'")]
    BadPath(String),
    #[error("bad callback name '{0}'")]
    BadCallback(String),
    #[error("message too large ({0} bytes)")]
    MessageTooLarge(usize),
    #[error("unknown callback '{0}'")]
    UnknownCallback(String),
    #[error("callback request {0} already answered")]
    DoubleResponse(u64),
    #[error("unknown callback request {0}")]
    UnknownRequest(u64),
    #[error("duplicate callback registration '{0}'")]
    DuplicateCallback(String),
    #[error("focus refused: NUI not open for '{0}'")]
    FocusWithoutUi(String),
}

fn valid_token(s: &str, what: fn(String) -> NuiError) -> Result<(), NuiError> {
    if s.is_empty() || s.len() > MAX_NAME_LEN {
        return Err(what(s.to_string()));
    }
    if s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-') {
        Ok(())
    } else {
        Err(what(s.to_string()))
    }
}

/// A parsed `aldnui://<resource>/<path>` origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NuiOrigin {
    pub resource: String,
    pub path: String,
}

impl NuiOrigin {
    /// Parse and validate. Path must be root-relative (`/…`), contain no
    /// `..` segments, no NUL bytes, and no backslashes.
    pub fn parse(origin: &str) -> Result<Self, NuiError> {
        let rest = origin.strip_prefix("aldnui://").ok_or_else(|| NuiError::BadOrigin(origin.to_string()))?;
        let (resource, path) = rest.split_once('/').ok_or_else(|| NuiError::BadOrigin(origin.to_string()))?;
        valid_token(resource, NuiError::BadResource)?;
        if path.is_empty() || path.contains('\0') || path.contains('\\') {
            return Err(NuiError::BadPath(path.to_string()));
        }
        if path.split('/').any(|seg| seg == "..") {
            return Err(NuiError::BadPath(path.to_string()));
        }
        Ok(NuiOrigin { resource: resource.to_string(), path: path.to_string() })
    }

    /// Whether this origin belongs to a resource allowed to use NUI.
    pub fn resource_allowed(&self, allowed: &[String]) -> bool {
        allowed.iter().any(|r| r == &self.resource)
    }
}

/// Bounded FIFO of server→UI messages.
#[derive(Debug, Default)]
pub struct MessageQueue {
    queue: VecDeque<(String, Vec<u8>)>,
    capacity: usize,
    dropped: u64,
}

impl MessageQueue {
    pub fn new(capacity: usize) -> Self {
        MessageQueue { queue: VecDeque::new(), capacity: capacity.max(1), dropped: 0 }
    }

    /// Enqueue (`SendNUIMessage`). Oversized payloads error; overflow drops
    /// the oldest and counts it (backpressure is visible, not silent).
    pub fn send(&mut self, resource: &str, payload: Vec<u8>) -> Result<(), NuiError> {
        valid_token(resource, NuiError::BadResource)?;
        if payload.len() > MAX_MESSAGE_LEN {
            return Err(NuiError::MessageTooLarge(payload.len()));
        }
        if self.queue.len() >= self.capacity {
            self.queue.pop_front();
            self.dropped += 1;
        }
        self.queue.push_back((resource.to_string(), payload));
        Ok(())
    }

    pub fn drain(&mut self) -> Vec<(String, Vec<u8>)> {
        std::mem::take(&mut self.queue).into_iter().collect()
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

/// Pending UI→server callback invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallbackRequest {
    pub id: u64,
    pub callback: String,
    pub resource: String,
    pub payload: Vec<u8>,
}

/// `RegisterNUICallback` table with exactly-once responses.
#[derive(Default)]
pub struct CallbackRegistry {
    handlers: HashMap<String, String>,
    pending: HashMap<u64, CallbackRequest>,
    answered: HashMap<u64, Vec<u8>>,
    next_id: u64,
}

impl CallbackRegistry {
    pub fn new() -> Self {
        CallbackRegistry::default()
    }

    /// Register `callback` for `resource`. Names strict; re-registration of
    /// the same name (even same resource) is refused — stale handlers must
    /// be unregistered explicitly.
    pub fn register(&mut self, resource: &str, callback: &str) -> Result<(), NuiError> {
        valid_token(resource, NuiError::BadResource)?;
        valid_token(callback, NuiError::BadCallback)?;
        if self.handlers.contains_key(callback) {
            return Err(NuiError::DuplicateCallback(callback.to_string()));
        }
        self.handlers.insert(callback.to_string(), resource.to_string());
        Ok(())
    }

    pub fn unregister(&mut self, callback: &str) -> bool {
        self.handlers.remove(callback).is_some()
    }

    /// Invoke from the UI side. Returns the request id the response must
    /// carry. Unknown callbacks and oversized payloads fail here.
    pub fn invoke(&mut self, resource: &str, callback: &str, payload: Vec<u8>) -> Result<u64, NuiError> {
        let owner = self.handlers.get(callback).ok_or_else(|| NuiError::UnknownCallback(callback.to_string()))?;
        if owner != resource {
            return Err(NuiError::UnknownCallback(callback.to_string()));
        }
        if payload.len() > MAX_MESSAGE_LEN {
            return Err(NuiError::MessageTooLarge(payload.len()));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.pending.insert(
            id,
            CallbackRequest { id, callback: callback.to_string(), resource: resource.to_string(), payload },
        );
        Ok(id)
    }

    /// Answer a request exactly once.
    pub fn respond(&mut self, id: u64, response: Vec<u8>) -> Result<(), NuiError> {
        if self.answered.contains_key(&id) {
            return Err(NuiError::DoubleResponse(id));
        }
        if self.pending.remove(&id).is_none() {
            return Err(NuiError::UnknownRequest(id));
        }
        self.answered.insert(id, response);
        Ok(())
    }

    /// Take an answered response (delivery to the UI side).
    pub fn take_response(&mut self, id: u64) -> Option<Vec<u8>> {
        self.answered.remove(&id)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
}

/// NUI focus tracking (`SetNuiFocus` + keep-input).
#[derive(Debug, Default)]
pub struct FocusState {
    ui_open: HashMap<String, bool>,
    focused: Option<String>,
    cursor: bool,
    keep_input: bool,
}

impl FocusState {
    pub fn new() -> Self {
        FocusState::default()
    }

    /// Mark a resource's UI open (page loaded) or closed.
    pub fn set_ui_open(&mut self, resource: &str, open: bool) {
        if open {
            self.ui_open.insert(resource.to_string(), true);
        } else {
            self.ui_open.remove(resource);
            if self.focused.as_deref() == Some(resource) {
                self.focused = None;
                self.cursor = false;
                self.keep_input = false;
            }
        }
    }

    /// Focus (or unfocus with `None`) a resource. Focusing a resource whose
    /// UI is not open is refused — focus without a page is a stuck-input bug.
    pub fn set_focus(&mut self, resource: Option<&str>, cursor: bool, keep_input: bool) -> Result<(), NuiError> {
        match resource {
            None => {
                self.focused = None;
                self.cursor = false;
                self.keep_input = false;
                Ok(())
            }
            Some(r) => {
                if !self.ui_open.contains_key(r) {
                    return Err(NuiError::FocusWithoutUi(r.to_string()));
                }
                self.focused = Some(r.to_string());
                self.cursor = cursor;
                self.keep_input = keep_input;
                Ok(())
            }
        }
    }

    pub fn focused(&self) -> Option<&str> {
        self.focused.as_deref()
    }

    pub fn has_cursor(&self) -> bool {
        self.cursor
    }

    pub fn keep_input(&self) -> bool {
        self.keep_input
    }
}

/// CSP header builder. Default denies everything; capabilities open
/// explicitly and visibly in the emitted header.
#[derive(Debug, Clone, Default)]
pub struct CspPolicy {
    pub allow_wasm: bool,
    pub allowed_origins: Vec<String>,
}

impl CspPolicy {
    pub fn header(&self) -> String {
        let mut directives = vec!["default-src 'none'".to_string()];
        let mut script = vec!["'self'".to_string()];
        if self.allow_wasm {
            script.push("'wasm-unsafe-eval'".to_string());
        }
        directives.push(format!("script-src {}", script.join(" ")));
        if self.allowed_origins.is_empty() {
            directives.push("connect-src 'none'".to_string());
        } else {
            directives.push(format!("connect-src {}", self.allowed_origins.join(" ")));
        }
        directives.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_parses_and_validates() {
        let o = NuiOrigin::parse("aldnui://chat/index.html").unwrap();
        assert_eq!(o.resource, "chat");
        assert_eq!(o.path, "index.html");
        assert!(o.resource_allowed(&["chat".to_string()]));
        assert!(!o.resource_allowed(&["other".to_string()]));
        for bad in [
            "https://chat/index.html",
            "aldnui://",
            "aldnui:///index.html",
            "aldnui://BAD NAME/index.html",
            "aldnui://chat/",
            "aldnui://chat/../secret.html",
            "aldnui://chat/a\\b.html",
            "aldnui://chat/a\0b.html",
            "aldnui://chat",
        ] {
            assert!(NuiOrigin::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn long_names_rejected() {
        let long = "r".repeat(MAX_NAME_LEN + 1);
        assert_eq!(NuiOrigin::parse(&format!("aldnui://{long}/i.html")), Err(NuiError::BadResource(long.clone())));
        let mut q = MessageQueue::new(4);
        assert!(q.send(&long, vec![]).is_err());
        let mut r = CallbackRegistry::new();
        assert!(r.register("res", &long).is_err());
    }

    #[test]
    fn message_queue_bounds_and_backpressure() {
        let mut q = MessageQueue::new(2);
        q.send("a", b"1".to_vec()).unwrap();
        q.send("a", b"2".to_vec()).unwrap();
        q.send("a", b"3".to_vec()).unwrap(); // drops "1"
        assert_eq!(q.dropped(), 1);
        let drained = q.drain();
        assert_eq!(drained, vec![("a".to_string(), b"2".to_vec()), ("a".to_string(), b"3".to_vec())]);
        assert!(q.is_empty());
        assert_eq!(q.send("a", vec![0; MAX_MESSAGE_LEN + 1]), Err(NuiError::MessageTooLarge(MAX_MESSAGE_LEN + 1)));
    }

    #[test]
    fn callbacks_register_invoke_respond_once() {
        let mut r = CallbackRegistry::new();
        r.register("bank", "deposit").unwrap();
        assert_eq!(r.register("bank", "deposit"), Err(NuiError::DuplicateCallback("deposit".into())));
        // Wrong resource cannot invoke another's callback.
        assert_eq!(r.invoke("shop", "deposit", vec![]), Err(NuiError::UnknownCallback("deposit".into())));
        assert_eq!(r.invoke("bank", "missing", vec![]), Err(NuiError::UnknownCallback("missing".into())));
        let id = r.invoke("bank", "deposit", b"{\"amount\":5}".to_vec()).unwrap();
        assert_eq!(r.pending_count(), 1);
        r.respond(id, b"ok".to_vec()).unwrap();
        assert_eq!(r.respond(id, b"again".to_vec()), Err(NuiError::DoubleResponse(id)));
        assert_eq!(r.respond(999, b"x".to_vec()), Err(NuiError::UnknownRequest(999)));
        assert_eq!(r.take_response(id), Some(b"ok".to_vec()));
        assert_eq!(r.take_response(id), None);
        // Unregister removes the name.
        assert!(r.unregister("deposit"));
        assert!(!r.unregister("deposit"));
        assert_eq!(r.invoke("bank", "deposit", vec![]), Err(NuiError::UnknownCallback("deposit".into())));
    }

    #[test]
    fn focus_needs_open_ui() {
        let mut f = FocusState::new();
        assert_eq!(f.set_focus(Some("chat"), true, false), Err(NuiError::FocusWithoutUi("chat".into())));
        f.set_ui_open("chat", true);
        f.set_focus(Some("chat"), true, true).unwrap();
        assert_eq!(f.focused(), Some("chat"));
        assert!(f.has_cursor() && f.keep_input());
        // Closing the UI clears focus (no stuck input).
        f.set_ui_open("chat", false);
        assert_eq!(f.focused(), None);
        assert!(!f.has_cursor() && !f.keep_input());
        // Explicit unfocus.
        f.set_ui_open("map", true);
        f.set_focus(Some("map"), false, false).unwrap();
        f.set_focus(None, false, false).unwrap();
        assert_eq!(f.focused(), None);
    }

    #[test]
    fn csp_default_deny_and_explicit_open() {
        let closed = CspPolicy::default().header();
        assert!(closed.contains("default-src 'none'"));
        assert!(closed.contains("connect-src 'none'"));
        assert!(!closed.contains("wasm-unsafe-eval"));
        let open = CspPolicy { allow_wasm: true, allowed_origins: vec!["aldnui://bank/".into()] }.header();
        assert!(open.contains("'wasm-unsafe-eval'"));
        assert!(open.contains("connect-src aldnui://bank/"));
    }
}
