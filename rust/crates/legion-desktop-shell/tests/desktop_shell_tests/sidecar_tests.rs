//! Sidecar preflight, runtime discovery, `BUG-09` env scrubbing, and supervised spawn tests.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use legion_desktop_shell::sidecar::{
    RuntimeDiscoverySource, SidecarConfig, SidecarController, SidecarError,
};
use legion_persistence::service_preflight::ServicePreflightResult;
use legion_process_ffi::supervisor::JobProcessSupervisor;

static TEST_DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

struct TestTempDir {
    path: PathBuf,
}

impl TestTempDir {
    fn new(prefix: &str) -> Result<Self, String> {
        let idx = TEST_DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("{prefix}-{}-{idx}", std::process::id()));
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[test]
fn sidecar_sanitizes_bug09_watch_debugger_and_desktop_override_env_vars() -> Result<(), String> {
    let dir = TestTempDir::new("legion-sidecar-env")?;
    let mut cfg = SidecarConfig {
        base_dir: dir.path().to_path_buf(),
        ..SidecarConfig::default()
    };
    cfg.extra_env
        .insert("WATCH_REPORT_DEPENDENCIES".to_owned(), "leaked".to_owned());
    cfg.extra_env
        .insert("LEGION_CUSTOM_KEEP".to_owned(), "preserved".to_owned());

    let controller = SidecarController::new(cfg);
    let sanitized = controller.sanitize_env_entries(
        [
            ("WATCH_REPORT_DEPENDENCIES", "1"),
            ("NODE_WATCH_TOKEN", "secret"),
            ("LEGIONCODE_PORT", "4242"),
            ("LEGIONCODE_HOST", "0.0.0.0"),
            (
                "NODE_OPTIONS",
                "--max-old-space-size=4096 --inspect --inspect-brk=9229 --inspect-port 9230",
            ),
            ("PATH", "/usr/local/bin:/usr/bin"),
        ],
        true,
    );

    assert_eq!(sanitized.get("WATCH_REPORT_DEPENDENCIES"), None);
    assert_eq!(sanitized.get("NODE_WATCH_TOKEN"), None);
    assert_eq!(sanitized.get("LEGIONCODE_PORT"), None);
    assert_eq!(sanitized.get("LEGIONCODE_HOST"), None);
    assert_eq!(
        sanitized.get("NODE_OPTIONS"),
        Some("--max-old-space-size=4096")
    );
    assert_eq!(sanitized.get("LEGION_CUSTOM_KEEP"), Some("preserved"));
    assert_eq!(
        sanitized.get("LEGIONCODE_HOME"),
        Some(dir.path().to_string_lossy().as_ref())
    );
    Ok(())
}

#[test]
fn sidecar_discovers_explicit_tauri_external_bin_and_path_runtimes() -> Result<(), String> {
    let dir = TestTempDir::new("legion-sidecar-discover")?;
    let exe_dir = dir.path().join("host");
    let bin_dir = exe_dir.join("binaries");
    fs::create_dir_all(&bin_dir).map_err(|e| e.to_string())?;
    let host_exe = exe_dir.join(if cfg!(windows) {
        "legioncode-desktop.exe"
    } else {
        "legioncode-desktop"
    });
    fs::write(&host_exe, b"stub-host").map_err(|e| e.to_string())?;

    let sidecar_name = if cfg!(windows) {
        "legioncode.exe"
    } else {
        "legioncode"
    };
    let tauri_sidecar = bin_dir.join(sidecar_name);
    fs::write(&tauri_sidecar, b"stub-sidecar").map_err(|e| e.to_string())?;

    let controller = SidecarController::new(SidecarConfig {
        base_dir: dir.path().to_path_buf(),
        ..SidecarConfig::default()
    });
    let found_tauri = controller.discover_runtime_with_inputs(None, None, Some(&host_exe));
    assert!(found_tauri.found);
    assert_eq!(found_tauri.source, RuntimeDiscoverySource::TauriExternalBin);
    assert_eq!(found_tauri.binary_path, tauri_sidecar);

    let explicit_bin = dir.path().join("custom-legioncode");
    fs::write(&explicit_bin, b"custom").map_err(|e| e.to_string())?;
    let explicit_ctrl = SidecarController::new(SidecarConfig {
        base_dir: dir.path().to_path_buf(),
        explicit_binary_path: Some(explicit_bin.clone()),
        ..SidecarConfig::default()
    });
    let found_explicit = explicit_ctrl.discover_runtime_with_inputs(None, None, Some(&host_exe));
    assert!(found_explicit.found);
    assert_eq!(found_explicit.source, RuntimeDiscoverySource::Explicit);
    assert_eq!(found_explicit.binary_path, explicit_bin);
    Ok(())
}

#[test]
fn sidecar_preflight_blocks_corrupt_db_and_prepares_clean_launch_args() -> Result<(), String> {
    let dir = TestTempDir::new("legion-sidecar-preflight")?;
    let bin = dir.path().join("fake-legioncode");
    fs::write(&bin, b"binary").map_err(|e| e.to_string())?;

    let controller = SidecarController::new(SidecarConfig {
        base_dir: dir.path().to_path_buf(),
        explicit_binary_path: Some(bin),
        bootstrap_fd: None,
        host: "127.0.0.1".to_owned(),
        port: 3773,
        ..SidecarConfig::default()
    });

    let prepared = controller
        .prepare_spawn_spec_with_env([("WATCH_REPORT_DEPENDENCIES", "1")], cfg!(windows))
        .map_err(|e| e.to_string())?;
    assert!(matches!(
        prepared.preflight,
        ServicePreflightResult::Ready {
            launcher_protocol: 4,
            ..
        }
    ));
    assert_eq!(
        prepared.launch_args,
        vec![
            "serve".to_owned(),
            "--host".to_owned(),
            "127.0.0.1".to_owned(),
            "--port".to_owned(),
            "3773".to_owned(),
            "--base-dir".to_owned(),
            dir.path().to_string_lossy().into_owned(),
            "--no-browser".to_owned(),
        ]
    );
    assert_eq!(
        prepared.sanitized_env.get("WATCH_REPORT_DEPENDENCIES"),
        None
    );

    // Writing non-SQLite bytes into `statev2.sqlite` must block launch before spawning.
    fs::create_dir_all(&prepared.paths.state_dir).map_err(|e| e.to_string())?;
    fs::write(&prepared.paths.db_path, b"corrupt-sqlite").map_err(|e| e.to_string())?;
    let err = controller
        .prepare_spawn_spec()
        .err()
        .ok_or("expected PreflightBlocked on corrupt SQLite")?;
    assert!(matches!(err, SidecarError::PreflightBlocked { .. }));
    Ok(())
}

#[test]
fn sidecar_controller_spawns_supervised_child_via_job_supervisor() -> Result<(), String> {
    let dir = TestTempDir::new("legion-sidecar-spawn")?;
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_legioncode-desktop"));

    let controller = SidecarController::new(SidecarConfig {
        base_dir: dir.path().to_path_buf(),
        explicit_binary_path: Some(exe),
        bootstrap_fd: None,
        extra_args: Vec::new(),
        ..SidecarConfig::default()
    });
    // Use `build_spawn_spec` directly with `--version` + controller-sanitized env for a real
    // supervised process roundtrip through `JobProcessSupervisor`.
    let preflight = controller.run_preflight();
    assert!(preflight.is_ready());
    let env = controller.sanitize_env_entries(
        [
            ("WATCH_REPORT_DEPENDENCIES", "1"),
            ("NODE_OPTIONS", "--inspect-brk --max-old-space-size=1024"),
        ],
        cfg!(windows),
    );
    let spec = legion_desktop_shell::sidecar::build_spawn_spec(
        Path::new(env!("CARGO_BIN_EXE_legioncode-desktop")),
        &["--version".to_owned()],
        dir.path(),
        env,
        true,
    );

    let mut supervisor = JobProcessSupervisor::new().map_err(|e| e.to_string())?;
    let (token, mut pipes) = supervisor.spawn(&spec).map_err(|e| e.to_string())?;
    let mut stdout = String::new();
    if let Some(mut reader) = pipes.take_stdout() {
        reader
            .read_to_string(&mut stdout)
            .map_err(|e| e.to_string())?;
    }
    let status = supervisor.wait(token).map_err(|e| e.to_string())?;
    assert!(status.success(), "supervised child failed: {status:?}");
    assert!(
        stdout.contains(env!("CARGO_PKG_VERSION")),
        "unexpected stdout: {stdout}"
    );
    Ok(())
}
