//! OpenSSH `config` directive tokenization, UTF-8 pattern matching, and option resolution.

/// Parsed OpenSSH config line (`directive`, or `args: None` on invalid quoting).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedConfigLine {
    /// Lowercase directive keyword.
    pub directive: String,
    /// Parsed argument tokens, or `None` when quoting was unclosed.
    pub args: Option<Vec<String>>,
}

pub(super) type HostCondition = Option<Vec<String>>;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RuleDirective {
    Hostname,
    Port,
    Uncertain,
}

pub(super) struct HostRule {
    pub(super) conditions: Vec<HostCondition>,
    pub(super) directive: RuleDirective,
    pub(super) value: Option<String>,
}

/// Splits one OpenSSH config line matching OpenSSH `readconf` / `misc.c argv_split()`.
#[must_use]
pub fn parse_ssh_config_line(line: &str) -> Option<ParsedConfigLine> {
    let trimmed_end = line.trim_end_matches([' ', '\t', '\r', '\x0c']);
    let trimmed_start = trimmed_end.trim_start_matches([' ', '\t', '\r']);
    if trimmed_start.is_empty() {
        return None;
    }

    let key_end = trimmed_start
        .find([' ', '\t', '\r', '='])
        .unwrap_or(trimmed_start.len());
    let directive = trimmed_start[..key_end].to_ascii_lowercase();
    let mut rest = trimmed_start[key_end..].trim_start_matches([' ', '\t', '\r']);
    if let Some(after_eq) = rest.strip_prefix('=') {
        rest = after_eq.trim_start_matches([' ', '\t', '\r']);
    }

    let mut args = Vec::new();
    let mut token: Option<String> = None;
    let mut quote: Option<char> = None;
    let mut chars = rest.chars().peekable();
    while let Some(ch) = chars.next() {
        let next = chars.peek().copied();
        if (ch == '\\' && matches!(next, Some('"' | '\'' | '\\')))
            || (ch == '\\' && quote.is_none() && next == Some(' '))
        {
            if let Some(escaped) = chars.next() {
                token.get_or_insert_with(String::new).push(escaped);
            }
        } else if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                token.get_or_insert_with(String::new).push(ch);
            }
        } else if ch == '"' || ch == '\'' {
            quote = Some(ch);
            token.get_or_insert_with(String::new);
        } else if ch == ' ' || ch == '\t' {
            if let Some(finished) = token.take() {
                args.push(finished);
            }
        } else if ch == '#' && token.is_none() {
            break;
        } else {
            token.get_or_insert_with(String::new).push(ch);
        }
    }
    if quote.is_some() {
        return Some(ParsedConfigLine {
            directive,
            args: None,
        });
    }
    if let Some(finished) = token {
        args.push(finished);
    }
    Some(ParsedConfigLine {
        directive,
        args: Some(args),
    })
}

pub(super) fn has_ssh_pattern(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.starts_with('!')
}

pub(super) fn has_glob_wildcard(value: &str) -> bool {
    value.contains('*') || value.contains('?')
}

/// OpenSSH matches `Host` patterns and `Include` globs on raw UTF-8 bytes (`?` = 1 byte).
pub(super) fn glob_matches_bytes(pattern: &[u8], value: &[u8]) -> bool {
    let (mut p_idx, mut v_idx) = (0usize, 0usize);
    let (mut star_p, mut star_v) = (None, 0usize);
    while v_idx < value.len() {
        if p_idx < pattern.len() && (pattern[p_idx] == b'?' || pattern[p_idx] == value[v_idx]) {
            p_idx += 1;
            v_idx += 1;
        } else if p_idx < pattern.len() && pattern[p_idx] == b'*' {
            star_p = Some(p_idx);
            p_idx += 1;
            star_v = v_idx;
        } else if let Some(sp) = star_p {
            p_idx = sp + 1;
            star_v += 1;
            v_idx = star_v;
        } else {
            return false;
        }
    }
    while p_idx < pattern.len() && pattern[p_idx] == b'*' {
        p_idx += 1;
    }
    p_idx == pattern.len()
}

fn matches_host_patterns(alias: &str, patterns: &[String]) -> bool {
    let mut matched = false;
    for pattern in patterns {
        let (negated, host_pattern) = match pattern.strip_prefix('!') {
            Some(rest) => (true, rest),
            None => (false, pattern.as_str()),
        };
        let is_match = host_pattern == "*"
            || if has_ssh_pattern(host_pattern) {
                glob_matches_bytes(host_pattern.as_bytes(), alias.as_bytes())
            } else {
                host_pattern == alias
            };
        if !is_match {
            continue;
        }
        if negated {
            return false;
        }
        matched = true;
    }
    matched
}

enum ResolvedValue<'a> {
    Unmatched,
    Uncertain,
    Value(&'a str),
}

fn resolve_configured_value<'a>(
    alias: &str,
    directive: RuleDirective,
    rules: &'a [HostRule],
) -> ResolvedValue<'a> {
    for rule in rules {
        if (rule.directive != RuleDirective::Uncertain && rule.directive != directive)
            || rule.conditions.iter().any(|cond| {
                cond.as_ref()
                    .is_some_and(|pats| !matches_host_patterns(alias, pats))
            })
        {
            continue;
        }
        if rule.conditions.iter().any(Option::is_none) {
            return ResolvedValue::Uncertain;
        }
        return match rule.value.as_deref() {
            None => ResolvedValue::Uncertain,
            Some(val) => ResolvedValue::Value(val),
        };
    }
    ResolvedValue::Unmatched
}

pub(super) fn resolve_configured_hostname(alias: &str, rules: &[HostRule]) -> Option<String> {
    let ResolvedValue::Value(raw) = resolve_configured_value(alias, RuleDirective::Hostname, rules)
    else {
        return None;
    };
    if raw.contains('$') {
        return None;
    }
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '%' && chars.peek() != Some(&'h') {
            return None;
        }
    }
    Some(raw.replace("%h", alias))
}

pub(super) fn resolve_configured_port(alias: &str, rules: &[HostRule]) -> String {
    match resolve_configured_value(alias, RuleDirective::Port, rules) {
        ResolvedValue::Unmatched => "22".to_owned(),
        ResolvedValue::Uncertain => "*".to_owned(),
        ResolvedValue::Value(port) => port.to_owned(),
    }
}
