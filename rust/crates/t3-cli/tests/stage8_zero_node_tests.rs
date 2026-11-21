//! Stage 8 zero-Node integration suite proving every `t3` command, flag, `--help`,
//! `--completions`, `--wizard`, `--log-level`, worker, and live client path executes
//! completely in Safe Rust without Node delegation.

#![forbid(unsafe_code)]

mod support;

#[path = "stage8/global_help.rs"]
mod global_help;

#[path = "stage8/auth_env_doctor.rs"]
mod auth_env_doctor;

#[path = "stage8/environment_live.rs"]
mod environment_live;

#[path = "stage8/remote_speech_connect.rs"]
mod remote_speech_connect;
