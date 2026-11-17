//! The three-way merge of a DXF file by unit and field: what an app's save (theirs) and the
//! doc's own versions (ours) changed since the version both started from (the base), merged
//! by handle, never by line.
//!
//! **Units**, each read on all three sides by its key: header variables (by name), table heads
//! (by table), table entries (by handle, or by name where a table entry has none), block
//! definitions (their `BLOCK` and `ENDBLK` units, by handle or name), entities with their
//! followers (by handle) and objects (by handle). Everything else (section markers, classes,
//! comments between units) is the **frame**.
//!
//! **The result is theirs, with ours' changes spliced in.** Theirs is the carrier because a
//! person's app wrote it in its own style: its bytes are kept, and ours' changes are written
//! into it, value line by value line where a value changed, in theirs' style (code padding, line
//! endings) where pairs are added. Every byte outside the units ours changed is theirs'.
//!
//! **Fields.** Where both sides changed one entity, it merges field by field: its layer,
//! colour, true colour, linetype, lineweight, transparency, text, geometry (its kind's data, as
//! one whole: a merge never mixes two sides' coordinates), and its content (extension data,
//! reactors, comments). Two changes to different fields both land; two different changes to the
//! same field are a [`MergeConflict`], naming the unit, the field and both sides: never one
//! side's value over the other's. Any other unit both sides changed differently, or one side
//! deleted and the other changed, is a conflict of the whole unit.
//!
//! **Never guessed** ([`MergeOutcome::Unsupported`]): a side that doesn't read, a drawing with
//! an entity or object without a handle, a key twice on one side, or a change by ours to the
//! frame. The kit then keeps both versions.
//!
//! Two values are derived, not merged: `$HANDSEED` takes the larger side, and a table's count
//! is the count of its entries after the merge. The variables every save rewrites (save times,
//! the version id, the header's extents, `$LASTSAVEDBY`) keep theirs.

use std::collections::{BTreeMap, BTreeSet};

use k2_mpc_cad_model::merge::{summary, MergeConflict, MergeOutcome};

use crate::structure::{Place, Structure, Unit};
use crate::token::{decode, Eol};
use crate::{codes, handle, mutant, structure, token, Limits};

/// Header variables every save may rewrite: theirs is kept when both changed them.
const VOLATILE: [&str; 10] = [
    "$TDUPDATE",
    "$TDUUPDATE",
    "$TDINDWG",
    "$TDUSRTIMER",
    "$EXTMIN",
    "$EXTMAX",
    "$LASTSAVEDBY",
    "$VERSIONGUID",
    "$TDCREATE",
    "$FINGERPRINTGUID",
];

/// The largest diff of one unit's pairs worked out pair by pair (cells of the table); a larger
/// unit is diffed by its common start and end only.
const DIFF_CELLS_MAX: usize = 4_000_000;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Header(String),
    TableHead(String),
    Entry(String, String),
    Block(String),
    EndBlk(String),
    Entity(u64),
    Object(u64),
}

impl Key {
    fn address(&self) -> String {
        match self {
            Key::Header(n) => n.clone(),
            Key::TableHead(t) => format!("name:{t}"),
            Key::Entry(_, id) | Key::Block(id) | Key::EndBlk(id) => match id.strip_prefix("h:") {
                Some(h) => format!("@h:{h}"),
                None => format!("name:{}", id.trim_start_matches("n:")),
            },
            Key::Entity(h) | Key::Object(h) => handle::key(*h),
        }
    }
}

/// Where a unit lives, for placing one ours added.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Home {
    Header,
    Table(String),
    Entities,
    Block(String),
    Objects,
}

#[derive(Debug, Clone)]
struct U {
    first: usize,
    end: usize,
    home: Home,
}

type Sem<'a> = Vec<(i32, &'a [u8])>;

/// A unit's pairs by the field each belongs to.
type Fields<'a> = BTreeMap<&'static str, Sem<'a>>;

struct Side<'a> {
    s: Structure<'a>,
    units: BTreeMap<Key, U>,
    frame: Vec<(i32, Vec<u8>)>,
    padded: bool,
}

fn first_value(s: &Structure<'_>, u: &Unit, code: i32) -> Option<String> {
    (u.first + 1..u.end)
        .find(|&i| s.tok.pairs[i].code == code)
        .map(|i| s.text(i).into_owned())
}

fn id_of(s: &Structure<'_>, u: &Unit) -> String {
    match first_value(s, u, 5)
        .or_else(|| first_value(s, u, 105))
        .and_then(|v| handle::parse(v.as_bytes()))
    {
        Some(h) => format!("h:{}", handle::norm(h)),
        None => format!(
            "n:{}",
            first_value(s, u, 2)
                .unwrap_or_default()
                .to_ascii_uppercase()
        ),
    }
}

fn side(bytes: &[u8]) -> Result<Side<'_>, String> {
    let limits = Limits::default();
    let t = token::tokenize(bytes, &limits).map_err(|e| e.summary())?;
    let s = structure::walk(t, &limits).map_err(|e| e.summary())?;
    let mut units: BTreeMap<Key, U> = BTreeMap::new();
    let mut put = |k: Key, u: U| -> Result<(), String> {
        if units.insert(k.clone(), u).is_some() {
            return Err(format!("{} appears twice", k.address()));
        }
        Ok(())
    };
    for v in &s.header {
        let u = U {
            first: v.first,
            end: v.end,
            home: Home::Header,
        };
        put(Key::Header(v.name.to_ascii_uppercase()), u)?;
    }
    for t in &s.tables {
        let home = Home::Table(t.name.clone());
        let head = U {
            first: t.head.first,
            end: t.head.end,
            home: home.clone(),
        };
        put(Key::TableHead(t.name.clone()), head)?;
        for e in &t.entries {
            let u = U {
                first: e.first,
                end: e.end,
                home: home.clone(),
            };
            put(Key::Entry(t.name.clone(), id_of(&s, e)), u)?;
        }
    }
    let mut block_ids = Vec::with_capacity(s.blocks.len());
    for b in &s.blocks {
        let id = id_of(&s, &b.head);
        put(
            Key::Block(id.clone()),
            U {
                first: b.head.first,
                end: b.head.end,
                home: Home::Block(id.clone()),
            },
        )?;
        if let Some(e) = &b.endblk {
            put(
                Key::EndBlk(id.clone()),
                U {
                    first: e.first,
                    end: e.end,
                    home: Home::Block(id.clone()),
                },
            )?;
        }
        block_ids.push(id);
    }
    for e in &s.entities {
        let h = first_value(&s, &e.head, 5)
            .and_then(|v| handle::parse(v.as_bytes()))
            .ok_or_else(|| "an entity without a handle".to_owned())?;
        let end = e.followers.last().map_or(e.head.end, |f| f.end);
        let home = match e.place {
            Place::Entities => Home::Entities,
            Place::Block(b) => Home::Block(block_ids[b].clone()),
        };
        put(
            Key::Entity(h),
            U {
                first: e.head.first,
                end,
                home,
            },
        )?;
    }
    for o in &s.objects {
        let h = first_value(&s, o, 5)
            .and_then(|v| handle::parse(v.as_bytes()))
            .ok_or_else(|| "an object without a handle".to_owned())?;
        put(
            Key::Object(h),
            U {
                first: o.first,
                end: o.end,
                home: Home::Objects,
            },
        )?;
    }
    let mut covered = vec![false; s.tok.pairs.len()];
    for u in units.values() {
        for c in &mut covered[u.first..u.end] {
            *c = true;
        }
    }
    let frame = (0..s.tok.pairs.len())
        .filter(|&i| !covered[i])
        .map(|i| (s.tok.pairs[i].code, s.tok.value(i).to_vec()))
        .collect();
    let (pad, plain) = s.tok.padding();
    Ok(Side {
        padded: pad > plain,
        s,
        units,
        frame,
    })
}

impl<'a> Side<'a> {
    fn sem(&self, u: &U) -> Sem<'a> {
        (u.first..u.end)
            .map(|i| (self.s.tok.pairs[i].code, self.s.tok.value(i)))
            .collect()
    }

    /// The field each pair of an entity unit belongs to.
    fn fields(&self, u: &U) -> Vec<&'static str> {
        let t = &self.s.tok;
        let kind = decode(t.value(u.first)).trim().to_ascii_uppercase();
        let mut out = Vec::with_capacity(u.end - u.first);
        let (mut in102, mut xdata, mut head) = (false, false, true);
        let mut seen: BTreeSet<i32> = BTreeSet::new();
        let mut follower = String::new();
        for i in u.first..u.end {
            let c = t.pairs[i].code;
            if c == 0 {
                if i == u.first {
                    out.push("kind");
                    continue;
                }
                head = false;
                in102 = false;
                xdata = false;
                follower = decode(t.value(i)).trim().to_ascii_uppercase();
                out.push("geom");
                continue;
            }
            if xdata || codes::is_xdata(c) {
                xdata = true;
                out.push("content");
                continue;
            }
            if c == 102 {
                in102 = t.value(i).trim_ascii().starts_with(b"{");
                out.push("content");
                continue;
            }
            if in102 || matches!(c, 999 | 100 | 67 | 410) {
                out.push("content");
                continue;
            }
            if head {
                let f = match c {
                    5 => "handle",
                    330 => "owner",
                    8 => "layer",
                    62 => "color",
                    420 => "rgb",
                    6 => "ltype",
                    370 => "lw",
                    440 => "alpha",
                    1 if matches!(kind.as_str(), "TEXT" | "ATTDEF" | "MTEXT") => "text",
                    3 if kind == "MTEXT" => "text",
                    _ => "geom",
                };
                if !matches!(f, "geom" | "text") && !seen.insert(c) {
                    out.push("geom");
                } else {
                    out.push(f);
                }
            } else if follower == "ATTRIB" && matches!(c, 1 | 2) {
                out.push("text");
            } else {
                out.push("geom");
            }
        }
        out
    }
}

/// Index pairs `(i, j)` where `a[i] == b[j]`, in order: a longest common subsequence (its
/// common start and end, then a table over the middle when it is small enough).
fn matches<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf]
    {
        suf += 1;
    }
    let mut out: Vec<(usize, usize)> = (0..pre).map(|i| (i, i)).collect();
    let (am, bm) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    if !am.is_empty() && !bm.is_empty() && am.len().saturating_mul(bm.len()) <= DIFF_CELLS_MAX {
        let (n, m) = (am.len(), bm.len());
        let mut t = vec![0u32; (n + 1) * (m + 1)];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * (m + 1) + j] = if am[i] == bm[j] {
                    t[(i + 1) * (m + 1) + j + 1] + 1
                } else {
                    t[(i + 1) * (m + 1) + j].max(t[i * (m + 1) + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n && j < m {
            if am[i] == bm[j] {
                out.push((pre + i, pre + j));
                i += 1;
                j += 1;
            } else if t[(i + 1) * (m + 1) + j] >= t[i * (m + 1) + j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
    }
    out.extend((0..suf).map(|k| (a.len() - suf + k, b.len() - suf + k)));
    out
}

/// The runs between matches: `(a_lo, a_hi, b_lo, b_hi)`, each a replacement of `a[a_lo..a_hi]`
/// by `b[b_lo..b_hi]`.
fn hunks(m: &[(usize, usize)], alen: usize, blen: usize) -> Vec<(usize, usize, usize, usize)> {
    let mut out = Vec::new();
    let (mut ai, mut bi) = (0, 0);
    for &(i, j) in m.iter().chain(std::iter::once(&(alen, blen))) {
        if i > ai || j > bi {
            out.push((ai, i, bi, j));
        }
        ai = i + 1;
        bi = j + 1;
    }
    out
}

/// One change to theirs' bytes: `lo..hi` replaced by `with`.
struct Edit {
    lo: usize,
    hi: usize,
    with: Vec<u8>,
}

struct Merger<'a, 'b> {
    b: &'b Side<'a>,
    o: &'b Side<'a>,
    t: &'b Side<'a>,
    edits: Vec<Edit>,
    conflicts: Vec<MergeConflict>,
    applied: usize,
    /// Table entries ours added (+) and removed (-), per table.
    entries: BTreeMap<String, i64>,
}

impl<'a, 'b> Merger<'a, 'b> {
    fn conflict(&mut self, k: &Key, field: &str, ours: String, theirs: String) {
        self.conflicts.push(MergeConflict {
            address: k.address(),
            field: field.to_owned(),
            ours: summary(&ours),
            theirs: summary(&theirs),
        });
    }

    /// Pair `p` of ours, written as theirs writes a pair: its code padded or not, and `eol`.
    fn render(&self, code: i32, value: &[u8], eol: Eol) -> Vec<u8> {
        let mut v = Vec::new();
        if self.t.padded && !mutant::active("OursStyleLeaks") {
            v.extend_from_slice(format!("{code:>3}").as_bytes());
        } else {
            v.extend_from_slice(code.to_string().as_bytes());
        }
        v.extend_from_slice(eol.bytes());
        v.extend_from_slice(value);
        v.extend_from_slice(eol.bytes());
        v
    }

    /// The line ending theirs uses at pair `i` (its value line's), or its first pair's.
    fn eol_at(&self, i: usize) -> Eol {
        let p = &self.t.s.tok.pairs;
        p.get(i)
            .or_else(|| i.checked_sub(1).and_then(|j| p.get(j)))
            .or_else(|| p.first())
            .map_or(Eol::Lf, |x| {
                if x.value_eol == Eol::None {
                    Eol::Lf
                } else {
                    x.value_eol
                }
            })
    }

    /// Ours' pairs `ou.first + o_lo .. ou.first + o_hi` in place of theirs' `t_lo..t_hi`
    /// (absolute pair indices; `t_lo == t_hi` inserts before `t_lo`).
    fn replace(&mut self, ou: &U, o_lo: usize, o_hi: usize, t_lo: usize, t_hi: usize) {
        let tp = &self.t.s.tok.pairs;
        let lo_byte = if t_lo < tp.len() {
            tp[t_lo].start()
        } else {
            self.t.s.tok.tail_at
        };
        let hi_byte = if t_hi > t_lo {
            tp[t_hi - 1].end()
        } else {
            lo_byte
        };
        let same_codes = o_hi - o_lo == t_hi - t_lo
            && (0..o_hi - o_lo)
                .all(|k| self.o.s.tok.pairs[ou.first + o_lo + k].code == tp[t_lo + k].code);
        let mut with = Vec::new();
        if same_codes && !mutant::active("OursStyleLeaks") {
            // value lines only: theirs' code lines and line endings stay
            for k in 0..o_hi - o_lo {
                let tq = &tp[t_lo + k];
                let tb = self.t.s.tok.bytes;
                with.extend_from_slice(&tb[tq.start()..tq.value_at as usize]);
                with.extend_from_slice(self.o.s.tok.value(ou.first + o_lo + k));
                with.extend_from_slice(tq.value_eol.bytes());
            }
        } else if same_codes {
            for k in 0..o_hi - o_lo {
                let oq = &self.o.s.tok.pairs[ou.first + o_lo + k];
                with.extend_from_slice(&self.o.s.tok.bytes[oq.start()..oq.end()]);
            }
        } else {
            let eol = self.eol_at(t_lo);
            for k in o_lo..o_hi {
                let i = ou.first + k;
                with.extend_from_slice(&self.render(
                    self.o.s.tok.pairs[i].code,
                    self.o.s.tok.value(i),
                    eol,
                ));
            }
        }
        self.edits.push(Edit {
            lo: lo_byte,
            hi: hi_byte,
            with,
        });
    }

    /// Ours' change of a unit theirs left as the base had it.
    fn apply_whole(&mut self, bu: &U, ou: &U, tu: &U) {
        let (bs, os) = (self.b.sem(bu), self.o.sem(ou));
        let m = matches(&bs, &os);
        for (alo, ahi, olo, ohi) in hunks(&m, bs.len(), os.len()) {
            // theirs' unit reads as the base's: its pairs line up one for one
            self.replace(ou, olo, ohi, tu.first + alo, tu.first + ahi);
        }
        self.applied += 1;
    }

    /// Where an added unit of `home` goes in theirs: before the end marker of its home.
    fn home_end(&self, home: &Home) -> Option<usize> {
        let s = &self.t.s;
        let section_end = |name: &str| {
            s.sections
                .iter()
                .find(|x| x.name == name && x.closed)
                .map(|x| x.end - 1)
        };
        match home {
            Home::Header => section_end("HEADER"),
            Home::Entities => section_end("ENTITIES"),
            Home::Objects => section_end("OBJECTS"),
            Home::Table(name) => s
                .tables
                .iter()
                .find(|t| &t.name == name && t.closed)
                .map(|t| t.entries.last().map_or(t.head.end, |e| e.end)),
            Home::Block(id) => self.t.units.get(&Key::EndBlk(id.clone())).map(|u| u.first),
        }
    }

    fn add(&mut self, k: &Key, ou: &U) {
        if mutant::active("AddDropped") {
            return;
        }
        // a whole block ours added goes in at once, its entities and its end with it
        if let Key::Block(id) = k {
            let end = self
                .o
                .units
                .get(&Key::EndBlk(id.clone()))
                .map_or(ou.end, |e| e.end);
            let span = U {
                first: ou.first,
                end,
                home: Home::Block(id.clone()),
            };
            let blocks_end = self
                .t
                .s
                .sections
                .iter()
                .find(|x| x.name == "BLOCKS" && x.closed)
                .map(|x| x.end - 1);
            match blocks_end {
                Some(at) => {
                    self.replace(&span, 0, end - ou.first, at, at);
                    self.applied += 1;
                }
                None => self.conflict(k, "whole", "added".into(), "has no block section".into()),
            }
            return;
        }
        if let (Key::Entity(_) | Key::EndBlk(_), Home::Block(id)) = (k, &ou.home) {
            if !self.b.units.contains_key(&Key::Block(id.clone()))
                && !self.t.units.contains_key(&Key::Block(id.clone()))
            {
                return; // in the block added whole
            }
        }
        match self.home_end(&ou.home) {
            Some(at) => {
                self.replace(ou, 0, ou.end - ou.first, at, at);
                self.applied += 1;
                if let Key::Entry(table, _) = k {
                    *self.entries.entry(table.clone()).or_default() += 1;
                }
            }
            None => self.conflict(k, "whole", "added".into(), "has no place for it".into()),
        }
    }

    fn remove(&mut self, k: &Key, tu: &U) {
        // a block's entities and end go with it
        if let (Key::Entity(_) | Key::EndBlk(_), Home::Block(id)) = (k, &tu.home) {
            if !self.o.units.contains_key(&Key::Block(id.clone())) {
                return;
            }
        }
        let end = match k {
            Key::Block(id) => self
                .t
                .units
                .get(&Key::EndBlk(id.clone()))
                .map_or(tu.end, |e| e.end),
            _ => tu.end,
        };
        let tp = &self.t.s.tok.pairs;
        self.edits.push(Edit {
            lo: tp[tu.first].start(),
            hi: tp[end - 1].end(),
            with: Vec::new(),
        });
        self.applied += 1;
        if let Key::Entry(table, _) = k {
            *self.entries.entry(table.clone()).or_default() -= 1;
        }
    }

    /// Both sides changed one entity: field by field.
    fn merge_fields(&mut self, k: &Key, bu: &U, ou: &U, tu: &U) {
        let (bs, os, ts) = (self.b.sem(bu), self.o.sem(ou), self.t.sem(tu));
        let (bf, of, tf) = (self.b.fields(bu), self.o.fields(ou), self.t.fields(tu));
        let by_field = |sem: &Sem<'a>, f: &[&'static str]| {
            let mut m: Fields<'a> = BTreeMap::new();
            for (p, name) in sem.iter().zip(f) {
                m.entry(*name).or_default().push(*p);
            }
            m
        };
        let (vb, vo, vt) = (by_field(&bs, &bf), by_field(&os, &of), by_field(&ts, &tf));
        let names: BTreeSet<&'static str> = vb
            .keys()
            .chain(vo.keys())
            .chain(vt.keys())
            .copied()
            .collect();
        let mut ours_only: BTreeSet<&'static str> = BTreeSet::new();
        let mut clash = false;
        let show = |v: Option<&Sem<'a>>| -> String {
            match v {
                None => "none".into(),
                Some(v) => v
                    .iter()
                    .map(|(c, x)| format!("{c}={}", decode(x).trim()))
                    .collect::<Vec<_>>()
                    .join(" "),
            }
        };
        for f in names {
            let (b, o, t) = (vb.get(f), vo.get(f), vt.get(f));
            let (oc, tc) = (o != b, t != b);
            if oc && tc && o != t {
                clash = true;
                self.conflict(k, f, show(o), show(t));
            } else if oc && !tc {
                ours_only.insert(f);
            }
        }
        if mutant::active("FieldsAsWhole") {
            self.conflict(k, "whole", "changed".into(), "changed".into());
            return;
        }
        if clash && mutant::active("LastWriterWins") {
            // the planted bug: ours' unit over theirs', the conflicts dropped
            self.conflicts.retain(|c| c.address != k.address());
            self.replace(ou, 0, ou.end - ou.first, tu.first, tu.end);
            self.applied += 1;
            return;
        }
        if clash || ours_only.is_empty() {
            return;
        }
        // ours' changes to its own fields, placed where theirs has the base's pairs
        let m_bt: Vec<Option<usize>> = {
            let mut v = vec![None; bs.len()];
            for (i, j) in matches(&bs, &ts) {
                v[i] = Some(j);
            }
            v
        };
        let m = matches(&bs, &os);
        for (alo, ahi, olo, ohi) in hunks(&m, bs.len(), os.len()) {
            let touched: BTreeSet<&'static str> =
                bf[alo..ahi].iter().chain(&of[olo..ohi]).copied().collect();
            if !touched.iter().all(|f| ours_only.contains(f)) {
                if touched.iter().any(|f| ours_only.contains(f)) {
                    self.conflict(k, "whole", "edits overlap".into(), "edits overlap".into());
                }
                continue;
            }
            let span = if ahi > alo {
                match (m_bt[alo], m_bt[ahi - 1]) {
                    (Some(lo), Some(hi)) if hi + 1 - lo == ahi - alo => Some((lo, hi + 1)),
                    _ => None,
                }
            } else if alo > 0 {
                m_bt[alo - 1].map(|j| (j + 1, j + 1))
            } else {
                m_bt.first()
                    .copied()
                    .flatten()
                    .map(|j| (j, j))
                    .or(Some((0, 0)))
            };
            match span {
                Some((lo, hi)) => self.replace(ou, olo, ohi, tu.first + lo, tu.first + hi),
                None => self.conflict(k, "whole", "edits overlap".into(), "edits overlap".into()),
            }
        }
        self.applied += 1;
    }

    /// One unit, by its presence and content on the three sides.
    fn unit(&mut self, k: &Key) {
        let (bu, ou, tu) = (
            self.b.units.get(k).cloned(),
            self.o.units.get(k).cloned(),
            self.t.units.get(k).cloned(),
        );
        let sem_of = |side: &Side<'a>, u: Option<&U>| -> Option<Sem<'a>> {
            let mut v = side.sem(u?);
            if let Key::TableHead(_) = k {
                v.retain(|p| p.0 != 70); // a table's count is derived
            }
            Some(v)
        };
        let (bsem, osem, tsem) = (
            sem_of(self.b, bu.as_ref()),
            sem_of(self.o, ou.as_ref()),
            sem_of(self.t, tu.as_ref()),
        );
        let ours_changed = bsem != osem;
        let theirs_changed = bsem != tsem;
        if !ours_changed {
            return;
        }
        if mutant::active("MergeTakesTheirs") {
            return;
        }
        match (bu.as_ref(), ou.as_ref(), tu.as_ref()) {
            (_, Some(o), None) if !theirs_changed || bu.is_none() => self.add(k, o),
            (Some(_), None, Some(t)) if !theirs_changed => self.remove(k, t),
            (Some(b), Some(o), Some(t)) if !theirs_changed => self.apply_whole(b, o, t),
            (_, None, None) => {} // both removed
            (None, Some(_), Some(_)) => {
                if osem != tsem {
                    self.conflict(k, "whole", "added".into(), "added otherwise".into());
                }
            }
            (Some(_), None, Some(_)) => {
                self.conflict(k, "whole", "deleted".into(), "changed".into())
            }
            (Some(_), Some(_), None) => {
                self.conflict(k, "whole", "changed".into(), "deleted".into())
            }
            (Some(b), Some(o), Some(t)) => {
                if osem == tsem {
                    return;
                }
                match k {
                    Key::Entity(_) => self.merge_fields(k, b, o, t),
                    Key::Header(name) if name == "$HANDSEED" => {
                        let seed = |side: &Side<'a>, u: &U| {
                            side.sem(u)
                                .iter()
                                .find(|p| p.0 == 5)
                                .and_then(|p| handle::parse(p.1))
                        };
                        if seed(self.o, o) > seed(self.t, t) {
                            self.apply_value(o, t);
                        }
                    }
                    Key::Header(name) if VOLATILE.contains(&name.as_str()) => {}
                    _ => self.conflict(k, "whole", "changed".into(), "changed otherwise".into()),
                }
            }
            (None, None, _) | (None, Some(_), None) => {}
        }
    }

    /// Ours' header variable in place of theirs', pair for pair where they line up.
    fn apply_value(&mut self, ou: &U, tu: &U) {
        let n = (ou.end - ou.first).min(tu.end - tu.first);
        self.replace(ou, 1, n, tu.first + 1, tu.first + n);
        self.applied += 1;
    }

    /// Each table's count, as its entries after the merge.
    fn counts(&mut self) {
        let changes: Vec<(String, i64)> =
            self.entries.iter().map(|(k, v)| (k.clone(), *v)).collect();
        for (table, delta) in changes {
            let Some(t) = self.t.s.tables.iter().find(|x| x.name == table) else {
                continue;
            };
            let want = t.entries.len() as i64 + delta;
            let Some(i) =
                (t.head.first + 1..t.head.end).find(|&i| self.t.s.tok.pairs[i].code == 70)
            else {
                continue;
            };
            let tp = &self.t.s.tok.pairs[i];
            let have: Option<i64> = self.t.s.text(i).trim().parse().ok();
            if have != Some(want) {
                self.edits.push(Edit {
                    lo: tp.value_at as usize,
                    hi: tp.value_end as usize,
                    with: want.to_string().into_bytes(),
                });
            }
        }
    }
}

/// The three-way merge of `base`, `ours` and `theirs` (module docs).
pub fn merge(base: &[u8], ours: &[u8], theirs: &[u8]) -> MergeOutcome {
    let b = match side(base) {
        Ok(x) => x,
        Err(e) => return MergeOutcome::Unsupported(format!("the base: {e}")),
    };
    let o = match side(ours) {
        Ok(x) => x,
        Err(e) => return MergeOutcome::Unsupported(format!("ours: {e}")),
    };
    let t = match side(theirs) {
        Ok(x) => x,
        Err(e) => return MergeOutcome::Unsupported(format!("theirs: {e}")),
    };
    if o.frame != b.frame && !mutant::active("FrameGuessed") {
        return MergeOutcome::Unsupported(
            "ours changed a part of the file outside the units this merge reads".into(),
        );
    }
    let mut m = Merger {
        b: &b,
        o: &o,
        t: &t,
        edits: Vec::new(),
        conflicts: Vec::new(),
        applied: 0,
        entries: BTreeMap::new(),
    };
    let keys: BTreeSet<Key> = b
        .units
        .keys()
        .chain(o.units.keys())
        .chain(t.units.keys())
        .cloned()
        .collect();
    for k in &keys {
        m.unit(k);
    }
    if !m.conflicts.is_empty() {
        return MergeOutcome::Conflict(m.conflicts);
    }
    m.counts();
    let mut edits = std::mem::take(&mut m.edits);
    let applied = m.applied;
    edits.sort_by_key(|e| (e.lo, e.hi));
    let tb = t.s.tok.bytes;
    let mut out = Vec::with_capacity(tb.len());
    let mut at = 0;
    for e in &edits {
        if e.lo < at {
            return MergeOutcome::Unsupported("two changes fell on one place in theirs".into());
        }
        out.extend_from_slice(&tb[at..e.lo]);
        out.extend_from_slice(&e.with);
        at = e.hi;
    }
    out.extend_from_slice(&tb[at..]);
    // the merged file must read as one
    let limits = Limits::default();
    match token::tokenize(&out, &limits).map(|t| structure::walk(t, &limits)) {
        Ok(Ok(_)) => MergeOutcome::Merged {
            bytes: out,
            applied,
        },
        _ => MergeOutcome::Unsupported("the merged file didn't read: a merge bug".into()),
    }
}
