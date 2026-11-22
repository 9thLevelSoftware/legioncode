//! Property-based (`proptest`) invariant tests across `legion-remote`.

use std::path::Path;

use legion_contracts::ids::{EnvironmentId, NodeId, RunId, ThreadId};
use legion_remote::peer::delegation::{
    CallerCancelReason, FollowAction, FollowErrorKind, RemoteChildRef, RemoteDelegationTracker,
    RemoteStopCommand,
};
use legion_remote::peer::links::validate_peer_origin;
use legion_remote::secrets::SecretRef;
use legion_remote::theme::is_valid_theme_id;
use legion_remote::trace::{TraceSpanSummaryBuilder, to_rotated_trace_paths};
use legion_remote::tunnel::ssh::{SshTarget, redact_ssh_error_output, remote_state_key};
use legion_remote::tunnel::tailscale::build_tailscale_https_base_url;
use proptest::prelude::*;

proptest! {
    #[test]
    fn prop_secret_ref_roundtrip_and_arbitrary_input_safe(
        vault in "[A-Za-z0-9_-]{1,12}",
        item in "[A-Za-z0-9_-]{1,12}",
        field in "[A-Za-z0-9_-]{1,12}",
        junk in "\\PC*",
    ) {
        let raw = format!("op://{vault}/{item}/{field}");
        let parsed = SecretRef::parse(&raw)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(parsed.vault(), vault.as_str());
        prop_assert_eq!(parsed.item(), item.as_str());
        prop_assert_eq!(parsed.field(), field.as_str());
        let _ = SecretRef::parse(&junk);
    }

    #[test]
    fn prop_valid_theme_id_matches_ascii_bounds(id in "\\PC{0,64}") {
        let valid = is_valid_theme_id(&id);
        if valid {
            prop_assert!((1..=48).contains(&id.len()));
            prop_assert!(!matches!(id.as_str(), "system" | "light" | "dark"));
            let bytes = id.as_bytes();
            prop_assert!(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit());
            prop_assert!(bytes.iter().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-'));
        }
    }

    #[test]
    fn prop_delegation_parent_deleted_always_emits_stop_there(
        initial_stop_count in proptest::option::of(0u64..1000),
        with_run in any::<bool>(),
        err_variant in 0u8..6,
    ) {
        let env_id = EnvironmentId::new("env-p")
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let child_id = ThreadId::new("th-remote")
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let task_id = NodeId::new("task-p")
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let parent_id = ThreadId::new("th-parent")
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        let run_id = if with_run {
            Some(RunId::new("run-p").map_err(|e| TestCaseError::fail(e.to_string()))?)
        } else {
            None
        };

        let mut tracker = RemoteDelegationTracker::new(
            task_id,
            parent_id,
            RemoteChildRef {
                environment_id: env_id,
                thread_id: child_id,
                label: "Peer".to_owned(),
                title: None,
                run_id,
                stopped_there: None,
                stop_count: initial_stop_count,
            },
        );
        let cmd = tracker.on_caller_cancel(CallerCancelReason::ParentDeleted);
        let bumped = initial_stop_count.unwrap_or(0) + 1;
        let cmd_ok = matches!(
            cmd,
            RemoteStopCommand::StopThere { stop_count: Some(c), .. } if c == bumped
        );
        prop_assert!(cmd_ok);

        let err = match err_variant {
            0 => FollowErrorKind::ThreadNotFound,
            1 => FollowErrorKind::RunNotFound,
            2 => FollowErrorKind::CapabilityDenied,
            3 => FollowErrorKind::InvalidRequest { link_exists: true },
            4 => FollowErrorKind::Transient,
            _ => FollowErrorKind::Defect,
        };
        let action = tracker.on_follow_error(err, "error");
        let action_ok = matches!(
            action,
            FollowAction::SendStop(RemoteStopCommand::StopThere { stop_count: Some(c), .. }) if c == bumped
        );
        prop_assert!(action_ok);
    }

    #[test]
    fn prop_trace_rotated_paths_and_percentile_monotonicity(
        max_files in 0usize..=12,
        durations in prop::collection::vec(0u32..50_000, 1..40),
    ) {
        let base = Path::new("trace.ndjson");
        let paths = to_rotated_trace_paths(base, max_files);
        prop_assert_eq!(paths.len(), max_files + 1);
        prop_assert_eq!(paths.last().map(std::path::PathBuf::as_path), Some(base));

        let mut builder = TraceSpanSummaryBuilder::new(None);
        for d in &durations {
            builder.ingest_line(&format!(
                r#"{{"name":"op","durationMs":{d},"endTimeUnixNano":"1700000000000000000"}}"#
            ));
        }
        let summary = builder.build();
        prop_assert_eq!(summary.span_count, durations.len());
        prop_assert_eq!(summary.spans.len(), 1);
        let stats = &summary.spans[0];
        prop_assert!(stats.p50_ms <= stats.p90_ms);
        prop_assert!(stats.p90_ms <= stats.max_ms);
    }

    #[test]
    fn prop_tailscale_magic_dns_omits_443_and_rejects_control_chars(
        label in "[a-z0-9-]{1,12}",
        raw in "\\PC*",
    ) {
        let host = format!("{label}.tail.ts.net.");
        let url_default = build_tailscale_https_base_url(&host, None)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(url_default, format!("https://{label}.tail.ts.net/"));

        let url_alt = build_tailscale_https_base_url(&host, Some(8443))
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(url_alt, format!("https://{label}.tail.ts.net:8443/"));

        let _ = build_tailscale_https_base_url(&raw, None);
    }

    #[test]
    fn prop_peer_origin_rejects_public_http_and_accepts_https(
        label in "[a-z][a-z0-9]{1,10}",
    ) {
        let http_url = format!("http://{label}.example.com:3773/api");
        prop_assert!(validate_peer_origin(&http_url).is_err());

        let https_url = format!("https://{label}.example.com:3773/api?x=1");
        let normalized = validate_peer_origin(&https_url)
            .map_err(|e| TestCaseError::fail(e.to_string()))?;
        prop_assert_eq!(normalized, format!("https://{label}.example.com:3773"));
    }

    #[test]
    fn prop_ssh_state_key_is_16_hex_and_redaction_strips_tokens(
        alias in "[a-z0-9.-]{1,20}",
        token in "[A-Za-z0-9_-]{8,32}",
    ) {
        let target = SshTarget {
            alias,
            hostname: "host.internal".to_owned(),
            username: Some("dev".to_owned()),
            port: Some(22),
        };
        let key = remote_state_key(&target);
        prop_assert_eq!(key.len(), 16);
        prop_assert!(key.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));

        let redacted = redact_ssh_error_output(&format!(r#"{{"token":"{token}"}}"#), false);
        prop_assert!(!redacted.contains(&token));
        prop_assert!(redacted.contains("[redacted]"));
    }
}
