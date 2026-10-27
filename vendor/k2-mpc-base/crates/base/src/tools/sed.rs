//! `sed`, a GNU-compatible subset: one pass over the doc's lines, no hold space.
//!
//! - Options: `-n` (`--quiet`, `--silent`), `-e SCRIPT` (`--expression`), `-E`/`-r`
//!   (`--regexp-extended`), `-s` (`--separate`), `--sandbox` (always on), `-i` /
//!   `--in-place` (GNU) and BSD `-i ''`. A backup suffix (`-i.bak`, `-i .bak`, `-in`) is
//!   refused: versions replace backups.
//! - Commands: `s///` (flags `g`, `N`, `i`/`I`, `p`; `&`, `\0`..`\9`, `\n`, `\t` in the
//!   replacement), `d`, `p`, `a`, `i`, `c` (GNU one-liners and the `a\` form), `y///`, `=`,
//!   `q`, and `#` comments (`#n` on the first line is `-n`).
//! - Addresses: `N`, `$`, `/re/`, `\cREc`, the `I` modifier, `a,b`, `a,+N`, and `!`.
//! - **Refused** (exit 2, nothing applied): `e`, `w`, `r`, `R`, `W` and the `s` flags `e`
//!   and `w` (they run commands or touch other files); backreferences in a pattern.
//! - Everything else (`n N D P h H g G x b t T : { } l z F v Q L`, `first~step`, `0,/re/`,
//!   `M`, case conversion `\U\L\u\l\E`, `q EXIT`) is unsupported (exit 2).
//!
//! Output follows GNU byte for byte, including a missing final newline: the last line is
//! printed without one when the input lacked it, and anything printed after it gets the
//! newline first. With `-i`, the output is the doc's new text (as GNU writes it to the
//! file); `q` therefore drops the rest, as it does in GNU.

use std::collections::HashMap;

use super::args::{self, opt, Arity, Item, Opt, Spec};
use super::error::ToolError;
use super::pattern::{self, Dialect, Flavor, Matcher, Search};
use super::read::{LineNo, LineSpec};
use super::{lines, too_much_output, MAX_OUTPUT_BYTES};

#[derive(Debug, Clone)]
pub(crate) struct Sed {
    cmds: Vec<Command>,
    regexes: Vec<Matcher>,
    quiet: bool,
    pub(crate) in_place: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addr {
    Line(usize),
    Last,
    /// Index into `regexes`; `None` is the empty regex (the last one used).
    Re(Option<usize>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Addr2 {
    At(Addr),
    Plus(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Repl {
    Lit(String),
    Group(usize),
}

#[derive(Debug, Clone)]
enum Cmd {
    Subst {
        re: Option<usize>,
        repl: Vec<Repl>,
        global: bool,
        nth: usize,
        print: bool,
    },
    Delete,
    Print,
    LineNo,
    Quit,
    Append(String),
    Insert(String),
    Change(String),
    Translit(HashMap<char, char>),
}

#[derive(Debug, Clone)]
struct Command {
    a1: Option<Addr>,
    a2: Option<Addr2>,
    negate: bool,
    cmd: Cmd,
}

const OPTS: &[Opt] = &[
    opt("n", Some('n'), Some("quiet"), Arity::No),
    opt("n", None, Some("silent"), Arity::No),
    opt("e", Some('e'), Some("expression"), Arity::Req),
    opt("E", Some('E'), Some("regexp-extended"), Arity::No),
    opt("E", Some('r'), None, Arity::No),
    opt("s", Some('s'), Some("separate"), Arity::No),
    opt("sandbox", None, Some("sandbox"), Arity::No),
    opt("i", Some('i'), Some("in-place"), Arity::Opt),
];

const SPEC: Spec = Spec {
    tool: "sed",
    opts: OPTS,
    unsupported: &[
        ("-f", "pass the script with -e"),
        ("--file", "pass the script with -e"),
        ("-z", "lines end with \\n"),
        ("--null-data", "lines end with \\n"),
        ("-u", "output is never buffered differently"),
        ("-l", "the l command isn't supported"),
        ("--posix", "GNU behaviour only"),
        ("--debug", "not supported"),
        ("--follow-symlinks", "the host resolves the doc"),
        ("-b", "lines end with \\n"),
    ],
    numeric: false,
    numeric_in_cluster: false,
};

fn backup_refused(suffix: &str) -> ToolError {
    ToolError::refused(
        format!("-i{suffix}"),
        "backup suffixes aren't supported: versions replace backups",
    )
}

impl Sed {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        // BSD `-i ''` (an empty suffix as its own argument) and `-i .bak`.
        let mut v: Vec<String> = Vec::with_capacity(args.len());
        let mut k = 0;
        while k < args.len() {
            let a = &args[k];
            v.push(a.clone());
            k += 1;
            if a == "-i" || a == "--in-place" {
                if let Some(next) = args.get(k) {
                    if next.is_empty() {
                        k += 1;
                    } else if next.starts_with('.') || next.starts_with('~') {
                        return Err(backup_refused(next));
                    }
                }
            }
        }
        let mut scripts: Vec<String> = Vec::new();
        let mut operands: Vec<String> = Vec::new();
        let mut quiet = false;
        let mut ere = false;
        let mut in_place = false;
        for it in args::parse(&v, &SPEC)? {
            match it {
                Item::Opt("n", _) => quiet = true,
                Item::Opt("e", Some(s)) => scripts.push(s),
                Item::Opt("E", _) => ere = true,
                Item::Opt("i", suffix) => {
                    if let Some(s) = suffix.filter(|s| !s.is_empty()) {
                        return Err(backup_refused(&s));
                    }
                    in_place = true;
                }
                Item::Opt(..) => {}
                Item::Operand(o) => operands.push(o),
            }
        }
        if scripts.is_empty() {
            if operands.is_empty() {
                return Err(ToolError::usage("sed needs a script"));
            }
            scripts.push(operands.remove(0));
        }
        if let Some(o) = operands.first() {
            return Err(ToolError::unsupported(
                o.clone(),
                "one doc per call; the host passes the doc",
            ));
        }
        let script = scripts.join("\n");
        if script.len() > pattern::MAX_PATTERN_BYTES {
            return Err(ToolError::Limit(format!(
                "script is {} bytes; at most {}",
                script.len(),
                pattern::MAX_PATTERN_BYTES
            )));
        }
        let mut p = Parser {
            cs: script.chars().collect(),
            i: 0,
            ere,
            regexes: Vec::new(),
        };
        if script.starts_with("#n") && matches!(p.cs.get(2), None | Some('\n')) {
            quiet = true;
        }
        let cmds = p.program()?;
        Ok(Sed {
            cmds,
            regexes: p.regexes,
            quiet,
            in_place,
        })
    }

    /// The viewing range for `sed -n 'a,bp'` / `sed -n 'Np'` with line numbers (P4).
    pub(crate) fn view(&self, count: usize) -> Option<(usize, usize)> {
        if !self.quiet || self.cmds.len() != 1 {
            return None;
        }
        let c = &self.cmds[0];
        if c.negate || !matches!(c.cmd, Cmd::Print) {
            return None;
        }
        let resolve = |a: Addr| match a {
            Addr::Line(n) => Some(LineNo::N(n)),
            Addr::Last => Some(LineNo::Last),
            Addr::Re(_) => None,
        };
        let first = resolve(c.a1?)?;
        let last = match c.a2 {
            None => first,
            Some(Addr2::At(a)) => resolve(a)?,
            Some(Addr2::Plus(k)) => LineNo::N(first.resolve(count).saturating_add(k)),
        };
        let spec = LineSpec { first, last };
        let a = spec.first.resolve(count);
        let b = spec.last.resolve(count).min(count);
        (a >= 1 && a <= b).then_some((a, b))
    }

    /// Run the script over `text`; returns the output (the new text with `-i`).
    pub(crate) fn run(&self, text: &str) -> Result<String, ToolError> {
        let ls = lines(text);
        let mut searches: Vec<Search<'_>> = self.regexes.iter().map(Matcher::search).collect();
        let mut st = vec![Range::default(); self.cmds.len()];
        let mut out = Out::default();
        let mut last_re: Option<usize> = None;
        let n = ls.len();
        'lines: for (idx, l) in ls.iter().enumerate() {
            let lineno = idx + 1;
            let is_last = idx + 1 == n;
            let mut ps = l.text.to_owned();
            let mut appends: Vec<&str> = Vec::new();
            let mut deleted = false;
            let mut quit = false;
            for (ci, c) in self.cmds.iter().enumerate() {
                let hit = self.address(
                    c,
                    &mut st[ci],
                    lineno,
                    is_last,
                    &ps,
                    &mut searches,
                    &mut last_re,
                )?;
                if hit == c.negate {
                    continue;
                }
                match &c.cmd {
                    Cmd::Subst {
                        re,
                        repl,
                        global,
                        nth,
                        print,
                    } => {
                        let r = resolve_re(*re, &mut last_re)?;
                        if let Some(new) = subst(
                            &ps,
                            &mut searches[r],
                            self.regexes[r].groups(),
                            repl,
                            *global,
                            *nth,
                        )? {
                            ps = new;
                            if *print {
                                out.line(&ps, l.nl || !is_last);
                            }
                        }
                    }
                    Cmd::Delete => {
                        deleted = true;
                        break;
                    }
                    Cmd::Print => out.line(&ps, l.nl || !is_last),
                    Cmd::LineNo => out.raw(&format!("{lineno}\n")),
                    Cmd::Quit => {
                        quit = true;
                        break;
                    }
                    Cmd::Append(t) => appends.push(t),
                    Cmd::Insert(t) => out.raw(t),
                    Cmd::Change(t) => {
                        if !st[ci].active {
                            out.raw(t);
                        }
                        deleted = true;
                        break;
                    }
                    Cmd::Translit(map) => {
                        ps = ps.chars().map(|ch| *map.get(&ch).unwrap_or(&ch)).collect();
                    }
                }
                if out.s.len() + ps.len() > MAX_OUTPUT_BYTES {
                    return Err(too_much_output());
                }
            }
            if !deleted && !self.quiet {
                out.line(&ps, l.nl || !is_last);
            }
            for t in appends {
                out.raw(t);
            }
            if out.s.len() > MAX_OUTPUT_BYTES {
                return Err(too_much_output());
            }
            if quit {
                break 'lines;
            }
        }
        Ok(out.s)
    }

    #[allow(clippy::too_many_arguments)]
    fn address(
        &self,
        c: &Command,
        r: &mut Range,
        lineno: usize,
        is_last: bool,
        ps: &str,
        searches: &mut [Search<'_>],
        last_re: &mut Option<usize>,
    ) -> Result<bool, ToolError> {
        let mut one = |a: Addr| -> Result<bool, ToolError> {
            Ok(match a {
                Addr::Line(n) => lineno == n,
                Addr::Last => is_last,
                Addr::Re(re) => {
                    let i = resolve_re(re, last_re)?;
                    searches[i].find(ps, 0).is_some()
                }
            })
        };
        let Some(a1) = c.a1 else { return Ok(true) };
        let Some(a2) = c.a2 else { return one(a1) };
        if r.active {
            let end = match a2 {
                Addr2::At(Addr::Line(n)) => Some(n),
                Addr2::Plus(_) => Some(r.end),
                Addr2::At(a) => {
                    if one(a)? {
                        r.active = false;
                    }
                    None
                }
            };
            if let Some(end) = end {
                if lineno >= end {
                    r.active = false;
                }
                return Ok(lineno <= end);
            }
            return Ok(true);
        }
        if !one(a1)? {
            return Ok(false);
        }
        match a2 {
            Addr2::At(Addr::Line(n)) if n <= lineno => return Ok(true),
            Addr2::Plus(0) => return Ok(true),
            Addr2::Plus(k) => r.end = lineno.saturating_add(k),
            _ => {}
        }
        r.active = true;
        Ok(true)
    }
}

fn resolve_re(re: Option<usize>, last: &mut Option<usize>) -> Result<usize, ToolError> {
    let i = match re {
        Some(i) => i,
        None => last.ok_or_else(|| ToolError::usage("no previous regular expression"))?,
    };
    *last = Some(i);
    Ok(i)
}

/// A range's state: `active` between its first and last line.
#[derive(Debug, Clone, Copy, Default)]
struct Range {
    active: bool,
    end: usize,
}

/// Output with GNU's missing-newline rule.
#[derive(Default)]
struct Out {
    s: String,
    missing: bool,
}

impl Out {
    fn flush(&mut self) {
        if self.missing {
            self.s.push('\n');
            self.missing = false;
        }
    }
    fn raw(&mut self, t: &str) {
        self.flush();
        self.s.push_str(t);
    }
    fn line(&mut self, ps: &str, nl: bool) {
        self.flush();
        self.s.push_str(ps);
        if nl {
            self.s.push('\n');
        } else {
            self.missing = true;
        }
    }
}

/// `s///` on the pattern space; `None` when nothing was replaced.
fn subst(
    ps: &str,
    s: &mut Search<'_>,
    groups: usize,
    repl: &[Repl],
    global: bool,
    nth: usize,
) -> Result<Option<String>, ToolError> {
    for r in repl {
        if let Repl::Group(g) = r {
            if *g >= groups {
                return Err(ToolError::usage(format!(
                    "invalid reference \\{g} on `s' command's RHS"
                )));
            }
        }
    }
    let len = ps.len();
    let mut out = String::with_capacity(len);
    let mut start = 0;
    let mut count = 0usize;
    let mut replaced = false;
    let mut prev_end: Option<usize> = None;
    while start <= len {
        let Some(m) = s.find(ps, start) else { break };
        if m.is_empty() && prev_end == Some(m.start) {
            // an empty match right after the previous match doesn't count
            let Some(c) = ps[m.start..].chars().next() else {
                break;
            };
            out.push_str(&ps[start..m.start + c.len_utf8()]);
            start = m.start + c.len_utf8();
            continue;
        }
        count += 1;
        out.push_str(&ps[start..m.start]);
        if count < nth {
            out.push_str(&ps[m.clone()]);
        } else {
            replaced = true;
            for r in repl {
                match r {
                    Repl::Lit(t) => out.push_str(t),
                    Repl::Group(0) => out.push_str(&ps[m.clone()]),
                    Repl::Group(g) => {
                        if let Some(sp) = s.group(*g) {
                            out.push_str(&ps[sp]);
                        }
                    }
                }
            }
            if out.len() > MAX_OUTPUT_BYTES {
                return Err(too_much_output());
            }
        }
        prev_end = Some(m.end);
        start = m.end;
        if m.is_empty() {
            match ps[start..].chars().next() {
                Some(c) => {
                    out.push_str(&ps[start..start + c.len_utf8()]);
                    start += c.len_utf8();
                }
                None => {
                    start = len + 1;
                }
            }
        }
        if replaced && !global {
            break;
        }
    }
    if !replaced {
        return Ok(None);
    }
    if start <= len {
        out.push_str(&ps[start..]);
    }
    Ok(Some(out))
}

struct Parser {
    cs: Vec<char>,
    i: usize,
    ere: bool,
    regexes: Vec<Matcher>,
}

fn unknown_cmd(c: char) -> ToolError {
    match c {
        'e' | 'w' | 'r' | 'R' | 'W' => ToolError::refused(
            format!("sed command `{c}'"),
            "commands that run programs or read or write other files are refused",
        ),
        'n' | 'N' | 'D' | 'P' | 'h' | 'H' | 'g' | 'G' | 'x' | 'b' | 't' | 'T' | ':' | '{'
        | '}' | 'l' | 'z' | 'F' | 'v' | 'Q' | 'L' => ToolError::unsupported(
            format!("sed command `{c}'"),
            "supported: s d p a i c y = q (pipe through your own sed for the rest, then write --base)",
        ),
        c => ToolError::usage(format!("unknown command: `{c}'")),
    }
}

impl Parser {
    fn peek(&self) -> Option<char> {
        self.cs.get(self.i).copied()
    }
    fn next(&mut self) -> Option<char> {
        let c = self.peek();
        if c.is_some() {
            self.i += 1;
        }
        c
    }
    fn blanks(&mut self) {
        while matches!(self.peek(), Some(' ') | Some('\t')) {
            self.i += 1;
        }
    }

    fn program(&mut self) -> Result<Vec<Command>, ToolError> {
        let mut cmds = Vec::new();
        loop {
            while matches!(self.peek(), Some(c) if c.is_whitespace() || c == ';') {
                self.i += 1;
            }
            let Some(c) = self.peek() else { break };
            if c == '#' {
                while !matches!(self.next(), None | Some('\n')) {}
                continue;
            }
            cmds.push(self.command()?);
        }
        Ok(cmds)
    }

    fn command(&mut self) -> Result<Command, ToolError> {
        let a1 = self.address()?;
        let mut a2 = None;
        if a1.is_some() {
            self.blanks();
            if self.peek() == Some(',') {
                self.i += 1;
                self.blanks();
                if self.peek() == Some('+') || self.peek() == Some('~') {
                    let op = self.next().expect("peeked");
                    let n = self
                        .number()
                        .ok_or_else(|| ToolError::usage("expected a number after `+'"))?;
                    if op == '~' {
                        return Err(ToolError::unsupported("addr1,~N", "use addr1,+N or a,b"));
                    }
                    a2 = Some(Addr2::Plus(n));
                } else {
                    let a = self
                        .address()?
                        .ok_or_else(|| ToolError::usage("unexpected `,'"))?;
                    if a == Addr::Line(0) {
                        return Err(ToolError::usage("invalid usage of line address 0"));
                    }
                    a2 = Some(Addr2::At(a));
                }
            }
        }
        if a1 == Some(Addr::Line(0)) {
            if matches!(a2, Some(Addr2::At(Addr::Re(_)))) {
                return Err(ToolError::unsupported("0,/re/", "use 1,/re/"));
            }
            return Err(ToolError::usage("invalid usage of line address 0"));
        }
        self.blanks();
        let mut negate = false;
        if self.peek() == Some('!') {
            self.i += 1;
            negate = true;
            self.blanks();
            if self.peek() == Some('!') {
                return Err(ToolError::usage("multiple `!'s"));
            }
        }
        let Some(c) = self.next() else {
            return Err(ToolError::usage("missing command"));
        };
        let cmd = match c {
            'd' => Cmd::Delete,
            'p' => Cmd::Print,
            '=' => Cmd::LineNo,
            'q' => {
                if a2.is_some() {
                    return Err(ToolError::usage("command only uses one address"));
                }
                self.blanks();
                if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    return Err(ToolError::unsupported(
                        "q EXIT-CODE",
                        "exit codes are the host's",
                    ));
                }
                Cmd::Quit
            }
            'a' | 'i' | 'c' => {
                let t = self.text()?;
                match c {
                    'a' => Cmd::Append(t),
                    'i' => Cmd::Insert(t),
                    _ => Cmd::Change(t),
                }
            }
            's' => self.subst()?,
            'y' => self.translit()?,
            c => return Err(unknown_cmd(c)),
        };
        if !matches!(cmd, Cmd::Append(_) | Cmd::Insert(_) | Cmd::Change(_)) {
            self.end_of_cmd()?;
        }
        Ok(Command {
            a1,
            a2,
            negate,
            cmd,
        })
    }

    fn end_of_cmd(&mut self) -> Result<(), ToolError> {
        self.blanks();
        match self.peek() {
            None | Some('\n') | Some(';') | Some('#') => Ok(()),
            Some('}') => Err(unknown_cmd('}')),
            Some(_) => Err(ToolError::usage("extra characters after command")),
        }
    }

    fn number(&mut self) -> Option<usize> {
        let s = self.i;
        let mut n: usize = 0;
        while let Some(c) = self.peek().filter(char::is_ascii_digit) {
            n = n
                .saturating_mul(10)
                .saturating_add(c as usize - '0' as usize);
            self.i += 1;
        }
        (self.i > s).then_some(n)
    }

    fn address(&mut self) -> Result<Option<Addr>, ToolError> {
        match self.peek() {
            Some(c) if c.is_ascii_digit() => {
                let n = self.number().expect("digit");
                if self.peek() == Some('~') {
                    return Err(ToolError::unsupported("first~step", "use a,b ranges"));
                }
                Ok(Some(Addr::Line(n)))
            }
            Some('$') => {
                self.i += 1;
                Ok(Some(Addr::Last))
            }
            Some('/') | Some('\\') => {
                let mut delim = self.next().expect("peeked");
                if delim == '\\' {
                    delim = self
                        .next()
                        .ok_or_else(|| ToolError::usage("unexpected end of script"))?;
                    if delim == '\n' || delim == '\\' {
                        return Err(ToolError::usage("delimiter is not a single-byte character"));
                    }
                }
                let body = self.delimited(delim, Part::Regex, "unterminated address regex")?;
                let mut icase = false;
                loop {
                    match self.peek() {
                        Some('I') => {
                            icase = true;
                            self.i += 1;
                        }
                        Some('M') => {
                            return Err(ToolError::unsupported("/re/M", "multi-line mode"))
                        }
                        _ => break,
                    }
                }
                Ok(Some(Addr::Re(self.regex(&body, icase)?)))
            }
            _ => Ok(None),
        }
    }

    fn regex(&mut self, body: &str, icase: bool) -> Result<Option<usize>, ToolError> {
        if body.is_empty() {
            if icase {
                return Err(ToolError::usage(
                    "no previous regular expression (modifiers on an empty regex)",
                ));
            }
            return Ok(None);
        }
        let d = if self.ere {
            Dialect::Extended
        } else {
            Dialect::Basic
        };
        let t = pattern::translate(body, d, Flavor::Sed)?;
        self.regexes.push(Matcher::new(&t, icase, true)?);
        Ok(Some(self.regexes.len() - 1))
    }

    /// Text up to an unescaped `delim`, GNU's `match_slash`: `\delim` is the delimiter
    /// itself (no backslash), `\` newline is a newline, other escapes are kept for the
    /// regex or replacement parser.
    fn delimited(&mut self, delim: char, part: Part, err: &str) -> Result<String, ToolError> {
        let mut out = String::new();
        loop {
            let Some(c) = self.next() else {
                return Err(ToolError::usage(err.to_owned()));
            };
            if c == delim {
                return Ok(out);
            }
            match c {
                '\\' => {
                    let Some(e) = self.next() else {
                        return Err(ToolError::usage(err.to_owned()));
                    };
                    if e == '\n' {
                        out.push('\n');
                    } else if e == delim && !(part == Part::Repl && e == '&') {
                        if part == Part::Regex && delim != '/' && "\\.*[]^$+?(){}|".contains(e) {
                            // GNU's handling of an escaped special delimiter has changed
                            // across versions; refuse rather than guess.
                            return Err(ToolError::unsupported(
                                format!("\\{e} with delimiter {e}"),
                                "use a delimiter that isn't a regex character",
                            ));
                        }
                        out.push(e);
                    } else {
                        out.push('\\');
                        out.push(e);
                    }
                }
                '\n' => return Err(ToolError::usage(err.to_owned())),
                c => out.push(c),
            }
        }
    }

    fn subst(&mut self) -> Result<Cmd, ToolError> {
        let delim = self
            .next()
            .ok_or_else(|| ToolError::usage("unterminated `s' command"))?;
        if delim == '\n' || delim == '\\' {
            return Err(ToolError::usage("unterminated `s' command"));
        }
        let re_src = self.delimited(delim, Part::Regex, "unterminated `s' command")?;
        let repl_src = self.delimited(delim, Part::Repl, "unterminated `s' command")?;
        let mut global = false;
        let mut print = false;
        let mut nth: Option<usize> = None;
        let mut icase = false;
        loop {
            match self.peek() {
                Some('g') => {
                    if global {
                        return Err(ToolError::usage("multiple `g' options to `s' command"));
                    }
                    global = true;
                    self.i += 1;
                }
                Some('p') => {
                    if print {
                        return Err(ToolError::usage("multiple `p' options to `s' command"));
                    }
                    print = true;
                    self.i += 1;
                }
                Some('i') | Some('I') => {
                    icase = true;
                    self.i += 1;
                }
                Some(c) if c.is_ascii_digit() => {
                    if nth.is_some() {
                        return Err(ToolError::usage("multiple number options to `s' command"));
                    }
                    let n = self.number().expect("digit");
                    if n == 0 {
                        return Err(ToolError::usage(
                            "number option to `s' command may not be zero",
                        ));
                    }
                    nth = Some(n);
                }
                Some('m') | Some('M') => {
                    return Err(ToolError::unsupported("s///M", "multi-line mode"))
                }
                Some(c @ ('e' | 'w')) => {
                    return Err(ToolError::refused(
                        format!("s///{c}"),
                        "flags that run programs or write other files are refused",
                    ))
                }
                _ => break,
            }
        }
        let re = self.regex(&re_src, icase)?;
        let repl = replacement(&repl_src)?;
        if let Some(i) = re {
            let groups = self.regexes[i].groups();
            for r in &repl {
                if let Repl::Group(g) = r {
                    if *g >= groups {
                        return Err(ToolError::usage(format!(
                            "invalid reference \\{g} on `s' command's RHS"
                        )));
                    }
                }
            }
        }
        Ok(Cmd::Subst {
            re,
            repl,
            global,
            nth: nth.unwrap_or(1),
            print,
        })
    }

    fn translit(&mut self) -> Result<Cmd, ToolError> {
        let delim = self
            .next()
            .ok_or_else(|| ToolError::usage("unterminated `y' command"))?;
        if delim == '\n' || delim == '\\' {
            return Err(ToolError::usage("unterminated `y' command"));
        }
        let a = self.delimited(delim, Part::Translit, "unterminated `y' command")?;
        let b = self.delimited(delim, Part::Translit, "unterminated `y' command")?;
        let (a, b) = (y_chars(&a)?, y_chars(&b)?);
        if a.len() != b.len() {
            return Err(ToolError::usage(
                "strings for `y' command are different lengths",
            ));
        }
        let mut map = HashMap::new();
        for (x, y) in a.into_iter().zip(b) {
            map.entry(x).or_insert(y);
        }
        Ok(Cmd::Translit(map))
    }

    /// The text of `a`, `i` or `c` (GNU's `read_text`), with a trailing newline. `a\` at the
    /// very end of the script is empty text (the `$a\` idiom only adds a missing newline).
    fn text(&mut self) -> Result<String, ToolError> {
        self.blanks();
        let mut raw = String::new();
        match self.peek() {
            None => return Err(ToolError::usage("expected \\ after `a', `c' or `i'")),
            Some('\\') => {
                self.i += 1;
                match self.next() {
                    None => return Ok(String::new()),
                    Some('\n') => {}
                    Some(c) => raw.push(c),
                }
            }
            Some(_) => {}
        }
        loop {
            match self.next() {
                None | Some('\n') => break,
                Some('\\') => match self.next() {
                    None => break,
                    Some(e) => raw.push(text_escape(e)?),
                },
                Some(c) => raw.push(c),
            }
        }
        raw.push('\n');
        Ok(raw)
    }
}

/// An escape in `a`/`i`/`c` text (GNU converts these; any other escaped character is
/// itself).
fn text_escape(e: char) -> Result<char, ToolError> {
    Ok(match e {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        'f' => '\x0c',
        'v' => '\x0b',
        'a' => '\x07',
        'c' | 'd' | 'o' | 'x' => {
            return Err(ToolError::unsupported(
                format!("\\{e}"),
                "character codes in a, i, c text",
            ))
        }
        e => e,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Regex,
    Repl,
    Translit,
}

fn y_chars(s: &str) -> Result<Vec<char>, ToolError> {
    let mut out = Vec::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('\\') => out.push('\\'),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(e) => {
                    return Err(ToolError::unsupported(
                        format!("\\{e} in y"),
                        "y takes \\\\, \\n, \\t and the escaped delimiter",
                    ))
                }
                None => return Err(ToolError::usage("unterminated `y' command")),
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

fn replacement(s: &str) -> Result<Vec<Repl>, ToolError> {
    let mut out: Vec<Repl> = Vec::new();
    let mut lit = String::new();
    let mut it = s.chars();
    let flush = |lit: &mut String, out: &mut Vec<Repl>| {
        if !lit.is_empty() {
            out.push(Repl::Lit(std::mem::take(lit)));
        }
    };
    while let Some(c) = it.next() {
        match c {
            '&' => {
                flush(&mut lit, &mut out);
                out.push(Repl::Group(0));
            }
            '\\' => {
                let Some(e) = it.next() else {
                    return Err(ToolError::usage("unterminated `s' command"));
                };
                match e {
                    '0'..='9' => {
                        flush(&mut lit, &mut out);
                        out.push(Repl::Group(e as usize - '0' as usize));
                    }
                    'n' | '\n' => lit.push('\n'),
                    't' => lit.push('\t'),
                    'r' => lit.push('\r'),
                    'f' => lit.push('\x0c'),
                    'v' => lit.push('\x0b'),
                    'a' => lit.push('\x07'),
                    'L' | 'U' | 'l' | 'u' | 'E' => {
                        return Err(ToolError::unsupported(
                            format!("\\{e}"),
                            "case conversion in a replacement isn't supported",
                        ))
                    }
                    e if e.is_ascii_alphanumeric() => {
                        return Err(ToolError::unsupported(
                            format!("\\{e}"),
                            "replacement escapes: & \\0-\\9 \\n \\t \\r \\f \\v \\a and escaped punctuation",
                        ))
                    }
                    e => lit.push(e),
                }
            }
            c => lit.push(c),
        }
    }
    flush(&mut lit, &mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sed(args: &[&str], text: &str) -> Result<String, ToolError> {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        Sed::parse(&a)?.run(text)
    }

    #[test]
    fn basics() {
        assert_eq!(sed(&["s/a/b/"], "aa\na\n").unwrap(), "ba\nb\n");
        assert_eq!(sed(&["s/a/b/g"], "aa\n").unwrap(), "bb\n");
        assert_eq!(sed(&["s/a/b/2"], "aaa\n").unwrap(), "aba\n");
        assert_eq!(sed(&["s/a/b/2g"], "aaaa\n").unwrap(), "abbb\n");
        assert_eq!(sed(&["s/x*/-/g"], "abc\n").unwrap(), "-a-b-c-\n");
        assert_eq!(sed(&["s/a*/x/g"], "baaac\n").unwrap(), "xbxcx\n");
        assert_eq!(sed(&["-n", "2p"], "a\nb\nc\n").unwrap(), "b\n");
        assert_eq!(sed(&["p"], "a").unwrap(), "a\na");
        assert_eq!(sed(&["$a\\"], "a").unwrap(), "a\n");
        assert_eq!(sed(&["$a\\"], "a\n").unwrap(), "a\n");
        assert_eq!(sed(&["2q"], "a\nb\nc\n").unwrap(), "a\nb\n");
        assert_eq!(
            sed(&["-e", "2,3c\\", "-e", "X"], "a\nb\nc\nd\n").unwrap(),
            "a\nX\nd\n"
        );
        assert_eq!(
            sed(&["-e", "2,3!c X"], "a\nb\nc\nd\n").unwrap(),
            "X\nb\nc\nX\n"
        );
        assert_eq!(sed(&["y/abc/xyz/"], "aabbcc\n").unwrap(), "xxyyzz\n");
        assert_eq!(
            sed(&["-E", "s/(a|ab)(c|bcd)/[\\1][\\2]/"], "abcd\n").unwrap(),
            "[a][bcd]\n"
        );
        assert_eq!(sed(&["s/a\\|ab/X/"], "abc\n").unwrap(), "Xc\n");
        assert_eq!(
            sed(&["s/\\(b\\)\\(c\\)/\\2\\1/"], "abcd\n").unwrap(),
            "acbd\n"
        );
        assert_eq!(sed(&["/b/,/c/d"], "a\nb\nx\nc\nd\n").unwrap(), "a\nd\n");
        assert_eq!(sed(&["2,+1d"], "a\nb\nc\nd\n").unwrap(), "a\nd\n");
        assert_eq!(sed(&["3,1d"], "a\nb\nc\nd\n").unwrap(), "a\nb\nd\n");
        assert_eq!(sed(&["="], "a\nb").unwrap(), "1\na\n2\nb");
        assert_eq!(sed(&["s/a/\\n/"], "xay\n").unwrap(), "x\ny\n");
        assert_eq!(sed(&["#n\n1p"], "a\nb\n").unwrap(), "a\n");
        assert_eq!(sed(&["s/b//;s//X/"], "abcb\n").unwrap(), "acX\n");
        assert_eq!(
            sed(&["-e", "1i\\", "-e", "top"], "a\n").unwrap(),
            "top\na\n"
        );
        assert_eq!(sed(&["1a foo\tbar"], "a\n").unwrap(), "a\nfoo\tbar\n");
    }

    #[test]
    fn refusals() {
        for s in [
            "e ls",
            "w out",
            "r f",
            "R f",
            "W f",
            "s/a/b/w f",
            "s/a/b/e",
            "s/\\(a\\)\\1/x/",
        ] {
            assert!(
                matches!(sed(&[s], "a\n"), Err(ToolError::Refused { .. })),
                "{s}"
            );
        }
        for a in [
            &["-i.bak", "p"][..],
            &["-i", ".bak", "p"],
            &["-in", "p"],
            &["--in-place=~", "p"],
        ] {
            assert!(
                matches!(sed(a, "a\n"), Err(ToolError::Refused { .. })),
                "{a:?}"
            );
        }
        for s in [
            "n",
            "N",
            "h",
            "x",
            "G",
            "b end",
            "{p}",
            "1~2p",
            "0,/a/p",
            "l",
            "s/a/\\U&/",
            "q5",
        ] {
            assert!(
                matches!(sed(&[s], "a\n"), Err(ToolError::Unsupported { .. })),
                "{s}"
            );
        }
        for s in [
            "s/a/b", "k", "s/a/b/gg", "1,p", "0p", "s/a/\\1/", "y/ab/c/", "p x",
        ] {
            assert!(matches!(sed(&[s], "a\n"), Err(ToolError::Usage(_))), "{s}");
        }
        assert!(
            Sed::parse(&["-i".into(), "".into(), "s/a/b/".into()])
                .unwrap()
                .in_place
        );
    }
}
