//! Durable workflow engine registry, read queries, Labs gating, and engine control bridge.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value};
use uuid::Uuid;

use crate::engine_queries::{ArcEngineControl, compute_latest_defs};
use crate::engine_replay::ReplayContext;
use crate::engine_step::{
    EngineControl, InFlightTracker, PassOutcome, RunEndInfo, StepExecContext,
};
use crate::journal::WorkflowJournal;
use crate::store::{RunRow, WorkflowStore};
use crate::time::{format_millis_iso, parse_iso_millis};
use crate::traits::{
    BoxFuture, UnavailableAgentDriver, UnavailableForgeDriver, WorkflowAgentDriver,
    WorkflowForgeDriver,
};
use crate::triggers_hooks::{TriggerDef, WebhookKeyFn, check_triggers, hook_id, hook_path};
use crate::types::{WorkflowError, WorkflowRun, WorkflowRunStatus};

/// User-facing error when Labs `workflowsEnabled` is false.
pub const WORKFLOWS_OFF_MESSAGE: &str = "Workflows are off on this server. Turn them on in its Settings → Labs → Workflows, or without a desktop, set \"workflowsEnabled\": true in userdata/settings.json in its LegionCode home (~/.legioncode by default).";

/// User-facing error when Labs is on but no workflows are defined.
pub const WORKFLOWS_NO_DEFS_MESSAGE: &str = "This server defines no workflows yet. Add a file to the workflows folder of its LegionCode home.";

/// Execution driver implemented by native Rust workflows, Boa JS modules, and stdio workers.
pub trait WorkflowBodyDriver: Send + Sync {
    /// Runs one deterministic replay pass of the workflow body.
    fn run_pass<'a>(&'a self, ctx: &'a mut ReplayContext) -> BoxFuture<'a, PassOutcome>;

    /// Runs optional `onEnd(payload, end)` cleanup when a run ends or is cancelled.
    fn run_on_end<'a>(
        &'a self,
        _ctx: &'a mut StepExecContext,
        _payload: Value,
        _end: RunEndInfo,
    ) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async { Ok(()) })
    }
}

/// Registered definition of one workflow version.
#[derive(Clone)]
pub struct WorkflowDefinition {
    /// Workflow name.
    pub name: String,
    /// Monotonic version number.
    pub version: i64,
    /// JSON Schema for input payloads.
    pub payload_schema: Map<String, Value>,
    /// Optional JSON Schema for return results (`None` accepts any JSON).
    pub returns_schema: Option<Map<String, Value>>,
    /// Automatic resume delays (ms) for consecutive step-failure pauses.
    pub resume_after_ms: Vec<i64>,
    /// Schedule, webhook, issue, and PR triggers.
    pub triggers: Vec<TriggerDef>,
    /// Whether this definition provides an `onEnd` cleanup handler.
    pub has_on_end: bool,
    /// Execution driver.
    pub driver: Arc<dyn WorkflowBodyDriver>,
}

impl WorkflowDefinition {
    /// Canonical `<name>@<version>` engine tag.
    pub fn engine_tag(&self) -> String {
        format!("{}@{}", self.name, self.version)
    }
}

#[derive(Clone)]
pub(crate) struct RegisteredHook {
    pub hook_id: String,
    pub token: String,
    pub workflow: String,
    pub name: String,
    pub path: String,
    pub key_fn: Option<WebhookKeyFn>,
}

/// Core durable workflow engine coordinating replay execution, waits, signals, hooks, and sweeps.
pub struct WorkflowEngine {
    pub(crate) store: WorkflowStore,
    pub(crate) journal: WorkflowJournal,
    pub(crate) agents: Arc<dyn WorkflowAgentDriver>,
    pub(crate) forge: Arc<dyn WorkflowForgeDriver>,
    pub(crate) in_flight: InFlightTracker,
    pub(crate) definitions: Vec<WorkflowDefinition>,
    pub(crate) by_tag: HashMap<String, WorkflowDefinition>,
    pub(crate) hooks: Vec<RegisteredHook>,
    pub(crate) enabled: Arc<Mutex<bool>>,
    pub(crate) resuming: Arc<Mutex<HashSet<String>>>,
    pub(crate) cancelling: Arc<Mutex<HashSet<String>>>,
    pub(crate) ending: Arc<Mutex<HashSet<String>>>,
    pub(crate) run_locks: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>>,
    pub(crate) looked_at: Arc<Mutex<HashMap<String, i64>>>,
    pub(crate) turned_down: Arc<Mutex<HashSet<String>>>,
    pub(crate) no_checks_since: Arc<Mutex<HashMap<String, i64>>>,
}

impl WorkflowEngine {
    /// Constructs a workflow engine with explicit agent and forge drivers.
    pub fn new(
        store: WorkflowStore,
        definitions: Vec<WorkflowDefinition>,
        agents: Arc<dyn WorkflowAgentDriver>,
        forge: Arc<dyn WorkflowForgeDriver>,
        workflows_enabled: bool,
    ) -> Result<Arc<Self>, WorkflowError> {
        let conn = store.connection();
        let journal = WorkflowJournal::new(conn);
        for def in &definitions {
            check_triggers(&def.name, &def.triggers)
                .map_err(|problem| WorkflowError::Input { problem })?;
        }
        let mut by_tag = HashMap::new();
        for def in &definitions {
            by_tag.insert(def.engine_tag(), def.clone());
        }
        let latest_defs = compute_latest_defs(&definitions);
        let kept_trigger_ids: Vec<String> = latest_defs
            .iter()
            .flat_map(|d| {
                d.triggers.iter().filter_map(|t| match t {
                    TriggerDef::Schedule { id, .. }
                    | TriggerDef::IssuesOpened { id, .. }
                    | TriggerDef::PullRequestsOpened { id, .. } => Some(format!("{}/{id}", d.name)),
                    TriggerDef::Webhook { .. } => None,
                })
            })
            .collect();
        store.triggers_keep(&kept_trigger_ids)?;

        let mut hooks = Vec::new();
        for def in &latest_defs {
            for trig in &def.triggers {
                if let TriggerDef::Webhook { name, key_fn } = trig {
                    let hid = hook_id(&def.name, name);
                    let fresh = Uuid::new_v4().to_string();
                    let token = store.hook_token(&hid, &fresh)?;
                    let path = hook_path(&hid, &token);
                    hooks.push(RegisteredHook {
                        hook_id: hid,
                        token,
                        workflow: def.name.clone(),
                        name: name.clone(),
                        path,
                        key_fn: key_fn.clone(),
                    });
                }
            }
        }

        Ok(Arc::new(Self {
            store,
            journal,
            agents,
            forge,
            in_flight: InFlightTracker::default(),
            definitions,
            by_tag,
            hooks,
            enabled: Arc::new(Mutex::new(workflows_enabled)),
            resuming: Arc::new(Mutex::new(HashSet::new())),
            cancelling: Arc::new(Mutex::new(HashSet::new())),
            ending: Arc::new(Mutex::new(HashSet::new())),
            run_locks: Arc::new(Mutex::new(HashMap::new())),
            looked_at: Arc::new(Mutex::new(HashMap::new())),
            turned_down: Arc::new(Mutex::new(HashSet::new())),
            no_checks_since: Arc::new(Mutex::new(HashMap::new())),
        }))
    }

    /// Creates a minimal engine with unavailable agent/forge drivers (useful in tests).
    pub fn minimal(
        store: WorkflowStore,
        definitions: Vec<WorkflowDefinition>,
        workflows_enabled: bool,
    ) -> Result<Arc<Self>, WorkflowError> {
        Self::new(
            store,
            definitions,
            Arc::new(UnavailableAgentDriver),
            Arc::new(UnavailableForgeDriver),
            workflows_enabled,
        )
    }

    /// Enables or disables workflow execution dynamically (Labs `workflowsEnabled` toggle).
    pub fn set_enabled(&self, on: bool) {
        if let Ok(mut g) = self.enabled.lock() {
            *g = on;
        }
    }

    /// Returns `true` when Labs `workflowsEnabled` is currently on.
    pub fn is_enabled(&self) -> bool {
        self.enabled.lock().is_ok_and(|g| *g)
    }

    pub(crate) fn require_enabled(&self) -> Result<(), WorkflowError> {
        if !self.is_enabled() {
            return Err(WorkflowError::Input {
                problem: WORKFLOWS_OFF_MESSAGE.to_owned(),
            });
        }
        Ok(())
    }

    pub(crate) fn require_active(&self) -> Result<(), WorkflowError> {
        self.require_enabled()?;
        if self.definitions.is_empty() {
            return Err(WorkflowError::Input {
                problem: WORKFLOWS_NO_DEFS_MESSAGE.to_owned(),
            });
        }
        Ok(())
    }

    pub(crate) fn require_run(&self, run_id: &str) -> Result<RunRow, WorkflowError> {
        self.store
            .run(run_id)?
            .ok_or_else(|| WorkflowError::RunNotFound {
                run_id: run_id.to_owned(),
            })
    }

    pub(crate) fn resolve_def(
        &self,
        name: &str,
        version: Option<i64>,
    ) -> Result<WorkflowDefinition, WorkflowError> {
        match version {
            Some(v) => self
                .definitions
                .iter()
                .find(|d| d.name == name && d.version == v)
                .cloned(),
            None => self
                .definitions
                .iter()
                .filter(|d| d.name == name)
                .max_by_key(|d| d.version)
                .cloned(),
        }
        .ok_or_else(|| WorkflowError::NotRegistered {
            workflow: name.to_owned(),
            version,
        })
    }

    pub(crate) fn resumes_at_ms(&self, row: &RunRow) -> Option<i64> {
        if row.status != WorkflowRunStatus::Paused || row.held == 1 {
            return None;
        }
        let idx = usize::try_from(row.pauses.saturating_sub(1)).ok()?;
        let delay = *self.by_tag.get(&row.engine_tag)?.resume_after_ms.get(idx)?;
        let updated_ms = parse_iso_millis(&row.updated_at)?;
        Some(updated_ms.saturating_add(delay))
    }

    pub(crate) fn to_wire_run(&self, row: &RunRow) -> WorkflowRun {
        let retired = row.ended_at.is_none() && !self.by_tag.contains_key(&row.engine_tag);
        let resumes_at = self.resumes_at_ms(row).map(format_millis_iso);
        WorkflowStore::to_wire_run(row, retired, resumes_at)
    }

    pub(crate) fn is_paused_or_resuming(&self, run_id: &str) -> Result<bool, WorkflowError> {
        if set_contains(&self.resuming, run_id) {
            return Ok(true);
        }
        Ok(self
            .store
            .run(run_id)?
            .is_some_and(|r| r.status == WorkflowRunStatus::Paused))
    }

    pub(crate) fn lock_for_run(&self, run_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        let Ok(mut g) = self.run_locks.lock() else {
            return Arc::new(tokio::sync::Mutex::new(()));
        };
        Arc::clone(
            g.entry(run_id.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(()))),
        )
    }

    pub(crate) fn control_handle(self: &Arc<Self>) -> Arc<dyn EngineControl> {
        Arc::new(ArcEngineControl(Arc::clone(self)))
    }
}

pub(crate) fn set_contains(set: &Mutex<HashSet<String>>, key: &str) -> bool {
    set.lock().is_ok_and(|g| g.contains(key))
}
