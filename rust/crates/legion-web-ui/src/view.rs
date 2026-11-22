//! XSS-safe HTML5 workspace chrome renderer for `legion-web-ui`.

use std::fmt::Write as _;

use legion_edge::i18n::SUPPORTED_LOCALES;

use crate::actions::RightPaneTab;
use crate::state::{ConnectionStatus, WebAppState};
use crate::view_chat::render_chat_panel;
use crate::view_palette::render_command_palette_modal;
use crate::view_panels::{render_diff_panel, render_terminal_panel};

/// Escapes untrusted text for safe insertion inside HTML5 text nodes and quoted attributes.
pub fn escape_html(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Renders the responsive HTML5 `LegionCode` workspace shell from `state`.
pub fn render_workspace_html(state: &WebAppState) -> String {
    let mut out = String::with_capacity(4096);
    let lang = escape_html(&state.locale.bcp47_tag);
    let conn = connection_status_label(state.client.connection_status);
    let tab = match state.right_tab {
        RightPaneTab::Chat => "chat",
        RightPaneTab::Diff => "diff",
        RightPaneTab::Terminal => "terminal",
    };
    let sidebar_cls = if state.sidebar_open {
        "lc-shell"
    } else {
        "lc-shell is-sidebar-collapsed"
    };

    let _ = write!(
        out,
        r#"<div class="{sidebar_cls}" data-testid="workspace-shell" lang="{lang}" data-locale="{}" data-connection="{conn}" data-tab="{tab}">"#,
        escape_html(&state.locale.catalog_locale)
    );
    render_sidebar(&mut out, state);
    out.push_str(r#"<main class="lc-main">"#);
    render_topbar(&mut out, state, conn);
    match state.right_tab {
        RightPaneTab::Chat => render_chat_panel(&mut out, state),
        RightPaneTab::Diff => render_diff_panel(&mut out, state),
        RightPaneTab::Terminal => render_terminal_panel(&mut out, state),
    }
    crate::view_features::render_linked_thread_pane(state, &mut out);
    let feature_copy = crate::i18n_web_features::feature_copy(&state.locale);
    crate::view_features::render_environments_map_panel(state, &mut out, &feature_copy);
    crate::view_features::render_multi_repo_picker(state, &mut out, &feature_copy);
    crate::view_features::render_keybinding_search(state, &mut out, &feature_copy);
    out.push_str("</main>");
    render_command_palette_modal(&mut out, state);
    out.push_str("</div>");
    out
}

fn render_sidebar(out: &mut String, state: &WebAppState) {
    out.push_str(r#"<aside class="lc-sidebar" data-testid="sidebar">"#);
    let _ = write!(
        out,
        r#"<div class="lc-brand-row"><h1>{}</h1><button type="button" class="lc-btn" data-testid="sidebar-toggle" data-action="toggle-sidebar">☰</button></div>"#,
        escape_html(state.locale.web.app_title)
    );
    render_locale_selector(out, state);
    render_pairing_section(out, state);
    render_projects_section(out, state);
    render_threads_section(out, state);

    let activity = state.localized_activity();
    let _ = write!(
        out,
        r#"<div class="lc-activity" data-testid="activity-summary"><span class="lc-chip">{}</span><span>{}</span></div>"#,
        escape_html(&activity.chip),
        escape_html(&activity.summary)
    );
    crate::view_features::render_sidebar_update_capsule(state, out);
    out.push_str("</aside>");
}

fn render_locale_selector(out: &mut String, state: &WebAppState) {
    let _ = write!(
        out,
        r#"<label class="lc-locale-row"><span>{}</span><select class="lc-select" data-testid="locale-selector" data-action="set-locale">"#,
        escape_html(state.locale.web.language_label)
    );
    for desc in SUPPORTED_LOCALES {
        let selected = if desc.code == state.locale.catalog_locale {
            " selected"
        } else {
            ""
        };
        let _ = write!(
            out,
            r#"<option value="{}"{selected}>{}</option>"#,
            escape_html(desc.code),
            escape_html(desc.native_name)
        );
    }
    out.push_str("</select></label>");
}

fn render_pairing_section(out: &mut String, state: &WebAppState) {
    out.push_str(r#"<section class="lc-card" data-testid="pairing-section">"#);
    let status_text = if state.auth.paired {
        state.locale.web.paired_status
    } else {
        state.locale.web.unpaired_status
    };
    let _ = write!(
        out,
        r#"<div class="lc-card-head"><strong>{}</strong><span class="lc-chip" data-testid="pairing-status">{}</span></div>"#,
        escape_html(state.locale.web.pairing_title),
        escape_html(status_text)
    );
    if state.auth.paired {
        let _ = write!(
            out,
            r#"<button type="button" class="lc-btn" data-testid="sign-out-button" data-action="sign-out">{}</button>"#,
            escape_html(state.locale.web.sign_out)
        );
    } else {
        let _ = write!(
            out,
            r#"<form class="lc-pairing-form" data-testid="pairing-form" data-action="submit-pairing"><input type="password" class="lc-input" data-testid="pairing-token-input" data-action="pairing-input" value="{}" placeholder="{}" /><button type="submit" class="lc-btn lc-btn-primary" data-testid="pairing-submit">{}</button></form>"#,
            escape_html(&state.auth.pairing_input),
            escape_html(state.locale.web.pairing_placeholder),
            escape_html(state.locale.web.pair_button)
        );
    }
    if let Some(ref err) = state.auth.last_error {
        let _ = write!(
            out,
            r#"<div class="lc-error" data-testid="auth-error">{}</div>"#,
            escape_html(err)
        );
    }
    out.push_str("</section>");
}

fn render_projects_section(out: &mut String, state: &WebAppState) {
    out.push_str(r#"<section class="lc-card" data-testid="project-switcher">"#);
    let _ = write!(
        out,
        r#"<div class="lc-card-head"><strong>{}</strong></div>"#,
        escape_html(state.locale.web.projects_title)
    );
    if state.projects.is_empty() {
        let _ = write!(
            out,
            r#"<div class="lc-muted">{}</div>"#,
            escape_html(state.locale.web.no_projects)
        );
    } else {
        out.push_str(r#"<ul class="lc-list">"#);
        for project in &state.projects {
            let active = if state.active_project_id.as_deref() == Some(project.id.as_str()) {
                " is-active"
            } else {
                ""
            };
            let _ = write!(
                out,
                r#"<li><button type="button" class="lc-list-btn{active}" data-testid="project-item" data-action="select-project" data-arg="{}" title="{}">{}</button></li>"#,
                escape_html(&project.id),
                escape_html(&project.workspace_root),
                escape_html(&project.title)
            );
        }
        out.push_str("</ul>");
    }
    let _ = write!(
        out,
        r#"<form class="lc-project-form" data-testid="project-create-form" data-action="create-project"><input type="text" class="lc-input" data-testid="project-workspace-input" data-action="project-ws-input" value="{}" placeholder="{}" /><button type="submit" class="lc-btn" data-testid="project-create-submit">{}</button></form>"#,
        escape_html(&state.new_project_workspace_root),
        escape_html(state.locale.web.workspace_placeholder),
        escape_html(state.locale.web.add_project)
    );
    out.push_str("</section>");
}

fn render_threads_section(out: &mut String, state: &WebAppState) {
    out.push_str(r#"<section class="lc-card" data-testid="thread-list">"#);
    let _ = write!(
        out,
        r#"<div class="lc-card-head"><strong>{}</strong><button type="button" class="lc-btn" data-testid="new-thread-button" data-action="new-thread">+</button></div>"#,
        escape_html(&state.locale.nav.threads)
    );
    if state.threads.is_empty() {
        let _ = write!(
            out,
            r#"<div class="lc-muted">{}</div>"#,
            escape_html(state.locale.web.no_threads)
        );
    } else {
        out.push_str(r#"<ul class="lc-list">"#);
        for thread in state.threads.values() {
            let active = if state.active_thread_id.as_deref() == Some(thread.id.as_str()) {
                " is-active"
            } else {
                ""
            };
            let _ = write!(
                out,
                r#"<li><button type="button" class="lc-list-btn{active}" data-testid="thread-item" data-action="select-thread" data-arg="{}">{}</button></li>"#,
                escape_html(&thread.id),
                escape_html(&thread.title)
            );
        }
        out.push_str("</ul>");
    }
    out.push_str("</section>");
}

fn render_topbar(out: &mut String, state: &WebAppState, conn: &str) {
    out.push_str(
        r#"<header class="lc-topbar"><div class="lc-tabs" data-testid="right-pane-tabs">"#,
    );
    let tabs = [
        (
            RightPaneTab::Chat,
            "chat",
            "tab-chat",
            state.locale.web.tab_chat,
        ),
        (
            RightPaneTab::Diff,
            "diff",
            "tab-diff",
            state.locale.web.tab_diff,
        ),
        (
            RightPaneTab::Terminal,
            "terminal",
            "tab-terminal",
            state.locale.web.tab_terminal,
        ),
    ];
    for (variant, arg, testid, label) in tabs {
        let active = if state.right_tab == variant {
            " is-active"
        } else {
            ""
        };
        let _ = write!(
            out,
            r#"<button type="button" class="lc-tab{active}" data-testid="{testid}" data-action="set-tab" data-arg="{arg}">{}</button>"#,
            escape_html(label)
        );
    }
    out.push_str("</div>");
    let _ = write!(
        out,
        r#"<div class="lc-topbar-right"><span class="lc-chip" data-testid="connection-badge">{conn}</span><button type="button" class="lc-btn" data-testid="command-palette-trigger" data-action="open-palette">{}</button></div>"#,
        escape_html(state.locale.web.palette_trigger)
    );
    out.push_str("</header>");
}

const fn connection_status_label(status: ConnectionStatus) -> &'static str {
    match status {
        ConnectionStatus::Disconnected => "disconnected",
        ConnectionStatus::Connecting => "connecting",
        ConnectionStatus::Synchronizing => "synchronizing",
        ConnectionStatus::Live => "live",
        ConnectionStatus::AwaitingApproval => "awaiting-approval",
    }
}
