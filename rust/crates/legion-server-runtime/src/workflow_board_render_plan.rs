#![forbid(unsafe_code)]
//! Deterministic `design.md`, `plan.md`, `tasks/<TSK-ID>.md`, and starter wireframe HTML rendering.

use std::fmt::Write as _;

use legion_contracts::workflow_board::{
    WorkflowBoardAcEvidenceMapping, WorkflowBoardEvidenceMethod, WorkflowBoardPlanDocument,
    WorkflowBoardTaskContract,
};

use crate::workflow_board_types::enum_as_str;

/// Renders `.legion/workflows/<card-id>/design.md` from the M2 plan document.
#[must_use]
pub(crate) fn render_design_markdown(
    card_id: &str,
    title: &str,
    plan: &WorkflowBoardPlanDocument,
) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# Design: {}\n", title.trim());
    let _ = writeln!(md, "- Card ID: `{card_id}`");
    let _ = writeln!(md, "- Updated At: `{}`\n", plan.updated_at);

    let _ = writeln!(md, "## Design Lenses");
    if plan.lenses.is_empty() {
        let _ = writeln!(md, "- None recorded.\n");
    } else {
        for lens in &plan.lenses {
            let name = enum_as_str(&lens.lens).unwrap_or_else(|| "unknown".to_owned());
            match lens.applicable {
                Some(true) => {
                    let _ = writeln!(
                        md,
                        "- **{name}** (applicable): {}",
                        lens.summary.as_deref().unwrap_or("").trim()
                    );
                }
                Some(false) => {
                    let _ = writeln!(
                        md,
                        "- **{name}** (not applicable): {}",
                        lens.reason.as_deref().unwrap_or("").trim()
                    );
                }
                None => {
                    let _ = writeln!(md, "- **{name}** (undecided)");
                }
            }
        }
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "## Design Alternatives Comparison");
    if plan.alternatives.is_empty() {
        let _ = writeln!(md, "- None recorded.\n");
    } else {
        for alt in &plan.alternatives {
            let _ = writeln!(
                md,
                "### {} (`{}`)\n- Summary: {}\n- Tradeoffs: {}\n",
                alt.title.trim(),
                alt.id.trim(),
                alt.summary.trim(),
                alt.tradeoffs.trim()
            );
        }
    }
    if let Some(ref sel) = plan.selected_alternative_id {
        let _ = writeln!(md, "- Selected Alternative: `{sel}`");
    }
    if let Some(ref rat) = plan.selection_rationale {
        let _ = writeln!(md, "- Selection Rationale: {}", rat.trim());
    }
    md
}

/// Renders `.legion/workflows/<card-id>/plan.md` from the M2 plan document.
#[must_use]
pub(crate) fn render_plan_markdown(
    card_id: &str,
    title: &str,
    plan: &WorkflowBoardPlanDocument,
) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# Plan: {}\n", title.trim());
    let _ = writeln!(md, "- Card ID: `{card_id}`");
    let _ = writeln!(
        md,
        "- Strategy: `{}`",
        plan.strategy
            .as_ref()
            .and_then(enum_as_str)
            .unwrap_or_else(|| "outcomes".to_owned())
    );
    if let Some(ref rat) = plan.strategy_rationale {
        let _ = writeln!(md, "- Strategy Rationale: {}", rat.trim());
    }
    let _ = writeln!(
        md,
        "- Task Granularity: `{}`",
        plan.granularity
            .as_ref()
            .and_then(enum_as_str)
            .unwrap_or_else(|| "balanced".to_owned())
    );
    let _ = writeln!(md, "- Updated At: `{}`\n", plan.updated_at);

    let _ = writeln!(md, "## Tasks");
    if plan.tasks.is_empty() {
        let _ = writeln!(md, "- None defined.\n");
    } else {
        for t in &plan.tasks {
            let deps = effective_deps(t);
            let dep_text = if deps.is_empty() {
                "none".to_owned()
            } else {
                deps.join(", ")
            };
            let _ = writeln!(
                md,
                "- **{}**: {} (dependsOn: {}; filesAllowed: {})",
                t.task_id,
                t.title.trim(),
                dep_text,
                t.files_allowed.join(", ")
            );
        }
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "## Planned Checks");
    if plan.checks.is_empty() {
        let _ = writeln!(md, "- None defined.\n");
    } else {
        for ch in &plan.checks {
            let _ = writeln!(md, "- `{}`: `{}`", ch.id, ch.argv.join(" "));
        }
        let _ = writeln!(md);
    }

    let _ = writeln!(md, "## Acceptance Criteria Evidence Map");
    if plan.ac_mappings.is_empty() {
        let _ = writeln!(md, "- None mapped.");
    } else {
        for m in &plan.ac_mappings {
            let _ = writeln!(md, "- **{}** ({})", m.criterion_id, describe_ac_mapping(m));
        }
    }
    md
}

/// Renders `.legion/workflows/<card-id>/tasks/<TSK-ID>.md`.
#[must_use]
pub(crate) fn render_task_contract_markdown(
    card_id: &str,
    task: &WorkflowBoardTaskContract,
) -> String {
    let mut md = String::new();
    let _ = writeln!(md, "# {}: {}\n", task.task_id, task.title.trim());
    let _ = writeln!(md, "- Card ID: `{card_id}`");
    let _ = writeln!(
        md,
        "- Testing Method: `{}`",
        enum_as_str(&task.testing_method).unwrap_or_else(|| "test-first".to_owned())
    );
    let _ = writeln!(
        md,
        "- Status: `{}`\n",
        task.status
            .as_ref()
            .and_then(enum_as_str)
            .unwrap_or_else(|| "todo".to_owned())
    );

    let deps = effective_deps(task);
    let _ = writeln!(
        md,
        "## Dependencies\n- {}\n",
        if deps.is_empty() {
            "None".to_owned()
        } else {
            deps.join(", ")
        }
    );
    let _ = writeln!(
        md,
        "## Files Allowed\n{}\n",
        format_code_bullets(&task.files_allowed, "- None")
    );
    let _ = writeln!(
        md,
        "## Files Forbidden\n{}\n",
        format_code_bullets(task.files_forbidden.as_deref().unwrap_or(&[]), "- None")
    );
    if let Some(ref lim) = task.testing_limitations {
        let _ = writeln!(md, "## Testing Limitations\n{}\n", lim.trim());
    }
    md
}

/// Renders a deterministic sandbox-safe starter wireframe HTML document.
#[must_use]
pub(crate) fn render_wireframe_starter_html(card_id: &str, title: &str, brief: &str) -> String {
    let esc_title = escape_html(title.trim());
    let esc_brief = escape_html(brief.trim());
    let esc_id = escape_html(card_id.trim());
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n  <meta charset=\"utf-8\" />\n  \
         <title>{esc_title} — Wireframe</title>\n  \
         <style>body{{font-family:system-ui,sans-serif;margin:24px;color:#111;background:#fafafa}}\
         main{{max-width:720px;padding:20px;border:1px solid #ddd;border-radius:8px;background:#fff}}\
         .meta{{font-size:12px;color:#555}}</style>\n</head>\n<body>\n  <main>\n    \
         <div class=\"meta\">Card {esc_id}</div>\n    <h1>{esc_title}</h1>\n    \
         <p>{esc_brief}</p>\n  </main>\n</body>\n</html>\n"
    )
}

fn effective_deps(task: &WorkflowBoardTaskContract) -> Vec<String> {
    let mut deps = Vec::new();
    for d in task
        .depends_on
        .iter()
        .flatten()
        .chain(task.blocked_by.iter().flatten())
    {
        let trimmed = d.trim();
        if !trimmed.is_empty() && !deps.iter().any(|x| x == trimmed) {
            deps.push(trimmed.to_owned());
        }
    }
    deps
}

fn describe_ac_mapping(m: &WorkflowBoardAcEvidenceMapping) -> String {
    match m.method {
        WorkflowBoardEvidenceMethod::Automated => {
            if let Some(ref cmd) = m.command {
                format!("automated: `{}`", cmd.join(" "))
            } else {
                format!(
                    "automated checks: {}",
                    m.check_ids.as_deref().unwrap_or(&[]).join(", ")
                )
            }
        }
        WorkflowBoardEvidenceMethod::Manual => {
            format!("manual: {}", m.procedure.as_deref().unwrap_or("").trim())
        }
        WorkflowBoardEvidenceMethod::External => format!(
            "external: {}",
            m.external_ref
                .as_deref()
                .or(m.external_source.as_deref())
                .or(m.procedure.as_deref())
                .unwrap_or("")
                .trim()
        ),
    }
}

fn format_code_bullets(items: &[String], fallback: &str) -> String {
    if items.is_empty() {
        return fallback.to_owned();
    }
    items
        .iter()
        .map(|s| format!("- `{}`", s.trim()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn escape_html(raw: &str) -> String {
    raw.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
