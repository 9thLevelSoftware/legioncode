//! Byte decoding and field extraction for systemd, launchd, and Windows Task Scheduler units.

fn unquote_systemd_value(raw: &str) -> String {
    let unquoted = if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        raw[1..raw.len() - 1]
            .replace("\\\"", "\"")
            .replace("\\\\", "\\")
    } else {
        raw.to_owned()
    };
    unquoted.replace("%%", "%")
}

fn unescape_xml_text(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn unquote_windows_argument(raw: &str) -> String {
    let unescaped = unescape_xml_text(raw);
    let trailing_slashes = unescaped.len() - unescaped.trim_end_matches('\\').len();
    let keep = unescaped.len() - (trailing_slashes / 2);
    unescaped[..keep].to_owned()
}

fn extract_plist_string_for_key(contents: &str, key_name: &str) -> Option<String> {
    let needle = format!("<key>{key_name}</key>");
    let start = contents.find(&needle)? + needle.len();
    let after_ws = contents[start..].trim_start();
    let rest = after_ws.strip_prefix("<string>")?;
    let end = rest.find("</string>")?;
    let inner = &rest[..end];
    if inner.contains('<') {
        return None;
    }
    Some(unescape_xml_text(inner))
}

fn extract_task_arguments(contents: &str) -> Option<&str> {
    let start = contents.find("<Arguments>")? + "<Arguments>".len();
    let tail = &contents[start..];
    let end = tail.find('<')?;
    Some(&tail[..end])
}

/// Decodes a boot-service unit file's raw bytes (`UTF-16LE` with BOM vs `UTF-8`, `serviceProtocol.ts:51-54`).
pub fn decode_service_unit_bytes(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] == 0xfe {
        let payload = &bytes[2..];
        let units: Vec<u16> = payload
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&pair| u16::from_le_bytes(pair))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    String::from_utf8_lossy(bytes).into_owned()
}

/// Returns `true` when Windows Task Scheduler `<Settings>` carries `<Enabled>false</Enabled>`.
pub fn service_task_is_disabled(task_xml: &str) -> bool {
    let Some(start) = task_xml.find("<Settings>") else {
        return false;
    };
    let tail = &task_xml[start..];
    let Some(end) = tail.find("</Settings>") else {
        return false;
    };
    tail[..end].contains("<Enabled>false</Enabled>")
}

/// Marks a Windows Task Scheduler XML definition disabled while leaving triggers intact.
pub fn disable_service_task(task_xml: &str) -> String {
    if service_task_is_disabled(task_xml) {
        return task_xml.to_owned();
    }
    task_xml.replacen(
        "  </Settings>",
        "    <Enabled>false</Enabled>\n  </Settings>",
        1,
    )
}

/// Extracts `T3CODE_HOME` or `T3CODE_INSTALL_BIN_DIR` from a systemd unit or launchd plist.
pub fn extract_service_unit_environment_value(contents: &str, name: &str) -> Option<String> {
    let prefix = format!("Environment={name}=");
    for line in contents.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(raw) = line.strip_prefix(&prefix) {
            return Some(unquote_systemd_value(raw.trim()));
        }
    }
    extract_plist_string_for_key(contents, name)
}

/// Extracts the served `T3CODE_HOME` / `--base-dir` from a systemd unit, launchd plist, or
/// Windows Task Scheduler XML (`serviceProtocol.ts:72-77`).
pub fn extract_service_unit_base_dir(contents: &str) -> Option<String> {
    if let Some(unix) = extract_service_unit_environment_value(contents, "T3CODE_HOME") {
        return Some(unix);
    }
    let args = extract_task_arguments(contents)?;
    let flag = " \"--base-dir\" \"";
    let start = args.find(flag)? + flag.len();
    let tail = &args[start..];
    let end = tail.find('"')?;
    Some(unquote_windows_argument(&tail[..end]))
}

/// Extracts the configured stdout/stderr log path from a systemd unit or launchd plist.
pub fn extract_service_unit_log_path(contents: &str) -> Option<String> {
    for line in contents.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(raw) = line.strip_prefix("StandardOutput=append:") {
            return Some(unquote_systemd_value(raw.trim()));
        }
    }
    extract_plist_string_for_key(contents, "StandardOutPath")
}

/// Extracts the primary launcher executable path from a systemd unit, launchd plist, or Task XML.
pub fn extract_service_unit_program(contents: &str) -> Option<String> {
    for line in contents.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(raw) = line.strip_prefix("ExecStart=") {
            return Some(extract_systemd_first_token(raw));
        }
    }
    if let Some(pos) = contents.find("<key>ProgramArguments</key>") {
        let after_key = contents[pos + "<key>ProgramArguments</key>".len()..].trim_start();
        if let Some(after_array) = after_key.strip_prefix("<array>") {
            let after_ws = after_array.trim_start();
            if let Some(rest) = after_ws.strip_prefix("<string>")
                && let Some(end) = rest.find("</string>")
                && !rest[..end].contains('<')
            {
                return Some(unescape_xml_text(&rest[..end]));
            }
        }
    }
    let args = extract_task_arguments(contents)?;
    let rest = args.strip_prefix("--headless \"")?;
    let end = rest.find('"')?;
    Some(unquote_windows_argument(&rest[..end]))
}

fn extract_systemd_first_token(raw: &str) -> String {
    if let Some(after_quote) = raw.strip_prefix('"') {
        let mut escaped = false;
        for (idx, ch) in after_quote.char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                return unquote_systemd_value(&raw[..=idx + 1]);
            }
        }
    }
    let end = raw.find(char::is_whitespace).unwrap_or(raw.len());
    unquote_systemd_value(&raw[..end])
}

/// Resolves the persistent user unit directory from systemd `UnitPath` entries (`serviceProtocol.ts:83-91`).
pub fn systemd_unit_directory_of(unit_paths: &[String]) -> Option<&str> {
    unit_paths.iter().find_map(|directory| {
        if directory.starts_with('/')
            && let Some(base) = directory.strip_suffix(".control")
            && base.ends_with("/systemd/user")
            && unit_paths.iter().any(|candidate| candidate == base)
        {
            Some(base)
        } else {
            None
        }
    })
}
