//! Integration tests covering Wave 3a Antigravity auth isolation, OAuth flow,
//! managed-release recovery, and `skills.json` discovery.

use legion_protocol::provider_env::SanitizedEnv;
use legion_providers::drivers::{
    AGY_CLI_DISABLE_AUTO_UPDATE_ENV, AntigravityAuthFlowPhase, AntigravityAuthMethod,
    AntigravityAuthState, AntigravityCliTokenStatus, AntigravityInstallTarget,
    AntigravityOAuthKind, ManagedReleaseIntegrity, build_antigravity_launch_spec_for_auth,
    clear_stale_auth_error, decode_go_keyring_secret, discover_antigravity_skills,
    evaluate_antigravity_cli_token, inspect_managed_release, parse_antigravity_authorization_url,
    resolve_antigravity_instance_directories, resolve_skills_config_path,
    sync_antigravity_user_rule_copies, validate_antigravity_login_response,
};
use serde_json::json;
use tempfile::tempdir;

#[test]
fn antigravity_auth_isolation_keyring_and_oauth_flow() {
    // 5283f33ca9: go-keyring-base64 & go-keyring-encoded decoding
    let tok_json = r#"{"token":{"access_token":"tok-1","refresh_token":"ref-1","expiry":"2026-11-21T20:00:00Z"}}"#;
    let b64_payload = b"go-keyring-base64:eyJ0b2tlbiI6eyJhY2Nlc3NfdG9rZW4iOiJ0b2stMSIsInJlZnJlc2hfdG9rZW4iOiJyZWYtMSIsImV4cGlyeSI6IjIwMjYtMTEtMjFUMjA6MDA6MDBaIn19";
    assert_eq!(
        decode_go_keyring_secret(b64_payload).ok().as_deref(),
        Some(tok_json)
    );
    let hex_payload = format!("go-keyring-encoded:{}", hex::encode(tok_json));
    assert_eq!(
        decode_go_keyring_secret(hex_payload.as_bytes())
            .ok()
            .as_deref(),
        Some(tok_json)
    );

    // 084ad5e1a0: oauth-personal isolates tokens from shared OS keyring
    let now_ms = 1_795_200_000_000_i64;
    let personal_missing = evaluate_antigravity_cli_token(
        None,
        Some(b64_payload),
        AntigravityAuthMethod::OauthPersonal,
        now_ms,
    );
    assert!(matches!(
        personal_missing,
        AntigravityCliTokenStatus::Missing { .. }
    ));
    let business_ok = evaluate_antigravity_cli_token(
        None,
        Some(b64_payload),
        AntigravityAuthMethod::OauthBusiness,
        now_ms,
    );
    assert!(matches!(
        business_ok,
        AntigravityCliTokenStatus::Available { .. }
    ));

    // Expired CLI token status
    let expired = evaluate_antigravity_cli_token(
        Some(r#"{"token":{"access_token":"tok-1","expiry":"2020-01-01T00:00:00Z"}}"#),
        None,
        AntigravityAuthMethod::OauthPersonal,
        now_ms,
    );
    assert!(matches!(expired, AntigravityCliTokenStatus::Expired { .. }));

    // 944d1820e2: OAuth loopback/code URL and response validation
    let auth_url = "https://accounts.google.com/o/oauth2/v2/auth?client_id=x&redirect_uri=http%3A%2F%2F127.0.0.1%3A41234%2F&state=nonce-abc";
    let pending = parse_antigravity_authorization_url(auth_url, AntigravityOAuthKind::Loopback)
        .expect("valid google loopback url");
    assert_eq!(
        validate_antigravity_login_response(
            &pending,
            "http://127.0.0.1:41234/?state=nonce-abc&code=auth-code-123"
        ),
        Some("auth-code-123".to_owned())
    );
    assert!(
        validate_antigravity_login_response(
            &pending,
            "http://127.0.0.1:41234/?state=wrong&code=auth-code-123"
        )
        .is_none()
    );

    // 320ab2a2ac: clear stale error outside active flow
    let cleared = clear_stale_auth_error(
        AntigravityAuthState {
            phase: AntigravityAuthFlowPhase::Failed,
            error: Some("stale".to_owned()),
        },
        false,
    );
    assert_eq!(cleared.phase, AntigravityAuthFlowPhase::Idle);
    assert!(cleared.error.is_none());

    // d1822be5a9 + 084ad5e1a0: AGY_CLI_DISABLE_AUTO_UPDATE + isolated SSH_CONNECTION
    let env = SanitizedEnv::from_entries([("PATH", "/usr/bin")], false);
    let dirs = resolve_antigravity_instance_directories("/data", "/tmp", "personal");
    let launch = build_antigravity_launch_spec_for_auth(
        &env,
        &AntigravityInstallTarget::AgyCli {
            executable: "/opt/agy".to_owned(),
        },
        &dirs,
        Some("/repo"),
        AntigravityAuthMethod::OauthPersonal,
    );
    assert_eq!(
        launch
            .env
            .get(AGY_CLI_DISABLE_AUTO_UPDATE_ENV)
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        launch.env.get("SSH_CONNECTION").map(String::as_str),
        Some("127.0.0.1 0 127.0.0.1 0")
    );
}

#[test]
fn antigravity_managed_release_and_skills_discovery() {
    let dir = tempdir().expect("tempdir");
    // d1822be5a9: changed byte size & pre-CLI binary report Repairable
    let rel_dir = dir.path().join("versions").join("r1");
    std::fs::create_dir_all(&rel_dir).expect("mkdir");
    std::fs::write(
        rel_dir.join(".legioncode-release.json"),
        r#"{"releaseId":"r1","version":"1.2.0","executable":{"name":"agy_acp_server","bytes":5}}"#,
    )
    .expect("write");
    assert!(matches!(
        inspect_managed_release(&dir.path().join("versions"), "r1", "agy"),
        ManagedReleaseIntegrity::Repairable { .. }
    ));

    // e923b5d8d8 + 1c379b63ba: sync rule copies without clobbering user edits
    let user_gemini = dir.path().join("user_gemini");
    let profile = dir.path().join("profile");
    std::fs::create_dir_all(user_gemini.join("config")).expect("mkdir");
    std::fs::create_dir_all(&profile).expect("mkdir");
    std::fs::write(user_gemini.join("AGENTS.md"), "global agents").expect("write");
    std::fs::write(user_gemini.join("GEMINI.md"), "global gemini").expect("write");
    std::fs::write(user_gemini.join("config").join("skills.json"), "{}").expect("write");
    sync_antigravity_user_rule_copies(&user_gemini, &profile).expect("sync");
    assert_eq!(
        std::fs::read_to_string(profile.join("AGENTS.md")).expect("read"),
        "global agents"
    );
    std::fs::write(profile.join("AGENTS.md"), "profile custom").expect("edit");
    std::fs::write(user_gemini.join("AGENTS.md"), "updated global").expect("update");
    sync_antigravity_user_rule_copies(&user_gemini, &profile).expect("resync");
    assert_eq!(
        std::fs::read_to_string(profile.join("AGENTS.md")).expect("read"),
        "profile custom"
    );

    // 1c379b63ba + 26b48d2da8 + ece4964b8e + 367f9d3759 + #40488 literal matching
    let ws = dir.path().join("ws");
    std::fs::create_dir_all(ws.join(".agents")).expect("mkdir");
    std::fs::create_dir_all(ws.join("extra-skills").join("alpha")).expect("mkdir");
    std::fs::create_dir_all(ws.join("extra-skills").join("experimental-draft")).expect("mkdir");
    std::fs::create_dir_all(ws.join("extra-skills").join("lint-draft")).expect("mkdir");
    std::fs::create_dir_all(ws.join("direct-skill")).expect("mkdir");
    std::fs::write(
        ws.join("extra-skills").join("alpha").join("SKILL.md"),
        "---\nname: alpha-skill\ndescription: Alpha helper\n---\n",
    )
    .expect("write");
    std::fs::write(
        ws.join("extra-skills")
            .join("experimental-draft")
            .join("SKILL.md"),
        "---\nname: experimental-draft\n---\n",
    )
    .expect("write");
    std::fs::write(
        ws.join("extra-skills").join("lint-draft").join("SKILL.md"),
        "---\nname: lint-draft\n---\n",
    )
    .expect("write");
    std::fs::write(
        ws.join("direct-skill").join("SKILL.md"),
        "---\nname: direct-skill\n---\n",
    )
    .expect("write");
    std::fs::write(
        ws.join(".agents").join("skills.json"),
        json!({
            "inherits": null,
            "entries": [
                { "path": "../extra-skills", "include_only": ["alpha", "experimental-draft", "lint-.*"], "exclude": ["experimental-.*"] },
                { "path": "../direct-skill/SKILL.md" }
            ]
        })
        .to_string(),
    )
    .expect("write");
    assert_eq!(
        resolve_skills_config_path("~/ignored-home", &ws.join(".agents")),
        ws.join(".agents").join("~/ignored-home")
    );
    let skills = discover_antigravity_skills(&ws, dir.path());
    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["alpha-skill", "direct-skill", "experimental-draft"]
    );
}
