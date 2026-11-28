//! Helper endpoint tests: ABI version, diff runs, UTF-16 frame parts, palette search, i18n,
//! and RPC permission guards.

use legion_shared::frame_parts::{FrameOutput, FramePartAssembler, PART_RECEIVED_FRAME};
use legion_wasm_ffi::bridge::{
    decorate_diff_runs_json, evaluate_rpc_scopes_json, palette_search_json,
    split_utf16_frame_parts_json, translate_i18n_json,
};
use legion_wasm_ffi::{abi_version, legion_wasm_abi_version};
use serde_json::Value;

fn parse_json(raw: &str) -> Result<Value, String> {
    serde_json::from_str(raw).map_err(|err| format!("failed to parse JSON: {err} ({raw})"))
}

#[test]
fn abi_version_is_one() {
    assert_eq!(abi_version(), 1);
    assert_eq!(legion_wasm_abi_version(), 1);
}

#[test]
fn decorate_diff_runs_preserves_font_styles_and_attributes() -> Result<(), String> {
    let input = r##"{
        "runs": [
            {
                "start": 0,
                "end": 6,
                "text": "foobar",
                "foregroundColor": "#ff00aa",
                "fontStyle": 3,
                "ligaturesEnabled": false,
                "attributes": { "scope": "identifier" }
            }
        ],
        "highlights": [{ "start": 3, "end": 6 }],
        "highlightBackground": "#334455"
    }"##;
    let out = parse_json(&decorate_diff_runs_json(input)?)?;
    let runs = out["runs"].as_array().ok_or("missing runs array")?;
    assert_eq!(runs.len(), 2);
    assert_eq!(runs[0]["text"], "foo");
    assert_eq!(runs[0]["wordHighlighted"], false);
    assert_eq!(runs[0]["backgroundColor"], Value::Null);
    assert_eq!(runs[1]["text"], "bar");
    assert_eq!(runs[1]["wordHighlighted"], true);
    assert_eq!(runs[1]["backgroundColor"], "#334455");
    assert_eq!(runs[1]["fontStyle"], 3);
    assert_eq!(runs[1]["attributes"]["scope"], "identifier");

    let from_tokens = parse_json(&decorate_diff_runs_json(
        r##"{"tokens":[{"content":"let x","color":"#112233","fontStyle":2}],"highlights":[{"start":0,"end":3}]}"##,
    )?)?;
    let token_runs = from_tokens["runs"].as_array().ok_or("missing token runs")?;
    assert_eq!(token_runs.len(), 2);
    assert_eq!(token_runs[0]["text"], "let");
    assert_eq!(token_runs[0]["fontStyle"], 2);
    assert!(decorate_diff_runs_json("{}").is_err());
    Ok(())
}

#[test]
fn split_utf16_frame_parts_handles_unpaired_surrogates_and_reassembles() -> Result<(), String> {
    // [0xD800, 0x4E2D, 0x4E2D] -> "\u{FFFD}中中" (9 UTF-8 bytes, BUG-21 unpaired surrogate = 3 bytes).
    let out = parse_json(&split_utf16_frame_parts_json(
        r#"{"utf16Units":[55296,20013,20013],"partBytes":5}"#,
    )?)?;
    assert_eq!(out["wireUtf8Bytes"], 9);
    assert_eq!(out["partCount"], 3);
    assert_eq!(out["partBytesLimit"], 5);
    let parts = out["parts"].as_array().ok_or("missing parts")?;

    let mut assembler = FramePartAssembler::new();
    let mut reassembled = None;
    for (idx, part_val) in parts.iter().enumerate() {
        let part = part_val.as_str().ok_or("part is not string")?;
        assert!(part.len() <= 5);
        match assembler.push(part) {
            FrameOutput::IntermediateAck(ack) => {
                assert!(idx < 2);
                assert_eq!(ack, PART_RECEIVED_FRAME);
            }
            FrameOutput::Complete(whole) => {
                reassembled = Some(whole.into_owned());
            }
        }
    }
    assert_eq!(reassembled.as_deref(), Some("\u{FFFD}中中"));

    assert!(split_utf16_frame_parts_json(r#"{"text":"hello","partBytes":1}"#).is_err());
    assert!(split_utf16_frame_parts_json("{}").is_err());
    Ok(())
}

#[test]
fn palette_search_ranks_matches_and_returns_utf16_ranges() -> Result<(), String> {
    let input = r#"{
        "query": "auth",
        "titles": [
            "Fix oauth redirect",
            "Authentication cleanup",
            "Share auth across worktrees"
        ]
    }"#;
    let out = parse_json(&palette_search_json(input)?)?;
    assert_eq!(out["normalizedQuery"], "auth");
    assert_eq!(out["totalIndexed"], 3);
    let matches = out["matches"].as_array().ok_or("missing matches")?;
    assert_eq!(matches.len(), 3);
    assert_eq!(matches[0]["itemIndex"], 2);
    assert_eq!(matches[0]["ranges"][0]["start"], 6);
    assert_eq!(matches[0]["ranges"][0]["end"], 10);

    let weighted = parse_json(&palette_search_json(
        r#"{
            "query": "relay",
            "entries": [
                {"fields":[{"text":"Thread A","weight":1.0},{"text":"cloud relay","weight":0.8,"context":true}],"usageBoost":1.0},
                {"fields":[{"text":"Relay tunnel","weight":1.0}],"usageBoost":1.2}
            ],
            "limit": 1
        }"#,
    )?)?;
    let w_matches = weighted["matches"]
        .as_array()
        .ok_or("missing weighted matches")?;
    assert_eq!(w_matches.len(), 1);
    assert_eq!(w_matches[0]["itemIndex"], 1);
    Ok(())
}

#[test]
fn translate_i18n_enforces_swedish_parity_and_trad_chinese_exclusion() -> Result<(), String> {
    let sv = parse_json(&translate_i18n_json(
        r#"{"language":"system","systemLocales":["zh-TW","sv-SE"],"path":"/download"}"#,
    )?)?;
    assert_eq!(sv["locale"], "sv");
    assert_eq!(sv["formattingLocale"], "sv-SE");
    assert_eq!(sv["edgeCatalogLocale"], "sv");
    assert_eq!(sv["localizedHref"], "/sv/download");
    assert_eq!(sv["navigation"]["settings"], "Inställningar");
    assert_eq!(sv["supportedLocales"].as_array().map(Vec::len), Some(10));

    let zh_tw = parse_json(&translate_i18n_json(r#"{"language":"zh-TW"}"#)?)?;
    assert_eq!(zh_tw["locale"], "en");

    let de_activity = parse_json(&translate_i18n_json(
        r#"{
            "language": "de",
            "activity": {
                "rows": [{"phase":"waiting_for_approval","title":"Run tests","project":"legioncode"}],
                "phase": "waiting_for_approval",
                "active": true
            }
        }"#,
    )?)?;
    assert_eq!(de_activity["locale"], "de");
    assert_eq!(de_activity["navigation"]["settings"], "Einstellungen");
    assert!(de_activity["liveActivity"]["action"].is_string());
    Ok(())
}

#[test]
fn evaluate_rpc_scopes_enforces_all_required_scopes() -> Result<(), String> {
    let denied = parse_json(&evaluate_rpc_scopes_json(
        r##"{
            "method": "git.preparePullRequestThread",
            "payload": {"cwd":"/repo","reference":"#42","mode":"worktree","threadId":"t1"},
            "session": {"authenticated": true, "permissions": ["source-control:write"]}
        }"##,
    )?)?;
    assert_eq!(denied["allowed"], false);
    assert_eq!(denied["requiredScopes"].as_array().map(Vec::len), Some(2));

    let allowed = parse_json(&evaluate_rpc_scopes_json(
        r##"{
            "method": "git.preparePullRequestThread",
            "payload": {"cwd":"/repo","reference":"#42","mode":"worktree","threadId":"t1"},
            "session": {
                "authenticated": true,
                "permissions": ["source-control:write", "orchestration:operate"]
            }
        }"##,
    )?)?;
    assert_eq!(allowed["allowed"], true);

    for method in [
        "workflowBoard.exploreStep",
        "workflowBoard.challengeStep",
        "workflowBoard.updatePlan",
        "workflowBoard.approvePlan",
        "workflowBoard.returnForRevision",
    ] {
        let denied_board = parse_json(&evaluate_rpc_scopes_json(&format!(
            r#"{{"method":"{method}","payload":{{}},"session":{{"authenticated":true,"permissions":["orchestration:read"]}}}}"#
        ))?)?;
        assert_eq!(denied_board["allowed"], false);
        assert_eq!(
            denied_board["requiredScopes"],
            serde_json::json!(["orchestration:operate"])
        );

        let allowed_board = parse_json(&evaluate_rpc_scopes_json(&format!(
            r#"{{"method":"{method}","payload":{{}},"session":{{"authenticated":true,"permissions":["orchestration:operate"]}}}}"#
        ))?)?;
        assert_eq!(allowed_board["allowed"], true);
    }

    let diff_res = parse_json(&evaluate_rpc_scopes_json(
        r#"{"method":"workflowBoard.getCardDiff","payload":{},"session":{"authenticated":true,"permissions":["orchestration:read"]}}"#,
    )?)?;
    assert_eq!(diff_res["allowed"], true);
    assert_eq!(diff_res["requiresHumanClientCaller"], false);
    assert_eq!(
        diff_res["requiredScopes"],
        serde_json::json!(["orchestration:read"])
    );

    for (method, human_only) in [
        ("workflowBoard.startStage", false),
        ("workflowBoard.pauseCard", false),
        ("workflowBoard.resumeCard", false),
        ("workflowBoard.retryStep", false),
        ("workflowBoard.proposeAmendment", false),
        ("workflowBoard.resolveFinding", true),
        ("workflowBoard.recordManualAcceptance", true),
        ("workflowBoard.approveDelivery", true),
        ("workflowBoard.previewCliImport", false),
        ("workflowBoard.applyCliImport", true),
    ] {
        let agent_eval = parse_json(&evaluate_rpc_scopes_json(&format!(
            r#"{{"method":"{method}","payload":{{}},"subject":"agent:sub-1","session":{{"authenticated":true,"permissions":["orchestration:operate"]}}}}"#
        ))?)?;
        assert_eq!(agent_eval["requiresHumanClientCaller"], human_only);
        assert_eq!(agent_eval["allowed"], !human_only);
        assert_eq!(
            agent_eval["requiredScopes"],
            serde_json::json!(["orchestration:operate"])
        );

        let client_eval = parse_json(&evaluate_rpc_scopes_json(&format!(
            r#"{{"method":"{method}","payload":{{}},"subject":"one-time-token","session":{{"authenticated":true,"permissions":["orchestration:operate"]}}}}"#
        ))?)?;
        assert_eq!(client_eval["allowed"], true);
    }
    Ok(())
}
