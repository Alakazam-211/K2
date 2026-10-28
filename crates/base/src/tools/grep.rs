//! `grep` on one doc, with GNU grep's output and exit status.
//!
//! Flags: `-E -F -G -e PAT -i -y --no-ignore-case -v -w -x -c -o -q -s -l -L -H -h -n -m N
//! -A N -B N -C N -NUM --color[=never|auto]`. Patterns are GNU BRE (default) or ERE
//! (see `pattern.rs`); a pattern holding newlines is several patterns. The doc's display
//! name is the file name for `-H`, `-c`, `-l` and `-L`. Exit 0 when a line was selected,
//! 1 when none was (also for `-L`, as grep 3.5+).
//!
//! One doc per call: a host grepping several docs runs the verb once per doc, adds `-H`
//! when there are several (as grep does), and combines the exit statuses.

use super::args::{self, opt, Arity, Item, Opt, Spec};
use super::error::ToolError;
use super::pattern::{self, Dialect, Flavor, Matcher, Search};
use super::{lines, ForEditArg, Line, ReadOutput, ToolDoc, FOR_EDIT, TTL};

#[derive(Debug, Clone)]
pub(crate) struct Grep {
    matcher: Matcher,
    invert: bool,
    word: bool,
    count: bool,
    only: bool,
    quiet: bool,
    list: Option<bool>,
    with_name: bool,
    number: bool,
    max: Option<u64>,
    after: usize,
    before: usize,
    pub(crate) for_edit: Option<Option<u64>>,
}

const OPTS: &[Opt] = &[
    opt("E", Some('E'), Some("extended-regexp"), Arity::No),
    opt("F", Some('F'), Some("fixed-strings"), Arity::No),
    opt("G", Some('G'), Some("basic-regexp"), Arity::No),
    opt("e", Some('e'), Some("regexp"), Arity::Req),
    opt("i", Some('i'), Some("ignore-case"), Arity::No),
    opt("i", Some('y'), None, Arity::No),
    opt("no-i", None, Some("no-ignore-case"), Arity::No),
    opt("v", Some('v'), Some("invert-match"), Arity::No),
    opt("w", Some('w'), Some("word-regexp"), Arity::No),
    opt("x", Some('x'), Some("line-regexp"), Arity::No),
    opt("c", Some('c'), Some("count"), Arity::No),
    opt("o", Some('o'), Some("only-matching"), Arity::No),
    opt("q", Some('q'), Some("quiet"), Arity::No),
    opt("q", None, Some("silent"), Arity::No),
    opt("s", Some('s'), Some("no-messages"), Arity::No),
    opt("l", Some('l'), Some("files-with-matches"), Arity::No),
    opt("L", Some('L'), Some("files-without-match"), Arity::No),
    opt("H", Some('H'), Some("with-filename"), Arity::No),
    opt("h", Some('h'), Some("no-filename"), Arity::No),
    opt("n", Some('n'), Some("line-number"), Arity::No),
    opt("m", Some('m'), Some("max-count"), Arity::Req),
    opt("A", Some('A'), Some("after-context"), Arity::Req),
    opt("B", Some('B'), Some("before-context"), Arity::Req),
    opt("C", Some('C'), Some("context"), Arity::Req),
    opt("color", None, Some("color"), Arity::Opt),
    opt("color", None, Some("colour"), Arity::Opt),
    FOR_EDIT,
    TTL,
];

const SPEC: Spec = Spec {
    tool: "grep",
    opts: OPTS,
    unsupported: &[
        ("-P", "Perl regexes aren't supported; use -E"),
        ("--perl-regexp", "Perl regexes aren't supported; use -E"),
        (
            "-f",
            "pass patterns with -e (or one per line in a single pattern)",
        ),
        (
            "--file",
            "pass patterns with -e (or one per line in a single pattern)",
        ),
        ("-r", "one doc per call; the host greps several docs"),
        ("-R", "one doc per call; the host greps several docs"),
        (
            "--recursive",
            "one doc per call; the host greps several docs",
        ),
        ("-z", "lines end with \\n"),
        ("--null-data", "lines end with \\n"),
        ("-b", "byte offsets aren't supported; use -n"),
        ("--byte-offset", "byte offsets aren't supported; use -n"),
        ("--label", "the host names the doc"),
    ],
    numeric: false,
    numeric_in_cluster: true,
};

fn number(v: &str, what: &str) -> Result<u64, ToolError> {
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ToolError::usage(format!("invalid {what} argument: '{v}'")));
    }
    Ok(v.bytes().fold(0u64, |n, b| {
        n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
    }))
}

fn context(v: &str) -> Result<usize, ToolError> {
    Ok(usize::try_from(number(v, "context length")?).unwrap_or(usize::MAX))
}

impl Grep {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        let mut dialect = Dialect::Basic;
        let mut patterns: Vec<String> = Vec::new();
        let mut operands: Vec<String> = Vec::new();
        let mut icase = false;
        let mut line = false;
        let mut g = GrepFlags::default();
        let mut fe = ForEditArg::default();
        for it in args::parse(args, &SPEC)? {
            if fe.take(&it)? {
                continue;
            }
            match it {
                Item::Opt("E", _) => dialect = Dialect::Extended,
                Item::Opt("F", _) => dialect = Dialect::Fixed,
                Item::Opt("G", _) => dialect = Dialect::Basic,
                Item::Opt("e", Some(p)) => patterns.push(p),
                Item::Opt("i", _) => icase = true,
                Item::Opt("no-i", _) => icase = false,
                Item::Opt("v", _) => g.invert = true,
                Item::Opt("w", _) => g.word = true,
                Item::Opt("x", _) => line = true,
                Item::Opt("c", _) => g.count = true,
                Item::Opt("o", _) => g.only = true,
                Item::Opt("q", _) => g.quiet = true,
                Item::Opt("l", _) => g.list = Some(true),
                Item::Opt("L", _) => g.list = Some(false),
                Item::Opt("H", _) => g.with_name = true,
                Item::Opt("h", _) => g.with_name = false,
                Item::Opt("n", _) => g.number = true,
                Item::Opt("m", Some(v)) => g.max = Some(number(&v, "max count")?),
                Item::Opt("A", Some(v)) => g.after = Some(context(&v)?),
                Item::Opt("B", Some(v)) => g.before = Some(context(&v)?),
                Item::Opt("C", Some(v)) | Item::Opt("NUM", Some(v)) => g.both = Some(context(&v)?),
                Item::Opt("color", v) => match v.as_deref() {
                    None | Some("never") | Some("auto") | Some("none") | Some("no") => {}
                    Some(w) => {
                        return Err(ToolError::unsupported(
                            format!("--color={w}"),
                            "output is never coloured",
                        ))
                    }
                },
                Item::Opt(..) => {}
                Item::Operand(o) => operands.push(o),
            }
        }
        if patterns.is_empty() {
            if operands.is_empty() {
                return Err(ToolError::usage("grep needs a pattern"));
            }
            patterns.push(operands.remove(0));
        }
        if let Some(o) = operands.first() {
            return Err(ToolError::unsupported(
                o.clone(),
                "one doc per call; the host passes the doc and greps several docs one by one",
            ));
        }
        let mut alts = Vec::new();
        let total: usize = patterns.iter().map(String::len).sum();
        if total > pattern::MAX_PATTERN_BYTES {
            return Err(ToolError::Limit(format!(
                "patterns are {total} bytes; at most {}",
                pattern::MAX_PATTERN_BYTES
            )));
        }
        for p in &patterns {
            for one in p.split('\n') {
                alts.push(format!(
                    "(?:{})",
                    pattern::translate(one, dialect, Flavor::Grep)?
                ));
            }
        }
        let mut re = alts.join("|");
        if line {
            re = format!("^(?:{re})$");
            g.word = false;
        }
        if g.only && (g.after.is_some() || g.before.is_some() || g.both.is_some()) {
            return Err(ToolError::unsupported(
                "-o with -A/-B/-C",
                "context lines with -o aren't supported",
            ));
        }
        let both = g.both.unwrap_or(0);
        Ok(Grep {
            matcher: Matcher::new(&re, icase, false)?,
            invert: g.invert,
            word: g.word,
            count: g.count,
            only: g.only,
            quiet: g.quiet,
            list: g.list,
            with_name: g.with_name,
            number: g.number,
            max: g.max,
            after: g.after.unwrap_or(both),
            before: g.before.unwrap_or(both),
            for_edit: fe.finish()?,
        })
    }

    fn matches(&self, s: &mut Search<'_>, line: &str) -> Result<bool, ToolError> {
        Ok(if self.word {
            s.find_word(line, 0)?.is_some()
        } else {
            s.find(line, 0).is_some()
        })
    }

    pub(crate) fn run(&self, doc: &ToolDoc) -> Result<ReadOutput, ToolError> {
        let ls = lines(doc.text);
        let mut s = self.matcher.search();
        let mut out = String::new();
        let mut selected: u64 = 0;
        let plain = !self.quiet && self.list.is_none() && !self.count;
        let ctx = plain && !self.only && (self.after > 0 || self.before > 0);
        let mut last_out: Option<usize> = None;
        let mut after_left = 0usize;
        let mut done = self.max == Some(0);
        // the selected lines' span (0-based), for notes and --for-edit
        let mut span: Option<(usize, usize)> = None;
        for (i, l) in ls.iter().enumerate() {
            if done {
                if ctx && after_left > 0 {
                    self.print(&mut out, doc.name, i, l.text, '-');
                    after_left -= 1;
                    continue;
                }
                break;
            }
            let sel = self.matches(&mut s, l.text)? != self.invert;
            if sel {
                selected += 1;
                span = Some(span.map_or((i, i), |(a, _)| (a, i)));
                if self.max == Some(selected) {
                    done = true;
                }
                if !plain {
                    if self.quiet || self.list.is_some() {
                        break;
                    }
                    continue;
                }
                if ctx {
                    let from = i
                        .saturating_sub(self.before)
                        .max(last_out.map_or(0, |o| o + 1));
                    if last_out.is_some_and(|o| from > o + 1) {
                        out.push_str("--\n");
                    }
                    for (j, lj) in ls.iter().enumerate().take(i).skip(from) {
                        self.print(&mut out, doc.name, j, lj.text, '-');
                    }
                    after_left = self.after;
                }
                last_out = Some(i);
                if self.only {
                    if !self.invert {
                        self.print_matches(&mut s, &mut out, doc.name, i, l)?;
                    }
                } else {
                    self.print(&mut out, doc.name, i, l.text, ':');
                }
            } else if ctx && after_left > 0 {
                self.print(&mut out, doc.name, i, l.text, '-');
                last_out = Some(i);
                after_left -= 1;
            }
            if out.len() > super::MAX_OUTPUT_BYTES {
                return Err(super::too_much_output());
            }
        }
        let exit = if selected > 0 { 0 } else { 1 };
        let touched = span.map(|(a, b)| (a + 1, b + 1));
        if self.quiet {
            let mut o = ReadOutput::new(Vec::new(), exit);
            o.touched = touched;
            return Ok(o);
        }
        if let Some(with) = self.list {
            if (selected > 0) == with {
                out.push_str(doc.name);
                out.push('\n');
            }
        } else if self.count {
            if self.with_name {
                out.push_str(doc.name);
                out.push(':');
            }
            out.push_str(&format!("{selected}\n"));
        }
        let mut o = ReadOutput::new(out.into_bytes(), exit);
        o.touched = touched;
        Ok(o)
    }

    fn prefix(&self, out: &mut String, name: &str, i: usize, sep: char) {
        if self.with_name {
            out.push_str(name);
            out.push(sep);
        }
        if self.number {
            out.push_str(&(i + 1).to_string());
            out.push(sep);
        }
    }

    fn print(&self, out: &mut String, name: &str, i: usize, text: &str, sep: char) {
        self.prefix(out, name, i, sep);
        out.push_str(text);
        out.push('\n');
    }

    fn print_matches(
        &self,
        s: &mut Search<'_>,
        out: &mut String,
        name: &str,
        i: usize,
        l: &Line<'_>,
    ) -> Result<(), ToolError> {
        let t = l.text;
        let mut from = 0;
        while from <= t.len() {
            let m = if self.word {
                s.find_word(t, from)?
            } else {
                s.find(t, from)
            };
            let Some(m) = m else { break };
            if m.is_empty() {
                // no output for an empty match; step one character and look again
                match t[m.start..].chars().next() {
                    Some(c) => from = m.start + c.len_utf8(),
                    None => break,
                }
                continue;
            }
            self.prefix(out, name, i, ':');
            out.push_str(&t[m.clone()]);
            out.push('\n');
            from = m.end;
        }
        Ok(())
    }
}

#[derive(Default)]
struct GrepFlags {
    invert: bool,
    word: bool,
    count: bool,
    only: bool,
    quiet: bool,
    list: Option<bool>,
    with_name: bool,
    number: bool,
    max: Option<u64>,
    after: Option<usize>,
    before: Option<usize>,
    both: Option<usize>,
}
