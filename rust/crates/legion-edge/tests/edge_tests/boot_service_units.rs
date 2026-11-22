//! Tests for `legion_edge::boot_service_units` (systemd, launchd, Task XML, protection).

use legion_contracts::brand::LEGION_CODE;
use legion_edge::boot_service_units::{
    BOOT_SERVICE_LAUNCHD_LABEL, BOOT_SERVICE_PLIST_FILE, BOOT_SERVICE_UNIT_FILE, BootServicePlan,
    BootServiceProblem, BootServiceRenderError, ProtectionMode, REMOTE_ACCESS_UNITS,
    build_service_protection_plan, build_service_protection_plan_for_brand,
    decode_service_unit_bytes, disable_service_task, encode_windows_task_utf16le_bom,
    extract_service_unit_base_dir, extract_service_unit_base_dir_for_brand,
    extract_service_unit_environment_value, extract_service_unit_log_path,
    extract_service_unit_program, format_boot_service_base_dir_option, format_boot_service_problem,
    format_boot_service_problem_for_brand, parse_loaded_remote_access_units,
    render_boot_service_launchd_plist, render_boot_service_launchd_plist_for_brand,
    render_boot_service_systemd_unit, render_boot_service_systemd_unit_for_brand,
    render_boot_service_windows_task_xml, render_boot_service_windows_task_xml_for_brand,
    render_protection_privileged_script, render_protection_privileged_script_for_brand,
    service_task_is_disabled, systemd_unit_directory_of,
};
use proptest::prelude::*;

fn sample_plan(base_dir: &str) -> BootServicePlan {
    BootServicePlan::new(
        vec![
            format!("{base_dir}/runtime/versions/1.2.3/legioncode"),
            "__service-launcher".to_owned(),
        ],
        base_dir,
        format!("{base_dir}/userdata/logs/boot-service.log"),
        "/home/theo/.config/systemd/user/legioncode.service",
    )
}

#[test]
fn systemd_unit_rendering_roundtrips_and_rejects_unsafe_executables() -> Result<(), String> {
    let plan =
        sample_plan("/home/theo/Legion Data/100%").with_install_bin_dir("/opt/legioncode/bin");
    let unit = render_boot_service_systemd_unit(&plan).map_err(|e| e.to_string())?;
    assert_eq!(
        extract_service_unit_base_dir(&unit).as_deref(),
        Some("/home/theo/Legion Data/100%")
    );
    assert_eq!(
        extract_service_unit_environment_value(&unit, "LEGIONCODE_INSTALL_BIN_DIR").as_deref(),
        Some("/opt/legioncode/bin")
    );
    assert_eq!(
        extract_service_unit_log_path(&unit).as_deref(),
        Some("/home/theo/Legion Data/100%/userdata/logs/boot-service.log")
    );
    assert_eq!(
        extract_service_unit_program(&unit).as_deref(),
        Some("/home/theo/Legion Data/100%/runtime/versions/1.2.3/legioncode")
    );

    let service_lines: Vec<&str> = unit.lines().collect();
    for expected in [
        "KillMode=mixed",
        "OOMPolicy=continue",
        "CPUWeight=1000",
        "IOWeight=1000",
        "MemoryLow=1536M",
        "OOMScoreAdjust=100",
        "ManagedOOMPreference=avoid",
    ] {
        assert!(service_lines.contains(&expected), "missing {expected}");
    }

    for unsafe_dir in [
        "/home/theo/Legion \"Data\"",
        "/home/theo/Theo's Legion",
        "/home/theo/Legion\\Data",
        "/home/theo/Legion\nData",
    ] {
        let bad = sample_plan(unsafe_dir);
        assert_eq!(
            render_boot_service_systemd_unit(&bad),
            Err(BootServiceRenderError::UnsupportedBaseDir {
                base_dir: unsafe_dir.to_owned(),
            })
        );
    }

    assert_eq!(
        systemd_unit_directory_of(&[
            "/usr/lib/systemd/user".to_owned(),
            "/home/theo/.config/systemd/user.control".to_owned(),
            "/home/theo/.config/systemd/user".to_owned(),
        ]),
        Some("/home/theo/.config/systemd/user")
    );
    Ok(())
}

#[test]
fn launchd_plist_and_windows_task_xml_roundtrip_and_utf16le_bom() {
    let plist_plan = sample_plan("/Users/theo/a&b <c>").with_install_bin_dir("/opt/legioncode/bin");
    let plist = render_boot_service_launchd_plist(&plist_plan, "/Users/theo", "/usr/bin:/bin");
    assert!(plist.contains(BOOT_SERVICE_LAUNCHD_LABEL));
    assert!(plist.contains(BOOT_SERVICE_PLIST_FILE));
    assert_eq!(
        extract_service_unit_base_dir(&plist).as_deref(),
        Some("/Users/theo/a&b <c>")
    );
    assert_eq!(
        extract_service_unit_environment_value(&plist, "LEGIONCODE_INSTALL_BIN_DIR").as_deref(),
        Some("/opt/legioncode/bin")
    );
    assert_eq!(
        extract_service_unit_log_path(&plist).as_deref(),
        Some("/Users/theo/a&b <c>/userdata/logs/boot-service.log")
    );
    assert_eq!(
        extract_service_unit_program(&plist).as_deref(),
        Some("/Users/theo/a&b <c>/runtime/versions/1.2.3/legioncode")
    );

    let win_plan = BootServicePlan::new(
        vec![
            "C:\\Users\\theo\\Legion a&b\\legioncode.exe".to_owned(),
            "__service-launcher".to_owned(),
        ],
        "C:\\Users\\theo\\Legion a&b\\",
        "C:\\Users\\theo\\log.txt",
        "C:\\Users\\theo\\AppData\\Local\\legioncode\\legioncode-task.xml",
    );
    let task_xml = render_boot_service_windows_task_xml(
        &win_plan,
        "PC\\theo",
        "C:\\Users\\theo",
        "C:\\Windows\\System32\\conhost.exe",
    );
    assert!(!task_xml.starts_with("<?xml"));
    assert_eq!(
        extract_service_unit_base_dir(&task_xml).as_deref(),
        Some("C:\\Users\\theo\\Legion a&b\\")
    );
    assert_eq!(
        extract_service_unit_program(&task_xml).as_deref(),
        Some("C:\\Users\\theo\\Legion a&b\\legioncode.exe")
    );

    let encoded = encode_windows_task_utf16le_bom(&task_xml);
    assert_eq!(&encoded[..2], &[0xff, 0xfe]);
    assert_eq!(decode_service_unit_bytes(&encoded), task_xml);
    assert_eq!(decode_service_unit_bytes(task_xml.as_bytes()), task_xml);

    assert!(!service_task_is_disabled(&task_xml));
    let disabled = disable_service_task(&task_xml);
    assert!(service_task_is_disabled(&disabled));
    assert_eq!(disable_service_task(&disabled), disabled);
}

#[test]
fn base_dir_option_and_problem_messages_match_boot_service_ts() {
    assert_eq!(
        format_boot_service_base_dir_option("/home/o'reilly/legioncode"),
        "--base-dir '/home/o'\"'\"'reilly/legioncode'"
    );
    assert_eq!(BootServiceProblem::LingerDisabled.code(), "linger-disabled");
    assert!(
        BootServiceProblem::UserManagerUnavailable
            .format_default()
            .contains("systemctl --user status")
    );
    assert!(
        BootServiceProblem::LingerUnavailable
            .format_default()
            .contains("systemd-logind is available")
    );
    assert!(
        BootServiceProblem::LingerDisabled
            .format_default()
            .contains("sudo loginctl enable-linger \"$(id -un)\"")
    );
    assert!(
        format_boot_service_problem(
            BootServiceProblem::ServiceDisabled,
            "vp exec legioncode service install",
            "vp exec legioncode service restart",
        )
        .contains("vp exec legioncode service install")
    );
    assert!(
        format_boot_service_problem(
            BootServiceProblem::ServiceStopped,
            "custom install",
            "custom restart",
        )
        .contains("systemctl --user status legioncode.service")
    );
    assert!(
        format_boot_service_problem(
            BootServiceProblem::RestartPending,
            "custom install",
            "custom restart",
        )
        .contains("custom restart")
    );
}

#[test]
fn protection_plan_scripts_and_loaded_units_match_service_protection_ts() -> Result<(), String> {
    let plan = build_service_protection_plan(
        1000,
        "/home/theo/.config/systemd/user",
        &["tailscaled.service", "ssh.service"],
        Some("/tmp/sys"),
    );
    let manager = plan
        .system
        .iter()
        .find(|d| d.unit == "user@1000.service")
        .ok_or("missing manager drop-in")?;
    assert_eq!(
        manager.contents,
        "[Service]\nMemoryLow=2G\nOOMScoreAdjust=-900\n"
    );
    assert!(!manager.shared);
    assert_eq!(
        plan.manager_defaults.path,
        "/home/theo/.config/systemd/user.conf.d/90-legioncode-protect.conf"
    );
    assert!(plan.user.iter().any(|d| d.unit == BOOT_SERVICE_UNIT_FILE));
    assert_eq!(
        plan.oom_units,
        vec!["user@1000.service", "tailscaled.service", "ssh.service"]
    );

    let apply_script = render_protection_privileged_script(&plan, ProtectionMode::Apply);
    assert!(apply_script.starts_with("set -e\n"));
    assert!(
        apply_script.contains("cat >'/tmp/sys/user@1000.service.d/90-legioncode-protect.conf'")
    );
    assert!(apply_script.contains("systemctl daemon-reload\n"));
    assert!(apply_script.contains("oom_score_adj"));

    let undo_script = render_protection_privileged_script(&plan, ProtectionMode::Undo);
    assert!(undo_script.contains("set -- '/tmp/sys'/user@*.service.d/90-legioncode-protect.conf"));
    assert!(undo_script.contains("if [ ! -e \"$1\" ]; then"));

    let debian = "Id=tailscaled.service\nLoadState=loaded\n\nId=ssh.service\nLoadState=loaded\n\nId=ssh.service\nLoadState=loaded\n";
    assert_eq!(
        parse_loaded_remote_access_units(debian),
        vec!["tailscaled.service", "ssh.service"]
    );
    let fedora = "Id=tailscaled.service\nLoadState=not-found\n\nId=ssh.service\nLoadState=not-found\n\nId=sshd.service\nLoadState=loaded\n";
    assert_eq!(
        parse_loaded_remote_access_units(fedora),
        vec!["sshd.service"]
    );
    assert_eq!(REMOTE_ACCESS_UNITS.len(), 3);
    Ok(())
}

#[test]
fn brand_parameterized_boot_service_units_and_protection_follow_brand_config() -> Result<(), String>
{
    let plan = sample_plan("/home/legion/data").with_install_bin_dir("/opt/legion/bin");
    let unit = render_boot_service_systemd_unit_for_brand(&plan, &LEGION_CODE)
        .map_err(|e| e.to_string())?;
    assert!(unit.contains("Description=LegionCode server"));
    assert!(unit.contains("Environment=LEGIONCODE_HOME=/home/legion/data"));
    assert!(unit.contains("Environment=LEGIONCODE_BOOT_SERVICE_UNIT=legioncode.service"));
    assert_eq!(
        extract_service_unit_base_dir_for_brand(&unit, &LEGION_CODE).as_deref(),
        Some("/home/legion/data")
    );

    let plist =
        render_boot_service_launchd_plist_for_brand(&plan, "/Users/l", "/bin", &LEGION_CODE);
    assert!(plist.contains("dev.legioncode.service"));
    assert!(plist.contains("<key>LEGIONCODE_HOME</key>"));
    assert_eq!(
        extract_service_unit_base_dir_for_brand(&plist, &LEGION_CODE).as_deref(),
        Some("/home/legion/data")
    );

    let task_xml = render_boot_service_windows_task_xml_for_brand(
        &plan,
        "PC\\l",
        "C:\\l",
        "conhost.exe",
        &LEGION_CODE,
    );
    assert!(task_xml.contains("<Description>LegionCode server</Description>"));
    assert!(task_xml.contains("legioncode-task.xml"));

    let problem = format_boot_service_problem_for_brand(
        BootServiceProblem::ServiceStopped,
        "legioncode service install",
        "legioncode service restart",
        &LEGION_CODE,
    );
    assert!(problem.contains("legioncode.service"));

    let prot = build_service_protection_plan_for_brand(
        1000,
        "/home/l/.config/systemd/user",
        &[],
        Some("/tmp/sys"),
        &LEGION_CODE,
    );
    assert!(prot.user.iter().any(|d| d.unit == "legioncode.service"));
    let script =
        render_protection_privileged_script_for_brand(&prot, ProtectionMode::Apply, &LEGION_CODE);
    assert!(script.contains("90-legioncode-protect.conf"));
    assert!(script.contains("LEGIONCODE_PROTECT"));
    Ok(())
}

proptest! {
    #[test]
    fn proptest_utf16le_bom_and_unit_base_dir_roundtrip(
        segment in "[a-zA-Z0-9 _.%-]{1,24}",
    ) {
        let base_dir = format!("/home/u/{segment}");
        let plan = sample_plan(&base_dir);
        let unit = render_boot_service_systemd_unit(&plan).map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(extract_service_unit_base_dir(&unit), Some(base_dir.clone()));

        let plist = render_boot_service_launchd_plist(&plan, "/home/u", "/usr/bin");
        prop_assert_eq!(extract_service_unit_base_dir(&plist), Some(base_dir.clone()));

        let task_xml = render_boot_service_windows_task_xml(&plan, "PC\\u", "C:\\u", "conhost.exe");
        let bom = encode_windows_task_utf16le_bom(&task_xml);
        prop_assert_eq!(&decode_service_unit_bytes(&bom), &task_xml);
        prop_assert_eq!(extract_service_unit_base_dir(&task_xml), Some(base_dir));
    }
}
