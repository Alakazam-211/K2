//! POSIX patterns (GNU BRE and ERE) on linear-time Rust regex engines.
//!
//! **Translation.** GNU basic and extended syntax is translated to `regex-automata` syntax:
//! `\(` `\)` `\{m,n\}` `\|` `\+` `\?` in BRE; a `*` at the start of an expression is literal;
//! `^` and `$` are anchors only where POSIX says so in BRE; bracket expressions take their
//! contents literally (a backslash is a backslash), with `[:class:]`, `[=c=]` and `[.c.]`;
//! the GNU escapes `\<` `\>` `\b` `\B` `\w` `\W` `\s` `\S` `` \` `` `\'`; in sed patterns
//! also `\n` `\t` `\r` `\f` `\v` `\a`, inside brackets too (GNU sed converts them first;
//! GNU grep doesn't, so in grep `[\t]` is a backslash or a `t`). Unicode classes:
//! `[:alpha:]`, `[:alnum:]`, `[:upper:]`, `[:lower:]` and `\w` are Unicode (as in a UTF-8
//! locale); the other bracket classes are ASCII.
//!
//! **Refused** (never approximated): backreferences `\1`..`\9` in a pattern (exit 2), and
//! escapes GNU doesn't define (`\d`: use `[0-9]`), which GNU would take as a literal letter.
//!
//! **Matching is POSIX leftmost-longest**, as GNU's: the leftmost start comes from a normal
//! search, then the longest match from that start from an anchored search with
//! `MatchKind::All`. Submatches (`\1` in a sed replacement) are the ones of that longest
//! match; when several splits of it exist, the engine's priority order picks one (POSIX
//! would pick the leftmost-longest split: a rare difference, e.g. `\(a\|ab\)\(c\|bcd\)`).
//!
//! **Bounds.** A pattern is at most [`MAX_PATTERN_BYTES`]; the compiled program is
//! size-limited, nesting is limited, and every search is linear in the haystack. The
//! shorter-match loop for `grep -w` counts its steps against a budget.

use regex_automata::meta::{self, Regex};
use regex_automata::util::captures::Captures;
use regex_automata::util::syntax;
use regex_automata::{Anchored, Input, MatchKind};

use super::error::ToolError;

/// Longest accepted pattern (all `-e` patterns together), in bytes.
pub const MAX_PATTERN_BYTES: usize = 64 * 1024;
const NFA_SIZE_LIMIT: usize = 4 << 20;
const CACHE_CAPACITY: usize = 2 << 20;
const NEST_LIMIT: u32 = 100;
/// Steps of the `grep -w` shorter-match loop per call.
const WORD_STEPS: u64 = 1 << 20;

/// Which syntax a pattern is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Dialect {
    Basic,
    Extended,
    Fixed,
}

/// Where a pattern comes from: `sed` also takes `\n` and `\t` escapes, and its `.` matches
/// a newline in the pattern space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Flavor {
    Grep,
    Sed,
}

fn is_meta(c: char) -> bool {
    matches!(
        c,
        '\\' | '.'
            | '+'
            | '*'
            | '?'
            | '('
            | ')'
            | '|'
            | '['
            | ']'
            | '{'
            | '}'
            | '^'
            | '$'
            | '#'
            | '&'
            | '-'
            | '~'
    )
}

fn lit(out: &mut String, c: char) {
    if is_meta(c) {
        out.push('\\');
    }
    out.push(c);
}

/// A literal string as a regex.
pub(crate) fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        lit(&mut out, c);
    }
    out
}

fn usage(m: &str) -> ToolError {
    ToolError::usage(m.to_owned())
}

/// Translate one GNU pattern to regex-automata syntax.
pub(crate) fn translate(pat: &str, dialect: Dialect, flavor: Flavor) -> Result<String, ToolError> {
    if pat.len() > MAX_PATTERN_BYTES {
        return Err(ToolError::Limit(format!(
            "pattern is {} bytes; at most {MAX_PATTERN_BYTES}",
            pat.len()
        )));
    }
    let ere = match dialect {
        Dialect::Fixed => return Ok(escape(pat)),
        Dialect::Basic => false,
        Dialect::Extended => true,
    };
    let cs: Vec<char> = pat.chars().collect();
    let mut out = String::with_capacity(pat.len() + 8);
    let mut i = 0;
    // At the start of an expression (pattern start, after a group open or an alternation,
    // after a leading `^`): `*` is literal there, and in BRE `^` is an anchor.
    let mut start = true;
    let mut depth = 0usize;
    while i < cs.len() {
        let c = cs[i];
        let was_start = start;
        start = false;
        match c {
            '\\' => {
                i += 1;
                let Some(&e) = cs.get(i) else {
                    return Err(usage("trailing backslash (\\)"));
                };
                match e {
                    '(' if !ere => {
                        out.push('(');
                        depth += 1;
                        start = true;
                    }
                    ')' if !ere => {
                        if depth == 0 {
                            return Err(usage("Unmatched ) or \\)"));
                        }
                        depth -= 1;
                        out.push(')');
                    }
                    '|' if !ere => {
                        out.push('|');
                        start = true;
                    }
                    '{' if !ere => {
                        if was_start {
                            return Err(usage("Invalid preceding regular expression"));
                        }
                        i = interval(&cs, i + 1, false, &mut out)?
                            .ok_or_else(|| usage("Invalid content of \\{\\}"))?;
                        continue;
                    }
                    '+' | '?' if !ere => {
                        if was_start {
                            lit(&mut out, e);
                        } else {
                            out.push(e);
                        }
                    }
                    '1'..='9' => {
                        return Err(ToolError::refused(
                            format!("\\{e}"),
                            "backreferences in a pattern aren't supported; \\1..\\9 work in a sed replacement",
                        ))
                    }
                    '<' => out.push_str(r"\b{start}"),
                    '>' => out.push_str(r"\b{end}"),
                    'b' | 'B' | 'w' | 'W' | 's' | 'S' => {
                        out.push('\\');
                        out.push(e);
                    }
                    '`' => out.push_str(r"\A"),
                    '\'' => out.push_str(r"\z"),
                    'n' | 't' | 'r' | 'f' | 'v' | 'a' if flavor == Flavor::Sed => {
                        out.push('\\');
                        out.push(e);
                    }
                    e if e.is_ascii_alphanumeric() => {
                        return Err(ToolError::unsupported(
                            format!("\\{e}"),
                            match e {
                                'd' => "not a GNU regex escape; use [0-9]",
                                'D' => "not a GNU regex escape; use [^0-9]",
                                _ => "not a GNU regex escape",
                            },
                        ))
                    }
                    e => lit(&mut out, e),
                }
            }
            '[' => {
                i = bracket(&cs, i, flavor, &mut out)?;
                continue;
            }
            '.' => out.push('.'),
            '*' => {
                if was_start {
                    out.push_str(r"\*");
                } else {
                    out.push('*');
                }
            }
            '^' => {
                if ere || was_start {
                    out.push('^');
                    start = true;
                } else {
                    out.push_str(r"\^");
                }
            }
            '$' => {
                let at_end = i + 1 == cs.len()
                    || (cs[i + 1] == '\\' && matches!(cs.get(i + 2), Some(')') | Some('|')));
                if ere || at_end {
                    out.push('$');
                } else {
                    out.push_str(r"\$");
                }
            }
            '(' if ere => {
                out.push('(');
                depth += 1;
                start = true;
            }
            ')' if ere => {
                if depth == 0 {
                    return Err(usage("Unmatched ) or \\)"));
                }
                depth -= 1;
                out.push(')');
            }
            '|' if ere => {
                out.push('|');
                start = true;
            }
            '+' | '?' if ere => {
                if was_start {
                    lit(&mut out, c);
                } else {
                    out.push(c);
                }
            }
            '{' if ere && !was_start => match interval(&cs, i + 1, true, &mut out)? {
                Some(next) => {
                    i = next;
                    continue;
                }
                None => out.push_str(r"\{"),
            },
            c => lit(&mut out, c),
        }
        i += 1;
    }
    if depth > 0 {
        return Err(usage("Unmatched ( or \\("));
    }
    Ok(out)
}

/// An interval body starting at `i` (just after `{` or `\{`): `m`, `m,`, `m,n` or `,n`,
/// closed by `}` (ERE) or `\}` (BRE). Returns the index after the close, or `None` when it
/// isn't an interval.
fn interval(
    cs: &[char],
    mut i: usize,
    ere: bool,
    out: &mut String,
) -> Result<Option<usize>, ToolError> {
    let num = |i: &mut usize| -> Option<u32> {
        let s = *i;
        while *i < cs.len() && cs[*i].is_ascii_digit() {
            *i += 1;
        }
        (s < *i).then(|| {
            cs[s..*i].iter().fold(0u32, |n, d| {
                n.saturating_mul(10).saturating_add(*d as u32 - '0' as u32)
            })
        })
    };
    let lo = num(&mut i);
    let mut hi = lo;
    let mut open = false;
    if cs.get(i) == Some(&',') {
        i += 1;
        hi = num(&mut i);
        open = hi.is_none();
    }
    let closed = if ere {
        if cs.get(i) == Some(&'}') {
            i += 1;
            true
        } else {
            false
        }
    } else if cs.get(i) == Some(&'\\') && cs.get(i + 1) == Some(&'}') {
        i += 2;
        true
    } else {
        false
    };
    if !closed || (lo.is_none() && hi.is_none()) {
        return Ok(None);
    }
    let lo = lo.unwrap_or(0);
    if let Some(h) = hi {
        if h < lo {
            return Err(usage("Invalid range end in \\{\\}"));
        }
    }
    if lo > 1000 || hi.is_some_and(|h| h > 1000) {
        return Err(ToolError::Limit("repetition count over 1000".into()));
    }
    match (open, hi) {
        (true, _) => out.push_str(&format!("{{{lo},}}")),
        (false, Some(h)) if h == lo => out.push_str(&format!("{{{lo}}}")),
        (false, Some(h)) => out.push_str(&format!("{{{lo},{h}}}")),
        (false, None) => unreachable!("closed interval without bounds"),
    }
    Ok(Some(i))
}

fn class_lit(out: &mut String, c: char) {
    if matches!(c, '\\' | '[' | ']' | '^' | '-' | '&' | '~') {
        out.push('\\');
    }
    out.push(c);
}

/// A bracket expression starting at `cs[i] == '['`. Returns the index after its `]`.
fn bracket(
    cs: &[char],
    mut i: usize,
    flavor: Flavor,
    out: &mut String,
) -> Result<usize, ToolError> {
    let unterminated = || usage("Unmatched [, [^, [:, [., or [=");
    i += 1;
    out.push('[');
    if cs.get(i) == Some(&'^') {
        out.push('^');
        i += 1;
    }
    let mut first = true;
    loop {
        let Some(&c) = cs.get(i) else {
            return Err(unterminated());
        };
        if c == ']' && !first {
            out.push(']');
            return Ok(i + 1);
        }
        first = false;
        // GNU sed (not grep) takes \n, \t... inside a bracket expression too
        if flavor == Flavor::Sed
            && c == '\\'
            && matches!(cs.get(i + 1), Some('n' | 't' | 'r' | 'f' | 'v' | 'a'))
        {
            out.push('\\');
            out.push(cs[i + 1]);
            i += 2;
            continue;
        }
        if c == '[' && matches!(cs.get(i + 1), Some(':') | Some('=') | Some('.')) {
            let kind = cs[i + 1];
            let body_start = i + 2;
            let mut j = body_start;
            while j + 1 < cs.len() && !(cs[j] == kind && cs[j + 1] == ']') {
                j += 1;
            }
            if j + 1 >= cs.len() {
                return Err(unterminated());
            }
            let body: String = cs[body_start..j].iter().collect();
            i = j + 2;
            match kind {
                ':' => out.push_str(match body.as_str() {
                    "alpha" => r"\p{Alphabetic}",
                    "upper" => r"\p{Uppercase}",
                    "lower" => r"\p{Lowercase}",
                    "alnum" => r"\p{Alphabetic}0-9",
                    "digit" => "0-9",
                    "xdigit" => "0-9A-Fa-f",
                    "space" => r"\s",
                    "blank" => r" \t",
                    "punct" => "[:punct:]",
                    "cntrl" => "[:cntrl:]",
                    "print" => "[:print:]",
                    "graph" => "[:graph:]",
                    _ => return Err(usage("Invalid character class name")),
                }),
                _ => {
                    let mut it = body.chars();
                    match (it.next(), it.next()) {
                        (Some(ch), None) => class_lit(out, ch),
                        _ => {
                            return Err(ToolError::unsupported(
                                format!("[{kind}{body}{kind}]"),
                                "multi-character collating elements",
                            ))
                        }
                    }
                }
            }
            continue;
        }
        // a range `c-d` (a `-` just before the closing `]` is literal)
        if cs.get(i + 1) == Some(&'-') && cs.get(i + 2).is_some_and(|&d| d != ']') {
            let d = cs[i + 2];
            if d == '[' {
                return Err(ToolError::unsupported(
                    "[a-[.x.]]",
                    "collating symbols as range ends",
                ));
            }
            if d < c {
                return Err(usage("Invalid range end"));
            }
            class_lit(out, c);
            out.push('-');
            class_lit(out, d);
            i += 3;
            continue;
        }
        class_lit(out, c);
        i += 1;
    }
}

/// A compiled pattern: leftmost-first for finding where the leftmost match starts, and
/// all-matches for the longest match from there.
#[derive(Debug, Clone)]
pub(crate) struct Matcher {
    first: Regex,
    all: Regex,
}

fn build(p: &str, icase: bool, dot_nl: bool, kind: MatchKind) -> Result<Regex, ToolError> {
    Regex::builder()
        .syntax(
            syntax::Config::new()
                .case_insensitive(icase)
                .dot_matches_new_line(dot_nl)
                .nest_limit(NEST_LIMIT),
        )
        .configure(
            Regex::config()
                .match_kind(kind)
                .nfa_size_limit(Some(NFA_SIZE_LIMIT))
                .hybrid_cache_capacity(CACHE_CAPACITY),
        )
        .build(p)
        .map_err(|e| {
            if e.size_limit().is_some() {
                ToolError::Limit("pattern too large once compiled".into())
            } else {
                ToolError::Usage(format!("invalid pattern: {e}"))
            }
        })
}

impl Matcher {
    /// Compile a translated pattern (see [`translate`]).
    pub(crate) fn new(p: &str, icase: bool, dot_nl: bool) -> Result<Self, ToolError> {
        Ok(Matcher {
            first: build(p, icase, dot_nl, MatchKind::LeftmostFirst)?,
            all: build(p, icase, dot_nl, MatchKind::All)?,
        })
    }

    /// Number of capture groups, the whole match included.
    pub(crate) fn groups(&self) -> usize {
        self.first.captures_len()
    }

    pub(crate) fn search(&self) -> Search<'_> {
        Search {
            m: self,
            c1: self.first.create_cache(),
            c2: self.all.create_cache(),
            caps: self.all.create_captures(),
            steps: 0,
        }
    }
}

/// Search state for one call (caches and captures).
pub(crate) struct Search<'m> {
    m: &'m Matcher,
    c1: meta::Cache,
    c2: meta::Cache,
    caps: Captures,
    steps: u64,
}

pub(crate) type Span = std::ops::Range<usize>;

fn is_word(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

impl Search<'_> {
    /// The leftmost-longest match starting at or after `from` (`^` still means the start of
    /// `hay`). Its groups are then available from [`Search::group`].
    pub(crate) fn find(&mut self, hay: &str, from: usize) -> Option<Span> {
        if from > hay.len() {
            return None;
        }
        let m = self
            .m
            .first
            .search_with(&mut self.c1, &Input::new(hay).span(from..hay.len()))?;
        let e = self.longest(hay, m.start(), hay.len()).unwrap_or(m.end());
        Some(m.start()..e)
    }

    /// The end of the longest match starting exactly at `s` and ending at or before `limit`.
    fn longest(&mut self, hay: &str, s: usize, limit: usize) -> Option<usize> {
        let input = Input::new(hay).span(s..limit).anchored(Anchored::Yes);
        self.m
            .all
            .search_captures_with(&mut self.c2, &input, &mut self.caps);
        self.caps.get_match().map(|m| m.end())
    }

    /// Group `i` of the last match found (0 is the whole match).
    pub(crate) fn group(&self, i: usize) -> Option<Span> {
        self.caps.get_group(i).map(|s| s.start..s.end)
    }

    /// Like [`Search::find`], but only matches with no word character just before or just
    /// after (`grep -w`): GNU's loop, trying shorter matches from the same start, then later
    /// starts.
    pub(crate) fn find_word(&mut self, hay: &str, from: usize) -> Result<Option<Span>, ToolError> {
        let mut from = from;
        loop {
            let Some(m) = self.find(hay, from) else {
                return Ok(None);
            };
            let s = m.start;
            let mut e = m.end;
            loop {
                self.steps += 1;
                if self.steps > WORD_STEPS {
                    return Err(ToolError::Limit(
                        "grep -w search took too many steps".into(),
                    ));
                }
                let before = hay[..s].chars().next_back().is_some_and(is_word);
                let after = hay[e..].chars().next().is_some_and(is_word);
                if !before && !after {
                    return Ok(Some(s..e));
                }
                if e == s {
                    break;
                }
                let shorter = e - hay[..e].chars().next_back().map_or(1, char::len_utf8);
                match self.longest(hay, s, shorter) {
                    Some(e2) => e = e2,
                    None => break,
                }
            }
            if s >= hay.len() {
                return Ok(None);
            }
            from = s + hay[s..].chars().next().map_or(1, char::len_utf8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tr(p: &str, ere: bool) -> String {
        translate(
            p,
            if ere {
                Dialect::Extended
            } else {
                Dialect::Basic
            },
            Flavor::Grep,
        )
        .unwrap()
    }

    fn find(p: &str, ere: bool, hay: &str) -> Option<(usize, usize)> {
        let m = Matcher::new(&tr(p, ere), false, false).unwrap();
        let mut s = m.search();
        s.find(hay, 0).map(|r| (r.start, r.end))
    }

    #[test]
    fn bre_translation() {
        assert_eq!(tr(r"a\(b\)*c", false), "a(b)*c");
        assert_eq!(tr(r"a+b?c|d(e){2}", false), r"a\+b\?c\|d\(e\)\{2\}");
        assert_eq!(tr(r"a\{2,3\}", false), "a{2,3}");
        assert_eq!(tr(r"a\{,3\}", false), "a{0,3}");
        assert_eq!(tr(r"*a", false), r"\*a");
        assert_eq!(tr(r"^*a", false), r"^\*a");
        assert_eq!(tr(r"a^b$c$", false), r"a\^b\$c$");
        assert_eq!(tr(r"\(^a$\)", false), "(^a$)");
        assert_eq!(tr(r"[]a\-]", false), r"[\]a\\\-]");
        assert_eq!(tr(r"[^]a]", false), r"[^\]a]");
        assert_eq!(tr(r"[[:digit:]x-z]", false), r"[0-9x-z]");
        assert_eq!(tr(r"\<a\>", false), r"\b{start}a\b{end}");
        assert_eq!(tr(r"a.b\.c", false), r"a.b\.c");
        assert_eq!(tr(r"a{2}(b)+|c", true), r"a{2}(b)+|c");
        assert_eq!(tr(r"a{x", true), r"a\{x");
        assert_eq!(tr(r"\(", true), r"\(");
        assert_eq!(tr(r"[ \t]", false), r"[ \\t]");
        assert_eq!(
            translate(r"[ \t]\n", Dialect::Basic, Flavor::Sed).unwrap(),
            r"[ \t]\n"
        );
    }

    #[test]
    fn refusals_and_errors() {
        for p in [r"\(a\)\1", r"x\9"] {
            assert!(matches!(
                translate(p, Dialect::Basic, Flavor::Grep),
                Err(ToolError::Refused { .. })
            ));
        }
        assert!(matches!(
            translate(r"(a)\1", Dialect::Extended, Flavor::Grep),
            Err(ToolError::Refused { .. })
        ));
        assert!(matches!(
            translate(r"\d", Dialect::Extended, Flavor::Grep),
            Err(ToolError::Unsupported { .. })
        ));
        for p in [r"a\", "[a", r"\(a", "a)", "[[:nope:]]", "[z-a]"] {
            let d = if p == "a)" {
                Dialect::Extended
            } else {
                Dialect::Basic
            };
            assert!(
                matches!(translate(p, d, Flavor::Grep), Err(ToolError::Usage(_))),
                "{p}"
            );
        }
        assert!(matches!(
            translate(
                &"a".repeat(MAX_PATTERN_BYTES + 1),
                Dialect::Basic,
                Flavor::Grep
            ),
            Err(ToolError::Limit(_))
        ));
        let big = translate(
            r"\([[:alpha:]]\{1000\}\)\{100\}",
            Dialect::Basic,
            Flavor::Grep,
        )
        .unwrap();
        assert!(matches!(
            Matcher::new(&big, false, false),
            Err(ToolError::Limit(_))
        ));
    }

    #[test]
    fn leftmost_longest() {
        assert_eq!(find(r"a\|ab", false, "xabc"), Some((1, 3)));
        assert_eq!(find(r"sam|samwise", true, "sam samwise"), Some((0, 3)));
        assert_eq!(find(r"x*", false, "abc"), Some((0, 0)));
        assert_eq!(find(r"b$", false, "ab b"), Some((3, 4)));
        assert_eq!(find(r"é+", true, "aééb"), Some((1, 5)));
        assert_eq!(find(r"é+", false, "aé+b"), Some((1, 4)));
        // groups belong to the longest match
        let m = Matcher::new(&tr(r"\(a\|ab\)\(c\|bcd\)\(d*\)", false), false, false).unwrap();
        let mut s = m.search();
        assert_eq!(s.find("abcd", 0), Some(0..4));
        assert!(s.group(1).is_some());
    }

    #[test]
    fn word_matches() {
        let m = Matcher::new(&tr("foo", false), false, false).unwrap();
        let mut s = m.search();
        assert_eq!(s.find_word("foobar foo", 0).unwrap(), Some(7..10));
        assert_eq!(s.find_word("foobar", 0).unwrap(), None);
        let m = Matcher::new(&tr("a*", false), false, false).unwrap();
        let mut s = m.search();
        // the longest "aa" fails ("aab"), shorter ones too; then later starts
        assert_eq!(s.find_word("aab aa", 0).unwrap(), Some(4..6));
        let m = Matcher::new(&tr("@foo", false), false, false).unwrap();
        let mut s = m.search();
        assert_eq!(s.find_word("a @foo b", 0).unwrap(), Some(2..6));
    }
}
