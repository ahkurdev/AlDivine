//! Aldivine Diagnostic Replay & Resource Debugger
//!
//! Provides deterministic session recording, tick-accurate event playback,
//! conditional breakpoints, state timeline scrubbing, and resource debugging.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use thiserror::Error;

/// Classification of recorded event types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ReplayEventKind {
    EventTrigger { event_name: String },
    NativeInvocation { native_hash: u64 },
    StateBagUpdate { key: String, value: String },
    EntitySpawn { entity_id: u32, model: String },
    EntityDespawn { entity_id: u32 },
    ResourceLifecycle { action: String },
}

/// A recorded timeline event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayEvent {
    pub tick: u64,
    pub resource: String,
    pub kind: ReplayEventKind,
    pub payload: String,
}

/// A complete diagnostic trace recorded during gameplay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplayRecording {
    pub session_id: String,
    pub max_tick: u64,
    pub events: Vec<ReplayEvent>,
}

impl ReplayRecording {
    pub fn new(session_id: impl Into<String>) -> Self {
        Self { session_id: session_id.into(), max_tick: 0, events: Vec::new() }
    }

    pub fn record_event(&mut self, event: ReplayEvent) {
        if event.tick > self.max_tick {
            self.max_tick = event.tick;
        }
        self.events.push(event);
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(json_str: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json_str)
    }
}

/// Why the replay debugger halted execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebuggerPauseReason {
    TickBreakpoint(u64),
    EventBreakpoint(String),
    StepCompleted,
    EndOfRecording,
}

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("seek out of bounds: target {target}, max {max}")]
    SeekOutOfBounds { target: u64, max: u64 },
}

/// Interactive resource debugger stepping through recorded timelines.
pub struct ReplayDebugger {
    recording: ReplayRecording,
    current_index: usize,
    current_tick: u64,
    tick_breakpoints: HashSet<u64>,
    event_breakpoints: HashSet<String>,
    live_entities: HashMap<u32, String>,
}

impl ReplayDebugger {
    pub fn new(recording: ReplayRecording) -> Self {
        Self {
            recording,
            current_index: 0,
            current_tick: 0,
            tick_breakpoints: HashSet::new(),
            event_breakpoints: HashSet::new(),
            live_entities: HashMap::new(),
        }
    }

    pub fn current_tick(&self) -> u64 {
        self.current_tick
    }

    pub fn live_entities(&self) -> &HashMap<u32, String> {
        &self.live_entities
    }

    pub fn add_tick_breakpoint(&mut self, tick: u64) {
        self.tick_breakpoints.insert(tick);
    }

    pub fn add_event_breakpoint(&mut self, event_name: impl Into<String>) {
        self.event_breakpoints.insert(event_name.into());
    }

    /// Step forward by a single event.
    pub fn step(&mut self) -> Option<&ReplayEvent> {
        if self.current_index >= self.recording.events.len() {
            return None;
        }

        let tick = self.recording.events[self.current_index].tick;
        let kind = self.recording.events[self.current_index].kind.clone();
        self.current_tick = tick;
        self.apply_event_state(&kind);
        self.current_index += 1;
        Some(&self.recording.events[self.current_index - 1])
    }

    /// Continue execution until a breakpoint or the end of recording.
    pub fn continue_exec(&mut self) -> DebuggerPauseReason {
        while self.current_index < self.recording.events.len() {
            let tick = self.recording.events[self.current_index].tick;
            let kind = self.recording.events[self.current_index].kind.clone();
            self.current_tick = tick;

            // Check tick breakpoint
            if self.tick_breakpoints.contains(&tick) {
                return DebuggerPauseReason::TickBreakpoint(tick);
            }

            // Check event name breakpoint
            if let ReplayEventKind::EventTrigger { ref event_name } = kind {
                if self.event_breakpoints.contains(event_name) {
                    return DebuggerPauseReason::EventBreakpoint(event_name.clone());
                }
            }

            self.apply_event_state(&kind);
            self.current_index += 1;
        }

        DebuggerPauseReason::EndOfRecording
    }

    /// Fast-forward or rewind timeline to a specific tick.
    pub fn seek(&mut self, target_tick: u64) -> Result<(), ReplayError> {
        if target_tick > self.recording.max_tick && !self.recording.events.is_empty() {
            return Err(ReplayError::SeekOutOfBounds { target: target_tick, max: self.recording.max_tick });
        }

        // Reset state and fast-forward to target_tick
        self.current_index = 0;
        self.current_tick = 0;
        self.live_entities.clear();

        while self.current_index < self.recording.events.len() {
            let tick = self.recording.events[self.current_index].tick;
            if tick > target_tick {
                break;
            }
            let kind = self.recording.events[self.current_index].kind.clone();
            self.current_tick = tick;
            self.apply_event_state(&kind);
            self.current_index += 1;
        }

        Ok(())
    }

    fn apply_event_state(&mut self, kind: &ReplayEventKind) {
        match kind {
            ReplayEventKind::EntitySpawn { entity_id, ref model } => {
                self.live_entities.insert(*entity_id, model.clone());
            }
            ReplayEventKind::EntityDespawn { entity_id } => {
                self.live_entities.remove(entity_id);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_recording() -> ReplayRecording {
        let mut rec = ReplayRecording::new("sess-1");
        rec.record_event(ReplayEvent {
            tick: 1,
            resource: "spawn".to_string(),
            kind: ReplayEventKind::EntitySpawn { entity_id: 10, model: "adder".to_string() },
            payload: "{}".to_string(),
        });
        rec.record_event(ReplayEvent {
            tick: 2,
            resource: "chat".to_string(),
            kind: ReplayEventKind::EventTrigger { event_name: "chat:message".to_string() },
            payload: "hello".to_string(),
        });
        rec.record_event(ReplayEvent {
            tick: 5,
            resource: "spawn".to_string(),
            kind: ReplayEventKind::EntityDespawn { entity_id: 10 },
            payload: "{}".to_string(),
        });
        rec
    }

    #[test]
    fn record_and_replay_sequential_ticks() {
        let rec = make_test_recording();
        assert_eq!(rec.max_tick, 5);
        assert_eq!(rec.events.len(), 3);
    }

    #[test]
    fn debugger_step_advances_one_tick() {
        let rec = make_test_recording();
        let mut dbg = ReplayDebugger::new(rec);

        let ev1 = dbg.step().unwrap();
        assert_eq!(ev1.tick, 1);
        assert_eq!(dbg.current_tick(), 1);

        let ev2 = dbg.step().unwrap();
        assert_eq!(ev2.tick, 2);
        assert_eq!(dbg.current_tick(), 2);
    }

    #[test]
    fn breakpoint_pauses_execution_at_target_tick() {
        let rec = make_test_recording();
        let mut dbg = ReplayDebugger::new(rec);
        dbg.add_tick_breakpoint(2);

        let reason = dbg.continue_exec();
        assert_eq!(reason, DebuggerPauseReason::TickBreakpoint(2));
        assert_eq!(dbg.current_tick(), 2);
    }

    #[test]
    fn event_breakpoint_pauses_on_matching_event() {
        let rec = make_test_recording();
        let mut dbg = ReplayDebugger::new(rec);
        dbg.add_event_breakpoint("chat:message");

        let reason = dbg.continue_exec();
        assert_eq!(reason, DebuggerPauseReason::EventBreakpoint("chat:message".to_string()));
    }

    #[test]
    fn seek_forward_reconstructs_entity_state() {
        let rec = make_test_recording();
        let mut dbg = ReplayDebugger::new(rec);

        // At tick 3, entity 10 has spawned but not despawned
        dbg.seek(3).unwrap();
        assert_eq!(dbg.current_tick(), 2);
        assert_eq!(dbg.live_entities().get(&10), Some(&"adder".to_string()));

        // At tick 5, entity 10 despawned
        dbg.seek(5).unwrap();
        assert_eq!(dbg.live_entities().get(&10), None);
    }

    #[test]
    fn seek_backward_resets_timeline() {
        let rec = make_test_recording();
        let mut dbg = ReplayDebugger::new(rec);

        dbg.seek(5).unwrap();
        assert_eq!(dbg.live_entities().get(&10), None);

        dbg.seek(1).unwrap();
        assert_eq!(dbg.live_entities().get(&10), Some(&"adder".to_string()));
    }

    #[test]
    fn filtering_events_by_resource() {
        let rec = make_test_recording();
        let chat_events: Vec<_> = rec.events.iter().filter(|e| e.resource == "chat").collect();
        assert_eq!(chat_events.len(), 1);
        assert_eq!(chat_events[0].tick, 2);
    }

    #[test]
    fn json_serialization_roundtrip_of_replay_recording() {
        let rec = make_test_recording();
        let json = rec.to_json().unwrap();
        let parsed = ReplayRecording::from_json(&json).unwrap();
        assert_eq!(rec, parsed);
    }

    #[test]
    fn empty_recording_handles_step_gracefully() {
        let rec = ReplayRecording::new("empty");
        let mut dbg = ReplayDebugger::new(rec);

        assert_eq!(dbg.step(), None);
        assert_eq!(dbg.continue_exec(), DebuggerPauseReason::EndOfRecording);
    }

    #[test]
    fn debugger_tracks_entity_spawns_and_despawns() {
        let mut rec = ReplayRecording::new("entities");
        for id in 1..=3 {
            rec.record_event(ReplayEvent {
                tick: id as u64,
                resource: "world".to_string(),
                kind: ReplayEventKind::EntitySpawn { entity_id: id, model: format!("car_{}", id) },
                payload: "{}".to_string(),
            });
        }

        let mut dbg = ReplayDebugger::new(rec);
        dbg.seek(2).unwrap();
        assert_eq!(dbg.live_entities().len(), 2);
    }
}
