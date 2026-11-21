//! Tests for `service_env`, `claude_history`, and `triage_session` in `t3-persistence`.

use std::collections::BTreeMap;
use std::fs;

use serde_json::json;
use t3_persistence::claude_history::{
    ClaudeHistoryError, ClaudeHistoryOptions, fork_session, get_session_messages,
};
use t3_persistence::paths::ServerDerivedPaths;
use t3_persistence::service_env::{parse_service_env, upsert_service_env};
use t3_persistence::triage_session::{
    TRIAGE_PLAYBOOK, TriageEntry, TriageSourceClone, WriteTriageSessionInput,
    build_triage_app_prompt, build_triage_launch_prompt, describe_server_process,
    write_triage_session,
};

#[test]
fn service_env_upsert_and_parse_match_node_up_test_cases() {
    let before = [
        "# my settings",
        "T3CODE_PORT=4000",
        "export T3CODE_LAN=0",
        "T3CODE_LAN=1",
        "",
    ]
    .join("\n");
    assert_eq!(
        upsert_service_env(
            &before,
            &[("T3CODE_LAN", Some("true")), ("T3CODE_HOST", None)]
        ),
        "# my settings\nT3CODE_PORT=4000\nT3CODE_LAN=true\n"
    );
    assert_eq!(
        upsert_service_env("", &[("T3CODE_LAN", Some("false"))]),
        "T3CODE_LAN=false\n"
    );
    assert_eq!(
        upsert_service_env(
            "T3CODE_LAN=\"a\nb\"\nCUSTOM=\"x\nT3CODE_LAN=y\"\nT3CODE_LAN=true\n",
            &[("T3CODE_LAN", Some("false"))]
        ),
        "T3CODE_LAN=false\nCUSTOM=\"x\nT3CODE_LAN=y\"\n"
    );
    assert_eq!(
        upsert_service_env(
            "T3CODE_LAN=\u{00a0}\"unused\nCUSTOM=keep\nEND=\"\nT3CODE_LAN=true\n",
            &[("T3CODE_LAN", Some("false"))]
        ),
        "T3CODE_LAN=false\nCUSTOM=keep\nEND=\"\n"
    );
    let long = format!("CUSTOM=\"{}\"\n", "\n".repeat(130_000));
    assert_eq!(
        upsert_service_env(
            &format!("{long}T3CODE_LAN=true\n"),
            &[("T3CODE_LAN", Some("false"))]
        ),
        format!("{long}T3CODE_LAN=false\n")
    );

    let parsed = parse_service_env(
        "# c\nFOO=bar\nexport BAZ=qux # comment\nQ=\"a\\nb\"\nS='x\ny'\nB=`u\nv`\nT=\u{00a0}\"z\n",
    );
    assert_eq!(
        parsed,
        BTreeMap::from([
            ("B".to_owned(), "u\nv".to_owned()),
            ("BAZ".to_owned(), "qux".to_owned()),
            ("FOO".to_owned(), "bar".to_owned()),
            ("Q".to_owned(), "a\nb".to_owned()),
            ("S".to_owned(), "x\ny".to_owned()),
            ("T".to_owned(), "\u{00a0}\"z".to_owned()),
        ])
    );
}

#[test]
fn claude_history_reads_filters_slices_and_forks_with_compaction_remap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session_id = "11111111-1111-4111-8111-111111111111";
    let entries = [
        json!({"type": "user", "uuid": "u1", "parentUuid": null, "sessionId": session_id, "isSidechain": false, "message": {"role": "user", "content": "q1"}}),
        json!({"type": "system", "subtype": "compact_boundary", "uuid": "s1", "parentUuid": null, "logicalParentUuid": "u1", "sessionId": session_id, "isSidechain": false, "compactMetadata": {"preservedSegment": {"headUuid": "u1", "anchorUuid": "u2", "tailUuid": "u1"}, "preservedMessages": {"anchorUuid": "u2", "uuids": ["u1"], "allUuids": ["u1"]}}}),
        json!({"type": "assistant", "uuid": "a1", "parentUuid": "s1", "sessionId": session_id, "isSidechain": false, "message": {"role": "assistant", "content": [{"type": "text", "text": "ans1"}]}}),
        json!({"type": "user", "uuid": "u2", "parentUuid": "a1", "sessionId": session_id, "isSidechain": false, "message": {"role": "user", "content": "q2"}}),
        json!({"type": "user", "uuid": "side", "parentUuid": "u2", "sessionId": session_id, "isSidechain": true, "message": {"role": "user", "content": "ignored"}}),
        json!({"type": "assistant", "uuid": "a2", "parentUuid": "u2", "sessionId": session_id, "isSidechain": false, "message": {"role": "assistant", "content": [{"type": "text", "text": "ans2"}]}}),
    ];
    let raw = entries
        .iter()
        .map(|v| serde_json::to_string(v).expect("json"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.path().join(format!("{session_id}.jsonl")), raw).expect("write jsonl");

    let no_sys = get_session_messages(
        session_id,
        &ClaudeHistoryOptions {
            dir: Some(dir.path().to_path_buf()),
            include_system_messages: false,
            ..Default::default()
        },
    )
    .expect("read");
    assert_eq!(
        no_sys.iter().map(|m| m.uuid.as_str()).collect::<Vec<_>>(),
        vec!["u1", "a1", "u2", "a2"]
    );

    let with_sys = get_session_messages(
        session_id,
        &ClaudeHistoryOptions {
            dir: Some(dir.path().to_path_buf()),
            include_system_messages: true,
            ..Default::default()
        },
    )
    .expect("read sys");
    assert_eq!(
        with_sys.iter().map(|m| m.uuid.as_str()).collect::<Vec<_>>(),
        vec!["u1", "s1", "a1", "u2", "a2"]
    );

    let forked = fork_session(
        session_id,
        &ClaudeHistoryOptions {
            dir: Some(dir.path().to_path_buf()),
            up_to_message_id: Some("a1".to_owned()),
            ..Default::default()
        },
    )
    .expect("fork");
    assert_ne!(forked.session_id, session_id);
    assert!(forked.head_message_id.is_some());

    let forked_raw = fs::read_to_string(dir.path().join(format!("{}.jsonl", forked.session_id)))
        .expect("read fork");
    assert!(!forked_raw.contains("ans2"));
    assert!(!forked_raw.contains("ignored"));
    assert!(forked_raw.contains("ans1"));
    assert_eq!(
        forked.head_message_id,
        get_session_messages(
            &forked.session_id,
            &ClaudeHistoryOptions {
                dir: Some(dir.path().to_path_buf()),
                ..Default::default()
            }
        )
        .expect("read forked")
        .last()
        .map(|m| m.uuid.clone())
    );

    let missing_anchor = fork_session(
        session_id,
        &ClaudeHistoryOptions {
            dir: Some(dir.path().to_path_buf()),
            up_to_message_id: Some("missing-id".to_owned()),
            ..Default::default()
        },
    )
    .expect_err("missing anchor");
    assert!(matches!(
        missing_anchor,
        ClaudeHistoryError::UpToMessageNotFound { .. }
    ));
}

#[test]
fn triage_session_describes_process_and_writes_exact_playbook_and_context() {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = ServerDerivedPaths::derive(dir.path(), false, false);
    fs::create_dir_all(&paths.state_dir).expect("mkdir");

    assert_eq!(
        describe_server_process(&paths.server_runtime_state_path).expect("missing"),
        "not running (no server-runtime.json; the server may never have started here)"
    );
    fs::write(
        &paths.server_runtime_state_path,
        format!(
            r#"{{"version":1,"pid":{},"port":3773,"origin":"http://127.0.0.1:3773","startedAt":"2026-11-20T00:00:00.000Z","serverVersion":"0.1.10"}}"#,
            std::process::id()
        ),
    )
    .expect("write state");
    let server = describe_server_process(&paths.server_runtime_state_path).expect("live");
    assert!(server.starts_with(&format!(
        "running (pid {}, http://127.0.0.1:3773, version 0.1.10)",
        std::process::id()
    )));

    fs::write(
        &paths.server_runtime_state_path,
        r#"{"version":1,"pid":99999999,"port":3773,"origin":"http://127.0.0.1:3773","startedAt":"2026-11-20T00:00:00.000Z"}"#,
    )
    .expect("write stale state");
    let stale = describe_server_process(&paths.server_runtime_state_path).expect("stale");
    assert_eq!(
        stale,
        "not running (state file is stale: pid 99999999 is dead; last origin http://127.0.0.1:3773, version not recorded)"
    );

    let written = write_triage_session(&WriteTriageSessionInput {
        base_dir: dir.path().to_path_buf(),
        paths: paths.clone(),
        entry: TriageEntry::App {
            client: "T3 Code Desktop 0.1.10".to_owned(),
        },
        server,
        source_clone: Some(TriageSourceClone::Cloning {
            directory: "/tmp/source/abc".to_owned(),
            git_ref: "v0.1.10".to_owned(),
            commit: "abc".to_owned(),
            server_version: "0.1.10".to_owned(),
        }),
        now_ms: Some(1_795_120_200_000),
        os_label: Some("windows x86_64".to_owned()),
        runtime_version: Some("rust-0.1.10".to_owned()),
    })
    .expect("write triage");

    assert!(written.context_file_path.exists());
    assert!(written.prompt_file_path.exists());
    let context = fs::read_to_string(&written.context_file_path).expect("context");
    let playbook = fs::read_to_string(&written.prompt_file_path).expect("playbook");
    assert!(context.contains("## This session runs inside T3 Code"));
    assert!(context.contains("The server is cloning the source for its own version (0.1.10)"));
    assert!(playbook.ends_with(TRIAGE_PLAYBOOK));
    assert_eq!(TRIAGE_PLAYBOOK.len(), 8722);

    let launch = build_triage_launch_prompt(r"C:\Users\a b\.t3\triage-playbook.md");
    assert!(!launch.contains('\n'));
    assert!(build_triage_app_prompt("docs/My File (1).md").contains("docs/My%20File%20%281%29.md"));
}
