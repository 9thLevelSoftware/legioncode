//! Safe Rust tests verifying Apple `XCFramework` and `Android` `Gradle`/`NDK` bundle generation.

use std::time::{SystemTime, UNIX_EPOCH};

use t3_mobile_ffi::core::packaging_bundle_json as core_packaging_bundle_json;
use t3_mobile_ffi::packaging::{
    ANDROID_TARGET_TRIPLES, APPLE_TARGET_TRIPLES, MobilePackagingBundle,
    generate_mobile_packaging_bundle, packaging_bundle_json,
};

#[test]
fn packaging_bundle_contains_all_apple_and_android_artifacts() -> Result<(), String> {
    let bundle = generate_mobile_packaging_bundle();
    assert_eq!(bundle.abi_version, 1);
    assert_eq!(bundle.apple_targets, APPLE_TARGET_TRIPLES);
    assert_eq!(bundle.android_targets, ANDROID_TARGET_TRIPLES);

    let required_paths = [
        "apple/include/t3_mobile_ffi.h",
        "apple/module.modulemap",
        "apple/Info.plist",
        "apple/Package.swift",
        "apple/build-xcframework.sh",
        "android/build.gradle.kts",
        "android/proguard-rules.pro",
        "android/T3MobileBridge.kt",
        "android/build-jni.sh",
    ];
    for rel in required_paths {
        assert!(
            bundle.file(rel).is_some(),
            "missing packaging artifact {rel}"
        );
    }

    let apple_sh = bundle
        .file("apple/build-xcframework.sh")
        .ok_or("missing apple/build-xcframework.sh")?;
    for triple in [
        "aarch64-apple-ios",
        "aarch64-apple-ios-sim",
        "x86_64-apple-ios",
    ] {
        assert!(apple_sh.contains(triple), "missing apple target {triple}");
    }

    let android_sh = bundle
        .file("android/build-jni.sh")
        .ok_or("missing android/build-jni.sh")?;
    for triple in [
        "aarch64-linux-android",
        "armv7-linux-androideabi",
        "x86_64-linux-android",
    ] {
        assert!(
            android_sh.contains(triple),
            "missing android target {triple}"
        );
    }

    let bridge_kt = bundle
        .file("android/T3MobileBridge.kt")
        .ok_or("missing android/T3MobileBridge.kt")?;
    assert!(bridge_kt.contains("package codes.t3.mobile"));
    assert!(bridge_kt.contains("System.loadLibrary(\"t3_mobile_ffi\")"));
    let expected_kotlin_fns = [
        "external fun nativeAbiVersion(): Int",
        "external fun nativeDecorateDiffJson(inputJson: String): String",
        "external fun nativeFilterClipboardJson(inputJson: String): String",
        "external fun nativeNavStringsJson(locale: String): String",
        "external fun nativeLiveActivityJson(inputJson: String): String",
        "external fun nativePaletteResolveJson(inputJson: String): String",
        "external fun nativeHostSwapChunksJson(inputJson: String): String",
        "external fun nativeThemePlanJson(inputJson: String): String",
        "external fun nativeVtRenderSnapshot(inputJson: String): ByteArray",
        "external fun nativeVtDecodeSnapshotJson(bytes: ByteArray): String",
    ];
    for sig in expected_kotlin_fns {
        assert!(bridge_kt.contains(sig), "T3MobileBridge.kt missing {sig}");
    }

    let raw_json = packaging_bundle_json().map_err(|err| err.to_string())?;
    assert_eq!(
        raw_json,
        core_packaging_bundle_json().map_err(|err| err.to_string())?
    );
    let reparsed: MobilePackagingBundle =
        serde_json::from_str(&raw_json).map_err(|err| err.to_string())?;
    assert_eq!(reparsed, bundle);

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp_dir = std::env::temp_dir().join(format!("t3_mobile_pkg_{stamp}"));
    let written = bundle
        .write_to_dir(&temp_dir)
        .map_err(|err| err.to_string())?;
    assert_eq!(written.len(), bundle.files.len());
    for path in &written {
        assert!(path.is_file(), "expected written file {}", path.display());
    }
    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(())
}
