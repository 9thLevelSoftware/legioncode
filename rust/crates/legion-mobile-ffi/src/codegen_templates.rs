//! Generated C header (`legion_mobile_ffi.h`) and iOS Swift (`LegionMobileCore.swift`) wrappers.

pub(crate) const C_HEADER_TEMPLATE: &str = r#"#ifndef LEGION_MOBILE_FFI_H
#define LEGION_MOBILE_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define LEGION_MOBILE_ABI_VERSION 1U
#define LEGION_MOBILE_OK 0
#define LEGION_MOBILE_ERR_NULL_PTR (-1)
#define LEGION_MOBILE_ERR_INVALID_UTF8 (-2)
#define LEGION_MOBILE_ERR_INVALID_INPUT (-3)
#define LEGION_MOBILE_ERR_INTERIOR_NUL (-4)
#define LEGION_MOBILE_ERR_PANIC (-125)

uint32_t legion_mobile_ffi_abi_version(void);
int32_t legion_mobile_ffi_decorate_diff_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_filter_clipboard_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_nav_strings_json(const char *locale, char **out_json);
int32_t legion_mobile_ffi_live_activity_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_palette_resolve_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_vt_render_snapshot(const char *input_json, uint8_t **out_ptr, size_t *out_len);
int32_t legion_mobile_ffi_vt_decode_snapshot_json(const uint8_t *bytes, size_t len, char **out_json);
int32_t legion_mobile_ffi_host_swap_chunks_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_theme_plan_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_filter_review_files_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_linked_lines_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_tunnel_guard_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_secret_card_json(const char *input_json, char **out_json);
int32_t legion_mobile_ffi_sdk_sources_json(char **out_json);
int32_t legion_mobile_ffi_free_string(char *ptr);
int32_t legion_mobile_ffi_free_bytes(uint8_t *ptr, size_t len);

#ifdef __cplusplus
}
#endif

#endif /* LEGION_MOBILE_FFI_H */
"#;

pub(crate) const SWIFT_WRAPPER_TEMPLATE: &str = r#"import Foundation

public enum LegionMobileFfiCode {
    public static let ok: Int32 = 0
    public static let nullPtr: Int32 = -1
    public static let invalidUtf8: Int32 = -2
    public static let invalidInput: Int32 = -3
    public static let interiorNul: Int32 = -4
    public static let panic: Int32 = -125
}

public enum LegionMobileFfiError: Error, Equatable {
    case nullPtr
    case invalidUtf8
    case invalidInput
    case interiorNul
    case panic
    case unknown(Int32)

    fileprivate static func fromCode(_ code: Int32) -> LegionMobileFfiError {
        switch code {
        case LegionMobileFfiCode.nullPtr: return .nullPtr
        case LegionMobileFfiCode.invalidUtf8: return .invalidUtf8
        case LegionMobileFfiCode.invalidInput: return .invalidInput
        case LegionMobileFfiCode.interiorNul: return .interiorNul
        case LegionMobileFfiCode.panic: return .panic
        default: return .unknown(code)
        }
    }
}

@_silgen_name("legion_mobile_ffi_abi_version")
private func c_legion_mobile_ffi_abi_version() -> UInt32

@_silgen_name("legion_mobile_ffi_decorate_diff_json")
private func c_legion_mobile_ffi_decorate_diff_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_filter_clipboard_json")
private func c_legion_mobile_ffi_filter_clipboard_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_nav_strings_json")
private func c_legion_mobile_ffi_nav_strings_json(
    _ locale: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_live_activity_json")
private func c_legion_mobile_ffi_live_activity_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_palette_resolve_json")
private func c_legion_mobile_ffi_palette_resolve_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_vt_render_snapshot")
private func c_legion_mobile_ffi_vt_render_snapshot(
    _ inputJson: UnsafePointer<CChar>?,
    _ outPtr: UnsafeMutablePointer<UnsafeMutablePointer<UInt8>?>?,
    _ outLen: UnsafeMutablePointer<UInt>?
) -> Int32

@_silgen_name("legion_mobile_ffi_vt_decode_snapshot_json")
private func c_legion_mobile_ffi_vt_decode_snapshot_json(
    _ bytes: UnsafePointer<UInt8>?,
    _ len: UInt,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_host_swap_chunks_json")
private func c_legion_mobile_ffi_host_swap_chunks_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_theme_plan_json")
private func c_legion_mobile_ffi_theme_plan_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_filter_review_files_json")
private func c_legion_mobile_ffi_filter_review_files_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_linked_lines_json")
private func c_legion_mobile_ffi_linked_lines_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_tunnel_guard_json")
private func c_legion_mobile_ffi_tunnel_guard_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_secret_card_json")
private func c_legion_mobile_ffi_secret_card_json(
    _ inputJson: UnsafePointer<CChar>?,
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_sdk_sources_json")
private func c_legion_mobile_ffi_sdk_sources_json(
    _ outJson: UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?
) -> Int32

@_silgen_name("legion_mobile_ffi_free_string")
private func c_legion_mobile_ffi_free_string(_ ptr: UnsafeMutablePointer<CChar>?) -> Int32

@_silgen_name("legion_mobile_ffi_free_bytes")
private func c_legion_mobile_ffi_free_bytes(_ ptr: UnsafeMutablePointer<UInt8>?, _ len: UInt) -> Int32

public enum LegionMobileCore {
    public static func abiVersion() -> UInt32 {
        c_legion_mobile_ffi_abi_version()
    }

    public static func decorateDiffJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_decorate_diff_json)
    }

    public static func filterClipboardJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_filter_clipboard_json)
    }

    public static func navStringsJson(_ locale: String) throws -> String {
        try invokeJson(locale, c_legion_mobile_ffi_nav_strings_json)
    }

    public static func liveActivityJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_live_activity_json)
    }

    public static func paletteResolveJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_palette_resolve_json)
    }

    public static func vtRenderSnapshot(_ inputJson: String) throws -> Data {
        var outPtr: UnsafeMutablePointer<UInt8>?
        var outLen: UInt = 0
        let code = inputJson.withCString { cStr in
            c_legion_mobile_ffi_vt_render_snapshot(cStr, &outPtr, &outLen)
        }
        defer {
            if let ptr = outPtr {
                _ = c_legion_mobile_ffi_free_bytes(ptr, outLen)
            }
        }
        guard code == LegionMobileFfiCode.ok, let ptr = outPtr else {
            throw LegionMobileFfiError.fromCode(code)
        }
        return Data(bytes: ptr, count: Int(outLen))
    }

    public static func vtDecodeSnapshotJson(_ bytes: Data) throws -> String {
        var outPtr: UnsafeMutablePointer<CChar>?
        let code: Int32 = bytes.withUnsafeBytes { rawBuf in
            var emptyByte: UInt8 = 0
            let base = rawBuf.bindMemory(to: UInt8.self).baseAddress
            if let ptr = base {
                return c_legion_mobile_ffi_vt_decode_snapshot_json(ptr, UInt(rawBuf.count), &outPtr)
            }
            return withUnsafePointer(to: &emptyByte) { emptyPtr in
                c_legion_mobile_ffi_vt_decode_snapshot_json(emptyPtr, 0, &outPtr)
            }
        }
        defer {
            if let ptr = outPtr {
                _ = c_legion_mobile_ffi_free_string(ptr)
            }
        }
        guard code == LegionMobileFfiCode.ok, let ptr = outPtr else {
            throw LegionMobileFfiError.fromCode(code)
        }
        return String(cString: ptr)
    }

    public static func hostSwapChunksJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_host_swap_chunks_json)
    }

    public static func themePlanJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_theme_plan_json)
    }

    public static func filterReviewFilesJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_filter_review_files_json)
    }

    public static func linkedLinesJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_linked_lines_json)
    }

    public static func tunnelGuardJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_tunnel_guard_json)
    }

    public static func secretCardJson(_ inputJson: String) throws -> String {
        try invokeJson(inputJson, c_legion_mobile_ffi_secret_card_json)
    }

    public static func sdkSourcesJson() throws -> String {
        var outPtr: UnsafeMutablePointer<CChar>?
        let code = c_legion_mobile_ffi_sdk_sources_json(&outPtr)
        defer {
            if let ptr = outPtr {
                _ = c_legion_mobile_ffi_free_string(ptr)
            }
        }
        guard code == LegionMobileFfiCode.ok, let ptr = outPtr else {
            throw LegionMobileFfiError.fromCode(code)
        }
        return String(cString: ptr)
    }

    private static func invokeJson(
        _ input: String,
        _ call: (UnsafePointer<CChar>?, UnsafeMutablePointer<UnsafeMutablePointer<CChar>?>?) -> Int32
    ) throws -> String {
        var outPtr: UnsafeMutablePointer<CChar>?
        let code = input.withCString { cStr in
            call(cStr, &outPtr)
        }
        defer {
            if let ptr = outPtr {
                _ = c_legion_mobile_ffi_free_string(ptr)
            }
        }
        guard code == LegionMobileFfiCode.ok, let ptr = outPtr else {
            throw LegionMobileFfiError.fromCode(code)
        }
        return String(cString: ptr)
    }
}
"#;
