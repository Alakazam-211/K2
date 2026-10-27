//! `append`, `insert`, `replace`, `delete`: the positional edit verbs.
//!
//! Line positions are GNU lines (a final `\n` ends the last line; it doesn't start a new
//! one), 1-based, and `$` is the last line. Line forms mirror sed: inserted text becomes
//! whole lines (a missing final `\n` is added), like `sed 'Ni\'`, `sed 'Na\'`,
//! `sed 'a,bc\'` and `sed 'a,bd'`; text added after a last line that lacks its `\n` gives
//! it one, as sed does.
//!
//! Character forms (`--at L:C`, `--range L:C-L:C`) take stdin as is. Lines are 1-based,
//! columns 1-based in characters; `chars + 1` is the end of a line. A range is half-open:
//! from its first position up to, not including, its second.

use super::args::{self, opt, Arity, Item, Opt, Spec};
use super::error::ToolError;
use super::lines;
use super::read::{LineNo, LineSpec};
use crate::session::position::{self, LineCol};

#[derive(Debug, Clone)]
pub(crate) enum PosEdit {
    Append { ensure_newline: bool },
    InsertLine(LineNo),
    InsertAfter(LineNo),
    InsertAt(LineCol),
    ReplaceLines(LineSpec),
    ReplaceRange(LineCol, LineCol),
    DeleteLines(LineSpec),
    DeleteRange(LineCol, LineCol),
}

const OPTS: &[Opt] = &[
    opt("ensure-newline", None, Some("ensure-newline"), Arity::No),
    opt("line", None, Some("line"), Arity::Req),
    opt("after", None, Some("after"), Arity::Req),
    opt("at", None, Some("at"), Arity::Req),
    opt("lines", None, Some("lines"), Arity::Req),
    opt("range", None, Some("range"), Arity::Req),
];

fn spec(tool: &'static str) -> Spec {
    Spec {
        tool,
        opts: OPTS,
        unsupported: &[],
        numeric: false,
        numeric_in_cluster: false,
    }
}

fn line_col(s: &str) -> Result<LineCol, ToolError> {
    let bad = || ToolError::usage(format!("invalid position '{s}': use LINE:COL"));
    let (l, c) = s.split_once(':').ok_or_else(bad)?;
    let line: usize = l.trim().parse().map_err(|_| bad())?;
    let col: usize = c.trim().parse().map_err(|_| bad())?;
    if line == 0 || col == 0 {
        return Err(ToolError::usage("lines and columns start at 1"));
    }
    Ok(LineCol { line, col })
}

fn range(s: &str) -> Result<(LineCol, LineCol), ToolError> {
    let (a, b) = s
        .split_once('-')
        .ok_or_else(|| ToolError::usage(format!("invalid range '{s}': use L:C-L:C")))?;
    let (a, b) = (line_col(a)?, line_col(b)?);
    if b < a {
        return Err(ToolError::usage(format!(
            "range '{s}' ends before it starts"
        )));
    }
    Ok((a, b))
}

impl PosEdit {
    pub(crate) fn parse(verb: &'static str, args: &[String]) -> Result<Self, ToolError> {
        let sp = spec(verb);
        let mut found: Vec<(&'static str, Option<String>)> = Vec::new();
        for it in args::parse(args, &sp)? {
            match it {
                Item::Opt(id, v) => found.push((id, v)),
                Item::Operand(o) => return Err(super::no_operands(verb, &o)),
            }
        }
        let allowed: &[&str] = match verb {
            "append" => &["ensure-newline"],
            "insert" => &["line", "after", "at"],
            _ => &["lines", "range"],
        };
        for (id, _) in &found {
            if !allowed.contains(id) {
                return Err(ToolError::unsupported(
                    format!("--{id}"),
                    format!(
                        "{verb} takes {}",
                        allowed
                            .iter()
                            .map(|a| format!("--{a}"))
                            .collect::<Vec<_>>()
                            .join(" or ")
                    ),
                ));
            }
        }
        if verb == "append" {
            return Ok(PosEdit::Append {
                ensure_newline: !found.is_empty(),
            });
        }
        let [(id, Some(v))] = found.as_slice() else {
            return Err(ToolError::usage(format!(
                "{verb} needs exactly one of {}",
                allowed
                    .iter()
                    .map(|a| format!("--{a}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        Ok(match (verb, *id) {
            ("insert", "line") => {
                let n = LineNo::parse(v)?;
                if n == LineNo::N(0) {
                    return Err(ToolError::usage("line numbers start at 1 (use --after 0)"));
                }
                PosEdit::InsertLine(n)
            }
            ("insert", "after") => PosEdit::InsertAfter(LineNo::parse(v)?),
            ("insert", _) => PosEdit::InsertAt(line_col(v)?),
            ("replace", "lines") => PosEdit::ReplaceLines(LineSpec::parse(v)?),
            ("replace", _) => {
                let (a, b) = range(v)?;
                PosEdit::ReplaceRange(a, b)
            }
            (_, "lines") => PosEdit::DeleteLines(LineSpec::parse(v)?),
            _ => {
                let (a, b) = range(v)?;
                PosEdit::DeleteRange(a, b)
            }
        })
    }

    /// The new text.
    pub(crate) fn apply(&self, text: &str, stdin: &str) -> Result<String, ToolError> {
        let ls = lines(text);
        let count = ls.len();
        let start_of = |n: usize| ls.get(n - 1).map_or(text.len(), |l| l.start);
        let as_lines = |s: &str| -> String {
            if s.is_empty() || s.ends_with('\n') {
                s.to_owned()
            } else {
                format!("{s}\n")
            }
        };
        // Insert whole lines at byte `at` (a line start, or the end of the text).
        let insert_lines = |at: usize, s: &str| -> String {
            let block = as_lines(s);
            if block.is_empty() {
                return text.to_owned();
            }
            let mut out = String::with_capacity(text.len() + block.len() + 1);
            out.push_str(&text[..at]);
            if at == text.len() && !text.is_empty() && !text.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&block);
            out.push_str(&text[at..]);
            out
        };
        let lines_span = |spec: &LineSpec| -> Result<(usize, usize), ToolError> {
            let a = spec.first.resolve(count);
            let b = spec.last.resolve(count);
            if a == 0 || a > count || b > count || a > b {
                return Err(ToolError::usage(format!(
                    "lines {a},{b} aren't in the doc ({count} line{})",
                    if count == 1 { "" } else { "s" }
                )));
            }
            let s = ls[a - 1].start;
            let e = ls.get(b).map_or(text.len(), |l| l.start);
            Ok((s, e))
        };
        let pos = |at: LineCol| -> Result<usize, ToolError> {
            position::offset(text, at)
                .ok_or_else(|| ToolError::usage(format!("{}:{} isn't in the doc", at.line, at.col)))
        };
        let splice = |s: usize, e: usize, with: &str| -> String {
            let mut out = String::with_capacity(text.len() + with.len());
            out.push_str(&text[..s]);
            out.push_str(with);
            out.push_str(&text[e..]);
            out
        };
        Ok(match self {
            PosEdit::Append { ensure_newline } => {
                let mut out = String::with_capacity(text.len() + stdin.len() + 1);
                out.push_str(text);
                if *ensure_newline && !text.is_empty() && !text.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(stdin);
                out
            }
            PosEdit::InsertLine(n) => {
                let n = n.resolve(count).max(1);
                if n > count + 1 {
                    return Err(ToolError::usage(format!(
                        "line {n} isn't in the doc ({count} lines; {} is the end)",
                        count + 1
                    )));
                }
                insert_lines(start_of(n), stdin)
            }
            PosEdit::InsertAfter(n) => {
                let n = n.resolve(count);
                if n > count {
                    return Err(ToolError::usage(format!(
                        "line {n} isn't in the doc ({count} lines)"
                    )));
                }
                insert_lines(if n == 0 { 0 } else { start_of(n + 1) }, stdin)
            }
            PosEdit::InsertAt(at) => {
                let p = pos(*at)?;
                splice(p, p, stdin)
            }
            PosEdit::ReplaceLines(spec) => {
                let (s, e) = lines_span(spec)?;
                splice(s, e, &as_lines(stdin))
            }
            PosEdit::DeleteLines(spec) => {
                let (s, e) = lines_span(spec)?;
                splice(s, e, "")
            }
            PosEdit::ReplaceRange(a, b) => splice(pos(*a)?, pos(*b)?, stdin),
            PosEdit::DeleteRange(a, b) => splice(pos(*a)?, pos(*b)?, ""),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        verb: &'static str,
        args: &[&str],
        text: &str,
        stdin: &str,
    ) -> Result<String, ToolError> {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        PosEdit::parse(verb, &a)?.apply(text, stdin)
    }

    #[test]
    fn line_edits_mirror_sed() {
        let t = "a\nb\nc\n";
        assert_eq!(
            run("insert", &["--line", "2"], t, "X").unwrap(),
            "a\nX\nb\nc\n"
        );
        assert_eq!(
            run("insert", &["--line", "4"], t, "X\n").unwrap(),
            "a\nb\nc\nX\n"
        );
        assert_eq!(
            run("insert", &["--line", "$"], t, "X").unwrap(),
            "a\nb\nX\nc\n"
        );
        assert_eq!(
            run("insert", &["--after", "0"], t, "X").unwrap(),
            "X\na\nb\nc\n"
        );
        assert_eq!(
            run("insert", &["--after", "$"], "a\nb", "X").unwrap(),
            "a\nb\nX\n"
        );
        assert!(run("insert", &["--after", "1"], "", "X").is_err());
        assert_eq!(run("insert", &["--after", "0"], "", "X").unwrap(), "X\n");
        assert_eq!(
            run("insert", &["--at", "2:2"], "ab\ncd\n", "Z").unwrap(),
            "ab\ncZd\n"
        );
        assert_eq!(
            run("replace", &["--lines", "2,3"], t, "Y").unwrap(),
            "a\nY\n"
        );
        assert_eq!(run("replace", &["--lines", "2"], t, "").unwrap(), "a\nc\n");
        assert_eq!(
            run("replace", &["--range", "1:2-2:2"], "ab\ncd\n", "-").unwrap(),
            "a-d\n"
        );
        assert_eq!(run("delete", &["--lines", "1,$"], t, "").unwrap(), "");
        assert_eq!(
            run("delete", &["--lines", "3"], "a\nb\nc", "").unwrap(),
            "a\nb\n"
        );
        assert_eq!(
            run("delete", &["--range", "1:1-1:3"], "abc\n", "").unwrap(),
            "c\n"
        );
        assert_eq!(run("append", &[], "a", "b").unwrap(), "ab");
        assert_eq!(
            run("append", &["--ensure-newline"], "a", "b").unwrap(),
            "a\nb"
        );
        assert_eq!(run("append", &["--ensure-newline"], "", "b").unwrap(), "b");
    }

    #[test]
    fn bad_positions() {
        for (v, a) in [
            ("insert", &["--line", "0"][..]),
            ("insert", &["--line", "9"]),
            ("insert", &["--line", "1", "--after", "1"]),
            ("insert", &[]),
            ("replace", &["--lines", "3,2"]),
            ("replace", &["--lines", "9"]),
            ("replace", &["--range", "1:5-1:1"]),
            ("delete", &["--range", "9:1-9:2"]),
            ("delete", &["--at", "1:1"]),
        ] {
            assert!(run(v, a, "a\nb\nc\n", "").is_err(), "{v} {a:?}");
        }
    }
}
