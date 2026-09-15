//! Aldivine Lua runtime — Lua 5.4 via mlua (vendored build).
//!
//! Sandboxing: the runtime is created WITHOUT the standard `io`, `os`,
//! `package`, `debug` and `jit` libraries. Resources get the Aldivine API
//! surface only; every capability must be granted explicitly by the host.

use mlua::{Lua, Result as LuaResult, Value};

/// A sandboxed Lua execution context for one resource.
pub struct LuaRuntime {
    lua: Lua,
}

impl LuaRuntime {
    /// Create a new sandboxed runtime. Unsafe stdlib is removed.
    pub fn new() -> LuaResult<Self> {
        let lua = Lua::new();
        // Remove libraries that allow filesystem/process/native access.
        let globals = lua.globals();
        for name in ["io", "os", "package", "debug", "jit", "ffi"] {
            globals.set(name, Value::Nil)?;
        }
        // Lock down require: no module loading.
        globals.set("require", Value::Nil)?;
        globals.set("loadfile", Value::Nil)?;
        globals.set("dofile", Value::Nil)?;
        globals.set("loadlib", Value::Nil)?;
        Ok(LuaRuntime { lua })
    }

    /// Register the Aldivine API version marker.
    pub fn register_api_version(&self, version: &str) -> LuaResult<()> {
        let globals = self.lua.globals();
        globals.set("ALDIVINE_API_VERSION", version)?;
        Ok(())
    }

    /// Execute a chunk of source. Returns the Lua error on failure.
    pub fn exec(&self, source: &str) -> LuaResult<()> {
        self.lua.load(source).exec()
    }

    /// Evaluate an expression and return it as a string.
    pub fn eval_string(&self, expr: &str) -> LuaResult<String> {
        let v: Value = self.lua.load(expr).eval()?;
        Ok(format!("{v:?}"))
    }

    /// Access the underlying Lua (host-registered APIs go through here).
    pub fn lua(&self) -> &Lua {
        &self.lua
    }
}

impl std::fmt::Debug for LuaRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LuaRuntime").finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sandbox_blocks_io() {
        let rt = LuaRuntime::new().unwrap();
        // `io` must be nil so scripts cannot touch the filesystem.
        let res = rt.exec("local x = io.open('/etc/passwd')");
        assert!(res.is_err(), "io must be unavailable");
    }

    #[test]
    fn os_blocked() {
        let rt = LuaRuntime::new().unwrap();
        assert!(rt.exec("os.execute('echo hi')").is_err());
    }

    #[test]
    fn require_blocked() {
        let rt = LuaRuntime::new().unwrap();
        assert!(rt.exec("require('ffi')").is_err());
    }

    #[test]
    fn pure_lua_runs() {
        let rt = LuaRuntime::new().unwrap();
        rt.exec("answer = 6 * 7").unwrap();
        let v = rt.eval_string("answer").unwrap();
        assert!(v.contains("42"), "got {v}");
    }

    #[test]
    fn api_version_registered() {
        let rt = LuaRuntime::new().unwrap();
        rt.register_api_version("v1").unwrap();
        let v = rt.eval_string("ALDIVINE_API_VERSION").unwrap();
        assert!(v.contains("v1"), "got {v}");
    }
}
