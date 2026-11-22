//! Localized labels for mobile text selection menus (`#40297`) and outdated PR approvals
//! (`#36868`) across all 10 supported locales without falling back to English.

use serde::{Deserialize, Serialize};

use crate::mobile_i18n::resolve_surface_locale;

/// Canonical locales shipped by T3 Code mobile and desktop apps.
pub const SUPPORTED_REVIEW_LOCALES: [&str; 10] = [
    "en", "de", "es", "fr", "ja", "ko", "pt-BR", "ru", "sv", "zh",
];

/// Native text-selection menu item labels (`e0f3ab9e78` #40297).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSelectionMenuLabels {
    /// Resolved canonical catalog locale.
    pub locale: String,
    /// Copy plain text (`a034d65164bb39de`).
    pub copy: String,
    /// Copy formatted Markdown (`0165ef55eb6c8a99`).
    pub copy_as_markdown: String,
    /// Copy single image URL (`439737cb6de2a5b3`).
    pub copy_image_url: String,
    /// Plural template containing `{count}` (`d3d24474c922eef2`).
    pub copy_image_urls_template: String,
}

impl TextSelectionMenuLabels {
    /// Resolves text-selection labels for the selected `language` and ordered `system_locales`.
    #[must_use]
    pub fn for_locale(language: &str, system_locales: &[&str]) -> Self {
        let (canonical, _) = resolve_surface_locale(language, system_locales);
        let (copy, copy_as_markdown, copy_image_url, copy_image_urls_template) =
            selection_catalog(canonical);
        Self {
            locale: canonical.to_owned(),
            copy: copy.to_owned(),
            copy_as_markdown: copy_as_markdown.to_owned(),
            copy_image_url: copy_image_url.to_owned(),
            copy_image_urls_template: copy_image_urls_template.to_owned(),
        }
    }

    /// Formats the image URL copy label for `count` images.
    #[must_use]
    pub fn format_copy_image_urls(&self, count: usize) -> String {
        if count <= 1 {
            self.copy_image_url.clone()
        } else {
            self.copy_image_urls_template
                .replace("{count}", &count.to_string())
        }
    }
}

/// Pull request reviewer approval labels distinguishing current vs. outdated approvals (`#36868`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewApprovalLabels {
    /// Resolved canonical catalog locale.
    pub locale: String,
    /// Current approval (`1e74602ca35d4c7e`).
    pub approved: String,
    /// Accessibility label when approved before the latest commit (`cf3ad61859b09910`).
    pub approved_before_latest_commit: String,
    /// Badge shown beside stale approvals (`3e562db94c685f71`).
    pub outdated: String,
    /// Comment-only review state (`9f3f817e9111e162`).
    pub commented: String,
    /// Fallback reviewer display name (`301edbbd0597cf4d`).
    pub unknown_reviewer: String,
}

impl ReviewApprovalLabels {
    /// Resolves approval labels for the selected `language` and ordered `system_locales`.
    #[must_use]
    pub fn for_locale(language: &str, system_locales: &[&str]) -> Self {
        let (canonical, _) = resolve_surface_locale(language, system_locales);
        let (approved, approved_before_latest_commit, outdated, commented, unknown_reviewer) =
            approval_catalog(canonical);
        Self {
            locale: canonical.to_owned(),
            approved: approved.to_owned(),
            approved_before_latest_commit: approved_before_latest_commit.to_owned(),
            outdated: outdated.to_owned(),
            commented: commented.to_owned(),
            unknown_reviewer: unknown_reviewer.to_owned(),
        }
    }
}

const fn selection_catalog(
    locale: &str,
) -> (&'static str, &'static str, &'static str, &'static str) {
    match locale.as_bytes() {
        b"de" => (
            "Kopieren",
            "Als Markdown kopieren",
            "Bild-URL kopieren",
            "{count} Bild-URLs kopieren",
        ),
        b"es" => (
            "Copiar",
            "Copiar como Markdown",
            "Copiar URL de la imagen",
            "Copiar {count} URL de imágenes",
        ),
        b"fr" => (
            "Copier",
            "Copier en Markdown",
            "Copier l’URL de l’image",
            "Copier {count} URL d’images",
        ),
        b"ja" => (
            "コピー",
            "Markdown としてコピー",
            "画像URLをコピー",
            "{count}件の画像URLをコピー",
        ),
        b"ko" => (
            "복사",
            "Markdown으로 복사",
            "이미지 URL 복사",
            "이미지 URL {count}개 복사",
        ),
        b"pt-BR" => (
            "Copiar",
            "Copiar como Markdown",
            "Copiar URL da imagem",
            "Copiar {count} URLs de imagens",
        ),
        b"ru" => (
            "Копировать",
            "Копировать как Markdown",
            "Копировать URL изображения",
            "Копировать URL изображений: {count}",
        ),
        b"sv" => (
            "Kopiera",
            "Kopiera som Markdown",
            "Kopiera bild-URL",
            "Kopiera {count} bild-URL:er",
        ),
        b"zh" => (
            "复制",
            "复制为 Markdown",
            "复制图片 URL",
            "复制 {count} 个图片 URL",
        ),
        _ => (
            "Copy",
            "Copy as Markdown",
            "Copy Image URL",
            "Copy {count} Image URLs",
        ),
    }
}

const fn approval_catalog(
    locale: &str,
) -> (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
) {
    match locale.as_bytes() {
        b"de" => (
            "Genehmigt",
            "Vor dem letzten Commit genehmigt",
            "Veraltet",
            "Kommentiert",
            "Unbekannter Reviewer",
        ),
        b"es" => (
            "Aprobada",
            "Aprobado antes del último commit",
            "Desactualizado",
            "Comentó",
            "Revisor desconocido",
        ),
        b"fr" => (
            "Approuvée",
            "Approuvé avant le dernier commit",
            "Obsolète",
            "Commentée",
            "Relecteur inconnu",
        ),
        b"ja" => (
            "承認済み",
            "最新のコミットより前に承認",
            "古い",
            "コメント済み",
            "不明なレビュアー",
        ),
        b"ko" => (
            "승인함",
            "최신 커밋 이전에 승인됨",
            "오래됨",
            "댓글 남김",
            "알 수 없는 검토자",
        ),
        b"pt-BR" => (
            "Aprovou",
            "Aprovado antes do último commit",
            "Desatualizado",
            "Comentou",
            "Revisor desconhecido",
        ),
        b"ru" => (
            "Одобрено",
            "Одобрено до последнего коммита",
            "Устарело",
            "Прокомментировано",
            "Неизвестный ревьюер",
        ),
        b"sv" => (
            "Godkänd",
            "Godkänd före den senaste committen",
            "Inaktuell",
            "Kommenterad",
            "Okänd granskare",
        ),
        b"zh" => (
            "已批准",
            "在最新提交之前批准",
            "已过时",
            "已评论",
            "未知审查者",
        ),
        _ => (
            "Approved",
            "Approved before the latest commit",
            "Outdated",
            "Commented",
            "Unknown reviewer",
        ),
    }
}
