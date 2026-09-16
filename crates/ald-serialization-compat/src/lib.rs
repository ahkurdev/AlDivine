//! Citizen-compatible serialization.
//!
//! FiveM's scripting runtimes exchange event arguments as MessagePack, not JSON.
//! That is not an arbitrary choice: msgpack carries the exact type tags Aldivine
//! must reproduce to interoperate — most importantly the Cfx extension types:
//!
//!   ext 20 = vector2   (2 x f32, little-endian)
//!   ext 21 = vector3   (3 x f32)
//!   ext 22 = vector4   (4 x f32)
//!   ext 23 = quaternion(4 x f32)
//!   ext 10 = funcref   (remote function reference)
//!   ext 11 = localfuncref
//!
//! A JSON event pipe silently flattens vectors into arrays and loses nil-vs-false
//! distinction, which changes handler behaviour in real resources. This crate is
//! the wire format for cross-runtime events; JSON stays only for NUI and tooling.
//!
//! The encoder/decoder here are hand-written against the msgpack spec so the crate
//! stays dependency-free and fuzzable. No invented formats: the ext tags above are
//! taken from CitizenFX's own `MsgPackDeserializer.cs` ext-type table.

use ald_core::AldError;

/// CitizenFX msgpack extension type tags.
pub mod ext {
    pub const FUNCREF: i8 = 10;
    pub const LOCAL_FUNCREF: i8 = 11;
    pub const VECTOR2: i8 = 20;
    pub const VECTOR3: i8 = 21;
    pub const VECTOR4: i8 = 22;
    pub const QUATERNION: i8 = 23;
}

/// Value with Citizen-compatible semantics. Order matters for the wire: the
/// encoder writes exactly what the variant holds, no implicit conversions.
#[derive(Debug, Clone, PartialEq)]
pub enum CfxValue {
    Nil,
    Bool(bool),
    /// Integers up to i64; the encoder picks the narrowest representation.
    Int(i64),
    /// Always written as f64 (msgpack `double`, 0xCB) — CfxLua numbers are doubles.
    Float(f64),
    Str(String),
    Bin(Vec<u8>),
    Array(Vec<CfxValue>),
    /// MessagePack maps hold arbitrary string keys.
    Map(Vec<(CfxValue, CfxValue)>),
    Vector2(f32, f32),
    Vector3(f32, f32, f32),
    Vector4(f32, f32, f32, f32),
    Quat(f32, f32, f32, f32),
    /// Opaque ext payload with an explicit tag, for funcrefs and forward-compat
    /// with types Aldivine does not yet model.
    Ext(i8, Vec<u8>),
}

impl CfxValue {
    pub fn is_nil(&self) -> bool {
        matches!(self, CfxValue::Nil)
    }

    /// True for the four vector family variants (quaternion included).
    pub fn is_vector(&self) -> bool {
        matches!(self, CfxValue::Vector2(..) | CfxValue::Vector3(..) | CfxValue::Vector4(..) | CfxValue::Quat(..))
    }

    /// Component count for vector family values; 0 otherwise.
    pub fn components(&self) -> usize {
        match self {
            CfxValue::Vector2(..) => 2,
            CfxValue::Vector3(..) => 3,
            CfxValue::Vector4(..) | CfxValue::Quat(..) => 4,
            _ => 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Encoder
// ---------------------------------------------------------------------------

/// Encode a value to Citizen-compatible msgpack bytes.
pub fn pack(v: &CfxValue) -> Result<Vec<u8>, AldError> {
    let mut out = Vec::with_capacity(32);
    write_value(&mut out, v)?;
    Ok(out)
}

fn write_value(out: &mut Vec<u8>, v: &CfxValue) -> Result<(), AldError> {
    match v {
        CfxValue::Nil => out.push(0xC0),
        CfxValue::Bool(false) => out.push(0xC2),
        CfxValue::Bool(true) => out.push(0xC3),
        CfxValue::Int(n) => write_int(out, *n),
        CfxValue::Float(f) => {
            out.push(0xCB);
            out.extend_from_slice(&f.to_be_bytes());
        }
        CfxValue::Str(s) => write_str(out, s),
        CfxValue::Bin(b) => write_bin(out, b),
        CfxValue::Array(items) => write_array(out, items)?,
        CfxValue::Map(entries) => write_map(out, entries)?,
        CfxValue::Vector2(x, y) => write_vector(out, ext::VECTOR2, &[*x, *y]),
        CfxValue::Vector3(x, y, z) => write_vector(out, ext::VECTOR3, &[*x, *y, *z]),
        CfxValue::Vector4(x, y, z, w) => write_vector(out, ext::VECTOR4, &[*x, *y, *z, *w]),
        CfxValue::Quat(x, y, z, w) => write_vector(out, ext::QUATERNION, &[*x, *y, *z, *w]),
        CfxValue::Ext(tag, data) => write_ext(out, *tag, data)?,
    }
    Ok(())
}

fn write_int(out: &mut Vec<u8>, n: i64) {
    // Positive values: prefer unsigned encodings; negative: signed, narrowest first.
    if (0..=127).contains(&n) {
        out.push(n as u8);
    } else if (0..=u8::MAX as i64).contains(&n) {
        out.push(0xCC);
        out.push(n as u8);
    } else if (0..=u16::MAX as i64).contains(&n) {
        out.push(0xCD);
        out.extend_from_slice(&n.to_be_bytes());
    } else if (0..=u32::MAX as i64).contains(&n) {
        out.push(0xCE);
        out.extend_from_slice(&n.to_be_bytes());
    } else if n >= 0 {
        out.push(0xCF);
        out.extend_from_slice(&n.to_be_bytes());
    } else if (i8::MIN as i64..=0).contains(&n) {
        // negative fixint covers -32..-1; int8 covers the rest of i8.
        if n >= -32 {
            out.push(n as i8 as u8);
        } else {
            out.push(0xD0);
            out.push(n as i8 as u8);
        }
    } else if (i16::MIN as i64..=i16::MAX as i64).contains(&n) {
        out.push(0xD1);
        out.extend_from_slice(&(n as i16).to_be_bytes());
    } else if (i32::MIN as i64..=i32::MAX as i64).contains(&n) {
        out.push(0xD2);
        out.extend_from_slice(&(n as i32).to_be_bytes());
    } else {
        out.push(0xD3);
        out.extend_from_slice(&n.to_be_bytes());
    }
}

fn write_str(out: &mut Vec<u8>, s: &str) {
    let b = s.as_bytes();
    if b.len() < 32 {
        out.push(0xA0 | b.len() as u8);
    } else if b.len() <= u8::MAX as usize {
        out.push(0xD9);
        out.push(b.len() as u8);
    } else if b.len() <= u16::MAX as usize {
        out.push(0xDA);
        out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    } else {
        out.push(0xDB);
        out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    }
    out.extend_from_slice(b);
}

fn write_bin(out: &mut Vec<u8>, b: &[u8]) {
    if b.len() <= u8::MAX as usize {
        out.push(0xC4);
        out.push(b.len() as u8);
    } else if b.len() <= u16::MAX as usize {
        out.push(0xC5);
        out.extend_from_slice(&(b.len() as u16).to_be_bytes());
    } else {
        out.push(0xC6);
        out.extend_from_slice(&(b.len() as u32).to_be_bytes());
    }
    out.extend_from_slice(b);
}

fn write_array(out: &mut Vec<u8>, items: &[CfxValue]) -> Result<(), AldError> {
    if items.len() < 16 {
        out.push(0x90 | items.len() as u8);
    } else if items.len() <= u16::MAX as usize {
        out.push(0xDC);
        out.extend_from_slice(&(items.len() as u16).to_be_bytes());
    } else {
        out.push(0xDD);
        out.extend_from_slice(&(items.len() as u32).to_be_bytes());
    }
    for item in items {
        write_value(out, item)?;
    }
    Ok(())
}

fn write_map(out: &mut Vec<u8>, entries: &[(CfxValue, CfxValue)]) -> Result<(), AldError> {
    if entries.len() < 16 {
        out.push(0x80 | entries.len() as u8);
    } else if entries.len() <= u16::MAX as usize {
        out.push(0xDE);
        out.extend_from_slice(&(entries.len() as u16).to_be_bytes());
    } else {
        out.push(0xDF);
        out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    }
    for (k, v) in entries {
        write_value(out, k)?;
        write_value(out, v)?;
    }
    Ok(())
}

/// Vectors travel as msgpack ext: one length byte per f32, then the tag, then
/// big-endian f32 components. Mirrors CitizenFX's `UnpackExt` exactly.
fn write_vector(out: &mut Vec<u8>, tag: i8, comps: &[f32]) {
    // fixext1/2/4/8/16 cover 1,2,4,8,16 bytes; vector2 is 8 (fixext8 does not fit
    // 8+tag) and vector3 is 12, so both go through ext8/ext16.
    let n = comps.len() * 4;
    if n <= u8::MAX as usize {
        out.push(0xC7);
        out.push(n as u8);
    } else if n <= u16::MAX as usize {
        out.push(0xC8);
        out.extend_from_slice(&(n as u16).to_be_bytes());
    } else {
        out.push(0xC9);
        out.extend_from_slice(&(n as u32).to_be_bytes());
    }
    out.push(tag as u8);
    for c in comps {
        out.extend_from_slice(&c.to_be_bytes());
    }
}

fn write_ext(out: &mut Vec<u8>, tag: i8, data: &[u8]) -> Result<(), AldError> {
    let n = data.len();
    match n {
        1 => out.push(0xD4),
        2 => out.push(0xD5),
        4 => out.push(0xD6),
        8 => out.push(0xD7),
        16 => out.push(0xD8),
        _ if n <= u8::MAX as usize => {
            out.push(0xC7);
            out.push(n as u8);
        }
        _ if n <= u16::MAX as usize => {
            out.push(0xC8);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        _ if n <= u32::MAX as usize => {
            out.push(0xC9);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        _ => return Err(AldError::Protocol("ext payload exceeds 4GiB".into())),
    }
    out.push(tag as u8);
    out.extend_from_slice(data);
    Ok(())
}

// ---------------------------------------------------------------------------
// Decoder
// ---------------------------------------------------------------------------

/// Decode Citizen-compatible msgpack bytes into a value.
pub fn unpack(bytes: &[u8]) -> Result<CfxValue, AldError> {
    let mut p = Cursor { bytes, pos: 0 };
    let v = read_value(&mut p)?;
    Ok(v)
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn read_u8(&mut self) -> Result<u8, AldError> {
        let b = self.bytes.get(self.pos).copied();
        b.ok_or_else(|| AldError::Protocol("unexpected end of msgpack buffer".into())).inspect(|_| self.pos += 1)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], AldError> {
        if self.pos + n > self.bytes.len() {
            return Err(AldError::Protocol("msgpack field overruns buffer".into()));
        }
        let s = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn read_u16(&mut self) -> Result<u16, AldError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn read_u32(&mut self) -> Result<u32, AldError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn read_f32(&mut self) -> Result<f32, AldError> {
        Ok(f32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn read_f64(&mut self) -> Result<f64, AldError> {
        Ok(f64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
}

fn read_value(c: &mut Cursor<'_>) -> Result<CfxValue, AldError> {
    let tag = c.read_u8()?;
    match tag {
        0xC0 => Ok(CfxValue::Nil),
        0xC2 => Ok(CfxValue::Bool(false)),
        0xC3 => Ok(CfxValue::Bool(true)),
        0xCA => Ok(CfxValue::Float(c.read_f32()? as f64)),
        0xCB => Ok(CfxValue::Float(c.read_f64()?)),
        0xCC => Ok(CfxValue::Int(c.read_u8()? as i64)),
        0xCD => Ok(CfxValue::Int(c.read_u16()? as i64)),
        0xCE => Ok(CfxValue::Int(c.read_u32()? as i64)),
        0xCF => Ok(CfxValue::Int(u64::from_be_bytes(c.take(8)?.try_into().unwrap()) as i64)),
        0xD0 => Ok(CfxValue::Int(c.read_u8()? as i8 as i64)),
        0xD1 => Ok(CfxValue::Int(i16::from_be_bytes(c.take(2)?.try_into().unwrap()) as i64)),
        0xD2 => Ok(CfxValue::Int(i32::from_be_bytes(c.take(4)?.try_into().unwrap()) as i64)),
        0xD3 => Ok(CfxValue::Int(i64::from_be_bytes(c.take(8)?.try_into().unwrap()))),
        0xC4 => {
            let n = c.read_u8()? as usize;
            read_bin(c, n)
        }
        0xC5 => {
            let n = c.read_u16()? as usize;
            read_bin(c, n)
        }
        0xC6 => {
            let n = c.read_u32()? as usize;
            read_bin(c, n)
        }
        0xC7 => {
            let n = c.read_u8()? as usize;
            read_ext(c, n)
        }
        0xC8 => {
            let n = c.read_u16()? as usize;
            read_ext(c, n)
        }
        0xC9 => {
            let n = c.read_u32()? as usize;
            read_ext(c, n)
        }
        0xD4 => read_ext(c, 1),
        0xD5 => read_ext(c, 2),
        0xD6 => read_ext(c, 4),
        0xD7 => read_ext(c, 8),
        0xD8 => read_ext(c, 16),
        0xD9 => {
            let n = c.read_u8()? as usize;
            read_str(c, n)
        }
        0xDA => {
            let n = c.read_u16()? as usize;
            read_str(c, n)
        }
        0xDB => {
            let n = c.read_u32()? as usize;
            read_str(c, n)
        }
        0xDC => {
            let n = c.read_u16()? as usize;
            read_array(c, n)
        }
        0xDD => {
            let n = c.read_u32()? as usize;
            read_array(c, n)
        }
        0xDE => {
            let n = c.read_u16()? as usize;
            read_map(c, n)
        }
        0xDF => {
            let n = c.read_u32()? as usize;
            read_map(c, n)
        }
        t if t <= 0x7F => Ok(CfxValue::Int(t as i64)),
        t if (0x80..=0x8F).contains(&t) => read_map(c, (t & 0x0F) as usize),
        t if (0x90..=0x9F).contains(&t) => read_array(c, (t & 0x0F) as usize),
        t if (0xA0..=0xBF).contains(&t) => read_str(c, (t & 0x1F) as usize),
        t if t >= 0xE0 => Ok(CfxValue::Int(t as i8 as i64)),
        // 0xF0-0xFF region unused by msgpack.
        other => Err(AldError::Protocol(format!("invalid msgpack tag 0x{other:02X}"))),
    }
}

fn read_bin(c: &mut Cursor<'_>, n: usize) -> Result<CfxValue, AldError> {
    Ok(CfxValue::Bin(c.take(n)?.to_vec()))
}

fn read_str(c: &mut Cursor<'_>, n: usize) -> Result<CfxValue, AldError> {
    let raw = c.take(n)?;
    let s = std::str::from_utf8(raw).map_err(|_| AldError::Protocol("msgpack string is not valid UTF-8".into()))?;
    Ok(CfxValue::Str(s.to_string()))
}

fn read_array(c: &mut Cursor<'_>, n: usize) -> Result<CfxValue, AldError> {
    let mut items = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        items.push(read_value(c)?);
    }
    Ok(CfxValue::Array(items))
}

fn read_map(c: &mut Cursor<'_>, n: usize) -> Result<CfxValue, AldError> {
    let mut entries = Vec::with_capacity(n.min(1024));
    for _ in 0..n {
        let k = read_value(c)?;
        let v = read_value(c)?;
        entries.push((k, v));
    }
    Ok(CfxValue::Map(entries))
}

fn read_ext(c: &mut Cursor<'_>, n: usize) -> Result<CfxValue, AldError> {
    let tag = c.read_u8()? as i8;
    // Decode f32 components straight off the cursor; the raw slice is kept only
    // for ext types we don't model, so unknown tags survive transit intact.
    match (tag, n) {
        (ext::VECTOR2, 8) => Ok(CfxValue::Vector2(c.read_f32()?, c.read_f32()?)),
        (ext::VECTOR3, 12) => Ok(CfxValue::Vector3(c.read_f32()?, c.read_f32()?, c.read_f32()?)),
        (ext::VECTOR4, 16) => Ok(CfxValue::Vector4(c.read_f32()?, c.read_f32()?, c.read_f32()?, c.read_f32()?)),
        (ext::QUATERNION, 16) => Ok(CfxValue::Quat(c.read_f32()?, c.read_f32()?, c.read_f32()?, c.read_f32()?)),
        _ => {
            let data = c.take(n)?;
            Ok(CfxValue::Ext(tag, data.to_vec()))
        }
    }
}

// ---------------------------------------------------------------------------
// JSON bridge — for NUI and tooling only, never the event wire.
// ---------------------------------------------------------------------------

/// Convert a CfxValue to a plain JSON value. Vectors degrade to arrays, which is
/// the documented NUI contract; do not use this as a cross-runtime serializer.
pub fn to_json(v: &CfxValue) -> serde_json::Value {
    match v {
        CfxValue::Nil => serde_json::Value::Null,
        CfxValue::Bool(b) => serde_json::Value::Bool(*b),
        CfxValue::Int(n) => serde_json::json!(*n),
        CfxValue::Float(f) => serde_json::json!(*f),
        CfxValue::Str(s) => serde_json::Value::String(s.clone()),
        CfxValue::Bin(b) => serde_json::json!(b),
        CfxValue::Array(items) => serde_json::Value::Array(items.iter().map(to_json).collect()),
        CfxValue::Map(entries) => {
            let mut m = serde_json::Map::new();
            for (k, val) in entries {
                if let CfxValue::Str(key) = k {
                    m.insert(key.clone(), to_json(val));
                }
            }
            serde_json::Value::Object(m)
        }
        CfxValue::Vector2(x, y) => serde_json::json!([*x, *y]),
        CfxValue::Vector3(x, y, z) => serde_json::json!([*x, *y, *z]),
        CfxValue::Vector4(x, y, z, w) => serde_json::json!([*x, *y, *z, *w]),
        CfxValue::Quat(x, y, z, w) => serde_json::json!([*x, *y, *z, *w]),
        CfxValue::Ext(tag, data) => serde_json::json!({"ext": *tag, "data": data}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rt(v: &CfxValue) -> CfxValue {
        let bytes = pack(v).unwrap();
        unpack(&bytes).unwrap()
    }

    #[test]
    fn scalars_roundtrip() {
        assert_eq!(rt(&CfxValue::Nil), CfxValue::Nil);
        assert_eq!(rt(&CfxValue::Bool(true)), CfxValue::Bool(true));
        assert_eq!(rt(&CfxValue::Bool(false)), CfxValue::Bool(false));
        assert_eq!(rt(&CfxValue::Int(0)), CfxValue::Int(0));
        assert_eq!(rt(&CfxValue::Int(127)), CfxValue::Int(127));
        assert_eq!(rt(&CfxValue::Int(128)), CfxValue::Int(128));
        assert_eq!(rt(&CfxValue::Int(-1)), CfxValue::Int(-1));
        assert_eq!(rt(&CfxValue::Int(-32)), CfxValue::Int(-32));
        assert_eq!(rt(&CfxValue::Int(-33)), CfxValue::Int(-33));
        assert_eq!(rt(&CfxValue::Int(i16::MIN as i64)), CfxValue::Int(i16::MIN as i64));
        assert_eq!(rt(&CfxValue::Int(i32::MIN as i64)), CfxValue::Int(i32::MIN as i64));
        assert_eq!(rt(&CfxValue::Int(i64::MAX)), CfxValue::Int(i64::MAX));
        assert_eq!(rt(&CfxValue::Str("hello".into())), CfxValue::Str("hello".into()));
        assert_eq!(rt(&CfxValue::Str("x".repeat(40))), CfxValue::Str("x".repeat(40)));
    }

    #[test]
    fn floats_are_doubles() {
        assert_eq!(rt(&CfxValue::Float(1.5)), CfxValue::Float(1.5));
        assert_eq!(rt(&CfxValue::Float(1e300)), CfxValue::Float(1e300));
    }

    #[test]
    fn vectors_roundtrip_with_citizen_ext_tags() {
        let v2 = CfxValue::Vector2(1.0, 2.0);
        let v3 = CfxValue::Vector3(1.0, 2.0, 3.0);
        let v4 = CfxValue::Vector4(1.0, 2.0, 3.0, 4.0);
        let q = CfxValue::Quat(0.707, 0.0, 0.707, 0.0);
        assert_eq!(rt(&v2), v2);
        assert_eq!(rt(&v3), v3);
        assert_eq!(rt(&v4), v4);
        assert_eq!(rt(&q), q);

        // Wire must carry the exact CitizenFX ext tag.
        let b3 = pack(&v3).unwrap();
        assert_eq!(b3[0], 0xC7); // ext8
        assert_eq!(b3[1], 12); // 3 x f32
        assert_eq!(b3[2], ext::VECTOR3 as u8);
        assert_eq!(b3.len(), 3 + 1 + 12 - 1); // 2 hdr + 1 tag + 12 data

        let b2 = pack(&v2).unwrap();
        assert_eq!(b2[0], 0xC7); // ext8
        assert_eq!(b2[1], 8); // 2 x f32
        assert_eq!(b2[2], ext::VECTOR2 as u8);
    }

    #[test]
    fn arrays_and_maps_roundtrip() {
        let a = CfxValue::Array(vec![CfxValue::Int(1), CfxValue::Str("a".into()), CfxValue::Nil]);
        assert_eq!(rt(&a), a);

        let big = CfxValue::Array((0..40).map(CfxValue::Int).collect());
        assert_eq!(rt(&big), big);

        let m = CfxValue::Map(vec![
            (CfxValue::Str("hp".into()), CfxValue::Int(100)),
            (CfxValue::Str("name".into()), CfxValue::Str("Test".into())),
        ]);
        assert_eq!(rt(&m), m);
    }

    #[test]
    fn bin_roundtrip() {
        let b = CfxValue::Bin(vec![0xDE, 0xAD, 0xBE, 0xEF]);
        assert_eq!(rt(&b), b);
    }

    #[test]
    fn unknown_ext_is_preserved_not_dropped() {
        // A future CitizenFX ext type must survive transit, not vanish.
        let e = CfxValue::Ext(42, vec![1, 2, 3, 4]);
        assert_eq!(rt(&e), e);
    }

    #[test]
    fn funcref_ext_tag_survives() {
        let e = CfxValue::Ext(ext::FUNCREF, vec![0; 16]);
        assert_eq!(rt(&e), e);
        let e = CfxValue::Ext(ext::LOCAL_FUNCREF, vec![0; 16]);
        assert_eq!(rt(&e), e);
    }

    #[test]
    fn nil_and_false_are_distinct() {
        // JSON collapses nil to null and keeps false, but the wire must keep both.
        let n = pack(&CfxValue::Nil).unwrap();
        let f = pack(&CfxValue::Bool(false)).unwrap();
        assert_ne!(n, f);
        assert_eq!(n, vec![0xC0]);
        assert_eq!(f, vec![0xC2]);
    }

    #[test]
    fn truncated_input_is_rejected() {
        assert!(unpack(&[0xC8]).is_err());
        assert!(unpack(&[0xDB, 0x10, 0x00]).is_err());
        assert!(unpack(&[]).is_err());
    }

    #[test]
    fn invalid_tag_rejected() {
        // 0xC1 is never a valid msgpack type byte.
        assert!(unpack(&[0xC1]).is_err());
    }

    #[test]
    fn deeply_nested_structure_is_bounded_by_input() {
        // 100-deep arrays must still decode, proving no recursion blowup at
        // realistic event depths.
        let mut v = CfxValue::Int(1);
        for _ in 0..100 {
            v = CfxValue::Array(vec![v]);
        }
        assert_eq!(rt(&v), v);
    }

    #[test]
    fn json_bridge_degrades_vectors_to_arrays() {
        let v = CfxValue::Vector3(1.0, 2.0, 3.0);
        assert_eq!(to_json(&v), serde_json::json!([1.0_f32, 2.0, 3.0]));
    }

    #[test]
    fn wire_matches_reference_msgpack_vectors() {
        // Golden vectors: the exact bytes stock msgpack emits for CitizenFX ext
        // types (ext8 + big-endian f32). If this breaks, we changed the wire
        // format and broke interop, not just internal round-tripping.
        let cases: &[(i8, &[f32], &[u8])] = &[
            (20, &[1.0, 2.0], &[0xC7, 8, 20, 0x3F, 0x80, 0, 0, 0x40, 0, 0, 0]),
            (21, &[1.0, 2.0, 3.0], &[0xC7, 12, 21, 0x3F, 0x80, 0, 0, 0x40, 0, 0, 0, 0x40, 0x40, 0, 0]),
            (
                22,
                &[1.0, 2.0, 3.0, 4.0],
                &[0xC7, 16, 22, 0x3F, 0x80, 0, 0, 0x40, 0, 0, 0, 0x40, 0x40, 0, 0, 0x40, 0x80, 0, 0],
            ),
        ];
        for (tag, comps, expected) in cases {
            let v = match comps.len() {
                2 => CfxValue::Vector2(comps[0], comps[1]),
                3 => CfxValue::Vector3(comps[0], comps[1], comps[2]),
                _ => CfxValue::Vector4(comps[0], comps[1], comps[2], comps[3]),
            };
            let packed = pack(&v).unwrap();
            assert_eq!(packed.as_slice(), *expected, "wire mismatch for ext tag {tag}");
            assert_eq!(unpack(expected).unwrap(), v, "decode mismatch for ext tag {tag}");
        }
    }

    #[test]
    fn empty_containers_roundtrip() {
        assert_eq!(rt(&CfxValue::Array(vec![])), CfxValue::Array(vec![]));
        assert_eq!(rt(&CfxValue::Map(vec![])), CfxValue::Map(vec![]));
        assert_eq!(rt(&CfxValue::Str("".into())), CfxValue::Str("".into()));
    }

    #[test]
    fn vector_helpers() {
        assert!(CfxValue::Vector3(0.0, 0.0, 0.0).is_vector());
        assert!(!CfxValue::Int(3).is_vector());
        assert_eq!(CfxValue::Vector3(0.0, 0.0, 0.0).components(), 3);
        assert_eq!(CfxValue::Quat(0.0, 0.0, 0.0, 0.0).components(), 4);
        assert!(CfxValue::Nil.is_nil());
    }
}
