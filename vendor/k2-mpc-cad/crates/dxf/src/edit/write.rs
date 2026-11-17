//! The edit verbs' writer: a plan of byte ranges over the version read, of three kinds only. A
//! value line replaced (the code line and both line endings never in the range), pairs inserted
//! before a pair, and whole pairs removed. New pairs are written as the file writes them: the
//! code padded when the file pads, the line ending of the pair they go before.

use crate::error::{CadError, SELF_CHECK};
use crate::mutant;
use crate::structure::Structure;
use crate::token::Eol;

/// One range of the plan: bytes `lo..hi` of the version read replaced by `with` (`lo == hi`: an
/// insert).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub lo: usize,
    pub hi: usize,
    pub with: Vec<u8>,
}

/// The plan as it is built.
pub(crate) struct Writer<'s, 'a> {
    s: &'s Structure<'a>,
    edits: Vec<Splice>,
    /// Pairs inserted and removed, for the self-check's count.
    pub inserted: usize,
    pub removed: usize,
    padded: bool,
}

impl<'s, 'a> Writer<'s, 'a> {
    pub fn new(s: &'s Structure<'a>) -> Self {
        let (pad, plain) = s.tok.padding();
        Writer {
            s,
            edits: Vec::new(),
            inserted: 0,
            removed: 0,
            padded: pad > plain,
        }
    }

    /// Pair `i`'s value line becomes `value` (its code line and line endings stay). No range
    /// when the value is already that.
    pub fn set_value(&mut self, i: usize, value: &[u8]) {
        let p = &self.s.tok.pairs[i];
        if self.s.tok.value(i) == value {
            return;
        }
        if mutant::active("SpanCoversCodeLine") {
            // the whole pair written again, in the writer's own style
            let eol = self.eol_before(i);
            let with = self.render(p.code, value, eol);
            self.edits.push(Splice {
                lo: p.start(),
                hi: p.end(),
                with,
            });
            return;
        }
        self.edits.push(Splice {
            lo: p.value_at as usize,
            hi: p.value_end as usize,
            with: value.to_vec(),
        });
    }

    /// `pairs` inserted before pair `i` (or at the end of the pairs when `i` is their count).
    pub fn insert_before(&mut self, i: usize, pairs: &[(i32, Vec<u8>)]) {
        if pairs.is_empty() {
            return;
        }
        let at = match self.s.tok.pairs.get(i) {
            Some(p) => p.start(),
            None => self.s.tok.tail_at,
        };
        let eol = self.eol_before(i);
        let mut with = Vec::new();
        for (c, v) in pairs {
            with.extend_from_slice(&self.render(*c, v, eol));
        }
        self.inserted += pairs.len();
        self.edits.push(Splice {
            lo: at,
            hi: at,
            with,
        });
    }

    /// Pairs `first..end` removed, whole.
    pub fn remove(&mut self, first: usize, end: usize) {
        if end <= first {
            return;
        }
        let tp = &self.s.tok.pairs;
        self.removed += end - first;
        self.edits.push(Splice {
            lo: tp[first].start(),
            hi: tp[end - 1].end(),
            with: Vec::new(),
        });
    }

    /// The line ending new pairs before pair `i` take: that pair's code line's (the last pair's
    /// value line's at the end); LF when the file has none.
    fn eol_before(&self, i: usize) -> Eol {
        let p = &self.s.tok.pairs;
        let e = match p.get(i) {
            Some(x) => x.code_eol,
            None => p.last().map_or(Eol::Lf, |x| x.value_eol),
        };
        if e == Eol::None || mutant::active("EditStyleLeaks") {
            Eol::Lf
        } else {
            e
        }
    }

    /// One pair as the file writes pairs.
    fn render(&self, code: i32, value: &[u8], eol: Eol) -> Vec<u8> {
        let mut v = Vec::with_capacity(value.len() + 8);
        if self.padded && !mutant::active("EditStyleLeaks") {
            v.extend_from_slice(format!("{code:>3}").as_bytes());
        } else {
            v.extend_from_slice(code.to_string().as_bytes());
        }
        v.extend_from_slice(eol.bytes());
        v.extend_from_slice(value);
        v.extend_from_slice(eol.bytes());
        v
    }

    /// The plan, in byte order: an insert before a range that starts where it does, none
    /// overlapping (two that do are this writer's bug, refused as the self-check's).
    pub fn finish(self) -> Result<Vec<Splice>, CadError> {
        let mut e = self.edits;
        // stable: inserts at one place keep the order they were made in
        e.sort_by_key(|x| (x.lo, x.hi != x.lo));
        for w in e.windows(2) {
            if w[1].lo < w[0].hi || (w[1].lo == w[0].lo && w[0].hi != w[0].lo) {
                return Err(CadError::new(
                    SELF_CHECK,
                    "two changes of the plan fall on one place",
                    "nothing was written; report it with the file",
                ));
            }
        }
        Ok(e)
    }
}

/// `bytes` with `plan` applied.
pub fn apply(bytes: &[u8], plan: &[Splice]) -> Vec<u8> {
    let mut out =
        Vec::with_capacity(bytes.len() + plan.iter().map(|s| s.with.len()).sum::<usize>());
    let mut at = 0;
    for s in plan {
        out.extend_from_slice(&bytes[at..s.lo]);
        out.extend_from_slice(&s.with);
        at = s.hi;
    }
    out.extend_from_slice(&bytes[at..]);
    out
}
