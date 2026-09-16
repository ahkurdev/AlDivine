# Windows Build

How this workspace stays buildable on Windows, and what can still fail.

## Environment (validated 2026-09-16)

- Toolchain pinned in `rust-toolchain.toml` (1.98.1,
  x86_64-pc-windows-msvc). Release CI uses `cargo build --locked`.
- Visual Studio 2022 Build Tools + Windows 11 SDK (10.0.22621.0).
- Only the compat-boundary crates invoke C toolchains (bindgen/clang-sys
  via rquickjs); the native server path compiles none.

## What we do to stay reliable

- Native dependencies isolated per `docs/NATIVE_DEPENDENCY_POLICY.md`;
  `ald dependencies audit-native` gates new ones in CI.
- No OpenSSL system dependency; no accidental DLL surface in core.
- `cargo fmt --check`, `cargo clippy`, `cargo test --workspace` green
  before any status is claimed.

## What can still fail (no false guarantees)

Cargo dependency changes, MSVC linker failures, Windows SDK problems,
proc macros, build.rs scripts, feature conflicts, toolchain regressions,
disk/storage failures, CI environment problems. Rust reduces these; it
does not eliminate them.
