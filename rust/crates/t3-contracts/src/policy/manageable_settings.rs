//! Manageable `ServerSettings` paths and built-in defaults (`managedSettings.ts:282-309` & `cli/policy.ts:247-253`).

use serde_json::{Value, json};

fn push_general_settings(entries: &mut Vec<(&'static str, Option<Value>)>) {
    entries.extend([
        ("addProjectBaseDirectory", Some(json!(""))),
        ("autoResumeLimitedThreads", Some(json!(false))),
        ("automaticGitFetchInterval", Some(json!(30_000))),
        ("backgroundActivity.baseProfile", None),
        (
            "backgroundActivity.overrides.automaticGitFetchInterval",
            None,
        ),
        (
            "backgroundActivity.overrides.hostPowerMonitorActiveInterval",
            None,
        ),
        (
            "backgroundActivity.overrides.hostPowerMonitorIdleInterval",
            None,
        ),
        ("backgroundActivity.overrides.pauseWhenClientLowPower", None),
        ("backgroundActivity.overrides.pauseWhenHostLocked", None),
        ("backgroundActivity.overrides.pauseWhenHostLowPower", None),
        ("backgroundActivity.overrides.pauseWhenOnBattery", None),
        (
            "backgroundActivity.overrides.providerHealthRefreshInterval",
            None,
        ),
        ("backgroundActivity.profile", Some(json!("balanced"))),
        ("backgroundActivity.schemaVersion", Some(json!(1))),
        ("backgroundActivityProfile", Some(json!("balanced"))),
        ("branchNameInstructions", Some(json!(""))),
        ("branchNamePrefix", Some(json!("t3"))),
        ("branchNamingMode", Some(json!("static"))),
        ("cloudSpeechCredentials.openai", Some(json!(""))),
        ("cloudSpeechCredentials.openrouter", Some(json!(""))),
        ("cloudSpeechCredentials.xai", Some(json!(""))),
        ("continueThreadsAfterServerUpdate", Some(json!(true))),
        ("defaultAppearance", Some(json!(""))),
        ("defaultAppearanceSetAt", Some(json!(""))),
        ("defaultAutoPull", Some(json!(false))),
        ("defaultModelSelection", Some(Value::Null)),
        ("defaultProjectScripts", Some(json!([]))),
        ("defaultRuntimeMode", Some(json!("full-access"))),
        ("defaultTheme", Some(json!(""))),
        ("defaultThemeSetAt", Some(json!(""))),
        ("defaultThreadEnvMode", None),
        ("deviceHosts", Some(json!([]))),
        ("deviceOnboardingCompleted", Some(json!(false))),
        ("easDeviceHosts", Some(json!([]))),
        ("enableAgentBrowserAccess", Some(json!(true))),
        ("enableAgentDeviceAccess", Some(json!(false))),
        ("enableDeviceSupport", Some(json!(false))),
        ("enablePreviewTunnel", Some(json!(true))),
        ("enableProviderUpdateChecks", Some(json!(true))),
        ("environmentIcon", Some(Value::Null)),
        ("fallbackModelSelection", Some(Value::Null)),
        ("fastWorktreeCheckout", Some(json!(false))),
        ("favorites", Some(Value::Null)),
        ("ghqCloneLayout", Some(json!(false))),
        ("guestSharingEnabled", Some(json!(false))),
        ("hiddenThreadTags", Some(json!([]))),
        ("hideTaggedThreads", Some(json!(false))),
        ("idleCompaction", None),
        ("keepAwakeWhileAgentsWork", Some(json!(true))),
        ("linkedEnvironmentsEnabled", Some(json!(false))),
        ("newWorktreesStartFromOrigin", Some(json!(true))),
        ("offeredSourceControlProviders", Some(json!({}))),
        ("previousWorktreesDirectories", Some(json!([]))),
    ]);
}

fn push_feature_settings(entries: &mut Vec<(&'static str, Option<Value>)>) {
    let snooze_presets = json!([
        { "kind": "after", "minutes": 60 },
        { "kind": "after", "minutes": 180 },
        { "kind": "at", "day": "today", "time": "18:00" },
        { "kind": "at", "day": "tomorrow", "time": "09:00" },
        { "kind": "at", "day": "monday", "time": "09:00" },
    ]);
    let reasoning_opts = json!([{ "id": "reasoningEffort", "value": "low" }]);
    entries.extend([
        ("projectSettingsFolded", Some(json!(false))),
        ("promptSuggestionsEnabled", Some(json!(false))),
        ("providerHealthRefreshInterval", Some(json!(300_000))),
        ("providerInstances", Some(json!({}))),
        ("providerModelPreferences", Some(Value::Null)),
        ("pullRequestMergeMethod", Some(Value::Null)),
        ("removeAgentCreditsOnMerge", Some(json!(false))),
        ("requireConnectDeviceApproval", Some(json!(false))),
        ("responseStreamingMode", Some(json!("paragraph"))),
        ("reviewGuideInstructions", Some(json!(""))),
        ("reviewGuideModelSelection", Some(Value::Null)),
        ("reviewGuidesEnabled", Some(json!(false))),
        ("secretProviders", Some(json!({}))),
        ("sidebarAutoSettleAfterDays", Some(json!(7))),
        ("sidebarAutoSettleKeepPullRequests", Some(json!(false))),
        ("sidebarAutoSettleOnMerge", Some(json!(true))),
        ("simlockDeviceHosts", Some(json!([]))),
        ("simlockDeviceHostsEnabled", Some(json!(false))),
        ("snoozeLimitedThreads", Some(json!(false))),
        ("snoozePresets", Some(snooze_presets)),
        ("sourceControlHosts", Some(json!({}))),
        ("sourceControlWriterModelSelection", Some(Value::Null)),
        ("sourceControlWritingStyle", Some(json!({}))),
        ("speechAcceleration", Some(json!("auto"))),
        (
            "speechModelId",
            Some(json!("handy-computer/parakeet-unified-en-0.6b-gguf")),
        ),
        ("speechModelUnloadTimeout", Some(json!("min_15"))),
        ("speechPostProcessingModelSelection", Some(Value::Null)),
        ("speechProjectCustomWords", Some(json!([]))),
        (
            "storageCleanup.browserArtifactsAfterDays",
            Some(Value::Null),
        ),
        ("storageCleanup.logsAfterDays", Some(Value::Null)),
        ("storageCleanup.worktreeAfterDays", Some(Value::Null)),
        (
            "storageCleanup.worktreeKeepWhen",
            Some(json!("uncommitted-changes")),
        ),
        ("storageCleanup.worktreeOnDelete", Some(json!(false))),
        ("storageCleanup.worktreeOnMerge", Some(json!(false))),
        ("storageCleanup.worktreeSettledAfterDays", Some(Value::Null)),
        ("storageCleanup.worktreeUnchanged", Some(json!(false))),
        (
            "textGenerationModelSelection.instanceId",
            Some(json!("codex")),
        ),
        (
            "textGenerationModelSelection.model",
            Some(json!("gpt-6-luna")),
        ),
        ("textGenerationModelSelection.options", Some(reasoning_opts)),
        ("textGenerators", Some(json!({}))),
        ("threadTitleInstructions", Some(json!(""))),
        ("usageHistoryRetentionDays", Some(json!(92))),
        ("usageLimitSources", Some(json!({}))),
        ("usageModelAliases", Some(json!({}))),
        ("usageModelFilter", None),
        ("usagePriceOverrides", Some(json!({}))),
        ("worktreeCleanup", Some(Value::Null)),
        ("worktreeSubmodules", Some(Value::Null)),
        ("worktreesDirectory", Some(json!(""))),
    ]);
}

/// Returns every manageable `ServerSettings` leaf dot path with its built-in JSON default (`None` when optional with no default), sorted lexicographically.
#[must_use]
pub fn list_manageable_settings() -> Vec<(&'static str, Option<Value>)> {
    let mut entries = Vec::with_capacity(110);
    push_general_settings(&mut entries);
    push_feature_settings(&mut entries);
    entries.sort_by_key(|(left, _)| *left);
    entries
}

/// Formats [`list_manageable_settings`] as `<path> = <json|(no default)>` lines for `t3 policy settings`.
#[must_use]
pub fn describe_manageable_settings() -> String {
    list_manageable_settings()
        .into_iter()
        .map(|(path, value)| match value {
            None => format!("{path} = (no default)"),
            Some(v) => format!(
                "{path} = {}",
                serde_json::to_string(&v).unwrap_or_else(|_| "(invalid)".to_owned())
            ),
        })
        .collect::<Vec<_>>()
        .join("\n")
}
