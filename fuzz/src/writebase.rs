//! `write --base` (`merge::merge3`: base = the version the agent read, ours = its new text,
//! theirs = the live text) on line-shaped inputs, judged independently of the kit's diff.
//!
//! Inputs are lines that share their start, as an agent's edits of a log or a list make
//! them ("p02:7", "p01:3 w2"): each base line kept, deleted, or rewritten by one side or
//! both, or with different words changed on each side; whole new lines in the gaps and at
//! the end, from one side or both; sometimes no final line break. These are the shapes that
//! paired lines one off under a character diff and merged two lines into one (the splice
//! that hid in `write --base` up to v0.9.7).
//!
//! A clean merge must (a conflict always passes: refusing the write is safe):
//! 1. hold only whole lines: every line of the result is a line of base, ours or theirs
//!    (a last line without its break counts as that line), checked here;
//! 2. pass the line-LCS rule over every alignment (`keepboth::check_lcs`, the fuzz crate's
//!    own LCS tables): no deleted line back, no line lost.

use std::collections::HashSet;

use arbitrary::{Arbitrary, Unstructured};

/// One line of the pool: a start ("p" + one of three participants + ":" + a counter) and,
/// one time in four, a word after it.
#[derive(Debug, Clone, Copy, Arbitrary)]
pub struct Line {
    p: u8,
    n: u8,
    w: u8,
}

impl Line {
    fn render(self) -> String {
        let mut s = format!("p{:02}:{}", self.p % 3 + 1, self.n % 12);
        if self.w.is_multiple_of(4) {
            s.push_str(&format!(" w{}", self.w / 4 % 5));
        }
        s
    }
}

#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum Side {
    Ours,
    Theirs,
    Both,
}

/// What happens at one base line (and the gap before it).
#[derive(Debug, Clone, Copy, Arbitrary)]
pub enum Edit {
    Keep,
    Delete(Side),
    Rewrite(Side, Line),
    /// Each side rewrites the line differently.
    RewriteEach(Line, Line),
    /// Each side changes one word of the line (its start "pNN:N" is word 0).
    Words(u8, u8),
    /// A whole new line before it, on one side or both; the base line is kept.
    Insert(Side, Line),
    /// Both insert a new line before it, different ones.
    InsertEach(Line, Line),
}

#[derive(Debug, Clone, Arbitrary)]
pub struct Case {
    base: Vec<Line>,
    edits: Vec<Edit>,
    /// Lines appended at the end: by ours, then by theirs.
    end_ours: Vec<Line>,
    end_theirs: Vec<Line>,
    /// The three texts end without a line break.
    open: bool,
}

const MAX_LINES: usize = 16;

fn with_word(line: &str, k: u8, w: &str) -> String {
    let mut words: Vec<&str> = line.split(' ').collect();
    let i = k as usize % (words.len() + 1);
    if i < words.len() {
        words[i] = w;
    } else {
        words.push(w);
    }
    words.join(" ")
}

impl Case {
    /// Base, ours and theirs.
    pub fn texts(&self) -> (String, String, String) {
        let base: Vec<String> = self
            .base
            .iter()
            .take(MAX_LINES)
            .map(|l| l.render())
            .collect();
        let (mut ours, mut theirs) = (Vec::new(), Vec::new());
        let on = |s: Side| {
            (
                matches!(s, Side::Ours | Side::Both),
                matches!(s, Side::Theirs | Side::Both),
            )
        };
        for (i, b) in base.iter().enumerate() {
            match self.edits.get(i).copied().unwrap_or(Edit::Keep) {
                Edit::Keep => {
                    ours.push(b.clone());
                    theirs.push(b.clone());
                }
                Edit::Delete(s) => {
                    let (o, t) = on(s);
                    if !o {
                        ours.push(b.clone());
                    }
                    if !t {
                        theirs.push(b.clone());
                    }
                }
                Edit::Rewrite(s, l) => {
                    let (o, t) = on(s);
                    ours.push(if o { l.render() } else { b.clone() });
                    theirs.push(if t { l.render() } else { b.clone() });
                }
                Edit::RewriteEach(x, y) => {
                    ours.push(x.render());
                    theirs.push(y.render());
                }
                Edit::Words(k, j) => {
                    ours.push(with_word(b, k, "wo"));
                    theirs.push(with_word(b, j, "wt"));
                }
                Edit::Insert(s, l) => {
                    let (o, t) = on(s);
                    if o {
                        ours.push(l.render());
                    }
                    if t {
                        theirs.push(l.render());
                    }
                    ours.push(b.clone());
                    theirs.push(b.clone());
                }
                Edit::InsertEach(x, y) => {
                    ours.push(x.render());
                    theirs.push(y.render());
                    ours.push(b.clone());
                    theirs.push(b.clone());
                }
            }
        }
        ours.extend(self.end_ours.iter().take(4).map(|l| l.render()));
        theirs.extend(self.end_theirs.iter().take(4).map(|l| l.render()));
        let join = |ls: &[String]| {
            let mut s: String = ls.iter().map(|l| format!("{l}\n")).collect();
            if self.open {
                s.pop();
            }
            s
        };
        (join(&base), join(&ours), join(&theirs))
    }
}

impl Case {
    pub fn from_bytes(data: &[u8]) -> Option<Case> {
        Case::arbitrary_take_rest(Unstructured::new(data)).ok()
    }
}

fn key(l: &str) -> &str {
    l.strip_suffix('\n').unwrap_or(l)
}

/// Check 1: every line of a clean merge is a whole line of base, ours or theirs. The error
/// says whether the line is two known lines end to end (a join) or neither (a splice).
pub fn whole_lines(base: &str, ours: &str, theirs: &str, result: &str) -> Result<(), String> {
    let known: HashSet<&str> = [base, ours, theirs]
        .iter()
        .flat_map(|t| t.split_inclusive('\n'))
        .map(key)
        .collect();
    for l in result.split_inclusive('\n').map(key) {
        if known.contains(l) {
            continue;
        }
        let join = l
            .char_indices()
            .skip(1)
            .any(|(i, _)| known.contains(&l[..i]) && known.contains(&l[i..]));
        return Err(format!(
            "(whole lines) {} in a clean merge: {l:?}",
            if join {
                "two lines joined"
            } else {
                "a line spliced"
            }
        ));
    }
    Ok(())
}

/// Both checks on a clean merge's text.
pub fn judge(base: &str, ours: &str, theirs: &str, result: &str) -> Result<(), String> {
    whole_lines(base, ours, theirs, result)?;
    crate::keepboth::check_lcs(base, ours, theirs, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spliced_or_joined_line_fails_and_whole_lines_pass() {
        let (b, o, t) = ("p01:1\np02:2\n", "p01:1 wo\np02:2\n", "p01:1\np02:2 wt\n");
        whole_lines(b, o, t, "p01:1 wo\np02:2 wt\n").unwrap();
        let e = whole_lines(b, o, t, "p01:1 wo wt\np02:2\n").unwrap_err();
        assert!(e.contains("spliced"), "{e}");
        let e = whole_lines(b, o, t, "p01:1p02:2\n").unwrap_err();
        assert!(e.contains("joined"), "{e}");
    }

    #[test]
    fn a_case_renders_three_texts() {
        let c = Case {
            base: vec![Line { p: 0, n: 1, w: 1 }, Line { p: 1, n: 2, w: 0 }],
            edits: vec![Edit::Words(1, 3), Edit::Delete(Side::Theirs)],
            end_ours: vec![Line { p: 2, n: 3, w: 1 }],
            end_theirs: vec![],
            open: false,
        };
        let (b, o, t) = c.texts();
        assert_eq!(b, "p01:1\np02:2 w0\n");
        assert_eq!(o, "p01:1 wo\np02:2 w0\np03:3\n");
        assert_eq!(t, "p01:1 wt\n");
    }
}
