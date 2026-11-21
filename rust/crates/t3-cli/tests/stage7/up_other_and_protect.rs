//! Parity tests for `t3 service protect` flags and `t3 up` other-home error formatting.

use crate::stage7_up_tools::create_fake_service_tools;
use crate::support::{TestResult, isolated_env, run_t3};

const NONEXISTENT_BIN_TS: &str = r"Z:\nonexistent\bin.ts";

#[test]
fn service_protect_flags_and_up_other_home_match_node_parity() -> TestResult {
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("fakebin");
    create_fake_service_tools(&fake_bin)?;
    let unit_path = temp.path().join("sandbox-unit").join("t3code-unit");
    let home = temp.path().join("prot-home");
    let home_str = home.to_string_lossy().into_owned();
    let unit_str = unit_path.to_string_lossy().into_owned();
    let path_str = fake_bin.to_string_lossy().into_owned();

    if !cfg!(target_os = "linux") {
        let prot_log = temp.path().join("protect-fallback.jsonl");
        let prot_log_str = prot_log.to_string_lossy().into_owned();
        let prot_env = isolated_env(
            temp.path(),
            &[
                ("PATH", &path_str),
                ("T3_BIN_TS", NONEXISTENT_BIN_TS),
                ("RUST_ANYTHING_FALLBACK_LOG", &prot_log_str),
            ],
        );
        let prot = run_t3(
            &[
                "service",
                "protect",
                "--base-dir",
                &home_str,
                "--undo",
                "--yes",
            ],
            &prot_env,
        )?;
        assert_eq!(prot.status.code(), Some(0));
        assert_eq!(
            String::from_utf8(prot.stdout)?,
            "Service protection needs Linux with systemd.\n"
        );
        assert!(!prot_log.exists());
    }

    let env = isolated_env(
        temp.path(),
        &[
            ("PATH", &path_str),
            ("T3_BIN_TS", NONEXISTENT_BIN_TS),
            ("T3CODE_BOOT_SERVICE_UNIT_PATH", &unit_str),
        ],
    );
    let other_home = temp.path().join("other-home");
    let up_cur = temp.path().join("cur-home");
    let install_other = run_t3(
        &[
            "service",
            "install",
            "--base-dir",
            &other_home.to_string_lossy(),
        ],
        &env,
    )?;
    assert_eq!(install_other.status.code(), Some(0));

    let up_other = run_t3(
        &["up", "--no-lan", "--base-dir", &up_cur.to_string_lossy()],
        &env,
    )?;
    assert_eq!(up_other.status.code(), Some(1));
    assert_eq!(up_other.stdout, [] as [u8; 0]);
    let err = String::from_utf8(up_other.stderr)?;
    assert!(err.starts_with("\nERROR\n  The T3 Code service on this machine serves "));
    assert!(err.contains("both data directories are kept."));
    Ok(())
}
