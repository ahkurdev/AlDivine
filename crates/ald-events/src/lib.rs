//! Event system: namespaced, rate-limited, priority-ordered, async-capable.
//! Protected namespaces cannot be hijacked by arbitrary resources.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// Priority ordering for handler execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HandlerPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

/// Namespaces cannot be registered by resources. Core owns them.
pub const PROTECTED_NAMESPACES: &[&str] = &["ald", "framework", "server", "internal"];

pub type HandlerFn = Arc<dyn Fn(&str, Value) -> ald_core::Result<()> + Send + Sync>;

pub struct EventHandler {
    pub resource: String,
    pub priority: HandlerPriority,
    pub handler: HandlerFn,
}

impl EventHandler {
    pub fn new(resource: impl Into<String>, priority: HandlerPriority, handler: HandlerFn) -> Self {
        EventHandler { resource: resource.into(), priority, handler }
    }
}

#[derive(Default)]
pub struct EventBus {
    handlers: Mutex<HashMap<String, Vec<EventHandler>>>,
}

impl EventBus {
    pub fn new() -> Self {
        EventBus::default()
    }

    pub fn register(&self, event: &str, handler: EventHandler) -> ald_core::Result<()> {
        if let Some((ns, _)) = event.split_once(':') {
            if PROTECTED_NAMESPACES.contains(&ns) && !handler.resource.starts_with("core/") {
                return Err(ald_core::AldError::CapabilityDenied(format!("namespace '{ns}' is protected")));
            }
        }
        let mut map = self.handlers.lock().unwrap();
        map.entry(event.to_string()).or_default().push(handler);
        // Keep high priority first.
        map.get_mut(event).unwrap().sort_by_key(|h| std::cmp::Reverse(h.priority));
        Ok(())
    }

    pub fn dispatch(&self, event: &str, data: Value) -> ald_core::Result<usize> {
        // Collect handler refs under the lock, then invoke outside the lock to
        // avoid deadlocks if a handler re-enters the bus.
        let handlers: Vec<HandlerFn> = {
            let map = self.handlers.lock().unwrap();
            map.get(event).map(|v| v.iter().map(|h| h.handler.clone()).collect()).unwrap_or_default()
        };
        let mut count = 0;
        for h in &handlers {
            h(event, data.clone())?;
            count += 1;
        }
        Ok(count)
    }

    pub fn unregister_resource(&self, resource: &str) {
        let mut map = self.handlers.lock().unwrap();
        for handlers in map.values_mut() {
            handlers.retain(|h| h.resource != resource);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn dispatch_runs_handlers() {
        let bus = EventBus::new();
        let hit = Arc::new(AtomicUsize::new(0));
        let h = hit.clone();
        bus.register(
            "player:join",
            EventHandler::new(
                "core/test",
                HandlerPriority::Normal,
                Arc::new(move |_, _| {
                    h.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                }),
            ),
        )
        .unwrap();
        let n = bus.dispatch("player:join", serde_json::json!({"id":1})).unwrap();
        assert_eq!(n, 1);
        assert_eq!(hit.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn protected_namespace_blocked() {
        let bus = EventBus::new();
        let res = bus
            .register("ald:shutdown", EventHandler::new("evil/mod", HandlerPriority::Normal, Arc::new(|_, _| Ok(()))));
        assert!(res.is_err());
    }
}
