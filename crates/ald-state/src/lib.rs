//! Native Aldivine state bags: replicated key/value stores with ownership.
//!
//! One [`StateBag`] per entity, player, or the global scope. Keys map to
//! [`Value`]s; every mutation bumps a per-bag sequence so replication can
//! order and deduplicate. Change handlers observe mutations with old/new
//! values.
//!
//! Rules enforced here (not call-site convention):
//! - [`WritePolicy::OwnerOnly`] bags reject writes from anyone but the
//!   recorded owner — including when no owner is set yet (fail closed).
//! - Keys: non-empty, no NUL bytes, at most [`MAX_KEY_LEN`] bytes.
//! - Values: bounded size ([`MAX_BLOB_LEN`]), bounded breadth
//!   ([`MAX_CHILDREN`]), bounded nesting depth ([`MAX_DEPTH`]).
//! - Replication is per-key: [`StateBag::set_replicated`] excludes keys
//!   (e.g. server-local scratch) from [`StateBag::snapshot_replicated`].
//! - Handlers fire in subscription order with old/new values. Handlers are
//!   observers by construction: they receive only `&ChangeEvent`, so they
//!   cannot write back into the bag they observe. Cross-bag reactions belong
//!   to the orchestration layer, which sequences whole `set` calls.

use std::collections::{BTreeMap, HashMap};

use thiserror::Error;

/// Who may write. Opaque to this crate: the owner token is whatever the
/// caller uses (server source id, system tag, …) as long as it is stable.
pub type OwnerId = u64;

/// Longest accepted key, in bytes.
pub const MAX_KEY_LEN: usize = 64;
/// Longest accepted string/blob value, in bytes.
pub const MAX_BLOB_LEN: usize = 65_536;
/// Most children accepted in one array/map value.
pub const MAX_CHILDREN: usize = 1_024;
/// Deepest accepted value nesting.
pub const MAX_DEPTH: usize = 16;

/// State value. Typed (never JSON-forced): integers, floats, strings, blobs,
/// arrays, and maps round-trip exactly.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Array(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    fn validate(&self) -> Result<(), StateError> {
        self.validate_at(0)
    }

    fn validate_at(&self, depth: usize) -> Result<(), StateError> {
        if depth > MAX_DEPTH {
            return Err(StateError::TooDeep);
        }
        match self {
            Value::Str(s) => {
                if s.len() > MAX_BLOB_LEN {
                    return Err(StateError::TooLarge);
                }
                Ok(())
            }
            Value::Bytes(b) => {
                if b.len() > MAX_BLOB_LEN {
                    return Err(StateError::TooLarge);
                }
                Ok(())
            }
            Value::Array(items) => {
                if items.len() > MAX_CHILDREN {
                    return Err(StateError::TooManyChildren);
                }
                items.iter().try_for_each(|v| v.validate_at(depth + 1))
            }
            Value::Map(entries) => {
                if entries.len() > MAX_CHILDREN {
                    return Err(StateError::TooManyChildren);
                }
                entries.iter().try_for_each(|(k, v)| {
                    validate_key(k)?;
                    v.validate_at(depth + 1)
                })
            }
            _ => Ok(()),
        }
    }
}

fn validate_key(key: &str) -> Result<(), StateError> {
    if key.is_empty() {
        return Err(StateError::BadKey("key is empty".into()));
    }
    if key.len() > MAX_KEY_LEN {
        return Err(StateError::BadKey("key too long".into()));
    }
    if key.contains('\0') {
        return Err(StateError::BadKey("key contains NUL".into()));
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StateError {
    #[error("invalid key: {0}")]
    BadKey(String),
    #[error("value too large")]
    TooLarge,
    #[error("too many array/map children")]
    TooManyChildren,
    #[error("value nested too deep")]
    TooDeep,
    #[error("writer {0} is not the bag owner")]
    NotOwner(OwnerId),
    #[error("no such handler")]
    NoSuchHandler,
}

/// Who may mutate a bag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WritePolicy {
    /// Any writer accepted.
    Open,
    /// Only the recorded owner. No owner recorded means nobody may write.
    OwnerOnly,
}

/// A mutation observed by handlers.
#[derive(Debug, Clone, PartialEq)]
pub struct ChangeEvent {
    pub bag: String,
    pub key: String,
    pub old: Option<Value>,
    pub new: Option<Value>,
    /// Per-bag sequence after this mutation.
    pub seq: u64,
}

type Handler = Box<dyn FnMut(&ChangeEvent)>;

/// One named key/value store with versions, policy, and handlers.
pub struct StateBag {
    id: String,
    policy: WritePolicy,
    owner: Option<OwnerId>,
    seq: u64,
    values: BTreeMap<String, (Value, bool)>,
    handlers: BTreeMap<u64, (Option<String>, Handler)>,
    next_handler: u64,
}

impl StateBag {
    pub fn new(id: &str, policy: WritePolicy) -> Self {
        StateBag {
            id: id.to_string(),
            policy,
            owner: None,
            seq: 0,
            values: BTreeMap::new(),
            handlers: BTreeMap::new(),
            next_handler: 0,
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    /// Record (or clear, via `None`) the owner. Owner assignment itself is
    /// the caller's authorization decision; this crate only enforces it.
    pub fn set_owner(&mut self, owner: Option<OwnerId>) {
        self.owner = owner;
    }

    pub fn owner(&self) -> Option<OwnerId> {
        self.owner
    }

    /// Write a key. Replicated unless later excluded via `set_replicated`.
    /// Returns the new per-bag sequence.
    pub fn set(&mut self, key: &str, value: Value, writer: OwnerId) -> Result<u64, StateError> {
        self.check_write(writer)?;
        validate_key(key)?;
        value.validate()?;
        let replicated = self.values.get(key).map(|(_, r)| *r).unwrap_or(true);
        self.apply(key, value, replicated)
    }

    /// Flip a key's replication flag. Unknown keys are created as Nil first
    /// so the flag has something to attach to — explicit, not magic.
    pub fn set_replicated(&mut self, key: &str, replicated: bool, writer: OwnerId) -> Result<(), StateError> {
        self.check_write(writer)?;
        validate_key(key)?;
        if let Some(slot) = self.values.get_mut(key) {
            slot.1 = replicated;
        } else {
            self.values.insert(key.to_string(), (Value::Nil, replicated));
            self.seq += 1;
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.values.get(key).map(|(v, _)| v)
    }

    /// Delete a key. Missing keys are a no-op returning `false`.
    pub fn delete(&mut self, key: &str, writer: OwnerId) -> Result<bool, StateError> {
        self.check_write(writer)?;
        validate_key(key)?;
        if self.values.remove(key).is_none() {
            return Ok(false);
        }
        self.seq += 1;
        let event = ChangeEvent { bag: self.id.clone(), key: key.to_string(), old: None, new: None, seq: self.seq };
        self.dispatch(event);
        Ok(true)
    }

    /// Replicated snapshot: sorted `(key, value, seq-at-write… )`. The seq
    /// reported is the bag's current seq; per-key versions are the values'
    /// insertion order, which BTreeMap iteration keeps stable for diffing.
    pub fn snapshot_replicated(&self) -> Vec<(String, Value)> {
        self.values.iter().filter(|(_, v)| v.1).map(|(k, (v, _))| (k.clone(), v.clone())).collect()
    }

    /// Subscribe to changes. `key_filter` scopes to one key; `None` gets all.
    /// Returns a handler id for [`StateBag::remove_handler`].
    pub fn on_change(&mut self, key_filter: Option<&str>, handler: Handler) -> Result<u64, StateError> {
        if let Some(k) = key_filter {
            validate_key(k)?;
        }
        let id = self.next_handler;
        self.next_handler += 1;
        self.handlers.insert(id, (key_filter.map(str::to_string), handler));
        Ok(id)
    }

    pub fn remove_handler(&mut self, id: u64) -> Result<(), StateError> {
        self.handlers.remove(&id).map(|_| ()).ok_or(StateError::NoSuchHandler)
    }

    fn check_write(&self, writer: OwnerId) -> Result<(), StateError> {
        match self.policy {
            WritePolicy::Open => Ok(()),
            WritePolicy::OwnerOnly => match self.owner {
                Some(o) if o == writer => Ok(()),
                _ => Err(StateError::NotOwner(writer)),
            },
        }
    }

    fn apply(&mut self, key: &str, value: Value, replicated: bool) -> Result<u64, StateError> {
        let old = self.values.get(key).map(|(v, _)| v.clone());
        self.values.insert(key.to_string(), (value.clone(), replicated));
        self.seq += 1;
        let event = ChangeEvent { bag: self.id.clone(), key: key.to_string(), old, new: Some(value), seq: self.seq };
        self.dispatch(event);
        Ok(self.seq)
    }

    /// Serve matching handlers in subscription-id order. Ids are snapshotted
    /// first because the borrow checker cannot know handlers are pure
    /// observers (they receive only `&ChangeEvent`).
    fn dispatch(&mut self, event: ChangeEvent) {
        let ids: Vec<u64> = self
            .handlers
            .iter()
            .filter(|(_, (f, _))| f.as_deref().is_none_or(|k| k == event.key))
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            if let Some((_, h)) = self.handlers.get_mut(&id) {
                h(&event);
            }
        }
    }
}

/// All bags on this server, plus the shared global bag id.
#[derive(Default)]
pub struct Registry {
    bags: HashMap<String, StateBag>,
}

impl Registry {
    pub fn new() -> Self {
        Registry::default()
    }

    pub fn get_or_create(&mut self, id: &str, policy: WritePolicy) -> &mut StateBag {
        self.bags.entry(id.to_string()).or_insert_with(|| StateBag::new(id, policy))
    }

    pub fn get(&self, id: &str) -> Option<&StateBag> {
        self.bags.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut StateBag> {
        self.bags.get_mut(id)
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.bags.remove(id).is_some()
    }

    pub fn bag_count(&self) -> usize {
        self.bags.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn set_get_roundtrip_all_types() {
        let mut b = StateBag::new("global", WritePolicy::Open);
        let cases: Vec<(&str, Value)> = vec![
            ("nil", Value::Nil),
            ("bool", Value::Bool(true)),
            ("int", Value::Int(-42)),
            ("float", Value::Float(1.5)),
            ("str", Value::Str("hi".into())),
            ("bytes", Value::Bytes(vec![0, 255])),
            ("arr", Value::Array(vec![Value::Int(1), Value::Str("x".into())])),
            ("map", Value::Map(BTreeMap::from([("k".to_string(), Value::Bool(false))]))),
        ];
        for (k, v) in &cases {
            b.set(k, v.clone(), 0).unwrap();
        }
        for (k, v) in &cases {
            assert_eq!(b.get(k), Some(v), "{k}");
        }
        assert_eq!(b.seq(), cases.len() as u64);
    }

    #[test]
    fn bad_keys_rejected() {
        let mut b = StateBag::new("g", WritePolicy::Open);
        assert!(matches!(b.set("", Value::Nil, 0), Err(StateError::BadKey(_))));
        assert!(matches!(b.set("a\0b", Value::Nil, 0), Err(StateError::BadKey(_))));
        assert!(matches!(b.set(&"k".repeat(65), Value::Nil, 0), Err(StateError::BadKey(_))));
        assert_eq!(b.seq(), 0);
    }

    #[test]
    fn oversized_values_rejected() {
        let mut b = StateBag::new("g", WritePolicy::Open);
        assert_eq!(b.set("s", Value::Str("x".repeat(MAX_BLOB_LEN + 1)), 0), Err(StateError::TooLarge));
        assert_eq!(b.set("y", Value::Bytes(vec![0; MAX_BLOB_LEN + 1]), 0), Err(StateError::TooLarge));
        assert_eq!(b.set("a", Value::Array(vec![Value::Nil; MAX_CHILDREN + 1]), 0), Err(StateError::TooManyChildren));
        // Nesting one past the limit.
        let mut deep = Value::Int(0);
        for _ in 0..=MAX_DEPTH {
            deep = Value::Array(vec![deep]);
        }
        assert_eq!(b.set("d", deep, 0), Err(StateError::TooDeep));
        assert_eq!(b.seq(), 0);
    }

    #[test]
    fn owner_only_fails_closed() {
        let mut b = StateBag::new("e1", WritePolicy::OwnerOnly);
        // No owner recorded: nobody writes.
        assert_eq!(b.set("hp", Value::Int(100), 7), Err(StateError::NotOwner(7)));
        b.set_owner(Some(7));
        assert_eq!(b.set("hp", Value::Int(100), 9), Err(StateError::NotOwner(9)));
        b.set("hp", Value::Int(100), 7).unwrap();
        assert_eq!(b.get("hp"), Some(&Value::Int(100)));
        // Clearing the owner locks the bag again.
        b.set_owner(None);
        assert_eq!(b.delete("hp", 7), Err(StateError::NotOwner(7)));
    }

    #[test]
    #[allow(clippy::type_complexity)]
    fn handlers_see_old_and_new_in_order() {
        let mut b = StateBag::new("g", WritePolicy::Open);
        let seen: Rc<RefCell<Vec<(Option<Value>, Option<Value>)>>> = Rc::new(RefCell::new(vec![]));
        let probe = Rc::clone(&seen);
        let order: Rc<RefCell<Vec<u64>>> = Rc::new(RefCell::new(vec![]));
        let order_p = Rc::clone(&order);
        let order_check = Rc::clone(&order);
        b.on_change(
            None,
            Box::new(move |e| {
                probe.borrow_mut().push((e.old.clone(), e.new.clone()));
            }),
        )
        .unwrap();
        b.on_change(
            Some("hp"),
            Box::new(move |_| {
                order_p.borrow_mut().push(2);
            }),
        )
        .unwrap();
        b.on_change(
            None,
            Box::new(move |_| {
                order.borrow_mut().push(3);
            }),
        )
        .unwrap();
        b.set("hp", Value::Int(1), 0).unwrap();
        b.set("hp", Value::Int(2), 0).unwrap();
        b.set("other", Value::Bool(true), 0).unwrap();
        let seen = seen.borrow();
        assert_eq!(seen.len(), 3);
        assert_eq!(seen[0], (None, Some(Value::Int(1))));
        assert_eq!(seen[1], (Some(Value::Int(1)), Some(Value::Int(2))));
        assert_eq!(seen[2], (None, Some(Value::Bool(true))));
        // Filtered handler fired only for "hp" (2 of 3 events); ids ascend:
        // hp -> [2, 3], hp -> [2, 3], other -> [3].
        assert_eq!(*order_check.borrow(), vec![2, 3, 2, 3, 3]);
    }

    #[test]
    fn handler_removal_and_unknown_id() {
        let mut b = StateBag::new("g", WritePolicy::Open);
        let id = b.on_change(None, Box::new(|_| {})).unwrap();
        b.remove_handler(id).unwrap();
        assert_eq!(b.remove_handler(id), Err(StateError::NoSuchHandler));
        assert_eq!(b.remove_handler(999), Err(StateError::NoSuchHandler));
    }

    #[test]
    fn events_carry_bag_key_and_seq() {
        let mut b = StateBag::new("entity:12", WritePolicy::Open);
        let seen: Rc<RefCell<Vec<ChangeEvent>>> = Rc::new(RefCell::new(vec![]));
        let probe = Rc::clone(&seen);
        b.on_change(None, Box::new(move |e| probe.borrow_mut().push(e.clone()))).unwrap();
        let s1 = b.set("hp", Value::Int(1), 0).unwrap();
        let s2 = b.set("hp", Value::Int(2), 0).unwrap();
        assert_eq!((s1, s2), (1, 2));
        let seen = seen.borrow();
        assert_eq!(seen.len(), 2);
        assert!(seen.iter().all(|e| e.bag == "entity:12" && e.key == "hp"));
        assert_eq!((seen[0].seq, seen[1].seq), (1, 2));
    }

    #[test]
    fn replication_flag_filters_snapshot() {
        let mut b = StateBag::new("g", WritePolicy::Open);
        b.set("shared", Value::Int(1), 0).unwrap();
        b.set("scratch", Value::Int(2), 0).unwrap();
        b.set_replicated("scratch", false, 0).unwrap();
        let snap = b.snapshot_replicated();
        assert_eq!(snap, vec![("shared".to_string(), Value::Int(1))]);
        // Unknown key materializes as Nil so the flag sticks.
        b.set_replicated("future", false, 0).unwrap();
        assert_eq!(b.get("future"), Some(&Value::Nil));
    }

    #[test]
    fn delete_reports_and_notifies() {
        let mut b = StateBag::new("g", WritePolicy::Open);
        assert!(!b.delete("missing", 0).unwrap());
        b.set("k", Value::Int(1), 0).unwrap();
        let seen: Rc<RefCell<Vec<ChangeEvent>>> = Rc::new(RefCell::new(vec![]));
        let probe = Rc::clone(&seen);
        b.on_change(Some("k"), Box::new(move |e| probe.borrow_mut().push(e.clone()))).unwrap();
        assert!(b.delete("k", 0).unwrap());
        assert_eq!(b.get("k"), None);
        let seen = seen.borrow();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].new, None);
    }

    #[test]
    fn registry_lifecycle() {
        let mut r = Registry::new();
        r.get_or_create("global", WritePolicy::Open).set("m", Value::Int(1), 0).unwrap();
        assert_eq!(r.bag_count(), 1);
        assert!(r.get("global").is_some());
        assert!(r.remove("global"));
        assert!(!r.remove("global"));
        assert_eq!(r.bag_count(), 0);
    }
}
