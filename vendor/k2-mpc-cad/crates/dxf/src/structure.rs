//! The structure of a DXF file over its tokens: sections, header variables, tables and their
//! entries, block definitions, entities with their followers, and objects, each by its range
//! of pair indices. Nothing is interpreted beyond the words that delimit them.
//!
//! **A unit** is a `0` pair and the pairs up to the next `0`. **An entity** is a unit plus its
//! followers: the ATTRIBs after an INSERT, the VERTEXes after a POLYLINE, and the SEQEND that
//! ends them. Followers move, colour and delete with their parent, so they are never entities
//! of their own.

use std::borrow::Cow;

use crate::error::{CadError, OVER_BOUND};
use crate::token::Tokens;
use crate::{mutant, Limits};

/// Pairs `first..end` (indices into [`Tokens::pairs`]), the first a `0` pair naming `kind`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    /// Its kind, upper case: a static word for the kinds this reader knows, else its own copy.
    pub kind: Cow<'static, str>,
    pub first: usize,
    pub end: usize,
}

/// A section: pairs `first..end`, from its `0`/`SECTION` to its `0`/`ENDSEC` included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub name: String,
    pub first: usize,
    pub end: usize,
    /// It ends with `ENDSEC`.
    pub closed: bool,
}

/// A header variable: its `9` pair and the pairs after it, up to the next variable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderVar {
    pub name: String,
    pub first: usize,
    pub end: usize,
}

/// A table: its head unit (`0`/`TABLE`), its entries, and whether `ENDTAB` closed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub head: Unit,
    pub entries: Vec<Unit>,
    pub closed: bool,
}

/// Where an entity is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// In the ENTITIES section.
    Entities,
    /// In block definition `n` of [`Structure::blocks`].
    Block(usize),
}

/// An entity and its followers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityUnit {
    pub head: Unit,
    /// ATTRIBs or VERTEXes, then the SEQEND.
    pub followers: Vec<Unit>,
    pub place: Place,
}

/// A block definition: its `BLOCK` unit, its entities (indices into
/// [`Structure::entities`]) and its `ENDBLK` unit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockDef {
    pub head: Unit,
    pub ents: Vec<usize>,
    pub endblk: Option<Unit>,
}

/// A file's structure.
#[derive(Debug, Clone)]
pub struct Structure<'a> {
    pub tok: Tokens<'a>,
    pub sections: Vec<Section>,
    pub header: Vec<HeaderVar>,
    pub tables: Vec<Table>,
    pub blocks: Vec<BlockDef>,
    pub entities: Vec<EntityUnit>,
    pub objects: Vec<Unit>,
    /// Comments (`999`) in the order they appear: pair indices.
    pub comments: Vec<usize>,
}

/// The kinds of unit a drawing commonly holds: a unit of one of these keeps the static word.
const KINDS: &[&str] = &[
    "LINE",
    "POINT",
    "CIRCLE",
    "ARC",
    "LWPOLYLINE",
    "POLYLINE",
    "VERTEX",
    "SEQEND",
    "ATTRIB",
    "ATTDEF",
    "INSERT",
    "TEXT",
    "MTEXT",
    "ELLIPSE",
    "SPLINE",
    "SOLID",
    "TRACE",
    "3DFACE",
    "HATCH",
    "DIMENSION",
    "LEADER",
    "MLEADER",
    "MULTILEADER",
    "VIEWPORT",
    "IMAGE",
    "WIPEOUT",
    "REGION",
    "3DSOLID",
    "BODY",
    "RAY",
    "XLINE",
    "TOLERANCE",
    "MLINE",
    "SHAPE",
    "BLOCK",
    "ENDBLK",
    "TABLE",
    "ENDTAB",
    "LAYER",
    "LTYPE",
    "STYLE",
    "VIEW",
    "UCS",
    "VPORT",
    "APPID",
    "DIMSTYLE",
    "BLOCK_RECORD",
    "DICTIONARY",
    "LAYOUT",
    "XRECORD",
    "GROUP",
    "SORTENTSTABLE",
];

/// A unit's kind, upper case: the static word when it is a known kind.
fn known(t: &str) -> Cow<'static, str> {
    let up = t.to_ascii_uppercase();
    match KINDS.iter().find(|k| **k == up) {
        Some(k) => Cow::Borrowed(k),
        None => Cow::Owned(up),
    }
}

/// The units of pairs `from..to`: each `0` pair starts one. Pairs before the first `0` belong
/// to none.
fn units(t: &Tokens<'_>, from: usize, to: usize) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::new();
    for i in from..to {
        if t.pairs[i].code == 0 {
            if let Some(u) = out.last_mut() {
                u.end = i;
            }
            out.push(Unit {
                kind: known(&t.text(i)),
                first: i,
                end: to,
            });
        }
    }
    out
}

impl<'a> Structure<'a> {
    /// A pair's value as text, trimmed.
    pub fn text(&self, i: usize) -> std::borrow::Cow<'a, str> {
        self.tok.text(i)
    }

    /// The header variable named `name` (case-insensitive, `$` included).
    pub fn var(&self, name: &str) -> Option<&HeaderVar> {
        self.header
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// The first value of header variable `name` with code `code`.
    pub fn var_value(&self, name: &str, code: i32) -> Option<std::borrow::Cow<'a, str>> {
        let v = self.var(name)?;
        (v.first + 1..v.end)
            .find(|&i| self.tok.pairs[i].code == code)
            .map(|i| self.text(i))
    }

    /// The table named `name` (case-insensitive).
    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables
            .iter()
            .find(|t| t.name.eq_ignore_ascii_case(name))
    }
}

/// The structure of `t`.
pub fn walk<'a>(t: Tokens<'a>, limits: &Limits) -> Result<Structure<'a>, CadError> {
    let n = t.pairs.len();
    let mut s = Structure {
        tok: t,
        sections: Vec::new(),
        header: Vec::new(),
        tables: Vec::new(),
        blocks: Vec::new(),
        entities: Vec::new(),
        objects: Vec::new(),
        comments: Vec::new(),
    };
    for i in 0..n {
        if s.tok.pairs[i].code == 999 {
            s.comments.push(i);
        }
    }
    let mut i = 0;
    while i < n {
        if !s.tok.is0(i, "SECTION") {
            i += 1;
            continue;
        }
        let name = if i + 1 < n && s.tok.pairs[i + 1].code == 2 {
            s.tok.text(i + 1).to_ascii_uppercase()
        } else {
            String::new()
        };
        // the section runs to its ENDSEC, or to the next SECTION or EOF when it has none
        let mut j = i + 1;
        let mut closed = false;
        while j < n {
            if s.tok.is0(j, "ENDSEC") {
                closed = true;
                break;
            }
            if s.tok.is0(j, "SECTION") || s.tok.is0(j, "EOF") {
                break;
            }
            j += 1;
        }
        let body_from = if name.is_empty() { i + 1 } else { i + 2 };
        let body_to = j.min(n);
        match name.as_str() {
            "HEADER" => header(&mut s, body_from, body_to),
            "TABLES" => tables(&mut s, body_from, body_to),
            "BLOCKS" => blocks(&mut s, body_from, body_to, limits)?,
            "ENTITIES" => {
                let us = units(&s.tok, body_from, body_to);
                entities(&mut s, us, Place::Entities, limits)?;
            }
            "OBJECTS" => s.objects = units(&s.tok, body_from, body_to),
            _ => {}
        }
        let end = if closed { j + 1 } else { j };
        s.sections.push(Section {
            name,
            first: i,
            end,
            closed,
        });
        i = end;
    }
    Ok(s)
}

fn header(s: &mut Structure<'_>, from: usize, to: usize) {
    let mut cur: Option<HeaderVar> = None;
    for i in from..to {
        if s.tok.pairs[i].code == 9 {
            if let Some(mut v) = cur.take() {
                v.end = i;
                s.header.push(v);
            }
            cur = Some(HeaderVar {
                name: s.tok.text(i).into_owned(),
                first: i,
                end: to,
            });
        }
    }
    if let Some(v) = cur {
        s.header.push(v);
    }
}

fn tables(s: &mut Structure<'_>, from: usize, to: usize) {
    let mut cur: Option<Table> = None;
    for u in units(&s.tok, from, to) {
        match &*u.kind {
            "TABLE" => {
                if let Some(t) = cur.take() {
                    s.tables.push(t);
                }
                let name = (u.first + 1..u.end)
                    .find(|&i| s.tok.pairs[i].code == 2)
                    .map(|i| s.tok.text(i).to_ascii_uppercase())
                    .unwrap_or_default();
                cur = Some(Table {
                    name,
                    head: u,
                    entries: Vec::new(),
                    closed: false,
                });
            }
            "ENDTAB" => {
                if let Some(mut t) = cur.take() {
                    t.closed = true;
                    s.tables.push(t);
                }
            }
            _ => {
                if let Some(t) = cur.as_mut() {
                    t.entries.push(u);
                }
            }
        }
    }
    if let Some(t) = cur {
        s.tables.push(t);
    }
}

fn blocks(s: &mut Structure<'_>, from: usize, to: usize, limits: &Limits) -> Result<(), CadError> {
    let mut cur: Option<(Unit, Vec<Unit>)> = None;
    for u in units(&s.tok, from, to) {
        match &*u.kind {
            "BLOCK" => {
                if let Some((head, body)) = cur.take() {
                    close_block(s, head, body, None, limits)?;
                }
                cur = Some((u, Vec::new()));
            }
            "ENDBLK" => {
                if let Some((head, body)) = cur.take() {
                    close_block(s, head, body, Some(u), limits)?;
                }
            }
            _ => {
                if let Some((_, body)) = cur.as_mut() {
                    body.push(u);
                }
            }
        }
    }
    if let Some((head, body)) = cur {
        close_block(s, head, body, None, limits)?;
    }
    Ok(())
}

fn close_block(
    s: &mut Structure<'_>,
    head: Unit,
    body: Vec<Unit>,
    endblk: Option<Unit>,
    limits: &Limits,
) -> Result<(), CadError> {
    let b = s.blocks.len();
    let first = s.entities.len();
    entities(s, body, Place::Block(b), limits)?;
    let ents = (first..s.entities.len()).collect();
    s.blocks.push(BlockDef { head, ents, endblk });
    Ok(())
}

/// Groups units into entities with their followers.
fn entities(
    s: &mut Structure<'_>,
    us: Vec<Unit>,
    place: Place,
    limits: &Limits,
) -> Result<(), CadError> {
    let as_nodes = mutant::active("FollowersAsNodes");
    // the parent still taking followers: its index in s.entities
    let mut open: Option<usize> = None;
    for u in us {
        let follower = matches!(&*u.kind, "ATTRIB" | "VERTEX" | "SEQEND");
        if follower && !as_nodes {
            if let Some(p) = open {
                let seqend = u.kind == "SEQEND";
                s.entities[p].followers.push(u);
                let e = &s.entities[p];
                let pairs = e.followers.last().map_or(0, |l| l.end) - e.head.first;
                if pairs > limits.max_entity_pairs {
                    return Err(CadError::new(
                        OVER_BOUND,
                        format!(
                            "an entity over the limit of {} pairs",
                            limits.max_entity_pairs
                        ),
                        "the drawing stays a plain file",
                    )
                    .at(s.tok.pairs[e.head.first].start()));
                }
                if seqend {
                    open = None;
                }
                continue;
            }
        }
        if u.end - u.first > limits.max_entity_pairs {
            return Err(CadError::new(
                OVER_BOUND,
                format!(
                    "an entity over the limit of {} pairs",
                    limits.max_entity_pairs
                ),
                "the drawing stays a plain file",
            )
            .at(s.tok.pairs[u.first].start()));
        }
        let parent = matches!(&*u.kind, "INSERT" | "POLYLINE");
        s.entities.push(EntityUnit {
            head: u,
            followers: Vec::new(),
            place,
        });
        open = parent.then(|| s.entities.len() - 1);
    }
    Ok(())
}
