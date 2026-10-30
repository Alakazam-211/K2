//! `replace --old TEXT --new TEXT [--all]`: exact-string replace, the third mode of
//! `replace` (beside `--lines` and `--range`).
//!
//! - **Exact.** `--old` is matched byte for byte on the doc text the other edit verbs see
//!   (`\n` line breaks, no BOM; the pack restores a CRLF file's `\r\n` and its BOM when it
//!   encodes the doc). No regex, no escapes, case-sensitive, whitespace as given.
//! - **Unique by default.** No match, or more than one, applies nothing and exits 1 with a
//!   report on stdout: the match count and each match's line(s), so the caller can retry
//!   with more context. Matches are counted left to right without overlaps (what `--all`
//!   replaces); when the only such match overlaps another occurrence of `--old` (`aa` in
//!   `aaa`), that is more than one too.
//! - **`--all`** replaces every non-overlapping match, left to right (`str::replace`).
//! - `--old` must be non-empty; `--new` may be empty (deletes the match). Either may be `-`:
//!   the text is stdin (one of them only). Otherwise stdin is not read.
//! - `--old` equal to `--new` with a unique match (or any match, with `--all`) succeeds and
//!   changes nothing: no new version.

use super::error::{exit, ToolError};
use super::{too_much_output, EditOutput, MAX_OUTPUT_BYTES};

/// The most match positions a report lists; the rest are counted ("and N more").
const LIST_CAP: usize = 20;

/// Where `--old` / `--new` come from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Src {
    Arg(String),
    Stdin,
}

/// A parsed `replace --old/--new [--all]`.
#[derive(Debug, Clone)]
pub(crate) struct Exact {
    old: Src,
    new: Src,
    all: bool,
}

/// The flags of the mode, as `edit.rs` collected them.
pub(crate) const FLAGS: &[&str] = &["old", "new", "all"];

impl Exact {
    /// From the collected options of `replace` (ids from `edit.rs`'s table). The caller has
    /// checked that one of [`FLAGS`] is there and none of the positional ones.
    pub(crate) fn from_found(found: &[(&'static str, Option<String>)]) -> Result<Self, ToolError> {
        let mut old: Option<Src> = None;
        let mut new: Option<Src> = None;
        let mut all = false;
        for (id, v) in found {
            let slot = match *id {
                "old" => &mut old,
                "new" => &mut new,
                "all" => {
                    all = true;
                    continue;
                }
                other => {
                    return Err(ToolError::unsupported(
                        format!("--{other}"),
                        "replace --old/--new takes only --all besides them",
                    ))
                }
            };
            if slot.is_some() {
                return Err(ToolError::usage(format!(
                    "--{id} given twice: replace takes one --old and one --new"
                )));
            }
            let v = v.clone().unwrap_or_default();
            *slot = Some(if v == "-" { Src::Stdin } else { Src::Arg(v) });
        }
        let Some(old) = old else {
            return Err(ToolError::usage(
                "replace needs --old TEXT: the exact text to replace (with --new TEXT)",
            ));
        };
        let Some(new) = new else {
            return Err(ToolError::usage(
                "replace --old needs --new TEXT (--new '' deletes the match)",
            ));
        };
        if old == Src::Stdin && new == Src::Stdin {
            return Err(ToolError::usage(
                "only one of --old and --new can be '-' (stdin): pass the other as text",
            ));
        }
        if old == Src::Arg(String::new()) {
            return Err(empty_old());
        }
        Ok(Exact { old, new, all })
    }

    /// Run on the live `text` of the doc `name`. A refusal (no match, or not unique) is
    /// `text: None`, exit 1, and the report on stdout.
    pub(crate) fn run(
        &self,
        name: &str,
        text: &str,
        stdin: &[u8],
    ) -> Result<EditOutput, ToolError> {
        let read = |s: &Src, what: &str| -> Result<String, ToolError> {
            match s {
                Src::Arg(t) => Ok(t.clone()),
                Src::Stdin => String::from_utf8(stdin.to_vec())
                    .map_err(|_| ToolError::usage(format!("{what} (stdin) is not UTF-8 text"))),
            }
        };
        let old = read(&self.old, "--old")?;
        let new = read(&self.new, "--new")?;
        if old.is_empty() {
            return Err(empty_old());
        }
        let label = if self.old == Src::Stdin {
            "--old (stdin)"
        } else {
            "--old"
        };
        let (count, starts) = find(text, &old);
        if count == 0 {
            return Ok(refuse(no_match(name, text, &old, label)));
        }
        let spans = |starts: &[usize]| -> Vec<(usize, usize)> {
            starts.iter().map(|&s| (s, s + old.len())).collect()
        };
        if self.all {
            let grown = new.len().saturating_sub(old.len()).saturating_mul(count);
            if text.len().saturating_add(grown) > MAX_OUTPUT_BYTES {
                return Err(too_much_output());
            }
            let out = text.replace(&old, &new);
            let report = if old == new {
                Vec::new()
            } else {
                format!(
                    "replaced {count} match{} in {name} ({} before the edit)\n",
                    plural_es(count),
                    at_lines(text, &spans(&starts), count)
                )
                .into_bytes()
            };
            return Ok(EditOutput::new(Some(out), report, exit::OK));
        }
        if count > 1 {
            return Ok(refuse(format!(
                "replace: {label} matches {count} times in {name} ({}); nothing applied\n\
                 to replace one, add surrounding text to {label} until it matches once; \
                 to replace every match, add --all\n",
                at_lines(text, &spans(&starts), count)
            )));
        }
        let s = starts[0];
        let overlapping = overlaps(text, &old, s);
        if overlapping.len() > 1 {
            let n = overlapping.len();
            return Ok(refuse(format!(
                "replace: {label} matches {}{n} times in {name}, overlapping itself ({}); \
                 nothing applied\n\
                 to replace one, add surrounding text to {label} until it matches once; \
                 --all replaces left to right without overlaps (1 match here)\n",
                if n > LIST_CAP { "more than " } else { "" },
                at_lines(
                    text,
                    &spans(&overlapping[..n.min(LIST_CAP)]),
                    n.min(LIST_CAP)
                ),
            )));
        }
        if text.len() - old.len() + new.len() > MAX_OUTPUT_BYTES {
            return Err(too_much_output());
        }
        let mut out = String::with_capacity(text.len() - old.len() + new.len());
        out.push_str(&text[..s]);
        out.push_str(&new);
        out.push_str(&text[s + old.len()..]);
        Ok(EditOutput::new(Some(out), Vec::new(), exit::OK))
    }
}

fn empty_old() -> ToolError {
    ToolError::usage("--old is empty: give the exact text to replace (to add text, use insert)")
}

fn refuse(report: String) -> EditOutput {
    EditOutput::new(None, report.into_bytes(), exit::NO)
}

fn plural_es(n: usize) -> &'static str {
    if n == 1 {
        ""
    } else {
        "es"
    }
}

/// Non-overlapping matches, left to right: their count and the first [`LIST_CAP`] starts.
fn find(text: &str, old: &str) -> (usize, Vec<usize>) {
    let mut count = 0;
    let mut starts = Vec::new();
    for (s, _) in text.match_indices(old) {
        count += 1;
        if starts.len() < LIST_CAP {
            starts.push(s);
        }
    }
    (count, starts)
}

/// Every occurrence of `old` from `s` (the only non-overlapping match, so every other one
/// starts inside it), up to one past [`LIST_CAP`].
fn overlaps(text: &str, old: &str, s: usize) -> Vec<usize> {
    let mut out = vec![s];
    let mut at = s;
    while out.len() <= LIST_CAP {
        let step = text[at..].chars().next().map_or(1, char::len_utf8);
        match text[at + step..].find(old) {
            Some(p) => {
                at += step + p;
                out.push(at);
            }
            None => break,
        }
    }
    out
}

/// `at line 4`, `at lines 2, 5 (×2), 9–10`, `… and 37 more`: the lines of `spans` (byte
/// ranges of `text`, in order), of `total` matches in all.
fn at_lines(text: &str, spans: &[(usize, usize)], total: usize) -> String {
    let nl: Vec<usize> = text
        .bytes()
        .enumerate()
        .filter(|(_, b)| *b == b'\n')
        .map(|(i, _)| i)
        .collect();
    let line = |p: usize| nl.partition_point(|&n| n < p) + 1;
    let mut items: Vec<(String, usize)> = Vec::new();
    for &(s, e) in spans {
        let (a, b) = (line(s), line(e.max(s + 1) - 1));
        let item = if a == b {
            format!("{a}")
        } else {
            format!("{a}–{b}")
        };
        match items.last_mut() {
            Some((last, n)) if *last == item => *n += 1,
            _ => items.push((item, 1)),
        }
    }
    let many = total > 1 || items.len() > 1 || items.iter().any(|(i, n)| *n > 1 || i.contains('–'));
    let mut out = format!("at line{} ", if many { "s" } else { "" });
    out.push_str(
        &items
            .iter()
            .map(|(i, n)| {
                if *n > 1 {
                    format!("{i} (×{n})")
                } else {
                    i.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
    );
    if total > spans.len() {
        out.push_str(&format!(" and {} more", total - spans.len()));
    }
    out
}

/// The report for no match, with hints for near misses (never applied).
fn no_match(name: &str, text: &str, old: &str, label: &str) -> String {
    let mut out = format!("replace: no exact match for {label} in {name}; nothing applied\n");
    let mut hinted = false;
    // a CR or BOM hint that matched explains it; the loosened folds would only repeat it
    let mut explained = false;
    let mut hint = |out: &mut String, s: String| {
        hinted = true;
        out.push_str("hint: ");
        out.push_str(&s);
        out.push('\n');
    };
    let found = |needle: &str| {
        let (n, starts) = find(text, needle);
        (n > 0).then(|| {
            let spans: Vec<(usize, usize)> =
                starts.iter().map(|&s| (s, s + needle.len())).collect();
            (n, at_lines(text, &spans, n))
        })
    };
    if old.contains('\r') {
        let lf = old.replace("\r\n", "\n");
        let what = format!(
            "{label} has \\r (CR) characters, but edit verbs see the doc's text with \\n \
             line breaks (a CRLF file's \\r\\n are restored when it is written)"
        );
        match found(&lf) {
            Some((n, at)) => {
                explained = true;
                hint(
                    &mut out,
                    format!("{what}; with \\n it matches {}, {at}", times(n)),
                )
            }
            None => hint(&mut out, format!("{what}; use \\n")),
        }
    }
    if let Some(rest) = old.strip_prefix('\u{feff}') {
        let what =
            format!("{label} starts with a byte-order mark (U+FEFF), which edit verbs don't see");
        match (!rest.is_empty()).then(|| found(rest)).flatten() {
            Some((n, at)) => {
                explained = true;
                hint(
                    &mut out,
                    format!("{what}; without it it matches {}, {at}", times(n)),
                )
            }
            None => hint(&mut out, format!("{what}; leave it out")),
        }
    }
    let kinds = [
        (
            Fold {
                case: false,
                ws: true,
            },
            "if whitespace is loosened (runs of spaces, tabs and line breaks as one space; \
             leading and trailing whitespace ignored)",
        ),
        (
            Fold {
                case: true,
                ws: false,
            },
            "if case is ignored",
        ),
        (
            Fold {
                case: true,
                ws: true,
            },
            "if case is ignored and whitespace loosened",
        ),
    ];
    let mut loose = false;
    for (i, (fold, how)) in kinds.iter().enumerate() {
        if explained {
            break;
        }
        if i == 2 && loose {
            break; // a looser fold adds nothing to a hint already given
        }
        if let Some((n, spans)) = fold.find(text, old) {
            loose = true;
            hint(
                &mut out,
                format!(
                    "{label} would match {} {how}, {}; nothing was applied: --old is exact",
                    times(n),
                    at_lines(text, &spans, n)
                ),
            );
        }
    }
    if !hinted {
        out.push_str(
            "--old is matched exactly (bytes, case, whitespace, line breaks); read the doc \
             again (cat -n, view --lines a,b) and copy the text from it\n",
        );
    }
    out
}

fn times(n: usize) -> String {
    if n == 1 {
        "once".into()
    } else {
        format!("{n} times")
    }
}

/// A loosened comparison for the no-match hints.
#[derive(Debug, Clone, Copy)]
struct Fold {
    /// Lowercase both sides.
    case: bool,
    /// Runs of whitespace become one space; the needle's ends are trimmed.
    ws: bool,
}

impl Fold {
    /// `s` folded, and for each source char that emitted something: (folded start, source
    /// start, source end).
    fn apply(self, s: &str) -> (String, Vec<(usize, usize, usize)>) {
        let mut out = String::with_capacity(s.len());
        let mut map = Vec::new();
        let mut in_ws = false;
        for (i, c) in s.char_indices() {
            let at = out.len();
            if self.ws && c.is_whitespace() {
                if !in_ws {
                    out.push(' ');
                    map.push((at, i, i + c.len_utf8()));
                } else if let Some(m) = map.last_mut() {
                    m.2 = i + c.len_utf8();
                }
                in_ws = true;
                continue;
            }
            in_ws = false;
            if self.case {
                out.extend(c.to_lowercase());
            } else {
                out.push(c);
            }
            map.push((at, i, i + c.len_utf8()));
        }
        (out, map)
    }

    /// Matches of the folded `needle` in the folded `text`: their count and the source
    /// spans of the first [`LIST_CAP`]. `None`: no match, or the needle folds to nothing.
    fn find(self, text: &str, needle: &str) -> Option<(usize, Vec<(usize, usize)>)> {
        let (n, _) = self.apply(needle);
        let n = if self.ws { n.trim_matches(' ') } else { &n[..] };
        if n.is_empty() {
            return None;
        }
        let (t, map) = self.apply(text);
        let src = |fs: usize, fe: usize| {
            let a = map.partition_point(|m| m.0 <= fs) - 1;
            let b = map.partition_point(|m| m.0 < fe) - 1;
            (map[a].1, map[b].2)
        };
        let mut count = 0;
        let mut spans = Vec::new();
        for (fs, m) in t.match_indices(n) {
            count += 1;
            if spans.len() < LIST_CAP {
                spans.push(src(fs, fs + m.len()));
            }
        }
        (count > 0).then_some((count, spans))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exact(old: &str, new: &str, all: bool) -> Exact {
        let mut f: Vec<(&'static str, Option<String>)> =
            vec![("old", Some(old.into())), ("new", Some(new.into()))];
        if all {
            f.push(("all", None));
        }
        Exact::from_found(&f).unwrap()
    }

    #[test]
    fn line_lists() {
        let t = "a\nb\nc\nd\n";
        assert_eq!(at_lines(t, &[(0, 1)], 1), "at line 1");
        assert_eq!(at_lines(t, &[(0, 3)], 1), "at lines 1–2");
        assert_eq!(at_lines(t, &[(0, 1), (2, 5)], 2), "at lines 1, 2–3");
        assert_eq!(at_lines(t, &[(0, 1), (0, 1)], 2), "at lines 1 (×2)");
        assert_eq!(at_lines(t, &[(6, 8)], 3), "at lines 4 and 2 more");
        // a match ending with its line's \n is on that line only
        assert_eq!(at_lines(t, &[(2, 4)], 1), "at line 2");
    }

    #[test]
    fn folds_map_back_to_the_source() {
        let t = "Keep  FILES\tplain\n";
        let ws = Fold {
            case: false,
            ws: true,
        };
        let ci = Fold {
            case: true,
            ws: false,
        };
        assert_eq!(ws.find(t, "FILES plain"), Some((1, vec![(6, 17)])));
        assert_eq!(ci.find(t, "files"), Some((1, vec![(6, 11)])));
        assert_eq!(ws.find(t, "  \n"), None);
        // İ lowercases to two chars: spans still land on the source's boundaries
        let t = "xİy";
        assert_eq!(ci.find(t, "i\u{307}y"), Some((1, vec![(1, 4)])));
    }

    #[test]
    fn overlapping_occurrences_are_not_unique() {
        let o = exact("aa", "X", false).run("n", "aaa", b"").unwrap();
        assert_eq!(o.text, None);
        assert_eq!(o.exit, 1);
        let o = exact("aa", "X", true).run("n", "aaa", b"").unwrap();
        assert_eq!(o.text.as_deref(), Some("Xa"));
        // a single non-overlapping occurrence after an unrelated one is still unique
        let o = exact("aba", "X", false).run("n", "abab", b"").unwrap();
        assert_eq!(o.text.as_deref(), Some("Xb"));
    }

    #[test]
    fn growth_is_bounded_before_allocating() {
        let text = "a".repeat(1 << 20);
        let big = "b".repeat(64);
        assert!(matches!(
            exact("a", &big, true).run("n", &text, b""),
            Err(ToolError::Limit(_))
        ));
    }
}
