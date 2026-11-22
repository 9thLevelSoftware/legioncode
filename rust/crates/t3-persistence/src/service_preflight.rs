//! Pre-start database and launcher protocol compatibility checks.
//!
//! Mirrors `apps/server/src/cloud/servicePreflight.ts`,
//! `apps/server/src/cloud/serviceProtocol.ts`,
//! `apps/server/src/persistence/databaseMigrationHead.ts`,
//! `apps/server/src/persistence/reconcileV2PreviewMigration.ts`, and
//! `apps/server/src/persistence/Migrations.ts`.

use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sqlite::{RecordedMigration, read_recorded_migrations};

/// Latest SQLite migration ID compiled into this build (`Migrations.ts:217`).
pub const BUILD_MIGRATION_HEAD: i64 = crate::sqlite::BUILD_MIGRATION_HEAD;

/// First V2 migration ID covered by [`V2_MIGRATION_MANIFEST`] (`Migrations.ts:170`).
pub const FIRST_V2_MIGRATION_ID: i64 = 55;

/// Current service launcher protocol version required by `t3` (`serviceProtocol.ts:18`).
pub const SERVICE_LAUNCHER_PROTOCOL: i64 = 4;

/// Service launcher protocol versions supported by this build (`serviceProtocol.ts:20-21`).
pub const SUPPORTED_LAUNCHER_PROTOCOLS: &[i64] = &[3, 4];

/// Expected V2 migration entries from `FIRST_V2_MIGRATION_ID` through `BUILD_MIGRATION_HEAD`.
pub const V2_MIGRATION_MANIFEST: &[(i64, &str)] = &[
    (55, "OrchestrationV2"),
    (56, "RemoveRedundantProjectionIndexes"),
    (57, "ScheduledTaskWebhooks"),
    (58, "WebhookRelayDeliveries"),
    (59, "McpAppModelContext"),
    (60, "ThreadSnapshotWindowIndexes"),
    (61, "PullRequestReviewGuides"),
    (62, "ThreadFailureLookupIndex"),
    (63, "DropUnusedV1Projections"),
    (64, "CanonicalizeThreadPullRequestLinks"),
    (65, "CodexUsageNativeTurnIndex"),
    (66, "RepairClientSettlementTimestamps"),
    (67, "WebhookDedupe"),
    (68, "ShellStopRequestIndex"),
    (69, "ThreadMessageSearch"),
    (70, "PeerLinks"),
    (71, "PeerLinkFence"),
    (72, "WaitingLinkRequestIndex"),
    (73, "ActiveTurnItemIndexes"),
    (74, "ExcludeAttachmentPlaceholdersFromSearch"),
    (75, "SubagentDeliveryState"),
    (76, "ProjectionProjectsPinArchive"),
    (77, "RepairBooleanSettledOverride"),
    (78, "DropOrchestrationEventCorrelationIndex"),
    (79, "DelegatedRecoveryIndexes"),
    (80, "ScheduledTaskThreadMode"),
    (81, "ProjectionMessageSearchText"),
    (82, "TurnItemConversationIndex"),
    (83, "CommandReadIndexes"),
    (84, "LastExecutedRunIndex"),
    (85, "TurnItemCounts"),
    (86, "QueuedTurnInputIndex"),
    (87, "ProjectionProjectsCustomTitle"),
    (88, "TurnItemTypeStatusIndex"),
    (89, "ShellThreadReadIndexes"),
    (90, "AuthConnectDevices"),
    (91, "AuthConnectDeniedRequests"),
    (92, "QueuedTurnInputIndexJsonValid"),
    (93, "ProviderTurnAttemptIndex"),
    (94, "TurnItemMessageOrdinalIndex"),
    (95, "TurnItemInlineWorkOutput"),
    (96, "RepairStaleTitleRegeneration"),
    (97, "IssueTrackerBindings"),
    (98, "ActiveShellIndexIndexes"),
    (99, "BackgroundRecoveryIndexes"),
    (100, "TurnStartReadIndexes"),
    (101, "DelegatedTaskReadIndexes"),
    (102, "ThreadItemCounts"),
    (103, "IssueTrackerAdditionalScopes"),
    (104, "ProjectionProjectsTrackedFolders"),
    (105, "TaskFolders"),
];

/// Result emitted by [`run_service_preflight`], serializing to exact wire JSON with `"status"` first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ServicePreflightResult {
    /// Launcher protocol and database migration history are compatible with this build.
    Ready {
        /// Running `t3` package version string.
        version: String,
        /// Negotiated service launcher protocol version.
        #[serde(rename = "launcherProtocol")]
        launcher_protocol: i64,
    },
    /// Starting `t3` against the installed launcher or database is unsafe.
    Blocked {
        /// Running `t3` package version string.
        version: String,
        /// Human-readable remediation message.
        reason: String,
    },
}

impl ServicePreflightResult {
    /// Returns `true` when preflight succeeded with [`ServicePreflightResult::Ready`].
    pub const fn is_ready(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Returns the package version reported in this result.
    pub fn version(&self) -> &str {
        match self {
            Self::Ready { version, .. } | Self::Blocked { version, .. } => version,
        }
    }

    /// Returns the blocking reason when [`ServicePreflightResult::Blocked`], or `None` if ready.
    pub fn reason(&self) -> Option<&str> {
        match self {
            Self::Ready { .. } => None,
            Self::Blocked { reason, .. } => Some(reason),
        }
    }
}

/// Returns `true` if `launcher_protocol` is in [`SUPPORTED_LAUNCHER_PROTOCOLS`].
pub const fn is_supported_launcher_protocol(launcher_protocol: i64) -> bool {
    matches!(launcher_protocol, 3 | 4)
}

/// Reads recorded rows from `effect_sql_migrations` using a fast read-only connection.
///
/// Returns `Some(vec![])` when `db_path` does not exist or has no `effect_sql_migrations` table,
/// and `None` when an existing file cannot be opened or queried (such as corrupt or non-SQLite bytes).
pub fn recorded_migrations(db_path: &Path) -> Option<Vec<RecordedMigration>> {
    read_recorded_migrations(db_path)
}

/// Reconciles legacy V2 preview migration IDs (`53` or `54` named `"OrchestrationV2"`) into the
/// canonical `53..=56` sequence (`reconcileV2PreviewMigration.ts:11-43`).
///
/// Returns `None` when the history after ID `52` matches neither normal history nor a known
/// single-build preview progression.
///
/// # Examples
///
/// ```
/// use t3_persistence::service_preflight::reconciled_v2_preview_history;
/// use t3_persistence::sqlite::RecordedMigration;
///
/// let preview = vec![
///     RecordedMigration { migration_id: 52, name: "PreV2".to_owned() },
///     RecordedMigration { migration_id: 54, name: "OrchestrationV2".to_owned() },
/// ];
/// let reconciled = reconciled_v2_preview_history(&preview);
/// assert_eq!(
///     reconciled,
///     Some(vec![
///         RecordedMigration { migration_id: 52, name: "PreV2".to_owned() },
///         RecordedMigration { migration_id: 53, name: "PullRequestFilesViewed".to_owned() },
///         RecordedMigration {
///             migration_id: 54,
///             name: "ProjectionThreadsAutoSettleDisabledAt".to_owned(),
///         },
///         RecordedMigration { migration_id: 55, name: "OrchestrationV2".to_owned() },
///     ]),
/// );
/// ```
pub fn reconciled_v2_preview_history(
    history: &[RecordedMigration],
) -> Option<Vec<RecordedMigration>> {
    let later: Vec<&RecordedMigration> = history
        .iter()
        .filter(|row| row.migration_id >= 53)
        .collect();
    let legacy = later.iter().copied().find(|row| {
        row.name == "OrchestrationV2" && (row.migration_id == 53 || row.migration_id == 54)
    });

    let Some(legacy) = legacy else {
        return Some(history.to_vec());
    };

    let valid = later.iter().all(|row| {
        *row == legacy
            || (legacy.migration_id == 54
                && ((row.migration_id == 53 && row.name == "PullRequestFilesViewed")
                    || (row.migration_id == 55 && row.name == "RemoveRedundantProjectionIndexes")))
    });
    if !valid {
        return None;
    }

    let mut out: Vec<RecordedMigration> = history
        .iter()
        .filter(|row| row.migration_id < 53)
        .cloned()
        .collect();
    out.push(RecordedMigration {
        migration_id: 53,
        name: "PullRequestFilesViewed".to_owned(),
    });
    out.push(RecordedMigration {
        migration_id: 54,
        name: "ProjectionThreadsAutoSettleDisabledAt".to_owned(),
    });
    out.push(RecordedMigration {
        migration_id: 55,
        name: "OrchestrationV2".to_owned(),
    });
    if history.iter().any(|row| row.migration_id == 55) {
        out.push(RecordedMigration {
            migration_id: 56,
            name: "RemoveRedundantProjectionIndexes".to_owned(),
        });
    }
    Some(out)
}

/// Identifies missing or mismatched V2 migrations up through the latest recorded migration ID
/// (`Migrations.ts:233-249`).
///
/// # Examples
///
/// ```
/// use t3_persistence::service_preflight::skipped_v2_migrations;
/// use t3_persistence::sqlite::RecordedMigration;
///
/// let history = vec![
///     RecordedMigration { migration_id: 55, name: "OrchestrationV2".to_owned() },
///     RecordedMigration { migration_id: 57, name: "ScheduledTaskWebhooks".to_owned() },
/// ];
/// assert_eq!(
///     skipped_v2_migrations(&history),
///     vec!["56_RemoveRedundantProjectionIndexes (recorded: nothing)"],
/// );
/// ```
pub fn skipped_v2_migrations(recorded: &[RecordedMigration]) -> Vec<String> {
    let latest_recorded_id = recorded
        .iter()
        .map(|row| row.migration_id)
        .max()
        .unwrap_or(0);
    let recorded_by_id: HashMap<i64, &str> = recorded
        .iter()
        .map(|row| (row.migration_id, row.name.as_str()))
        .collect();

    let mut skipped = Vec::new();
    for &(id, expected_name) in V2_MIGRATION_MANIFEST {
        if id >= FIRST_V2_MIGRATION_ID && id <= latest_recorded_id {
            match recorded_by_id.get(&id).copied() {
                Some(name) if name == expected_name => {}
                other => {
                    let recorded_label = other.unwrap_or("nothing");
                    skipped.push(format!("{id}_{expected_name} (recorded: {recorded_label})"));
                }
            }
        }
    }
    skipped
}

/// Checks launcher protocol and database migration compatibility prior to starting `t3`
/// (`servicePreflight.ts:39-93`).
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use t3_persistence::service_preflight::{ServicePreflightResult, run_service_preflight};
///
/// let dir = tempfile::tempdir()?;
/// let db_path = dir.path().join("statev2.sqlite");
/// let res = run_service_preflight(&db_path, 4, "1.2.3");
/// assert_eq!(
///     serde_json::to_string(&res)?,
///     r#"{"status":"ready","version":"1.2.3","launcherProtocol":4}"#,
/// );
/// # Ok(())
/// # }
/// ```
pub fn run_service_preflight(
    db_path: &Path,
    launcher_protocol: i64,
    version: &str,
) -> ServicePreflightResult {
    if !is_supported_launcher_protocol(launcher_protocol) {
        return ServicePreflightResult::Blocked {
            version: version.to_owned(),
            reason: format!(
                "The installed service launcher speaks protocol {launcher_protocol}, but t3@{version} requires protocol {SERVICE_LAUNCHER_PROTOCOL}. On the server machine, install this exact CLI: `curl -fsSL https://t3.codes/install.sh | T3CODE_VERSION={version} sh`. Check that `t3 --version` prints {version} and that `which -a t3` lists no older t3 (such as an npm or nvm install) first, then run `~/.local/bin/t3 service install` (the installer's default path). Updating the desktop app does not update this CLI, and `t3 service update` only repairs the service with whichever CLI runs it."
            ),
        };
    }

    let Some(recorded) = recorded_migrations(db_path) else {
        return ServicePreflightResult::Blocked {
            version: version.to_owned(),
            reason: format!("Could not read this database to confirm t3@{version} supports it."),
        };
    };

    let newer: Vec<String> = recorded
        .iter()
        .filter(|row| row.migration_id > BUILD_MIGRATION_HEAD)
        .map(|row| format!("{}_{}", row.migration_id, row.name))
        .collect();
    if !newer.is_empty() {
        let joined = newer.join(", ");
        return ServicePreflightResult::Blocked {
            version: version.to_owned(),
            reason: format!(
                "This database has migrations that t3@{version} does not know: {joined}. A newer build wrote them, and running t3@{version} on this database is not safe. Stay on the newer release, or wait for a release that includes these migrations."
            ),
        };
    }

    let incompatible = match reconciled_v2_preview_history(&recorded) {
        Some(reconciled) => skipped_v2_migrations(&reconciled),
        None => vec!["an unexpected V2 preview history".to_owned()],
    };
    if !incompatible.is_empty() {
        let joined = incompatible.join(", ");
        return ServicePreflightResult::Blocked {
            version: version.to_owned(),
            reason: format!(
                "This database has incompatible migration history: {joined}. Start the build that wrote it, or report this migration history so an upgrade path can be added."
            ),
        };
    }

    ServicePreflightResult::Ready {
        version: version.to_owned(),
        launcher_protocol,
    }
}
