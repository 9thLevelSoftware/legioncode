//! Global rule and `skills.json` copy synchronization into isolated `GEMINI_HOME` profiles.

use std::collections::BTreeMap;
use std::path::Path;

const RULE_COPIES_MANIFEST: &str = ".legioncode-rule-copies.json";

/// Global Antigravity rule and skills manifest files synchronized from `~/.gemini` (`e923b5d8d8`, `1c379b63ba`).
pub const ANTIGRAVITY_USER_RULE_FILES: &[&str] = &["AGENTS.md", "GEMINI.md", "config/skills.json"];

/// Synchronizes global rule/manifest copies from `user_gemini_dir` (`~/.gemini`) into
/// `profile_dir`, preserving profile-local edits tracked via `.legioncode-rule-copies.json`
/// (`e923b5d8d8`, `1c379b63ba`).
pub fn sync_antigravity_user_rule_copies(
    user_gemini_dir: &Path,
    profile_dir: &Path,
) -> std::io::Result<()> {
    let manifest_path = profile_dir.join(RULE_COPIES_MANIFEST);
    let mut copies: BTreeMap<String, String> = std::fs::read_to_string(&manifest_path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();

    for rel in ANTIGRAVITY_USER_RULE_FILES {
        let dest = profile_dir.join(rel);
        let src = user_gemini_dir.join(rel);
        let current = std::fs::read_to_string(&dest).ok();
        if let Some(existing) = &current
            && copies.get(*rel) != Some(existing)
        {
            copies.remove(*rel);
            continue;
        }
        if let Ok(source_contents) = std::fs::read_to_string(&src) {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&dest, &source_contents)?;
            copies.insert((*rel).to_owned(), source_contents);
        } else {
            if current.is_some() {
                let _ = std::fs::remove_file(&dest);
            }
            copies.remove(*rel);
        }
    }

    if let Some(parent) = manifest_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let serialized = serde_json::to_string_pretty(&copies).unwrap_or_else(|_| "{}".to_owned());
    std::fs::write(manifest_path, serialized)
}
