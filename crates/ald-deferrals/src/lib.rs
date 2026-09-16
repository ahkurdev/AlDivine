//! Pre-join deferrals: ordered gates a connecting player passes before queue.
//!
//! Mirrors the FiveM deferral shape (present status, wait, accept, deny) with
//! server-side watchdog semantics:
//!
//! - Gates run in order. Each [`DeferralGate::check`] returns [`GateDecision`]:
//!   `Pass` advances, `Wait` pauses the session with a player-facing message,
//!   `Deny` ends it with a stable client-safe code.
//! - [`DeferralSession::poll`] drives the gates. It is synchronous on purpose:
//!   I/O-bound checks (ban lookup, entitlement refresh) run on their own task
//!   and flip gate state; the session only reads decisions. No gate may block
//!   the network loop.
//! - A poll watchdog (`max_polls`) denies stuck sessions with
//!   `ALD-QUEUE-TIMEOUT` instead of waiting forever. Gate bugs and wedged
//!   background checks surface as denials with the gate name attached, never
//!   as silent hangs.
//! - Player-facing messages are bounded ([`MAX_MESSAGE_LEN`], truncated) and
//!   defaulted when empty, so a lazy gate cannot push unbounded text or a
//!   blank loading screen.
//!
//! Denial codes are namespaced `ALD-QUEUE-*`. Internal detail stays on the
//! server: [`SessionOutcome::Denied`] carries only what the client may see
//! plus the gate name for Aegis correlation.

/// Stable denial when the watchdog fires.
pub const TIMEOUT_CODE: &str = "ALD-QUEUE-TIMEOUT";
/// Message shown when a gate waits without providing one.
pub const DEFAULT_WAIT_MESSAGE: &str = "Connecting…";
/// Player-facing messages longer than this are truncated.
pub const MAX_MESSAGE_LEN: usize = 256;

/// Who is connecting. Values come from the socket/queue, never from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferralContext {
    pub player_id: String,
    pub remote_ip: String,
    /// Waitlist position if the player already holds one, for messages.
    pub queue_position: Option<usize>,
}

/// One gate's verdict for this poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Gate passed; run the next gate.
    Pass,
    /// Gate needs more time; pause here with a player-facing message.
    Wait { message: String },
    /// Refuse the connection now. `code` is stable and client-safe.
    Deny { code: String, reason: String },
}

/// A single pre-join check. Implementations must be quick and non-blocking;
///
/// slow work happens elsewhere and the gate reports the latest known state.
pub trait DeferralGate {
    fn name(&self) -> &str;
    fn check(&mut self, ctx: &DeferralContext) -> GateDecision;
}

/// Gate adapter for closures: `FnGate::new("ban-check", |ctx| ...)`.
pub struct FnGate<F>
where
    F: FnMut(&DeferralContext) -> GateDecision,
{
    name: String,
    f: F,
}

impl<F> FnGate<F>
where
    F: FnMut(&DeferralContext) -> GateDecision,
{
    pub fn new(name: &str, f: F) -> Self {
        FnGate { name: name.to_string(), f }
    }
}

impl<F> DeferralGate for FnGate<F>
where
    F: FnMut(&DeferralContext) -> GateDecision,
{
    fn name(&self) -> &str {
        &self.name
    }

    fn check(&mut self, ctx: &DeferralContext) -> GateDecision {
        (self.f)(ctx)
    }
}

/// Outcome of [`DeferralSession::poll`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOutcome {
    /// Every gate passed. The player proceeds to the queue / login pipeline.
    Admit,
    /// A gate refused. Only client-safe fields travel to the player.
    Denied { code: String, reason: String, gate: String },
    /// Session is still working. Show `message`, attributed to `gate`.
    Waiting { message: String, gate: String },
}

/// An ordered gate run for one connecting player.
pub struct DeferralSession {
    gates: Vec<Box<dyn DeferralGate>>,
    current: usize,
    polls: u64,
    max_polls: u64,
}

impl DeferralSession {
    /// `max_polls` bounds total [`poll`](Self::poll) calls before the
    /// watchdog denies the session. Must be >= 1; 0 is clamped to 1 so a
    /// session can never be stillborn-denied without running a gate.
    pub fn new(gates: Vec<Box<dyn DeferralGate>>, max_polls: u64) -> Self {
        DeferralSession { gates, current: 0, polls: 0, max_polls: max_polls.max(1) }
    }

    /// Drive gates forward. Runs every gate from the current one until a
    /// gate waits, a gate denies, or all pass.
    pub fn poll(&mut self, ctx: &DeferralContext) -> SessionOutcome {
        self.polls += 1;
        if self.polls > self.max_polls {
            return SessionOutcome::Denied {
                code: TIMEOUT_CODE.to_string(),
                reason: "Connection timed out during pre-join checks.".to_string(),
                gate: self.gates.get(self.current).map(|g| g.name().to_string()).unwrap_or_default(),
            };
        }
        while self.current < self.gates.len() {
            let gate = &mut self.gates[self.current];
            match gate.check(ctx) {
                GateDecision::Pass => self.current += 1,
                GateDecision::Wait { message } => {
                    return SessionOutcome::Waiting { message: bound_message(&message), gate: gate.name().to_string() };
                }
                GateDecision::Deny { code, reason } => {
                    return SessionOutcome::Denied {
                        code,
                        reason: bound_message(&reason),
                        gate: gate.name().to_string(),
                    };
                }
            }
        }
        SessionOutcome::Admit
    }

    /// Polls consumed so far (watchdog accounting, exposed for Aegis).
    pub fn polls_used(&self) -> u64 {
        self.polls
    }

    /// Gates passed so far.
    pub fn gates_passed(&self) -> usize {
        self.current
    }

    pub fn gate_count(&self) -> usize {
        self.gates.len()
    }
}

fn bound_message(raw: &str) -> String {
    let trimmed = raw.trim();
    let base = if trimmed.is_empty() { DEFAULT_WAIT_MESSAGE } else { trimmed };
    // Truncate on a char boundary, never mid-codepoint.
    if base.len() <= MAX_MESSAGE_LEN {
        return base.to_string();
    }
    let mut end = MAX_MESSAGE_LEN;
    while !base.is_char_boundary(end) {
        end -= 1;
    }
    base[..end].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;

    fn ctx() -> DeferralContext {
        DeferralContext { player_id: "p1".into(), remote_ip: "198.51.100.7".into(), queue_position: None }
    }

    fn pass_gate(name: &str) -> Box<dyn DeferralGate> {
        Box::new(FnGate::new(name, |_| GateDecision::Pass))
    }

    #[test]
    fn empty_session_admits() {
        let mut s = DeferralSession::new(vec![], 10);
        assert_eq!(s.poll(&ctx()), SessionOutcome::Admit);
    }

    #[test]
    fn all_pass_admits_and_counts() {
        let mut s = DeferralSession::new(vec![pass_gate("a"), pass_gate("b"), pass_gate("c")], 10);
        assert_eq!(s.poll(&ctx()), SessionOutcome::Admit);
        assert_eq!(s.gates_passed(), 3);
        assert_eq!(s.polls_used(), 1);
    }

    #[test]
    fn deny_stops_at_gate_with_name() {
        let mut s = DeferralSession::new(
            vec![
                pass_gate("first"),
                Box::new(FnGate::new("ban-check", |_| GateDecision::Deny {
                    code: "ALD-BAN-001".into(),
                    reason: "Banned.".into(),
                })),
                pass_gate("never-reached"),
            ],
            10,
        );
        assert_eq!(
            s.poll(&ctx()),
            SessionOutcome::Denied { code: "ALD-BAN-001".into(), reason: "Banned.".into(), gate: "ban-check".into() }
        );
        assert_eq!(s.gates_passed(), 1);
    }

    #[test]
    fn wait_pauses_and_resumes() {
        let calls = Rc::new(Cell::new(0));
        let probe = Rc::clone(&calls);
        let mut s = DeferralSession::new(
            vec![
                pass_gate("fast"),
                Box::new(FnGate::new("slow-check", move |_| {
                    calls.set(calls.get() + 1);
                    if calls.get() < 3 {
                        GateDecision::Wait { message: "Checking…".into() }
                    } else {
                        GateDecision::Pass
                    }
                })),
            ],
            10,
        );
        let c = ctx();
        assert_eq!(s.poll(&c), SessionOutcome::Waiting { message: "Checking…".into(), gate: "slow-check".into() });
        assert_eq!(s.poll(&c), SessionOutcome::Waiting { message: "Checking…".into(), gate: "slow-check".into() });
        assert_eq!(s.poll(&c), SessionOutcome::Admit);
        assert_eq!(probe.get(), 3);
        // Passed gates are not re-polled: one more poll stays Admit with no
        // extra gate calls.
        assert_eq!(s.poll(&c), SessionOutcome::Admit);
        assert_eq!(probe.get(), 3);
    }

    #[test]
    fn watchdog_denies_stuck_session() {
        let mut s = DeferralSession::new(
            vec![Box::new(FnGate::new("wedged", |_| GateDecision::Wait { message: "Soon…".into() }))],
            3,
        );
        let c = ctx();
        for _ in 0..3 {
            assert!(matches!(s.poll(&c), SessionOutcome::Waiting { .. }));
        }
        assert_eq!(
            s.poll(&c),
            SessionOutcome::Denied {
                code: TIMEOUT_CODE.into(),
                reason: "Connection timed out during pre-join checks.".into(),
                gate: "wedged".into()
            }
        );
    }

    #[test]
    fn zero_max_polls_clamps_to_one() {
        let mut s = DeferralSession::new(vec![pass_gate("a")], 0);
        assert_eq!(s.poll(&ctx()), SessionOutcome::Admit);
        // Second poll exceeds the clamped budget of 1.
        assert!(matches!(s.poll(&ctx()), SessionOutcome::Denied { code, .. } if code == TIMEOUT_CODE));
    }

    #[test]
    fn empty_wait_message_gets_default() {
        let mut s = DeferralSession::new(
            vec![Box::new(FnGate::new("lazy", |_| GateDecision::Wait { message: "   ".into() }))],
            5,
        );
        assert_eq!(
            s.poll(&ctx()),
            SessionOutcome::Waiting { message: DEFAULT_WAIT_MESSAGE.into(), gate: "lazy".into() }
        );
    }

    #[test]
    fn long_messages_truncated_on_char_boundary() {
        // Multi-byte tail: truncation must not split a codepoint.
        let long = "é".repeat(200); // 400 bytes
        let mut s = DeferralSession::new(
            vec![Box::new(FnGate::new("chatty", move |_| GateDecision::Wait { message: long.clone() }))],
            5,
        );
        match s.poll(&ctx()) {
            SessionOutcome::Waiting { message, .. } => {
                assert!(message.len() <= MAX_MESSAGE_LEN);
                assert!(message.is_char_boundary(message.len()));
                assert!(message.chars().all(|c| c == 'é'));
            }
            other => panic!("expected Waiting, got {other:?}"),
        }
    }

    #[test]
    fn context_reaches_gates() {
        let seen = Rc::new(Cell::new(false));
        let probe = Rc::clone(&seen);
        let mut s = DeferralSession::new(
            vec![Box::new(FnGate::new("spy", move |ctx| {
                probe.set(ctx.player_id == "p1" && ctx.remote_ip == "198.51.100.7");
                GateDecision::Pass
            }))],
            5,
        );
        assert_eq!(s.poll(&ctx()), SessionOutcome::Admit);
        assert!(seen.get());
    }

    #[test]
    fn denial_reason_bounded() {
        let mut s = DeferralSession::new(
            vec![Box::new(FnGate::new("loud", |_| GateDecision::Deny {
                code: "ALD-QUEUE-001".into(),
                reason: "x".repeat(10_000),
            }))],
            5,
        );
        match s.poll(&ctx()) {
            SessionOutcome::Denied { reason, .. } => assert!(reason.len() <= MAX_MESSAGE_LEN),
            other => panic!("expected Denied, got {other:?}"),
        }
    }
}
