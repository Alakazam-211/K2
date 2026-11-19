//! The deck's verbs on the kit's tier-0 path ([`PptxPack`](crate::PptxPack)'s `verbs()`,
//! `docs/SESSIONS.md` in the kit, "Verbs on a blob doc"): `slide list`, and the seven edits
//! of [`crate::verbs`], each a pure function from a version's bytes and its arguments to the
//! ranges it replaces ([`crate::verbs::plan`]). The kit applies the ranges, reads the output
//! with the deck's own index, refuses a node the verb didn't declare, and records one
//! version.
//!
//! **Under the lease.** Each edit is [`VerbMerge::Lease`]: it runs only while the caller holds
//! the doc's lease. A verb's write followed by a person's save from the app is a whole-file
//! conflict until this pack merges a save against the version the app opened, so the lease is
//! what keeps the app from saving over the verb meanwhile.
//!
//! **Package limits.** A verb opens the deck under the package layer's default [`Limits`]:
//! the kit hands a verb the version's bytes and its arguments only.
//!
//! **Holds** ([`hold_reach`]): a hold on a slide covers its shapes and its notes; on a group,
//! the shapes in it; on a master, its layouts; on anything else, the node itself.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use k2_mpc_base::errors::{ErrorEntry, Value};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::session::{
    BlobPlan, BlobVerb, BlobVerbCtx, BlobVerbOut, BlobVerbRefused, HoldReach, VerbInput, VerbMerge,
};
use k2_mpc_base::tools::exit::{NOT_FOUND, OK, USAGE};

use crate::verbs::{self, parse_address, Edit, LayoutSel, Place, VerbError};
use crate::Limits;

/// The pack's own refusals ([`crate::PptxPack`]'s `errors()`).
pub static PPTX_ERRORS: &[ErrorEntry] = &[
    ErrorEntry::pack(
        "PPTX-E001",
        "deck_declined",
        2,
        "this pack can't read the deck",
        "Edit it in the app: the deck verbs need a deck this pack reads.",
    ),
    ErrorEntry::pack(
        "PPTX-E002",
        "deck_refused",
        2,
        "the verb doesn't do that to this deck",
        "Change the call as the reason says, or make that change in the app.",
    ),
    ErrorEntry::pack(
        "PPTX-E004",
        "deck_not_found",
        4,
        "nothing at that address",
        "List the slides (slide list --shapes) and use an address it shows.",
    ),
];

/// A [`VerbError`] as the kit takes it: a usage error as usage (exit 2), the rest by the
/// pack's entries.
pub fn refusal(e: VerbError) -> BlobVerbRefused {
    let entry = |id: &str| PPTX_ERRORS.iter().find(|x| x.id == id);
    let (id, why) = match e {
        VerbError::Usage(m) => return BlobVerbRefused::usage(m),
        VerbError::Declined(d) => ("PPTX-E001", d.to_string()),
        VerbError::Refused(m) => ("PPTX-E002", m),
        VerbError::NotFound(a) => ("PPTX-E004", format!("nothing at {a}")),
    };
    match entry(id) {
        Some(x) => BlobVerbRefused::pack(x, why, x.next),
        None => BlobVerbRefused::usage(why),
    }
}

/// How far a hold on a node of `kind` reaches (the module's **Holds**).
pub fn hold_reach(kind: &str) -> HoldReach {
    match kind {
        "slide" | "group" | "master" => HoldReach::Subtree,
        _ => HoldReach::Node,
    }
}

// ---------------------------------------------------------------------------------------------
// the arguments
// ---------------------------------------------------------------------------------------------

/// A call's arguments: its positionals, the flags with a value, the switches.
struct Args {
    pos: Vec<String>,
    vals: BTreeMap<&'static str, String>,
    on: BTreeSet<&'static str>,
}

fn usage(m: impl Into<String>) -> BlobVerbRefused {
    BlobVerbRefused::usage(m)
}

/// Read `args`: `with_value` flags take the next argument whatever it is; `switches` stand
/// alone; after `--`, everything is a positional. An unknown or repeated flag is usage.
fn parse(
    args: &[String],
    with_value: &[&'static str],
    switches: &[&'static str],
) -> Result<Args, BlobVerbRefused> {
    let mut a = Args {
        pos: Vec::new(),
        vals: BTreeMap::new(),
        on: BTreeSet::new(),
    };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        if x == "--" {
            a.pos.extend(it.cloned());
            break;
        }
        if let Some(&f) = with_value.iter().find(|f| **f == x.as_str()) {
            let v = it
                .next()
                .ok_or_else(|| usage(format!("{f} needs a value")))?;
            if a.vals.insert(f, v.clone()).is_some() {
                return Err(usage(format!("{f} given twice")));
            }
        } else if let Some(&f) = switches.iter().find(|f| **f == x.as_str()) {
            if !a.on.insert(f) {
                return Err(usage(format!("{f} given twice")));
            }
        } else if x.starts_with("--") {
            return Err(usage(format!("{x}: not a flag of this verb")));
        } else {
            a.pos.push(x.clone());
        }
    }
    Ok(a)
}

impl Args {
    /// The one positional, else usage naming `what`.
    fn one(&self, what: &str) -> Result<&str, BlobVerbRefused> {
        match self.pos.as_slice() {
            [one] => Ok(one),
            _ => Err(usage(format!("give one {what}"))),
        }
    }

    fn none(&self) -> Result<(), BlobVerbRefused> {
        match self.pos.first() {
            None => Ok(()),
            Some(x) => Err(usage(format!("{x:?}: this verb takes no address"))),
        }
    }

    fn need(&self, f: &str) -> Result<&str, BlobVerbRefused> {
        self.vals
            .get(f)
            .map(String::as_str)
            .ok_or_else(|| usage(format!("give {f}")))
    }
}

fn slide_of(s: &str) -> Result<u32, BlobVerbRefused> {
    match parse_address(s).map_err(refusal)? {
        (sid, None) => Ok(sid),
        _ => Err(usage(format!("{s:?} isn't a slide (@sld:ID)"))),
    }
}

fn shape_of(s: &str) -> Result<(u32, u32), BlobVerbRefused> {
    match parse_address(s).map_err(refusal)? {
        (sid, Some(fid)) => Ok((sid, fid)),
        _ => Err(usage(format!("{s:?} isn't a shape (@sld:ID/sp:N)"))),
    }
}

fn int(v: &str, what: &str) -> Result<i64, BlobVerbRefused> {
    v.trim()
        .parse::<i64>()
        .map_err(|_| usage(format!("{what} isn't a whole number of EMU")))
}

/// `--after @sld:ID` or `--first`, or neither.
fn place(a: &Args) -> Result<Option<Place>, BlobVerbRefused> {
    match (a.vals.get("--after"), a.on.contains("--first")) {
        (Some(_), true) => Err(usage("give --after or --first, not both")),
        (Some(s), false) => Ok(Some(Place::After(slide_of(s)?))),
        (None, true) => Ok(Some(Place::First)),
        (None, false) => Ok(None),
    }
}

/// Each edit verb's arguments, as an [`Edit`].
fn edit_of(verb: &str, args: &[String]) -> Result<Edit, BlobVerbRefused> {
    match verb {
        "slide add" => {
            let a = parse(args, &["--layout", "--after"], &["--first"])?;
            a.none()?;
            let l = a.need("--layout")?;
            let layout = match l.strip_prefix("@layout:") {
                Some(id) => LayoutSel::Id(
                    id.parse::<u64>()
                        .map_err(|_| usage(format!("{l:?} isn't @layout:ID")))?,
                ),
                None => LayoutSel::Name(l.to_owned()),
            };
            Ok(Edit::AddSlide {
                layout,
                place: place(&a)?,
            })
        }
        "slide move" => {
            let a = parse(args, &["--after"], &["--first"])?;
            let slide = slide_of(a.one("slide (@sld:ID)")?)?;
            let place = place(&a)?.ok_or_else(|| usage("give --after @sld:ID or --first"))?;
            Ok(Edit::MoveSlide { slide, place })
        }
        "shape add" => {
            let a = parse(args, &["--kind", "--at", "--text"], &[])?;
            let slide = slide_of(a.one("slide (@sld:ID)")?)?;
            if a.need("--kind")? != "textbox" {
                return Err(usage("--kind: textbox is the kind this verb adds"));
            }
            let at = a.need("--at")?;
            let n: Vec<&str> = at.split(',').collect();
            let [x, y, w, h] = n.as_slice() else {
                return Err(usage("--at is X,Y,W,H in EMU"));
            };
            Ok(Edit::AddTextBox {
                slide,
                at: (int(x, "X")?, int(y, "Y")?, int(w, "W")?, int(h, "H")?),
                text: a.need("--text")?.to_owned(),
            })
        }
        "shape text" => {
            let a = parse(args, &["--text"], &[])?;
            let (slide, shape) = shape_of(a.one("shape (@sld:ID/sp:N)")?)?;
            Ok(Edit::SetText {
                slide,
                shape,
                text: a.need("--text")?.to_owned(),
            })
        }
        "shape move" => {
            let a = parse(args, &["--x", "--y", "--w", "--h"], &[])?;
            let (slide, shape) = shape_of(a.one("shape (@sld:ID/sp:N)")?)?;
            let v = |f: &str| a.vals.get(f).map(|v| int(v, f)).transpose();
            let (x, y, w, h) = (v("--x")?, v("--y")?, v("--w")?, v("--h")?);
            if x.is_none() && y.is_none() && w.is_none() && h.is_none() {
                return Err(usage("give at least one of --x, --y, --w, --h"));
            }
            Ok(Edit::MoveShape {
                slide,
                shape,
                x,
                y,
                w,
                h,
            })
        }
        "shape delete" => {
            let a = parse(args, &[], &[])?;
            let (slide, shape) = shape_of(a.one("shape (@sld:ID/sp:N)")?)?;
            Ok(Edit::DeleteShape { slide, shape })
        }
        "notes set" => {
            let a = parse(args, &["--text"], &[])?;
            let slide = slide_of(a.one("slide (@sld:ID)")?)?;
            Ok(Edit::SetNotes {
                slide,
                text: a.need("--text")?.to_owned(),
            })
        }
        other => Err(usage(format!("{other}: not a deck verb"))),
    }
}

// ---------------------------------------------------------------------------------------------
// the verbs
// ---------------------------------------------------------------------------------------------

fn read_all(src: &mut dyn Read) -> Result<Vec<u8>, BlobVerbRefused> {
    let mut bytes = Vec::new();
    src.read_to_end(&mut bytes)
        .map_err(|_| usage("the version can't be read"))?;
    Ok(bytes)
}

/// The address an edit's result is about: what it made, else what it names.
fn about(edit: &Edit, made: Option<&str>) -> String {
    if let Some(m) = made {
        return m.to_owned();
    }
    match edit {
        Edit::AddSlide { .. } => String::new(),
        Edit::MoveSlide { slide, .. } | Edit::AddTextBox { slide, .. } => format!("@sld:{slide}"),
        Edit::SetNotes { slide, .. } => format!("@notes:{slide}"),
        Edit::SetText { slide, shape, .. }
        | Edit::MoveShape { slide, shape, .. }
        | Edit::DeleteShape { slide, shape } => format!("@sld:{slide}/sp:{shape}"),
    }
}

/// Plans edit verb `verb` (its past-tense `word`) on the version the kit hands it.
fn edit_verb(
    src: &mut dyn Read,
    args: &[String],
    verb: &str,
    word: &str,
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let edit = edit_of(verb, args)?;
    let bytes = read_all(src)?;
    let p = verbs::plan(&bytes, &edit, &Limits::default()).map_err(refusal)?;
    let mut plan = BlobPlan::new();
    for s in p.spans {
        plan = plan.span(s.at as u64, s.len as u64, s.with);
    }
    let addr = about(&edit, p.address.as_deref());
    let stdout = if plan.is_empty() {
        format!("unchanged {addr}\n")
    } else {
        format!("{word} {addr}\n")
    };
    let mut out = BlobVerbOut::edit(plan, stdout)
        .with_data(Value::Obj(vec![("address".into(), Value::from(&addr))]));
    for k in p.touched {
        out = out.touching(k);
    }
    for k in p.created {
        out = out.creating(k);
    }
    if !addr.is_empty() {
        out = out.viewing(addr);
    }
    Ok(out)
}

/// `slide list [--shapes]`: each slide's address, position, layout, title, hidden and notes;
/// with `--shapes`, each shape's address, kind, name, bounds and text.
fn slide_list(
    _ctx: &BlobVerbCtx<'_>,
    src: &mut dyn Read,
    args: &[String],
) -> Result<BlobVerbOut, BlobVerbRefused> {
    let a = parse(args, &[], &["--shapes"])?;
    a.none()?;
    let shapes = a.on.contains("--shapes");
    let bytes = read_all(src)?;
    let slides = verbs::list(&bytes, &Limits::default()).map_err(refusal)?;
    let s = |x: &str| Value::from(x);
    let opt = |x: Option<&str>| x.map_or(Value::Null, Value::from);
    let rows = slides
        .iter()
        .map(|sl| {
            let mut o = vec![
                ("address".to_owned(), s(&sl.address)),
                ("position".to_owned(), Value::from(sl.position)),
                ("layout".to_owned(), opt(sl.layout.as_deref())),
                ("title".to_owned(), s(&sl.title)),
                ("hidden".to_owned(), Value::from(sl.hidden)),
                ("notes".to_owned(), opt(sl.notes.as_deref())),
            ];
            if shapes {
                let list = sl
                    .shapes
                    .iter()
                    .map(|sh| {
                        let bounds = sh.bounds.map_or(Value::Null, |(x, y, w, h)| {
                            Value::List(vec![x.into(), y.into(), w.into(), h.into()])
                        });
                        Value::Obj(vec![
                            ("address".to_owned(), opt(sh.address.as_deref())),
                            ("kind".to_owned(), s(sh.kind)),
                            ("name".to_owned(), s(&sh.name)),
                            ("bounds".to_owned(), bounds),
                            ("text".to_owned(), opt(sh.text.as_deref())),
                            ("depth".to_owned(), Value::from(sh.depth)),
                        ])
                    })
                    .collect();
                o.push(("shapes".to_owned(), Value::List(list)));
            }
            Value::Obj(o)
        })
        .collect();
    Ok(BlobVerbOut::read(verbs::render(&slides, shapes))
        .with_data(Value::Obj(vec![("slides".into(), Value::List(rows))])))
}

const READ: &[PersonAction] = &[PersonAction::ReadAll];
const EDIT: &[PersonAction] = &[PersonAction::Rewrite];
const EXITS: &[u8] = &[OK, USAGE, NOT_FOUND];

macro_rules! edit_verbs {
    ($($f:ident => $name:literal, $word:literal, $usage:literal;)*) => {
        $(
            fn $f(
                _ctx: &BlobVerbCtx<'_>,
                src: &mut dyn Read,
                args: &[String],
            ) -> Result<BlobVerbOut, BlobVerbRefused> {
                edit_verb(src, args, $name, $word)
            }
        )*
        /// The edit verbs, as the kit lists them: each under the caller's own lease.
        const EDIT_VERBS: &[BlobVerb] = &[
            $(BlobVerb::edit($name, $usage, EDIT, VerbMerge::Lease, VerbInput::Whole, $f)
                .with_word($word)
                .with_exits(EXITS),)*
        ];
    };
}

edit_verbs! {
    e_slide_add => "slide add", "added", "--layout NAME|@layout:ID [--after @sld:ID | --first] (the layout's placeholders, empty; default: last)";
    e_slide_move => "slide move", "moved", "@sld:ID (--after @sld:ID | --first) (a deck with sections: exit 2)";
    e_shape_add => "shape add", "added", "@sld:ID --kind textbox --at X,Y,W,H (EMU) --text TEXT (in front of the others)";
    e_shape_text => "shape text", "set", "@sld:ID/sp:N --text TEXT (the whole text body; each line a paragraph, keeping the first paragraph's and run's formatting)";
    e_shape_move => "shape move", "moved", "@sld:ID/sp:N [--x X] [--y Y] [--w W] [--h H] (EMU; a shape placed by its layout needs all four)";
    e_shape_delete => "shape delete", "deleted", "@sld:ID/sp:N (an animated or connected shape: exit 2)";
    e_notes_set => "notes set", "set", "@sld:ID --text TEXT (makes the notes slide from the notes master when there is none)";
}

/// Every verb the pack gives its blob docs: `slide list`, then the edits.
pub static VERB_SET: &[BlobVerb] = &[
    BlobVerb::read(
        "slide list",
        "[--shapes] (each slide: address, position, layout, title, hidden, notes; --shapes: each \
         shape's address, kind, name, bounds and text)",
        READ,
        VerbInput::Whole,
        slide_list,
    )
    .with_exits(EXITS),
    EDIT_VERBS[0],
    EDIT_VERBS[1],
    EDIT_VERBS[2],
    EDIT_VERBS[3],
    EDIT_VERBS[4],
    EDIT_VERBS[5],
    EDIT_VERBS[6],
];
