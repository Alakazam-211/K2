//! `cat`, `head`, `tail`, `wc`, `view`: the simple read verbs.
//!
//! Byte tools (`cat` without flags, `wc`, `head -c`, `tail -c`) see the file's bytes, as
//! the pack encodes them (BOM and CRLF restored). Line tools (`cat -n`/`-b`, `head -n`,
//! `tail -n`, `view`) see the doc text (`\n` line breaks, no BOM).

use super::args::{self, opt, Arity, Item, Opt, Spec};
use super::error::ToolError;
use super::{lines, no_operands, ForEditArg, ReadOutput, ToolDoc, FOR_EDIT, TTL};

#[derive(Debug, Clone)]
pub(crate) struct Cat {
    number: bool,
    nonblank: bool,
    pub(crate) for_edit: Option<Option<u64>>,
}

const CAT_OPTS: &[Opt] = &[
    opt("n", Some('n'), Some("number"), Arity::No),
    opt("b", Some('b'), Some("number-nonblank"), Arity::No),
    opt("u", Some('u'), None, Arity::No),
    FOR_EDIT,
    TTL,
];

const CAT: Spec = Spec {
    tool: "cat",
    opts: CAT_OPTS,
    unsupported: &[
        ("-A", "pipe cat's output through your own cat -A"),
        ("-E", "pipe cat's output through your own cat -E"),
        ("-T", "pipe cat's output through your own cat -T"),
        ("-v", "pipe cat's output through your own cat -v"),
        ("-s", "pipe cat's output through your own cat -s"),
    ],
    numeric: false,
    numeric_in_cluster: false,
};

impl Cat {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        let mut c = Cat {
            number: false,
            nonblank: false,
            for_edit: None,
        };
        let mut fe = ForEditArg::default();
        for it in args::parse(args, &CAT)? {
            if fe.take(&it)? {
                continue;
            }
            match it {
                Item::Opt("n", _) => c.number = true,
                Item::Opt("b", _) => c.nonblank = true,
                Item::Opt(..) => {}
                Item::Operand(o) => return Err(no_operands("cat", &o)),
            }
        }
        c.for_edit = fe.finish()?;
        Ok(c)
    }

    pub(crate) fn run(&self, doc: &ToolDoc) -> ReadOutput {
        // the whole doc is shown: those are the lines touched (no viewing range: P3)
        let n = lines(doc.text).len();
        let mut o = self.output(doc);
        o.touched = (n > 0).then_some((1, n));
        o
    }

    fn output(&self, doc: &ToolDoc) -> ReadOutput {
        if !self.number && !self.nonblank {
            return ReadOutput::new(doc.bytes.to_vec(), 0);
        }
        let mut out = String::with_capacity(doc.text.len() + doc.text.len() / 8);
        let mut n = 0usize;
        for l in lines(doc.text) {
            if self.nonblank && l.text.is_empty() {
                // `-b` overrides `-n`: blank lines get no number
            } else {
                n += 1;
                out.push_str(&format!("{n:>6}\t"));
            }
            out.push_str(l.text);
            if l.nl {
                out.push('\n');
            }
        }
        ReadOutput::new(out.into_bytes(), 0)
    }
}

/// `head -n N` / `-n -N` / `-c N` / `-c -N`.
#[derive(Debug, Clone, Copy)]
enum Amount {
    Lines { n: u64, all_but: bool },
    Bytes { n: u64, all_but: bool },
}

#[derive(Debug, Clone)]
pub(crate) struct Head {
    amount: Amount,
    pub(crate) for_edit: Option<Option<u64>>,
}

const HEAD_OPTS: &[Opt] = &[
    opt("n", Some('n'), Some("lines"), Arity::Req),
    opt("c", Some('c'), Some("bytes"), Arity::Req),
    opt("q", Some('q'), Some("quiet"), Arity::No),
    opt("q", None, Some("silent"), Arity::No),
    FOR_EDIT,
    TTL,
];

const HEAD: Spec = Spec {
    tool: "head",
    opts: HEAD_OPTS,
    unsupported: &[
        ("-v", "a single doc has no header to print"),
        ("--verbose", "a single doc has no header to print"),
        ("-z", "lines end with \\n"),
        ("--zero-terminated", "lines end with \\n"),
    ],
    numeric: true,
    numeric_in_cluster: false,
};

fn signed(v: &str, what: &str) -> Result<(u64, bool), ToolError> {
    match v.strip_prefix('-') {
        Some(rest) => Ok((args::count(rest, what)?, true)),
        None => Ok((args::count(v, what)?, false)),
    }
}

impl Head {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        let mut a = Amount::Lines {
            n: 10,
            all_but: false,
        };
        let mut fe = ForEditArg::default();
        for it in args::parse(args, &HEAD)? {
            if fe.take(&it)? {
                continue;
            }
            match it {
                Item::Opt("n", Some(v)) | Item::Opt("NUM", Some(v)) => {
                    let (n, all_but) = signed(&v, "lines")?;
                    a = Amount::Lines { n, all_but };
                }
                Item::Opt("c", Some(v)) => {
                    let (n, all_but) = signed(&v, "bytes")?;
                    a = Amount::Bytes { n, all_but };
                }
                Item::Opt(..) => {}
                Item::Operand(o) => return Err(no_operands("head", &o)),
            }
        }
        Ok(Head {
            amount: a,
            for_edit: fe.finish()?,
        })
    }

    pub(crate) fn run(&self, doc: &ToolDoc) -> ReadOutput {
        match self.amount {
            Amount::Bytes { n, all_but } => {
                let len = doc.bytes.len();
                let k = if all_but {
                    len.saturating_sub(clamp(n))
                } else {
                    len.min(clamp(n))
                };
                ReadOutput::new(doc.bytes[..k].to_vec(), 0)
            }
            Amount::Lines { n, all_but } => {
                let ls = lines(doc.text);
                let k = if all_but {
                    ls.len().saturating_sub(clamp(n))
                } else {
                    ls.len().min(clamp(n))
                };
                let end = ls.get(k).map_or(doc.text.len(), |l| l.start);
                ReadOutput::new(doc.text.as_bytes()[..end].to_vec(), 0)
                    .showing((k > 0).then_some((1, k)))
            }
        }
    }
}

fn clamp(n: u64) -> usize {
    usize::try_from(n).unwrap_or(usize::MAX)
}

/// `tail -n N` / `-n +N` / `-c N` / `-c +N`.
#[derive(Debug, Clone)]
pub(crate) struct Tail {
    bytes: bool,
    n: u64,
    from_start: bool,
    pub(crate) for_edit: Option<Option<u64>>,
}

const TAIL_OPTS: &[Opt] = &[
    opt("n", Some('n'), Some("lines"), Arity::Req),
    opt("c", Some('c'), Some("bytes"), Arity::Req),
    opt("q", Some('q'), Some("quiet"), Arity::No),
    opt("q", None, Some("silent"), Arity::No),
    FOR_EDIT,
    TTL,
];

const TAIL: Spec = Spec {
    tool: "tail",
    opts: TAIL_OPTS,
    unsupported: &[
        ("-f", "use the host's watch (tail -f) verb"),
        ("-F", "use the host's watch (tail -f) verb"),
        ("--follow", "use the host's watch (tail -f) verb"),
        ("--retry", "use the host's watch (tail -f) verb"),
        ("--pid", "use the host's watch (tail -f) verb"),
        ("-s", "use the host's watch (tail -f) verb"),
        ("-v", "a single doc has no header to print"),
        ("--verbose", "a single doc has no header to print"),
        ("-z", "lines end with \\n"),
        ("--zero-terminated", "lines end with \\n"),
    ],
    numeric: true,
    numeric_in_cluster: false,
};

impl Tail {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        let mut t = Tail {
            bytes: false,
            n: 10,
            from_start: false,
            for_edit: None,
        };
        let mut fe = ForEditArg::default();
        for it in args::parse(args, &TAIL)? {
            if fe.take(&it)? {
                continue;
            }
            match it {
                Item::Opt(id @ ("n" | "c" | "NUM"), Some(v)) => {
                    let what = if id == "c" { "bytes" } else { "lines" };
                    let (n, from_start) = match v.strip_prefix('+') {
                        Some(rest) => (args::count(rest, what)?, true),
                        None => (args::count(v.strip_prefix('-').unwrap_or(&v), what)?, false),
                    };
                    t = Tail {
                        bytes: id == "c",
                        n,
                        from_start,
                        for_edit: None,
                    };
                }
                Item::Opt(..) => {}
                Item::Operand(o) => return Err(no_operands("tail", &o)),
            }
        }
        t.for_edit = fe.finish()?;
        Ok(t)
    }

    pub(crate) fn run(&self, doc: &ToolDoc) -> ReadOutput {
        let n = clamp(self.n);
        if self.bytes {
            let len = doc.bytes.len();
            let from = if self.from_start {
                n.saturating_sub(1).min(len)
            } else {
                len.saturating_sub(n)
            };
            return ReadOutput::new(doc.bytes[from..].to_vec(), 0);
        }
        let ls = lines(doc.text);
        let first = if self.from_start {
            n.saturating_sub(1).min(ls.len())
        } else {
            ls.len().saturating_sub(n)
        };
        let start = ls.get(first).map_or(doc.text.len(), |l| l.start);
        ReadOutput::new(doc.text.as_bytes()[start..].to_vec(), 0)
            .showing((first < ls.len()).then_some((first + 1, ls.len())))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Wc {
    lines: bool,
    words: bool,
    chars: bool,
    bytes: bool,
}

const WC_OPTS: &[Opt] = &[
    opt("l", Some('l'), Some("lines"), Arity::No),
    opt("w", Some('w'), Some("words"), Arity::No),
    opt("c", Some('c'), Some("bytes"), Arity::No),
    opt("m", Some('m'), Some("chars"), Arity::No),
];

const WC: Spec = Spec {
    tool: "wc",
    opts: WC_OPTS,
    unsupported: &[
        ("-L", "maximum line width isn't supported"),
        ("--max-line-length", "maximum line width isn't supported"),
        ("--files0-from", "one doc per call"),
        ("--total", "one doc per call"),
    ],
    numeric: false,
    numeric_in_cluster: false,
};

impl Wc {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        let mut w = Wc {
            lines: false,
            words: false,
            chars: false,
            bytes: false,
        };
        for it in args::parse(args, &WC)? {
            match it {
                Item::Opt("l", _) => w.lines = true,
                Item::Opt("w", _) => w.words = true,
                Item::Opt("c", _) => w.bytes = true,
                Item::Opt("m", _) => w.chars = true,
                Item::Opt(..) => {}
                Item::Operand(o) => return Err(no_operands("wc", &o)),
            }
        }
        if !(w.lines || w.words || w.chars || w.bytes) {
            w.lines = true;
            w.words = true;
            w.bytes = true;
        }
        Ok(w)
    }

    /// As `wc [flags] NAME` on the file: the counts, then the name. One count is printed
    /// unpadded; several are right-aligned to the width of the file's byte count (GNU's
    /// rule for one regular file).
    pub(crate) fn run(&self, doc: &ToolDoc) -> ReadOutput {
        let b = doc.bytes;
        let text = String::from_utf8_lossy(b);
        let mut fields: Vec<usize> = Vec::new();
        if self.lines {
            fields.push(b.iter().filter(|&&c| c == b'\n').count());
        }
        if self.words {
            let mut n = 0;
            let mut in_word = false;
            for c in text.chars() {
                if c.is_whitespace() {
                    in_word = false;
                } else if !in_word {
                    in_word = true;
                    n += 1;
                }
            }
            fields.push(n);
        }
        if self.chars {
            fields.push(text.chars().count());
        }
        if self.bytes {
            fields.push(b.len());
        }
        let width = if fields.len() == 1 {
            1
        } else {
            b.len().to_string().len()
        };
        let mut out = String::new();
        for (i, f) in fields.iter().enumerate() {
            if i > 0 {
                out.push(' ');
            }
            out.push_str(&format!("{f:>width$}"));
        }
        if !doc.name.is_empty() {
            out.push(' ');
            out.push_str(doc.name);
        }
        out.push('\n');
        ReadOutput::new(out.into_bytes(), 0)
    }
}

/// `view --lines a,b`: those lines, and the caller's viewing range (P4).
#[derive(Debug, Clone)]
pub(crate) struct View {
    range: LineSpec,
    pub(crate) for_edit: Option<Option<u64>>,
}

/// `a,b`, `a` or `a,$` (1-based, inclusive; `$` is the last line).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LineSpec {
    pub first: LineNo,
    pub last: LineNo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LineNo {
    N(usize),
    Last,
}

impl LineNo {
    pub(crate) fn parse(s: &str) -> Result<Self, ToolError> {
        if s == "$" {
            return Ok(LineNo::Last);
        }
        let n: usize = s
            .parse()
            .map_err(|_| ToolError::usage(format!("invalid line number: '{s}'")))?;
        Ok(LineNo::N(n))
    }

    pub(crate) fn resolve(self, count: usize) -> usize {
        match self {
            LineNo::N(n) => n,
            LineNo::Last => count,
        }
    }
}

impl LineSpec {
    pub(crate) fn parse(s: &str) -> Result<Self, ToolError> {
        let (a, b) = s.split_once(',').unwrap_or((s, s));
        let spec = LineSpec {
            first: LineNo::parse(a.trim())?,
            last: LineNo::parse(b.trim())?,
        };
        if spec.first == LineNo::N(0) || spec.last == LineNo::N(0) {
            return Err(ToolError::usage("line numbers start at 1"));
        }
        Ok(spec)
    }
}

const VIEW_OPTS: &[Opt] = &[opt("lines", None, Some("lines"), Arity::Req), FOR_EDIT, TTL];

const VIEW: Spec = Spec {
    tool: "view",
    opts: VIEW_OPTS,
    unsupported: &[],
    numeric: false,
    numeric_in_cluster: false,
};

impl View {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        let mut range = None;
        let mut fe = ForEditArg::default();
        for it in args::parse(args, &VIEW)? {
            if fe.take(&it)? {
                continue;
            }
            match it {
                Item::Opt("lines", Some(v)) => range = Some(LineSpec::parse(&v)?),
                Item::Opt(..) => {}
                Item::Operand(o) => return Err(no_operands("view", &o)),
            }
        }
        let range = range.ok_or_else(|| ToolError::usage("view needs --lines a,b"))?;
        Ok(View {
            range,
            for_edit: fe.finish()?,
        })
    }

    pub(crate) fn run(&self, doc: &ToolDoc) -> ReadOutput {
        let ls = lines(doc.text);
        let a = self.range.first.resolve(ls.len()).max(1);
        let b = self.range.last.resolve(ls.len()).min(ls.len());
        if a > b {
            return ReadOutput::new(Vec::new(), 0);
        }
        let s = ls[a - 1].start;
        let e = ls.get(b).map_or(doc.text.len(), |l| l.start);
        ReadOutput::new(doc.text.as_bytes()[s..e].to_vec(), 0).showing(Some((a, b)))
    }
}
