//! Quarantined C-ABI boundary for `t3-mobile-ffi` with `catch_unwind` panic firewall.

use std::ffi::{CStr, CString, c_char};
use std::ptr::{null_mut, slice_from_raw_parts_mut};

use crate::core::{self, MobileFfiError};

/// Success status code returned by `t3_mobile_ffi_*` entrypoints.
pub const T3_MOBILE_OK: i32 = 0;
/// Error code returned when any required pointer argument is null.
pub const T3_MOBILE_ERR_NULL_PTR: i32 = -1;
/// Error code returned when an input C string is not valid `UTF-8`.
pub const T3_MOBILE_ERR_INVALID_UTF8: i32 = -2;
/// Error code returned when `JSON` parsing, `T3VT` decoding, or input validation fails.
pub const T3_MOBILE_ERR_INVALID_INPUT: i32 = -3;
/// Error code returned when an output string contains an interior NUL byte.
pub const T3_MOBILE_ERR_INTERIOR_NUL: i32 = -4;
/// Error code returned when a Rust panic is caught at the C-ABI boundary.
pub const T3_MOBILE_ERR_PANIC: i32 = -125;

fn map_core_error(err: &MobileFfiError) -> i32 {
    match err {
        MobileFfiError::InteriorNul => T3_MOBILE_ERR_INTERIOR_NUL,
        MobileFfiError::InvalidJson(_)
        | MobileFfiError::InvalidInput(_)
        | MobileFfiError::Vt(_) => T3_MOBILE_ERR_INVALID_INPUT,
    }
}

// SAFETY: Private FFI helper; caller guarantees `ptr` is non-null and NUL-terminated.
unsafe fn read_c_str<'a>(ptr: *const c_char) -> Result<&'a str, i32> {
    // SAFETY: `ptr` was verified non-null and is NUL-terminated per caller contract.
    unsafe { CStr::from_ptr(ptr) }
        .to_str()
        .map_err(|_| T3_MOBILE_ERR_INVALID_UTF8)
}

// SAFETY: Private FFI helper; caller guarantees `out_json` is non-null, aligned, and writable.
unsafe fn write_json_out(
    out_json: *mut *mut c_char,
    result: Result<String, MobileFfiError>,
) -> i32 {
    let encoded = match result {
        Ok(text) => text,
        Err(err) => return map_core_error(&err),
    };
    let Ok(out_cstr) = CString::new(encoded) else {
        return T3_MOBILE_ERR_INTERIOR_NUL;
    };
    // SAFETY: `out_json` remains valid and writable for this call per caller contract.
    unsafe { *out_json = out_cstr.into_raw() };
    T3_MOBILE_OK
}

// SAFETY: Private FFI helper; caller upholds `# Safety` pointer contracts.
pub(crate) unsafe fn run_json_bridge(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
    op: fn(&str) -> Result<String, MobileFfiError>,
) -> i32 {
    if !out_json.is_null() {
        // SAFETY: `out_json` is non-null and writable per caller contract.
        unsafe { *out_json = null_mut() };
    }
    if input_json.is_null() || out_json.is_null() {
        return T3_MOBILE_ERR_NULL_PTR;
    }
    // SAFETY: `input_json` was verified non-null and obeys caller NUL-termination contract.
    let raw = match unsafe { read_c_str(input_json) } {
        Ok(text) => text,
        Err(code) => return code,
    };
    // SAFETY: `out_json` was verified non-null and writable.
    unsafe { write_json_out(out_json, op(raw)) }
}

// SAFETY: Private FFI helper; caller upholds `# Safety` pointer contract for `out_json`.
unsafe fn run_no_input_json_bridge(
    out_json: *mut *mut c_char,
    op: fn() -> Result<String, MobileFfiError>,
) -> i32 {
    if out_json.is_null() {
        return T3_MOBILE_ERR_NULL_PTR;
    }
    // SAFETY: `out_json` is non-null and writable per caller contract.
    unsafe { *out_json = null_mut() };
    // SAFETY: `out_json` was verified non-null and writable.
    unsafe { write_json_out(out_json, op()) }
}

/// Returns the C-ABI version exported by `t3-mobile-ffi` (`1`).
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
pub extern "C" fn t3_mobile_ffi_abi_version() -> u32 {
    std::panic::catch_unwind(crate::abi_version).unwrap_or_default()
}

/// Decorates review-diff syntax runs across `UTF-16` word-diff highlights (`BUG-22`).
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_decorate_diff_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core::decorate_diff_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Filters composer clipboard context records against copied plain text (`BUG-23`).
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_filter_clipboard_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core::filter_clipboard_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Resolves localized mobile navigation chrome strings for `locale` (`BUG-24`).
/// # Safety
/// `locale` is non-null `UTF-8`; `out_json` is writable; free via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_nav_strings_json(
    locale: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `locale` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(locale, out_json, core::nav_strings_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Renders localized Live Activity and notification metadata (`BUG-24`).
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_live_activity_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core::live_activity_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Resolves sparse 256-color `ANSI` palette overrides and `xterm-256` fallbacks (`BUG-25`).
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_palette_resolve_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core::palette_resolve_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Parses `ANSI`/`VT` streams and renders a length-matched `"T3VT"` snapshot (`Box<[u8]>`).
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_ptr`/`out_len` are writable; free via [`t3_mobile_ffi_free_bytes`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer dereferences occur only after null checks under caller `# Safety` invariants.
pub unsafe extern "C" fn t3_mobile_ffi_vt_render_snapshot(
    input_json: *const c_char,
    out_ptr: *mut *mut u8,
    out_len: *mut usize,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !out_ptr.is_null() {
            // SAFETY: `out_ptr` was checked non-null and is writable per caller contract.
            unsafe { *out_ptr = null_mut() };
        }
        if !out_len.is_null() {
            // SAFETY: `out_len` was checked non-null and is writable per caller contract.
            unsafe { *out_len = 0 };
        }
        if input_json.is_null() || out_ptr.is_null() || out_len.is_null() {
            return T3_MOBILE_ERR_NULL_PTR;
        }
        // SAFETY: `input_json` is non-null and NUL-terminated per caller contract.
        let raw_json = match unsafe { read_c_str(input_json) } {
            Ok(text) => text,
            Err(code) => return code,
        };
        let bytes = match core::vt_render_snapshot(raw_json) {
            Ok(buf) => buf,
            Err(err) => return map_core_error(&err),
        };
        let boxed: Box<[u8]> = bytes.into_boxed_slice();
        let byte_len = boxed.len();
        let raw_ptr = Box::into_raw(boxed).cast::<u8>();
        // SAFETY: `out_ptr` and `out_len` were checked non-null and writable.
        unsafe {
            *out_ptr = raw_ptr;
            *out_len = byte_len;
        }
        T3_MOBILE_OK
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Decodes `"T3VT"` binary snapshot bytes into structured `JSON`.
/// # Safety
/// `bytes` is readable for `len` bytes; `out_json` is writable; free via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Slice construction is guarded by null and `isize::MAX` checks under caller `# Safety`.
pub unsafe extern "C" fn t3_mobile_ffi_vt_decode_snapshot_json(
    bytes: *const u8,
    len: usize,
    out_json: *mut *mut c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !out_json.is_null() {
            // SAFETY: `out_json` was checked non-null and is writable per caller contract.
            unsafe { *out_json = null_mut() };
        }
        if bytes.is_null() || out_json.is_null() {
            return T3_MOBILE_ERR_NULL_PTR;
        }
        if len > isize::MAX as usize {
            return T3_MOBILE_ERR_INVALID_INPUT;
        }
        // SAFETY: `bytes` is non-null, aligned for `u8`, and readable for `len <= isize::MAX` bytes.
        let slice = unsafe { std::slice::from_raw_parts(bytes, len) };
        // SAFETY: `out_json` was verified non-null and writable.
        unsafe { write_json_out(out_json, core::vt_decode_snapshot_json(slice)) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Streams incoming HTTP chunks through header-only `Host:` rewriting (`BUG-26`).
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_host_swap_chunks_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core::host_swap_chunks_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Computes per-pane content-addressed Ghostty theme file write plans (`BUG-26`).
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_theme_plan_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core::theme_plan_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Emits generated `t3_mobile_ffi.h`, `T3MobileCore.swift`, and `T3MobileCore.kt` sources as `JSON`.
/// # Safety
/// `out_json` must be writable; free `*out_json` via [`t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_no_input_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_sdk_sources_json(out_json: *mut *mut c_char) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `out_json` obeys caller `# Safety` contract.
        unsafe { run_no_input_json_bridge(out_json, core::sdk_sources_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Releases a `UTF-8` C string previously allocated by `t3_mobile_ffi_*` entrypoints.
/// # Safety
/// `ptr` must be an unfreed pointer returned via `*out_json`, or null.
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: `CString::from_raw` is gated on a null check and caller ownership contract.
pub unsafe extern "C" fn t3_mobile_ffi_free_string(ptr: *mut c_char) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ptr.is_null() {
            return T3_MOBILE_ERR_NULL_PTR;
        }
        // SAFETY: `ptr` was allocated via `CString::into_raw` in this module and is non-null.
        let owned = unsafe { CString::from_raw(ptr) };
        drop(owned);
        T3_MOBILE_OK
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Releases a length-matched byte buffer allocated by [`t3_mobile_ffi_vt_render_snapshot`].
/// # Safety
/// `(ptr, len)` must match an unfreed allocation from [`t3_mobile_ffi_vt_render_snapshot`], or null.
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: `Box::from_raw` uses `slice_from_raw_parts_mut(ptr, len)` matching `into_boxed_slice`.
pub unsafe extern "C" fn t3_mobile_ffi_free_bytes(ptr: *mut u8, len: usize) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ptr.is_null() {
            return T3_MOBILE_ERR_NULL_PTR;
        }
        let slice_ptr = slice_from_raw_parts_mut(ptr, len);
        // SAFETY: `ptr` and `len` match `Box::into_raw(bytes.into_boxed_slice())` with zero capacity mismatch.
        let owned = unsafe { Box::from_raw(slice_ptr) };
        drop(owned);
        T3_MOBILE_OK
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}
