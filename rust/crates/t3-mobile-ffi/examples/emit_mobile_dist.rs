//! Emits `t3-mobile-ffi` C/Swift/Kotlin SDK sources and Apple/Android packaging bundles.

#![forbid(unsafe_code)]

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use t3_mobile_ffi::codegen::generate_sdk_sources;
use t3_mobile_ffi::packaging::generate_mobile_packaging_bundle;

fn write_sdk_file(
    base: &Path,
    dir: &str,
    filename: &str,
    contents: &str,
) -> Result<(), Box<dyn Error>> {
    let target_dir = base.join(dir);
    fs::create_dir_all(&target_dir)?;
    fs::write(target_dir.join(filename), contents)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let out_dir = env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("dist/mobile"), PathBuf::from);
    let sdk = generate_sdk_sources();
    write_sdk_file(&out_dir, "include", &sdk.c_header_filename, &sdk.c_header)?;
    write_sdk_file(&out_dir, "swift", &sdk.swift_filename, &sdk.swift_source)?;
    write_sdk_file(&out_dir, "kotlin", &sdk.kotlin_filename, &sdk.kotlin_source)?;

    let bundle = generate_mobile_packaging_bundle();
    let written = bundle.write_to_dir(&out_dir)?;
    println!(
        "Emitted mobile SDK + {} packaging files to {}",
        written.len(),
        out_dir.display()
    );
    Ok(())
}
