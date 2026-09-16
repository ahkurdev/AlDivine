use ald_cache::sha256_hex;
use ald_protocol::{decode_request, encode_response, TransferHeader, CHUNK_CAP};

pub fn search_dirs(configured: &str) -> [std::path::PathBuf; 3] {
    [
        std::path::PathBuf::from(configured),
        std::path::PathBuf::from("base-resources"),
        std::path::PathBuf::from("../../base-resources"),
    ]
}

fn resolve_file(name: &str, configured: &str) -> Result<std::path::PathBuf, String> {
    let (resource, rel) = name.split_once('/').ok_or_else(|| format!("bad transfer name '{name}'"))?;
    if resource.trim().is_empty() || rel.trim().is_empty() {
        return Err(format!("bad transfer name '{name}'"));
    }
    if rel.split(['/', '\\']).any(|c| c == ".." || c.is_empty()) {
        return Err(format!("traversal refused for '{name}'"));
    }
    for base in search_dirs(configured) {
        let root = base.join(resource);
        let candidate = root.join(rel);
        let canonical = candidate.canonicalize().unwrap_or(candidate.clone());
        let root_canon = root.canonicalize().unwrap_or(root.clone());
        if (candidate == canonical || canonical.starts_with(&root_canon)) && canonical.is_file() {
            return Ok(canonical);
        }
    }
    Err(format!("file '{name}' not served"))
}

/// Serve one chunk. `ready` must reflect the peer's admission state; only
/// Ready sessions may pull bytes.
pub fn serve_chunk(ready: bool, body: &[u8], configured: &str) -> Result<Vec<u8>, String> {
    if !ready {
        return Err("peer not admitted".into());
    }
    let req = decode_request(body).map_err(|e| e.to_string())?;
    let path = resolve_file(&req.name, configured)?;
    let bytes = std::fs::read(&path).map_err(|e| format!("read: {e}"))?;
    let total = bytes.len() as u64;
    if req.offset > total {
        return Err("offset past end".into());
    }
    let end = (req.offset + req.len.min(CHUNK_CAP)).min(total);
    let header = TransferHeader { name: req.name.clone(), offset: req.offset, total, sha256: sha256_hex(&bytes) };
    encode_response(&header, &bytes[req.offset as usize..end as usize]).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ald_protocol::{decode_response, encode_request, TransferRequest};

    fn fixture(tag: &str) -> (std::path::PathBuf, String) {
        let dir = std::env::temp_dir().join(format!("ald-xfer-{}-{tag}", std::process::id()));
        let res = dir.join("demo");
        std::fs::create_dir_all(&res).unwrap();
        std::fs::write(res.join("ald_manifest.toml"), "name = \"demo\"\nversion = \"0.1.0\"\n").unwrap();
        std::fs::write(res.join("data.bin"), b"0123456789abcdef").unwrap();
        (dir, "demo/data.bin".into())
    }

    fn req(name: &str, offset: u64, len: u64) -> Vec<u8> {
        encode_request(&TransferRequest { name: name.into(), offset, len }).unwrap()
    }

    #[test]
    fn full_and_partial_serve() {
        let (dir, name) = fixture("full");
        let cfg = dir.to_str().unwrap();
        let all = serve_chunk(true, &req(&name, 0, 99), cfg).unwrap();
        let (h, d) = decode_response(&all).unwrap();
        assert_eq!(d, b"0123456789abcdef");
        assert_eq!(h.total, 16);
        assert_eq!(h.sha256, sha256_hex(b"0123456789abcdef"));
        let part = serve_chunk(true, &req(&name, 4, 4), cfg).unwrap();
        let (h2, d2) = decode_response(&part).unwrap();
        assert_eq!(d2, b"4567");
        assert_eq!(h2.offset, 4);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unadmitted_refused() {
        let (dir, name) = fixture("unadmitted");
        assert!(serve_chunk(false, &req(&name, 0, 4), dir.to_str().unwrap()).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn traversal_and_missing_refused() {
        let (dir, _) = fixture("traversal");
        let cfg = dir.to_str().unwrap();
        assert!(serve_chunk(true, &req("demo/../evil", 0, 4), cfg).is_err());
        assert!(serve_chunk(true, &req("demo/nope.bin", 0, 4), cfg).is_err());
        assert!(serve_chunk(true, &req("noslash", 0, 4), cfg).is_err());
        assert!(serve_chunk(true, &req("demo/data.bin", 99, 4), cfg).is_err());
        std::fs::remove_dir_all(&dir).ok();
    }
}
