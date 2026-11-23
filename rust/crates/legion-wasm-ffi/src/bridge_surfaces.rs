//! Pure-Rust host-testable bridge helpers for [`legion_surfaces`], [`legion_edge::i18n`], and
//! [`legion_contracts::permissions`].

use std::collections::BTreeMap;

use legion_client_core::diff_markdown::Utf16Span;
use legion_contracts::permissions::{
    EnvironmentScope, SessionGrantInput, WsMethod, grants_all_scopes, required_scopes_for_rpc,
};
use legion_edge::i18n::{
    DEFAULT_LOCALE, SUPPORTED_LOCALE_CODES, SYSTEM_LANGUAGE, localized_href, lookup_locale,
    resolve_locale_selection,
};
use legion_surfaces::mobile_diff::{
    AttributedDiffRun, FontStyleFlags, SyntaxTokenInput, build_syntax_runs_from_tokens,
    decorate_diff_runs, decorate_diff_runs_with_background,
};
use legion_surfaces::mobile_i18n::{
    AgentActivityInput, AgentLiveActivityPresentation, AgentLiveActivityPresenter,
    MobileNavigationPresenter, MobileNavigationStrings,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DiffRunInput {
    start: u32,
    end: Option<u32>,
    text: String,
    #[serde(alias = "foreground_color")]
    foreground_color: Option<String>,
    #[serde(alias = "background_color")]
    background_color: Option<String>,
    #[serde(alias = "font_style")]
    font_style: u8,
    #[serde(alias = "word_highlighted")]
    word_highlighted: bool,
    #[serde(alias = "ligatures_enabled")]
    ligatures_enabled: bool,
    #[serde(alias = "is_note")]
    is_note: bool,
    attributes: BTreeMap<String, String>,
}

impl DiffRunInput {
    fn into_attributed_run(self) -> AttributedDiffRun {
        let utf16_len = u32::try_from(self.text.encode_utf16().count()).unwrap_or(u32::MAX);
        let end = self
            .end
            .unwrap_or_else(|| self.start.saturating_add(utf16_len));
        AttributedDiffRun {
            start: self.start,
            end,
            text: self.text,
            foreground_color: self.foreground_color,
            background_color: self.background_color,
            font_style: FontStyleFlags::from_bits_truncate(self.font_style),
            word_highlighted: self.word_highlighted,
            ligatures_enabled: self.ligatures_enabled,
            is_note: self.is_note,
            attributes: self.attributes,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct DecorateDiffRequest {
    #[serde(alias = "syntaxRuns", alias = "syntax_runs")]
    runs: Option<Vec<DiffRunInput>>,
    tokens: Option<Vec<SyntaxTokenInput>>,
    #[serde(
        alias = "wordHighlightRanges",
        alias = "wordHighlights",
        alias = "word_highlights"
    )]
    highlights: Vec<Utf16Span>,
    #[serde(alias = "highlight_background")]
    highlight_background: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DecorateDiffResponse {
    runs: Vec<AttributedDiffRun>,
}

/// Splits and coalesces syntax diff runs against intra-line UTF-16 highlight ranges (`BUG-22`).
pub fn decorate_diff_runs_json(input_json: &str) -> Result<String, String> {
    let req: DecorateDiffRequest = serde_json::from_str(input_json)
        .map_err(|err| format!("invalid decorate diff JSON: {err}"))?;

    let base_runs = if let Some(runs) = req.runs {
        runs.into_iter()
            .map(DiffRunInput::into_attributed_run)
            .collect()
    } else if let Some(tokens) = req.tokens {
        build_syntax_runs_from_tokens(&tokens)
    } else {
        return Err("decorate_diff_runs_json requires `runs` or `tokens`".to_owned());
    };

    let decorated = match req.highlight_background.as_deref() {
        Some(bg) => decorate_diff_runs_with_background(&base_runs, &req.highlights, bg),
        None => decorate_diff_runs(&base_runs, &req.highlights),
    };

    serde_json::to_string(&DecorateDiffResponse { runs: decorated })
        .map_err(|err| format!("failed to serialize diff runs: {err}"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct TranslateI18nRequest {
    #[serde(alias = "locale", alias = "tag")]
    language: Option<String>,
    #[serde(alias = "system_locales")]
    system_locales: Vec<String>,
    path: Option<String>,
    activity: Option<AgentActivityInput>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslateI18nResponse {
    locale: &'static str,
    formatting_locale: String,
    edge_catalog_locale: &'static str,
    bcp47_tag: &'static str,
    native_name: &'static str,
    marketing_name: &'static str,
    url_path: &'static str,
    localized_href: Option<String>,
    supported_locales: &'static [&'static str],
    navigation: MobileNavigationStrings,
    live_activity: Option<AgentLiveActivityPresentation>,
}

/// Resolves locale settings (`BUG-27`) and localized surface chrome/Live Activity (`BUG-24`).
pub fn translate_i18n_json(input_json: &str) -> Result<String, String> {
    let req: TranslateI18nRequest =
        serde_json::from_str(input_json).map_err(|err| format!("invalid i18n JSON: {err}"))?;

    let language = req.language.as_deref().unwrap_or(SYSTEM_LANGUAGE);
    let sys_refs: Vec<&str> = req.system_locales.iter().map(String::as_str).collect();
    let nav = MobileNavigationPresenter::from_settings(language, &sys_refs);
    let edge_selection = resolve_locale_selection(language, &sys_refs);
    let descriptor = lookup_locale(nav.locale())
        .or_else(|| lookup_locale(DEFAULT_LOCALE))
        .ok_or_else(|| format!("missing canonical descriptor for `{}`", nav.locale()))?;
    let localized_path = req
        .path
        .as_deref()
        .map(|path| localized_href(descriptor, path));
    let live_activity = req.activity.as_ref().map(|activity| {
        let presenter = AgentLiveActivityPresenter::from_settings(language, &sys_refs);
        presenter.present(activity)
    });

    let response = TranslateI18nResponse {
        locale: nav.locale(),
        formatting_locale: nav.formatting_locale().to_owned(),
        edge_catalog_locale: edge_selection.catalog_locale,
        bcp47_tag: descriptor.bcp47_tag,
        native_name: descriptor.native_name,
        marketing_name: descriptor.marketing_name,
        url_path: descriptor.url_path,
        localized_href: localized_path,
        supported_locales: SUPPORTED_LOCALE_CODES,
        navigation: nav.strings(),
        live_activity,
    };
    serde_json::to_string(&response).map_err(|err| format!("failed to serialize i18n JSON: {err}"))
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct SessionGrantDto {
    authenticated: bool,
    scopes: Option<Vec<EnvironmentScope>>,
    permissions: Option<Vec<EnvironmentScope>>,
    #[serde(alias = "server_update_scope")]
    server_update_scope: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateRpcScopesRequest {
    method: String,
    #[serde(default)]
    payload: Option<serde_json::Value>,
    #[serde(default)]
    session: Option<SessionGrantDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EvaluateRpcScopesResponse<'a> {
    method: &'a str,
    required_scopes: Vec<EnvironmentScope>,
    allowed: Option<bool>,
}

/// Evaluates fail-closed multi-scope `RPC` authorization requirements (`BUG-17`).
pub fn evaluate_rpc_scopes_json(input_json: &str) -> Result<String, String> {
    let req: EvaluateRpcScopesRequest =
        serde_json::from_str(input_json).map_err(|err| format!("invalid RPC scope JSON: {err}"))?;
    let method = WsMethod::parse(&req.method).map_err(|err| err.to_string())?;
    let required_scopes =
        required_scopes_for_rpc(&method, req.payload.as_ref()).map_err(|err| err.to_string())?;
    let allowed = req.session.map(|dto| {
        let session = SessionGrantInput {
            authenticated: dto.authenticated,
            scopes: dto.scopes,
            permissions: dto.permissions,
            server_update_scope: dto.server_update_scope,
        };
        grants_all_scopes(&session, &required_scopes)
    });

    let response = EvaluateRpcScopesResponse {
        method: method.as_str(),
        required_scopes,
        allowed,
    };
    serde_json::to_string(&response)
        .map_err(|err| format!("failed to serialize RPC scopes JSON: {err}"))
}
