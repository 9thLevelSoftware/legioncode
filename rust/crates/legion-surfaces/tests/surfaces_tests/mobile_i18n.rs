//! BUG-24 integration tests: localized mobile workspace navigation and Live Activity
//! presenters across all `legion_edge::i18n` locales with zero English fallback leakage.

use std::collections::BTreeMap;

use legion_edge::i18n::{SUPPORTED_LOCALE_CODES, SUPPORTED_LOCALES, lookup_locale};
use legion_surfaces::mobile_i18n::{
    AgentActivityInput, AgentActivityPhase, AgentActivityRowInput, AgentLiveActivityPresenter,
    MobileNavigationPresenter, NavigationTabKind,
};

fn nav_fields(p: &MobileNavigationPresenter) -> Vec<(&'static str, String)> {
    let s = p.strings();
    vec![
        ("chats", s.chats),
        ("threads", s.threads),
        ("pulls", s.pulls),
        ("pull_requests", s.pull_requests),
        ("prs_short", s.prs_short),
        ("issues", s.issues),
        ("usage", s.usage),
        ("settings", s.settings),
        ("new_thread", s.new_thread),
        ("new_chat", s.new_chat),
        ("close_rail", s.close_rail),
        ("expand_rail", s.expand_rail),
        ("collapse_rail", s.collapse_rail),
    ]
}

#[test]
fn supported_locales_have_zero_english_fallback_leakage_in_navigation() {
    assert_eq!(SUPPORTED_LOCALES.len(), 10);
    let en = MobileNavigationPresenter::for_locale("en");
    let en_fields = nav_fields(&en);

    let mut test_tags: Vec<&str> = SUPPORTED_LOCALE_CODES.to_vec();
    test_tags.push("zh-CN");

    for tag in test_tags {
        if tag == "en" {
            continue;
        }
        let presenter = MobileNavigationPresenter::for_locale(tag);
        let expected_locale = if tag == "zh-CN" { "zh" } else { tag };
        assert_eq!(presenter.locale(), expected_locale);
        for ((key, en_val), (_, loc_val)) in en_fields.iter().zip(nav_fields(&presenter)) {
            assert!(!loc_val.is_empty(), "empty nav {key} in {tag}");
            assert_ne!(
                loc_val, *en_val,
                "English leakage for nav field '{key}' in locale '{tag}': '{loc_val}'"
            );
        }
        // Verify compact, expanded, and iOS labels remain localized across every tab.
        for tab in [
            NavigationTabKind::Threads,
            NavigationTabKind::PullRequests,
            NavigationTabKind::Issues,
            NavigationTabKind::Usage,
            NavigationTabKind::Settings,
        ] {
            assert_ne!(presenter.tab_title(tab), en.tab_title(tab));
            assert_ne!(
                presenter.expanded_tab_title(tab),
                en.expanded_tab_title(tab)
            );
            assert_ne!(presenter.ios_tab_label(tab), en.ios_tab_label(tab));
        }
    }
}

#[test]
fn supported_locales_have_zero_english_fallback_leakage_in_live_activity() {
    let en = AgentLiveActivityPresenter::for_locale("en");
    let all_phases = [
        AgentActivityPhase::Starting,
        AgentActivityPhase::Running,
        AgentActivityPhase::WaitingForApproval,
        AgentActivityPhase::WaitingForInput,
        AgentActivityPhase::Stale,
        AgentActivityPhase::Completed,
        AgentActivityPhase::Failed,
    ];

    let mut test_tags: Vec<&str> = SUPPORTED_LOCALE_CODES.to_vec();
    test_tags.push("zh-CN");

    for tag in test_tags {
        if tag == "en" {
            continue;
        }
        let p = AgentLiveActivityPresenter::for_locale(tag);
        assert_ne!(p.dismiss_label(), en.dismiss_label(), "dismiss in {tag}");
        assert_ne!(
            p.fallback_title(),
            en.fallback_title(),
            "fallback_title in {tag}"
        );
        for phase in all_phases {
            assert_ne!(
                p.status_label(phase),
                en.status_label(phase),
                "status {phase:?} in {tag}"
            );
            assert_ne!(
                p.heading_label(phase),
                en.heading_label(phase),
                "heading {phase:?} in {tag}"
            );
            assert_ne!(
                p.action_label(phase),
                en.action_label(phase),
                "action {phase:?} in {tag}"
            );
        }
    }
}

#[test]
fn system_locale_fallback_and_traditional_chinese_exclusion_match_edge_i18n() {
    assert_eq!(lookup_locale("zh-TW"), None);
    let nav_tw = MobileNavigationPresenter::for_locale("zh-TW");
    assert_eq!(nav_tw.locale(), "en");
    let act_tw = AgentLiveActivityPresenter::for_locale("zh-TW");
    assert_eq!(act_tw.locale(), "en");

    let nav_zh_cn = MobileNavigationPresenter::from_settings("zh-CN", &["en-US"]);
    assert_eq!(nav_zh_cn.locale(), "zh");
    assert_eq!(nav_zh_cn.formatting_locale(), "zh-CN");
    assert_eq!(nav_zh_cn.tab_title(NavigationTabKind::Threads), "聊天");
    assert_eq!(nav_zh_cn.tab_title(NavigationTabKind::PullRequests), "拉取");
    assert_eq!(
        nav_zh_cn.expanded_tab_title(NavigationTabKind::PullRequests),
        "拉取请求"
    );
    assert_eq!(
        nav_zh_cn.ios_tab_label(NavigationTabKind::PullRequests),
        "合并请求"
    );
    assert_eq!(nav_zh_cn.strings().threads, "会话");

    let nav_system_de = MobileNavigationPresenter::from_settings("system", &["zh-TW", "de-DE"]);
    assert_eq!(nav_system_de.locale(), "de");
    assert_eq!(nav_system_de.formatting_locale(), "de-DE");
    assert_eq!(
        nav_system_de.tab_title(NavigationTabKind::Settings),
        "Einstellungen"
    );
    assert_eq!(
        nav_system_de.rail_toggle_label(false),
        "Navigationsleiste ausklappen"
    );
    assert_eq!(
        nav_system_de.rail_toggle_label(true),
        "Navigationsleiste einklappen"
    );
}

#[test]
fn live_activity_presenter_renders_multi_row_attention_inactive_and_push_data() {
    let mut data = BTreeMap::new();
    data.insert(
        "activity_line_0".to_owned(),
        "waiting_for_approval\tDeploy prod\tLegionCode".to_owned(),
    );
    data.insert(
        "activity_line_1".to_owned(),
        "running\tRefactor tests\tLegionCode".to_owned(),
    );
    data.insert(
        "activity_phase".to_owned(),
        "waiting_for_approval".to_owned(),
    );

    let input = AgentActivityInput::from_push_data(&data, true);
    let fr = AgentLiveActivityPresenter::for_locale("fr").present(&input);
    assert_eq!(fr.locale, "fr");
    assert_eq!(fr.summary, "1 a besoin de vous");
    assert_eq!(fr.subtext.as_deref(), Some("LegionCode · 2 actifs"));
    assert_eq!(fr.chip, "Approuver");
    assert_eq!(fr.action.as_deref(), Some("Approuver"));
    assert_eq!(fr.dismiss_label, "Refuser");
    assert_eq!(fr.phase_heading.as_deref(), Some("Approbation requise"));
    assert_eq!(fr.rows[0].status_label, "Approbation");
    assert_eq!(fr.rows[1].status_label, "En cours");

    let ended = AgentActivityInput {
        rows: vec![
            AgentActivityRowInput {
                phase: AgentActivityPhase::Completed,
                title: "Build".to_owned(),
                project: "Core".to_owned(),
            },
            AgentActivityRowInput {
                phase: AgentActivityPhase::Failed,
                title: "Lint".to_owned(),
                project: "Mobile".to_owned(),
            },
        ],
        phase: Some(AgentActivityPhase::Failed),
        active: false,
        ..AgentActivityInput::default()
    };
    let ja_ended = AgentLiveActivityPresenter::for_locale("ja").present(&ended);
    assert_eq!(ja_ended.action, None);
    assert_eq!(ja_ended.chip, "失敗");
    assert_eq!(ja_ended.summary, "完了, 1 件が失敗");
    assert_eq!(ja_ended.subtext.as_deref(), Some("2 スレッド"));
}
