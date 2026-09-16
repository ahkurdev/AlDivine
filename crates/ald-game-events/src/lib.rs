//! Low-level game event bridge: typed envelopes, ordered dispatch, dedup.
//!
//! The Astryn Game Event Router carries game occurrences (damage, population
//! ticks, resource lifecycle, bucket changes) from producers to subscribers
//! with two guarantees: in-order delivery per producer and at-most-once
//! delivery under retransmission.
//!
//! Honesty boundary: event NAMES come from the platform spec's catalog, but
//! payload DECODING per GTA build needs real game bytes this dev box cannot
//! produce. So payloads travel as bounded opaque bytes, and
//! [`DecoderRegistry`] routes (edition, build, event) to a decoder slot:
//! `Exact` (certified for this build), `EditionDefault` (edition-level
//! fallback, marked as such), or `Unregistered`. No decoder is faked —
//! dispatch works on envelopes today; typed field access arrives with lab
//! evidence per build.
//!
//! Ordering: each producer tags envelopes with a sequence; the router drops
//! replays (`seq <= last seen`, 32-bit wrap-aware) and buffers nothing —
//! out-of-order future sequences are delivered (the network reliability
//! layer, not this router, is responsible for gap repair).

use std::collections::{BTreeMap, HashMap};

use ald_game_editions::GameEdition;
use thiserror::Error;

/// Longest accepted payload (events are signals, not bulk transfer).
pub const MAX_PAYLOAD_LEN: usize = 64 * 1024;

/// Event names from the platform spec's catalog. Unknown names are rejected
/// at publish/subscribe time — a typo must fail closed, not vanish.
pub const KNOWN_EVENTS: &[&str] = &[
    "gameEventTriggered",
    "entityDamaged",
    "populationTick",
    "resourceLifecycle",
    "bucketChanged",
    "entityCreated",
    "entityDestroyed",
];

/// One game occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Envelope {
    pub name: String,
    pub producer: u64,
    pub seq: u32,
    pub payload: Vec<u8>,
}

impl Envelope {
    pub fn new(name: &str, producer: u64, seq: u32, payload: Vec<u8>) -> Result<Self, RouterError> {
        if !KNOWN_EVENTS.contains(&name) {
            return Err(RouterError::UnknownEvent(name.to_string()));
        }
        if payload.len() > MAX_PAYLOAD_LEN {
            return Err(RouterError::PayloadTooLarge(payload.len()));
        }
        Ok(Envelope { name: name.to_string(), producer, seq, payload })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RouterError {
    #[error("unknown game event '{0}'")]
    UnknownEvent(String),
    #[error("payload too large ({0} bytes)")]
    PayloadTooLarge(usize),
    #[error("no such subscription")]
    NoSuchSubscription,
    #[error("decoder already registered for {0:?} build {1} event '{2}'")]
    DuplicateDecoder(GameEdition, u32, String),
}

/// Which decoder serves an (edition, build, event) triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderSlot {
    /// Certified decoder for this exact build.
    Exact,
    /// Edition-level fallback. Consumers must treat fields as provisional.
    EditionDefault,
    /// Nothing registered: envelope stays opaque.
    Unregistered,
}

/// Routes (edition, build, event) to decoder slots. Exact build entries win;
/// otherwise an edition default applies; otherwise unregistered. Registration
/// records evidence strings (lab run ids) so certification is traceable.
#[derive(Debug, Default)]
pub struct DecoderRegistry {
    exact: HashMap<(GameEdition, u32, String), String>,
    edition_default: HashMap<(GameEdition, String), String>,
}

impl DecoderRegistry {
    pub fn new() -> Self {
        DecoderRegistry::default()
    }

    /// Register an exact-build decoder with its evidence reference.
    pub fn register_exact(
        &mut self,
        edition: GameEdition,
        build: u32,
        event: &str,
        evidence: &str,
    ) -> Result<(), RouterError> {
        if !KNOWN_EVENTS.contains(&event) {
            return Err(RouterError::UnknownEvent(event.to_string()));
        }
        let key = (edition, build, event.to_string());
        if self.exact.contains_key(&key) {
            return Err(RouterError::DuplicateDecoder(edition, build, event.to_string()));
        }
        self.exact.insert(key, evidence.to_string());
        Ok(())
    }

    /// Register an edition-level fallback decoder with its evidence reference.
    pub fn register_edition_default(
        &mut self,
        edition: GameEdition,
        event: &str,
        evidence: &str,
    ) -> Result<(), RouterError> {
        if !KNOWN_EVENTS.contains(&event) {
            return Err(RouterError::UnknownEvent(event.to_string()));
        }
        self.edition_default.insert((edition, event.to_string()), evidence.to_string());
        Ok(())
    }

    pub fn resolve(&self, edition: GameEdition, build: u32, event: &str) -> DecoderSlot {
        if self.exact.contains_key(&(edition, build, event.to_string())) {
            DecoderSlot::Exact
        } else if self.edition_default.contains_key(&(edition, event.to_string())) {
            DecoderSlot::EditionDefault
        } else {
            DecoderSlot::Unregistered
        }
    }

    /// Evidence reference for a registered slot, if any.
    pub fn evidence(&self, edition: GameEdition, build: u32, event: &str) -> Option<&str> {
        if let Some(e) = self.exact.get(&(edition, build, event.to_string())) {
            return Some(e);
        }
        self.edition_default.get(&(edition, event.to_string())).map(String::as_str)
    }
}

/// Subscription id for later removal.
pub type SubscriptionId = u64;

/// One subscription: event name plus handler.
pub type Subscription = (String, Box<dyn FnMut(&Envelope)>);

/// Ordered, deduplicating dispatch to subscribers.
#[derive(Default)]
pub struct Router {
    subs: BTreeMap<SubscriptionId, Subscription>,
    next_sub: SubscriptionId,
    /// Last delivered seq per producer (wrap-aware comparison).
    last_seq: HashMap<u64, u32>,
    delivered: u64,
    dropped_replay: u64,
}

impl Router {
    pub fn new() -> Self {
        Router::default()
    }

    /// Subscribe to one known event. Returns an id for `unsubscribe`.
    pub fn subscribe(
        &mut self,
        event: &str,
        handler: Box<dyn FnMut(&Envelope)>,
    ) -> Result<SubscriptionId, RouterError> {
        if !KNOWN_EVENTS.contains(&event) {
            return Err(RouterError::UnknownEvent(event.to_string()));
        }
        let id = self.next_sub;
        self.next_sub += 1;
        self.subs.insert(id, (event.to_string(), handler));
        Ok(id)
    }

    pub fn unsubscribe(&mut self, id: SubscriptionId) -> Result<(), RouterError> {
        self.subs.remove(&id).map(|_| ()).ok_or(RouterError::NoSuchSubscription)
    }

    /// Publish one envelope. Replays (`seq` not newer than the producer's
    /// last delivered, 32-bit wrap-aware) are dropped and counted.
    /// Returns the count of handlers invoked.
    pub fn publish(&mut self, env: &Envelope) -> usize {
        if let Some(last) = self.last_seq.get(&env.producer) {
            if !seq_is_newer(env.seq, *last) {
                self.dropped_replay += 1;
                return 0;
            }
        }
        self.last_seq.insert(env.producer, env.seq);
        self.delivered += 1;
        let ids: Vec<SubscriptionId> =
            self.subs.iter().filter(|(_, (n, _))| *n == env.name).map(|(id, _)| *id).collect();
        for id in &ids {
            if let Some((_, h)) = self.subs.get_mut(id) {
                h(env);
            }
        }
        ids.len()
    }

    pub fn delivered(&self) -> u64 {
        self.delivered
    }

    pub fn dropped_replay(&self) -> u64 {
        self.dropped_replay
    }
}

/// 32-bit wrap-aware "a is newer than b" (RFC 1982 style): a differs from b
/// and the forward distance is less than half the space.
fn seq_is_newer(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) < (1 << 31)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn env(name: &str, producer: u64, seq: u32) -> Envelope {
        Envelope::new(name, producer, seq, vec![1, 2]).unwrap()
    }

    #[test]
    fn unknown_names_rejected_everywhere() {
        assert_eq!(
            Envelope::new("playerShotLaser", 1, 0, vec![]),
            Err(RouterError::UnknownEvent("playerShotLaser".into()))
        );
        let mut r = Router::new();
        assert!(r.subscribe("playerShotLaser", Box::new(|_| {})).is_err());
        let mut d = DecoderRegistry::new();
        assert!(d.register_exact(GameEdition::Enhanced, 1, "playerShotLaser", "ev").is_err());
        assert_eq!(d.resolve(GameEdition::Enhanced, 1, "gameEventTriggered"), DecoderSlot::Unregistered);
    }

    #[test]
    fn oversized_payload_rejected() {
        assert_eq!(
            Envelope::new("entityDamaged", 1, 0, vec![0; MAX_PAYLOAD_LEN + 1]),
            Err(RouterError::PayloadTooLarge(MAX_PAYLOAD_LEN + 1))
        );
    }

    #[test]
    fn publish_dispatches_in_subscription_order() {
        let mut r = Router::new();
        let order: Rc<RefCell<Vec<&'static str>>> = Rc::new(RefCell::new(vec![]));
        for tag in ["first", "second"] {
            let o = Rc::clone(&order);
            r.subscribe("entityDamaged", Box::new(move |_| o.borrow_mut().push(tag))).unwrap();
        }
        // Different event: must not fire.
        let o = Rc::clone(&order);
        r.subscribe("populationTick", Box::new(move |_| o.borrow_mut().push("wrong"))).unwrap();
        assert_eq!(r.publish(&env("entityDamaged", 7, 1)), 2);
        assert_eq!(*order.borrow(), vec!["first", "second"]);
        assert_eq!(r.delivered(), 1);
    }

    #[test]
    fn replay_dropped_and_counted() {
        let mut r = Router::new();
        let n: Rc<RefCell<usize>> = Rc::new(RefCell::new(0));
        let p = Rc::clone(&n);
        r.subscribe("entityDamaged", Box::new(move |_| *p.borrow_mut() += 1)).unwrap();
        assert_eq!(r.publish(&env("entityDamaged", 7, 10)), 1);
        assert_eq!(r.publish(&env("entityDamaged", 7, 10)), 0); // exact replay
        assert_eq!(r.publish(&env("entityDamaged", 7, 9)), 0); // older
        assert_eq!(r.publish(&env("entityDamaged", 7, 11)), 1); // newer
        assert_eq!(*n.borrow(), 2);
        assert_eq!(r.dropped_replay(), 2);
        // Other producers unaffected by producer 7's seq.
        assert_eq!(r.publish(&env("entityDamaged", 8, 1)), 1);
    }

    #[test]
    fn seq_wrap_treated_as_newer() {
        let mut r = Router::new();
        r.subscribe("entityDamaged", Box::new(|_| {})).unwrap();
        assert_eq!(r.publish(&env("entityDamaged", 7, u32::MAX)), 1);
        assert_eq!(r.publish(&env("entityDamaged", 7, 0)), 1); // wraps: newer
        assert_eq!(r.publish(&env("entityDamaged", 7, u32::MAX)), 0); // replay
                                                                      // Half-space jump is NOT newer (ambiguous direction fails closed).
        let mut r2 = Router::new();
        r2.subscribe("entityDamaged", Box::new(|_| {})).unwrap();
        assert_eq!(r2.publish(&env("entityDamaged", 7, 0)), 1);
        assert_eq!(r2.publish(&env("entityDamaged", 7, 1 << 31)), 0);
    }

    #[test]
    fn unsubscribe_and_unknown_id() {
        let mut r = Router::new();
        let id = r.subscribe("entityDamaged", Box::new(|_| {})).unwrap();
        r.unsubscribe(id).unwrap();
        assert_eq!(r.unsubscribe(id), Err(RouterError::NoSuchSubscription));
        assert_eq!(r.unsubscribe(999), Err(RouterError::NoSuchSubscription));
        assert_eq!(r.publish(&env("entityDamaged", 7, 1)), 0);
    }

    #[test]
    fn decoder_exact_beats_edition_default() {
        let mut d = DecoderRegistry::new();
        d.register_edition_default(GameEdition::Enhanced, "entityDamaged", "lab-edition-run").unwrap();
        assert_eq!(d.resolve(GameEdition::Enhanced, 90001, "entityDamaged"), DecoderSlot::EditionDefault);
        d.register_exact(GameEdition::Enhanced, 90001, "entityDamaged", "lab-build-run").unwrap();
        assert_eq!(d.resolve(GameEdition::Enhanced, 90001, "entityDamaged"), DecoderSlot::Exact);
        assert_eq!(d.resolve(GameEdition::Enhanced, 90002, "entityDamaged"), DecoderSlot::EditionDefault);
        assert_eq!(d.resolve(GameEdition::Legacy, 90001, "entityDamaged"), DecoderSlot::Unregistered);
        assert_eq!(d.evidence(GameEdition::Enhanced, 90001, "entityDamaged"), Some("lab-build-run"));
        assert_eq!(d.evidence(GameEdition::Enhanced, 90002, "entityDamaged"), Some("lab-edition-run"));
        assert_eq!(d.evidence(GameEdition::Legacy, 90001, "entityDamaged"), None);
    }

    #[test]
    fn duplicate_exact_decoder_refused() {
        let mut d = DecoderRegistry::new();
        d.register_exact(GameEdition::Enhanced, 90001, "entityDamaged", "a").unwrap();
        assert_eq!(
            d.register_exact(GameEdition::Enhanced, 90001, "entityDamaged", "b"),
            Err(RouterError::DuplicateDecoder(GameEdition::Enhanced, 90001, "entityDamaged".into()))
        );
    }

    #[test]
    fn catalog_lists_spec_events() {
        assert!(KNOWN_EVENTS.contains(&"gameEventTriggered"));
        assert!(KNOWN_EVENTS.contains(&"entityDamaged"));
        assert!(!KNOWN_EVENTS.is_empty());
    }
}
