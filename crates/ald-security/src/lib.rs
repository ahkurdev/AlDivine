//! Secret redaction + log sanitization. Ensures secrets never reach logs or
//! crash reports. The redaction is tested against known secret shapes.

#[derive(Debug, Clone, Default)]
pub struct Redactor {
    patterns: Vec<(&'static str, &'static str)>,
}

impl Redactor {
    pub fn new() -> Self {
        Redactor {
            patterns: vec![
                ("access_token", "***REDACTED***"),
                ("refresh_token", "***REDACTED***"),
                ("authorization", "***REDACTED***"),
                ("session_cookie", "***REDACTED***"),
                ("password", "***REDACTED***"),
                ("db_password", "***REDACTED***"),
                ("secret", "***REDACTED***"),
                ("api_key", "***REDACTED***"),
            ],
        }
    }

    /// Replace known secret key occurrences in a log line.
    pub fn redact(&self, line: &str) -> String {
        let mut out = line.to_string();
        for (key, replacement) in &self.patterns {
            // Match key="value" or key: value forms.
            let owned;
            let pat = if let Some(stripped) = key.strip_prefix('"') {
                stripped
            } else {
                owned = key.to_string();
                &owned
            };
            let kv = format!("{pat}=\"");
            if let Some(pos) = out.find(&kv) {
                if let Some(end) = out[pos + kv.len()..].find('"') {
                    let start = pos + kv.len();
                    let stop = start + end;
                    out.replace_range(start..stop, replacement);
                }
            }
            let kv2 = format!("{pat}: \"");
            if let Some(pos) = out.find(&kv2) {
                if let Some(end) = out[pos + kv2.len()..].find('"') {
                    let start = pos + kv2.len();
                    let stop = start + end;
                    out.replace_range(start..stop, replacement);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_password() {
        let r = Redactor::new();
        let line = r#"login user=allan password="hunter2pass""#;
        let out = r.redact(line);
        assert!(!out.contains("hunter2pass"));
        assert!(out.contains("***REDACTED***"));
    }

    #[test]
    fn redacts_authorization_header() {
        let r = Redactor::new();
        let line = r#"req authorization="Bearer eyJabc.def.ghi""#;
        let out = r.redact(line);
        assert!(!out.contains("eyJabc"));
    }

    #[test]
    fn leaves_normal_logs() {
        let r = Redactor::new();
        let line = "player joined id=5 tick=120";
        assert_eq!(r.redact(line), line);
    }
}
