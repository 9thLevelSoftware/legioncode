//! Tests for contract-aligned `ServerProviderSnapshot`, default model catalogs, and CLI discovery specs.

use legion_protocol::provider_env::SanitizedEnv;
use legion_providers::core::{
    ProviderDiscoveryOutcome, ServerProviderAuthStatus, ServerProviderState,
    build_server_provider_snapshot,
};
use legion_providers::drivers::{
    DISCOVERY_CLI_DRIVERS, ProviderProbeStatus, builtin_cli_probe_spec, extract_cli_version,
    find_builtin_driver_spec,
};

#[test]
fn server_provider_snapshot_and_builtin_cli_specs_align_with_contracts() {
    assert_eq!(
        find_builtin_driver_spec("museCode").map(|s| s.slug),
        Some("muse")
    );
    assert_eq!(
        find_builtin_driver_spec("replay-dev").map(|s| s.slug),
        Some("devReplay")
    );

    let env = SanitizedEnv::from_entries([("CLAUDECODE", "1")], false);
    for slug in DISCOVERY_CLI_DRIVERS {
        let probe = builtin_cli_probe_spec(&env, slug).expect("CLI driver must yield probe spec");
        assert_ne!(probe.executable, "");
    }
    assert!(builtin_cli_probe_spec(&env, "devReplay").is_none());

    assert_eq!(
        extract_cli_version("codex-cli v0.42.1-beta"),
        Some("0.42.1-beta".to_owned())
    );

    let codex_spec = find_builtin_driver_spec("codex").expect("codex spec");
    let snap = build_server_provider_snapshot(
        codex_spec,
        ProviderDiscoveryOutcome {
            instance_id: "codex".to_owned(),
            probe_status: ProviderProbeStatus::Ready,
            version: Some("0.42.0".to_owned()),
            checked_at: "2026-11-22T00:00:00Z".to_owned(),
            message: None,
        },
    );
    assert_eq!(snap.status, ServerProviderState::Ready);
    assert_eq!(snap.auth.status, ServerProviderAuthStatus::Authenticated);
    assert!(
        snap.models
            .iter()
            .any(|m| m.is_default == Some(true) && m.capabilities.is_some())
    );

    let json = serde_json::to_value(&snap).expect("serialize ServerProviderSnapshot");
    assert_eq!(json["instanceId"], "codex");
    assert_eq!(json["driver"], "codex");
    assert_eq!(json["installedVersion"], "0.42.0");
    assert_eq!(json["models"][0]["isCustom"], false);
}
