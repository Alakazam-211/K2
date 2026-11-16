//! The view index: what a view of a
//! structured doc draws from, in one shape whichever source it was read from.
//!
//! - **The view schema** ([`ViewSchema`]). A pack declares once which list root holds the
//!   containers (a deck's slides) and which the units (its shapes), which unit field names its
//!   container, which fields carry a unit's geometry, text, name and visibility, how the page is
//!   sized and in what native unit ([`crate::session::Pack::view_schema`]). `None`, the default,
//!   means the pack has no view index.
//! - **Addresses**. Everything a view reads or sends names an element by its address,
//!   never by the live store's key: the pack maps one to the other
//!   ([`crate::session::Pack::address_of`] and its inverse [`crate::session::Pack::key_of`];
//!   identity by default), and the kit converts at its edge (the index here; the presence
//!   setters and claims by address, [`crate::session::Session::set_focus_at`] and its siblings;
//!   [`key_candidates`] and [`crate::session::Session::resolve_address`]).
//! - **One shape, two sources**. [`ViewIndex::live`] reads a live doc's keyed state
//!   through the schema; [`ViewIndex::blob`] reads one version's structure index through the
//!   schema's [`BlobView`]. `tier` says which. Containers come in order, units by container,
//!   back to front, a nested unit right after its parent. The pack then fills what only it can
//!   work out ([`crate::session::Pack::view_derive`]: a deck's slide titles, inherited boxes,
//!   fonts...) and the kit finishes (units grouped by container, `unit_count`).
//! - **The carrier** ([`ViewCarrier`]). On the live tier the hook also reads the doc's
//!   carrier, for what only the file's bytes hold (a deck's fonts, render hashes, field kinds, a
//!   group's children, a layout's boxes). The live store wins: the carrier supplies only what
//!   the store doesn't model. The pack's decode of it runs once per carrier, and the kit keeps a
//!   doc's index by (carrier id, rev) until either changes.
//! - **Paged for the watch.** [`ViewIndex::snapshot`] is everything but the units;
//!   [`ViewIndex::units_page`] reads one container's units in pages under a byte budget,
//!   counted as the page's compact JSON ([`UnitsPage::to_value`]), as the structure reads are
//!   ([`crate::index::READ_BUDGET`]).
//!
//! Nothing here holds a raw digest: a container's `render_hash` is a keyed [`NodeHash`].

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::elem::{Key, KeyedRoot, KeyedState, Value};
use crate::errors::Value as Json;
use crate::index::{cut_text, Digest, IndexNode, IndexView, NodeHash};
use crate::placement::{Link, Mat4, Placement, Space, SpaceSlot, SpaceSrc};
use crate::schema::{Kind, Schema};
use crate::session::{BlobPack, Pack};
use crate::table::CELL_SEP;

/// The longest name, alt text, title or text summary a view index carries, in bytes (cut on a
/// character boundary; a cut text summary says `truncated`).
pub const VIEW_TEXT_MAX: usize = 1024;

/// The smallest budget [`ViewIndex::units_page`] takes, in bytes: the structure reads' floor
/// ([`crate::index::READ_BUDGET_MIN`]). Any one unit fits in it.
pub const VIEW_PAGE_MIN: usize = crate::index::READ_BUDGET_MIN;

/// The default budget of one page of units, in bytes ([`crate::index::READ_BUDGET`]).
pub const VIEW_PAGE_BUDGET: usize = crate::index::READ_BUDGET;

/// The most names a schema's [`ViewSchema::unit_derived`] declares.
pub const UNIT_DERIVED_MAX: usize = 8;

/// The most names a schema's [`ViewSchema::container_derived`] declares.
pub const CONTAINER_DERIVED_MAX: usize = 8;

/// The most relations a schema's [`ViewSchema::nesting`] declares.
pub const NESTING_MAX: usize = 16;

/// The longest unit kind a [`Nesting`] names, in bytes.
pub const NESTING_KIND_MAX: usize = 64;

/// The longest name of a unit's derived value, in bytes.
pub const DERIVED_NAME_MAX: usize = 64;

/// The deepest a container sits in the containers' tree ([`Container::parent`]), in levels: a
/// container at the top level is at level 1, one inside it at level 2. A container that would
/// sit deeper has its parent refused ([`ParentRefused::TooDeep`]) and is listed at the top
/// level, the containers inside it counting their levels from it. It bounds what a viewer walks
/// and draws; real outlines (a scene's collections, a document's sections) stay far above it.
pub const CONTAINER_DEPTH_MAX: usize = 32;

// ----- the schema -----

/// Where a view field's value comes from in the live store: a field of the element (its key in
/// the element's list root's [composite](crate::session::Pack::composite) map, `<key>\x1f<field>`),
/// the element's own list value, or the pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// Not carried: the view field stays empty, unless the pack's derive hook sets it.
    None,
    /// The element's field of this name.
    Field(&'static str),
    /// The element's own value in its list (a deck's shape kind).
    ListValue,
    /// Worked out by the pack ([`crate::session::Pack::view_derive`]), under this name. The kit
    /// leaves the view field empty for the hook.
    Derived(&'static str),
    /// A field whose raw values stand for the contract's words, as `(raw, word)` pairs (a deck's
    /// `edit` field: `1` is `plain`). A raw value not listed gives nothing.
    Values {
        field: &'static str,
        values: &'static [(&'static str, &'static str)],
    },
}

/// Where a view field's value comes from in a version's structure index: the node's own kind,
/// name or text, or one of its attributes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeSrc {
    None,
    Kind,
    Name,
    Text,
    /// The node's attribute of this name ([`IndexNode::attrs`]).
    Attr(&'static str),
}

/// What a field holds, for the change frames' op kinds (contract §1.2, `field_class`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldClass {
    Geometry,
    Text,
    Style,
    Visibility,
    Meta,
    /// A grid cell's value (a field a [`ViewSchema::field_class_prefix`] rule names).
    Cell,
}

impl FieldClass {
    pub fn as_str(self) -> &'static str {
        match self {
            FieldClass::Geometry => "geometry",
            FieldClass::Text => "text",
            FieldClass::Style => "style",
            FieldClass::Visibility => "visibility",
            FieldClass::Meta => "meta",
            FieldClass::Cell => "cell",
        }
    }
}

/// One part of the geometry field, in its order in the field's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GeomPart {
    /// The box's left edge, in native units, page space.
    X,
    /// Its top edge.
    Y,
    /// Its width.
    W,
    /// Its height.
    H,
    /// Rotation, clockwise about the box centre, in the schema's `rot_unit`.
    Rot,
    /// Flipped across (`0`/`1`).
    FlipH,
    /// Flipped up-down (`0`/`1`).
    FlipV,
}

impl GeomPart {
    pub fn as_str(self) -> &'static str {
        match self {
            GeomPart::X => "x",
            GeomPart::Y => "y",
            GeomPart::W => "w",
            GeomPart::H => "h",
            GeomPart::Rot => "rot",
            GeomPart::FlipH => "fh",
            GeomPart::FlipV => "fv",
        }
    }
}

/// The geometry: **one field** whose value is its parts, comma-separated, in `parts` order (a
/// deck's `geom`, `x,y,w,h,rot,fh,fv`), so a geometry change is one map op. An empty value (a
/// shape that takes its box from its layout) gives no box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    pub field: &'static str,
    pub parts: &'static [GeomPart],
    /// What a rotation counts in, as a word (`"60000th-degree"` for a deck).
    pub rot_unit: &'static str,
}

/// Where the page size is: two keys of a Map root.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageSpec {
    pub root: &'static str,
    pub w: &'static str,
    pub h: &'static str,
}

/// A container's view fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContainerFields {
    pub title: Src,
    pub hidden: Src,
    /// The container's layout's address: one the kit's address grammar reads ([`crate::addr`];
    /// a bare key too), and, where the index holds layouts ([`ViewIndex::layouts`]), one of
    /// theirs. Checked when the index is built, on both tiers: a value that isn't one is not
    /// carried ([`Container::layout`] is `None`) and is a [`ViewRefusal::Layout`] on
    /// [`ViewIndex::refusals`]; a test build fails on it loudly.
    pub layout: Src,
    /// The container the container sits inside, for a tree of containers (a scene's
    /// collections inside collections): what [`Container::parent`] carries.
    ///
    /// - **A value the kit reads** (a field, the list value, a `values` word) is the parent's
    ///   **key** in the containers' list, as the live store holds it; the kit maps it to the
    ///   parent's address ([`crate::session::Pack::address_of`]). A value that isn't a key is
    ///   refused ([`ParentRefused::NotAKey`]).
    /// - **[`Src::Derived`]**: the pack's derive hook sets [`Container::parent`] itself, as an
    ///   address. Declare it so: the change frames carry a container's parent only for a schema
    ///   that declares one.
    /// - An empty value is none: the container is at the top level.
    ///
    /// Checked when the index is built, on both tiers (on the blob tier from
    /// [`BlobView::container_parent`]): the parent must be another container of the same index,
    /// with no cycle, at most [`CONTAINER_DEPTH_MAX`] levels deep. A parent that isn't is not
    /// carried (the container is listed at the top level, in its place in the flat order) and
    /// is a [`ViewRefusal::Parent`] on [`ViewIndex::refusals`]; a test build fails on it loudly.
    /// [`Src::None`] (the default): containers don't nest.
    pub parent: Src,
}

impl ContainerFields {
    pub const NONE: ContainerFields = ContainerFields {
        title: Src::None,
        hidden: Src::None,
        layout: Src::None,
        parent: Src::None,
    };
}

/// A unit's view fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnitFields {
    pub kind: Src,
    pub name: Src,
    pub alt: Src,
    pub text: Src,
    pub hidden: Src,
    /// Its words are `plain`, `formatted` and `never` ([`EditableText`]).
    pub editable_text: Src,
    /// `<type>:<idx>`, either part empty when the file has none ([`Placeholder`]).
    pub placeholder: Src,
}

impl UnitFields {
    pub const NONE: UnitFields = UnitFields {
        kind: Src::None,
        name: Src::None,
        alt: Src::None,
        text: Src::None,
        hidden: Src::None,
        editable_text: Src::None,
        placeholder: Src::None,
    };
}

/// The view fields the pack's derive hook sets, by their contract names (data for the view
/// side: a field listed here is the pack's, never the kit's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Derived {
    pub containers: &'static [&'static str],
    pub units: &'static [&'static str],
}

impl Derived {
    pub const NONE: Derived = Derived {
        containers: &[],
        units: &[],
    };
}

/// How the structure index's nodes ([`IndexView`]) map to the view: which node kind is a
/// container, which kinds under a container aren't units, and where each view field is on a
/// node. A container's `hidden` and a unit's are the node's own visibility; a unit's box is
/// the node's bounds; its rotation and flips are attributes. Units are the nodes under a
/// container, a nested unit's parent the unit it is under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlobView {
    pub container_kind: &'static str,
    /// Node kinds under a container that aren't units (a slide's notes), with what is under them.
    pub skip_kinds: &'static [&'static str],
    /// The page size: two of the index's doc attributes.
    pub page: Option<(&'static str, &'static str)>,
    /// The node kind of a layout: each such node is a layout, its parent node its master.
    pub layout_kind: Option<&'static str>,
    pub container_title: NodeSrc,
    pub container_layout: NodeSrc,
    /// A container's parent container ([`ContainerFields::parent`]): the parent's node key, which
    /// is its address on this tier. Checked as the live tier's is.
    pub container_parent: NodeSrc,
    pub unit_kind: NodeSrc,
    pub unit_name: NodeSrc,
    pub unit_alt: NodeSrc,
    pub unit_text: NodeSrc,
    pub unit_rot: NodeSrc,
    pub unit_flip_h: NodeSrc,
    pub unit_flip_v: NodeSrc,
    /// Raw values as the live field's ([`UnitFields::editable_text`]'s `values` map them).
    pub unit_editable_text: NodeSrc,
    pub placeholder_type: NodeSrc,
    pub placeholder_idx: NodeSrc,
}

impl BlobView {
    /// Nothing mapped but the container kind: start from it with `..BlobView::NONE`.
    pub const NONE: BlobView = BlobView {
        container_kind: "",
        skip_kinds: &[],
        page: None,
        layout_kind: None,
        container_title: NodeSrc::None,
        container_layout: NodeSrc::None,
        container_parent: NodeSrc::None,
        unit_kind: NodeSrc::Kind,
        unit_name: NodeSrc::Name,
        unit_alt: NodeSrc::None,
        unit_text: NodeSrc::Text,
        unit_rot: NodeSrc::None,
        unit_flip_h: NodeSrc::None,
        unit_flip_v: NodeSrc::None,
        unit_editable_text: NodeSrc::None,
        placeholder_type: NodeSrc::None,
        placeholder_idx: NodeSrc::None,
    };
}

/// One nesting relation of a view schema ([`ViewSchema::nesting`]): units of kind `kind` sit
/// inside a unit of kind `parent` (a collection's items, a group's shapes), so a client can draw
/// the hierarchy before it has every unit. The kinds are the units' `kind` as the index carries
/// it ([`UnitFields::kind`], [`Unit::kind`]); which unit a nested one is in is its own
/// [`Unit::parent`], by address. A declaration only: the kit builds the index as before.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nesting {
    /// The kind that nests.
    pub kind: &'static str,
    /// The kind it nests inside (the same kind for a group in a group).
    pub parent: &'static str,
}

/// A pack's view schema (contract §1.2): declared once, as a `const`, starting from
/// [`ViewSchema::NONE`] (`ViewSchema { containers: "slides", ..ViewSchema::NONE }`), so a field
/// the kit adds later has its default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewSchema {
    /// The MovableList root of the containers, in their order.
    pub containers: &'static str,
    /// The MovableList root of the units, back to front within a container.
    pub units: &'static str,
    /// The unit field that names its container.
    pub unit_container_field: &'static str,
    /// The container field that value matches (a deck's `sid`). `None`: the value is the
    /// container's key itself.
    pub container_id_field: Option<&'static str>,
    /// The file's own coordinates, as a word (`"emu"`).
    pub native_unit: &'static str,
    pub page: Option<PageSpec>,
    pub container_fields: ContainerFields,
    pub unit_fields: UnitFields,
    pub geometry: Option<Geometry>,
    /// What each field holds (the change frames' op kinds), by exact name.
    pub field_class: &'static [(&'static str, FieldClass)],
    /// What fields hold by the start of their name, in order: a field with no exact
    /// [`ViewSchema::field_class`] entry takes the class of the first rule whose prefix it starts
    /// with (a grid's cells are named by their column key: `c1`, `c2`... are cells).
    pub field_class_prefix: &'static [(&'static str, FieldClass)],
    pub derived: Derived,
    /// How a version's structure index maps to the view. `None`: no view index on the blob tier.
    pub blob: Option<BlobView>,
    /// A sparse grid's rows, columns, cells and numbering ([`crate::grid::GridSlot`]). `None`:
    /// not a grid.
    pub grid: Option<crate::grid::GridSlot>,
    /// The names of the pack's **derived values per unit**: small texts only the pack works out
    /// (a scene object's transform as people read it, a row's number), which its derive hook
    /// sets on each unit ([`Unit::set_derived`]) and the kit carries on the units' pages
    /// (`derived` on a unit), in change frames (`derived.units.<unit>.<name>`, the text, or
    /// `null` when it has none now) and to a host's per-unit read ([`ViewIndex::unit`]). At
    /// most [`UNIT_DERIVED_MAX`] names, each 1 to [`DERIVED_NAME_MAX`] bytes of ASCII letters,
    /// digits, `_`, `-` or `.`, none twice and none a name of the grid's
    /// [`crate::grid::GridSlot::cell_derived`]. A value is cut to [`VIEW_TEXT_MAX`] bytes; a
    /// name not declared here is dropped. Empty (the default): none.
    pub unit_derived: &'static [&'static str],
    /// The names of the pack's **derived values per container**: small texts only the pack
    /// works out for a container (a timeline track's label and kind), which its derive hook
    /// sets on each container ([`Container::set_derived`]), the container half of
    /// [`ViewSchema::unit_derived`] under the same rules. The kit carries them on the
    /// snapshot's containers (`derived` on a container), in change frames
    /// (`derived.containers.<container>.<name>`, the text, or `null` when it has none now; beside
    /// the pack's own keys there, which win) and to a host's read ([`ViewIndex::container`]). At
    /// most [`CONTAINER_DERIVED_MAX`] names, each 1 to [`DERIVED_NAME_MAX`] bytes of ASCII
    /// letters, digits, `_`, `-` or `.`, none twice. A value is cut to [`VIEW_TEXT_MAX`] bytes;
    /// a name not declared here is dropped; the kit keeps them in this order. Empty (the
    /// default): none. (Not [`Derived::containers`], which names the view fields the hook
    /// sets.)
    pub container_derived: &'static [&'static str],
    /// The 3D slot: a unit's (and a container's) placement in the doc's space
    /// ([`crate::placement`], [`crate::session::Pack::view_placement`]). `None`: no 3D.
    pub space: Option<SpaceSlot>,
    /// Which unit kinds nest inside which ([`Nesting`]): at most [`NESTING_MAX`], each kind 1
    /// to [`NESTING_KIND_MAX`] bytes, no pair twice. On the wire as `nesting` only when there
    /// are some, so a schema without it encodes as before and an older reader ignores it.
    /// Empty (the default): none declared.
    pub nesting: &'static [Nesting],
    /// The unit a participant's time position counts in (`"tick"`: a timeline's), as a word:
    /// what [`crate::session::TimePosition::at`] means on this doc. 1 to 64 bytes of ASCII
    /// letters, digits, `_`, `-` or `.`. On the wire as `time_unit` only when declared.
    /// `None` (the default): the pack declares none.
    pub time_unit: Option<&'static str>,
}

impl ViewSchema {
    /// Nothing declared: a pack's schema starts from it.
    pub const NONE: ViewSchema = ViewSchema {
        containers: "",
        units: "",
        unit_container_field: "",
        container_id_field: None,
        native_unit: "",
        page: None,
        container_fields: ContainerFields::NONE,
        unit_fields: UnitFields::NONE,
        geometry: None,
        field_class: &[],
        field_class_prefix: &[],
        derived: Derived::NONE,
        blob: None,
        grid: None,
        unit_derived: &[],
        container_derived: &[],
        space: None,
        nesting: &[],
        time_unit: None,
    };

    /// Why this schema doesn't fit the doc's `schema` and `composites`
    /// ([`crate::session::Pack::composite`]), or `Ok`: the containers and units are two
    /// MovableList roots; the units have a composite map (their container field is in it), as
    /// the containers do when a container field is read from one; the page's root is a Map; the
    /// geometry has parts, none twice.
    pub fn check(
        &self,
        schema: &Schema,
        composites: &[(&'static str, &'static str)],
    ) -> Result<(), String> {
        let list = |r: &str| schema.kind_of(r) == Some(Kind::MovableList);
        if !list(self.containers) || !list(self.units) || self.containers == self.units {
            return Err(format!(
                "the view schema's containers {:?} and units {:?} must be two list roots",
                self.containers, self.units
            ));
        }
        let fields_of = |l: &str| composites.iter().any(|(_, x)| *x == l);
        if !fields_of(self.units) || self.unit_container_field.is_empty() {
            return Err(format!(
                "the view schema's units {:?} need a composite map holding {:?}",
                self.units, self.unit_container_field
            ));
        }
        let c = self.container_fields;
        let reads_field = [c.title, c.hidden, c.layout, c.parent]
            .iter()
            .any(|s| matches!(s, Src::Field(_) | Src::Values { .. }));
        if (self.container_id_field.is_some() || reads_field) && !fields_of(self.containers) {
            return Err(format!(
                "the view schema reads fields of {:?}, which has no composite map",
                self.containers
            ));
        }
        if let Some(p) = self.page {
            if schema.kind_of(p.root) != Some(Kind::Map) {
                return Err(format!("the page's root {:?} isn't a map", p.root));
            }
        }
        if let Some(g) = self.geometry {
            let mut seen = Vec::new();
            for p in g.parts {
                if seen.contains(p) {
                    return Err(format!("the geometry names {} twice", p.as_str()));
                }
                seen.push(*p);
            }
            if g.parts.is_empty() || g.field.is_empty() {
                return Err("the geometry has no field or no parts".into());
            }
        }
        if self.space.is_some_and(|sp| sp.unit_fields.is_empty()) {
            return Err("the 3D slot names no unit field".into());
        }
        if self.unit_derived.len() > UNIT_DERIVED_MAX {
            return Err(format!(
                "the schema declares {} unit derived values, at most {UNIT_DERIVED_MAX}",
                self.unit_derived.len()
            ));
        }
        for (i, n) in self.unit_derived.iter().enumerate() {
            if !derived_name(n) {
                return Err(format!(
                    "the unit derived value's name {n:?} isn't 1 to {DERIVED_NAME_MAX} bytes of \
                     ASCII letters, digits, `_`, `-` or `.`"
                ));
            }
            if self.unit_derived[..i].contains(n) {
                return Err(format!("the unit derived value {n:?} is declared twice"));
            }
            if self.grid.is_some_and(|g| g.cell_derived.contains(n)) {
                return Err(format!(
                    "the unit derived value {n:?} is also a cell derived value of the grid"
                ));
            }
        }
        if self.container_derived.len() > CONTAINER_DERIVED_MAX {
            return Err(format!(
                "the schema declares {} container derived values, at most \
                 {CONTAINER_DERIVED_MAX}",
                self.container_derived.len()
            ));
        }
        for (i, n) in self.container_derived.iter().enumerate() {
            if !derived_name(n) {
                return Err(format!(
                    "the container derived value's name {n:?} isn't 1 to {DERIVED_NAME_MAX} \
                     bytes of ASCII letters, digits, `_`, `-` or `.`"
                ));
            }
            if self.container_derived[..i].contains(n) {
                return Err(format!(
                    "the container derived value {n:?} is declared twice"
                ));
            }
        }
        if let Some(u) = self.time_unit {
            let good = !u.is_empty()
                && u.len() <= 64
                && u.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'));
            if !good {
                return Err(format!(
                    "the time unit {u:?} isn't 1 to 64 bytes of ASCII letters, digits, `_`, `-` \
                     or `.`"
                ));
            }
        }
        if self.nesting.len() > NESTING_MAX {
            return Err(format!(
                "the schema declares {} nesting relations, at most {NESTING_MAX}",
                self.nesting.len()
            ));
        }
        for (i, n) in self.nesting.iter().enumerate() {
            let good = |k: &str| !k.is_empty() && k.len() <= NESTING_KIND_MAX;
            if !good(n.kind) || !good(n.parent) {
                return Err(format!(
                    "the nesting of {:?} in {:?} names a kind that isn't 1 to {NESTING_KIND_MAX} \
                     bytes",
                    n.kind, n.parent
                ));
            }
            if self.nesting[..i].contains(n) {
                return Err(format!(
                    "the nesting of {:?} in {:?} is declared twice",
                    n.kind, n.parent
                ));
            }
        }
        if let Some(g) = &self.grid {
            let map_of = |m: &str, l: &str| {
                schema.kind_of(m) == Some(Kind::Map) && composites.contains(&(m, l))
            };
            if g.rows != self.units || !list(g.cols) || g.cols == g.rows {
                return Err(format!(
                    "the grid's rows {:?} must be the units and its columns {:?} another list root",
                    g.rows, g.cols
                ));
            }
            if !map_of(g.cells, g.rows) || !map_of(g.col_fields, g.cols) {
                return Err(format!(
                    "the grid's cells {:?} and column fields {:?} must be the composite maps of its \
                     rows and columns",
                    g.cells, g.col_fields
                ));
            }
            if g.col_container_field.is_empty()
                || g.row_numbering.gap_field.is_empty()
                || g.col_numbering.gap_field.is_empty()
                || !g.col_of_cell_field.contains("{field}")
            {
                return Err(
                    "the grid needs a column container field, both gap fields and a cell field's \
                     column template naming {field}"
                        .into(),
                );
            }
            if g.max_rows == 0 || g.max_cols == 0 {
                return Err("the grid's last row and column must be at least 1".into());
            }
        }
        Ok(())
    }

    /// What `field` holds: its exact [`ViewSchema::field_class`] entry, else the first
    /// [`ViewSchema::field_class_prefix`] rule it starts with; `None` when neither names it.
    pub fn class_of_field(&self, field: &str) -> Option<FieldClass> {
        if let Some((_, c)) = self.field_class.iter().find(|(f, _)| *f == field) {
            return Some(*c);
        }
        self.field_class_prefix
            .iter()
            .find(|(p, _)| !p.is_empty() && field.starts_with(p))
            .map(|(_, c)| *c)
    }

    /// The schema as the watch snapshot carries it (contract §1.2). `field_class_prefix`,
    /// `grid`, `unit_derived`, `container_derived`, `nesting` and the container fields' `parent`
    /// are there only when the pack declares them, so a schema without them encodes as before.
    pub fn to_value(&self) -> Json {
        let src = |s: Src| -> Json {
            match s {
                Src::None => Json::Null,
                Src::Field(f) => Json::from(f),
                Src::ListValue => Json::obj([("list_value", Json::from(true))]),
                Src::Derived(d) => Json::obj([("derived", Json::from(d))]),
                Src::Values { field, values } => Json::obj([
                    ("field", Json::from(field)),
                    (
                        "values",
                        Json::obj(values.iter().map(|(r, w)| (*r, Json::from(*w)))),
                    ),
                ]),
            }
        };
        let c = self.container_fields;
        let u = self.unit_fields;
        let mut container_fields = vec![
            ("title".to_owned(), src(c.title)),
            ("hidden".to_owned(), src(c.hidden)),
            ("layout".to_owned(), src(c.layout)),
        ];
        if c.parent != Src::None {
            container_fields.push(("parent".to_owned(), src(c.parent)));
        }
        let out = Json::obj([
            ("containers", Json::from(self.containers)),
            ("units", Json::from(self.units)),
            (
                "unit_container_field",
                Json::from(self.unit_container_field),
            ),
            ("container_id_field", Json::from(self.container_id_field)),
            ("native_unit", Json::from(self.native_unit)),
            (
                "page",
                self.page.map_or(Json::Null, |p| {
                    Json::obj([
                        ("root", Json::from(p.root)),
                        ("w", Json::from(p.w)),
                        ("h", Json::from(p.h)),
                    ])
                }),
            ),
            ("container_fields", Json::Obj(container_fields)),
            (
                "unit_fields",
                Json::obj([
                    ("kind", src(u.kind)),
                    ("name", src(u.name)),
                    ("alt", src(u.alt)),
                    ("text", src(u.text)),
                    ("hidden", src(u.hidden)),
                    ("editable_text", src(u.editable_text)),
                    ("placeholder", src(u.placeholder)),
                ]),
            ),
            (
                "geometry",
                self.geometry.map_or(Json::Null, |g| {
                    Json::obj([
                        ("field", Json::from(g.field)),
                        ("parts", Json::list(g.parts.iter().map(|p| p.as_str()))),
                        ("rot_unit", Json::from(g.rot_unit)),
                    ])
                }),
            ),
            (
                "field_class",
                Json::obj(
                    self.field_class
                        .iter()
                        .map(|(f, c)| (*f, Json::from(c.as_str()))),
                ),
            ),
            (
                "derived",
                Json::obj([
                    (
                        "containers",
                        Json::list(self.derived.containers.iter().copied()),
                    ),
                    ("units", Json::list(self.derived.units.iter().copied())),
                ]),
            ),
        ]);
        let Json::Obj(mut out) = out else {
            unreachable!("an object")
        };
        if !self.field_class_prefix.is_empty() {
            out.push((
                "field_class_prefix".into(),
                Json::List(
                    self.field_class_prefix
                        .iter()
                        .map(|(p, c)| Json::list([*p, c.as_str()]))
                        .collect(),
                ),
            ));
        }
        if let Some(g) = &self.grid {
            out.push(("grid".into(), g.to_value()));
        }
        if let Some(sp) = &self.space {
            out.push(("space".into(), sp.to_value()));
        }
        if !self.unit_derived.is_empty() {
            out.push((
                "unit_derived".into(),
                Json::list(self.unit_derived.iter().copied()),
            ));
        }
        if !self.container_derived.is_empty() {
            out.push((
                "container_derived".into(),
                Json::list(self.container_derived.iter().copied()),
            ));
        }
        if let Some(u) = self.time_unit {
            out.push(("time_unit".into(), Json::from(u)));
        }
        if !self.nesting.is_empty() {
            out.push((
                "nesting".into(),
                Json::List(
                    self.nesting
                        .iter()
                        .map(|n| {
                            Json::obj([
                                ("kind", Json::from(n.kind)),
                                ("parent", Json::from(n.parent)),
                            ])
                        })
                        .collect(),
                ),
            ));
        }
        Json::Obj(out)
    }
}

/// A good name of a derived value ([`ViewSchema::unit_derived`],
/// [`ViewSchema::container_derived`]): 1 to [`DERIVED_NAME_MAX`] bytes of ASCII letters,
/// digits, `_`, `-` or `.`.
fn derived_name(n: &str) -> bool {
    !n.is_empty()
        && n.len() <= DERIVED_NAME_MAX
        && n.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

// ----- the index -----

/// Which source a [`ViewIndex`] was read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tier {
    /// A live doc's keyed state.
    #[default]
    Live,
    /// One version's structure index.
    Blob,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::Live => "live",
            Tier::Blob => "blob",
        }
    }
}

/// The page: the container's drawing area, in native units.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Page {
    pub w: i64,
    pub h: i64,
    pub unit: String,
}

/// One font the doc uses (the pack's): a family name, never font bytes.
///
/// **Build it with `..FontUse::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FontUse {
    pub family: String,
    /// `latin`, `ea`, `cs` or `sym`.
    pub script: String,
    /// `theme-major`, `theme-minor` or `explicit`.
    pub from: String,
    pub embedded: bool,
}

/// A layout, by its address, with its own name and its master's address.
///
/// **Build it with `..Layout::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Layout {
    pub key: String,
    pub name: String,
    pub master: Option<String>,
}

/// One container (a slide).
///
/// **Build it with `..Container::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Container {
    /// Its address.
    pub key: String,
    /// From 1, in the containers' order.
    pub position: u32,
    /// Cut to [`VIEW_TEXT_MAX`]; `None` when the source doesn't carry one.
    pub title: Option<String>,
    pub hidden: bool,
    /// Its layout's address (its name is in [`ViewIndex::layouts`]); `None` when it has none, or
    /// the value the source gave isn't a layout reference ([`ViewRefusal::Layout`]).
    pub layout: Option<String>,
    /// The container it sits inside, by address (a collection inside a collection); `None` at
    /// the top level. An outliner draws the containers' tree from it, from the index alone.
    /// Always another container of the same index, never in a cycle, at most
    /// [`CONTAINER_DEPTH_MAX`] levels deep: the kit checks it when it builds the index
    /// ([`ContainerFields::parent`]; a parent that fails is `None` here and a
    /// [`ViewRefusal::Parent`]). `position` stays the containers' flat order. On the wire only
    /// when set.
    pub parent: Option<String>,
    /// Keyed by the space's key: never a raw digest. Set by the pack's render hash.
    pub render_hash: Option<NodeHash>,
    /// Indexes into [`ViewIndex::fonts`].
    pub fonts: Vec<u32>,
    /// The field kinds its units' text holds (`slidenum`, `datetime`).
    pub has_fields: Vec<String>,
    /// Its units, nested ones included (the kit counts them).
    pub unit_count: u32,
    /// Its placement, for a schema whose 3D slot has container fields.
    pub placement: Option<Placement>,
    /// The pack's derived values ([`ViewSchema::container_derived`]): `(name, text)`, in the
    /// schema's order once the kit has finished the index, each name once, each text cut to
    /// [`VIEW_TEXT_MAX`]. Set them with [`Container::set_derived`]. On the wire only when there
    /// are some.
    pub derived: Vec<(String, String)>,
}

impl Container {
    /// Set the derived value `name` to `text` (replacing one already set). The kit keeps it
    /// only when the schema declares `name` ([`ViewSchema::container_derived`]), cut to
    /// [`VIEW_TEXT_MAX`].
    pub fn set_derived(&mut self, name: &str, text: impl Into<String>) {
        let text = text.into();
        match self.derived.iter_mut().find(|(n, _)| n == name) {
            Some((_, t)) => *t = text,
            None => self.derived.push((name.to_owned(), text)),
        }
    }

    /// The derived value `name`, if the container has one.
    pub fn derived_value(&self, name: &str) -> Option<&str> {
        self.derived
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.as_str())
    }
}

/// A box in native units, page space: the transform box, never the ink.
///
/// **Build it with `..Bbox::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Bbox {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// A unit's flips.
///
/// **Build it with `..Flip::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flip {
    pub h: bool,
    pub v: bool,
}

/// A unit's text, cut to [`VIEW_TEXT_MAX`].
///
/// **Build it with `..UnitText::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct UnitText {
    pub summary: String,
    pub truncated: bool,
}

/// A placeholder's type and index, as the file writes them.
///
/// **Build it with `..Placeholder::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Placeholder {
    pub ty: String,
    pub idx: Option<String>,
}

/// Whether an agent can set a unit's text, and at what cost (contract Item 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditableText {
    Plain,
    /// Only with its mixed formatting dropped.
    Formatted,
    Never,
}

impl EditableText {
    pub fn as_str(self) -> &'static str {
        match self {
            EditableText::Plain => "plain",
            EditableText::Formatted => "formatted",
            EditableText::Never => "never",
        }
    }

    /// The contract's word.
    pub fn parse(word: &str) -> Option<EditableText> {
        match word {
            "plain" => Some(EditableText::Plain),
            "formatted" => Some(EditableText::Formatted),
            "never" => Some(EditableText::Never),
            _ => None,
        }
    }
}

/// One unit (a shape).
///
/// **Build it with `..Unit::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Unit {
    /// Its address.
    pub key: String,
    /// Its container's address.
    pub container: String,
    /// The unit it is nested in (a group), by address.
    pub parent: Option<String>,
    /// A top-level unit: an element of the live store's units, which can be selected and
    /// claimed. A nested unit (a group's child) is read from the file and never is.
    pub live: bool,
    pub kind: String,
    /// Cut to [`VIEW_TEXT_MAX`].
    pub name: String,
    /// `None` until the pack says.
    pub name_is_generic: Option<bool>,
    /// Cut to [`VIEW_TEXT_MAX`].
    pub alt: Option<String>,
    /// 0 at the back, within its parent (or its container).
    pub z: u32,
    pub bbox: Option<Bbox>,
    pub bbox_inherited: bool,
    /// Clockwise about the box centre, in the geometry's `rot_unit`.
    pub rot: i64,
    pub flip: Flip,
    pub approx_box: bool,
    pub text: Option<UnitText>,
    /// The field kinds its text holds.
    pub fields: Vec<String>,
    pub hidden: bool,
    pub placeholder: Option<Placeholder>,
    /// `None` when the source doesn't say.
    pub editable_text: Option<EditableText>,
    /// The pack's derived values ([`ViewSchema::unit_derived`]): `(name, text)`, in the schema's
    /// order once the kit has finished the index, each name once, each text cut to
    /// [`VIEW_TEXT_MAX`]. Set them with [`Unit::set_derived`].
    pub derived: Vec<(String, String)>,
    /// Its placement, for a schema with a 3D slot (`None`: not placed by the slot).
    pub placement: Option<Placement>,
}

impl Unit {
    /// Set the derived value `name` to `text` (replacing one already set). The kit keeps it
    /// only when the schema declares `name` ([`ViewSchema::unit_derived`]), cut to
    /// [`VIEW_TEXT_MAX`].
    pub fn set_derived(&mut self, name: &str, text: impl Into<String>) {
        let text = text.into();
        match self.derived.iter_mut().find(|(n, _)| n == name) {
            Some((_, t)) => *t = text,
            None => self.derived.push((name.to_owned(), text)),
        }
    }

    /// The derived value `name`, if the unit has one.
    pub fn derived_value(&self, name: &str) -> Option<&str> {
        self.derived
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, t)| t.as_str())
    }
}

/// The view index of one rev of a live doc, or of one version of a blob doc (contract Item 1).
///
/// **Build it with `..ViewIndex::default()`**, setting the fields you have: the kit adds
/// fields to it in minor releases, and a full literal stops compiling when it does.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewIndex {
    pub tier: Tier,
    /// The live doc's change number (live only).
    pub rev: Option<u64>,
    /// The version (always set on the blob tier).
    pub version: Option<u64>,
    pub page: Option<Page>,
    /// Set by the pack.
    pub fonts: Vec<FontUse>,
    pub layouts: Vec<Layout>,
    /// In their order.
    pub containers: Vec<Container>,
    /// The doc's space, for a schema with a 3D slot.
    pub space: Option<Space>,
    /// By container, in the containers' order; back to front within one; a nested unit right
    /// after its parent.
    pub units: Vec<Unit>,
    /// What the kit refused of what the pack gave it, in the containers' order: each value is
    /// not carried. Empty for a pack whose values are all good. Not on the wire: the host's.
    pub refusals: Vec<ViewRefusal>,
}

/// A value the view index refused when it was built ([`ViewIndex::refusals`]): the index
/// leaves it out, so a view never draws from it, and a test build fails on it loudly.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ViewRefusal {
    /// A container's layout ([`ContainerFields::layout`], [`BlobView::container_layout`], or
    /// the pack's derive hook) that isn't a layout reference: its [`Container::layout`] is
    /// `None`. `container` is the container's address, `value` what the source gave.
    Layout {
        container: String,
        value: String,
        why: LayoutRefused,
    },
    /// A container's parent ([`ContainerFields::parent`], [`BlobView::container_parent`], or
    /// the pack's derive hook) that the index can't carry: its [`Container::parent`] is `None`
    /// (it is listed at the top level, in its place in the flat order). `container` is the
    /// container's address; `value` the parent's address as the kit read it (a live value
    /// mapped from its key), or, for [`ParentRefused::NotAKey`], the value as the store holds
    /// it.
    Parent {
        container: String,
        value: String,
        why: ParentRefused,
    },
}

/// Why a container's parent was refused ([`ViewRefusal::Parent`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParentRefused {
    /// A live value that isn't a key of the kit's grammar ([`crate::elem::Key`]), so it names
    /// no element of the containers' list.
    NotAKey,
    /// It names no container of the index.
    NoSuchContainer,
    /// It names the container itself.
    OwnParent,
    /// It closes a cycle of parents. Each cycle is broken at one container: of those on it, the
    /// first in the containers' order has its parent refused; the others keep theirs.
    Cycle,
    /// The container would sit deeper than [`CONTAINER_DEPTH_MAX`] levels.
    TooDeep,
}

impl ParentRefused {
    pub fn as_str(self) -> &'static str {
        match self {
            ParentRefused::NotAKey => "not_a_key",
            ParentRefused::NoSuchContainer => "no_such_container",
            ParentRefused::OwnParent => "own_parent",
            ParentRefused::Cycle => "cycle",
            ParentRefused::TooDeep => "too_deep",
        }
    }
}

/// Why a container's layout was refused ([`ViewRefusal::Layout`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum LayoutRefused {
    /// It doesn't parse as an address in the kit's grammar ([`crate::addr::parse`], with
    /// native IDs): empty, or a malformed native ID (`@layout`) or key (`@k:`).
    NotAnAddress,
    /// It is an address, but the index holds layouts and none of them is it.
    NoSuchLayout,
}

impl LayoutRefused {
    pub fn as_str(self) -> &'static str {
        match self {
            LayoutRefused::NotAnAddress => "not_an_address",
            LayoutRefused::NoSuchLayout => "no_such_layout",
        }
    }
}

/// Why `value` isn't a layout reference of an index whose layouts are `layouts` (`None`: it
/// is one): it must parse as an address, and name one of them when there are some.
fn layout_refused(value: &str, layouts: &HashSet<&str>) -> Option<LayoutRefused> {
    if crate::addr::parse(value, crate::addr::Forms::none().native()).is_err() {
        return Some(LayoutRefused::NotAnAddress);
    }
    if !layouts.is_empty() && !layouts.contains(value) {
        return Some(LayoutRefused::NoSuchLayout);
    }
    None
}

/// The planted fault [`crate::differential::ViewMutant::LayoutUnchecked`].
fn planted_layout_unchecked() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::LayoutUnchecked)
    )
}

/// A container's derived values as the schema declares them: its names only, each once, in
/// the schema's order, each text cut to [`VIEW_TEXT_MAX`].
fn declared(values: &[(String, String)], names: &[&str]) -> Vec<(String, String)> {
    let mut kept: Vec<(String, String)> = Vec::with_capacity(values.len());
    if planted_container_derived_order_lost() {
        // the pack's order, its names filtered
        for (n, t) in values {
            if names.contains(&n.as_str()) && !kept.iter().any(|(k, _)| k == n) {
                kept.push((n.clone(), cut(t)));
            }
        }
        return kept;
    }
    for name in names {
        if let Some((n, t)) = values.iter().find(|(n, _)| n == name) {
            kept.push((n.clone(), cut(t)));
        }
    }
    if planted_container_derived_undeclared_kept() {
        for (n, t) in values {
            if !kept.iter().any(|(k, _)| k == n) {
                kept.push((n.clone(), cut(t)));
            }
        }
    }
    kept
}

/// The planted fault [`crate::differential::ViewMutant::ContainerDerivedUndeclaredKept`].
fn planted_container_derived_undeclared_kept() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::ContainerDerivedUndeclaredKept)
    )
}

/// The planted fault [`crate::differential::ViewMutant::ContainerDerivedOrderLost`].
fn planted_container_derived_order_lost() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::ContainerDerivedOrderLost)
    )
}

/// The planted fault [`crate::differential::ViewMutant::ContainerDerivedDroppedOnWire`].
fn planted_container_derived_dropped_on_wire() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::ContainerDerivedDroppedOnWire)
    )
}

/// The planted fault [`crate::differential::ViewMutant::ParentUnchecked`].
fn planted_parent_unchecked() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::ParentUnchecked)
    )
}

/// The planted fault [`crate::differential::ViewMutant::ParentNotValidated`].
fn planted_parent_not_validated() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::ParentNotValidated)
    )
}

/// The planted fault [`crate::differential::ViewMutant::ParentCycleAccepted`].
fn planted_parent_cycle_accepted() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::ParentCycleAccepted)
    )
}

/// The planted fault [`crate::differential::ViewMutant::ParentDepthUnbounded`].
fn planted_parent_depth_unbounded() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::ParentDepthUnbounded)
    )
}

/// The planted fault [`crate::differential::ViewMutant::ParentDroppedOnWire`].
fn planted_parent_dropped_on_wire() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::ParentDroppedOnWire)
    )
}

/// One container's units' derived values, by unit address, each `(name, text)`
/// ([`crate::session::Session::view_derived`]).
pub type UnitsDerived = Vec<(String, Vec<(String, String)>)>;

/// The units' derived values a change set (`Some(text)`) or removed (`None`), by unit address,
/// each `(name, value)` ([`ViewChange::unit_derived`]).
pub type DerivedChanges = Vec<(String, Vec<(String, Option<String>)>)>;

/// What a pack's derive hook reads ([`crate::session::Pack::view_derive`]).
#[derive(Debug, Clone, Copy)]
pub enum ViewSource<'a> {
    /// The live doc's keyed state the index was built from, and its carrier when the pack
    /// keeps one and the kit holds it ([`ViewCarrier`]): the file's bytes, for what only they
    /// hold. **The live store wins:** what the store models comes from the state, and the
    /// carrier supplies only what it doesn't.
    Live(&'a KeyedState, Option<&'a ViewCarrier>),
    /// The version's structure index.
    Blob(&'a IndexView),
}

/// The view index around one change, as a keyed watch's change frame reads it
/// ([`crate::session::Pack::watch_extras`]): the index after the change and, when the kit holds
/// it, the one kept before it (the previous change's, while a watch is open), both through the
/// index the kit keeps by (carrier id, rev).
#[derive(Debug, Clone, Copy)]
pub struct ViewChange<'a> {
    pub before: Option<&'a ViewIndex>,
    pub after: &'a ViewIndex,
}

impl ViewChange<'_> {
    /// The units' derived values the change set or removed ([`ViewSchema::unit_derived`]), by
    /// unit address in the index's order after it: `(unit, [(name, Some(text) | None)])`. A
    /// unit after the change with a value it didn't have before, or another one, has
    /// `Some(text)`; a unit in both with a value before and none after has `None`; a unit only
    /// before (deleted) has nothing. Every unit's values when the index before isn't held.
    pub fn unit_derived(&self) -> DerivedChanges {
        let before: HashMap<&str, &Unit> = self
            .before
            .map(|b| b.units.iter().map(|u| (u.key.as_str(), u)).collect())
            .unwrap_or_default();
        let mut out = Vec::new();
        for u in &self.after.units {
            let was = before.get(u.key.as_str());
            let had = was.is_some_and(|w| !w.derived.is_empty());
            if u.derived.is_empty() && !had {
                continue;
            }
            let mut d: Vec<(String, Option<String>)> = Vec::new();
            for (n, t) in &u.derived {
                if was.and_then(|w| w.derived_value(n)) != Some(t.as_str()) {
                    d.push((n.clone(), Some(t.clone())));
                }
            }
            if let Some(w) = was {
                for (n, _) in &w.derived {
                    if u.derived_value(n).is_none() {
                        d.push((n.clone(), None));
                    }
                }
            }
            if !d.is_empty() {
                out.push((u.key.clone(), d));
            }
        }
        out
    }

    /// The containers' derived values the change set or removed
    /// ([`ViewSchema::container_derived`]), by container address in the index's order after
    /// it, as [`ViewChange::unit_derived`] gives the units': `(container, [(name, Some(text) |
    /// None)])`; nothing for a container only before (deleted); every container's values when
    /// the index before isn't held.
    pub fn container_derived(&self) -> DerivedChanges {
        let before: HashMap<&str, &Container> = self
            .before
            .map(|b| b.containers.iter().map(|c| (c.key.as_str(), c)).collect())
            .unwrap_or_default();
        let mut out = Vec::new();
        for c in &self.after.containers {
            let was = before.get(c.key.as_str());
            let had = was.is_some_and(|w| !w.derived.is_empty());
            if c.derived.is_empty() && !had {
                continue;
            }
            let mut d: Vec<(String, Option<String>)> = Vec::new();
            for (n, t) in &c.derived {
                if was.and_then(|w| w.derived_value(n)) != Some(t.as_str()) {
                    d.push((n.clone(), Some(t.clone())));
                }
            }
            if let Some(w) = was {
                for (n, _) in &w.derived {
                    if c.derived_value(n).is_none() {
                        d.push((n.clone(), None));
                    }
                }
            }
            if !d.is_empty() {
                out.push((c.key.clone(), d));
            }
        }
        out
    }

    /// The containers whose render hash changed, by address, in order: each with a hash after
    /// the change that differs from its hash before it (or that wasn't there before); every
    /// container with a hash when the index before isn't held.
    pub fn render_hashes(&self) -> Vec<(String, NodeHash)> {
        self.after
            .containers
            .iter()
            .filter_map(|c| {
                let now = c.render_hash?;
                let was = self
                    .before
                    .map(|b| b.containers.iter().find(|x| x.key == c.key));
                match was {
                    Some(Some(x)) if x.render_hash == Some(now) => None,
                    _ => Some((c.key.clone(), now)),
                }
            })
            .collect()
    }
}

/// A live doc's **carrier** as the derive hook reads it (`docs/SESSIONS.md`, "The carrier"
/// ): the bytes the kit last read from the doc's file or wrote to it, which hold what the
/// live store doesn't model (a deck's fonts, a group's children, a layout's boxes).
///
/// - **Decoded once per carrier** ([`ViewCarrier::decoded`]): the pack's decode runs on the
///   first read after the carrier changes and its result is kept with the carrier, so a read
///   after a change of the doc reuses it; only a new carrier (the kit's write, an outside save
///   taken in, a reload) is decoded again.
/// - **Named by its id** ([`ViewCarrier::id`]), the keyed hash of its bytes: the kit keeps a
///   doc's index by (carrier id, rev) and reuses it until either changes
///   ([`crate::session::Session::view_index`]).
/// - **Never leaves the kit** but to the hook: a raw digest the pack works out from it is keyed
///   here before it goes in the index ([`ViewCarrier::render_hash`]).
pub struct ViewCarrier {
    id: [u8; 32],
    bytes: std::sync::Arc<[u8]>,
    /// The key render hashes are keyed under.
    hash_key: [u8; 32],
    /// The pack's decode of the bytes, shared by every handle on the same bytes (a new
    /// pairing on the same carrier keeps it).
    decoded: std::sync::Arc<std::sync::OnceLock<Option<Box<dyn std::any::Any + Send + Sync>>>>,
    /// The carrier's pairing, for a pack that pairs ([`crate::session::Pack::wants_pairing`]).
    pairing: Option<std::sync::Arc<crate::session::Pairing>>,
}

impl std::fmt::Debug for ViewCarrier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ViewCarrier({} bytes, decoded: {})",
            self.bytes.len(),
            self.decoded.get().is_some()
        )
    }
}

impl ViewCarrier {
    /// The carrier `bytes` of a doc whose hash key is `doc_key`: its id is their keyed hash.
    pub(crate) fn new(bytes: std::sync::Arc<[u8]>, doc_key: &[u8; 32]) -> ViewCarrier {
        ViewCarrier {
            id: crate::hash::hmac(doc_key, &bytes),
            bytes,
            hash_key: crate::index::digest_key(doc_key),
            decoded: std::sync::Arc::new(std::sync::OnceLock::new()),
            pairing: None,
        }
    }

    /// The same carrier with `pairing` (its decode shared, not run again).
    pub(crate) fn with_pairing(
        &self,
        pairing: Option<std::sync::Arc<crate::session::Pairing>>,
    ) -> ViewCarrier {
        ViewCarrier {
            id: self.id,
            bytes: self.bytes.clone(),
            hash_key: self.hash_key,
            decoded: self.decoded.clone(),
            pairing,
        }
    }

    /// Whether this handle carries `pairing` (the same one, or none of both).
    pub(crate) fn has_pairing(
        &self,
        pairing: Option<&std::sync::Arc<crate::session::Pairing>>,
    ) -> bool {
        match (&self.pairing, pairing) {
            (None, None) => true,
            (Some(a), Some(b)) => std::sync::Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    /// The pairing this handle carries, as the kit keeps it.
    pub(crate) fn pairing_arc(&self) -> Option<&std::sync::Arc<crate::session::Pairing>> {
        self.pairing.as_ref()
    }

    /// The carrier's id: the keyed hash of its bytes, the same for the same bytes in one doc.
    pub fn id(&self) -> &[u8; 32] {
        &self.id
    }

    /// The carrier's bytes, exactly as the file held them at the sync point.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The carrier's pairing ([`crate::session::Pairing`]): the keys of the units it holds,
    /// root by root, in the order the pack's decode of it yields them, as the kit made it at the
    /// sync point the carrier was taken (`docs/SESSIONS.md`, "The carrier's pairing"). A view
    /// hook finds a unit's file-only facts by its key through it, not by its live position, so
    /// units that come, go or move between carriers don't shift them: a key the pairing doesn't
    /// name (a unit added since) has none. A read-only borrow; `None` for a pack that doesn't
    /// pair ([`crate::session::Pack::wants_pairing`]) and while the kit holds no pairing.
    pub fn pairing(&self) -> Option<&crate::session::Pairing> {
        self.pairing.as_deref()
    }

    /// Whether these are the carrier `bytes` the session holds (the same allocation).
    pub(crate) fn holds(&self, bytes: &std::sync::Arc<[u8]>) -> bool {
        std::sync::Arc::ptr_eq(&self.bytes, bytes)
    }

    /// What `decode` makes of the bytes, **decoded once per carrier**: the first call runs
    /// `decode` (one carrier decode, counted by `crate::differential::take_carrier_decodes` in
    /// test builds) and keeps its result with the carrier; every later call, this read's or a
    /// later one's, gives it back until the carrier changes. `None` when `decode` gave none, or
    /// when the kept result isn't a `T` (a pack decodes one type).
    pub fn decoded<T: std::any::Any + Send + Sync>(
        &self,
        decode: impl FnOnce(&[u8]) -> Option<T>,
    ) -> Option<&T> {
        self.decoded
            .get_or_init(|| {
                #[cfg(any(test, feature = "differential"))]
                crate::differential::count_carrier_decode();
                decode(&self.bytes).map(|t| Box::new(t) as Box<dyn std::any::Any + Send + Sync>)
            })
            .as_ref()?
            .downcast_ref::<T>()
    }

    /// A container's render hash from the raw digest the pack worked out, keyed
    /// under the doc's key as every hash the index carries.
    pub fn render_hash(&self, digest: &Digest) -> NodeHash {
        crate::index::key_digest(&self.hash_key, digest)
    }
}

fn cut(s: &str) -> String {
    cut_text(s, VIEW_TEXT_MAX as u32).0.to_owned()
}

fn summary(s: &str, flagged: bool) -> UnitText {
    let (kept, was_cut) = cut_text(s, VIEW_TEXT_MAX as u32);
    UnitText {
        summary: kept.to_owned(),
        truncated: was_cut || flagged,
    }
}

/// An address or alt text: an empty value is none.
fn nonempty(v: &str) -> Option<String> {
    (!v.is_empty()).then(|| v.to_owned())
}

pub(crate) fn flag(v: Option<&str>) -> bool {
    matches!(v, Some("1" | "true"))
}

/// The contract word for a raw value under `src` (a [`Src::Values`] maps it; otherwise the raw
/// value is the word).
fn word(src: Src, raw: &str) -> Option<&str> {
    match src {
        Src::Values { values, .. } => values.iter().find(|(r, _)| *r == raw).map(|(_, w)| *w),
        _ => Some(raw),
    }
}

/// `<type>:<idx>`, either side empty.
fn placeholder(v: &str) -> Placeholder {
    let (ty, idx) = v.split_once(':').unwrap_or((v, ""));
    Placeholder {
        ty: ty.to_owned(),
        idx: (!idx.is_empty()).then(|| idx.to_owned()),
    }
}

/// A geometry value: its box, rotation and flips; `None` when it is empty or doesn't parse.
pub(crate) fn parse_geom(v: &str, parts: &[GeomPart]) -> Option<(Bbox, i64, Flip)> {
    if v.is_empty() {
        return None;
    }
    let vals: Vec<&str> = v.split(',').collect();
    if vals.len() != parts.len() {
        return None;
    }
    let (mut b, mut rot, mut flip) = (Bbox::default(), 0i64, Flip::default());
    for (p, s) in parts.iter().zip(vals) {
        let n = || s.trim().parse::<i64>().ok();
        let f = || match s.trim() {
            "0" | "false" => Some(false),
            "1" | "true" => Some(true),
            _ => None,
        };
        match p {
            GeomPart::X => b.x = n()?,
            GeomPart::Y => b.y = n()?,
            GeomPart::W => b.w = n()?,
            GeomPart::H => b.h = n()?,
            GeomPart::Rot => rot = n()?,
            GeomPart::FlipH => flip.h = f()?,
            GeomPart::FlipV => flip.v = f()?,
        }
    }
    Some((b, rot, flip))
}

fn root_of<'a>(state: &'a KeyedState, name: &str) -> Option<&'a KeyedRoot> {
    state.roots.iter().find(|(r, _)| r == name).map(|(_, r)| r)
}

fn live_of<'a>(state: &'a KeyedState, name: &str) -> &'a [(Key, Value)] {
    match root_of(state, name) {
        Some(KeyedRoot::List { live, .. }) => live,
        _ => &[],
    }
}

fn map_of<'a>(state: &'a KeyedState, name: &str) -> Option<&'a BTreeMap<String, Value>> {
    match root_of(state, name) {
        Some(KeyedRoot::Map(m)) => Some(m),
        _ => None,
    }
}

/// The composite map holding `list`'s element fields.
fn fields_root(composites: &[(&'static str, &'static str)], list: &str) -> Option<&'static str> {
    composites.iter().find(|(_, l)| *l == list).map(|(m, _)| *m)
}

/// One element's fields: its composite map and its key.
struct Elem<'a> {
    key: &'a Key,
    value: &'a Value,
    props: Option<&'a BTreeMap<String, Value>>,
}

impl<'a> Elem<'a> {
    fn field(&self, f: &str) -> Option<&'a str> {
        self.props?
            .get(&format!("{}{CELL_SEP}{f}", self.key.as_str()))?
            .as_str()
    }

    /// The value `src` reads, as a contract word for [`Src::Values`].
    fn read(&self, src: Src) -> Option<&'a str> {
        match src {
            Src::None | Src::Derived(_) => None,
            Src::ListValue => self.value.as_str(),
            Src::Field(f) => self.field(f),
            Src::Values { field, values } => {
                let raw = self.field(field)?;
                values.iter().find(|(r, _)| *r == raw).map(|(_, w)| *w)
            }
        }
    }
}

fn node_src(src: NodeSrc, n: &IndexNode) -> Option<&str> {
    match src {
        NodeSrc::None => None,
        NodeSrc::Kind => Some(&n.kind),
        NodeSrc::Name => Some(&n.name),
        NodeSrc::Text => n.text.as_deref(),
        NodeSrc::Attr(a) => n.attrs.get(a).map(String::as_str),
    }
}

/// The planted fault [`crate::differential::ViewMutant::SchemaFieldIgnored`].
fn planted_schema_field_ignored() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::SchemaFieldIgnored)
    )
}

/// The planted fault [`crate::differential::ViewMutant::AddressNotInverse`].
fn planted_address_not_inverse() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::AddressNotInverse)
    )
}

impl ViewIndex {
    /// The index of a live doc whose keyed state is `state` at change number `rev`, built by the
    /// kit from `pack`'s view schema, then finished by its derive hook. `None` when the pack has
    /// no view schema; a schema that doesn't fit the doc is the caller's to refuse first
    /// ([`ViewSchema::check`]; [`crate::session::Session::view_index`] does).
    ///
    /// - **Containers**: the containers' live elements, in order (`position` from 1), each by
    ///   its address ([`crate::session::Pack::address_of`]).
    /// - **Units**: the units' live elements, each in the container its container field names
    ///   (one that names no live container isn't shown), back to front in the units' list order;
    ///   `z` from 0 within its container; `live` true.
    /// - **Fields** as the schema says; a missing or unreadable field gives the empty value (no
    ///   box for a geometry that doesn't parse; `hidden` false), and an empty layout or alt text
    ///   is none.
    pub fn live<P: Pack + ?Sized>(pack: &P, state: &KeyedState, rev: u64) -> Option<ViewIndex> {
        Self::live_carried(pack, state, rev, None)
    }

    /// [`ViewIndex::live`] with the doc's carrier, which the derive hook reads for what
    /// the live store doesn't model ([`ViewSource::Live`]).
    pub fn live_carried<P: Pack + ?Sized>(
        pack: &P,
        state: &KeyedState,
        rev: u64,
        carrier: Option<&ViewCarrier>,
    ) -> Option<ViewIndex> {
        let schema = pack.view_schema()?;
        let mut ix = build_live(pack, &schema, state, rev, carrier);
        pack.view_derive(ViewSource::Live(state, carrier), &mut ix);
        ix.finish(&schema);
        Some(ix)
    }

    /// The index of one version of a blob doc, from its structure index `view`, built by the
    /// kit from `pack`'s view schema's [`BlobView`], then finished by its derive hook. `None`
    /// when the pack has no view schema or the schema no blob mapping.
    ///
    /// - **Containers**: the nodes of the container kind, in the outline's order (a node, then
    ///   its children by their sibling order), `position` from 1.
    /// - **Units**: every node under a container (not under another container, and not of a
    ///   skipped kind or under one), back to front (`z` from 0 within its parent), a nested
    ///   unit right after its parent; `parent` the unit it is under; `live` true for a unit
    ///   right under its container.
    /// - A node's box is its bounds; `hidden` its own visibility flag.
    /// - **The view items** (row format 2: [`crate::index::NodeView`], a node's render hash,
    ///   [`IndexView::fonts`]), when the version's row has them: the doc's fonts; a container's
    ///   render hash, fonts and field kinds (`has_fields`); a unit's box, rotation, flips,
    ///   `bbox_inherited` and `approx_box` (its view's geometry: none is no box), field kinds,
    ///   and its text and `editable_text` when its view gives them. A row of format 1 has none:
    ///   those fields stay as the plain fields give them (fewer details, never other ones).
    pub fn blob<P: BlobPack + ?Sized>(pack: &P, view: &IndexView) -> Option<ViewIndex> {
        let schema = pack.view_schema()?;
        let b = schema.blob?;
        let mut ix = build_blob(&schema, &b, view);
        pack.view_derive(ViewSource::Blob(view), &mut ix);
        ix.finish(&schema);
        Some(ix)
    }

    /// After the pack's hook: units in their containers' order (stable within one; a unit
    /// whose container isn't in the index is dropped), each container's `unit_count`, each
    /// unit's derived values as the schema declares them (its names, in its order, cut), and
    /// each container's layout and parent checked ([`ViewIndex::check_layouts`],
    /// [`ViewIndex::check_parents`]).
    fn finish(&mut self, schema: &ViewSchema) {
        let at: HashMap<String, usize> = self
            .containers
            .iter()
            .enumerate()
            .map(|(i, c)| (c.key.clone(), i))
            .collect();
        let mut keyed: Vec<(usize, Unit)> = std::mem::take(&mut self.units)
            .into_iter()
            .filter_map(|u| at.get(u.container.as_str()).map(|i| (*i, u)))
            .collect();
        keyed.sort_by_key(|(i, _)| *i);
        let mut counts = vec![0u32; self.containers.len()];
        for (i, _) in &keyed {
            counts[*i] += 1;
        }
        self.units = keyed.into_iter().map(|(_, u)| u).collect();
        for (c, n) in self.containers.iter_mut().zip(counts) {
            c.unit_count = n;
        }
        for u in &mut self.units {
            if u.derived.is_empty() {
                continue;
            }
            let mut kept = Vec::with_capacity(u.derived.len());
            for name in schema.unit_derived {
                if let Some((n, t)) = u.derived.iter().find(|(n, _)| n == name) {
                    kept.push((n.clone(), cut(t)));
                }
            }
            u.derived = kept;
        }
        for c in &mut self.containers {
            if !c.derived.is_empty() {
                c.derived = declared(&c.derived, schema.container_derived);
            }
        }
        self.check_refs();
        if cfg!(any(test, feature = "differential")) && !planted_cycle_unchecked() {
            if let Some(cycle) = self.placement_cycle() {
                panic!(
                    "{}: the pack's placement chain is a cycle: {} -> {}",
                    cycle[0],
                    cycle.join(" -> "),
                    cycle[0]
                );
            }
        }
    }

    /// Each container's layout and parent checked ([`ViewIndex::check_layouts`],
    /// [`ViewIndex::check_parents`]), and what they refused recorded in
    /// [`ViewIndex::refusals`], in the containers' order (a container's layout before its
    /// parent). The values refused before (an index kept through a change carries those of the
    /// containers it didn't build again) are checked again, so a kept index refuses what a
    /// whole build does, and a value that has become good since is taken.
    fn check_refs(&mut self) {
        let mut layouts: HashMap<String, String> = HashMap::new();
        let mut parents: HashMap<String, String> = HashMap::new();
        let mut not_keys: HashMap<String, String> = HashMap::new();
        for r in std::mem::take(&mut self.refusals) {
            match r {
                ViewRefusal::Layout {
                    container, value, ..
                } => {
                    layouts.insert(container, value);
                }
                ViewRefusal::Parent {
                    container,
                    value,
                    why: ParentRefused::NotAKey,
                } => {
                    not_keys.insert(container, value);
                }
                ViewRefusal::Parent {
                    container, value, ..
                } => {
                    parents.insert(container, value);
                }
            }
        }
        let mut refused = self.check_layouts(layouts);
        refused.extend(self.check_parents(parents, not_keys));
        refused.sort_by_key(|(i, _)| *i);
        self.refusals = refused.into_iter().map(|(_, r)| r).collect();
    }

    /// Each container's layout, checked against the kit's address grammar and the index's
    /// layouts ([`ContainerFields::layout`]): one that isn't a layout reference is taken out of
    /// the container and returned, with the container's place in the order, and a test build
    /// fails on it, naming it. `held`: the values refused before, by container address.
    fn check_layouts(&mut self, mut held: HashMap<String, String>) -> Vec<(usize, ViewRefusal)> {
        let layouts: HashSet<&str> = self.layouts.iter().map(|l| l.key.as_str()).collect();
        let mut refused = Vec::new();
        for (i, c) in self.containers.iter_mut().enumerate() {
            let Some(value) = c.layout.take().or_else(|| held.remove(&c.key)) else {
                continue;
            };
            match layout_refused(&value, &layouts) {
                None => c.layout = Some(value),
                Some(why) => refused.push((
                    i,
                    ViewRefusal::Layout {
                        container: c.key.clone(),
                        value,
                        why,
                    },
                )),
            }
        }
        if cfg!(any(test, feature = "differential")) && !planted_layout_unchecked() {
            if let Some((
                _,
                ViewRefusal::Layout {
                    container,
                    value,
                    why,
                },
            )) = refused.first()
            {
                panic!(
                    "{container}: the pack's layout {value:?} isn't a layout reference ({})",
                    why.as_str()
                );
            }
        }
        refused
    }

    /// Each container's parent ([`ContainerFields::parent`]), checked as one tree: it must name
    /// another container of the index (by address), no cycle may run through the parents, and
    /// no container may sit deeper than [`CONTAINER_DEPTH_MAX`] levels. A parent that fails is
    /// taken out of its container (which is then at the top level) and returned, with the
    /// container's place in the order; a test build fails on the first, naming it.
    ///
    /// - **A cycle** is broken at one container: of those on it, the first in the containers'
    ///   order has its parent refused ([`ParentRefused::Cycle`]); the others keep theirs, so the
    ///   rest of the tree stands. (Each container has one parent, so each cycle is its own and
    ///   the choice doesn't depend on where a walk starts.)
    /// - **Too deep**: the levels are counted from the top, after the cycles are broken; a
    ///   container that would be at level `CONTAINER_DEPTH_MAX + 1` has its parent refused
    ///   ([`ParentRefused::TooDeep`]) and is at level 1, the containers inside it counting from
    ///   there.
    ///
    /// `held`: the parents refused before, by container address, checked again; `not_keys`: the
    /// live values refused before as no keys ([`ParentRefused::NotAKey`]), kept for a container
    /// still in the index whose parent the pack hasn't set since.
    fn check_parents(
        &mut self,
        mut held: HashMap<String, String>,
        not_keys: HashMap<String, String>,
    ) -> Vec<(usize, ViewRefusal)> {
        let n = self.containers.len();
        let raw: Vec<Option<String>> = self
            .containers
            .iter_mut()
            .map(|c| c.parent.take().or_else(|| held.remove(&c.key)))
            .collect();
        // each parent as a container's place, and why one is refused
        let mut up: Vec<Option<usize>> = vec![None; n];
        let mut why: Vec<Option<ParentRefused>> = vec![None; n];
        {
            let mut at: HashMap<&str, usize> = HashMap::with_capacity(n);
            for (i, c) in self.containers.iter().enumerate() {
                at.entry(c.key.as_str()).or_insert(i);
            }
            let checked = !planted_parent_not_validated();
            for (i, r) in raw.iter().enumerate() {
                let Some(r) = r else { continue };
                match at.get(r.as_str()) {
                    Some(&j) if j == i => {
                        if checked {
                            why[i] = Some(ParentRefused::OwnParent);
                        }
                    }
                    Some(&j) => up[i] = Some(j),
                    None => {
                        if checked {
                            why[i] = Some(ParentRefused::NoSuchContainer);
                        }
                    }
                }
            }
        }
        // cycles: a walk up from each container not yet seen; one that comes back to its own
        // path has found a cycle, broken at its first member in order
        if !planted_parent_cycle_accepted() {
            // 0: not seen; 1: on the walk's path; 2: done
            let mut seen = vec![0u8; n];
            for s in 0..n {
                if seen[s] != 0 {
                    continue;
                }
                let mut path: Vec<usize> = Vec::new();
                let mut at = s;
                loop {
                    match seen[at] {
                        0 => {
                            seen[at] = 1;
                            path.push(at);
                            match up[at] {
                                Some(p) => at = p,
                                None => break,
                            }
                        }
                        1 => {
                            let from = path.iter().position(|&x| x == at).unwrap_or(0);
                            if let Some(&cut) = path[from..].iter().min() {
                                up[cut] = None;
                                why[cut] = Some(ParentRefused::Cycle);
                            }
                            break;
                        }
                        _ => break,
                    }
                }
                for x in path {
                    seen[x] = 2;
                }
            }
        }
        // levels from the top (1), each container's after its parent's
        if !planted_parent_depth_unbounded() {
            let mut level = vec![0usize; n];
            for s in 0..n {
                if level[s] != 0 {
                    continue;
                }
                // up to a container whose level is known, or the top (a walk longer than the
                // containers can only be a cycle a planted fault kept: it stops there)
                let mut chain = vec![s];
                let mut at = s;
                while let Some(p) = up[at] {
                    if level[p] != 0 || chain.len() > n {
                        break;
                    }
                    chain.push(p);
                    at = p;
                }
                for &x in chain.iter().rev() {
                    let l = up[x].map_or(1, |p| level[p] + 1);
                    if l > CONTAINER_DEPTH_MAX {
                        up[x] = None;
                        why[x] = Some(ParentRefused::TooDeep);
                        level[x] = 1;
                    } else {
                        level[x] = l;
                    }
                }
            }
        }
        let mut refused = Vec::new();
        for (i, (c, r)) in self.containers.iter_mut().zip(raw).enumerate() {
            match (r, why[i]) {
                (Some(value), Some(w)) => refused.push((
                    i,
                    ViewRefusal::Parent {
                        container: c.key.clone(),
                        value,
                        why: w,
                    },
                )),
                (Some(value), None) => c.parent = Some(value),
                (None, _) => {
                    if let Some(value) = not_keys.get(&c.key) {
                        refused.push((
                            i,
                            ViewRefusal::Parent {
                                container: c.key.clone(),
                                value: value.clone(),
                                why: ParentRefused::NotAKey,
                            },
                        ));
                    }
                }
            }
        }
        if cfg!(any(test, feature = "differential")) && !planted_parent_unchecked() {
            if let Some((
                _,
                ViewRefusal::Parent {
                    container,
                    value,
                    why,
                },
            )) = refused.first()
            {
                panic!(
                    "{container}: the pack's parent {value:?} isn't one the index carries ({})",
                    why.as_str()
                );
            }
        }
        refused
    }

    /// The first cycle among the placements' `relative_to` links, as the addresses around it
    /// from the first in address order, or `None`. A test build refuses an index with one
    /// ([`ViewIndex::finish`]); a release build keeps it, and the world matrix of anything
    /// placed through it is `None`, as for any broken chain.
    fn placement_cycle(&self) -> Option<Vec<String>> {
        // a unit before a container of the same address, as `world_matrix` looks them up
        let rel: HashMap<&str, &str> = self
            .containers
            .iter()
            .map(|c| (c.key.as_str(), c.placement.as_ref()))
            .chain(
                self.units
                    .iter()
                    .map(|u| (u.key.as_str(), u.placement.as_ref())),
            )
            .filter_map(|(a, p)| Some((a, p?.relative_to.as_deref()?)))
            .collect();
        let mut starts: Vec<&str> = rel.keys().copied().collect();
        starts.sort_unstable();
        let mut done: HashSet<&str> = HashSet::new();
        for start in starts {
            let mut path: Vec<&str> = Vec::new();
            let mut on: HashMap<&str, usize> = HashMap::new();
            let mut at = start;
            while !done.contains(at) {
                if let Some(&i) = on.get(at) {
                    return Some(path[i..].iter().map(|a| (*a).to_owned()).collect());
                }
                on.insert(at, path.len());
                path.push(at);
                match rel.get(at) {
                    Some(next) => at = next,
                    None => break,
                }
            }
            done.extend(path);
        }
        None
    }

    /// The unit whose address is `address` (a nested one too), or `None`.
    pub fn unit(&self, address: &str) -> Option<&Unit> {
        self.units.iter().find(|u| u.key == address)
    }

    /// The container whose address is `address`, or `None`.
    pub fn container(&self, address: &str) -> Option<&Container> {
        self.containers.iter().find(|c| c.key == address)
    }

    /// Every unit's derived values in the container whose address is `container`, back to
    /// front, by unit address: `(unit, [(name, text)])`, units with none left out. `None` for a
    /// container the index doesn't hold. What a host reads for one container without its
    /// units' pages.
    #[allow(clippy::type_complexity)]
    pub fn derived_of(&self, container: &str) -> Option<Vec<(&str, &[(String, String)])>> {
        Some(
            self.units_of(container)?
                .iter()
                .filter(|u| !u.derived.is_empty())
                .map(|u| (u.key.as_str(), u.derived.as_slice()))
                .collect(),
        )
    }

    /// The units of the container whose address is `container`, back to front: a slice of
    /// [`ViewIndex::units`]. `None` for a container the index doesn't hold.
    pub fn units_of(&self, container: &str) -> Option<&[Unit]> {
        if !self.containers.iter().any(|c| c.key == container) {
            return None;
        }
        let start = self.units.iter().position(|u| u.container == container);
        Some(match start {
            None => &[],
            Some(s) => {
                let n = self.units[s..]
                    .iter()
                    .take_while(|u| u.container == container)
                    .count();
                &self.units[s..s + n]
            }
        })
    }

    /// The watch snapshot's part of the index (contract Item 1, "On the wire"): everything but
    /// the units, which are read by container ([`ViewIndex::units_page`]).
    pub fn snapshot(&self) -> ViewSnapshot {
        ViewSnapshot {
            tier: self.tier,
            rev: self.rev,
            version: self.version,
            page: self.page.clone(),
            fonts: self.fonts.clone(),
            layouts: self.layouts.clone(),
            containers: self.containers.clone(),
            space: self.space.clone(),
        }
    }

    /// One page of the units of container `container` (its address), from its unit `from` (0 for
    /// the first page; then the page's `next`), as many as fit in `budget` bytes of the page's
    /// compact JSON ([`UnitsPage::to_value`]), and always at least one. Reading every page of
    /// every container gives [`ViewIndex::units`] back, in order.
    pub fn units_page(
        &self,
        container: &str,
        from: usize,
        budget: usize,
    ) -> Result<UnitsPage, ViewPageError> {
        if budget < VIEW_PAGE_MIN {
            return Err(ViewPageError::BudgetTooSmall {
                budget,
                min: VIEW_PAGE_MIN,
            });
        }
        let all = self
            .units_of(container)
            .ok_or_else(|| ViewPageError::UnknownContainer(container.to_owned()))?;
        let total = all.len();
        if from > total {
            return Err(ViewPageError::PastEnd { from, total });
        }
        // the frame with the longest `next` it can have (a number up to `total`, or null), so
        // the page never grows past it
        let frame = [Some(total), None]
            .into_iter()
            .map(|next| {
                UnitsPage {
                    container: container.to_owned(),
                    from,
                    units: Vec::new(),
                    next,
                    total,
                }
                .to_value()
                .to_json()
                .len()
            })
            .max()
            .unwrap_or(0);
        let mut used = frame;
        let mut n = 0;
        for u in &all[from..] {
            let len = unit_value(u).to_json().len() + usize::from(n > 0);
            if n > 0 && used + len > budget {
                break;
            }
            used += len;
            n += 1;
        }
        let next = (from + n < total).then_some(from + n);
        Ok(UnitsPage {
            container: container.to_owned(),
            from,
            units: all[from..from + n].to_vec(),
            next,
            total,
        })
    }

    /// The whole index as JSON, in the contract's field names (GOLD `index-rev40.json`).
    pub fn to_value(&self) -> Json {
        let mut v = self.snapshot().fields();
        v.push((
            "units".into(),
            Json::List(self.units.iter().map(unit_value).collect()),
        ));
        Json::Obj(v)
    }
}

/// Why [`ViewIndex::units_page`] gave no page.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ViewPageError {
    #[error("no container has the address {0:?}")]
    UnknownContainer(String),
    #[error("a page budget of {budget} bytes is under the least, {min}")]
    BudgetTooSmall { budget: usize, min: usize },
    #[error("unit {from} is past the container's {total} units")]
    PastEnd { from: usize, total: usize },
}

/// The watch snapshot's part of a [`ViewIndex`]: everything but the units.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ViewSnapshot {
    pub tier: Tier,
    pub rev: Option<u64>,
    pub version: Option<u64>,
    pub page: Option<Page>,
    pub fonts: Vec<FontUse>,
    pub layouts: Vec<Layout>,
    pub containers: Vec<Container>,
    pub space: Option<Space>,
}

impl ViewSnapshot {
    fn fields(&self) -> Vec<(String, Json)> {
        let page = self.page.as_ref().map_or(Json::Null, |p| {
            Json::obj([
                ("w", Json::from(p.w)),
                ("h", Json::from(p.h)),
                ("unit", Json::from(&p.unit)),
            ])
        });
        let fonts = self.fonts.iter().map(|f| {
            Json::obj([
                ("family", Json::from(&f.family)),
                ("script", Json::from(&f.script)),
                ("from", Json::from(&f.from)),
                ("embedded", Json::from(f.embedded)),
            ])
        });
        let layouts = self.layouts.iter().map(|l| {
            Json::obj([
                ("key", Json::from(&l.key)),
                ("name", Json::from(&l.name)),
                ("master", Json::from(l.master.as_ref())),
            ])
        });
        let containers = self.containers.iter().map(container_value);
        vec![
            ("tier".into(), Json::from(self.tier.as_str())),
            ("rev".into(), Json::from(self.rev)),
            ("version".into(), Json::from(self.version)),
            ("page".into(), page),
            ("fonts".into(), Json::List(fonts.collect())),
            ("layouts".into(), Json::List(layouts.collect())),
            ("containers".into(), Json::List(containers.collect())),
        ]
        .into_iter()
        .chain(
            self.space
                .as_ref()
                .map(|sp| ("space".to_owned(), sp.to_value())),
        )
        .collect()
    }

    /// The snapshot as JSON, with the doc's view schema (contract Item 1: the snapshot is the
    /// schema, `page`, `fonts`, `layouts` and `containers`).
    pub fn to_value(&self, schema: &ViewSchema) -> Json {
        let mut v = vec![("schema".to_owned(), schema.to_value())];
        v.extend(self.fields());
        Json::Obj(v)
    }
}

/// One container as the snapshot carries it, in the contract's field names. `parent` only when
/// it has one ([`Container::parent`]), `placement` only when placed and `derived`
/// (`{name: text}`) only when it has derived values ([`ViewSchema::container_derived`]), so a
/// container of a pack that declares none of them encodes as before (and an older reader, which
/// doesn't know them, reads it as before).
fn container_value(c: &Container) -> Json {
    let mut m = vec![
        ("key".to_owned(), Json::from(&c.key)),
        ("position".to_owned(), Json::from(c.position)),
        ("title".to_owned(), Json::from(c.title.as_ref())),
        ("hidden".to_owned(), Json::from(c.hidden)),
        ("layout".to_owned(), Json::from(c.layout.as_ref())),
        (
            "render_hash".to_owned(),
            Json::from(c.render_hash.as_ref().map(NodeHash::to_hex)),
        ),
        ("fonts".to_owned(), Json::list(c.fonts.iter().copied())),
        ("has_fields".to_owned(), Json::list(c.has_fields.iter())),
        ("unit_count".to_owned(), Json::from(c.unit_count)),
    ];
    if let Some(p) = &c.parent {
        if !planted_parent_dropped_on_wire() {
            m.push(("parent".to_owned(), Json::from(p)));
        }
    }
    if let Some(p) = &c.placement {
        m.push(("placement".to_owned(), p.to_value()));
    }
    if !c.derived.is_empty() && !planted_container_derived_dropped_on_wire() {
        m.push((
            "derived".to_owned(),
            Json::obj(c.derived.iter().map(|(n, t)| (n.as_str(), Json::from(t)))),
        ));
    }
    Json::Obj(m)
}

/// One page of a container's units ([`ViewIndex::units_page`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitsPage {
    /// The container's address.
    pub container: String,
    /// Where the page starts among the container's units.
    pub from: usize,
    pub units: Vec<Unit>,
    /// Where the next page starts; `None` on the last.
    pub next: Option<usize>,
    /// How many units the container has.
    pub total: usize,
}

impl UnitsPage {
    /// The page as JSON: what the budget counts.
    pub fn to_value(&self) -> Json {
        Json::obj([
            ("container", Json::from(&self.container)),
            ("from", Json::from(self.from)),
            (
                "units",
                Json::List(self.units.iter().map(unit_value).collect()),
            ),
            ("next", Json::from(self.next)),
            ("total", Json::from(self.total)),
        ])
    }
}

/// One unit as JSON, in the contract's field names. `derived` (`{name: text}`) only when the
/// unit has derived values ([`ViewSchema::unit_derived`]), so a unit of a pack that declares
/// none encodes as before.
pub fn unit_value(u: &Unit) -> Json {
    let v = Json::obj([
        ("key", Json::from(&u.key)),
        ("container", Json::from(&u.container)),
        ("parent", Json::from(u.parent.as_ref())),
        ("live", Json::from(u.live)),
        ("kind", Json::from(&u.kind)),
        ("name", Json::from(&u.name)),
        ("name_is_generic", Json::from(u.name_is_generic)),
        ("alt", Json::from(u.alt.as_ref())),
        ("z", Json::from(u.z)),
        (
            "bbox",
            u.bbox.map_or(Json::Null, |b| {
                Json::obj([
                    ("x", Json::from(b.x)),
                    ("y", Json::from(b.y)),
                    ("w", Json::from(b.w)),
                    ("h", Json::from(b.h)),
                ])
            }),
        ),
        ("bbox_inherited", Json::from(u.bbox_inherited)),
        ("rot", Json::from(u.rot)),
        (
            "flip",
            Json::obj([("h", Json::from(u.flip.h)), ("v", Json::from(u.flip.v))]),
        ),
        ("approx_box", Json::from(u.approx_box)),
        (
            "text",
            u.text.as_ref().map_or(Json::Null, |t| {
                Json::obj([
                    ("summary", Json::from(&t.summary)),
                    ("truncated", Json::from(t.truncated)),
                ])
            }),
        ),
        ("fields", Json::list(u.fields.iter())),
        ("hidden", Json::from(u.hidden)),
        (
            "placeholder",
            u.placeholder.as_ref().map_or(Json::Null, |p| {
                Json::obj([
                    ("type", Json::from(&p.ty)),
                    ("idx", Json::from(p.idx.as_ref())),
                ])
            }),
        ),
        (
            "editable_text",
            Json::from(u.editable_text.map(EditableText::as_str)),
        ),
    ]);
    let Json::Obj(mut v) = v else {
        unreachable!("an object")
    };
    if let Some(p) = &u.placement {
        v.push(("placement".into(), p.to_value()));
    }
    if !u.derived.is_empty() {
        v.push((
            "derived".into(),
            Json::obj(u.derived.iter().map(|(n, t)| (n.as_str(), Json::from(t)))),
        ));
    }
    Json::Obj(v)
}

/// What the live builder reads once per build: the schema and the two composite maps.
struct LiveCtx<'a> {
    schema: &'a ViewSchema,
    cprops: Option<&'a BTreeMap<String, Value>>,
    uprops: Option<&'a BTreeMap<String, Value>>,
}

impl<'a> LiveCtx<'a> {
    fn new<P: Pack + ?Sized>(pack: &P, schema: &'a ViewSchema, state: &'a KeyedState) -> Self {
        let composites = pack.composite();
        LiveCtx {
            schema,
            cprops: fields_root(composites, schema.containers).and_then(|m| map_of(state, m)),
            uprops: fields_root(composites, schema.units).and_then(|m| map_of(state, m)),
        }
    }

    fn page(&self, state: &KeyedState) -> Option<Page> {
        let p = self.schema.page?;
        let m = map_of(state, p.root)?;
        let n = |k: &str| m.get(k)?.as_str()?.trim().parse::<i64>().ok();
        Some(Page {
            w: n(p.w)?,
            h: n(p.h)?,
            unit: self.schema.native_unit.to_owned(),
        })
    }

    fn celem(&self, k: &'a Key, v: &'a Value) -> Elem<'a> {
        Elem {
            key: k,
            value: v,
            props: self.cprops,
        }
    }

    fn uelem(&self, k: &'a Key, v: &'a Value) -> Elem<'a> {
        Elem {
            key: k,
            value: v,
            props: self.uprops,
        }
    }

    /// The value a unit's container field must match: the container's id field, or its key.
    fn container_id(&self, k: &'a Key, v: &'a Value) -> Option<String> {
        match self.schema.container_id_field {
            Some(f) => self.celem(k, v).field(f).map(str::to_owned),
            None => Some(k.as_str().to_owned()),
        }
    }

    /// Container `k` at `i` (from 0), its kit fields only.
    fn container(&self, i: usize, k: &'a Key, v: &'a Value, address: String) -> Container {
        let cf = self.schema.container_fields;
        let e = self.celem(k, v);
        Container {
            key: address,
            position: u32::try_from(i + 1).unwrap_or(u32::MAX),
            title: e.read(cf.title).map(cut),
            hidden: flag(e.read(cf.hidden)),
            layout: e.read(cf.layout).and_then(nonempty),
            ..Container::default()
        }
    }

    /// Container `k`'s parent ([`ContainerFields::parent`]) onto `c`: the value read is the
    /// parent's key, mapped to its address; one that isn't a key is recorded in `refused`
    /// ([`ParentRefused::NotAKey`]) and not carried. The kit checks the rest when it finishes
    /// the index ([`ViewIndex::check_parents`]).
    fn container_parent<P: Pack + ?Sized>(
        &self,
        pack: &P,
        k: &'a Key,
        v: &'a Value,
        c: &mut Container,
        refused: &mut Vec<ViewRefusal>,
    ) {
        let Some(raw) = self
            .celem(k, v)
            .read(self.schema.container_fields.parent)
            .and_then(nonempty)
        else {
            return;
        };
        match Key::new(raw.as_str()) {
            Ok(key) => c.parent = Some(pack.address_of(self.schema.containers, &key)),
            Err(_) if planted_parent_not_validated() => c.parent = Some(raw),
            Err(_) => refused.push(ViewRefusal::Parent {
                container: c.key.clone(),
                value: raw,
                why: ParentRefused::NotAKey,
            }),
        }
    }

    /// The container field of unit `k`.
    fn unit_container(&self, k: &'a Key, v: &'a Value) -> Option<&'a str> {
        self.uelem(k, v).field(self.schema.unit_container_field)
    }

    /// Unit `k`, in container `container` at `z`, its kit fields only: one unit built
    /// (`crate::differential::take_view_unit_builds`).
    fn unit(&self, k: &'a Key, v: &'a Value, address: String, container: String, z: u32) -> Unit {
        #[cfg(any(test, feature = "differential"))]
        crate::differential::count_view_unit_build();
        let uf = self.schema.unit_fields;
        let e = self.uelem(k, v);
        let geom = self
            .schema
            .geometry
            .and_then(|g| e.field(g.field).and_then(|s| parse_geom(s, g.parts)));
        let hidden = if planted_schema_field_ignored() {
            false
        } else {
            flag(e.read(uf.hidden))
        };
        Unit {
            key: address,
            container,
            parent: None,
            live: true,
            kind: e.read(uf.kind).unwrap_or_default().to_owned(),
            name: cut(e.read(uf.name).unwrap_or_default()),
            alt: e.read(uf.alt).and_then(nonempty).map(|a| cut(&a)),
            z,
            bbox: geom.map(|(b, _, _)| b),
            rot: geom.map_or(0, |(_, r, _)| r),
            flip: geom.map_or(Flip::default(), |(_, _, f)| f),
            text: e.read(uf.text).map(|t| summary(t, false)),
            hidden,
            placeholder: e.read(uf.placeholder).map(placeholder),
            editable_text: e.read(uf.editable_text).and_then(EditableText::parse),
            ..Unit::default()
        }
    }
}

fn build_live<P: Pack + ?Sized>(
    pack: &P,
    schema: &ViewSchema,
    state: &KeyedState,
    rev: u64,
    carrier: Option<&ViewCarrier>,
) -> ViewIndex {
    let cx = LiveCtx::new(pack, schema, state);
    let source = ViewSource::Live(state, carrier);
    let mut ix = ViewIndex {
        tier: Tier::Live,
        rev: Some(rev),
        version: None,
        page: cx.page(state),
        space: schema.space.and_then(|sp| space_of(pack, &sp, source)),
        ..ViewIndex::default()
    };
    let mut by_id: HashMap<String, usize> = HashMap::new();
    for (i, (k, v)) in live_of(state, schema.containers).iter().enumerate() {
        if let Some(id) = cx.container_id(k, v) {
            by_id.entry(id).or_insert(i);
        }
        let address = pack.address_of(schema.containers, k);
        let mut c = cx.container(i, k, v, address);
        cx.container_parent(pack, k, v, &mut c, &mut ix.refusals);
        c.placement = placed_container(pack, schema, source, k, &c.key);
        ix.containers.push(c);
    }
    let mut per: Vec<Vec<Unit>> = vec![Vec::new(); ix.containers.len()];
    for (k, v) in live_of(state, schema.units) {
        let Some(ci) = cx
            .unit_container(k, v)
            .and_then(|id| by_id.get(id).copied())
        else {
            continue;
        };
        let z = u32::try_from(per[ci].len()).unwrap_or(u32::MAX);
        let address = pack.address_of(schema.units, k);
        let mut u = cx.unit(k, v, address, ix.containers[ci].key.clone(), z);
        u.placement = placed_unit(pack, schema, source, k, &u.key);
        per[ci].push(u);
    }
    ix.units = per.into_iter().flatten().collect();
    ix
}

// ----- the 3D slot -----

/// The planted fault [`crate::differential::PlaceMutant::AskEveryUnit`].
fn planted_ask_every_unit() -> bool {
    plant_on!(
        crate::differential::place_mutant() == Some(crate::differential::PlaceMutant::AskEveryUnit)
    )
}

/// The planted fault [`crate::differential::PlaceMutant::NoneIsContainerFrame`].
fn planted_none_is_container_frame() -> bool {
    plant_on!(
        crate::differential::place_mutant()
            == Some(crate::differential::PlaceMutant::NoneIsContainerFrame)
    )
}

/// The planted fault [`crate::differential::PlaceMutant::CycleUnchecked`].
fn planted_cycle_unchecked() -> bool {
    plant_on!(
        crate::differential::place_mutant()
            == Some(crate::differential::PlaceMutant::CycleUnchecked)
    )
}

/// The doc's space under `slot`: fixed, or the pack's.
fn space_of<P: Pack + ?Sized>(pack: &P, slot: &SpaceSlot, source: ViewSource<'_>) -> Option<Space> {
    match slot.space {
        SpaceSrc::Fixed { .. } => slot.fixed(),
        SpaceSrc::Pack { .. } => pack.view_space(source),
    }
}

/// Whether the changes since an index wrote a root the doc's space is read from.
fn space_touched(slot: &SpaceSlot, dirty: &ViewDirty) -> bool {
    match slot.space {
        SpaceSrc::Pack { roots } => roots.iter().any(|r| dirty.roots.contains(*r)),
        SpaceSrc::Fixed { .. } => false,
    }
}

/// Whether the composite map key `map_key` of element `e` is one of `fields`.
fn placing(fields: Option<&'static [&'static str]>, e: &Key, map_key: &str) -> bool {
    let Some(fields) = fields else { return false };
    let field = map_key
        .get(e.as_str().len() + CELL_SEP.len_utf8()..)
        .unwrap_or("");
    fields.contains(&field)
}

/// Element `key` of `root` placed by the pack ([`crate::session::Pack::view_placement`]), at
/// the slot's precision. A placement the slot can't carry ([`Placement::problem`]) is dropped,
/// and fails a test build loudly, naming the element.
fn placed<P: Pack + ?Sized>(
    pack: &P,
    slot: &SpaceSlot,
    source: ViewSource<'_>,
    root: &str,
    key: &Key,
    address: &str,
) -> Option<Placement> {
    #[cfg(any(test, feature = "differential"))]
    crate::differential::count_placement_ask();
    let mut p = pack.view_placement(source, root, key)?;
    if let Some(why) = p.problem(slot.precision) {
        if cfg!(any(test, feature = "differential")) {
            panic!("{address}: the pack's placement can't be carried: {why}");
        }
        return None;
    }
    p.precision = slot.precision;
    Some(p)
}

fn placed_unit<P: Pack + ?Sized>(
    pack: &P,
    schema: &ViewSchema,
    source: ViewSource<'_>,
    key: &Key,
    address: &str,
) -> Option<Placement> {
    let slot = schema.space?;
    placed(pack, &slot, source, schema.units, key, address)
}

fn placed_container<P: Pack + ?Sized>(
    pack: &P,
    schema: &ViewSchema,
    source: ViewSource<'_>,
    key: &Key,
    address: &str,
) -> Option<Placement> {
    let slot = schema.space.filter(|s| !s.container_fields.is_empty())?;
    placed(pack, &slot, source, schema.containers, key, address)
}

impl ViewIndex {
    /// The world matrix of the unit or container whose address is `address`, in the doc's
    /// space ([`crate::placement::world_matrix`]): `None` when it has no placement or its chain
    /// is broken (an element with none, one not in the index, a degenerate step, a cycle: one
    /// fails a test build when the index is built).
    pub fn world_matrix(&self, address: &str) -> Option<Mat4> {
        fn link<'a>(ix: &'a ViewIndex, a: &str) -> Link<'a> {
            if let Some(u) = ix.unit(a) {
                return match &u.placement {
                    Some(p) => Link::Placed(p),
                    None => Link::Unplaced,
                };
            }
            match ix.container(a) {
                Some(c) => match &c.placement {
                    Some(p) => Link::Placed(p),
                    None => Link::Unplaced,
                },
                None => Link::NotShown,
            }
        }
        let lookup = |a: &str| link(self, a);
        if planted_none_is_container_frame() {
            // a placement relative to nothing read as relative to its unit's container
            if let Some(u) = self.unit(address) {
                if let (Some(p), Some(c)) = (&u.placement, self.container(&u.container)) {
                    if p.relative_to.is_none() {
                        let own = p.matrix()?;
                        let parent = c.placement.as_ref().and_then(Placement::matrix)?;
                        return Some(crate::placement::mul(&parent, &own));
                    }
                }
            }
        }
        crate::placement::world_matrix(address, lookup)
    }
}

// ----- keeping a live index through changes -----

/// What the changes since a live index was built touched, as the kit keeps it while it holds
/// the index (the session, change by change, from each change's applied stream): enough to
/// update that index ([`ViewIndex::live_update`]) instead of building it again.
#[derive(Debug, Clone, Default)]
pub(crate) struct ViewDirty {
    /// The doc's version the record is current to.
    pub(crate) upto: u64,
    /// Units and containers by key: an op on the element or on one of its fields.
    pub(crate) units: HashSet<Key>,
    pub(crate) containers: HashSet<Key>,
    /// A list op other than a value's set: elements came, went or moved.
    pub(crate) unit_order: bool,
    pub(crate) container_order: bool,
    /// The page's root was written.
    pub(crate) page: bool,
    /// Every other root written (and a composite map's key naming no element).
    pub(crate) roots: BTreeSet<String>,
    /// Units and containers whose placement fields (the 3D slot's) a change wrote, or that
    /// came, went or moved: their placements are asked for again.
    pub(crate) placed: HashSet<Key>,
    pub(crate) placed_c: HashSet<Key>,
    /// More than [`VIEW_DIRTY_MAX`] elements, or a change not taken in order: build again.
    pub(crate) wide: bool,
}

/// The most elements a [`ViewDirty`] names before an update is no cheaper than a build.
pub(crate) const VIEW_DIRTY_MAX: usize = 4096;

impl ViewDirty {
    /// Nothing changed since `upto`.
    pub(crate) fn at(upto: u64) -> ViewDirty {
        ViewDirty {
            upto,
            ..ViewDirty::default()
        }
    }

    /// One change that took the doc from `before` to `after`, from its applied stream. A
    /// change that doesn't follow the last one taken makes it wide.
    pub(crate) fn take<P: Pack + ?Sized>(
        &mut self,
        pack: &P,
        schema: &ViewSchema,
        before: u64,
        after: u64,
        stream: &[crate::elem::Applied],
    ) {
        use crate::elem::{Applied, Effect, ListOpKind};
        if self.wide || self.upto != before {
            self.wide = true;
            self.upto = after;
            return;
        }
        self.upto = after;
        let composites = pack.composite();
        let umap = fields_root(composites, schema.units);
        let cmap = fields_root(composites, schema.containers);
        let page = schema.page.map(|p| p.root);
        let sp = schema.space;
        for x in stream {
            match x {
                Applied::List { root, ops }
                    if root == schema.units || root == schema.containers =>
                {
                    let units = root == schema.units;
                    for o in ops {
                        if matches!(o.effect, Effect::None { .. }) {
                            continue;
                        }
                        let order = o.kind != ListOpKind::Set;
                        if units {
                            self.units.insert(o.key.clone());
                            self.placed.insert(o.key.clone());
                            self.unit_order |= order;
                        } else {
                            self.containers.insert(o.key.clone());
                            self.placed_c.insert(o.key.clone());
                            self.container_order |= order;
                        }
                    }
                }
                Applied::Map { root, ops }
                    if Some(root.as_str()) == umap || Some(root.as_str()) == cmap =>
                {
                    for o in ops {
                        if matches!(o.effect, Effect::None { .. }) {
                            continue;
                        }
                        match crate::session::elem_of_map_key(composites, root, &o.key) {
                            Some((l, e)) if l == schema.units => {
                                if placing(sp.map(|s| s.unit_fields), &e, &o.key) {
                                    self.placed.insert(e.clone());
                                }
                                self.units.insert(e);
                            }
                            Some((_, e)) => {
                                if placing(sp.map(|s| s.container_fields), &e, &o.key) {
                                    self.placed_c.insert(e.clone());
                                }
                                self.containers.insert(e);
                            }
                            None => {
                                self.roots.insert(root.clone());
                            }
                        }
                    }
                }
                _ => {
                    if Some(x.root()) == page {
                        self.page = true;
                    }
                    self.roots.insert(x.root().to_owned());
                }
            }
        }
        if self.units.len() + self.containers.len() > VIEW_DIRTY_MAX {
            self.wide = true;
        }
    }
}

/// What the changes since a pack's index was last derived touched, for its incremental derive
/// ([`crate::session::Pack::view_derive_change`]), by address.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ViewTouched {
    /// The units the kit built again (an op on the unit or one of its fields, a new unit, or
    /// its container changed), in the index's order: their kit fields are new and the fields
    /// the pack derives are empty (no derived values, no nested units under them). Every other
    /// unit is as the pack left it, nested units included.
    pub units: Vec<String>,
    /// The containers the kit built again, the same way: their derived fields are empty.
    pub containers: Vec<String>,
    /// Units or containers came, went or moved (their order and `z` are the kit's already).
    pub order: bool,
    /// The page's root was written (the kit read the page again).
    pub page: bool,
    /// Every other root written: neither the units' nor the containers' list or composite map
    /// (the page's root among them when it changed). A pack whose derived values read one
    /// derives again what depends on it.
    pub roots: Vec<String>,
}

/// The planted fault [`crate::differential::ViewMutant::UpdateSkipsTouched`].
fn planted_update_skips_touched() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::UpdateSkipsTouched)
    )
}

/// The planted fault [`crate::differential::ViewMutant::UpdateRebuildsAll`].
fn planted_update_rebuilds_all() -> bool {
    plant_on!(
        crate::differential::view_mutant()
            == Some(crate::differential::ViewMutant::UpdateRebuildsAll)
    )
}

impl ViewIndex {
    /// The index of a live doc at `rev` from `prev`, its index at the version `dirty` started
    /// from, and `dirty`, what the changes since touched: the kit builds again only the
    /// touched units and containers (and new ones), keeps every other one as `prev` has it
    /// (the pack's derived fields and nested units included), puts them in the doc's order,
    /// and asks the pack to derive what the changes touched
    /// ([`crate::session::Pack::view_derive_change`]). `None` when the pack has no view schema
    /// or doesn't derive by change: the caller builds the index whole
    /// ([`ViewIndex::live_carried`]).
    pub(crate) fn live_update<P: Pack + ?Sized>(
        pack: &P,
        prev: &ViewIndex,
        state: &KeyedState,
        rev: u64,
        carrier: Option<&ViewCarrier>,
        dirty: &ViewDirty,
    ) -> Option<ViewIndex> {
        let schema = pack.view_schema()?;
        let cx = LiveCtx::new(pack, &schema, state);
        let source = ViewSource::Live(state, carrier);
        let mut touched = ViewTouched {
            order: dirty.unit_order || dirty.container_order,
            page: dirty.page,
            roots: dirty.roots.iter().cloned().collect(),
            ..ViewTouched::default()
        };
        let mut ix = ViewIndex {
            tier: Tier::Live,
            rev: Some(rev),
            version: None,
            page: match dirty.page {
                true => cx.page(state),
                false => prev.page.clone(),
            },
            fonts: prev.fonts.clone(),
            space: match schema.space {
                Some(sp) if space_touched(&sp, dirty) => space_of(pack, &sp, source),
                _ => prev.space.clone(),
            },
            layouts: prev.layouts.clone(),
            containers: Vec::new(),
            units: Vec::new(),
            refusals: Vec::new(),
        };
        // the containers: the touched and new ones built again, the rest as they were
        let was: HashMap<&str, &Container> = prev
            .containers
            .iter()
            .map(|c| (c.key.as_str(), c))
            .collect();
        let regroup = dirty.container_order || !dirty.containers.is_empty();
        let mut fresh_refusals: Vec<ViewRefusal> = Vec::new();
        let mut by_id: HashMap<String, usize> = HashMap::new();
        for (i, (k, v)) in live_of(state, schema.containers).iter().enumerate() {
            if let Some(id) = cx.container_id(k, v) {
                by_id.entry(id).or_insert(i);
            }
            let address = pack.address_of(schema.containers, k);
            let c = match was.get(address.as_str()) {
                Some(c) if !dirty.containers.contains(k) => Container {
                    position: u32::try_from(i + 1).unwrap_or(u32::MAX),
                    ..(*c).clone()
                },
                _ => {
                    touched.containers.push(address.clone());
                    let mut n = cx.container(i, k, v, address);
                    cx.container_parent(pack, k, v, &mut n, &mut fresh_refusals);
                    n.placement = match was.get(n.key.as_str()) {
                        Some(c) if !dirty.placed_c.contains(k) && !planted_ask_every_unit() => {
                            c.placement.clone()
                        }
                        _ => placed_container(pack, &schema, source, k, &n.key),
                    };
                    n
                }
            };
            ix.containers.push(c);
        }
        // the refused layouts and parents of the containers kept as they were, checked again
        // by `finish` (a container built again has its value back, or its new refusal)
        ix.refusals = prev
            .refusals
            .iter()
            .filter(|r| match r {
                ViewRefusal::Layout { container, .. } | ViewRefusal::Parent { container, .. } => {
                    !touched.containers.contains(container)
                }
            })
            .cloned()
            .chain(fresh_refusals)
            .collect();
        let at: HashMap<String, usize> = ix
            .containers
            .iter()
            .enumerate()
            .map(|(i, c)| (c.key.clone(), i))
            .collect();
        // `prev`'s runs: each top-level unit and the nested units right after it
        let mut runs: HashMap<&str, (usize, usize)> = HashMap::new();
        let mut i = 0;
        while i < prev.units.len() {
            let mut j = i + 1;
            while j < prev.units.len() && prev.units[j].parent.is_some() {
                j += 1;
            }
            if prev.units[i].parent.is_none() {
                runs.insert(prev.units[i].key.as_str(), (i, j));
            }
            i = j;
        }
        // the units, in the doc's order: the touched and new ones built again
        let mut per: Vec<Vec<Unit>> = vec![Vec::new(); ix.containers.len()];
        let mut tops: Vec<u32> = vec![0; ix.containers.len()];
        for (k, v) in live_of(state, schema.units) {
            let address = pack.address_of(schema.units, k);
            let run = runs.get(address.as_str()).copied();
            let fresh = planted_update_rebuilds_all()
                || run.is_none()
                || (dirty.units.contains(k) && !planted_update_skips_touched());
            let ci = match run {
                Some((s, _)) if !fresh && !regroup => at.get(prev.units[s].container.as_str()),
                _ => cx.unit_container(k, v).and_then(|id| by_id.get(id)),
            };
            let Some(&ci) = ci else { continue };
            let container = ix.containers[ci].key.clone();
            let z = tops[ci];
            tops[ci] += 1;
            match run {
                Some((s, e)) if !fresh && prev.units[s].container == container => {
                    per[ci].push(Unit {
                        z,
                        ..prev.units[s].clone()
                    });
                    per[ci].extend(prev.units[s + 1..e].iter().cloned());
                }
                _ => {
                    touched.units.push(address.clone());
                    let mut u = cx.unit(k, v, address, container, z);
                    u.placement = match run {
                        Some((s, _)) if !dirty.placed.contains(k) && !planted_ask_every_unit() => {
                            prev.units[s].placement.clone()
                        }
                        _ => placed_unit(pack, &schema, source, k, &u.key),
                    };
                    per[ci].push(u);
                }
            }
        }
        ix.units = per.into_iter().flatten().collect();
        if !pack.view_derive_change(ViewSource::Live(state, carrier), &mut ix, &touched) {
            return None;
        }
        ix.finish(&schema);
        Some(ix)
    }
}

/// How [`build_blob`] makes one unit: of node `k`, under `parent`, in `container`, at `z`.
type MakeUnit<'a> = dyn Fn(usize, Option<&str>, &str, u32) -> Unit + 'a;

/// The units under node `i`, back to front, each followed by its own ([`build_blob`]).
fn walk(
    i: usize,
    parent: Option<&str>,
    container: &str,
    kids: &[Vec<usize>],
    is_unit: &dyn Fn(usize) -> bool,
    unit: &MakeUnit<'_>,
    out: &mut Vec<Unit>,
) {
    let units: Vec<usize> = kids[i].iter().copied().filter(|k| is_unit(*k)).collect();
    // sibling order is top to bottom: the back is the last
    for (z, k) in units.iter().rev().enumerate() {
        let u = unit(*k, parent, container, u32::try_from(z).unwrap_or(u32::MAX));
        let me = u.key.clone();
        out.push(u);
        walk(*k, Some(&me), container, kids, is_unit, unit, out);
    }
}

fn build_blob(schema: &ViewSchema, b: &BlobView, view: &IndexView) -> ViewIndex {
    let page = b.page.and_then(|(w, h)| {
        let n = |k: &str| view.attrs.get(k)?.trim().parse::<i64>().ok();
        Some(Page {
            w: n(w)?,
            h: n(h)?,
            unit: schema.native_unit.to_owned(),
        })
    });
    let mut ix = ViewIndex {
        tier: Tier::Blob,
        rev: None,
        version: Some(view.version),
        page,
        fonts: view.fonts.clone(),
        ..ViewIndex::default()
    };
    // the outline: each node's children by their sibling order (top to bottom)
    let at: HashMap<&str, usize> = view
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| (n.key.as_str(), i))
        .collect();
    let mut kids: Vec<Vec<usize>> = vec![Vec::new(); view.nodes.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (i, n) in view.nodes.iter().enumerate() {
        match n.parent.as_ref().and_then(|p| at.get(p.as_str())) {
            Some(p) => kids[*p].push(i),
            None => roots.push(i),
        }
    }
    let order = |v: &mut Vec<usize>| v.sort_by_key(|i| view.nodes[*i].order);
    order(&mut roots);
    for k in kids.iter_mut() {
        order(k);
    }
    let mut outline = Vec::with_capacity(view.nodes.len());
    let mut stack: Vec<usize> = roots.into_iter().rev().collect();
    while let Some(i) = stack.pop() {
        outline.push(i);
        stack.extend(kids[i].iter().rev());
    }
    if let Some(lk) = b.layout_kind {
        for i in &outline {
            let n = &view.nodes[*i];
            if n.kind == lk {
                ix.layouts.push(Layout {
                    key: n.key.as_str().to_owned(),
                    name: n.name.clone(),
                    master: n.parent.as_ref().map(|p| p.as_str().to_owned()),
                });
            }
        }
    }
    let is_unit = |i: usize| {
        let k = view.nodes[i].kind.as_str();
        k != b.container_kind && !b.skip_kinds.contains(&k)
    };
    for i in outline {
        let n = &view.nodes[i];
        if n.kind != b.container_kind {
            continue;
        }
        let key = n.key.as_str().to_owned();
        ix.containers.push(Container {
            key: key.clone(),
            position: u32::try_from(ix.containers.len() + 1).unwrap_or(u32::MAX),
            title: node_src(b.container_title, n).map(cut),
            hidden: !n.visible,
            layout: node_src(b.container_layout, n).and_then(nonempty),
            parent: node_src(b.container_parent, n).and_then(nonempty),
            render_hash: n.render_hash,
            fonts: n.view.as_ref().map(|v| v.fonts.clone()).unwrap_or_default(),
            has_fields: n
                .view
                .as_ref()
                .map(|v| v.fields.clone())
                .unwrap_or_default(),
            ..Container::default()
        });
        let unit = |k: usize, parent: Option<&str>, container: &str, z: u32| -> Unit {
            let n = &view.nodes[k];
            let attr_flag = |s: NodeSrc| flag(node_src(s, n));
            let mut u = Unit {
                key: n.key.as_str().to_owned(),
                container: container.to_owned(),
                parent: parent.map(str::to_owned),
                live: parent.is_none(),
                kind: node_src(b.unit_kind, n).unwrap_or_default().to_owned(),
                name: cut(node_src(b.unit_name, n).unwrap_or_default()),
                alt: node_src(b.unit_alt, n).and_then(nonempty).map(|a| cut(&a)),
                z,
                bbox: n.bounds.map(|r| Bbox {
                    x: r.left,
                    y: r.top,
                    w: r.right.saturating_sub(r.left),
                    h: r.bottom.saturating_sub(r.top),
                }),
                rot: node_src(b.unit_rot, n)
                    .and_then(|r| r.trim().parse().ok())
                    .unwrap_or(0),
                flip: Flip {
                    h: attr_flag(b.unit_flip_h),
                    v: attr_flag(b.unit_flip_v),
                },
                text: node_src(b.unit_text, n)
                    .map(|t| summary(t, n.flags.contains("text_truncated"))),
                hidden: !n.visible,
                placeholder: node_src(b.placeholder_type, n).map(|ty| Placeholder {
                    ty: ty.to_owned(),
                    idx: node_src(b.placeholder_idx, n).map(str::to_owned),
                }),
                editable_text: node_src(b.unit_editable_text, n)
                    .and_then(|raw| word(schema.unit_fields.editable_text, raw))
                    .and_then(EditableText::parse),
                ..Unit::default()
            };
            // the view items (row format 2) over the plain fields
            if let Some(v) = &n.view {
                let g = v.geom;
                u.bbox = g.map(|g| g.bbox);
                u.rot = g.map_or(0, |g| g.rot);
                u.flip = g.map_or(Flip::default(), |g| g.flip);
                u.bbox_inherited = g.is_some_and(|g| g.inherited);
                u.approx_box = g.is_some_and(|g| g.approx);
                u.fields = v.fields.clone();
                if let Some(t) = &v.text {
                    u.text = Some(summary(t, v.text_truncated));
                }
                if v.editable_text.is_some() {
                    u.editable_text = v.editable_text;
                }
            }
            u
        };
        walk(i, None, &key, &kids, &is_unit, &unit, &mut ix.units);
    }
    ix
}

// ----- addresses -----

/// Why an address names no element ([`key_candidates`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AddressError {
    /// No list root takes it.
    #[error("no element has the address {0:?}")]
    Unknown(String),
    /// The pack's mapping isn't one to one: the key it gives for `address` has another address
    /// (a pack bug, refused loudly rather than acting on the wrong element).
    #[error(
        "the address {address:?} reads as {root} {key}, whose address is {back:?}: the pack's \
         addresses are not one to one"
    )]
    NotInverse {
        address: String,
        root: String,
        key: String,
        back: String,
    },
}

/// The elements `address` can name: each MovableList root of `pack`'s schema whose
/// [`crate::session::Pack::key_of`] takes it, with the key, in schema order. It reads no doc (a
/// session keeps those that exist: [`crate::session::Session::resolve_address`]). Every key is
/// checked back ([`crate::session::Pack::address_of`] gives `address` again), so the kit never
/// acts on an element the address doesn't name.
pub fn key_candidates<P: Pack + ?Sized>(
    pack: &P,
    address: &str,
) -> Result<Vec<(&'static str, Key)>, AddressError> {
    let mut out = Vec::new();
    for (root, kind) in pack.schema().roots {
        if *kind != Kind::MovableList {
            continue;
        }
        let key = if planted_address_not_inverse() {
            Key::new(address).ok()
        } else {
            pack.key_of(root, address)
        };
        let Some(key) = key else { continue };
        let back = pack.address_of(root, &key);
        if back != address {
            return Err(AddressError::NotInverse {
                address: address.to_owned(),
                root: (*root).to_owned(),
                key: key.as_str().to_owned(),
                back,
            });
        }
        out.push((*root, key));
    }
    if out.is_empty() {
        return Err(AddressError::Unknown(address.to_owned()));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARTS: &[GeomPart] = &[
        GeomPart::X,
        GeomPart::Y,
        GeomPart::W,
        GeomPart::H,
        GeomPart::Rot,
        GeomPart::FlipH,
        GeomPart::FlipV,
    ];

    #[test]
    fn a_geometry_value_parses_whole_or_not_at_all() {
        assert_eq!(
            parse_geom("1,2,3,4,5400000,1,0", PARTS),
            Some((
                Bbox {
                    x: 1,
                    y: 2,
                    w: 3,
                    h: 4
                },
                5_400_000,
                Flip { h: true, v: false }
            ))
        );
        assert_eq!(parse_geom("", PARTS), None, "empty: the layout's box");
        assert_eq!(parse_geom("1,2,3,4,0,0", PARTS), None, "a part short");
        assert_eq!(parse_geom("1,2,3,4,0,0,2", PARTS), None, "a flip isn't 0/1");
        assert_eq!(parse_geom("1,2,x,4,0,0,0", PARTS), None);
    }

    #[test]
    fn a_placeholder_is_type_then_index() {
        assert_eq!(
            placeholder("ctrTitle:"),
            Placeholder {
                ty: "ctrTitle".into(),
                idx: None
            }
        );
        assert_eq!(
            placeholder("body:1"),
            Placeholder {
                ty: "body".into(),
                idx: Some("1".into())
            }
        );
        assert_eq!(
            placeholder(":12"),
            Placeholder {
                ty: String::new(),
                idx: Some("12".into())
            }
        );
    }

    /// A version's index with a container holding a group of two (one of them nested deeper),
    /// a skipped kind, and nodes outside any container: units back to front, each nested one
    /// right after its parent, `z` within its parent, `live` only right under the container.
    #[test]
    fn the_blob_tier_nests_units_under_their_parents() {
        use crate::index::{Fingerprint, KeyClass, NodeKey};
        struct P;
        impl BlobPack for P {
            fn view_schema(&self) -> Option<ViewSchema> {
                Some(ViewSchema {
                    blob: Some(BlobView {
                        container_kind: "page",
                        skip_kinds: &["note"],
                        ..BlobView::NONE
                    }),
                    ..ViewSchema::NONE
                })
            }
        }
        let hash = NodeHash::from_raw([0; 32]);
        // (key, kind, parent, order among siblings: top to bottom)
        let rows: &[(&str, &str, Option<&str>, u32)] = &[
            ("@p:1", "page", None, 0),
            ("@p:1/i:9", "box", Some("@p:1"), 0),
            ("@p:1/i:5", "group", Some("@p:1"), 1),
            ("@p:1/i:7", "box", Some("@p:1/i:5"), 0),
            ("@p:1/i:6", "group", Some("@p:1/i:5"), 1),
            ("@p:1/i:8", "box", Some("@p:1/i:6"), 0),
            ("@p:1/n:1", "note", Some("@p:1"), 2),
            ("@p:1/n:1/i:4", "box", Some("@p:1/n:1"), 0),
            ("@q:1", "other", None, 1),
            ("@q:1/i:3", "box", Some("@q:1"), 0),
        ];
        let nodes = rows
            .iter()
            .map(|(k, kind, parent, order)| IndexNode {
                key: NodeKey((*k).to_owned()),
                key_class: KeyClass::Native,
                native: None,
                kind: (*kind).to_owned(),
                name: String::new(),
                parent: parent.map(|p| NodeKey(p.to_owned())),
                order: *order,
                depth: 0,
                bounds: None,
                visible: true,
                visible_in_tree: true,
                text: None,
                attrs: BTreeMap::new(),
                flags: Default::default(),
                record_hash: hash,
                content_hash: None,
                fingerprint: Fingerprint {
                    kind: (*kind).to_owned(),
                    name: String::new(),
                    bounds: None,
                    content: None,
                    weak: BTreeMap::new(),
                },
                view: None,
                render_hash: None,
            })
            .collect();
        let view = IndexView {
            version: 7,
            source: "t".into(),
            attrs: BTreeMap::new(),
            nodes,
            fonts: Vec::new(),
        };
        let ix = ViewIndex::blob(&P, &view).expect("a schema with a blob mapping");
        let got: Vec<(&str, Option<&str>, bool, u32)> = ix
            .units
            .iter()
            .map(|u| (u.key.as_str(), u.parent.as_deref(), u.live, u.z))
            .collect();
        assert_eq!(
            got,
            vec![
                ("@p:1/i:5", None, true, 0),
                ("@p:1/i:6", Some("@p:1/i:5"), false, 0),
                ("@p:1/i:8", Some("@p:1/i:6"), false, 0),
                ("@p:1/i:7", Some("@p:1/i:5"), false, 1),
                ("@p:1/i:9", None, true, 1),
            ]
        );
        assert_eq!(ix.containers.len(), 1);
        assert_eq!(ix.containers[0].unit_count, 5);
        assert_eq!((ix.tier, ix.version, ix.rev), (Tier::Blob, Some(7), None));
    }

    #[test]
    fn text_is_cut_on_a_character_boundary_and_says_so() {
        let long = "é".repeat(VIEW_TEXT_MAX);
        let t = summary(&long, false);
        assert!(t.truncated);
        assert!(t.summary.len() <= VIEW_TEXT_MAX && t.summary.chars().all(|c| c == 'é'));
        assert!(!summary("short", false).truncated);
        assert!(summary("short", true).truncated, "the index cut it");
    }
}
