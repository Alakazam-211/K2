//! A set of sheets, read together (`av.set/1`):
//!
//! - **devices**: a device is every block reference whose attribute, by one of the configured
//!   tags, holds its identifier; the same identifier on several sheets is one device. A value
//!   under a device tag that doesn't have an identifier's shape is a finding;
//! - **boundaries**: closed polylines on the configured layers or in the configured linetypes,
//!   each named by the first text inside it;
//! - **cables**: each line or polyline on a cable layer, and each reference of a cable block (a
//!   straight run drawn as a block: its ends its block's long axis as placed, its attributes
//!   given), with its end points, the devices at its ends (within the snap distance of a
//!   device's insertion point), its label and the boundaries it crosses. Its label is the text
//!   of a leader one of whose arrows (a multileader's, on any of its lines) touches it (within
//!   the snap distance), else the nearest text within the label distance that no leader holds;
//!   matched by its plain text (inline formatting codes dropped), of the configured label shape
//!   when there is one. An end at no device and at no other cable (a junction, where the run
//!   goes on), or a cable without a label, is a finding;
//! - **the wire pull list**: the cables that cross a boundary, each a pull;
//! - **flags**: an outgoing flag paired with the incoming flag of the same identifier, across the
//!   set; or, for flags of one kind with no direction, the two of the same identifier (any value,
//!   not only an identifier's shape). One without its partner, or an identifier used twice in one
//!   direction (or more than twice for one kind), is a finding, as is a flag pointing at a sheet
//!   its partner isn't on;
//! - **reference drawings**: a drawing the set's sheets reference (an external reference resolved
//!   by its path within the set) that has no sheet title of its own, a backdrop or a title block
//!   drawing, listed with the sheets that reference it, apart from the sheet index;
//! - **the sheet index**: each sheet's number, name and latest revision, from its title block; a
//!   sheet without one is a finding.
//!
//! A block reference goes by its block's name, a dynamic block's own for a reference drawn
//! through one of its anonymous blocks. Addresses are the drawing's own (`@h:<HANDLE>`), with
//! the sheet's name beside them.

use std::collections::BTreeMap;

use k2_mpc_base::errors::Value;
use k2_mpc_cad_model::error::CadError;
use k2_mpc_cad_model::geom::{in_polygon, segments, to_segment};
use k2_mpc_cad_model::reads::{ltype_eff, relate_points, ReadOut, Relation};
use k2_mpc_cad_model::{mutant, BlockKind, Doc};

use crate::config::{among, Config};
use crate::SHEET_UNREADABLE;

/// One sheet of the set: its name (its file's) and its drawing.
pub struct Sheet {
    pub name: String,
    pub doc: Doc,
}

fn finding(code: &str, sheet: &str, address: Option<String>, message: String) -> Value {
    Value::obj([
        ("code", Value::from(code)),
        ("sheet", Value::from(sheet)),
        ("address", address.map_or(Value::Null, Value::from)),
        ("message", Value::from(message)),
    ])
}

/// The value under `tag` among entity `i`'s attributes (the first, without case).
fn attr(doc: &Doc, i: usize, tag: &str) -> Option<String> {
    doc.entities[i].extra.as_ref().and_then(|x| {
        x.attribs
            .iter()
            .find(|a| a.tag.eq_ignore_ascii_case(tag))
            .map(|a| a.value.to_string())
    })
}

/// The shortest distance from `p` to an outline (its points, closed or not).
fn to_outline(pts: &[[f64; 2]], closed: bool, p: [f64; 2]) -> f64 {
    let segs = segments(pts, closed);
    if segs.is_empty() {
        return pts.first().map_or(f64::INFINITY, |q| {
            ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt()
        });
    }
    segs.iter()
        .map(|(a, b)| to_segment(p, *a, *b))
        .fold(f64::INFINITY, f64::min)
}

/// Entity `k`'s text as it reads: a multiline text's or a multileader's without its inline
/// formatting codes, trimmed.
fn text_of(doc: &Doc, k: usize) -> String {
    let e = &doc.entities[k];
    let t = e.text.as_deref().unwrap_or("");
    if matches!(&*e.kind, "MTEXT" | "MULTILEADER" | "MLEADER") {
        k2_mpc_cad_model::leader::plain(t).trim().to_owned()
    } else {
        t.trim().to_owned()
    }
}

/// A leader that labels: its arrow points and its plain text.
type Labelling = (Vec<[f64; 2]>, String);

/// The leaders of `doc` that carry a text: each one's arrow points (a multileader's every
/// line's) and its plain text (a multileader's own; a leader's annotation's, by handle, else the nearest text to its last point within the
/// label distance). With every entity whose text a leader holds (the leaders themselves, their
/// annotations): no cable takes those by nearness.
fn leaders(cfg: &Config, doc: &Doc) -> (Vec<Labelling>, Vec<usize>) {
    let (mut out, mut held) = (Vec::new(), Vec::new());
    if mutant::active("LeaderLabelIgnored") {
        // the planted bug: leaders read as any other text
        return (out, held);
    }
    for (j, e) in doc.entities.iter().enumerate() {
        if !matches!(&*e.kind, "LEADER" | "MULTILEADER" | "MLEADER") {
            continue;
        }
        let Some(pts) = e.extra.as_ref().map(|x| &x.verts).filter(|v| !v.is_empty()) else {
            continue;
        };
        held.push(j);
        let annotation = e.extra.as_ref().and_then(|x| x.annotation);
        let by_handle = annotation
            .and_then(|h| doc.entity_by_handle(h))
            .filter(|&k| doc.entities[k].text.is_some());
        let tail = pts[pts.len() - 1];
        let source = if e.text.is_some() {
            Some(j)
        } else if by_handle.is_some() {
            by_handle
        } else {
            doc.entities
                .iter()
                .enumerate()
                .filter(|(k, t)| {
                    *k != j
                        && t.block.is_none()
                        && t.text.is_some()
                        && !matches!(&*t.kind, "LEADER" | "MULTILEADER" | "MLEADER")
                })
                .filter_map(|(k, t)| {
                    let p = t.at?;
                    let d = ((p[0] - tail[0]).powi(2) + (p[1] - tail[1]).powi(2)).sqrt();
                    (d <= cfg.label_within).then_some((d, k))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, k)| k)
        };
        if let Some(k) = source {
            held.push(k);
            // every arrow: a multileader's lines' first points, a leader's first point
            let lines = e.extra.as_ref().map_or(&[][..], |x| x.lines.as_slice());
            let arrows: Vec<[f64; 2]> = if lines.is_empty() {
                vec![pts[0]]
            } else {
                lines.iter().filter_map(|l| l.first().copied()).collect()
            };
            out.push((arrows, text_of(doc, k)));
        }
    }
    (out, held)
}

/// The reading's counts, never a value: where the sheets' titles came from, each device tag's
/// values by class ([`device_class`]), each flag tag's values (how many, empty, distinct, used
/// twice, once, more, identifier-shaped), and how cables' ends met devices.
/// The reading's counts, gathered for [`diagnostics`].
struct Counts<'a> {
    chain: &'a BTreeMap<&'static str, u64>,
    number_by: &'a BTreeMap<String, u64>,
    hints: &'a BTreeMap<&'static str, u64>,
    /// Sheets with title rows, then their revision tags, filled, date tags, filled, others.
    rows: [u64; 6],
    by_tag: &'a [BTreeMap<&'static str, u64>],
    flag_refs: (u64, u64),
    flag_tags: &'a BTreeMap<String, BTreeMap<String, u64>>,
    flag_ends: &'a BTreeMap<&'static str, u64>,
    ends: &'a BTreeMap<&'static str, u64>,
    open: &'a BTreeMap<&'static str, u64>,
    junctions: &'a BTreeMap<&'static str, u64>,
    cable_flags: &'a BTreeMap<&'static str, u64>,
}

fn diagnostics(c: &Counts<'_>) -> Value {
    let (chain, by_tag, flag_refs, flag_tags, ends) =
        (c.chain, c.by_tag, c.flag_refs, c.flag_tags, c.ends);
    let n = |m: &BTreeMap<&'static str, u64>, k: &str| Value::from(m.get(k).copied().unwrap_or(0));
    let title = Value::obj(
        [
            "sheets",
            "own",
            "by-reference",
            "copy-in-reference",
            "placing",
            "revision",
        ]
        .map(|k| (k, n(chain, k))),
    );
    let devices = Value::List(
        by_tag
            .iter()
            .map(|m| Value::obj(DEVICE_CLASSES.map(|k| (k, n(m, k)))))
            .collect(),
    );
    let flags = Value::List(
        flag_tags
            .iter()
            .map(|(tag, values)| {
                let count = |f: fn(u64) -> bool| {
                    Value::from(
                        values
                            .iter()
                            .filter(|(v, c)| !v.is_empty() && f(**c))
                            .count(),
                    )
                };
                Value::obj([
                    ("tag", Value::from(tag.as_str())),
                    ("values", Value::from(values.values().sum::<u64>())),
                    ("empty", Value::from(values.get("").copied().unwrap_or(0))),
                    (
                        "distinct",
                        Value::from(values.keys().filter(|v| !v.is_empty()).count()),
                    ),
                    ("paired", count(|c| c == 2)),
                    ("single", count(|c| c == 1)),
                    ("more", count(|c| c > 2)),
                    (
                        "id_shaped",
                        Value::from(values.keys().filter(|v| id_like(v)).count()),
                    ),
                ])
            })
            .collect(),
    );
    Value::obj([
        ("title", title),
        ("device_tags", devices),
        (
            "flag_refs",
            Value::obj([
                ("refs", Value::from(flag_refs.0)),
                ("with_id_tag", Value::from(flag_refs.1)),
            ]),
        ),
        ("flag_tags", flags),
        (
            "cable_ends",
            Value::obj(
                [
                    "ends",
                    "at-insertion",
                    "at-symbol",
                    "at-flag",
                    "junction",
                    "open",
                ]
                .map(|k| (k, n(ends, k))),
            ),
        ),
        (
            "open_ends",
            Value::obj(DISTANCES.map(|k| (k, n(c.open, k)))),
        ),
        (
            "cable_flags",
            Value::obj(["cables", "labelled"].map(|k| (k, n(c.cable_flags, k)))),
        ),
        (
            "junction_ends",
            Value::obj(DISTANCES.map(|k| (k, n(c.junctions, k)))),
        ),
        (
            "flag_ends",
            Value::obj(
                [
                    "ends",
                    "labelled",
                    "unlabelled",
                    "label-is-device",
                    "label-is-sheet",
                    "kinds",
                ]
                .map(|k| (k, n(c.flag_ends, k))),
            ),
        ),
        (
            "numbers",
            Value::obj(
                c.number_by
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::from(*v)))
                    .chain(
                        [
                            ("layout-yes", "yes"),
                            ("layout-not", "not"),
                            ("layout-none", "none"),
                            ("stem-yes", "stem-yes"),
                            ("stem-not", "stem-not"),
                        ]
                        .map(|(k, h)| (k.to_owned(), n(c.hints, h))),
                    ),
            ),
        ),
        (
            "rows",
            Value::obj(
                [
                    "sheets",
                    "rev-tags",
                    "rev-filled",
                    "date-tags",
                    "date-filled",
                    "other-numbered",
                ]
                .iter()
                .zip(c.rows)
                .map(|(k, v)| (*k, Value::from(v))),
            ),
        ),
    ])
}

/// An end tally's keys: how many, then each distance bucket, then those on a sheet with no
/// device, then those within 12 and 24 units of a flag.
const DISTANCES: [&str; 10] = [
    "ends",
    "lt0.5",
    "lt1",
    "lt3",
    "lt6",
    "lt12",
    "ge12",
    "no-device",
    "near-flag-12",
    "near-flag-24",
];

/// The open ends' distance buckets, each by its upper bound (drawing units); beyond the last,
/// `ge12`.
const OPEN_BUCKETS: [(&str, f64); 5] = [
    ("lt0.5", 0.5),
    ("lt1", 1.0),
    ("lt3", 3.0),
    ("lt6", 6.0),
    ("lt12", 12.0),
];

/// The classes [`device_class`] gives, in the order the summary prints them.
const DEVICE_CLASSES: [&str; 9] = [
    "values",
    "empty",
    "id-shaped",
    "lowercase",
    "separator",
    "no-separator",
    "suffix",
    "out-of-range",
    "other",
];

/// Whether `v` looks like an identifier of any size: capitals, a hyphen, digits.
fn id_like(v: &str) -> bool {
    v.split_once('-').is_some_and(|(l, d)| {
        !l.is_empty()
            && !d.is_empty()
            && l.bytes().all(|b| b.is_ascii_uppercase())
            && d.bytes().all(|b| b.is_ascii_digit())
    })
}

/// Entity `i` as a cable: its outline (a line's or a polyline's points on a cable layer; a run
/// drawn as a cable block, its block's long axis as placed), whether that is closed, and
/// whether it is a block; `None` when it isn't a cable.
fn run_outline(cfg: &Config, doc: &Doc, i: usize) -> Option<(Vec<[f64; 2]>, bool, bool)> {
    let e = &doc.entities[i];
    let block_run = e.block.is_some()
        && doc
            .block_name(e)
            .is_some_and(|b| among(&cfg.cable_blocks, b));
    if !block_run
        && !(among(&cfg.cable_layers, &e.layer)
            && matches!(&*e.kind, "LINE" | "LWPOLYLINE" | "POLYLINE"))
    {
        return None;
    }
    let (pts, closed) = if block_run {
        (doc.placed_axis(i).map_or(Vec::new(), |a| a.to_vec()), false)
    } else {
        doc.outline(i)
    };
    Some((pts, closed, block_run))
}

/// Where a cable's end lies: at a device (its identifier, and how: `at-insertion`,
/// `at-symbol`), at a flag (its label), at another cable, or nowhere.
enum EndAt {
    Device(String, &'static str),
    Flag(Option<String>),
    Junction,
    Open,
}

/// A flag on a sheet: where it sits, its symbol's box as placed, its label.
struct FlagAt {
    at: [f64; 2],
    symbol: Option<[f64; 4]>,
    label: Option<String>,
}

/// The distance from `p` to a placed symbol: to its insertion point `at` or its box, the
/// nearer.
fn near(at: [f64; 2], symbol: Option<[f64; 4]>, p: &[f64; 2]) -> f64 {
    let to_point = (p[0] - at[0]).hypot(p[1] - at[1]);
    let to_box = symbol.map_or(f64::INFINITY, |b| {
        let dx = (b[0] - p[0]).max(p[0] - b[2]).max(0.0);
        let dy = (b[1] - p[1]).max(p[1] - b[3]).max(0.0);
        dx.hypot(dy)
    });
    to_point.min(to_box)
}

/// A device's place on a sheet: its identifier, its insertion point and its symbol's box as
/// placed.
#[derive(Clone)]
struct DeviceAt {
    id: String,
    at: [f64; 2],
    symbol: Option<[f64; 4]>,
}

/// Why a device tag's value is or isn't an identifier, one word each: `empty`, `id-shaped`,
/// `lowercase` (an identifier in capitals), `separator` (one with a space, `_`, `.` or `/` for
/// its hyphen), `no-separator` (letters straight on to digits), `suffix` (an identifier, then
/// more), `out-of-range` (letters, a hyphen, digits, too many or too few), `other`.
fn device_class(cfg: &Config, v: &str) -> &'static str {
    if v.is_empty() {
        return "empty";
    }
    if cfg.is_device_id(v) {
        return "id-shaped";
    }
    if v.bytes().any(|b| b.is_ascii_lowercase()) && cfg.is_device_id(&v.to_ascii_uppercase()) {
        return "lowercase";
    }
    if [' ', '_', '.', '/']
        .iter()
        .any(|c| cfg.is_device_id(&v.replacen(*c, "-", 1)))
    {
        return "separator";
    }
    let letters = v.bytes().take_while(u8::is_ascii_alphabetic).count();
    if letters > 0
        && letters < v.len()
        && cfg.is_device_id(&format!("{}-{}", &v[..letters], &v[letters..]))
    {
        return "no-separator";
    }
    if let Some((l, d)) = v.split_once('-') {
        let digits = d.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && digits < d.len() && cfg.is_device_id(&format!("{l}-{}", &d[..digits])) {
            return "suffix";
        }
    }
    if id_like(v) {
        return "out-of-range";
    }
    "other"
}

struct Title {
    number: Option<String>,
    name: Option<String>,
    revision: Option<String>,
    /// The latest revision's date, when the dates are configured.
    date: Option<String>,
    /// The drawing of the set the revision rows come from, when not the sheet itself.
    from: Option<String>,
    /// How many different sheet numbers the sheet's own blocks give.
    numbers: usize,
    /// Where the revision rows came from: `own`, `by-reference`, `copy-in-reference`, `none`.
    source: &'static str,
    /// The sheet places a reference to a drawing of the set.
    placing: bool,
    /// Where its number came from: `tag-<k>` (the k-th sheet tag), `layout`, `file-stem`,
    /// `none`.
    number_source: String,
    /// Its rows' tags: revision tags and how many filled, date tags and how many filled, and
    /// other tags ending in a digit.
    row_counts: [u64; 5],
}

/// Whether `t` is shaped as a sheet number: up to four letters, a hyphen or a dot or nothing,
/// digits, a point and digits or nothing, a letter or nothing (`AV-201`, `101`, `E2.1`, `12A`),
/// without case.
fn sheet_like(t: &str) -> bool {
    let t = t.trim().to_ascii_uppercase();
    let b = t.as_bytes();
    let letters = b.iter().take_while(|c| c.is_ascii_uppercase()).count();
    if letters > 4 {
        return false;
    }
    let mut k = letters;
    if letters > 0 && matches!(b.get(k), Some(b'-' | b'.')) {
        k += 1;
    }
    let digits = b[k..].iter().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        return false;
    }
    k += digits;
    if b.get(k) == Some(&b'.') {
        let more = b[k + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
        if more == 0 {
            return false;
        }
        k += 1 + more;
    }
    if b.get(k).is_some_and(u8::is_ascii_uppercase) {
        k += 1;
    }
    k == b.len()
}

/// The sheet number a drawing's paper layout's name gives: the first paper layout named as a
/// sheet number.
fn layout_number(doc: &Doc) -> Option<String> {
    doc.blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Layout)
        .filter_map(|b| b.layout.as_deref())
        .find(|n| mutant::active("LayoutAnyName") || sheet_like(n))
        .map(|n| n.trim().to_owned())
}

/// The sheet number a file's name gives: its last word (split at spaces and `_`) shaped as one,
/// its extension and folders aside.
fn stem_number(name: &str) -> Option<String> {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let stem = file.rsplit_once('.').map_or(file, |(s, _)| s);
    stem.rsplit([' ', '_'])
        .find(|w| sheet_like(w))
        .map(str::to_owned)
}

/// How a drawing's layouts and file name bear on its number, for the diagnostics: `yes` when
/// a paper layout's name is shaped as a sheet number, `not` when none is, `none` when it names
/// no paper layout; and whether its file's name holds one.
fn number_hints(doc: &Doc, name: &str) -> (&'static str, bool) {
    let names: Vec<&str> = doc
        .blocks
        .iter()
        .filter(|b| b.kind == BlockKind::Layout)
        .filter_map(|b| b.layout.as_deref())
        .collect();
    let layout = if names.is_empty() {
        "none"
    } else if names.iter().any(|n| sheet_like(n)) {
        "yes"
    } else {
        "not"
    };
    (layout, stem_number(name).is_some())
}

/// The references of block `block` (by its real name) in `doc`: paper space's first (a sheet's
/// title is laid out on paper), then model space's, then those inside block definitions.
fn refs_of(doc: &Doc, block: &str) -> Vec<usize> {
    let mut v: Vec<usize> = (0..doc.entities.len())
        .filter(|&i| {
            doc.block_name(&doc.entities[i])
                .is_some_and(|b| b.eq_ignore_ascii_case(block))
        })
        .collect();
    v.sort_by_key(|&i| match doc.owner_kind(&doc.entities[i]) {
        Some(BlockKind::Layout) => 0,
        Some(BlockKind::Model) => 1,
        _ => 2,
    });
    v
}

/// A path's parts as the set compares them: either separator, `.` and `..` resolved (a relative
/// path against `base`, a folder's parts), a drive dropped, without case, the last part's
/// extension dropped.
fn parts(base: &[String], path: &str) -> Vec<String> {
    let p = path.trim().replace('\\', "/");
    let absolute = p.starts_with('/') || p.as_bytes().get(1) == Some(&b':');
    let mut out: Vec<String> = if absolute { Vec::new() } else { base.to_vec() };
    for (k, w) in p.split('/').enumerate() {
        match w {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            _ if k == 0 && w.ends_with(':') => {}
            _ => out.push(w.to_lowercase()),
        }
    }
    if let Some(last) = out.last_mut() {
        if let Some(dot) = last.rfind('.').filter(|d| *d > 0) {
            last.truncate(dot);
        }
    }
    out
}

/// The sheet of the set that an external reference's `path`, written in sheet `from`, names:
/// the one whose path (its folders and name, `paths`) ends the reference's, the longest such.
/// `None` when no sheet's does, or two do equally.
fn resolve(paths: &[Vec<String>], from: usize, path: &str) -> Option<usize> {
    let dir = &paths[from][..paths[from].len().saturating_sub(1)];
    let want = parts(dir, path);
    if mutant::active("XrefByStem") {
        // the planted bug: the first drawing of the same name, whatever its folder
        return (0..paths.len()).find(|&j| j != from && paths[j].last() == want.last());
    }
    let mut best: Option<(usize, usize)> = None;
    let mut tie = false;
    for (j, p) in paths.iter().enumerate() {
        if j == from || p.is_empty() || !want.ends_with(p) {
            continue;
        }
        match best {
            Some((_, n)) if n > p.len() => {}
            Some((_, n)) if n == p.len() => tie = true,
            _ => {
                best = Some((j, p.len()));
                tie = false;
            }
        }
    }
    best.filter(|_| !tie).map(|(j, _)| j)
}

/// Sheet `si`'s title: its number and name from its own sheet blocks when they are configured
/// (the first configured tag with a value), else from its title block; its revision rows from
/// its title block: placed on the sheet, else in a drawing of the set it references and places
/// (`xrefs`: its external references' blocks and the sheets they name), else a copy inside a
/// reference's block. `None` when it has neither.
fn title(
    cfg: &Config,
    sheets: &[Sheet],
    si: usize,
    xrefs: &[(usize, Option<usize>)],
) -> Option<Title> {
    let doc = &sheets[si].doc;
    let tb = cfg.title_block.as_deref();
    let placed = |d: &Doc, i: usize| {
        mutant::active("OwnTitleInsideBlocks")
            || matches!(
                d.owner_kind(&d.entities[i]),
                Some(BlockKind::Layout | BlockKind::Model)
            )
    };
    // the title block placed on the sheet itself
    let own = tb.and_then(|b| refs_of(doc, b).into_iter().find(|&i| placed(doc, i)));
    // else in the drawing of the set a reference the sheet places names
    let by_reference = || {
        doc.entities.iter().find_map(|e| {
            let bi = doc.insert_block(e)?;
            let t = xrefs.iter().find(|(b, _)| *b == bi)?.1?;
            let td = &sheets[t].doc;
            let i = refs_of(td, tb?).into_iter().find(|&i| placed(td, i))?;
            Some((td, i, Some(sheets[t].name.clone()), "by-reference"))
        })
    };
    // else a copy of it inside a reference's block (as a converter writes a referenced
    // drawing's content), named by that reference's path
    let copied = || {
        let i = refs_of(doc, tb?)
            .into_iter()
            .find(|&i| doc.entities[i].owner.is_some_and(|o| doc.blocks[o].xref))?;
        let o = doc.entities[i].owner?;
        let from = doc.blocks[o]
            .path
            .clone()
            .unwrap_or_else(|| doc.blocks[o].name.clone());
        Some((doc, i, Some(from), "copy-in-reference"))
    };
    let rows: Option<(&Doc, usize, Option<String>, &'static str)> = match own {
        Some(i) => Some((doc, i, None, "own")),
        None => by_reference().or_else(copied),
    };
    let numbered: Vec<usize> = if cfg.title_sheet_blocks.is_empty() {
        own.into_iter().collect()
    } else {
        let mut v: Vec<usize> = cfg
            .title_sheet_blocks
            .iter()
            .flat_map(|b| refs_of(doc, b))
            .filter(|&i| placed(doc, i))
            .collect();
        v.sort_by_key(|&i| u8::from(doc.owner_kind(&doc.entities[i]) != Some(BlockKind::Layout)));
        v
    };
    let placing = doc.entities.iter().any(|e| {
        doc.insert_block(e)
            .is_some_and(|bi| xrefs.iter().any(|(b, t)| *b == bi && t.is_some()))
    });
    // a reference's value by the first of `tags` it holds with a value
    let first_of = |tags: &[String], i: usize| {
        let tags = if mutant::active("SheetTagFirstOnly") {
            &tags[..tags.len().min(1)]
        } else {
            tags
        };
        tags.iter()
            .filter_map(|t| attr(doc, i, t))
            .map(|v| v.trim().to_owned())
            .find(|v| !v.is_empty())
    };
    // each sheet block's number, with the place of the tag that gave it
    let found: Vec<(usize, String)> = numbered
        .iter()
        .filter_map(|&i| {
            let tags = &cfg.title_sheet_tags;
            let tags = if mutant::active("SheetTagFirstOnly") {
                &tags[..tags.len().min(1)]
            } else {
                &tags[..]
            };
            tags.iter().enumerate().find_map(|(k, t)| {
                attr(doc, i, t)
                    .map(|v| v.trim().to_owned())
                    .filter(|v| !v.is_empty())
                    .map(|v| (k, v))
            })
        })
        .collect();
    let mut numbers: Vec<String> = found.iter().map(|(_, v)| v.clone()).collect();
    numbers.sort();
    numbers.dedup();
    // the number from the configured sources in order: the blocks, the layout's name, the file's
    let (mut number, mut number_source) = (None, "none".to_owned());
    for src in &cfg.number_from {
        let got = match src.as_str() {
            "blocks" => found
                .first()
                .map(|(k, v)| (format!("tag-{}", k + 1), v.clone())),
            "layout" => layout_number(doc).map(|v| ("layout".to_owned(), v)),
            _ => stem_number(&sheets[si].name).map(|v| ("file-stem".to_owned(), v)),
        };
        if let Some((how, v)) = got {
            number = Some(v);
            number_source = how;
            break;
        }
    }
    if rows.is_none() && numbered.is_empty() && number.is_none() {
        return None;
    }
    let name = numbered
        .iter()
        .find_map(|&i| first_of(&cfg.title_name_tags, i));
    let (mut revision, mut date, mut from, mut source) = (None, None, None, "none");
    // the rows' tags in counts: revision tags and filled, date tags and filled, other numbered
    let mut row_counts = [0u64; 5];
    if let Some((rd, ri, src, how)) = rows {
        source = how;
        let prefix = cfg.rev_prefix.to_ascii_uppercase();
        let dprefix = cfg.date_prefix.as_deref().map(str::to_ascii_uppercase);
        // a row's number after its prefix, a separator (`_`, `-`, a space) between allowed
        let row = |t: &str, p: &str| {
            let n = t.strip_prefix(p)?;
            let n = n.strip_prefix(['_', '-', ' ']).unwrap_or(n);
            n.parse::<u32>().ok()
        };
        let mut revs: BTreeMap<u32, String> = BTreeMap::new();
        let mut dates: BTreeMap<u32, String> = BTreeMap::new();
        for a in rd.entities[ri].extra.iter().flat_map(|x| x.attribs.iter()) {
            let t = a.tag.to_ascii_uppercase();
            let v = a.value.trim();
            let (r, d) = (
                row(&t, &prefix),
                dprefix.as_deref().and_then(|d| row(&t, d)),
            );
            if let Some(n) = r {
                row_counts[0] += 1;
                if !v.is_empty() {
                    row_counts[1] += 1;
                    revs.insert(n, v.to_owned());
                }
            }
            if let Some(n) = d {
                row_counts[2] += 1;
                row_counts[3] += u64::from(!v.is_empty());
                dates.insert(n, v.to_owned());
            }
            if r.is_none() && d.is_none() && t.ends_with(|c: char| c.is_ascii_digit()) {
                row_counts[4] += 1;
            }
        }
        let pick = if mutant::active("FirstRevision") {
            revs.iter().next()
        } else {
            revs.iter().next_back()
        };
        if let Some((n, v)) = pick {
            revision = Some(v.clone());
            date = dates.get(n).filter(|d| !d.is_empty()).cloned();
        }
        from = src;
    }
    Some(Title {
        number,
        name,
        revision,
        date,
        from,
        numbers: numbers.len(),
        source,
        placing,
        number_source,
        row_counts,
    })
}

/// A sheet-index row.
struct IndexRow {
    sheet: String,
    number: Option<String>,
    name: Option<String>,
    revision: Option<String>,
    date: Option<String>,
    from: Option<String>,
}

/// A flag: its sheet, its address, the sheet it points to.
struct Flag {
    sheet: String,
    address: String,
    target: Option<String>,
}

/// The set read together (module docs), as `av.set/1` typed data with its text.
pub fn read_set(cfg: &Config, sheets: &[Sheet]) -> ReadOut {
    let mut findings: Vec<Value> = Vec::new();
    // each sheet's external references, resolved within the set: their blocks and the sheets
    // they name
    let paths: Vec<Vec<String>> = sheets.iter().map(|s| parts(&[], &s.name)).collect();
    let xrefs: Vec<Vec<(usize, Option<usize>)>> = sheets
        .iter()
        .enumerate()
        .map(|(si, s)| {
            s.doc
                .blocks
                .iter()
                .enumerate()
                .filter(|(_, b)| b.xref)
                .map(|(bi, b)| (bi, b.path.as_deref().and_then(|p| resolve(&paths, si, p))))
                .collect()
        })
        .collect();
    // the drawings each is referenced by
    let mut referenced_by: Vec<Vec<usize>> = vec![Vec::new(); sheets.len()];
    for (si, xs) in xrefs.iter().enumerate() {
        for t in xs.iter().filter_map(|(_, t)| *t) {
            if !referenced_by[t].contains(&si) {
                referenced_by[t].push(si);
            }
        }
    }
    // the sheet index; a drawing the set's sheets reference that has no sheet title of its own
    // (a backdrop, a title block drawing) is a reference drawing, not a sheet
    let mut index: Vec<IndexRow> = Vec::new();
    let mut references: Vec<(String, Vec<String>)> = Vec::new();
    // where the sheets' titles came from, in counts; where their numbers came from; what their
    // layouts' and files' names hold; their rows' tags
    let mut chain: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut number_by: BTreeMap<String, u64> = BTreeMap::new();
    let mut hints: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut rows_sum = [0u64; 6];
    for (si, s) in sheets.iter().enumerate() {
        let t = title(cfg, sheets, si, &xrefs[si]);
        let untitled = t
            .as_ref()
            .is_none_or(|t| t.number.is_none() && t.name.is_none());
        if !referenced_by[si].is_empty() && (untitled || mutant::active("ReferenceByXrefOnly")) {
            let mut by: Vec<String> = referenced_by[si]
                .iter()
                .map(|&r| sheets[r].name.clone())
                .collect();
            by.sort();
            references.push((s.name.clone(), by));
            continue;
        }
        *chain.entry("sheets").or_default() += 1;
        let (layout, stem) = number_hints(&s.doc, &s.name);
        *hints.entry(layout).or_default() += 1;
        *hints
            .entry(if stem { "stem-yes" } else { "stem-not" })
            .or_default() += 1;
        match &t {
            Some(t) => {
                *chain.entry(t.source).or_default() += 1;
                *chain.entry("placing").or_default() += u64::from(t.placing);
                *chain.entry("revision").or_default() += u64::from(t.revision.is_some());
                *number_by.entry(t.number_source.clone()).or_default() += 1;
                if t.source != "none" {
                    rows_sum[0] += 1;
                    for (k, n) in t.row_counts.iter().enumerate() {
                        rows_sum[k + 1] += n;
                    }
                }
            }
            None => *number_by.entry("none".to_owned()).or_default() += 1,
        }
        match t {
            Some(t) => {
                if t.numbers > 1 {
                    findings.push(finding(
                        "sheet_number_many",
                        &s.name,
                        None,
                        format!(
                            "its sheet blocks give {} different numbers; the first is used",
                            t.numbers
                        ),
                    ));
                }
                index.push(IndexRow {
                    sheet: s.name.clone(),
                    number: t.number,
                    name: t.name,
                    revision: t.revision,
                    date: t.date,
                    from: t.from,
                });
            }
            None => {
                findings.push(finding(
                    "title_missing",
                    &s.name,
                    None,
                    "no title block on this sheet".into(),
                ));
                index.push(IndexRow {
                    sheet: s.name.clone(),
                    number: None,
                    name: None,
                    revision: None,
                    date: None,
                    from: None,
                });
            }
        }
    }
    index.sort_by(|a, b| {
        let num = |x: &Option<String>| x.as_deref().and_then(|v| v.parse::<u64>().ok());
        (num(&a.number), &a.number, &a.sheet).cmp(&(num(&b.number), &b.number, &b.sheet))
    });
    let number_of: BTreeMap<String, Option<String>> = index
        .iter()
        .map(|r| (r.sheet.clone(), r.number.clone()))
        .collect();

    // devices, and where each sits on each sheet (for the cables' ends)
    // a reference is a device by the first of its configured tags that holds an identifier;
    // one that holds none is one finding, for its first non-empty value (an empty value is
    // counted, not a finding)
    let mut devices: BTreeMap<String, Vec<(String, String, String)>> = BTreeMap::new();
    let mut at: Vec<Vec<DeviceAt>> = vec![Vec::new(); sheets.len()];
    let mut by_tag: Vec<BTreeMap<&'static str, u64>> = vec![BTreeMap::new(); cfg.device_tags.len()];
    for (si, s) in sheets.iter().enumerate() {
        let doc = &s.doc;
        for i in 0..doc.entities.len() {
            let mut id: Option<String> = None;
            let mut bad: Option<(&str, String)> = None;
            for (k, tag) in cfg.device_tags.iter().enumerate() {
                let Some(v) = attr(doc, i, tag) else { continue };
                let v = v.trim().to_owned();
                let class = device_class(cfg, &v);
                *by_tag[k].entry("values").or_default() += 1;
                *by_tag[k].entry(class).or_default() += 1;
                if class == "id-shaped" {
                    id = id.or(Some(v));
                } else if class != "empty" && bad.is_none() {
                    bad = Some((tag.as_str(), v));
                }
            }
            match (id, bad) {
                (Some(v), _) => {
                    let key = if mutant::active("DevicePerSheet") {
                        format!("{}:{v}", s.name)
                    } else {
                        v.clone()
                    };
                    devices.entry(key).or_default().push((
                        s.name.clone(),
                        doc.entity_address(i),
                        doc.block_name(&doc.entities[i]).unwrap_or("").to_owned(),
                    ));
                    if let Some(p) = doc.entities[i].at {
                        at[si].push(DeviceAt {
                            id: v,
                            at: p,
                            symbol: doc.placed_box(i),
                        });
                    }
                }
                (None, Some((tag, v))) => findings.push(finding(
                    "device_id_malformed",
                    &s.name,
                    Some(doc.entity_address(i)),
                    format!("{tag} holds {v:?}, not a device identifier"),
                )),
                (None, None) => {}
            }
        }
    }

    // boundaries, the cables, and the wire pull list (the cables that cross a boundary)
    let mut cables: Vec<Value> = Vec::new();
    let mut pulls: Vec<Value> = Vec::new();
    let mut ends_seen: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut open_ends: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut junction_ends: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut cable_flags: BTreeMap<&'static str, u64> = BTreeMap::new();
    // each sheet's flags (references of the configured flag blocks): where each sits, its
    // symbol's box as placed, its label (the label tag's value, else the identifier's)
    let flags: Vec<Vec<FlagAt>> = sheets
        .iter()
        .map(|s| {
            let doc = &s.doc;
            let tag = cfg.flag_label_tag.as_deref().or(cfg.flag_id_tag.as_deref());
            (0..doc.entities.len())
                .filter(|&i| {
                    doc.block_name(&doc.entities[i]).is_some_and(|b| {
                        [&cfg.flag_out_blocks, &cfg.flag_in_blocks, &cfg.flag_blocks]
                            .iter()
                            .any(|l| among(l, b))
                    })
                })
                .filter_map(|i| {
                    Some(FlagAt {
                        at: doc.entities[i].at?,
                        symbol: doc.placed_box(i),
                        label: tag
                            .and_then(|t| attr(doc, i, t))
                            .map(|v| v.trim().to_owned())
                            .filter(|v| !v.is_empty()),
                    })
                })
                .collect()
        })
        .collect();
    let mut text = String::new();
    for (si, s) in sheets.iter().enumerate() {
        let doc = &s.doc;
        let (leaders, held) = leaders(cfg, doc);
        // every cable's outline on the sheet, for the junctions
        let runs: Vec<(usize, Vec<[f64; 2]>, bool)> = (0..doc.entities.len())
            .filter_map(|i| run_outline(cfg, doc, i).map(|(p, c, _)| (i, p, c)))
            .collect();
        let mut bounds: Vec<(usize, Vec<[f64; 2]>, String)> = Vec::new();
        for i in 0..doc.entities.len() {
            let e = &doc.entities[i];
            let Some(x) = e.extra.as_ref().filter(|x| x.closed && x.verts.len() >= 3) else {
                continue;
            };
            if !(among(&cfg.boundary_layers, &e.layer)
                || among(&cfg.boundary_ltypes, &ltype_eff(doc, i)))
            {
                continue;
            }
            let name = doc
                .entities
                .iter()
                .find(|t| {
                    t.text.is_some()
                        && t.block.is_none()
                        && t.at.is_some_and(|p| in_polygon(p, &x.verts))
                })
                .and_then(|t| t.text.as_deref())
                .unwrap_or("")
                .to_owned();
            bounds.push((i, x.verts.clone(), name));
        }
        for i in 0..doc.entities.len() {
            let e = &doc.entities[i];
            let Some((pts, closed, block_run)) = run_outline(cfg, doc, i) else {
                continue;
            };
            let crossed: Vec<&str> = if mutant::active("CrossingIgnored") {
                Vec::new()
            } else {
                bounds
                    .iter()
                    .filter(|(b, poly, _)| {
                        *b != i && relate_points(&pts, closed, poly) == Relation::Crosses
                    })
                    .map(|(_, _, n)| n.as_str())
                    .collect()
            };
            if crossed.is_empty() && mutant::active("CablesOnlyPulls") {
                continue;
            }
            let snap = if mutant::active("SnapIgnored") {
                f64::INFINITY
            } else {
                cfg.cable_snap
            };
            // an end meets a device at its insertion point, else at its symbol's edge (its
            // block's drawn box as placed), within the snap distance; the nearest wins
            let end_device = |p: Option<&[f64; 2]>| -> Option<(String, &'static str)> {
                let p = p?;
                let dist = |q: [f64; 2]| ((p[0] - q[0]).powi(2) + (p[1] - q[1]).powi(2)).sqrt();
                let by_point = at[si]
                    .iter()
                    .map(|d| (dist(d.at), &d.id))
                    .filter(|(d, _)| *d <= snap)
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, id)) = by_point {
                    return Some((id.clone(), "at-insertion"));
                }
                if mutant::active("EndsToInsertionOnly") {
                    return None;
                }
                at[si]
                    .iter()
                    .filter_map(|d| {
                        let b = d.symbol?;
                        let dx = (b[0] - p[0]).max(p[0] - b[2]).max(0.0);
                        let dy = (b[1] - p[1]).max(p[1] - b[3]).max(0.0);
                        Some((dx.hypot(dy), &d.id))
                    })
                    .filter(|(d, _)| *d <= snap)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, id)| (id.clone(), "at-symbol"))
            };
            let ends = [pts.first().copied(), pts.last().copied()];
            // an end within the snap distance of another cable (its end or its body) is a
            // junction: the run goes on there
            let joins = |p: &[f64; 2]| {
                runs.iter()
                    .any(|(j, q, c)| *j != i && to_outline(q, *c, *p) <= cfg.cable_snap)
            };
            // an end within the snap distance of a flag (its insertion point or its symbol) is
            // at that flag: an off-sheet end, the flag's label the cable's destination
            let at_flag = |p: &[f64; 2]| -> Option<Option<String>> {
                flags[si]
                    .iter()
                    .map(|f| (near(f.at, f.symbol, p), f))
                    .filter(|(d, _)| *d <= snap)
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, f)| f.label.clone())
            };
            // a device first, then a flag, then another cable; else open
            let met = ends.map(|p| {
                let p = p?;
                if mutant::active("JunctionBeforeDevice") && joins(&p) {
                    return Some(EndAt::Junction);
                }
                if mutant::active("FlagBeforeDevice") {
                    if let Some(l) = at_flag(&p) {
                        return Some(EndAt::Flag(l));
                    }
                }
                if let Some((id, how)) = end_device(Some(&p)) {
                    return Some(EndAt::Device(id, how));
                }
                if let Some(l) = at_flag(&p) {
                    return Some(EndAt::Flag(l));
                }
                Some(if joins(&p) {
                    EndAt::Junction
                } else {
                    EndAt::Open
                })
            });
            // each end: at a device (how), at a flag, a junction, or open; a junction's and an
            // open end's distance to the nearest device, in buckets, and to the nearest flag
            for (end, m) in ends.iter().zip(&met) {
                let (Some(p), Some(m)) = (end, m) else {
                    continue;
                };
                *ends_seen.entry("ends").or_default() += 1;
                let (how, tally) = match m {
                    EndAt::Device(_, how) => (*how, None),
                    EndAt::Flag(_) => ("at-flag", None),
                    EndAt::Junction => ("junction", Some(&mut junction_ends)),
                    EndAt::Open => ("open", Some(&mut open_ends)),
                };
                *ends_seen.entry(how).or_default() += 1;
                let Some(tally) = tally else { continue };
                *tally.entry("ends").or_default() += 1;
                let d = at[si]
                    .iter()
                    .map(|d| near(d.at, d.symbol, p))
                    .fold(f64::INFINITY, f64::min);
                let bucket = OPEN_BUCKETS
                    .iter()
                    .find(|(_, under)| d < *under)
                    .map_or("ge12", |(name, _)| *name);
                let bucket = if at[si].is_empty() {
                    "no-device"
                } else {
                    bucket
                };
                *tally.entry(bucket).or_default() += 1;
                let f = flags[si]
                    .iter()
                    .map(|f| near(f.at, f.symbol, p))
                    .fold(f64::INFINITY, f64::min);
                *tally.entry("near-flag-12").or_default() += u64::from(f <= 12.0);
                *tally.entry("near-flag-24").or_default() += u64::from(f <= 24.0);
            }
            let settled = met
                .each_ref()
                .map(|m| matches!(m, Some(EndAt::Junction | EndAt::Flag(_))));
            let at_flags: Vec<&Option<String>> = met
                .iter()
                .filter_map(|m| match m {
                    Some(EndAt::Flag(l)) => Some(l),
                    _ => None,
                })
                .collect();
            let destination = at_flags.iter().find_map(|l| (*l).clone());
            if !at_flags.is_empty() {
                *cable_flags.entry("cables").or_default() += 1;
                *cable_flags.entry("labelled").or_default() += u64::from(destination.is_some());
            }
            let met = met.map(|m| match m {
                Some(EndAt::Device(id, how)) => Some((id, how)),
                _ => None,
            });
            let [from, to] = met.map(|m| m.map(|(id, _)| id));
            let shape_ignored = mutant::active("LabelShapeIgnored");
            let shaped = |t: &str| shape_ignored || cfg.is_label(t);
            // the leader whose arrow touches the cable labels it; else the nearest text that
            // no leader holds
            let by_leader = leaders
                .iter()
                .filter(|(_, t)| shaped(t))
                .map(|(arrows, t)| {
                    let d = arrows
                        .iter()
                        .map(|a| to_outline(&pts, closed, *a))
                        .fold(f64::INFINITY, f64::min);
                    (d, t)
                })
                .filter(|(d, _)| *d <= cfg.cable_snap)
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(_, t)| t.clone());
            // a run drawn as a block may carry its label as an attribute: it labels itself first
            let by_attrib = cfg
                .cable_label_tags
                .iter()
                .filter(|_| block_run)
                .filter_map(|t| attr(doc, i, t))
                .map(|v| v.trim().to_owned())
                .find(|v| !v.is_empty() && shaped(v));
            let label = by_attrib.or(by_leader).or_else(|| {
                doc.entities
                    .iter()
                    .enumerate()
                    .filter(|(j, t)| {
                        *j != i && t.block.is_none() && t.text.is_some() && !held.contains(j)
                    })
                    .filter_map(|(j, t)| {
                        let d = to_outline(&pts, closed, t.at?);
                        let l = text_of(doc, j);
                        (d <= cfg.label_within && shaped(&l)).then_some((d, l))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, l)| l)
            });
            let address = doc.entity_address(i);
            for (k, (word, end, device)) in [("from", ends[0], &from), ("to", ends[1], &to)]
                .into_iter()
                .enumerate()
            {
                if let (Some(p), None, false) = (end, device, settled[k]) {
                    findings.push(finding(
                        "cable_end_open",
                        &s.name,
                        Some(address.clone()),
                        format!(
                            "its {word} end at ({}, {}) reaches no device within {}",
                            p[0], p[1], cfg.cable_snap
                        ),
                    ));
                }
            }
            if label.is_none() {
                findings.push(finding(
                    "cable_unlabelled",
                    &s.name,
                    Some(address.clone()),
                    format!("no label within {} of it", cfg.label_within),
                ));
            }
            let opt = |x: &Option<String>| x.as_deref().map_or(Value::Null, Value::from);
            let point = |p: Option<[f64; 2]>| {
                p.map_or(Value::Null, |p| {
                    Value::List(
                        p.iter()
                            .map(|c| Value::float(*c).unwrap_or(Value::Null))
                            .collect(),
                    )
                })
            };
            text.push_str(&format!(
                "cable\t{}\t{address}\t{}\t{} -> {}\n",
                s.name,
                label.as_deref().unwrap_or("-"),
                from.as_deref().unwrap_or("?"),
                to.as_deref().unwrap_or("?"),
            ));
            cables.push(Value::obj([
                ("sheet", Value::from(s.name.as_str())),
                ("cable", Value::from(address.as_str())),
                (
                    "block",
                    if block_run {
                        doc.block_name(e).map_or(Value::Null, Value::from)
                    } else {
                        Value::Null
                    },
                ),
                ("label", opt(&label)),
                ("from", opt(&from)),
                ("to", opt(&to)),
                ("destination", opt(&destination)),
                (
                    "attribs",
                    match e.extra.as_ref().filter(|_| block_run) {
                        Some(x) => Value::obj(
                            x.attribs
                                .iter()
                                .map(|a| (a.tag.to_string(), Value::from(&*a.value))),
                        ),
                        None => Value::Null,
                    },
                ),
                (
                    "ends",
                    Value::List(ends.iter().map(|p| point(*p)).collect()),
                ),
                ("boundaries", Value::list(crossed.iter().copied())),
            ]));
            if crossed.is_empty() && !mutant::active("PullsAllCables") {
                continue;
            }
            text.push_str(&format!(
                "pull\t{}\t{address}\t{}\t{} -> {}\t{}\n",
                s.name,
                label.as_deref().unwrap_or("-"),
                from.as_deref().unwrap_or("?"),
                to.as_deref().unwrap_or("?"),
                crossed.join(", ")
            ));
            pulls.push(Value::obj([
                ("sheet", Value::from(s.name.as_str())),
                ("cable", Value::from(address)),
                ("label", opt(&label)),
                ("from", opt(&from)),
                ("to", opt(&to)),
                ("boundaries", Value::list(crossed)),
            ]));
        }
    }

    // flags, paired by identifier across the set
    let mut outs: BTreeMap<String, Vec<Flag>> = BTreeMap::new();
    let mut ins: BTreeMap<String, Vec<Flag>> = BTreeMap::new();
    // flags of one kind, in the order they come
    let mut ones: Vec<(String, Flag)> = Vec::new();
    // flags of the configured blocks, and those with the configured tag; each tag's values
    let mut flag_refs = (0u64, 0u64);
    let mut flag_others: Vec<String> = Vec::new();
    let mut flag_tags: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    // flags as labelled ends: each names where it leads, none pairs
    let mut flag_ends: BTreeMap<&'static str, u64> = BTreeMap::new();
    let mut flag_end_rows: Vec<Value> = Vec::new();
    if let Some(label_tag) = cfg.flag_label_tag.as_deref() {
        let lt = if mutant::active("FlagLabelFromKind") {
            cfg.flag_kind_tag.as_deref().unwrap_or(label_tag)
        } else {
            label_tag
        };
        let sheet_words: Vec<String> = index
            .iter()
            .flat_map(|r| [r.number.clone(), r.name.clone()])
            .flatten()
            .map(|w| w.trim().to_uppercase())
            .collect();
        let mut kinds: Vec<String> = Vec::new();
        for s in sheets {
            let doc = &s.doc;
            for i in 0..doc.entities.len() {
                let Some(block) = doc.block_name(&doc.entities[i]) else {
                    continue;
                };
                if ![&cfg.flag_out_blocks, &cfg.flag_in_blocks, &cfg.flag_blocks]
                    .iter()
                    .any(|l| among(l, block))
                {
                    continue;
                }
                let value = |t: Option<&str>| {
                    t.and_then(|t| attr(doc, i, t))
                        .map(|v| v.trim().to_owned())
                        .filter(|v| !v.is_empty())
                };
                let (label, kind) = (value(Some(lt)), value(cfg.flag_kind_tag.as_deref()));
                *flag_ends.entry("ends").or_default() += 1;
                match &label {
                    Some(l) => {
                        *flag_ends.entry("labelled").or_default() += 1;
                        *flag_ends.entry("label-is-device").or_default() +=
                            u64::from(devices.contains_key(l));
                        *flag_ends.entry("label-is-sheet").or_default() +=
                            u64::from(sheet_words.contains(&l.to_uppercase()));
                    }
                    None => *flag_ends.entry("unlabelled").or_default() += 1,
                }
                if let Some(k) = &kind {
                    if !kinds.contains(k) {
                        kinds.push(k.clone());
                    }
                }
                let opt = |x: &Option<String>| x.as_deref().map_or(Value::Null, Value::from);
                flag_end_rows.push(Value::obj([
                    ("sheet", Value::from(s.name.as_str())),
                    ("flag", Value::from(doc.entity_address(i).as_str())),
                    ("kind", opt(&kind)),
                    ("label", opt(&label)),
                ]));
            }
        }
        flag_ends.insert("kinds", kinds.len() as u64);
    }
    if let Some(id_tag) = cfg
        .flag_id_tag
        .as_deref()
        .filter(|_| cfg.flag_label_tag.is_none())
    {
        for s in sheets {
            let doc = &s.doc;
            for i in 0..doc.entities.len() {
                let Some(block) = doc.block_name(&doc.entities[i]) else {
                    continue;
                };
                let side = if among(&cfg.flag_out_blocks, block) {
                    Some(&mut outs)
                } else if among(&cfg.flag_in_blocks, block) {
                    Some(&mut ins)
                } else if among(&cfg.flag_blocks, block) {
                    None
                } else {
                    continue;
                };
                // every tag of every flag, in counts: the configured one and the others in the
                // order first seen
                flag_refs.0 += 1;
                for a in doc.entities[i].extra.iter().flat_map(|x| x.attribs.iter()) {
                    let tag = a.tag.to_ascii_uppercase();
                    let key = if tag.eq_ignore_ascii_case(id_tag) {
                        flag_refs.1 += 1;
                        "id".to_owned()
                    } else {
                        let k = match flag_others.iter().position(|t| *t == tag) {
                            Some(k) => k,
                            None => {
                                flag_others.push(tag);
                                flag_others.len() - 1
                            }
                        };
                        format!("other-{}", k + 1)
                    };
                    let v = a.value.trim().to_owned();
                    *flag_tags.entry(key).or_default().entry(v).or_default() += 1;
                }
                // an empty value is no identifier
                let Some(id) = attr(doc, i, id_tag).filter(|v| !v.trim().is_empty()) else {
                    findings.push(finding(
                        "flag_without_id",
                        &s.name,
                        Some(doc.entity_address(i)),
                        format!("a flag with no {id_tag}, or an empty one"),
                    ));
                    continue;
                };
                let fl = Flag {
                    sheet: s.name.clone(),
                    address: doc.entity_address(i),
                    target: cfg
                        .flag_sheet_tag
                        .as_deref()
                        .and_then(|t| attr(doc, i, t))
                        .map(|v| v.trim().to_owned()),
                };
                let id = id.trim().to_owned();
                match side {
                    Some(side) => side.entry(id).or_default().push(fl),
                    None => ones.push((id, fl)),
                }
            }
        }
    }
    let mut pairs: Vec<Value> = Vec::new();
    let flag_value = |fl: &Flag| {
        Value::obj([
            ("sheet", Value::from(fl.sheet.as_str())),
            ("flag", Value::from(fl.address.as_str())),
        ])
    };
    if mutant::active("FlagByOrder") {
        // the planted bug: flags paired in the order they come, whatever their identifiers
        let o: Vec<&Flag> = outs.values().flatten().collect();
        let n: Vec<&Flag> = ins.values().flatten().collect();
        for (a, b) in o.iter().zip(&n) {
            pairs.push(Value::obj([("out", flag_value(a)), ("in", flag_value(b))]));
        }
    } else {
        let ids: Vec<String> = outs.keys().chain(ins.keys()).cloned().collect();
        let mut done: Vec<String> = Vec::new();
        for id in ids {
            if done.contains(&id) {
                continue;
            }
            done.push(id.clone());
            let (o, n) = (outs.get(&id), ins.get(&id));
            match (o.map_or(0, Vec::len), n.map_or(0, Vec::len)) {
                (1, 1) => {
                    let (a, b) = (&o.expect("one")[0], &n.expect("one")[0]);
                    let target_sheet_number = number_of.get(&b.sheet).cloned().flatten();
                    if let (Some(t), Some(have)) = (&a.target, &target_sheet_number) {
                        if t != have {
                            findings.push(finding(
                                "flag_target_mismatch",
                                &a.sheet,
                                Some(a.address.clone()),
                                format!(
                                    "flag {id} points to sheet {t}; its partner is on sheet {have}"
                                ),
                            ));
                        }
                    }
                    text.push_str(&format!(
                        "flag\t{id}\t{} {} -> {} {}\n",
                        a.sheet, a.address, b.sheet, b.address
                    ));
                    pairs.push(Value::obj([
                        ("id", Value::from(id.as_str())),
                        ("directed", Value::from(true)),
                        ("out", flag_value(a)),
                        ("in", flag_value(b)),
                    ]));
                }
                (no, ni) => {
                    for fl in o.into_iter().flatten().chain(n.into_iter().flatten()) {
                        let (code, message) = if no > 1 || ni > 1 {
                            (
                                "flag_reused",
                                format!("flag {id} is used {no} times out and {ni} in"),
                            )
                        } else if no == 0 {
                            (
                                "flag_unpaired",
                                format!("incoming flag {id} has no outgoing partner"),
                            )
                        } else {
                            (
                                "flag_unpaired",
                                format!("outgoing flag {id} has no incoming partner"),
                            )
                        };
                        findings.push(finding(code, &fl.sheet, Some(fl.address.clone()), message));
                    }
                }
            }
        }
    }

    // flags of one kind: the two with the same identifier are a pair, with no direction
    let mut by_id: BTreeMap<&str, Vec<&Flag>> = BTreeMap::new();
    if mutant::active("OneKindByOrder") {
        // the planted bug: one-kind flags paired two by two in the order they come
        for two in ones.chunks(2) {
            by_id.insert(&two[0].0, two.iter().map(|(_, f)| f).collect());
        }
    } else {
        for (id, fl) in &ones {
            by_id.entry(id.as_str()).or_default().push(fl);
        }
    }
    for (id, fls) in &by_id {
        if let [a, b] = fls.as_slice() {
            for (x, y) in [(a, b), (b, a)] {
                let have = number_of.get(&y.sheet).cloned().flatten();
                if let (Some(t), Some(have)) = (&x.target, &have) {
                    if t != have {
                        findings.push(finding(
                            "flag_target_mismatch",
                            &x.sheet,
                            Some(x.address.clone()),
                            format!(
                                "flag {id} points to sheet {t}; its partner is on sheet {have}"
                            ),
                        ));
                    }
                }
            }
            text.push_str(&format!(
                "flag\t{id}\t{} {} -- {} {}\n",
                a.sheet, a.address, b.sheet, b.address
            ));
            pairs.push(Value::obj([
                ("id", Value::from(*id)),
                ("directed", Value::from(false)),
                ("a", flag_value(a)),
                ("b", flag_value(b)),
            ]));
            continue;
        }
        for fl in fls {
            let (code, message) = if fls.len() > 2 {
                (
                    "flag_reused",
                    format!("flag {id} is used {} times", fls.len()),
                )
            } else {
                ("flag_unpaired", format!("flag {id} has no partner"))
            };
            findings.push(finding(code, &fl.sheet, Some(fl.address.clone()), message));
        }
    }

    references.sort();
    for (sheet, by) in &references {
        text.push_str(&format!("reference\t{sheet}\t{}\n", by.join(", ")));
    }
    for r in &index {
        text.push_str(&format!(
            "sheet\t{}\t{}\t{}\trev {}\n",
            r.number.as_deref().unwrap_or("-"),
            r.sheet,
            r.name.as_deref().unwrap_or("-"),
            r.revision.as_deref().unwrap_or("-")
        ));
    }
    for (id, places) in &devices {
        let sheets_of: Vec<&str> = {
            let mut v: Vec<&str> = places.iter().map(|p| p.0.as_str()).collect();
            v.dedup();
            v
        };
        text.push_str(&format!("device\t{id}\t{}\n", sheets_of.join(", ")));
    }
    let device_rows: Vec<Value> = devices
        .iter()
        .map(|(id, places)| {
            let mut sheets_of: Vec<&str> = places.iter().map(|p| p.0.as_str()).collect();
            sheets_of.dedup();
            Value::obj([
                ("id", Value::from(id.as_str())),
                ("sheets", Value::list(sheets_of)),
                (
                    "places",
                    Value::List(
                        places
                            .iter()
                            .map(|(sh, a, b)| {
                                Value::obj([
                                    ("sheet", Value::from(sh.as_str())),
                                    ("insert", Value::from(a.as_str())),
                                    ("block", Value::from(b.as_str())),
                                ])
                            })
                            .collect(),
                    ),
                ),
            ])
        })
        .collect();
    let sheet_rows: Vec<Value> = index
        .iter()
        .map(|r| {
            let opt = |x: &Option<String>| x.as_deref().map_or(Value::Null, Value::from);
            Value::obj([
                ("sheet", Value::from(r.sheet.as_str())),
                ("number", opt(&r.number)),
                ("name", opt(&r.name)),
                ("revision", opt(&r.revision)),
                ("revision_date", opt(&r.date)),
                ("title_from", opt(&r.from)),
            ])
        })
        .collect();
    ReadOut {
        stdout: text,
        data: Value::obj([
            ("sheets", Value::List(sheet_rows)),
            ("flag_ends", Value::List(flag_end_rows)),
            (
                "diagnostics",
                diagnostics(&Counts {
                    chain: &chain,
                    number_by: &number_by,
                    hints: &hints,
                    rows: rows_sum,
                    by_tag: &by_tag,
                    flag_refs,
                    flag_tags: &flag_tags,
                    flag_ends: &flag_ends,
                    ends: &ends_seen,
                    open: &open_ends,
                    junctions: &junction_ends,
                    cable_flags: &cable_flags,
                }),
            ),
            (
                "references",
                Value::List(
                    references
                        .iter()
                        .map(|(sheet, by)| {
                            Value::obj([
                                ("sheet", Value::from(sheet.as_str())),
                                ("referenced_by", Value::list(by.iter().map(String::as_str))),
                            ])
                        })
                        .collect(),
                ),
            ),
            ("devices", Value::List(device_rows)),
            ("cables", Value::List(cables)),
            ("pulls", Value::List(pulls)),
            ("flags", Value::List(pairs)),
            ("findings", Value::List(findings)),
        ]),
        spec: ("av.set", 1),
        next: None,
    }
}

/// The set of `files` (each a name and its bytes, DXF), read with the configuration `config`.
pub fn read_files(config: &str, files: &[(String, Vec<u8>)]) -> Result<ReadOut, CadError> {
    let cfg = Config::parse(config)?;
    let mut sheets = Vec::with_capacity(files.len());
    for (name, bytes) in files {
        let doc = k2_mpc_dxf::read_doc(bytes, &k2_mpc_dxf::Limits::default()).map_err(|e| {
            CadError::new(
                SHEET_UNREADABLE,
                format!("{name}: {}", e.summary()),
                "`check` on that sheet names what doesn't read",
            )
        })?;
        sheets.push(Sheet {
            name: name.clone(),
            doc,
        });
    }
    Ok(read_set(&cfg, &sheets))
}

/// The set's reading in counts and finding codes only, never a value (no identifier, label,
/// sheet number or file name): what a person can pass on about a private set. One fact per
/// line: `sheets`, `references`, `devices`, `cables`, `pulls`, `flags`, then `finding <code>
/// <count>` for each finding code.
pub fn summary(data: &Value) -> String {
    let list = |k: &str| match data.get(k) {
        Some(Value::List(l)) => l.as_slice(),
        _ => &[],
    };
    let has = |v: &Value, k: &str| !matches!(v.get(k), None | Some(Value::Null));
    let n_list = |v: &Value, k: &str| match v.get(k) {
        Some(Value::List(l)) => l.len(),
        _ => 0,
    };
    let sheets = list("sheets");
    let mut out = format!(
        "sheets {} with-number {} with-name {} with-revision {} with-revision-date {} title-by-reference {}\n",
        sheets.len(),
        sheets.iter().filter(|s| has(s, "number")).count(),
        sheets.iter().filter(|s| has(s, "name")).count(),
        sheets.iter().filter(|s| has(s, "revision")).count(),
        sheets.iter().filter(|s| has(s, "revision_date")).count(),
        sheets.iter().filter(|s| has(s, "title_from")).count(),
    );
    out.push_str(&format!("references {}\n", list("references").len()));
    let devices = list("devices");
    out.push_str(&format!(
        "devices {} on-several-sheets {}\n",
        devices.len(),
        devices.iter().filter(|d| n_list(d, "sheets") > 1).count()
    ));
    let cables = list("cables");
    out.push_str(&format!(
        "cables {} drawn-as-blocks {} labelled {} both-ends-at-devices {}\n",
        cables.len(),
        cables.iter().filter(|c| has(c, "block")).count(),
        cables.iter().filter(|c| has(c, "label")).count(),
        cables
            .iter()
            .filter(|c| has(c, "from") && has(c, "to"))
            .count()
    ));
    out.push_str(&format!("pulls {}\n", list("pulls").len()));
    let flags = list("flags");
    out.push_str(&format!(
        "flags {} directed {} one-kind {}\n",
        flags.len(),
        flags
            .iter()
            .filter(|f| f.get("directed") == Some(&Value::Bool(true)))
            .count(),
        flags
            .iter()
            .filter(|f| f.get("directed") == Some(&Value::Bool(false)))
            .count()
    ));
    // the diagnostics: counts only
    let dg = data.get("diagnostics");
    let num = |v: Option<&Value>, k: &str| {
        v.and_then(|v| v.get(k))
            .map_or("0".to_owned(), Value::to_json)
    };
    let t = dg.and_then(|d| d.get("title"));
    out.push_str(&format!(
        "title-chain sheets {} own {} by-reference {} copy-in-reference {} placing-a-resolved-reference {} with-revision-rows {}\n",
        num(t, "sheets"),
        num(t, "own"),
        num(t, "by-reference"),
        num(t, "copy-in-reference"),
        num(t, "placing"),
        num(t, "revision")
    ));
    let rows = |k: &str| match dg.and_then(|d| d.get(k)) {
        Some(Value::List(l)) => l.as_slice(),
        _ => &[],
    };
    for (k, m) in rows("device_tags").iter().enumerate() {
        let parts: Vec<String> = [
            "values",
            "empty",
            "id-shaped",
            "lowercase",
            "separator",
            "no-separator",
            "suffix",
            "out-of-range",
            "other",
        ]
        .iter()
        .map(|c| format!("{c} {}", num(Some(m), c)))
        .collect();
        out.push_str(&format!("device-tag {} {}\n", k + 1, parts.join(" ")));
    }
    let f = dg.and_then(|d| d.get("flag_refs"));
    out.push_str(&format!(
        "flag-refs {} with-id-tag {}\n",
        num(f, "refs"),
        num(f, "with_id_tag")
    ));
    for m in rows("flag_tags") {
        out.push_str(&format!(
            "flag-tag {} values {} empty {} distinct {} paired {} single {} more {} id-shaped {}\n",
            m.get("tag").and_then(Value::as_str).unwrap_or("?"),
            num(Some(m), "values"),
            num(Some(m), "empty"),
            num(Some(m), "distinct"),
            num(Some(m), "paired"),
            num(Some(m), "single"),
            num(Some(m), "more"),
            num(Some(m), "id_shaped")
        ));
    }
    let fe = dg.and_then(|d| d.get("flag_ends"));
    out.push_str(&format!(
        "flag-ends {} labelled {} unlabelled {} label-is-device {} label-is-sheet {} kinds {}\n",
        num(fe, "ends"),
        num(fe, "labelled"),
        num(fe, "unlabelled"),
        num(fe, "label-is-device"),
        num(fe, "label-is-sheet"),
        num(fe, "kinds")
    ));
    for (key, head) in [
        ("open_ends", "open-ends"),
        ("junction_ends", "junction-ends"),
    ] {
        let oe = dg.and_then(|d| d.get(key));
        let parts: Vec<String> = DISTANCES[1..]
            .iter()
            .map(|k| format!("{k} {}", num(oe, k)))
            .collect();
        out.push_str(&format!("{head} {} {}\n", num(oe, "ends"), parts.join(" ")));
    }
    let rw = dg.and_then(|d| d.get("rows"));
    out.push_str(&format!(
        "revision-rows sheets-with-rows {} rev-tags {} rev-filled {} date-tags {} date-filled {} other-numbered-tags {}\n",
        num(rw, "sheets"),
        num(rw, "rev-tags"),
        num(rw, "rev-filled"),
        num(rw, "date-tags"),
        num(rw, "date-filled"),
        num(rw, "other-numbered")
    ));
    let nb = dg.and_then(|d| d.get("numbers"));
    let mut by: Vec<String> = match nb {
        Some(Value::Obj(kv)) => kv
            .iter()
            .filter(|(k, _)| k.starts_with("tag-"))
            .map(|(k, v)| format!("{k} {}", v.to_json()))
            .collect(),
        _ => Vec::new(),
    };
    for k in ["layout", "file-stem", "none"] {
        by.push(format!("{k} {}", num(nb, k)));
    }
    out.push_str(&format!("number-source {}\n", by.join(" ")));
    out.push_str(&format!(
        "layout-names sheet-like {} not {} none {}\n",
        num(nb, "layout-yes"),
        num(nb, "layout-not"),
        num(nb, "layout-none")
    ));
    out.push_str(&format!(
        "file-stems sheet-like {} not {}\n",
        num(nb, "stem-yes"),
        num(nb, "stem-not")
    ));
    let c = dg.and_then(|d| d.get("cable_ends"));
    out.push_str(&format!(
        "cable-ends {} at-insertion {} at-symbol {} at-flag {} junction {} open {}\n",
        num(c, "ends"),
        num(c, "at-insertion"),
        num(c, "at-symbol"),
        num(c, "at-flag"),
        num(c, "junction"),
        num(c, "open")
    ));
    let cf = dg.and_then(|d| d.get("cable_flags"));
    out.push_str(&format!(
        "cable-flags cables-at-a-flag {} labelled-by-flag {}\n",
        num(cf, "cables"),
        num(cf, "labelled")
    ));
    let mut codes: BTreeMap<&str, u64> = BTreeMap::new();
    for f in list("findings") {
        *codes
            .entry(f.get("code").and_then(Value::as_str).unwrap_or("?"))
            .or_default() += 1;
    }
    for (code, n) in codes {
        out.push_str(&format!("finding {code} {n}\n"));
    }
    out
}
