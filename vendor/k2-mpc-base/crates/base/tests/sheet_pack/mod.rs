//! A workbook-shaped keyed doc for the live grid's rows, and the shared vector
//! (`client/test/vectors/grid_view.tsv`) read into the kit's types.
//!
//! The doc: `sheets`, `cols` and `rows` (MovableLists) with their fields in `sheet_props`,
//! `col_props` and `cells` (composite Maps), and a plain Map `book`. A sheet `s1` is
//! `@sheet:1` on the wire, a row `s1-r3` `@sheet:1/row:r3`, a column `s1-c3` `@sheet:1/col:c3`.
//! A row's sheet is its `@sheet` field (the sheet's `sid`), a column's its `sheet` field; a row's
//! cells are its fields not starting with `@`, named by their column key's tail (`c3`).
//!
//! The file is the vector's state lines (tab-separated, by address). The pack's one verb,
//! `grid step <rev>`, applies the vector's ops for that rev and reports what the vector says the
//! verb wrote ([`StructOutput::writing`]); its `page_derived` gives the vector's A1 text.
#![allow(dead_code)]

use std::collections::BTreeMap;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Anchor, KeyedRoot, KeyedState, ListOp, MapOp};
use k2_mpc_base::grid::{
    Axis, Band, BandOp, CellParts, CellRange, Cells, CellsReport, ColLabel, Corner, GridSlot,
    Numbering, PageDerived, StyleHintSpec,
};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{Declined, Pack, PackContent, PackError};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError, VerbCtx};
use k2_mpc_base::view_index::{ContainerFields, FieldClass, Src, UnitFields, ViewSchema};
use k2_mpc_base::{Edit, Key, Value};

pub const SHEETS: &str = "sheets";
pub const SHEET_PROPS: &str = "sheet_props";
pub const COLS: &str = "cols";
pub const COL_PROPS: &str = "col_props";
pub const ROWS: &str = "rows";
pub const CELLS: &str = "cells";
pub const BOOK: &str = "book";
pub const SEP: char = '\u{1f}';

pub const SCHEMA: Schema = Schema {
    name: "sheet",
    roots: &[
        (SHEETS, Kind::MovableList),
        (SHEET_PROPS, Kind::Map),
        (COLS, Kind::MovableList),
        (COL_PROPS, Kind::Map),
        (ROWS, Kind::MovableList),
        (CELLS, Kind::Map),
        (BOOK, Kind::Map),
    ],
};

pub const GRID: GridSlot = GridSlot {
    rows: ROWS,
    cols: COLS,
    cells: CELLS,
    col_fields: COL_PROPS,
    col_container_field: "sheet",
    row_fields_start: "@",
    col_of_cell_field: "{container}/col:{field}",
    row_numbering: Numbering {
        gap_field: "@gap",
        first: 1,
    },
    col_numbering: Numbering {
        gap_field: "gap",
        first: 1,
    },
    col_label: ColLabel::Letters,
    max_rows: 1_048_576,
    max_cols: 16_384,
    cell: Some(CellParts {
        encoding: "json",
        ty: "t",
        value: "v",
        formula: Some("f"),
        formula_kind: Some("fk"),
        style: Some("s"),
        types: &[
            ("n", "number"),
            ("s", "text"),
            ("b", "bool"),
            ("e", "error"),
        ],
    }),
    row_hidden: Some("@hidden"),
    row_height: Some("@ht"),
    height_unit: "pt",
    col_hidden: Some("hidden"),
    col_width: Some("w"),
    width_unit: "max digit widths",
    viewport: "cells",
    style_hint: Some(StyleHintSpec {
        field: "s",
        default: "0",
        table: "styles",
        parts: &["bold", "fill", "numfmt"],
    }),
    cell_derived: &["a1"],
};

pub const VIEW: ViewSchema = ViewSchema {
    containers: SHEETS,
    units: ROWS,
    unit_container_field: "@sheet",
    container_id_field: Some("sid"),
    native_unit: "cell",
    container_fields: ContainerFields {
        title: Src::Field("name"),
        hidden: Src::Field("hidden"),
        layout: Src::None,
        parent: Src::None,
    },
    unit_fields: UnitFields {
        kind: Src::ListValue,
        hidden: Src::Field("@hidden"),
        ..UnitFields::NONE
    },
    field_class: &[
        ("@sheet", FieldClass::Meta),
        ("@gap", FieldClass::Meta),
        ("@hidden", FieldClass::Visibility),
        ("@ht", FieldClass::Style),
    ],
    field_class_prefix: &[("c", FieldClass::Cell)],
    grid: Some(GRID),
    ..ViewSchema::NONE
};

/// The workbook pack. `prefix` off: the schema has no prefix rule (the row that shows a cell
/// write is classed `props` without one).
pub struct Sheet {
    pub view: ViewSchema,
}

impl Default for Sheet {
    fn default() -> Self {
        Sheet { view: VIEW }
    }
}

/// `@sheet:1` -> `s1`, `@sheet:1/row:r3` -> `s1-r3`, `@sheet:1/col:c3` -> `s1-c3`.
pub fn key_of_address(a: &str) -> Option<Key> {
    let rest = a.strip_prefix("@sheet:")?;
    let k = match rest.split_once('/') {
        None => format!("s{rest}"),
        Some((s, e)) => {
            let tail = e.strip_prefix("row:").or_else(|| e.strip_prefix("col:"))?;
            format!("s{s}-{tail}")
        }
    };
    Key::new(k).ok()
}

pub fn address_of_key(root: &str, k: &str) -> String {
    let Some(rest) = k.strip_prefix('s') else {
        return k.to_owned();
    };
    match (root, rest.split_once('-')) {
        (SHEETS, None) => format!("@sheet:{rest}"),
        (ROWS, Some((s, r))) => format!("@sheet:{s}/row:{r}"),
        (COLS, Some((s, c))) => format!("@sheet:{s}/col:{c}"),
        _ => k.to_owned(),
    }
}

pub fn k(a: &str) -> Key {
    key_of_address(a).unwrap_or_else(|| Key::new(a).unwrap())
}

impl Pack for Sheet {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        &[(SHEET_PROPS, SHEETS), (COL_PROPS, COLS), (CELLS, ROWS)]
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        Some(self.view)
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        address_of_key(root, key.as_str())
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        let ok = match root {
            SHEETS => !address.contains('/'),
            ROWS => address.contains("/row:"),
            COLS => address.contains("/col:"),
            _ => false,
        };
        if ok {
            key_of_address(address)
        } else {
            None
        }
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let bad = |l: &str| Declined {
            code: "malformed",
            message: format!("bad line {l:?}"),
        };
        let mut lists: BTreeMap<&str, Vec<(Key, Value)>> = BTreeMap::new();
        let mut maps: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
        for (r, kind) in SCHEMA.roots {
            match kind {
                Kind::MovableList => {
                    lists.insert(r, Vec::new());
                }
                _ => {
                    maps.insert(r, BTreeMap::new());
                }
            }
        }
        for line in std::str::from_utf8(b).map_err(|_| bad("utf-8"))?.lines() {
            let p: Vec<&str> = line.split('\t').collect();
            match p.as_slice() {
                ["list", root, a, v] => lists
                    .get_mut(root)
                    .ok_or_else(|| bad(line))?
                    .push((k(a), Value::str(*v))),
                ["map", root, a, f, v] => {
                    let key = if f.is_empty() {
                        (*a).to_owned()
                    } else {
                        format!("{}{SEP}{f}", k(a))
                    };
                    maps.get_mut(root)
                        .ok_or_else(|| bad(line))?
                        .insert(key, Value::str(*v));
                }
                _ => return Err(bad(line)),
            }
        }
        let roots = SCHEMA
            .roots
            .iter()
            .map(|(r, kind)| {
                let n = match kind {
                    Kind::MovableList => Neutral::List(lists.remove(r).unwrap_or_default()),
                    _ => Neutral::Map(maps.remove(r).unwrap_or_default()),
                };
                ((*r).to_owned(), n)
            })
            .collect();
        Ok(PackContent {
            roots,
            codec: Vec::new(),
        })
    }
    fn encode(&self, _: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(Vec::new())
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
    fn page_derived(&self, _state: &KeyedState, units: &[Key]) -> Vec<PageDerived> {
        let v = vector();
        let mut out: BTreeMap<Key, Vec<(String, String)>> = BTreeMap::new();
        for (unit, field, text) in &v.a1 {
            if units.contains(unit) {
                out.entry(unit.clone())
                    .or_default()
                    .push((field.clone(), text.clone()));
            }
        }
        out.into_iter()
            .map(|(unit, values)| PageDerived {
                unit,
                name: "a1",
                values,
            })
            .collect()
    }
}

const S: &[PersonAction] = &[PersonAction::Format];

const VERBS: &[PackVerb] =
    &[
        PackVerb::structured_edit("grid step", "grid step <rev>", S, &[0, 2], grid_step)
            .positions(&[]),
    ];

/// `grid step <rev>`: the vector's ops of that rev, and its report.
fn grid_step(args: &[String]) -> Result<StructEdit, ToolError> {
    let rev: u64 = args
        .first()
        .and_then(|a| a.parse().ok())
        .ok_or_else(|| ToolError::usage("grid step <rev>"))?;
    let step = vector()
        .steps
        .iter()
        .find(|s| s.rev == rev)
        .cloned()
        .ok_or_else(|| ToolError::usage(format!("no rev {rev} in the vector")))?;
    Ok(Box::new(move |_: &VerbCtx<'_>| {
        let mut out = StructOutput::new(step.edits.clone(), Vec::new(), 0);
        if let Some(r) = step.report.clone() {
            out = out.writing(r);
        }
        Ok(out)
    }))
}

// ---------- the vector ----------

/// One rev of the vector.
#[derive(Debug, Clone)]
pub struct Step {
    pub rev: u64,
    /// `None`: an outside save.
    pub verb: Option<String>,
    pub edits: Vec<Edit>,
    pub report: Option<CellsReport>,
    /// The agent's cells after it, when the vector says.
    pub agent_cells: Option<Option<Cells>>,
    pub claim_lines: Option<(usize, usize)>,
}

/// The vector's rows, each as the line gives it.
#[allow(clippy::type_complexity)]
#[derive(Debug, Clone, Default)]
pub struct Vector {
    /// The doc at rev 20, as the pack's file.
    pub file: String,
    pub steps: Vec<Step>,
    /// The agent's claim: container, range, note, hold.
    pub claim: Option<(Key, CellRange, String, bool)>,
    pub claim_cells: Option<Cells>,
    /// `(rev, container address, axis, [(address, number or letters)])`.
    pub numbers: Vec<(u64, String, Axis, Vec<(String, String)>)>,
    pub blank_rows: Vec<(u64, String, Vec<u64>)>,
    /// `(rev, id, cells, active A1, ranges A1)`.
    pub cells_a1: Vec<(u64, String, Cells, String, String)>,
    /// `(rev, id, container, range, A1)`.
    pub range_a1: Vec<(u64, String, Key, CellRange, String)>,
    /// `(rev, id, container, corner, A1)`.
    pub corner_a1: Vec<(u64, String, Key, Corner, String)>,
    /// `(rev, container, corner, why)`.
    pub corner_refused: Vec<(u64, Key, Corner, String)>,
    /// `(unit, field, text)`.
    pub a1: Vec<(Key, String, String)>,
}

pub fn corner(s: &str) -> Corner {
    let p: Vec<&str> = s.split(',').collect();
    assert_eq!(p.len(), 4, "a corner: {s}");
    let key = |x: &str| (x != "-").then(|| k(x));
    Corner::new(
        key(p[0]),
        p[1].parse().unwrap(),
        key(p[2]),
        p[3].parse().unwrap(),
    )
}

pub fn range(s: &str) -> CellRange {
    // addresses hold colons too: the corners are the first four parts and the last four, the
    // fourth part being `<offset>:<row>`
    let p: Vec<&str> = s.split(',').collect();
    assert_eq!(p.len(), 7, "a range is two corners: {s}");
    let (off, row) = p[3].split_once(':').expect("a range is two corners");
    CellRange::new(
        corner(&[p[0], p[1], p[2], off].join(",")),
        corner(&[row, p[4], p[5], p[6]].join(",")),
    )
}

pub fn ranges(s: &str) -> Vec<CellRange> {
    if s == "-" {
        return Vec::new();
    }
    s.split(';').map(range).collect()
}

/// `<container> <active> <ranges>` (`-` for none).
pub fn cells(p: &[&str]) -> Option<Cells> {
    if p.first() == Some(&"-") {
        return None;
    }
    let active = (p[1] != "-").then(|| corner(p[1]));
    Some(Cells::new(k(p[0]), active, ranges(p[2])))
}

fn path() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../client/test/vectors/grid_view.tsv")
}

pub fn vector() -> &'static Vector {
    static V: std::sync::OnceLock<Vector> = std::sync::OnceLock::new();
    V.get_or_init(|| parse(&std::fs::read_to_string(path()).expect("the grid vector")))
}

fn parse(text: &str) -> Vector {
    let mut v = Vector::default();
    let mut file = String::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let p: Vec<&str> = line.split('\t').collect();
        match p[0] {
            "state" => {}
            "list" | "map" => {
                file.push_str(line);
                file.push('\n');
            }
            "a1" => v.a1.push((k(p[1]), p[2].to_owned(), p[3].to_owned())),
            "claim" => v.claim = Some((k(p[1]), range(p[2]), p[3].to_owned(), p[4] == "hold")),
            "claim_cells" => v.claim_cells = cells(&p[1..]),
            "rev" => v.steps.push(Step {
                rev: p[1].parse().unwrap(),
                verb: (p[2] != "-").then(|| p[2].to_owned()),
                edits: Vec::new(),
                report: None,
                agent_cells: None,
                claim_lines: None,
            }),
            "op" => {
                let e = match (p[1], p[5]) {
                    ("map", "set") => Edit::Map {
                        root: p[2].to_owned(),
                        ops: vec![MapOp::Set {
                            key: map_key(p[3], p[4]),
                            value: Value::str(p[6]),
                        }],
                    },
                    ("map", "delete") => Edit::Map {
                        root: p[2].to_owned(),
                        ops: vec![MapOp::Delete {
                            key: map_key(p[3], p[4]),
                        }],
                    },
                    ("list", _) => {
                        assert_eq!(p[4], "insert", "{line}");
                        let at = if p[5] == "-" {
                            Anchor::Start
                        } else {
                            Anchor::After(k(p[5]))
                        };
                        Edit::List {
                            root: p[2].to_owned(),
                            ops: vec![ListOp::Insert {
                                key: k(p[3]),
                                at,
                                value: Value::str(p[6]),
                            }],
                        }
                    }
                    _ => panic!("an op: {line}"),
                };
                last_step(&mut v).edits.push(e);
            }
            "report" => {
                let r = match p[1] {
                    "written" => CellsReport::Written {
                        container: k(p[2]),
                        ranges: ranges(p[3]),
                    },
                    "band" => CellsReport::Band {
                        container: k(p[2]),
                        band: Band {
                            axis: if p[3] == "rows" {
                                Axis::Rows
                            } else {
                                Axis::Cols
                            },
                            op: if p[4] == "insert" {
                                BandOp::Insert
                            } else {
                                BandOp::Delete
                            },
                            from: p[5].parse().unwrap(),
                            to: p[6].parse().unwrap(),
                        },
                    },
                    "container" => CellsReport::Container { container: k(p[2]) },
                    _ => panic!("a report: {line}"),
                };
                last_step(&mut v).report = Some(r);
            }
            "agent_cells" => last_step(&mut v).agent_cells = Some(cells(&p[1..])),
            "claim_lines" => {
                last_step(&mut v).claim_lines = Some((p[1].parse().unwrap(), p[2].parse().unwrap()))
            }
            "numbers" => {
                let pairs = if p[4] == "-" {
                    Vec::new()
                } else {
                    p[4].split(',')
                        .map(|x| {
                            let (a, n) = x.rsplit_once('=').unwrap();
                            (a.to_owned(), n.to_owned())
                        })
                        .collect()
                };
                let axis = if p[3] == "rows" {
                    Axis::Rows
                } else {
                    Axis::Cols
                };
                v.numbers
                    .push((p[1].parse().unwrap(), p[2].to_owned(), axis, pairs));
            }
            "blank_rows" => {
                let ns = if p[3] == "-" {
                    Vec::new()
                } else {
                    p[3].split(',').map(|n| n.parse().unwrap()).collect()
                };
                v.blank_rows
                    .push((p[1].parse().unwrap(), p[2].to_owned(), ns));
            }
            "cells_a1" => {
                let c = cells(&p[3..6]).expect("cells");
                v.cells_a1.push((
                    p[1].parse().unwrap(),
                    p[2].to_owned(),
                    c,
                    p[6].to_owned(),
                    p[7].to_owned(),
                ));
            }
            "range_a1" => v.range_a1.push((
                p[1].parse().unwrap(),
                p[2].to_owned(),
                k(p[3]),
                range(p[4]),
                p[5].to_owned(),
            )),
            "corner_a1" => v.corner_a1.push((
                p[1].parse().unwrap(),
                p[2].to_owned(),
                k(p[3]),
                corner(p[4]),
                p[5].to_owned(),
            )),
            "corner_refused" => v.corner_refused.push((
                p[1].parse().unwrap(),
                k(p[3]),
                corner(p[4]),
                p[5].to_owned(),
            )),
            other => panic!("an unknown vector line {other:?}"),
        }
    }
    v.file = file;
    v
}

fn last_step(v: &mut Vector) -> &mut Step {
    v.steps.last_mut().expect("a rev line first")
}

fn map_key(a: &str, field: &str) -> String {
    if field.is_empty() {
        a.to_owned()
    } else {
        format!("{}{SEP}{field}", k(a))
    }
}

/// The doc's keyed state's `cells` entries, `(row key, field)`.
pub fn cell_entries(state: &KeyedState) -> Vec<(Key, String)> {
    let Some((_, KeyedRoot::Map(m))) = state.roots.iter().find(|(r, _)| r == CELLS) else {
        return Vec::new();
    };
    m.keys()
        .filter_map(|key| {
            let (r, f) = key.split_once(SEP)?;
            Some((Key::new(r).ok()?, f.to_owned()))
        })
        .collect()
}
