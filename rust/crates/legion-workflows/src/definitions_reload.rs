#![forbid(unsafe_code)]
//! Hot-reload reconciliation, conflict checking, retired version archiving, and engine rebuild.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::definitions_fs::{
    EvaluatedFile, LoadedBackend, RETRY_DIR_MS, RETRY_HELD_MS, WorkflowsLocation,
    evaluate_workflow_file, scan_workflow_sources,
};
use crate::engine::{WorkflowDefinition, WorkflowEngine};
use crate::store::WorkflowStore;
use crate::traits::{WorkflowAgentDriver, WorkflowForgeDriver};
use crate::types::WorkflowDefinitionProblem;

pub(crate) struct LoadedRecord {
    pub source: String,
    pub defs: Vec<WorkflowDefinition>,
    pub backend: LoadedBackend,
}

pub(crate) struct FailedRecord {
    pub problems: Vec<WorkflowDefinitionProblem>,
    pub next_retry: Instant,
}

pub(crate) struct RetiredVersion {
    pub def: WorkflowDefinition,
    pub _backend: LoadedBackend,
}

pub(crate) struct ManagerInner {
    pub location: WorkflowsLocation,
    pub revision: u64,
    pub dir_problem: Option<String>,
    pub dir_next_retry: Option<Instant>,
    pub disk_sources: BTreeMap<String, String>,
    pub loaded: BTreeMap<String, LoadedRecord>,
    pub failed: BTreeMap<String, FailedRecord>,
    pub retired: HashMap<(String, i64), RetiredVersion>,
    pub engine: Arc<WorkflowEngine>,
    pub enabled: bool,
}

pub(crate) fn perform_reload(
    inner: &Mutex<ManagerInner>,
    store: &WorkflowStore,
    agents: &Arc<dyn WorkflowAgentDriver>,
    forge: &Arc<dyn WorkflowForgeDriver>,
    force_retry: bool,
) -> Vec<WorkflowDefinitionProblem> {
    let location = {
        let Ok(g) = inner.lock() else {
            return Vec::new();
        };
        if !force_retry
            && let Some(retry_at) = g.dir_next_retry
            && Instant::now() < retry_at
        {
            return Vec::new();
        }
        g.location.clone()
    };

    let scanned = match scan_workflow_sources(&location) {
        Ok(s) => s,
        Err(msg) => {
            if let Ok(mut g) = inner.lock() {
                g.dir_problem = Some(msg.clone());
                g.dir_next_retry = Some(Instant::now() + Duration::from_millis(RETRY_DIR_MS));
                g.revision += 1;
            }
            return vec![WorkflowDefinitionProblem {
                message: msg,
                file: None,
                line: None,
            }];
        }
    };

    let Some((to_eval, deleted_names)) = collect_pending_changes(inner, scanned, force_retry)
    else {
        return Vec::new();
    };

    let mut evaluated_ok: Vec<(String, String, EvaluatedFile)> = Vec::new();
    let mut evaluated_err: Vec<(String, WorkflowDefinitionProblem)> = Vec::new();
    for (name, src) in to_eval {
        let path = location.directory.join(&name);
        match evaluate_workflow_file(Some(&location.directory), &path, &name, &src) {
            Ok(ev) => evaluated_ok.push((name, src, ev)),
            Err(prob) => evaluated_err.push((name, prob)),
        }
    }

    let Ok(mut g) = inner.lock() else {
        return Vec::new();
    };
    for del in deleted_names {
        archive_retired(&mut g, &del, store);
        g.loaded.remove(&del);
        g.failed.remove(&del);
    }
    for (name, prob) in evaluated_err {
        g.failed.insert(
            name,
            FailedRecord {
                problems: vec![prob],
                next_retry: Instant::now() + Duration::from_millis(RETRY_HELD_MS),
            },
        );
    }
    for (name, src, ev) in evaluated_ok {
        if let Some(prob) = find_conflict_against_loaded(&g.loaded, &name, &ev.definitions) {
            g.failed.insert(
                name,
                FailedRecord {
                    problems: vec![prob],
                    next_retry: Instant::now() + Duration::from_millis(RETRY_HELD_MS),
                },
            );
        } else {
            archive_retired(&mut g, &name, store);
            g.failed.remove(&name);
            g.loaded.insert(
                name,
                LoadedRecord {
                    source: src,
                    defs: ev.definitions,
                    backend: ev.backend,
                },
            );
        }
    }
    rebuild_engine(&mut g, store, agents, forge);
    collect_all_problems(&g)
}

type PendingScan = (Vec<(String, String)>, Vec<String>);

fn collect_pending_changes(
    inner: &Mutex<ManagerInner>,
    scanned: BTreeMap<String, String>,
    force_retry: bool,
) -> Option<PendingScan> {
    let now = Instant::now();
    let mut g = inner.lock().ok()?;
    g.dir_problem = None;
    g.dir_next_retry = None;
    let deleted: Vec<String> = g
        .disk_sources
        .keys()
        .filter(|k| !scanned.contains_key(*k))
        .cloned()
        .collect();
    let mut eval_list = Vec::new();
    for (name, src) in &scanned {
        let prev = g.disk_sources.get(name);
        let src_changed = prev.is_none_or(|p| p != src);
        let due_retry = g
            .failed
            .get(name)
            .is_some_and(|f| force_retry || !deleted.is_empty() || now >= f.next_retry);
        if src_changed || due_retry {
            eval_list.push((name.clone(), src.clone()));
        }
    }
    let any_change = !deleted.is_empty() || !eval_list.is_empty();
    g.disk_sources = scanned;
    any_change.then_some((eval_list, deleted))
}

pub(crate) fn find_conflict_against_loaded(
    loaded: &BTreeMap<String, LoadedRecord>,
    file_name: &str,
    defs: &[WorkflowDefinition],
) -> Option<WorkflowDefinitionProblem> {
    for (other_file, rec) in loaded {
        if other_file == file_name {
            continue;
        }
        for existing in &rec.defs {
            if defs
                .iter()
                .any(|d| d.name == existing.name && d.version == existing.version)
            {
                return Some(WorkflowDefinitionProblem {
                    message: format!(
                        "Workflow '{}@{}' is already defined in '{other_file}'.",
                        existing.name, existing.version
                    ),
                    file: Some(file_name.to_owned()),
                    line: None,
                });
            }
        }
    }
    None
}

pub(crate) fn archive_retired(g: &mut ManagerInner, file_name: &str, store: &WorkflowStore) {
    let Some(prev) = g.loaded.get(file_name) else {
        return;
    };
    let Ok(unfinished) = store.unfinished() else {
        return;
    };
    let active_set: HashSet<(String, i64)> = unfinished.into_iter().collect();
    for def in &prev.defs {
        let key = (def.name.clone(), def.version);
        if active_set.contains(&key) {
            g.retired.insert(
                key,
                RetiredVersion {
                    def: def.clone(),
                    _backend: prev.backend.clone(),
                },
            );
        }
    }
}

pub(crate) fn rebuild_engine(
    g: &mut ManagerInner,
    store: &WorkflowStore,
    agents: &Arc<dyn WorkflowAgentDriver>,
    forge: &Arc<dyn WorkflowForgeDriver>,
) {
    let unfinished: HashSet<(String, i64)> =
        store.unfinished().unwrap_or_default().into_iter().collect();
    let mut defs: Vec<WorkflowDefinition> = g
        .loaded
        .values()
        .flat_map(|r| r.defs.iter().cloned())
        .collect();
    let present: HashSet<(String, i64)> =
        defs.iter().map(|d| (d.name.clone(), d.version)).collect();
    g.retired
        .retain(|k, _| unfinished.contains(k) && !present.contains(k));
    for ret in g.retired.values() {
        defs.push(ret.def.clone());
    }
    if let Ok(next_engine) = WorkflowEngine::new(
        store.clone(),
        defs,
        Arc::clone(agents),
        Arc::clone(forge),
        g.enabled,
    ) {
        g.engine = next_engine;
    }
    g.revision += 1;
}

pub(crate) fn collect_all_problems(g: &ManagerInner) -> Vec<WorkflowDefinitionProblem> {
    let mut probs: Vec<WorkflowDefinitionProblem> = g
        .failed
        .values()
        .flat_map(|f| f.problems.iter().cloned())
        .collect();
    if let Some(msg) = &g.dir_problem {
        probs.push(WorkflowDefinitionProblem {
            message: msg.clone(),
            file: None,
            line: None,
        });
    }
    probs
}
