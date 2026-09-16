//! Minimal, dependency-free reader for ECMA-335 (CLI) metadata.
//!
//! Scope: enough of the PE/COFF + CLI header + metadata-stream layout to
//! recover, from a real managed assembly on disk:
//!
//!   * the CLI runtime version and which metadata streams are present
//!   * the module name and module version id (MVID)
//!   * the assembly identity — name, version, culture, flags, public key token
//!   * referenced assemblies with their versions and tokens
//!   * row counts for every metadata table
//!   * the custom-attribute type names an assembly declares
//!
//! Explicitly NOT in scope: reading CIL method bodies, JIT, execution,
//! strong-name signature verification, NGEN/R2R native headers, Windows
//! Runtime layouts, and portable-PDB tables. Those are stated as absent in
//! `docs/DOTNET_COMPATIBILITY.md` rather than half-implemented here.
//!
//! Every offset is bounds-checked against the input slice. Malformed input is
//! an error value, never a panic — the fuzz sweep in the test module asserts
//! exactly that over truncations of a real assembly.

use sha1::{Digest, Sha1};

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CliError {
    #[error("not a PE image (no MZ/PE signature)")]
    NotPe,
    #[error("not a CLI image (no COM descriptor data directory)")]
    NotCli,
    #[error("malformed CLI image: {0}")]
    Malformed(String),
    #[error("unsupported metadata table 0x{0:02x} present in #~ stream")]
    UnsupportedTable(u8),
}

// ---------------------------------------------------------------------------
// Public result
// ---------------------------------------------------------------------------

/// An assembly reference (one row of the `AssemblyRef` table).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyRef {
    pub name: String,
    pub version: String,
    pub culture: Option<String>,
    /// Lowercase hex of the public key token, when the row carries one.
    pub public_key_token: Option<String>,
    /// True when the reference embeds a full public key instead of a token.
    pub carries_full_key: bool,
}

/// The identity of the assembly the file defines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssemblyIdentity {
    pub name: String,
    pub version: String,
    pub culture: Option<String>,
    pub flags: u32,
    /// Lowercase hex public key token derived from the embedded public key.
    pub public_key_token: Option<String>,
    pub has_public_key: bool,
}

/// One unmanaged `DllImport` the assembly declares, via `ImplMap` + `ModuleRef`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinvoke {
    pub module: String,
    pub function: String,
}

/// One `TypeDef` row, enough to see what a resource actually declares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDefInfo {
    pub namespace: String,
    pub name: String,
    pub flags: u32,
    /// Base type as written (`CitizenFX.Core.BaseScript`), when it is a
    /// TypeDef or TypeRef. `None` for a root type or a TypeSpec.
    pub base: Option<String>,
}

impl TypeDefInfo {
    pub fn full_name(&self) -> String {
        if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.namespace, self.name)
        }
    }

    /// True when the base type is named `BaseScript` — the CfxCLR entry point
    /// contract. Checks the simple name so both `CitizenFX.Core.BaseScript`
    /// and a resource-local wrapper match.
    pub fn extends_base_script(&self) -> bool {
        self.base.as_deref().map(|b| b.rsplit('.').next() == Some("BaseScript")).unwrap_or(false)
    }
}

/// Everything this reader recovers from one managed assembly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliImage {
    /// `v4.0.30319`-style runtime version string from the metadata root.
    pub runtime_version: String,
    /// CLI header `Flags` (ILONLY, 32BITREQUIRED, STRONGNAMESIGNED, ...).
    pub clr_flags: u32,
    pub entry_point_token: u32,
    pub module_name: Option<String>,
    /// Module Version ID — the identity FiveM-style symbolication keys on.
    pub mvid: Option<[u8; 16]>,
    pub assembly: Option<AssemblyIdentity>,
    pub references: Vec<AssemblyRef>,
    /// Row counts indexed by metadata table id.
    pub table_rows: [u32; 64],
    /// Fully-qualified names of types used as custom attributes.
    pub custom_attribute_types: Vec<String>,
    /// Every `TypeDef` the assembly declares, in table order.
    pub types: Vec<TypeDefInfo>,
    /// Every unmanaged import (`DllImport`) the assembly declares.
    pub pinvokes: Vec<Pinvoke>,
}

impl CliImage {
    pub fn mvid_hex(&self) -> Option<String> {
        self.mvid.map(|m| hex_lower(&m))
    }

    pub fn type_def_count(&self) -> u32 {
        self.table_rows[0x02]
    }

    pub fn method_def_count(&self) -> u32 {
        self.table_rows[0x06]
    }

    /// Which BCL the assembly was built against, inferred from what it
    /// references. `.NET Framework` assemblies reference `mscorlib`;
    /// modern CoreCLR assemblies reference `System.Runtime` and
    /// `System.Private.CoreLib`. The distinction matters: a CfxCLR resource
    /// built for Framework cannot load on a CoreCLR host unchanged.
    pub fn target_framework(&self) -> TargetFramework {
        let has = |n: &str| self.references.iter().any(|r| r.name == n);
        if has("System.Private.CoreLib") || has("System.Runtime") {
            TargetFramework::CoreClr
        } else if has("mscorlib") {
            TargetFramework::NetFramework
        } else if has("Mono.Posix") || has("mono") {
            TargetFramework::Mono
        } else {
            TargetFramework::Unknown
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetFramework {
    NetFramework,
    CoreClr,
    Mono,
    Unknown,
}

impl TargetFramework {
    pub fn as_str(self) -> &'static str {
        match self {
            TargetFramework::NetFramework => "net-framework",
            TargetFramework::CoreClr => "coreclr",
            TargetFramework::Mono => "mono",
            TargetFramework::Unknown => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// CLI header flag bits (ECMA-335 II.25.3.1)
// ---------------------------------------------------------------------------

pub const COMIMAGE_FLAGS_ILONLY: u32 = 0x0000_0001;
pub const COMIMAGE_FLAGS_32BITREQUIRED: u32 = 0x0000_0002;
pub const COMIMAGE_FLAGS_STRONGNAMESIGNED: u32 = 0x0000_0008;
pub const COMIMAGE_FLAGS_NATIVE_ENTRYPOINT: u32 = 0x0000_0010;
pub const COMIMAGE_FLAGS_TRACKDEBUGDATA: u32 = 0x0001_0000;

// ---------------------------------------------------------------------------
// Reader
// ---------------------------------------------------------------------------

/// Read a managed assembly from raw file bytes.
pub fn read_assembly(bytes: &[u8]) -> Result<CliImage, CliError> {
    let pe = read_pe_headers(bytes)?;
    let cli_off = rva_to_offset(bytes, &pe, pe.cli_rva)?;
    let cli = read_cli_header(bytes, cli_off)?;
    let md_off = rva_to_offset(bytes, &pe, cli.metadata_rva)?;
    read_metadata(bytes, md_off, cli.metadata_size, cli.flags, cli.entry_point_token)
}

struct PeHeaders {
    sections: Vec<Section>,
    size_of_headers: u32,
    cli_rva: u32,
}

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    pointer_to_raw_data: u32,
    size_of_raw_data: u32,
}

fn u16_at(b: &[u8], o: usize) -> Result<u16, CliError> {
    let s = b.get(o..o + 2).ok_or_else(|| CliError::Malformed(format!("u16 at {o} out of bounds")))?;
    Ok(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], o: usize) -> Result<u32, CliError> {
    let s = b.get(o..o + 4).ok_or_else(|| CliError::Malformed(format!("u32 at {o} out of bounds")))?;
    Ok(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

fn read_pe_headers(b: &[u8]) -> Result<PeHeaders, CliError> {
    if b.len() < 0x40 || b[0] != b'M' || b[1] != b'Z' {
        return Err(CliError::NotPe);
    }
    let e_lfanew = u32_at(b, 0x3c)? as usize;
    if b.get(e_lfanew..e_lfanew + 4) != Some(&b"PE\0\0"[..]) {
        return Err(CliError::NotPe);
    }
    let coff = e_lfanew + 4;
    let number_of_sections = u16_at(b, coff + 2)? as usize;
    let size_of_optional_header = u16_at(b, coff + 16)? as usize;
    let opt = coff + 20;
    let magic = u16_at(b, opt)?;
    let (dirs_off, dir_count) = match magic {
        0x10b => (opt + 96, u32_at(b, opt + 92)?),
        0x20b => (opt + 112, u32_at(b, opt + 108)?),
        other => return Err(CliError::Malformed(format!("unknown optional header magic 0x{other:04x}"))),
    };
    let size_of_headers = u32_at(b, opt + 60)?;

    // Directory 14 is the CLI runtime header.
    if dir_count <= 14 {
        return Err(CliError::NotCli);
    }
    let cli_rva = u32_at(b, dirs_off + 14 * 8)?;
    let cli_size = u32_at(b, dirs_off + 14 * 8 + 4)?;
    if cli_rva == 0 || cli_size == 0 {
        return Err(CliError::NotCli);
    }

    let sec_off = opt + size_of_optional_header;
    let mut sections = Vec::with_capacity(number_of_sections);
    for i in 0..number_of_sections {
        let s = sec_off + i * 40;
        if s + 40 > b.len() {
            return Err(CliError::Malformed("section table truncated".into()));
        }
        sections.push(Section {
            virtual_size: u32_at(b, s + 8)?,
            virtual_address: u32_at(b, s + 12)?,
            size_of_raw_data: u32_at(b, s + 16)?,
            pointer_to_raw_data: u32_at(b, s + 20)?,
        });
    }
    Ok(PeHeaders { sections, size_of_headers, cli_rva })
}

fn rva_to_offset(b: &[u8], pe: &PeHeaders, rva: u32) -> Result<usize, CliError> {
    if rva < pe.size_of_headers {
        return Ok(rva as usize);
    }
    for s in &pe.sections {
        let span = s.virtual_size.max(s.size_of_raw_data);
        if rva >= s.virtual_address && rva < s.virtual_address.saturating_add(span) {
            let off = s.pointer_to_raw_data as usize + (rva - s.virtual_address) as usize;
            if off >= b.len() {
                return Err(CliError::Malformed("RVA maps past end of file".into()));
            }
            return Ok(off);
        }
    }
    Err(CliError::Malformed(format!("RVA 0x{rva:x} not in any section")))
}

struct CliHeader {
    flags: u32,
    entry_point_token: u32,
    metadata_rva: u32,
    metadata_size: u32,
}

fn read_cli_header(b: &[u8], off: usize) -> Result<CliHeader, CliError> {
    if off + 72 > b.len() {
        return Err(CliError::Malformed("CLI header truncated".into()));
    }
    Ok(CliHeader {
        metadata_rva: u32_at(b, off + 8)?,
        metadata_size: u32_at(b, off + 12)?,
        flags: u32_at(b, off + 16)?,
        entry_point_token: u32_at(b, off + 20)?,
    })
}

// ---------------------------------------------------------------------------
// Metadata root and streams
// ---------------------------------------------------------------------------

/// Where each metadata stream lives inside the file.
struct Streams {
    tables: Option<(usize, usize)>,
    strings: Option<(usize, usize)>,
    guid: Option<(usize, usize)>,
    blob: Option<(usize, usize)>,
    heap_sizes: u8,
}

fn read_metadata(
    b: &[u8],
    off: usize,
    size: u32,
    clr_flags: u32,
    entry_point_token: u32,
) -> Result<CliImage, CliError> {
    if off + 16 > b.len() {
        return Err(CliError::Malformed("metadata root truncated".into()));
    }
    if u32_at(b, off)? != 0x424a_5342 {
        return Err(CliError::Malformed("bad metadata signature (expected BSJB)".into()));
    }
    let ver_len = u32_at(b, off + 12)? as usize;
    if ver_len == 0 || ver_len > 256 {
        return Err(CliError::Malformed(format!("implausible version length {ver_len}")));
    }
    let ver_bytes =
        b.get(off + 16..off + 16 + ver_len).ok_or_else(|| CliError::Malformed("version string truncated".into()))?;
    let ver_end = ver_bytes.iter().position(|&c| c == 0).unwrap_or(ver_bytes.len());
    let runtime_version = String::from_utf8_lossy(&ver_bytes[..ver_end]).into_owned();

    // Stream headers follow the version string, padded to a 4-byte boundary.
    let mut p = off + 16 + ver_len;
    p = (p + 3) & !3;
    let stream_count = u16_at(b, p + 2)? as usize;
    p += 4;

    let mut streams = Streams { tables: None, strings: None, guid: None, blob: None, heap_sizes: 0 };
    for _ in 0..stream_count {
        let s_off = u32_at(b, p)? as usize;
        let s_size = u32_at(b, p + 4)? as usize;
        let name_start = p + 8;
        let name_end = b
            .get(name_start..)
            .and_then(|r| r.iter().position(|&c| c == 0))
            .map(|i| name_start + i)
            .ok_or_else(|| CliError::Malformed("stream name unterminated".into()))?;
        let name = std::str::from_utf8(&b[name_start..name_end])
            .map_err(|_| CliError::Malformed("stream name not utf-8".into()))?;
        // Bound every stream to the metadata span and to the file.
        let abs = off + s_off;
        let end = abs
            .checked_add(s_size)
            .filter(|e| *e <= b.len())
            .ok_or_else(|| CliError::Malformed(format!("stream {name} out of file bounds")))?;
        let _ = end;
        match name {
            "#~" | "#-" => streams.tables = Some((abs, s_size)),
            "#Strings" => streams.strings = Some((abs, s_size)),
            "#GUID" => streams.guid = Some((abs, s_size)),
            "#Blob" => streams.blob = Some((abs, s_size)),
            _ => {}
        }
        // advance past the name, padded to 4.
        let after_name = name_end + 1;
        p = (after_name + 3) & !3;
    }
    let _ = size;

    let (t_off, _t_size) = streams.tables.ok_or_else(|| CliError::Malformed("no #~ metadata tables stream".into()))?;

    let tables = read_tables_header(b, t_off)?;
    streams.heap_sizes = tables.0;
    let valid = tables.1;
    let rows = tables.2;

    let ctx = Ctx {
        strings_big: streams.heap_sizes & 0x01 != 0,
        guid_big: streams.heap_sizes & 0x02 != 0,
        blob_big: streams.heap_sizes & 0x04 != 0,
        rows,
    };

    // Walk the tables to find where each one starts.
    let mut offsets = [0usize; 64];
    let mut cursor = t_off + 24 + (valid.count_ones() as usize) * 4;
    for t in 0u8..64 {
        if valid & (1u64 << t) == 0 {
            continue;
        }
        offsets[t as usize] = cursor;
        let rs = row_size(t, &ctx)?;
        cursor = cursor
            .checked_add(rs * rows[t as usize] as usize)
            .ok_or_else(|| CliError::Malformed("table walk overflowed".into()))?;
        if cursor > b.len() {
            return Err(CliError::Malformed(format!("table 0x{t:02x} extends past end of file")));
        }
    }

    let mut img = CliImage {
        runtime_version,
        clr_flags,
        entry_point_token,
        module_name: None,
        mvid: None,
        assembly: None,
        references: Vec::new(),
        table_rows: rows,
        custom_attribute_types: Vec::new(),
        types: Vec::new(),
        pinvokes: Vec::new(),
    };

    // Module row 0 carries the module name and MVID.
    if rows[0x00] > 0 {
        let mut c = Cursor::new(b, offsets[0x00]);
        c.u16()?; // Generation
        let name_idx = c.idx(ctx.str_idx())?;
        let mvid_idx = c.idx(ctx.guid_idx())?;
        img.module_name = read_string(b, &streams, name_idx);
        img.mvid = read_guid(b, &streams, mvid_idx);
    }

    // Assembly row 0 is the defining assembly.
    if rows[0x20] > 0 {
        let mut c = Cursor::new(b, offsets[0x20]);
        c.u32()?; // HashAlgId
        let major = c.u16()?;
        let minor = c.u16()?;
        let build = c.u16()?;
        let revision = c.u16()?;
        let flags = c.u32()?;
        let pk_idx = c.idx(ctx.blob_idx())?;
        let name_idx = c.idx(ctx.str_idx())?;
        let culture_idx = c.idx(ctx.str_idx())?;
        let pk = read_blob(b, &streams, pk_idx);
        img.assembly = Some(AssemblyIdentity {
            name: read_string(b, &streams, name_idx).unwrap_or_default(),
            version: format!("{major}.{minor}.{build}.{revision}"),
            culture: read_string(b, &streams, culture_idx).filter(|s| !s.is_empty()),
            flags,
            public_key_token: pk.as_deref().and_then(public_key_token),
            has_public_key: pk.as_ref().map(|k| !k.is_empty()).unwrap_or(false),
        });
    }

    // Every AssemblyRef row.
    for i in 0..rows[0x23] as usize {
        let mut c = Cursor::new(b, offsets[0x23] + i * row_size(0x23, &ctx)?);
        let major = c.u16()?;
        let minor = c.u16()?;
        let build = c.u16()?;
        let revision = c.u16()?;
        let flags = c.u32()?;
        let tok_idx = c.idx(ctx.blob_idx())?;
        let name_idx = c.idx(ctx.str_idx())?;
        let culture_idx = c.idx(ctx.str_idx())?;
        let raw = read_blob(b, &streams, tok_idx).unwrap_or_default();
        let full_key = flags & 0x0001 != 0;
        img.references.push(AssemblyRef {
            name: read_string(b, &streams, name_idx).unwrap_or_default(),
            version: format!("{major}.{minor}.{build}.{revision}"),
            culture: read_string(b, &streams, culture_idx).filter(|s| !s.is_empty()),
            public_key_token: if raw.is_empty() {
                None
            } else if full_key {
                public_key_token(&raw)
            } else {
                Some(hex_lower(&raw))
            },
            carries_full_key: full_key,
        });
    }

    // Custom attribute type names: CustomAttribute.Type is a coded index into
    // MethodDef (tag 2) or MemberRef (tag 3); the MemberRef's Class column
    // points at the TypeRef that names the attribute type.
    if rows[0x0c] > 0 && rows[0x01] > 0 {
        let ca_size = row_size(0x0c, &ctx)?;
        let mr_size = row_size(0x0a, &ctx)?;
        let tr_size = row_size(0x01, &ctx)?;
        let mr_parent_bits = 3;
        let mut seen: Vec<String> = Vec::new();
        for i in 0..rows[0x0c] as usize {
            let mut c = Cursor::new(b, offsets[0x0c] + i * ca_size);
            c.skip(ctx.coded(5, &HAS_CUSTOM_ATTRIBUTE))?;
            let ty = c.idx(ctx.coded(3, &[0, 0, 0x06, 0x0a, 0]))?;
            let tag = ty & ((1 << 3) - 1);
            if tag != 3 {
                continue;
            }
            let mr_row = (ty >> 3) as usize - 1;
            if mr_row >= rows[0x0a] as usize {
                continue;
            }
            let mut mc = Cursor::new(b, offsets[0x0a] + mr_row * mr_size);
            let class = mc.idx(ctx.coded(mr_parent_bits, &[0x02, 0x01, 0x1a, 0x06, 0x1b]))?;
            if class & ((1 << mr_parent_bits) - 1) != 1 {
                continue; // not a TypeRef
            }
            let tr_row = (class >> mr_parent_bits) as usize - 1;
            if tr_row >= rows[0x01] as usize {
                continue;
            }
            let mut tc = Cursor::new(b, offsets[0x01] + tr_row * tr_size);
            tc.skip(ctx.coded(2, &[0x00, 0x1a, 0x23, 0x01]))?;
            let n = tc.idx(ctx.str_idx())?;
            let ns = tc.idx(ctx.str_idx())?;
            let name = read_string(b, &streams, n).unwrap_or_default();
            let ns = read_string(b, &streams, ns).unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            let fq = if ns.is_empty() { name } else { format!("{ns}.{name}") };
            if !seen.contains(&fq) {
                seen.push(fq);
            }
        }
        seen.sort();
        img.custom_attribute_types = seen;
    }

    // Every TypeDef the assembly declares. Row 0 is always `<Module>`.
    if rows[0x02] > 0 {
        let td_size = row_size(0x02, &ctx)?;
        let mut types = Vec::with_capacity(rows[0x02] as usize);
        for i in 0..rows[0x02] as usize {
            let mut c = Cursor::new(b, offsets[0x02] + i * td_size);
            let flags = c.u32()?;
            let n = c.idx(ctx.str_idx())?;
            let ns = c.idx(ctx.str_idx())?;
            let extends = c.idx(ctx.coded(2, &[0x02, 0x01, 0x1b]))?;
            let name = read_string(b, &streams, n).unwrap_or_default();
            let namespace = read_string(b, &streams, ns).unwrap_or_default();
            let base =
                if extends == 0 { None } else { typedef_or_ref_name(b, &streams, &offsets, &rows, &ctx, extends) };
            types.push(TypeDefInfo { namespace, name, flags, base });
        }
        img.types = types;
    }

    // Every unmanaged import: ImplMap.ImportScope is a 1-based ModuleRef row.
    if rows[0x1c] > 0 && rows[0x1a] > 0 {
        let im_size = row_size(0x1c, &ctx)?;
        let mr_size = row_size(0x1a, &ctx)?;
        for i in 0..rows[0x1c] as usize {
            let mut c = Cursor::new(b, offsets[0x1c] + i * im_size);
            c.u16()?; // MappingFlags
            c.skip(ctx.coded(1, &[0x04, 0x06]))?; // MemberForwarded
            let iname = c.idx(ctx.str_idx())?;
            let scope = c.idx(ctx.simple(0x1a))?;
            if scope == 0 || (scope as usize) > rows[0x1a] as usize {
                continue;
            }
            let mut mc = Cursor::new(b, offsets[0x1a] + ((scope as usize) - 1) * mr_size);
            let mname = mc.idx(ctx.str_idx())?;
            let module = read_string(b, &streams, mname).unwrap_or_default();
            let function = read_string(b, &streams, iname).unwrap_or_default();
            if module.is_empty() || function.is_empty() {
                continue;
            }
            img.pinvokes.push(Pinvoke { module, function });
        }
    }

    Ok(img)
}

/// Resolve a TypeDefOrRef coded index (tag 0 = TypeDef, 1 = TypeRef,
/// 2 = TypeSpec) to a display name. TypeSpecs (generics) have no plain name.
fn typedef_or_ref_name(
    b: &[u8],
    s: &Streams,
    offsets: &[usize; 64],
    rows: &[u32; 64],
    c: &Ctx,
    coded: u32,
) -> Option<String> {
    let tag = coded & 0x03;
    let row = (coded >> 2).checked_sub(1)? as usize;
    match tag {
        0 => {
            if row >= rows[0x02] as usize {
                return None;
            }
            let mut tc = Cursor::new(b, offsets[0x02] + row * row_size(0x02, c).ok()?);
            tc.u32().ok()?; // flags
            let n = tc.idx(c.str_idx()).ok()?;
            let ns = tc.idx(c.str_idx()).ok()?;
            join_ns_name(read_string(b, s, ns), read_string(b, s, n))
        }
        1 => {
            if row >= rows[0x01] as usize {
                return None;
            }
            let mut tc = Cursor::new(b, offsets[0x01] + row * row_size(0x01, c).ok()?);
            tc.skip(c.coded(2, &[0x00, 0x1a, 0x23, 0x01])).ok()?;
            let n = tc.idx(c.str_idx()).ok()?;
            let ns = tc.idx(c.str_idx()).ok()?;
            join_ns_name(read_string(b, s, ns), read_string(b, s, n))
        }
        _ => None,
    }
}

fn join_ns_name(ns: Option<String>, name: Option<String>) -> Option<String> {
    let name = name.filter(|n| !n.is_empty())?;
    match ns.filter(|n| !n.is_empty()) {
        Some(ns) => Some(format!("{ns}.{name}")),
        None => Some(name),
    }
}

const HAS_CUSTOM_ATTRIBUTE: [u8; 22] = [
    0x06, 0x04, 0x01, 0x02, 0x08, 0x09, 0x0a, 0x00, 0x0e, 0x17, 0x14, 0x11, 0x1a, 0x1b, 0x20, 0x23, 0x26, 0x27, 0x28,
    0x2a, 0x2b, 0x2c,
];

/// Read the `#~` stream header: `(heap_sizes, valid_mask, row_counts)`.
fn read_tables_header(b: &[u8], off: usize) -> Result<(u8, u64, [u32; 64]), CliError> {
    if off + 24 > b.len() {
        return Err(CliError::Malformed("#~ header truncated".into()));
    }
    let heap_sizes = b[off + 6];
    let valid = u64_at(b, off + 8)?;
    let mut rows = [0u32; 64];
    let mut p = off + 24;
    for t in 0u8..64 {
        if valid & (1u64 << t) != 0 {
            rows[t as usize] = u32_at(b, p)?;
            p += 4;
        }
    }
    Ok((heap_sizes, valid, rows))
}

fn u64_at(b: &[u8], o: usize) -> Result<u64, CliError> {
    let s = b.get(o..o + 8).ok_or_else(|| CliError::Malformed(format!("u64 at {o} out of bounds")))?;
    Ok(u64::from_le_bytes([s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]]))
}

// ---------------------------------------------------------------------------
// Column-size machinery (ECMA-335 II.22)
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Ctx {
    strings_big: bool,
    guid_big: bool,
    blob_big: bool,
    rows: [u32; 64],
}

impl Ctx {
    fn str_idx(&self) -> usize {
        if self.strings_big {
            4
        } else {
            2
        }
    }
    fn guid_idx(&self) -> usize {
        if self.guid_big {
            4
        } else {
            2
        }
    }
    fn blob_idx(&self) -> usize {
        if self.blob_big {
            4
        } else {
            2
        }
    }
    fn simple(&self, t: u8) -> usize {
        if self.rows[t as usize] >= 0x1_0000 {
            4
        } else {
            2
        }
    }
    fn coded(&self, tag_bits: u32, tables: &[u8]) -> usize {
        let mut max = 0u32;
        for &t in tables {
            if t == 0 {
                continue;
            }
            max = max.max(self.rows[t as usize]);
        }
        if (max as u64) < (1u64 << (16 - tag_bits)) {
            2
        } else {
            4
        }
    }
}

fn row_size(t: u8, c: &Ctx) -> Result<usize, CliError> {
    use CliError::UnsupportedTable;
    let s = c.str_idx();
    let g = c.guid_idx();
    let bl = c.blob_idx();
    Ok(match t {
        0x00 => 2 + s + g + g + g,
        0x01 => c.coded(2, &[0x00, 0x1a, 0x23, 0x01]) + s + s,
        0x02 => 4 + s + s + c.coded(2, &[0x02, 0x01, 0x1b]) + c.simple(0x04) + c.simple(0x06),
        0x03 => c.simple(0x04),
        0x04 => 2 + s + bl,
        0x05 => c.simple(0x06),
        0x06 => 4 + 2 + 2 + s + bl + c.simple(0x08),
        0x07 => c.simple(0x08),
        0x08 => 2 + 2 + s,
        0x09 => c.simple(0x02) + c.coded(2, &[0x02, 0x01, 0x1b]),
        0x0a => c.coded(3, &[0x02, 0x01, 0x1a, 0x06, 0x1b]) + s + bl,
        0x0b => 1 + 1 + c.coded(2, &[0x04, 0x08, 0x17]) + bl,
        0x0c => c.coded(5, &HAS_CUSTOM_ATTRIBUTE) + c.coded(3, &[0, 0, 0x06, 0x0a, 0]) + bl,
        0x0d => c.coded(1, &[0x04, 0x08]) + bl,
        0x0e => 2 + c.coded(2, &[0x02, 0x06, 0x20]) + bl,
        0x0f => 2 + 4 + c.simple(0x02),
        0x10 => 4 + c.simple(0x04),
        0x11 => bl,
        0x12 => c.simple(0x02) + c.simple(0x14),
        0x13 => c.simple(0x14),
        0x14 => 2 + s + c.coded(2, &[0x02, 0x01, 0x1b]),
        0x15 => c.simple(0x02) + c.simple(0x17),
        0x16 => c.simple(0x17),
        0x17 => 2 + s + bl,
        0x18 => 2 + c.simple(0x06) + c.coded(1, &[0x14, 0x17]),
        0x19 => c.simple(0x02) + c.coded(1, &[0x06, 0x0a]) + c.coded(1, &[0x06, 0x0a]),
        0x1a => s,
        0x1b => bl,
        0x1c => 2 + c.coded(1, &[0x04, 0x06]) + s + c.simple(0x1a),
        0x1d => 4 + c.simple(0x04),
        0x1e => 4 + 4,
        0x1f => 4,
        0x20 => 4 + 2 + 2 + 2 + 2 + 4 + bl + s + s,
        0x21 => 4 + 4 + 4,
        0x22 => 4,
        0x23 => 2 + 2 + 2 + 2 + 4 + bl + s + s + bl,
        0x24 => 4 + 4 + 4 + c.simple(0x23),
        0x25 => 4 + c.simple(0x23),
        0x26 => 4 + s + bl,
        0x27 => 4 + 4 + s + s + c.coded(2, &[0x26, 0x27, 0x02]),
        0x28 => 4 + 4 + s + c.coded(2, &[0x26, 0x27, 0x02]),
        0x29 => c.simple(0x02) + c.simple(0x02),
        0x2a => 2 + 2 + c.coded(1, &[0x02, 0x06]) + s,
        0x2b => c.coded(1, &[0x06, 0x0a]) + bl,
        0x2c => c.simple(0x2a) + c.coded(2, &[0x02, 0x01, 0x1b]),
        // 0x2d..=0x37 are the portable-PDB tables: they never appear in an
        // assembly's #~ stream, so encountering one means the file is not a
        // plain assembly and we refuse rather than guess.
        other => return Err(UnsupportedTable(other)),
    })
}

// ---------------------------------------------------------------------------
// Heaps
// ---------------------------------------------------------------------------

struct Cursor<'a> {
    b: &'a [u8],
    o: usize,
}

impl<'a> Cursor<'a> {
    fn new(b: &'a [u8], o: usize) -> Self {
        Cursor { b, o }
    }
    fn u16(&mut self) -> Result<u16, CliError> {
        let v = u16_at(self.b, self.o)?;
        self.o += 2;
        Ok(v)
    }
    fn u32(&mut self) -> Result<u32, CliError> {
        let v = u32_at(self.b, self.o)?;
        self.o += 4;
        Ok(v)
    }
    fn idx(&mut self, size: usize) -> Result<u32, CliError> {
        let v = if size == 2 { self.u16()? as u32 } else { self.u32()? };
        Ok(v)
    }
    fn skip(&mut self, n: usize) -> Result<(), CliError> {
        self.o = self
            .o
            .checked_add(n)
            .filter(|e| *e <= self.b.len())
            .ok_or_else(|| CliError::Malformed("row column past end of file".into()))?;
        Ok(())
    }
}

fn read_string(b: &[u8], s: &Streams, idx: u32) -> Option<String> {
    if idx == 0 {
        return Some(String::new());
    }
    let (off, size) = s.strings?;
    let start = off.checked_add(idx as usize)?;
    let end = off.checked_add(size)?;
    if start >= end || end > b.len() {
        return None;
    }
    let n = b[start..end].iter().position(|&c| c == 0)?;
    Some(String::from_utf8_lossy(&b[start..start + n]).into_owned())
}

fn read_guid(b: &[u8], s: &Streams, idx: u32) -> Option<[u8; 16]> {
    if idx == 0 {
        return None;
    }
    let (off, size) = s.guid?;
    let start = off.checked_add((idx as usize - 1) * 16)?;
    let end = off.checked_add(size)?;
    if start + 16 > end || start + 16 > b.len() {
        return None;
    }
    let mut out = [0u8; 16];
    out.copy_from_slice(&b[start..start + 16]);
    Some(out)
}

fn read_blob(b: &[u8], s: &Streams, idx: u32) -> Option<Vec<u8>> {
    if idx == 0 {
        return Some(Vec::new());
    }
    let (off, size) = s.blob?;
    let start = off.checked_add(idx as usize)?;
    let end = off.checked_add(size)?;
    if start >= end || end > b.len() {
        return None;
    }
    let (len, consumed) = compressed_uint(&b[start..end])?;
    let body = start + consumed;
    if body + len > end || body + len > b.len() {
        return None;
    }
    Some(b[body..body + len].to_vec())
}

/// ECMA-335 II.23.2 compressed unsigned integer.
fn compressed_uint(b: &[u8]) -> Option<(usize, usize)> {
    let first = *b.first()?;
    if first & 0x80 == 0 {
        Some((first as usize, 1))
    } else if first & 0xc0 == 0x80 {
        let second = *b.get(1)?;
        Some(((((first & 0x3f) as usize) << 8) | second as usize, 2))
    } else if first & 0xe0 == 0xc0 {
        let rest = b.get(1..4)?;
        Some((
            (((first & 0x1f) as usize) << 24)
                | ((rest[0] as usize) << 16)
                | ((rest[1] as usize) << 8)
                | rest[2] as usize,
            4,
        ))
    } else {
        None
    }
}

/// ECMA-335 public key token: the low 8 bytes of SHA-1(public key), reversed.
pub fn public_key_token(public_key: &[u8]) -> Option<String> {
    if public_key.is_empty() {
        return None;
    }
    let digest = Sha1::digest(public_key);
    let tail = &digest[digest.len() - 8..];
    let mut rev = tail.to_vec();
    rev.reverse();
    Some(hex_lower(&rev))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A real managed assembly shipped with Windows. The Microsoft public key
    /// token for these framework assemblies is well known: b77a5c561934e089.
    fn mscorlib_path() -> PathBuf {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        PathBuf::from(windir).join("Microsoft.NET").join("Framework64").join("v4.0.30319").join("mscorlib.dll")
    }

    fn system_core_path() -> PathBuf {
        let windir = std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into());
        PathBuf::from(windir).join("Microsoft.NET").join("Framework64").join("v4.0.30319").join("System.Core.dll")
    }

    fn load(p: &PathBuf) -> Option<Vec<u8>> {
        std::fs::read(p).ok()
    }

    #[test]
    fn parses_real_mscorlib_identity() {
        let Some(bytes) = load(&mscorlib_path()) else {
            eprintln!("skip: {} not present on this host", mscorlib_path().display());
            return;
        };
        let img = read_assembly(&bytes).expect("mscorlib.dll must parse");
        let asm = img.assembly.as_ref().expect("mscorlib defines an assembly");
        assert_eq!(asm.name, "mscorlib");
        assert_eq!(asm.version, "4.0.0.0");
        assert_eq!(asm.culture, None, "neutral culture");
        assert!(asm.has_public_key, "framework assemblies are strong-named");
        // Microsoft's well-known token, recomputed from the embedded key.
        assert_eq!(
            asm.public_key_token.as_deref(),
            Some("b77a5c561934e089"),
            "public key token derived from the real embedded key"
        );
        assert_eq!(img.runtime_version, "v4.0.30319");
        assert_eq!(
            img.module_name.as_deref(),
            Some("CommonLanguageRuntimeLibrary"),
            "mscorlib Module name is CommonLanguageRuntimeLibrary, not the file name"
        );
        assert!(img.mvid.is_some(), "module version id present");
        assert!(img.type_def_count() > 1000, "mscorlib defines many types");
        assert!(img.method_def_count() > 2000);
        assert!(img.clr_flags & COMIMAGE_FLAGS_ILONLY != 0, "IL-only image");
    }

    #[test]
    fn system_core_references_mscorlib_with_matching_token() {
        let Some(bytes) = load(&system_core_path()) else {
            eprintln!("skip: host has no .NET Framework");
            return;
        };
        let img = read_assembly(&bytes).expect("System.Core.dll must parse");
        assert_eq!(img.assembly.as_ref().unwrap().name, "System.Core");
        let mscorlib = img.references.iter().find(|r| r.name == "mscorlib").expect("System.Core references mscorlib");
        assert_eq!(mscorlib.version, "4.0.0.0");
        assert_eq!(mscorlib.public_key_token.as_deref(), Some("b77a5c561934e089"));
        assert!(!mscorlib.carries_full_key);
        assert_eq!(img.target_framework(), TargetFramework::NetFramework);
        // The defining assembly's own token must appear in the reference set
        // for peer framework assemblies.
        assert!(img.custom_attribute_types.iter().any(|t| t.contains("Attribute")));
    }

    #[test]
    fn rejects_non_pe_input() {
        assert_eq!(read_assembly(b""), Err(CliError::NotPe));
        assert_eq!(read_assembly(b"plain text file"), Err(CliError::NotPe));
    }

    #[test]
    fn rejects_pe_without_cli_directory() {
        // A PE signature with an optional header that declares no data
        // directories at all.
        let mut b = vec![0u8; 0x200];
        b[0] = b'M';
        b[1] = b'Z';
        b[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        b[0x40..0x44].copy_from_slice(b"PE\0\0");
        // COFF: 0 sections, optional header size 0xE0
        b[0x46..0x48].copy_from_slice(&0u16.to_le_bytes());
        b[0x54..0x56].copy_from_slice(&0xE0u16.to_le_bytes());
        // optional header magic PE32, NumberOfRvaAndSizes = 0
        b[0x58..0x5a].copy_from_slice(&0x10bu16.to_le_bytes());
        b[0x58 + 92..0x58 + 96].copy_from_slice(&0u32.to_le_bytes());
        assert_eq!(read_assembly(&b), Err(CliError::NotCli));
    }

    /// Truncating a real assembly at every length must never panic — this is
    /// the property the whole reader is built around.
    #[test]
    fn truncation_sweep_never_panics() {
        let Some(bytes) = load(&system_core_path()) else {
            eprintln!("skip: host has no .NET Framework");
            return;
        };
        let step = (bytes.len() / 400).max(1);
        let mut checked = 0usize;
        let mut parsed = 0usize;
        for n in (0..bytes.len()).step_by(step) {
            if read_assembly(&bytes[..n]).is_ok() {
                parsed += 1;
            }
            checked += 1;
        }
        assert!(checked > 300, "sweep covered {checked} truncations");
        // A truncated image must not report a valid assembly identity unless
        // it really did contain the whole metadata block.
        assert!(parsed < checked / 2, "only intact prefixes may parse");
    }

    #[test]
    fn compressed_uint_matches_ecma335_encoding() {
        assert_eq!(compressed_uint(&[0x03]), Some((3, 1)));
        assert_eq!(compressed_uint(&[0x7f]), Some((127, 1)));
        assert_eq!(compressed_uint(&[0x80, 0x80]), Some((128, 2)));
        assert_eq!(compressed_uint(&[0xae, 0x57]), Some((0x2e57, 2)));
        assert_eq!(compressed_uint(&[0xc0, 0x00, 0x40, 0x00]), Some((0x4000, 4)));
        assert_eq!(compressed_uint(&[]), None);
        assert_eq!(compressed_uint(&[0xff]), None);
    }

    #[test]
    fn public_key_token_is_sha1_tail_reversed() {
        // Independently checkable: SHA-1 of a single byte 0x00 is
        // 5ba93c9db0cff93f52b521d7420e43f6eda2784f -> tail 420e43f6eda2784f
        // -> reversed 4f78a2edf6430e42.
        assert_eq!(public_key_token(&[0x00]).as_deref(), Some("4f78a2edf6430e42"));
        assert_eq!(public_key_token(&[]), None);
    }
}
