//! Cross-platform native OS folder and file picker dialogs (`PowerShell`, `osascript`, `zenity`/`kdialog`).

use std::fs;
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Maximum theme file size (`256 KiB`) loaded directly into IPC memory (`ipc.ts:543`).
pub const MAX_THEME_FILE_BYTES: u64 = 256 * 1024;

/// Picked theme file metadata matching `PickedThemeFile` in `packages/contracts/src/ipc.ts`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PickedThemeFile {
    /// Filename without directory prefix.
    pub name: String,
    /// Total file size in bytes on disk.
    pub size: u64,
    /// File UTF-8 text (`""` when oversized beyond [`MAX_THEME_FILE_BYTES`]).
    pub text: String,
}

/// Opens a native OS folder picker dialog, returning `Ok(None)` cleanly on cancel.
pub fn pick_folder(initial_path: Option<&str>) -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        let script = r"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.FolderBrowserDialog
$dlg.ShowNewFolderButton = $true
if ($env:LEGION_PICKER_INIT -and (Test-Path -LiteralPath $env:LEGION_PICKER_INIT)) {
  $dlg.SelectedPath = $env:LEGION_PICKER_INIT
}
$owner = New-Object System.Windows.Forms.Form -Property @{ TopMost = $true; ShowInTaskbar = $false; Opacity = 0 }
$res = $dlg.ShowDialog($owner)
$owner.Dispose()
if ($res -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dlg.SelectedPath }
";
        run_windows_picker(script, initial_path).map(|mut paths| paths.pop())
    }
    #[cfg(target_os = "macos")]
    {
        let script = r#"on run argv
  try
    if (count of argv) > 0 then
      set p to choose folder default location POSIX file (item 1 of argv)
    else
      set p to choose folder
    end if
    return POSIX path of p
  on error number -128
    return ""
  end try
end run"#;
        run_macos_picker(script, initial_path).map(|mut paths| paths.pop())
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        run_linux_folder_picker(initial_path)
    }
}

/// Opens a native OS single-image picker dialog for a project favicon.
pub fn pick_project_favicon(initial_path: Option<&str>) -> Result<Option<String>, String> {
    #[cfg(target_os = "windows")]
    {
        let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.OpenFileDialog
$dlg.Multiselect = $false
$dlg.Filter = "Images|*.png;*.jpg;*.jpeg;*.webp;*.svg;*.ico;*.gif|All files|*.*"
if ($env:LEGION_PICKER_INIT -and (Test-Path -LiteralPath $env:LEGION_PICKER_INIT)) {
  $dlg.InitialDirectory = $env:LEGION_PICKER_INIT
}
$owner = New-Object System.Windows.Forms.Form -Property @{ TopMost = $true; ShowInTaskbar = $false; Opacity = 0 }
$res = $dlg.ShowDialog($owner)
$owner.Dispose()
if ($res -eq [System.Windows.Forms.DialogResult]::OK) { Write-Output $dlg.FileName }
"#;
        run_windows_picker(script, initial_path).map(|mut paths| paths.pop())
    }
    #[cfg(target_os = "macos")]
    {
        let script = r#"on run argv
  try
    if (count of argv) > 0 then
      set p to choose file of type {"public.image"} default location POSIX file (item 1 of argv)
    else
      set p to choose file of type {"public.image"}
    end if
    return POSIX path of p
  on error number -128
    return ""
  end try
end run"#;
        run_macos_picker(script, initial_path).map(|mut paths| paths.pop())
    }
    #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
    {
        run_linux_file_picker(false, initial_path).map(|mut paths| paths.pop())
    }
}

/// Opens a native OS multi-file picker for theme JSON/CSS files, reading safe bounded contents.
pub fn pick_theme_files() -> Result<Option<Vec<PickedThemeFile>>, String> {
    let paths: Vec<String> = {
        #[cfg(target_os = "windows")]
        {
            let script = r#"
Add-Type -AssemblyName System.Windows.Forms
$dlg = New-Object System.Windows.Forms.OpenFileDialog
$dlg.Multiselect = $true
$dlg.Filter = "Theme files|*.json;*.css;*.txt|All files|*.*"
$owner = New-Object System.Windows.Forms.Form -Property @{ TopMost = $true; ShowInTaskbar = $false; Opacity = 0 }
$res = $dlg.ShowDialog($owner)
$owner.Dispose()
if ($res -eq [System.Windows.Forms.DialogResult]::OK) { $dlg.FileNames | ForEach-Object { Write-Output $_ } }
"#;
            run_windows_picker(script, None)?
        }
        #[cfg(target_os = "macos")]
        {
            let script = r#"try
  set chosen to choose file with multiple selections allowed
  set out to ""
  repeat with f in chosen
    set out to out & (POSIX path of f) & linefeed
  end repeat
  return out
on error number -128
  return ""
end try"#;
            run_macos_picker(script, None)?
        }
        #[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
        {
            run_linux_file_picker(true, None)?
        }
    };

    if paths.is_empty() {
        return Ok(None);
    }
    read_picked_theme_files(&paths).map(Some)
}

/// Reads metadata and bounded UTF-8 contents (`<= 256 KiB`) for `paths`.
pub fn read_picked_theme_files(paths: &[String]) -> Result<Vec<PickedThemeFile>, String> {
    let mut files = Vec::with_capacity(paths.len());
    for raw_path in paths {
        let path = Path::new(raw_path);
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(raw_path)
            .to_owned();
        let meta = fs::metadata(path)
            .map_err(|err| format!("failed inspecting theme file '{}': {err}", path.display()))?;
        let size = meta.len();
        let text = if size <= MAX_THEME_FILE_BYTES {
            fs::read_to_string(path)
                .map_err(|err| format!("failed reading theme file '{}': {err}", path.display()))?
        } else {
            String::new()
        };
        files.push(PickedThemeFile { name, size, text });
    }
    Ok(files)
}

#[cfg(target_os = "windows")]
fn run_windows_picker(script: &str, initial_path: Option<&str>) -> Result<Vec<String>, String> {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-STA", "-Command", script]);
    if let Some(init) = initial_path.map(str::trim).filter(|s| !s.is_empty()) {
        cmd.env("LEGION_PICKER_INIT", init);
    }
    let out = cmd
        .output()
        .map_err(|err| format!("failed spawning PowerShell file picker: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "PowerShell picker failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(split_nonempty_lines(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(target_os = "macos")]
fn run_macos_picker(script: &str, initial_path: Option<&str>) -> Result<Vec<String>, String> {
    let mut cmd = Command::new("osascript");
    cmd.args(["-e", script]);
    if let Some(init) = initial_path.map(str::trim).filter(|s| !s.is_empty()) {
        cmd.arg(init);
    }
    let out = cmd
        .output()
        .map_err(|err| format!("failed spawning osascript picker: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "osascript picker failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    Ok(split_nonempty_lines(&String::from_utf8_lossy(&out.stdout)))
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn run_linux_folder_picker(initial_path: Option<&str>) -> Result<Option<String>, String> {
    let mut zenity = Command::new("zenity");
    zenity.args(["--file-selection", "--directory"]);
    if let Some(init) = initial_path.map(str::trim).filter(|s| !s.is_empty()) {
        zenity.arg(format!("--filename={init}/"));
    }
    if let Ok(out) = zenity.output() {
        if out.status.success() {
            return Ok(split_nonempty_lines(&String::from_utf8_lossy(&out.stdout)).pop());
        }
        return Ok(None);
    }
    let mut kdialog = Command::new("kdialog");
    kdialog.arg("--getexistingdirectory");
    if let Some(init) = initial_path.map(str::trim).filter(|s| !s.is_empty()) {
        kdialog.arg(init);
    }
    match kdialog.output() {
        Ok(out) if out.status.success() => {
            Ok(split_nonempty_lines(&String::from_utf8_lossy(&out.stdout)).pop())
        }
        _ => Ok(None),
    }
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
fn run_linux_file_picker(
    multiple: bool,
    initial_path: Option<&str>,
) -> Result<Vec<String>, String> {
    let mut zenity = Command::new("zenity");
    zenity.arg("--file-selection");
    if multiple {
        zenity.args(["--multiple", "--separator=\n"]);
    }
    if let Some(init) = initial_path.map(str::trim).filter(|s| !s.is_empty()) {
        zenity.arg(format!("--filename={init}"));
    }
    match zenity.output() {
        Ok(out) if out.status.success() => {
            Ok(split_nonempty_lines(&String::from_utf8_lossy(&out.stdout)))
        }
        _ => Ok(Vec::new()),
    }
}

fn split_nonempty_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}
