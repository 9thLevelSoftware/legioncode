//! Self-contained C-ABI `Android` `JNI` bridge (`Java_codes_t3_mobile_T3MobileBridge_*`).

use std::ffi::{CStr, CString, c_char, c_void};
use std::ptr::null_mut;

use crate::core;

/// `JNI` 32-bit signed integer (`jint`).
#[expect(non_camel_case_types, reason = "Standard JDK jni.h typedef")]
pub type jint = i32;
/// `JNI` 64-bit signed integer (`jlong`).
#[expect(non_camel_case_types, reason = "Standard JDK jni.h typedef")]
pub type jlong = i64;
/// `JNI` size (`jsize`).
pub type JSize = jint;
/// `JNI` signed byte (`jbyte`).
pub type JByte = i8;
/// `JNI` boolean (`jboolean`).
pub type JBoolean = u8;
/// Opaque `JNI` class reference (`jclass`).
pub type JClass = *mut c_void;
/// Opaque `JNI` string reference (`jstring`).
pub type JString = *mut c_void;
/// Opaque `JNI` byte-array reference (`jbyteArray`).
pub type JByteArray = *mut c_void;
/// `JNI` environment pointer (`const struct JNINativeInterface_ *`).
pub type JNIEnv = *const JNINativeInterface_;

/// `JNI` environment pointer pointer passed into exported `extern "system"` functions.
pub type EnvPtr = *mut JNIEnv;

/// Standard `JDK` `JNINativeInterface_` pointer table up through `SetByteArrayRegion`.
#[repr(C)]
pub struct JNINativeInterface_ {
    _head: [*const c_void; 6],
    // SAFETY: JNI slot 6 (`FindClass`).
    pub find_class: Option<unsafe extern "system" fn(EnvPtr, *const c_char) -> JClass>,
    _mid_a: [*const c_void; 8],
    // SAFETY: JNI slot 15 (`ThrowNew`).
    pub throw_new: Option<unsafe extern "system" fn(EnvPtr, JClass, *const c_char) -> jint>,
    _mid_b: [*const c_void; 151],
    // SAFETY: JNI slot 167 (`NewStringUTF`).
    pub new_string_utf: Option<unsafe extern "system" fn(EnvPtr, *const c_char) -> JString>,
    // SAFETY: JNI slot 168 (`GetStringUTFLength`).
    pub get_string_utf_length: Option<unsafe extern "system" fn(EnvPtr, JString) -> JSize>,
    // SAFETY: JNI slot 169 (`GetStringUTFChars`).
    pub get_string_utf_chars:
        Option<unsafe extern "system" fn(EnvPtr, JString, *mut JBoolean) -> *const c_char>,
    // SAFETY: JNI slot 170 (`ReleaseStringUTFChars`).
    pub release_string_utf_chars: Option<unsafe extern "system" fn(EnvPtr, JString, *const c_char)>,
    // SAFETY: JNI slot 171 (`GetArrayLength`).
    pub get_array_length: Option<unsafe extern "system" fn(EnvPtr, JByteArray) -> JSize>,
    _mid_c: [*const c_void; 4],
    // SAFETY: JNI slot 176 (`NewByteArray`).
    pub new_byte_array: Option<unsafe extern "system" fn(EnvPtr, JSize) -> JByteArray>,
    _mid_d: [*const c_void; 23],
    // SAFETY: JNI slot 200 (`GetByteArrayRegion`).
    pub get_byte_array_region:
        Option<unsafe extern "system" fn(EnvPtr, JByteArray, JSize, JSize, *mut JByte)>,
    _mid_e: [*const c_void; 7],
    // SAFETY: JNI slot 208 (`SetByteArrayRegion`).
    pub set_byte_array_region:
        Option<unsafe extern "system" fn(EnvPtr, JByteArray, JSize, JSize, *const JByte)>,
}

impl JNINativeInterface_ {
    /// Returns a zeroed `JNINativeInterface_` table with all function slots cleared.
    pub const fn zeroed() -> Self {
        // SAFETY: All fields are raw pointers or `Option<extern "system" fn>` where zero bits mean `null`/`None`.
        unsafe { std::mem::zeroed() }
    }
}

// SAFETY: Caller guarantees `env` is null or a valid `JNIEnv*`.
unsafe fn env_tbl<'a>(env: EnvPtr) -> Option<&'a JNINativeInterface_> {
    if env.is_null() {
        return None;
    }
    // SAFETY: `env` was checked non-null and points to a `JNIEnv` pointer.
    let v = unsafe { *env };
    if v.is_null() {
        return None;
    }
    // SAFETY: `v` was verified non-null and points to an initialized `JNINativeInterface_`.
    Some(unsafe { &*v })
}

// SAFETY: Caller guarantees `env` is a valid `JNIEnv*` or null.
unsafe fn jni_fail(env: EnvPtr, msg: &str) -> *mut c_void {
    // SAFETY: `env` obeys caller `JNI` contract.
    let t = unsafe { env_tbl(env) };
    if let Some((f, throw)) = t.and_then(|v| v.find_class.zip(v.throw_new))
        // SAFETY: `f` accepts static class descriptor C string.
        && let cls = unsafe { f(env, c"java/lang/IllegalArgumentException".as_ptr()) }
        && !cls.is_null()
    {
        let c = CString::new(msg.replace('\0', " ")).unwrap_or_else(|_| c"bad arg".to_owned());
        // SAFETY: `cls` and `c` are valid JNI/C arguments.
        let _ = unsafe { throw(env, cls, c.as_ptr()) };
    }
    null_mut()
}

// SAFETY: Caller guarantees `env` is valid and `input` is a `JString` or null.
unsafe fn read_jstr(env: EnvPtr, input: JString) -> Result<String, String> {
    if input.is_null() {
        return Err("null JString".into());
    }
    // SAFETY: `env` obeys caller `JNI` contract.
    let t = unsafe { env_tbl(env) }.ok_or("null JNIEnv")?;
    let get = t.get_string_utf_chars.ok_or("no get")?;
    let rel = t.release_string_utf_chars.ok_or("no rel")?;
    // SAFETY: `env` and `input` are non-null `JNI` handles.
    let raw = unsafe { get(env, input, null_mut()) };
    if raw.is_null() {
        return Err("GetStringUTFChars failed".into());
    }
    // SAFETY: `raw` is NUL-terminated `UTF-8` until `rel`.
    let res = unsafe { CStr::from_ptr(raw) }.to_str().map(str::to_owned);
    // SAFETY: `raw` came from `get` on `input` and is released once.
    unsafe { rel(env, input, raw) };
    res.map_err(|_| "invalid UTF-8".into())
}

// SAFETY: Caller guarantees `env` is valid.
unsafe fn write_jstr(env: EnvPtr, text: &str) -> JString {
    // SAFETY: `env` obeys caller `JNI` contract.
    let Some(new_utf) = (unsafe { env_tbl(env) }).and_then(|t| t.new_string_utf) else {
        return null_mut();
    };
    let Ok(c) = CString::new(text) else {
        // SAFETY: `env` is valid for throwing exception.
        return unsafe { jni_fail(env, "interior NUL") };
    };
    // SAFETY: `c` is valid NUL-terminated C string.
    unsafe { new_utf(env, c.as_ptr()) }
}

// SAFETY: Caller guarantees `env` is valid and `array` is a `JByteArray` or null.
unsafe fn read_jbytes(env: EnvPtr, array: JByteArray) -> Result<Vec<u8>, String> {
    if array.is_null() {
        return Err("null JByteArray".into());
    }
    // SAFETY: `env` obeys caller `JNI` contract.
    let t = unsafe { env_tbl(env) }.ok_or("null JNIEnv")?;
    let lf = t.get_array_length.ok_or("no len")?;
    let rf = t.get_byte_array_region.ok_or("no reg")?;
    // SAFETY: `env` and `array` are non-null `JNI` handles.
    let len = unsafe { lf(env, array) };
    let mut buf = vec![0_i8; usize::try_from(len).map_err(|_| "bad len".to_string())?];
    if !buf.is_empty() {
        // SAFETY: `buf` holds `len` bytes within bounds `0..len`.
        unsafe { rf(env, array, 0, len, buf.as_mut_ptr()) };
    }
    Ok(buf.into_iter().map(|b| b.to_ne_bytes()[0]).collect())
}

// SAFETY: Caller guarantees `env` is valid.
unsafe fn write_jbytes(env: EnvPtr, bytes: &[u8]) -> JByteArray {
    // SAFETY: `env` obeys caller `JNI` contract.
    let t = unsafe { env_tbl(env) };
    let Some((na, sr)) = t.and_then(|v| v.new_byte_array.zip(v.set_byte_array_region)) else {
        return null_mut();
    };
    let Ok(len) = JSize::try_from(bytes.len()) else {
        // SAFETY: `env` is valid for throwing exception.
        return unsafe { jni_fail(env, "length exceeds jsize") };
    };
    // SAFETY: `env` is valid and `len >= 0`.
    let arr = unsafe { na(env, len) };
    let s: Vec<JByte> = bytes.iter().map(|&b| JByte::from_ne_bytes([b])).collect();
    if !arr.is_null() && !s.is_empty() {
        // SAFETY: `arr` has length `len` and `s` has `len` bytes.
        unsafe { sr(env, arr, 0, len, s.as_ptr()) };
    }
    arr
}

macro_rules! jni_fns {
    ($($fn_name:ident ($in_ty:ty => $out_ty:ty) = $rd:ident -> $op:ident -> $wr:ident);* $(;)?) => {$(
        /// `JNI` entrypoint forwarding to [`crate::core`].
        /// # Safety
        /// `env` must be valid `JNIEnv*`; handle must be valid or null.
        // SAFETY: Exported JNI symbol guarded by `std::panic::catch_unwind`.
        #[unsafe(no_mangle)]
        // SAFETY: Pointer checks are enforced in helper functions.
        pub unsafe extern "system" fn $fn_name(env: EnvPtr, _c: JClass, x: $in_ty) -> $out_ty {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // SAFETY: `env` and `x` obey caller `# Safety` contract.
                let r = unsafe { $rd(env, x) }.and_then(|v| core::$op(&v).map_err(|e| e.to_string()));
                // SAFETY: `env` is valid for JNI return allocation or throwing `IllegalArgumentException`.
                unsafe { match r { Ok(o) => $wr(env, &o), Err(e) => jni_fail(env, &e) } }
            }))
            .unwrap_or(null_mut())
        }
    )*};
}

/// Returns `t3-mobile-ffi` `ABI` version (`1`).
// SAFETY: Exported JNI symbol guarded by `std::panic::catch_unwind`.
#[unsafe(no_mangle)]
pub extern "system" fn Java_codes_t3_mobile_T3MobileBridge_nativeAbiVersion(
    _: EnvPtr,
    _: JClass,
) -> jint {
    std::panic::catch_unwind(|| jint::try_from(crate::abi_version()).unwrap_or(0)).unwrap_or(-125)
}

/// Shared `JNI` `JString -> Result<String, E> -> JString` helper guarded by `catch_unwind`.
/// # Safety
/// `env` must be valid `JNIEnv*`; `input` must be a valid `JString` or null.
pub(crate) unsafe fn run_jstr_bridge<E: std::fmt::Display>(
    env: EnvPtr,
    input: JString,
    op: fn(&str) -> Result<String, E>,
) -> JString {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // SAFETY: `env` and `input` obey caller `# Safety` contract.
        let r = unsafe { read_jstr(env, input) }.and_then(|v| op(&v).map_err(|e| e.to_string()));
        // SAFETY: `env` is valid for JNI return allocation or throwing `IllegalArgumentException`.
        unsafe {
            match r {
                Ok(o) => write_jstr(env, &o),
                Err(e) => jni_fail(env, &e),
            }
        }
    }))
    .unwrap_or(null_mut())
}

jni_fns! {
    Java_codes_t3_mobile_T3MobileBridge_nativeDecorateDiffJson (JString => JString) = read_jstr -> decorate_diff_json -> write_jstr;
    Java_codes_t3_mobile_T3MobileBridge_nativeFilterClipboardJson (JString => JString) = read_jstr -> filter_clipboard_json -> write_jstr;
    Java_codes_t3_mobile_T3MobileBridge_nativeNavStringsJson (JString => JString) = read_jstr -> nav_strings_json -> write_jstr;
    Java_codes_t3_mobile_T3MobileBridge_nativeLiveActivityJson (JString => JString) = read_jstr -> live_activity_json -> write_jstr;
    Java_codes_t3_mobile_T3MobileBridge_nativePaletteResolveJson (JString => JString) = read_jstr -> palette_resolve_json -> write_jstr;
    Java_codes_t3_mobile_T3MobileBridge_nativeHostSwapChunksJson (JString => JString) = read_jstr -> host_swap_chunks_json -> write_jstr;
    Java_codes_t3_mobile_T3MobileBridge_nativeThemePlanJson (JString => JString) = read_jstr -> theme_plan_json -> write_jstr;
    Java_codes_t3_mobile_T3MobileBridge_nativeVtRenderSnapshot (JString => JByteArray) = read_jstr -> vt_render_snapshot -> write_jbytes;
    Java_codes_t3_mobile_T3MobileBridge_nativeVtDecodeSnapshotJson (JByteArray => JString) = read_jbytes -> vt_decode_snapshot_json -> write_jstr;
}
