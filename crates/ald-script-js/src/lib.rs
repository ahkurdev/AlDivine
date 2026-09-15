//! Aldivine JavaScript runtime — QuickJS via rquickjs (bindgen build).
//!
//! Sandboxing: no filesystem, no process, no native module loading. The
//! runtime exposes the Aldivine API only. Every capability is granted
//! explicitly by the host.

use rquickjs::{CatchResultExt, Context, Runtime};

/// A sandboxed QuickJS execution context for one resource.
pub struct JsRuntime {
    // Runtime must outlive Context; kept to extend lifetime.
    _runtime: Runtime,
    ctx: Context,
}

impl JsRuntime {
    /// Create a new sandboxed runtime.
    pub fn new() -> anyhow::Result<Self> {
        let runtime = Runtime::new()?;
        let ctx = Context::full(&runtime)?;
        Ok(JsRuntime { _runtime: runtime, ctx })
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
            ctx.eval::<rquickjs::Value, _>(expr)
                .catch(&ctx)
                .map(|v| format!("{v:?}"))
                .map_err(|e| anyhow::anyhow!("js error: {e:?}"))
        })
    }
}

impl std::fmt::Debug for JsRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsRuntime").finish()
    }
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
}
