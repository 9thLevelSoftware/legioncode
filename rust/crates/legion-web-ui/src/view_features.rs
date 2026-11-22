//! XSS-safe HTML5 rendering for Wave 5 UI surfaces (`#![forbid(unsafe_code)]`).

use legion_surfaces::mobile_review::secret_request_privacy_note;

use crate::i18n_web_features::{WebFeatureCopy, feature_copy};
use crate::state::WebAppState;
use crate::state_features::{
    FleetEdgeKind, LinkedPaneSurfaceMode, McpSignInUiStatus, UpdateCapsulePhase,
};
use crate::update_settings_features::{
    evaluate_when_condition, filter_keybindings, keybinding_context,
};

fn push_escaped(out: &mut String, text: &str) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}

/// Renders active thread error banner tagged with its monotonic `occurrence_id` (`5872b0a488`).
pub(crate) fn render_thread_error_banner(state: &WebAppState, out: &mut String) {
    let copy = feature_copy(&state.locale);
    let Some(tid) = state.active_thread_id.as_deref() else {
        return;
    };
    let Some(banner) = state.features.thread_errors.get(tid) else {
        return;
    };
    out.push_str(
        "<div class=\"lc-thread-error-banner\" role=\"alert\" data-error-occurrence-id=\"",
    );
    out.push_str(&banner.occurrence_id.to_string());
    out.push_str("\"><span>");
    push_escaped(out, &banner.message);
    out.push_str(
        "</span><button type=\"button\" data-action=\"dismiss-thread-error\" data-occurrence-id=\"",
    );
    out.push_str(&banner.occurrence_id.to_string());
    out.push_str("\">");
    push_escaped(out, copy.dismiss);
    out.push_str("</button></div>");
}

/// Renders multi-value / reveal-toggle secret request cards without echoing masked values in DOM
/// HTML attributes (`0bc9b57b93`).
pub(crate) fn render_secret_cards(state: &WebAppState, out: &mut String) {
    let copy = feature_copy(&state.locale);
    let Some(active_tid) = state.active_thread_id.as_deref() else {
        return;
    };
    for card in state.features.secret_cards.values() {
        if card.thread_id != active_tid {
            continue;
        }
        out.push_str("<section class=\"lc-secret-card\" data-secret-card-id=\"");
        push_escaped(out, &card.turn_item_id);
        out.push_str("\"><h4>");
        push_escaped(out, &card.label);
        out.push_str("</h4><p class=\"lc-muted\">");
        push_escaped(out, secret_request_privacy_note(card.delivery_file));
        out.push_str("</p>");
        for spec in card.card.inputs() {
            let revealed = card.card.is_revealed(&spec.key);
            out.push_str("<div class=\"lc-secret-field\" data-secret-key=\"");
            push_escaped(out, &spec.key);
            out.push_str("\"><label>");
            push_escaped(out, &spec.label);
            out.push_str("</label><input type=\"");
            out.push_str(if revealed { "text" } else { "password" });
            out.push_str("\" placeholder=\"");
            push_escaped(out, &spec.placeholder);
            out.push_str("\" value=\"");
            if revealed {
                push_escaped(out, card.card.value(&spec.key));
            }
            out.push_str("\"/>");
            if spec.secret && spec.revealable {
                out.push_str("<button type=\"button\" data-action=\"toggle-secret-reveal\" data-secret-key=\"");
                push_escaped(out, &spec.key);
                out.push_str("\">");
                push_escaped(out, if revealed { copy.hide } else { copy.show });
                out.push_str("</button>");
            }
            out.push_str("</div>");
        }
        out.push_str("<div class=\"lc-secret-actions\"><button type=\"button\" data-action=\"save-secret-card\">");
        push_escaped(out, copy.save);
        out.push_str("</button><button type=\"button\" data-action=\"decline-secret-card\">");
        push_escaped(out, copy.decline);
        out.push_str("</button></div></section>");
    }
}

/// Renders MCP OAuth sign-in action cards (`669d8941e2`).
pub(crate) fn render_mcp_sign_in_cards(state: &WebAppState, out: &mut String) {
    let copy = feature_copy(&state.locale);
    for notice in state.features.mcp_sign_ins.values() {
        out.push_str("<section class=\"lc-mcp-signin\" data-mcp-notice-id=\"");
        push_escaped(out, &notice.notice_id);
        out.push_str("\" data-mcp-status=\"");
        let status_slug = match notice.status {
            McpSignInUiStatus::Pending => "pending",
            McpSignInUiStatus::Authorizing => "authorizing",
            McpSignInUiStatus::Completing => "completing",
            McpSignInUiStatus::SignedIn => "signed-in",
            McpSignInUiStatus::Error => "error",
        };
        out.push_str(status_slug);
        out.push_str("\"><span>");
        push_escaped(out, &notice.server);
        out.push_str("</span>");
        let action_label = match notice.status {
            McpSignInUiStatus::Authorizing | McpSignInUiStatus::Completing => copy.mcp_signing_in,
            McpSignInUiStatus::SignedIn => copy.mcp_signed_in,
            McpSignInUiStatus::Pending | McpSignInUiStatus::Error => copy.mcp_sign_in,
        };
        out.push_str("<button type=\"button\" data-action=\"mcp-sign-in\">");
        push_escaped(out, action_label);
        out.push_str("</button>");
        if matches!(
            notice.status,
            McpSignInUiStatus::Authorizing | McpSignInUiStatus::Completing
        ) {
            out.push_str("<button type=\"button\" data-action=\"mcp-sign-in-cancel\">");
            push_escaped(out, copy.cancel);
            out.push_str("</button>");
        }
        if let Some(err) = notice.error_message.as_deref() {
            out.push_str("<span class=\"lc-error\">");
            push_escaped(out, err);
            out.push_str("</span>");
        }
        out.push_str("</section>");
    }
}

/// Renders composer dictation read-only lock banner (`b19ca97827`, `bb0d718b31`).
pub(crate) fn render_dictation_badge(state: &WebAppState, out: &mut String) {
    if !state.composer.dictating {
        return;
    }
    let copy = feature_copy(&state.locale);
    out.push_str("<div class=\"lc-dictation-lock\" data-dictating=\"true\" aria-live=\"polite\">");
    push_escaped(out, copy.dictating_locked);
    out.push_str("</div>");
}

/// Renders the Environments Map + batch/transitive P2P links (no Ping cloud relay, `ccabed086a`).
pub(crate) fn render_environments_map_panel(
    state: &WebAppState,
    out: &mut String,
    copy: &WebFeatureCopy,
) {
    let map = &state.features.environments_map;
    if !map.open && map.nodes.is_empty() {
        return;
    }
    out.push_str("<section class=\"lc-environments-map\" data-testid=\"environments-map\"><h3>");
    push_escaped(out, copy.environments_map);
    out.push_str("</h3><ul>");
    for node in &map.nodes {
        out.push_str("<li data-env-id=\"");
        push_escaped(out, &node.id);
        out.push_str("\"><span>");
        push_escaped(out, &node.label);
        out.push_str("</span>");
        for edge in &map.edges {
            if edge.to_id == node.id
                && let FleetEdgeKind::TransitivePeerLink { via_peer_id } = &edge.edge
            {
                out.push_str(" <small>(");
                push_escaped(out, copy.via_peer);
                out.push(' ');
                push_escaped(out, via_peer_id);
                out.push_str(")</small>");
            }
        }
        out.push_str("</li>");
    }
    out.push_str("</ul><button type=\"button\" data-action=\"start-batch-link\">");
    push_escaped(out, copy.link_all);
    out.push_str("</button></section>");
}

/// Renders unified Side / Subagent / Linked Thread read-only pane (`1cb3a8c753`, `7c6e889ddb`).
pub(crate) fn render_linked_thread_pane(state: &WebAppState, out: &mut String) {
    let copy = feature_copy(&state.locale);
    let pane = &state.features.linked_pane;
    if !pane.open {
        if pane.last_target.is_some() {
            out.push_str("<button type=\"button\" data-action=\"reopen-linked-pane\">");
            push_escaped(out, copy.reopen);
            out.push_str("</button>");
        }
        return;
    }
    out.push_str("<aside class=\"lc-linked-thread-pane\" data-readonly=\"true\" data-mode=\"");
    let mode_slug = match pane.mode {
        Some(LinkedPaneSurfaceMode::SideChat) => "side-chat",
        Some(LinkedPaneSurfaceMode::Subagent) => "subagent",
        Some(LinkedPaneSurfaceMode::LinkedThread) | None => "linked-thread",
    };
    out.push_str(mode_slug);
    out.push_str("\"><header><h4>");
    push_escaped(out, pane.title.as_deref().unwrap_or(""));
    out.push_str("</h4><span class=\"lc-badge\">");
    push_escaped(out, copy.read_only);
    out.push_str("</span><button type=\"button\" data-action=\"close-linked-pane\">");
    push_escaped(out, copy.close);
    out.push_str("</button></header>");
    if pane.has_earlier {
        out.push_str("<button type=\"button\" data-action=\"load-earlier-linked\">");
        push_escaped(out, copy.load_earlier);
        out.push_str("</button>");
    }
    out.push_str("<div class=\"lc-linked-messages\">");
    for msg in &pane.messages {
        out.push_str("<article data-role=\"");
        push_escaped(out, &msg.role);
        out.push_str("\">");
        push_escaped(out, &msg.text);
        out.push_str("</article>");
    }
    out.push_str("</div></aside>");
}

/// Renders multi-repo project picker & pinned thread repositories (`60863edc41`).
pub(crate) fn render_multi_repo_picker(
    state: &WebAppState,
    out: &mut String,
    copy: &WebFeatureCopy,
) {
    let picker = &state.features.repo_picker;
    if !picker.open {
        return;
    }
    let Some(pid) = state.active_project_id.as_deref() else {
        return;
    };
    let repos = crate::update_fleet::project_git_repositories(state, pid);
    out.push_str("<section class=\"lc-repo-picker\" data-testid=\"repo-picker\"><h4>");
    push_escaped(out, copy.repositories);
    out.push_str("</h4><ul>");
    for repo in &repos {
        out.push_str("<li data-repo-path=\"");
        push_escaped(out, &repo.repository_path);
        out.push_str("\">");
        push_escaped(out, &repo.name);
        out.push_str("</li>");
    }
    out.push_str("</ul></section>");
}

/// Renders keybinding search + evaluated `when` availability (`342b58a79f`, `3d31ffcd9a`).
pub(crate) fn render_keybinding_search(
    state: &WebAppState,
    out: &mut String,
    copy: &WebFeatureCopy,
) {
    if state.features.keybindings.is_empty() && state.features.keybinding_query.is_empty() {
        return;
    }
    let ctx = keybinding_context(state);
    let matches = filter_keybindings(
        &state.features.keybindings,
        &state.features.keybinding_query,
    );
    out.push_str("<section class=\"lc-keybindings\" data-testid=\"keybindings-search\"><h4>");
    push_escaped(out, copy.search_keybindings);
    out.push_str("</h4><ul>");
    for kb in matches {
        let active = evaluate_when_condition(kb.when_expr.as_deref(), ctx);
        out.push_str("<li data-command-id=\"");
        push_escaped(out, &kb.command_id);
        out.push_str("\" data-when-active=\"");
        out.push_str(if active { "true" } else { "false" });
        out.push_str("\"><span>");
        push_escaped(out, &kb.label);
        out.push_str("</span> <kbd>");
        push_escaped(out, &kb.shortcut);
        out.push_str("</kbd></li>");
    }
    out.push_str("</ul></section>");
}

/// Renders the compact sidebar update capsule countdown pill in sidebar footer.
pub(crate) fn render_sidebar_update_capsule(state: &WebAppState, out: &mut String) {
    let cap = &state.features.update_capsule;
    if cap.dismissed || matches!(cap.phase, UpdateCapsulePhase::Hidden) {
        return;
    }
    let copy = feature_copy(&state.locale);
    out.push_str(
        "<div class=\"lc-update-capsule\" data-testid=\"sidebar-update-capsule\" data-phase=\"",
    );
    let phase_str = match cap.phase {
        UpdateCapsulePhase::Hidden => "hidden",
        UpdateCapsulePhase::Checking => "checking",
        UpdateCapsulePhase::Downloading => "downloading",
        UpdateCapsulePhase::Providers => "providers",
        UpdateCapsulePhase::Servers => "servers",
        UpdateCapsulePhase::Countdown => "countdown",
        UpdateCapsulePhase::Installing => "installing",
        UpdateCapsulePhase::Error => "error",
    };
    out.push_str(phase_str);
    out.push_str("\">");
    match cap.phase {
        UpdateCapsulePhase::Providers => push_escaped(out, copy.update_providers),
        UpdateCapsulePhase::Servers => push_escaped(out, copy.update_servers),
        UpdateCapsulePhase::Countdown => {
            push_escaped(out, copy.update_restarting_in);
            out.push(' ');
            out.push_str(&cap.countdown_seconds.to_string());
            out.push('s');
            out.push_str(" <button type=\"button\" data-action=\"postpone-update\">");
            push_escaped(out, copy.update_later);
            out.push_str("</button>");
        }
        UpdateCapsulePhase::Error => {
            push_escaped(out, cap.error_message.as_deref().unwrap_or(copy.retry));
            out.push_str(" <button type=\"button\" data-action=\"retry-update\">");
            push_escaped(out, copy.retry);
            out.push_str("</button>");
        }
        _ => {
            push_escaped(out, cap.version.as_deref().unwrap_or(phase_str));
        }
    }
    out.push_str("</div>");
}
