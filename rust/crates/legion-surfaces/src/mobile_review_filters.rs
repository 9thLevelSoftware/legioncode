//! Changed-file classification and pull-request/review test-file filtering
//! (`packages/shared/src/changedFileKind.ts`, `pullRequestFileFilters.ts`, #26759).

use serde::{Deserialize, Serialize};

/// Category assigned to a changed path by [`classify_changed_file`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangedFileKind {
    /// Application or library source code.
    Code,
    /// Unit, integration, e2e, or story test files.
    Tests,
    /// Test support artifacts: mocks, fixtures, stubs, fakes, snapshots.
    Mocks,
    /// Generated artifacts, lockfiles, and source maps.
    Generated,
    /// Prose documentation (Markdown, reStructuredText, `AsciiDoc`, plain text).
    Docs,
}

const GENERATED_FILES: &[&str] = &[
    "bun.lock",
    "bun.lockb",
    "cargo.lock",
    "composer.lock",
    "deno.lock",
    "flake.lock",
    "gemfile.lock",
    "go.sum",
    "gradle.lockfile",
    "mix.lock",
    "npm-shrinkwrap.json",
    "package-lock.json",
    "packages.lock.json",
    "pipfile.lock",
    "pnpm-lock.yaml",
    "podfile.lock",
    "poetry.lock",
    "pubspec.lock",
    "uv.lock",
    "yarn.lock",
];

const GENERATED_SUFFIXES: &[&str] = &[
    ".g.dart",
    ".pb.go",
    "_pb2.py",
    "_pb2_grpc.py",
    ".pb.cc",
    ".pb.h",
    ".min.js",
    ".min.css",
    ".js.map",
    ".css.map",
];

const GENERATED_DIRS: &[&str] = &["__generated__", "_generated", "generated"];

const MOCK_DIRS: &[&str] = &[
    "__fixtures__",
    "__mocks__",
    "__snapshots__",
    "fakes",
    "fixture",
    "fixtures",
    "mock",
    "mocks",
    "stubs",
    "testdata",
];

const TEST_DIRS: &[&str] = &[
    "__tests__",
    "__test__",
    "cypress",
    "e2e",
    "evals",
    "spec",
    "specs",
    "test",
    "tests",
];

/// Classifies a repository-relative path into code, tests, mocks, generated, or docs.
#[must_use]
pub fn classify_changed_file(path: &str) -> ChangedFileKind {
    let normalized = path.replace('\\', "/");
    let mut parts = normalized.split('/');
    let name = parts.next_back().unwrap_or("");
    let lower_name = name.to_ascii_lowercase();
    let in_dir = |targets: &[&str]| {
        normalized.rsplit_once('/').is_some_and(|(dir_part, _)| {
            dir_part
                .split('/')
                .any(|seg| targets.iter().any(|t| seg.eq_ignore_ascii_case(t)))
        })
    };

    if GENERATED_FILES.contains(&lower_name.as_str())
        || is_generated_name(name)
        || in_dir(GENERATED_DIRS)
    {
        return ChangedFileKind::Generated;
    }
    if is_mock_name(name) || in_dir(MOCK_DIRS) {
        return ChangedFileKind::Mocks;
    }
    if is_test_name(&lower_name) || is_class_test_name(name) || in_dir(TEST_DIRS) {
        return ChangedFileKind::Tests;
    }
    if is_docs_name(&lower_name) && !is_not_docs_name(&lower_name) {
        return ChangedFileKind::Docs;
    }
    ChangedFileKind::Code
}

/// Returns `true` when `path` classifies as a test or test-support (`Mocks`) file.
#[must_use]
pub fn is_test_changed_file(path: &str) -> bool {
    matches!(
        classify_changed_file(path),
        ChangedFileKind::Tests | ChangedFileKind::Mocks
    )
}

fn is_generated_name(name: &str) -> bool {
    name.contains(".generated.")
        || name.contains(".gen.")
        || GENERATED_SUFFIXES
            .iter()
            .any(|suffix| name.ends_with(suffix))
}

fn is_mock_name(name: &str) -> bool {
    const KINDS: &[&str] = &[
        "fixtures", "fixture", "mocks", "mock", "stubs", "stub", "fakes", "fake",
    ];
    if name.strip_suffix(".snap").is_some() {
        return true;
    }
    if let Some((stem, ext)) = name.rsplit_once('.')
        && !ext.is_empty()
        && KINDS.iter().any(|kind| stem.ends_with(&format!(".{kind}")))
    {
        return true;
    }
    KINDS.iter().any(|kind| {
        name.strip_prefix(kind).is_some_and(|rest| {
            rest.is_empty()
                || matches!(
                    rest.as_bytes().first(),
                    Some(b'-' | b'_' | b'.' | b'A'..=b'Z')
                )
        })
    })
}

fn is_test_name(lower_name: &str) -> bool {
    if lower_name == "conftest.py"
        || lower_name
            .strip_prefix("test_")
            .and_then(|rest| rest.strip_suffix(".py"))
            .is_some_and(|mid| !mid.is_empty())
    {
        return true;
    }
    let Some((stem, ext)) = lower_name.rsplit_once('.') else {
        return false;
    };
    if ext.is_empty() {
        return false;
    }
    matches!(stem, "vitest.setup" | "jest.setup")
        || [
            ".test", ".spec", ".e2e", ".eval", ".stories", "_test", "_spec",
        ]
        .iter()
        .any(|marker| stem.ends_with(marker))
}

fn is_class_test_name(name: &str) -> bool {
    let Some((stem, ext)) = name.rsplit_once('.') else {
        return false;
    };
    matches!(
        ext,
        "java" | "kt" | "kts" | "scala" | "groovy" | "cs" | "php" | "swift" | "m"
    ) && (stem.ends_with("Test") || stem.ends_with("Tests") || stem.ends_with("Spec"))
}

fn is_docs_name(lower_name: &str) -> bool {
    let Some((_, ext)) = lower_name.rsplit_once('.') else {
        return false;
    };
    matches!(ext, "md" | "mdx" | "markdown" | "rst" | "adoc" | "txt")
}

fn is_not_docs_name(lower_name: &str) -> bool {
    let Some(stem) = lower_name.strip_suffix(".txt") else {
        return false;
    };
    stem.starts_with("requirements")
        || stem.starts_with("constraints")
        || matches!(stem, "cmakelists" | "robots" | "llms")
}

/// Summary of one file inside a parsed review diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReviewFileEntry {
    /// Repository-relative file path.
    pub path: String,
    /// Added line count.
    pub additions: u32,
    /// Deleted line count.
    pub deletions: u32,
}

/// Filtered review diff summary returned by [`filter_review_files`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilteredReviewFiles {
    /// Files kept visible in the review sheet.
    pub files: Vec<ReviewFileEntry>,
    /// Number of visible files.
    pub file_count: usize,
    /// Number of test/mock files hidden by the filter.
    pub hidden_test_count: usize,
    /// Total added lines across visible files.
    pub additions: u32,
    /// Total deleted lines across visible files.
    pub deletions: u32,
}

/// Hides test and mock files unless `include_path` explicitly selects one (`#26759`).
#[must_use]
pub fn filter_review_files(
    files: &[ReviewFileEntry],
    hide_test_files: bool,
    include_path: Option<&str>,
) -> FilteredReviewFiles {
    let kept: Vec<ReviewFileEntry> = if hide_test_files {
        files
            .iter()
            .filter(|file| {
                include_path == Some(file.path.as_str()) || !is_test_changed_file(&file.path)
            })
            .cloned()
            .collect()
    } else {
        files.to_vec()
    };
    let hidden_test_count = files.len().saturating_sub(kept.len());
    let additions = kept
        .iter()
        .fold(0_u32, |acc, f| acc.saturating_add(f.additions));
    let deletions = kept
        .iter()
        .fold(0_u32, |acc, f| acc.saturating_add(f.deletions));
    FilteredReviewFiles {
        file_count: kept.len(),
        files: kept,
        hidden_test_count,
        additions,
        deletions,
    }
}

/// Returns the next up-to-3 visible patch indices around `index` (or from the top),
/// skipping hidden test files (`shownPatchIndices`, #26759).
#[must_use]
pub fn shown_patch_indices(
    paths: &[&str],
    hide_test_files: bool,
    index: Option<usize>,
) -> Vec<usize> {
    let shown: Vec<usize> = paths
        .iter()
        .enumerate()
        .filter_map(|(pos, path)| {
            if hide_test_files && is_test_changed_file(path) {
                None
            } else {
                Some(pos)
            }
        })
        .collect();
    let Some(target) = index else {
        return shown.into_iter().take(3).collect();
    };
    match shown.iter().position(|&pos| pos == target) {
        Some(pos) => shown.into_iter().skip(pos).take(3).collect(),
        None => vec![target],
    }
}
