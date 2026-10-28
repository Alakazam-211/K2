//! Markdown sections: finding headings, and `replace-section` (parity row P13).
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

/// Which section [`replace_section`] replaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionRequest {
    /// `## Goals` (level and text) or `Goals` (any level).
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

/// Replace one section of `text` with `body` (a pure function).
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
    let (level, name) = wanted(&req.heading);
    let matching: Vec<&Heading> = all
        .iter()
        .filter(|h| h.text == name && level.map_or(true, |l| l == h.level))
        .collect();
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

/// The `replace-section` verb (P13), for [`crate::MarkdownPack`].
pub const REPLACE_SECTION: PackVerb = PackVerb::edit(
    "replace-section",
    "replace-section --heading '## X' [--nth K] [--include-heading] (new body on stdin)",
    &[PersonAction::Paste],
    &[exit::OK, exit::NO],
    parse_replace_section,
);

const USAGE: &str = "replace-section --heading '## X' [--nth K] [--include-heading]";

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
