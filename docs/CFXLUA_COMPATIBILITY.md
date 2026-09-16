# CfxLua Compatibility Specification

This document specifies CfxLua behavior compatibility implemented in `crates/ald-cfxlua-compat`.

## 1. Vector Family Types

Full support for FiveM-compatible vector primitives:
- `vector2(x, y)`: 2D vector
- `vector3(x, y, z)`: 3D vector
- `vector4(x, y, z, w)`: 4D vector
- `quat(w, x, y, z)` / `quaternion`: Quaternion rotation

### Arithmetic Operations
- Component-wise addition (`+`), subtraction (`-`), multiplication (`*`), division (`/`)
- Scalar multiplication and division
- Vector negation (`-v`)
- Dot product and cross product (`v1:cross(v2)`)
- Length / magnitude calculation

## 2. Compile-Time & Runtime Hashes

- Backtick hash syntax: `` `adder` `` rewrites to Jenkins one-at-a-time (joaat) hash integer.
- Global `joaat(string)` function returning 32-bit unsigned/signed hash integer.

## 3. Global Compatibility Utilities

- `json.encode(val)` & `json.decode(str)`
- `msgpack.pack(val)` & `msgpack.unpack(bin)` with CitizenFX vector extension tags
- `promise.new(fn)` & `Citizen.Await(p)` for synchronous-like asynchronous promise awaiting
- `Citizen.CreateThread(fn)` & `Citizen.Wait(ms)` coroutine scheduling
