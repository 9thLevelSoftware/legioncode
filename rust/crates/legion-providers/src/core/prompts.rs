//! Structured prompt builders and output sanitizers for metadata generation.
//!
//! Mirrors `packages/provider-core/src/server/textGenerationPrompts.ts`,
//! `threadTitleContext.ts`, and `textGenerationUtils.ts`.

use legion_shared::utf16::{utf8_len_to_utf16, utf16_slice_safe};

use super::budget::{DEFAULT_TRUNCATION_MARKER, truncate_head_tail_utf16};

/// Maximum characters preserved per prompt section for commit/PR synthesis.
pub const TEXT_GEN_SECTION_MAX_CHARS: usize = 24_000;
/// Maximum characters retained from a single message during thread-title generation.
pub const TITLE_MESSAGE_MAX_CHARS: usize = 1_200;
/// Maximum sanitized thread title length in UTF-16 code units.
pub const MAX_THREAD_TITLE_CHARS: usize = 80;
/// Maximum sanitized commit subject length in UTF-16 code units.
pub const MAX_COMMIT_SUBJECT_CHARS: usize = 72;

/// Commit-subject instructions from `conventionalCommitsTextGenerationPolicy`.
pub const CONVENTIONAL_COMMITS_COMMIT_INSTRUCTIONS: &str = "Use Conventional Commits when generating commit subjects. Prefer the narrowest accurate type and include a scope only when it is obvious from the diff. Start the description after the colon with a lowercase word, never a proper noun or acronym, because commitlint rejects a capitalized description.";

/// Pull/merge-request title instructions from `conventionalCommitsTextGenerationPolicy` (`#40571`).
pub const CONVENTIONAL_COMMITS_CHANGE_REQUEST_INSTRUCTIONS: &str = "Keep the change request title concise. When the listed commits use Conventional Commits, write the title as a Conventional Commit subject too, for example 'feat(web): add a recorder', with the narrowest accurate type, a scope only when obvious, and a lowercase description. Otherwise do not force Conventional Commit syntax into the title.";

/// Role label for messages passed into [`TextGenerationPrompts::build_thread_title_prompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleContextRole {
    /// Initial or follow-up user turn.
    User,
    /// Assistant reply.
    Assistant,
}

/// Conversation message excerpt included in thread title prompt synthesis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TitleContextMessage {
    /// Speaker role.
    pub role: TitleContextRole,
    /// Message text content.
    pub text: String,
}

/// Inputs for [`TextGenerationPrompts::build_commit_message_prompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitMessagePromptInput {
    /// Active git branch name (`main`, `feat/...`, etc.).
    pub branch: Option<String>,
    /// Output from `git status --short`.
    pub staged_summary: String,
    /// Unified diff patch text (automatically truncated to [`TEXT_GEN_SECTION_MAX_CHARS`]).
    pub diff_patch: String,
}

/// Inputs for [`TextGenerationPrompts::build_pr_content_prompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrContentPromptInput {
    /// Target base branch name.
    pub base_branch: String,
    /// Source head branch name.
    pub head_branch: String,
    /// Commit log summary lines.
    pub commit_summary: String,
    /// Unified diff summary/patch text.
    pub diff_summary: String,
}

/// Inputs for [`TextGenerationPrompts::build_branch_name_prompt`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchNamePromptInput {
    /// Task or user message describing the change.
    pub message: String,
    /// Attachment names, if any.
    pub attachments: Vec<String>,
}

/// Paired system + user prompt generated for provider text-generation calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedTextPrompt {
    /// System / instruction framing prompt.
    pub system: String,
    /// User / task payload prompt.
    pub prompt: String,
}

/// Stateless prompt constructors and response sanitizers for commit messages,
/// PR titles/bodies, branch names, and thread titles.
///
/// # Examples
///
/// ```
/// use legion_providers::core::{
///     CommitMessagePromptInput, TextGenerationPrompts, sanitize_thread_title,
/// };
///
/// let built = TextGenerationPrompts::build_commit_message_prompt(&CommitMessagePromptInput {
///     branch: Some("feat/rust-providers".to_owned()),
///     staged_summary: "M rust/crates/legion-providers/src/lib.rs".to_owned(),
///     diff_patch: "+pub mod acp;".to_owned(),
/// });
/// assert!(built.prompt.contains("feat/rust-providers"));
/// assert_eq!(sanitize_thread_title("  \"Title: `Fix ACP framing`\"  "), "Fix ACP framing");
/// ```
#[derive(Debug, Clone, Copy, Default)]
pub struct TextGenerationPrompts;

impl TextGenerationPrompts {
    /// Builds a thread-title generation prompt from conversation excerpts and
    /// attachment filenames.
    #[must_use]
    pub fn build_thread_title_prompt(
        messages: &[TitleContextMessage],
        attachments: &[String],
    ) -> GeneratedTextPrompt {
        let mut sections = Vec::new();
        for msg in messages {
            let role_tag = match msg.role {
                TitleContextRole::User => "User",
                TitleContextRole::Assistant => "Assistant",
            };
            let bounded = limit_title_message(&msg.text, TITLE_MESSAGE_MAX_CHARS);
            sections.push(format!("{role_tag}:\n{bounded}"));
        }
        if !attachments.is_empty() {
            sections.push(format!("Attachments: {}", attachments.join(", ")));
        }

        let prompt = if sections.is_empty() {
            "Generate a concise thread title (max 6 words).".to_owned()
        } else {
            format!(
                "Summarize the conversation below as a concise thread title (3-6 words, plain text, no quotes):\n\n{}",
                sections.join("\n\n")
            )
        };

        GeneratedTextPrompt {
            system: "You write concise, specific software engineering thread titles. Return only the title.".to_owned(),
            prompt,
        }
    }

    /// Builds a git commit message prompt from branch metadata, status, and bounded diff.
    #[must_use]
    pub fn build_commit_message_prompt(input: &CommitMessagePromptInput) -> GeneratedTextPrompt {
        let branch_line = input.branch.as_deref().unwrap_or("(detached)");
        let bounded_status = limit_section(&input.staged_summary, TEXT_GEN_SECTION_MAX_CHARS);
        let bounded_diff = limit_section(&input.diff_patch, TEXT_GEN_SECTION_MAX_CHARS);
        let prompt = format!(
            "Branch: {branch_line}\n\nStaged summary:\n{bounded_status}\n\nDiff:\n{bounded_diff}"
        );
        GeneratedTextPrompt {
            system: "Write a conventional commit message with a short imperative subject (<= 72 chars) and concise body if helpful. Respond with JSON {\"subject\": string, \"body\": string}.".to_owned(),
            prompt,
        }
    }

    /// Builds a pull-request title and summary prompt from commits and bounded diff.
    #[must_use]
    pub fn build_pr_content_prompt(input: &PrContentPromptInput) -> GeneratedTextPrompt {
        Self::build_pr_content_prompt_with_instructions(input, None)
    }

    /// Builds a pull-request prompt appending custom preset instructions (e.g.
    /// [`CONVENTIONAL_COMMITS_CHANGE_REQUEST_INSTRUCTIONS`]) (`#40571`).
    #[must_use]
    pub fn build_pr_content_prompt_with_instructions(
        input: &PrContentPromptInput,
        change_request_instructions: Option<&str>,
    ) -> GeneratedTextPrompt {
        let bounded_commits = limit_section(&input.commit_summary, TEXT_GEN_SECTION_MAX_CHARS);
        let bounded_diff = limit_section(&input.diff_summary, TEXT_GEN_SECTION_MAX_CHARS);
        let prompt = format!(
            "Base branch: {}\nHead branch: {}\n\nCommits:\n{}\n\nDiff summary:\n{}",
            input.base_branch, input.head_branch, bounded_commits, bounded_diff
        );
        let mut system =
            "Draft a clear pull request title and Markdown body covering what changed and why."
                .to_owned();
        if let Some(extra) = change_request_instructions
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            system.push(' ');
            system.push_str(extra);
        }
        system.push_str(" Respond with JSON {\"title\": string, \"body\": string}.");
        GeneratedTextPrompt { system, prompt }
    }

    /// Builds a git branch name suggestion prompt.
    #[must_use]
    pub fn build_branch_name_prompt(input: &BranchNamePromptInput) -> GeneratedTextPrompt {
        let bounded_msg = limit_section(&input.message, TITLE_MESSAGE_MAX_CHARS);
        let mut prompt = format!("Task:\n{bounded_msg}");
        if !input.attachments.is_empty() {
            prompt.push_str("\n\nAttachments:\n");
            prompt.push_str(&input.attachments.join(", "));
        }
        GeneratedTextPrompt {
            system: "Generate a short kebab-case git branch name (lowercase ASCII, hyphens, optional feat/ or fix/ prefix). Respond with JSON {\"branch\": string}.".to_owned(),
            prompt,
        }
    }
}

/// Bounds a general prompt section using head-and-tail preservation around
/// [`DEFAULT_TRUNCATION_MARKER`] when exceeding `max_chars` UTF-16 code units.
#[must_use]
pub fn limit_section(text: &str, max_chars: usize) -> String {
    truncate_head_tail_utf16(text.trim(), max_chars, DEFAULT_TRUNCATION_MARKER)
}

/// Bounds a thread-title context message using head-and-tail preservation around
/// [`DEFAULT_TRUNCATION_MARKER`] so both task setup and final instructions stay intact.
#[must_use]
pub fn limit_title_message(text: &str, max_chars: usize) -> String {
    truncate_head_tail_utf16(text.trim(), max_chars, DEFAULT_TRUNCATION_MARKER)
}

/// Cleans generated thread titles by stripping quotes, markdown ticks, common
/// prefixes (`Title:`), whitespace runs, and excess characters.
#[must_use]
pub fn sanitize_thread_title(raw: &str) -> String {
    let stripped = strip_wrapping_delimiters(raw);
    let no_prefix = strip_wrapping_delimiters(strip_ascii_prefix(stripped, "title:"));
    collapse_and_cap(no_prefix, MAX_THREAD_TITLE_CHARS)
}

/// Cleans generated commit message subjects to a single bounded line.
#[must_use]
pub fn sanitize_commit_subject(raw: &str) -> String {
    let first_line = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let stripped = strip_wrapping_delimiters(first_line);
    collapse_and_cap(stripped, MAX_COMMIT_SUBJECT_CHARS)
}

/// Cleans generated pull request titles by removing markdown prefixes and outer delimiters.
#[must_use]
pub fn sanitize_pr_title(raw: &str) -> String {
    let first_line = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let stripped = strip_wrapping_delimiters(first_line);
    let no_prefix = strip_wrapping_delimiters(strip_ascii_prefix(stripped, "title:"));
    collapse_and_cap(no_prefix, 120)
}

fn strip_wrapping_delimiters(raw: &str) -> &str {
    let mut s = raw.trim();
    loop {
        let next = s
            .trim()
            .trim_start_matches(['#', '*', '-', '>', ' '])
            .trim_matches(['"', '\'', '`', '*']);
        if next == s {
            return next;
        }
        s = next;
    }
}

fn strip_ascii_prefix<'a>(raw: &'a str, prefix: &str) -> &'a str {
    let trimmed = raw.trim();
    if let Some(head) = trimmed.get(..prefix.len())
        && head.eq_ignore_ascii_case(prefix)
    {
        return trimmed.get(prefix.len()..).unwrap_or("").trim();
    }
    trimmed
}

fn collapse_and_cap(raw: &str, max_utf16_chars: usize) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if utf8_len_to_utf16(&collapsed) <= max_utf16_chars {
        collapsed
    } else {
        utf16_slice_safe(&collapsed, 0, max_utf16_chars)
            .trim_end()
            .to_owned()
    }
}
