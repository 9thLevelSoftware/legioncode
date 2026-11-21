//! Integration tests for Wave 5 UI behaviors: repeatable thread errors, secret cards,
//! MCP OAuth sign-in, composer dictation lock/chips, keybindings, HDL highlighting, and countdowns.

use t3_client_core::diff_markdown::DiffLineKind;
use t3_contracts::mcp_sign_in::{McpSignInStatus, OrchestrationMcpSignInNotice};
use t3_contracts::orchestration::SecretRequestField;
use t3_web_ui::actions_features::{
    AnswerSecretCardPayload, CompleteMcpSignInPayload, DismissThreadErrorPayload,
    KeybindingListPayload, RecordThreadErrorPayload, ResolveMcpSignInPayload,
    SecretFieldKeyPayload, SecretFieldValuePayload, SetUpdateCapsulePayload, StartMcpSignInPayload,
    UpsertMcpSignInPayload, UpsertSecretCardPayload,
};
use t3_web_ui::hdl_highlight::{HdlLanguage, resolve_hdl_language, tokenize_hdl};
use t3_web_ui::state_features::{KeybindingEntry, ThreadErrorSource, UpdateCapsulePhase};
use t3_web_ui::{WebAppAction, WebAppEngine, WebHostEffect};

fn init_thread(engine: &mut WebAppEngine, project_id: &str) -> Result<String, String> {
    engine
        .try_dispatch(WebAppAction::SelectProject {
            project_id: project_id.to_owned(),
        })
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::CreateThread {
            project_id: project_id.to_owned(),
            title: "Wave 5 thread".to_owned(),
            model: None,
        })
        .map_err(|e| e.to_string())?;
    engine
        .state()
        .active_thread_id
        .clone()
        .ok_or_else(|| "missing active thread id".to_owned())
}

#[test]
fn repeatable_thread_error_banners_use_monotonic_occurrence_ids() -> Result<(), String> {
    let mut engine = WebAppEngine::new("en", 1_000);
    let tid = init_thread(&mut engine, "proj-1")?;

    engine
        .try_dispatch(WebAppAction::RecordThreadError(RecordThreadErrorPayload {
            thread_id: tid.clone(),
            message: "Turn timed out".to_owned(),
            source: ThreadErrorSource::Local,
            runtime_key: None,
        }))
        .map_err(|e| e.to_string())?;
    let first_id = engine
        .state()
        .features
        .thread_errors
        .get(&tid)
        .map(|b| b.occurrence_id);
    assert_eq!(first_id, Some(1));
    assert!(
        engine
            .render_html()
            .contains(r#"data-error-occurrence-id="1""#)
    );

    engine
        .try_dispatch(WebAppAction::DismissThreadError(
            DismissThreadErrorPayload {
                thread_id: tid.clone(),
                occurrence_id: Some(1),
            },
        ))
        .map_err(|e| e.to_string())?;
    assert!(!engine.state().features.thread_errors.contains_key(&tid));

    // Raising the exact same error text a second time allocates occurrence_id = 2 and shows it.
    engine
        .try_dispatch(WebAppAction::RecordThreadError(RecordThreadErrorPayload {
            thread_id: tid.clone(),
            message: "Turn timed out".to_owned(),
            source: ThreadErrorSource::Local,
            runtime_key: None,
        }))
        .map_err(|e| e.to_string())?;
    assert_eq!(
        engine
            .state()
            .features
            .thread_errors
            .get(&tid)
            .map(|b| b.occurrence_id),
        Some(2)
    );
    // Dismissing stale occurrence_id = 1 leaves occurrence_id = 2 intact.
    engine
        .try_dispatch(WebAppAction::DismissThreadError(
            DismissThreadErrorPayload {
                thread_id: tid,
                occurrence_id: Some(1),
            },
        ))
        .map_err(|e| e.to_string())?;
    assert!(
        engine
            .render_html()
            .contains(r#"data-error-occurrence-id="2""#)
    );
    Ok(())
}

#[test]
fn secret_card_toggle_reveal_prevents_dom_leak_and_submits_multi_values() -> Result<(), String> {
    let mut engine = WebAppEngine::new("en", 1_000);
    let tid = init_thread(&mut engine, "proj-sec")?;

    engine
        .try_dispatch(WebAppAction::UpsertSecretCard(UpsertSecretCardPayload {
            thread_id: tid,
            turn_item_id: "item-sec".to_owned(),
            label: "API Credentials".to_owned(),
            placeholder: None,
            reason: Some("Deploy service".to_owned()),
            url: None,
            delivery_file: false,
            fields: Some(vec![
                SecretRequestField {
                    name: "email".to_owned(),
                    label: "Email".to_owned(),
                    placeholder: None,
                    secret: Some(false),
                    revealable: true,
                },
                SecretRequestField {
                    name: "token".to_owned(),
                    label: "API Token".to_owned(),
                    placeholder: None,
                    secret: Some(true),
                    revealable: true,
                },
            ]),
        }))
        .map_err(|e| e.to_string())?;

    engine
        .try_dispatch(WebAppAction::SetSecretFieldValue(SecretFieldValuePayload {
            turn_item_id: "item-sec".to_owned(),
            key: "email".to_owned(),
            value: "ops@example.com".to_owned(),
        }))
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::SetSecretFieldValue(SecretFieldValuePayload {
            turn_item_id: "item-sec".to_owned(),
            key: "token".to_owned(),
            value: "tok_super_secret".to_owned(),
        }))
        .map_err(|e| e.to_string())?;

    // Masked secret must NOT appear inside DOM attributes while masked.
    let masked_html = engine.render_html();
    assert!(masked_html.contains("type=\"password\""));
    assert!(!masked_html.contains("tok_super_secret"));

    // Toggle reveal shows plaintext, toggle again hides it (reverse state).
    engine
        .try_dispatch(WebAppAction::ToggleSecretFieldReveal(
            SecretFieldKeyPayload {
                turn_item_id: "item-sec".to_owned(),
                key: "token".to_owned(),
            },
        ))
        .map_err(|e| e.to_string())?;
    assert!(engine.render_html().contains("tok_super_secret"));
    engine
        .try_dispatch(WebAppAction::ToggleSecretFieldReveal(
            SecretFieldKeyPayload {
                turn_item_id: "item-sec".to_owned(),
                key: "token".to_owned(),
            },
        ))
        .map_err(|e| e.to_string())?;
    assert!(!engine.render_html().contains("tok_super_secret"));

    let effects = engine
        .try_dispatch(WebAppAction::AnswerSecretCard(AnswerSecretCardPayload {
            turn_item_id: "item-sec".to_owned(),
            decline: false,
        }))
        .map_err(|e| e.to_string())?;
    assert!(matches!(
        effects.first(),
        Some(WebHostEffect::WsSendFrameParts { .. })
    ));
    Ok(())
}

#[test]
fn mcp_sign_in_and_dictation_lock_with_chip_removal() -> Result<(), String> {
    let mut engine = WebAppEngine::new("fr", 2_000);
    engine
        .try_dispatch(WebAppAction::UpsertMcpSignIn(UpsertMcpSignInPayload {
            thread_id: "th-mcp".to_owned(),
            notice_id: "notice-1".to_owned(),
            notice: OrchestrationMcpSignInNotice {
                server: "github".to_owned(),
            },
        }))
        .map_err(|e| e.to_string())?;
    let start_fx = engine
        .try_dispatch(WebAppAction::StartMcpSignIn(StartMcpSignInPayload {
            notice_id: "notice-1".to_owned(),
            provider_thread_id: Some("pt-1".to_owned()),
        }))
        .map_err(|e| e.to_string())?;
    assert_eq!(start_fx.len(), 1);
    let comp_fx = engine
        .try_dispatch(WebAppAction::CompleteMcpSignIn(CompleteMcpSignInPayload {
            notice_id: "notice-1".to_owned(),
            callback_url: "t3://oauth/cb?code=secret&state=st".to_owned(),
            provider_thread_id: None,
        }))
        .map_err(|e| e.to_string())?;
    assert_eq!(comp_fx.len(), 1);
    engine
        .try_dispatch(WebAppAction::ResolveMcpSignIn(ResolveMcpSignInPayload {
            notice_id: "notice-1".to_owned(),
            status: None,
            contract_status: Some(McpSignInStatus::SignedIn {
                server: "github".to_owned(),
                signed_in_at: "2026-11-21T12:00:00Z".to_owned(),
            }),
            error_message: Some("https://cb.test?code=leak_token".to_owned()),
        }))
        .map_err(|e| e.to_string())?;
    let html = engine.render_html();
    assert!(html.contains(r#"data-mcp-status="signed-in""#));
    assert!(!html.contains("leak_token"));

    // Dictation freezes draft edits and paste while preserving chip removal.
    engine
        .try_dispatch(WebAppAction::UpdateComposerDraft {
            text: "initial".to_owned(),
        })
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::AddContextPill {
            context_id: Some("ctx-1".to_owned()),
            screenshot_context_id: None,
            label: Some("app.ts".to_owned()),
            path: None,
        })
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::SetDictationActive { active: true })
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::UpdateComposerDraft {
            text: "should-be-blocked".to_owned(),
        })
        .map_err(|e| e.to_string())?;
    assert_eq!(engine.state().composer.draft, "initial");
    assert!(engine.render_html().contains(r#"data-dictating="true""#));
    engine
        .try_dispatch(WebAppAction::RemoveContextPill { index: 0 })
        .map_err(|e| e.to_string())?;
    assert_eq!(engine.state().composer.context_pills.len(), 0);
    engine
        .try_dispatch(WebAppAction::SetDictationActive { active: false })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[test]
fn keybinding_when_search_hdl_highlight_and_update_countdown_capsule() -> Result<(), String> {
    let mut engine = WebAppEngine::new("de", 3_000);
    engine
        .try_dispatch(WebAppAction::SetKeybindings(KeybindingListPayload {
            entries: vec![
                KeybindingEntry {
                    command_id: "composer.focusPrevious".to_owned(),
                    label: "Focus previous".to_owned(),
                    shortcut: "ArrowUp".to_owned(),
                    category: "Composer".to_owned(),
                    when_expr: Some("!modalOpen && composerEmpty".to_owned()),
                },
                KeybindingEntry {
                    command_id: "app.openPalette".to_owned(),
                    label: "Command palette".to_owned(),
                    shortcut: "Mod+K".to_owned(),
                    category: "Navigation".to_owned(),
                    when_expr: None,
                },
            ],
        }))
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::SearchKeybindings {
            query: "composer".to_owned(),
        })
        .map_err(|e| e.to_string())?;
    assert!(engine.render_html().contains(r#"data-when-active="true""#));

    engine
        .try_dispatch(WebAppAction::UpdateComposerDraft {
            text: "hello".to_owned(),
        })
        .map_err(|e| e.to_string())?;
    assert!(engine.render_html().contains(r#"data-when-active="false""#));

    // HDL language resolving & tokenizing + DecorateDiffTokens auto-tokenizing.
    assert_eq!(
        resolve_hdl_language("rtl/uart_tx.sv"),
        Some(HdlLanguage::SystemVerilog)
    );
    let sv_tokens = tokenize_hdl(
        HdlLanguage::SystemVerilog,
        "always_ff @(posedge clk) q <= 8'hFF;",
    );
    assert!(
        sv_tokens
            .iter()
            .any(|t| t.content == "always_ff" && t.font_style.is_bold())
    );
    let vhdl_tokens = tokenize_hdl(HdlLanguage::Vhdl, "signal data : std_logic := '1'; -- ok");
    assert!(
        vhdl_tokens
            .iter()
            .any(|t| t.content == "std_logic" && t.font_style.is_bold())
    );
    engine
        .try_dispatch(WebAppAction::LoadUnifiedDiff {
            patch: "always_comb begin end".to_owned(),
            switch_tab: false,
        })
        .map_err(|e| e.to_string())?;
    engine
        .try_dispatch(WebAppAction::DecorateDiffTokens {
            file_name: "alu.sv".to_owned(),
            tokens: Vec::new(),
            word_highlights: Vec::new(),
            highlight_background: None,
        })
        .map_err(|e| e.to_string())?;
    assert_eq!(
        engine.state().diff.files[0].lines[0].kind,
        DiffLineKind::Add
    );

    // Compact sidebar update capsule countdown & postpone reverse state.
    engine
        .try_dispatch(WebAppAction::SetUpdateCapsule(SetUpdateCapsulePayload {
            phase: UpdateCapsulePhase::Countdown,
            version: Some("0.2.0".to_owned()),
            progress_percent: 100,
            restart_at_ms: Some(15_000),
            now_ms: 10_000,
            error_message: None,
        }))
        .map_err(|e| e.to_string())?;
    assert!(engine.render_html().contains("5s"));
    engine
        .try_dispatch(WebAppAction::TickUpdateCountdown { now_ms: 15_000 })
        .map_err(|e| e.to_string())?;
    assert_eq!(
        engine.state().features.update_capsule.phase,
        UpdateCapsulePhase::Installing
    );
    engine
        .try_dispatch(WebAppAction::PostponeUpdateCapsule)
        .map_err(|e| e.to_string())?;
    assert!(
        !engine
            .render_html()
            .contains(r#"data-testid="sidebar-update-capsule""#)
    );
    Ok(())
}
