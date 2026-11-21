//! XSS-safe HTML5 renderer for the chat stream and prompt composer (`data-testid="chat-stream"`, `"composer"`).

use std::fmt::Write as _;

use crate::state::WebAppState;
use crate::state_types::ThreadTimeline;
use crate::view::escape_html;

pub(crate) fn render_chat_panel(out: &mut String, state: &WebAppState) {
    out.push_str(r#"<section class="t3-panel t3-chat-panel" data-testid="chat-stream">"#);

    if let Some(tid) = state.active_thread_id.as_deref() {
        let thread = state.threads.get(tid);
        let timeline = state.timelines.get(tid);
        let title = thread.map_or(tid, |t| t.title.as_str());
        let branch = thread.and_then(|t| t.branch.as_deref());
        let model = thread.and_then(|t| t.model.as_deref());
        let synced =
            timeline.is_some_and(|tl| tl.synchronized) || thread.is_some_and(|t| t.synchronized);
        let seq = timeline.map_or(0, |tl| tl.last_sequence);

        let _ = write!(
            out,
            r#"<div class="t3-thread-header"><h2 class="t3-thread-title">{}</h2><div class="t3-thread-meta">"#,
            escape_html(title)
        );
        if let Some(b) = branch {
            let _ = write!(out, r#"<span class="t3-chip">{}</span>"#, escape_html(b));
        }
        if let Some(m) = model {
            let _ = write!(out, r#"<span class="t3-chip">{}</span>"#, escape_html(m));
        }
        let sync_label = if synced { "live" } else { "sync" };
        let _ = write!(
            out,
            r#"<span class="t3-chip" data-testid="thread-sync-badge">{sync_label} · #{seq}</span>"#,
        );
        if let Some(tl) = timeline
            && tl.unknown_event_count > 0
        {
            let _ = write!(
                out,
                r#"<span class="t3-chip" data-testid="thread-unknown-events">+{} unknown</span>"#,
                tl.unknown_event_count
            );
        }
        out.push_str("</div></div>");
        render_timeline_messages(out, timeline, state.locale.web.select_thread_hint);
    } else {
        let _ = write!(
            out,
            r#"<div class="t3-empty-state" data-testid="chat-empty">{}</div>"#,
            escape_html(state.locale.web.select_thread_hint)
        );
    }

    out.push_str("</section>");
    crate::view_features::render_thread_error_banner(state, out);
    crate::view_features::render_secret_cards(state, out);
    crate::view_features::render_mcp_sign_in_cards(state, out);
    render_composer(out, state);
}

fn render_timeline_messages(out: &mut String, timeline: Option<&ThreadTimeline>, empty_hint: &str) {
    out.push_str(r#"<div class="t3-messages">"#);
    let Some(tl) = timeline else {
        let _ = write!(
            out,
            r#"<div class="t3-empty-state">{}</div></div>"#,
            escape_html(empty_hint)
        );
        return;
    };
    if tl.messages.is_empty() {
        let _ = write!(
            out,
            r#"<div class="t3-empty-state">{}</div></div>"#,
            escape_html(empty_hint)
        );
        return;
    }
    for msg in &tl.messages {
        let role_esc = escape_html(&msg.role);
        let kind_esc = escape_html(&msg.kind);
        let id_esc = escape_html(&msg.id);
        let stream_cls = if msg.streaming { " t3-streaming" } else { "" };
        let _ = write!(
            out,
            r#"<article class="t3-msg t3-msg-{role_esc}{stream_cls}" data-testid="stream-message" data-message-id="{id_esc}" data-role="{role_esc}" data-kind="{kind_esc}">"#,
        );
        out.push_str(r#"<header class="t3-msg-head">"#);
        let _ = write!(out, r#"<span class="t3-msg-role">{role_esc}</span>"#);
        if let Some(ref title) = msg.title {
            let _ = write!(
                out,
                r#"<span class="t3-msg-title">{}</span>"#,
                escape_html(title)
            );
        }
        if let Some(ref status) = msg.status {
            let _ = write!(
                out,
                r#"<span class="t3-msg-status">{}</span>"#,
                escape_html(status)
            );
        }
        if msg.streaming {
            out.push_str(r#"<span class="t3-streaming-dot">streaming</span>"#);
        }
        out.push_str("</header>");
        let _ = write!(
            out,
            r#"<div class="t3-msg-body">{}</div></article>"#,
            escape_html(&msg.body)
        );
    }
    out.push_str("</div>");
}

fn render_composer(out: &mut String, state: &WebAppState) {
    out.push_str(r#"<section class="t3-composer" data-testid="composer">"#);
    crate::view_features::render_dictation_badge(state, out);
    if !state.composer.context_pills.is_empty() {
        out.push_str(r#"<div class="t3-pills" data-testid="composer-pills">"#);
        for (idx, pill) in state.composer.context_pills.iter().enumerate() {
            let ctx_id = pill.context.context_id().unwrap_or("");
            let _ = write!(
                out,
                r#"<span class="t3-pill" data-testid="composer-pill" data-context-id="{}">{}<button type="button" class="t3-pill-remove" data-action="remove-pill" data-arg="{idx}" aria-label="Remove">×</button></span>"#,
                escape_html(ctx_id),
                escape_html(&pill.label)
            );
        }
        out.push_str("</div>");
    }
    let readonly_attr = if state.composer.dictating {
        r#" readonly="" aria-readonly="true" data-dictating="true""#
    } else {
        ""
    };
    let _ = write!(
        out,
        r#"<form class="t3-composer-form" data-testid="composer-form" data-action="send-message"><textarea class="t3-composer-input" data-testid="composer-input" data-action="composer-input" rows="3"{readonly_attr} placeholder="{}">{}</textarea><div class="t3-composer-actions"><button type="submit" class="t3-btn t3-btn-primary" data-testid="composer-send">{}</button></div></form>"#,
        escape_html(state.locale.web.composer_placeholder),
        escape_html(&state.composer.draft),
        escape_html(state.locale.web.send_button)
    );
    out.push_str("</section>");
}
