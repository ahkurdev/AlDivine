# NUI

Embedded-UI groundwork (`ald-nui`): origins, messages, callbacks, focus,
CSP. The browser process itself (CEF/Chromium) is an allowed native
boundary that is not vendored — no browser hookup is claimed here.

## Rules

- Origins are `aldnui://<resource>/<path>` (Aldivine's own scheme).
  Strict resource names; paths reject traversal, NUL, backslashes.
- Server→UI messages bounded (1 MiB); queue overflow drops oldest and
  counts it — backpressure visible, never silent.
- Callbacks register explicitly (re-registration refused); invocation is
  owner-checked; each request answers exactly once.
- Focus requires an open UI page; closing clears focus (no stuck input).
- CSP defaults to deny-everything; WASM and origins open explicitly in
  the emitted header.

## Status

IMPLEMENTED (6 tests). Browser embedding, runtime textures (DUI), and
script-language NUI natives arrive with their phases (BLOCKED_EXTERNAL:
no CEF vendored on this dev box).
