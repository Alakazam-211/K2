//! Provenance of restored text (undo spec v1.8, U23): every live element's **root**.
//!
//! Elements are named by the engine's element ID ([`ElemId`]: client and clock, the clock in
//! the engine's UTF-16 units; a character's ID is its first unit's). An element inserted by
//! an edit, `write --base`, `replace_text`, `restore`, an import or an external change is
//! **its own root**. An element an undo, redo or revert inserts puts back an element `d`
//! that the reversed change removed, and records `root(d)`, path-compressed when it is made:
//! roots are never chains. Only the kit stamps it (its own undo, redo and revert); nothing
//! from a client ever does (`sec.client_provenance_refused`).
//!
//! **The live map** ([`Prov`]): per text root, runs aligned with the live text, each
//! `{bytes, units, id, root}` (the character at UTF-16 offset `k` of a run has ID `id + k`
//! and, for a restored run, root `root + k`; `root: None` is "its own root"). Runs live in
//! chunks of at most [`CHUNK`] runs behind `Arc`s: applying a hunk finds its chunk by a scan
//! of the chunks' byte totals (O(runs / [`CHUNK`])), then splices only the chunks it touches
//! (O([`CHUNK`]) plus the runs it removes), so a keystroke's cost never grows with the doc's
//! characters; a clone (a plan's scratch copy) copies the chunk pointers only, and a write
//! to a shared chunk copies that chunk (copy on write).
//!
//! **`by_root`**: the live *restored* elements by root, as runs keyed by their first root
//! (O(log n) per run). A fresh root, once deleted, is an engine tombstone forever, so "root
//! `r` is live" is "`r`'s original is live" (impossible while `r` is being put back: only a
//! change that removed it can put it back) or "a restoration of `r` is live", which this
//! index answers. Invariant **I1** (at most one live element per root) is asserted on every
//! insertion into it.
//!
//! The map is built from the engine's blame runs and the store's provenance rows the first
//! time a session needs it after a load (O(doc): the engine walks a throwaway copy), then
//! kept exact by every change as it applies: the session's own ([`Prov::apply`]) and, since
//! v0.9.5, outside edits ([`Prov::apply_runs`], from the change at element identity). A
//! path that replaces the doc (a recovery, a purge) drops it, to be rebuilt on next need.
//! Blame checks the kept map against a rebuild ([`Prov::differs`]) on a sample of calls.

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::text::diff::Hunk;

/// An element's ID: the engine's (client, clock); clocks count UTF-16 code units.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub(crate) struct ElemId {
    pub client: u64,
    pub clock: u32,
}

impl ElemId {
    pub fn new(client: u64, clock: u32) -> Self {
        Self { client, clock }
    }

    /// The ID `units` UTF-16 units further on in the same client's run.
    pub fn plus(self, units: u32) -> Self {
        Self {
            client: self.client,
            clock: self.clock + units,
        }
    }
}

fn units(s: &str) -> u32 {
    s.encode_utf16().count() as u32
}

/// A run of characters: `bytes` of text (`units` UTF-16 units) whose character at UTF-16
/// offset `k` has ID (or root) `start + k`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IdRun {
    pub bytes: u32,
    pub units: u32,
    pub start: ElemId,
}

/// One ID (or root) per character of a text, as runs, normalised (two runs are joined
/// when the second continues the first), so two equal sequences are equal values.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Ids(Vec<IdRun>);

impl Ids {
    /// `text`'s characters as one run of consecutive IDs from `start`.
    pub fn fresh(text: &str, start: ElemId) -> Self {
        let mut out = Self::default();
        out.push(IdRun {
            bytes: text.len() as u32,
            units: units(text),
            start,
        });
        out
    }

    pub fn runs(&self) -> &[IdRun] {
        &self.0
    }

    pub fn from_runs(runs: impl IntoIterator<Item = IdRun>) -> Self {
        let mut out = Self::default();
        for r in runs {
            out.push(r);
        }
        out
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The text length these IDs are aligned with, in bytes.
    pub fn bytes(&self) -> usize {
        self.0.iter().map(|r| r.bytes as usize).sum()
    }

    /// Memory held, for the undo budget (U6).
    pub fn mem(&self) -> usize {
        self.0.len() * std::mem::size_of::<IdRun>()
    }

    pub fn push(&mut self, r: IdRun) {
        if r.bytes == 0 {
            return;
        }
        if let Some(l) = self.0.last_mut() {
            if l.start.client == r.start.client && l.start.clock + l.units == r.start.clock {
                l.bytes += r.bytes;
                l.units += r.units;
                return;
            }
        }
        self.0.push(r);
    }

    pub fn extend(&mut self, other: &Ids) {
        for r in &other.0 {
            self.push(*r);
        }
    }

    /// The IDs of `text[lo..hi]`, `text` being the text these are aligned with.
    pub fn slice(&self, text: &str, lo: usize, hi: usize) -> Ids {
        debug_assert_eq!(self.bytes(), text.len(), "ids aligned with their text");
        let mut out = Ids::default();
        let mut at = 0usize;
        for r in &self.0 {
            let (s, e) = (at, at + r.bytes as usize);
            at = e;
            if e <= lo || s >= hi {
                if s >= hi {
                    break;
                }
                continue;
            }
            let (a, b) = (s.max(lo), e.min(hi));
            let skip = units(&text[s..a]);
            out.push(IdRun {
                bytes: (b - a) as u32,
                units: units(&text[a..b]),
                start: r.start.plus(skip),
            });
        }
        out
    }

    /// One ID per character of `text` (the text these are aligned with).
    pub fn chars(&self, text: &str) -> Vec<ElemId> {
        debug_assert_eq!(self.bytes(), text.len(), "ids aligned with their text");
        let mut out = Vec::with_capacity(text.len());
        let mut at = 0usize;
        for r in &self.0 {
            let mut k = 0u32;
            for ch in text[at..at + r.bytes as usize].chars() {
                out.push(r.start.plus(k));
                k += ch.len_utf16() as u32;
            }
            at += r.bytes as usize;
        }
        out
    }

    /// From one ID per character of `text`.
    pub fn from_chars(text: &str, ids: &[ElemId]) -> Ids {
        let mut out = Ids::default();
        for (ch, id) in text.chars().zip(ids) {
            out.push(IdRun {
                bytes: ch.len_utf8() as u32,
                units: ch.len_utf16() as u32,
                start: *id,
            });
        }
        debug_assert_eq!(out.bytes(), text.len());
        out
    }

    /// These IDs after `hunks` (canonical, on `text`) replaced their ranges with the
    /// elements `inserted` (one per hunk, aligned with its insert).
    pub fn apply(&self, text: &str, hunks: &[Hunk], inserted: &[Ids]) -> Ids {
        debug_assert_eq!(hunks.len(), inserted.len());
        let mut out = Ids::default();
        let mut at = 0usize;
        for (h, ins) in hunks.iter().zip(inserted) {
            out.extend(&self.slice(text, at, h.start));
            debug_assert_eq!(ins.bytes(), h.insert.len());
            out.extend(ins);
            at = h.end;
        }
        out.extend(&self.slice(text, at, text.len()));
        out
    }
}

/// Coalesce touching hunks (as [`crate::text::diff::coalesce`]) and their inserted IDs with
/// them, in step.
pub(crate) fn coalesce_with(hunks: Vec<Hunk>, ids: Vec<Ids>) -> (Vec<Hunk>, Vec<Ids>) {
    debug_assert_eq!(hunks.len(), ids.len());
    let mut hs: Vec<Hunk> = Vec::with_capacity(hunks.len());
    let mut is: Vec<Ids> = Vec::with_capacity(ids.len());
    for (h, i) in hunks.into_iter().zip(ids) {
        match hs.last_mut() {
            Some(l) if l.end == h.start => {
                l.end = h.end;
                l.insert.push_str(&h.insert);
                is.last_mut().expect("in step").extend(&i);
            }
            _ => {
                hs.push(h);
                is.push(i);
            }
        }
    }
    debug_assert_eq!(hs, crate::text::diff::coalesce(hs.clone()));
    (hs, is)
}

/// One run of live text: `bytes` (`units` UTF-16 units) whose character at UTF-16 offset `k`
/// has ID `id + k` and root `root + k` (`None`: each is its own root).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Run {
    pub bytes: u32,
    pub units: u32,
    pub id: ElemId,
    pub root: Option<ElemId>,
}

impl Run {
    /// The part of this run from byte `a` to byte `b` of it; `text` is its text.
    fn slice(&self, text: &str, a: usize, b: usize) -> Run {
        let skip = units(&text[..a]);
        Run {
            bytes: (b - a) as u32,
            units: units(&text[a..b]),
            id: self.id.plus(skip),
            root: self.root.map(|r| r.plus(skip)),
        }
    }

    /// Does `next` continue this run (both its IDs and its roots)?
    fn continued_by(&self, next: &Run) -> bool {
        self.id.client == next.id.client
            && self.id.clock + self.units == next.id.clock
            && match (self.root, next.root) {
                (None, None) => true,
                (Some(a), Some(b)) => a.client == b.client && a.clock + self.units == b.clock,
                _ => false,
            }
    }

    /// The roots of this run as an ID run.
    fn root_run(&self) -> IdRun {
        IdRun {
            bytes: self.bytes,
            units: self.units,
            start: self.root.unwrap_or(self.id),
        }
    }
}

/// At most this many runs per chunk of a [`TextMap`].
pub(crate) const CHUNK: usize = 64;

#[derive(Debug, Clone, Default)]
struct Chunk {
    bytes: usize,
    runs: Vec<Run>,
}

/// One text root's live runs, chunked (module docs).
#[derive(Debug, Clone, Default)]
pub(crate) struct TextMap {
    chunks: Vec<Arc<Chunk>>,
}

/// At most this many bytes per run of a [`TextMap`]: cutting a run inside costs a scan of
/// its text up to the cut (its UTF-16 offset), so a long run would make one keystroke
/// inside it cost the run's length.
pub(crate) const MAX_RUN: usize = 4096;

/// `r` (whose text is `text`) cut into runs of at most about [`MAX_RUN`] bytes.
fn split_long(text: &str, r: Run, out: &mut Vec<Run>) {
    if (r.bytes as usize) <= MAX_RUN {
        push_run(out, r);
        return;
    }
    let mut a = 0usize;
    while a < text.len() {
        let mut b = (a + MAX_RUN).min(text.len());
        while !text.is_char_boundary(b) {
            b -= 1;
        }
        // a piece starts a new run: never joined back past the bound
        out.push(r.slice(text, a, b));
        a = b;
    }
}

fn push_run(out: &mut Vec<Run>, r: Run) {
    if r.bytes == 0 {
        return;
    }
    match out.last_mut() {
        Some(l) if l.continued_by(&r) && (l.bytes + r.bytes) as usize <= MAX_RUN => {
            l.bytes += r.bytes;
            l.units += r.units;
        }
        _ => out.push(r),
    }
}

fn chunked(runs: Vec<Run>) -> Vec<Arc<Chunk>> {
    runs.chunks(CHUNK)
        .map(|c| {
            Arc::new(Chunk {
                bytes: c.iter().map(|r| r.bytes as usize).sum(),
                runs: c.to_vec(),
            })
        })
        .collect()
}

impl TextMap {
    /// From runs covering `text` exactly.
    pub fn from_runs(text: &str, runs: impl IntoIterator<Item = Run>) -> Self {
        let mut v = Vec::new();
        let mut at = 0usize;
        for r in runs {
            let e = at + r.bytes as usize;
            split_long(&text[at..e], r, &mut v);
            at = e;
        }
        debug_assert_eq!(at, text.len(), "the runs cover the text");
        Self { chunks: chunked(v) }
    }

    pub fn bytes(&self) -> usize {
        self.chunks.iter().map(|c| c.bytes).sum()
    }

    /// The runs, normalised: every run joined with the next when that continues it (no
    /// [`MAX_RUN`] or chunk cuts), so two maps of the same elements are equal values.
    fn normal(&self) -> Vec<Run> {
        let mut out: Vec<Run> = Vec::new();
        for r in self.chunks.iter().flat_map(|c| &c.runs) {
            match out.last_mut() {
                Some(l) if l.continued_by(r) => {
                    l.bytes += r.bytes;
                    l.units += r.units;
                }
                _ => out.push(*r),
            }
        }
        out
    }

    /// The runs of `text[lo..hi]` (`text`: the live text this map is aligned with), clipped.
    pub fn query(&self, text: &str, lo: usize, hi: usize) -> Vec<Run> {
        debug_assert_eq!(self.bytes(), text.len(), "the map is aligned with the text");
        let mut out = Vec::new();
        let mut at = 0usize;
        for c in &self.chunks {
            if at + c.bytes <= lo {
                at += c.bytes;
                continue;
            }
            if at >= hi {
                break;
            }
            for r in &c.runs {
                let (s, e) = (at, at + r.bytes as usize);
                at = e;
                if e <= lo {
                    continue;
                }
                if s >= hi {
                    break;
                }
                let (a, b) = (s.max(lo), e.min(hi));
                push_run(&mut out, r.slice(&text[s..e], a - s, b - s));
            }
        }
        out
    }

    /// The roots of `text[lo..hi]`.
    pub fn roots(&self, text: &str, lo: usize, hi: usize) -> Ids {
        Ids::from_runs(self.query(text, lo, hi).iter().map(Run::root_run))
    }

    /// Replace `before[s..e]` with the runs `ins`; returns the runs removed. `before` is the
    /// text this map is aligned with up to `e` (the caller applies hunks back to front, so
    /// everything before `e` is still `before`'s).
    fn splice(&mut self, before: &str, s: usize, e: usize, ins: &[Run]) -> Vec<Run> {
        // the chunks [ci, cj) holding every run that meets [s, e), and the insertion point
        // (at the text's end: the last chunk)
        let n = self.chunks.len();
        let (mut ci, mut base, mut at) = (n, 0usize, 0usize);
        for (k, c) in self.chunks.iter().enumerate() {
            if at + c.bytes > s {
                (ci, base) = (k, at);
                break;
            }
            at += c.bytes;
        }
        if ci == n && n > 0 {
            ci = n - 1;
            base = at - self.chunks[ci].bytes;
        }
        let (mut cj, mut end) = (ci, base);
        while cj < n {
            end += self.chunks[cj].bytes;
            cj += 1;
            if end >= e {
                break;
            }
        }
        let mut out: Vec<Run> = Vec::new();
        let mut removed = Vec::new();
        let mut placed = false;
        let mut p = base;
        for c in &self.chunks[ci..cj] {
            for &r in &c.runs {
                let (rs, re) = (p, p + r.bytes as usize);
                p = re;
                if re <= s {
                    push_run(&mut out, r);
                    continue;
                }
                // the part `a..b` of this run. Only text before `e` is still `before`'s (a
                // later hunk, applied first, may have joined new elements onto this run past
                // `e`), so a cut reads `before` up to `e` at most: the part after `e` takes
                // the run's remaining units
                let part = |a: usize, b: usize| {
                    if a == rs && b == re {
                        return r;
                    }
                    let skip = units(&before[rs..a]);
                    Run {
                        bytes: (b - a) as u32,
                        units: if b == re {
                            r.units - skip
                        } else {
                            units(&before[a..b])
                        },
                        id: r.id.plus(skip),
                        root: r.root.map(|x| x.plus(skip)),
                    }
                };
                if rs < s {
                    push_run(&mut out, part(rs, s));
                }
                if !placed {
                    for x in ins {
                        push_run(&mut out, *x);
                    }
                    placed = true;
                }
                if rs < e {
                    removed.push(part(rs.max(s), re.min(e)));
                }
                if re > e {
                    push_run(&mut out, part(rs.max(e), re));
                }
            }
        }
        if !placed {
            for x in ins {
                push_run(&mut out, *x);
            }
        }
        // keep chunks from fragmenting: a short result takes in its short neighbours (their
        // runs are whole, past the change or before it)
        while out.len() < CHUNK / 2 && cj < n && self.chunks[cj].runs.len() < CHUNK / 2 {
            for &r in &self.chunks[cj].runs {
                push_run(&mut out, r);
            }
            cj += 1;
        }
        while out.len() < CHUNK / 2 && ci > 0 && self.chunks[ci - 1].runs.len() < CHUNK / 2 {
            ci -= 1;
            let mut v = self.chunks[ci].runs.clone();
            for r in out {
                push_run(&mut v, r);
            }
            out = v;
        }
        self.chunks.splice(ci..cj, chunked(out));
        removed
    }
}

/// The live restored elements by root, as runs keyed by their first root: `(units, first
/// live ID)`. The element of root `r` inside the run starting at root `s` is `live + (r - s)`.
#[derive(Debug, Clone, Default)]
pub(crate) struct ByRoot(BTreeMap<ElemId, (u32, ElemId)>);

impl ByRoot {
    /// The live element restoring root `r`, if any.
    pub fn get(&self, r: ElemId) -> Option<ElemId> {
        let (s, (n, live)) = self.0.range(..=r).next_back()?;
        (s.client == r.client && r.clock < s.clock + n).then(|| live.plus(r.clock - s.clock))
    }

    /// Is any root of `start .. start + units` live here?
    pub fn any(&self, start: ElemId, units: u32) -> bool {
        let end = start.plus(units);
        if self.get(start).is_some() {
            return true;
        }
        self.0
            .range(start..end)
            .next()
            .is_some_and(|(s, _)| s.client == start.client)
    }

    fn insert(&mut self, root: ElemId, units: u32, live: ElemId) {
        if units == 0 {
            return;
        }
        // I1: at most one live element per root
        debug_assert!(
            !self.any(root, units),
            "I1: root {root:?}+{units} is already live (restored again as {live:?})"
        );
        self.0.insert(root, (units, live));
    }

    fn remove(&mut self, root: ElemId, units: u32, live: ElemId) {
        let end = root.clock + units;
        let mut keys: Vec<ElemId> = Vec::new();
        if let Some((s, (n, _))) = self.0.range(..root).next_back() {
            if s.client == root.client && s.clock + n > root.clock {
                keys.push(*s);
            }
        }
        keys.extend(
            self.0
                .range(root..ElemId::new(root.client, end))
                .map(|(s, _)| *s),
        );
        let mut got = 0u32;
        for s in keys {
            let (n, l) = self.0.remove(&s).expect("just found");
            let (a, b) = (s.clock.max(root.clock), (s.clock + n).min(end));
            debug_assert_eq!(
                l.plus(a - s.clock),
                live.plus(a - root.clock),
                "by_root names another live element"
            );
            got += b - a;
            if s.clock < a {
                self.0.insert(s, (a - s.clock, l));
            }
            if s.clock + n > b {
                self.0.insert(
                    ElemId::new(s.client, b),
                    (s.clock + n - b, l.plus(b - s.clock)),
                );
            }
        }
        debug_assert_eq!(got, units, "a removed restoration wasn't in by_root");
    }

    /// Normalised: each run joined with the next when both its roots and its live IDs
    /// continue it.
    fn normal(&self) -> Vec<(ElemId, u32, ElemId)> {
        let mut out: Vec<(ElemId, u32, ElemId)> = Vec::new();
        for (s, (n, l)) in &self.0 {
            match out.last_mut() {
                Some((ps, pn, pl))
                    if ps.client == s.client
                        && ps.clock + *pn == s.clock
                        && pl.client == l.client
                        && pl.clock + *pn == l.clock =>
                {
                    *pn += n
                }
                _ => out.push((*s, *n, *l)),
            }
        }
        out
    }

    #[cfg(test)]
    pub fn len_runs(&self) -> usize {
        self.0.len()
    }
}

/// What one hunk of an applied change inserted: its first element ID (the rest follow
/// consecutively), and the roots it restores (`None`: fresh elements, each its own root).
#[derive(Debug, Clone)]
pub(crate) struct Inserted {
    pub id: ElemId,
    pub roots: Option<Ids>,
}

/// The live map of a doc's text roots, and the index of its live restored elements.
#[derive(Debug, Clone, Default)]
pub(crate) struct Prov {
    texts: Vec<(&'static str, TextMap)>,
    by_root: ByRoot,
    /// Which build this map descends from (unique per [`Prov::build`]), and how many
    /// changes it has taken since: [`Prov::token`].
    lineage: u64,
    applied: u64,
}

/// A map's identity at one moment ([`Prov::token`]): equal tokens, the same map with no
/// change applied in between (blame's off-lock check, v0.9.7).
pub(crate) type ProvToken = (u64, u64);

impl Prov {
    /// Build from each text root's live text and runs (`(root, text, runs)`, the runs
    /// covering the text).
    pub fn build(texts: Vec<(&'static str, &str, Vec<Run>)>) -> Self {
        let mut by_root = ByRoot::default();
        let mut out = Vec::with_capacity(texts.len());
        for (name, text, runs) in texts {
            let map = TextMap::from_runs(text, runs);
            for c in &map.chunks {
                for r in &c.runs {
                    if let Some(root) = r.root {
                        by_root.insert(root, r.units, r.id);
                    }
                }
            }
            out.push((name, map));
        }
        static LINEAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            texts: out,
            by_root,
            lineage: LINEAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            applied: 0,
        }
    }

    /// This map's identity now: its build, and the changes applied since.
    pub fn token(&self) -> ProvToken {
        (self.lineage, self.applied)
    }

    /// Does it map text root `root`?
    pub fn has(&self, root: &str) -> bool {
        self.map(root).is_some()
    }

    fn map(&self, root: &str) -> Option<&TextMap> {
        self.texts.iter().find(|(n, _)| *n == root).map(|(_, m)| m)
    }

    /// The live runs of `text[lo..hi]` of text root `root` (`text` its live text).
    pub fn query(&self, root: &str, text: &str, lo: usize, hi: usize) -> Vec<Run> {
        self.map(root)
            .map_or_else(Vec::new, |m| m.query(text, lo, hi))
    }

    /// The roots of `text[lo..hi]` of text root `root`.
    pub fn roots(&self, root: &str, text: &str, lo: usize, hi: usize) -> Ids {
        self.map(root)
            .map_or_else(Ids::default, |m| m.roots(text, lo, hi))
    }

    /// Is any root of the run `r` live (a restoration of it is)?
    pub fn any_live(&self, r: &IdRun) -> bool {
        self.by_root.any(r.start, r.units)
    }

    /// The live element restoring root `r`, if any.
    #[cfg(test)]
    pub fn live_of(&self, r: ElemId) -> Option<ElemId> {
        self.by_root.get(r)
    }

    /// Apply a change's canonical `hunks` on text root `root` (`before`: its text before the
    /// change), each hunk inserting the elements `ins` describes. Keeps `by_root` exact:
    /// removed restorations leave it, inserted ones enter it (asserting I1).
    pub fn apply(&mut self, root: &str, before: &str, hunks: &[Hunk], ins: &[Inserted]) {
        debug_assert_eq!(hunks.len(), ins.len());
        let runs: Vec<Vec<Run>> = hunks
            .iter()
            .zip(ins)
            .map(|(h, x)| new_runs(&h.insert, x))
            .collect();
        self.apply_runs(root, before, hunks, &runs);
    }

    /// [`Prov::apply`] with each hunk's inserted elements given as runs covering its insert
    /// (any IDs, any roots).
    pub fn apply_runs(&mut self, root: &str, before: &str, hunks: &[Hunk], ins: &[Vec<Run>]) {
        debug_assert_eq!(hunks.len(), ins.len());
        let Some(k) = self.texts.iter().position(|(n, _)| *n == root) else {
            debug_assert!(false, "no map for root {root:?}");
            return;
        };
        self.applied += 1;
        let mutant = mutant();
        for (h, x) in hunks.iter().zip(ins).rev() {
            let runs = {
                let mut v = Vec::new();
                let mut at = 0usize;
                for r in x {
                    let e = at + r.bytes as usize;
                    let mut r = *r;
                    if mutant == Some(Mutant::RestoredAsRestorer) {
                        // planted: a restored element blamed to its restorer, not its root
                        r.root = None;
                    }
                    split_long(&h.insert[at..e], r, &mut v);
                    at = e;
                }
                debug_assert_eq!(at, h.insert.len(), "inserted runs cover the insert");
                v
            };
            // planted: the deletion never reaches the map (it keeps the deleted elements)
            let end = if mutant == Some(Mutant::MissDeletion) {
                h.start
            } else {
                h.end
            };
            let removed = self.texts[k].1.splice(before, h.start, end, &runs);
            if mutant != Some(Mutant::MissDeletionInIndex) {
                for r in removed {
                    if let Some(root) = r.root {
                        self.by_root.remove(root, r.units, r.id);
                    }
                }
            }
            for r in runs {
                if let Some(root) = r.root {
                    self.by_root.insert(root, r.units, r.id);
                }
            }
        }
    }

    /// Is each text root's map aligned with its live text (`texts`), byte for byte? O(roots
    /// x chunks). A map that isn't is wrong for sure; one that is may still be wrong
    /// ([`Prov::differs`] says).
    pub fn aligned(&self, texts: &[(&'static str, String)]) -> bool {
        texts
            .iter()
            .all(|(name, text)| self.map(name).is_some_and(|m| m.bytes() == text.len()))
            && self.texts.len() == texts.len()
    }

    /// Where this map differs from `other` (a rebuild from the engine and the store), if it
    /// does: each root's runs (IDs and roots, compared character for character as
    /// normalised runs: chunking and run cuts aside) and the live restorations by root. A
    /// description with no doc content (a root name, a byte offset), or `None`: the same.
    /// O(runs).
    pub fn differs(&self, other: &Prov) -> Option<String> {
        if self.texts.len() != other.texts.len() {
            return Some("a different set of text roots".into());
        }
        for (name, m) in &self.texts {
            let Some(o) = other.map(name) else {
                return Some(format!("root {name:?} not in the rebuild"));
            };
            let (a, b) = (m.normal(), o.normal());
            if m.bytes() != o.bytes() {
                return Some(format!(
                    "root {name:?}: {} bytes mapped, the text has {}",
                    m.bytes(),
                    o.bytes()
                ));
            }
            if a != b {
                let mut at = 0usize;
                for (x, y) in a.iter().zip(&b) {
                    if x != y {
                        break;
                    }
                    at += x.bytes as usize;
                }
                return Some(format!("root {name:?}: the runs differ from byte {at}"));
            }
        }
        if self.by_root.normal() != other.by_root.normal() {
            return Some("the live restorations by root differ".into());
        }
        None
    }
}

/// The planted faults ([`crate::differential::ProvMutant`]): never compiled into a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mutant {
    MissDeletion,
    MissDeletionInIndex,
    RestoredAsRestorer,
}

#[cfg(any(test, feature = "differential"))]
fn mutant() -> Option<Mutant> {
    use crate::differential::ProvMutant as P;
    crate::differential::prov_mutant().map(|m| match m {
        P::MissDeletion => Mutant::MissDeletion,
        P::MissDeletionInIndex => Mutant::MissDeletionInIndex,
        P::RestoredAsRestorer => Mutant::RestoredAsRestorer,
    })
}

#[cfg(not(any(test, feature = "differential")))]
#[inline(always)]
fn mutant() -> Option<Mutant> {
    None
}

/// A text root's live runs from the engine's blame runs (`blame`, covering `text`) and the
/// store's provenance rows meeting each (`rows`, one list per blame run, in clock order):
/// an element a row covers restores the row's root at its offset; every other element is
/// its own root (it was never restored, or was made before store format 5).
pub(crate) fn live_runs(
    text: &str,
    blame: &[crate::engine::BlameRun],
    rows: &[Vec<crate::store::ProvRow>],
) -> Vec<Run> {
    debug_assert_eq!(blame.len(), rows.len());
    let mut out = Vec::new();
    let mut at = 0usize;
    for (b, rows) in blame.iter().zip(rows) {
        let mut clock = b.clock;
        let mut k = 0;
        for ch in text[at..at + b.bytes].chars() {
            while k < rows.len() && rows[k].end <= clock {
                k += 1;
            }
            let root = rows
                .get(k)
                .filter(|r| r.start <= clock && clock < r.end)
                .map(|r| ElemId::new(r.root.0, r.root.1 + (clock - r.start)));
            let u = ch.len_utf16() as u32;
            push_run(
                &mut out,
                Run {
                    bytes: ch.len_utf8() as u32,
                    units: u,
                    id: ElemId::new(b.client, clock),
                    root,
                },
            );
            clock += u;
        }
        at += b.bytes;
    }
    out
}

/// The runs a hunk inserts: one per root run (restorations), or one fresh run.
fn new_runs(text: &str, x: &Inserted) -> Vec<Run> {
    match &x.roots {
        None if text.is_empty() => Vec::new(),
        None => vec![Run {
            bytes: text.len() as u32,
            units: units(text),
            id: x.id,
            root: None,
        }],
        Some(roots) => {
            debug_assert_eq!(roots.bytes(), text.len(), "restored roots fit the text");
            let mut at = 0u32;
            roots
                .runs()
                .iter()
                .map(|r| {
                    let run = Run {
                        bytes: r.bytes,
                        units: r.units,
                        id: x.id.plus(at),
                        root: Some(r.start),
                    };
                    at += r.units;
                    run
                })
                .collect()
        }
    }
}

/// Each character of `text`, its element ID, from the engine's blame runs covering it;
/// `None` when the runs don't cover the text exactly.
pub(crate) fn elem_ids(text: &str, runs: &[crate::engine::BlameRun]) -> Option<Vec<ElemId>> {
    let mut out = Vec::with_capacity(text.len());
    let mut at = 0usize;
    for r in runs {
        let mut clock = r.clock;
        for ch in text.get(at..at + r.bytes)?.chars() {
            out.push(ElemId::new(r.client, clock));
            clock += ch.len_utf16() as u32;
        }
        at += r.bytes;
    }
    (at == text.len()).then_some(out)
}

/// The change from `old` to `new` **at element identity** (SPEC-UNDO, preamble: every
/// applied change is reported with each inserted or deleted element at its actual index),
/// from each character's element ID (`old_ids`, `new_ids`): an old element missing from
/// `new` is deleted, a new element missing from `old` is inserted where it stands, and the
/// surviving elements keep their order (an engine merge never reorders them). The hunks are
/// canonical (sorted; separated by at least one surviving element, so never touching) and
/// replay `old` into `new` byte for byte. `None` if the IDs say otherwise (survivors out of
/// order, a survivor whose character changed, or a replay that differs).
///
/// A diff of the two texts can't stand in for this: in repeated text it names another
/// element than the one the engine inserted or deleted (v0.9.3: an outside edit putting a
/// copy of a participant's line just before it was reported after it, and the participant's
/// undo region was carried onto the copy).
pub(crate) fn identity_hunks(
    old: &str,
    old_ids: &[ElemId],
    new: &str,
    new_ids: &[ElemId],
) -> Option<Vec<Hunk>> {
    use std::collections::HashSet;
    let o: Vec<(usize, char)> = old.char_indices().collect();
    let n: Vec<char> = new.chars().collect();
    if o.len() != old_ids.len() || n.len() != new_ids.len() {
        return None;
    }
    let in_old: HashSet<ElemId> = old_ids.iter().copied().collect();
    let in_new: HashSet<ElemId> = new_ids.iter().copied().collect();
    let byte = |i: usize| o.get(i).map_or(old.len(), |c| c.0);
    let (mut hunks, mut cur): (Vec<Hunk>, Option<Hunk>) = (Vec::new(), None);
    let (mut i, mut j) = (0usize, 0usize);
    loop {
        if i < o.len() && !in_new.contains(&old_ids[i]) {
            // deleted
            let at = byte(i);
            let h = cur.get_or_insert_with(|| Hunk {
                start: at,
                end: at,
                insert: String::new(),
            });
            h.end = at + o[i].1.len_utf8();
            i += 1;
        } else if j < n.len() && !in_old.contains(&new_ids[j]) {
            // inserted, where it stands
            let at = byte(i);
            cur.get_or_insert_with(|| Hunk {
                start: at,
                end: at,
                insert: String::new(),
            })
            .insert
            .push(n[j]);
            j += 1;
        } else if i < o.len() && j < n.len() {
            // a survivor on both sides: the same element, the same character
            if old_ids[i] != new_ids[j] || o[i].1 != n[j] {
                return None;
            }
            hunks.extend(cur.take());
            i += 1;
            j += 1;
        } else if i == o.len() && j == n.len() {
            break;
        } else {
            return None;
        }
    }
    hunks.extend(cur.take());
    (crate::text::diff::apply(old, &hunks) == new).then_some(hunks)
}

#[cfg(test)]
mod tests {
    /// `identity_hunks` names the elements the engine changed, where a diff of the texts
    /// names others in repeated text; and refuses IDs that don't describe a merge.
    #[test]
    fn identity_hunks_follow_the_elements() {
        let id = |c: u64, k: u32| ElemId::new(c, k);
        // old `A B A` (a0's B at 2); the merge put a copy `B A` before a0's B: `A [B A] B A`
        let old = "ABA";
        let old_ids = [id(1, 0), id(7, 0), id(1, 1)];
        let new = "ABABA";
        let new_ids = [id(1, 0), id(9, 0), id(9, 1), id(7, 0), id(1, 1)];
        let h = identity_hunks(old, &old_ids, new, &new_ids).unwrap();
        assert_eq!(
            h,
            vec![Hunk {
                start: 1,
                end: 1,
                insert: "BA".into()
            }]
        );
        // the diff of the texts puts it after a0's B
        assert_eq!(crate::text::diff::hunks(old, new)[0].start, 3);
        // deletions and insertions, multi-byte, two separate hunks
        let old = "aé日b";
        let old_ids = [id(1, 0), id(1, 1), id(1, 2), id(1, 3)];
        let new = "xé日yb";
        let new_ids = [id(2, 0), id(1, 1), id(1, 2), id(2, 1), id(1, 3)];
        let h = identity_hunks(old, &old_ids, new, &new_ids).unwrap();
        assert_eq!(crate::text::diff::apply(old, &h), new);
        assert_eq!(h.len(), 2);
        assert_eq!((h[0].start, h[0].end, h[0].insert.as_str()), (0, 1, "x"));
        assert_eq!((h[1].start, h[1].end, h[1].insert.as_str()), (6, 6, "y"));
        // survivors out of order, or a survivor whose character changed: refused
        assert!(identity_hunks("ab", &[id(1, 0), id(1, 1)], "ba", &[id(1, 1), id(1, 0)]).is_none());
        assert!(identity_hunks("ab", &[id(1, 0), id(1, 1)], "ac", &[id(1, 0), id(1, 1)]).is_none());
        // IDs that don't fit the text
        assert!(identity_hunks("ab", &[id(1, 0)], "ab", &[id(1, 0), id(1, 1)]).is_none());
    }

    use super::*;
    use proptest::prelude::*;

    /// The naive model: one `(char, id, root)` per live character.
    type El = (char, ElemId, Option<ElemId>);

    fn flat(text: &str, runs: &[Run]) -> Vec<El> {
        let mut out = Vec::new();
        let mut at = 0;
        for r in runs {
            let mut k = 0;
            for ch in text[at..at + r.bytes as usize].chars() {
                out.push((ch, r.id.plus(k), r.root.map(|x| x.plus(k))));
                k += ch.len_utf16() as u32;
            }
            at += r.bytes as usize;
        }
        out
    }

    #[test]
    fn ids_slice_apply_and_normalise() {
        let a = ElemId::new(5, 10);
        let t = "aé🎉b";
        let ids = Ids::fresh(t, a);
        assert_eq!(ids.runs().len(), 1);
        let s = ids.slice(t, 1, 7); // "é🎉"
        assert_eq!(s.runs()[0].start, a.plus(1));
        assert_eq!(s.runs()[0].units, 3);
        // per-character runs normalise back to one
        let chars = ids.chars(t);
        assert_eq!(Ids::from_chars(t, &chars), ids);
        // applying a hunk with its own inserted ids
        let h = vec![Hunk {
            start: 1,
            end: 3,
            insert: "XY".into(),
        }];
        let x = Ids::fresh("XY", ElemId::new(9, 0));
        let got = ids.apply(t, &h, &[x]);
        assert_eq!(got.bytes(), "aXY🎉b".len());
        assert_eq!(got.runs().len(), 3);
    }

    #[test]
    fn by_root_splits_and_asserts_i1() {
        let mut b = ByRoot::default();
        let r = ElemId::new(1, 0);
        b.insert(r, 4, ElemId::new(7, 100));
        assert_eq!(b.get(r.plus(2)), Some(ElemId::new(7, 102)));
        assert!(b.any(ElemId::new(1, 3), 5));
        assert!(!b.any(ElemId::new(1, 4), 5));
        assert!(!b.any(ElemId::new(2, 0), 5));
        b.remove(r.plus(1), 2, ElemId::new(7, 101));
        assert_eq!(b.get(r), Some(ElemId::new(7, 100)));
        assert_eq!(b.get(r.plus(1)), None);
        assert_eq!(b.get(r.plus(3)), Some(ElemId::new(7, 103)));
        assert_eq!(b.len_runs(), 2);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "I1")]
    fn two_live_restorations_of_one_root_panic() {
        let mut b = ByRoot::default();
        b.insert(ElemId::new(1, 0), 2, ElemId::new(7, 0));
        b.insert(ElemId::new(1, 1), 1, ElemId::new(8, 0));
    }

    /// Long runs are cut at [`MAX_RUN`] bytes (a cut inside one scans its text up to the
    /// cut), and scattered small edits don't fragment the chunks.
    #[test]
    fn long_runs_are_cut_and_chunks_stay_full() {
        let text: String = "é".repeat(3 * MAX_RUN);
        let id = ElemId::new(4, 0);
        let mut p = Prov::build(vec![(
            "r",
            text.as_str(),
            vec![Run {
                bytes: text.len() as u32,
                units: units(&text),
                id,
                root: None,
            }],
        )]);
        let runs = p.query("r", &text, 0, text.len());
        assert!(runs.len() >= 6, "{} runs", runs.len());
        let roots = p.roots("r", &text, 1000, 2000);
        assert_eq!(
            roots.runs(),
            &[IdRun {
                bytes: 1000,
                units: 500,
                start: id.plus(500)
            }]
        );
        // many scattered one-char insertions by other clients
        let mut text = text;
        for k in 0..400usize {
            let mut at = (k * 7919 * 2) % text.len();
            while !text.is_char_boundary(at) {
                at -= 1;
            }
            let h = Hunk {
                start: at,
                end: at,
                insert: "x".into(),
            };
            let x = Inserted {
                id: ElemId::new(100 + k as u64, k as u32),
                roots: None,
            };
            p.apply("r", &text, std::slice::from_ref(&h), &[x]);
            text.insert(at, 'x');
        }
        let map = &p.texts[0].1;
        let n: usize = map.chunks.iter().map(|c| c.runs.len()).sum();
        assert!(
            map.chunks.len() <= 2 * n / (CHUNK / 2) + 2,
            "{} chunks for {n} runs",
            map.chunks.len()
        );
        assert_eq!(map.bytes(), text.len());
    }

    fn ops() -> impl Strategy<Value = Vec<(u16, u8, String, bool)>> {
        prop::collection::vec(
            (any::<u16>(), 0u8..4, "[ab\né🎉]{0,3}", any::<bool>()),
            1..24,
        )
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        /// The chunked map equals the naive per-character model through random hunks, and
        /// `by_root` holds exactly the live restored characters.
        #[test]
        fn map_equals_the_naive_model(seed in "[ab\né]{0,6}", ops in ops()) {
            let mut text = seed.clone();
            let mut next = 0u32;
            let mut alloc = |n: u32| { let c = next; next += n; ElemId::new(3, c) };
            let first = alloc(units(&seed));
            let mut p = Prov::build(vec![("r", seed.as_str(), vec![Run {
                bytes: seed.len() as u32, units: units(&seed), id: first, root: None,
            }].into_iter().filter(|r| r.bytes > 0).collect())]);
            let mut model: Vec<El> = flat(&seed, &p.query("r", &text, 0, text.len()));
            // roots given out once, so restorations never repeat a root (I1)
            let mut root_clock = 0u32;
            for (pos, del, ins, restore) in ops {
                let chars: Vec<usize> = text.char_indices().map(|(i, _)| i)
                    .chain(std::iter::once(text.len())).collect();
                let si = pos as usize % chars.len();
                let ei = (si + del as usize).min(chars.len() - 1);
                let (s, e) = (chars[si], chars[ei]);
                if s == e && ins.is_empty() { continue; }
                let n = units(&ins);
                let id = alloc(n);
                let roots = restore.then(|| {
                    let r = Ids::fresh(&ins, ElemId::new(99, root_clock));
                    root_clock += n;
                    r
                });
                let h = Hunk { start: s, end: e, insert: ins.clone() };
                p.apply("r", &text, std::slice::from_ref(&h), &[Inserted { id, roots: roots.clone() }]);
                let new_els: Vec<El> = {
                    let rr = roots.as_ref().map(|r| r.chars(&ins));
                    let mut k = 0;
                    ins.chars().enumerate().map(|(i, ch)| {
                        let el = (ch, id.plus(k), rr.as_ref().map(|v| v[i]));
                        k += ch.len_utf16() as u32;
                        el
                    }).collect()
                };
                model.splice(si..ei, new_els);
                text.replace_range(s..e, &ins);
                let got = flat(&text, &p.query("r", &text, 0, text.len()));
                prop_assert_eq!(&got, &model);
                // every live restored char is in by_root, and nothing else is
                for (_, id, root) in &model {
                    if let Some(r) = root {
                        prop_assert_eq!(p.live_of(*r), Some(*id));
                    }
                }
                let live: usize = model.iter().filter(|x| x.2.is_some()).count();
                let indexed: u32 = p.by_root.0.values().map(|(n, _)| *n).sum();
                let live_units: u32 = model.iter().filter(|x| x.2.is_some())
                    .map(|x| x.0.len_utf16() as u32).sum();
                prop_assert_eq!(indexed, live_units, "{} live restored chars", live);
                // a sub-range query is the model's slice
                let lo = chars[si.min(chars.len() - 1)].min(text.len());
                let sub = flat(&text[lo..], &p.query("r", &text, lo, text.len()));
                let skip = text[..lo].chars().count();
                prop_assert_eq!(&sub[..], &model[skip..]);
            }
        }

        /// Multi-hunk changes by one client, applied back to front with clocks in the
        /// engine adapter's order (`insert_clocks`): a later hunk's insertion can join the
        /// client's own run that an earlier hunk then cuts (the case a cut must not read
        /// `before` past the earlier hunk's end for).
        #[test]
        fn multi_hunk_changes_equal_the_naive_model(
            seed in "[ab\né]{0,8}",
            ops in prop::collection::vec(
                prop::collection::vec((any::<u16>(), 0u8..3, "[ab\né🎉]{0,3}"), 1..4),
                1..12,
            ),
        ) {
            const C: u64 = 3;
            let mut text = seed.clone();
            let mut clock = units(&seed);
            let mut p = Prov::build(vec![("r", seed.as_str(), vec![Run {
                bytes: seed.len() as u32, units: units(&seed), id: ElemId::new(C, 0), root: None,
            }].into_iter().filter(|r| r.bytes > 0).collect())]);
            let mut model: Vec<El> = flat(&seed, &p.query("r", &text, 0, text.len()));
            for raw in ops {
                let chars: Vec<usize> = text.char_indices().map(|(i, _)| i)
                    .chain(std::iter::once(text.len())).collect();
                // canonical hunks: sorted, never touching, never empty
                let mut picks: Vec<(usize, usize, String)> = raw.into_iter().map(|(pos, del, ins)| {
                    let si = pos as usize % chars.len();
                    (si, (si + del as usize).min(chars.len() - 1), ins)
                }).collect();
                picks.sort_by_key(|x| (x.0, x.1));
                let mut hs: Vec<(usize, usize, String)> = Vec::new();
                for (si, ei, ins) in picks {
                    if hs.last().is_some_and(|l| si <= l.1) || (si == ei && ins.is_empty()) {
                        continue;
                    }
                    hs.push((si, ei, ins));
                }
                if hs.is_empty() { continue; }
                let hunks: Vec<Hunk> = hs.iter().map(|(si, ei, ins)| Hunk {
                    start: chars[*si], end: chars[*ei], insert: ins.clone(),
                }).collect();
                let edit = crate::doc::Edit::Text { root: "r".into(), hunks: hunks.clone() };
                let firsts = crate::engine::insert_clocks(clock, std::slice::from_ref(&edit)).remove(0);
                let ins: Vec<Inserted> = firsts.iter()
                    .map(|c| Inserted { id: ElemId::new(C, *c), roots: None }).collect();
                p.apply("r", &text, &hunks, &ins);
                for ((si, ei, s), c) in hs.iter().zip(&firsts).rev() {
                    let mut k = 0;
                    let els: Vec<El> = s.chars().map(|ch| {
                        let el = (ch, ElemId::new(C, c + k), None);
                        k += ch.len_utf16() as u32;
                        el
                    }).collect();
                    model.splice(*si..*ei, els);
                }
                clock += hunks.iter().map(|h| units(&h.insert)).sum::<u32>();
                text = crate::text::diff::apply(&text, &hunks);
                let got = flat(&text, &p.query("r", &text, 0, text.len()));
                prop_assert_eq!(&got, &model);
            }
        }
    }
}
