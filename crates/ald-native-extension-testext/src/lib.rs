//! Minimal fixture extension for `ald-native-extension` load tests.
//!
//! Exports the required `ald_extension_init` entry point: returns 0 when the
//! host speaks ABI 1, non-zero (7) otherwise, so the foreign-ABI test has a
//! real refusal to assert on.

/// Host ABI this fixture was built against. Must equal `HOST_ABI` (1).
pub const FIXTURE_ABI: u32 = 1;

/// ABI mismatch sentinel returned by [`ald_extension_init`].
pub const ABI_MISMATCH_CODE: i32 = 7;

/// Required entry point. See `INIT_SYMBOL` in `ald-native-extension`.
#[no_mangle]
pub extern "C" fn ald_extension_init(host_abi: u32) -> i32 {
    if host_abi == FIXTURE_ABI {
        0
    } else {
        ABI_MISMATCH_CODE
    }
}
