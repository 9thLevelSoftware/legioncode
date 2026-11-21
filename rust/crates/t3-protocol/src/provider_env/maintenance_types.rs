//! Types and built-in definitions for 15-tier provider CLI maintenance resolution.
//!
//! Mirrors `packages/provider-core/src/server/maintenance.ts` and
//! `packages/provider-core/src/server/maintenanceResolver.ts`.

use std::collections::BTreeSet;

use t3_contracts::ids::ProviderDriverKind;

/// Package manager or native updater owning a provider CLI installation.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MaintenanceInstaller {
    /// Windows `WinGet` package.
    WinGet,
    /// Windows Chocolatey package.
    Chocolatey,
    /// `mise` runtime version manager.
    Mise,
    /// Nix store derivation (externally managed).
    Nix,
    /// macOS `MacPorts` port.
    MacPorts,
    /// Provider's own native updater at a known standalone binary location.
    NativeInstaller,
    /// Vite+ global JS package store (`vp i -g`).
    VitePlusGlobal,
    /// Bun global JS package store (`bun i -g`; exact pin disabled per BUG-14).
    BunGlobal,
    /// pnpm global JS package store (`pnpm add -g`).
    PnpmGlobal,
    /// Yarn global JS package store (`yarn global add`).
    YarnGlobal,
    /// Volta toolchain manager (`volta install`).
    Volta,
    /// npm global prefix installation (`npm install -g [--prefix ...]`).
    NpmGlobal {
        /// Optional resolved npm global prefix directory.
        prefix: Option<String>,
    },
    /// macOS/Linux Homebrew formula or cask (`brew upgrade`).
    Homebrew,
    /// Provider native update command used as fallback.
    NativeFallback,
    /// External/unknown installation requiring manual update instructions.
    Manual,
}

/// Ordered tier trace steps in [`super::resolve_provider_maintenance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MaintenanceTier {
    /// Tier 1: Vite+ `system_first` shim unwrap.
    VitePlusSystemFirst,
    /// Tier 2: Windows `WinGet`.
    WinGet,
    /// Tier 3: Windows Chocolatey.
    Chocolatey,
    /// Tier 4: `mise` active latest install.
    Mise,
    /// Tier 5: Nix store derivation.
    Nix,
    /// Tier 6: `MacPorts`.
    MacPorts,
    /// Tier 7: Standalone native installer binary.
    NativeInstaller,
    /// Tier 8: Vite+ global JS package manager.
    VitePlusGlobal,
    /// Tier 9: Bun global JS package manager.
    BunGlobal,
    /// Tier 10: pnpm global JS package manager.
    PnpmGlobal,
    /// Tier 11: Yarn global JS package manager.
    YarnGlobal,
    /// Tier 12: Volta package manager.
    Volta,
    /// Tier 13: npm global prefix installation.
    NpmGlobal,
    /// Tier 14: Homebrew formula or cask.
    Homebrew,
    /// Tier 15: Native fallback or manual instructions.
    Fallback,
}

/// Discovered external ownership or install-location probe flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MaintenanceProbe {
    /// Windows `WinGet` owns the installed provider package.
    WinGet,
    /// Windows Chocolatey owns the installed provider package.
    Chocolatey,
    /// `mise` actively tracks the `latest` version.
    MiseActiveLatest,
    /// `MacPorts` owns the resolved binary.
    MacPorts,
    /// Resolved binary matches a known standalone native installer location.
    NativeBinaryPath,
    /// Homebrew owns the installed formula or cask.
    Homebrew,
}

/// Vite+ binary shim resolution mode configured in user environment.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MaintenanceShimMode {
    /// No Vite+ shim mode active.
    #[default]
    Disabled,
    /// Vite+ manages the active binary inside `.vite-plus`.
    Managed,
    /// Vite+ shim forwards to an underlying system binary when unwrapped.
    SystemFirst,
}

/// Concrete CLI invocation for upgrading or pinning a provider binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateCommandSpec {
    /// Executable name or path (for example `"npm"`, `"pnpm"`, `"bun"`, `"grok"`).
    pub executable: String,
    /// Arguments passed to [`Self::executable`].
    pub args: Vec<String>,
}

/// Provider-specific package metadata consumed by [`super::resolve_provider_maintenance`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderMaintenanceDefinition {
    /// Canonical provider slug (for example `"grok"`, `"claudeAgent"`, `"codex"`).
    pub provider: String,
    /// npm package specifier when distributed as a JS CLI (for example `"@xai-official/grok"`).
    pub npm_package_name: Option<String>,
    /// Extra `--allow-build=<pkg>` package flags passed to `pnpm add -g`.
    pub pnpm_allow_build: Vec<String>,
    /// Windows `WinGet` package identifier, if published there.
    pub winget_package_id: Option<String>,
    /// Windows Chocolatey package name, if published there.
    pub choco_package_name: Option<String>,
    /// `mise` tool identifier, if available.
    pub mise_package: Option<String>,
    /// Whether `mise upgrade` may update the tool automatically.
    pub mise_upgrade_allowed: bool,
    /// `MacPorts` port name, if packaged.
    pub macports_port: Option<String>,
    /// Homebrew formula name, if packaged as formula.
    pub brew_formula: Option<String>,
    /// Homebrew cask name, if packaged as cask.
    pub brew_cask: Option<String>,
    /// True when Homebrew distribution is cask-only (`brew upgrade --cask`).
    pub homebrew_cask_only: bool,
    /// Subcommand arguments for native self-update (for example `["update"]`).
    pub native_update_args: Option<Vec<String>>,
    /// True if native update requires resolving a non-JS binary path first.
    pub requires_command_path: bool,
    /// Optional command template displayed when [`MaintenanceInstaller::Manual`] applies.
    pub manual_hint: Option<String>,
}

impl ProviderMaintenanceDefinition {
    /// Built-in maintenance definition for `grok` (BUG-13 JS package managers supported).
    pub fn grok() -> Self {
        Self {
            provider: "grok".to_owned(),
            npm_package_name: Some("@xai-official/grok".to_owned()),
            pnpm_allow_build: Vec::new(),
            winget_package_id: None,
            choco_package_name: None,
            mise_package: Some("grok".to_owned()),
            mise_upgrade_allowed: true,
            macports_port: None,
            brew_formula: None,
            brew_cask: Some("grok".to_owned()),
            homebrew_cask_only: true,
            native_update_args: Some(vec!["update".to_owned()]),
            requires_command_path: false,
            manual_hint: Some(
                "Install or update Grok via npm/pnpm/bun/brew or grok update".to_owned(),
            ),
        }
    }

    /// Built-in maintenance definition for `claudeAgent` (`claude`).
    pub fn claude_agent() -> Self {
        Self {
            provider: "claudeAgent".to_owned(),
            npm_package_name: Some("@anthropic-ai/claude-code".to_owned()),
            pnpm_allow_build: Vec::new(),
            winget_package_id: Some("Anthropic.ClaudeCode".to_owned()),
            choco_package_name: Some("claude-code".to_owned()),
            mise_package: Some("claude".to_owned()),
            mise_upgrade_allowed: true,
            macports_port: None,
            brew_formula: None,
            brew_cask: Some("claude-code".to_owned()),
            homebrew_cask_only: true,
            native_update_args: Some(vec!["update".to_owned()]),
            requires_command_path: false,
            manual_hint: Some("Run `claude update` or update via your package manager".to_owned()),
        }
    }

    /// Built-in maintenance definition for `codex`.
    pub fn codex() -> Self {
        Self {
            provider: "codex".to_owned(),
            npm_package_name: Some("@openai/codex".to_owned()),
            pnpm_allow_build: Vec::new(),
            winget_package_id: None,
            choco_package_name: None,
            mise_package: Some("codex".to_owned()),
            mise_upgrade_allowed: true,
            macports_port: None,
            brew_formula: None,
            brew_cask: Some("codex".to_owned()),
            homebrew_cask_only: true,
            native_update_args: None,
            requires_command_path: true,
            manual_hint: Some("Update @openai/codex via your package manager".to_owned()),
        }
    }

    /// Looks up a built-in maintenance definition by [`ProviderDriverKind`] when available.
    pub fn for_driver(kind: &ProviderDriverKind) -> Option<Self> {
        match kind.as_str() {
            "grok" => Some(Self::grok()),
            "claudeAgent" => Some(Self::claude_agent()),
            "codex" => Some(Self::codex()),
            _ => None,
        }
    }
}

/// Environment & probe inputs evaluated across the 15-tier maintenance precedence.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MaintenanceResolutionInput {
    /// Executable path discovered on `PATH` or configured path override.
    pub resolved_binary_path: Option<String>,
    /// Symlink-resolved target path of [`Self::resolved_binary_path`].
    pub resolved_target_path: Option<String>,
    /// Vite+ shim configuration mode.
    pub vite_plus_shim_mode: MaintenanceShimMode,
    /// Unwrapped system binary path when Vite+ `system_first` shim is active.
    pub vite_plus_unwrapped_path: Option<String>,
    /// Symlink-resolved target path of [`Self::vite_plus_unwrapped_path`].
    pub vite_plus_unwrapped_target_path: Option<String>,
    /// Whether host platform is Windows.
    pub is_windows: bool,
    /// Discovered external ownership or install-path probes.
    pub ownership_probes: BTreeSet<MaintenanceProbe>,
    /// Explicit global prefix configured/discovered for npm.
    pub npm_global_prefix: Option<String>,
    /// Additional `.vite-plus` directories to recognize.
    pub vite_plus_global_dirs: Vec<String>,
    /// Additional `.bun` global install directories to recognize.
    pub bun_global_dirs: Vec<String>,
    /// Additional `.pnpm` global store directories to recognize.
    pub pnpm_global_dirs: Vec<String>,
    /// Additional `.yarn` global directories to recognize.
    pub yarn_global_dirs: Vec<String>,
    /// Additional `.volta` shim/install directories to recognize.
    pub volta_dirs: Vec<String>,
}

impl MaintenanceResolutionInput {
    /// Returns `true` when `probe` is recorded in [`Self::ownership_probes`].
    pub fn has_probe(&self, probe: MaintenanceProbe) -> bool {
        self.ownership_probes.contains(&probe)
    }
}

/// Resolved maintenance plan and pin capability metadata for a provider CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaintenanceCapabilities {
    /// Winning installer category.
    pub installer: MaintenanceInstaller,
    /// Exact tier in the 15-tier precedence order that selected [`Self::installer`].
    pub resolved_tier: MaintenanceTier,
    /// True when Vite+ `system_first` shim unwrapping ran at Tier 1.
    pub shim_unwrapped: bool,
    /// Whether this installer supports `@<version>` exact pinning (`false` for `bun-global`).
    pub supports_exact_pin: bool,
    /// Effective install path inspected after Tier 1 unwrapping, if any.
    pub install_path: Option<String>,
    /// Mutex lock key preventing overlapping mutations on the same package store.
    pub lock_key: String,
    /// Concrete command to run for default update, if automated update is supported.
    pub update_command: Option<UpdateCommandSpec>,
    /// Human-readable fallback guidance when automated update is unavailable.
    pub manual_command: Option<String>,
}
