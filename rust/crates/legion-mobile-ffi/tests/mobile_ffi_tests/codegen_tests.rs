//! Tests verifying generated C header (`legion_mobile_ffi.h`), iOS Swift (`LegionMobileCore.swift`),
//! and Android Kotlin/JNA (`LegionMobileCore.kt`) wrappers match all C-ABI exports.

use legion_mobile_ffi::codegen::{EXPORTED_SYMBOLS, SdkSources, generate_sdk_sources};
use legion_mobile_ffi::core::sdk_sources_json;

#[test]
fn sdk_sources_contain_every_exported_symbol_and_error_code() -> Result<(), String> {
    let sdk = generate_sdk_sources();
    assert_eq!(sdk.abi_version, 1);
    assert_eq!(sdk.c_header_filename, "legion_mobile_ffi.h");
    assert_eq!(sdk.swift_filename, "LegionMobileCore.swift");
    assert_eq!(sdk.kotlin_filename, "LegionMobileCore.kt");
    assert_eq!(sdk.exported_symbols.len(), EXPORTED_SYMBOLS.len());

    assert!(sdk.c_header.contains("#ifndef LEGION_MOBILE_FFI_H"));
    assert!(sdk.c_header.contains("#define LEGION_MOBILE_OK 0"));
    assert!(
        sdk.c_header
            .contains("#define LEGION_MOBILE_ERR_NULL_PTR (-1)")
    );
    assert!(
        sdk.c_header
            .contains("#define LEGION_MOBILE_ERR_INVALID_UTF8 (-2)")
    );
    assert!(
        sdk.c_header
            .contains("#define LEGION_MOBILE_ERR_INVALID_INPUT (-3)")
    );
    assert!(
        sdk.c_header
            .contains("#define LEGION_MOBILE_ERR_INTERIOR_NUL (-4)")
    );
    assert!(
        sdk.c_header
            .contains("#define LEGION_MOBILE_ERR_PANIC (-125)")
    );

    assert!(sdk.swift_source.contains("public enum LegionMobileCore"));
    assert!(sdk.swift_source.contains("@_silgen_name"));
    assert!(sdk.kotlin_source.contains("object LegionMobileCore"));
    assert!(
        sdk.kotlin_source
            .contains("internal interface LegionMobileFfiLibrary : Library")
    );
    assert!(
        sdk.kotlin_source
            .contains("const val LEGION_MOBILE_OK: Int = 0")
    );
    assert!(
        sdk.kotlin_source
            .contains("const val LEGION_MOBILE_ERR_PANIC: Int = -125")
    );

    for symbol in EXPORTED_SYMBOLS {
        assert!(
            sdk.c_header.contains(symbol),
            "C header missing symbol {symbol}"
        );
        assert!(
            sdk.swift_source.contains(symbol),
            "Swift wrapper missing symbol {symbol}"
        );
        assert!(
            sdk.kotlin_source.contains(symbol),
            "Kotlin wrapper missing symbol {symbol}"
        );
    }

    let raw_json = sdk_sources_json().map_err(|err| err.to_string())?;
    let parsed: SdkSources = serde_json::from_str(&raw_json).map_err(|err| err.to_string())?;
    assert_eq!(parsed, sdk);
    Ok(())
}
