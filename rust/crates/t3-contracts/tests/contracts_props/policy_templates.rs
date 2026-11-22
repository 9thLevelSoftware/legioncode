use indexmap::IndexMap;
use proptest::prelude::*;
use serde_json::{Value, json};
use t3_contracts::policy::{
    LoadedPolicySummary, ManagedPolicyClient, ManagedPolicyDocument, ManagedPolicyLimits,
    ManagedPolicySetting, PolicyTemplateError, RunningServerState, RuntimeMode, SAMPLE_POLICY,
    describe_policy, describe_running_server, macos_profile, policy_fingerprint_12hex,
    sample_policy, windows_adml, windows_admx,
};

#[test]
fn policy_fingerprints_match_legacy_golden_digests() {
    let mut settings = IndexMap::new();
    settings.insert(
        "storageCleanup.logsAfterDays".to_owned(),
        ManagedPolicySetting {
            value: json!(14),
            locked: true,
        },
    );
    let sample_complex = ManagedPolicyDocument {
        schema_version: 1,
        organization: "Acme Engineering".to_owned(),
        support_url: Some("https://acme.example/it".to_owned()),
        settings: Some(settings),
        limits: Some(ManagedPolicyLimits {
            max_runtime_mode: Some(RuntimeMode::AutoAcceptEdits),
            clients: Some(vec![
                ManagedPolicyClient::Desktop,
                ManagedPolicyClient::LocalBrowser,
            ]),
            telemetry: Some(false),
            ..Default::default()
        }),
    };
    assert_eq!(
        policy_fingerprint_12hex(&sample_complex).expect("complex digest"),
        "becd3c42eb33"
    );

    let minimal = ManagedPolicyDocument {
        schema_version: 1,
        organization: "Minimal Org".to_owned(),
        support_url: None,
        settings: None,
        limits: None,
    };
    assert_eq!(
        policy_fingerprint_12hex(&minimal).expect("minimal digest"),
        "a95193de6477"
    );
    assert_eq!(
        policy_fingerprint_12hex(&sample_policy()).expect("cli sample digest"),
        "13ab1a3e32bc"
    );
    assert_eq!(
        policy_fingerprint_12hex(&SAMPLE_POLICY).expect("static sample digest"),
        "13ab1a3e32bc"
    );
}

#[test]
fn policy_serialization_preserves_exact_order_omits_none_and_denies_unknown() {
    let doc = ManagedPolicyDocument {
        schema_version: 1,
        organization: "Minimal Org".to_owned(),
        support_url: None,
        settings: None,
        limits: None,
    };
    assert_eq!(
        serde_json::to_string(&doc).expect("compact json"),
        r#"{"schemaVersion":1,"organization":"Minimal Org"}"#
    );

    let bad_root = r#"{"schemaVersion":1,"organization":"Org","unexpected":true}"#;
    assert!(serde_json::from_str::<ManagedPolicyDocument>(bad_root).is_err());

    let bad_limits = r#"{"schemaVersion":1,"organization":"Org","limits":{"extra":1}}"#;
    assert!(serde_json::from_str::<ManagedPolicyDocument>(bad_limits).is_err());

    let bad_version = r#"{"schemaVersion":2,"organization":"Org"}"#;
    assert!(serde_json::from_str::<ManagedPolicyDocument>(bad_version).is_err());

    let empty_clients = r#"{"schemaVersion":1,"organization":"Org","limits":{"clients":[]}}"#;
    assert!(serde_json::from_str::<ManagedPolicyDocument>(empty_clients).is_err());
}

#[test]
fn cli_policy_descriptions_match_ts_output() {
    let none_text = describe_policy("/etc/t3/policy.json", None).expect("describe none");
    assert_eq!(
        none_text,
        "No managed policy on this machine. Every setting belongs to the user.\n"
    );

    let text =
        describe_policy("/etc/t3/policy.json", Some(&SAMPLE_POLICY)).expect("describe sample");
    assert!(
        text.contains("Managed by Example Corp (/etc/t3/policy.json, fingerprint 13ab1a3e32bc).")
    );
    assert!(text.contains("No managed settings."));
    assert!(text.contains("\"maxRuntimeMode\": \"auto-accept-edits\""));

    assert!(describe_running_server(None, None).contains("No T3 Code server is running"));

    let running = RunningServerState {
        pid: 4242,
        started_at: "2026-04-01T00:00:00Z".to_owned(),
        managed_policy: Some(Some(LoadedPolicySummary {
            source: "/etc/t3/policy.json".to_owned(),
            organization: "Example Corp".to_owned(),
            fingerprint: "13ab1a3e32bc".to_owned(),
        })),
    };
    let installed = LoadedPolicySummary {
        source: "/etc/t3/policy.json".to_owned(),
        organization: "Example Corp".to_owned(),
        fingerprint: "13ab1a3e32bc".to_owned(),
    };
    assert_eq!(
        describe_running_server(Some(&running), Some(&installed)),
        "The running server (pid 4242, started 2026-04-01T00:00:00Z) loaded this policy."
    );
}

#[test]
fn windows_templates_and_macos_profile_generate_expected_artifacts() {
    let win_admx = windows_admx();
    let win_strings = windows_adml();
    assert!(win_admx.contains(r"SOFTWARE\Policies\T3Tools\T3Code"));
    assert!(win_admx.contains("MaxRuntimeMode"));
    assert!(win_admx.contains("AllowedDrivers"));
    assert!(win_admx.contains("PreviewTunnel"));
    assert!(win_strings.contains("T3 Code"));
    assert!(win_strings.contains("PreviewTunnel"));
    for &(_, label) in t3_contracts::policy::KNOWN_DRIVER_LABELS {
        assert!(win_strings.contains(label), "missing ADML label `{label}`");
    }
    assert_eq!(t3_contracts::policy::KNOWN_DRIVER_LABELS.len(), 11);

    let profile = macos_profile(&SAMPLE_POLICY, "profile-1", "payload-1").expect("plist");
    assert!(profile.contains("<string>com.t3tools.t3code</string>"));
    assert!(profile.contains("<string>Example Corp</string>"));
    assert!(profile.contains("<key>telemetry</key>"));
    assert!(profile.contains("<key>previewTunnel</key>"));
    assert!(profile.contains("<false />"));

    let mut null_settings = IndexMap::new();
    null_settings.insert(
        "ui.theme".to_owned(),
        ManagedPolicySetting {
            value: Value::Null,
            locked: false,
        },
    );
    let doc_with_null = ManagedPolicyDocument {
        schema_version: 1,
        organization: "Null Org".to_owned(),
        support_url: None,
        settings: Some(null_settings),
        limits: None,
    };
    let err = macos_profile(&doc_with_null, "p1", "p2").expect_err("null rejected in plist");
    assert!(matches!(err, PolicyTemplateError::NullInProfile { .. }));
}

proptest! {
    #[test]
    fn macos_profile_xml_escapes_special_chars_and_round_trips_policy(
        org_suffix in "[a-zA-Z0-9<>&\"']{1,24}",
        url_suffix in "[a-zA-Z0-9?&=_/]{1,16}"
    ) {
        let org = format!("Org {org_suffix}");
        let doc = ManagedPolicyDocument {
            schema_version: 1,
            organization: org.clone(),
            support_url: Some(format!("https://example.com/{url_suffix}")),
            settings: None,
            limits: None,
        };
        let encoded = serde_json::to_string(&doc).expect("encode doc");
        let decoded: ManagedPolicyDocument = serde_json::from_str(&encoded).expect("decode doc");
        prop_assert_eq!(&decoded, &doc);

        let plist = macos_profile(&doc, "uuid-1", "uuid-2").expect("generate profile");
        prop_assert!(plist.contains("<?xml version=\"1.0\""));
        prop_assert!(!plist.contains("<<"));
    }
}
