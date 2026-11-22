//! Local Android/iOS device-hub process manager with emulator camera probe cache (`#39764`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;

use crate::error::ServerRuntimeError;

/// Env var configuring extra emulator CLI flags for the Expo device hub.
pub const EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS_ENV: &str = "EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS";

/// Extra CLI arguments forcing software emulated camera when emulator lacks image-file support.
pub const EMULATED_CAMERA_EXTRA_ARGS: &str = "-camera-front emulated -camera-back emulated";

/// Snapshot of a running local device-hub child process and its cached emulator probe metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceHubProcessState {
    /// Whether the hub is currently running.
    pub running: bool,
    /// `EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS` value applied when this hub started.
    pub emulator_extra_args: Option<String>,
    /// Binary `mtime_ms:size` stamp of the Android emulator when the hub started.
    pub emulator_stamp: Option<String>,
    /// Total environment map passed to the hub spawn.
    pub env: BTreeMap<String, String>,
}

/// Abstracts spawning/stopping the device hub and probing emulator camera support (`#39764`).
pub trait DeviceHubBackend {
    /// Probe the emulator binary for camera help flags (`-help-camera-front`).
    fn probe_emulator_camera_help(
        &mut self,
        emulator_bin: &Path,
    ) -> Result<String, ServerRuntimeError>;

    /// Spawn (or respawn) the device-hub process with the supplied environment variables.
    fn spawn_hub(&mut self, env: &BTreeMap<String, String>) -> Result<(), ServerRuntimeError>;

    /// Stop the currently running device-hub process.
    fn stop_hub(&mut self) -> Result<(), ServerRuntimeError>;
}

/// Real OS process backend executing the Android `emulator -help-camera-front` binary probe.
#[derive(Debug, Default)]
pub struct OsDeviceHubBackend;

impl DeviceHubBackend for OsDeviceHubBackend {
    fn probe_emulator_camera_help(
        &mut self,
        emulator_bin: &Path,
    ) -> Result<String, ServerRuntimeError> {
        let out = Command::new(emulator_bin)
            .arg("-help-camera-front")
            .output()?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        Ok(format!("{stdout}\n{stderr}"))
    }

    fn spawn_hub(&mut self, _env: &BTreeMap<String, String>) -> Result<(), ServerRuntimeError> {
        Ok(())
    }

    fn stop_hub(&mut self) -> Result<(), ServerRuntimeError> {
        Ok(())
    }
}

/// Computes `mtime_ms:size` stamp for `$ANDROID_HOME/emulator/emulator[.exe]`, or `None` if missing.
pub fn compute_emulator_stamp(android_home: Option<&Path>, is_windows: bool) -> Option<String> {
    let sdk = android_home?;
    let bin_name = if is_windows {
        "emulator.exe"
    } else {
        "emulator"
    };
    let binary = sdk.join("emulator").join(bin_name);
    let meta = std::fs::metadata(binary).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    Some(format!("{mtime_ms}:{}", meta.len()))
}

/// Resolves the emulator binary path inside `android_home`.
pub fn resolve_emulator_binary(android_home: &Path, is_windows: bool) -> PathBuf {
    android_home.join("emulator").join(if is_windows {
        "emulator.exe"
    } else {
        "emulator"
    })
}

/// Computes whether emulated camera fallback args are needed from `-help-camera-front` text.
pub fn camera_extra_args_from_help(help_text: &str) -> Option<String> {
    let lower = help_text.to_ascii_lowercase();
    if lower.contains("imagefile:") {
        None
    } else if !lower.trim().is_empty() {
        Some(EMULATED_CAMERA_EXTRA_ARGS.to_owned())
    } else {
        None
    }
}

/// Coordinates device-hub lifecycle, restarting when the emulator binary changes camera support (`#39764`).
#[derive(Debug, Default)]
pub struct LocalDeviceHubManager {
    running: Option<DeviceHubProcessState>,
    is_windows: bool,
}

impl LocalDeviceHubManager {
    /// Create a [`LocalDeviceHubManager`] for the current host OS.
    pub fn new() -> Self {
        Self {
            running: None,
            is_windows: cfg!(windows),
        }
    }

    /// Create a [`LocalDeviceHubManager`] with explicit platform flag (for cross-platform tests).
    pub const fn with_platform(is_windows: bool) -> Self {
        Self {
            running: None,
            is_windows,
        }
    }

    /// Returns the currently tracked running hub state, if any.
    pub const fn current_state(&self) -> Option<&DeviceHubProcessState> {
        self.running.as_ref()
    }

    /// Marks the running hub state externally (e.g. crash/stopped simulation in tests).
    pub fn mark_stopped(&mut self) {
        self.running = None;
    }

    /// Ensures the device hub is ready; skips probing if running with an identical emulator stamp,
    /// updates stamp in place if camera flags match, or stops + restarts when camera flags change (`#39764`).
    pub fn ensure_ready<B: DeviceHubBackend>(
        &mut self,
        backend: &mut B,
        android_home: Option<&Path>,
        base_env: &BTreeMap<String, String>,
    ) -> Result<&DeviceHubProcessState, ServerRuntimeError> {
        let current_stamp = compute_emulator_stamp(android_home, self.is_windows);

        let cached_extra_args = if let Some(ref mut active) = self.running
            && active.running
        {
            if active.emulator_stamp == current_stamp {
                return self.running.as_ref().ok_or_else(|| {
                    ServerRuntimeError::Process("device hub state missing".to_owned())
                });
            }

            let next_extra_args = probe_camera_args(backend, android_home, self.is_windows);
            if next_extra_args == active.emulator_extra_args {
                active.emulator_stamp = current_stamp;
                return self.running.as_ref().ok_or_else(|| {
                    ServerRuntimeError::Process("device hub state missing".to_owned())
                });
            }

            backend.stop_hub()?;
            self.running = None;
            Some(next_extra_args)
        } else {
            None
        };

        let extra_args = match cached_extra_args {
            Some(args) => args,
            None => probe_camera_args(backend, android_home, self.is_windows),
        };
        let mut spawn_env = base_env.clone();
        if let Some(ref args) = extra_args {
            spawn_env.insert(
                EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS_ENV.to_owned(),
                args.clone(),
            );
        } else {
            spawn_env.remove(EXPO_DEVICE_HUB_EMULATOR_EXTRA_ARGS_ENV);
        }

        backend.spawn_hub(&spawn_env)?;
        self.running = Some(DeviceHubProcessState {
            running: true,
            emulator_extra_args: extra_args,
            emulator_stamp: current_stamp,
            env: spawn_env,
        });
        self.running
            .as_ref()
            .ok_or_else(|| ServerRuntimeError::Process("device hub state missing".to_owned()))
    }
}

fn probe_camera_args<B: DeviceHubBackend>(
    backend: &mut B,
    android_home: Option<&Path>,
    is_windows: bool,
) -> Option<String> {
    let sdk = android_home?;
    let bin = resolve_emulator_binary(sdk, is_windows);
    if !bin.exists() {
        return None;
    }
    let help = backend.probe_emulator_camera_help(&bin).ok()?;
    camera_extra_args_from_help(&help)
}
