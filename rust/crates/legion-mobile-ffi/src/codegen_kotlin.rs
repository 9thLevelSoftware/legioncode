//! Generated Android Kotlin/JNA (`LegionMobileCore.kt`) wrapper template.

pub(crate) const KOTLIN_WRAPPER_TEMPLATE: &str = r#"package expo.modules.legionmobile

import com.sun.jna.IntegerType
import com.sun.jna.Library
import com.sun.jna.Native
import com.sun.jna.Pointer
import com.sun.jna.ptr.ByReference
import com.sun.jna.ptr.PointerByReference

class SizeT(value: Long = 0L) : IntegerType(Native.SIZE_T_SIZE, value, true)

class SizeTByReference : ByReference(Native.SIZE_T_SIZE) {
    var value: Long
        get() = if (Native.SIZE_T_SIZE == 4) {
            pointer.getInt(0).toLong() and 0xFFFF_FFFFL
        } else {
            pointer.getLong(0)
        }
        set(next) {
            if (Native.SIZE_T_SIZE == 4) {
                pointer.setInt(0, next.toInt())
            } else {
                pointer.setLong(0, next)
            }
        }
}

class LegionMobileFfiException(val code: Int) : RuntimeException("legion_mobile_ffi failed with code $code")

internal interface LegionMobileFfiLibrary : Library {
    fun legion_mobile_ffi_abi_version(): Int
    fun legion_mobile_ffi_decorate_diff_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_filter_clipboard_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_nav_strings_json(locale: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_live_activity_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_palette_resolve_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_vt_render_snapshot(
        inputJson: String,
        outPtr: PointerByReference,
        outLen: SizeTByReference
    ): Int
    fun legion_mobile_ffi_vt_decode_snapshot_json(
        bytes: ByteArray,
        len: SizeT,
        outJson: PointerByReference
    ): Int
    fun legion_mobile_ffi_host_swap_chunks_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_theme_plan_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_filter_review_files_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_linked_lines_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_tunnel_guard_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_secret_card_json(inputJson: String, outJson: PointerByReference): Int
    fun legion_mobile_ffi_sdk_sources_json(outJson: PointerByReference): Int
    fun legion_mobile_ffi_free_string(ptr: Pointer): Int
    fun legion_mobile_ffi_free_bytes(ptr: Pointer, len: SizeT): Int
}

object LegionMobileCore {
    const val LEGION_MOBILE_OK: Int = 0
    const val LEGION_MOBILE_ERR_NULL_PTR: Int = -1
    const val LEGION_MOBILE_ERR_INVALID_UTF8: Int = -2
    const val LEGION_MOBILE_ERR_INVALID_INPUT: Int = -3
    const val LEGION_MOBILE_ERR_INTERIOR_NUL: Int = -4
    const val LEGION_MOBILE_ERR_PANIC: Int = -125

    private val lib: LegionMobileFfiLibrary by lazy {
        Native.load("legion_mobile_ffi", LegionMobileFfiLibrary::class.java)
    }

    fun abiVersion(): Int = lib.legion_mobile_ffi_abi_version()

    fun decorateDiffJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_decorate_diff_json(inputJson, out) }

    fun filterClipboardJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_filter_clipboard_json(inputJson, out) }

    fun navStringsJson(locale: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_nav_strings_json(locale, out) }

    fun liveActivityJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_live_activity_json(inputJson, out) }

    fun paletteResolveJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_palette_resolve_json(inputJson, out) }

    fun vtRenderSnapshot(inputJson: String): ByteArray {
        val outPtr = PointerByReference()
        val outLen = SizeTByReference()
        val code = lib.legion_mobile_ffi_vt_render_snapshot(inputJson, outPtr, outLen)
        val ptr = outPtr.value
        val len = outLen.value
        try {
            if (code != LEGION_MOBILE_OK || ptr == null) {
                throw LegionMobileFfiException(code)
            }
            return ptr.getByteArray(0, len.toInt())
        } finally {
            if (ptr != null) {
                lib.legion_mobile_ffi_free_bytes(ptr, SizeT(len))
            }
        }
    }

    fun vtDecodeSnapshotJson(bytes: ByteArray): String =
        invokeJson { out ->
            lib.legion_mobile_ffi_vt_decode_snapshot_json(bytes, SizeT(bytes.size.toLong()), out)
        }

    fun hostSwapChunksJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_host_swap_chunks_json(inputJson, out) }

    fun themePlanJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_theme_plan_json(inputJson, out) }

    fun filterReviewFilesJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_filter_review_files_json(inputJson, out) }

    fun linkedLinesJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_linked_lines_json(inputJson, out) }

    fun tunnelGuardJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_tunnel_guard_json(inputJson, out) }

    fun secretCardJson(inputJson: String): String =
        invokeJson { out -> lib.legion_mobile_ffi_secret_card_json(inputJson, out) }

    fun sdkSourcesJson(): String =
        invokeJson { out -> lib.legion_mobile_ffi_sdk_sources_json(out) }

    private fun invokeJson(call: (PointerByReference) -> Int): String {
        val outRef = PointerByReference()
        val code = call(outRef)
        val ptr = outRef.value
        try {
            if (code != LEGION_MOBILE_OK || ptr == null) {
                throw LegionMobileFfiException(code)
            }
            return ptr.getString(0, "UTF-8")
        } finally {
            if (ptr != null) {
                lib.legion_mobile_ffi_free_string(ptr)
            }
        }
    }
}
"#;
