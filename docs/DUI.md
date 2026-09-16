# DUI

Runtime-texture groundwork (`ald-dui`): registry, message routing, input
forwarding, URL policy. Rendering and browser embedding are an allowed
native boundary that is not built here and not claimed.

## Rules

- Textures: strict names, bounded non-zero dimensions, owner recorded.
  Pixel uploads validated against `w*h*4` RGBA — wrong sizes rejected,
  never partially applied.
- Navigation passes an explicit scheme allowlist (`http(s)`, `aldnui` by
  default); `file://`, `about:`, unknown schemes, and control characters
  refused.
- Mouse input clamps into texture space (reported, never panicked/dropped).
- Page messages bounded and routed to known textures only.

## Status

IMPLEMENTED (7 tests). Renderer, browser embedding, and script-language
DUI natives arrive with their phases (BLOCKED_EXTERNAL).
