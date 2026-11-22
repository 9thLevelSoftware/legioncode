//! Localized web workspace chrome labels across all 10 `legion_edge::i18n` locales (`BUG-27`).
//!
//! Complements [`legion_surfaces::mobile_i18n::MobileNavigationPresenter`] and
//! [`legion_surfaces::mobile_i18n::AgentLiveActivityPresenter`] so every panel, button, placeholder,
//! and status badge updates synchronously when switching across `en`, `de`, `es`, `fr`, `ja`,
//! `ko`, `pt-BR`, `ru`, `sv`, and `zh`.

use serde::Serialize;

use crate::i18n_web_intl::{COPY_KO, COPY_PT_BR, COPY_RU, COPY_SV, COPY_ZH};

/// Web-specific localized chrome labels for all 10 supported locales.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WebCopyCatalog {
    /// Workspace app title.
    pub app_title: &'static str,
    /// Pairing section title.
    pub pairing_title: &'static str,
    /// Pairing token input placeholder.
    pub pairing_placeholder: &'static str,
    /// Pair submit button text.
    pub pair_button: &'static str,
    /// Paired status label.
    pub paired_status: &'static str,
    /// Unpaired status label.
    pub unpaired_status: &'static str,
    /// Sign out button label.
    pub sign_out: &'static str,
    /// Projects section heading.
    pub projects_title: &'static str,
    /// Project workspace root input placeholder.
    pub workspace_placeholder: &'static str,
    /// Add project button label.
    pub add_project: &'static str,
    /// Empty projects state label.
    pub no_projects: &'static str,
    /// Empty threads state label.
    pub no_threads: &'static str,
    /// Chat right-pane tab label.
    pub tab_chat: &'static str,
    /// Diff right-pane tab label.
    pub tab_diff: &'static str,
    /// Terminal right-pane tab label.
    pub tab_terminal: &'static str,
    /// Composer textarea placeholder.
    pub composer_placeholder: &'static str,
    /// Send prompt button label.
    pub send_button: &'static str,
    /// Empty chat state hint.
    pub select_thread_hint: &'static str,
    /// Empty diff state hint.
    pub no_diff_loaded: &'static str,
    /// Terminal paste input placeholder.
    pub terminal_paste_placeholder: &'static str,
    /// Terminal snapshot export button label.
    pub terminal_export: &'static str,
    /// Terminal reset button label.
    pub terminal_reset: &'static str,
    /// Command palette search input placeholder.
    pub palette_placeholder: &'static str,
    /// Command palette trigger button label.
    pub palette_trigger: &'static str,
    /// Command palette empty result hint.
    pub palette_empty: &'static str,
    /// Locale selector label.
    pub language_label: &'static str,
}

/// Returns the static localized web copy catalog for a canonical `legion_edge::i18n` locale code.
pub fn web_copy_for_locale(locale: &str) -> &'static WebCopyCatalog {
    match locale {
        "de" => &COPY_DE,
        "es" => &COPY_ES,
        "fr" => &COPY_FR,
        "ja" => &COPY_JA,
        "ko" => &COPY_KO,
        "pt-BR" => &COPY_PT_BR,
        "ru" => &COPY_RU,
        "sv" => &COPY_SV,
        "zh" => &COPY_ZH,
        _ => &COPY_EN,
    }
}

const COPY_EN: WebCopyCatalog = WebCopyCatalog {
    app_title: "LegionCode",
    pairing_title: "Pairing",
    pairing_placeholder: "Enter pairing token",
    pair_button: "Pair",
    paired_status: "Paired",
    unpaired_status: "Unpaired",
    sign_out: "Sign out",
    projects_title: "Projects",
    workspace_placeholder: "Workspace path",
    add_project: "Add project",
    no_projects: "No projects loaded",
    no_threads: "No threads yet",
    tab_chat: "Chat",
    tab_diff: "Diff",
    tab_terminal: "Terminal",
    composer_placeholder: "Message the coding agent...",
    send_button: "Send",
    select_thread_hint: "Select or create a thread to stream updates.",
    no_diff_loaded: "No diff loaded for review.",
    terminal_paste_placeholder: "Paste command into terminal",
    terminal_export: "Export LCVT",
    terminal_reset: "Reset VT",
    palette_placeholder: "Search commands, projects, and threads...",
    palette_trigger: "Command Palette (Ctrl+K)",
    palette_empty: "No matching commands",
    language_label: "Language",
};

const COPY_DE: WebCopyCatalog = WebCopyCatalog {
    app_title: "LegionCode",
    pairing_title: "Kopplung",
    pairing_placeholder: "Kopplungs-Token eingeben",
    pair_button: "Koppeln",
    paired_status: "Gekoppelt",
    unpaired_status: "Nicht gekoppelt",
    sign_out: "Abmelden",
    projects_title: "Projekte",
    workspace_placeholder: "Arbeitsbereich-Pfad",
    add_project: "Projekt hinzufügen",
    no_projects: "Keine Projekte geladen",
    no_threads: "Noch keine Themen",
    tab_chat: "Chat",
    tab_diff: "Diff",
    tab_terminal: "Terminal",
    composer_placeholder: "Nachricht an den Coding-Agent...",
    send_button: "Senden",
    select_thread_hint: "Thema auswählen oder erstellen, um Updates zu streamen.",
    no_diff_loaded: "Kein Diff zur Prüfung geladen.",
    terminal_paste_placeholder: "Befehl ins Terminal einfügen",
    terminal_export: "LCVT exportieren",
    terminal_reset: "VT zurücksetzen",
    palette_placeholder: "Befehle, Projekte und Themen suchen...",
    palette_trigger: "Befehlspalette (Ctrl+K)",
    palette_empty: "Keine passenden Befehle",
    language_label: "Sprache",
};

const COPY_ES: WebCopyCatalog = WebCopyCatalog {
    app_title: "LegionCode",
    pairing_title: "Vinculación",
    pairing_placeholder: "Ingresa el token de vinculación",
    pair_button: "Vincular",
    paired_status: "Vinculado",
    unpaired_status: "Sin vincular",
    sign_out: "Cerrar sesión",
    projects_title: "Proyectos",
    workspace_placeholder: "Ruta del espacio de trabajo",
    add_project: "Agregar proyecto",
    no_projects: "Sin proyectos cargados",
    no_threads: "Aún no hay hilos",
    tab_chat: "Chat",
    tab_diff: "Diferencias",
    tab_terminal: "Terminal",
    composer_placeholder: "Escribe al agente de programación...",
    send_button: "Enviar",
    select_thread_hint: "Selecciona o crea un hilo para ver eventos.",
    no_diff_loaded: "No hay diff cargado para revisar.",
    terminal_paste_placeholder: "Pegar comando en terminal",
    terminal_export: "Exportar LCVT",
    terminal_reset: "Reiniciar VT",
    palette_placeholder: "Buscar comandos, proyectos e hilos...",
    palette_trigger: "Paleta de comandos (Ctrl+K)",
    palette_empty: "Sin comandos coincidentes",
    language_label: "Idioma",
};

const COPY_FR: WebCopyCatalog = WebCopyCatalog {
    app_title: "LegionCode",
    pairing_title: "Appairage",
    pairing_placeholder: "Saisir le jeton d'appairage",
    pair_button: "Appairer",
    paired_status: "Appairé",
    unpaired_status: "Non appairé",
    sign_out: "Déconnexion",
    projects_title: "Projets",
    workspace_placeholder: "Chemin de l'espace de travail",
    add_project: "Ajouter un projet",
    no_projects: "Aucun projet chargé",
    no_threads: "Aucun fil pour l'instant",
    tab_chat: "Discussion",
    tab_diff: "Diff",
    tab_terminal: "Terminal",
    composer_placeholder: "Écrire à l'agent de code...",
    send_button: "Envoyer",
    select_thread_hint: "Sélectionnez ou créez un fil pour suivre le flux.",
    no_diff_loaded: "Aucun diff chargé pour révision.",
    terminal_paste_placeholder: "Coller une commande dans le terminal",
    terminal_export: "Exporter LCVT",
    terminal_reset: "Réinitialiser VT",
    palette_placeholder: "Rechercher commandes, projets et fils...",
    palette_trigger: "Palette de commandes (Ctrl+K)",
    palette_empty: "Aucune commande correspondante",
    language_label: "Langue",
};

const COPY_JA: WebCopyCatalog = WebCopyCatalog {
    app_title: "LegionCode",
    pairing_title: "ペアリング",
    pairing_placeholder: "ペアリングトークンを入力",
    pair_button: "ペアリング",
    paired_status: "接続済み",
    unpaired_status: "未接続",
    sign_out: "サインアウト",
    projects_title: "プロジェクト",
    workspace_placeholder: "ワークスペースパス",
    add_project: "プロジェクト追加",
    no_projects: "プロジェクトがありません",
    no_threads: "スレッドがまだありません",
    tab_chat: "チャット",
    tab_diff: "差分",
    tab_terminal: "ターミナル",
    composer_placeholder: "コーディングエージェントへ送信...",
    send_button: "送信",
    select_thread_hint: "スレッドを選択または作成して更新を受信します。",
    no_diff_loaded: "レビュー対象の差分はありません。",
    terminal_paste_placeholder: "ターミナルへコマンドを貼り付け",
    terminal_export: "LCVT出力",
    terminal_reset: "VTリセット",
    palette_placeholder: "コマンド・プロジェクト・スレッドを検索...",
    palette_trigger: "コマンドパレット (Ctrl+K)",
    palette_empty: "一致するコマンドはありません",
    language_label: "言語",
};
