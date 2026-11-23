#![forbid(unsafe_code)]
//! Filesystem discovery, filename validation, and hybrid JS/TS workflow file evaluation.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::engine::WorkflowDefinition;
use crate::js_boa::{BoaHost, BoaLoadOutcome};
use crate::js_worker::JsWorkerHost;
use crate::types::WorkflowDefinitionProblem;

/// Env var overriding the `LegionCode` workflow directory or single file.
pub const LEGION_WORKFLOWS_ENV: &str = "LEGIONCODE_WORKFLOWS";
/// Legacy/compatible env var fallback for workflow directory or single file.
pub const T3_WORKFLOWS_ENV: &str = "T3CODE_WORKFLOWS";

/// Retry interval for broken/held workflow files (15s).
pub const RETRY_HELD_MS: u64 = 15_000;
/// Retry interval when the workflows directory itself cannot be read (30s).
pub const RETRY_DIR_MS: u64 = 30_000;

/// Resolved workflow folder or single-file configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowsLocation {
    /// Directory containing workflow files.
    pub directory: PathBuf,
    /// When pointing to a single file via env var, that filename.
    pub single: Option<String>,
}

/// Resolves the workflows location from `LEGIONCODE_WORKFLOWS`, `T3CODE_WORKFLOWS`, or `<home>/userdata/workflows`.
pub fn resolve_workflows_location(home_dir: &Path) -> WorkflowsLocation {
    let raw = std::env::var(LEGION_WORKFLOWS_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var(T3_WORKFLOWS_ENV)
                .ok()
                .filter(|s| !s.trim().is_empty())
        });
    if let Some(val) = raw {
        let candidate = PathBuf::from(val.trim());
        let is_script = candidate
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "ts" | "mts" | "js" | "mjs"));
        if is_script {
            let dir = candidate
                .parent()
                .filter(|p| !p.as_os_str().is_empty())
                .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
            let name = candidate
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("workflow.ts")
                .to_owned();
            return WorkflowsLocation {
                directory: dir,
                single: Some(name),
            };
        }
        return WorkflowsLocation {
            directory: candidate,
            single: None,
        };
    }
    WorkflowsLocation {
        directory: home_dir.join("userdata").join("workflows"),
        single: None,
    }
}

/// Validates workflow filenames (`/^[A-Za-z0-9_][\w.-]*\.(ts|mts|js|mjs)$/`).
pub fn is_valid_workflow_filename(name: &str) -> bool {
    let Some(stem_end) = name.rfind('.') else {
        return false;
    };
    let ext = &name[stem_end + 1..];
    if !matches!(ext, "ts" | "mts" | "js" | "mjs") {
        return false;
    }
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '-')
}

/// Keep-alive handle retaining the thread/process backing a loaded workflow file.
#[derive(Clone)]
pub(crate) enum LoadedBackend {
    #[expect(
        dead_code,
        reason = "retains Boa worker thread until file/version unloads"
    )]
    Boa(Arc<BoaHost>),
    #[expect(
        dead_code,
        reason = "retains external JS worker process until file/version unloads"
    )]
    Worker(Arc<JsWorkerHost>),
}

/// Successfully evaluated workflow file contents and compiled definitions.
pub(crate) struct EvaluatedFile {
    pub definitions: Vec<WorkflowDefinition>,
    pub backend: LoadedBackend,
}

/// Evaluates `file_path` via embedded `BoaHost` first, escalating to `JsWorkerHost` if external imports exist.
pub(crate) fn evaluate_workflow_file(
    root_dir: Option<&Path>,
    file_path: &Path,
    file_name: &str,
    source: &str,
) -> Result<EvaluatedFile, WorkflowDefinitionProblem> {
    match BoaHost::load_source(root_dir, file_path, file_name, source) {
        BoaLoadOutcome::Loaded {
            definitions,
            backend,
        } => Ok(EvaluatedFile {
            definitions,
            backend: LoadedBackend::Boa(backend),
        }),
        BoaLoadOutcome::NeedsExternalRuntime(ext) => {
            let (definitions, worker) =
                JsWorkerHost::load_file(file_path, file_name, &ext.specifier)?;
            Ok(EvaluatedFile {
                definitions,
                backend: LoadedBackend::Worker(worker),
            })
        }
        BoaLoadOutcome::Problem(problem) => Err(problem),
    }
}

/// Scans `location` on disk returning `(filename -> source text)` or directory read error.
pub(crate) fn scan_workflow_sources(
    location: &WorkflowsLocation,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    if let Some(single_name) = &location.single {
        if !is_valid_workflow_filename(single_name) {
            return Err(format!("Invalid workflow filename '{single_name}'."));
        }
        let path = location.directory.join(single_name);
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Cannot read workflow file {}: {e}", path.display()))?;
        out.insert(single_name.clone(), text);
        return Ok(out);
    }

    if !location.directory.exists() {
        return Ok(out);
    }
    let entries = std::fs::read_dir(&location.directory).map_err(|e| {
        format!(
            "Cannot read workflows folder {}: {e}",
            location.directory.display()
        )
    })?;
    for entry_res in entries {
        let entry = entry_res.map_err(|e| format!("Failed reading workflows folder entry: {e}"))?;
        let fname = entry.file_name().to_string_lossy().into_owned();
        if !is_valid_workflow_filename(&fname) {
            continue;
        }
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("Cannot read workflow file {}: {e}", path.display()))?;
        out.insert(fname, text);
    }
    Ok(out)
}
