//! Wave 5 desktop tests: browser context Reload (`#22650`), `Cmd+T`/`Ctrl+T` forwarding (`#39769`),
//! destroyed webview cleanup (`#40177`), workstation-locked frame streaming (`#39198`),
//! WSL login-shell `T3CODE_HOME` (`#39771`), cross-track update status (`#38506`),
//! `appleSpeech` IPC (`#34027`), and `ACTIVE_BRAND` routing.

use std::path::PathBuf;

use serde_json::{Value, json};
use t3_contracts::brand::ACTIVE_BRAND;
use t3_contracts::ipc::DesktopAppleSpeechErrorCode;
use t3_desktop_shell::apple_speech_ipc::AppleSpeechHelper;
use t3_desktop_shell::browser_webview::{
    ContextMenuEditFlags, DesktopPreviewWebviewHost, FocusedWebviewShortcutAction,
    PreviewWebviewError, build_browser_context_menu, match_focused_browser_webview_shortcut,
};
use t3_desktop_shell::ipc::{DesktopIpcRequest, DesktopIpcRouter};
use t3_desktop_shell::sidecar::{SidecarConfig, SidecarController};
use t3_desktop_shell::wsl_env::{
    build_wsl_login_probe_command, build_wsl_runtime_install_script,
    build_wsl_runtime_prune_script, evaluate_wsl_probe_stdout, parse_wsl_t3_home,
    wsl_home_launch_env,
};
use t3_edge::desktop_updates::{DesktopUpdateChannel, DesktopUpdateMachine};
use t3_speech::apple_speech::{AppleSpeechAssetStatus, map_helper_exit_code};
use t3_surfaces::desktop_shortcuts::{DesktopKeyChord, DesktopPlatform};

#[test]
fn browser_context_menu_reloads_only_targeted_alive_webview_and_forwards_mod_t()
-> Result<(), String> {
    let main_menu = build_browser_context_menu(false, 1, ContextMenuEditFlags::default());
    assert!(
        main_menu
            .iter()
            .all(|item| item.label.as_deref() != Some("Reload"))
    );

    let guest_menu = build_browser_context_menu(
        true,
        42,
        ContextMenuEditFlags {
            can_cut: false,
            can_copy: true,
            can_paste: false,
            can_select_all: true,
        },
    );
    let reload = guest_menu
        .iter()
        .find(|item| item.label.as_deref() == Some("Reload"))
        .ok_or("guest browser context menu has Reload")?;
    assert_eq!(reload.accelerator, None);
    assert_eq!(reload.target_web_contents_id, Some(42));

    let mac_cmd_t = DesktopKeyChord::new("t").with_meta();
    assert_eq!(
        match_focused_browser_webview_shortcut(DesktopPlatform::MacOs, &mac_cmd_t),
        Some(FocusedWebviewShortcutAction::RightPanelNewTab)
    );
    let win_ctrl_t = DesktopKeyChord::new("t").with_ctrl();
    assert_eq!(
        match_focused_browser_webview_shortcut(DesktopPlatform::Windows, &win_ctrl_t),
        Some(FocusedWebviewShortcutAction::RightPanelNewTab)
    );
    let ctrl_shift_t = DesktopKeyChord::new("t").with_ctrl().with_shift();
    assert_eq!(
        match_focused_browser_webview_shortcut(DesktopPlatform::Windows, &ctrl_shift_t),
        None
    );

    let mut host = DesktopPreviewWebviewHost::new();
    host.prepare_webview(42, "guest-1");
    host.prepare_webview(43, "guest-2");
    assert!(host.reload_web_contents(42));
    assert_eq!(host.web_contents_state(42).map(|w| w.reload_count), Some(1));
    assert_eq!(host.web_contents_state(43).map(|w| w.reload_count), Some(0));
    Ok(())
}

#[test]
fn destroyed_webview_detaches_cleanly_and_workstation_lock_unthrottles_capture()
-> Result<(), String> {
    let mut host = DesktopPreviewWebviewHost::new();
    host.set_main_window(1);
    host.create_tab("tab_1");
    host.prepare_webview(49, "attach-1");
    host.register_webview("tab_1", 49)
        .map_err(|e| format!("{e:?}"))?;

    // Workstation lock hides window while capture is active -> unthrottles both guest and main.
    host.set_workstation_locked(true);
    host.set_server_capture_active("thr-1", "tab_1", true)
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(
        host.main_window()
            .map(|w| w.background_throttling_calls.as_slice()),
        Some(&[false][..])
    );
    assert_eq!(
        host.web_contents_state(49)
            .map(|w| w.background_throttling_calls.as_slice()),
        Some(&[false][..])
    );

    host.set_server_capture_active("thr-1", "tab_1", false)
        .map_err(|e| format!("{e:?}"))?;
    assert_eq!(
        host.main_window()
            .map(|w| w.background_throttling_calls.as_slice()),
        Some(&[false, true][..])
    );

    // Destroyed guest detaches cleanly so PiP reports WebviewNotInitialized until reattached.
    host.on_web_contents_destroyed(49);
    assert_eq!(host.tab("tab_1").and_then(|t| t.web_contents_id), None);
    assert_eq!(
        host.open_picture_in_picture("tab_1"),
        Err(PreviewWebviewError::WebviewNotInitialized(
            "tab_1".to_owned()
        ))
    );
    Ok(())
}

#[test]
fn wsl_login_shell_resolves_t3_home_and_builds_install_prune_scripts() {
    let probe_cmd = build_wsl_login_probe_command();
    assert!(probe_cmd.contains("T3CODE_HOME"));
    assert!(probe_cmd.contains("bash -lc"));

    assert_eq!(
        parse_wsl_t3_home("resolvedPath:/usr/bin\nt3Home:relative/path\n"),
        None
    );
    let abs_out = "resolvedPath:/usr/local/bin:/usr/bin\nt3Home:/mnt/wsl-data/t3\n";
    let probed = evaluate_wsl_probe_stdout(abs_out, 0);
    assert!(probed.ok);
    assert_eq!(probed.t3_home.as_deref(), Some("/mnt/wsl-data/t3"));

    let install_custom = build_wsl_runtime_install_script(
        Some("/tmp/runtime.tgz"),
        "rt-1",
        "deadbeef",
        probed.t3_home.as_deref(),
    );
    assert!(install_custom.contains("'/mnt/wsl-data/t3/wsl-runtime'"));

    let prune_default = build_wsl_runtime_prune_script("rt-1", None);
    assert!(prune_default.contains(&format!("$HOME/{}/wsl-runtime", ACTIVE_BRAND.home_dir_name)));
    assert_eq!(
        wsl_home_launch_env(probed.t3_home.as_deref()),
        Some((
            format!("{}HOME", ACTIVE_BRAND.primary_env_prefix),
            "/mnt/wsl-data/t3".to_owned()
        ))
    );
}

#[test]
fn cross_track_update_no_offer_surfaces_track_mismatch_over_ipc() -> Result<(), String> {
    let machine = DesktopUpdateMachine::new(DesktopUpdateChannel::Nightly, "0.1.10");
    let mut router = DesktopIpcRouter::default().with_update_machine(machine);

    let res = router.dispatch(DesktopIpcRequest {
        id: Some(json!("u1")),
        method: "desktop.updateStep".to_owned(),
        params: json!({
            "action": "noOffer",
            "checkedAt": "2026-11-21T00:00:00Z",
            "hasReleaseFloor": false
        }),
    });
    assert!(res.ok);
    let val = res.result.ok_or("missing update result")?;
    assert_eq!(val["status"], "error");
    assert_eq!(val["errorContext"], "check");
    assert_eq!(val["trackMismatch"]["buildChannel"], "latest");

    // Floor prevents mismatch error.
    let res_floor = router.dispatch(DesktopIpcRequest {
        id: Some(json!("u2")),
        method: "desktop.updateStep".to_owned(),
        params: json!({
            "action": "noOffer",
            "checkedAt": "2026-11-21T00:01:00Z",
            "hasReleaseFloor": true
        }),
    });
    let val_floor = res_floor.result.ok_or("missing floor result")?;
    assert_eq!(val_floor["status"], "up-to-date");
    assert!(val_floor.get("trackMismatch").is_none());
    Ok(())
}

struct MockAppleSpeechHelper {
    locales: Vec<String>,
    wav_seen: Option<Vec<u8>>,
    transcript: Result<String, DesktopAppleSpeechErrorCode>,
}

impl AppleSpeechHelper for MockAppleSpeechHelper {
    fn supported_locales(&mut self) -> Vec<String> {
        self.locales.clone()
    }

    fn prepare(
        &mut self,
        _locale: &str,
    ) -> Result<AppleSpeechAssetStatus, DesktopAppleSpeechErrorCode> {
        Ok(AppleSpeechAssetStatus::Installed)
    }

    fn transcribe(
        &mut self,
        _locale: &str,
        wav_bytes: &[u8],
    ) -> Result<String, DesktopAppleSpeechErrorCode> {
        self.wav_seen = Some(wav_bytes.to_vec());
        self.transcript.clone()
    }
}

#[test]
fn apple_speech_ipc_handlers_prepare_encode_wav_and_transcribe() -> Result<(), String> {
    let mut router = DesktopIpcRouter::default();
    let mut helper = MockAppleSpeechHelper {
        locales: vec!["en-US".to_owned(), "fr-FR".to_owned()],
        wav_seen: None,
        transcript: Ok("hello from apple speech".to_owned()),
    };

    let prep_resp = router.dispatch_with_apple_speech(
        DesktopIpcRequest {
            id: Some(json!(1)),
            method: "desktop.appleSpeech.prepare".to_owned(),
            params: json!("en_us"),
        },
        &mut helper,
    );
    assert!(prep_resp.ok);
    assert_eq!(
        prep_resp.result,
        Some(json!({ "ok": true, "value": "en-US" }))
    );

    let avail_resp = router.dispatch_with_apple_speech(
        DesktopIpcRequest {
            id: Some(json!(2)),
            method: "desktop.appleSpeech.available".to_owned(),
            params: Value::Null,
        },
        &mut helper,
    );
    assert_eq!(
        avail_resp.result,
        Some(json!({ "available": true, "supportedLocales": ["en-US", "fr-FR"] }))
    );

    let pcm: Vec<u8> = [0.5_f32, -0.5_f32]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let tx_resp = router.dispatch_with_apple_speech(
        DesktopIpcRequest {
            id: Some(json!(3)),
            method: "desktop.appleSpeech.transcribe".to_owned(),
            params: json!({ "locale": "en-US", "pcm": pcm }),
        },
        &mut helper,
    );
    assert_eq!(
        tx_resp.result,
        Some(json!({ "ok": true, "value": "hello from apple speech" }))
    );
    let wav = helper.wav_seen.as_ref().ok_or("missing wav")?;
    assert_eq!(&wav[0..4], b"RIFF");
    assert_eq!(&wav[8..12], b"WAVE");
    assert_eq!(
        map_helper_exit_code(2),
        DesktopAppleSpeechErrorCode::UnsupportedLocale
    );

    // Sidecar & active brand home resolution alignment.
    let sc = SidecarController::new(SidecarConfig {
        base_dir: PathBuf::from("/tmp/t3-test"),
        ..SidecarConfig::default()
    });
    let env = sc.sanitize_env_entries([("T3CODE_PORT", "9999")], false);
    assert_eq!(
        env.get(&format!("{}HOME", ACTIVE_BRAND.primary_env_prefix)),
        Some("/tmp/t3-test")
    );
    Ok(())
}
