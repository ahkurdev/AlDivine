//! Entity ownership transfer: compare-and-swap with anti-theft rules.
//!
//! The [`Authority`] on an entity says who simulates it. Changing that owner
//! is a security-sensitive operation — a client must never seize an entity
//! from the server or another player — so transfer is a compare-and-swap:
//! the caller names the expected current owner, and the swap lands only if
//! it matches. Stale entity ids fail with [`TransferOutcome::NotFound`];
//! generation safety comes from the [`EntityStore`] itself.
//!
//! Transfer rules (enforced, not conventional):
//! - `Server -> anyone`: allowed. The server delegates.
//! - `X -> Server` (X = Player/Resource): allowed. Owners may release.
//! - `Player(a) -> Player(b)`, `Resource(a) -> Resource(b)`: **refused**
//!   with [`TransferRefusal::MustReleaseFirst`]. Migration is release +
//!   re-acquire as two auditable steps, mediated by the server — never a
//!   direct lateral handoff one client can trigger against another.
//! - `Player <-> Resource` cross transfers: refused the same way, except via
//!   the server (`Server` is the hub all ownership flows through).
//! - `X -> X` (no-op): allowed, reports [`TransferOutcome::Transferred`]
//!   with identical from/to so callers need no special case.

use ald_core::EntityId;

use crate::{Authority, EntityStore};

/// Why a transfer was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferRefusal {
    /// Expected current owner did not match the entity's actual owner.
    OwnerMismatch { expected: Authority, actual: Authority },
    /// Direct lateral handoff. Release to the server first.
    MustReleaseFirst { from: Authority, to: Authority },
}

/// Outcome of [`transfer`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferOutcome {
    Transferred {
        from: Authority,
        to: Authority,
    },
    Refused {
        reason: TransferRefusal,
    },
    /// Entity unknown or id stale (destroyed and slot reused).
    NotFound,
}

/// Compare-and-swap an entity's owner. See the module docs for the rules.
pub fn transfer(store: &mut EntityStore, id: EntityId, expected: Authority, next: Authority) -> TransferOutcome {
    let entity = match store.get_mut(id) {
        Some(e) => e,
        None => return TransferOutcome::NotFound,
    };
    let actual = entity.authority;
    if actual != expected {
        return TransferOutcome::Refused { reason: TransferRefusal::OwnerMismatch { expected, actual } };
    }
    if actual != Authority::Server && next != Authority::Server && actual != next {
        return TransferOutcome::Refused { reason: TransferRefusal::MustReleaseFirst { from: actual, to: next } };
    }
    entity.authority = next;
    TransferOutcome::Transferred { from: actual, to: next }
}

/// Current owner, or `None` for unknown/stale ids.
pub fn owner_of(store: &EntityStore, id: EntityId) -> Option<Authority> {
    store.get(id).map(|e| e.authority)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_delegates_to_player() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Server);
        assert_eq!(
            transfer(&mut s, id, Authority::Server, Authority::Player(7)),
            TransferOutcome::Transferred { from: Authority::Server, to: Authority::Player(7) }
        );
        assert_eq!(owner_of(&s, id), Some(Authority::Player(7)));
    }

    #[test]
    fn player_releases_to_server() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Player(7));
        assert_eq!(
            transfer(&mut s, id, Authority::Player(7), Authority::Server),
            TransferOutcome::Transferred { from: Authority::Player(7), to: Authority::Server }
        );
    }

    #[test]
    fn player_to_player_refused() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Player(7));
        assert_eq!(
            transfer(&mut s, id, Authority::Player(7), Authority::Player(9)),
            TransferOutcome::Refused {
                reason: TransferRefusal::MustReleaseFirst { from: Authority::Player(7), to: Authority::Player(9) }
            }
        );
        // Owner unchanged by the refusal.
        assert_eq!(owner_of(&s, id), Some(Authority::Player(7)));
    }

    #[test]
    fn mediated_migration_release_then_acquire() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Player(7));
        assert!(matches!(
            transfer(&mut s, id, Authority::Player(7), Authority::Server),
            TransferOutcome::Transferred { .. }
        ));
        assert!(matches!(
            transfer(&mut s, id, Authority::Server, Authority::Player(9)),
            TransferOutcome::Transferred { .. }
        ));
        assert_eq!(owner_of(&s, id), Some(Authority::Player(9)));
    }

    #[test]
    fn cas_mismatch_refused() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Player(7));
        // Attacker (or stale caller) claims the server owns it: refused, and
        // the refusal names the actual owner for audit.
        assert_eq!(
            transfer(&mut s, id, Authority::Server, Authority::Player(9)),
            TransferOutcome::Refused {
                reason: TransferRefusal::OwnerMismatch { expected: Authority::Server, actual: Authority::Player(7) }
            }
        );
        assert_eq!(owner_of(&s, id), Some(Authority::Player(7)));
    }

    #[test]
    fn stale_id_is_not_found() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Player(7));
        assert!(s.destroy(id));
        assert_eq!(transfer(&mut s, id, Authority::Player(7), Authority::Server), TransferOutcome::NotFound);
        assert_eq!(owner_of(&s, id), None);
        // Slot reuse does not resurrect the old owner's claim.
        let id2 = s.spawn(Authority::Server);
        assert_ne!(id.generation, id2.generation);
        assert_eq!(transfer(&mut s, id, Authority::Player(7), Authority::Server), TransferOutcome::NotFound);
    }

    #[test]
    fn resource_rules_mirror_player() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Resource(3));
        // Resource -> Resource refused; via server allowed.
        assert!(matches!(
            transfer(&mut s, id, Authority::Resource(3), Authority::Resource(4)),
            TransferOutcome::Refused { reason: TransferRefusal::MustReleaseFirst { .. } }
        ));
        assert!(matches!(
            transfer(&mut s, id, Authority::Resource(3), Authority::Server),
            TransferOutcome::Transferred { .. }
        ));
        assert!(matches!(
            transfer(&mut s, id, Authority::Server, Authority::Resource(4)),
            TransferOutcome::Transferred { .. }
        ));
    }

    #[test]
    fn self_transfer_is_noop_success() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Player(7));
        assert_eq!(
            transfer(&mut s, id, Authority::Player(7), Authority::Player(7)),
            TransferOutcome::Transferred { from: Authority::Player(7), to: Authority::Player(7) }
        );
    }

    #[test]
    fn destroy_drops_ownership() {
        let mut s = EntityStore::new();
        let id = s.spawn(Authority::Player(7));
        assert!(s.destroy(id));
        assert_eq!(owner_of(&s, id), None);
    }
}
