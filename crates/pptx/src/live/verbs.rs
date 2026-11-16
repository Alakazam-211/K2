//! The agent's verbs on a live deck, as the kit's structured verbs: each computes ops by
//! key on the live store under the session lock, and the kit applies them as one change with
//! its guards (an agent's edit needs `--base` or `--if-version`; a facet written since the
//! base is refused, by name).
//!
//! | Verb | Ops |
//! |---|---|
//! | `slide list [--after @sld:ID] [--limit N]` | read: one line per slide, paged |
//! | `slide show @sld:ID [--after @sld:ID/sp:N] [--limit N]` | read: the slide, then its shapes, at most 64 a page, text cut at 1,024 bytes |
//! | `shape show @sld:ID/sp:N` | read: every field, text up to 16,384 bytes |
//! | `slide add --layout NAME\|@layout:ID [--after @sld:ID \| --first]` | insert in `slides`, the layout's placeholders in `shapes`, their fields |
//! | `slide move @sld:ID (--after @sld:ID \| --first)` | one move in `slides` |
//! | `slide delete @sld:ID` | one delete in `slides` |
//! | `slide hide @sld:ID [--show]` | set `hidden` |
//! | `shape add @sld:ID --kind textbox --at X,Y,W,H --text T` | insert after the slide's last shape; its fields |
//! | `shape text @sld:ID/sp:N --text T [--drop-formatting]` | set `text` |
//! | `shape move @sld:ID/sp:N [--x X] [--y Y] [--w W] [--h H]` | set `geom` (one field) |
//! | `shape order @sld:ID/sp:N (--front \| --back \| --after @sld:ID/sp:M)` | one move in `shapes`, within its slide |
//! | `shape delete @sld:ID/sp:N` | one delete in `shapes` |
//! | `notes set @sld:ID --text T` | set `notes` |
//!
//! **New ids are fixed inside the change**: a shape's `cNvPr` one past the higher of the
//! slide's id mark (its whole tree at import, a group's children included) and every id the
//! store holds for that slide, live or deleted; a slide's `sldId` one past the higher of the
//! deck's mark and every slide id the store holds. The kit serialises changes under its lock,
//! so two adds never take one id.
//!
//! **Refusals are typed:** each prints `refused: <code>: <why>` and a `next:` line naming a
//! verb, and exits 2; nothing applies. A missing slide or shape exits 4.

use std::collections::BTreeMap;

use k2_mpc_base::doc::Edit;
use k2_mpc_base::elem::{Anchor, Key, ListOp, MapOp, Value};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::session::View;
use k2_mpc_base::tools::exit::{NOT_FOUND, OK, USAGE};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, VerbCtx};

use super::model::{
    f, fid_mark_key, parse_key, prop, shape_key, slide_key, Geom, Ph, TextEdit, DECK, SHAPES,
    SHAPE_PROPS, SLD_MARK, SLIDES, SLIDE_PROPS, TEXT_MAX,
};
use super::store_max_fid;
use crate::deck::{SLIDE_ID_MAX, SLIDE_ID_MIN};
use crate::mutant;
use crate::verbs::{parse_address, COORD_MAX};
use crate::xmlx::{cut, escape_text};

/// The most shapes one `slide show` page holds.
pub const SHAPES_PER_PAGE: usize = 64;
/// Text per shape in `slide show`, in bytes.
pub const SHOW_TEXT_MAX: usize = 1024;
/// Text in `shape show`, in bytes (the kit's smallest read budget).
pub const SHAPE_TEXT_MAX: usize = 16_384;
/// Slides per `slide list` page: the default and the most.
pub const LIST_DEFAULT: usize = 100;
pub const LIST_MAX: usize = 500;
/// A title in `slide list`, in bytes.
pub const TITLE_CUT: usize = 128;

const EXITS: &[u8] = &[OK, USAGE, NOT_FOUND];

/// The verbs, as the kit routes them. The kit's parity registry (`Registry::for_pack`) takes
/// them under the keyed tier (`Tier::Keyed`); `serves` names the nearest person actions.
pub const VERBS: &[PackVerb] = &[
    PackVerb::structured_read(
        "slide list",
        "[--after @sld:ID] [--limit N] (each slide: address, position, layout, title, hidden, shapes; at most 500 a page)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_slide_list,
    ),
    PackVerb::structured_read(
        "slide show",
        "@sld:ID [--after @sld:ID/sp:N] [--limit N] (the slide and its shapes back to front, at most 64 a page, text cut at 1,024 bytes)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_slide_show,
    ),
    PackVerb::structured_read(
        "shape show",
        "@sld:ID/sp:N (every field; text up to 16,384 bytes)",
        &[PersonAction::ViewRange],
        EXITS,
        parse_shape_show,
    ),
    PackVerb::structured_edit(
        "slide add",
        "--layout NAME|@layout:ID [--after @sld:ID | --first] (the layout's placeholders, empty; default: last)",
        &[PersonAction::RowInsert],
        EXITS,
        parse_slide_add,
    ),
    PackVerb::structured_edit(
        "slide move",
        "@sld:ID (--after @sld:ID | --first) (a deck with sections: exit 2)",
        &[PersonAction::RowMove],
        EXITS,
        parse_slide_move,
    ),
    PackVerb::structured_edit(
        "slide delete",
        "@sld:ID (a slide another links to, or a deck with sections: exit 2)",
        &[PersonAction::RowDelete],
        EXITS,
        parse_slide_delete,
    ),
    PackVerb::structured_edit(
        "slide hide",
        "@sld:ID [--show]",
        &[PersonAction::Format],
        EXITS,
        parse_slide_hide,
    ),
    PackVerb::structured_edit(
        "shape add",
        "@sld:ID --kind textbox --at X,Y,W,H (EMU) --text TEXT (in front of the others)",
        &[PersonAction::RowInsert],
        EXITS,
        parse_shape_add,
    ),
    PackVerb::structured_edit(
        "shape text",
        "@sld:ID/sp:N --text TEXT [--drop-formatting] (each line a paragraph; a shape with more than one format needs --drop-formatting)",
        &[PersonAction::TypeOver],
        EXITS,
        parse_shape_text,
    ),
    PackVerb::structured_edit(
        "shape move",
        "@sld:ID/sp:N [--x X] [--y Y] [--w W] [--h H] (EMU; a shape placed by its layout needs all four)",
        &[PersonAction::RowMove],
        EXITS,
        parse_shape_move,
    ),
    PackVerb::structured_edit(
        "shape order",
        "@sld:ID/sp:N (--front | --back | --after @sld:ID/sp:M) (within its slide)",
        &[PersonAction::RowMove],
        EXITS,
        parse_shape_order,
    ),
    PackVerb::structured_edit(
        "shape delete",
        "@sld:ID/sp:N (an animated or connected shape: exit 2)",
        &[PersonAction::RowDelete],
        EXITS,
        parse_shape_delete,
    ),
    PackVerb::structured_edit(
        "notes set",
        "@sld:ID --text TEXT (makes the notes slide from the notes master when there is none)",
        &[PersonAction::TypeOver],
        EXITS,
        parse_notes_set,
    ),
];

// ---------- the doc, as a verb reads it ----------

struct Doc<'v> {
    slides: &'v [(Key, Value)],
    shapes: &'v [(Key, Value)],
    sp: &'v BTreeMap<String, Value>,
    hp: &'v BTreeMap<String, Value>,
    deck: &'v BTreeMap<String, Value>,
    view: &'v View,
}

static EMPTY: BTreeMap<String, Value> = BTreeMap::new();

impl<'v> Doc<'v> {
    fn of(view: &'v View) -> Doc<'v> {
        Doc {
            slides: view.list(SLIDES).unwrap_or(&[]),
            shapes: view.list(SHAPES).unwrap_or(&[]),
            sp: view.map(SLIDE_PROPS).unwrap_or(&EMPTY),
            hp: view.map(SHAPE_PROPS).unwrap_or(&EMPTY),
            deck: view.map(DECK).unwrap_or(&EMPTY),
            view,
        }
    }

    /// The slide's position (from 0), when it is live.
    fn position(&self, sid: u32) -> Option<usize> {
        let k = slide_key(sid);
        self.slides.iter().position(|(x, _)| *x == k)
    }

    fn slide_field(&self, sid: u32, fld: &str) -> Option<&'v str> {
        self.sp
            .get(&prop(&slide_key(sid), fld))
            .and_then(Value::as_str)
    }

    fn shape_field(&self, sid: u32, fid: u32, fld: &str) -> Option<&'v str> {
        self.hp
            .get(&prop(&shape_key(sid, fid), fld))
            .and_then(Value::as_str)
    }

    fn deck(&self, k: &str) -> Option<&'v str> {
        self.deck.get(k).and_then(Value::as_str)
    }

    /// The live shapes of a slide, back to front: `(fid, kind)`.
    fn shapes_of(&self, sid: u32) -> Vec<(u32, &'v str)> {
        self.shapes
            .iter()
            .filter_map(|(k, v)| match parse_key(k.as_str()) {
                Some((s, Some(f))) if s == sid => Some((f, v.as_str().unwrap_or(""))),
                _ => None,
            })
            .collect()
    }

    fn shape_live(&self, sid: u32, fid: u32) -> bool {
        let k = shape_key(sid, fid);
        self.position(sid).is_some() && self.shapes.iter().any(|(x, _)| *x == k)
    }

    /// The layouts: `(id, name)`.
    fn layouts(&self) -> Vec<(u64, &'v str)> {
        self.deck
            .iter()
            .filter_map(|(k, v)| {
                let id = k.strip_prefix("layout/")?.strip_suffix("/name")?;
                Some((id.parse().ok()?, v.as_str().unwrap_or("")))
            })
            .collect()
    }

    fn layout_name(&self, id: &str) -> Option<&'v str> {
        let id = id.strip_prefix("@layout:")?;
        self.deck(&format!("layout/{id}/name"))
    }

    /// The title placeholder's text of a slide.
    fn title(&self, sid: u32) -> &'v str {
        for (fid, _) in self.shapes_of(sid) {
            let ph = self
                .shape_field(sid, fid, f::PH)
                .and_then(Ph::parse)
                .map(|p| p.kind().to_owned());
            if matches!(ph.as_deref(), Some("title" | "ctrTitle")) {
                return self.shape_field(sid, fid, f::TEXT).unwrap_or("");
            }
        }
        ""
    }

    /// A new shape id for slide `sid`.
    fn next_fid(&self, sid: u32) -> Option<u32> {
        let mark: u32 = self
            .deck(&fid_mark_key(sid))
            .and_then(|m| m.parse().ok())
            .unwrap_or(0);
        let store = store_max_fid(self.view, sid);
        let base = if mutant::active("FidReused") {
            store
        } else {
            mark.max(store)
        };
        base.max(1).checked_add(1)
    }

    /// A new slide id.
    fn next_sid(&self) -> Option<u32> {
        let mark: u32 = self
            .deck(SLD_MARK)
            .and_then(|m| m.parse().ok())
            .unwrap_or(0);
        let live = self.slides.iter().map(|(k, _)| k);
        let gone = self.view.hidden(SLIDES).into_iter().flat_map(|h| h.keys());
        let store = live
            .chain(gone)
            .filter_map(|k| match parse_key(k.as_str()) {
                Some((s, None)) => Some(s),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        let id = mark.max(store).checked_add(1)?.max(SLIDE_ID_MIN);
        (id <= SLIDE_ID_MAX).then_some(id)
    }
}

// ---------- arguments and outputs ----------

type R = Result<StructOutput, ToolError>;

fn usage(m: impl Into<String>) -> ToolError {
    ToolError::usage(m)
}

/// A typed refusal: nothing applies, exit 2.
fn refuse(code: &str, why: &str, next: &str) -> R {
    Ok(StructOutput::new(
        Vec::new(),
        format!("refused: {code}: {why}\nnext: {next}\n").into_bytes(),
        USAGE,
    ))
}

fn not_found(addr: &str) -> R {
    Ok(StructOutput::new(
        Vec::new(),
        format!("not found: {addr}\nnext: slide list\n").into_bytes(),
        NOT_FOUND,
    ))
}

fn done(edits: Vec<Edit>, out: String) -> R {
    Ok(StructOutput::new(edits, out.into_bytes(), OK))
}

/// Flags and positionals, parsed: `--name value`, `--switch`, and bare words.
struct Args {
    pos: Vec<String>,
    vals: BTreeMap<String, String>,
    switches: Vec<String>,
}

fn parse_args(args: &[String], with_value: &[&str], switches: &[&str]) -> Result<Args, ToolError> {
    let mut a = Args {
        pos: Vec::new(),
        vals: BTreeMap::new(),
        switches: Vec::new(),
    };
    let mut i = 0;
    while i < args.len() {
        let x = &args[i];
        if with_value.contains(&x.as_str()) {
            let v = args
                .get(i + 1)
                .ok_or_else(|| usage(format!("{x} needs a value")))?;
            if a.vals.insert(x.clone(), v.clone()).is_some() {
                return Err(usage(format!("{x} given twice")));
            }
            i += 2;
        } else if switches.contains(&x.as_str()) {
            a.switches.push(x.clone());
            i += 1;
        } else if x.starts_with("--") {
            return Err(usage(format!("unknown flag {x}")));
        } else {
            a.pos.push(x.clone());
            i += 1;
        }
    }
    Ok(a)
}

impl Args {
    fn one(&self, what: &str) -> Result<&str, ToolError> {
        match self.pos.as_slice() {
            [one] => Ok(one),
            _ => Err(usage(format!("give one {what}"))),
        }
    }
    fn has(&self, s: &str) -> bool {
        self.switches.iter().any(|x| x == s)
    }
    fn val(&self, k: &str) -> Option<&str> {
        self.vals.get(k).map(String::as_str)
    }
}

fn slide_addr(s: &str) -> Result<u32, ToolError> {
    match parse_address(s).map_err(|e| usage(e.to_string()))? {
        (sid, None) => Ok(sid),
        _ => Err(usage(format!("{s:?} isn't a slide (@sld:ID)"))),
    }
}

fn shape_addr(s: &str) -> Result<(u32, u32), ToolError> {
    match parse_address(s).map_err(|e| usage(e.to_string()))? {
        (sid, Some(fid)) => Ok((sid, fid)),
        _ => Err(usage(format!("{s:?} isn't a shape (@sld:ID/sp:N)"))),
    }
}

fn limit(a: &Args, default: usize, max: usize) -> Result<usize, ToolError> {
    match a.val("--limit") {
        None => Ok(default),
        Some(v) => match v.parse::<usize>() {
            Ok(n) if (1..=max).contains(&n) => Ok(n),
            _ => Err(usage(format!("--limit is 1 to {max}"))),
        },
    }
}

/// One line of text: `\`, newline and tab escaped.
fn one_line(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\t', "\\t")
}

/// Text an agent gives: line ends made `\n`, within the field limit, nothing a deck can't
/// carry.
fn text_in(t: &str) -> Result<String, ToolError> {
    let t = t.replace("\r\n", "\n").replace('\r', "\n");
    if t.len() > TEXT_MAX {
        return Err(usage(format!(
            "the text is {} bytes, over the live limit of {TEXT_MAX}",
            t.len()
        )));
    }
    if escape_text(&t).is_none() {
        return Err(usage("the text holds a character a deck can't carry"));
    }
    Ok(t)
}

fn list(root: &str, ops: Vec<ListOp>) -> Edit {
    Edit::List {
        root: root.to_owned(),
        ops,
    }
}

fn map(root: &str, ops: Vec<MapOp>) -> Edit {
    Edit::Map {
        root: root.to_owned(),
        ops,
    }
}

fn set(key: String, v: impl Into<String>) -> MapOp {
    MapOp::Set {
        key,
        value: Value::str(v.into()),
    }
}

fn check_coord(v: &str, what: &str, size: bool) -> Result<i64, ToolError> {
    let n: i64 = v
        .trim()
        .parse()
        .map_err(|_| usage(format!("{what} isn't a number")))?;
    let ok = if size {
        (0..=COORD_MAX).contains(&n)
    } else {
        n.abs() <= COORD_MAX
    };
    if ok {
        Ok(n)
    } else {
        Err(usage(format!("{what} is out of range")))
    }
}

// ---------- reads ----------

fn parse_slide_list(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--after", "--limit"], &[])?;
    if !a.pos.is_empty() {
        return Err(usage("slide list takes no address"));
    }
    let after = a.val("--after").map(slide_addr).transpose()?;
    let n = limit(&a, LIST_DEFAULT, LIST_MAX)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        let start = match after {
            None => 0,
            Some(s) => match d.position(s) {
                Some(p) => p + 1,
                None => return not_found(&format!("@sld:{s}")),
            },
        };
        let mut out = String::new();
        let page: Vec<&(Key, Value)> = d.slides.iter().skip(start).take(n).collect();
        for (i, (k, _)) in page.iter().enumerate() {
            let Some((sid, None)) = parse_key(k.as_str()) else {
                continue;
            };
            let layout = d
                .slide_field(sid, f::LAYOUT)
                .and_then(|l| d.layout_name(l))
                .unwrap_or("-");
            out.push_str(&format!(
                "@sld:{sid}\t{}\t{}\t{}\t{}\t{} shapes\n",
                start + i + 1,
                one_line(layout),
                one_line(cut(d.title(sid), TITLE_CUT)),
                if d.slide_field(sid, f::HIDDEN) == Some("1") {
                    "hidden"
                } else {
                    "shown"
                },
                d.shapes_of(sid).len()
            ));
        }
        if start + page.len() < d.slides.len() {
            if let Some((k, _)) = page.last() {
                out.push_str(&format!(
                    "next: slide list --after @sld:{}\n",
                    &k.as_str()[1..]
                ));
            }
        }
        done(Vec::new(), out)
    }))
}

fn shape_line(d: &Doc<'_>, sid: u32, fid: u32, kind: &str, text_max: usize) -> String {
    let g = |fld: &str| d.shape_field(sid, fid, fld).unwrap_or("");
    let text = d.shape_field(sid, fid, f::TEXT);
    let shown = text.map(|t| cut(t, text_max));
    format!(
        "@sld:{sid}/sp:{fid}\t{kind}\t{}\tgeom {}\tph {}\tedit {}\t{}{}\n",
        one_line(g(f::NAME)),
        if g(f::GEOM).is_empty() {
            "-"
        } else {
            g(f::GEOM)
        },
        if g(f::PH).is_empty() { "-" } else { g(f::PH) },
        g(f::EDIT),
        shown.map_or_else(|| "no text".to_owned(), |t| format!("text {}", one_line(t))),
        if text.is_some_and(|t| t.len() > text_max) {
            "\ttext_truncated"
        } else {
            ""
        }
    )
}

fn parse_slide_show(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--after", "--limit"], &[])?;
    let sid = slide_addr(a.one("slide")?)?;
    let after = a.val("--after").map(shape_addr).transpose()?;
    let n = limit(&a, SHAPES_PER_PAGE, SHAPES_PER_PAGE)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        let Some(pos) = d.position(sid) else {
            return not_found(&format!("@sld:{sid}"));
        };
        let shapes = d.shapes_of(sid);
        let start = match after {
            None => 0,
            Some((s, fid)) if s == sid => match shapes.iter().position(|(x, _)| *x == fid) {
                Some(i) => i + 1,
                None => return not_found(&format!("@sld:{s}/sp:{fid}")),
            },
            Some((s, _)) => {
                return Err(usage(format!(
                    "--after names a shape of @sld:{s}, not @sld:{sid}"
                )))
            }
        };
        let notes = d.slide_field(sid, f::NOTES);
        let mut out = format!(
            "@sld:{sid}\tposition {}\tlayout {}\t{}\tnotes {}\t{} shapes\n",
            pos + 1,
            one_line(
                d.slide_field(sid, f::LAYOUT)
                    .and_then(|l| d.layout_name(l))
                    .unwrap_or("-")
            ),
            if d.slide_field(sid, f::HIDDEN) == Some("1") {
                "hidden"
            } else {
                "shown"
            },
            notes.map_or_else(|| "none".to_owned(), |t| format!("{} bytes", t.len())),
            shapes.len()
        );
        let take = if mutant::active("ReadUnbounded") {
            usize::MAX
        } else {
            n
        };
        let page: Vec<&(u32, &str)> = shapes.iter().skip(start).take(take).collect();
        for (fid, kind) in &page {
            out.push_str(&shape_line(&d, sid, *fid, kind, SHOW_TEXT_MAX));
        }
        if start + page.len() < shapes.len() {
            if let Some((fid, _)) = page.last() {
                out.push_str(&format!(
                    "next: slide show @sld:{sid} --after @sld:{sid}/sp:{fid}\n"
                ));
            }
        }
        done(Vec::new(), out)
    }))
}

fn parse_shape_show(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let (sid, fid) = shape_addr(a.one("shape")?)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if !d.shape_live(sid, fid) {
            return not_found(&format!("@sld:{sid}/sp:{fid}"));
        }
        let kind = d
            .shapes_of(sid)
            .into_iter()
            .find(|(x, _)| *x == fid)
            .map_or("", |(_, k)| k);
        let mut out = format!("address\t@sld:{sid}/sp:{fid}\nkind\t{kind}\n");
        for fld in [
            f::NAME,
            f::DESCR,
            f::GEOM,
            f::PH,
            f::EDIT,
            f::HIDDEN,
            f::REFD,
        ] {
            if let Some(v) = d.shape_field(sid, fid, fld) {
                out.push_str(&format!("{fld}\t{}\n", one_line(v)));
            }
        }
        if let Some(t) = d.shape_field(sid, fid, f::TEXT) {
            out.push_str(&format!(
                "text\t{}\ntext_truncated\t{}\n",
                one_line(cut(t, SHAPE_TEXT_MAX)),
                if t.len() > SHAPE_TEXT_MAX {
                    "yes"
                } else {
                    "no"
                }
            ));
        }
        done(Vec::new(), out)
    }))
}

// ---------- edits ----------

fn place(a: &Args) -> Result<Option<Option<u32>>, ToolError> {
    match (a.val("--after"), a.has("--first")) {
        (Some(_), true) => Err(usage("give --after or --first, not both")),
        (Some(s), false) => Ok(Some(Some(slide_addr(s)?))),
        (None, true) => Ok(Some(None)),
        (None, false) => Ok(None),
    }
}

fn parse_slide_add(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--layout", "--after"], &["--first"])?;
    if !a.pos.is_empty() {
        return Err(usage("slide add takes no address"));
    }
    let layout = a
        .val("--layout")
        .ok_or_else(|| usage("give --layout NAME or --layout @layout:ID"))?
        .to_owned();
    let at = place(&a)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        let id = match layout.strip_prefix("@layout:") {
            Some(id) => {
                let found = id
                    .parse::<u64>()
                    .ok()
                    .filter(|i| d.layouts().iter().any(|(x, _)| x == i));
                match found {
                    Some(i) => i,
                    None => return not_found(&layout),
                }
            }
            None => {
                let m: Vec<u64> = d
                    .layouts()
                    .into_iter()
                    .filter(|(_, n)| *n == layout)
                    .map(|(i, _)| i)
                    .collect();
                match m.as_slice() {
                    [one] => *one,
                    [] => return not_found(&format!("layout {layout:?}")),
                    many => {
                        let ids: Vec<String> =
                            many.iter().map(|i| format!("@layout:{i}")).collect();
                        return refuse(
                            "pptx_layout_ambiguous",
                            &format!("{} layouts are named {layout:?}", many.len()),
                            &format!("slide add --layout {}", ids.join(" or --layout ")),
                        );
                    }
                }
            }
        };
        let anchor = match at {
            Some(Some(s)) => match d.position(s) {
                Some(_) => Anchor::After(slide_key(s)),
                None => return not_found(&format!("@sld:{s}")),
            },
            Some(None) => Anchor::Start,
            None => Anchor::End,
        };
        let Some(sid) = d.next_sid() else {
            return refuse("pptx_no_slide_id", "no slide id is left", "slide list");
        };
        // the layout's placeholders, as the deck map holds them (the file verb's `slide add`
        // makes the same ones, with ids from 2)
        let deck =
            super::model::LiveDeck::from_parts(&[], &[], &EMPTY, &EMPTY, d.deck).map_err(usage)?;
        let facts = deck
            .layout(id)
            .map(|l| l.placeholders.clone())
            .unwrap_or_default();
        let sk = slide_key(sid);
        let sops = vec![
            set(prop(&sk, f::SID), sid.to_string()),
            set(prop(&sk, f::LAYOUT), format!("@layout:{id}")),
            set(prop(&sk, f::HIDDEN), "0"),
        ];
        let mut list_ops = Vec::new();
        let mut hops = Vec::new();
        for (i, ph) in facts.iter().enumerate() {
            let fid = 2 + i as u32;
            let k = shape_key(sid, fid);
            list_ops.push(ListOp::Insert {
                key: k.clone(),
                at: Anchor::End,
                value: Value::str("placeholder"),
            });
            hops.push(set(prop(&k, f::SLIDE), sid.to_string()));
            hops.push(set(prop(&k, f::FID), fid.to_string()));
            hops.push(set(prop(&k, f::NAME), ph.name.clone()));
            hops.push(set(prop(&k, f::GEOM), ""));
            hops.push(set(prop(&k, f::EDIT), TextEdit::Plain.as_str()));
            hops.push(set(prop(&k, f::HIDDEN), "0"));
            if ph.body {
                hops.push(set(prop(&k, f::TEXT), ""));
            }
            hops.push(set(
                prop(&k, f::PH),
                Ph {
                    ty: ph.ty.clone(),
                    idx: ph.idx.clone(),
                }
                .encode(),
            ));
        }
        let mut edits = vec![list(
            SLIDES,
            vec![ListOp::Insert {
                key: sk,
                at: anchor,
                value: Value::str("slide"),
            }],
        )];
        if !list_ops.is_empty() {
            edits.push(list(SHAPES, list_ops));
        }
        edits.push(map(SLIDE_PROPS, sops));
        if !hops.is_empty() {
            edits.push(map(SHAPE_PROPS, hops));
        }
        done(edits, format!("@sld:{sid}\n"))
    }))
}

fn parse_slide_move(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--after"], &["--first"])?;
    let sid = slide_addr(a.one("slide")?)?;
    let Some(to) = place(&a)? else {
        return Err(usage("give --after @sld:ID or --first"));
    };
    if to == Some(sid) {
        return Err(usage("a slide can't go after itself"));
    }
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        let Some(i) = d.position(sid) else {
            return not_found(&format!("@sld:{sid}"));
        };
        if d.deck("sections") == Some("1") {
            return refuse(
                "pptx_sections",
                "the deck has sections: moving slides between them isn't live yet",
                "slide list",
            );
        }
        let (anchor, already) = match to {
            None => (Anchor::Start, i == 0),
            Some(a) => match d.position(a) {
                Some(j) => (Anchor::After(slide_key(a)), j + 1 == i),
                None => return not_found(&format!("@sld:{a}")),
            },
        };
        if already {
            return done(Vec::new(), "already there\n".into());
        }
        done(
            vec![list(
                SLIDES,
                vec![ListOp::Move {
                    key: slide_key(sid),
                    to: anchor,
                }],
            )],
            format!("@sld:{sid}\n"),
        )
    }))
}

fn parse_slide_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let sid = slide_addr(a.one("slide")?)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if d.position(sid).is_none() {
            return not_found(&format!("@sld:{sid}"));
        }
        if d.deck("sections") == Some("1") {
            return refuse(
                "pptx_sections",
                "the deck has sections: deleting slides from them isn't live yet",
                "slide list",
            );
        }
        if d.slide_field(sid, f::LINKED) == Some("1") {
            return refuse(
                "pptx_slide_linked",
                "another slide links to this one: its link would point at nothing",
                &format!("slide hide @sld:{sid}"),
            );
        }
        done(
            vec![list(
                SLIDES,
                vec![ListOp::Delete {
                    key: slide_key(sid),
                }],
            )],
            format!("@sld:{sid}\n"),
        )
    }))
}

fn parse_slide_hide(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &["--show"])?;
    let sid = slide_addr(a.one("slide")?)?;
    let hidden = if a.has("--show") { "0" } else { "1" };
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if d.position(sid).is_none() {
            return not_found(&format!("@sld:{sid}"));
        }
        if d.slide_field(sid, f::HIDDEN) == Some(hidden) {
            return done(Vec::new(), "already so\n".into());
        }
        done(
            vec![map(
                SLIDE_PROPS,
                vec![set(prop(&slide_key(sid), f::HIDDEN), hidden)],
            )],
            format!("@sld:{sid}\n"),
        )
    }))
}

fn parse_shape_add(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--kind", "--at", "--text"], &[])?;
    let sid = slide_addr(a.one("slide")?)?;
    match a.val("--kind") {
        Some("textbox") => {}
        Some(k) => return Err(usage(format!("--kind {k}: only textbox is live"))),
        None => return Err(usage("give --kind textbox")),
    }
    let at = a.val("--at").ok_or_else(|| usage("give --at X,Y,W,H"))?;
    let v: Vec<&str> = at.split(',').collect();
    let [x, y, w, h] = v.as_slice() else {
        return Err(usage("--at is X,Y,W,H"));
    };
    let g = Geom {
        x: check_coord(x, "x", false)?,
        y: check_coord(y, "y", false)?,
        w: check_coord(w, "the width", true)?,
        h: check_coord(h, "the height", true)?,
        ..Geom::default()
    };
    let text = text_in(a.val("--text").ok_or_else(|| usage("give --text"))?)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if d.position(sid).is_none() {
            return not_found(&format!("@sld:{sid}"));
        }
        let Some(fid) = d.next_fid(sid) else {
            return refuse(
                "pptx_no_shape_id",
                "no shape id is left on this slide",
                "slide show",
            );
        };
        let k = shape_key(sid, fid);
        let at = match d.shapes_of(sid).last() {
            Some((last, _)) => Anchor::After(shape_key(sid, *last)),
            None => Anchor::End,
        };
        let hops = vec![
            set(prop(&k, f::SLIDE), sid.to_string()),
            set(prop(&k, f::FID), fid.to_string()),
            set(prop(&k, f::NAME), format!("TextBox {}", fid - 1)),
            set(prop(&k, f::GEOM), g.encode()),
            set(prop(&k, f::EDIT), TextEdit::Plain.as_str()),
            set(prop(&k, f::HIDDEN), "0"),
            set(prop(&k, f::TEXT), text.clone()),
        ];
        done(
            vec![
                list(
                    SHAPES,
                    vec![ListOp::Insert {
                        key: k,
                        at,
                        value: Value::str("textbox"),
                    }],
                ),
                map(SHAPE_PROPS, hops),
            ],
            format!("@sld:{sid}/sp:{fid}\n"),
        )
    }))
}

fn parse_shape_text(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--text"], &["--drop-formatting"])?;
    let addr = a.one("shape")?.to_owned();
    let (sid, fid) = shape_addr(&addr)?;
    let text = text_in(a.val("--text").ok_or_else(|| usage("give --text"))?)?;
    let drop = a.has("--drop-formatting");
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if !d.shape_live(sid, fid) {
            return not_found(&addr);
        }
        let edit = d
            .shape_field(sid, fid, f::EDIT)
            .and_then(TextEdit::parse)
            .unwrap_or(TextEdit::Never);
        match edit {
            TextEdit::Never => {
                return refuse(
                    "pptx_text_not_editable",
                    "this shape's text isn't editable live (a picture, table, group, chart, \
                     alternate content, or text holding a slide number or date)",
                    &format!("shape show {addr}"),
                )
            }
            TextEdit::Formatted if !drop => {
                return refuse(
                    "pptx_text_formatted",
                    "the shape's text has more than one format: setting it keeps only the first \
                     paragraph's and run's",
                    &format!("shape text {addr} --text TEXT --drop-formatting"),
                )
            }
            _ => {}
        }
        let k = shape_key(sid, fid);
        let mut ops = Vec::new();
        if d.shape_field(sid, fid, f::TEXT) != Some(text.as_str()) {
            ops.push(set(prop(&k, f::TEXT), text.clone()));
        }
        if edit == TextEdit::Formatted {
            ops.push(set(prop(&k, f::EDIT), TextEdit::Plain.as_str()));
        }
        if ops.is_empty() {
            return done(Vec::new(), "already so\n".into());
        }
        done(vec![map(SHAPE_PROPS, ops)], format!("{addr}\n"))
    }))
}

fn parse_shape_move(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--x", "--y", "--w", "--h"], &[])?;
    let addr = a.one("shape")?.to_owned();
    let (sid, fid) = shape_addr(&addr)?;
    let v = [
        a.val("--x")
            .map(|v| check_coord(v, "x", false))
            .transpose()?,
        a.val("--y")
            .map(|v| check_coord(v, "y", false))
            .transpose()?,
        a.val("--w")
            .map(|v| check_coord(v, "the width", true))
            .transpose()?,
        a.val("--h")
            .map(|v| check_coord(v, "the height", true))
            .transpose()?,
    ];
    if v.iter().all(Option::is_none) {
        return Err(usage("give --x, --y, --w or --h"));
    }
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if !d.shape_live(sid, fid) {
            return not_found(&addr);
        }
        if d.shape_field(sid, fid, f::MC) == Some("1") {
            return refuse(
                "pptx_alternate_content",
                "an alternate-content shape is edited whole only",
                &format!("shape show {addr}"),
            );
        }
        let old = d.shape_field(sid, fid, f::GEOM).unwrap_or("");
        let mut g = if old.is_empty() {
            let [Some(x), Some(y), Some(w), Some(h)] = v else {
                return refuse(
                    "pptx_needs_a_box",
                    "the shape takes its place from its layout: it needs all four values",
                    &format!("shape move {addr} --x X --y Y --w W --h H"),
                );
            };
            Geom {
                x,
                y,
                w,
                h,
                ..Geom::default()
            }
        } else {
            Geom::parse(old).ok_or_else(|| usage(format!("{addr}: a damaged geom {old:?}")))?
        };
        if let Some(x) = v[0] {
            g.x = x;
        }
        if let Some(y) = v[1] {
            g.y = y;
        }
        if let Some(w) = v[2] {
            g.w = w;
        }
        if let Some(h) = v[3] {
            g.h = h;
        }
        let new = g.encode();
        if new == old {
            return done(Vec::new(), "already so\n".into());
        }
        done(
            vec![map(
                SHAPE_PROPS,
                vec![set(prop(&shape_key(sid, fid), f::GEOM), new)],
            )],
            format!("{addr}\n"),
        )
    }))
}

fn parse_shape_order(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--after"], &["--front", "--back"])?;
    let addr = a.one("shape")?.to_owned();
    let (sid, fid) = shape_addr(&addr)?;
    let after = a.val("--after").map(shape_addr).transpose()?;
    let n =
        usize::from(a.has("--front")) + usize::from(a.has("--back")) + usize::from(after.is_some());
    if n != 1 {
        return Err(usage("give one of --front, --back, --after @sld:ID/sp:M"));
    }
    if after == Some((sid, fid)) {
        return Err(usage("a shape can't go after itself"));
    }
    let front = a.has("--front");
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if !d.shape_live(sid, fid) {
            return not_found(&addr);
        }
        let run: Vec<u32> = d.shapes_of(sid).into_iter().map(|(x, _)| x).collect();
        let i = run.iter().position(|x| *x == fid).expect("live");
        let (to, already) = match after {
            Some((s2, f2)) => {
                if s2 != sid && !mutant::active("ZOrderGlobal") {
                    return refuse(
                        "pptx_order_other_slide",
                        "--after names a shape on another slide: z-order is within one slide",
                        &format!("slide show @sld:{sid}"),
                    );
                }
                if !d.shape_live(s2, f2) {
                    return not_found(&format!("@sld:{s2}/sp:{f2}"));
                }
                let j = run.iter().position(|x| *x == f2);
                (
                    Anchor::After(shape_key(s2, f2)),
                    j.is_some_and(|j| j + 1 == i),
                )
            }
            None if front => {
                let last = *run.last().expect("live");
                (Anchor::After(shape_key(sid, last)), last == fid)
            }
            None => {
                let first = run[0];
                (Anchor::Before(shape_key(sid, first)), first == fid)
            }
        };
        if already {
            return done(Vec::new(), "already there\n".into());
        }
        done(
            vec![list(
                SHAPES,
                vec![ListOp::Move {
                    key: shape_key(sid, fid),
                    to,
                }],
            )],
            format!("{addr}\n"),
        )
    }))
}

fn parse_shape_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &[], &[])?;
    let addr = a.one("shape")?.to_owned();
    let (sid, fid) = shape_addr(&addr)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if !d.shape_live(sid, fid) {
            return not_found(&addr);
        }
        if d.shape_field(sid, fid, f::REFD) == Some("1") {
            return refuse(
                "pptx_shape_referenced",
                "an animation or connector names this shape",
                &format!("shape show {addr}"),
            );
        }
        done(
            vec![list(
                SHAPES,
                vec![ListOp::Delete {
                    key: shape_key(sid, fid),
                }],
            )],
            format!("{addr}\n"),
        )
    }))
}

fn parse_notes_set(args: &[String]) -> Result<StructEdit, ToolError> {
    let a = parse_args(args, &["--text"], &[])?;
    let sid = slide_addr(a.one("slide")?)?;
    let text = text_in(a.val("--text").ok_or_else(|| usage("give --text"))?)?;
    Ok(Box::new(move |ctx: &VerbCtx<'_>| {
        let d = Doc::of(ctx.view());
        if d.position(sid).is_none() {
            return not_found(&format!("@sld:{sid}"));
        }
        if d.slide_field(sid, f::NO_NOTES) == Some("1") {
            return refuse(
                "pptx_notes_fixed",
                "the slide's notes slide has no notes placeholder",
                &format!("slide show @sld:{sid}"),
            );
        }
        let now = d.slide_field(sid, f::NOTES);
        if now.is_none() && d.deck("notes_master") != Some("1") {
            return refuse(
                "pptx_no_notes_master",
                "the deck has no notes master to make the notes slide from",
                &format!("slide show @sld:{sid}"),
            );
        }
        if now == Some(text.as_str()) {
            return done(Vec::new(), "already so\n".into());
        }
        done(
            vec![map(
                SLIDE_PROPS,
                vec![set(prop(&slide_key(sid), f::NOTES), text.clone())],
            )],
            format!("@sld:{sid}\n"),
        )
    }))
}
