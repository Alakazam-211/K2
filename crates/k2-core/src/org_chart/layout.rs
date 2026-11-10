//! The canvas lays itself out ("self right", prd-org-chart-v1 §6.6). Pure
//! and deterministic: every client and a headless box get the same chart.
//!
//! 1. **Cards** are one size (220×72). A bucket's grid is `ceil(sqrt(n))`
//!    columns (at most 4), rows as needed, plus padding and a title bar, so
//!    a bucket grows as members join and shrinks as they leave. A person's
//!    resize is a minimum, never smaller than the cards inside.
//! 2. **Inside a bucket**, a card a person dragged keeps its spot (relative
//!    to the bucket); the rest fill the grid around it, head first, then by
//!    name (the caller orders them).
//! 3. **Buckets** sit in rows by their `below` lines (an org above another,
//!    or agents in different orgs above one another). Each row is ordered by
//!    name; buckets with no lines go at the right end of the top row. A
//!    bucket a person placed never moves: auto buckets flow around it.
//! 4. **Agents in no org** sit in a "Not in an org" row under the buckets;
//!    agents on other servers in a row under that.

use std::collections::{BTreeMap, BTreeSet};

pub const CARD_W: f64 = 220.0;
pub const CARD_H: f64 = 72.0;
/// Between cards in a bucket.
pub const GAP: f64 = 16.0;
/// Inside a bucket's edge.
pub const PAD: f64 = 16.0;
/// A bucket's title bar.
pub const TITLE_H: f64 = 40.0;
/// Between buckets in a row.
pub const BUCKET_GAP: f64 = 80.0;
/// Between rows.
pub const ROW_GAP: f64 = 120.0;
/// At most this many columns in a bucket.
pub const MAX_COLS: usize = 4;
/// Cards per row in the "Not in an org" row.
pub const LOOSE_COLS: usize = 6;

/// A rectangle on the canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// One bucket to lay out.
#[derive(Debug, Clone, PartialEq)]
pub struct OrgIn {
    pub id: String,
    pub name: String,
    /// Members in grid order (head first, then by name).
    pub members: Vec<String>,
    /// Where a person put it: `(x, y)`.
    pub placed: Option<(f64, f64)>,
    /// A person's resize: `(w, h)`, a minimum.
    pub min_size: Option<(f64, f64)>,
}

/// The input.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Input {
    pub orgs: Vec<OrgIn>,
    /// Members a person dragged inside their bucket: agent → `(x, y)`
    /// relative to the bucket.
    pub placed_members: BTreeMap<String, (f64, f64)>,
    /// Agents in no org, in order (by name).
    pub loose: Vec<String>,
    /// Loose agents a person placed: agent → canvas `(x, y)`.
    pub placed_loose: BTreeMap<String, (f64, f64)>,
    /// Agents on other servers, in order.
    pub remote: Vec<String>,
    /// `(upper org, lower org)` pairs from the `below` lines.
    pub org_edges: Vec<(String, String)>,
}

/// Where everything went.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Output {
    /// Bucket rectangles, and whether a person placed each.
    pub orgs: BTreeMap<String, (Rect, bool)>,
    /// Card top-left corners on the canvas (members included), and whether
    /// a person placed each.
    pub agents: BTreeMap<String, (f64, f64, bool)>,
    /// Agents on other servers.
    pub remote: BTreeMap<String, (f64, f64)>,
}

/// A bucket's grid for `n` cards: `(columns, rows)`. An empty bucket
/// keeps room for one card.
pub fn grid(n: usize) -> (usize, usize) {
    if n == 0 {
        return (1, 1);
    }
    let cols = ((n as f64).sqrt().ceil() as usize).clamp(1, MAX_COLS);
    (cols, n.div_ceil(cols))
}

/// The size of a bucket holding a `cols` × `rows` grid.
pub fn grid_size(cols: usize, rows: usize) -> (f64, f64) {
    let w = PAD * 2.0 + cols as f64 * CARD_W + cols.saturating_sub(1) as f64 * GAP;
    let h = TITLE_H + PAD * 2.0 + rows as f64 * CARD_H + rows.saturating_sub(1) as f64 * GAP;
    (w, h)
}

fn cell(col: usize, row: usize) -> (f64, f64) {
    (PAD + col as f64 * (CARD_W + GAP), TITLE_H + PAD + row as f64 * (CARD_H + GAP))
}

fn card_at(x: f64, y: f64) -> Rect {
    Rect { x, y, w: CARD_W, h: CARD_H }
}

/// One bucket's inside: each member's position relative to the bucket,
/// whether a person placed it, and the bucket's size.
pub fn bucket(org: &OrgIn, placed: &BTreeMap<String, (f64, f64)>) -> (Vec<(String, f64, f64, bool)>, f64, f64) {
    let (cols, _) = grid(org.members.len());
    let fixed: Vec<Rect> = org
        .members
        .iter()
        .filter_map(|m| placed.get(m).map(|(x, y)| card_at(*x, *y)))
        .collect();
    let mut out = Vec::with_capacity(org.members.len());
    let mut next = 0usize;
    for m in &org.members {
        if let Some((x, y)) = placed.get(m) {
            out.push((m.clone(), *x, *y, true));
            continue;
        }
        // The next grid cell no person-placed card covers.
        loop {
            let (cx, cy) = cell(next % cols, next / cols);
            next += 1;
            if !fixed.iter().any(|f| f.overlaps(&card_at(cx, cy))) {
                out.push((m.clone(), cx, cy, false));
                break;
            }
        }
    }
    let rows_used = if next == 0 { 0 } else { next.div_ceil(cols) };
    let (gc, gr) = grid(org.members.len());
    let (mut w, mut h) = grid_size(gc, gr.max(rows_used));
    for (_, x, y, _) in &out {
        w = w.max(x + CARD_W + PAD);
        h = h.max(y + CARD_H + PAD);
    }
    if let Some((mw, mh)) = org.min_size {
        w = w.max(mw);
        h = h.max(mh);
    }
    (out, w, h)
}

/// Row (level) of each org from the `below` edges: the longest path from a
/// top org. A loop (lines that disagree) can't push past the org count.
fn levels(orgs: &[OrgIn], edges: &[(String, String)]) -> BTreeMap<String, usize> {
    let ids: BTreeSet<&str> = orgs.iter().map(|o| o.id.as_str()).collect();
    let edges: Vec<(&str, &str)> = edges
        .iter()
        .filter(|(a, b)| a != b && ids.contains(a.as_str()) && ids.contains(b.as_str()))
        .map(|(a, b)| (a.as_str(), b.as_str()))
        .collect();
    let mut level: BTreeMap<String, usize> = orgs.iter().map(|o| (o.id.clone(), 0)).collect();
    for _ in 0..orgs.len() {
        let mut changed = false;
        for (a, b) in &edges {
            let la = level[*a];
            if level[*b] < la + 1 && la + 1 < orgs.len().max(1) {
                level.insert((*b).to_string(), la + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    level
}

/// Lay out the whole chart.
pub fn layout(input: &Input) -> Output {
    let mut out = Output::default();
    let level = levels(&input.orgs, &input.org_edges);
    let linked: BTreeSet<&str> = input
        .org_edges
        .iter()
        .flat_map(|(a, b)| [a.as_str(), b.as_str()])
        .collect();

    // Sizes first.
    let mut insides: BTreeMap<&str, (Vec<(String, f64, f64, bool)>, f64, f64)> = BTreeMap::new();
    for o in &input.orgs {
        insides.insert(o.id.as_str(), bucket(o, &input.placed_members));
    }

    // Person-placed buckets stay put.
    let mut taken: Vec<Rect> = Vec::new();
    for o in &input.orgs {
        if let Some((x, y)) = o.placed {
            let (_, w, h) = &insides[o.id.as_str()];
            let r = Rect { x, y, w: *w, h: *h };
            taken.push(r);
            out.orgs.insert(o.id.clone(), (r, true));
        }
    }

    // Auto buckets, row by row.
    let max_level = level.values().copied().max().unwrap_or(0);
    let mut row_y = 0.0;
    for lv in 0..=max_level {
        let mut row: Vec<&OrgIn> = input
            .orgs
            .iter()
            .filter(|o| o.placed.is_none() && level.get(&o.id).copied().unwrap_or(0) == lv)
            .collect();
        // Linked first, then by name; unlinked at the right of the top row.
        row.sort_by(|a, b| {
            let la = linked.contains(a.id.as_str());
            let lb = linked.contains(b.id.as_str());
            lb.cmp(&la).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.id.cmp(&b.id))
        });
        if row.is_empty() {
            continue;
        }
        let mut x = 0.0;
        let mut row_h: f64 = 0.0;
        for o in row {
            let (_, w, h) = &insides[o.id.as_str()];
            let mut r = Rect { x, y: row_y, w: *w, h: *h };
            // Step right past any person-placed bucket in the way.
            while let Some(block) = taken.iter().find(|t| t.overlaps(&r)).copied() {
                r.x = block.x + block.w + BUCKET_GAP;
            }
            taken.push(r);
            out.orgs.insert(o.id.clone(), (r, false));
            x = r.x + r.w + BUCKET_GAP;
            row_h = row_h.max(*h);
        }
        row_y += row_h + ROW_GAP;
    }

    // Members on the canvas.
    for o in &input.orgs {
        let (rect, _) = out.orgs[&o.id];
        let (cards, _, _) = &insides[o.id.as_str()];
        for (id, rx, ry, person) in cards {
            out.agents.insert(id.clone(), (rect.x + rx, rect.y + ry, *person));
        }
    }

    // The "Not in an org" row, under every bucket.
    let bottom = out.orgs.values().map(|(r, _)| r.y + r.h).fold(0.0_f64, f64::max);
    let mut y0 = if out.orgs.is_empty() { 0.0 } else { bottom + ROW_GAP };
    let fixed: Vec<Rect> = input.placed_loose.values().map(|(x, y)| card_at(*x, *y)).collect();
    let mut i = 0usize;
    for id in &input.loose {
        if let Some((x, y)) = input.placed_loose.get(id) {
            out.agents.insert(id.clone(), (*x, *y, true));
            continue;
        }
        loop {
            let x = (i % LOOSE_COLS) as f64 * (CARD_W + GAP);
            let y = y0 + (i / LOOSE_COLS) as f64 * (CARD_H + GAP);
            i += 1;
            if !fixed.iter().any(|f| f.overlaps(&card_at(x, y))) {
                out.agents.insert(id.clone(), (x, y, false));
                break;
            }
        }
    }
    if i > 0 {
        y0 += i.div_ceil(LOOSE_COLS) as f64 * (CARD_H + GAP) - GAP + ROW_GAP;
    }
    // Agents on other servers.
    for (j, id) in input.remote.iter().enumerate() {
        let x = (j % LOOSE_COLS) as f64 * (CARD_W + GAP);
        let y = y0 + (j / LOOSE_COLS) as f64 * (CARD_H + GAP);
        out.remote.insert(id.clone(), (x, y));
    }
    out
}
