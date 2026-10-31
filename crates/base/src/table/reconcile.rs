//! The aligner (A8, contract §2.9): a file has no keys, so a new rendering of a table (an
//! outside save by a spreadsheet, a text edit verb's result, an agent's `write --base` body)
//! is turned into **keyed grid ops** against the table as the file last held it.
//!
//! 1. **Columns.** By unique header name (header on). With no header and the width unchanged
//!    (the text's widest row has as many fields as the base has live columns), by position.
//!    Otherwise (a header, or the width changed) by content, to find the column that went or
//!    came (a column whose sorted values equal exactly one unmatched column's on the other
//!    side, and aren't all empty); then by position among the unmatched. A matched column whose name changed is a rename. An unmatched old column
//!    is deleted, unless the old rendering never showed it and the new one wouldn't either
//!    (a trailing column of absent cells in a ragged table).
//! 2. **Rows.** Each row's fingerprint is its rendered fields under the matched columns. A
//!    longest common subsequence of fingerprints (Myers, with the kit's counted work budget:
//!    no clock, the same result on every machine) keeps equal rows on their keys; inside
//!    each changed stretch, equal rows pair in order (what the diff left when the budget ran
//!    out); then a new row equal to exactly one unmatched old row anywhere, and no other, is
//!    that row **moved**; then rows sharing at least half their non-empty fields pair up as
//!    edits: first, anywhere, an old and a new row that are each other's **unique best
//!    match** (most shared fields, no tie: so a save that moves one row and edits another
//!    keeps both keys), then inside each stretch, in order (a counted budget; past it, the
//!    rest pair by position); then **by position in an unchanged slot**: a run of unpaired
//!    new rows and a run of unpaired old rows of the same length, between the same two paired
//!    rows (or the same end of the table), pair in order (a blank line edited, any cell
//!    edited in a one-column table: nothing shared, but nothing else could be meant). The
//!    rest are inserts (keys minted) and deletes. **Ambiguity rule:** a row that ties with
//!    another (two equally good matches) is not paired by the best-match step; one that
//!    shares fewer than half its fields with every old row and isn't in its old slot (moved
//!    and rewritten: its neighbours aren't its old neighbours, or the counts around it
//!    differ) is a new row, its old one deleted.
//! 3. **Order.** A longest run of kept rows (columns) in their old relative order stays; every
//!    other one is placed right after its new left neighbour, left to right, so consecutive
//!    placements are one forward chain (K-CSV-10b). Moves are the minimal set (n − LIS).
//! 4. **Cells, names, quoting.** Per rendered field, by the render rules read backwards: a
//!    non-empty (or quoted) field is a present cell with that value; an empty field keeps an
//!    absent cell absent and sets a present one to `""` (clear is a set, K-CSV-10a); a
//!    missing field (past the record's end) is an absent cell (a map delete: the one place
//!    the kit deletes a cell). A `ragged` row's last field is always present (it renders
//!    through it); a non-blank `rect` row has at least one present cell. Quote exceptions
//!    follow the fields. Hidden rows and columns are never touched.
//!
//! **Properties** (`tests`, `tests/table.rs`): `reconcile(t, t.render())` is no edit; for any
//! table `g` reachable by grid ops from `t`, applying `reconcile(t, g.render())` to `t`
//! renders as `g` does (and unchanged rows keep their keys; with no structural change, the
//! result is `g` exactly); the output is a pure function of its input.

use std::collections::{BTreeMap, HashMap, VecDeque};

use super::ops::Change;
use super::*;
use crate::elem::lis;
use crate::text::myers::{self, Budget};

/// Field comparisons the similarity pairing may spend, per reconcile.
const SIMILAR_BUDGET: usize = 1 << 22;

/// Candidates by what they share: (old indices, new indices). A pair forms where each side
/// has exactly one.
type Pairing<T> = BTreeMap<T, (Vec<usize>, Vec<usize>)>;

/// What [`reconcile()`] made of a new rendering.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconciled {
    /// One change: apply it to the table (and the doc) as is.
    pub edits: Vec<Edit>,
    /// The key of each data record of the rendering, in order (new rows' keys are minted).
    pub rows: Vec<Key>,
    /// The key of each field position (column) of the rendering.
    pub cols: Vec<Key>,
    pub inserted_rows: Vec<Key>,
    pub deleted_rows: Vec<Key>,
    pub moved_rows: Vec<Key>,
    pub inserted_cols: Vec<Key>,
    pub deleted_cols: Vec<Key>,
    pub moved_cols: Vec<Key>,
    /// The kept rows the rendering left **exact** (v0.10.0): under an exact column mapping, the
    /// row's decoded field values are the old ones, exactly: nothing normalised (a trailing
    /// space counts, a blank line isn't `""`), but how a field is quoted isn't a change: a tool
    /// can requote a whole file on save. A merge of the text (`merge3_text`,
    /// `merge3_outside_text`) treats every other kept row as rewritten: pairing it was a
    /// guess about identity, so it never merges cell by cell.
    pub exact_rows: Vec<Key>,
    /// The column mapping is exact: each kept column at its own position, or matched by its
    /// unique header name. Otherwise no row is exact, and the text remapped the columns. A
    /// column the text deleted leaves no row exact either (it changed every row), without a
    /// remap.
    pub cols_exact: bool,
}

impl Reconciled {
    /// The rendering is the table's: nothing to apply.
    pub fn is_empty(&self) -> bool {
        self.edits.is_empty()
    }
}

/// Keyed grid ops that make `t` render as `records` (the file's records in order: the header
/// first when `t.header`). Values are checked under `t`'s limit ([`Table::max_value`]); the table's shape and header
/// setting are kept, except that a header-on table whose file is now one blank line switches
/// its header off (a lone blank line is no header: [`Table::header_renders`]; the names stay,
/// unrendered). See the module docs for the algorithm.
pub fn reconcile(t: &Table, records: &[Record]) -> Result<Reconciled, TableError> {
    for f in records.iter().flatten() {
        t.check(&f.value)?;
    }
    let header = t.header && !lone_blank(records);
    let empty: Record = Vec::new();
    let (head, body): (&Record, &[Record]) = match (header, records.split_first()) {
        (true, Some((h, b))) => (h, b),
        (true, None) => (&empty, &[]),
        (false, _) => (&empty, records),
    };
    let old = t.render();
    let old_body: &[Record] = &old[old.len() - old_rows_len(t)..];
    let old_rows = t.live_rows();
    let old_cols = t.live_cols();
    let mut w = t.clone();
    let mut ch = Change::new(&w);
    let mut out = Reconciled::default();
    if t.header && !header {
        ch.meta(&mut w, "header", "off");
    }

    // ---- columns ----
    let new_w = body
        .iter()
        .map(Vec::len)
        .chain([head.len()])
        .max()
        .unwrap_or(0);
    let shown_w = old.iter().map(Vec::len).max().unwrap_or(0);
    let mut col_of: Vec<Option<usize>> = vec![None; new_w];
    let pair = |j: usize, i: usize, col_of: &mut Vec<Option<usize>>| col_of[j] = Some(i);
    if header {
        let mut by_name: Pairing<&str> = BTreeMap::new();
        for (i, c) in old_cols.iter().enumerate() {
            if let Some(n) = t.name(c) {
                by_name.entry(n).or_default().0.push(i);
            }
        }
        for (j, f) in head.iter().enumerate() {
            by_name.entry(&f.value).or_default().1.push(j);
        }
        for (o, n) in by_name.values() {
            if let ([i], [j]) = (o.as_slice(), n.as_slice()) {
                pair(*j, *i, &mut col_of);
            }
        }
    }
    // a column mapping is exact only by its unique header name, or at its own position
    let by_name: Vec<bool> = col_of.iter().map(Option::is_some).collect();
    // with no header and the width unchanged (the text's widest row against the base's live
    // columns), columns pair by position: a content match there can be coincidence (it compares
    // each column's values sorted, so two columns holding the same values in other rows match).
    // Content pairs only find the column that went or came when the width changed
    if header || new_w != old_cols.len() {
        let mut by_content: Pairing<Vec<Option<&str>>> = BTreeMap::new();
        for i in (0..old_cols.len()).filter(|&i| !taken_at(&col_of, i)) {
            if let Some(s) = column(old_body, i) {
                by_content.entry(s).or_default().0.push(i);
            }
        }
        for j in (0..new_w).filter(|&j| col_of[j].is_none()) {
            if let Some(s) = column(body, j) {
                by_content.entry(s).or_default().1.push(j);
            }
        }
        for (o, n) in by_content.values() {
            if let ([i], [j]) = (o.as_slice(), n.as_slice()) {
                pair(*j, *i, &mut col_of);
            }
        }
    }
    let unmatched_old: Vec<usize> = (0..old_cols.len())
        .filter(|&i| !taken_at(&col_of, i))
        .collect();
    let unmatched_new: Vec<usize> = (0..new_w).filter(|&j| col_of[j].is_none()).collect();
    for (&i, &j) in unmatched_old.iter().zip(&unmatched_new) {
        pair(j, i, &mut col_of);
    }
    let all_blank = records.iter().all(Vec::is_empty);
    let mut gone_cols = Vec::new();
    let mut kept_hidden = Vec::new();
    for (i, c) in old_cols.iter().enumerate() {
        if col_of.contains(&Some(i)) {
            continue;
        }
        // (a column the header line showed was shown: a header-on column that is invisible
        // has no name, so it never makes a header line render)
        let invisible = i >= shown_w && (t.shape == Shape::Ragged || all_blank);
        if invisible {
            kept_hidden.push(c.clone());
        } else {
            gone_cols.push(c.clone());
        }
    }
    let mut tcols = Vec::with_capacity(new_w + kept_hidden.len());
    let mut fresh_cols = HashSet::new();
    for o in &col_of {
        tcols.push(match o {
            Some(i) => old_cols[*i].clone(),
            None => {
                let k = w.mint_col();
                fresh_cols.insert(k.clone());
                out.inserted_cols.push(k.clone());
                k
            }
        });
    }
    let target_cols: Vec<Key> = tcols.iter().chain(&kept_hidden).cloned().collect();
    out.moved_cols = place(&mut ch, &mut w, COLS, &gone_cols, &target_cols, &fresh_cols)?;
    out.deleted_cols = gone_cols;

    // ---- rows ----
    let matched: Vec<(usize, usize)> = col_of
        .iter()
        .enumerate()
        .filter_map(|(j, o)| o.map(|i| (j, i)))
        .collect();
    let ot: Vec<String> = old_body
        .iter()
        .map(|r| token(r, matched.iter().map(|&(_, i)| i)))
        .collect();
    let nt: Vec<String> = body
        .iter()
        .map(|r| token(r, matched.iter().map(|&(j, _)| j)))
        .collect();
    let mut new_old: Vec<Option<usize>> = vec![None; body.len()];
    let mut old_new: Vec<Option<usize>> = vec![None; old_body.len()];
    // (a) the longest common subsequence of fingerprints
    let gaps = {
        let a: Vec<&str> = ot.iter().map(String::as_str).collect();
        let b: Vec<&str> = nt.iter().map(String::as_str).collect();
        let bytes = a.iter().chain(&b).map(|s| s.len() + 1).sum();
        let changes = myers::diff(&a, &b, &mut Budget::for_input(bytes));
        let (mut o, mut n) = (0, 0);
        let mut gaps = Vec::with_capacity(changes.len());
        for c in changes {
            while o < c.old.start {
                link(o, n, &mut new_old, &mut old_new);
                o += 1;
                n += 1;
            }
            gaps.push((c.old.clone(), c.new.clone()));
            o = c.old.end;
            n = c.new.end;
        }
        while o < old_body.len() {
            link(o, n, &mut new_old, &mut old_new);
            o += 1;
            n += 1;
        }
        gaps
    };
    // (b) equal rows inside each stretch, in order
    for (og, ng) in &gaps {
        let mut queue: HashMap<&str, VecDeque<usize>> = HashMap::new();
        for o in og.clone() {
            queue.entry(ot[o].as_str()).or_default().push_back(o);
        }
        let mut last = None;
        for n in ng.clone() {
            let Some(q) = queue.get_mut(nt[n].as_str()) else {
                continue;
            };
            while q.front().is_some_and(|&o| last.is_some_and(|l| o <= l)) {
                q.pop_front();
            }
            if let Some(o) = q.pop_front() {
                link(o, n, &mut new_old, &mut old_new);
                last = Some(o);
            }
        }
    }
    // (c) moves: a row equal to exactly one unmatched row on the other side
    {
        let mut by_token: Pairing<&str> = BTreeMap::new();
        for (o, s) in ot.iter().enumerate() {
            if old_new[o].is_none() {
                by_token.entry(s).or_default().0.push(o);
            }
        }
        for (n, s) in nt.iter().enumerate() {
            if new_old[n].is_none() {
                by_token.entry(s).or_default().1.push(n);
            }
        }
        for (os, ns) in by_token.values() {
            if let ([o], [n]) = (os.as_slice(), ns.as_slice()) {
                link(*o, *n, &mut new_old, &mut old_new);
            }
        }
    }
    // (d) edits: a row whose best match is unique both ways, anywhere (a move and an edit in
    // one save land in different stretches), then similar rows inside each stretch, in
    // order; by position past the budget
    let mut work = 0usize;
    pair_best(
        old_body,
        body,
        &matched,
        &mut new_old,
        &mut old_new,
        &mut work,
    );
    for (og, ng) in &gaps {
        let mut from = og.start;
        for n in ng.clone() {
            if new_old[n].is_some() {
                continue;
            }
            let mut o = from;
            while o < og.end {
                if old_new[o].is_none()
                    && (work > SIMILAR_BUDGET
                        || similar(&old_body[o], &body[n], &matched, &mut work))
                {
                    link(o, n, &mut new_old, &mut old_new);
                    from = o + 1;
                    break;
                }
                o += 1;
            }
        }
    }
    pair_in_place(&mut new_old, &mut old_new);
    let cols_exact = col_of
        .iter()
        .enumerate()
        .all(|(j, o)| o.map_or(true, |i| i == j || by_name[j]));
    out.cols_exact = cols_exact;
    // a column the file deleted changes every row, though each may render as before
    if cols_exact && out.deleted_cols.is_empty() {
        out.exact_rows = new_old
            .iter()
            .enumerate()
            .filter_map(|(n, o)| o.map(|o| (n, o)))
            .filter(|&(n, o)| row_is_exact(&old_body[o], &body[n], &col_of))
            .map(|(_, o)| old_rows[o].clone())
            .collect();
    }
    let mut fresh_rows = HashSet::new();
    for o in &new_old {
        out.rows.push(match o {
            Some(o) => old_rows[*o].clone(),
            None => {
                let k = w.mint_row();
                fresh_rows.insert(k.clone());
                out.inserted_rows.push(k.clone());
                k
            }
        });
    }
    let gone_rows: Vec<Key> = old_rows
        .iter()
        .enumerate()
        .filter(|(o, _)| old_new[*o].is_none())
        .map(|(_, k)| k.clone())
        .collect();
    out.moved_rows = place(&mut ch, &mut w, ROWS, &gone_rows, &out.rows, &fresh_rows)?;
    out.deleted_rows = gone_rows;

    // ---- cells, names, quoting ----
    for (rec, r) in body.iter().zip(&out.rows) {
        let want = desired(t.shape, rec, new_w, |j| w.cell(r, &tcols[j]).is_some());
        for (j, d) in want.into_iter().enumerate() {
            let c = &tcols[j];
            let at = QuoteAt::Cell(r.clone(), c.clone());
            match d {
                Some((v, q)) => {
                    ch.cell(&mut w, r, c, Some(v));
                    ch.quote(&mut w, at, q);
                }
                None if w.cell(r, c).is_some() => {
                    ch.cell(&mut w, r, c, None);
                    ch.quote(&mut w, at, None);
                }
                None => {}
            }
        }
    }
    if header {
        let want = desired(t.shape, head, new_w, |j| w.name(&tcols[j]).is_some());
        for (j, d) in want.into_iter().enumerate() {
            let c = &tcols[j];
            let at = QuoteAt::Header(c.clone());
            match d {
                Some((v, q)) => {
                    ch.name(&mut w, c, Some(v));
                    ch.quote(&mut w, at, q);
                }
                None if w.name(c).is_some() => {
                    ch.name(&mut w, c, None);
                    ch.quote(&mut w, at, None);
                }
                None => {}
            }
        }
    }
    out.cols = tcols;
    out.edits = ch.finish(&mut w);
    Ok(out)
}

/// A column's values over `recs` (a missing field is `None`), sorted: equal for a column that
/// only moved. `None` when it holds nothing but empty fields (no evidence).
fn column(recs: &[Record], i: usize) -> Option<Vec<Option<&str>>> {
    let mut v: Vec<Option<&str>> = recs
        .iter()
        .map(|r| r.get(i).map(|f| f.value.as_str()))
        .collect();
    if !v.iter().any(|x| x.is_some_and(|s| !s.is_empty())) {
        return None;
    }
    v.sort_unstable();
    Some(v)
}

/// The new record's decoded values are the old one's under `col_of` (new position to old):
/// every field present on one side has the same value on the other. No fingerprint, nothing
/// normalised (a missing field differs from an empty one); how a field is quoted doesn't count.
fn row_is_exact(old: &Record, new: &Record, col_of: &[Option<usize>]) -> bool {
    let matched = col_of.iter().enumerate().all(|(j, o)| {
        let (n, m) = (new.get(j), o.and_then(|i| old.get(i)));
        n.map(|f| &f.value) == m.map(|f| &f.value)
    });
    matched && (0..old.len()).all(|i| col_of.contains(&Some(i)))
}

fn old_rows_len(t: &Table) -> usize {
    t.rows.live_len()
}

fn taken_at(col_of: &[Option<usize>], i: usize) -> bool {
    col_of.contains(&Some(i))
}

fn link(o: usize, n: usize, new_old: &mut [Option<usize>], old_new: &mut [Option<usize>]) {
    new_old[n] = Some(o);
    old_new[o] = Some(n);
}

/// A row's fingerprint: its fields at `idx` (a missing field differs from an empty one),
/// as one token. Values hold no NUL ([`check_value`]).
fn token(rec: &Record, idx: impl Iterator<Item = usize>) -> String {
    let mut s = String::new();
    for i in idx {
        match rec.get(i) {
            Some(f) => {
                s.push('\u{1}');
                s.push_str(&f.value);
            }
            None => s.push('\u{2}'),
        }
        s.push('\0');
    }
    s
}

/// Two rows share at least half their non-empty fields (under the matched columns), and at
/// least one.
fn similar(o: &Record, n: &Record, matched: &[(usize, usize)], work: &mut usize) -> bool {
    score(o, n, matched, work) > 0
}

/// How alike two rows are: the non-empty fields they share, when they are [`similar`] (at
/// least half of either side's, and at least one); 0 when they aren't.
fn score(o: &Record, n: &Record, matched: &[(usize, usize)], work: &mut usize) -> usize {
    let (mut eq, mut po, mut pn) = (0usize, 0usize, 0usize);
    for &(j, i) in matched {
        *work += 1;
        let a = o.get(i).map(|f| f.value.as_str()).filter(|v| !v.is_empty());
        let b = n.get(j).map(|f| f.value.as_str()).filter(|v| !v.is_empty());
        po += usize::from(a.is_some());
        pn += usize::from(b.is_some());
        eq += usize::from(a.is_some() && a == b);
    }
    if eq >= 1 && 2 * eq >= po.max(pn) {
        eq
    } else {
        0
    }
}

/// The edit pairing that doesn't depend on stretches: among the rows still unmatched, an old
/// row and a new row whose best match is each other, **uniquely** (the highest [`score`] on
/// both sides, no tie), are one row edited, wherever each is. Repeated while it pairs rows (a
/// pair can leave another row's best unique). A tie is ambiguous and pairs nothing here (the
/// stretch pairing then decides by position). Stops, pairing nothing more, past the budget.
fn pair_best(
    old: &[Record],
    new: &[Record],
    matched: &[(usize, usize)],
    new_old: &mut [Option<usize>],
    old_new: &mut [Option<usize>],
    work: &mut usize,
) {
    loop {
        let os: Vec<usize> = (0..old.len()).filter(|&o| old_new[o].is_none()).collect();
        let ns: Vec<usize> = (0..new.len()).filter(|&n| new_old[n].is_none()).collect();
        if os.is_empty() || ns.is_empty() {
            return;
        }
        let cost = os.len() * ns.len() * matched.len().max(1);
        if work.saturating_add(cost) > SIMILAR_BUDGET {
            return;
        }
        // best[x] = (score, the one partner with it, or None on a tie)
        let mut best_o: Vec<(usize, Option<usize>)> = vec![(0, None); os.len()];
        let mut best_n: Vec<(usize, Option<usize>)> = vec![(0, None); ns.len()];
        let better = |b: &mut (usize, Option<usize>), s: usize, x: usize| {
            if s > b.0 {
                *b = (s, Some(x));
            } else if s == b.0 && s > 0 {
                b.1 = None;
            }
        };
        for (a, &o) in os.iter().enumerate() {
            for (b, &n) in ns.iter().enumerate() {
                let s = score(&old[o], &new[n], matched, work);
                better(&mut best_o[a], s, b);
                better(&mut best_n[b], s, a);
            }
        }
        let mut linked = false;
        for (a, &o) in os.iter().enumerate() {
            if let (s, Some(b)) = best_o[a] {
                if s > 0 && best_n[b].1 == Some(a) {
                    link(o, ns[b], new_old, old_new);
                    linked = true;
                }
            }
        }
        if !linked {
            return;
        }
    }
}

/// The last pairing: **by position in an unchanged slot**. Each maximal run of unpaired new
/// rows sits between two paired rows (or an end of the table); the old rows between those
/// rows' partners (or the same end) are its old slot. When the slot is a run of unpaired old
/// rows exactly as long, they pair in order: whatever they hold, the file has those rows in
/// those places. A row whose slot changed (its neighbours aren't its old neighbours, the
/// counts differ, or a row in between is paired elsewhere) is left alone: moved and
/// rewritten, it is a new row. O(rows).
fn pair_in_place(new_old: &mut [Option<usize>], old_new: &mut [Option<usize>]) {
    let (n_len, o_len) = (new_old.len(), old_new.len());
    let mut n1 = 0;
    while n1 < n_len {
        if new_old[n1].is_some() {
            n1 += 1;
            continue;
        }
        let mut n2 = n1;
        while n2 < n_len && new_old[n2].is_none() {
            n2 += 1;
        }
        let o1 = match n1.checked_sub(1) {
            None => Some(0),
            Some(l) => new_old[l].map(|o| o + 1),
        };
        let o2 = if n2 == n_len {
            Some(o_len)
        } else {
            new_old[n2]
        };
        if let (Some(o1), Some(o2)) = (o1, o2) {
            if o2 >= o1 && o2 - o1 == n2 - n1 && (o1..o2).all(|o| old_new[o].is_none()) {
                for (o, n) in (o1..o2).zip(n1..n2) {
                    link(o, n, new_old, old_new);
                }
            }
        }
        n1 = n2;
    }
}

/// The cells (or names) a record asks for, by field position `0..width`: `Some((value,
/// quote))` present, `None` absent. `present(j)`: that position's cell is present now.
fn desired(
    shape: Shape,
    rec: &Record,
    width: usize,
    present: impl Fn(usize) -> bool,
) -> Vec<Option<(&str, Option<Quote>)>> {
    let mut out: Vec<Option<(&str, Option<Quote>)>> = (0..width)
        .map(|j| match rec.get(j) {
            None => None,
            Some(f) if !f.value.is_empty() || f.quote.is_some() => {
                Some((f.value.as_str(), f.quote))
            }
            Some(_) => present(j).then_some(("", None)),
        })
        .collect();
    if let Some(last) = rec.len().checked_sub(1) {
        match shape {
            Shape::Ragged if out[last].is_none() => out[last] = Some(("", None)),
            Shape::Rect if out.iter().all(Option::is_none) => out[0] = Some(("", None)),
            _ => {}
        }
    }
    out
}

fn axis<'a>(t: &'a Table, root: &str) -> &'a Axis {
    if root == ROWS {
        &t.rows
    } else {
        &t.cols
    }
}

/// Delete `gone`, then make the live order of `root` equal `target` (`fresh` keys are
/// inserted; every other key of `target` is live, and every live key is in `target`): a
/// longest run of live keys in their current relative order stays, and each other key is
/// placed right after its new left neighbour (or at the start), left to right, so
/// consecutive placements form one forward chain. A key already right after its new left
/// neighbour isn't moved. Returns the keys moved.
pub(crate) fn place(
    ch: &mut Change,
    w: &mut Table,
    root: &'static str,
    gone: &[Key],
    target: &[Key],
    fresh: &HashSet<Key>,
) -> Result<Vec<Key>, TableError> {
    for k in gone {
        ch.list(w, root, ListOp::Delete { key: k.clone() })?;
    }
    let cur = axis(w, root).live_keys();
    let pos: HashMap<&Key, usize> = cur.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let kept: Vec<&Key> = target.iter().filter(|k| !fresh.contains(*k)).collect();
    let seq: Vec<usize> = kept
        .iter()
        .map(|k| *pos.get(*k).expect("a kept key is live"))
        .collect();
    let stay: HashSet<&Key> = lis(&seq).into_iter().map(|i| kept[i]).collect();
    let mut moved = Vec::new();
    for (i, k) in target.iter().enumerate() {
        if stay.contains(k) {
            continue;
        }
        let prev = i.checked_sub(1).map(|p| &target[p]);
        let at = prev.map_or(Anchor::Start, |p| Anchor::After(p.clone()));
        if fresh.contains(k) {
            ch.list(
                w,
                root,
                ListOp::Insert {
                    key: k.clone(),
                    at,
                    value: Value::Null,
                },
            )?;
        } else {
            if axis(w, root).next_live_after(prev) == Some(k) {
                continue;
            }
            ch.list(
                w,
                root,
                ListOp::Move {
                    key: k.clone(),
                    to: at,
                },
            )?;
            moved.push(k.clone());
        }
    }
    Ok(moved)
}
