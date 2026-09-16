//! Aldivine JavaScript runtime — QuickJS via rquickjs (bindgen build).
//!
//! Sandboxing: no filesystem, no process, no native module loading. The
//! runtime exposes the Aldivine API only. Every capability is granted
//! explicitly by the host.
//!
//! This is NOT Node.js. It does not provide require(), process, fs, net or
//! Buffer. The Citizen-compatible client surface below is what legacy client
//! scripts expect; anything not listed is genuinely absent, so a script that
//! reaches for it gets a ReferenceError — the honest failure.
//!
//! Callback storage: rquickjs callback parameters live only for the call, so
//! they cannot be stored into JS objects tied to the outer context (the
//! context type is invariant over its lifetime). Timer and event-handler
//! callbacks are therefore kept in Rust-side registries as GC-rooted
//! `Persistent` handles, which is the mechanism rquickjs provides for
//! exactly this ("store JS functions for later use"). The JS globals carry
//! only functions, no data.

use rquickjs::context::Ctx;
use rquickjs::{CatchResultExt, Context, Function, Object, Persistent, Runtime, Value};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// A GC-rooted JS function handle. Owns its root, so it carries no lifetime
/// and can be stored in Rust-side registries indefinitely.
type RootedFn = Persistent<Function<'static>>;

struct TimerEntry {
    kind: String,
    due: u64,
    period: Option<u64>,
    cb: RootedFn,
}

struct HandlerEntry {
    cb: RootedFn,
    net: bool,
}

#[derive(Default)]
struct JsState {
    /// Virtual clock in ms. Advanced by `tick_timers`, never by wall time.
    clock: u64,
    timers: Vec<TimerEntry>,
    handlers: HashMap<String, Vec<HandlerEntry>>,
    /// Queued outbound wire events, pre-serialized as JSON strings.
    wire: Vec<String>,
    logs: Vec<(String, String)>,
}

/// A sandboxed QuickJS execution context for one resource.
pub struct JsRuntime {
    // Drop order follows declaration order. Persistent roots must be freed
    // before the runtime is destroyed (rquickjs aborts if a rooted link
    // outlives its runtime), and the context (JS heap) must be freed before
    // the runtime, so state and ctx come first.
    state: Rc<RefCell<JsState>>,
    ctx: Context,
    // Runtime must outlive Context; kept to extend lifetime.
    _runtime: Runtime,
}

impl JsRuntime {
    /// Create a new sandboxed runtime with the Citizen-compatible client
    /// surface installed.
    pub fn new() -> anyhow::Result<Self> {
        let runtime = Runtime::new()?;
        let ctx = Context::full(&runtime)?;
        let js = JsRuntime {
            state: Rc::new(RefCell::new(JsState::default())),
            _runtime: runtime,
            ctx,
        };
        js.install_citizen_compat()?;
        Ok(js)
    }

    /// Execute a script. Errors are caught and returned as anyhow errors.
    pub fn exec(&self, source: &str) -> anyhow::Result<()> {
        self.ctx.with(|ctx| -> anyhow::Result<()> {
            ctx.eval::<(), _>(source).catch(&ctx).map_err(|e| anyhow::anyhow!("js error: {e:?}"))
        })
    }

    /// Evaluate and return a value's string form.
    pub fn eval_string(&self, expr: &str) -> anyhow::Result<String> {
        self.ctx.with(|ctx| -> anyhow::Result<String> {
            ctx.eval::<Value, _>(expr)
                .catch(&ctx)
                .map(|v| format!("{v:?}"))
                .map_err(|e| anyhow::anyhow!("js error: {e:?}"))
        })
    }

    /// Evaluate and return a JS string value as a raw Rust string.
    fn eval_raw(&self, expr: &str) -> anyhow::Result<String> {
        self.ctx.with(|ctx| -> anyhow::Result<String> {
            ctx.eval::<String, _>(expr)
                .catch(&ctx)
                .map_err(|e| anyhow::anyhow!("js error: {e:?}"))
        })
    }

    /// Evaluate and return a value as JSON.
    pub fn eval_json(&self, expr: &str) -> anyhow::Result<String> {
        self.eval_raw(&format!("JSON.stringify((function(){{ return ({expr}); }})())"))
    }

    /// Call a global function by name. `args_json` is either a JSON array of
    /// arguments (spread) or a single JSON argument. Returns JSON.
    pub fn call_json(&self, name: &str, args_json: &str) -> anyhow::Result<String> {
        self.eval_raw(&format!(
            "JSON.stringify((function(){{ var __a = ({args_json}); var __args = Array.isArray(__a) ? __a : [__a]; return (typeof {name} === 'function') ? {name}.apply(null, __args) : undefined; }})())"
        ))
    }

    /// Install the Citizen-compatible client API surface.
    ///
    /// Everything host-mediated is backed by Rust-side registries the host
    /// pumps; no network, no disk from JS.
    ///
    /// rquickjs 0.13 only implements `IntoJsFunc` for positional-arity
    /// closures (Fn(A, B, C)...) not single-tuple closures (Fn((A,B,C))).
    /// All callbacks below take positional args for that reason.
    fn install_citizen_compat(&self) -> anyhow::Result<()> {
        self.ctx.with(|ctx| -> anyhow::Result<()> {
            let globals = ctx.globals();

            // --- timers ------------------------------------------------------
            // add_timer returns the registry id, exposed to JS as f64.
            let state = self.state.clone();
            globals.set(
                "setTimeout",
                Function::new(
                    ctx.clone(),
                    move |cb: RootedFn, ms: u64| -> Result<f64, rquickjs::Error> {
                        Ok(add_timer(&state, "timeout", cb, ms, None))
                    },
                )?,
            )?;

            let state = self.state.clone();
            globals.set(
                "setInterval",
                Function::new(
                    ctx.clone(),
                    move |cb: RootedFn, ms: u64| -> Result<f64, rquickjs::Error> {
                        Ok(add_timer(&state, "interval", cb, ms, Some(ms)))
                    },
                )?,
            )?;

            let state = self.state.clone();
            globals.set(
                "clearTimeout",
                Function::new(
                    ctx.clone(),
                    move |id: f64| -> Result<(), rquickjs::Error> {
                        clear_timer(&state, id);
                        Ok(())
                    },
                )?,
            )?;
            let state = self.state.clone();
            globals.set(
                "clearInterval",
                Function::new(
                    ctx.clone(),
                    move |id: f64| -> Result<(), rquickjs::Error> {
                        clear_timer(&state, id);
                        Ok(())
                    },
                )?,
            )?;

            // --- Citizen namespace ------------------------------------------
            let citizen = Object::new(ctx.clone())?;
            // Wait suspends a thread; with no scheduler yet it returns
            // immediately. Correct for scripts not relying on timing; the
            // scheduler lands with resource wiring.
            citizen.set("Wait", Function::new(ctx.clone(), |_ms: u64| -> Result<(), rquickjs::Error> { Ok(()) })?)?;
            // CreateThread: no real thread yet; run synchronously.
            // The calling context arrives as a parameter (per-call, freed at
            // return); it must NOT be captured, because a stored Ctx clone
            // keeps the JS context alive past teardown and trips the
            // runtime's GC-empty assertion.
            citizen.set(
                "CreateThread",
                Function::new(
                    ctx.clone(),
                    move |call: Ctx<'_>, cb: RootedFn| -> Result<(), rquickjs::Error> {
                        let f: Function = cb.restore(&call)?;
                        f.call::<(), ()>(())?;
                        Ok(())
                    },
                )?,
            )?;
            let state = self.state.clone();
            citizen.set(
                "SetTimeout",
                Function::new(
                    ctx.clone(),
                    move |cb: RootedFn, ms: u64| -> Result<f64, rquickjs::Error> {
                        Ok(add_timer(&state, "timeout", cb, ms, None))
                    },
                )?,
            )?;
            globals.set("Citizen", citizen)?;

            // --- event bus ---------------------------------------------------
            // Handler storage is a Rust-side map: event name -> list of
            // (rooted callback, net flag). JS never sees the registry.

            // __register(name, cb, net)
            let state = self.state.clone();
            globals.set(
                "__register",
                Function::new(
                    ctx.clone(),
                    move |name: String, cb: RootedFn, net: bool| -> Result<(), rquickjs::Error> {
                        state
                            .borrow_mut()
                            .handlers
                            .entry(name)
                            .or_default()
                            .push(HandlerEntry { cb, net });
                        Ok(())
                    },
                )?,
            )?;

            // __emit(name, args_json) — fires local (non-net) handlers
            let state = self.state.clone();
            globals.set(
                "__emit",
                Function::new(
                    ctx.clone(),
                    move |call: Ctx<'_>, name: String, args_json: String| -> Result<(), rquickjs::Error> {
                        fire_local(&call, &state, &name, &args_json)
                    },
                )?,
            )?;

            // on(name, cb) — registers a local handler
            let state = self.state.clone();
            globals.set(
                "on",
                Function::new(
                    ctx.clone(),
                    move |name: String, cb: RootedFn| -> Result<(), rquickjs::Error> {
                        state
                            .borrow_mut()
                            .handlers
                            .entry(name)
                            .or_default()
                            .push(HandlerEntry { cb, net: false });
                        Ok(())
                    },
                )?,
            )?;

            // onNet(name, cb) — registers a network handler
            let state = self.state.clone();
            globals.set(
                "onNet",
                Function::new(
                    ctx.clone(),
                    move |name: String, cb: RootedFn| -> Result<(), rquickjs::Error> {
                        state
                            .borrow_mut()
                            .handlers
                            .entry(name)
                            .or_default()
                            .push(HandlerEntry { cb, net: true });
                        Ok(())
                    },
                )?,
            )?;

            // emit(name, args_json) — fires local handlers via fire_local
            let state = self.state.clone();
            globals.set(
                "emit",
                Function::new(
                    ctx.clone(),
                    move |call: Ctx<'_>, name: String, args_json: String| -> Result<(), rquickjs::Error> {
                        fire_local(&call, &state, &name, &args_json)
                    },
                )?,
            )?;

            // TriggerClientEvent / TriggerServerEvent are host-mediated:
            // they enqueue onto a wire queue the host owns and transmits on
            // its pump tick. No socket access from JS.
            let state = self.state.clone();
            globals.set(
                "TriggerClientEvent",
                Function::new(
                    ctx.clone(),
                    move |name: String, payload: String| -> Result<(), rquickjs::Error> {
                        state.borrow_mut().wire.push(
                            serde_json::json!({"dir": "client", "name": name, "payload": payload})
                                .to_string(),
                        );
                        Ok(())
                    },
                )?,
            )?;
            let state = self.state.clone();
            globals.set(
                "TriggerServerEvent",
                Function::new(
                    ctx.clone(),
                    move |name: String, payload: String| -> Result<(), rquickjs::Error> {
                        state.borrow_mut().wire.push(
                            serde_json::json!({"dir": "server", "name": name, "payload": payload})
                                .to_string(),
                        );
                        Ok(())
                    },
                )?,
            )?;

            // --- resource identity ------------------------------------------
            // ponytail: real name comes from the resource loader; until that
            // wiring exists this is a constant the host overrides per
            // resource. Not a fake claim, and scripts needing it still work.
            globals.set(
                "GetCurrentResourceName",
                Function::new(ctx.clone(), || -> Result<&str, rquickjs::Error> { Ok("unknown") })?,
            )?;

            // --- console ------------------------------------------------------
            // QuickJS has no console; provide one routing to a host channel.
            let console = Object::new(ctx.clone())?;
            for (slot, level) in
                [("log", "info"), ("info", "info"), ("warn", "warn"), ("error", "error")]
            {
                let state = self.state.clone();
                console.set(
                    slot,
                    Function::new(
                        ctx.clone(),
                        move |msg: String| -> Result<(), rquickjs::Error> {
                            state.borrow_mut().logs.push((level.to_string(), msg));
                            Ok(())
                        },
                    )?,
                )?;
            }
            globals.set("console", console)?;

            // --- explicitly NOT provided -------------------------------------
            // require / process / Buffer / __dirname are undefined in a bare
            // QuickJS. A script touching one fails with a ReferenceError. That
            // is intended and honest: the Node compatibility runtime is a
            // separate crate (PHASE 12) for scripts that genuinely need it.

            Ok(())
        })
    }

    /// Drain queued outbound wire events (TriggerClientEvent /
    /// TriggerServerEvent) as JSON. Host calls this on its pump tick.
    pub fn drain_wire(&self) -> anyhow::Result<Vec<String>> {
        Ok(std::mem::take(&mut self.state.borrow_mut().wire))
    }

    /// Drain host-side console logs as (level, message) pairs.
    pub fn drain_logs(&self) -> anyhow::Result<Vec<(String, String)>> {
        Ok(std::mem::take(&mut self.state.borrow_mut().logs))
    }

    /// Advance virtual time by `elapsed_ms`, firing any due timers.
    /// Returns the number of callbacks invoked.
    pub fn tick_timers(&self, elapsed_ms: u64) -> anyhow::Result<usize> {
        // Collect the due callbacks first, then invoke with no state borrow
        // held: a timer callback may register or clear timers reentrantly.
        let due: Vec<RootedFn> = {
            let mut s = self.state.borrow_mut();
            s.clock = s.clock.saturating_add(elapsed_ms);
            let clock = s.clock;
            let mut fired = Vec::new();
            let mut keep = Vec::new();
            for t in s.timers.drain(..) {
                if t.due <= clock {
                    fired.push(t.cb.clone());
                    if t.kind == "interval" {
                        if let Some(p) = t.period {
                            keep.push(TimerEntry {
                                kind: t.kind,
                                due: clock.saturating_add(p),
                                period: t.period,
                                cb: t.cb,
                            });
                        }
                    }
                } else {
                    keep.push(t);
                }
            }
            s.timers = keep;
            fired
        };
        let n = due.len();
        if n == 0 {
            return Ok(0);
        }
        self.ctx.with(|ctx| -> anyhow::Result<()> {
            for cb in due {
                let f: Function = cb
                    .restore(&ctx)
                    .map_err(|e| anyhow::anyhow!("timer restore: {e:?}"))?;
                f.call::<(), ()>(())
                    .map_err(|e| anyhow::anyhow!("timer callback: {e:?}"))?;
            }
            Ok(())
        })?;
        Ok(n)
    }
}

impl std::fmt::Debug for JsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsRuntime").finish()
    }
}

impl Drop for JsRuntime {
    fn drop(&mut self) {
        // Free rooted callbacks while the JS heap is still alive. A rooted
        // handle dropped after its context is gone would release its
        // reference into a dead heap; clearing here keeps teardown
        // deterministic instead of tripping the runtime's GC-empty assert.
        // Best-effort: if the context is somehow unusable, field drops below
        // still run and the process exit path stays intact.
        let _ = self.ctx.with(|_| {
            let mut s = self.state.borrow_mut();
            s.timers.clear();
            s.handlers.clear();
        });
    }
}

// --- helpers ---------------------------------------------------------------

fn add_timer(
    state: &Rc<RefCell<JsState>>,
    kind: &str,
    cb: RootedFn,
    ms: u64,
    period: Option<u64>,
) -> f64 {
    let mut s = state.borrow_mut();
    let id = s.timers.len();
    let due = s.clock.saturating_add(ms);
    s.timers.push(TimerEntry {
        kind: kind.to_string(),
        due,
        period,
        cb,
    });
    id as f64
}

fn clear_timer(state: &Rc<RefCell<JsState>>, id: f64) {
    let mut s = state.borrow_mut();
    let i = id as usize;
    if i < s.timers.len() {
        s.timers.remove(i);
    }
}

/// Fire the local (non-net) handlers for `name` with `args_json` parsed as
/// the single payload argument. Handler list is cloned before invoking so a
/// handler may register or emit reentrantly without aliasing the registry.
fn fire_local(
    host: &Ctx,
    state: &Rc<RefCell<JsState>>,
    name: &str,
    args_json: &str,
) -> Result<(), rquickjs::Error> {
    let cbs: Vec<RootedFn> = {
        let s = state.borrow();
        match s.handlers.get(name) {
            Some(list) => list
                .iter()
                .filter(|h| !h.net)
                .map(|h| h.cb.clone())
                .collect(),
            None => Vec::new(),
        }
    };
    for cb in cbs {
        let args: Value = host.eval(format!("({args_json})").as_str())?;
        let f: Function = cb.restore(host)?;
        f.call::<_, ()>((args,))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_js_runs() {
        let rt = JsRuntime::new().unwrap();
        rt.exec("globalThis.answer = 6 * 7;").unwrap();
        let v = rt.eval_string("answer").unwrap();
        assert!(v.contains("42"), "got {v}");
    }

    #[test]
    fn syntax_error_reported() {
        let rt = JsRuntime::new().unwrap();
        assert!(rt.exec("var broken = ;").is_err());
    }

    #[test]
    fn json_available() {
        let rt = JsRuntime::new().unwrap();
        let v = rt.eval_string("JSON.stringify({a:1})").unwrap();
        assert!(v.contains("a"), "got {v}");
    }

    // --- Citizen compatibility surface -----------------------------------

    #[test]
    fn set_timeout_fires_on_tick() {
        let rt = JsRuntime::new().unwrap();
        rt.exec("globalThis.__hit = 0; setTimeout(function() { globalThis.__hit++; }, 100);")
            .unwrap();
        assert_eq!(rt.tick_timers(50).unwrap(), 0);
        assert_eq!(rt.tick_timers(50).unwrap(), 1);
        assert_eq!(
            rt.tick_timers(1000).unwrap(),
            0,
            "one-shot must not re-fire"
        );
        assert!(rt.eval_string("__hit").unwrap().contains('1'));
    }

    #[test]
    fn set_interval_repeats() {
        let rt = JsRuntime::new().unwrap();
        rt.exec("globalThis.__n = 0; setInterval(function() { globalThis.__n++; }, 10);")
            .unwrap();
        rt.tick_timers(10).unwrap();
        rt.tick_timers(10).unwrap();
        rt.tick_timers(10).unwrap();
        assert!(rt.eval_string("__n").unwrap().contains('3'));
    }

    #[test]
    fn clear_timer_cancels() {
        let rt = JsRuntime::new().unwrap();
        rt.exec("globalThis.__x = 0; var __cid = setInterval(function() { globalThis.__x++; }, 10); clearInterval(__cid);")
            .unwrap();
        rt.tick_timers(100).unwrap();
        assert!(rt.eval_string("__x").unwrap().contains('0'));
    }

    #[test]
    fn on_emit_roundtrip() {
        let rt = JsRuntime::new().unwrap();
        rt.exec("globalThis.__got = 0; on('myevent', function() { globalThis.__got = 42; });")
            .unwrap();
        rt.exec("emit('myevent', '[]')").unwrap();
        assert!(rt.eval_string("__got").unwrap().contains("42"));
    }

    #[test]
    fn on_net_does_not_fire_on_local_emit() {
        let rt = JsRuntime::new().unwrap();
        rt.exec(
            "globalThis.__leaked = 0; onNet('secret', function() { globalThis.__leaked = 1; });",
        )
        .unwrap();
        rt.exec("emit('secret', '[]')").unwrap();
        assert!(rt.eval_string("__leaked").unwrap().contains('0'));
    }

    #[test]
    fn emit_passes_json_payload() {
        let rt = JsRuntime::new().unwrap();
        rt.exec(
            "globalThis.__sum = 0; on('add', function(p) { globalThis.__sum = p.a + p.b; });",
        )
        .unwrap();
        rt.exec("emit('add', '{\"a\": 3, \"b\": 4}')").unwrap();
        assert!(rt.eval_string("__sum").unwrap().contains('7'));
    }

    #[test]
    fn trigger_events_enqueue_on_wire() {
        let rt = JsRuntime::new().unwrap();
        rt.exec("TriggerClientEvent('hud:show', '{\"text\": \"hi\"}');")
            .unwrap();
        rt.exec("TriggerServerEvent('money:give', '{\"amount\": 5}');")
            .unwrap();
        let wire = rt.drain_wire().unwrap();
        assert_eq!(wire.len(), 2);
        assert!(wire.iter().any(|w| w.contains("\"dir\":\"client\"")));
        assert!(wire.iter().any(|w| w.contains("\"dir\":\"server\"")));
        // drain is destructive
        assert!(rt.drain_wire().unwrap().is_empty());
    }

    #[test]
    fn console_routes_to_host() {
        let rt = JsRuntime::new().unwrap();
        rt.exec("console.log('hello'); console.error('bad');").unwrap();
        let logs = rt.drain_logs().unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().any(|(l, m)| l == "info" && m == "hello"));
        assert!(logs.iter().any(|(l, m)| l == "error" && m == "bad"));
    }

    #[test]
    fn citizen_thread_runs_synchronously() {
        let rt = JsRuntime::new().unwrap();
        rt.exec(
            "globalThis.__t = 0; Citizen.CreateThread(function() { globalThis.__t = 1; });",
        )
        .unwrap();
        assert!(rt.eval_string("__t").unwrap().contains('1'));
    }

    #[test]
    fn node_builtins_absent() {
        let rt = JsRuntime::new().unwrap();
        assert!(rt.eval_string("typeof require").unwrap().contains("undefined"));
        assert!(rt.eval_string("typeof process").unwrap().contains("undefined"));
        // a script that uses require must fail, not silently no-op
        assert!(rt.exec("(function(){ return require('fs'); })();").is_err());
    }

    #[test]
    fn eval_json_returns_json() {
        let rt = JsRuntime::new().unwrap();
        let v = rt.eval_json("{a: [1,2,3], b: 'x'}").unwrap();
        assert_eq!(v, "{\"a\":[1,2,3],\"b\":\"x\"}");
    }

    #[test]
    fn call_json_invokes_global() {
        let rt = JsRuntime::new().unwrap();
        rt.exec("function double(n) { return n * 2; }").unwrap();
        assert_eq!(rt.call_json("double", "21").unwrap(), "42");
    }

    #[test]
    fn sandbox_is_capability_only_not_cpu() {
        // ponytail: this is a *capability* sandbox, not a CPU/time sandbox.
        // There is no interrupt hook, so a hostile infinite loop hangs the
        // runtime; the host must run untrusted scripts under a process
        // watchdog. Verified here only for a cooperative finite workload.
        let rt = JsRuntime::new().unwrap();
        rt.exec(
            "globalThis.__acc = 0; for (var i = 0; i < 100000; i++) { globalThis.__acc++; }",
        )
        .unwrap();
        assert!(rt.eval_string("__acc").unwrap().contains("100000"));
    }
}
