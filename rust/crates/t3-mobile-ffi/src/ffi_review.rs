//! Quarantined C-ABI exports for mobile review filtering, linked-line highlights, tunnel guards,
//! and multi-value secret cards behind `std::panic::catch_unwind`.

use std::ffi::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::core_review;
use crate::ffi::{T3_MOBILE_ERR_PANIC, run_json_bridge};

/// Filters changed review diff files and computes visible preload patch indices.
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`crate::t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_filter_review_files_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core_review::filter_review_files_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Normalizes linked-line highlight ranges, computes selection marker bar widths, and checks
/// git review section source selectors.
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`crate::t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_linked_lines_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core_review::linked_lines_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Evaluates preview tunnel disabled guards (settings + managed policy + HTTP refusal headers).
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`crate::t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_tunnel_guard_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core_review::tunnel_guard_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}

/// Evaluates multi-value and single-value secret request cards, field masking toggles, and save
/// or decline answers.
/// # Safety
/// `input_json` is non-null `UTF-8`; `out_json` is writable; free via [`crate::t3_mobile_ffi_free_string`].
// SAFETY: Exported C ABI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
// SAFETY: Pointer preconditions are validated in `run_json_bridge` per `# Safety` contract.
pub unsafe extern "C" fn t3_mobile_ffi_secret_card_json(
    input_json: *const c_char,
    out_json: *mut *mut c_char,
) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: `input_json` and `out_json` obey caller `# Safety` contract.
        unsafe { run_json_bridge(input_json, out_json, core_review::secret_card_json) }
    }))
    .unwrap_or(T3_MOBILE_ERR_PANIC)
}
