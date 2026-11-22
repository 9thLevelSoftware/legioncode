//! Integration and C-ABI tests for mobile review filtering, linked-line highlighting,
//! tunnel-disabled checks, and multi-value secret cards.

use std::ffi::{CStr, CString, c_char};
use std::ptr::{null, null_mut};

use legion_mobile_ffi::{
    LEGION_MOBILE_ERR_INVALID_INPUT, LEGION_MOBILE_ERR_NULL_PTR, LEGION_MOBILE_OK,
    legion_mobile_ffi_filter_review_files_json, legion_mobile_ffi_free_string,
    legion_mobile_ffi_linked_lines_json, legion_mobile_ffi_secret_card_json,
    legion_mobile_ffi_tunnel_guard_json,
};
use serde_json::Value;

// SAFETY: Wraps valid C-ABI calls in this test helper and always frees allocated strings.
fn call_ffi(
    func: unsafe extern "C" fn(*const c_char, *mut *mut c_char) -> i32,
    input: &str,
) -> Result<Value, String> {
    let c_in = CString::new(input).map_err(|err| err.to_string())?;
    let mut out_ptr: *mut c_char = null_mut();
    // SAFETY: `c_in` is valid NUL-terminated UTF-8 and `out_ptr` is writable.
    let code = unsafe { func(c_in.as_ptr(), &raw mut out_ptr) };
    if code != LEGION_MOBILE_OK {
        return Err(format!("ffi returned status {code}"));
    }
    if out_ptr.is_null() {
        return Err("ffi returned OK with null out_ptr".to_owned());
    }
    // SAFETY: `out_ptr` is NUL-terminated UTF-8 allocated by `legion-mobile-ffi`.
    let text = unsafe { CStr::from_ptr(out_ptr) }
        .to_str()
        .map_err(|err| err.to_string())?
        .to_owned();
    // SAFETY: `out_ptr` is released once via `legion_mobile_ffi_free_string`.
    let free_code = unsafe { legion_mobile_ffi_free_string(out_ptr) };
    assert_eq!(free_code, LEGION_MOBILE_OK);
    serde_json::from_str(&text).map_err(|err| err.to_string())
}

#[test]
fn c_abi_review_files_and_linked_lines_round_trip() -> Result<(), String> {
    let filtered = call_ffi(
        legion_mobile_ffi_filter_review_files_json,
        r#"{
            "files": [
                { "path": "src/main.ts", "additions": 10, "deletions": 2 },
                { "path": "src/main.test.ts", "additions": 20, "deletions": 1 },
                { "path": "src/__mocks__/api.ts", "additions": 5, "deletions": 0 },
                { "path": "docs/guide.md", "additions": 3, "deletions": 1 }
            ],
            "hideTestFiles": true,
            "includePath": "src/main.test.ts",
            "selectedIndex": 0
        }"#,
    )?;
    assert_eq!(filtered["fileCount"].as_u64(), Some(3));
    assert_eq!(filtered["hiddenTestCount"].as_u64(), Some(1));
    assert_eq!(filtered["additions"].as_u64(), Some(33));
    assert_eq!(
        filtered["shownPatchIndices"].as_array().map(Vec::len),
        Some(2)
    );

    let linked = call_ffi(
        legion_mobile_ffi_linked_lines_json,
        r#"{
            "startLine": 18,
            "endLine": 12,
            "changeBarWidthDp": 2.0,
            "changeBarWidthPx": 4.0,
            "density": 3.0,
            "requestSourceKind": "working-tree",
            "fileSourceKind": "working-tree"
        }"#,
    )?;
    assert_eq!(linked["range"]["startLine"].as_u64(), Some(12));
    assert_eq!(linked["range"]["endLine"].as_u64(), Some(18));
    assert_eq!(linked["lineCount"].as_u64(), Some(7));
    assert_eq!(linked["markerWidthDp"].as_f64(), Some(4.0));
    assert_eq!(linked["markerWidthPx"].as_f64(), Some(12.0));
    assert_eq!(linked["sourceKind"].as_str(), Some("working-tree"));

    // Mismatched source selectors and 0-based line numbers fail cleanly with INVALID_INPUT.
    let mut err_out: *mut c_char = null_mut();
    let mismatch =
        CString::new(r#"{"requestSourceKind":"working-tree","fileSourceKind":"branch-range"}"#)
            .map_err(|err| err.to_string())?;
    // SAFETY: Valid input C string and writable output pointer.
    let code = unsafe { legion_mobile_ffi_linked_lines_json(mismatch.as_ptr(), &raw mut err_out) };
    assert_eq!(code, LEGION_MOBILE_ERR_INVALID_INPUT);
    assert!(err_out.is_null());

    // Null input check.
    // SAFETY: Explicitly testing null-pointer validation guard.
    assert_eq!(
        unsafe { legion_mobile_ffi_filter_review_files_json(null(), &raw mut err_out) },
        LEGION_MOBILE_ERR_NULL_PTR
    );
    Ok(())
}

#[test]
fn c_abi_tunnel_guard_and_secret_card_enforce_policy_and_masking() -> Result<(), String> {
    let policy_blocked = call_ffi(
        legion_mobile_ffi_tunnel_guard_json,
        r#"{
            "enablePreviewTunnel": true,
            "managedPolicy": {
                "organization": "Acme",
                "lockedSettings": ["enablePreviewTunnel"],
                "limits": { "previewTunnel": false }
            },
            "bind": {
                "webviewId": "wv-1",
                "environmentId": "env-1",
                "proxyHost": "127.0.0.1",
                "proxyPort": 1080,
                "standInHost": "lclo.test"
            }
        }"#,
    )?;
    assert_eq!(policy_blocked["allowed"].as_bool(), Some(false));
    assert_eq!(policy_blocked["tunnelDisabled"].as_bool(), Some(true));
    assert_eq!(
        policy_blocked["effectiveState"].as_str(),
        Some("tunnel_disabled")
    );
    assert_eq!(policy_blocked["bindAllowed"].as_bool(), Some(false));

    let header_refused = call_ffi(
        legion_mobile_ffi_tunnel_guard_json,
        r#"{
            "enablePreviewTunnel": true,
            "httpStatus": 502,
            "headers": [["X-Legioncode-Tunnel-Port-Refused", "true"]]
        }"#,
    )?;
    assert_eq!(header_refused["allowed"].as_bool(), Some(false));
    assert_eq!(header_refused["tunnelDisabled"].as_bool(), Some(false));
    assert_eq!(
        header_refused["effectiveState"].as_str(),
        Some("port_refused")
    );

    let card = call_ffi(
        legion_mobile_ffi_secret_card_json,
        r#"{
            "fields": [
                { "name": "username", "label": "Username", "secret": false, "revealable": true },
                { "name": "apiKey", "label": "API Key", "secret": true, "revealable": true },
                { "name": "locked", "label": "Locked", "secret": true, "revealable": false }
            ],
            "deliveryFile": true,
            "values": { "username": "alice", "apiKey": "sec_123", "locked": "k99" },
            "toggleRevealKey": "apiKey"
        }"#,
    )?;
    assert_eq!(card["isReady"].as_bool(), Some(true));
    assert_eq!(card["fieldStates"][0]["isRevealed"].as_bool(), Some(true));
    assert_eq!(card["fieldStates"][1]["isRevealed"].as_bool(), Some(true));
    assert_eq!(card["fieldStates"][2]["isMasked"].as_bool(), Some(true));
    assert_eq!(card["answer"]["type"].as_str(), Some("save"));
    assert_eq!(card["answer"]["values"]["apiKey"].as_str(), Some("sec_123"));
    assert!(
        card["privacyNote"]
            .as_str()
            .unwrap_or_default()
            .contains("24 hours")
    );

    let declined = call_ffi(
        legion_mobile_ffi_secret_card_json,
        r#"{"label":"Token","submitAction":"decline"}"#,
    )?;
    assert_eq!(declined["isReady"].as_bool(), Some(false));
    assert_eq!(declined["answer"]["type"].as_str(), Some("decline"));
    Ok(())
}
