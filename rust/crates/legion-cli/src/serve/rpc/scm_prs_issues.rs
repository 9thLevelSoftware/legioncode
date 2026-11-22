//! Pull requests, issue trackers, source-control hosts, and project clone RPC router.

use std::sync::Arc;

use legion_protocol::rpc_codec::RpcExit;
use serde_json::Value;

use super::{AppState, scm_issues, scm_prs};

pub(crate) async fn dispatch(state: &Arc<AppState>, tag: &str, payload: Value) -> RpcExit {
    if let Some(method) = tag.strip_prefix("pullRequests.") {
        return scm_prs::dispatch(state, method, payload).await;
    }
    scm_issues::dispatch(state, tag, payload).await
}
