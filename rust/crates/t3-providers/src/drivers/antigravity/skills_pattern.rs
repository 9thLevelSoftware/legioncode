//! Linear-time NFA pattern parser and matcher for untrusted `skills.json` filters.

#[derive(Debug, Clone)]
enum CharMatcher {
    Any,
    Literal(char),
    Digit {
        negated: bool,
    },
    Word {
        negated: bool,
    },
    Whitespace {
        negated: bool,
    },
    Class {
        negated: bool,
        singles: Vec<char>,
        ranges: Vec<(char, char)>,
    },
}

impl CharMatcher {
    fn matches(&self, ch: char) -> bool {
        match self {
            Self::Any => true,
            Self::Literal(expected) => ch == *expected,
            Self::Digit { negated } => ch.is_ascii_digit() ^ *negated,
            Self::Word { negated } => (ch.is_ascii_alphanumeric() || ch == '_') ^ *negated,
            Self::Whitespace { negated } => ch.is_whitespace() ^ *negated,
            Self::Class {
                negated,
                singles,
                ranges,
            } => {
                let hit =
                    singles.contains(&ch) || ranges.iter().any(|(lo, hi)| ch >= *lo && ch <= *hi);
                hit ^ *negated
            }
        }
    }
}

#[derive(Debug, Clone)]
struct PatternToken {
    matcher: CharMatcher,
    optional: bool,
    repeats: bool,
}

/// Linear-time filter matcher compiled from a `skills.json` `include_only` / `exclude` entry.
#[derive(Debug, Clone)]
pub struct SkillPatternMatcher {
    literal: Option<String>,
    tokens: Vec<PatternToken>,
}

impl SkillPatternMatcher {
    /// Compiles `pattern` into a linear-time NFA; syntax outside literals, escapes,
    /// `.`, `[...]`, `*`, `+`, and `?` falls back to exact literal comparison (`1c379b63ba`).
    #[must_use]
    pub fn compile(pattern: &str) -> Self {
        match parse_pattern_tokens(pattern) {
            Some(tokens) => Self {
                literal: None,
                tokens,
            },
            None => Self {
                literal: Some(pattern.to_owned()),
                tokens: Vec::new(),
            },
        }
    }

    /// Returns `true` when `entry` matches this pattern in linear time.
    #[must_use]
    pub fn matches(&self, entry: &str) -> bool {
        if let Some(exact) = &self.literal {
            return entry == exact;
        }
        let n = self.tokens.len();
        let mut states = vec![false; n + 1];
        states[0] = true;
        close_epsilon(&self.tokens, &mut states);
        for ch in entry.chars() {
            let mut next = vec![false; n + 1];
            let mut any = false;
            for (idx, active) in states.iter().copied().enumerate() {
                if !active {
                    continue;
                }
                if let Some(token) = self.tokens.get(idx)
                    && token.matcher.matches(ch)
                {
                    next[idx + 1] = true;
                    any = true;
                    if token.repeats {
                        next[idx] = true;
                    }
                }
            }
            if !any {
                return false;
            }
            close_epsilon(&self.tokens, &mut next);
            states = next;
        }
        states[n]
    }
}

fn close_epsilon(tokens: &[PatternToken], states: &mut [bool]) {
    for idx in 0..tokens.len() {
        if states[idx] && tokens[idx].optional {
            states[idx + 1] = true;
        }
    }
}

fn parse_pattern_tokens(pattern: &str) -> Option<Vec<PatternToken>> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let matcher = match ch {
            '\\' => {
                let escaped = *chars.get(i + 1)?;
                i += 2;
                escape_matcher(escaped)
            }
            '[' => {
                let (m, next_i) = parse_char_class(&chars, i + 1)?;
                i = next_i;
                m
            }
            '.' => {
                i += 1;
                CharMatcher::Any
            }
            '(' | ')' | '|' | '{' | '}' | '^' | '$' | '*' | '+' | '?' => return None,
            _ => {
                i += 1;
                CharMatcher::Literal(ch)
            }
        };
        let quantifier = chars.get(i).copied();
        let optional = matches!(quantifier, Some('*' | '?'));
        let repeats = matches!(quantifier, Some('*' | '+'));
        if optional || repeats {
            i += 1;
            if chars.get(i) == Some(&'?') {
                i += 1;
            }
        }
        tokens.push(PatternToken {
            matcher,
            optional,
            repeats,
        });
    }
    Some(tokens)
}

const fn escape_matcher(ch: char) -> CharMatcher {
    match ch {
        'd' => CharMatcher::Digit { negated: false },
        'D' => CharMatcher::Digit { negated: true },
        'w' => CharMatcher::Word { negated: false },
        'W' => CharMatcher::Word { negated: true },
        's' => CharMatcher::Whitespace { negated: false },
        'S' => CharMatcher::Whitespace { negated: true },
        other => CharMatcher::Literal(other),
    }
}

fn parse_char_class(chars: &[char], start: usize) -> Option<(CharMatcher, usize)> {
    let mut i = start;
    let negated = chars.get(i) == Some(&'^');
    if negated {
        i += 1;
    }
    let mut items = Vec::new();
    while i < chars.len() {
        if chars[i] == ']' && !items.is_empty() {
            return build_class_matcher(negated, &items).map(|m| (m, i + 1));
        }
        if chars[i] == '\\' {
            let esc = *chars.get(i + 1)?;
            items.push(ClassAtom::Escaped(esc));
            i += 2;
        } else {
            items.push(ClassAtom::Char(chars[i]));
            i += 1;
        }
    }
    None
}

#[derive(Clone, Copy)]
enum ClassAtom {
    Char(char),
    Escaped(char),
}

impl ClassAtom {
    const fn as_char(self) -> char {
        match self {
            Self::Char(c) | Self::Escaped(c) => c,
        }
    }
}

fn build_class_matcher(negated: bool, items: &[ClassAtom]) -> Option<CharMatcher> {
    let mut singles = Vec::new();
    let mut ranges = Vec::new();
    let mut idx = 0;
    while idx < items.len() {
        if idx + 2 < items.len() && matches!(items[idx + 1], ClassAtom::Char('-')) {
            let lo = items[idx].as_char();
            let hi = items[idx + 2].as_char();
            if lo > hi {
                return None;
            }
            ranges.push((lo, hi));
            idx += 3;
        } else {
            match items[idx] {
                ClassAtom::Escaped('d') => ranges.push(('0', '9')),
                ClassAtom::Escaped('w') => {
                    ranges.extend([('0', '9'), ('A', 'Z'), ('a', 'z')]);
                    singles.push('_');
                }
                ClassAtom::Escaped('s') => singles.extend([' ', '\t', '\n', '\r']),
                atom => singles.push(atom.as_char()),
            }
            idx += 1;
        }
    }
    Some(CharMatcher::Class {
        negated,
        singles,
        ranges,
    })
}
