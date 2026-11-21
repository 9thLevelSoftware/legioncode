//! Semantic version parsing, range matching, and bundled compatibility manifest JSON.

use std::fmt;

/// Parsed `major.minor.patch` semantic version triple.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Semver {
    /// Major version component.
    pub major: u64,
    /// Minor version component.
    pub minor: u64,
    /// Patch version component.
    pub patch: u64,
}

impl fmt::Display for Semver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl Semver {
    /// Parses a strict `X.Y.Z` triple with non-empty ASCII digits and no prerelease/build tags.
    #[must_use]
    pub fn parse_strict(input: &str) -> Option<Self> {
        let mut parts = input.trim().split('.');
        let major = parse_digits(parts.next()?)?;
        let minor = parse_digits(parts.next()?)?;
        let patch = parse_digits(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
        })
    }
}

pub(crate) fn parse_digits(segment: &str) -> Option<u64> {
    if segment.is_empty() || !segment.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    segment.parse::<u64>().ok()
}

pub(crate) fn parse_partial_version(raw: &str) -> Option<(u64, Option<u64>, Option<u64>)> {
    let trimmed = raw.trim().trim_start_matches('v');
    let mut parts = trimmed.split('.');
    let major = parse_digits(parts.next()?)?;
    let minor = match parts.next() {
        Some(p) if p != "*" && !p.eq_ignore_ascii_case("x") => Some(parse_digits(p)?),
        _ => None,
    };
    let patch = match parts.next() {
        Some(p) if p != "*" && !p.eq_ignore_ascii_case("x") => Some(parse_digits(p)?),
        _ => None,
    };
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}

fn eval_caret_comparator(version: Semver, rest: &str) -> bool {
    parse_partial_version(rest).is_some_and(|(ma, mi, pa)| {
        let lower = Semver {
            major: ma,
            minor: mi.unwrap_or(0),
            patch: pa.unwrap_or(0),
        };
        let upper = if ma > 0 || mi.is_none() {
            Semver {
                major: ma.saturating_add(1),
                minor: 0,
                patch: 0,
            }
        } else if mi.unwrap_or(0) > 0 || pa.is_none() {
            Semver {
                major: 0,
                minor: mi.unwrap_or(0).saturating_add(1),
                patch: 0,
            }
        } else {
            Semver {
                major: 0,
                minor: 0,
                patch: pa.unwrap_or(0).saturating_add(1),
            }
        };
        version >= lower && version < upper
    })
}

fn eval_tilde_comparator(version: Semver, rest: &str) -> bool {
    parse_partial_version(rest).is_some_and(|(ma, mi, pa)| {
        let lower = Semver {
            major: ma,
            minor: mi.unwrap_or(0),
            patch: pa.unwrap_or(0),
        };
        let upper = match mi {
            Some(m) => Semver {
                major: ma,
                minor: m.saturating_add(1),
                patch: 0,
            },
            None => Semver {
                major: ma.saturating_add(1),
                minor: 0,
                patch: 0,
            },
        };
        version >= lower && version < upper
    })
}

fn eval_single_comparator(version: Semver, token: &str) -> bool {
    let token = token.trim();
    if token.is_empty() || token == "*" || token.eq_ignore_ascii_case("x") {
        return true;
    }
    if let Some(rest) = token.strip_prefix(">=") {
        return parse_partial_version(rest).is_some_and(|(ma, mi, pa)| {
            version
                >= Semver {
                    major: ma,
                    minor: mi.unwrap_or(0),
                    patch: pa.unwrap_or(0),
                }
        });
    }
    if let Some(rest) = token.strip_prefix("<=") {
        return parse_partial_version(rest).is_some_and(|(ma, mi, pa)| match (mi, pa) {
            (Some(mi), Some(pa)) => {
                version
                    <= Semver {
                        major: ma,
                        minor: mi,
                        patch: pa,
                    }
            }
            (Some(mi), None) => (version.major, version.minor) <= (ma, mi),
            (None, _) => version.major <= ma,
        });
    }
    if let Some(rest) = token.strip_prefix('>') {
        return parse_partial_version(rest).is_some_and(|(ma, mi, pa)| match (mi, pa) {
            (Some(mi), Some(pa)) => {
                version
                    > Semver {
                        major: ma,
                        minor: mi,
                        patch: pa,
                    }
            }
            (Some(mi), None) => (version.major, version.minor) > (ma, mi),
            (None, _) => version.major > ma,
        });
    }
    if let Some(rest) = token.strip_prefix('<') {
        return parse_partial_version(rest).is_some_and(|(ma, mi, pa)| {
            version
                < Semver {
                    major: ma,
                    minor: mi.unwrap_or(0),
                    patch: pa.unwrap_or(0),
                }
        });
    }
    if let Some(rest) = token.strip_prefix('^') {
        return eval_caret_comparator(version, rest);
    }
    if let Some(rest) = token.strip_prefix('~') {
        return eval_tilde_comparator(version, rest);
    }
    let eq_raw = token.strip_prefix('=').unwrap_or(token);
    parse_partial_version(eq_raw).is_some_and(|(ma, mi, pa)| match (mi, pa) {
        (Some(mi), Some(pa)) => {
            version
                == Semver {
                    major: ma,
                    minor: mi,
                    patch: pa,
                }
        }
        (Some(mi), None) => version.major == ma && version.minor == mi,
        (None, _) => version.major == ma,
    })
}

/// Evaluates whether a strict `X.Y.Z` version satisfies a semver range expression.
#[must_use]
pub fn satisfies_semver_range(version: &str, range: &str) -> bool {
    let Some(parsed) = Semver::parse_strict(version.trim_start_matches('v')) else {
        return false;
    };
    range.split("||").any(|clause| {
        let mut tokens = clause.split_whitespace().peekable();
        if tokens.peek().is_none() {
            return false;
        }
        tokens.all(|token| eval_single_comparator(parsed, token))
    })
}

pub(crate) const BUNDLED_COMPATIBILITY_JSON: &str = r#"{
  "compatibility": [
    {
      "driver": "codex",
      "t3CodeRange": ">=0.0.44",
      "recommendedRange": ">=0.159.0",
      "ranges": [
        { "range": ">=0.156.0", "status": "supported" },
        { "range": ">=0.149.0 <0.156.0", "status": "unsupported" },
        { "range": "<0.149.0", "status": "broken" }
      ]
    },
    {
      "driver": "claudeAgent",
      "t3CodeRange": ">=0.0.42",
      "recommendedRange": ">=2.1.301",
      "ranges": [
        { "range": ">=2.1.280", "status": "supported" },
        { "range": ">=2.1.111 <2.1.280", "status": "graceful" },
        { "range": "<2.1.111", "status": "unsupported" }
      ]
    },
    {
      "driver": "cursor",
      "t3CodeRange": ">=0.0.42",
      "versionPattern": "^(\\d{4}\\.\\d{2}\\.\\d{2})-[a-f0-9]+$",
      "recommendedRange": ">=2026.05.09",
      "ranges": [
        { "range": ">=2026.05.09", "status": "supported" },
        { "range": "<2026.05.09", "status": "unknown" }
      ]
    },
    {
      "driver": "grok",
      "t3CodeRange": ">=0.0.42",
      "recommendedRange": ">=1.0.13",
      "ranges": [
        { "range": ">=1.0.13", "status": "supported" },
        { "range": "<1.0.13", "status": "broken" }
      ]
    },
    {
      "driver": "opencode",
      "t3CodeRange": ">=0.0.42 <0.0.46",
      "recommendedRange": ">=1.14.19 <2.0.0",
      "recommendedVersion": "1.14.19",
      "ranges": [
        { "range": ">=2.0.0", "status": "broken" },
        { "range": ">=1.14.19 <2.0.0", "status": "supported" },
        { "range": "<1.14.19", "status": "broken" }
      ]
    },
    {
      "driver": "opencode",
      "t3CodeRange": ">=0.0.46",
      "versionPattern": "^(\\d+\\.\\d+\\.\\d+)(?:-[0-9A-Za-z.-]+)?(?:\\+[0-9A-Za-z.-]+)?$",
      "recommendedRange": ">=2.0.18",
      "ranges": [
        { "range": ">=2.0.18", "status": "supported" },
        { "range": ">=2.0.0 <2.0.18", "status": "unsupported" },
        { "range": ">=1.14.19 <2.0.0", "status": "graceful" },
        { "range": "<1.14.19", "status": "broken" }
      ]
    },
    {
      "driver": "antigravity",
      "t3CodeRange": ">=0.0.42 <0.1.10",
      "versionPattern": "^(?:agy_acp_server_)?(\\d+\\.\\d+\\.\\d+)(?:-[0-9A-Za-z.-]+)?$",
      "recommendedRange": "=1.4.5 || =1.4.1",
      "ranges": [
        { "range": "=1.4.5", "status": "supported" },
        { "range": "=1.4.1", "status": "supported" },
        { "range": "=1.3.0", "status": "supported" },
        { "range": "=1.1.1", "status": "supported" },
        { "range": "<1.5.0", "status": "broken" }
      ]
    },
    {
      "driver": "antigravity",
      "t3CodeRange": ">=0.1.10",
      "versionPattern": "^(?:agy_acp_server_)?(\\d+\\.\\d+\\.\\d+)(?:-[0-9A-Za-z.-]+)?$",
      "recommendedRange": ">=1.5.12 <2",
      "ranges": [
        { "range": ">=1.5.0 <2", "status": "supported" },
        { "range": "<1.5.0", "status": "broken" }
      ]
    },
    {
      "driver": "kiro",
      "t3CodeRange": ">=0.0.46",
      "recommendedRange": ">=2.27.0",
      "ranges": [{ "range": ">=2.27.0", "status": "supported" }]
    },
    {
      "driver": "pi",
      "t3CodeRange": ">=0.0.46",
      "recommendedRange": ">=1.0.0",
      "ranges": [
        { "range": ">=0.84.3", "status": "supported" },
        { "range": "<0.84.3", "status": "broken" }
      ]
    },
    {
      "driver": "muse",
      "t3CodeRange": ">=0.0.46",
      "versionPattern": "^(\\d+\\.\\d+\\.\\d+)-R\\d+(?:\\.\\d+)?$",
      "recommendedRange": ">=1.4.2",
      "ranges": [
        { "range": ">=1.4.2", "status": "supported" },
        { "range": "<1.4.2", "status": "unknown" }
      ]
    },
    {
      "driver": "fx",
      "t3CodeRange": ">=0.0.46",
      "recommendedRange": ">=0.0.13",
      "ranges": [
        { "range": ">=0.0.13", "status": "supported" },
        { "range": "<0.0.13", "status": "unsupported" }
      ]
    }
  ]
}"#;
