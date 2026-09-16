//! Asset residency: Mounted/Registered is not ready — the game must hold it.
//!
//! After `ald-mount` registers an asset, this tracker walks the residency
//! handshake on a caller-supplied virtual tick clock:
//!
//! ```text
//! Requested -> Waiting -> Resident -> Ready
//! ```
//!
//! - `request()` asks the game bridge (through [`GameCommit`]) to take
//!   residency. `Accepted` moves to `Resident`; `Retryable` parks in
//!   `Waiting` with a deadline; `Fatal` ends in `Failed`.
//! - `poll()` on `Waiting` past its deadline re-requests (bounded by
//!   `max_retries`) or ends `Expired` when retries run out. No indefinite
//!   waits — the tracker reports, the caller recovers (re-mount, re-fetch).
//! - `poll()` on `Resident` asks the bridge to confirm; `true` moves to
//!   `Ready`, `false` keeps waiting under the same deadline rules.
//! - Only `Ready` means the asset may be used. Mount != ready, enforced by
//!   type and test, not by comment.
//!
//! [`GameCommit`] is implemented by the game bridge in a later phase; tests
//! use a scripted fake that counts calls, proving the tracker calls the
//! bridge exactly on request and retry — never in a poll spin.

use std::collections::HashMap;

use thiserror::Error;

/// Bridge answer to a residency request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitOutcome {
    Accepted,
    Retryable,
    Fatal,
}

/// The game-side half of the handshake. Implemented by the bridge later;
/// faked in tests.
pub trait GameCommit {
    fn request_residency(&mut self, asset_id: &str) -> CommitOutcome;
    fn confirm_resident(&mut self, asset_id: &str) -> bool;
}

/// Residency state. Terminal: Ready, Failed, Expired, Cancelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResidencyState {
    Requested,
    Waiting { deadline_tick: u64 },
    Resident { deadline_tick: u64 },
    Ready,
    Failed { reason: String },
    Expired,
    Cancelled,
}

impl ResidencyState {
    pub fn terminal(&self) -> bool {
        matches!(
            self,
            ResidencyState::Ready | ResidencyState::Failed { .. } | ResidencyState::Expired | ResidencyState::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ResidencyError {
    #[error("unknown residency request '{0}'")]
    Unknown(String),
    #[error("residency request '{0}' is terminal")]
    Terminal(String),
    #[error("residency request '{0}' already tracked")]
    Duplicate(String),
}

struct Request {
    state: ResidencyState,
    retries_used: u32,
    max_retries: u32,
    timeout_ticks: u64,
}

/// Tracks residency handshakes to READY on a virtual tick clock.
#[derive(Default)]
pub struct ResidencyTracker {
    requests: HashMap<String, Request>,
}

impl ResidencyTracker {
    pub fn new() -> Self {
        ResidencyTracker::default()
    }

    /// Start tracking. Immediately calls the bridge once; the outcome sets
    /// the first state. Duplicate ids are refused (re-requests go through
    /// `poll`, not a second `request`).
    pub fn request(
        &mut self,
        asset_id: &str,
        now_tick: u64,
        timeout_ticks: u64,
        max_retries: u32,
        commit: &mut impl GameCommit,
    ) -> Result<ResidencyState, ResidencyError> {
        if self.requests.contains_key(asset_id) {
            return Err(ResidencyError::Duplicate(asset_id.to_string()));
        }
        let timeout_ticks = timeout_ticks.max(1);
        let mut req = Request { state: ResidencyState::Requested, retries_used: 0, max_retries, timeout_ticks };
        req.state = match commit.request_residency(asset_id) {
            CommitOutcome::Accepted => ResidencyState::Resident { deadline_tick: now_tick + timeout_ticks },
            CommitOutcome::Retryable => ResidencyState::Waiting { deadline_tick: now_tick + timeout_ticks },
            CommitOutcome::Fatal => ResidencyState::Failed { reason: "bridge refused residency".into() },
        };
        let state = req.state.clone();
        self.requests.insert(asset_id.to_string(), req);
        Ok(state)
    }

    /// Advance one request. Terminal states report without touching the bridge.
    pub fn poll(
        &mut self,
        asset_id: &str,
        now_tick: u64,
        commit: &mut impl GameCommit,
    ) -> Result<ResidencyState, ResidencyError> {
        let req = self.requests.get_mut(asset_id).ok_or_else(|| ResidencyError::Unknown(asset_id.to_string()))?;
        match req.state.clone() {
            s if s.terminal() => Ok(s),
            ResidencyState::Requested => {
                // Unreachable through the public API (`request` resolves it
                // immediately), but handled, not panicked, in case of drift.
                req.state = ResidencyState::Waiting { deadline_tick: now_tick + req.timeout_ticks };
                Ok(req.state.clone())
            }
            ResidencyState::Waiting { deadline_tick } => {
                if now_tick < deadline_tick {
                    return Ok(req.state.clone());
                }
                if req.retries_used >= req.max_retries {
                    req.state = ResidencyState::Expired;
                } else {
                    req.retries_used += 1;
                    req.state = match commit.request_residency(asset_id) {
                        CommitOutcome::Accepted => {
                            ResidencyState::Resident { deadline_tick: now_tick + req.timeout_ticks }
                        }
                        CommitOutcome::Retryable => {
                            ResidencyState::Waiting { deadline_tick: now_tick + req.timeout_ticks }
                        }
                        CommitOutcome::Fatal => {
                            ResidencyState::Failed { reason: "bridge refused residency on retry".into() }
                        }
                    };
                }
                Ok(req.state.clone())
            }
            ResidencyState::Resident { deadline_tick } => {
                if commit.confirm_resident(asset_id) {
                    req.state = ResidencyState::Ready;
                } else if now_tick >= deadline_tick {
                    req.state = ResidencyState::Expired;
                }
                Ok(req.state.clone())
            }
            // Terminal arms are matched above via the guard; this is belt
            // and braces for exhaustiveness under refactor.
            s => Ok(s),
        }
    }

    /// Cancel a non-terminal request.
    pub fn cancel(&mut self, asset_id: &str) -> Result<(), ResidencyError> {
        let req = self.requests.get_mut(asset_id).ok_or_else(|| ResidencyError::Unknown(asset_id.to_string()))?;
        if req.state.terminal() {
            return Err(ResidencyError::Terminal(asset_id.to_string()));
        }
        req.state = ResidencyState::Cancelled;
        Ok(())
    }

    pub fn state_of(&self, asset_id: &str) -> Option<&ResidencyState> {
        self.requests.get(asset_id).map(|r| &r.state)
    }

    pub fn retries_used(&self, asset_id: &str) -> Option<u32> {
        self.requests.get(asset_id).map(|r| r.retries_used)
    }

    /// Drop terminal requests (hygiene for long-lived trackers). Returns ids.
    pub fn reap_terminal(&mut self) -> Vec<String> {
        let dead: Vec<String> =
            self.requests.iter().filter(|(_, r)| r.state.terminal()).map(|(id, _)| id.clone()).collect();
        for id in &dead {
            self.requests.remove(id);
        }
        dead
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// Scripted bridge: programmed outcomes plus call counts.
    struct FakeCommit {
        requests: VecDeque<CommitOutcome>,
        confirm: VecDeque<bool>,
        request_calls: usize,
        confirm_calls: usize,
        default: CommitOutcome,
    }

    impl FakeCommit {
        fn accept() -> Self {
            FakeCommit {
                requests: VecDeque::new(),
                confirm: VecDeque::from([true]),
                request_calls: 0,
                confirm_calls: 0,
                default: CommitOutcome::Accepted,
            }
        }
    }

    impl GameCommit for FakeCommit {
        fn request_residency(&mut self, _asset_id: &str) -> CommitOutcome {
            self.request_calls += 1;
            self.requests.pop_front().unwrap_or(self.default)
        }

        fn confirm_resident(&mut self, _asset_id: &str) -> bool {
            self.confirm_calls += 1;
            self.confirm.pop_front().unwrap_or(false)
        }
    }

    #[test]
    fn happy_path_to_ready() {
        let mut t = ResidencyTracker::new();
        let mut bridge = FakeCommit::accept();
        assert!(matches!(t.request("a", 0, 10, 3, &mut bridge), Ok(ResidencyState::Resident { .. })));
        assert_eq!(t.poll("a", 1, &mut bridge).unwrap(), ResidencyState::Ready);
        assert_eq!(bridge.request_calls, 1);
        // Terminal: bridge untouched afterwards.
        assert_eq!(t.poll("a", 100, &mut bridge).unwrap(), ResidencyState::Ready);
        assert_eq!(bridge.confirm_calls, 1);
    }

    #[test]
    fn retry_then_accept() {
        let mut t = ResidencyTracker::new();
        let mut bridge = FakeCommit {
            requests: VecDeque::from([CommitOutcome::Retryable, CommitOutcome::Accepted]),
            confirm: VecDeque::from([true]),
            request_calls: 0,
            confirm_calls: 0,
            default: CommitOutcome::Retryable,
        };
        assert!(matches!(t.request("a", 0, 10, 3, &mut bridge), Ok(ResidencyState::Waiting { .. })));
        // Before deadline: no bridge call, still waiting.
        assert!(matches!(t.poll("a", 5, &mut bridge), Ok(ResidencyState::Waiting { .. })));
        assert_eq!(bridge.request_calls, 1);
        // At deadline: retry fires exactly once.
        assert!(matches!(t.poll("a", 10, &mut bridge), Ok(ResidencyState::Resident { .. })));
        assert_eq!(bridge.request_calls, 2);
        assert_eq!(t.retries_used("a"), Some(1));
        assert_eq!(t.poll("a", 11, &mut bridge).unwrap(), ResidencyState::Ready);
    }

    #[test]
    fn retries_exhausted_expires() {
        let mut t = ResidencyTracker::new();
        let mut bridge = FakeCommit {
            requests: VecDeque::new(),
            confirm: VecDeque::new(),
            request_calls: 0,
            confirm_calls: 0,
            default: CommitOutcome::Retryable,
        };
        t.request("a", 0, 5, 2, &mut bridge).unwrap();
        assert_eq!(t.poll("a", 5, &mut bridge).unwrap(), ResidencyState::Waiting { deadline_tick: 10 });
        assert_eq!(t.poll("a", 10, &mut bridge).unwrap(), ResidencyState::Waiting { deadline_tick: 15 });
        assert_eq!(t.poll("a", 15, &mut bridge).unwrap(), ResidencyState::Expired);
        assert_eq!(bridge.request_calls, 3); // initial + 2 retries, then stop
        assert_eq!(t.poll("a", 100, &mut bridge).unwrap(), ResidencyState::Expired);
        assert_eq!(bridge.request_calls, 3); // terminal: no more calls
    }

    #[test]
    fn fatal_ends_failed() {
        let mut t = ResidencyTracker::new();
        let mut bridge = FakeCommit {
            requests: VecDeque::from([CommitOutcome::Fatal]),
            confirm: VecDeque::new(),
            request_calls: 0,
            confirm_calls: 0,
            default: CommitOutcome::Accepted,
        };
        assert!(matches!(t.request("a", 0, 10, 5, &mut bridge), Ok(ResidencyState::Failed { .. })));
    }

    #[test]
    fn unconfirmed_resident_expires() {
        let mut t = ResidencyTracker::new();
        let mut bridge = FakeCommit {
            requests: VecDeque::new(),
            confirm: VecDeque::new(), // always false
            request_calls: 0,
            confirm_calls: 0,
            default: CommitOutcome::Accepted,
        };
        t.request("a", 0, 5, 3, &mut bridge).unwrap();
        assert!(matches!(t.poll("a", 1, &mut bridge).unwrap(), ResidencyState::Resident { .. }));
        assert_eq!(t.poll("a", 5, &mut bridge).unwrap(), ResidencyState::Expired);
    }

    #[test]
    fn cancel_and_duplicates() {
        let mut t = ResidencyTracker::new();
        let mut bridge = FakeCommit::accept();
        // accept() confirms true on first confirm; drain it for this test.
        bridge.confirm.clear();
        t.request("a", 0, 100, 3, &mut bridge).unwrap();
        t.cancel("a").unwrap();
        assert_eq!(t.state_of("a"), Some(&ResidencyState::Cancelled));
        assert_eq!(t.cancel("a"), Err(ResidencyError::Terminal("a".into())));
        assert_eq!(t.request("a", 0, 10, 1, &mut bridge), Err(ResidencyError::Duplicate("a".into())));
        assert_eq!(t.poll("ghost", 0, &mut bridge), Err(ResidencyError::Unknown("ghost".into())));
        assert_eq!(t.retries_used("ghost"), None);
    }

    #[test]
    fn reap_terminal_hygiene() {
        let mut t = ResidencyTracker::new();
        let mut bridge = FakeCommit::accept();
        t.request("a", 0, 10, 1, &mut bridge).unwrap();
        t.poll("a", 1, &mut bridge).unwrap();
        let mut bridge2 = FakeCommit {
            requests: VecDeque::from([CommitOutcome::Fatal]),
            confirm: VecDeque::new(),
            request_calls: 0,
            confirm_calls: 0,
            default: CommitOutcome::Accepted,
        };
        t.request("b", 0, 10, 1, &mut bridge2).unwrap();
        let mut reaped = t.reap_terminal();
        reaped.sort();
        assert_eq!(reaped, vec!["a".to_string(), "b".to_string()]);
        assert_eq!(t.state_of("a"), None);
    }

    #[test]
    fn zero_timeout_clamped() {
        let mut t = ResidencyTracker::new();
        let mut bridge = FakeCommit {
            requests: VecDeque::from([CommitOutcome::Retryable, CommitOutcome::Accepted]),
            confirm: VecDeque::new(),
            request_calls: 0,
            confirm_calls: 0,
            default: CommitOutcome::Retryable,
        };
        t.request("a", 0, 0, 1, &mut bridge).unwrap(); // clamped to 1
        assert!(matches!(t.poll("a", 1, &mut bridge).unwrap(), ResidencyState::Resident { .. }));
    }
}
