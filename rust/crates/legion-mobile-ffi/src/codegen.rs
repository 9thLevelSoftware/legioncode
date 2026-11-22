//! SDK header and native wrapper codegen (`legion_mobile_ffi.h`, Swift, Kotlin/JNA).

use serde::{Deserialize, Serialize};

use crate::codegen_kotlin::KOTLIN_WRAPPER_TEMPLATE;
use crate::codegen_templates::{C_HEADER_TEMPLATE, SWIFT_WRAPPER_TEMPLATE};

/// Ordered list of every C-ABI symbol exported by `legion-mobile-ffi`.
pub const EXPORTED_SYMBOLS: [&str; 17] = [
    "legion_mobile_ffi_abi_version",
    "legion_mobile_ffi_decorate_diff_json",
    "legion_mobile_ffi_filter_clipboard_json",
    "legion_mobile_ffi_nav_strings_json",
    "legion_mobile_ffi_live_activity_json",
    "legion_mobile_ffi_palette_resolve_json",
    "legion_mobile_ffi_vt_render_snapshot",
    "legion_mobile_ffi_vt_decode_snapshot_json",
    "legion_mobile_ffi_host_swap_chunks_json",
    "legion_mobile_ffi_theme_plan_json",
    "legion_mobile_ffi_filter_review_files_json",
    "legion_mobile_ffi_linked_lines_json",
    "legion_mobile_ffi_tunnel_guard_json",
    "legion_mobile_ffi_secret_card_json",
    "legion_mobile_ffi_sdk_sources_json",
    "legion_mobile_ffi_free_string",
    "legion_mobile_ffi_free_bytes",
];

/// Complete C header, iOS Swift wrapper, and Android Kotlin/JNA wrapper bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SdkSources {
    /// C-ABI version constant matching [`crate::abi_version`].
    pub abi_version: u32,
    /// Filename for the C header (`"legion_mobile_ffi.h"`).
    pub c_header_filename: String,
    /// Full C/C++ header file contents (`legion_mobile_ffi.h`).
    pub c_header: String,
    /// Filename for the iOS Swift wrapper (`"LegionMobileCore.swift"`).
    pub swift_filename: String,
    /// Full iOS Swift wrapper source (`LegionMobileCore.swift`).
    pub swift_source: String,
    /// Filename for the Android Kotlin/JNA wrapper (`"LegionMobileCore.kt"`).
    pub kotlin_filename: String,
    /// Full Android Kotlin/JNA wrapper source (`LegionMobileCore.kt`).
    pub kotlin_source: String,
    /// Every exported `legion_mobile_ffi_*` symbol declared across all three generated sources.
    pub exported_symbols: Vec<String>,
}

/// Generates the complete C header, iOS Swift wrapper, and Android Kotlin/JNA wrapper bundle.
///
/// # Examples
///
/// ```
/// let sdk = legion_mobile_ffi::codegen::generate_sdk_sources();
/// assert_eq!(sdk.c_header_filename, "legion_mobile_ffi.h");
/// assert_eq!(sdk.swift_filename, "LegionMobileCore.swift");
/// assert_eq!(sdk.kotlin_filename, "LegionMobileCore.kt");
/// for symbol in &sdk.exported_symbols {
///     assert!(sdk.c_header.contains(symbol));
///     assert!(sdk.swift_source.contains(symbol));
///     assert!(sdk.kotlin_source.contains(symbol));
/// }
/// ```
pub fn generate_sdk_sources() -> SdkSources {
    SdkSources {
        abi_version: crate::abi_version(),
        c_header_filename: "legion_mobile_ffi.h".to_owned(),
        c_header: generate_c_header(),
        swift_filename: "LegionMobileCore.swift".to_owned(),
        swift_source: generate_swift_wrapper(),
        kotlin_filename: "LegionMobileCore.kt".to_owned(),
        kotlin_source: generate_kotlin_wrapper(),
        exported_symbols: EXPORTED_SYMBOLS
            .iter()
            .map(|sym| (*sym).to_owned())
            .collect(),
    }
}

/// Returns the complete C/C++ header (`legion_mobile_ffi.h`) matching all exported `legion_mobile_ffi_*`
/// symbols and status codes.
pub fn generate_c_header() -> String {
    C_HEADER_TEMPLATE.to_owned()
}

/// Returns the complete iOS Swift wrapper (`LegionMobileCore.swift`) matching all exported
/// `legion_mobile_ffi_*` symbols and deallocators.
pub fn generate_swift_wrapper() -> String {
    SWIFT_WRAPPER_TEMPLATE.to_owned()
}

/// Returns the complete Android Kotlin/JNA wrapper (`LegionMobileCore.kt`) matching all exported
/// `legion_mobile_ffi_*` symbols and deallocators.
pub fn generate_kotlin_wrapper() -> String {
    KOTLIN_WRAPPER_TEMPLATE.to_owned()
}
