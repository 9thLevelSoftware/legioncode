//! Ordinary English word guard and distinctive project-name recasing (`d20e6c294c` #39884).

use std::collections::HashSet;

use crate::custom_words::apply_speech_aliases;
use crate::types::SpeechCustomWord;

const ORDINARY_ENGLISH_WORDS: &[&str] = &[
    "agent",
    "agents",
    "api",
    "app",
    "apps",
    "auth",
    "backend",
    "base",
    "blog",
    "bot",
    "bots",
    "box",
    "branch",
    "branches",
    "build",
    "chat",
    "chats",
    "cli",
    "client",
    "clients",
    "cloud",
    "code",
    "common",
    "config",
    "core",
    "dashboard",
    "data",
    "db",
    "demo",
    "desktop",
    "dev",
    "diff",
    "doc",
    "docs",
    "edge",
    "engine",
    "env",
    "example",
    "file",
    "files",
    "flow",
    "frontend",
    "git",
    "home",
    "host",
    "hub",
    "index",
    "infra",
    "internal",
    "ios",
    "issue",
    "issues",
    "kit",
    "lab",
    "labs",
    "lib",
    "libs",
    "linux",
    "local",
    "mac",
    "main",
    "master",
    "mobile",
    "model",
    "models",
    "native",
    "new",
    "next",
    "node",
    "notes",
    "open",
    "ops",
    "package",
    "packages",
    "page",
    "pages",
    "pkg",
    "platform",
    "plugin",
    "plugins",
    "portal",
    "preview",
    "pro",
    "project",
    "projects",
    "proxy",
    "relay",
    "repo",
    "repos",
    "review",
    "root",
    "run",
    "runner",
    "runtime",
    "rust",
    "sandbox",
    "schema",
    "script",
    "scripts",
    "sdk",
    "search",
    "server",
    "service",
    "services",
    "session",
    "setting",
    "settings",
    "shared",
    "shell",
    "site",
    "spec",
    "speech",
    "src",
    "stack",
    "state",
    "store",
    "studio",
    "surface",
    "sync",
    "system",
    "target",
    "team",
    "term",
    "terminal",
    "test",
    "tests",
    "theme",
    "thread",
    "threads",
    "tool",
    "tools",
    "tree",
    "tunnel",
    "ui",
    "update",
    "updates",
    "user",
    "users",
    "util",
    "utils",
    "voice",
    "web",
    "website",
    "worker",
    "workers",
    "workspace",
    "worktree",
];

/// Returns `true` when `token` is an ordinary English dictionary/project word that must never be
/// recased by project-name hints (`"code"`, `"web"`, `"app"`, `"main"`, etc.).
pub fn is_ordinary_english_word(token: &str) -> bool {
    let lower = token.trim().to_ascii_lowercase();
    ORDINARY_ENGLISH_WORDS
        .binary_search(&lower.as_str())
        .is_ok()
}

/// Returns `true` when `token` has distinctive mixed/upper casing (CamelCase, internal capitals,
/// or mixed digits+capitals like `"T3Code"`, `"OpenAI"`, `"macOS"`, `"GitHub"`, `"NextJS"`)
/// rather than plain lowercase or simple single-initial `TitleCase` (`"Code"`, `"Web"`, `"App"`).
pub fn has_distinctive_project_casing(token: &str) -> bool {
    let trimmed = token.trim();
    if trimmed.is_empty() || is_ordinary_english_word(trimmed) {
        return false;
    }
    let chars: Vec<char> = trimmed.chars().collect();
    let has_upper = chars.iter().any(|ch| ch.is_uppercase());
    let has_lower = chars.iter().any(|ch| ch.is_lowercase());
    let has_digit = chars.iter().any(char::is_ascii_digit);
    let has_internal_upper = chars
        .iter()
        .enumerate()
        .skip(1)
        .any(|(idx, ch)| ch.is_uppercase() && chars[idx - 1].is_lowercase());
    let upper_count = chars.iter().filter(|ch| ch.is_uppercase()).count();

    has_internal_upper || (has_upper && has_digit) || (upper_count >= 2 && has_lower)
}

/// Extracts only distinctive, non-ordinary project-name tokens safe to use for token recasing.
pub fn project_name_recase_hints(project_name: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for raw in
        project_name.split(|ch: char| ch.is_whitespace() || matches!(ch, '-' | '_' | '/' | '.'))
    {
        let token = raw.trim();
        if has_distinctive_project_casing(token) && seen.insert(token.to_lowercase()) {
            out.push(token.to_owned());
        }
    }
    out
}

/// Recases spoken tokens matching distinctive project-name tokens without touching ordinary words.
pub fn apply_project_name_hints(text: &str, project_name: &str) -> String {
    let hints = project_name_recase_hints(project_name);
    if hints.is_empty() {
        return text.to_owned();
    }
    let dict: Vec<SpeechCustomWord> = hints
        .into_iter()
        .map(|term| SpeechCustomWord {
            term,
            aliases: Vec::new(),
        })
        .collect();
    apply_speech_aliases(text, &dict)
}
