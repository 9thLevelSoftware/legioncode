use legion_cli::serve::RunningServer;
use legion_persistence::project_store::{
    CreateProjectInput, create_project, ensure_project_store_schema,
};
use legion_persistence::sqlite::SqliteStore;
use legion_protocol::rpc_codec::{RpcCauseReason, RpcExit};
use serde_json::{Value, json};

use super::helpers::ws_client::WsTestClient;
use super::helpers::{TestResult, call_unary, connect_ws, issue_token, spawn_srv};

async fn assert_favicon_never_serves_secret(
    ws: &mut WsTestClient,
    http: &reqwest::Client,
    origin: &str,
    id: &str,
    resource: Value,
) -> TestResult {
    match call_unary(ws, id, "assets.createUrl", json!({ "resource": resource })).await? {
        RpcExit::Success { value } => {
            let rel = value["relativeUrl"].as_str().ok_or("missing relativeUrl")?;
            assert!(
                rel.ends_with("/project-favicon-missing"),
                "expected missing fallback marker, got {rel}"
            );
            let resp = http.get(format!("{origin}{rel}")).send().await?;
            assert_eq!(resp.status(), 404);
            let body = resp.text().await?;
            assert!(!body.contains("TOP_SECRET_"));
        }
        RpcExit::Failure { cause } => {
            let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                panic!("expected Fail cause, got {cause:?}");
            };
            assert_eq!(error["_tag"], "AssetWorkspaceContextNotFoundError");
        }
    }
    Ok(())
}

fn set_project_favicon_override(server: &RunningServer, path: Option<&str>) -> TestResult {
    let store = SqliteStore::open(&server.paths().db_path)?;
    store.connection().execute(
        "UPDATE projection_projects SET favicon_path = ?1 WHERE project_id = 'proj-fav-1'",
        [path],
    )?;
    Ok(())
}

#[tokio::test]
async fn ws_project_favicon_ignores_untrusted_path_and_confines_to_known_project() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("ws-favicon-scope")).await?;
    let known_ws = temp.path().join("known-ws");
    let unknown_ws = temp.path().join("unknown-ws");
    let outside_dir = temp.path().join("outside");
    std::fs::create_dir_all(&known_ws)?;
    std::fs::create_dir_all(&unknown_ws)?;
    std::fs::create_dir_all(&outside_dir)?;
    std::fs::write(known_ws.join("secret.txt"), "TOP_SECRET_WORKSPACE_FILE")?;
    std::fs::write(unknown_ws.join("secret.txt"), "TOP_SECRET_UNREGISTERED")?;
    let outside_secret = outside_dir.join("host_secret.txt");
    std::fs::write(&outside_secret, "TOP_SECRET_OUTSIDE_HOST_FILE")?;
    std::fs::write(outside_dir.join("host_icon.svg"), "<svg>outside</svg>")?;

    {
        let store = SqliteStore::open(&server.paths().db_path)?;
        ensure_project_store_schema(store.connection())?;
        create_project(
            store.connection(),
            &CreateProjectInput::new(known_ws.to_string_lossy().as_ref())
                .with_project_id("proj-fav-1"),
        )?;
    }

    let read_tok = issue_token(&server, "one-time-token", &["orchestration:read"])?;
    let mut ws_read = connect_ws(&server, &read_tok).await?;
    let http = reqwest::Client::new();
    let known_str = known_ws.to_string_lossy().into_owned();
    let unknown_str = unknown_ws.to_string_lossy().into_owned();
    let outside_str = outside_secret.to_string_lossy().into_owned();

    for (id, resource) in [
        (
            "f1",
            json!({ "_tag": "project-favicon", "cwd": unknown_str, "path": "secret.txt" }),
        ),
        (
            "f2",
            json!({ "_tag": "project-favicon", "cwd": known_str, "path": "secret.txt" }),
        ),
        (
            "f3",
            json!({ "kind": "project-favicon", "cwd": known_str, "path": "secret.txt" }),
        ),
        (
            "f4",
            json!({ "_tag": "project-favicon", "cwd": known_str, "path": outside_str }),
        ),
        (
            "f5",
            json!({ "_tag": "project-favicon", "cwd": known_str, "path": "../outside/host_secret.txt" }),
        ),
    ] {
        assert_favicon_never_serves_secret(&mut ws_read, &http, server.origin(), id, resource)
            .await?;
    }

    for bad_db_path in ["secret.txt", "../outside/host_icon.svg"] {
        set_project_favicon_override(&server, Some(bad_db_path))?;
        let res = json!({ "_tag": "project-favicon", "cwd": known_str, "path": "secret.txt" });
        assert_favicon_never_serves_secret(&mut ws_read, &http, server.origin(), "f6", res).await?;
    }

    set_project_favicon_override(&server, None)?;
    std::fs::write(known_ws.join("favicon.svg"), "<svg>legit</svg>")?;
    let RpcExit::Success { value } = call_unary(
        &mut ws_read,
        "f7",
        "assets.createUrl",
        json!({ "resource": { "_tag": "project-favicon", "cwd": known_str, "path": "secret.txt" } }),
    )
    .await?
    else {
        panic!("expected success for real project favicon");
    };
    let rel = value["relativeUrl"].as_str().ok_or("missing relativeUrl")?;
    assert!(rel.ends_with("/favicon.svg"), "unexpected url {rel}");
    let resp = http.get(format!("{}{rel}", server.origin())).send().await?;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await?, "<svg>legit</svg>");

    server.shutdown().await?;
    Ok(())
}

#[tokio::test]
async fn ws_attachment_confines_to_attachments_dir_and_rejects_traversal() -> TestResult {
    let temp = tempfile::tempdir()?;
    let server = spawn_srv(&temp.path().join("sub").join("ws-att-scope")).await?;
    let outside = temp.path().join("outside");
    let known_ws = temp.path().join("known-ws");
    std::fs::create_dir_all(&outside)?;
    std::fs::create_dir_all(&known_ws)?;
    let outside_secret = outside.join("host_secret.txt");
    std::fs::write(&outside_secret, "TOP_SECRET_HOST")?;
    std::fs::write(known_ws.join("secret.txt"), "TOP_SECRET_WS")?;
    std::fs::write(
        server.paths().state_dir.join("state_secret.txt"),
        "TOP_SECRET_STATE_DIR",
    )?;
    std::fs::create_dir_all(&server.paths().attachments_dir)?;
    std::fs::write(
        server.paths().attachments_dir.join("valid-att-1.txt"),
        "ATTACHMENT_OK",
    )?;

    let read_tok = issue_token(&server, "one-time-token", &["orchestration:read"])?;
    let mut ws = connect_ws(&server, &read_tok).await?;
    let outside_str = outside_secret.to_string_lossy().into_owned();
    for (id, key, att_id) in [
        ("a1", "_tag", "../state_secret.txt"),
        ("a2", "_tag", outside_str.as_str()),
        ("a3", "kind", outside_str.as_str()),
        ("a4", "_tag", "../../../known-ws/secret.txt"),
        ("a5", "_tag", "C:state_secret.txt"),
    ] {
        let resource = json!({ key: "attachment", "attachmentId": att_id });
        let RpcExit::Failure { cause } = call_unary(
            &mut ws,
            id,
            "assets.createUrl",
            json!({ "resource": resource }),
        )
        .await?
        else {
            panic!("expected AssetAttachmentNotFoundError for case {id}");
        };
        let Some(RpcCauseReason::Fail { error }) = cause.first() else {
            panic!("expected Fail cause for case {id}, got {cause:?}");
        };
        assert_eq!(error["_tag"], "AssetAttachmentNotFoundError");
    }

    let RpcExit::Success { value } = call_unary(
        &mut ws,
        "a6",
        "assets.createUrl",
        json!({ "resource": { "_tag": "attachment", "attachmentId": "valid-att-1" } }),
    )
    .await?
    else {
        panic!("expected success for valid attachment");
    };
    let rel = value["relativeUrl"].as_str().ok_or("missing relativeUrl")?;
    let resp = reqwest::get(format!("{}{rel}", server.origin())).await?;
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await?, "ATTACHMENT_OK");

    server.shutdown().await?;
    Ok(())
}
