#![deny(unsafe_op_in_unsafe_fn)]
//! Quarantined OS process supervision, `FFI` boundary, and `native/resource-monitor`
//! protocol v3 engine for `LegionCode` (`BUG-09`).
//!
//! All pure-Rust crates in the workspace enforce `#![forbid(unsafe_code)]`; only this crate
//! encapsulates platform `FFI` bindings (`win32_job` and `ffi_boundary`) behind safe Rust
//! wrappers and a `catch_unwind`-guarded C ABI.

pub mod async_log;
pub mod env;
pub mod ffi_boundary;
pub mod keep_awake;
pub mod pipes;
pub mod pty;
#[cfg(unix)]
mod pty_unix;
#[cfg(windows)]
mod pty_win32;
#[cfg(windows)]
mod pty_win32_ffi;
pub mod resource_monitor;
mod resource_monitor_codec;
mod resource_monitor_events;
mod resource_monitor_history;
mod resource_monitor_sampler;
mod resource_monitor_types;
pub mod spawn_flags;
pub mod supervisor;
pub mod win32_job;

/// Returns the `ABI` version implemented by this quarantined `FFI` crate.
///
/// # Examples
///
/// ```
/// assert_eq!(legion_process_ffi::abi_version(), 1);
/// ```
pub const fn abi_version() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::abi_version;

    #[test]
    fn reports_expected_abi_version() {
        assert_eq!(abi_version(), 1);
    }
}
