//! Aegis alerting and incident timeline.
//!
//! Alerts fire on operational signals: server offline, high CPU, tick
//! degradation, crash loops, database failure, packet loss, latency, disk.
//! Each alert is deduplicated so a persistent condition raises one open
//! alert rather than a flood, and it is resolvable when the condition clears.
//!
//! The incident timeline correlates otherwise separate events — a deployment,
//! a config change, a resource restart, a player spike, a performance
//! anomaly — into one ordered view, because the most common cause of an
//! incident at 3am is "we deployed something".

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Alert severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Severity {
    Info,
    Warning,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Info => "INFO",
            Severity::Warning => "WARNING",
            Severity::Critical => "CRITICAL",
        }
    }
}

/// Conditions Aegis can raise an alert on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AlertKind {
    ServerOffline,
    HighCpu,
    HighMemory,
    TickDegradation,
    ResourceCrashLoop,
    DatabaseFailure,
    NetworkPacketLoss,
    HighLatency,
    DiskLow,
}

impl AlertKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AlertKind::ServerOffline => "server_offline",
            AlertKind::HighCpu => "high_cpu",
            AlertKind::HighMemory => "high_memory",
            AlertKind::TickDegradation => "tick_degradation",
            AlertKind::ResourceCrashLoop => "resource_crash_loop",
            AlertKind::DatabaseFailure => "database_failure",
            AlertKind::NetworkPacketLoss => "network_packet_loss",
            AlertKind::HighLatency => "high_latency",
            AlertKind::DiskLow => "disk_low",
        }
    }

    pub fn default_severity(self) -> Severity {
        match self {
            AlertKind::ServerOffline | AlertKind::DatabaseFailure | AlertKind::ResourceCrashLoop => Severity::Critical,
            AlertKind::HighCpu
            | AlertKind::HighMemory
            | AlertKind::TickDegradation
            | AlertKind::NetworkPacketLoss
            | AlertKind::DiskLow => Severity::Warning,
            AlertKind::HighLatency => Severity::Info,
        }
    }
}

/// One alert occurrence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub kind: AlertKind,
    pub severity: Severity,
    /// First time this condition was observed.
    pub first_fired: u64,
    /// Most recent time it was still observed.
    pub last_fired: u64,
    /// How many times the condition was re-evaluated as still true.
    pub occurrences: u64,
    pub message: String,
    /// Which resource/server this applies to.
    pub subject: String,
    /// None while the condition is still active.
    pub resolved_at: Option<u64>,
}

/// The alert manager: deduplicated, resolvable.
pub struct Alerts {
    open: HashMap<(String, AlertKind), Alert>,
    resolved: Vec<Alert>,
    /// Webhook URLs to notify on state transitions.
    webhooks: Vec<String>,
}

impl Alerts {
    pub fn new() -> Self {
        Alerts { open: HashMap::new(), resolved: Vec::new(), webhooks: Vec::new() }
    }

    /// Register a webhook endpoint for alert notifications.
    pub fn add_webhook(&mut self, url: String) {
        if !url.trim().is_empty() && !self.webhooks.contains(&url) {
            self.webhooks.push(url);
        }
    }

    pub fn webhooks(&self) -> &[String] {
        &self.webhooks
    }

    /// Report that a condition currently holds. Creates a new alert or
    /// refreshes an existing one. Returns whether this was a new alert.
    pub fn fire(&mut self, subject: &str, kind: AlertKind, message: &str, now: u64) -> bool {
        let key = (subject.to_string(), kind);
        let is_new = !self.open.contains_key(&key);
        let alert = self.open.entry(key).or_insert_with(|| Alert {
            kind,
            severity: kind.default_severity(),
            first_fired: now,
            last_fired: now,
            occurrences: 0,
            message: message.into(),
            subject: subject.into(),
            resolved_at: None,
        });
        alert.last_fired = now;
        alert.occurrences += 1;
        alert.message = message.into();
        is_new
    }

    /// Mark a condition cleared. A re-fire afterwards opens a fresh alert.
    pub fn resolve(&mut self, subject: &str, kind: AlertKind, now: u64) -> bool {
        let key = (subject.to_string(), kind);
        match self.open.remove(&key) {
            Some(mut alert) => {
                alert.resolved_at = Some(now);
                self.resolved.push(alert);
                true
            }
            None => false,
        }
    }

    pub fn open_alerts(&self) -> Vec<&Alert> {
        let mut all: Vec<&Alert> = self.open.values().collect();
        all.sort_by(|a, b| b.severity_rank().cmp(&a.severity_rank()).then(b.last_fired.cmp(&a.last_fired)));
        all
    }

    pub fn resolved_alerts(&self) -> &[Alert] {
        &self.resolved
    }

    pub fn open_count(&self) -> usize {
        self.open.len()
    }

    /// Check every open alert against a health probe; resolve any whose
    /// condition no longer holds.
    ///
    /// The probe answers "is this condition still true right now?" for a
    /// given (subject, kind). Conditions the probe cannot evaluate are left
    /// alone rather than auto-resolved — silently clearing an alert that may
    /// still be real is worse than leaving it open.
    pub fn reconcile<F>(&mut self, now: u64, mut still_true: F)
    where
        F: FnMut(&str, AlertKind) -> Option<bool>,
    {
        let keys: Vec<(String, AlertKind)> = self.open.keys().cloned().collect();
        for (subject, kind) in keys {
            match still_true(&subject, kind) {
                Some(false) => {
                    self.resolve(&subject, kind, now);
                }
                Some(true) | None => {}
            }
        }
    }
}

impl Default for Alerts {
    fn default() -> Self {
        Alerts::new()
    }
}

impl Alert {
    fn severity_rank(&self) -> u8 {
        match self.severity {
            Severity::Critical => 2,
            Severity::Warning => 1,
            Severity::Info => 0,
        }
    }
}

/// A timeline event for incident correlation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    pub timestamp: u64,
    pub kind: TimelineKind,
    pub actor: Option<String>,
    pub subject: String,
    pub detail: String,
}

/// The categories of event the timeline correlates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineKind {
    Deployment,
    ConfigChange,
    ResourceRestart,
    ServerRestart,
    PlayerSpike,
    PerformanceAnomaly,
    Crash,
    Alert,
    Audit,
}

/// An ordered incident timeline.
#[derive(Debug, Default)]
pub struct IncidentTimeline {
    events: Vec<TimelineEvent>,
    capacity: usize,
}

impl IncidentTimeline {
    pub fn new(capacity: usize) -> Self {
        IncidentTimeline { events: Vec::new(), capacity: capacity.max(1) }
    }

    pub fn record(&mut self, event: TimelineEvent) {
        self.events.push(event);
        if self.events.len() > self.capacity {
            // Drop the oldest, keeping recent context.
            self.events.remove(0);
        }
        self.events.sort_by_key(|e| e.timestamp);
    }

    pub fn events(&self) -> &[TimelineEvent] {
        &self.events
    }

    /// Everything that happened in a window around a given time.
    pub fn around(&self, t: u64, window_secs: u64) -> Vec<&TimelineEvent> {
        self.events.iter().filter(|e| e.timestamp.abs_diff(t) <= window_secs).collect()
    }

    /// The event immediately before `t`, if any — usually "what did we just
    /// change?".
    pub fn immediately_before(&self, t: u64) -> Option<&TimelineEvent> {
        self.events.iter().filter(|e| e.timestamp <= t).max_by_key(|e| e.timestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_fire_creates_alert() {
        let mut a = Alerts::new();
        assert!(a.fire("srv-1", AlertKind::HighCpu, "cpu 92%", 100));
        assert_eq!(a.open_count(), 1);
    }

    #[test]
    fn repeat_fire_deduplicates_and_counts() {
        let mut a = Alerts::new();
        a.fire("srv-1", AlertKind::HighCpu, "cpu 90%", 100);
        let is_new = a.fire("srv-1", AlertKind::HighCpu, "cpu 95%", 200);
        assert!(!is_new);
        let open = a.open_alerts();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].occurrences, 2);
        assert_eq!(open[0].last_fired, 200);
        assert_eq!(open[0].message, "cpu 95%");
    }

    #[test]
    fn different_subjects_are_separate_alerts() {
        let mut a = Alerts::new();
        a.fire("srv-1", AlertKind::HighCpu, "x", 1);
        a.fire("srv-2", AlertKind::HighCpu, "x", 1);
        assert_eq!(a.open_count(), 2);
    }

    #[test]
    fn resolve_moves_alert_out_of_open() {
        let mut a = Alerts::new();
        a.fire("srv-1", AlertKind::HighCpu, "x", 100);
        assert!(a.resolve("srv-1", AlertKind::HighCpu, 200));
        assert_eq!(a.open_count(), 0);
        assert_eq!(a.resolved_alerts().len(), 1);
        assert_eq!(a.resolved_alerts()[0].resolved_at, Some(200));
    }

    #[test]
    fn resolve_unknown_alert_is_false() {
        let mut a = Alerts::new();
        assert!(!a.resolve("srv-1", AlertKind::HighCpu, 1));
    }

    #[test]
    fn refire_after_resolve_creates_new_alert() {
        let mut a = Alerts::new();
        a.fire("srv-1", AlertKind::HighCpu, "x", 100);
        a.resolve("srv-1", AlertKind::HighCpu, 200);
        assert!(a.fire("srv-1", AlertKind::HighCpu, "x", 300));
        assert_eq!(a.open_count(), 1);
    }

    #[test]
    fn critical_alerts_sort_first() {
        let mut a = Alerts::new();
        a.fire("s", AlertKind::HighLatency, "x", 1);
        a.fire("s", AlertKind::ServerOffline, "x", 2);
        a.fire("s", AlertKind::HighCpu, "x", 3);
        let open = a.open_alerts();
        assert_eq!(open[0].kind, AlertKind::ServerOffline);
    }

    #[test]
    fn reconcile_resolves_cleared_conditions() {
        let mut a = Alerts::new();
        a.fire("s", AlertKind::HighCpu, "x", 1);
        a.fire("s", AlertKind::HighMemory, "x", 1);
        a.reconcile(100, |subject, kind| {
            if kind == AlertKind::HighCpu {
                Some(false) // cleared
            } else if kind == AlertKind::HighMemory {
                Some(true) // still high
            } else {
                None
            }
        });
        assert_eq!(a.open_count(), 1);
        assert!(matches!(a.open_alerts()[0].kind, AlertKind::HighMemory));
    }

    #[test]
    fn reconcile_leaves_unknown_conditions_open() {
        // A probe that cannot answer must not auto-resolve anything.
        let mut a = Alerts::new();
        a.fire("s", AlertKind::DatabaseFailure, "x", 1);
        a.reconcile(100, |_, _| None);
        assert_eq!(a.open_count(), 1);
    }

    #[test]
    fn webhooks_dedup_and_ignore_empty() {
        let mut a = Alerts::new();
        a.add_webhook("https://hooks.example.com/n1".into());
        a.add_webhook("https://hooks.example.com/n1".into());
        a.add_webhook("  ".into());
        assert_eq!(a.webhooks().len(), 1);
    }

    #[test]
    fn timeline_orders_events() {
        let mut t = IncidentTimeline::new(16);
        t.record(TimelineEvent {
            timestamp: 300,
            kind: TimelineKind::Crash,
            actor: None,
            subject: "srv".into(),
            detail: "tick stall".into(),
        });
        t.record(TimelineEvent {
            timestamp: 100,
            kind: TimelineKind::Deployment,
            actor: Some("dev".into()),
            subject: "inventory".into(),
            detail: "v2".into(),
        });
        let ts: Vec<u64> = t.events().iter().map(|e| e.timestamp).collect();
        assert_eq!(ts, vec![100, 300]);
    }

    #[test]
    fn timeline_window_around_an_incident() {
        let mut t = IncidentTimeline::new(16);
        t.record(TimelineEvent {
            timestamp: 100,
            kind: TimelineKind::Deployment,
            actor: None,
            subject: "a".into(),
            detail: "".into(),
        });
        t.record(TimelineEvent {
            timestamp: 150,
            kind: TimelineKind::Crash,
            actor: None,
            subject: "a".into(),
            detail: "".into(),
        });
        t.record(TimelineEvent {
            timestamp: 900,
            kind: TimelineKind::Alert,
            actor: None,
            subject: "a".into(),
            detail: "".into(),
        });
        let near = t.around(150, 60);
        assert_eq!(near.len(), 2);
    }

    #[test]
    fn immediately_before_finds_the_change() {
        let mut t = IncidentTimeline::new(16);
        t.record(TimelineEvent {
            timestamp: 100,
            kind: TimelineKind::ConfigChange,
            actor: None,
            subject: "a".into(),
            detail: "".into(),
        });
        t.record(TimelineEvent {
            timestamp: 900,
            kind: TimelineKind::Crash,
            actor: None,
            subject: "a".into(),
            detail: "".into(),
        });
        let before = t.immediately_before(500).unwrap();
        assert_eq!(before.kind, TimelineKind::ConfigChange);
    }

    #[test]
    fn timeline_is_bounded() {
        let mut t = IncidentTimeline::new(3);
        for i in 0..10 {
            t.record(TimelineEvent {
                timestamp: i,
                kind: TimelineKind::Alert,
                actor: None,
                subject: "a".into(),
                detail: "".into(),
            });
        }
        assert_eq!(t.events().len(), 3);
    }

    #[test]
    fn alert_severities() {
        assert_eq!(AlertKind::ServerOffline.default_severity(), Severity::Critical);
        assert_eq!(AlertKind::HighCpu.default_severity(), Severity::Warning);
        assert_eq!(AlertKind::HighLatency.default_severity(), Severity::Info);
        assert_eq!(AlertKind::DatabaseFailure.as_str(), "database_failure");
    }
}
