//! Workflow triggers (schedule, webhook, opened issues/PRs), hook routing, and signature safety.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use crate::time::{ScheduleSpec, format_duration_human, parse_time_of_day};
use serde::{Deserialize, Serialize};

pub use crate::hooks_util::{
    constant_time_equals, encode_uri_component, hook_id, hook_path, reference_from_opened_entry,
    resolve_webhook_key,
};

/// Prefix separating workflow webhook IDs from scheduled-task webhook IDs.
pub const HOOK_PREFIX: &str = "wf-";

/// Delivery headers checked for webhook retry deduplication in priority order.
pub const DELIVERY_HEADERS: &[&str] = &[
    "x-github-delivery",
    "x-gitea-delivery",
    "x-gitlab-event-uuid",
    "linear-delivery",
    "webhook-id",
    "idempotency-key",
];

const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Incoming HTTP webhook request handed to workflow triggers and payload validators.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebhookRequest {
    /// Uppercase HTTP method (`POST`, `PUT`, `PATCH`, `GET`).
    pub method: String,
    /// Lowercased HTTP header map.
    pub headers: BTreeMap<String, String>,
    /// Raw query string without leading `?`.
    pub query: String,
    /// UTF-8 request body.
    pub body: String,
}

/// Incoming webhook delivery dispatch input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookDeliveryInput {
    /// Hook identifier (`wf-<workflow>.<name>`).
    pub hook_id: String,
    /// Secret path token.
    pub token: String,
    /// Captured HTTP request.
    pub request: WebhookRequest,
    /// Optional relay-verified delivery ID.
    pub delivery_id: Option<String>,
}

/// Callback extracting a custom deduplication key from a [`WebhookRequest`].
pub type WebhookKeyFn =
    Arc<dyn Fn(&WebhookRequest) -> Result<Option<String>, String> + Send + Sync>;

/// Trigger specification attached to a workflow definition.
#[derive(Clone)]
pub enum TriggerDef {
    /// Interval or fixed-time schedule trigger.
    Schedule {
        /// Stable trigger ID (`every:<ms>` or `daily:<time>:<days>`).
        id: String,
        /// Schedule specification.
        schedule: ScheduleSpec,
    },
    /// Inbound HTTP webhook trigger.
    Webhook {
        /// Hook name unique within the workflow.
        name: String,
        /// Optional custom key extraction function.
        key_fn: Option<WebhookKeyFn>,
    },
    /// Polling trigger for newly opened issues in a project.
    IssuesOpened {
        /// Stable trigger ID.
        id: String,
        /// Project ID or workspace path.
        project: String,
        /// Labels every matched issue must carry.
        labels: Vec<String>,
        /// Optional search query.
        query: Option<String>,
    },
    /// Polling trigger for newly opened pull requests in a project.
    PullRequestsOpened {
        /// Stable trigger ID.
        id: String,
        /// Project ID or workspace path.
        project: String,
        /// Optional search query.
        query: Option<String>,
    },
}

impl std::fmt::Debug for TriggerDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Schedule { id, schedule } => f
                .debug_struct("Schedule")
                .field("id", id)
                .field("schedule", schedule)
                .finish(),
            Self::Webhook { name, .. } => f.debug_struct("Webhook").field("name", name).finish(),
            Self::IssuesOpened {
                id,
                project,
                labels,
                query,
            } => f
                .debug_struct("IssuesOpened")
                .field("id", id)
                .field("project", project)
                .field("labels", labels)
                .field("query", query)
                .finish(),
            Self::PullRequestsOpened { id, project, query } => f
                .debug_struct("PullRequestsOpened")
                .field("id", id)
                .field("project", project)
                .field("query", query)
                .finish(),
        }
    }
}

/// Constructs an interval schedule trigger (`every_ms >= 60_000`).
pub fn every_trigger(every_ms: i64) -> Result<TriggerDef, String> {
    if every_ms < 60_000 {
        return Err(format!(
            "every({every_ms}) is shorter than the shortest, one minute."
        ));
    }
    Ok(TriggerDef::Schedule {
        id: format!("every:{every_ms}"),
        schedule: ScheduleSpec::Interval { every_ms },
    })
}

/// Constructs a fixed-time daily schedule trigger (`HH:MM` and optional weekdays `0..=6`).
pub fn daily_trigger(time_of_day: &str, weekdays: Option<Vec<u8>>) -> Result<TriggerDef, String> {
    if parse_time_of_day(time_of_day).is_none() {
        return Err(format!(
            "daily({time_of_day:?}) needs a 24-hour time such as \"09:00\"."
        ));
    }
    if let Some(ref days) = weekdays
        && days.iter().any(|d| *d > 6)
    {
        let joined = days
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "daily's weekdays are 0 (Sunday) to 6 (Saturday), not {joined}."
        ));
    }
    let day_key = match &weekdays {
        None => "all".to_owned(),
        Some(days) => {
            let mut sorted = days.clone();
            sorted.sort_unstable();
            sorted
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(",")
        }
    };
    Ok(TriggerDef::Schedule {
        id: format!("daily:{time_of_day}:{day_key}"),
        schedule: ScheduleSpec::FixedTime {
            time_of_day: time_of_day.to_owned(),
            weekdays,
        },
    })
}

/// Builds a stable `issues.opened` trigger ID (`JSON.stringify(["issues", project, sortedLabels, query])`).
pub fn issues_opened_trigger(
    project: String,
    labels: Vec<String>,
    query: Option<String>,
) -> TriggerDef {
    let mut sorted = labels.clone();
    sorted.sort();
    let id =
        serde_json::json!(["issues", project, sorted, query.as_deref().unwrap_or("")]).to_string();
    TriggerDef::IssuesOpened {
        id,
        project,
        labels,
        query,
    }
}

/// Builds a stable `pullRequests.opened` trigger ID.
pub fn pull_requests_opened_trigger(project: String, query: Option<String>) -> TriggerDef {
    let id = serde_json::json!([
        "pullRequests",
        project,
        Vec::<String>::new(),
        query.as_deref().unwrap_or("")
    ])
    .to_string();
    TriggerDef::PullRequestsOpened { id, project, query }
}

/// Validates that a workflow does not declare duplicate hooks or duplicate trigger IDs.
pub fn check_triggers(workflow: &str, triggers: &[TriggerDef]) -> Result<(), String> {
    let mut seen = HashSet::new();
    for trigger in triggers {
        let key = match trigger {
            TriggerDef::Webhook { name, .. } => format!("webhook:{name}"),
            TriggerDef::Schedule { id, .. }
            | TriggerDef::IssuesOpened { id, .. }
            | TriggerDef::PullRequestsOpened { id, .. } => id.clone(),
        };
        if !seen.insert(key) {
            return Err(match trigger {
                TriggerDef::Webhook { name, .. } => format!(
                    "workflow({workflow:?}) has two hooks named {name:?}. Give each its own name."
                ),
                TriggerDef::Schedule { .. } => {
                    format!("workflow({workflow:?}) lists the same schedule twice. List it once.")
                }
                TriggerDef::IssuesOpened { .. } | TriggerDef::PullRequestsOpened { .. } => {
                    format!("workflow({workflow:?}) lists the same trigger twice. List it once.")
                }
            });
        }
    }
    Ok(())
}

/// Formats a trigger description for [`WorkflowSummary::triggers`](crate::types::WorkflowSummary).
pub fn format_trigger_summary(trigger: &TriggerDef) -> String {
    match trigger {
        TriggerDef::Webhook { name, .. } => format!("webhook {name}"),
        TriggerDef::IssuesOpened { project, .. } => format!("new issues in {project}"),
        TriggerDef::PullRequestsOpened { project, .. } => {
            format!("new pull requests in {project}")
        }
        TriggerDef::Schedule { schedule, .. } => match schedule {
            ScheduleSpec::Interval { every_ms } => {
                format!("every {}", format_duration_human(*every_ms))
            }
            ScheduleSpec::FixedTime {
                time_of_day,
                weekdays,
            } => match weekdays {
                None => format!("daily at {time_of_day}"),
                Some(days) => {
                    let labels: Vec<String> = days
                        .iter()
                        .map(|d| {
                            WEEKDAYS
                                .get(usize::from(*d))
                                .map_or_else(|| d.to_string(), |s| (*s).to_owned())
                        })
                        .collect();
                    format!("daily at {time_of_day} on {}", labels.join(", "))
                }
            },
        },
    }
}
