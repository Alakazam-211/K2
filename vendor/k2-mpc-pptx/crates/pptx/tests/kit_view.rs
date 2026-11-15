//! The kit's view index of a deck (the pack's view schema, addresses, and the kit's `ViewIndex`
//! built from it) against the hand-made expected index kept in the test, field by field, and the
//! blob tier's index against the live one.
//!
//! On the live tier the pack reads what only the file's bytes hold (fonts, render hashes, field
//! kinds, inherited boxes, a group's children) from the carrier the kit hands its derive hook
//! (`live::view_derive`), so the live index is the golden with no gap listed ([`LIVE_KNOWN`] is
//! empty; render hashes are compared as the golden's labels: one hash per label, and a different
//! one per label). The blob tier reads the version's structure index, whose view items (the kit's
//! row format 2) the index reader works out with the same code (`index::draft`, through
//! `view::read_open`), so it gives the live tier's index with no gap listed either ([`BLOB_KNOWN`]
//! is empty; render hashes as labels on both tiers, since each tier keys them under its own key,
//! and the raw digests are checked to be the pack's view's). Any other difference fails a row, and
//! so does a listed one that no longer shows (the lists are kept exact). Where the golden itself is
//! the odd one out, [`GOLDEN_DISAGREE`] says so.
//!
//! The carrier's rows: N reads of an unchanged deck decode the carrier zero times, a group the
//! store moved moves its children (as the pack's view of the written file places them), and a
//! keyed watch's change frame carries the touched slide's new render hash.

mod common;

use std::collections::BTreeSet;
use std::sync::Arc;

use common::kit_view_golden::rev40;
use common::live::fx;
use common::live_rows::agent_verb;
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::index::IndexLimits;
use k2_mpc_base::session::{
    ConnId, ParticipantKind, Session, Stay, WatchFrame, WatchOptions, KEYED_WATCH_FEATURE,
};
use k2_mpc_base::store::DocKey;
use k2_mpc_base::view_index::ViewIndex;
use k2_mpc_pptx::index::draft;
use k2_mpc_pptx::live::{view_derive, SHAPES, SLIDES};
use k2_mpc_pptx::testgen::golden::{self, Extra};
use k2_mpc_pptx::{view, Pptx, PptxPack};

/// Where the kit's live index differs from the golden: nowhere (the carrier's items closed
/// every gap).
const LIVE_KNOWN: &[(&str, &str)] = &[];

fn live_known() -> Vec<(String, String)> {
    LIVE_KNOWN
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

/// Where the blob tier's index differs from the live one (both the kit's), by path, with why:
/// nowhere (the structure index's view items closed every gap).
const BLOB_KNOWN: &[(&str, &str)] = &[];

fn blob_known() -> Vec<(String, String)> {
    BLOB_KNOWN
        .iter()
        .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
        .collect()
}

/// `got`'s render hashes as the golden's labels (`rh:256:a`), when they stand for them: every
/// container has one, two containers have the same hash exactly when the golden gives them the
/// same label. Otherwise `got` is left as it is, and the comparison shows the hashes.
fn as_labels(got: &mut Json, want: &Json) {
    let hashes = |v: &Json| -> Vec<Option<String>> {
        match v.get("containers") {
            Some(Json::List(l)) => l
                .iter()
                .map(|c| {
                    c.get("render_hash")
                        .and_then(Json::as_str)
                        .map(str::to_owned)
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let (g, w) = (hashes(got), hashes(want));
    let fits = g.len() == w.len()
        && g.iter().all(Option::is_some)
        && (0..g.len()).all(|i| (0..g.len()).all(|j| (g[i] == g[j]) == (w[i] == w[j])));
    if !fits {
        return;
    }
    if let Json::Obj(fields) = got {
        for (k, v) in fields.iter_mut() {
            if k == "containers" {
                if let Json::List(l) = v {
                    for (c, label) in l.iter_mut().zip(&w) {
                        if let Json::Obj(cf) = c {
                            for (ck, cv) in cf.iter_mut() {
                                if ck == "render_hash" {
                                    *cv = Json::from(label.as_ref());
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Where the golden is the odd one out: `(what, why)`. Printed by
/// `golden_disagreements_are_listed`.
pub const GOLDEN_DISAGREE: &[(&str, &str)] = &[(
    "units[*].approx_box",
    "the golden's units leave out `approx_box` (the view index has it on every unit, and \
     the cases give it); the kit's index carries it, false on this deck",
)];

/// Every place two JSON values differ, by path, each once: the golden's (`want`) fields only
/// (a field the golden leaves out is checked once, by [`unit_fields_row`]); numbers compared
/// as written.
fn diff(path: &str, got: &Json, want: &Json, out: &mut Vec<String>) {
    match (got, want) {
        (Json::Obj(_), Json::Obj(b)) => {
            for (k, y) in b {
                let p = format!("{path}.{k}");
                match got.get(k) {
                    Some(x) => diff(&p, x, y, out),
                    None => out.push(format!("{p}: missing")),
                }
            }
        }
        (Json::List(a), Json::List(b)) => {
            if a.len() != b.len() {
                out.push(format!("{path}: {} items, want {}", a.len(), b.len()));
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                diff(&format!("{path}[{i}]"), x, y, out);
            }
        }
        _ if got.to_json() != want.to_json() => out.push(format!(
            "{path}: got {}, want {}",
            got.to_json(),
            want.to_json()
        )),
        _ => {}
    }
}

fn units(v: &Json) -> Vec<(String, &Json)> {
    match v.get("units") {
        Some(Json::List(l)) => l
            .iter()
            .map(|u| {
                let k = u
                    .get("key")
                    .and_then(Json::as_str)
                    .unwrap_or("?")
                    .to_owned();
                (k, u)
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// `got` against `want`: every field but the units by path, the units by key.
fn compare(got: &Json, want: &Json) -> Vec<String> {
    let mut out = Vec::new();
    if let (Json::Obj(_), Json::Obj(w)) = (got, want) {
        for (k, y) in w.iter().filter(|(k, _)| k != "units") {
            match got.get(k) {
                Some(x) => diff(k, x, y, &mut out),
                None => out.push(format!("{k}: missing")),
            }
        }
    }
    let (g, w) = (units(got), units(want));
    for (k, wu) in &w {
        match g.iter().find(|(gk, _)| gk == k) {
            Some((_, gu)) => diff(&format!("units[{k}]"), gu, wu, &mut out),
            None => out.push(format!("units[{k}]: missing")),
        }
    }
    for (k, _) in &g {
        if !w.iter().any(|(wk, _)| wk == k) {
            out.push(format!("units[{k}]: not wanted"));
        }
    }
    let order = |v: &[(String, &Json)]| v.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
    let (go, wo): (Vec<String>, Vec<String>) = (order(&g), order(&w));
    let common: Vec<&String> = wo.iter().filter(|k| go.contains(k)).collect();
    let got_common: Vec<&String> = go.iter().filter(|k| wo.contains(k)).collect();
    if common != got_common {
        out.push(format!(
            "units: in the order {got_common:?}, want {common:?}"
        ));
    }
    out
}

/// The differences not explained by `known`, and the known ones that didn't show.
fn judge(row: &str, diffs: Vec<String>, known: &[(String, String)]) -> Result<(), String> {
    let path = |l: &str| l.split(": ").next().unwrap_or("").to_owned();
    let seen: BTreeSet<String> = diffs.iter().map(|l| path(l)).collect();
    let mut bad: Vec<String> = diffs
        .iter()
        .filter(|l| !known.iter().any(|(k, _)| *k == path(l)))
        .cloned()
        .collect();
    for (k, why) in known {
        if !seen.contains(k) {
            bad.push(format!("{k}: listed as known ({why}) but the same now"));
        }
    }
    match bad.is_empty() {
        true => Ok(()),
        false => Err(format!("{row}:\n{}", bad.join("\n"))),
    }
}

/// The deck's index on both tiers, from a live session over the golden deck's file and a blob
/// session over the same bytes.
fn indexes() -> Result<(ViewIndex, ViewIndex, u64), String> {
    let f = fx("kit-view", |_| {});
    let bytes = golden::deck(Extra::None);
    f.write(&bytes);
    let s = f.open();
    let c = f.person(&s);
    let live = s
        .view_index(c)
        .map_err(|e| format!("live: {e}"))?
        .ok_or("live: the pack has no view index")?;
    let rev = s.status().unwrap().version;
    let b = f
        .mgr
        .open_blob(
            Arc::new(PptxPack::default()),
            &DocKey {
                space: "ws".into(),
                path: "blob.pptx".into(),
            },
            Some(&mut &bytes[..]),
        )
        .map_err(|e| format!("blob: {e}"))?;
    b.index_job(None)
        .map_err(|e| format!("blob: {e}"))?
        .ok_or("blob: no index job")?
        .run()
        .map_err(|e| format!("blob: {e}"))?;
    let bc = b
        .join(
            k2_mpc_base::session::Participant::new(
                "ada",
                "ada",
                "#123456",
                ParticipantKind::Person,
                "cli",
            ),
            Stay::Held,
        )
        .map_err(|e| format!("blob: {e}"))?
        .conn;
    let blob = b
        .view_index(bc, None)
        .map_err(|e| format!("blob: {e}"))?
        .ok_or("blob: the pack has no view index")?;
    Ok((live, blob, rev))
}

/// The live tier against the expected index, field by field. The golden's `rev` is its
/// script's: the index's own is checked to be the session's version, then set to 40.
pub fn row_live_rev40() -> Result<(), String> {
    let (mut live, _, rev) = indexes()?;
    if live.rev != Some(rev) {
        return Err(format!("live: rev {:?}, the session's is {rev}", live.rev));
    }
    live.rev = Some(40);
    let want = rev40();
    let mut got = live.to_value();
    as_labels(&mut got, &want);
    judge("row_live_rev40", compare(&got, &want), &live_known())
}

/// The blob tier against the live tier, field by field (`tier`, `rev` and `version` aside),
/// each tier's render hashes as the golden's labels (each tier keys them under its own key);
/// and, under that, the draft's raw render digests are the pack's own view's of the same bytes
/// (what the live tier keys for a slide it holds as the file does).
pub fn row_blob_vs_live() -> Result<(), String> {
    let (live, mut blob, _) = indexes()?;
    if blob.version.is_none() || blob.rev.is_some() {
        return Err(format!(
            "blob: rev {:?}, version {:?}",
            blob.rev, blob.version
        ));
    }
    (blob.tier, blob.rev, blob.version) = (live.tier, live.rev, live.version);
    let want = rev40();
    let (mut b, mut l) = (blob.to_value(), live.to_value());
    as_labels(&mut b, &want);
    as_labels(&mut l, &want);
    judge("row_blob_vs_live", compare(&b, &l), &blob_known())?;
    let bytes = golden::deck(Extra::None);
    let lim = common::limits();
    let p = Pptx::open(&bytes, &lim).map_err(|d| d.to_string())?;
    let d = draft(&p, &IndexLimits::new(100_000, 64, 64 << 20, 64 << 10), &lim)
        .map_err(|e| e.to_string())?;
    let file = view::index(&bytes, &lim).map_err(|e| e.to_string())?;
    for c in &file.containers {
        let n = d
            .nodes
            .iter()
            .find(|n| n.key.as_deref() == Some(c.key.as_str()))
            .ok_or_else(|| format!("row_blob_vs_live: no node {}", c.key))?;
        if n.render != Some(c.render_hash) {
            return Err(format!(
                "row_blob_vs_live: {}: the draft's render digest isn't the view's",
                c.key
            ));
        }
    }
    Ok(())
}

/// The kit's unit fields are the golden's, and `approx_box` ([`GOLDEN_DISAGREE`]).
pub fn unit_fields_row() -> Result<(), String> {
    let (live, _, _) = indexes()?;
    let fields = |v: &Json| -> Vec<String> {
        match v {
            Json::Obj(m) => m.iter().map(|(k, _)| k.clone()).collect(),
            _ => Vec::new(),
        }
    };
    let got = live.to_value();
    let want = rev40();
    let g = fields(units(&got).first().ok_or("no units")?.1);
    let mut w = fields(units(&want)[0].1);
    w.insert(
        w.iter().position(|k| k == "flip").unwrap() + 1,
        "approx_box".into(),
    );
    match g == w {
        true => Ok(()),
        false => Err(format!("unit fields {g:?}, want {w:?}")),
    }
}

#[test]
fn the_kit_index_of_the_golden_deck_is_the_golden_but_the_listed_gaps() {
    row_live_rev40().unwrap();
}

#[test]
fn the_blob_tier_gives_the_live_tier_s_index_but_the_listed_gaps() {
    row_blob_vs_live().unwrap();
}

#[test]
fn the_kit_index_has_the_golden_s_unit_fields() {
    unit_fields_row().unwrap();
}

/// The keys of the live index are the pack's addresses, and lead back to the store's keys.
#[test]
fn the_index_speaks_addresses() {
    let f = fx("kit-view-addr", |_| {});
    f.write(&golden::deck(Extra::None));
    let s = f.open();
    let c = f.person(&s);
    let ix = s.view_index(c).unwrap().expect("a view index");
    for (root, keys) in [
        (
            SLIDES,
            ix.containers
                .iter()
                .map(|x| x.key.clone())
                .collect::<Vec<_>>(),
        ),
        // the live units: a group's children are read from the carrier, not the store's
        (
            SHAPES,
            ix.units
                .iter()
                .filter(|x| x.live)
                .map(|x| x.key.clone())
                .collect(),
        ),
    ] {
        for a in keys {
            let (r, k) = s.resolve_address(c, &a).unwrap();
            assert_eq!(r, root, "{a}");
            assert_eq!(s.address_of(&r, &k), a, "{a} round trip");
        }
    }
    assert_eq!(s.resolve_address(c, "@sld:257").unwrap().1.as_str(), "s257");
    assert_eq!(
        s.resolve_address(c, "@sld:257/sp:7").unwrap().1.as_str(),
        "s257-7"
    );
}

#[test]
fn golden_disagreements_are_listed() {
    for (what, why) in GOLDEN_DISAGREE {
        println!("golden disagreement: {what}: {why}");
    }
    for (k, why) in live_known() {
        println!("not from the live store: {k}: {why}");
    }
    for (k, why) in blob_known() {
        println!("not on the blob tier: {k}: {why}");
    }
}

// ----- the carrier's rows -----

/// How many reads the counter row makes of an unchanged deck.
const READS: usize = 50;

/// The golden deck open live, with a person and an agent.
fn live_deck(tag: &str) -> (common::live::Fx, Arc<Session>, ConnId, ConnId) {
    let f = fx(tag, |_| {});
    f.write(&golden::deck(Extra::None));
    let s = f.open();
    let (p, a) = (f.person(&s), f.agent(&s));
    (f, s, p, a)
}

fn index(s: &Session, c: ConnId) -> Result<ViewIndex, String> {
    s.view_index(c)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no view index".to_owned())
}

/// `(x, y, w, h)` of a unit's box.
fn bbox_of(ix: &ViewIndex, key: &str) -> Option<(i64, i64, i64, i64)> {
    ix.units
        .iter()
        .find(|u| u.key == key)
        .and_then(|u| u.bbox)
        .map(|b| (b.x, b.y, b.w, b.h))
}

/// The carrier is decoded once per carrier, never per read: the first read of the live
/// deck's index decodes it once, `READS` more reads of the unchanged deck zero times (the same
/// index), a verb's change rebuilds the index with zero decodes, and the kit's write (a new
/// carrier) decodes it once again.
pub fn row_reads_decode_the_carrier_once() -> Result<(), String> {
    const ROW: &str = "row_reads_decode_the_carrier_once";
    let (_f, s, p, a) = live_deck("kit-view-decodes");
    view_derive::take_decodes();
    let first = index(&s, p)?;
    let n = view_derive::take_decodes();
    if n != 1 {
        return Err(format!(
            "{ROW}: the first read: {n} carrier decodes, want 1"
        ));
    }
    for i in 0..READS {
        let ix = index(&s, p)?;
        let n = view_derive::take_decodes();
        if n != 0 || ix != first {
            return Err(format!(
                "{ROW}: unchanged read {}: {n} carrier decodes (want 0), the same index: {}",
                i + 1,
                ix == first
            ));
        }
    }
    println!("{ROW}: {READS} reads with no change: 0 carrier decodes");
    let o = agent_verb(&s, a, &["shape", "move", "@sld:257/sp:7", "--x", "7100000"])
        .map_err(|e| format!("{ROW}: {e}"))?;
    if o.exit != 0 {
        return Err(format!("{ROW}: control: the move exits {}", o.exit));
    }
    let after = index(&s, p)?;
    let n = view_derive::take_decodes();
    if n != 0 || after.rev == first.rev {
        return Err(format!(
            "{ROW}: the read after a change: {n} carrier decodes (want 0), rev {:?} (was {:?})",
            after.rev, first.rev
        ));
    }
    if !s.flush(a).map_err(|e| e.to_string())?.written {
        return Err(format!("{ROW}: control: the kit wrote the file"));
    }
    index(&s, p)?;
    let n = view_derive::take_decodes();
    if n != 1 {
        return Err(format!(
            "{ROW}: the read after the kit's write (a new carrier): {n} carrier decodes, want 1"
        ));
    }
    Ok(())
}

#[test]
fn reads_decode_the_carrier_once() {
    row_reads_decode_the_carrier_once().unwrap();
}

/// A group the store moved moves its children: after the agent moves `@sld:257/sp:4` (a group
/// turned 90°) by (100000, 100000), unwritten, its children's composed boxes move with it; after
/// the kit's write they are the same, and they are the pack's own view of the written file's.
pub fn row_a_group_move_moves_its_children() -> Result<(), String> {
    const ROW: &str = "row_a_group_move_moves_its_children";
    let (f, s, p, a) = live_deck("kit-view-group");
    let before = index(&s, p)?;
    let kids = ["@sld:257/sp:5", "@sld:257/sp:6"];
    let was: Vec<_> = kids.iter().map(|k| bbox_of(&before, k)).collect();
    if was
        != [
            Some((2_000_000, 500_000, 2_000_000, 2_000_000)),
            Some((2_000_000, 3_000_000, 2_000_000, 1_000_000)),
        ]
    {
        return Err(format!(
            "{ROW}: control: the children before the move {was:?}"
        ));
    }
    let o = agent_verb(
        &s,
        a,
        &[
            "shape",
            "move",
            "@sld:257/sp:4",
            "--x",
            "1100000",
            "--y",
            "1600000",
        ],
    )
    .map_err(|e| format!("{ROW}: {e}"))?;
    if o.exit != 0 {
        return Err(format!("{ROW}: control: the move exits {}", o.exit));
    }
    let want = vec![
        Some((2_100_000, 600_000, 2_000_000, 2_000_000)),
        Some((2_100_000, 3_100_000, 2_000_000, 1_000_000)),
    ];
    let live = index(&s, p)?;
    let got: Vec<_> = kids.iter().map(|k| bbox_of(&live, k)).collect();
    if got != want {
        return Err(format!(
            "{ROW}: after the move (unwritten), the children {got:?}, want {want:?}"
        ));
    }
    if !s.flush(a).map_err(|e| e.to_string())?.written {
        return Err(format!("{ROW}: control: the kit wrote the file"));
    }
    let written = index(&s, p)?;
    let got: Vec<_> = kids.iter().map(|k| bbox_of(&written, k)).collect();
    let file = view::index(&f.bytes(), &common::limits()).map_err(|d| d.to_string())?;
    let theirs: Vec<_> = kids
        .iter()
        .map(|k| {
            file.units
                .iter()
                .find(|u| u.key.as_deref() == Some(*k))
                .and_then(|u| u.bbox)
                .map(|b| (b.x, b.y, b.w, b.h))
        })
        .collect();
    if got != want || theirs != want {
        return Err(format!(
            "{ROW}: after the write, the children {got:?}, the file's view {theirs:?}, want {want:?}"
        ));
    }
    let hash = |ix: &ViewIndex| {
        ix.containers
            .iter()
            .find(|c| c.key == "@sld:257")
            .and_then(|c| c.render_hash)
    };
    if hash(&before) == hash(&live) || hash(&live).is_none() {
        return Err(format!(
            "{ROW}: the slide's render hash didn't change with the move"
        ));
    }
    Ok(())
}

#[test]
fn a_group_move_moves_its_children() {
    row_a_group_move_moves_its_children().unwrap();
}

/// A keyed watch's change frame carries the touched slide's new render hash (and no other
/// slide's), and its derived values: the slide's group children at their new boxes.
pub fn row_a_change_frame_carries_the_slide_s_render_hash() -> Result<(), String> {
    const ROW: &str = "row_a_change_frame_carries_the_slide_s_render_hash";
    let (_f, s, p, a) = live_deck("kit-view-watch");
    let mut w = s
        .watch_keyed(p, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .map_err(|e| e.to_string())?
        .ok_or("control: no watch")?;
    s.watch_snapshot(&mut w).map_err(|e| e.to_string())?;
    w.try_frames();
    let o = agent_verb(
        &s,
        a,
        &[
            "shape",
            "move",
            "@sld:257/sp:4",
            "--x",
            "1100000",
            "--y",
            "1600000",
        ],
    )
    .map_err(|e| format!("{ROW}: {e}"))?;
    if o.exit != 0 {
        return Err(format!("{ROW}: control: the move exits {}", o.exit));
    }
    let frames = w.try_frames();
    let changes: Vec<_> = frames
        .iter()
        .filter_map(|f| match f {
            WatchFrame::Change(c) => Some(c),
            _ => None,
        })
        .collect();
    if changes.len() != 1 {
        return Err(format!(
            "{ROW}: control: {} change frames, want 1",
            changes.len()
        ));
    }
    let ix = index(&s, p)?;
    let hash = ix
        .containers
        .iter()
        .find(|c| c.key == "@sld:257")
        .and_then(|c| c.render_hash)
        .ok_or("control: no render hash")?
        .to_hex();
    let want = vec![("@sld:257".to_owned(), hash)];
    if changes[0].extras.render_hashes != want {
        return Err(format!(
            "{ROW}: the frame's render hashes {:?}, want {want:?}",
            changes[0].extras.render_hashes
        ));
    }
    let derived = changes[0]
        .extras
        .derived
        .as_ref()
        .map(Json::to_json)
        .unwrap_or_default();
    if !derived.contains("\"@sld:257\"")
        || !derived.contains("\"key\":\"@sld:257/sp:5\"")
        || !derived.contains("\"x\":2100000,\"y\":600000")
    {
        return Err(format!("{ROW}: the frame's derived values {derived}"));
    }
    Ok(())
}

#[test]
fn a_change_frame_carries_the_slide_s_render_hash() {
    row_a_change_frame_carries_the_slide_s_render_hash().unwrap();
}
