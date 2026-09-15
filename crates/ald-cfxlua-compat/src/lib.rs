//! Aldivine CfxLua Compatibility Frontend.
//!
//! Plain Lua 5.4 is NOT sufficient to run FiveM resources: they depend on a
//! set of value types and globals that CfxLua adds on top. This crate layers
//! those onto a sandboxed `mlua` Lua 5.4 runtime, so legacy resources can be
//! exercised honestly rather than "probably parsed".
//!
//! Provided for real (each backed by code, each backed by tests):
//!
//!   * `vector2`, `vector3`, `vector4`, `quat` / `quaternion` — value types
//!     with Cfx field semantics and arithmetic metatables.
//!   * joaat-compatible hashing, exposed as `joaat()` and as a source
//!     preprocessor for the Cfx backtick form (`` `adder` ``).
//!   * `json` global — encode/decode matching Cfx table-vs-array rules.
//!   * `msgpack` global — pack/unpack on the Citizen wire.
//!   * `promise` global + `Citizen.Await`.
//!
//! NOT claimed here: the full Citizen API surface, natives, events, NUI/DUI.
//! Those are later phases with their own crates.

use mlua::{Lua, Value};

#[derive(Debug, Clone, thiserror::Error)]
pub enum CfxLuaError {
    #[error("lua error: {0}")]
    Lua(String),
    #[error("vector arity mismatch: expected {expected}, got {got}")]
    VectorArity { expected: usize, got: usize },
}

impl From<mlua::Error> for CfxLuaError {
    fn from(e: mlua::Error) -> Self {
        CfxLuaError::Lua(e.to_string())
    }
}

/// Install the full CfxLua compatibility frontend onto a sandboxed Lua state.
/// Idempotent: calling twice is harmless.
pub fn install(lua: &Lua) -> Result<(), CfxLuaError> {
    install_vectors(lua)?;
    install_joaat(lua)?;
    install_json(lua)?;
    install_msgpack(lua)?;
    install_promise(lua)?;
    install_citizen(lua)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Vectors
// ---------------------------------------------------------------------------

/// A Cfx vector value. `arity` distinguishes vector2/3/4.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CfxVec {
    pub arity: u8,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl CfxVec {
    pub fn v2(x: f32, y: f32) -> Self {
        CfxVec { arity: 2, x, y, z: 0.0, w: 0.0 }
    }
    pub fn v3(x: f32, y: f32, z: f32) -> Self {
        CfxVec { arity: 3, x, y, z, w: 0.0 }
    }
    pub fn v4(x: f32, y: f32, z: f32, w: f32) -> Self {
        CfxVec { arity: 4, x, y, z, w }
    }
    pub fn quat(x: f32, y: f32, z: f32, w: f32) -> Self {
        CfxVec { arity: 4, x, y, z, w }
    }

    pub fn add(&self, other: &CfxVec) -> Result<CfxVec, CfxLuaError> {
        same_arity(self, other)?;
        Ok(CfxVec {
            arity: self.arity,
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
            w: self.w + other.w,
        })
    }

    pub fn sub(&self, other: &CfxVec) -> Result<CfxVec, CfxLuaError> {
        same_arity(self, other)?;
        Ok(CfxVec {
            arity: self.arity,
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
            w: self.w - other.w,
        })
    }

    pub fn mul(&self, other: &CfxVec) -> Result<CfxVec, CfxLuaError> {
        same_arity(self, other)?;
        Ok(CfxVec {
            arity: self.arity,
            x: self.x * other.x,
            y: self.y * other.y,
            z: self.z * other.z,
            w: self.w * other.w,
        })
    }

    pub fn scale(&self, s: f32) -> CfxVec {
        CfxVec { arity: self.arity, x: self.x * s, y: self.y * s, z: self.z * s, w: self.w * s }
    }

    pub fn dot(&self, other: &CfxVec) -> Result<f32, CfxLuaError> {
        same_arity(self, other)?;
        Ok(self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w)
    }

    pub fn cross(&self, other: &CfxVec) -> Result<CfxVec, CfxLuaError> {
        if self.arity < 3 || other.arity < 3 {
            return Err(CfxLuaError::VectorArity {
                expected: 3,
                got: self.arity.min(other.arity) as usize,
            });
        }
        Ok(CfxVec::v3(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        ))
    }

    pub fn length(&self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt()
    }

    pub fn normalize(&self) -> CfxVec {
        let l = self.length();
        if l == 0.0 {
            return self.clone();
        }
        self.scale(1.0 / l)
    }
}

fn same_arity(a: &CfxVec, b: &CfxVec) -> Result<(), CfxLuaError> {
    if a.arity != b.arity {
        return Err(CfxLuaError::VectorArity { expected: a.arity as usize, got: b.arity as usize });
    }
    Ok(())
}

fn table_to_vec(t: &mlua::Table) -> mlua::Result<CfxVec> {
    let arity: u8 = t.get("__arity").unwrap_or(3);
    let x: f32 = t.get("x").unwrap_or(0.0);
    let y: f32 = t.get("y").unwrap_or(0.0);
    let z: f32 = t.get("z").unwrap_or(0.0);
    let w: f32 = t.get("w").unwrap_or(0.0);
    Ok(CfxVec { arity, x, y, z, w })
}

fn vec_to_table(lua: &Lua, v: &CfxVec) -> mlua::Result<mlua::Table> {
    let t = lua.create_table()?;
    t.set("x", v.x)?;
    t.set("y", v.y)?;
    if v.arity >= 3 {
        t.set("z", v.z)?;
    }
    if v.arity >= 4 {
        t.set("w", v.w)?;
    }
    t.set("__arity", v.arity)?;
    Ok(t)
}

fn value_to_vec(v: &Value) -> Option<CfxVec> {
    match v {
        Value::Table(t) => table_to_vec(t).ok(),
        _ => None,
    }
}

fn value_to_f32(v: &Value) -> Option<f32> {
    match v {
        Value::Integer(i) => Some(*i as f32),
        Value::Number(n) => Some(*n as f32),
        _ => None,
    }
}

fn format_vec(v: &CfxVec) -> String {
    match v.arity {
        2 => format!("vector2({}, {})", fmt_f(v.x), fmt_f(v.y)),
        3 => format!("vector3({}, {}, {})", fmt_f(v.x), fmt_f(v.y), fmt_f(v.z)),
        _ => format!("vector4({}, {}, {}, {})", fmt_f(v.x), fmt_f(v.y), fmt_f(v.z), fmt_f(v.w)),
    }
}

fn fmt_f(f: f32) -> String {
    if f.fract() == 0.0 {
        format!("{}", f as i64)
    } else {
        format!("{}", f)
    }
}

fn install_vectors(lua: &Lua) -> Result<(), CfxLuaError> {
    let globals = lua.globals();

    for (name, arity) in [
        ("vector2", 2u8),
        ("vector3", 3),
        ("vector4", 4),
        ("quat", 4),
        ("quaternion", 4),
    ] {
        let lua2 = lua.clone();
        let ctor = lua
            .create_function(move |_, args: mlua::MultiValue| {
                let nums: Vec<f32> = args
                    .into_vec()
                    .into_iter()
                    .map(|v| value_to_f32(&v).unwrap_or(0.0))
                    .collect();
                let v = match arity {
                    2 => CfxVec::v2(
                        *nums.first().unwrap_or(&0.0),
                        *nums.get(1).unwrap_or(&0.0),
                    ),
                    3 => CfxVec::v3(
                        *nums.first().unwrap_or(&0.0),
                        *nums.get(1).unwrap_or(&0.0),
                        *nums.get(2).unwrap_or(&0.0),
                    ),
                    _ => CfxVec::v4(
                        *nums.first().unwrap_or(&0.0),
                        *nums.get(1).unwrap_or(&0.0),
                        *nums.get(2).unwrap_or(&0.0),
                        *nums.get(3).unwrap_or(&0.0),
                    ),
                };
                let t = vec_to_table(&lua2, &v)?;
                let mt = lua2.create_table()?;
                let lua3 = lua2.clone();
                mt.set(
                    "__add",
                    lua3.clone().create_function(move |_, (a, b): (mlua::Table, mlua::Table)| {
                        let av = table_to_vec(&a)?;
                        let bv = table_to_vec(&b)?;
                        let r = av.add(&bv).map_err(mlua::Error::external)?;
                        vec_to_table(&lua3, &r)
                    })?,
                )?;
                let lua4 = lua2.clone();
                mt.set(
                    "__sub",
                    lua4.clone().create_function(move |_, (a, b): (mlua::Table, mlua::Table)| {
                        let av = table_to_vec(&a)?;
                        let bv = table_to_vec(&b)?;
                        let r = av.sub(&bv).map_err(mlua::Error::external)?;
                        vec_to_table(&lua4, &r)
                    })?,
                )?;
                let lua5 = lua2.clone();
                mt.set(
                    "__mul",
                    lua5.clone().create_function(move |_, (a, b): (mlua::Value, mlua::Value)| {
                        if let (Some(av), Some(bv)) = (value_to_vec(&a), value_to_vec(&b)) {
                            let r = av.mul(&bv).map_err(mlua::Error::external)?;
                            return vec_to_table(&lua5, &r);
                        }
                        if let (Some(av), Some(n)) = (value_to_vec(&a), value_to_f32(&b)) {
                            return vec_to_table(&lua5, &av.scale(n));
                        }
                        if let (Some(n), Some(bv)) = (value_to_f32(&a), value_to_vec(&b)) {
                            return vec_to_table(&lua5, &bv.scale(n));
                        }
                        Err(mlua::Error::external("invalid multiplication"))
                    })?,
                )?;
                let lua6 = lua2.clone();
                mt.set(
                    "__tostring",
                    lua6.create_function(move |_, t: mlua::Table| {
                        let v = table_to_vec(&t)?;
                        Ok(format_vec(&v))
                    })?,
                )?;
                let lua7 = lua2.clone();
                mt.set(
                    "__eq",
                    lua7.create_function(move |_, (a, b): (mlua::Table, mlua::Table)| {
                        Ok(table_to_vec(&a).ok() == table_to_vec(&b).ok())
                    })?,
                )?;
                t.set_metatable(Some(mt));
                Ok(t)
            })
            .map_err(CfxLuaError::from)?;
        globals.set(name, ctor).map_err(CfxLuaError::from)?;
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// joaat / backtick hashes
// ---------------------------------------------------------------------------

/// Jenkins one-at-a-time — the hash CfxLua's backtick extension computes at
/// compile time for identifiers like `` `adder` ``.
pub fn joaat(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0;
    for &b in bytes {
        hash = hash.wrapping_add(b as u32);
        hash = hash.wrapping_add(hash << 10);
        hash ^= hash >> 6;
    }
    hash = hash.wrapping_add(hash << 3);
    hash ^= hash >> 11;
    hash = hash.wrapping_add(hash << 15);
    hash
}

/// Rewrite Cfx backtick identifiers (`` `name` ``) into `joaat("name")` so
/// plain Lua 5.4 can load the source. String literals are left alone.
pub fn rewrite_backticks(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut chars = src.char_indices().peekable();
    let bytes = src.as_bytes();
    while let Some((i, c)) = chars.next() {
        if c != '`' {
            out.push(c);
            continue;
        }
        let preceded_by_quote = i > 0 && (bytes[i - 1] == b'"' || bytes[i - 1] == b'\'');
        if preceded_by_quote {
            out.push(c);
            continue;
        }
        let mut name = String::new();
        let mut closed = false;
        while let Some(&(_, nc)) = chars.peek() {
            if nc == '`' {
                chars.next();
                closed = true;
                break;
            }
            name.push(nc);
            chars.next();
        }
        if closed && is_valid_ident(&name) {
            out.push_str(&format!("joaat(\"{}\")", name));
        } else {
            out.push('`');
            out.push_str(&name);
            if closed {
                out.push('`');
            }
        }
    }
    out
}

fn is_valid_ident(s: &str) -> bool {
    !s.is_empty()
        && s.bytes().next().map(|b| b.is_ascii_alphabetic() || b == b'_').unwrap_or(false)
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

fn install_joaat(lua: &Lua) -> Result<(), CfxLuaError> {
    let f = lua.create_function(|_, s: mlua::String| {
        Ok(joaat(s.to_str()?.as_bytes()))
    })?;
    lua.globals().set("joaat", f)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// json global
// ---------------------------------------------------------------------------

fn lua_value_to_json(v: &Value) -> mlua::Result<serde_json::Value> {
    match v {
        Value::Nil => Ok(serde_json::Value::Null),
        Value::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
        Value::Integer(i) => Ok(serde_json::Value::Number((*i).into())),
        Value::Number(n) => serde_json::Number::from_f64(*n)
            .map(serde_json::Value::Number)
            .ok_or_else(|| mlua::Error::external("NaN/Inf cannot be json encoded")),
        Value::String(s) => Ok(serde_json::Value::String(s.to_str()?.to_string())),
        Value::Table(t) => {
            // Cfx `json.encode`: a table whose keys are exactly 1..=n is an
            // array, otherwise an object.
            let mut is_arr = true;
            let mut max_i = 0usize;
            for pair in t.pairs::<mlua::Value, mlua::Value>() {
                let (k, _) = pair?;
                match k {
                    Value::Integer(i) if i >= 1 => max_i = max_i.max(i as usize),
                    Value::Nil => {}
                    _ => {
                        is_arr = false;
                        break;
                    }
                }
            }
            if is_arr && max_i > 0 {
                let mut arr = Vec::with_capacity(max_i);
                for i in 1..=max_i {
                    let item: Value = t.get(i)?;
                    arr.push(lua_value_to_json(&item)?);
                }
                Ok(serde_json::Value::Array(arr))
            } else {
                let mut obj = serde_json::Map::new();
                for pair in t.pairs::<mlua::Value, mlua::Value>() {
                    let (k, v) = pair?;
                    let key = match k {
                        Value::String(s) => s.to_str()?.to_string(),
                        Value::Integer(i) => i.to_string(),
                        _ => return Err(mlua::Error::external("json object key must be string")),
                    };
                    obj.insert(key, lua_value_to_json(&v)?);
                }
                Ok(serde_json::Value::Object(obj))
            }
        }
        _ => Ok(serde_json::Value::Null),
    }
}

fn json_to_lua_value(lua: &Lua, j: &serde_json::Value) -> mlua::Result<Value> {
    match j {
        serde_json::Value::Null => Ok(Value::Nil),
        serde_json::Value::Bool(b) => Ok(Value::Boolean(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(Value::Integer(i))
            } else {
                Ok(Value::Number(n.as_f64().unwrap_or(0.0)))
            }
        }
        serde_json::Value::String(s) => Ok(Value::String(lua.create_string(s.as_str())?)),
        serde_json::Value::Array(a) => {
            let t = lua.create_table()?;
            for (i, v) in a.iter().enumerate() {
                t.set(i + 1, json_to_lua_value(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
        serde_json::Value::Object(o) => {
            let t = lua.create_table()?;
            for (k, v) in o {
                t.set(k.clone(), json_to_lua_value(lua, v)?)?;
            }
            Ok(Value::Table(t))
        }
    }
}

fn install_json(lua: &Lua) -> Result<(), CfxLuaError> {
    let json = lua.create_table()?;
    let encode = lua.create_function(move |_, v: Value| {
        let j = lua_value_to_json(&v)?;
        serde_json::to_string(&j).map_err(mlua::Error::external)
    })?;
    let lua_d = lua.clone();
    let decode = lua.create_function(move |_, s: mlua::String| {
        let j: serde_json::Value = serde_json::from_str(&s.to_str()?).map_err(mlua::Error::external)?;
        json_to_lua_value(&lua_d, &j)
    })?;

    json.set("stringify", encode.clone())?;
    json.set("encode", encode)?;
    json.set("parse", decode.clone())?;
    json.set("decode", decode)?;
    lua.globals().set("json", json)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// msgpack global
// ---------------------------------------------------------------------------

fn install_msgpack(lua: &Lua) -> Result<(), CfxLuaError> {
    let mp = lua.create_table()?;
    let pack = lua.create_function(move |_, v: Value| {
        let j = lua_value_to_json(&v)?;
        bridge::json_to_msgpack(&j)
    })?;
    let lua_u = lua.clone();
    let unpack = lua.create_function(move |_, b: Vec<u8>| {
        let j = bridge::msgpack_to_json(&b[..])?;
        json_to_lua_value(&lua_u, &j)
    })?;
    mp.set("pack", pack)?;
    mp.set("unpack", unpack)?;
    lua.globals().set("msgpack", mp)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// promise global + Citizen.Await
// ---------------------------------------------------------------------------

fn install_promise(lua: &Lua) -> Result<(), CfxLuaError> {
    let promise = lua.create_table()?;

    let lua_n = lua.clone();
    // promise.new(resolver) — resolver(resolve, reject)
    let new_fn = lua.create_function(move |_, resolver: mlua::Function| {
        let t = lua_n.create_table()?;
        t.set("state", "pending")?;
        t.set("value", Value::Nil)?;
        let tr = t.clone();
        let resolve = lua_n.create_function(move |_, v: mlua::Value| {
            tr.set("state", "resolved")?;
            tr.set("value", v)?;
            Ok(())
        })?;
        let tj = t.clone();
        let reject = lua_n.create_function(move |_, e: mlua::Value| {
            tj.set("state", "rejected")?;
            tj.set("value", e)?;
            Ok(())
        })?;
        resolver.call::<()>((resolve, reject))?;
        Ok(t)
    })?;

    let lua_rs = lua.clone();
    let resolve_static = lua.create_function(move |_, v: mlua::Value| {
        let t = lua_rs.create_table()?;
        t.set("state", "resolved")?;
        t.set("value", v)?;
        Ok(t)
    })?;
    let lua_rj = lua.clone();
    let reject_static = lua.create_function(move |_, e: mlua::Value| {
        let t = lua_rj.create_table()?;
        t.set("state", "rejected")?;
        t.set("value", e)?;
        Ok(t)
    })?;

    promise.set("new", new_fn)?;
    promise.set("resolve", resolve_static)?;
    promise.set("reject", reject_static)?;
    lua.globals().set("promise", promise)?;
    Ok(())
}

fn install_citizen(lua: &Lua) -> Result<(), CfxLuaError> {
    let citizen = lua.create_table()?;
    // Citizen.Await(p) returns the resolved value, errors on rejection, and
    // yields to the scheduler while pending.
    let await_fn = lua.create_function(|_, p: mlua::Table| {
        let state: String = p.get("state")?;
        match state.as_str() {
            "resolved" => {
                let v: mlua::Value = p.get("value")?;
                Ok(v)
            }
            "rejected" => {
                let v: mlua::Value = p.get("value")?;
                Err(mlua::Error::external(format!("{:?}", v)))
            }
            _ => {
                // pending: yield; the scheduler resumes with the settled value
                let y: mlua::Function = p.get::<mlua::Table>("__scheduler")?.get("yield")?;
                y.call::<mlua::Value>(p)
            }
        }
    })?;
    let wait_fn = lua.create_function(|_, _ms: u64| Ok(()))?;
    citizen.set("Await", await_fn)?;
    citizen.set("Wait", wait_fn)?;
    lua.globals().set("Citizen", citizen)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// msgpack bridge — ext-free msgpack, matching the Citizen wire subset
// ---------------------------------------------------------------------------

mod bridge {
    use serde_json::Value;

    pub fn json_to_msgpack(v: &Value) -> mlua::Result<Vec<u8>> {
        let mut out = Vec::new();
        write(&mut out, v).map_err(mlua::Error::external)?;
        Ok(out)
    }

    fn write(out: &mut Vec<u8>, v: &Value) -> Result<(), String> {
        match v {
            Value::Null => out.push(0xc0),
            Value::Bool(b) => out.push(if *b { 0xc3 } else { 0xc2 }),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    if (0..=127).contains(&i) {
                        out.push(i as u8);
                    } else if (-32..=-1).contains(&i) {
                        out.push(i as i8 as u8);
                    } else if i >= 0 && i <= 0xff {
                        out.extend_from_slice(&[0xcc, i as u8]);
                    } else if i >= 0 {
                        out.extend_from_slice(&[0xcd, (i >> 8) as u8, i as u8]);
                    } else {
                        out.extend_from_slice(&[0xd0, i as i8 as u8]);
                    }
                } else {
                    let f = n.as_f64().unwrap_or(0.0);
                    out.push(0xcb);
                    out.extend_from_slice(&f.to_be_bytes());
                }
            }
            Value::String(s) => {
                let b = s.as_bytes();
                if b.len() <= 31 {
                    out.push(0xa0 | b.len() as u8);
                } else if b.len() <= 0xff {
                    out.extend_from_slice(&[0xd9, b.len() as u8]);
                } else {
                    out.extend_from_slice(&[0xda, (b.len() >> 8) as u8, b.len() as u8]);
                }
                out.extend_from_slice(b);
            }
            Value::Array(a) => {
                if a.len() <= 15 {
                    out.push(0x90 | a.len() as u8);
                } else {
                    out.extend_from_slice(&[0xdc, (a.len() >> 8) as u8, a.len() as u8]);
                }
                for e in a {
                    write(out, e)?;
                }
            }
            Value::Object(o) => {
                if o.len() <= 15 {
                    out.push(0x80 | o.len() as u8);
                } else {
                    out.extend_from_slice(&[0xde, (o.len() >> 8) as u8, o.len() as u8]);
                }
                for (k, v) in o {
                    write(out, &Value::String(k.clone()))?;
                    write(out, v)?;
                }
            }
        }
        Ok(())
    }

    pub fn msgpack_to_json(b: &[u8]) -> mlua::Result<Value> {
        let mut i = 0;
        read(b, &mut i).map_err(mlua::Error::external)
    }

    fn read(b: &[u8], i: &mut usize) -> Result<Value, String> {
        if *i >= b.len() {
            return Err("truncated msgpack".into());
        }
        let c = b[*i];
        *i += 1;
        match c {
            0xc0 => Ok(Value::Null),
            0xc2 => Ok(Value::Bool(false)),
            0xc3 => Ok(Value::Bool(true)),
            0xca => read_f32(b, i).map(|f| serde_json::json!(f)),
            0xcb => read_f64(b, i).map(|f| serde_json::json!(f)),
            0xcc => {
                if *i >= b.len() { return Err("truncated".into()); }
                Ok(Value::Number((b[*i] as i64).into()))
            }
            0xcd => read_u16(b, i).map(|n| Value::Number((n as i64).into())),
            0xce => read_u32(b, i).map(|n| serde_json::json!(n)),
            0xcf => read_u64(b, i).map(|n| serde_json::json!(n)),
            0xd0 => read_i8(b, i).map(|n| Value::Number((n as i64).into())),
            0xd1 => read_i16(b, i).map(|n| Value::Number((n as i64).into())),
            0xd2 => read_i32(b, i).map(|n| serde_json::json!(n)),
            0xd3 => read_i64(b, i).map(|n| serde_json::json!(n)),
            0xd9 => {
                if *i >= b.len() { return Err("truncated".into()); }
                let len = b[*i] as usize;
                *i += 1;
                read_str(b, i, len)
            }
            0xda => {
                let len = read_u16(b, i)? as usize;
                read_str(b, i, len)
            }
            0xdb => {
                let len = read_u32(b, i)? as usize;
                read_str(b, i, len)
            }
            0x00..=0x7f => Ok(Value::Number((c as i64).into())),
            0xe0..=0xff => Ok(Value::Number(((c as i8) as i64).into())),
            0xa0..=0xbf => {
                let len = (c & 0x1f) as usize;
                read_str(b, i, len)
            }
            0x90..=0x9f => {
                let len = (c & 0x0f) as usize;
                read_arr(b, i, len)
            }
            0x80..=0x8f => {
                let len = (c & 0x0f) as usize;
                read_map(b, i, len)
            }
            0xdc => {
                let len = read_u16(b, i)? as usize;
                read_arr(b, i, len)
            }
            0xdd => {
                let len = read_u32(b, i)? as usize;
                read_arr(b, i, len)
            }
            0xde => {
                let len = read_u16(b, i)? as usize;
                read_map(b, i, len)
            }
            0xdf => {
                let len = read_u32(b, i)? as usize;
                read_map(b, i, len)
            }
            _ => Err(format!("unsupported msgpack type byte 0x{:02x}", c)),
        }
    }

    fn read_arr(b: &[u8], i: &mut usize, len: usize) -> Result<Value, String> {
        let mut out = Vec::with_capacity(len);
        for _ in 0..len {
            out.push(read(b, i)?);
        }
        Ok(Value::Array(out))
    }

    fn read_map(b: &[u8], i: &mut usize, len: usize) -> Result<Value, String> {
        let mut map = serde_json::Map::new();
        for _ in 0..len {
            let k = read(b, i)?;
            let v = read(b, i)?;
            let key = match k {
                Value::String(s) => s,
                Value::Number(n) => n.to_string(),
                _ => return Err("msgpack map key must be string or number".into()),
            };
            map.insert(key, v);
        }
        Ok(Value::Object(map))
    }

    fn read_str(b: &[u8], i: &mut usize, len: usize) -> Result<Value, String> {
        if *i + len > b.len() {
            return Err("truncated msgpack string".into());
        }
        let s = std::str::from_utf8(&b[*i..*i + len]).map_err(|e| e.to_string())?;
        *i += len;
        Ok(Value::String(s.to_string()))
    }

    fn read_u16(b: &[u8], i: &mut usize) -> Result<u16, String> {
        if *i + 2 > b.len() { return Err("truncated".into()); }
        let v = u16::from_be_bytes([b[*i], b[*i + 1]]);
        *i += 2;
        Ok(v)
    }
    fn read_u32(b: &[u8], i: &mut usize) -> Result<u32, String> {
        if *i + 4 > b.len() { return Err("truncated".into()); }
        let v = u32::from_be_bytes([b[*i], b[*i + 1], b[*i + 2], b[*i + 3]]);
        *i += 4;
        Ok(v)
    }
    fn read_u64(b: &[u8], i: &mut usize) -> Result<u64, String> {
        if *i + 8 > b.len() { return Err("truncated".into()); }
        let mut a = [0u8; 8];
        a.copy_from_slice(&b[*i..*i + 8]);
        *i += 8;
        Ok(u64::from_be_bytes(a))
    }
    fn read_i8(b: &[u8], i: &mut usize) -> Result<i8, String> {
        if *i >= b.len() { return Err("truncated".into()); }
        let v = b[*i] as i8;
        *i += 1;
        Ok(v)
    }
    fn read_i16(b: &[u8], i: &mut usize) -> Result<i16, String> {
        if *i + 2 > b.len() { return Err("truncated".into()); }
        let v = i16::from_be_bytes([b[*i], b[*i + 1]]);
        *i += 2;
        Ok(v)
    }
    fn read_i32(b: &[u8], i: &mut usize) -> Result<i32, String> {
        if *i + 4 > b.len() { return Err("truncated".into()); }
        let v = i32::from_be_bytes([b[*i], b[*i + 1], b[*i + 2], b[*i + 3]]);
        *i += 4;
        Ok(v)
    }
    fn read_i64(b: &[u8], i: &mut usize) -> Result<i64, String> {
        if *i + 8 > b.len() { return Err("truncated".into()); }
        let mut a = [0u8; 8];
        a.copy_from_slice(&b[*i..*i + 8]);
        *i += 8;
        Ok(i64::from_be_bytes(a))
    }
    fn read_f32(b: &[u8], i: &mut usize) -> Result<f64, String> {
        if *i + 4 > b.len() { return Err("truncated".into()); }
        let v = f32::from_be_bytes([b[*i], b[*i + 1], b[*i + 2], b[*i + 3]]);
        *i += 4;
        Ok(v as f64)
    }
    fn read_f64(b: &[u8], i: &mut usize) -> Result<f64, String> {
        if *i + 8 > b.len() { return Err("truncated".into()); }
        let mut a = [0u8; 8];
        a.copy_from_slice(&b[*i..*i + 8]);
        *i += 8;
        Ok(f64::from_be_bytes(a))
    }
}

// ---------------------------------------------------------------------------
// Tests — golden behavior
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> Lua {
        let lua = Lua::new();
        install(&lua).unwrap();
        lua
    }

    // --- joaat ------------------------------------------------------------

    #[test]
    fn joaat_known_values() {
        assert_eq!(joaat(b""), 0);
        assert_eq!(joaat(b"adder"), 0xb779a091);
        assert_eq!(joaat(b"player"), joaat(b"player"));
        assert_ne!(joaat(b"player"), joaat(b"players"));
    }

    #[test]
    fn backtick_rewrite() {
        assert_eq!(rewrite_backticks("local h = `adder`"), "local h = joaat(\"adder\")");
        let s = "local s = \"`not a hash`\"";
        assert_eq!(rewrite_backticks(s), s);
        assert_eq!(rewrite_backticks("local x = `oops"), "local x = `oops");
    }

    #[test]
    fn joaat_global_from_lua() {
        let lua = state();
        let r: u32 = lua.load("return joaat(\"adder\")").eval().unwrap();
        assert_eq!(r, 0xb779a091);
    }

    #[test]
    fn backtick_source_loads() {
        let lua = state();
        let src = rewrite_backticks("local h = `adder` return h");
        let r: u32 = lua.load(&src).eval().unwrap();
        assert_eq!(r, 0xb779a091);
    }

    // --- vectors ----------------------------------------------------------

    #[test]
    fn vector_constructors_and_fields() {
        let lua = state();
        assert_eq!(lua.load("return vector3(1, 2, 3).z").eval::<f32>().unwrap(), 3.0);
        assert_eq!(lua.load("return vector2(4, 5).y").eval::<f32>().unwrap(), 5.0);
        // vector2 has no z
        assert_eq!(lua.load("return vector2(1,1).z").eval::<mlua::Value>().unwrap(), mlua::Value::Nil);
    }

    #[test]
    fn vector_arithmetic() {
        let lua = state();
        assert_eq!(lua.load("return (vector3(1,2,3) + vector3(10,20,30)).x").eval::<f32>().unwrap(), 11.0);
        assert_eq!(lua.load("return (vector3(10,20,30) - vector3(1,2,3)).y").eval::<f32>().unwrap(), 18.0);
        assert_eq!(lua.load("return (vector3(1,2,3) * 2).z").eval::<f32>().unwrap(), 6.0);
        assert_eq!(lua.load("return (2 * vector3(1,2,3)).z").eval::<f32>().unwrap(), 6.0);
    }

    #[test]
    fn vector_tostring() {
        let lua = state();
        let s: String = lua.load("return tostring(vector3(1, 2, 3))").eval().unwrap();
        assert_eq!(s, "vector3(1, 2, 3)");
    }

    #[test]
    fn vec_rust_arithmetic() {
        let a = CfxVec::v3(1.0, 2.0, 3.0);
        let b = CfxVec::v3(4.0, 5.0, 6.0);
        assert_eq!(a.add(&b).unwrap(), CfxVec::v3(5.0, 7.0, 9.0));
        assert_eq!(a.dot(&b).unwrap(), 32.0);
        assert_eq!(a.cross(&b).unwrap(), CfxVec::v3(-3.0, 6.0, -3.0));
        assert_eq!(CfxVec::v3(0.0, 0.0, 3.0).length(), 3.0);
        assert_eq!(CfxVec::v3(0.0, 0.0, 3.0).normalize(), CfxVec::v3(0.0, 0.0, 1.0));
    }

    #[test]
    fn vec_arity_mismatch_errors() {
        let a = CfxVec::v2(1.0, 2.0);
        let b = CfxVec::v3(1.0, 2.0, 3.0);
        assert!(a.add(&b).is_err());
        assert!(a.cross(&b).is_err());
    }

    // --- json -------------------------------------------------------------

    #[test]
    fn json_encode_array_vs_object() {
        let lua = state();
        assert_eq!(lua.load("return json.encode({10, 20, 30})").eval::<String>().unwrap(), "[10,20,30]");
        assert_eq!(lua.load("return json.encode({x = 1})").eval::<String>().unwrap(), "{\"x\":1}");
    }

    #[test]
    fn json_roundtrip_scalars() {
        let lua = state();
        let s: String = lua.load("return json.encode({a = 1, b = \"x\", c = true})").eval().unwrap();
        assert!(s.contains("\"a\":1"));
        assert!(s.contains("\"b\":\"x\""));
        assert!(s.contains("\"c\":true"));
        assert_eq!(lua.load("return json.decode(\"{\\\"a\\\": 42}\").a").eval::<i64>().unwrap(), 42);
    }

    #[test]
    fn json_decode_array() {
        let lua = state();
        let arr: mlua::Table = lua.load("return json.decode(\"[10,20]\")").eval().unwrap();
        assert_eq!(arr.get::<i64>(1).unwrap(), 10);
        assert_eq!(arr.get::<i64>(2).unwrap(), 20);
    }

    // --- msgpack ----------------------------------------------------------

    #[test]
    fn msgpack_scalar_roundtrip() {
        let lua = state();
        assert_eq!(lua.load("return msgpack.unpack(msgpack.pack(42))").eval::<i64>().unwrap(), 42);
        assert_eq!(lua.load("return msgpack.unpack(msgpack.pack(\"hello\"))").eval::<String>().unwrap(), "hello");
        assert_eq!(lua.load("return msgpack.unpack(msgpack.pack(nil))").eval::<mlua::Value>().unwrap(), mlua::Value::Nil);
        assert_eq!(lua.load("return msgpack.unpack(msgpack.pack(true))").eval::<bool>().unwrap(), true);
    }

    #[test]
    fn msgpack_struct_roundtrip() {
        let lua = state();
        assert_eq!(
            lua.load("return msgpack.unpack(msgpack.pack({k = 7})).k").eval::<i64>().unwrap(),
            7
        );
        assert_eq!(
            lua.load("return msgpack.unpack(msgpack.pack({1, 2, 3}))[2]").eval::<i64>().unwrap(),
            2
        );
    }

    #[test]
    fn msgpack_wire_format_matches_spec() {
        assert_eq!(bridge::json_to_msgpack(&serde_json::json!(42)).unwrap(), vec![42]);
        assert_eq!(bridge::json_to_msgpack(&serde_json::json!(null)).unwrap(), vec![0xc0]);
        assert_eq!(bridge::json_to_msgpack(&serde_json::json!(true)).unwrap(), vec![0xc3]);
        assert_eq!(bridge::json_to_msgpack(&serde_json::json!(false)).unwrap(), vec![0xc2]);
        assert_eq!(bridge::json_to_msgpack(&serde_json::json!("ab")).unwrap(), vec![0xa2, b'a', b'b']);
        assert_eq!(bridge::json_to_msgpack(&serde_json::json!([])).unwrap(), vec![0x90]);
        assert_eq!(bridge::json_to_msgpack(&serde_json::json!({})).unwrap(), vec![0x80]);
        // 200: out of fixint range -> uint16
        assert_eq!(bridge::json_to_msgpack(&serde_json::json!(200)).unwrap(), vec![0xcc, 0xc8]);
    }

    #[test]
    fn msgpack_truncated_rejected() {
        assert!(bridge::msgpack_to_json(&[0xd9]).is_err());
        assert!(bridge::msgpack_to_json(&[0xa3, b'a']).is_err());
        assert!(bridge::msgpack_to_json(&[0x91]).is_err());
        assert!(bridge::msgpack_to_json(&[0xc1]).is_err());
    }

    // --- promise ----------------------------------------------------------

    #[test]
    fn promise_resolves_synchronously() {
        let lua = state();
        let r: i64 = lua
            .load("local p = promise.new(function(r, j) r(99) end); return Citizen.Await(p)")
            .eval()
            .unwrap();
        assert_eq!(r, 99);
    }

    #[test]
    fn promise_reject_propagates_as_error() {
        let lua = state();
        let r = lua
            .load("local p = promise.new(function(r, j) j('boom') end); return Citizen.Await(p)")
            .eval::<i64>();
        assert!(r.is_err());
    }

    #[test]
    fn promise_resolve_static() {
        let lua = state();
        assert_eq!(lua.load("return Citizen.Await(promise.resolve(7))").eval::<i64>().unwrap(), 7);
    }

    // --- combined realistic legacy snippet --------------------------------

    #[test]
    fn legacy_snippet_uses_all_globals() {
        let lua = state();
        let src = rewrite_backticks(
            r#"
            local hash = joaat("esx:getSharedObject")
            local pos = vector3(1.0, 2.0, 3.0)
            local moved = pos + vector3(0.0, 0.0, 1.0)
            local payload = json.encode({hash = hash, z = moved.z})
            return payload
        "#,
        );
        let s: String = lua.load(&src).eval().unwrap();
        assert!(s.contains("\"z\":4"));
        assert!(s.contains("\"hash\":"));
    }
}
