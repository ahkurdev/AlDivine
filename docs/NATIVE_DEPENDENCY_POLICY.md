# Native Dependency Policy

Non-Rust/native dependencies are allowed **only at isolated compatibility
or platform boundaries**. Everything else stays pure Rust. This document
maps every boundary; the executable gate is
`ald dependencies audit-native` (CI: workspace root, offline, exit 1 on
violation).

## Rule

A native compatibility runtime must NOT contaminate native Aldivine mode.
Each boundary dependency lives in exactly one crate, version-pinned in
`Cargo.lock`, and loads only when its runtime is selected.

## Allowed boundaries (verified 2026-09-16 via `cargo tree`)

| Boundary | Crate | Native dep | Scope |
|----------|-------|-----------|-------|
| JS engine (QuickJS bindgen) | ald-script-js | rquickjs → bindgen/clang-sys/cc | client JS runtime only |
| Lua 5.4 (vendored) | ald-script-lua | mlua (vendored, no system Lua) | Lua runtime only |
| Native extension loading | ald-native-extension | libloading (same loader family) | trust-gated extensions only |
| OS API shims | tokio (via ald-server) | windows-sys | OS syscalls, not C compilation |
| Game/GTA integration | client/game-bridge (future) | Win32/GTA interfaces | game path only |
| Browser (NUI/DUI) | future CEF boundary | CEF/Chromium | UI processes only |
| CLR compat | future host | Mono/CoreCLR | .NET resources only |
| Node compat | future host | Node/V8/libuv | Node resources only |

Engine-independent halves (ald-script-node profiles/resolution,
ald-script-dotnet ECMA-335 reader) are pure Rust by design and carry no
native dependency — verified: neither crate's depth-1 tree contains one.

## Core audit result

`ald-server` full non-dev closure (126 crates): **zero** occurrences of
libloading, mlua, rquickjs, bindgen, clang-sys, openssl-sys, cmake, v8,
boa, cef, tauri. The shipped native server links no compatibility
runtime and no C toolchain.

## Adding a boundary

1. Isolate it in its own crate (never core).
2. Pin it in Cargo.lock; record it in the table above.
3. Extend `DENY`/`AUDITED_ROOTS` in `cli/ald/src/audit.rs` if the native
   profile must exclude it.
4. Run `ald dependencies audit-native` green before claiming isolation.

## Script hosts are cargo features, not default deps

`ald-server` orchestrates script runtimes but must not compile C code in its
default closure: `ald-script-lua` and `mlua` are optional dependencies behind
`--features lua`. The lifecycle executes real Lua only when the feature is on;
without it, resource start fails honestly ("rebuild with --features lua").
`cargo tree -p ald-server` (default) stays C-free so `audit-native` keeps
passing unmodified; CI additionally runs the full suite with `--features lua`
plus the loopback vertical e2e. Same pattern applies when Node/CLR hosts land.
