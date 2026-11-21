//! Quarantined C-ABI boundary with null-pointer checks and `catch_unwind` panic firewall.
//!
//! Every `extern "C"` entrypoint wraps its execution in [`std::panic::catch_unwind`]
//! and translates null pointers, invalid UTF-8/JSON, and caught panics into explicit
//! negative integer error codes rather than unwinding across the C ABI.

use core::ffi::{c_char, c_void};
use std::ffi::{CStr, CString};

use crate::env::SanitizedChildEnv;

/// Success return code for C-ABI functions.
pub const FFI_OK: i32 = 0;
/// Error code returned when any required pointer or function-pointer argument is null.
pub const FFI_ERR_NULL_POINTER: i32 = -1;
/// Error code returned when an input C string is not valid UTF-8.
pub const FFI_ERR_INVALID_UTF8: i32 = -2;
/// Error code returned when an input JSON payload cannot be parsed or encoded.
pub const FFI_ERR_INVALID_JSON: i32 = -3;
/// Error code returned when sanitized output contains an interior NUL byte.
pub const FFI_ERR_INTERIOR_NUL: i32 = -4;
/// Error code returned when a Rust panic was caught at the C-ABI boundary.
pub const FFI_ERR_PANIC: i32 = -125;

/// Callback signature accepted by [`t3_process_ffi_eval_guarded`].
// SAFETY: Callers uphold pointer and lifetime safety of `user_data` when invoking this callback.
pub type GuardedEvalCallback = unsafe extern "C-unwind" fn(*mut c_void) -> i32;

/// Returns the C-ABI version exported by `t3-process-ffi`.
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
pub extern "C" fn t3_process_ffi_abi_version() -> u32 {
    std::panic::catch_unwind(crate::abi_version).unwrap_or_default()
}

/// Sanitizes a JSON object of environment variables for child spawning (`BUG-09`).
///
/// On [`FFI_OK`], `*out_json` holds a heap-allocated NUL-terminated UTF-8 JSON string that
/// the caller must release via [`t3_process_ffi_free_string`].
///
/// # Safety
/// - `input_json` must be a valid, non-null, NUL-terminated C string.
/// - `out_json` must be a valid, non-null, aligned writable pointer to `*mut c_char`.
// SAFETY: Exported C ABI symbol; pointer preconditions are checked and all panics are caught.
#[unsafe(no_mangle)]
// SAFETY: Raw pointer dereferences are gated on null checks and caller `# Safety` invariants.
pub unsafe extern "C" fn t3_process_ffi_sanitize_env_json(
    input_json: *const c_char,
    is_windows: u8,
    out_json: *mut *mut c_char,
) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if input_json.is_null() || out_json.is_null() {
            return FFI_ERR_NULL_POINTER;
        }
        // SAFETY: `out_json` is verified non-null and writable per caller `# Safety` contract.
        unsafe {
            *out_json = std::ptr::null_mut();
        }
        // SAFETY: `input_json` is non-null and NUL-terminated per caller `# Safety` contract.
        let raw_cstr = unsafe { CStr::from_ptr(input_json) };
        let Ok(raw_json) = raw_cstr.to_str() else {
            return FFI_ERR_INVALID_UTF8;
        };
        let Ok(sanitized) = SanitizedChildEnv::from_json_str(raw_json, is_windows != 0) else {
            return FFI_ERR_INVALID_JSON;
        };
        let Ok(encoded) = sanitized.to_json_string() else {
            return FFI_ERR_INVALID_JSON;
        };
        let Ok(out_cstr) = CString::new(encoded) else {
            return FFI_ERR_INTERIOR_NUL;
        };
        // SAFETY: `out_json` remains valid and writable for the duration of this call.
        unsafe {
            *out_json = out_cstr.into_raw();
        }
        FFI_OK
    })) {
        Ok(code) => code,
        Err(_) => FFI_ERR_PANIC,
    }
}

/// Evaluates an arbitrary FFI callback behind a `catch_unwind` panic firewall.
///
/// # Safety
/// - `callback`, when `Some`, must be valid to invoke with `user_data`.
/// - `out_result` must be a valid, non-null, aligned writable pointer to `i32`.
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Callback invocation and output write uphold caller `# Safety` invariants after null checks.
pub unsafe extern "C" fn t3_process_ffi_eval_guarded(
    callback: Option<GuardedEvalCallback>,
    user_data: *mut c_void,
    out_result: *mut i32,
) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if out_result.is_null() {
            return FFI_ERR_NULL_POINTER;
        }
        // SAFETY: `out_result` is verified non-null and writable per caller `# Safety` contract.
        unsafe {
            *out_result = 0;
        }
        let Some(cb) = callback else {
            return FFI_ERR_NULL_POINTER;
        };
        // SAFETY: `cb` is non-null and valid for `user_data` per caller `# Safety` contract.
        let result = unsafe { cb(user_data) };
        // SAFETY: `out_result` remains valid and writable for the duration of this call.
        unsafe {
            *out_result = result;
        }
        FFI_OK
    })) {
        Ok(code) => code,
        Err(_) => FFI_ERR_PANIC,
    }
}

/// Frees a C string previously allocated by [`t3_process_ffi_sanitize_env_json`].
///
/// # Safety
/// - `ptr` must be a pointer returned by [`t3_process_ffi_sanitize_env_json`] that has not
///   already been freed, or null (which returns [`FFI_ERR_NULL_POINTER`]).
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: `CString::from_raw` is guarded by null check and caller ownership contract.
pub unsafe extern "C" fn t3_process_ffi_free_string(ptr: *mut c_char) -> i32 {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if ptr.is_null() {
            return FFI_ERR_NULL_POINTER;
        }
        // SAFETY: `ptr` was created via `CString::into_raw` in this module and is non-null.
        let owned = unsafe { CString::from_raw(ptr) };
        drop(owned);
        FFI_OK
    })) {
        Ok(code) => code,
        Err(_) => FFI_ERR_PANIC,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::ptr::{null, null_mut};

    // SAFETY: Test callback ignores `user_data` and returns constant `42`.
    unsafe extern "C-unwind" fn return_42(_: *mut c_void) -> i32 {
        42
    }

    // SAFETY: Test callback intentionally unwinds across `C-unwind` to verify `catch_unwind`.
    unsafe extern "C-unwind" fn unwind_panic(_: *mut c_void) -> i32 {
        std::panic::resume_unwind(Box::new("simulated ffi panic"))
    }

    #[test]
    fn abi_version_matches_crate_version() {
        assert_eq!(t3_process_ffi_abi_version(), crate::abi_version());
    }

    #[test]
    fn sanitize_env_json_rejects_null_utf8_and_invalid_json() -> Result<(), String> {
        let mut out: *mut c_char = null_mut();
        // SAFETY: Null `input_json` is explicitly handled and returns an error code.
        let code_null_in = unsafe { t3_process_ffi_sanitize_env_json(null(), 1, &raw mut out) };
        assert_eq!(code_null_in, FFI_ERR_NULL_POINTER);

        let valid_c = CString::new("{}").map_err(|e| e.to_string())?;
        // SAFETY: Null `out_json` is explicitly handled and returns an error code.
        let code_null_out =
            unsafe { t3_process_ffi_sanitize_env_json(valid_c.as_ptr(), 0, null_mut()) };
        assert_eq!(code_null_out, FFI_ERR_NULL_POINTER);

        let bad_utf8: [u8; 2] = [0xFF, 0x00];
        // SAFETY: `bad_utf8` is NUL-terminated and intentionally non-UTF-8; `out` is valid.
        let code_utf8 = unsafe {
            t3_process_ffi_sanitize_env_json(bad_utf8.as_ptr().cast::<c_char>(), 0, &raw mut out)
        };
        assert_eq!(code_utf8, FFI_ERR_INVALID_UTF8);
        assert!(out.is_null());

        let bad_json = CString::new("{not-json").map_err(|e| e.to_string())?;
        // SAFETY: `bad_json` is a valid C string and `out` is a valid stack pointer.
        let code_json =
            unsafe { t3_process_ffi_sanitize_env_json(bad_json.as_ptr(), 0, &raw mut out) };
        assert_eq!(code_json, FFI_ERR_INVALID_JSON);
        assert!(out.is_null());

        // SAFETY: Passing null to free_string returns FFI_ERR_NULL_POINTER cleanly.
        assert_eq!(
            unsafe { t3_process_ffi_free_string(null_mut()) },
            FFI_ERR_NULL_POINTER
        );
        Ok(())
    }

    #[test]
    fn sanitize_env_json_roundtrip_strips_watch_and_inspect_flags() -> Result<(), String> {
        let payload = serde_json::json!({
            "PATH": "/usr/bin",
            "WATCH_REPORT_DEPENDENCIES": "1",
            "NODE_WATCH_STATE": "active",
            "NODE_OPTIONS": "--max-old-space-size=4096 --inspect --inspect-brk=9229 --inspect-port=9230"
        })
        .to_string();
        let input = CString::new(payload).map_err(|e| e.to_string())?;
        let mut out: *mut c_char = null_mut();

        // SAFETY: `input` is a valid C string and `out` points to stack memory.
        let code = unsafe { t3_process_ffi_sanitize_env_json(input.as_ptr(), 0, &raw mut out) };
        assert_eq!(code, FFI_OK);
        assert!(!out.is_null());

        // SAFETY: `out` was populated by `t3_process_ffi_sanitize_env_json` on `FFI_OK`.
        let out_str = unsafe { CStr::from_ptr(out) }
            .to_str()
            .map_err(|e| e.to_string())?
            .to_owned();
        // SAFETY: `out` is freed once using the paired allocator destructor.
        assert_eq!(unsafe { t3_process_ffi_free_string(out) }, FFI_OK);

        let parsed: BTreeMap<String, String> =
            serde_json::from_str(&out_str).map_err(|e| e.to_string())?;
        assert_eq!(parsed.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert!(!parsed.contains_key("WATCH_REPORT_DEPENDENCIES"));
        assert!(!parsed.contains_key("NODE_WATCH_STATE"));
        assert_eq!(
            parsed.get("NODE_OPTIONS").map(String::as_str),
            Some("--max-old-space-size=4096")
        );
        Ok(())
    }

    #[test]
    fn eval_guarded_checks_null_pointers_and_catches_panics() {
        let mut out_val: i32 = -1;

        // SAFETY: Null `out_result` is validated and returns `FFI_ERR_NULL_POINTER`.
        assert_eq!(
            unsafe { t3_process_ffi_eval_guarded(Some(return_42), null_mut(), null_mut()) },
            FFI_ERR_NULL_POINTER
        );

        // SAFETY: Null `callback` (`None`) zeroes `out_val` and returns `FFI_ERR_NULL_POINTER`.
        assert_eq!(
            unsafe { t3_process_ffi_eval_guarded(None, null_mut(), &raw mut out_val) },
            FFI_ERR_NULL_POINTER
        );
        assert_eq!(out_val, 0);

        // SAFETY: `return_42` is valid and returns `42` without panicking.
        assert_eq!(
            unsafe { t3_process_ffi_eval_guarded(Some(return_42), null_mut(), &raw mut out_val) },
            FFI_OK
        );
        assert_eq!(out_val, 42);

        // SAFETY: `unwind_panic` unwinds across `C-unwind` and is caught by `catch_unwind`.
        assert_eq!(
            unsafe {
                t3_process_ffi_eval_guarded(Some(unwind_panic), null_mut(), &raw mut out_val)
            },
            FFI_ERR_PANIC
        );
        assert_eq!(out_val, 0);
    }
}
