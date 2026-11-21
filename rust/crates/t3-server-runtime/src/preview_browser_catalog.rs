//! Managed Chromium (`chrome-headless-shell`) download catalog pinned to `158.0.8127.36` (`#39669`).

use sha2::{Digest, Sha256};

/// Managed preview Chromium version pinned with security fixes (`#39669`).
///
/// Never pin below `156.0.8078.70` so compromised intermediate roots remain rejected.
pub const MANAGED_CHROME_VERSION: &str = "158.0.8127.36";

/// Supported Chrome-for-Testing platform keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChromePlatform {
    Linux64,
    LinuxArm64,
    MacArm64,
    MacX64,
    Win32,
    Win64,
}

impl ChromePlatform {
    /// Platform archive slug used in Chrome-for-Testing download URLs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Linux64 => "linux64",
            Self::LinuxArm64 => "linux-arm64",
            Self::MacArm64 => "mac-arm64",
            Self::MacX64 => "mac-x64",
            Self::Win32 => "win32",
            Self::Win64 => "win64",
        }
    }

    /// Detects the compile-time target [`ChromePlatform`], or `None` if unsupported.
    pub const fn current() -> Option<Self> {
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            Some(Self::Linux64)
        }
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        {
            Some(Self::LinuxArm64)
        }
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            Some(Self::MacArm64)
        }
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        {
            Some(Self::MacX64)
        }
        #[cfg(all(target_os = "windows", target_arch = "x86"))]
        {
            Some(Self::Win32)
        }
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        {
            Some(Self::Win64)
        }
        #[cfg(not(any(
            all(target_os = "linux", target_arch = "x86_64"),
            all(target_os = "linux", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "aarch64"),
            all(target_os = "macos", target_arch = "x86_64"),
            all(target_os = "windows", target_arch = "x86"),
            all(target_os = "windows", target_arch = "x86_64"),
        )))]
        {
            None
        }
    }
}

/// Download metadata (exact archive size and SHA-256) for one platform.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChromeArchiveSpec {
    pub platform: ChromePlatform,
    pub version: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

impl ChromeArchiveSpec {
    /// Chrome-for-Testing headless shell zip URL.
    pub fn download_url(self) -> String {
        let p = self.platform.as_str();
        format!(
            "https://storage.googleapis.com/chrome-for-testing-public/{}/{}/chrome-headless-shell-{}.zip",
            self.version, p, p
        )
    }

    /// Verifies downloaded archive size and SHA-256 digest before extraction.
    pub fn verify_archive_bytes(self, archive: &[u8]) -> Result<(), String> {
        let actual_len = u64::try_from(archive.len()).unwrap_or(u64::MAX);
        if actual_len != self.bytes {
            return Err(format!(
                "Chrome archive size mismatch for {}: expected {} bytes, got {} bytes",
                self.platform.as_str(),
                self.bytes,
                actual_len
            ));
        }
        let digest = hex::encode(Sha256::digest(archive));
        if digest != self.sha256 {
            return Err(format!(
                "Chrome archive SHA-256 mismatch for {}: expected {}, got {}",
                self.platform.as_str(),
                self.sha256,
                digest
            ));
        }
        Ok(())
    }
}

/// Returns all pinned Chrome-for-Testing archive specs for [`MANAGED_CHROME_VERSION`].
pub const fn all_chrome_archive_specs() -> [ChromeArchiveSpec; 6] {
    [
        ChromeArchiveSpec {
            platform: ChromePlatform::Linux64,
            version: MANAGED_CHROME_VERSION,
            bytes: 132_087_165,
            sha256: "2d3a23017323c54fd457a678e4351f895c4076d8050e39d5aa6247f93448b73a",
        },
        ChromeArchiveSpec {
            platform: ChromePlatform::LinuxArm64,
            version: MANAGED_CHROME_VERSION,
            bytes: 129_938_239,
            sha256: "176ee5c55486c22de43128871189061365ea28e881fc96016fd18e7cb59437c8",
        },
        ChromeArchiveSpec {
            platform: ChromePlatform::MacArm64,
            version: MANAGED_CHROME_VERSION,
            bytes: 105_057_133,
            sha256: "c24d40a3786322ff3a5e3ac6eed162226cbaf07ebe468715407827226b0f8897",
        },
        ChromeArchiveSpec {
            platform: ChromePlatform::MacX64,
            version: MANAGED_CHROME_VERSION,
            bytes: 112_306_247,
            sha256: "e0c5d07d8f5bcc5bdd5ef370e7daf3770856b585fcc9eecf441c7e3bb2e3d1cf",
        },
        ChromeArchiveSpec {
            platform: ChromePlatform::Win32,
            version: MANAGED_CHROME_VERSION,
            bytes: 122_509_290,
            sha256: "c1fc009622363bccbc1e5f60be654b90fa7b70ad83e642875da1fcb147ae430d",
        },
        ChromeArchiveSpec {
            platform: ChromePlatform::Win64,
            version: MANAGED_CHROME_VERSION,
            bytes: 127_876_264,
            sha256: "9655950dfeddf565561cd64cfe8ee716ccd6482fa3798503b632219efc65a71e",
        },
    ]
}

/// Looks up the pinned [`ChromeArchiveSpec`] for `platform`.
pub const fn chrome_archive_spec(platform: ChromePlatform) -> ChromeArchiveSpec {
    match platform {
        ChromePlatform::Linux64 => all_chrome_archive_specs()[0],
        ChromePlatform::LinuxArm64 => all_chrome_archive_specs()[1],
        ChromePlatform::MacArm64 => all_chrome_archive_specs()[2],
        ChromePlatform::MacX64 => all_chrome_archive_specs()[3],
        ChromePlatform::Win32 => all_chrome_archive_specs()[4],
        ChromePlatform::Win64 => all_chrome_archive_specs()[5],
    }
}
