//! DUI groundwork: runtime-texture registry, message routing, input, URLs.
//!
//! Runtime textures (browser pages painted into world-space UI) need a real
//! renderer and game hookup — an allowed native boundary that is NOT built
//! here and is not claimed. What this crate proves is everything around it
//! that is pure registry logic:
//!
//! - [`TextureRegistry`]: create/destroy runtime textures with bounded
//!   dimensions and an owning resource. Pixel uploads are validated against
//!   the texture's own size (`w*h*4` RGBA) — wrong sizes are rejected, never
//!   partially applied. Destroying drops the texture; double-destroy and
//!   unknown ids are errors.
//! - [`DuiMessage`]: typed messages routed to a texture (URL changes, mouse
//!   input, opaque page messages). Unknown textures and oversized payloads
//!   fail at route time.
//! - Input forwarding: mouse positions clamp to the texture's dimensions
//!   (a click outside is still reported, clamped — the page sees where the
//!   user pointed, mapped into its space, never a panic or a drop).
//! - [`DuiUrlPolicy`]: which URL schemes a texture may navigate to.
//!   `file://`, `about:`, and unknown schemes are refused; the allowlist is
//!   explicit per registry (`http(s)` plus Aldivine's own schemes).
//!
//! No rendering, no browser, no game textures. The renderer consumes
//! validated textures and routed messages through the bridge later.

use std::collections::HashMap;

use thiserror::Error;

/// Longest accepted texture name / URL / message payload.
pub const MAX_NAME_LEN: usize = 64;
pub const MAX_URL_LEN: usize = 2048;
pub const MAX_MESSAGE_LEN: usize = 262_144; // 256 KiB
/// Dimension bounds for runtime textures.
pub const MAX_DIMENSION: u32 = 4096;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DuiError {
    #[error("bad texture name '{0}'")]
    BadName(String),
    #[error("bad texture dimensions {0}x{1}")]
    BadDimensions(u32, u32),
    #[error("unknown texture '{0}'")]
    UnknownTexture(String),
    #[error("duplicate texture '{0}'")]
    DuplicateTexture(String),
    #[error("pixel data length {0} does not match {1}x{2} RGBA ({3} expected)")]
    BadPixelLength(usize, u32, u32, usize),
    #[error("message too large ({0} bytes)")]
    MessageTooLarge(usize),
    #[error("URL not allowed '{0}'")]
    UrlNotAllowed(String),
}

/// One runtime texture's metadata. Pixel bytes live with the renderer; the
/// registry tracks shape, ownership, and the current URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeTexture {
    pub id: String,
    pub resource: String,
    pub width: u32,
    pub height: u32,
    pub url: Option<String>,
}

impl RuntimeTexture {
    pub fn pixel_bytes(&self) -> usize {
        self.width as usize * self.height as usize * 4
    }
}

/// URL scheme policy.
#[derive(Debug, Clone)]
pub struct DuiUrlPolicy {
    pub allowed_schemes: Vec<String>,
}

impl Default for DuiUrlPolicy {
    fn default() -> Self {
        DuiUrlPolicy { allowed_schemes: vec!["http".into(), "https".into(), "aldnui".into()] }
    }
}

impl DuiUrlPolicy {
    /// Scheme of `url` if allowed. Refuses missing schemes, unknown schemes,
    /// overlong URLs, and control characters.
    pub fn check(&self, url: &str) -> Result<String, DuiError> {
        if url.is_empty() || url.len() > MAX_URL_LEN || url.chars().any(|c| c.is_control()) {
            return Err(DuiError::UrlNotAllowed(url.to_string()));
        }
        let scheme = url.split("://").next().unwrap_or("").to_ascii_lowercase();
        if scheme.is_empty() || !self.allowed_schemes.contains(&scheme) {
            return Err(DuiError::UrlNotAllowed(url.to_string()));
        }
        Ok(scheme)
    }
}

/// Message routed to a texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DuiMessage {
    UrlChanged { url: String },
    MouseMove { x: i32, y: i32 },
    MouseClick { x: i32, y: i32, button: MouseButton, down: bool },
    PageMessage { payload: Vec<u8> },
}

/// Mouse buttons (canonical names, same vocabulary as ald-input).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Clamped, validated mouse input forwarded into texture space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardedMouse {
    pub x: u32,
    pub y: u32,
    pub button: Option<MouseButton>,
    pub down: bool,
}

/// Runtime-texture registry with message routing.
#[derive(Default)]
pub struct TextureRegistry {
    textures: HashMap<String, RuntimeTexture>,
    url_policy: DuiUrlPolicy,
}

impl TextureRegistry {
    pub fn new() -> Self {
        TextureRegistry::default()
    }

    pub fn with_url_policy(policy: DuiUrlPolicy) -> Self {
        TextureRegistry { textures: HashMap::new(), url_policy: policy }
    }

    fn valid_name(name: &str) -> Result<(), DuiError> {
        if name.is_empty()
            || name.len() > MAX_NAME_LEN
            || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(DuiError::BadName(name.to_string()));
        }
        Ok(())
    }

    /// Create a texture. Dimensions must be non-zero and bounded.
    pub fn create(&mut self, id: &str, resource: &str, width: u32, height: u32) -> Result<(), DuiError> {
        Self::valid_name(id)?;
        Self::valid_name(resource)?;
        if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
            return Err(DuiError::BadDimensions(width, height));
        }
        if self.textures.contains_key(id) {
            return Err(DuiError::DuplicateTexture(id.to_string()));
        }
        self.textures.insert(
            id.to_string(),
            RuntimeTexture { id: id.to_string(), resource: resource.to_string(), width, height, url: None },
        );
        Ok(())
    }

    pub fn destroy(&mut self, id: &str) -> Result<(), DuiError> {
        self.textures.remove(id).map(|_| ()).ok_or_else(|| DuiError::UnknownTexture(id.to_string()))
    }

    pub fn get(&self, id: &str) -> Option<&RuntimeTexture> {
        self.textures.get(id)
    }

    /// Validate a pixel upload against the texture's shape. Returns the
    /// expected byte count; the bytes themselves travel to the renderer.
    pub fn check_pixels(&self, id: &str, pixels: &[u8]) -> Result<usize, DuiError> {
        let tex = self.textures.get(id).ok_or_else(|| DuiError::UnknownTexture(id.to_string()))?;
        let expected = tex.pixel_bytes();
        if pixels.len() != expected {
            return Err(DuiError::BadPixelLength(pixels.len(), tex.width, tex.height, expected));
        }
        Ok(expected)
    }

    /// Navigate a texture. URL must pass policy; the previous URL is
    /// replaced (history lives in the browser, not here).
    pub fn navigate(&mut self, id: &str, url: &str) -> Result<DuiMessage, DuiError> {
        self.url_policy.check(url)?;
        let tex = self.textures.get_mut(id).ok_or_else(|| DuiError::UnknownTexture(id.to_string()))?;
        tex.url = Some(url.to_string());
        Ok(DuiMessage::UrlChanged { url: url.to_string() })
    }

    /// Forward mouse input, clamped into texture space.
    pub fn forward_mouse(
        &self,
        id: &str,
        x: i32,
        y: i32,
        button: Option<MouseButton>,
        down: bool,
    ) -> Result<ForwardedMouse, DuiError> {
        let tex = self.textures.get(id).ok_or_else(|| DuiError::UnknownTexture(id.to_string()))?;
        Ok(ForwardedMouse {
            x: x.clamp(0, tex.width.saturating_sub(1) as i32) as u32,
            y: y.clamp(0, tex.height.saturating_sub(1) as i32) as u32,
            button,
            down,
        })
    }

    /// Route an opaque page message. Bounded; unknown textures fail.
    pub fn route_page_message(&self, id: &str, payload: Vec<u8>) -> Result<DuiMessage, DuiError> {
        if !self.textures.contains_key(id) {
            return Err(DuiError::UnknownTexture(id.to_string()));
        }
        if payload.len() > MAX_MESSAGE_LEN {
            return Err(DuiError::MessageTooLarge(payload.len()));
        }
        Ok(DuiMessage::PageMessage { payload })
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_destroy_lifecycle() {
        let mut r = TextureRegistry::new();
        r.create("scoreboard", "hud", 512, 256).unwrap();
        assert_eq!(r.texture_count(), 1);
        assert_eq!(r.create("scoreboard", "hud", 512, 256), Err(DuiError::DuplicateTexture("scoreboard".into())));
        assert!(r.destroy("scoreboard").is_ok());
        assert_eq!(r.destroy("scoreboard"), Err(DuiError::UnknownTexture("scoreboard".into())));
        assert_eq!(r.texture_count(), 0);
    }

    #[test]
    fn bad_creates_rejected() {
        let mut r = TextureRegistry::new();
        assert_eq!(r.create("", "hud", 64, 64), Err(DuiError::BadName("".into())));
        assert_eq!(r.create("bad name", "hud", 64, 64), Err(DuiError::BadName("bad name".into())));
        assert_eq!(r.create("t", "hud", 0, 64), Err(DuiError::BadDimensions(0, 64)));
        assert_eq!(r.create("t", "hud", 64, 0), Err(DuiError::BadDimensions(64, 0)));
        assert_eq!(r.create("t", "hud", MAX_DIMENSION + 1, 64), Err(DuiError::BadDimensions(MAX_DIMENSION + 1, 64)));
        assert_eq!(r.texture_count(), 0);
    }

    #[test]
    fn pixel_uploads_validated() {
        let mut r = TextureRegistry::new();
        r.create("t", "hud", 4, 2).unwrap(); // 4*2*4 = 32 bytes
        assert_eq!(r.check_pixels("t", &[0; 32]), Ok(32));
        assert_eq!(r.check_pixels("t", &[0; 31]), Err(DuiError::BadPixelLength(31, 4, 2, 32)));
        assert_eq!(r.check_pixels("t", &[0; 33]), Err(DuiError::BadPixelLength(33, 4, 2, 32)));
        assert_eq!(r.check_pixels("ghost", &[]), Err(DuiError::UnknownTexture("ghost".into())));
    }

    #[test]
    fn navigation_policy() {
        let mut r = TextureRegistry::new();
        r.create("browser", "help", 512, 512).unwrap();
        assert_eq!(
            r.navigate("browser", "https://docs.example/help"),
            Ok(DuiMessage::UrlChanged { url: "https://docs.example/help".into() })
        );
        assert_eq!(r.get("browser").unwrap().url.as_deref(), Some("https://docs.example/help"));
        for bad in
            ["file:///etc/passwd", "about:blank", "gopher://x/", "no-scheme-here", "", "HTTPS://UPPER-OK.example"]
        {
            let res = r.navigate("browser", bad);
            if bad == "HTTPS://UPPER-OK.example" {
                assert!(res.is_ok(), "{bad:?} (schemes case-insensitive)");
            } else {
                assert_eq!(res, Err(DuiError::UrlNotAllowed(bad.into())), "{bad:?}");
            }
        }
        assert_eq!(r.navigate("ghost", "https://x.example"), Err(DuiError::UnknownTexture("ghost".into())));
    }

    #[test]
    fn mouse_clamped_into_texture() {
        let mut r = TextureRegistry::new();
        r.create("t", "hud", 100, 50).unwrap();
        assert_eq!(
            r.forward_mouse("t", 10, 20, Some(MouseButton::Left), true).unwrap(),
            ForwardedMouse { x: 10, y: 20, button: Some(MouseButton::Left), down: true }
        );
        // Outside clamps to edges, never panics or drops.
        assert_eq!(
            r.forward_mouse("t", -5, 999, None, false).unwrap(),
            ForwardedMouse { x: 0, y: 49, button: None, down: false }
        );
        assert_eq!(
            r.forward_mouse("t", 100, 50, Some(MouseButton::Right), true).unwrap(),
            ForwardedMouse { x: 99, y: 49, button: Some(MouseButton::Right), down: true }
        );
        assert_eq!(r.forward_mouse("ghost", 0, 0, None, false), Err(DuiError::UnknownTexture("ghost".into())));
    }

    #[test]
    fn page_messages_routed_and_bounded() {
        let mut r = TextureRegistry::new();
        r.create("t", "hud", 64, 64).unwrap();
        assert_eq!(
            r.route_page_message("t", b"hello".to_vec()).unwrap(),
            DuiMessage::PageMessage { payload: b"hello".to_vec() }
        );
        assert_eq!(
            r.route_page_message("t", vec![0; MAX_MESSAGE_LEN + 1]),
            Err(DuiError::MessageTooLarge(MAX_MESSAGE_LEN + 1))
        );
        assert_eq!(r.route_page_message("ghost", vec![]), Err(DuiError::UnknownTexture("ghost".into())));
    }

    #[test]
    fn custom_url_policy() {
        let mut r = TextureRegistry::with_url_policy(DuiUrlPolicy { allowed_schemes: vec!["aldnui".into()] });
        r.create("t", "hud", 64, 64).unwrap();
        assert!(r.navigate("t", "aldnui://help/index.html").is_ok());
        assert_eq!(r.navigate("t", "https://x.example"), Err(DuiError::UrlNotAllowed("https://x.example".into())));
    }
}
