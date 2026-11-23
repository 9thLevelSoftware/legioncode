//! Live-server HTTP client calls for `legioncode auth` mutations when a local server is active.

use std::time::Duration;

use legion_persistence::paths::ServerDerivedPaths;
use reqwest::header::AUTHORIZATION;
use serde_json::json;

use crate::cmd_pair::exchange_local_bearer;
use crate::fallback::write_stdout_line;

pub(crate) fn try_live_auth_delete(
    paths: &ServerDerivedPaths,
    origin: &str,
    path: &str,
    ok_msg: &str,
    missing_msg: &str,
) -> Option<i32> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let bearer = exchange_local_bearer(&paths.db_path, origin, "legioncode auth cli")
            .await
            .ok()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok()?;
        let url = format!("{}{path}", origin.trim_end_matches('/'));
        let resp = client
            .delete(&url)
            .header(AUTHORIZATION, format!("Bearer {bearer}"))
            .send()
            .await
            .ok()?;
        let status = resp.status();
        if status.is_success() {
            Some(write_stdout_line(ok_msg, "auth"))
        } else if status.as_u16() == 404 {
            let body = resp.text().await.unwrap_or_default();
            body.contains("not found")
                .then(|| write_stdout_line(missing_msg, "auth"))
        } else {
            None
        }
    })
}

pub(crate) fn try_live_connect_device_post(
    paths: &ServerDerivedPaths,
    origin: &str,
    path: &str,
    code: &str,
    ok_msg: &str,
    missing_msg: &str,
    conflict_msg: &str,
) -> Option<i32> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .ok()?;
    rt.block_on(async {
        let bearer = exchange_local_bearer(&paths.db_path, origin, "legioncode auth cli")
            .await
            .ok()?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .ok()?;
        let url = format!("{}{path}", origin.trim_end_matches('/'));
        let resp = client
            .post(&url)
            .header(AUTHORIZATION, format!("Bearer {bearer}"))
            .json(&json!({ "code": code }))
            .send()
            .await
            .ok()?;
        let status = resp.status();
        if status.is_success() {
            Some(write_stdout_line(ok_msg, "auth"))
        } else if status.as_u16() == 404 {
            let body = resp.text().await.unwrap_or_default();
            body.contains("no ")
                .then(|| write_stdout_line(missing_msg, "auth"))
        } else if status.as_u16() == 409 {
            Some(write_stdout_line(conflict_msg, "auth"))
        } else {
            None
        }
    })
}
