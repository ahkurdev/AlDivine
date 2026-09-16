//! Client join driver: Handshake → Deferrals → Queued → Downloading →
//! Residency → Ready, fed by server messages on a virtual tick clock.
//!
//! The background connect task owns the socket; this driver owns the *order*
//! of joining. Every server message enters through [`JoinDriver::apply`],
//! which validates that the message belongs to the current phase —
//! out-of-order messages are rejected, never silently absorbed. Terminal
//! states stick; late messages after Ready/Failed report instead of
//! corrupting a finished join.
//!
//! Stall policy lives here too: each phase has a tick budget without server
//! events ([`JoinDriver::check_stall`]). The driver reports; the connect
//! task fails the join and surfaces the phase, so "stuck on Downloading"
//! is a diagnosable state, not a frozen screen.
//!
//! What this driver does NOT do: move bytes or touch the game. Downloads
//! land through the transport into `ald-download`; residency completes in
//! the game bridge. The driver tracks the handshake around them.

use thiserror::Error;

/// Ticks without a server event before a phase reads stalled.
pub const STALL_BUDGET_TICKS: u64 = 300;

/// Client-visible join phase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JoinPhase {
    Handshake,
    Deferrals { message: String },
    Queued { position: usize },
    Downloading { received: u64, total: u64 },
    Residency { pending: usize },
    Ready,
    Failed { reason: String },
}

impl JoinPhase {
    pub fn terminal(&self) -> bool {
        matches!(self, JoinPhase::Ready | JoinPhase::Failed { .. })
    }
}

/// Server messages the driver consumes, in join order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    DeferralUpdate { message: String },
    DeferralDone,
    QueuePosition { position: usize },
    QueueAdmitted,
    Manifest { total_bytes: u64 },
    ChunkArrived { received: u64 },
    DownloadComplete,
    ResidencyPending { pending: usize },
    ResidencyReady,
    Rejected { reason: String },
    Kicked { reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum JoinError {
    #[error("late event after terminal state")]
    LateEvent,
    #[error("out-of-order event: {0:?} in phase {1:?}")]
    OutOfOrder(ServerEvent, String),
    #[error("unknown join driver")]
    Unknown,
}

/// The join state machine. Virtual ticks: the caller passes `now`.
pub struct JoinDriver {
    phase: JoinPhase,
    last_event_tick: u64,
    stall_budget: u64,
}

impl JoinDriver {
    pub fn new(now_tick: u64) -> Self {
        JoinDriver { phase: JoinPhase::Handshake, last_event_tick: now_tick, stall_budget: STALL_BUDGET_TICKS }
    }

    pub fn with_stall_budget(now_tick: u64, stall_budget: u64) -> Self {
        JoinDriver { phase: JoinPhase::Handshake, last_event_tick: now_tick, stall_budget: stall_budget.max(1) }
    }

    pub fn phase(&self) -> &JoinPhase {
        &self.phase
    }

    pub fn last_event_tick(&self) -> u64 {
        self.last_event_tick
    }

    /// Apply one server message. Rejections and kicks land `Failed` from any
    /// non-terminal phase; anything else must fit the current phase.
    pub fn apply(&mut self, event: ServerEvent, now_tick: u64) -> Result<(), JoinError> {
        if self.phase.terminal() {
            return Err(JoinError::LateEvent);
        }
        let next = match (&self.phase, &event) {
            (_, ServerEvent::Rejected { reason }) => Some(JoinPhase::Failed { reason: format!("rejected: {reason}") }),
            (_, ServerEvent::Kicked { reason }) => Some(JoinPhase::Failed { reason: format!("kicked: {reason}") }),
            (JoinPhase::Handshake, ServerEvent::DeferralUpdate { message }) => {
                Some(JoinPhase::Deferrals { message: message.clone() })
            }
            (JoinPhase::Handshake, ServerEvent::DeferralDone) => Some(JoinPhase::Deferrals { message: String::new() }),
            (JoinPhase::Deferrals { .. }, ServerEvent::DeferralUpdate { message }) => {
                Some(JoinPhase::Deferrals { message: message.clone() })
            }
            (JoinPhase::Deferrals { .. }, ServerEvent::DeferralDone) => None, // stay: queue position lands next
            (JoinPhase::Deferrals { .. }, ServerEvent::QueuePosition { position }) => {
                Some(JoinPhase::Queued { position: *position })
            }
            (JoinPhase::Handshake, ServerEvent::QueuePosition { position }) => {
                // No-deferral servers admit straight to queue.
                Some(JoinPhase::Queued { position: *position })
            }
            (JoinPhase::Queued { .. }, ServerEvent::QueuePosition { position }) => {
                Some(JoinPhase::Queued { position: *position })
            }
            (JoinPhase::Queued { .. }, ServerEvent::QueueAdmitted) => {
                Some(JoinPhase::Downloading { received: 0, total: 0 })
            }
            (JoinPhase::Downloading { received, .. }, ServerEvent::Manifest { total_bytes }) => {
                Some(JoinPhase::Downloading { received: *received, total: *total_bytes })
            }
            (JoinPhase::Downloading { total, .. }, ServerEvent::ChunkArrived { received }) => {
                if *received > *total && *total > 0 {
                    return Err(JoinError::OutOfOrder(event, format!("{:?}", self.phase)));
                }
                Some(JoinPhase::Downloading { received: *received, total: *total })
            }
            (JoinPhase::Downloading { .. }, ServerEvent::DownloadComplete) => Some(JoinPhase::Residency { pending: 0 }),
            (JoinPhase::Residency { .. }, ServerEvent::ResidencyPending { pending }) => {
                Some(JoinPhase::Residency { pending: *pending })
            }
            (JoinPhase::Residency { .. }, ServerEvent::ResidencyReady) => Some(JoinPhase::Ready),
            _ => return Err(JoinError::OutOfOrder(event, format!("{:?}", self.phase))),
        };
        // DeferralDone while already past deferrals resolves to "stay": model
        // it by keeping the current phase (None arm above).
        if let Some(phase) = next {
            self.phase = phase;
        }
        self.last_event_tick = now_tick;
        Ok(())
    }

    /// Cancel from any non-terminal phase.
    pub fn cancel(&mut self) {
        if !self.phase.terminal() {
            self.phase = JoinPhase::Failed { reason: "cancelled by user".into() };
        }
    }

    /// Phase name when stalled (no server event within budget), else `None`.
    /// Terminal phases never stall.
    pub fn check_stall(&self, now_tick: u64) -> Option<String> {
        if self.phase.terminal() {
            return None;
        }
        if now_tick.saturating_sub(self.last_event_tick) > self.stall_budget {
            Some(format!("{:?}", self.phase))
        } else {
            None
        }
    }

    /// 0.0–1.0 download fraction for UI. `None` outside Downloading or when
    /// the manifest has not arrived yet.
    pub fn download_fraction(&self) -> Option<f32> {
        match &self.phase {
            JoinPhase::Downloading { received, total } if *total > 0 => {
                Some((*received as f32 / *total as f32).clamp(0.0, 1.0))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn driver() -> JoinDriver {
        JoinDriver::with_stall_budget(0, 10)
    }

    #[test]
    fn happy_path_to_ready() {
        let mut d = driver();
        d.apply(ServerEvent::DeferralUpdate { message: "checking ban".into() }, 1).unwrap();
        assert_eq!(d.phase(), &JoinPhase::Deferrals { message: "checking ban".into() });
        d.apply(ServerEvent::DeferralDone, 2).unwrap();
        d.apply(ServerEvent::QueuePosition { position: 3 }, 3).unwrap();
        assert_eq!(d.phase(), &JoinPhase::Queued { position: 3 });
        d.apply(ServerEvent::QueuePosition { position: 1 }, 4).unwrap();
        d.apply(ServerEvent::QueueAdmitted, 5).unwrap();
        d.apply(ServerEvent::Manifest { total_bytes: 100 }, 6).unwrap();
        d.apply(ServerEvent::ChunkArrived { received: 40 }, 7).unwrap();
        assert_eq!(d.download_fraction(), Some(0.4));
        d.apply(ServerEvent::DownloadComplete, 8).unwrap();
        d.apply(ServerEvent::ResidencyPending { pending: 2 }, 9).unwrap();
        d.apply(ServerEvent::ResidencyReady, 10).unwrap();
        assert_eq!(d.phase(), &JoinPhase::Ready);
    }

    #[test]
    fn out_of_order_rejected() {
        let mut d = driver();
        // Chunks before admission: rejected, phase unchanged.
        assert!(matches!(d.apply(ServerEvent::ChunkArrived { received: 10 }, 1), Err(JoinError::OutOfOrder(_, _))));
        assert_eq!(d.phase(), &JoinPhase::Handshake);
        // Residency before download: rejected.
        d.apply(ServerEvent::QueuePosition { position: 1 }, 1).unwrap();
        d.apply(ServerEvent::QueueAdmitted, 2).unwrap();
        assert!(d.apply(ServerEvent::ResidencyReady, 3).is_err());
    }

    #[test]
    fn terminal_sticks_and_late_events_report() {
        let mut d = driver();
        d.apply(ServerEvent::Rejected { reason: "banned".into() }, 1).unwrap();
        assert_eq!(d.phase(), &JoinPhase::Failed { reason: "rejected: banned".into() });
        assert_eq!(d.apply(ServerEvent::QueuePosition { position: 1 }, 2), Err(JoinError::LateEvent));
        assert!(d.check_stall(10_000).is_none()); // terminal never stalls
        assert_eq!(d.download_fraction(), None);
    }

    #[test]
    fn kick_from_any_phase() {
        let mut d = driver();
        d.apply(ServerEvent::QueuePosition { position: 1 }, 1).unwrap();
        d.apply(ServerEvent::QueueAdmitted, 2).unwrap();
        d.apply(ServerEvent::Kicked { reason: "admin".into() }, 3).unwrap();
        assert_eq!(d.phase(), &JoinPhase::Failed { reason: "kicked: admin".into() });
    }

    #[test]
    fn stall_fires_and_activity_resets() {
        let mut d = driver();
        assert!(d.check_stall(10).is_none());
        assert!(d.check_stall(11).is_some());
        d.apply(ServerEvent::QueuePosition { position: 2 }, 11).unwrap();
        assert!(d.check_stall(21).is_none());
        assert!(d.check_stall(22).is_some());
    }

    #[test]
    fn chunk_beyond_manifest_rejected() {
        let mut d = driver();
        d.apply(ServerEvent::QueuePosition { position: 1 }, 0).unwrap();
        d.apply(ServerEvent::QueueAdmitted, 1).unwrap();
        d.apply(ServerEvent::Manifest { total_bytes: 100 }, 2).unwrap();
        assert!(d.apply(ServerEvent::ChunkArrived { received: 101 }, 3).is_err());
        // Before the manifest arrives total is 0: any count is accepted as
        // early progress (manifest may lag the first bytes).
        let mut d2 = driver();
        d2.apply(ServerEvent::QueuePosition { position: 1 }, 0).unwrap();
        d2.apply(ServerEvent::QueueAdmitted, 1).unwrap();
        d2.apply(ServerEvent::ChunkArrived { received: 10 }, 2).unwrap();
        assert_eq!(d2.download_fraction(), None);
    }

    #[test]
    fn cancel_ends_join() {
        let mut d = driver();
        d.apply(ServerEvent::QueuePosition { position: 1 }, 1).unwrap();
        d.cancel();
        assert!(matches!(d.phase(), JoinPhase::Failed { .. }));
        d.cancel(); // idempotent on terminal
        assert!(d.apply(ServerEvent::QueueAdmitted, 2).is_err());
    }

    #[test]
    fn zero_stall_budget_clamped() {
        let d = JoinDriver::with_stall_budget(0, 0);
        assert!(d.check_stall(1).is_none());
        assert!(d.check_stall(2).is_some());
    }
}
