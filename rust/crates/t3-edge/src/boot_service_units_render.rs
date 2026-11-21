//! OS-specific unit, plist, and Task Scheduler XML renderers (`bootService.ts:72-272`).

use crate::boot_service_units::{
    BOOT_SERVICE_LAUNCHD_LABEL, BOOT_SERVICE_PLIST_FILE, BOOT_SERVICE_TASK_FILE,
    BOOT_SERVICE_UNIT_FILE, BootServicePlan, BootServiceRenderError, SERVICE_UNIT_ENV,
};

fn escape_systemd_specifiers(value: &str) -> String {
    value.replace('%', "%%")
}

fn quote_systemd_value(value: &str) -> String {
    let escaped = escape_systemd_specifiers(value);
    if escaped
        .chars()
        .any(|c| c.is_whitespace() || matches!(c, '"' | '\'' | '\\'))
    {
        format!("\"{}\"", escaped.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        escaped
    }
}

fn is_systemd_unsafe_executable(plan: &BootServicePlan) -> bool {
    let exe = plan.program.first().map_or("", String::as_str);
    exe.chars()
        .any(|c| (c as u32) <= 0x1f || c == '\x7f' || matches!(c, '"' | '\'' | '\\'))
}

fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn quote_windows_argument(value: &str) -> String {
    let trailing_slashes = value.len() - value.trim_end_matches('\\').len();
    let mut out = String::with_capacity(value.len() + trailing_slashes + 2);
    out.push('"');
    out.push_str(value);
    for _ in 0..trailing_slashes {
        out.push('\\');
    }
    out.push('"');
    out
}

/// Renders a Linux `systemd` user unit matching `renderBootServiceUnit` in `bootService.ts:102-147`.
pub fn render_boot_service_systemd_unit(
    plan: &BootServicePlan,
) -> Result<String, BootServiceRenderError> {
    if is_systemd_unsafe_executable(plan) {
        return Err(BootServiceRenderError::UnsupportedBaseDir {
            base_dir: plan.base_dir.clone(),
        });
    }
    let exec_args: Vec<String> = plan
        .program
        .iter()
        .map(|p| quote_systemd_value(p))
        .collect();
    let mut lines = vec![
        "[Unit]".to_owned(),
        "Description=T3 Code server".to_owned(),
        "StartLimitIntervalSec=300".to_owned(),
        "StartLimitBurst=5".to_owned(),
        String::new(),
        "[Service]".to_owned(),
        "Type=simple".to_owned(),
        "WorkingDirectory=%h".to_owned(),
        format!(
            "Environment=T3CODE_HOME={}",
            quote_systemd_value(&plan.base_dir)
        ),
    ];
    if let Some(bin_dir) = plan.install_bin_dir.as_deref() {
        lines.push(format!(
            "Environment=T3CODE_INSTALL_BIN_DIR={}",
            quote_systemd_value(bin_dir)
        ));
    }
    lines.extend([
        format!("Environment={SERVICE_UNIT_ENV}={BOOT_SERVICE_UNIT_FILE}"),
        format!("ExecStart={}", exec_args.join(" ")),
        "KillMode=mixed".to_owned(),
        "OOMPolicy=continue".to_owned(),
        "Restart=always".to_owned(),
        "RestartSec=5".to_owned(),
        "CPUWeight=1000".to_owned(),
        "IOWeight=1000".to_owned(),
        "MemoryLow=1536M".to_owned(),
        "OOMScoreAdjust=100".to_owned(),
        "ManagedOOMPreference=avoid".to_owned(),
        format!(
            "StandardOutput=append:{}",
            escape_systemd_specifiers(&plan.log_path)
        ),
        format!(
            "StandardError=append:{}",
            escape_systemd_specifiers(&plan.log_path)
        ),
        String::new(),
        "[Install]".to_owned(),
        "WantedBy=default.target".to_owned(),
        String::new(),
    ]);
    Ok(lines.join("\n"))
}

/// Renders a macOS `launchd` user agent plist matching `renderBootServicePlist` in `bootService.ts:154-207`.
pub fn render_boot_service_launchd_plist(
    plan: &BootServicePlan,
    home_dir: &str,
    environment_path: &str,
) -> String {
    let mut lines = vec![
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>".to_owned(),
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">".to_owned(),
        "<plist version=\"1.0\">".to_owned(),
        "<dict>".to_owned(),
        "  <key>Label</key>".to_owned(),
        format!("  <string>{BOOT_SERVICE_LAUNCHD_LABEL}</string>"),
        "  <key>ProgramArguments</key>".to_owned(),
        "  <array>".to_owned(),
    ];
    for arg in &plan.program {
        lines.push(format!("    <string>{}</string>", escape_xml_text(arg)));
    }
    lines.extend([
        "  </array>".to_owned(),
        "  <key>EnvironmentVariables</key>".to_owned(),
        "  <dict>".to_owned(),
        "    <key>PATH</key>".to_owned(),
        format!("    <string>{}</string>", escape_xml_text(environment_path)),
        "    <key>T3CODE_HOME</key>".to_owned(),
        format!("    <string>{}</string>", escape_xml_text(&plan.base_dir)),
    ]);
    if let Some(bin_dir) = plan.install_bin_dir.as_deref() {
        lines.push("    <key>T3CODE_INSTALL_BIN_DIR</key>".to_owned());
        lines.push(format!("    <string>{}</string>", escape_xml_text(bin_dir)));
    }
    lines.extend([
        format!("    <key>{SERVICE_UNIT_ENV}</key>"),
        format!("    <string>{BOOT_SERVICE_PLIST_FILE}</string>"),
        "  </dict>".to_owned(),
        "  <key>WorkingDirectory</key>".to_owned(),
        format!("  <string>{}</string>", escape_xml_text(home_dir)),
        "  <key>RunAtLoad</key>".to_owned(),
        "  <true/>".to_owned(),
        "  <key>KeepAlive</key>".to_owned(),
        "  <true/>".to_owned(),
        "  <key>ThrottleInterval</key>".to_owned(),
        "  <integer>5</integer>".to_owned(),
        "  <key>ExitTimeOut</key>".to_owned(),
        "  <integer>90</integer>".to_owned(),
        "  <key>ProcessType</key>".to_owned(),
        "  <string>Interactive</string>".to_owned(),
        "  <key>StandardOutPath</key>".to_owned(),
        format!("  <string>{}</string>", escape_xml_text(&plan.log_path)),
        "  <key>StandardErrorPath</key>".to_owned(),
        format!("  <string>{}</string>", escape_xml_text(&plan.log_path)),
        "</dict>".to_owned(),
        "</plist>".to_owned(),
        String::new(),
    ]);
    lines.join("\n")
}

/// Renders a Windows Task Scheduler XML task definition matching `renderBootServiceTask` in `bootService.ts:214-272`.
pub fn render_boot_service_windows_task_xml(
    plan: &BootServicePlan,
    user_id: &str,
    home_dir: &str,
    conhost_path: &str,
) -> String {
    let mut raw_args: Vec<&str> = plan.program.iter().map(String::as_str).collect();
    raw_args.extend([
        "--base-dir",
        &plan.base_dir,
        "--log-file",
        &plan.log_path,
        "--service-unit",
        BOOT_SERVICE_TASK_FILE,
    ]);
    let launcher_arguments: Vec<String> =
        raw_args.into_iter().map(quote_windows_argument).collect();
    let escaped_user = escape_xml_text(user_id);

    [
        "<Task version=\"1.2\" xmlns=\"http://schemas.microsoft.com/windows/2004/02/mit/task\">",
        "  <RegistrationInfo>",
        "    <Description>T3 Code server</Description>",
        "  </RegistrationInfo>",
        "  <Triggers>",
        "    <LogonTrigger>",
        &format!("      <UserId>{escaped_user}</UserId>"),
        "    </LogonTrigger>",
        "    <TimeTrigger>",
        "      <StartBoundary>2000-01-01T00:00:00</StartBoundary>",
        "      <Repetition>",
        "        <Interval>PT1M</Interval>",
        "      </Repetition>",
        "    </TimeTrigger>",
        "  </Triggers>",
        "  <Principals>",
        "    <Principal id=\"Author\">",
        &format!("      <UserId>{escaped_user}</UserId>"),
        "      <LogonType>InteractiveToken</LogonType>",
        "      <RunLevel>LeastPrivilege</RunLevel>",
        "    </Principal>",
        "  </Principals>",
        "  <Settings>",
        "    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>",
        "    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>",
        "    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>",
        "    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>",
        "    <Priority>5</Priority>",
        "  </Settings>",
        "  <Actions Context=\"Author\">",
        "    <Exec>",
        &format!("      <Command>{}</Command>", escape_xml_text(conhost_path)),
        &format!(
            "      <Arguments>--headless {}</Arguments>",
            escape_xml_text(&launcher_arguments.join(" "))
        ),
        &format!(
            "      <WorkingDirectory>{}</WorkingDirectory>",
            escape_xml_text(home_dir)
        ),
        "    </Exec>",
        "  </Actions>",
        "</Task>",
        "",
    ]
    .join("\n")
}

/// Encodes a Windows Task Scheduler XML string as `UTF-16LE` prefixed with the `0xFF 0xFE` BOM
/// (`encodeServiceTask` in `serviceProtocol.ts:47-48`).
pub fn encode_windows_task_utf16le_bom(task_xml: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + task_xml.len() * 2);
    out.extend_from_slice(&[0xff, 0xfe]);
    for unit in task_xml.encode_utf16() {
        out.extend_from_slice(&unit.to_le_bytes());
    }
    out
}
