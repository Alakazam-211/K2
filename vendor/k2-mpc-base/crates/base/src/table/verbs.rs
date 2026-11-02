//! The grid verbs (CSV contract §3.1) for any table pack (A9): [`VERBS`], listed by a pack's
//! [`crate::session::Pack::verbs`]. Engine-free and codec-free: each verb computes on the doc
//! as a [`crate::session::View`] ([`VerbCtx`]) and returns its edits by key; the pack's codec
//! is reached only through the context (a pasted block, rendered lines).
//!
//! **Addressing** (§1.7), in the kit's one grammar ([`crate::addr`]), resolved when the verb
//! runs, against the live table:
//! - rows: a key (`r17`, or `@k:r17` / `key:r17`: follows moves and sorts), `#N` (the Nth live
//!   row), `LN` (the row rendered line N belongs to), or `--where COL=VALUE` (the one live row
//!   whose cell in COL is VALUE);
//! - columns: a key (`c3`), `#N`, a header name (`email`), or forced: `@k:c3` / `key:c3` /
//!   `name:c3`. A bare word that is a known column key is the key; otherwise a name. A bare
//!   number is a key or a name, never a position (`#N` is).
//!
//! **Exits** (§3.3): an unknown or deleted element, `#N` past the end, a `--where` with no
//! match, a sort column that doesn't exist: **4** (nothing applies). An ambiguous name or
//! `--where`: **2**, `MPC-E170 ambiguous_address`, the line listing the keys and
//! `details.candidates` each one's `@k:` address and `#N` (K-ADDR-2; never the first match).
//! More values than columns, a bad value (NUL, over 64 KiB), renaming with the header off, a
//! bad flag: **2**. `--if-version`, `--base` and the claim
//! policy are the kit's (taken out of the arguments before these see them).

use std::collections::HashSet;
use std::fmt::Write as _;

use super::{Axis, LineOwner, SortKey, Table, TableError, ROWS};
use crate::addr::{self, Addr, Forms, Unique};
use crate::doc::Edit;
use crate::elem::{Anchor, ElemState, Facet, Key};
use crate::parity::PersonAction as P;
use crate::session::ClaimSpan;
use crate::tools::{PackVerb, StructEdit, StructOutput, ToolError, VerbCtx};

const EDIT_EXITS: &[u8] = &[0, 1, 2, 4];
const READ_EXITS: &[u8] = &[0, 2, 4];

/// Every grid verb, for a table pack's [`crate::session::Pack::verbs`].
pub const VERBS: &[PackVerb] = &[
    PackVerb::structured_read(
        "cell get",
        "cell get --row R --col C [--json]",
        &[P::ReadAll],
        READ_EXITS,
        cell_get,
    ),
    PackVerb::structured_edit(
        "cell set",
        "cell set --row R --col C (--value V | stdin)",
        &[P::Type, P::TypeOver, P::FindReplace],
        EDIT_EXITS,
        cell_set,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "cells set",
        "cells set --from R,C (stdin: a block in the doc's dialect; rows past the end are added)",
        &[P::Paste],
        EDIT_EXITS,
        cells_set,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "cells clear",
        "cells clear --from R,C --to R,C",
        &[P::Delete],
        EDIT_EXITS,
        cells_clear,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "row insert",
        "row insert (--at #N | --after R | --before R | --start | --end) [--values a,b,... | stdin] (prints its key)",
        &[P::RowInsert],
        EDIT_EXITS,
        row_insert,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "row move",
        "row move --row R (--after R2 | --before R2 | --to #N | --start | --end)",
        &[P::RowMove],
        EDIT_EXITS,
        row_move,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "row delete",
        "row delete --row R",
        &[P::RowDelete, P::Delete],
        EDIT_EXITS,
        row_delete,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "row restore",
        "row restore --row R",
        &[P::RestoreElement],
        EDIT_EXITS,
        row_restore,
    )
    .positional(positional),
    PackVerb::structured_read(
        "row list",
        "row list [--cols C,...] [--where C=V] [-n N]",
        &[P::ReadAll],
        READ_EXITS,
        row_list,
    ),
    PackVerb::structured_edit(
        "col insert",
        "col insert (--at #N | --after C | --before C | --start | --end) [--name NAME] (prints its key)",
        &[P::ColInsert],
        EDIT_EXITS,
        col_insert,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "col move",
        "col move --col C (--after C2 | --before C2 | --to #N | --start | --end)",
        &[P::ColMove],
        EDIT_EXITS,
        col_move,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "col delete",
        "col delete --col C",
        &[P::ColDelete],
        EDIT_EXITS,
        col_delete,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "col restore",
        "col restore --col C",
        &[P::RestoreElement],
        EDIT_EXITS,
        col_restore,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "col rename",
        "col rename --col C --name NAME",
        &[P::ColRename],
        EDIT_EXITS,
        col_rename,
    )
    .positional(positional),
    PackVerb::structured_read("col list", "col list", &[P::ReadAll], READ_EXITS, col_list),
    PackVerb::structured_edit(
        "sort",
        "sort --by C [--desc] [--numeric] [--then C2 [--desc] [--numeric] ...]",
        &[P::Sort],
        EDIT_EXITS,
        sort,
    )
    .positional(positional),
    PackVerb::structured_edit(
        "header",
        "header on|off",
        &[P::ColRename],
        EDIT_EXITS,
        header,
    )
    .positional(positional),
];

// ---------- addressing ----------

/// A row as a verb names it (§1.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowAt {
    Key(Key),
    /// The Nth live row (1-based).
    Nth(usize),
    /// The row rendered line N (1-based) belongs to.
    Line(usize),
    /// The one live row whose cell in the column is the value.
    Where(ColAt, String),
}

/// A column as a verb names it (§1.7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ColAt {
    Key(Key),
    Nth(usize),
    Name(String),
    /// A bare word: the column key if it is one, else a header name.
    Word(String),
}

/// A refusal while resolving or running: the exit (2 or 4) and its line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    pub exit: u8,
    pub message: String,
    /// A deleted element the refusal names (root, key): the run says who deleted it.
    deleted: Option<(&'static str, Key)>,
    /// Its own error id, when it has one (a refusal with one is reported, not a usage error).
    entry: Option<&'static crate::errors::ErrorEntry>,
    /// A refusal with its own report and details (an ambiguous address, `MPC-E170`): exit 2,
    /// returned as [`ToolError::Address`].
    report: Option<Box<crate::errors::ErrorReport>>,
}

fn refusal(exit: u8, message: impl Into<String>) -> Refusal {
    Refusal {
        exit,
        message: message.into(),
        deleted: None,
        entry: None,
        report: None,
    }
}

/// An ambiguous name or `--where` (K-ADDR-2): exit 2 with the keys listed, and each match's
/// `@k:` address and `#N` among the live elements in `details.candidates`.
fn ambiguous(what: &str, given: &str, axis: &Axis, many: &[Key], message: String) -> Refusal {
    let candidates: Vec<addr::Candidate> = many
        .iter()
        .map(|k| {
            let c = addr::Candidate::new(Addr::Key(k.to_string()).to_string());
            match axis.live().position(|x| x == k) {
                Some(i) => c.at(format!("#{}", i + 1)),
                None => c,
            }
        })
        .collect();
    Refusal {
        report: Some(Box::new(addr::ambiguous(
            message.clone(),
            what,
            given,
            &candidates,
        ))),
        ..refusal(2, message)
    }
}

fn refuse<T>(exit: u8, message: impl Into<String>) -> Result<T, Refusal> {
    Err(refusal(exit, message))
}

impl Refusal {
    /// The line the verb prints: a deleted element's refusal names who deleted it and in
    /// which change, when the caller may read history (`row r9 is deleted (by ana, v12);
    /// row restore --row r9`).
    fn line(&self, ctx: &VerbCtx<'_>) -> String {
        let Some((root, key)) = &self.deleted else {
            return self.message.clone();
        };
        match ctx.last_write(root, key.as_str(), Facet::Existence) {
            Some(b) if b.deleted => self.message.replacen(
                " is deleted",
                &format!(" is deleted (by {}, v{})", b.author, b.change),
                1,
            ),
            _ => self.message.clone(),
        }
    }
}

fn number(s: &str, what: &str) -> Result<usize, ToolError> {
    s.parse::<usize>()
        .ok()
        .filter(|n| *n > 0)
        .ok_or_else(|| ToolError::usage(format!("{what}: not a number from 1: {s:?}")))
}

fn key_of(s: &str) -> Result<Key, ToolError> {
    Key::new(s).map_err(|e| ToolError::usage(format!("not a key: {s:?} ({e})")))
}

/// The forms a row address takes ([`crate::addr`]): keys, `#N` and `LN`; no native IDs.
const ROW_FORMS: Forms = Forms::none().nth().line();
/// The forms a column address takes: keys, `#N`, names; no lines, no native IDs (a header
/// named `@timestamp` or `L3` is a name).
const COL_FORMS: Forms = Forms::none().nth();

fn addr_err(e: crate::addr::AddrError) -> ToolError {
    ToolError::usage(e.to_string())
}

/// A row address: `#N`, `LN`, or a key (`r17`, `@k:r17`, `key:r17`).
pub fn parse_row(s: &str) -> Result<RowAt, ToolError> {
    match addr::parse(s, ROW_FORMS).map_err(addr_err)? {
        Addr::Nth(n) => Ok(RowAt::Nth(n)),
        Addr::Line(n) => Ok(RowAt::Line(n)),
        Addr::Key(k) => Ok(RowAt::Key(key_of(&k)?)),
        Addr::Name(_) => Err(ToolError::usage(format!(
            "rows have no names: {s:?} (a row is a key, #N or LN)"
        ))),
        Addr::Word(w) => Ok(RowAt::Key(key_of(&w)?)),
        Addr::Native(_) => unreachable!("rows read no native ids"),
    }
}

/// A column address: `#N`, `@k:K` / `key:K`, `name:N`, or a bare word.
pub fn parse_col(s: &str) -> Result<ColAt, ToolError> {
    if s.is_empty() {
        return Err(ToolError::usage("an empty column"));
    }
    match addr::parse(s, COL_FORMS).map_err(addr_err)? {
        Addr::Nth(n) => Ok(ColAt::Nth(n)),
        Addr::Key(k) => Ok(ColAt::Key(key_of(&k)?)),
        Addr::Name(n) => Ok(ColAt::Name(n)),
        Addr::Word(w) => Ok(ColAt::Word(w)),
        Addr::Line(_) | Addr::Native(_) => unreachable!("columns read no lines or native ids"),
    }
}

/// `--where COL=VALUE`.
pub fn parse_where(s: &str) -> Result<RowAt, ToolError> {
    let (c, v) = s
        .split_once('=')
        .ok_or_else(|| ToolError::usage(format!("--where: not COL=VALUE: {s:?}")))?;
    Ok(RowAt::Where(parse_col(c)?, v.to_owned()))
}

/// Which elements a resolution accepts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Want {
    /// A live element (every verb but restore).
    Live,
    /// A deleted one may be named by key (restore; `#N` and lines name live ones only).
    AnyKnown,
}

fn keys_text(keys: &[Key]) -> String {
    keys.iter().map(Key::as_str).collect::<Vec<_>>().join(", ")
}

fn by_key(t: &Table, root: &'static str, k: &Key, want: Want) -> Result<Key, Refusal> {
    let (axis, what, restore) = if root == ROWS {
        (&t.rows, "row", "row restore --row")
    } else {
        (&t.cols, "column", "col restore --col")
    };
    match axis.state(k) {
        ElemState::Unknown => refuse(4, format!("{what} {k} is unknown")),
        ElemState::Deleted if want == Want::Live => Err(Refusal {
            deleted: Some((root, k.clone())),
            ..refusal(4, format!("{what} {k} is deleted; {restore} {k}"))
        }),
        _ => Ok(k.clone()),
    }
}

/// `col` in `t` (`pos`: where `#N` counts, K-ADDR-3).
fn resolve_col(t: &Table, col: &ColAt, want: Want, pos: &Pos<'_, '_>) -> Result<Key, Refusal> {
    match col {
        ColAt::Key(k) => by_key(t, super::COLS, k, want),
        ColAt::Nth(n) => match pos.table(t).cols.live().nth(n - 1) {
            Some(k) => pos.live(t, super::COLS, k, want),
            None => refuse(
                4,
                format!(
                    "there is no column #{n} ({} live columns)",
                    pos.table(t).cols.live_len()
                ),
            ),
        },
        ColAt::Name(n) => by_name(t, n),
        ColAt::Word(w) => match Key::new(w.as_str()) {
            Ok(k) if t.cols.knows(&k) => by_key(t, super::COLS, &k, want),
            _ => by_name(t, w),
        },
    }
}

fn by_name(t: &Table, name: &str) -> Result<Key, Refusal> {
    if !t.header {
        return refuse(
            4,
            format!("no column {name:?}: the header is off (name columns by key or #N)"),
        );
    }
    let live: Vec<Key> = t
        .cols_named(name)
        .into_iter()
        .filter(|k| t.cols.is_live(k))
        .collect();
    match addr::unique(live) {
        Unique::None => refuse(4, format!("no column named {name:?}")),
        Unique::One(k) => Ok(k),
        Unique::Many(many) => Err(ambiguous(
            "column",
            name,
            &t.cols,
            &many,
            format!(
                "{} columns are named {name:?}: {} (name one by key)",
                many.len(),
                keys_text(&many)
            ),
        )),
    }
}

/// `row` in `t` (`pos`: where `#N` and `LN` count, K-ADDR-3).
fn resolve_row(t: &Table, row: &RowAt, want: Want, pos: &Pos<'_, '_>) -> Result<Key, Refusal> {
    match row {
        RowAt::Key(k) => by_key(t, ROWS, k, want),
        RowAt::Nth(n) => match pos.table(t).rows.live().nth(n - 1) {
            Some(k) => pos.live(t, ROWS, k, want),
            None => refuse(
                4,
                format!(
                    "there is no row #{n} ({} live rows)",
                    pos.table(t).rows.live_len()
                ),
            ),
        },
        RowAt::Line(n) => {
            let map = pos.line_map(t);
            match map.owner(*n) {
                Some(LineOwner::Row(k)) => pos.live(t, ROWS, k, want),
                Some(LineOwner::Header) => refuse(4, format!("line {n} is the header, not a row")),
                Some(LineOwner::Preamble) => refuse(
                    4,
                    format!("line {n} is the file's preamble (the codec's own line), not a row"),
                ),
                None => refuse(
                    4,
                    format!("there is no line {n} ({} lines)", map.lines.len()),
                ),
            }
        }
        RowAt::Where(col, value) => {
            let c = resolve_col(t, col, Want::Live, pos)?;
            let hits: Vec<Key> = t
                .rows
                .live()
                .filter(|r| t.cell(r, &c).unwrap_or("") == value)
                .cloned()
                .collect();
            match addr::unique(hits) {
                Unique::None => refuse(4, format!("no row where {c} is {value:?}")),
                Unique::One(k) => Ok(k),
                Unique::Many(many) => Err(ambiguous(
                    "row",
                    &format!("{c}={value}"),
                    &t.rows,
                    &many,
                    format!(
                        "{} rows where {c} is {value:?}: {} (name one by key)",
                        many.len(),
                        keys_text(&many)
                    ),
                )),
            }
        }
    }
}

// ---------- flags ----------

fn flag(args: &[String], f: &str) -> Option<String> {
    args.iter()
        .position(|a| a == f)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn has(args: &[String], f: &str) -> bool {
    args.iter().any(|a| a == f)
}

fn need(args: &[String], f: &str) -> Result<String, ToolError> {
    flag(args, f).ok_or_else(|| ToolError::usage(format!("{f} is required")))
}

/// The flags a verb knows; anything else is exit 2 (a typo never runs a different verb).
fn known(args: &[String], with_value: &[&str], bare: &[&str]) -> Result<(), ToolError> {
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        if with_value.contains(&a) {
            if i + 1 >= args.len() {
                return Err(ToolError::usage(format!("{a} needs a value")));
            }
            i += 2;
        } else if bare.contains(&a) {
            i += 1;
        } else {
            return Err(ToolError::usage(format!("unknown argument {a:?}")));
        }
    }
    Ok(())
}

/// The row a verb names: `--row R`, or `--where COL=VALUE` (one of them).
fn row_arg(args: &[String]) -> Result<RowAt, ToolError> {
    match (flag(args, "--row"), flag(args, "--where")) {
        (Some(r), None) => parse_row(&r),
        (None, Some(w)) => parse_where(&w),
        (Some(_), Some(_)) => Err(ToolError::usage("--row or --where, not both")),
        (None, None) => Err(ToolError::usage("--row (or --where COL=VALUE) is required")),
    }
}

/// `R,C`: a row and a column address.
fn cell_arg(s: &str) -> Result<(RowAt, ColAt), ToolError> {
    let (r, c) = s
        .split_once(',')
        .ok_or_else(|| ToolError::usage(format!("not R,C: {s:?}")))?;
    Ok((parse_row(r)?, parse_col(c)?))
}

/// Where a new or moved element goes, by its addresses (`--at #N`/`--to #N` place it to be
/// the Nth; past the end is the end).
#[derive(Debug, Clone)]
enum Place<A> {
    Start,
    End,
    After(A),
    Before(A),
    Nth(usize),
}

fn place<A>(
    args: &[String],
    nth_flag: &str,
    parse: fn(&str) -> Result<A, ToolError>,
) -> Result<Place<A>, ToolError> {
    let picked: Vec<&str> = ["--after", "--before", nth_flag, "--start", "--end"]
        .into_iter()
        .filter(|f| has(args, f))
        .collect();
    if picked.len() != 1 {
        return Err(ToolError::usage(format!(
            "where? one of --after, --before, {nth_flag}, --start or --end"
        )));
    }
    Ok(match picked[0] {
        "--after" => Place::After(parse(&need(args, "--after")?)?),
        "--before" => Place::Before(parse(&need(args, "--before")?)?),
        "--start" => Place::Start,
        "--end" => Place::End,
        _ => {
            let n = need(args, nth_flag)?;
            let n = n
                .strip_prefix('#')
                .ok_or_else(|| ToolError::usage(format!("{nth_flag} takes #N: {n:?}")))?;
            Place::Nth(number(n, nth_flag)?)
        }
    })
}

/// A place as an anchor in `live` (the axis's live keys where `#N` counts, the moving
/// element left out); `check` is what the Nth key must pass in the live table (K-ADDR-3: when
/// `#N` counted in the `--base` version, that element must still be live).
fn anchor_of<A>(
    p: &Place<A>,
    live: &[Key],
    resolve: impl Fn(&A) -> Result<Key, Refusal>,
    check: impl Fn(&Key) -> Result<Key, Refusal>,
) -> Result<Anchor, Refusal> {
    Ok(match p {
        Place::Start => Anchor::Start,
        Place::End => Anchor::End,
        Place::After(a) => Anchor::After(resolve(a)?),
        Place::Before(a) => Anchor::Before(resolve(a)?),
        Place::Nth(n) => match live.get(n - 1) {
            Some(k) => Anchor::Before(check(k)?),
            None => Anchor::End,
        },
    })
}

// ---------- running ----------

/// A verb's work on the table: edits and stdout, or a refusal.
type Work = Result<(Vec<Edit>, String), Refusal>;

/// Run `f` on the context's table; a refusal is its exit with nothing applied (exit 2 as a
/// usage error, 4 on stdout), a table error mapped to its exit.
fn run(f: impl FnOnce(&VerbCtx<'_>, &mut Table) -> Work + 'static) -> StructEdit {
    Box::new(move |ctx: &VerbCtx<'_>| {
        let mut t = ctx
            .view()
            .table()
            .map_err(|e| ToolError::usage(e.to_string()))?;
        match f(ctx, &mut t) {
            Ok((edits, out)) => Ok(StructOutput::new(edits, out.into_bytes(), 0)),
            Err(Refusal {
                report: Some(rep), ..
            }) => Err(ToolError::address(*rep)),
            Err(r) if r.exit == 2 && r.entry.is_none() => Err(ToolError::usage(r.message)),
            Err(r) => {
                let mut o = StructOutput::new(
                    Vec::new(),
                    format!("{}\n", r.line(ctx)).into_bytes(),
                    r.exit,
                );
                o.refusal = r.entry;
                Ok(o)
            }
        }
    })
}

/// A table op's error as a refusal (§3.3): unknown, deleted, or a missing sort column → 4;
/// anything else (a bad value, too many values, the header off) → 2.
/// [`run`] for a read verb that shows rows or cells by key: `--for-edit` claims them
/// ([`StructOutput::shown`]).
fn run_read(
    f: impl FnOnce(&VerbCtx<'_>, &Table) -> Result<(String, Option<ClaimSpan>), Refusal> + 'static,
) -> StructEdit {
    Box::new(move |ctx: &VerbCtx<'_>| {
        let t = ctx
            .view()
            .table()
            .map_err(|e| ToolError::usage(e.to_string()))?;
        match f(ctx, &t) {
            Ok((out, shown)) => {
                let o = StructOutput::new(Vec::new(), out.into_bytes(), 0);
                Ok(match shown {
                    Some(s) => o.showing(s),
                    None => o,
                })
            }
            Err(Refusal {
                report: Some(rep), ..
            }) => Err(ToolError::address(*rep)),
            Err(r) if r.exit == 2 && r.entry.is_none() => Err(ToolError::usage(r.message)),
            Err(r) => Ok(StructOutput::new(
                Vec::new(),
                format!("{}\n", r.line(ctx)).into_bytes(),
                r.exit,
            )),
        }
    })
}

fn table_err(e: TableError) -> Refusal {
    let exit = match &e {
        TableError::Unknown { .. } | TableError::Deleted { .. } => 4,
        _ => 2,
    };
    let deleted = match &e {
        TableError::Deleted { root, key } => Some((*root, key.clone())),
        _ => None,
    };
    Refusal {
        deleted,
        ..refusal(exit, e.to_string())
    }
}

/// Where a verb's positional addresses count (K-ADDR-3, R-1): an edit's `--base` version
/// when the kit gives it ([`VerbCtx::positions_view`]), else the live table. Keys and names
/// always resolve in the live table; an element `#N` or `LN` names at the base must still be
/// live (or known, for restore) there.
struct Pos<'c, 'a> {
    ctx: &'c VerbCtx<'a>,
    /// The base version's table, when positions count there.
    base: Option<Table>,
}

impl Pos<'_, '_> {
    /// The table positions count in.
    fn table<'t>(&'t self, live: &'t Table) -> &'t Table {
        self.base.as_ref().unwrap_or(live)
    }

    /// The rendered lines `LN` counts in.
    fn line_map(&self, live: &Table) -> super::LineMap {
        self.ctx
            .positions_line_map()
            .unwrap_or_else(|| self.table(live).line_map())
    }

    /// `k`, found by position, as the live table has it: itself at live; at a base, checked
    /// as a key is.
    fn live(&self, t: &Table, root: &'static str, k: &Key, want: Want) -> Result<Key, Refusal> {
        if self.base.is_some() {
            by_key(t, root, k, want)
        } else {
            Ok(k.clone())
        }
    }
}

fn lines_of<'c, 'a>(ctx: &'c VerbCtx<'a>) -> Result<Pos<'c, 'a>, Refusal> {
    let base = match ctx.positions_at() {
        Some(v) => Some(v.table().map_err(|e| refusal(2, e.to_string()))?),
        None => None,
    };
    Ok(Pos { ctx, base })
}

/// A grid verb's positional address (K-ADDR-3; [`PackVerb::positional`]): the first `#N`
/// or `LN` among its address flags' values (a cell's `R,C` and `--where`'s column
/// included), as given.
fn positional(args: &[String]) -> Option<String> {
    const FLAGS: &[&str] = &[
        "--row", "--col", "--from", "--to", "--at", "--after", "--before", "--by", "--then",
        "--where",
    ];
    let mut it = args.iter();
    while let Some(a) = it.next() {
        if !FLAGS.contains(&a.as_str()) {
            continue;
        }
        let Some(v) = it.next() else { break };
        let addr = if a == "--where" {
            v.split_once('=').map_or(v.as_str(), |(c, _)| c)
        } else {
            v.as_str()
        };
        if addr
            .split(',')
            .any(|x| addr::parse(x, ROW_FORMS).is_ok_and(|p| p.is_positional()))
        {
            return Some(format!("{a} {v}"));
        }
    }
    None
}

/// A value from `--value`, else stdin (as given: no trailing newline is added or removed
/// but one final `\n`, as a shell's `echo` adds).
fn value_arg(args: &[String]) -> Option<String> {
    flag(args, "--value")
}

fn stdin_value(stdin: &[u8]) -> Result<String, Refusal> {
    let s = std::str::from_utf8(stdin).map_err(|_| refusal(2, "stdin is not UTF-8"))?;
    Ok(s.strip_suffix('\n').unwrap_or(s).to_owned())
}

// ---------- the verbs ----------

fn cell_get(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--row", "--where", "--col"], &["--json"])?;
    let (row, col, json) = (
        row_arg(args)?,
        parse_col(&need(args, "--col")?)?,
        has(args, "--json"),
    );
    Ok(run_read(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let r = resolve_row(t, &row, Want::Live, &lines)?;
        let c = resolve_col(t, &col, Want::Live, &lines)?;
        let v = t.cell(&r, &c).unwrap_or("");
        let out = if json {
            // the doc's version (a `--base` for the next write) and the cell's last write,
            // as `blame --cell` gives it (null without the history right or a recorded write)
            let version = ctx.version().map_or("null".to_owned(), |v| v.to_string());
            let cell = super::cell_key(&r, &c);
            let (author, change) = match ctx.last_write(super::CELLS, &cell, Facet::Value) {
                Some(b) => (json_str(&b.author), b.change.to_string()),
                None => ("null".to_owned(), "null".to_owned()),
            };
            format!(
                "{{\"value\":{},\"row\":{},\"col\":{},\"version\":{version},\"author\":{author},\"change\":{change}}}\n",
                json_str(v),
                json_str(r.as_str()),
                json_str(c.as_str())
            )
        } else {
            format!("{v}\n")
        };
        let shown = ClaimSpan::Cells {
            rows: (r.clone(), r),
            cols: (c.clone(), c),
        };
        Ok((out, Some(shown)))
    }))
}

fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn cell_set(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--row", "--where", "--col", "--value"], &[])?;
    let (row, col, value) = (
        row_arg(args)?,
        parse_col(&need(args, "--col")?)?,
        value_arg(args),
    );
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let r = resolve_row(t, &row, Want::Live, &lines)?;
        let c = resolve_col(t, &col, Want::Live, &lines)?;
        let v = match value {
            Some(v) => v,
            None => stdin_value(ctx.stdin())?,
        };
        Ok((t.set_cell(&r, &c, &v).map_err(table_err)?, String::new()))
    }))
}

fn cells_set(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--from"], &[])?;
    let (row, col) = cell_arg(&need(args, "--from")?)?;
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let r = resolve_row(t, &row, Want::Live, &lines)?;
        let c = resolve_col(t, &col, Want::Live, &lines)?;
        let block: Vec<Vec<String>> = ctx
            .decode_block(ctx.stdin())
            .map_err(|e| refusal(2, e.to_string()))?
            .into_iter()
            .map(|rec| rec.into_iter().map(|f| f.value).collect())
            .collect();
        if block.is_empty() {
            return refuse(2, "stdin: no block to paste");
        }
        Ok((
            t.set_block(&r, &c, &block).map_err(table_err)?,
            String::new(),
        ))
    }))
}

fn cells_clear(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--from", "--to"], &[])?;
    let (a, b) = (
        cell_arg(&need(args, "--from")?)?,
        cell_arg(&need(args, "--to")?)?,
    );
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let (ra, ca) = (
            resolve_row(t, &a.0, Want::Live, &lines)?,
            resolve_col(t, &a.1, Want::Live, &lines)?,
        );
        let (rb, cb) = (
            resolve_row(t, &b.0, Want::Live, &lines)?,
            resolve_col(t, &b.1, Want::Live, &lines)?,
        );
        Ok((
            t.clear_block((&ra, &ca), (&rb, &cb)).map_err(table_err)?,
            String::new(),
        ))
    }))
}

fn row_insert(args: &[String]) -> Result<StructEdit, ToolError> {
    known(
        args,
        &["--at", "--after", "--before", "--values"],
        &["--start", "--end"],
    )?;
    let at = place(args, "--at", parse_row)?;
    let values = flag(args, "--values");
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let live = lines.table(t).live_rows();
        let anchor = anchor_of(
            &at,
            &live,
            |r| resolve_row(t, r, Want::Live, &lines),
            |k| lines.live(t, ROWS, k, Want::Live),
        )?;
        let vals: Vec<String> = match values {
            Some(v) => v.split(',').map(str::to_owned).collect(),
            None if ctx.stdin().is_empty() => Vec::new(),
            None => {
                let mut recs = ctx
                    .decode_block(ctx.stdin())
                    .map_err(|e| refusal(2, e.to_string()))?;
                if recs.len() > 1 {
                    return refuse(2, "stdin: one record (the new row's values)");
                }
                recs.pop()
                    .map(|rec| rec.into_iter().map(|f| f.value).collect())
                    .unwrap_or_default()
            }
        };
        let (k, edits) = t.insert_row(anchor, &vals).map_err(table_err)?;
        Ok((edits, format!("{k}\n")))
    }))
}

fn row_move(args: &[String]) -> Result<StructEdit, ToolError> {
    known(
        args,
        &["--row", "--where", "--after", "--before", "--to"],
        &["--start", "--end"],
    )?;
    let (row, to) = (row_arg(args)?, place(args, "--to", parse_row)?);
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let r = resolve_row(t, &row, Want::Live, &lines)?;
        // `--to #N`: the Nth among the others (it lands there)
        let others: Vec<Key> = lines
            .table(t)
            .live_rows()
            .into_iter()
            .filter(|k| *k != r)
            .collect();
        let anchor = anchor_of(
            &to,
            &others,
            |x| resolve_row(t, x, Want::Live, &lines),
            |k| lines.live(t, ROWS, k, Want::Live),
        )?;
        Ok((t.move_row(&r, anchor).map_err(table_err)?, String::new()))
    }))
}

fn row_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--row", "--where"], &[])?;
    let row = row_arg(args)?;
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let r = resolve_row(t, &row, Want::Live, &lines)?;
        Ok((t.delete_row(&r).map_err(table_err)?, String::new()))
    }))
}

fn row_restore(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--row"], &[])?;
    let row = parse_row(&need(args, "--row")?)?;
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let r = resolve_row(t, &row, Want::AnyKnown, &lines)?;
        if t.rows.is_live(&r) {
            return Ok((
                Vec::new(),
                format!("row {r} is not deleted: nothing to restore\n"),
            ));
        }
        Ok((t.restore_row(&r).map_err(table_err)?, String::new()))
    }))
}

fn row_list(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--cols", "--where", "-n"], &[])?;
    let cols: Option<Vec<ColAt>> = flag(args, "--cols")
        .map(|s| s.split(',').map(parse_col).collect::<Result<_, _>>())
        .transpose()?;
    let wh = flag(args, "--where").map(|w| parse_where(&w)).transpose()?;
    let limit = flag(args, "-n").map(|n| number(&n, "-n")).transpose()?;
    Ok(run_read(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let cols: Vec<Key> = match &cols {
            Some(cs) => cs
                .iter()
                .map(|c| resolve_col(t, c, Want::Live, &lines))
                .collect::<Result<_, _>>()?,
            None => t.live_cols(),
        };
        let filter: Option<(Key, String)> = match &wh {
            Some(RowAt::Where(c, v)) => Some((resolve_col(t, c, Want::Live, &lines)?, v.clone())),
            _ => None,
        };
        let map = ctx.line_map().unwrap_or_else(|| t.line_map());
        let filter = filter.as_ref().map(|(c, v)| (c, v.as_str()));
        let (out, n) = row_listing(t, &map, &cols, filter, limit);
        if n == 0 && filter.is_some() {
            return refuse(4, "no row matches --where");
        }
        // the rows listed, first to last: what --for-edit claims
        let listed = listed_rows(t, filter, limit);
        let shown = match (listed.first(), listed.last()) {
            (Some(a), Some(b)) => Some(ClaimSpan::Rows {
                first: (*a).clone(),
                last: (*b).clone(),
            }),
            _ => None,
        };
        Ok((out, shown))
    }))
}

/// The live rows `row list` shows, in order: those whose cell in `filter`'s column is its
/// value, at most `limit`.
fn listed_rows<'t>(
    t: &'t Table,
    filter: Option<(&Key, &str)>,
    limit: Option<usize>,
) -> Vec<&'t Key> {
    t.rows
        .live()
        .filter(|r| match filter {
            Some((c, v)) => t.cell(r, c).unwrap_or("") == v,
            None => true,
        })
        .take(limit.unwrap_or(usize::MAX))
        .collect()
}

/// `row list`'s output (what a tab's row view shows): one line per live row, `key  #N
/// L<lines>  values…` (TSV; tabs, line breaks and backslashes in values escaped), over
/// `cols`, the rows whose cell in `filter`'s column is its value, at most `limit`. Returns
/// the text and how many rows it lists.
pub fn row_listing(
    t: &Table,
    map: &super::LineMap,
    cols: &[Key],
    filter: Option<(&Key, &str)>,
    limit: Option<usize>,
) -> (String, usize) {
    let mut out = String::new();
    let mut n = 0;
    let listed: HashSet<&Key> = listed_rows(t, filter, limit).into_iter().collect();
    for (i, r) in t.rows.live().enumerate() {
        if !listed.contains(r) {
            continue;
        }
        n += 1;
        let lines = match map.lines_of(r) {
            Some(l) if l.len() == 1 => format!("L{}", l.start),
            Some(l) => format!("L{}-{}", l.start, l.end - 1),
            None => "-".into(),
        };
        let vals: Vec<String> = cols
            .iter()
            .map(|c| tsv_field(t.cell(r, c).unwrap_or("")))
            .collect();
        let _ = writeln!(out, "{r}\t#{}\t{lines}\t{}", i + 1, vals.join("\t"));
    }
    (out, n)
}

/// `col list`'s output: one line per live column, `key  #N  name` (no name with the header
/// off).
pub fn col_listing(t: &Table) -> String {
    let mut out = String::new();
    for (i, c) in t.cols.live().enumerate() {
        let name = if t.header {
            t.name(c).unwrap_or("")
        } else {
            ""
        };
        let _ = writeln!(out, "{c}\t#{}\t{}", i + 1, tsv_field(name));
    }
    out
}

/// A value in a TSV listing: tabs, line breaks and backslashes escaped (one row, one line).
fn tsv_field(v: &str) -> String {
    let mut o = String::with_capacity(v.len());
    for ch in v.chars() {
        match ch {
            '\t' => o.push_str("\\t"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\\' => o.push_str("\\\\"),
            c => o.push(c),
        }
    }
    o
}

fn col_insert(args: &[String]) -> Result<StructEdit, ToolError> {
    known(
        args,
        &["--at", "--after", "--before", "--name"],
        &["--start", "--end"],
    )?;
    let (at, name) = (place(args, "--at", parse_col)?, flag(args, "--name"));
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let live = lines.table(t).live_cols();
        let anchor = anchor_of(
            &at,
            &live,
            |c| resolve_col(t, c, Want::Live, &lines),
            |k| lines.live(t, super::COLS, k, Want::Live),
        )?;
        let (k, edits) = t.insert_col(anchor, name.as_deref()).map_err(table_err)?;
        Ok((edits, format!("{k}\n")))
    }))
}

fn col_move(args: &[String]) -> Result<StructEdit, ToolError> {
    known(
        args,
        &["--col", "--after", "--before", "--to"],
        &["--start", "--end"],
    )?;
    let (col, to) = (
        parse_col(&need(args, "--col")?)?,
        place(args, "--to", parse_col)?,
    );
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let c = resolve_col(t, &col, Want::Live, &lines)?;
        let others: Vec<Key> = lines
            .table(t)
            .live_cols()
            .into_iter()
            .filter(|k| *k != c)
            .collect();
        let anchor = anchor_of(
            &to,
            &others,
            |x| resolve_col(t, x, Want::Live, &lines),
            |k| lines.live(t, super::COLS, k, Want::Live),
        )?;
        Ok((t.move_col(&c, anchor).map_err(table_err)?, String::new()))
    }))
}

fn col_delete(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--col"], &[])?;
    let col = parse_col(&need(args, "--col")?)?;
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let c = resolve_col(t, &col, Want::Live, &lines)?;
        Ok((t.delete_col(&c).map_err(table_err)?, String::new()))
    }))
}

fn col_restore(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--col"], &[])?;
    let col = parse_col(&need(args, "--col")?)?;
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let c = resolve_col(t, &col, Want::AnyKnown, &lines)?;
        if t.cols.is_live(&c) {
            return Ok((
                Vec::new(),
                format!("column {c} is not deleted: nothing to restore\n"),
            ));
        }
        Ok((t.restore_col(&c).map_err(table_err)?, String::new()))
    }))
}

fn col_rename(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &["--col", "--name"], &[])?;
    let (col, name) = (parse_col(&need(args, "--col")?)?, need(args, "--name")?);
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        if !t.header {
            return refuse(
                2,
                "the header is off: columns have no names (header on first)",
            );
        }
        let c = resolve_col(t, &col, Want::Live, &lines)?;
        Ok((t.rename_col(&c, &name).map_err(table_err)?, String::new()))
    }))
}

fn col_list(args: &[String]) -> Result<StructEdit, ToolError> {
    known(args, &[], &[])?;
    Ok(run(move |_, t| Ok((Vec::new(), col_listing(t)))))
}

/// `--by C [--desc] [--numeric] [--then C2 [--desc] [--numeric]] ...`: each `--desc` and
/// `--numeric` applies to the column before it.
fn sort_keys(args: &[String]) -> Result<Vec<(ColAt, bool, bool)>, ToolError> {
    let mut keys: Vec<(ColAt, bool, bool)> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            f @ ("--by" | "--then") => {
                if f == "--by" && !keys.is_empty() {
                    return Err(ToolError::usage("--by once; more columns with --then"));
                }
                if f == "--then" && keys.is_empty() {
                    return Err(ToolError::usage("--then after --by"));
                }
                let c = args
                    .get(i + 1)
                    .ok_or_else(|| ToolError::usage(format!("{f} needs a column")))?;
                keys.push((parse_col(c)?, false, false));
                i += 2;
            }
            "--desc" => {
                keys.last_mut()
                    .ok_or_else(|| ToolError::usage("--desc after a column"))?
                    .1 = true;
                i += 1;
            }
            "--numeric" => {
                keys.last_mut()
                    .ok_or_else(|| ToolError::usage("--numeric after a column"))?
                    .2 = true;
                i += 1;
            }
            other => return Err(ToolError::usage(format!("unknown argument {other:?}"))),
        }
    }
    if keys.is_empty() {
        return Err(ToolError::usage("--by is required"));
    }
    Ok(keys)
}

fn sort(args: &[String]) -> Result<StructEdit, ToolError> {
    let keys = sort_keys(args)?;
    Ok(run(move |ctx, t| {
        let lines = lines_of(ctx)?;
        let mut by = Vec::with_capacity(keys.len());
        for (c, desc, numeric) in &keys {
            let mut k = SortKey::by(resolve_col(t, c, Want::Live, &lines)?);
            k.desc = *desc;
            k.numeric = *numeric;
            by.push(k);
        }
        Ok((t.sort(&by).map_err(table_err)?, String::new()))
    }))
}

fn header(args: &[String]) -> Result<StructEdit, ToolError> {
    let on = match args {
        [a] if a == "on" => true,
        [a] if a == "off" => false,
        _ => return Err(ToolError::usage("header on|off")),
    };
    Ok(run(move |ctx, t| {
        if t.header == on {
            return Ok((
                Vec::new(),
                format!("the header is already {}\n", if on { "on" } else { "off" }),
            ));
        }
        // the pack's rule for the switch (CSV: the file's bytes never change)
        let edits = ctx.set_header_declined(t, on).map_err(|d| {
            let mut r = refusal(2, d.message);
            r.entry = match d.code {
                "header_names_unrendered" => Some(&crate::errors::ids::HEADER_NAMES_UNRENDERED),
                "header_only_row_blank" => Some(&crate::errors::ids::HEADER_ONLY_ROW_BLANK),
                _ => None,
            };
            r
        })?;
        Ok((edits, String::new()))
    }))
}
