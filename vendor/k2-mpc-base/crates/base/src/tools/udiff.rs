//! Unified diffs: `diff -u` (making them) and `patch` / `apply` (applying them).
//!
//! **`diff`** compares two texts: the old one comes from the host (a version, the file on
//! disk) through stdin, the new one is the live doc. Flags: `-u`, `-U N`, `--unified[=N]`,
//! `--label L` (twice: old, then new). Exit 0 same, 1 different. The line diff is the kit's
//! deterministic Myers (`text/myers.rs`): minimal, so it agrees with GNU diff whenever the
//! minimal diff is unique; where several are minimal, the hunks may sit differently.
//!
//! **`patch`** applies one file's unified diff from stdin, like GNU patch: each hunk is
//! looked for at its line plus the offset of the hunks before it, then at growing offsets
//! either way; then with fuzz (ignoring up to `-F N` lines of leading and trailing context,
//! default 2, never more than the hunk has). A hunk with less leading than trailing
//! context must sit at the start of the file (and the reverse at its end). Lines compare
//! exactly, final newline included (`\ No newline at end of file`). Context lines keep the
//! live text. Flags: `-p N` (accepted; the doc is the target), `-R`, `--dry-run`
//! (`--check`), `-F N`, `--partial`, `-s`, `-u`, `--no-backup-if-mismatch`.
//!
//! **All or nothing by default** (unlike GNU patch): if any hunk fails nothing is applied,
//! exit 1, and each failing hunk is printed with the live text where it was expected.
//! `--partial` applies the hunks that fit and prints the rejects (GNU's `.rej` content) on
//! stdout, exit 1. Messages follow GNU's (`patching file NAME`, `Hunk #2 succeeded at 14
//! (offset 2 lines).`, `Hunk #3 FAILED at 20.`).

use super::args::{self, opt, Arity, Item, Opt, Spec};
use super::error::ToolError;
use super::{lines, no_operands, too_much_output, Line, MAX_OUTPUT_BYTES};
use crate::text::myers::{self, Budget};

// ---------- diff ----------

#[derive(Debug, Clone)]
pub(crate) struct DiffCmd {
    context: usize,
    from: Option<String>,
    to: Option<String>,
}

const DIFF_OPTS: &[Opt] = &[
    opt("u", Some('u'), None, Arity::No),
    opt("U", Some('U'), None, Arity::Req),
    opt("unified", None, Some("unified"), Arity::Opt),
    opt("label", None, Some("label"), Arity::Req),
];

const DIFF: Spec = Spec {
    tool: "diff",
    opts: DIFF_OPTS,
    unsupported: &[
        ("-c", "unified diffs only (-u)"),
        ("-C", "unified diffs only (-u)"),
        ("-e", "unified diffs only (-u)"),
        ("-y", "unified diffs only (-u)"),
        ("-q", "the exit status says whether they differ"),
        ("--brief", "the exit status says whether they differ"),
        ("-r", "one doc per call"),
        ("-N", "one doc per call"),
        ("-w", "whitespace is compared exactly"),
        ("-b", "whitespace is compared exactly"),
        ("-B", "blank lines are compared exactly"),
        ("-i", "case is compared exactly"),
    ],
    numeric: false,
    numeric_in_cluster: false,
};

fn small_number(v: &str, what: &str) -> Result<usize, ToolError> {
    if v.is_empty() || !v.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ToolError::usage(format!("invalid {what}: '{v}'")));
    }
    Ok(v.parse::<usize>().unwrap_or(usize::MAX))
}

impl DiffCmd {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        let mut d = DiffCmd {
            context: 3,
            from: None,
            to: None,
        };
        let mut labels = Vec::new();
        for it in args::parse(args, &DIFF)? {
            match it {
                Item::Opt("U", Some(v)) | Item::Opt("unified", Some(v)) => {
                    d.context = small_number(&v, "context length")?
                }
                Item::Opt("label", Some(v)) => labels.push(v),
                Item::Opt(..) => {}
                Item::Operand(o) => return Err(no_operands("diff", &o)),
            }
        }
        if labels.len() > 2 {
            return Err(ToolError::usage("too many --label options"));
        }
        let mut it = labels.into_iter();
        d.from = it.next();
        d.to = it.next();
        Ok(d)
    }

    pub(crate) fn run(&self, name: &str, old: &str, new: &str) -> (Vec<u8>, u8) {
        let from = self.from.as_deref().unwrap_or(name);
        let to = self.to.as_deref().unwrap_or(name);
        let out = unified(old, new, self.context, from, to);
        let exit = u8::from(!out.is_empty());
        (out.into_bytes(), exit)
    }
}

fn range(start: usize, count: usize) -> String {
    // An empty range names the line before it; a one-line range has no count.
    match count {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        n => format!("{},{n}", start + 1),
    }
}

fn push_line(out: &mut String, tag: char, l: &str) {
    out.push(tag);
    match l.strip_suffix('\n') {
        Some(body) => {
            out.push_str(body);
            out.push('\n');
        }
        None => {
            out.push_str(l);
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
}

/// `diff -u` of two texts (empty when they are equal).
pub fn unified(old: &str, new: &str, context: usize, from: &str, to: &str) -> String {
    if old == new {
        return String::new();
    }
    let a: Vec<&str> = old.split_inclusive('\n').collect();
    let b: Vec<&str> = new.split_inclusive('\n').collect();
    let changes = myers::diff(&a, &b, &mut Budget::for_input(old.len() + new.len()));
    let mut out = format!("--- {from}\n+++ {to}\n");
    let mut i = 0;
    while i < changes.len() {
        // group changes whose gaps are at most 2 * context lines
        let mut j = i;
        while j + 1 < changes.len() && changes[j + 1].old.start - changes[j].old.end <= 2 * context
        {
            j += 1;
        }
        let (first, last) = (&changes[i], &changes[j]);
        let olo = first.old.start.saturating_sub(context);
        let ohi = (last.old.end + context).min(a.len());
        let nlo = first.new.start - (first.old.start - olo);
        let nhi = last.new.end + (ohi - last.old.end);
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(olo, ohi - olo),
            range(nlo, nhi - nlo)
        ));
        let mut o = olo;
        for c in &changes[i..=j] {
            for l in &a[o..c.old.start] {
                push_line(&mut out, ' ', l);
            }
            for l in &a[c.old.clone()] {
                push_line(&mut out, '-', l);
            }
            for l in &b[c.new.clone()] {
                push_line(&mut out, '+', l);
            }
            o = c.old.end;
        }
        for l in &a[o..ohi] {
            push_line(&mut out, ' ', l);
        }
        i = j + 1;
    }
    out
}

// ---------- patch ----------

#[derive(Debug, Clone)]
pub(crate) struct PatchCmd {
    reverse: bool,
    pub(crate) dry_run: bool,
    fuzz: usize,
    partial: bool,
    silent: bool,
}

const PATCH_OPTS: &[Opt] = &[
    opt("p", Some('p'), Some("strip"), Arity::Req),
    opt("R", Some('R'), Some("reverse"), Arity::No),
    opt("dry", None, Some("dry-run"), Arity::No),
    opt("dry", None, Some("check"), Arity::No),
    opt("F", Some('F'), Some("fuzz"), Arity::Req),
    opt("partial", None, Some("partial"), Arity::No),
    opt("s", Some('s'), Some("silent"), Arity::No),
    opt("s", None, Some("quiet"), Arity::No),
    opt("u", Some('u'), Some("unified"), Arity::No),
    opt("nb", None, Some("no-backup-if-mismatch"), Arity::No),
];

const PATCH: Spec = Spec {
    tool: "patch",
    opts: PATCH_OPTS,
    unsupported: &[
        ("-b", "backups aren't supported: versions replace backups"),
        (
            "--backup",
            "backups aren't supported: versions replace backups",
        ),
        ("-B", "backups aren't supported: versions replace backups"),
        ("-V", "backups aren't supported: versions replace backups"),
        ("-i", "the patch comes on stdin"),
        ("--input", "the patch comes on stdin"),
        ("-o", "the doc is patched in place"),
        ("--output", "the doc is patched in place"),
        ("-r", "rejects are printed (--partial)"),
        ("--reject-file", "rejects are printed (--partial)"),
        ("-N", "already-applied detection isn't supported"),
        ("--forward", "already-applied detection isn't supported"),
        ("-c", "unified diffs only"),
        ("-n", "unified diffs only"),
        ("-e", "unified diffs only"),
        ("-l", "whitespace is compared exactly"),
        ("--ignore-whitespace", "whitespace is compared exactly"),
        ("-E", "the doc is never removed"),
        ("-d", "the host picks the doc"),
        ("--directory", "the host picks the doc"),
    ],
    numeric: false,
    numeric_in_cluster: false,
};

impl PatchCmd {
    pub(crate) fn parse(args: &[String]) -> Result<Self, ToolError> {
        let mut p = PatchCmd {
            reverse: false,
            dry_run: false,
            fuzz: 2,
            partial: false,
            silent: false,
        };
        for it in args::parse(args, &PATCH)? {
            match it {
                Item::Opt("p", Some(v)) => {
                    small_number(&v, "strip count")?;
                }
                Item::Opt("R", _) => p.reverse = true,
                Item::Opt("dry", _) => p.dry_run = true,
                Item::Opt("F", Some(v)) => p.fuzz = small_number(&v, "fuzz factor")?,
                Item::Opt("partial", _) => p.partial = true,
                Item::Opt("s", _) => p.silent = true,
                Item::Opt(..) => {}
                Item::Operand(o) => {
                    return Err(ToolError::unsupported(
                        o,
                        "the patch comes on stdin and the host picks the doc",
                    ))
                }
            }
        }
        Ok(p)
    }

    /// Apply the patch in `diff` to `text`. Returns the new text (`None`: nothing to apply),
    /// the messages for stdout, and the exit status.
    pub(crate) fn run(
        &self,
        name: &str,
        text: &str,
        diff: &str,
    ) -> Result<(Option<String>, String, u8), ToolError> {
        let mut hunks = parse_patch(diff)?;
        if self.reverse {
            for h in &mut hunks {
                h.reverse();
            }
        }
        let input = lines(text);
        let mut msg = String::new();
        msg.push_str(if self.dry_run {
            "checking file "
        } else {
            "patching file "
        });
        msg.push_str(name);
        msg.push('\n');
        let mut last_offset: isize = 0;
        let mut frozen = 0usize; // input lines (1-based) before this are taken
        let mut placed: Vec<(usize, usize)> = Vec::new(); // (hunk, where)
        let mut failed: Vec<(usize, usize)> = Vec::new(); // (hunk, guess)
        for (k, h) in hunks.iter().enumerate() {
            let first = h.first();
            let guess = clamp_line(first as isize + last_offset);
            let maxfuzz = self.fuzz.min(h.prefix().max(h.suffix()));
            let mut found = None;
            for fuzz in 0..=maxfuzz {
                if let Some(w) = locate(&input, h, guess, frozen, fuzz) {
                    found = Some((w, fuzz));
                    break;
                }
            }
            let newfirst = |at: isize| h.new_first() as isize + at - first as isize;
            match found {
                Some((w, fuzz)) => {
                    let offset = w as isize - first as isize;
                    last_offset = offset;
                    frozen = w + h.old_lines() - 1;
                    placed.push((k, w));
                    if !self.silent && (fuzz > 0 || offset != 0) {
                        msg.push_str(&format!(
                            "Hunk #{} succeeded at {}",
                            k + 1,
                            newfirst(w as isize)
                        ));
                        if fuzz > 0 {
                            msg.push_str(&format!(" with fuzz {fuzz}"));
                        }
                        if offset != 0 {
                            msg.push_str(&format!(
                                " (offset {offset} line{})",
                                if offset.abs() == 1 { "" } else { "s" }
                            ));
                        }
                        msg.push_str(".\n");
                    }
                }
                None => {
                    failed.push((k, guess));
                    msg.push_str(&format!(
                        "Hunk #{} FAILED at {}.\n",
                        k + 1,
                        newfirst(guess as isize)
                    ));
                }
            }
        }
        let total = hunks.len();
        let s = if total == 1 { "" } else { "s" };
        if failed.is_empty() {
            let new = (!self.dry_run).then(|| build(&input, &hunks, &placed));
            return Ok((new, msg, 0));
        }
        let applies = self.partial && !placed.is_empty() && !self.dry_run;
        let n = failed.len();
        if self.dry_run {
            msg.push_str(&format!("{n} out of {total} hunk{s} FAILED\n"));
        } else if self.partial {
            msg.push_str(&format!(
                "{n} out of {total} hunk{s} FAILED -- rejects follow\n"
            ));
        } else {
            msg.push_str(&format!(
                "{n} out of {total} hunk{s} FAILED -- nothing applied (all or nothing; --partial applies the rest)\n"
            ));
        }
        for (k, guess) in &failed {
            let h = &hunks[*k];
            msg.push_str(&format!(
                "--- hunk #{} (expected at line {guess}):\n",
                k + 1
            ));
            h.write(&mut msg);
            if !self.partial {
                let lo = guess.saturating_sub(3).max(1);
                let hi = (guess + h.old_lines() + 2).min(input.len());
                if lo <= hi && !input.is_empty() {
                    msg.push_str(&format!("--- live text, lines {lo}-{hi}:\n"));
                    for (i, l) in input.iter().enumerate().take(hi).skip(lo - 1) {
                        msg.push_str(&format!("{}\t{}\n", i + 1, l.text));
                    }
                } else {
                    msg.push_str(&format!(
                        "--- live text has {} line{} (none near line {guess})\n",
                        input.len(),
                        if input.len() == 1 { "" } else { "s" }
                    ));
                }
            }
            if msg.len() > MAX_OUTPUT_BYTES {
                return Err(too_much_output());
            }
        }
        let new = applies.then(|| build(&input, &hunks, &placed));
        Ok((new, msg, 1))
    }
}

fn clamp_line(n: isize) -> usize {
    n.max(1) as usize
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HLine {
    kind: char,
    text: String,
    nl: bool,
}

#[derive(Debug, Clone)]
struct PHunk {
    old_start: usize,
    old_len: usize,
    new_start: usize,
    new_len: usize,
    lines: Vec<HLine>,
}

impl PHunk {
    fn reverse(&mut self) {
        std::mem::swap(&mut self.old_start, &mut self.new_start);
        std::mem::swap(&mut self.old_len, &mut self.new_len);
        for l in &mut self.lines {
            l.kind = match l.kind {
                '-' => '+',
                '+' => '-',
                k => k,
            };
        }
        // keep removals before additions within each change, as diff writes them
        let mut out: Vec<HLine> = Vec::with_capacity(self.lines.len());
        let mut adds: Vec<HLine> = Vec::new();
        for l in self.lines.drain(..) {
            match l.kind {
                '+' => adds.push(l),
                '-' => out.push(l),
                _ => {
                    out.append(&mut adds);
                    out.push(l);
                }
            }
        }
        out.append(&mut adds);
        self.lines = out;
    }

    /// The 1-based line the hunk's old part starts at (an empty old part is placed after
    /// its line, as GNU patch does).
    fn first(&self) -> usize {
        if self.old_len == 0 {
            self.old_start + 1
        } else {
            self.old_start.max(1)
        }
    }

    fn new_first(&self) -> usize {
        if self.new_len == 0 {
            self.new_start + 1
        } else {
            self.new_start.max(1)
        }
    }

    fn old(&self) -> impl Iterator<Item = &HLine> {
        self.lines.iter().filter(|l| l.kind != '+')
    }

    fn old_lines(&self) -> usize {
        self.old_len
    }

    fn prefix(&self) -> usize {
        self.lines.iter().take_while(|l| l.kind == ' ').count()
    }

    fn suffix(&self) -> usize {
        self.lines
            .iter()
            .rev()
            .take_while(|l| l.kind == ' ')
            .count()
    }

    fn write(&self, out: &mut String) {
        out.push_str(&format!(
            "@@ -{} +{} @@\n",
            range(
                self.old_start.saturating_sub(usize::from(self.old_len > 0)),
                self.old_len
            ),
            range(
                self.new_start.saturating_sub(usize::from(self.new_len > 0)),
                self.new_len
            )
        ));
        for l in &self.lines {
            out.push(l.kind);
            out.push_str(&l.text);
            out.push('\n');
            if !l.nl {
                out.push_str("\\ No newline at end of file\n");
            }
        }
    }
}

/// Does the hunk's old part (less `pf` leading and `sf` trailing context lines) sit at
/// 1-based line `at`?
fn matches_at(input: &[Line<'_>], h: &PHunk, at: usize, pf: usize, sf: usize) -> bool {
    let n = h.old_lines();
    if at == 0 || at - 1 + n > input.len() + sf {
        return false;
    }
    h.old()
        .enumerate()
        .skip(pf)
        .take(n.saturating_sub(pf + sf))
        .all(|(i, l)| {
            input
                .get(at - 1 + i)
                .is_some_and(|x| x.text == l.text && x.nl == l.nl)
        })
}

/// GNU patch's `locate_hunk`.
fn locate(
    input: &[Line<'_>],
    h: &PHunk,
    guess: usize,
    frozen: usize,
    fuzz: usize,
) -> Option<usize> {
    let n = input.len() as isize;
    let pat = h.old_lines() as isize;
    let (prefix, suffix) = (h.prefix() as isize, h.suffix() as isize);
    let context = prefix.max(suffix);
    let pf = fuzz as isize + prefix - context;
    let sf = fuzz as isize + suffix - context;
    let g = guess as isize;
    let max_pos = n - g - pat + 1;
    let max_neg = g - frozen as isize - 1;
    if pf < 0 {
        // less leading context than trailing: only at the start of the file
        if frozen > 0 {
            return None;
        }
        if sf < 0 {
            // only the whole file
            return (pat == n && matches_at(input, h, 1, 0, 0)).then_some(1);
        }
        return (1 - g <= max_pos && matches_at(input, h, 1, 0, sf as usize)).then_some(1);
    }
    if sf < 0 {
        // less trailing context than leading: only at the end of the file
        let at = n - pat + 1;
        return (at >= 1
            && at > frozen as isize
            && matches_at(input, h, at as usize, pf as usize, 0))
        .then_some(at as usize);
    }
    let (pf, sf) = (pf as usize, sf as usize);
    let reach = max_pos.max(max_neg).max(0);
    for off in 0..=reach {
        if off <= max_pos && matches_at(input, h, (g + off) as usize, pf, sf) {
            return Some((g + off) as usize);
        }
        if off > 0 && off <= max_neg && matches_at(input, h, (g - off) as usize, pf, sf) {
            return Some((g - off) as usize);
        }
    }
    None
}

fn build(input: &[Line<'_>], hunks: &[PHunk], placed: &[(usize, usize)]) -> String {
    let mut out = String::new();
    let mut cur = 0usize; // 0-based input line
    let push = |out: &mut String, t: &str, nl: bool| {
        out.push_str(t);
        if nl {
            out.push('\n');
        }
    };
    for &(k, at) in placed {
        let h = &hunks[k];
        for l in &input[cur..at - 1] {
            push(&mut out, l.text, l.nl);
        }
        let mut pos = at - 1;
        for hl in &h.lines {
            match hl.kind {
                ' ' => {
                    if let Some(l) = input.get(pos) {
                        push(&mut out, l.text, l.nl);
                    }
                    pos += 1;
                }
                '-' => pos += 1,
                _ => push(&mut out, &hl.text, hl.nl),
            }
        }
        cur = pos.min(input.len());
    }
    for l in &input[cur..] {
        push(&mut out, l.text, l.nl);
    }
    out
}

fn malformed(line: usize, what: &str) -> ToolError {
    ToolError::usage(format!("malformed patch at line {line}: {what}"))
}

fn header_num(s: &str, line: usize) -> Result<(usize, usize), ToolError> {
    let (a, b) = s.split_once(',').unwrap_or((s, "1"));
    let p = |v: &str| {
        v.parse::<usize>()
            .map_err(|_| malformed(line, "bad hunk header"))
    };
    Ok((p(a)?, p(b)?))
}

/// One file's unified diff: optional `---`/`+++` headers and leading garbage (`diff`,
/// `index` lines...), then hunks. A second file is refused.
fn parse_patch(diff: &str) -> Result<Vec<PHunk>, ToolError> {
    let raw: Vec<&str> = diff
        .split_inclusive('\n')
        .map(|l| {
            let l = l.strip_suffix('\n').unwrap_or(l);
            l.strip_suffix('\r').unwrap_or(l)
        })
        .collect();
    let mut hunks: Vec<PHunk> = Vec::new();
    let mut headers = 0usize;
    let mut i = 0;
    while i < raw.len() {
        let l = raw[i];
        let lineno = i + 1;
        if l.starts_with("--- ") && raw.get(i + 1).is_some_and(|n| n.starts_with("+++ ")) {
            headers += 1;
            if headers > 1 || (!hunks.is_empty()) {
                return Err(ToolError::unsupported(
                    "a patch for several files",
                    "one doc per call: split the patch per file",
                ));
            }
            i += 2;
            continue;
        }
        if let Some(rest) = l.strip_prefix("@@ -") {
            let Some((ranges, _)) = rest.split_once(" @@") else {
                return Err(malformed(lineno, "bad hunk header"));
            };
            let Some((o, n)) = ranges.split_once(" +") else {
                return Err(malformed(lineno, "bad hunk header"));
            };
            let (old_start, old_len) = header_num(o, lineno)?;
            let (new_start, new_len) = header_num(n, lineno)?;
            let mut h = PHunk {
                old_start,
                old_len,
                new_start,
                new_len,
                lines: Vec::new(),
            };
            let (mut need_old, mut need_new) = (old_len, new_len);
            i += 1;
            while need_old > 0 || need_new > 0 {
                let Some(&b) = raw.get(i) else {
                    return Err(malformed(i + 1, "hunk ends early"));
                };
                let (kind, text) = match b.chars().next() {
                    None => (' ', ""),
                    Some(c @ (' ' | '-' | '+')) => (c, &b[1..]),
                    Some('\\') => {
                        if let Some(last) = h.lines.last_mut() {
                            last.nl = false;
                        }
                        i += 1;
                        continue;
                    }
                    Some(_) => return Err(malformed(i + 1, "unexpected line in hunk")),
                };
                match kind {
                    ' ' if need_old > 0 && need_new > 0 => {
                        need_old -= 1;
                        need_new -= 1;
                    }
                    '-' if need_old > 0 => need_old -= 1,
                    '+' if need_new > 0 => need_new -= 1,
                    _ => return Err(malformed(i + 1, "hunk is longer than its header says")),
                }
                h.lines.push(HLine {
                    kind,
                    text: text.to_owned(),
                    nl: true,
                });
                i += 1;
            }
            if raw.get(i).is_some_and(|l| l.starts_with('\\')) {
                if let Some(last) = h.lines.last_mut() {
                    last.nl = false;
                }
                i += 1;
            }
            hunks.push(h);
            continue;
        }
        // leading garbage, or text between hunks: ignored as GNU patch does
        i += 1;
    }
    if hunks.is_empty() {
        return Err(ToolError::usage(
            "only garbage was found in the patch input",
        ));
    }
    Ok(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(args: &[&str], text: &str, diff: &str) -> (Option<String>, String, u8) {
        let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        PatchCmd::parse(&a)
            .unwrap()
            .run("f.md", text, diff)
            .unwrap()
    }

    #[test]
    fn diff_format() {
        assert_eq!(unified("a\n", "a\n", 3, "x", "y"), "");
        assert_eq!(
            unified("a\nb\nc\n", "a\nB\nc\n", 3, "x", "y"),
            "--- x\n+++ y\n@@ -1,3 +1,3 @@\n a\n-b\n+B\n c\n"
        );
        assert_eq!(
            unified("", "a\n", 3, "x", "y"),
            "--- x\n+++ y\n@@ -0,0 +1 @@\n+a\n"
        );
        assert_eq!(
            unified("a", "b", 3, "x", "y"),
            "--- x\n+++ y\n@@ -1 +1 @@\n-a\n\\ No newline at end of file\n+b\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn patch_round_trip_and_offsets() {
        let a: String = (1..=20).map(|i| format!("{i}\n")).collect();
        let b = a
            .replace("\n3\n", "\nthree\n")
            .replace("\n15\n", "\nfifteen\n");
        let (a, b) = (a.as_str(), b.as_str());
        let d = unified(a, b, 3, "a", "b");
        assert_eq!(d.matches("@@ -").count(), 2, "{d}");
        assert_eq!(
            patch(&[], a, &d),
            (Some(b.into()), "patching file f.md\n".into(), 0)
        );
        let shifted = format!("0\n0\n{a}");
        let (t, m, x) = patch(&[], &shifted, &d);
        assert_eq!(x, 0);
        assert_eq!(t.unwrap(), format!("0\n0\n{b}"));
        // hunk 1 has less leading context than trailing, so at fuzz 0 it may only sit at
        // the start; fuzz 1 lets it move
        assert!(
            m.contains("Hunk #1 succeeded at 3 with fuzz 1 (offset 2 lines)."),
            "{m}"
        );
        assert!(
            m.contains("Hunk #2 succeeded at 14 (offset 2 lines)."),
            "{m}"
        );
        let (t, m, x) = patch(&["-R"], b, &d);
        assert_eq!((t.unwrap().as_str(), x), (a, 0), "{m}");
        // fuzz: one changed context line
        let fuzzy = a.replace("\n18\n", "\neighteen\n");
        let (t, m, x) = patch(&[], &fuzzy, &d);
        assert_eq!(x, 0, "{m}");
        assert!(m.contains("with fuzz 1"), "{m}");
        assert_eq!(t.unwrap(), b.replace("\n18\n", "\neighteen\n"));
        // all or nothing
        let broken = a.replace("\n15\n", "\nFIFTEEN\n");
        let (t, m, x) = patch(&[], &broken, &d);
        assert_eq!((t, x), (None, 1));
        assert!(m.contains("Hunk #2 FAILED"), "{m}");
        let (t, m, x) = patch(&["--partial"], &broken, &d);
        assert_eq!(x, 1);
        assert_eq!(t.unwrap(), broken.replace("\n3\n", "\nthree\n"), "{m}");
        let (t, _, x) = patch(&["--dry-run"], a, &d);
        assert_eq!((t, x), (None, 0));
    }

    #[test]
    fn patch_input_errors() {
        let a: Vec<String> = Vec::new();
        let p = PatchCmd::parse(&a).unwrap();
        assert!(matches!(
            p.run("f", "a\n", "hello\n"),
            Err(ToolError::Usage(_))
        ));
        assert!(matches!(
            p.run("f", "a\n", "@@ -1,2 +1,2 @@\n a\n"),
            Err(ToolError::Usage(_))
        ));
        let two = "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-a\n+b\n--- a/y\n+++ b/y\n@@ -1 +1 @@\n-a\n+b\n";
        assert!(matches!(
            p.run("f", "a\n", two),
            Err(ToolError::Unsupported { .. })
        ));
    }
}
