//! Antigravity `skills.json` discovery, filter inheritance, and user rule syncing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

pub use super::skills_sync::{ANTIGRAVITY_USER_RULE_FILES, sync_antigravity_user_rule_copies};

const MAX_INHERIT_DEPTH: usize = 10;
const MAX_SCAN_ENTRIES: usize = 10_000;

/// Parsed `skills.json` configuration tolerating `null` or omitted arrays (`ece4964b8e`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct AntigravitySkillsConfig {
    /// Optional inherited `skills.json` manifests processed before local `entries`.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub inherits: Vec<SkillConfigEntry>,
    /// Skill directories or direct `SKILL.md` file entries.
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub entries: Vec<SkillConfigEntry>,
}

/// One entry in `skills.json` `inherits` or `entries` (`1c379b63ba`, `ece4964b8e`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SkillConfigEntry {
    /// Directory, `SKILL.md` file, or inherited `skills.json` path (no `~/` expansion).
    pub path: String,
    /// Optional allowlist patterns (`null` / `[]` treated as unrestricted).
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub include_only: Vec<String>,
    /// Optional denylist patterns (`null` / `[]` treated as empty).
    #[serde(default, deserialize_with = "deserialize_null_default")]
    pub exclude: Vec<String>,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    T: Default + Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Discovered Antigravity skill metadata returned by [`discover_antigravity_skills`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AntigravityDiscoveredSkill {
    /// Unique skill identifier (frontmatter `name` or filename stem).
    pub name: String,
    /// Frontmatter summary when available.
    pub description: Option<String>,
    /// Resolved path to the `SKILL.md` file.
    pub path: PathBuf,
    /// Origin scope (`"project"` or `"user"`).
    pub scope: String,
}

#[derive(Debug, Clone, Default)]
struct SkillEntryFilter {
    include_groups: Vec<Vec<String>>,
    exclude_names: Vec<String>,
}

impl SkillEntryFilter {
    fn narrow(&self, entry: &SkillConfigEntry) -> Self {
        let mut include_groups = self.include_groups.clone();
        if !entry.include_only.is_empty() {
            include_groups.push(entry.include_only.clone());
        }
        let mut exclude_names = self.exclude_names.clone();
        if !entry.exclude.is_empty() {
            exclude_names.extend(entry.exclude.iter().cloned());
        }
        Self {
            include_groups,
            exclude_names,
        }
    }

    fn matches(&self, entry_name: &str) -> bool {
        self.include_groups
            .iter()
            .all(|group| group.iter().any(|name| name == entry_name))
            && !self.exclude_names.iter().any(|name| name == entry_name)
    }
}

/// Resolves a `skills.json` entry path strictly against `base_dir` without expanding `~/`
/// (`367f9d3759`, matching native `agy`).
#[must_use]
pub fn resolve_skills_config_path(raw_path: &str, base_dir: &Path) -> PathBuf {
    let candidate = Path::new(raw_path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        base_dir.join(candidate)
    }
}

/// Discovers Antigravity skills across `.agents/skills.json`, `.agent/skills.json`, and
/// `<user_home>/.gemini/config/skills.json`, honoring `inherits`, cycles, filters, and
/// direct `SKILL.md` file entries (`1c379b63ba`, `26b48d2da8`, `ece4964b8e`, `367f9d3759`).
#[must_use]
pub fn discover_antigravity_skills(
    cwd: &Path,
    user_home: &Path,
) -> Vec<AntigravityDiscoveredSkill> {
    let mut scanner = SkillScanner {
        skills: BTreeMap::new(),
        remaining_entries: MAX_SCAN_ENTRIES,
    };
    let manifest_roots = [
        (cwd.join(".agents").join("skills.json"), "project"),
        (cwd.join(".agent").join("skills.json"), "project"),
        (
            user_home.join(".gemini").join("config").join("skills.json"),
            "user",
        ),
    ];
    for (config_path, scope) in manifest_roots {
        let base_dir = config_path.parent().unwrap_or(cwd);
        let mut visited = BTreeSet::new();
        scanner.scan_config(
            &config_path,
            base_dir,
            scope,
            &SkillEntryFilter::default(),
            0,
            &mut visited,
        );
    }
    scanner.skills.into_values().collect()
}

struct SkillScanner {
    skills: BTreeMap<String, AntigravityDiscoveredSkill>,
    remaining_entries: usize,
}

impl SkillScanner {
    fn scan_config(
        &mut self,
        config_path: &Path,
        base_dir: &Path,
        scope: &str,
        filter: &SkillEntryFilter,
        depth: usize,
        visited: &mut BTreeSet<PathBuf>,
    ) {
        if depth > MAX_INHERIT_DEPTH {
            return;
        }
        let canonical =
            std::fs::canonicalize(config_path).unwrap_or_else(|_| config_path.to_path_buf());
        if !visited.insert(canonical) {
            return;
        }
        let Ok(raw) = std::fs::read_to_string(config_path) else {
            return;
        };
        let Ok(parsed) = serde_json::from_str::<AntigravitySkillsConfig>(&raw) else {
            return;
        };
        for inherited in &parsed.inherits {
            let next_path = resolve_skills_config_path(&inherited.path, base_dir);
            let next_base = next_path.parent().unwrap_or(base_dir);
            let next_filter = filter.narrow(inherited);
            self.scan_config(
                &next_path,
                next_base,
                scope,
                &next_filter,
                depth + 1,
                visited,
            );
        }
        for entry in &parsed.entries {
            let entry_path = resolve_skills_config_path(&entry.path, base_dir);
            let next_filter = filter.narrow(entry);
            self.scan_target(&entry_path, scope, true, &next_filter, true);
        }
    }

    fn scan_target(
        &mut self,
        target: &Path,
        scope: &str,
        scan_children: bool,
        filter: &SkillEntryFilter,
        accept_skill_file: bool,
    ) {
        let Ok(meta) = std::fs::metadata(target) else {
            return;
        };
        if meta.is_file() && accept_skill_file {
            if self.remaining_entries == 0 {
                return;
            }
            self.remaining_entries -= 1;
            let parent_name = target
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .unwrap_or_default();
            if filter.matches(parent_name) {
                self.add_skill_file(target, scope);
            }
            return;
        }
        if !meta.is_dir() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(target) else {
            return;
        };
        let mut names: Vec<(String, PathBuf)> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                e.file_name()
                    .to_str()
                    .map(|name| (name.to_owned(), e.path()))
            })
            .collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        if let Some((_, skill_path)) = names
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("skill.md"))
        {
            self.add_skill_file(skill_path, scope);
            return;
        }
        if !scan_children {
            return;
        }
        for (name, child_path) in names {
            if self.remaining_entries == 0 {
                return;
            }
            self.remaining_entries -= 1;
            if filter.matches(&name) {
                self.scan_target(&child_path, scope, false, filter, false);
            }
        }
    }

    fn add_skill_file(&mut self, skill_path: &Path, scope: &str) {
        let Some(file_name) = skill_path.file_name().and_then(|s| s.to_str()) else {
            return;
        };
        if !file_name
            .strip_suffix(".md")
            .is_some_and(|stem| stem.eq_ignore_ascii_case("skill"))
        {
            return;
        }
        let Ok(contents) = std::fs::read_to_string(skill_path) else {
            return;
        };
        let (name, description) = parse_skill_frontmatter(&contents, skill_path);
        self.skills
            .entry(name.clone())
            .or_insert_with(|| AntigravityDiscoveredSkill {
                name,
                description,
                path: skill_path.to_path_buf(),
                scope: scope.to_owned(),
            });
    }
}

fn parse_skill_frontmatter(contents: &str, skill_path: &Path) -> (String, Option<String>) {
    let mut name = None;
    let mut description = None;
    if let Some(rest) = contents
        .strip_prefix("---\n")
        .or_else(|| contents.strip_prefix("---\r\n"))
        && let Some((fm, _)) = rest
            .split_once("\n---")
            .or_else(|| rest.split_once("\r\n---"))
    {
        for line in fm.lines() {
            if let Some(val) = line.strip_prefix("name:") {
                let cleaned = val.trim().trim_matches(['"', '\'']);
                if !cleaned.is_empty() {
                    name = Some(cleaned.to_owned());
                }
            } else if let Some(val) = line.strip_prefix("description:") {
                let cleaned = val.trim().trim_matches(['"', '\'']);
                if !cleaned.is_empty() {
                    description = Some(cleaned.to_owned());
                }
            }
        }
    }
    let resolved_name = name.unwrap_or_else(|| {
        skill_path
            .parent()
            .and_then(Path::file_name)
            .and_then(|s| s.to_str())
            .unwrap_or("skill")
            .to_owned()
    });
    (resolved_name, description)
}
