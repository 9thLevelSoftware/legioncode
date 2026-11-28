#![forbid(unsafe_code)]
//! Live workflow definitions manager, hot-reload, verification-before-save, and file CRUD.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::definitions_fs::{
    EvaluatedFile, WorkflowsLocation, evaluate_workflow_file, is_valid_workflow_filename,
};
use crate::definitions_reload::{
    LoadedRecord, ManagerInner, archive_retired, collect_all_problems, perform_reload,
    rebuild_engine,
};
use crate::engine::{WorkflowDefinition, WorkflowEngine};
use crate::store::WorkflowStore;
use crate::traits::{WorkflowAgentDriver, WorkflowForgeDriver};
use crate::types::{
    WorkflowDefinitionChangeResult, WorkflowDefinitionFile, WorkflowDefinitionFiles,
    WorkflowDefinitionProblem, WorkflowDefinitionState, WorkflowError, WorkflowRunsSnapshot,
};

/// Manages `<home>/userdata/workflows` files and keeps an active [`WorkflowEngine`] synchronized.
pub struct WorkflowDefinitionsManager {
    store: WorkflowStore,
    agents: Arc<dyn WorkflowAgentDriver>,
    forge: Arc<dyn WorkflowForgeDriver>,
    inner: Mutex<ManagerInner>,
}

impl WorkflowDefinitionsManager {
    /// Initializes definitions from `location` and builds the initial [`WorkflowEngine`].
    pub fn open(
        store: WorkflowStore,
        agents: Arc<dyn WorkflowAgentDriver>,
        forge: Arc<dyn WorkflowForgeDriver>,
        location: WorkflowsLocation,
        workflows_enabled: bool,
    ) -> Result<Arc<Self>, WorkflowError> {
        Self::open_with_builtins(
            store,
            agents,
            forge,
            location,
            Vec::new(),
            workflows_enabled,
        )
    }

    /// Initializes definitions from `location` alongside native Rust built-in workflow definitions.
    pub fn open_with_builtins(
        store: WorkflowStore,
        agents: Arc<dyn WorkflowAgentDriver>,
        forge: Arc<dyn WorkflowForgeDriver>,
        location: WorkflowsLocation,
        builtins: Vec<WorkflowDefinition>,
        workflows_enabled: bool,
    ) -> Result<Arc<Self>, WorkflowError> {
        let engine = WorkflowEngine::new(
            store.clone(),
            builtins.clone(),
            Arc::clone(&agents),
            Arc::clone(&forge),
            workflows_enabled,
        )?;
        let mgr = Arc::new(Self {
            store,
            agents,
            forge,
            inner: Mutex::new(ManagerInner {
                location,
                revision: 0,
                dir_problem: None,
                dir_next_retry: None,
                builtins,
                disk_sources: BTreeMap::new(),
                loaded: BTreeMap::new(),
                failed: BTreeMap::new(),
                retired: HashMap::new(),
                engine,
                enabled: workflows_enabled,
            }),
        });
        let _ = mgr.reload_now(true);
        Ok(mgr)
    }

    /// Returns the currently active [`WorkflowEngine`] snapshot.
    pub fn engine(&self) -> Arc<WorkflowEngine> {
        self.inner.lock().map_or_else(
            |e| Arc::clone(&e.into_inner().engine),
            |g| Arc::clone(&g.engine),
        )
    }

    /// Toggles Labs `workflowsEnabled` dynamically across current and future reloaded engines.
    pub fn set_enabled(&self, enabled: bool) {
        if let Ok(mut g) = self.inner.lock() {
            g.enabled = enabled;
            g.engine.set_enabled(enabled);
        }
    }

    /// Returns current definition reload revision and optional directory error.
    pub fn definition_state(&self) -> WorkflowDefinitionState {
        let g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        WorkflowDefinitionState {
            revision: g.revision,
            problem: g.dir_problem.clone(),
        }
    }

    /// Reads workflow files, workflow names per file, problems, and definition state.
    pub fn read(&self) -> WorkflowDefinitionFiles {
        let g = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let mut all_names: BTreeMap<String, String> = g.disk_sources.clone();
        for name in g.loaded.keys() {
            all_names
                .entry(name.clone())
                .or_insert_with(|| g.loaded.get(name).map_or_default(|r| r.source.clone()));
        }
        let mut files = Vec::new();
        let mut problems = Vec::new();
        for (name, source) in all_names {
            let file_probs = g.failed.get(&name).map_or_default(|f| f.problems.clone());
            problems.extend(file_probs.clone());
            let wf_names: Vec<String> = g.loaded.get(&name).map_or_default(|r| {
                let mut seen = HashSet::new();
                r.defs
                    .iter()
                    .filter(|d| seen.insert(d.name.clone()))
                    .map(|d| d.name.clone())
                    .collect()
            });
            files.push(WorkflowDefinitionFile {
                name,
                source,
                workflows: wf_names,
                problems: file_probs,
            });
        }
        WorkflowDefinitionFiles {
            directory: g.location.directory.to_string_lossy().into_owned(),
            single: g.location.single.is_some(),
            files,
            problems,
            revision: g.revision,
            problem: g.dir_problem.clone(),
        }
    }

    /// Returns `workflows.list`-compatible snapshot annotated with definition state.
    pub fn snapshot(&self) -> Result<WorkflowRunsSnapshot, WorkflowError> {
        let engine = self.engine();
        engine.snapshot(Some(self.definition_state()))
    }

    /// Verifies `source` via a dot-prefixed temp file beside `name` before writing to disk.
    pub fn save(
        &self,
        name: &str,
        source: &str,
    ) -> Result<WorkflowDefinitionChangeResult, WorkflowError> {
        if !is_valid_workflow_filename(name) {
            return Err(WorkflowError::Input {
                problem: format!(
                    "Invalid workflow filename '{name}'. Use [A-Za-z0-9_][A-Za-z0-9_.-]*.(ts|mts|js|mjs)."
                ),
            });
        }
        let location = {
            let g = self.inner.lock().map_err(|_| lock_err())?;
            if let Some(single) = &g.location.single
                && single != name
            {
                return Err(WorkflowError::Input {
                    problem: format!("Single-file mode only allows saving '{single}'."),
                });
            }
            g.location.clone()
        };

        std::fs::create_dir_all(&location.directory).map_err(|e| WorkflowError::Storage {
            message: format!(
                "Cannot create workflows folder {}: {e}",
                location.directory.display()
            ),
        })?;

        let temp_name = format!(".verify-{}-{name}", Uuid::new_v4().simple());
        let temp_path = location.directory.join(&temp_name);
        std::fs::write(&temp_path, source).map_err(|e| WorkflowError::Storage {
            message: format!("Cannot write verification temp file: {e}"),
        })?;
        let eval_res = evaluate_workflow_file(Some(&location.directory), &temp_path, name, source);
        let _ = std::fs::remove_file(&temp_path);

        let evaluated = match eval_res {
            Ok(ev) => ev,
            Err(problem) => {
                return Ok(WorkflowDefinitionChangeResult {
                    changed: false,
                    problems: vec![problem],
                });
            }
        };

        if let Some(conflict) = self.check_cross_file_conflicts(name, &evaluated.defs_tags()) {
            return Ok(WorkflowDefinitionChangeResult {
                changed: false,
                problems: vec![conflict],
            });
        }

        let target_path = location.directory.join(name);
        let prev_disk = std::fs::read_to_string(&target_path).ok();
        let changed = prev_disk.as_deref() != Some(source);
        std::fs::write(&target_path, source).map_err(|e| WorkflowError::Storage {
            message: format!("Cannot write workflow file {}: {e}", target_path.display()),
        })?;

        let problems = self.install_verified(name, source, evaluated);
        Ok(WorkflowDefinitionChangeResult { changed, problems })
    }

    /// Deletes a workflow file cleanly and reconciles the registry (holding active retired versions).
    pub fn remove(&self, name: &str) -> Result<WorkflowDefinitionChangeResult, WorkflowError> {
        if !is_valid_workflow_filename(name) {
            return Err(WorkflowError::Input {
                problem: format!("Invalid workflow filename '{name}'."),
            });
        }
        let target: PathBuf = {
            let g = self.inner.lock().map_err(|_| lock_err())?;
            if g.location.single.is_some() {
                return Err(WorkflowError::Input {
                    problem: "Cannot remove workflow files in single-file mode.".to_owned(),
                });
            }
            g.location.directory.join(name)
        };
        if !target.exists() {
            return Ok(WorkflowDefinitionChangeResult {
                changed: false,
                problems: Vec::new(),
            });
        }
        std::fs::remove_file(&target).map_err(|e| WorkflowError::Storage {
            message: format!("Cannot remove workflow file {}: {e}", target.display()),
        })?;
        let problems = self.reload_now(true);
        Ok(WorkflowDefinitionChangeResult {
            changed: true,
            problems,
        })
    }

    /// Re-scans the workflows directory and reconciles changes + retry timers.
    pub fn reload_now(&self, force_retry: bool) -> Vec<WorkflowDefinitionProblem> {
        perform_reload(
            &self.inner,
            &self.store,
            &self.agents,
            &self.forge,
            force_retry,
        )
    }

    /// Spawns a periodic background watcher checking folder edits and 15s/30s retries.
    pub fn spawn_watcher(self: &Arc<Self>) -> JoinHandle<()> {
        let mgr = Arc::clone(self);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(2));
            loop {
                interval.tick().await;
                let m = Arc::clone(&mgr);
                let _ = tokio::task::spawn_blocking(move || m.reload_now(false)).await;
            }
        })
    }

    fn check_cross_file_conflicts(
        &self,
        name: &str,
        tags: &[(String, i64)],
    ) -> Option<WorkflowDefinitionProblem> {
        let g = self.inner.lock().ok()?;
        for builtin in &g.builtins {
            if tags
                .iter()
                .any(|(n, v)| n == &builtin.name && *v == builtin.version)
            {
                return Some(WorkflowDefinitionProblem {
                    message: format!(
                        "Workflow '{}@{}' is reserved by a built-in stage job.",
                        builtin.name, builtin.version
                    ),
                    file: Some(name.to_owned()),
                    line: None,
                });
            }
        }
        for (other_file, rec) in &g.loaded {
            if other_file == name {
                continue;
            }
            for def in &rec.defs {
                if tags
                    .iter()
                    .any(|(n, v)| n == &def.name && *v == def.version)
                {
                    return Some(WorkflowDefinitionProblem {
                        message: format!(
                            "Workflow '{}@{}' is already defined in '{other_file}'.",
                            def.name, def.version
                        ),
                        file: Some(name.to_owned()),
                        line: None,
                    });
                }
            }
        }
        None
    }

    fn install_verified(
        &self,
        name: &str,
        source: &str,
        ev: EvaluatedFile,
    ) -> Vec<WorkflowDefinitionProblem> {
        let Ok(mut g) = self.inner.lock() else {
            return Vec::new();
        };
        archive_retired(&mut g, name, &self.store);
        g.disk_sources.insert(name.to_owned(), source.to_owned());
        g.failed.remove(name);
        g.loaded.insert(
            name.to_owned(),
            LoadedRecord {
                source: source.to_owned(),
                defs: ev.definitions,
                backend: ev.backend,
            },
        );
        rebuild_engine(&mut g, &self.store, &self.agents, &self.forge);
        collect_all_problems(&g)
    }
}

impl EvaluatedFile {
    fn defs_tags(&self) -> Vec<(String, i64)> {
        self.definitions
            .iter()
            .map(|d| (d.name.clone(), d.version))
            .collect()
    }
}

fn lock_err() -> WorkflowError {
    WorkflowError::Storage {
        message: "Workflow definitions lock poisoned".to_owned(),
    }
}
