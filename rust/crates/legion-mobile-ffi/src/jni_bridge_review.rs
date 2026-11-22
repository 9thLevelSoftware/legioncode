//! Android `JNI` exports for mobile review filtering, linked-line highlights, tunnel guards, and secret cards.

use crate::core_review;
use crate::jni_bridge::{EnvPtr, JClass, JString, run_jstr_bridge};

macro_rules! jni_review_fns {
    ($($fn_name:ident => $op:ident);* $(;)?) => {$(
        /// `JNI` review/tunnel/secret entrypoint forwarding to [`crate::core_review`].
        /// # Safety
        /// `env` must be a valid `JNIEnv*`; `input` must be a valid `JString` or null.
        // SAFETY: Exported JNI symbol guarded inside `run_jstr_bridge` with `std::panic::catch_unwind`.
        #[unsafe(no_mangle)]
        // SAFETY: Pointer validation and UTF-8 decoding are enforced by `run_jstr_bridge`.
        pub unsafe extern "system" fn $fn_name(env: EnvPtr, _cls: JClass, input: JString) -> JString {
            // SAFETY: `env` and `input` uphold caller `# Safety` contract.
            unsafe { run_jstr_bridge(env, input, core_review::$op) }
        }
    )*};
}

jni_review_fns! {
    Java_dev_legioncode_mobile_LegionMobileBridge_nativeFilterReviewFilesJson => filter_review_files_json;
    Java_dev_legioncode_mobile_LegionMobileBridge_nativeLinkedLinesJson => linked_lines_json;
    Java_dev_legioncode_mobile_LegionMobileBridge_nativeTunnelGuardJson => tunnel_guard_json;
    Java_dev_legioncode_mobile_LegionMobileBridge_nativeSecretCardJson => secret_card_json;
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::jni_bridge::{
        self as j, EnvPtr as E, JBoolean as B, JByte as I, JByteArray as A, JClass as C, JNIEnv,
        JNINativeInterface_ as V, JSize as S, JString as T,
        Java_dev_legioncode_mobile_LegionMobileBridge_nativeAbiVersion as abi,
        Java_dev_legioncode_mobile_LegionMobileBridge_nativeVtDecodeSnapshotJson as vd,
        Java_dev_legioncode_mobile_LegionMobileBridge_nativeVtRenderSnapshot as vr, jint,
    };
    use std::ffi::{CStr, c_char, c_void};
    use std::mem::{offset_of, size_of};
    use std::ptr::{copy_nonoverlapping as cp, null_mut, without_provenance_mut as wpm};
    use std::slice::from_raw_parts as rs;

    #[derive(Default)]
    pub(crate) struct St {
        pub s: Vec<Vec<u8>>,
        pub a: Vec<Vec<u8>>,
        pub thrown: Vec<String>,
        pub acq: usize,
        pub rel: usize,
    }

    #[repr(C)]
    pub(crate) struct MockEnv(pub JNIEnv, pub St);

    impl St {
        pub(crate) fn mk_s(&mut self, b: &[u8]) -> T {
            self.s.push([b, &[0]].concat());
            wpm(self.s.len())
        }

        pub(crate) fn mk_b(&mut self, b: &[u8]) -> A {
            self.a.push(b.to_vec());
            wpm(self.a.len())
        }

        pub(crate) fn get_s(&self, s: T) -> Result<String, String> {
            let i = s.addr().checked_sub(1).ok_or("null")?;
            String::from_utf8(self.s[i][..self.s[i].len() - 1].to_vec()).map_err(|e| e.to_string())
        }

        fn len_a(&self, a: A) -> S {
            S::try_from(self.a.get(a.addr().saturating_sub(1)).map_or(0, Vec::len)).unwrap_or(0)
        }

        fn span(&mut self, a: A, st: S, n: S) -> Option<&mut [u8]> {
            let (s, l) = (usize::try_from(st).ok()?, usize::try_from(n).ok()?);
            self.a.get_mut(a.addr().checked_sub(1)?)?.get_mut(s..s + l)
        }
    }

    // SAFETY: `e` points to live `MockEnv`.
    unsafe fn st<'a>(e: E) -> &'a mut St {
        unsafe { &mut (*e.cast::<MockEnv>()).1 }
    }

    // SAFETY: Returns sentinel class handle.
    unsafe extern "system" fn m_find(_: E, _: *const c_char) -> C {
        wpm(1)
    }

    // SAFETY: `e` is `MockEnv` and `m` is NUL-terminated.
    unsafe extern "system" fn m_throw(e: E, _: C, m: *const c_char) -> jint {
        // SAFETY: `m` is valid C string.
        let txt = unsafe { CStr::from_ptr(m) }.to_string_lossy().into_owned();
        // SAFETY: `e` is valid `MockEnv`.
        unsafe { st(e) }.thrown.push(txt);
        0
    }

    // SAFETY: `e` is `MockEnv` and `p` is NUL-terminated.
    unsafe extern "system" fn m_new_s(e: E, p: *const c_char) -> T {
        // SAFETY: `e` is `MockEnv` and `p` is valid C string.
        unsafe { st(e).mk_s(CStr::from_ptr(p).to_bytes()) }
    }

    // SAFETY: `e` is `MockEnv`.
    unsafe extern "system" fn m_get_s(e: E, s: T, _: *mut B) -> *const c_char {
        // SAFETY: `e` is `MockEnv`.
        let m = unsafe { st(e) };
        m.acq += 1;
        m.s.get(s.addr().saturating_sub(1))
            .map_or(std::ptr::null(), |b| b.as_ptr().cast())
    }

    // SAFETY: `e` is `MockEnv`.
    unsafe extern "system" fn m_rel_s(e: E, _: T, _: *const c_char) {
        // SAFETY: `e` is `MockEnv`.
        unsafe { st(e) }.rel += 1;
    }

    // SAFETY: `e` is `MockEnv`.
    unsafe extern "system" fn m_len(e: E, a: A) -> S {
        // SAFETY: `e` is `MockEnv`.
        unsafe { st(e) }.len_a(a)
    }

    // SAFETY: `e` is `MockEnv`.
    unsafe extern "system" fn m_new_a(e: E, n: S) -> A {
        // SAFETY: `e` is `MockEnv`.
        unsafe { st(e) }.mk_b(&vec![0; usize::try_from(n).unwrap_or(0)])
    }

    // SAFETY: `env` is `MockEnv` and `buf` is writable for `len` bytes.
    unsafe extern "system" fn m_get_r(env: E, arr: A, off: S, len: S, buf: *mut I) {
        // SAFETY: `env` is `MockEnv`.
        if let Some(v) = unsafe { st(env) }.span(arr, off, len) {
            // SAFETY: `v` and `buf` are non-overlapping for `v.len()` bytes.
            unsafe { cp(v.as_ptr(), buf.cast(), v.len()) };
        }
    }

    // SAFETY: `env` is `MockEnv` and `buf` is readable for `len` bytes.
    unsafe extern "system" fn m_set_r(env: E, arr: A, off: S, len: S, buf: *const I) {
        // SAFETY: `env` is `MockEnv`.
        if let Some(d) = unsafe { st(env) }.span(arr, off, len) {
            // SAFETY: `buf` is readable for `d.len()` bytes.
            d.copy_from_slice(unsafe { rs(buf.cast(), d.len()) });
        }
    }

    pub(crate) fn mock_vtable() -> V {
        let mut tbl = V::zeroed();
        tbl.find_class = Some(m_find);
        tbl.throw_new = Some(m_throw);
        tbl.new_string_utf = Some(m_new_s);
        tbl.get_string_utf_chars = Some(m_get_s);
        tbl.release_string_utf_chars = Some(m_rel_s);
        tbl.get_array_length = Some(m_len);
        tbl.new_byte_array = Some(m_new_a);
        tbl.get_byte_array_region = Some(m_get_r);
        tbl.set_byte_array_region = Some(m_set_r);
        tbl
    }

    // SAFETY: Function pointer alias for mock JSON entrypoints.
    pub(crate) type JniFn = unsafe extern "system" fn(E, C, T) -> T;

    #[test]
    fn jni_vtable_offsets_and_all_entrypoints_round_trip() -> Result<(), String> {
        let p = size_of::<*const c_void>();
        assert_eq!(offset_of!(V, find_class), 6 * p);
        assert_eq!(offset_of!(V, throw_new), 15 * p);
        assert_eq!(offset_of!(V, new_string_utf), 167 * p);
        assert_eq!(offset_of!(V, get_string_utf_chars), 169 * p);
        assert_eq!(offset_of!(V, release_string_utf_chars), 170 * p);
        assert_eq!(offset_of!(V, get_array_length), 171 * p);
        assert_eq!(offset_of!(V, new_byte_array), 176 * p);
        assert_eq!(offset_of!(V, get_byte_array_region), 200 * p);
        assert_eq!(offset_of!(V, set_byte_array_region), 208 * p);
        let tbl = mock_vtable();
        let mut bx = Box::new(MockEnv(&raw const tbl, St::default()));
        let ep: E = (&raw mut *bx).cast::<JNIEnv>();
        let n = null_mut();
        assert_eq!(abi(ep, n), 1);
        let vt_in =
            bx.1.mk_s(br#"{"cols":8,"rows":2,"ansiText":"\u001b[32mhi\u001b[0m"}"#);
        let cases: [(JniFn, &[u8], &str); 7] = [
            (j::Java_dev_legioncode_mobile_LegionMobileBridge_nativeDecorateDiffJson, br#"{"tokens":[{"content":"fn x","fontStyle":2}],"wordHighlights":[{"start":3,"end":4}]}"#, "\"wordHighlighted\":true"),
            (j::Java_dev_legioncode_mobile_LegionMobileBridge_nativeFilterClipboardJson, br#"{"text":"use /c1)","records":[{"contextId":"","label":"no"},{"contextId":"c1","label":"ok"}]}"#, "\"c1\""),
            (j::Java_dev_legioncode_mobile_LegionMobileBridge_nativeNavStringsJson, b"de", "Einstellungen"),
            (j::Java_dev_legioncode_mobile_LegionMobileBridge_nativeLiveActivityJson, br#"{"locale":"fr","active":true,"pushData":{"activity_line_0":"waiting_for_approval\tGo\tLC"}}"#, "Approuver"),
            (j::Java_dev_legioncode_mobile_LegionMobileBridge_nativePaletteResolveJson, br##"{"priorOverrides":[{"index":1,"color":"#ff0000"}],"overrides":[],"indices":[1]}"##, "\"overrideCount\":0"),
            (j::Java_dev_legioncode_mobile_LegionMobileBridge_nativeHostSwapChunksJson, br#"{"fromHost":"lclo.test","toHost":"127.0.0.1","chunks":["GET / HTTP/1.1\r\nHost: lclo.test\r\n\r\nHost: lclo.test"]}"#, "Host: 127.0.0.1"),
            (j::Java_dev_legioncode_mobile_LegionMobileBridge_nativeThemePlanJson, br#"{"paneId":"pane/1","themeConfig":"background = #111111"}"#, "\"changed\":true"),
        ];
        let (df, nv, th) = (cases[0].0, cases[2].0, cases[6].0);
        let (bad_j, bad_b, bad_u) = (bx.1.mk_s(b"{bad"), bx.1.mk_b(b"BAD!"), bx.1.mk_s(&[0xFF]));
        // SAFETY: `ep` points to live `MockEnv` with full vtable for the test lifetime.
        unsafe {
            assert!(df(null_mut(), n, n).is_null());
            assert!(nv(ep, n, n).is_null());
            assert!(vr(ep, n, bad_j).is_null());
            assert!(vd(ep, n, bad_b).is_null());
            assert!(vd(ep, n, n).is_null());
            assert!(th(ep, n, bad_u).is_null());
            for (f, inp, exp) in cases {
                let h = bx.1.mk_s(inp);
                assert!(bx.1.get_s(f(ep, n, h))?.contains(exp));
            }
            let dec = bx.1.get_s(vd(ep, n, vr(ep, n, vt_in)))?;
            assert!(dec.contains("\"viewportLines\":[\"hi\",\"\"]"));
        }
        assert_eq!((bx.1.acq, bx.1.thrown.len()), (bx.1.rel, 5));
        Ok(())
    }

    #[test]
    fn jni_review_tunnel_and_secret_entrypoints_round_trip() -> Result<(), String> {
        let tbl = mock_vtable();
        let mut bx = Box::new(MockEnv(&raw const tbl, St::default()));
        let ep: E = (&raw mut *bx).cast::<JNIEnv>();
        let n = null_mut();
        let cases: [(JniFn, &[u8], &str); 4] = [
            (
                super::Java_dev_legioncode_mobile_LegionMobileBridge_nativeFilterReviewFilesJson,
                br#"{"files":[{"path":"src/lib.rs","additions":4,"deletions":1},{"path":"src/lib.test.ts","additions":2,"deletions":0}],"hideTestFiles":true}"#,
                "\"hiddenTestCount\":1",
            ),
            (
                super::Java_dev_legioncode_mobile_LegionMobileBridge_nativeLinkedLinesJson,
                br#"{"startLine":14,"endLine":9,"changeBarWidthDp":1.5,"density":3.0}"#,
                "\"lineCount\":6",
            ),
            (
                super::Java_dev_legioncode_mobile_LegionMobileBridge_nativeTunnelGuardJson,
                br#"{"enablePreviewTunnel":false}"#,
                "\"tunnelDisabled\":true",
            ),
            (
                super::Java_dev_legioncode_mobile_LegionMobileBridge_nativeSecretCardJson,
                br#"{"fields":[{"name":"user","label":"Email","secret":false},{"name":"token","label":"Token","revealable":true}],"values":{"user":"a@b.c","token":"shh"},"toggleRevealKey":"token"}"#,
                "\"isReady\":true",
            ),
        ];
        // SAFETY: `ep` points to live `MockEnv` for the duration of the test.
        unsafe {
            assert!(cases[0].0(ep, n, n).is_null());
            for (f, inp, exp) in cases {
                let h = bx.1.mk_s(inp);
                assert!(bx.1.get_s(f(ep, n, h))?.contains(exp));
            }
        }
        assert_eq!((bx.1.acq, bx.1.thrown.len()), (bx.1.rel, 1));
        Ok(())
    }
}
