//! Windows Group Policy registry constants and policy element metadata.

/// Windows registry key holding T3 Code Group Policy / Intune settings.
pub const WINDOWS_POLICY_KEY: &str = r"HKLM\SOFTWARE\Policies\T3Tools\T3Code";

/// ADMX-relative registry key (hive selected by `class="Machine"`).
pub(super) const ADMX_KEY: &str = r"SOFTWARE\Policies\T3Tools\T3Code";

/// Registry value names written by Windows Group Policy template controls.
pub const WINDOWS_POLICY_VALUES: &[&str] = &[
    "Organization",
    "SupportUrl",
    "Settings",
    "MaxRuntimeMode",
    "Connect",
    "WebhookTasks",
    "Telemetry",
    "BrowserCookieImport",
    "ProviderInstances",
    "UpdatesMode",
    "UpdatesFeedUrl",
    "AllowNightly",
];

/// Registry list subkeys whose DWORD values represent enabled checklist entries.
pub const WINDOWS_POLICY_LISTS: &[&str] = &["Clients", "AllowedDrivers"];

/// Grouped policies whose enabled/disabled DWORD disambiguates child values.
pub const WINDOWS_POLICY_GROUPS: &[&str] = &["Clients", "AllowedDrivers", "Updates"];

/// Built-in provider driver identifiers and UI labels in `knownDriverLabels()` order.
pub const KNOWN_DRIVER_LABELS: &[(&str, &str)] = &[
    ("codex", "Codex"),
    ("claudeAgent", "Claude"),
    ("antigravity", "Antigravity"),
    ("cursor", "Cursor"),
    ("grok", "Grok"),
    ("fx", "fx"),
    ("kiro", "Kiro"),
    ("muse", "Muse Code"),
    ("opencode", "OpenCode"),
    ("pi", "Pi"),
    ("acpRegistry", "ACP Registry"),
];

pub(super) enum Element<'a> {
    Text {
        name: &'a str,
        label: &'a str,
        json: bool,
    },
    Enum {
        name: &'a str,
        label: &'a str,
        items: &'a [(&'a str, &'a str)],
        default_item: usize,
    },
    Checkbox {
        list: Option<&'a str>,
        name: &'a str,
        label: &'a str,
        default_checked: bool,
    },
}

pub(super) struct PolicyDef<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub help: &'a str,
    pub toggle: Option<&'a str>,
    pub elements: Option<Vec<Element<'a>>>,
}

const RUNTIME_MODES: &[(&str, &str)] = &[
    ("approval-required", "Supervised"),
    ("auto-accept-edits", "Auto-accept edits"),
    ("auto", "Auto"),
    ("full-access", "Full access"),
];

const UPDATES_ITEMS: &[(&str, &str)] = &[("auto", "Automatic"), ("off", "Off")];

fn core_policies() -> Vec<PolicyDef<'static>> {
    vec![
        PolicyDef {
            id: "Organization",
            title: "Organization",
            help: "Required whenever any other T3 Code policy is set: T3 Code will not start with policies that do not name an organization.\n\nUsers see the name as \"Managed by <organization>\" beside every control the policy locks. The support URL is optional.",
            toggle: None,
            elements: Some(vec![
                Element::Text {
                    name: "Organization",
                    label: "Organization name",
                    json: false,
                },
                Element::Text {
                    name: "SupportUrl",
                    label: "Support URL (optional)",
                    json: false,
                },
            ]),
        },
        PolicyDef {
            id: "MaxRuntimeMode",
            title: "Broadest permission mode",
            help: "The broadest permission mode any thread may use, including threads agents and scheduled tasks start.\n\nA thread already above it cannot run another turn until someone lowers its mode. Under this limit, each provider's launch settings (binary path, launch arguments, environment variables) come only from the policy.\n\nDisabled or Not Configured: no restriction.",
            toggle: None,
            elements: Some(vec![Element::Enum {
                name: "MaxRuntimeMode",
                label: "Broadest permission mode",
                items: RUNTIME_MODES,
                default_item: 1,
            }]),
        },
        PolicyDef {
            id: "Clients",
            title: "Allowed clients",
            help: "What may connect to T3 Code on this machine.\n\nWithout Remote, T3 Code only listens on this machine, does not create pairing links, signs out devices that paired earlier, and turns T3 Connect off. Check at least one client.\n\nDisabled or Not Configured: no restriction.",
            toggle: None,
            elements: Some(vec![
                Element::Checkbox {
                    list: Some("Clients"),
                    name: "desktop",
                    label: "Desktop app on this machine",
                    default_checked: true,
                },
                Element::Checkbox {
                    list: Some("Clients"),
                    name: "local-browser",
                    label: "A browser on this machine",
                    default_checked: true,
                },
                Element::Checkbox {
                    list: Some("Clients"),
                    name: "remote",
                    label: "Remote: phones, other computers, app.t3.codes",
                    default_checked: false,
                },
            ]),
        },
        PolicyDef {
            id: "Connect",
            title: "Allow T3 Connect",
            help: "Disabled turns T3 Connect off and unlinks this machine. Other remote access still follows Allowed clients.\n\nEnabled or Not Configured leaves T3 Connect available.",
            toggle: Some("Connect"),
            elements: None,
        },
        PolicyDef {
            id: "WebhookTasks",
            title: "Allow webhook tasks",
            help: "Disabled stops scheduled tasks from using webhook triggers. Existing webhook tasks are kept but do not run.",
            toggle: Some("WebhookTasks"),
            elements: None,
        },
        PolicyDef {
            id: "BrowserCookieImport",
            title: "Allow importing browser cookies",
            help: "Disabled stops users from importing cookies from another browser into T3 Code's browser.",
            toggle: Some("BrowserCookieImport"),
            elements: None,
        },
        PolicyDef {
            id: "Telemetry",
            title: "Allow usage data",
            help: "Disabled stops T3 Code from sending product usage data and relay connection traces. Enabled cannot turn them on for a user who turned them off.",
            toggle: Some("Telemetry"),
            elements: None,
        },
    ]
}

fn provider_and_update_policies() -> Vec<PolicyDef<'static>> {
    let driver_elements = KNOWN_DRIVER_LABELS
        .iter()
        .map(|&(driver, label)| Element::Checkbox {
            list: Some("AllowedDrivers"),
            name: driver,
            label,
            default_checked: driver == "codex" || driver == "claudeAgent",
        })
        .collect();

    vec![
        PolicyDef {
            id: "AllowedDrivers",
            title: "Allowed providers",
            help: "Only the checked providers may run. Threads on other providers stay, marked unavailable.\n\nDisabled or Not Configured: no restriction.",
            toggle: None,
            elements: Some(driver_elements),
        },
        PolicyDef {
            id: "Updates",
            title: "Updates",
            help: "Off stops T3 Code from checking for updates, for machines that receive T3 Code through device management.\n\nA feed URL makes the desktop app update from your own mirror of the release feed instead of GitHub; it must be https. Unchecking nightly makes the nightly channel unavailable.\n\nDisabled or Not Configured: no restriction.",
            toggle: None,
            elements: Some(vec![
                Element::Enum {
                    name: "UpdatesMode",
                    label: "Updates",
                    items: UPDATES_ITEMS,
                    default_item: 0,
                },
                Element::Text {
                    name: "UpdatesFeedUrl",
                    label: "Feed URL (optional)",
                    json: false,
                },
                Element::Checkbox {
                    list: None,
                    name: "AllowNightly",
                    label: "Allow the nightly channel",
                    default_checked: true,
                },
            ]),
        },
        PolicyDef {
            id: "Settings",
            title: "Managed settings (JSON)",
            help: "Defaults and locks for server settings, as the \"settings\" object of a T3 Code policy. Each key is a setting path; each value is {\"value\": ..., \"locked\": true|false}.\n\nExample: {\"enableAgentBrowserAccess\": {\"value\": false, \"locked\": true}}\n\nRun \"t3 policy check\" on a policy file with the same object to validate it first.",
            toggle: None,
            elements: Some(vec![Element::Text {
                name: "Settings",
                label: "Settings JSON",
                json: true,
            }]),
        },
        PolicyDef {
            id: "ProviderInstances",
            title: "Provider settings (JSON)",
            help: "Fields set on every instance of a provider, as the \"limits.providers.instances\" object of a T3 Code policy: environment variables, binaryPath, launchArgs.\n\nExample: {\"claudeAgent\": {\"locked\": true, \"environment\": [{\"name\": \"CLAUDE_CODE_USE_BEDROCK\", \"value\": \"1\"}]}}\n\nEvery user on the machine can read this value, so do not put credentials in it.",
            toggle: None,
            elements: Some(vec![Element::Text {
                name: "ProviderInstances",
                label: "Provider settings JSON",
                json: true,
            }]),
        },
    ]
}

pub(super) fn policies() -> Vec<PolicyDef<'static>> {
    let mut all = core_policies();
    all.extend(provider_and_update_policies());
    all
}
