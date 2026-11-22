//! Apple `XCFramework` and `Android` `Gradle`/`NDK` packaging bundle generator.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::codegen::generate_c_header;
use crate::core::MobileFfiError;
use crate::mobile_packaging_templates::{
    ANDROID_BRIDGE_KT, ANDROID_BUILD_GRADLE_KTS, ANDROID_BUILD_JNI_SH, ANDROID_PROGUARD_RULES,
    APPLE_BUILD_XCFRAMEWORK_SH, APPLE_INFO_PLIST_TEMPLATE, APPLE_MODULEMAP_TEMPLATE,
    APPLE_PACKAGE_SWIFT_TEMPLATE,
};

/// Ordered `Rust` compilation triples for `iOS` device and simulator `XCFramework` slices.
pub const APPLE_TARGET_TRIPLES: [&str; 3] = [
    "aarch64-apple-ios",
    "aarch64-apple-ios-sim",
    "x86_64-apple-ios",
];

/// Ordered `Rust` compilation triples for `Android` `JNI` shared libraries.
pub const ANDROID_TARGET_TRIPLES: [&str; 3] = [
    "aarch64-linux-android",
    "armv7-linux-androideabi",
    "x86_64-linux-android",
];

/// Generated `Apple` `XCFramework` and `Android` `Gradle`/`NDK` packaging bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MobilePackagingBundle {
    /// Exported `legion-mobile-ffi` `ABI` version.
    pub abi_version: u32,
    /// Crate version embedded in packaging manifests.
    pub crate_version: String,
    /// Ordered `iOS` target triples.
    pub apple_targets: Vec<String>,
    /// Ordered `Android` target triples.
    pub android_targets: Vec<String>,
    /// Deterministic path-to-source mapping (`apple/...` and `android/...`).
    pub files: BTreeMap<String, String>,
}

impl MobilePackagingBundle {
    /// Looks up a generated bundle file by relative path (e.g. `"apple/Package.swift"`).
    pub fn file(&self, path: &str) -> Option<&str> {
        self.files.get(path).map(String::as_str)
    }

    /// Writes every generated packaging file beneath `out_dir`.
    pub fn write_to_dir(&self, out_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
        let mut written = Vec::with_capacity(self.files.len());
        for (rel_path, contents) in &self.files {
            let full_path = out_dir.join(rel_path);
            if let Some(parent) = full_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&full_path, contents)?;
            written.push(full_path);
        }
        Ok(written)
    }
}

/// Generates the complete `Apple` `XCFramework` and `Android` `Gradle`/`NDK` packaging bundle.
///
/// # Examples
///
/// ```
/// let bundle = legion_mobile_ffi::packaging::generate_mobile_packaging_bundle();
/// assert!(bundle.file("apple/module.modulemap").is_some());
/// assert!(bundle.file("apple/Info.plist").is_some());
/// assert!(bundle.file("apple/Package.swift").is_some());
/// assert!(bundle.file("apple/build-xcframework.sh").is_some());
/// assert!(bundle.file("android/build.gradle.kts").is_some());
/// assert!(bundle.file("android/proguard-rules.pro").is_some());
/// assert!(bundle.file("android/LegionMobileBridge.kt").is_some());
/// assert!(bundle.file("android/build-jni.sh").is_some());
/// ```
pub fn generate_mobile_packaging_bundle() -> MobilePackagingBundle {
    let mut files = BTreeMap::new();
    files.insert(
        "apple/include/legion_mobile_ffi.h".to_owned(),
        generate_c_header(),
    );
    files.insert(
        "apple/module.modulemap".to_owned(),
        APPLE_MODULEMAP_TEMPLATE.to_owned(),
    );
    files.insert(
        "apple/Info.plist".to_owned(),
        APPLE_INFO_PLIST_TEMPLATE.to_owned(),
    );
    files.insert(
        "apple/Package.swift".to_owned(),
        APPLE_PACKAGE_SWIFT_TEMPLATE.to_owned(),
    );
    files.insert(
        "apple/build-xcframework.sh".to_owned(),
        APPLE_BUILD_XCFRAMEWORK_SH.to_owned(),
    );
    files.insert(
        "android/build.gradle.kts".to_owned(),
        ANDROID_BUILD_GRADLE_KTS.to_owned(),
    );
    files.insert(
        "android/proguard-rules.pro".to_owned(),
        ANDROID_PROGUARD_RULES.to_owned(),
    );
    files.insert(
        "android/LegionMobileBridge.kt".to_owned(),
        ANDROID_BRIDGE_KT.to_owned(),
    );
    files.insert(
        "android/build-jni.sh".to_owned(),
        ANDROID_BUILD_JNI_SH.to_owned(),
    );

    MobilePackagingBundle {
        abi_version: crate::abi_version(),
        crate_version: env!("CARGO_PKG_VERSION").to_owned(),
        apple_targets: APPLE_TARGET_TRIPLES
            .iter()
            .map(|target| (*target).to_owned())
            .collect(),
        android_targets: ANDROID_TARGET_TRIPLES
            .iter()
            .map(|target| (*target).to_owned())
            .collect(),
        files,
    }
}

/// Serializes [`generate_mobile_packaging_bundle`] as compact `JSON`.
pub fn packaging_bundle_json() -> Result<String, MobileFfiError> {
    serde_json::to_string(&generate_mobile_packaging_bundle())
        .map_err(|err| MobileFfiError::InvalidJson(err.to_string()))
}
