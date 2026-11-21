//! Network release resolution, SHA-256 verification, and pinned archive extraction for `t3 update`.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest as _, Sha256};
use t3_persistence::service_launcher::{is_exact_service_version, runtime_entry_paths};

use crate::cmd_install::write_launcher_file;
use crate::invocation::non_empty_env;

const RELEASE_REPO: &str = "pingdotgg/t3code";
const DEFAULT_RELEASE_DOWNLOAD_BASE: &str = "https://github.com/pingdotgg/t3code/releases/download";
const INDEX_MAX_PAGES: usize = 10;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReleaseChannel {
    Stable,
    Nightly,
    Preview,
}

impl ReleaseChannel {
    pub(crate) fn parse(raw: &str) -> Option<Self> {
        match raw {
            "stable" => Some(Self::Stable),
            "nightly" => Some(Self::Nightly),
            "preview" => Some(Self::Preview),
            _ => None,
        }
    }

    pub(crate) fn of_version(version: &str) -> Self {
        if version.contains("-nightly.") {
            Self::Nightly
        } else if version.contains("-preview.") {
            Self::Preview
        } else {
            Self::Stable
        }
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
            Self::Preview => "preview",
        }
    }
}

pub(crate) fn resolve_newest_release(channel: ReleaseChannel) -> Result<String, String> {
    let token = non_empty_env("GH_TOKEN").or_else(|| non_empty_env("GITHUB_TOKEN"));
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Could not initialize HTTP runtime: {err}"))?;

    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!("t3-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| format!("Could not initialize HTTP client: {err}"))?;

        let mut best: Option<String> = None;
        for page in 1..=INDEX_MAX_PAGES {
            let url = format!(
                "https://api.github.com/repos/{RELEASE_REPO}/releases?per_page=100&page={page}"
            );
            let mut req = client
                .get(&url)
                .header("Accept", "application/vnd.github+json");
            if let Some(ref tok) = token {
                req = req.header("Authorization", format!("Bearer {tok}"));
            }
            let resp = req
                .send()
                .await
                .map_err(|_| "Could not list t3 releases.".to_owned())?;
            if !resp.status().is_success() {
                return Err("Could not list t3 releases.".to_owned());
            }
            let text = resp
                .text()
                .await
                .map_err(|_| "Could not list t3 releases.".to_owned())?;
            let entries: Vec<GitHubRelease> = serde_json::from_str(&text)
                .map_err(|_| "The t3 release index had an unexpected shape.".to_owned())?;
            if entries.is_empty() {
                break;
            }
            for rel in &entries {
                if rel.draft {
                    continue;
                }
                let ver = rel.tag_name.strip_prefix('v').unwrap_or(&rel.tag_name);
                if !is_exact_service_version(ver) || ReleaseChannel::of_version(ver) != channel {
                    continue;
                }
                let replace = best.as_deref().is_none_or(|cur| {
                    t3_persistence::service_launcher::compare_exact_service_versions(ver, cur)
                        == std::cmp::Ordering::Greater
                });
                if replace {
                    best = Some(ver.to_owned());
                }
            }
            if best.is_some() {
                return Ok(best.unwrap_or_default());
            }
        }
        best.ok_or_else(|| {
            format!(
                "No published release found on the {} channel.",
                channel.label()
            )
        })
    })
}

pub(crate) fn install_target_version(
    base_dir: &Path,
    target_version: &str,
    release_base_override: Option<&str>,
) -> Result<PathBuf, String> {
    let (entry_path, sentinel_path) = runtime_entry_paths(base_dir, target_version);
    if t3_persistence::service_launcher::runtime_exists(base_dir, target_version) {
        return Ok(entry_path);
    }

    let plat_key = cli_archive_platform_key()?;
    let ext = if plat_key.starts_with("win32") {
        "zip"
    } else {
        "tar.gz"
    };
    let archive_name = format!("t3-{target_version}-{plat_key}.{ext}");
    let base = release_base_override
        .map(|s| s.trim_end_matches('/'))
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_RELEASE_DOWNLOAD_BASE);
    let checksums_url = format!("{base}/v{target_version}/SHA256SUMS");
    let archive_url = format!("{base}/v{target_version}/{archive_name}");

    let (checksums_text, archive_bytes) = fetch_release_pair(&checksums_url, &archive_url)?;
    let checksums = parse_checksums(&checksums_text);
    let expected = checksums.get(&archive_name).ok_or_else(|| {
        format!("SHA256SUMS for t3@{target_version} did not list {archive_name}.")
    })?;
    let actual = format!("{:x}", Sha256::digest(&archive_bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(format!(
            "Checksum mismatch for {archive_name}: expected {expected}, got {actual}."
        ));
    }

    let version_dir = entry_path
        .parent()
        .ok_or_else(|| "Invalid runtime entry path.".to_owned())?;
    let parent_versions = version_dir
        .parent()
        .ok_or_else(|| "Invalid runtime versions directory.".to_owned())?;
    fs::create_dir_all(parent_versions)
        .map_err(|err| format!("Could not create versions directory: {err}"))?;
    let staging = parent_versions.join(format!(".staging-{target_version}-{}", std::process::id()));
    if staging.exists() {
        let _ = fs::remove_dir_all(&staging);
    }
    fs::create_dir_all(&staging)
        .map_err(|err| format!("Could not create staging directory: {err}"))?;

    let archive_file = staging.join(&archive_name);
    fs::write(&archive_file, &archive_bytes)
        .map_err(|err| format!("Could not write downloaded archive: {err}"))?;

    let tar_cmd = if cfg!(windows) {
        let sys_root = non_empty_env("SystemRoot")
            .or_else(|| non_empty_env("windir"))
            .unwrap_or_else(|| "C:\\Windows".to_owned());
        format!("{sys_root}\\System32\\tar.exe")
    } else {
        "tar".to_owned()
    };
    let status = Command::new(&tar_cmd)
        .args([
            "-xf",
            &archive_file.to_string_lossy(),
            "-C",
            &staging.to_string_lossy(),
        ])
        .status()
        .map_err(|err| format!("Could not run `{tar_cmd}` to extract archive: {err}"))?;
    if !status.success() {
        let _ = fs::remove_dir_all(&staging);
        return Err(format!(
            "Failed to extract {archive_name} (exit code {}).",
            status.code().unwrap_or(1)
        ));
    }
    let _ = fs::remove_file(&archive_file);

    if version_dir.exists() {
        let _ = fs::remove_dir_all(version_dir);
    }
    fs::rename(&staging, version_dir)
        .map_err(|err| format!("Could not move staged version into place: {err}"))?;

    validate_extracted_binary(&entry_path, target_version)?;
    fs::write(&sentinel_path, format!("{target_version}\n"))
        .map_err(|err| format!("Could not write runtime sentinel: {err}"))?;
    repoint_recorded_launchers(base_dir, &entry_path);

    Ok(entry_path)
}

fn fetch_release_pair(checksums_url: &str, archive_url: &str) -> Result<(String, Vec<u8>), String> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| format!("Could not initialize HTTP runtime: {err}"))?;
    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(DOWNLOAD_TIMEOUT)
            .user_agent(concat!("t3-cli/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|err| format!("Could not initialize HTTP client: {err}"))?;

        let sums_resp =
            client.get(checksums_url).send().await.map_err(|err| {
                format!("Could not download checksums from {checksums_url}: {err}")
            })?;
        if !sums_resp.status().is_success() {
            return Err(format!(
                "Could not download checksums from {checksums_url} (HTTP {}).",
                sums_resp.status()
            ));
        }
        let sums = sums_resp
            .text()
            .await
            .map_err(|err| format!("Could not read checksums response: {err}"))?;

        let bin_resp = client
            .get(archive_url)
            .send()
            .await
            .map_err(|err| format!("Could not download archive from {archive_url}: {err}"))?;
        if !bin_resp.status().is_success() {
            return Err(format!(
                "Could not download archive from {archive_url} (HTTP {}).",
                bin_resp.status()
            ));
        }
        let bytes = bin_resp
            .bytes()
            .await
            .map_err(|err| format!("Could not read archive response: {err}"))?;

        Ok((sums, bytes.to_vec()))
    })
}

fn cli_archive_platform_key() -> Result<&'static str, String> {
    match (env::consts::OS, env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64"),
        ("linux", "aarch64") => Ok("linux-arm64"),
        ("linux", "x86_64") => Ok("linux-x64"),
        ("windows", "aarch64") => Ok("win32-arm64"),
        ("windows", "x86_64") => Ok("win32-x64"),
        (os, arch) => Err(format!("No CLI archive is published for {os}-{arch}.")),
    }
}

fn parse_checksums(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        let Some((hash, rest)) = trimmed.split_once(char::is_whitespace) else {
            continue;
        };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let file = rest.trim().trim_start_matches('*');
        if !file.is_empty() {
            map.insert(file.to_owned(), hash.to_ascii_lowercase());
        }
    }
    map
}

fn validate_extracted_binary(entry_path: &Path, expected_version: &str) -> Result<(), String> {
    let out = Command::new(entry_path)
        .arg("--version")
        .output()
        .map_err(|err| format!("Failed to verify downloaded t3@{expected_version}: {err}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() || !stdout.trim().ends_with(expected_version) {
        return Err(format!(
            "Downloaded binary {} did not report t3 v{expected_version}.",
            entry_path.display()
        ));
    }
    Ok(())
}

fn repoint_recorded_launchers(base_dir: &Path, new_entry: &Path) {
    let launchers_file = base_dir.join("runtime").join("launchers");
    let Ok(contents) = fs::read_to_string(launchers_file) else {
        return;
    };
    for line in contents.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let launcher = Path::new(line);
        if launcher.exists() {
            let _ = write_launcher_file(launcher, new_entry);
        }
    }
}
