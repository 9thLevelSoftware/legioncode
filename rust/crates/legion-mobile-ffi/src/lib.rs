//! Quarantined C-ABI `FFI` crate for `LegionCode` native mobile hosts (`iOS` and `Android`).
//!
//! All core workspace crates remain pure `#![forbid(unsafe_code)]` Rust; this crate isolates the
//! `extern "C"` boundary in [`ffi`] behind null-pointer validation, `UTF-8` checks, and
//! [`std::panic::catch_unwind`] panic firewalls while providing safe Rust bridges ([`core`]) and
//! C/Swift/Kotlin wrapper codegen ([`codegen`]).

#![deny(unsafe_op_in_unsafe_fn)]

pub mod codegen;
mod codegen_kotlin;
mod codegen_templates;
pub mod core;
pub mod core_review;
mod core_review_types;
mod core_tunnel;
mod core_tunnel_types;
mod core_types;
mod core_vt;
mod core_vt_types;
pub mod ffi;
pub mod ffi_review;
pub mod jni_bridge;
pub mod jni_bridge_review;
pub mod mobile_packaging;
mod mobile_packaging_templates;

pub use mobile_packaging as packaging;

pub use ffi::{
    LEGION_MOBILE_ERR_INTERIOR_NUL, LEGION_MOBILE_ERR_INVALID_INPUT,
    LEGION_MOBILE_ERR_INVALID_UTF8, LEGION_MOBILE_ERR_NULL_PTR, LEGION_MOBILE_ERR_PANIC,
    LEGION_MOBILE_OK, legion_mobile_ffi_abi_version, legion_mobile_ffi_decorate_diff_json,
    legion_mobile_ffi_filter_clipboard_json, legion_mobile_ffi_free_bytes,
    legion_mobile_ffi_free_string, legion_mobile_ffi_host_swap_chunks_json,
    legion_mobile_ffi_live_activity_json, legion_mobile_ffi_nav_strings_json,
    legion_mobile_ffi_palette_resolve_json, legion_mobile_ffi_sdk_sources_json,
    legion_mobile_ffi_theme_plan_json, legion_mobile_ffi_vt_decode_snapshot_json,
    legion_mobile_ffi_vt_render_snapshot,
};
pub use ffi_review::{
    legion_mobile_ffi_filter_review_files_json, legion_mobile_ffi_linked_lines_json,
    legion_mobile_ffi_secret_card_json, legion_mobile_ffi_tunnel_guard_json,
};

/// Returns the C-ABI version implemented by `legion-mobile-ffi`.
///
/// # Examples
///
/// ```
/// assert_eq!(legion_mobile_ffi::abi_version(), 1);
/// assert_eq!(legion_mobile_ffi::legion_mobile_ffi_abi_version(), 1);
/// ```
pub const fn abi_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use std::ffi::{CStr, CString, c_char};
    use std::ptr::{null, null_mut};

    use serde_json::Value;

    use super::{
        LEGION_MOBILE_ERR_INVALID_INPUT, LEGION_MOBILE_ERR_INVALID_UTF8,
        LEGION_MOBILE_ERR_NULL_PTR, LEGION_MOBILE_ERR_PANIC, LEGION_MOBILE_OK, abi_version,
        legion_mobile_ffi_abi_version, legion_mobile_ffi_decorate_diff_json,
        legion_mobile_ffi_filter_clipboard_json, legion_mobile_ffi_free_bytes,
        legion_mobile_ffi_free_string, legion_mobile_ffi_host_swap_chunks_json,
        legion_mobile_ffi_live_activity_json, legion_mobile_ffi_nav_strings_json,
        legion_mobile_ffi_palette_resolve_json, legion_mobile_ffi_sdk_sources_json,
        legion_mobile_ffi_theme_plan_json, legion_mobile_ffi_vt_decode_snapshot_json,
        legion_mobile_ffi_vt_render_snapshot,
    };

    // SAFETY: Test helper reads and frees a C string produced by `legion_mobile_ffi_*`.
    unsafe fn take_c_string(ptr: *mut c_char) -> Result<String, String> {
        // SAFETY: `ptr` is non-null and NUL-terminated on `LEGION_MOBILE_OK`.
        let text = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .map_err(|err| err.to_string())?
            .to_owned();
        // SAFETY: `ptr` was allocated by `legion_mobile_ffi_*` and is freed once here.
        let free_code = unsafe { legion_mobile_ffi_free_string(ptr) };
        if free_code != LEGION_MOBILE_OK {
            return Err(format!("free_string failed with {free_code}"));
        }
        Ok(text)
    }

    // SAFETY: Test helper invokes a `(input_json, out_json)` C-ABI function and parses JSON output.
    unsafe fn call_json_ffi(
        func: unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> i32,
        input: &str,
    ) -> Result<Value, String> {
        let c_input = CString::new(input).map_err(|err| err.to_string())?;
        let mut out_ptr: *mut c_char = null_mut();
        // SAFETY: `c_input` is valid NUL-terminated UTF-8 and `out_ptr` is stack-allocated.
        let code = unsafe { func(c_input.as_ptr(), &raw mut out_ptr) };
        if code != LEGION_MOBILE_OK {
            return Err(format!("ffi call returned error code {code}"));
        }
        if out_ptr.is_null() {
            return Err("expected non-null out_json".to_owned());
        }
        // SAFETY: `out_ptr` was populated by `func` on `LEGION_MOBILE_OK`.
        let json_str = unsafe { take_c_string(out_ptr) }?;
        serde_json::from_str(&json_str).map_err(|err| err.to_string())
    }

    #[test]
    fn abi_version_and_panic_firewall_contracts_hold() {
        assert_eq!(legion_mobile_ffi_abi_version(), abi_version());
        assert_eq!(abi_version(), 1);

        let caught = std::panic::catch_unwind(|| -> i32 {
            std::panic::resume_unwind(Box::new("simulated panic"))
        });
        let code = match caught {
            Ok(v) => v,
            Err(_) => LEGION_MOBILE_ERR_PANIC,
        };
        assert_eq!(code, LEGION_MOBILE_ERR_PANIC);
    }

    #[test]
    fn c_abi_rejects_null_pointers_bad_utf8_and_invalid_inputs() -> Result<(), String> {
        let mut out_str: *mut c_char = null_mut();
        let mut out_bytes: *mut u8 = null_mut();
        let mut out_len: usize = 99;
        let valid_c = CString::new("{}").map_err(|err| err.to_string())?;
        let bad_utf8: [u8; 2] = [0xFF, 0x00];

        // SAFETY: Null inputs and null outputs cleanly return `LEGION_MOBILE_ERR_NULL_PTR`.
        unsafe {
            assert_eq!(
                legion_mobile_ffi_decorate_diff_json(null(), &raw mut out_str),
                LEGION_MOBILE_ERR_NULL_PTR
            );
            assert_eq!(
                legion_mobile_ffi_decorate_diff_json(valid_c.as_ptr(), null_mut()),
                LEGION_MOBILE_ERR_NULL_PTR
            );
            assert_eq!(
                legion_mobile_ffi_sdk_sources_json(null_mut()),
                LEGION_MOBILE_ERR_NULL_PTR
            );
            assert_eq!(
                legion_mobile_ffi_vt_render_snapshot(null(), &raw mut out_bytes, &raw mut out_len),
                LEGION_MOBILE_ERR_NULL_PTR
            );
            assert_eq!(out_len, 0);
            assert_eq!(
                legion_mobile_ffi_vt_render_snapshot(
                    valid_c.as_ptr(),
                    null_mut(),
                    &raw mut out_len
                ),
                LEGION_MOBILE_ERR_NULL_PTR
            );
            assert_eq!(
                legion_mobile_ffi_vt_decode_snapshot_json(null(), 0, &raw mut out_str),
                LEGION_MOBILE_ERR_NULL_PTR
            );
            assert_eq!(
                legion_mobile_ffi_free_string(null_mut()),
                LEGION_MOBILE_ERR_NULL_PTR
            );
            assert_eq!(
                legion_mobile_ffi_free_bytes(null_mut(), 0),
                LEGION_MOBILE_ERR_NULL_PTR
            );
        }

        // SAFETY: `bad_utf8` is NUL-terminated non-UTF-8; `out_str` and `out_bytes`/`out_len` are valid.
        unsafe {
            assert_eq!(
                legion_mobile_ffi_nav_strings_json(
                    bad_utf8.as_ptr().cast::<c_char>(),
                    &raw mut out_str
                ),
                LEGION_MOBILE_ERR_INVALID_UTF8
            );
            assert!(out_str.is_null());
            assert_eq!(
                legion_mobile_ffi_vt_render_snapshot(
                    bad_utf8.as_ptr().cast::<c_char>(),
                    &raw mut out_bytes,
                    &raw mut out_len
                ),
                LEGION_MOBILE_ERR_INVALID_UTF8
            );
            assert!(out_bytes.is_null());
            assert_eq!(out_len, 0);
        }

        let bad_json = CString::new("{broken").map_err(|err| err.to_string())?;
        let dummy_lcvt = *b"BAD!";
        // SAFETY: Valid pointers carrying malformed payloads return `LEGION_MOBILE_ERR_INVALID_INPUT`.
        unsafe {
            assert_eq!(
                legion_mobile_ffi_host_swap_chunks_json(bad_json.as_ptr(), &raw mut out_str),
                LEGION_MOBILE_ERR_INVALID_INPUT
            );
            assert_eq!(
                legion_mobile_ffi_vt_decode_snapshot_json(
                    dummy_lcvt.as_ptr(),
                    dummy_lcvt.len(),
                    &raw mut out_str
                ),
                LEGION_MOBILE_ERR_INVALID_INPUT
            );
            assert!(out_str.is_null());
        }
        Ok(())
    }

    #[test]
    fn c_abi_round_trips_all_json_and_lcvt_entrypoints() -> Result<(), String> {
        // SAFETY: Every call supplies valid NUL-terminated JSON/locale strings and stack outputs.
        unsafe {
            let diff = call_json_ffi(
                legion_mobile_ffi_decorate_diff_json,
                r##"{"tokens":[{"content":"fn x()","color":"#ff00aa","fontStyle":3,"attributes":{"role":"kw"}}],"wordHighlights":[{"start":3,"end":4}],"highlightBackground":"#334455"}"##,
            )?;
            assert_eq!(diff["runs"][1]["text"].as_str(), Some("x"));
            assert_eq!(diff["runs"][1]["fontStyle"].as_u64(), Some(3));

            let clip = call_json_ffi(
                legion_mobile_ffi_filter_clipboard_json,
                r#"{"text":"see /c1) and /)","records":[{"contextId":"","label":"blank"},{"contextId":"c1","label":"kept"}]}"#,
            )?;
            assert_eq!(clip["selectedRecords"].as_array().map(Vec::len), Some(1));

            let nav = call_json_ffi(legion_mobile_ffi_nav_strings_json, "de")?;
            assert_eq!(nav["locale"].as_str(), Some("de"));
            assert_eq!(nav["settings"].as_str(), Some("Einstellungen"));

            let act = call_json_ffi(
                legion_mobile_ffi_live_activity_json,
                r#"{"locale":"fr","active":true,"pushData":{"activity_line_0":"waiting_for_approval\tShip\tLegionCode"}}"#,
            )?;
            assert_eq!(act["action"].as_str(), Some("Approuver"));

            let pal = call_json_ffi(
                legion_mobile_ffi_palette_resolve_json,
                r##"{"priorOverrides":[{"index":1,"color":"#ff0000"}],"overrides":[],"indices":[1]}"##,
            )?;
            assert_eq!(pal["overrideCount"].as_u64(), Some(0));
            assert_eq!(pal["resolved"][0]["isOverride"].as_bool(), Some(false));

            let swap = call_json_ffi(
                legion_mobile_ffi_host_swap_chunks_json,
                r#"{"fromHost":"lclo.test","toHost":"127.0.0.1","chunks":["POST / HTTP/1.1\r\nHost: lc","lo.test\r\n\r\n{\"h\":\"Host: lclo.test\"}"]}"#,
            )?;
            assert_eq!(swap["phase"].as_str(), Some("BodyPassthrough"));
            assert!(
                swap["output"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("Host: 127.0.0.1\r\n")
            );

            let theme = call_json_ffi(
                legion_mobile_ffi_theme_plan_json,
                r#"{"paneId":"pane/1","themeConfig":"background = #111111","fontSize":14.0,"appearance":"dark"}"#,
            )?;
            assert_eq!(theme["changed"].as_bool(), Some(true));
        }

        // Render and decode LCVT binary snapshot + free_bytes, and fetch SDK sources.
        let vt_req = CString::new(r#"{"cols":12,"rows":2,"ansiText":"\u001b[31mhi\u001b[0m"}"#)
            .map_err(|err| err.to_string())?;
        let mut vt_ptr: *mut u8 = null_mut();
        let mut vt_len: usize = 0;
        // SAFETY: `vt_req` is valid C string and `vt_ptr`/`vt_len` are stack-allocated outputs.
        let render_code = unsafe {
            legion_mobile_ffi_vt_render_snapshot(vt_req.as_ptr(), &raw mut vt_ptr, &raw mut vt_len)
        };
        assert_eq!(render_code, LEGION_MOBILE_OK);
        assert!(!vt_ptr.is_null());
        assert!(vt_len >= 36);

        let mut decoded_ptr: *mut c_char = null_mut();
        // SAFETY: `vt_ptr` is readable for `vt_len` bytes until freed below.
        let decode_code = unsafe {
            legion_mobile_ffi_vt_decode_snapshot_json(vt_ptr, vt_len, &raw mut decoded_ptr)
        };
        // SAFETY: `vt_ptr` and `vt_len` match the allocation returned by `vt_render_snapshot`.
        assert_eq!(
            unsafe { legion_mobile_ffi_free_bytes(vt_ptr, vt_len) },
            LEGION_MOBILE_OK
        );
        assert_eq!(decode_code, LEGION_MOBILE_OK);
        // SAFETY: `decoded_ptr` was allocated on `LEGION_MOBILE_OK`.
        let decoded_json = unsafe { take_c_string(decoded_ptr) }?;
        let decoded: Value = serde_json::from_str(&decoded_json).map_err(|err| err.to_string())?;
        assert_eq!(decoded["viewportLines"][0].as_str(), Some("hi"));

        let mut sdk_ptr: *mut c_char = null_mut();
        // SAFETY: `sdk_ptr` is a valid stack pointer.
        assert_eq!(
            unsafe { legion_mobile_ffi_sdk_sources_json(&raw mut sdk_ptr) },
            LEGION_MOBILE_OK
        );
        // SAFETY: `sdk_ptr` was allocated on `LEGION_MOBILE_OK`.
        let sdk_json = unsafe { take_c_string(sdk_ptr) }?;
        let sdk: Value = serde_json::from_str(&sdk_json).map_err(|err| err.to_string())?;
        assert_eq!(sdk["abiVersion"].as_u64(), Some(1));
        assert_eq!(sdk["exportedSymbols"].as_array().map(Vec::len), Some(17));
        Ok(())
    }
}
