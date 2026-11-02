//! How positions are encoded on each surface (K-ADDR-4), as data: the one table
//! `capabilities` states ([`crate::errors::capabilities`], its `positions` key) and
//! `docs/POSITIONS.md` explains.
//!
//! - **Inside the kit** a position is a UTF-8 byte offset into a root's doc text (`\n` line
//!   breaks, no BOM), always on a character boundary ([`crate::session::position`]; edits'
//!   [`crate::text::diff::Hunk`]s, carets, claims, change events).
//! - **People and command-line verbs** speak `line:col`: 1-based lines, 1-based columns
//!   counted in Unicode scalar values ([`crate::session::LineCol`]).
//! - **The hunk channel** (a live client's frames and carets, `client/WIRE.md`) carries the
//!   kit's UTF-8 byte offsets; the JS client converts to UTF-16 only for its editor.
//! - **The Yjs wire** (sync updates exchanged with Yjs clients, feature `untrusted-wire`)
//!   counts UTF-16 code units, as Yjs does: only inside the update bytes, converted at the
//!   engine adapter; no offset in the kit's API is in UTF-16. What an engine counts
//!   internally (element clocks, provenance) is not part of this statement: it is the doc's
//!   engine's, and may differ between engines.
//!
//! [`VERB_POSITIONS`] lists every verb (the kit's, and the grid verbs of
//! [`crate::table::verbs::VERBS`]) that takes or prints positions, each form with its
//! [`Unit`]. A test holds it to the verbs' usage lines.

use super::Tier;
use Unit::{
    FileBytes, FileChars, GridCell, GridCol, GridRow, LineCol, Lines, Ordinal, RenderedLines,
};

/// A unit positions (or position counts) are in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Unit {
    /// UTF-8 byte offsets of a root's doc text (`\n` line breaks, no BOM), 0-based, on
    /// character boundaries.
    TextBytes,
    /// UTF-16 code units, 0-based: positions inside the update bytes exchanged with Yjs
    /// clients. Never an offset of the kit's API.
    Utf16,
    /// Bytes of the file as the pack encodes it (BOM and CRLF included).
    FileBytes,
    /// Unicode scalar values of the file as the pack encodes it (a BOM is one, a CR one).
    FileChars,
    /// 1-based lines of the text the verb sees (a text doc: its doc text; a grid: its
    /// rendered file), `\n` ending a line; ranges inclusive, `$` the last line.
    Lines,
    /// `L:C`: a 1-based line and a 1-based column counted in Unicode scalar values of the
    /// line's doc text (an emoji is one column); `chars + 1` is the line's end. A range
    /// `L:C-L:C` is half-open.
    LineCol,
    /// On a grid: 1-based rendered lines of the file, as the pack's line map counts them
    /// (the header's first; a line break inside a value starts a line).
    RenderedLines,
    /// On a grid, a row: a key (`r17`), `#N` (the Nth live row, 1-based) or `LN` (the row
    /// rendered line N belongs to).
    GridRow,
    /// On a grid, a column: a key (`c3`), `#N` (the Nth live column, 1-based) or a header name.
    GridCol,
    /// On a grid, a cell: `R,C`, a row and a column as [`Unit::GridRow`] and
    /// [`Unit::GridCol`].
    GridCell,
    /// On a grid: `#N`, the Nth live row or column (1-based), as an insert or move target.
    Ordinal,
}

/// Every unit, in the order `capabilities` lists them.
pub const UNITS: &[Unit] = &[
    Unit::TextBytes,
    Unit::Utf16,
    Unit::FileBytes,
    Unit::FileChars,
    Unit::Lines,
    Unit::LineCol,
    Unit::RenderedLines,
    Unit::GridRow,
    Unit::GridCol,
    Unit::GridCell,
    Unit::Ordinal,
];

impl Unit {
    /// Its stable name in `capabilities`.
    pub fn word(self) -> &'static str {
        match self {
            Unit::TextBytes => "utf8_bytes",
            Unit::Utf16 => "utf16_code_units",
            Unit::FileBytes => "file_bytes",
            Unit::FileChars => "file_chars",
            Unit::Lines => "lines",
            Unit::LineCol => "line_col",
            Unit::RenderedLines => "rendered_lines",
            Unit::GridRow => "grid_row",
            Unit::GridCol => "grid_col",
            Unit::GridCell => "grid_cell",
            Unit::Ordinal => "ordinal",
        }
    }

    /// Where counting starts: 0 for offsets, 1 for lines, columns and `#N`; `None` for a
    /// count (bytes, characters) or an address that is not only a number.
    pub fn base(self) -> Option<u8> {
        match self {
            Unit::TextBytes | Unit::Utf16 => Some(0),
            Unit::Lines | Unit::LineCol | Unit::RenderedLines | Unit::Ordinal => Some(1),
            Unit::FileBytes | Unit::FileChars | Unit::GridRow | Unit::GridCol | Unit::GridCell => {
                None
            }
        }
    }

    /// One line saying what it counts.
    pub fn meaning(self) -> &'static str {
        match self {
            Unit::TextBytes => {
                "UTF-8 byte offsets of a root's doc text (\\n line breaks, no BOM), on character \
                 boundaries"
            }
            Unit::Utf16 => {
                "UTF-16 code units, inside the update bytes exchanged with Yjs clients only; \
                 never an offset of the kit's API"
            }
            Unit::FileBytes => "bytes of the file as the pack encodes it (BOM and CRLF included)",
            Unit::FileChars => {
                "Unicode scalar values of the file as the pack encodes it (a BOM is one, a CR one)"
            }
            Unit::Lines => {
                "lines of the text the verb sees (a grid: its rendered file), \\n ending a line; \
                 ranges inclusive, $ the last line"
            }
            Unit::LineCol => {
                "L:C, a line and a column counted in Unicode scalar values (an emoji is one \
                 column); chars + 1 is the line's end; a range L:C-L:C is half-open"
            }
            Unit::RenderedLines => {
                "a grid's rendered lines, as the pack's line map counts them (the header's first)"
            }
            Unit::GridRow => {
                "a row: a key (r17), #N (the Nth live row) or LN (rendered line N's row)"
            }
            Unit::GridCol => "a column: a key (c3), #N (the Nth live column) or a header name",
            Unit::GridCell => "R,C: a row and a column",
            Unit::Ordinal => "#N: the Nth live row or column",
        }
    }
}

/// One way a verb takes or prints positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionForm {
    /// The flag that takes the position, as the verb's usage spells it (`--at`, `-c`); or
    /// `stdin` (what the verb reads there, e.g. a diff's hunk headers), `script` (`sed`'s
    /// line addresses) or `stdout` (what it prints).
    pub at: &'static str,
    pub unit: Unit,
    /// The tier this form is for, when the verb counts differently on another; `None`: every
    /// tier the verb works on.
    pub tier: Option<Tier>,
}

/// The positions one verb takes or prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerbPositions {
    /// The verb's registry name ([`super::VerbSpec::name`]).
    pub verb: &'static str,
    pub forms: &'static [PositionForm],
}

/// The non-flag places a [`PositionForm::at`] can name.
pub const NON_FLAG_PLACES: &[&str] = &["stdin", "script", "stdout"];

/// Inside the kit (and every session API offset: hunks, carets, claims, change events).
pub const INTERNAL: Unit = Unit::TextBytes;
/// What `line:col` counts, as `capabilities` states it.
pub const LINE_COL: &str = "1-based lines; 1-based columns in Unicode scalar values";
/// The hunk channel's frames and carets (`client/WIRE.md`): the kit's own offsets.
pub const WIRE: Unit = Unit::TextBytes;
/// Sync updates exchanged with Yjs clients (feature `untrusted-wire`): Yjs counts UTF-16 code
/// units (the kit's Yrs runs with `OffsetKind::Utf16` to match). Not a promise about any
/// engine's internal units.
pub const YJS_WIRE: Unit = Unit::Utf16;

const fn f(at: &'static str, unit: Unit) -> PositionForm {
    PositionForm {
        at,
        unit,
        tier: None,
    }
}

const fn on(at: &'static str, unit: Unit, tier: Tier) -> PositionForm {
    PositionForm {
        at,
        unit,
        tier: Some(tier),
    }
}

const fn v(verb: &'static str, forms: &'static [PositionForm]) -> VerbPositions {
    VerbPositions { verb, forms }
}

/// Every verb that takes or prints positions, with each form's unit. `capabilities` lists
/// the rows of the verbs a host serves; the grid verbs' rows appear with a grid pack's
/// registry.
pub const VERB_POSITIONS: &[VerbPositions] = &[
    // the kit's read verbs (`tools/read.rs`:1-5: byte tools see the file, line tools the text)
    v("cat", &[f("-n", Lines), f("-b", Lines)]),
    v(
        "wc",
        &[f("-l", Lines), f("-c", FileBytes), f("-m", FileChars)],
    ),
    v("head", &[f("-n", Lines), f("-c", FileBytes)]),
    v("tail", &[f("-n", Lines), f("-c", FileBytes)]),
    v("sed", &[f("script", Lines)]),
    v("view", &[f("--lines", Lines)]),
    v(
        "grep",
        &[
            f("-n", Lines),
            f("-A", Lines),
            f("-B", Lines),
            f("-C", Lines),
        ],
    ),
    v("diff", &[f("-U", Lines), f("stdout", Lines)]),
    // presence
    v("who", &[f("stdout", LineCol), f("stdout", Lines)]),
    v("activity", &[f("stdout", Lines)]),
    v(
        "claim",
        &[
            f("--lines", Lines),
            on("--rows", GridRow, Tier::Grid),
            on("--cols", GridCol, Tier::Grid),
            on("--cells", GridCell, Tier::Grid),
        ],
    ),
    v("follow", &[f("stdout", LineCol)]),
    v(
        "cursor",
        &[
            f("--at", LineCol),
            f("--select", LineCol),
            on("--cell", GridCell, Tier::Grid),
            on("--to", GridCell, Tier::Grid),
        ],
    ),
    // the kit's edit verbs (`tools/edit.rs`:1-11)
    v(
        "insert",
        &[f("--line", Lines), f("--after", Lines), f("--at", LineCol)],
    ),
    v("replace", &[f("--lines", Lines), f("--range", LineCol)]),
    v("delete", &[f("--lines", Lines), f("--range", LineCol)]),
    v("sed -i", &[f("script", Lines)]),
    v("patch", &[f("stdin", Lines), f("-F", Lines)]),
    v("apply", &[f("stdin", Lines)]),
    // history
    v("diff --version", &[f("-U", Lines), f("stdout", Lines)]),
    v(
        "blame",
        &[
            on("-L", Lines, Tier::Text),
            on("-L", RenderedLines, Tier::Grid),
            on("stdout", Lines, Tier::Text),
            on("stdout", RenderedLines, Tier::Grid),
        ],
    ),
    v("blame --cell", &[f("--cell", GridCell)]),
    // the grid verbs (`table/verbs.rs`:6-11)
    v("cell get", &[f("--row", GridRow), f("--col", GridCol)]),
    v("cell set", &[f("--row", GridRow), f("--col", GridCol)]),
    v("cells set", &[f("--from", GridCell)]),
    v("cells clear", &[f("--from", GridCell), f("--to", GridCell)]),
    v(
        "row insert",
        &[
            f("--at", Ordinal),
            f("--after", GridRow),
            f("--before", GridRow),
        ],
    ),
    v(
        "row move",
        &[
            f("--row", GridRow),
            f("--after", GridRow),
            f("--before", GridRow),
            f("--to", Ordinal),
        ],
    ),
    v("row delete", &[f("--row", GridRow)]),
    v("row restore", &[f("--row", GridRow)]),
    v("row list", &[f("--cols", GridCol), f("--where", GridCol)]),
    v(
        "col insert",
        &[
            f("--at", Ordinal),
            f("--after", GridCol),
            f("--before", GridCol),
        ],
    ),
    v(
        "col move",
        &[
            f("--col", GridCol),
            f("--after", GridCol),
            f("--before", GridCol),
            f("--to", Ordinal),
        ],
    ),
    v("col delete", &[f("--col", GridCol)]),
    v("col restore", &[f("--col", GridCol)]),
    v("col rename", &[f("--col", GridCol)]),
    v("sort", &[f("--by", GridCol), f("--then", GridCol)]),
];

/// `verb`'s row, if it takes or prints positions.
pub fn of(verb: &str) -> Option<&'static VerbPositions> {
    VERB_POSITIONS.iter().find(|r| r.verb == verb)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn one_row_per_verb_and_unique_unit_words() {
        let mut seen = HashSet::new();
        for r in VERB_POSITIONS {
            assert!(seen.insert(r.verb), "two rows for {}", r.verb);
            assert!(!r.forms.is_empty(), "{} has no forms", r.verb);
            for p in r.forms {
                assert!(
                    p.at.starts_with('-') || NON_FLAG_PLACES.contains(&p.at),
                    "{}: {:?} is neither a flag nor {NON_FLAG_PLACES:?}",
                    r.verb,
                    p.at
                );
            }
        }
        let words: HashSet<&str> = UNITS.iter().map(|u| u.word()).collect();
        assert_eq!(words.len(), UNITS.len());
    }
}
