//! Deterministic `Apple` `XCFramework` and `Android` `Gradle`/`NDK` templates.

pub(crate) const APPLE_MODULEMAP_TEMPLATE: &str = r#"module LegionMobileFFI {
    header "legion_mobile_ffi.h"
    export *
}
"#;

pub(crate) const APPLE_INFO_PLIST_TEMPLATE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleIdentifier</key>
    <string>dev.legioncode.legionmobileffi</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>LegionMobileFFI</string>
    <key>CFBundlePackageType</key>
    <string>FMWK</string>
    <key>CFBundleShortVersionString</key>
    <string>0.1.10</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>MinimumOSVersion</key>
    <string>16.1</string>
</dict>
</plist>
"#;

pub(crate) const APPLE_PACKAGE_SWIFT_TEMPLATE: &str = r#"// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "LegionMobileCore",
    platforms: [
        .iOS(.v16)
    ],
    products: [
        .library(
            name: "LegionMobileCore",
            targets: ["LegionMobileCore", "LegionMobileFFI"]
        )
    ],
    targets: [
        .binaryTarget(
            name: "LegionMobileFFI",
            path: "LegionMobileFFI.xcframework"
        ),
        .target(
            name: "LegionMobileCore",
            dependencies: ["LegionMobileFFI"],
            path: "Sources/LegionMobileCore"
        )
    ]
)
"#;

pub(crate) const APPLE_BUILD_XCFRAMEWORK_SH: &str = r#"#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_RUST_DIR="${REPO_RUST_DIR:-$(cd "${SCRIPT_DIR}/../../.." && pwd)}"
MANIFEST="${REPO_RUST_DIR}/Cargo.toml"
OUT_DIR="${OUT_DIR:-${SCRIPT_DIR}/build}"
HEADERS_DIR="${OUT_DIR}/headers"
SIM_FAT_LIB="${OUT_DIR}/liblegion_mobile_ffi_sim.a"
XCFRAMEWORK_OUT="${OUT_DIR}/LegionMobileFFI.xcframework"

TARGETS=(
  "aarch64-apple-ios"
  "aarch64-apple-ios-sim"
  "x86_64-apple-ios"
)

for target in "${TARGETS[@]}"; do
  cargo build --manifest-path "${MANIFEST}" --release --lib --target "${target}" -p legion-mobile-ffi
done

mkdir -p "${HEADERS_DIR}"
cp "${SCRIPT_DIR}/include/legion_mobile_ffi.h" "${HEADERS_DIR}/legion_mobile_ffi.h"
cp "${SCRIPT_DIR}/module.modulemap" "${HEADERS_DIR}/module.modulemap"

IOS_LIB="${REPO_RUST_DIR}/target/aarch64-apple-ios/release/liblegion_mobile_ffi.a"
SIM_ARM64="${REPO_RUST_DIR}/target/aarch64-apple-ios-sim/release/liblegion_mobile_ffi.a"
SIM_X64="${REPO_RUST_DIR}/target/x86_64-apple-ios/release/liblegion_mobile_ffi.a"

lipo -create "${SIM_ARM64}" "${SIM_X64}" -output "${SIM_FAT_LIB}"
rm -rf "${XCFRAMEWORK_OUT}"
xcodebuild -create-xcframework \
  -library "${IOS_LIB}" -headers "${HEADERS_DIR}" \
  -library "${SIM_FAT_LIB}" -headers "${HEADERS_DIR}" \
  -output "${XCFRAMEWORK_OUT}"
"#;

pub(crate) const ANDROID_BUILD_GRADLE_KTS: &str = r#"plugins {
    id("com.android.library")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "dev.legioncode.mobile"
    compileSdk = 35

    defaultConfig {
        minSdk = 26
        consumerProguardFiles("proguard-rules.pro")
        ndk {
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64")
        }
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDirs("src/main/jniLibs")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }
}
"#;

pub(crate) const ANDROID_PROGUARD_RULES: &str = r"-keep class dev.legioncode.mobile.LegionMobileBridge { *; }
-keepclasseswithmembernames class dev.legioncode.mobile.LegionMobileBridge {
    native <methods>;
}
-dontwarn dev.legioncode.mobile.**
";

pub(crate) const ANDROID_BRIDGE_KT: &str = r#"package dev.legioncode.mobile

object LegionMobileBridge {
    init {
        System.loadLibrary("legion_mobile_ffi")
    }

    external fun nativeAbiVersion(): Int
    external fun nativeDecorateDiffJson(inputJson: String): String
    external fun nativeFilterClipboardJson(inputJson: String): String
    external fun nativeNavStringsJson(locale: String): String
    external fun nativeLiveActivityJson(inputJson: String): String
    external fun nativePaletteResolveJson(inputJson: String): String
    external fun nativeHostSwapChunksJson(inputJson: String): String
    external fun nativeThemePlanJson(inputJson: String): String
    external fun nativeVtRenderSnapshot(inputJson: String): ByteArray
    external fun nativeVtDecodeSnapshotJson(bytes: ByteArray): String
    external fun nativeFilterReviewFilesJson(inputJson: String): String
    external fun nativeLinkedLinesJson(inputJson: String): String
    external fun nativeTunnelGuardJson(inputJson: String): String
    external fun nativeSecretCardJson(inputJson: String): String
}
"#;

pub(crate) const ANDROID_BUILD_JNI_SH: &str = r#"#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_RUST_DIR="${REPO_RUST_DIR:-$(cd "${SCRIPT_DIR}/../../.." && pwd)}"
MANIFEST="${REPO_RUST_DIR}/Cargo.toml"
JNILIBS_DIR="${SCRIPT_DIR}/src/main/jniLibs"

PAIRS=(
  "aarch64-linux-android:arm64-v8a"
  "armv7-linux-androideabi:armeabi-v7a"
  "x86_64-linux-android:x86_64"
)

for entry in "${PAIRS[@]}"; do
  target="${entry%%:*}"
  abi="${entry##*:}"
  cargo build --manifest-path "${MANIFEST}" --release --lib --target "${target}" -p legion-mobile-ffi
  mkdir -p "${JNILIBS_DIR}/${abi}"
  cp "${REPO_RUST_DIR}/target/${target}/release/liblegion_mobile_ffi.so" \
    "${JNILIBS_DIR}/${abi}/liblegion_mobile_ffi.so"
done
"#;
