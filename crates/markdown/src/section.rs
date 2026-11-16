//! Markdown sections: finding headings, and `replace-section` (the parity row "Paste a block").
//!
//! A **section** is a heading and the lines after it, up to the next heading of the same or
//! a higher level (fewer `#`), or the end of the doc. Subsections belong to it.
//!
//! Headings follow CommonMark, for the parts that matter here:
//! - **ATX**: up to 3 spaces, 1–6 `#`, then a space, a tab or the end of the line; an
//!   optional closing run of `#` (after a space) is not part of the text.
//! - **Setext**: a paragraph (one or more lines) underlined by `=` (level 1) or `-`
//!   (level 2), the underline indented at most 3 spaces.
//! - Not headings: lines inside fenced code (```` ``` ```` or `~~~`, closed by the same
//!   character, at least as long, with no info string; an unclosed fence runs to the end),
//!   and YAML front matter (`---` on the first line, up to the next `---` or `...`).
//!   Raw HTML blocks are not parsed: a `# line` inside one counts as a heading.
//!
//! A heading is named as the agent types it: `## Goals` (level and text), or `Goals` (the
//! text at any level). Texts are compared with runs of whitespace collapsed. A setext
//! heading answers to its level, so `## Part` names `Part` underlined by `---`.
//!
//! A heading also has an **address**, `@h:<slug>` ([`addresses`]), in the kit's address
//! grammar: its [`slug`] (GitHub's anchor rule), a repeat numbered `-1`, `-2` in document
//! order. `replace-section --heading @h:<slug>` names that one heading; an address that names
//! no heading is refused with [`NO_SUCH_HEADING`], never resolved to the nearest one.

use std::collections::HashMap;

use k2_mpc_base::addr::{self, Addr, Forms};
use k2_mpc_base::errors::{ErrorEntry, ErrorReport};
use k2_mpc_base::ids::{IdClass, IdKind};
use k2_mpc_base::parity::positions::{PositionForm, Unit};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::tools::{exit, EditOutput, PackEdit, PackVerb, ToolError};

/// One heading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1 to 6.
    pub level: u8,
    /// Its text, whitespace collapsed, without `#` markers.
    pub text: String,
    /// Its first line (1-based): the `#` line, or a setext heading's first paragraph line.
    pub line: usize,
    /// Its last line (the setext underline; for ATX, `line`).
    pub last_line: usize,
}

struct Line<'a> {
    text: &'a str,
    start: usize,
}

fn split(text: &str) -> Vec<Line<'_>> {
    let mut out = Vec::new();
    let mut start = 0;
    for piece in text.split_inclusive('\n') {
        out.push(Line {
            text: piece.strip_suffix('\n').unwrap_or(piece),
            start,
        });
        start += piece.len();
    }
    out
}

/// Strip up to 3 leading spaces; `None` when indented 4 or more (or by a tab).
fn indent3(s: &str) -> Option<&str> {
    let n = s.len() - s.trim_start_matches(' ').len();
    if n > 3 || s[n..].starts_with('\t') {
        None
    } else {
        Some(&s[n..])
    }
}

fn collapse(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `(level, text)` of an ATX heading line.
fn atx(line: &str) -> Option<(u8, String)> {
    let s = indent3(line)?;
    let hashes = s.len() - s.trim_start_matches('#').len();
    if !(1..=6).contains(&hashes) {
        return None;
    }
    let rest = &s[hashes..];
    if !(rest.is_empty() || rest.starts_with(' ') || rest.starts_with('\t')) {
        return None;
    }
    let mut t = rest.trim();
    // an optional closing sequence: spaces then only '#'
    let without = t.trim_end_matches('#');
    if without.is_empty() {
        t = "";
    } else if without.ends_with([' ', '\t']) {
        t = without.trim_end();
    }
    Some((hashes as u8, collapse(t)))
}

/// A fence opener: (char, length), when it has no backtick in a backtick info string.
fn fence_open(line: &str) -> Option<(char, usize)> {
    let s = indent3(line)?;
    let c = s.chars().next()?;
    if c != '`' && c != '~' {
        return None;
    }
    let n = s.len() - s.trim_start_matches(c).len();
    if n < 3 || (c == '`' && s[n..].contains('`')) {
        return None;
    }
    Some((c, n))
}

fn fence_closes(line: &str, (c, n): (char, usize)) -> bool {
    let Some(s) = indent3(line) else {
        return false;
    };
    let run = s.len() - s.trim_start_matches(c).len();
    run >= n && s[run..].trim().is_empty()
}

/// A setext underline: its level.
fn underline(line: &str) -> Option<u8> {
    let s = indent3(line)?.trim_end();
    let c = s.chars().next()?;
    if (c == '=' || c == '-') && s.chars().all(|x| x == c) {
        Some(if c == '=' { 1 } else { 2 })
    } else {
        None
    }
}

/// A line that can be part of a paragraph (and so of a setext heading's text).
fn paragraph_line(line: &str) -> bool {
    let Some(s) = indent3(line) else {
        return false;
    };
    let t = s.trim_end();
    if t.is_empty() || atx(line).is_some() || fence_open(line).is_some() || t.starts_with('>') {
        return false;
    }
    // list items and thematic breaks don't take a setext underline
    let item = |m: &str| t == m.trim_end() || t.starts_with(m);
    if item("- ") || item("* ") || item("+ ") {
        return false;
    }
    let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if (1..=9).contains(&digits) {
        let after = &t[digits..];
        if after == "." || after == ")" || after.starts_with(". ") || after.starts_with(") ") {
            return false;
        }
    }
    let marks: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    let thematic = marks.len() >= 3
        && marks
            .chars()
            .next()
            .is_some_and(|c| matches!(c, '-' | '*' | '_') && marks.chars().all(|x| x == c));
    !thematic
}

/// Every heading of a Markdown text, in order (see the module docs for what counts).
pub fn headings(text: &str) -> Vec<Heading> {
    let lines = split(text);
    let mut out = Vec::new();
    let mut i = 0;
    // front matter
    if lines.first().is_some_and(|l| l.text.trim_end() == "---") {
        if let Some(end) = lines[1..]
            .iter()
            .position(|l| matches!(l.text.trim_end(), "---" | "..."))
        {
            i = end + 2;
        }
    }
    let mut fence: Option<(char, usize)> = None;
    // first line of the paragraph running up to the previous line, if any
    let mut para: Option<usize> = None;
    while i < lines.len() {
        let l = lines[i].text;
        if let Some(f) = fence {
            if fence_closes(l, f) {
                fence = None;
            }
            i += 1;
            continue;
        }
        if let Some(f) = fence_open(l) {
            fence = Some(f);
            para = None;
            i += 1;
            continue;
        }
        if let Some(start) = para {
            if let Some(level) = underline(l) {
                let t: Vec<&str> = lines[start..i].iter().map(|l| l.text).collect();
                out.push(Heading {
                    level,
                    text: collapse(&t.join(" ")),
                    line: start + 1,
                    last_line: i + 1,
                });
                para = None;
                i += 1;
                continue;
            }
        }
        if let Some((level, t)) = atx(l) {
            out.push(Heading {
                level,
                text: t,
                line: i + 1,
                last_line: i + 1,
            });
            para = None;
        } else if paragraph_line(l) {
            para.get_or_insert(i);
        } else {
            para = None;
        }
        i += 1;
    }
    out
}

/// The kind in a heading's address, `@h:<slug>` (a native ID in the kit's address grammar).
pub const ADDRESS_KIND: &str = "h";

/// A heading's slug before repeats are numbered, by GitHub's anchor rule: the text
/// lowercased (character by character: a final capital sigma is `σ`); letters, numbers, combining marks, `-` and `_` kept; each space a `-`;
/// everything else (punctuation, symbols, emoji) dropped. `text` is the heading's text as
/// [`headings`] gives it (whitespace collapsed, no `#` markers). Inline Markdown is not
/// rendered first: the slug is of the source text (a link's target counts, `_x_` keeps its
/// underscores), so a host can compute the same slug from the text alone.
pub fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        if c == ' ' {
            out.push('-');
        } else if c.is_alphanumeric() || c == '-' || c == '_' || combining(c) {
            out.push(c);
        }
    }
    out
}

/// A combining mark of the combining-diacritics blocks (`e` + U+0301). Other scripts' marks
/// count as [`char::is_alphanumeric`] counts them: most vowel signs are letters and stay; a
/// mark that isn't one (a virama) is dropped, where GitHub keeps it.
fn combining(c: char) -> bool {
    COMBINING.iter().any(|&(a, b)| (a..=b).contains(&c))
}

/// The combining-diacritics blocks, first and last.
const COMBINING: [(char, char); 5] = [
    ('\u{0300}', '\u{036F}'),
    ('\u{1AB0}', '\u{1AFF}'),
    ('\u{1DC0}', '\u{1DFF}'),
    ('\u{20D0}', '\u{20FF}'),
    ('\u{FE20}', '\u{FE2F}'),
];

/// The numbered slug of each heading, in order: GitHub's rule ([`addresses`]).
fn numbered(all: &[Heading]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(all.len());
    for h in all {
        let base = slug(&h.text);
        let mut s = base.clone();
        while seen.contains_key(&s) {
            let n = seen
                .get_mut(&base)
                .expect("a taken slug's base was given first");
            *n += 1;
            s = format!("{base}-{n}");
        }
        seen.insert(s.clone(), 0);
        out.push(s);
    }
    out
}

/// The index of the heading whose numbered slug is `want`: that slug exactly, never the nearest.
fn find_slug(slugs: &[String], want: &str) -> Option<usize> {
    slugs.iter().position(|s| s == want)
}

/// The slug a heading address names: `Some` for `@h:<slug>` (`Err` when malformed), `None`
/// for anything else (a heading's text).
fn address_slug(heading: &str) -> Option<Result<String, addr::AddrError>> {
    if !heading.starts_with("@h:") {
        return None;
    }
    let got = match addr::parse(heading, Forms::none().native()) {
        Ok(Addr::Native(mut segs)) if segs.len() == 1 && segs[0].0 == ADDRESS_KIND => {
            Ok(segs.remove(0).1)
        }
        Ok(_) => Err(addr::AddrError::Malformed {
            given: heading.to_owned(),
            reason: "a heading address is @h:<slug>",
        }),
        Err(e) => Err(e),
    };
    Some(got)
}

/// One heading and its address ([`addresses`]).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct HeadingAddress {
    /// Its slug, numbered when it repeats (`notes`, `notes-1`); empty for a first heading
    /// whose text has nothing a slug keeps.
    pub slug: String,
    /// `@h:<slug>`; `None` when the slug is empty.
    pub address: Option<String>,
    pub heading: Heading,
}

/// Every heading of `text` with its address, in document order, numbered as GitHub numbers its
/// anchors. Every slug given out is remembered, and each base keeps a count. A heading whose
/// slug is still free gets it. One whose slug was already given out counts its base up by one
/// and tries `<base>-<count>`, and again while that is taken too, then gets the first free one.
/// A numbered slug is remembered like any other, so a later heading whose own slug is that text
/// is numbered from it in turn. So `Notes`, `Notes`, `Notes`, `Notes 1` are `notes`, `notes-1`,
/// `notes-2`, `notes-1-1`; `a`, `a-1`, `a` are `a`, `a-1`, `a-2`; no two headings share one.
/// The cases are in `tests/data/heading-slugs.json`.
pub fn addresses(text: &str) -> Vec<HeadingAddress> {
    let all = headings(text);
    let slugs = numbered(&all);
    all.into_iter()
        .zip(slugs)
        .map(|(heading, slug)| HeadingAddress {
            address: (!slug.is_empty()).then(|| format!("@{ADDRESS_KIND}:{slug}")),
            slug,
            heading,
        })
        .collect()
}

/// The heading `address` (`@h:<slug>`) names in `text`, if one does: exactly that slug,
/// never the nearest one. Anything that isn't a heading address names none.
pub fn resolve(text: &str, address: &str) -> Option<Heading> {
    let want = address_slug(address)?.ok()?;
    let mut all = headings(text);
    let i = find_slug(&numbered(&all), &want)?;
    Some(all.swap_remove(i))
}

/// The refusal for a heading address that names no heading in the version the verb ran on.
pub const NO_SUCH_HEADING: &ErrorEntry = &ErrorEntry::pack(
    "MARKDOWN-E001",
    "no_such_heading",
    exit::USAGE,
    "{given} names no heading in this version",
    "The heading address names no heading in the doc as it is now: the heading was renamed or \
     removed, or a repeat above it was added or removed (a repeated heading's -1, -2 count the \
     ones above it). Nothing was applied; the nearest heading is never used instead. Read the \
     doc again and name the heading by its address now.",
)
.step(
    "cat",
    &[],
    "The heading address names no heading now; read the doc again and use the heading's \
     address now.",
);

/// The pack's own refusals ([`k2_mpc_base::session::Pack::errors`]).
pub const ERRORS: &[ErrorEntry] = &[*NO_SUCH_HEADING];

/// The pack's own address kinds ([`k2_mpc_base::session::Pack::ids`]); `docs/IDS.md` is
/// generated from them.
pub const IDS: &[IdKind] = &[
    IdKind {
        kind: "heading-address",
        example: "--heading @h:goals",
        class: IdClass::UniqueName,
        scope: "a Markdown doc's headings",
        accepted_by: &["replace-section"],
        note: "`@h:<slug>`, GitHub's anchor rule on the heading's source text: lowercased; \
               letters, numbers, combining marks, `-` and `_` kept; each space a `-`; anything \
               else dropped. A repeat gets `-1`, `-2` in document order. A rename, or a repeat \
               added or removed above, changes it; one that names no heading is refused \
               (`MARKDOWN-E001`), never the nearest",
    },
    IdKind {
        kind: "heading-text",
        example: "--heading '## Goals' | --heading Goals",
        class: IdClass::UniqueName,
        scope: "a Markdown doc's headings",
        accepted_by: &["replace-section"],
        note: "the level and the text, or the text at any level, whitespace collapsed; several \
               matching: exit 1 with their lines, then `--nth K` (a position: name the version \
               with `--if-version`)",
    },
];

/// Which section [`replace_section`] replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRequest {
    /// `## Goals` (level and text), `Goals` (any level), or `@h:goals` (an address).
    pub heading: String,
    /// The K-th match, 1-based. Required when several headings match.
    pub nth: Option<usize>,
    /// Replace the heading too, not just the body under it.
    pub include_heading: bool,
}

/// What [`replace_section`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionOutcome {
    /// The new text.
    Replaced(String),
    /// No heading matches (or fewer than `nth`): how many matched.
    NotFound { matches: usize },
    /// Several headings match and no `nth` was given: their first lines.
    Ambiguous { lines: Vec<usize> },
}

/// `(level, text)` the request names; level `None` = any.
fn wanted(heading: &str) -> (Option<u8>, String) {
    match atx(heading) {
        Some((level, t)) => (Some(level), t),
        None => (None, collapse(heading)),
    }
}

/// Replace one section of `text` with `body` (a pure function). `req.heading` is a heading's
/// text, or its address (`@h:<slug>`, [`addresses`]): an address names at most one heading,
/// and one that names none is [`SectionOutcome::NotFound`] (the verb refuses it typed).
///
/// - **Without** `include_heading`: the body under the heading is replaced, keeping its
///   leading and trailing blank lines (the layout around the heading stays). An empty body
///   becomes a blank line, `body`, and a blank line before a following heading.
/// - **With** `include_heading`: the heading and its body up to the last non-blank line are
///   replaced.
/// - `body` becomes whole lines (a missing final `\n` is added), unless the section ends the
///   text without a final `\n`, which stays so.
pub fn replace_section(text: &str, req: &SectionRequest, body: &str) -> SectionOutcome {
    let all = headings(text);
    let matching: Vec<&Heading> = match address_slug(&req.heading) {
        // an address names one heading, or none (a malformed one names none)
        Some(want) => want
            .ok()
            .and_then(|w| find_slug(&numbered(&all), &w))
            .map(|i| &all[i])
            .into_iter()
            .collect(),
        None => {
            let (level, name) = wanted(&req.heading);
            all.iter()
                .filter(|h| h.text == name && level.map_or(true, |l| l == h.level))
                .collect()
        }
    };
    let h = match (req.nth, matching.len()) {
        (_, 0) => return SectionOutcome::NotFound { matches: 0 },
        (None, 1) => matching[0],
        (None, _) => {
            return SectionOutcome::Ambiguous {
                lines: matching.iter().map(|h| h.line).collect(),
            }
        }
        (Some(k), n) if k == 0 || k > n => return SectionOutcome::NotFound { matches: n },
        (Some(k), _) => matching[k - 1],
    };
    let lines = split(text);
    // the section ends before the next heading of the same or a higher level
    let end = all
        .iter()
        .find(|x| x.line > h.last_line && x.level <= h.level)
        .map_or(lines.len(), |x| x.line - 1); // 0-based index of the next heading's line
    let blank = |i: usize| lines[i].text.trim().is_empty();
    let first = if req.include_heading {
        h.line - 1
    } else {
        h.last_line
    };
    let mut a = first;
    if !req.include_heading {
        while a < end && blank(a) {
            a += 1;
        }
    }
    let mut b = end;
    while b > a && blank(b - 1) {
        b -= 1;
    }
    let byte = |i: usize| lines.get(i).map_or(text.len(), |l| l.start);
    let mut new_body = body.to_owned();
    if !new_body.is_empty() && !new_body.ends_with('\n') {
        new_body.push('\n');
    }
    let (from, to) = if a < b {
        // the region's last line may be the text's last, without a final newline
        let to = byte(b);
        if to == text.len() && !text.ends_with('\n') && new_body.ends_with('\n') {
            new_body.pop();
        }
        (byte(a), to)
    } else {
        // an empty body: a blank line, the body, and a blank line before what follows
        let at = byte(first);
        let (from, to) = (at, byte(end));
        let mut s = String::new();
        if !new_body.is_empty() {
            if at == text.len() && !text.ends_with('\n') && !text.is_empty() {
                s.push('\n');
            }
            s.push('\n');
            s.push_str(&new_body);
            if end < lines.len() {
                s.push('\n');
            }
        } else {
            s.push_str(&text[from..to]);
        }
        new_body = s;
        (from, to)
    };
    let mut out = String::with_capacity(text.len() + new_body.len());
    out.push_str(&text[..from]);
    out.push_str(&new_body);
    out.push_str(&text[to..]);
    SectionOutcome::Replaced(out)
}

/// The `replace-section` verb (the parity row "Paste a block"), for [`crate::MarkdownPack`].
///
/// `--nth K` is a position: the Kth section that matches, counted in the version the edit
/// applies to. The verb declares it (its unit, and where to find it in a call), so an agent's
/// `--nth` edit without `--if-version` gets the kit's positional rule, as the kit's own
/// verbs by line do.
pub const REPLACE_SECTION: PackVerb = PackVerb::edit(
    "replace-section",
    "replace-section --heading '## X' [--nth K] [--include-heading] [--if-version V] (new body on stdin)",
    &[PersonAction::Paste],
    &[exit::OK, exit::NO],
    parse_replace_section,
)
.positions(&[PositionForm::new("--nth", Unit::PackNth)])
.positional(nth_address);

/// `replace-section`'s positional address: its `--nth` flag and value, as the caller wrote
/// them (`--nth 2` or `--nth=2`), or `None`. Walks the arguments as
/// [`parse_replace_section`] does, so a `--heading` value spelled `--nth` is not one.
fn nth_address(args: &[String]) -> Option<String> {
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--nth" => return it.next().map(|v| format!("--nth {v}")),
            "--heading" => {
                it.next();
            }
            _ if a.starts_with("--nth=") => return Some(a.clone()),
            _ => {}
        }
    }
    None
}

const USAGE: &str =
    "replace-section --heading '## X' [--nth K] [--include-heading] [--if-version V]";

fn parse_replace_section(args: &[String]) -> Result<PackEdit, ToolError> {
    let mut heading = None;
    let mut nth = None;
    let mut include_heading = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let (flag, inline) = match a.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f, Some(v.to_owned())),
            _ => (a.as_str(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| ToolError::usage(format!("{name} needs a value ({USAGE})")))
        };
        match flag {
            "--heading" => heading = Some(value("--heading")?),
            "--nth" => {
                let v = value("--nth")?;
                let k: usize = v
                    .parse()
                    .ok()
                    .filter(|k| *k > 0)
                    .ok_or_else(|| ToolError::usage(format!("--nth {v}: a number from 1")))?;
                nth = Some(k);
            }
            "--include-heading" if inline.is_none() => include_heading = true,
            _ => return Err(ToolError::unsupported(a.clone(), USAGE)),
        }
    }
    let heading =
        heading.ok_or_else(|| ToolError::usage(format!("missing --heading ({USAGE})")))?;
    let address = match address_slug(&heading) {
        Some(Err(e)) => return Err(ToolError::usage(format!("--heading {heading}: {e}"))),
        Some(Ok(_)) if nth.is_some() => {
            return Err(ToolError::usage(format!(
                "--heading {heading} names one heading: --nth is for a heading's text"
            )))
        }
        Some(Ok(_)) => true,
        None => false,
    };
    if collapse(&heading).is_empty() || wanted(&heading).1.is_empty() {
        return Err(ToolError::usage("--heading needs the heading's text"));
    }
    let req = SectionRequest {
        heading,
        nth,
        include_heading,
    };
    Ok(Box::new(move |name, text, stdin| {
        let body = std::str::from_utf8(stdin)
            .map_err(|_| ToolError::usage("the new body (stdin) is not UTF-8 text"))?;
        Ok(match replace_section(text, &req, body) {
            SectionOutcome::Replaced(t) => EditOutput::new(Some(t), Vec::new(), exit::OK),
            // an address that names no heading: refused typed, never the nearest heading
            SectionOutcome::NotFound { .. } if address => {
                let given = req.heading.as_str();
                let msg =
                    format!("replace-section: {name}: {given} names no heading in this version");
                let report = ErrorReport::new(NO_SUCH_HEADING, msg)
                    .with("given", given)
                    .with("what", "heading");
                return Err(ToolError::address(report));
            }
            SectionOutcome::NotFound { matches } => {
                let msg = match (req.nth, matches) {
                    (Some(k), n) if n > 0 => format!(
                        "replace-section: {name}: --nth {k}, but only {n} section(s) match '{}'\n",
                        req.heading
                    ),
                    _ => format!("replace-section: {name}: no section '{}'\n", req.heading),
                };
                EditOutput::new(None, msg.into_bytes(), exit::NO)
            }
            SectionOutcome::Ambiguous { lines } => {
                let list: Vec<String> = lines.iter().map(usize::to_string).collect();
                let msg = format!(
                    "replace-section: {name}: {} sections match '{}', at lines {}; pick one with --nth K\n",
                    lines.len(),
                    req.heading,
                    list.join(", ")
                );
                EditOutput::new(None, msg.into_bytes(), exit::NO)
            }
        })
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(h: &str) -> SectionRequest {
        SectionRequest {
            heading: h.into(),
            nth: None,
            include_heading: false,
        }
    }

    fn replaced(text: &str, r: &SectionRequest, body: &str) -> String {
        match replace_section(text, r, body) {
            SectionOutcome::Replaced(t) => t,
            o => panic!("{o:?}"),
        }
    }

    fn levels(text: &str) -> Vec<(u8, String, usize)> {
        headings(text)
            .into_iter()
            .map(|h| (h.level, h.text, h.line))
            .collect()
    }

    #[test]
    fn atx_headings() {
        let t = "# One\n  ## Two ##\n###   Three   #\n#5 not\n####### seven\n    # code\n##\n#\tTab\n## a #b\n";
        assert_eq!(
            levels(t),
            vec![
                (1, "One".into(), 1),
                (2, "Two".into(), 2),
                (3, "Three".into(), 3),
                (2, "".into(), 7),
                (1, "Tab".into(), 8),
                (2, "a #b".into(), 9),
            ]
        );
    }

    #[test]
    fn setext_headings_fences_and_front_matter() {
        let t = "---\ntitle: x\n---\n\nTitle\non two lines\n===\n\n```md\n## Fenced\n```\n\n~~~~\n# also\n~~~\nstill fenced\n~~~~\n\nPart\n---\n\n- item\n---\n\n> quote\n---\n";
        assert_eq!(
            levels(t),
            vec![(1, "Title on two lines".into(), 5), (2, "Part".into(), 19)]
        );
        // an unclosed fence runs to the end
        assert_eq!(levels("# A\n```\n# B\n"), vec![(1, "A".into(), 1)]);
        // front matter without an end is just a thematic break
        assert_eq!(levels("---\n# A\n"), vec![(1, "A".into(), 2)]);
    }

    const DOC: &str =
        "# Plan\n\nIntro.\n\n## Goals\n\n- one\n\n### Sub\n\nsub text\n\n## Notes\n\nTODO\n";

    #[test]
    fn replaces_the_body_keeping_blank_lines() {
        assert_eq!(
            replaced(DOC, &req("## Goals"), "- new\n"),
            "# Plan\n\nIntro.\n\n## Goals\n\n- new\n\n## Notes\n\nTODO\n"
        );
        // by text, at any level; the last section runs to the end; a missing \n is added
        assert_eq!(
            replaced(DOC, &req("Notes"), "done"),
            "# Plan\n\nIntro.\n\n## Goals\n\n- one\n\n### Sub\n\nsub text\n\n## Notes\n\ndone\n"
        );
        // a level-1 section holds everything under it
        assert_eq!(replaced(DOC, &req("# Plan"), "x\n"), "# Plan\n\nx\n");
        // a subsection ends at the next heading of its level or higher
        assert_eq!(
            replaced(DOC, &req("### Sub"), "y\n"),
            "# Plan\n\nIntro.\n\n## Goals\n\n- one\n\n### Sub\n\ny\n\n## Notes\n\nTODO\n"
        );
        // an empty body deletes the content, keeping the layout
        assert_eq!(
            replaced(DOC, &req("## Notes"), ""),
            "# Plan\n\nIntro.\n\n## Goals\n\n- one\n\n### Sub\n\nsub text\n\n## Notes\n\n"
        );
    }

    #[test]
    fn include_heading() {
        let r = SectionRequest {
            include_heading: true,
            ..req("## Goals")
        };
        assert_eq!(
            replaced(DOC, &r, "## Aims\n\n- a\n"),
            "# Plan\n\nIntro.\n\n## Aims\n\n- a\n\n## Notes\n\nTODO\n"
        );
        let setext = "Part\n----\n\nbody\n\nNext\n----\n";
        let r = SectionRequest {
            include_heading: true,
            ..req("## Part")
        };
        assert_eq!(replaced(setext, &r, "## P\nb\n"), "## P\nb\n\nNext\n----\n");
    }

    #[test]
    fn empty_bodies_and_no_final_newline() {
        assert_eq!(
            replaced("## A\n## B\n", &req("A"), "x"),
            "## A\n\nx\n\n## B\n"
        );
        assert_eq!(
            replaced("## A\n\n## B\n", &req("A"), "x\n"),
            "## A\n\nx\n\n## B\n"
        );
        assert_eq!(replaced("## A", &req("A"), "x"), "## A\n\nx\n");
        assert_eq!(replaced("## A\nold", &req("A"), "new\n"), "## A\nnew");
        assert_eq!(replaced("## A\n## B\n", &req("A"), ""), "## A\n## B\n");
    }

    #[test]
    fn not_found_ambiguous_nth() {
        let t = "## Notes\n\na\n\n# Other\n\n## Notes\n\nb\n";
        assert_eq!(
            replace_section(t, &req("## Notes"), "x"),
            SectionOutcome::Ambiguous { lines: vec![1, 7] }
        );
        let r = SectionRequest {
            nth: Some(2),
            ..req("## Notes")
        };
        assert_eq!(
            replaced(t, &r, "x\n"),
            "## Notes\n\na\n\n# Other\n\n## Notes\n\nx\n"
        );
        let r = SectionRequest {
            nth: Some(3),
            ..req("## Notes")
        };
        assert_eq!(
            replace_section(t, &r, "x"),
            SectionOutcome::NotFound { matches: 2 }
        );
        assert_eq!(
            replace_section(t, &req("### Notes"), "x"),
            SectionOutcome::NotFound { matches: 0 }
        );
        let fenced = "```\n## Notes\n```\n";
        assert_eq!(
            replace_section(fenced, &req("Notes"), "x"),
            SectionOutcome::NotFound { matches: 0 }
        );
    }

    // ---- heading addresses (`@h:<slug>`)

    #[test]
    fn repeated_headings_are_numbered_in_document_order() {
        let t = "# Notes\n\n## Notes\n\n```\n## Notes\n```\n\nNotes\n-----\n\n### Notes 1\n\n\
                 ## Other\n\n## Notes\n\n## !!!\n\n## ???\n";
        let got: Vec<(Option<String>, usize)> = addresses(t)
            .into_iter()
            .map(|a| (a.address, a.heading.line))
            .collect();
        let want: Vec<(Option<String>, usize)> = [
            (Some("@h:notes"), 1),
            (Some("@h:notes-1"), 3),
            // the fenced look-alike is code; the setext heading counts
            (Some("@h:notes-2"), 9),
            // "notes-1" is taken: the repeat of that slug is numbered in turn
            (Some("@h:notes-1-1"), 12),
            (Some("@h:other"), 14),
            (Some("@h:notes-3"), 16),
            // nothing a slug keeps: no address for the first, `-1` for the repeat
            (None, 18),
            (Some("@h:-1"), 20),
        ]
        .into_iter()
        .map(|(a, l)| (a.map(str::to_owned), l))
        .collect();
        assert_eq!(got, want);
        // every address parses in the kit's grammar as one native ID of kind `h`
        for a in addresses(t).into_iter().filter_map(|a| a.address) {
            match k2_mpc_base::addr::parse(&a, k2_mpc_base::addr::Forms::none().native()) {
                Ok(k2_mpc_base::addr::Addr::Native(segs)) => {
                    assert_eq!(segs.len(), 1, "{a}");
                    assert_eq!(segs[0].0, ADDRESS_KIND, "{a}");
                }
                other => panic!("{a}: {other:?}"),
            }
        }
    }

    #[test]
    fn an_address_names_its_own_heading() {
        let t = "## Notes\n\na\n\n# Other\n\n## Notes\n\nb\n";
        // the repeat, by its address: the second section, with no --nth
        assert_eq!(
            replaced(t, &req("@h:notes-1"), "x\n"),
            "## Notes\n\na\n\n# Other\n\n## Notes\n\nx\n"
        );
        assert_eq!(resolve(t, "@h:notes-1").map(|h| h.line), Some(7));
        assert_eq!(
            replaced(t, &req("@h:notes"), "x\n"),
            "## Notes\n\nx\n\n# Other\n\n## Notes\n\nb\n"
        );
        assert_eq!(resolve(t, "@h:notes").map(|h| h.line), Some(1));
        // non-ASCII headings, by their own letters
        let u = "# Über uns\n\nalt\n\n# 日本語の見出し\n\nalt\n";
        assert_eq!(
            replaced(u, &req("@h:über-uns"), "neu\n"),
            "# Über uns\n\nneu\n\n# 日本語の見出し\n\nalt\n"
        );
        assert_eq!(
            replaced(u, &req("@h:日本語の見出し"), "新\n"),
            "# Über uns\n\nalt\n\n# 日本語の見出し\n\n新\n"
        );
        // through the verb: the same edit, exit 0
        let edit = REPLACE_SECTION
            .parse(&argv(&["--heading", "@h:notes-1"]))
            .expect("parses");
        let o = edit("n.md", t, b"x\n").expect("runs");
        assert_eq!(
            (o.text.as_deref(), o.exit),
            (Some("## Notes\n\na\n\n# Other\n\n## Notes\n\nx\n"), 0)
        );
    }

    #[test]
    fn an_address_that_names_no_heading_is_refused_not_the_nearest() {
        // read as `## Notes` twice (`@h:notes`, `@h:notes-1`); the second renamed since
        let t = "## Notes\n\na\n\n## Later\n\nb\n";
        assert_eq!(resolve(t, "@h:notes-1"), None);
        assert_eq!(
            replace_section(t, &req("@h:notes-1"), "x"),
            SectionOutcome::NotFound { matches: 0 }
        );
        // the verb refuses it typed: exit 2, the pack's own id, nothing applied
        let edit = REPLACE_SECTION
            .parse(&argv(&["--heading", "@h:notes-1"]))
            .expect("parses");
        let e = edit("n.md", t, b"x\n").expect_err("refused");
        assert_eq!(e.exit(), 2);
        let ToolError::Address(r) = &e else {
            panic!("not a typed refusal: {e:?}")
        };
        assert_eq!(r.id(), "MARKDOWN-E001");
        assert_eq!(
            r.message(),
            "replace-section: n.md: @h:notes-1 names no heading in this version"
        );
        assert_eq!(
            r.detail("given"),
            Some(&k2_mpc_base::errors::Value::from("@h:notes-1"))
        );
        // the first renamed instead: the old repeat's address names nothing; `@h:notes` is
        // now the one left (its own slug, by the rule)
        let t2 = "## Later\n\na\n\n## Notes\n\nb\n";
        assert_eq!(resolve(t2, "@h:notes-1"), None);
        assert_eq!(resolve(t2, "@h:notes").map(|h| h.line), Some(5));
    }

    fn argv(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| (*x).to_owned()).collect()
    }

    #[test]
    fn the_verb() {
        let run = |args: &[&str], text: &str, stdin: &str| {
            let edit = REPLACE_SECTION.parse(&argv(args)).expect("parses");
            edit("n.md", text, stdin.as_bytes()).expect("runs")
        };
        let t = "## A\n\nold\n\n## A\n\nold\n";
        let o = run(&["--heading", "## A"], t, "x");
        assert_eq!((o.text, o.exit), (None, 1));
        assert_eq!(
            String::from_utf8(o.stdout).unwrap(),
            "replace-section: n.md: 2 sections match '## A', at lines 1, 5; pick one with --nth K\n"
        );
        let o = run(
            &["--heading=## A", "--nth=2", "--include-heading"],
            t,
            "## B\n",
        );
        assert_eq!(o.text.as_deref(), Some("## A\n\nold\n\n## B\n"));
        let o = run(&["--heading", "## Z"], t, "x");
        assert_eq!(o.exit, 1);
        assert_eq!(o.stdout, b"replace-section: n.md: no section '## Z'\n");
        let o = run(&["--heading", "A", "--nth", "3"], t, "x");
        assert_eq!(
            o.stdout,
            b"replace-section: n.md: --nth 3, but only 2 section(s) match 'A'\n"
        );
        for bad in [
            &[][..],
            &["--heading"],
            &["--heading", "##"],
            &["--heading", "A", "--nth", "0"],
            &["--heading", "A", "--loud"],
            &["--heading", "A", "--include-heading=yes"],
            // an address names one heading: no --nth; and it is well formed
            &["--heading", "@h:a", "--nth", "1"],
            &["--heading", "@h:"],
            &["--heading", "@h:a b"],
            &["--heading", "@h:a/b"],
        ] {
            let e = REPLACE_SECTION.parse(&argv(bad)).err().expect("refused");
            assert_eq!(e.exit(), 2, "{bad:?}");
        }
        let edit = REPLACE_SECTION.parse(&argv(&["--heading", "A"])).unwrap();
        assert_eq!(edit("n.md", t, b"\xff").unwrap_err().exit(), 2);
    }

    mod props {
        use super::*;
        use proptest::prelude::*;

        fn line() -> impl Strategy<Value = String> {
            prop_oneof![
                Just(String::new()),
                Just("# A".to_owned()),
                Just("## B".to_owned()),
                Just("### A".to_owned()),
                Just("text".to_owned()),
                Just("more text".to_owned()),
                Just("---".to_owned()),
                Just("===".to_owned()),
                Just("```".to_owned()),
                Just("- item".to_owned()),
            ]
        }

        proptest! {
            /// Replacing a section's content with itself changes nothing, and text before
            /// the section never changes.
            #[test]
            fn identity_and_locality(ls in proptest::collection::vec(line(), 0..24), nl in any::<bool>()) {
                let mut text = ls.join("\n");
                if nl && !text.is_empty() {
                    text.push('\n');
                }
                let hs = headings(&text);
                for h in &hs {
                    let same: Vec<&Heading> = hs.iter().filter(|x| x.text == h.text && x.level == h.level).collect();
                    let k = same.iter().position(|x| x.line == h.line).unwrap() + 1;
                    let r = SectionRequest {
                        heading: format!("{} {}", "#".repeat(usize::from(h.level)), h.text),
                        nth: Some(k),
                        include_heading: true,
                    };
                    if h.text.is_empty() {
                        continue;
                    }
                    // the section's own text, heading to last non-blank line
                    let probe = replaced(&text, &r, "\u{1}");
                    let at = probe.find('\u{1}').unwrap();
                    prop_assert_eq!(&probe[..at], &text[..at], "before the section");
                    let lines: Vec<&str> = text.split_inclusive('\n').collect();
                    let start: usize = lines[..h.line - 1].iter().map(|l| l.len()).sum();
                    prop_assert_eq!(at, start);
                    // the marker gained a "\n" unless the section ends the text without one
                    let after = if probe.len() == at + 1 { "" } else { &probe[at + 2..] };
                    let own = &text[start..text.len() - after.len()];
                    prop_assert_eq!(replaced(&text, &r, own), text.clone());
                }
            }
        }
    }
}
