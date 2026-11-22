//! Localized mobile navigation chrome catalogs across all 10 `legion_edge::i18n` locales.

/// Localized strings for mobile navigation tabs, rails, and compose chrome (`BUG-24`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NavCatalog {
    pub(crate) threads: &'static str,
    pub(crate) chats: &'static str,
    pub(crate) pulls: &'static str,
    pub(crate) pull_requests: &'static str,
    pub(crate) prs_short: &'static str,
    pub(crate) issues: &'static str,
    pub(crate) usage: &'static str,
    pub(crate) settings: &'static str,
    pub(crate) new_thread: &'static str,
    pub(crate) new_chat: &'static str,
    pub(crate) close_rail: &'static str,
    pub(crate) expand_rail: &'static str,
    pub(crate) collapse_rail: &'static str,
}

pub(crate) fn nav_catalog(locale: &str) -> &'static NavCatalog {
    match locale {
        "de" => &NAV_DE,
        "es" => &NAV_ES,
        "fr" => &NAV_FR,
        "ja" => &NAV_JA,
        "ko" => &NAV_KO,
        "pt-BR" => &NAV_PT_BR,
        "ru" => &NAV_RU,
        "sv" => &NAV_SV,
        "zh" => &NAV_ZH,
        _ => &NAV_EN,
    }
}

const NAV_EN: NavCatalog = NavCatalog {
    threads: "Threads",
    chats: "Chats",
    pulls: "Pulls",
    pull_requests: "Pull Requests",
    prs_short: "PRs",
    issues: "Issues",
    usage: "Usage",
    settings: "Settings",
    new_thread: "New thread",
    new_chat: "New chat",
    close_rail: "Close navigation rail",
    expand_rail: "Expand navigation rail",
    collapse_rail: "Collapse navigation rail",
};

const NAV_DE: NavCatalog = NavCatalog {
    threads: "Themen",
    chats: "Unterhaltungen",
    pulls: "Pull-Anfragen",
    pull_requests: "Pull-Anfragen (gesamt)",
    prs_short: "Anfragen",
    issues: "Aufgaben",
    usage: "Nutzung",
    settings: "Einstellungen",
    new_thread: "Neuer Thread",
    new_chat: "Neuer Chat",
    close_rail: "Navigationsleiste schließen",
    expand_rail: "Navigationsleiste ausklappen",
    collapse_rail: "Navigationsleiste einklappen",
};

const NAV_ES: NavCatalog = NavCatalog {
    threads: "Hilos",
    chats: "Conversaciones",
    pulls: "Solicitudes",
    pull_requests: "Solicitudes de cambio",
    prs_short: "Cambios",
    issues: "Incidencias",
    usage: "Uso",
    settings: "Ajustes",
    new_thread: "Nuevo hilo",
    new_chat: "Nuevo chat",
    close_rail: "Cerrar barra de navegación",
    expand_rail: "Expandir barra de navegación",
    collapse_rail: "Contraer barra de navegación",
};

const NAV_FR: NavCatalog = NavCatalog {
    threads: "Fils",
    chats: "Discussions",
    pulls: "Demandes",
    pull_requests: "Demandes de tirage",
    prs_short: "Demandes PR",
    issues: "Tickets",
    usage: "Utilisation",
    settings: "Paramètres",
    new_thread: "Nouveau fil",
    new_chat: "Nouvelle discussion",
    close_rail: "Fermer le rail de navigation",
    expand_rail: "Développer le rail de navigation",
    collapse_rail: "Réduire le rail de navigation",
};

const NAV_JA: NavCatalog = NavCatalog {
    threads: "スレッド",
    chats: "チャット",
    pulls: "プル要求",
    pull_requests: "プルリクエスト",
    prs_short: "PR一覧",
    issues: "課題",
    usage: "使用量",
    settings: "設定",
    new_thread: "新しいスレッド",
    new_chat: "新しいチャット",
    close_rail: "ナビゲーションレールを閉じる",
    expand_rail: "ナビゲーションレールを展開",
    collapse_rail: "ナビゲーションレールを折りたたむ",
};

const NAV_KO: NavCatalog = NavCatalog {
    threads: "스레드",
    chats: "채팅",
    pulls: "풀 요청",
    pull_requests: "풀 리퀘스트",
    prs_short: "PR 목록",
    issues: "이슈",
    usage: "사용량",
    settings: "설정",
    new_thread: "새 스레드",
    new_chat: "새 채팅",
    close_rail: "탐색 레일 닫기",
    expand_rail: "탐색 레일 펼치기",
    collapse_rail: "탐색 레일 접기",
};

const NAV_PT_BR: NavCatalog = NavCatalog {
    threads: "Tópicos",
    chats: "Conversas",
    pulls: "Revisões",
    pull_requests: "Solicitações de pull",
    prs_short: "Revisões PR",
    issues: "Problemas",
    usage: "Uso",
    settings: "Configurações",
    new_thread: "Nova thread",
    new_chat: "Novo chat",
    close_rail: "Fechar barra de navegação",
    expand_rail: "Expandir barra de navegação",
    collapse_rail: "Recolher barra de navegação",
};

const NAV_RU: NavCatalog = NavCatalog {
    threads: "Треды",
    chats: "Чаты",
    pulls: "Запросы",
    pull_requests: "Пул-реквесты",
    prs_short: "Запросы PR",
    issues: "Задачи",
    usage: "Использование",
    settings: "Настройки",
    new_thread: "Новый тред",
    new_chat: "Новый чат",
    close_rail: "Закрыть панель навигации",
    expand_rail: "Развернуть панель навигации",
    collapse_rail: "Свернуть панель навигации",
};

const NAV_SV: NavCatalog = NavCatalog {
    threads: "Trådar",
    chats: "Chattar",
    pulls: "Förfrågningar",
    pull_requests: "Pull-förfrågningar",
    prs_short: "PR-lista",
    issues: "Ärenden",
    usage: "Användning",
    settings: "Inställningar",
    new_thread: "Ny tråd",
    new_chat: "Ny chatt",
    close_rail: "Stäng navigeringsfält",
    expand_rail: "Expandera navigeringsfält",
    collapse_rail: "Minimera navigeringsfält",
};

const NAV_ZH: NavCatalog = NavCatalog {
    threads: "会话",
    chats: "聊天",
    pulls: "拉取",
    pull_requests: "拉取请求",
    prs_short: "合并请求",
    issues: "议题",
    usage: "用量",
    settings: "设置",
    new_thread: "新建会话",
    new_chat: "新聊天",
    close_rail: "关闭导航栏",
    expand_rail: "展开导航栏",
    collapse_rail: "收起导航栏",
};
