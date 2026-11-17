//! The read verbs on one version's bytes. The shared ones (`drawing info`, `layer list`,
//! `block list`, `entity find`, `text find`) are the model's ([`k2_mpc_cad_model::reads`]); the
//! DXF ones are here:
//!
//! - `header get $VAR…`: header variables as written (`dxf.header/1`). `$LASTSAVEDBY`, which
//!   usually holds a login name, is read only when named;
//! - `check [--writeback]`: structure, handle, reference, table and header findings, each with a
//!   severity, a code (`dxf.check.<name>`) and an address or byte offset (`dxf.check/1`); with
//!   `--writeback`, the tokens written back are compared with the bytes, and the entities the
//!   edit verbs could change are counted.

use std::collections::BTreeMap;

use k2_mpc_base::errors::Value;
use k2_mpc_cad_model::args::{self, Spec};
use k2_mpc_cad_model::reads;
pub use k2_mpc_cad_model::reads::ReadOut;
use k2_mpc_cad_model::{BlockKind, Doc};

use crate::error::{CadError, USAGE};
use crate::structure::Structure;
use crate::token::decode;
use crate::{convert, handle, mutant, structure, token, Limits};

/// Every read verb, by name.
pub const NAMES: [&str; 12] = [
    "drawing info",
    "layer list",
    "block list",
    "entity find",
    "text find",
    "attrib find",
    "block show",
    "entity props",
    "header get",
    "check",
    "draw header",
    "draw page",
];

const DRAWING_INFO: Spec = Spec {
    verb: "drawing info",
    valued: &[],
    bare: &[],
    words: false,
};

/// Runs read verb `verb` on `bytes` with `args` (the words after the verb).
pub fn run(bytes: &[u8], verb: &str, args: &[&str]) -> Result<ReadOut, CadError> {
    if !NAMES.contains(&verb) {
        return Err(CadError::new(
            USAGE,
            format!("no read verb {verb:?}"),
            format!("the read verbs: {}", NAMES.join(", ")),
        ));
    }
    let limits = Limits::default();
    if verb == "draw header" || verb == "draw page" {
        let d = crate::draw::DxfDrawing::open(bytes, &limits)?;
        return if verb == "draw header" {
            k2_mpc_cad_model::draw::header(&d, args, &k2_mpc_cad_model::draw::NoXrefs)
        } else {
            k2_mpc_cad_model::draw::page(&d, args)
        };
    }
    let t = token::tokenize(bytes, &limits)?;
    let s = structure::walk(t, &limits)?;
    let doc = convert::doc(&s);
    match verb {
        "drawing info" => {
            DRAWING_INFO.parse(args)?;
            let tail = s.tok.bytes.len() - s.tok.tail_at;
            Ok(reads::info(
                &doc,
                vec![
                    ("bom", Value::from(s.tok.bom)),
                    ("tail_bytes", Value::from(tail)),
                    ("comments", Value::from(s.comments.len())),
                ],
            ))
        }
        "layer list" => reads::layer_list(&doc, args),
        "block list" => reads::block_list(&doc, args),
        "entity find" => reads::entity_find(&doc, args),
        "text find" => reads::text_find(&doc, args),
        "attrib find" => reads::attrib_find(&doc, args),
        "block show" => reads::block_show(&doc, args),
        "entity props" => reads::entity_props(&doc, args),
        "header get" => header_get(&s, args),
        _ => check(&s, &doc, args),
    }
}

const HEADER_GET: Spec = Spec {
    verb: "header get",
    valued: &[],
    bare: &[],
    words: true,
};

/// `header get $VAR…`.
pub fn header_get(s: &Structure<'_>, a: &[&str]) -> Result<ReadOut, CadError> {
    let args = HEADER_GET.parse(a)?;
    if args.words.is_empty() {
        return Err(args::usage(
            &HEADER_GET,
            "header get needs a variable: header get $INSUNITS",
        ));
    }
    let mut vars = Vec::new();
    let mut missing = Vec::new();
    let mut out = String::new();
    for w in &args.words {
        let Some(v) = s.var(w) else {
            missing.push(Value::from(w.as_str()));
            continue;
        };
        let pairs: Vec<Value> = (v.first + 1..v.end)
            .map(|i| {
                let text = decode(s.tok.value(i)).into_owned();
                out.push_str(&format!("{}\t{}\t{}\n", v.name, s.tok.pairs[i].code, text));
                Value::obj([
                    ("code", Value::from(i64::from(s.tok.pairs[i].code))),
                    ("value", Value::from(text)),
                ])
            })
            .collect();
        vars.push(Value::obj([
            ("name", Value::from(v.name.as_str())),
            ("pairs", Value::List(pairs)),
        ]));
    }
    Ok(ReadOut {
        stdout: out,
        data: Value::obj([
            ("vars", Value::List(vars)),
            ("missing", Value::List(missing)),
        ]),
        spec: ("dxf.header", 1),
        next: None,
    })
}

const CHECK: Spec = Spec {
    verb: "check",
    valued: &[],
    bare: &["--writeback"],
    words: false,
};

/// At most this many findings of one code are listed; the rest are counted.
pub const FINDINGS_PER_CODE: usize = 200;

struct Findings {
    rows: Vec<Value>,
    per: BTreeMap<&'static str, usize>,
    counts: [u64; 3],
    text: String,
}

impl Findings {
    fn add(
        &mut self,
        sev: &'static str,
        code: &'static str,
        at: At,
        message: String,
        next: Option<&str>,
    ) {
        let k = match sev {
            "error" => 0,
            "warning" => 1,
            _ => 2,
        };
        self.counts[k] += 1;
        let n = self.per.entry(code).or_default();
        *n += 1;
        if *n > FINDINGS_PER_CODE {
            return;
        }
        let (address, offset) = match at {
            At::Address(a) => (Value::from(a.as_str()), Value::Null),
            At::Offset(o) => (Value::Null, Value::from(o as u64)),
            At::None => (Value::Null, Value::Null),
        };
        let place = match (&address, &offset) {
            (Value::Str(a), _) => a.clone(),
            (_, Value::UInt(o)) => format!("byte {o}"),
            _ => "-".into(),
        };
        self.text
            .push_str(&format!("{sev}\t{code}\t{place}\t{message}\n"));
        self.rows.push(Value::obj([
            ("severity", Value::from(sev)),
            ("code", Value::from(code)),
            ("address", address),
            ("offset", offset),
            ("message", Value::from(message)),
            ("next", next.map_or(Value::Null, Value::from)),
        ]));
    }
}

enum At {
    Address(String),
    Offset(usize),
    None,
}

/// `check [--writeback]`.
pub fn check(s: &Structure<'_>, doc: &Doc, a: &[&str]) -> Result<ReadOut, CadError> {
    let args = CHECK.parse(a)?;
    let mut f = Findings {
        rows: Vec::new(),
        per: BTreeMap::new(),
        counts: [0; 3],
        text: String::new(),
    };
    // structure
    if !s.tok.eof {
        f.add(
            "error",
            "dxf.check.eof_missing",
            At::Offset(s.tok.bytes.len()),
            "the file has no end marker (0/EOF): it may be cut short".into(),
            Some("open it in the program that wrote it and save it again"),
        );
    }
    for sec in s.sections.iter().filter(|x| !x.closed) {
        f.add(
            "error",
            "dxf.check.section_unclosed",
            At::Offset(s.tok.pairs[sec.first].start()),
            format!("section {} has no ENDSEC", sec.name),
            None,
        );
    }
    if s.tok.tail_at < s.tok.bytes.len() {
        f.add(
            "info",
            "dxf.check.bytes_after_eof",
            At::Offset(s.tok.tail_at),
            format!(
                "{} bytes after the end marker, kept as they are",
                s.tok.bytes.len() - s.tok.tail_at
            ),
            None,
        );
    }
    // handles
    let mut owned: BTreeMap<u64, u32> = BTreeMap::new();
    for e in &doc.entities {
        if let Some(h) = e.handle {
            *owned.entry(h).or_default() += 1;
        }
    }
    for l in &doc.layers {
        if let Some(h) = l.handle {
            *owned.entry(h).or_default() += 1;
        }
    }
    for b in &doc.blocks {
        if let Some(h) = b.handle {
            *owned.entry(h).or_default() += 1;
        }
    }
    for (h, n) in owned.iter().filter(|(_, n)| **n > 1) {
        f.add(
            "error",
            "dxf.check.duplicate_handle",
            At::Address(handle::key(*h)),
            format!("{n} objects have the handle {}", handle::norm(*h)),
            Some("the program that wrote it may renumber on its next save; edits by handle are unsafe until then"),
        );
    }
    if doc.handles == "partial" {
        for (i, e) in doc.entities.iter().enumerate() {
            if e.handle.is_none() {
                f.add(
                    "warning",
                    "dxf.check.handle_missing",
                    At::Address(doc.entity_address(i)),
                    format!(
                        "a {} without a handle in a drawing whose objects have them",
                        e.kind
                    ),
                    None,
                );
            }
        }
    }
    if let (Some(seed), Some(max)) = (doc.handseed, doc.hmax) {
        if seed <= max {
            f.add(
                "warning",
                "dxf.check.handseed_low",
                At::None,
                format!(
                    "$HANDSEED {} is not above the highest handle {}",
                    handle::norm(seed),
                    handle::norm(max)
                ),
                None,
            );
        }
    }
    // references
    let skip_blocks = mutant::active("CheckSkipsBlocks");
    let apps: Vec<String> = s
        .table("APPID")
        .map(|t| {
            t.entries
                .iter()
                .filter_map(|u| (u.first + 1..u.end).find(|&j| s.tok.pairs[j].code == 2))
                .map(|j| s.text(j).to_ascii_uppercase())
                .collect()
        })
        .unwrap_or_default();

    for (i, e) in doc.entities.iter().enumerate() {
        if skip_blocks && doc.owner_kind(e) == Some(BlockKind::Block) {
            continue;
        }
        if let Some(b) = &e.block {
            if doc.block_named(b).is_none() {
                f.add(
                    "error",
                    "dxf.check.insert_undefined_block",
                    At::Address(doc.entity_address(i)),
                    format!("a block reference to {b:?}, which no block defines"),
                    Some("block list"),
                );
            }
        }
        if doc.layer_named(&e.layer).is_none() {
            f.add(
                "warning",
                "dxf.check.layer_undefined",
                At::Address(doc.entity_address(i)),
                format!(
                    "on layer {:?}, which the layer table doesn't define",
                    e.layer
                ),
                Some("layer list"),
            );
        }
        for x in &e.xd {
            if !apps.contains(&x.to_ascii_uppercase()) {
                f.add(
                    "error",
                    "dxf.check.appid_missing",
                    At::Address(doc.entity_address(i)),
                    format!(
                        "extension data under {x:?}, which the application table doesn't register"
                    ),
                    None,
                );
            }
        }
    }
    // tables
    for t in &s.tables {
        let stated = (t.head.first + 1..t.head.end)
            .find(|&j| s.tok.pairs[j].code == 70)
            .and_then(|j| s.text(j).trim().parse::<usize>().ok());
        if let Some(n) = stated {
            if n != t.entries.len() {
                f.add(
                    "info",
                    "dxf.check.table_count",
                    At::Offset(s.tok.pairs[t.head.first].start()),
                    format!(
                        "table {} says {n} entries and holds {}",
                        t.name,
                        t.entries.len()
                    ),
                    None,
                );
            }
        }
    }
    // header
    let ext = reads::extents(doc);
    if ext.get("header_sentinel") == Some(&Value::Bool(true)) {
        f.add(
            "info",
            "dxf.check.extents_sentinel",
            At::None,
            "the header's extents are unset (the sentinel values)".into(),
            Some("drawing info gives the computed extents"),
        );
    } else if ext.get("header_stale") == Some(&Value::Bool(true)) {
        f.add(
            "info",
            "dxf.check.extents_stale",
            At::None,
            "the header's extents don't contain the drawing".into(),
            Some("drawing info gives the computed extents"),
        );
    }
    if doc.version.as_str() >= "AC1015" && doc.units.insunits.is_none_or(|u| u == 0) {
        f.add(
            "info",
            "dxf.check.unitless",
            At::None,
            "the drawing states no units: geometry edits will need --unit drawing".into(),
            None,
        );
    }
    // text in a code page this reader doesn't decode: shown as code page 1252, never guessed
    if doc.version.as_str() < "AC1021"
        && !crate::token::CODE_PAGES_READ.contains(&doc.encoding.as_str())
    {
        let high = s
            .tok
            .pairs
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.code, 1 | 3))
            .filter(|(i, _)| s.tok.value(*i).iter().any(|b| *b >= 0x80))
            .count();
        if high > 0 {
            f.add(
                "warning",
                "dxf.check.code_page_unread",
                At::None,
                format!(
                    "{high} text values hold characters of code page {}, which this reader doesn't decode: they are shown as code page 1252",
                    doc.encoding
                ),
                Some("the edit verbs keep such text byte for byte"),
            );
        }
    }
    let writeback = if args.has("--writeback") {
        let identical = s.tok.reemit() == s.tok.bytes;
        Value::obj([
            ("identical", Value::from(identical)),
            ("bytes", Value::from(s.tok.bytes.len())),
            ("editable", reads::editable(doc)),
        ])
    } else {
        Value::Null
    };
    let [e, w, i] = f.counts;
    let mut text = f.text;
    text.push_str(&format!("{e} errors, {w} warnings, {i} notes\n"));
    Ok(ReadOut {
        stdout: text,
        data: Value::obj([
            ("findings", Value::List(f.rows)),
            (
                "counts",
                Value::obj([
                    ("error", Value::from(e)),
                    ("warning", Value::from(w)),
                    ("info", Value::from(i)),
                ]),
            ),
            (
                "by_code",
                Value::obj(f.per.iter().map(|(code, n)| (*code, Value::from(*n)))),
            ),
            ("writeback", writeback),
        ]),
        spec: ("dxf.check", 1),
        next: None,
    })
}
