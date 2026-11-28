#[path = "ws_client.rs"]
pub mod ws_client;

use std::time::{SystemTime, UNIX_EPOCH};

use legion_cli::serve::{RunningServer, ServeConfig, ServeMode, spawn_server};
use legion_persistence::auth_mutations::{IssueSessionInput, issue_scoped_session};
use legion_persistence::pairing_tokens::{
    IssuePairingTokenInput, STANDARD_CLIENT_SCOPES, issue_pairing_token_with_conn,
};
use legion_persistence::sqlite::SqliteStore;
use legion_protocol::rpc_codec::{
    ClientRpcFrame, RpcCauseReason, RpcExit, RpcId, ServerRpcFrame, decode_server_frames,
    encode_client_frame,
};
use serde_json::{Value, json};
use ws_client::WsTestClient;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;

pub async fn spawn_srv(
    base: &std::path::Path,
) -> Result<RunningServer, Box<dyn std::error::Error>> {
    spawn_server(ServeConfig {
        base_dir: base.to_path_buf(),
        base_dir_is_explicit: true,
        port: 0,
        host: Some("127.0.0.1".to_owned()),
        mode: ServeMode::Web,
        no_browser: true,
        lan: false,
        static_dir: None,
        cwd: base.to_path_buf(),
    })
    .await
    .map_err(Into::into)
}

pub async fn connect_ws(
    server: &RunningServer,
    token: &str,
) -> Result<WsTestClient, Box<dyn std::error::Error>> {
    WsTestClient::connect(
        server.addr(),
        &format!("/ws?orchestrationProtocol=2&wsTicket={token}"),
    )
    .await
}

pub async fn call_unary(
    ws: &mut WsTestClient,
    id: &str,
    tag: &str,
    payload: Value,
) -> Result<RpcExit, Box<dyn std::error::Error>> {
    let frame = ClientRpcFrame::Request {
        id: RpcId::from(id),
        tag: tag.to_owned(),
        payload,
        headers: Vec::new(),
        is_notification: false,
        discard: false,
        trace_id: None,
        span_id: None,
        sampled: None,
    };
    ws.send_text(&encode_client_frame(&frame)?).await?;
    loop {
        for resp in decode_server_frames(&ws.recv_text().await?)? {
            if let ServerRpcFrame::Exit { request_id, exit } = resp
                && request_id == RpcId::from(id)
            {
                return Ok(exit);
            }
        }
    }
}

pub fn assert_auth_denied(exit: RpcExit, expected_scope: &str, expected_perm: &str) {
    match exit {
        RpcExit::Failure { cause } => {
            let Some(RpcCauseReason::Fail { error }) = cause.first() else {
                panic!("expected Fail cause, got {cause:?}");
            };
            assert_eq!(error["_tag"], "EnvironmentAuthorizationError");
            assert_eq!(error["requiredScope"], expected_scope);
            assert_eq!(error["requiredPermission"], expected_perm);
        }
        other @ RpcExit::Success { .. } => {
            panic!("expected EnvironmentAuthorizationError failure, got {other:?}")
        }
    }
}

pub fn issue_token(
    server: &RunningServer,
    subject: &str,
    scopes: &[&str],
) -> Result<String, Box<dyn std::error::Error>> {
    let store = SqliteStore::open(&server.paths().db_path)?;
    let issued = issue_scoped_session(
        store.connection(),
        IssueSessionInput {
            subject: Some(subject.to_owned()),
            scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
            ..IssueSessionInput::default()
        },
    )?;
    Ok(issued.token)
}

pub async fn pair_standard_client(
    server: &RunningServer,
) -> Result<String, Box<dyn std::error::Error>> {
    let now_ms = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let credential = {
        let store = SqliteStore::open(&server.paths().db_path)?;
        issue_pairing_token_with_conn(
            store.connection(),
            &IssuePairingTokenInput {
                scopes: STANDARD_CLIENT_SCOPES
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                ..IssuePairingTokenInput::default()
            },
            now_ms,
        )?
        .credential
    };
    let resp = reqwest::Client::new()
        .post(format!("{}/api/auth/pair", server.origin()))
        .json(&json!({ "token": credential, "clientLabel": "std-client" }))
        .send()
        .await?;
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await?;
    Ok(body["accessToken"]
        .as_str()
        .ok_or("missing accessToken")?
        .to_owned())
}
