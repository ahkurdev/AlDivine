//! Input + key mapping: canonical names, per-resource bindings, conflicts.
//!
//! Game input hookup (reading physical keys inside GTA) belongs to the game
//! bridge and is not claimed here. What this crate proves is everything
//! around it that is pure mapping logic:
//!
//! - [`KeyCode`]: canonical key names (`"F8"`, `"E"`, `"SPACE"`, `"MOUSE_LEFT"`,
//!   …). Parse is strict and case-insensitive; unknown names fail — a typo
//!   in a binding must error, not silently bind nothing. The catalog covers
//!   keyboard function keys, alphanumerics, common controls, arrows, numpad,
//!   and mouse buttons. It is Aldivine's catalog, documented as such —
//!   physical scancode mapping happens at the bridge.
//! - [`InputMapper`]: per-resource bindings of key → action with press-type
//!   (`JustPressed`, `Pressed`, `JustReleased`). One key may serve many
//!   resources; conflicts are *reported* ([`InputMapper::conflicts`]), not
//!   silently resolved — the operator/resource decides priority, and
//!   `resolve_for` returns all claimants in registration order.
//! - Reserved keys: `F8` (developer console) cannot be bound without
//!   explicit opt-in (`allow_console_key`), so resources cannot hijack the
//!   console by accident.
//!
//! No polling, no hooks, no game state. The bridge feeds physical presses
//! in; this crate answers "which resource actions fire".

use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

/// Developer console key. Reserved by default.
pub const CONSOLE_KEY: &str = "F8";

/// How a press counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PressType {
    JustPressed,
    Pressed,
    JustReleased,
}

/// Canonical key identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCode(pub String);

impl KeyCode {
    /// Parse a key name (case-insensitive, trimmed). Accepts the catalog in
    /// [`KNOWN_KEYS`] plus `MOUSE_*` buttons. Anything else errors.
    pub fn parse(name: &str) -> Result<Self, InputError> {
        let upper = name.trim().to_ascii_uppercase().replace(' ', "_");
        if upper.is_empty() || upper.len() > 24 {
            return Err(InputError::UnknownKey(name.to_string()));
        }
        if KNOWN_KEYS.contains(&upper.as_str()) {
            return Ok(KeyCode(upper));
        }
        Err(InputError::UnknownKey(name.to_string()))
    }

    pub fn is_console(&self) -> bool {
        self.0 == CONSOLE_KEY
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum InputError {
    #[error("unknown key '{0}'")]
    UnknownKey(String),
    #[error("key '{0}' is reserved (developer console)")]
    ReservedKey(String),
    #[error("no such binding")]
    NoSuchBinding,
}

/// Canonical key catalog (Aldivine-defined names).
pub const KNOWN_KEYS: &[&str] = &[
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F11",
    "F12",
    "ESCAPE",
    "TAB",
    "CAPSLOCK",
    "SHIFT",
    "CTRL",
    "ALT",
    "SPACE",
    "ENTER",
    "BACKSPACE",
    "UP",
    "DOWN",
    "LEFT",
    "RIGHT",
    "INSERT",
    "DELETE",
    "HOME",
    "END",
    "PAGEUP",
    "PAGEDOWN",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "NUMPAD0",
    "NUMPAD1",
    "NUMPAD2",
    "NUMPAD3",
    "NUMPAD4",
    "NUMPAD5",
    "NUMPAD6",
    "NUMPAD7",
    "NUMPAD8",
    "NUMPAD9",
    "MOUSE_LEFT",
    "MOUSE_RIGHT",
    "MOUSE_MIDDLE",
    "MOUSE_X1",
    "MOUSE_X2",
];

/// One binding: which resource action a key press fires.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub resource: String,
    pub action: String,
    pub key: KeyCode,
    pub press: PressType,
}

/// Per-resource key bindings with conflict reporting.
#[derive(Default)]
pub struct InputMapper {
    bindings: BTreeMap<u64, Binding>,
    next_id: u64,
    allow_console_key: bool,
}

impl InputMapper {
    pub fn new() -> Self {
        InputMapper::default()
    }

    /// Allow binding the console key (default: refused). Explicit opt-in so
    /// no resource hijacks F8 by accident.
    pub fn set_allow_console_key(&mut self, allow: bool) {
        self.allow_console_key = allow;
    }

    /// Bind `key` → `action` for `resource`. Returns a binding id.
    pub fn bind(&mut self, resource: &str, action: &str, key: KeyCode, press: PressType) -> Result<u64, InputError> {
        if key.is_console() && !self.allow_console_key {
            return Err(InputError::ReservedKey(key.0.clone()));
        }
        if resource.is_empty() || resource.len() > 64 || action.is_empty() || action.len() > 64 {
            return Err(InputError::UnknownKey(format!("{resource}:{action}")));
        }
        let id = self.next_id;
        self.next_id += 1;
        self.bindings.insert(id, Binding { resource: resource.to_string(), action: action.to_string(), key, press });
        Ok(id)
    }

    /// Remove a binding. Unknown ids error.
    pub fn unbind(&mut self, id: u64) -> Result<(), InputError> {
        self.bindings.remove(&id).map(|_| ()).ok_or(InputError::NoSuchBinding)
    }

    /// All bindings claiming `(key, press)`, in registration order.
    pub fn resolve_for(&self, key: &KeyCode, press: PressType) -> Vec<(u64, &Binding)> {
        self.bindings.iter().filter(|(_, b)| b.key == *key && b.press == press).map(|(id, b)| (*id, b)).collect()
    }

    /// Keys claimed by more than one binding (resource conflicts), each with
    /// its claimant list. Sorted by key name for stable reports.
    pub fn conflicts(&self) -> Vec<(&KeyCode, Vec<(u64, &Binding)>)> {
        let mut by_key: HashMap<(&KeyCode, PressType), Vec<(u64, &Binding)>> = HashMap::new();
        for (id, b) in &self.bindings {
            by_key.entry((&b.key, b.press)).or_default().push((*id, b));
        }
        let mut out: Vec<(&KeyCode, Vec<(u64, &Binding)>)> =
            by_key.into_iter().filter(|(_, v)| v.len() > 1).map(|((k, _), v)| (k, v)).collect();
        out.sort_by(|a, b| a.0 .0.cmp(&b.0 .0));
        out
    }

    pub fn binding_count(&self) -> usize {
        self.bindings.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_parse_strictly() {
        assert_eq!(KeyCode::parse("f8").unwrap(), KeyCode("F8".into()));
        assert_eq!(KeyCode::parse("  Space ").unwrap(), KeyCode("SPACE".into()));
        assert_eq!(KeyCode::parse("mouse_left").unwrap(), KeyCode("MOUSE_LEFT".into()));
        for bad in ["", "   ", "F13", "SUPER", "MOUSE_WHEEL", "SHIFT_L", "this-key-is-way-too-long-for-sure"] {
            assert_eq!(KeyCode::parse(bad), Err(InputError::UnknownKey(bad.to_string())), "{bad:?}");
        }
    }

    #[test]
    fn console_key_reserved_by_default() {
        let mut m = InputMapper::new();
        let f8 = KeyCode::parse("F8").unwrap();
        assert!(f8.is_console());
        assert_eq!(
            m.bind("res", "menu", f8.clone(), PressType::JustPressed),
            Err(InputError::ReservedKey("F8".into()))
        );
        m.set_allow_console_key(true);
        assert!(m.bind("res", "menu", f8, PressType::JustPressed).is_ok());
        m.set_allow_console_key(false);
        assert!(m.bind("res2", "menu", KeyCode::parse("F8").unwrap(), PressType::JustPressed).is_err());
    }

    #[test]
    fn bind_resolve_unbind() {
        let mut m = InputMapper::new();
        let e = KeyCode::parse("E").unwrap();
        let id = m.bind("shop", "open", e.clone(), PressType::JustPressed).unwrap();
        let hit = m.resolve_for(&e, PressType::JustPressed);
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].1.action, "open");
        // Different press type does not match.
        assert!(m.resolve_for(&e, PressType::Pressed).is_empty());
        m.unbind(id).unwrap();
        assert_eq!(m.unbind(id), Err(InputError::NoSuchBinding));
        assert!(m.resolve_for(&e, PressType::JustPressed).is_empty());
    }

    #[test]
    fn bad_binding_names_rejected() {
        let mut m = InputMapper::new();
        let e = KeyCode::parse("E").unwrap();
        assert!(m.bind("", "open", e.clone(), PressType::JustPressed).is_err());
        assert!(m.bind("res", "", e.clone(), PressType::JustPressed).is_err());
        assert!(m.bind("res", &"a".repeat(65), e, PressType::JustPressed).is_err());
    }

    #[test]
    fn conflicts_reported_not_resolved() {
        let mut m = InputMapper::new();
        let e = KeyCode::parse("E").unwrap();
        m.bind("shop", "open", e.clone(), PressType::JustPressed).unwrap();
        m.bind("garage", "enter", e.clone(), PressType::JustPressed).unwrap();
        m.bind("solo", "ping", KeyCode::parse("Q").unwrap(), PressType::JustPressed).unwrap();
        let conflicts = m.conflicts();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, &e);
        let actions: Vec<&str> = conflicts[0].1.iter().map(|(_, b)| b.action.as_str()).collect();
        assert_eq!(actions, vec!["open", "enter"]); // registration order
                                                    // Same key, different press type: no conflict.
        m.bind("shop", "hold", e.clone(), PressType::Pressed).unwrap();
        assert_eq!(m.conflicts().len(), 1);
    }

    #[test]
    fn unbind_clears_conflict() {
        let mut m = InputMapper::new();
        let e = KeyCode::parse("E").unwrap();
        let a = m.bind("r1", "a", e.clone(), PressType::JustPressed).unwrap();
        m.bind("r2", "b", e.clone(), PressType::JustPressed).unwrap();
        assert_eq!(m.conflicts().len(), 1);
        m.unbind(a).unwrap();
        assert!(m.conflicts().is_empty());
    }
}
