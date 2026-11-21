//! SDK header and native wrapper codegen (`t3_mobile_ffi.h`, Swift, Kotlin/JNA).

use serde::{Deserialize, Serialize};

use crate::codegen_kotlin::KOTLIN_WRAPPER_TEMPLATE;
use crate::codegen_templates::{C_HEADER_TEMPLATE, SWIFT_WRAPPER_TEMPLATE};

/// Ordered list of every C-ABI symbol exported by `t3-mobile-ffi`.
pub const EXPORTED_SYMBOLS: [&str; 13] = [
    "t3_mobile_ffi_abi_version",
    "t3_mobile_ffi_decorate_diff_json",
    "t3_mobile_ffi_filter_clipboard_json",
    "t3_mobile_ffi_nav_strings_json",
    "t3_mobile_ffi_live_activity_json",
    "t3_mobile_ffi_palette_resolve_json",
    "t3_mobile_ffi_vt_render_snapshot",
    "t3_mobile_ffi_vt_decode_snapshot_json",
    "t3_mobile_ffi_host_swap_chunks_json",
    "t3_mobile_ffi_theme_plan_json",
    "t3_mobile_ffi_sdk_sources_json",
    "t3_mobile_ffi_free_string",
    "t3_mobile_ffi_free_bytes",
];

/// Complete C header, iOS Swift wrapper, and Android Kotlin/JNA wrapper bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkSources {
    /// C-ABI version constant matching [`crate::abi_version`].
    pub abi_version: u32,
    /// Filename for the C header (`"t3_mobile_ffi.h"`).
    pub c_header_filename: String,
    /// Full C/C++ header file contents (`t3_mobile_ffi.h`).
    pub c_header: String,
    /// Filename for the iOS Swift wrapper (`"T3MobileCore.swift"`).
    pub swift_filename: String,
    /// Full iOS Swift wrapper source (`T3MobileCore.swift`).
    pub swift_source: String,
    /// Filename for the Android Kotlin/JNA wrapper (`"T3MobileCore.kt"`).
    pub kotlin_filename: String,
    /// Full Android Kotlin/JNA wrapper source (`T3MobileCore.kt`).
    pub kotlin_source: String,
    /// Every exported `t3_mobile_ffi_*` symbol declared across all three generated sources.
    pub exported_symbols: Vec<String>,
}

/// Generates the complete C header, iOS Swift wrapper, and Android Kotlin/JNA wrapper bundle.
///
/// # Examples
///
/// ```
/// let sdk = t3_mobile_ffi::codegen::generate_sdk_sources();
/// assert_eq!(sdk.c_header_filename, "t3_mobile_ffi.h");
/// assert_eq!(sdk.swift_filename, "T3MobileCore.swift");
/// assert_eq!(sdk.kotlin_filename, "T3MobileCore.kt");
/// for symbol in &sdk.exported_symbols {
///     assert!(sdk.c_header.contains(symbol));
///     assert!(sdk.swift_source.contains(symbol));
///     assert!(sdk.kotlin_source.contains(symbol));
/// }
/// ```
pub fn generate_sdk_sources() -> SdkSources {
    SdkSources {
        abi_version: crate::abi_version(),
        c_header_filename: "t3_mobile_ffi.h".to_owned(),
        c_header: generate_c_header(),
        swift_filename: "T3MobileCore.swift".to_owned(),
        swift_source: generate_swift_wrapper(),
        kotlin_filename: "T3MobileCore.kt".to_owned(),
        kotlin_source: generate_kotlin_wrapper(),
        exported_symbols: EXPORTED_SYMBOLS
            .iter()
            .map(|sym| (*sym).to_owned())
            .collect(),
    }
}

/// Returns the complete C/C++ header (`t3_mobile_ffi.h`) matching all exported `t3_mobile_ffi_*`
/// symbols and status codes.
pub fn generate_c_header() -> String {
    C_HEADER_TEMPLATE.to_owned()
}

/// Returns the complete iOS Swift wrapper (`T3MobileCore.swift`) matching all exported
/// `t3_mobile_ffi_*` symbols and deallocators.
pub fn generate_swift_wrapper() -> String {
    SWIFT_WRAPPER_TEMPLATE.to_owned()
}

/// Returns the complete Android Kotlin/JNA wrapper (`T3MobileCore.kt`) matching all exported
/// `t3_mobile_ffi_*` symbols and deallocators.
pub fn generate_kotlin_wrapper() -> String {
    KOTLIN_WRAPPER_TEMPLATE.to_owned()
}
