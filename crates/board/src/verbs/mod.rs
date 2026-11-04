//! The friendly verbs, for agents and anything else that isn't drawing a canvas. They work on a
//! board, with no live session and no host: [`plan`] reads the board and gives back a [`Group`],
//! and [`crate::apply_group`] applies it. So a save through [`crate::BoardFile`] and a live session
//! later run the same op code.
//!
//! **A verb that overwrites a value names the revision it read.** A verb that only adds, or only
//! changes the caller's own entries, doesn't need one. See `docs/VERBS.md`.
//!
//! No clock, no random source and no host ids here: [`Ctx`] brings them.

mod arrange;
mod clone;
mod edit;
mod inverse;
mod raw;
mod read;
mod templates;

pub use inverse::inverse;
pub use raw::{
    field_from_text, flags, group_from_json, labelled_from_json, settings_flags, FieldEdits,
};
pub use read::{
    fields, list, meta, read, reference_value, search, templates, MetaView, ObjectView,
};
pub use templates::TEMPLATES;

use crate::hash::{hex, Sha256};
use crate::json::{Fields, Value};
use crate::model::{mint_person_token, Board, Id, Object, ObjectType, Person, Reference};
use crate::ops::{Conflict, Group, Op, Position};
use crate::refusal::{Refusal, RefusalCode};
use crate::schema;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

/// Who's acting, as this file knows them.
#[derive(Clone, Debug, PartialEq)]
pub struct Me {
    /// Their token in this file.
    pub token: Id,
    /// Set when the file doesn't have them yet: the group adds them first.
    pub new: Option<Person>,
}

/// Finds a person by name and kind, or mints a token from the host's random bytes. With no name
/// there's nothing to match, so it always mints: a file with its names stripped doesn't get one
/// back. Two people with one name and kind share a token; a token is display data, not access.
pub fn me(board: &Board, name: Option<&str>, kind: Option<&str>, random: [u8; 8]) -> Me {
    if let Some(name) = name {
        let found = board
            .people
            .iter()
            .find(|(_, p)| p.name.as_deref() == Some(name) && p.kind.as_deref() == kind);
        if let Some((token, _)) = found {
            return Me {
                token: token.clone(),
                new: None,
            };
        }
    }
    Me {
        token: mint_person_token(random),
        new: Some(Person {
            name: name.map(str::to_string),
            kind: kind.map(str::to_string),
            extra: Fields::new(),
        }),
    }
}

/// What a verb needs from its host.
pub struct Ctx<'a> {
    /// The id the group gets.
    pub group_id: String,
    /// Who's acting. Reactions, votes and replies need it.
    pub by: Option<Me>,
    /// The time, RFC 3339. Replies need it; with `by`, added objects get `created` from it.
    pub at: Option<String>,
    /// Random bytes for new ids.
    pub random: &'a mut dyn FnMut() -> [u8; 8],
}

/// An object, and the revision the caller read.
#[derive(Clone, Debug, PartialEq)]
pub struct ObjRev {
    /// The object.
    pub id: Id,
    /// Its revision.
    pub rev: Sha256,
}

/// One end of a new connector.
#[derive(Clone, Debug, PartialEq)]
pub enum End {
    /// Attached to an object, which has to be there.
    Object {
        /// The object.
        id: Id,
        /// A side, or `None` for `auto`.
        anchor: Option<String>,
    },
    /// A free point.
    Point {
        /// Across.
        x: f64,
        /// Down.
        y: f64,
    },
}

/// Where [`Verb::Order`] puts its objects.
#[derive(Clone, Debug, PartialEq)]
pub enum Layer {
    /// The top.
    Front,
    /// The bottom.
    Back,
    /// Directly above the first of these still there.
    After(Vec<Id>),
    /// One step past the nearest object above that overlaps them.
    Forward,
    /// One step past the nearest object below that overlaps them.
    Backward,
}

/// What [`Verb::Align`] lines up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignTo {
    /// Left edges.
    Left,
    /// Centres across.
    Center,
    /// Right edges.
    Right,
    /// Top edges.
    Top,
    /// Centres down.
    Middle,
    /// Bottom edges.
    Bottom,
}

/// Across or down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    /// Across.
    Horizontal,
    /// Down.
    Vertical,
}

/// A side of an object.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Above.
    Top,
    /// To the right.
    Right,
    /// Below.
    Bottom,
    /// To the left.
    Left,
}

impl AlignTo {
    /// Every value.
    pub const ALL: &'static [AlignTo] = &[
        AlignTo::Left,
        AlignTo::Center,
        AlignTo::Right,
        AlignTo::Top,
        AlignTo::Middle,
        AlignTo::Bottom,
    ];

    /// Its word: `left`, `center`, `right`, `top`, `middle` or `bottom`.
    pub fn as_str(self) -> &'static str {
        match self {
            AlignTo::Left => "left",
            AlignTo::Center => "center",
            AlignTo::Right => "right",
            AlignTo::Top => "top",
            AlignTo::Middle => "middle",
            AlignTo::Bottom => "bottom",
        }
    }

    /// The value a word names: [`AlignTo::as_str`]'s words, and no others.
    pub fn parse(word: &str) -> Option<AlignTo> {
        AlignTo::ALL.iter().copied().find(|a| a.as_str() == word)
    }
}

impl Axis {
    /// Every value.
    pub const ALL: &'static [Axis] = &[Axis::Horizontal, Axis::Vertical];

    /// Its word: `horizontal` or `vertical`.
    pub fn as_str(self) -> &'static str {
        match self {
            Axis::Horizontal => "horizontal",
            Axis::Vertical => "vertical",
        }
    }

    /// The value a word names: [`Axis::as_str`]'s words, and no others.
    pub fn parse(word: &str) -> Option<Axis> {
        Axis::ALL.iter().copied().find(|a| a.as_str() == word)
    }
}

impl Side {
    /// Every value.
    pub const ALL: &'static [Side] = &[Side::Top, Side::Right, Side::Bottom, Side::Left];

    /// Its word: `top`, `right`, `bottom` or `left`.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Right => "right",
            Side::Bottom => "bottom",
            Side::Left => "left",
        }
    }

    /// The value a word names: [`Side::as_str`]'s words, and no others.
    pub fn parse(word: &str) -> Option<Side> {
        Side::ALL.iter().copied().find(|s| s.as_str() == word)
    }
}

/// An object a delete took, with what's needed to put it back where it was.
#[derive(Clone, Debug, PartialEq)]
pub struct Removed {
    /// The object as it was, `locked` included.
    pub object: Object,
    /// What was below it, nearest first.
    pub after: Vec<Id>,
    /// For a frame: the frames before it in presentation order, nearest first.
    pub frame_after: Vec<Id>,
}

/// One thing an agent can do to a board. Each has a row in [`crate::parity`].
#[derive(Clone, Debug, PartialEq)]
pub enum Verb {
    /// Adds an object of any type. A table with no `columns` or `rows` gets three of each.
    Add {
        /// Its type.
        kind: ObjectType,
        /// Its fields, without `id` and `type`.
        fields: Fields,
        /// Where it goes.
        at: Position,
    },
    /// Changes the named fields, and only those.
    Update {
        /// The object.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// Fields to set.
        set: Fields,
        /// Fields to remove.
        unset: Vec<String>,
    },
    /// Deletes an object. With `cascade`, what hangs off it goes too: attached connectors, mind
    /// children and pinned comments. A locked object refuses; a locked dependent goes with it and
    /// is named in [`Planned::removed_locked`].
    Delete {
        /// The object.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// Whether what hangs off it goes too.
        cascade: bool,
    },
    /// Puts back what a delete took, as it was.
    PutBack {
        /// From [`Planned::removed`].
        removed: Removed,
    },
    /// Adds a connector. An end's object has to be there.
    Connect {
        /// Its start.
        from: End,
        /// Its end.
        to: End,
        /// Its other fields.
        fields: Fields,
    },
    /// Adds or takes off the caller's reaction. Needs no revision: it changes only their entry.
    React {
        /// The object.
        id: Id,
        /// The reaction.
        emoji: String,
        /// On or off. `None` toggles it: on if the caller's isn't there, off if it is.
        on: Option<bool>,
    },
    /// Adds or takes back the caller's votes, while voting is on and within the settings' limit.
    Vote {
        /// The object.
        id: Id,
        /// How many to add, or take back if negative.
        delta: i32,
    },
    /// Adds a comment, pinned to an object or at a point.
    Comment {
        /// The object it's pinned to.
        target: Option<Id>,
        /// Its text.
        text: String,
        /// Where it sits, when it isn't pinned.
        point: Option<(f64, f64)>,
    },
    /// Adds the caller's reply to a comment.
    Reply {
        /// The comment.
        id: Id,
        /// Its text.
        text: String,
    },
    /// Resolves a comment or reopens it.
    Resolve {
        /// The comment.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// Resolved or not.
        resolved: bool,
    },
    /// Changes the board's settings.
    Meta {
        /// The revision the caller read.
        rev: Sha256,
        /// Fields to set.
        set: Fields,
        /// Fields to remove.
        unset: Vec<String>,
    },
    /// Locks objects or unlocks them. One already that way is left alone.
    Lock {
        /// The objects.
        ids: Vec<ObjRev>,
        /// Locked or not.
        locked: bool,
    },
    /// Adds a check item.
    CheckAdd {
        /// The object.
        id: Id,
        /// Its text.
        text: String,
    },
    /// Changes a check item.
    CheckSet {
        /// The object.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// The item's id.
        item: String,
        /// New text, if any.
        text: Option<String>,
        /// Ticked or not, if given.
        done: Option<bool>,
    },
    /// Removes a check item.
    CheckRemove {
        /// The object.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// The item's id.
        item: String,
    },
    /// Sets a table cell.
    TableCell {
        /// The table.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// The row's id.
        row: String,
        /// The column's id.
        column: String,
        /// Its text.
        text: String,
    },
    /// Adds a table row.
    TableAddRow {
        /// The table.
        id: Id,
        /// The row it goes below, or the top.
        after: Option<String>,
    },
    /// Removes a table row and its cells.
    TableRemoveRow {
        /// The table.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// The row's id.
        row: String,
    },
    /// Adds a table column.
    TableAddColumn {
        /// The table.
        id: Id,
        /// The column it goes after, or the first.
        after: Option<String>,
        /// Its width, if any.
        width: Option<f64>,
    },
    /// Removes a table column and its cells.
    TableRemoveColumn {
        /// The table.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// The column's id.
        column: String,
    },
    /// Moves objects by an offset. A group's members and a frame's contents come along; a locked
    /// one stays and is named in [`Planned::skipped`].
    Move {
        /// The objects.
        ids: Vec<ObjRev>,
        /// Across.
        dx: f64,
        /// Down.
        dy: f64,
    },
    /// Moves objects in z-order, keeping their order among themselves.
    Order {
        /// The objects.
        ids: Vec<Id>,
        /// Where they go.
        to: Layer,
    },
    /// Moves a frame in presentation order.
    FrameOrder {
        /// The frame.
        id: Id,
        /// Where it goes.
        to: Position,
    },
    /// Makes objects one group.
    Group {
        /// At least two objects.
        ids: Vec<ObjRev>,
    },
    /// Takes objects out of their groups. One in no group is named in [`Planned::skipped`].
    Ungroup {
        /// The objects.
        ids: Vec<ObjRev>,
    },
    /// Lines objects up, by their stored boxes.
    Align {
        /// At least two objects.
        ids: Vec<ObjRev>,
        /// What lines up.
        to: AlignTo,
    },
    /// Spaces objects evenly, by their stored boxes.
    Distribute {
        /// At least three objects.
        ids: Vec<ObjRev>,
        /// Across or down.
        axis: Axis,
    },
    /// Makes neat rows from a messy selection. Connectors, frames, drawings and comments stay put
    /// and are named in [`Planned::skipped`].
    Tidy {
        /// The objects.
        ids: Vec<ObjRev>,
    },
    /// Copies the look of one object to others: only fields the other type holds in the same kind.
    CopyStyle {
        /// The source.
        from: Id,
        /// The objects it goes to.
        to: Vec<ObjRev>,
    },
    /// Copies objects with fresh ids, keeping connectors, groups and mind links inside the set.
    Duplicate {
        /// The objects.
        ids: Vec<Id>,
        /// Across.
        dx: f64,
        /// Down.
        dy: f64,
    },
    /// Adds objects read earlier, from this board or another, with fresh ids, centred on a point.
    Paste {
        /// The objects, back to front.
        objects: Vec<Object>,
        /// The point across.
        x: f64,
        /// The point down.
        y: f64,
    },
    /// Adds an empty copy of a sticky, shape or card on one side of it, joined by a connector.
    ConnectedCopy {
        /// The object.
        id: Id,
        /// The side.
        side: Side,
    },
    /// Adds a mind-map child, laid out beside its parent. A sibling is a child of the same parent.
    MindChild {
        /// The parent.
        parent: Id,
        /// Its text.
        text: String,
    },
    /// Adds a template's objects at a point. [`TEMPLATES`] names them.
    Template {
        /// Its name.
        name: String,
        /// Across.
        x: f64,
        /// Down.
        y: f64,
    },
    /// Clears every reference to a blob: how a file with a missing picture becomes saveable.
    ClearBlob {
        /// The blob.
        hash: Sha256,
    },
}

impl Verb {
    /// The verb's name, as [`crate::parity`] and `docs/VERBS.md` give it.
    pub fn name(&self) -> &'static str {
        match self {
            Verb::Add { .. } => "add",
            Verb::Update { .. } => "update",
            Verb::Delete { .. } => "delete",
            Verb::PutBack { .. } => "put-back",
            Verb::Connect { .. } => "connect",
            Verb::React { .. } => "react",
            Verb::Vote { .. } => "vote",
            Verb::Comment { .. } => "comment",
            Verb::Reply { .. } => "reply",
            Verb::Resolve { .. } => "resolve",
            Verb::Meta { .. } => "meta",
            Verb::Lock { .. } => "lock",
            Verb::CheckAdd { .. } => "check-add",
            Verb::CheckSet { .. } => "check-set",
            Verb::CheckRemove { .. } => "check-remove",
            Verb::TableCell { .. } => "table-cell",
            Verb::TableAddRow { .. } => "table-add-row",
            Verb::TableRemoveRow { .. } => "table-remove-row",
            Verb::TableAddColumn { .. } => "table-add-column",
            Verb::TableRemoveColumn { .. } => "table-remove-column",
            Verb::Move { .. } => "move",
            Verb::Order { .. } => "order",
            Verb::FrameOrder { .. } => "frame-order",
            Verb::Group { .. } => "group",
            Verb::Ungroup { .. } => "ungroup",
            Verb::Align { .. } => "align",
            Verb::Distribute { .. } => "distribute",
            Verb::Tidy { .. } => "tidy",
            Verb::CopyStyle { .. } => "copy-style",
            Verb::Duplicate { .. } => "duplicate",
            Verb::Paste { .. } => "paste",
            Verb::ConnectedCopy { .. } => "connected-copy",
            Verb::MindChild { .. } => "mind-child",
            Verb::Template { .. } => "template",
            Verb::ClearBlob { .. } => "clear-blob",
        }
    }
}

/// Every verb [`plan`] takes, by name.
pub const WRITE_VERBS: &[&str] = &[
    "add",
    "update",
    "delete",
    "put-back",
    "connect",
    "react",
    "vote",
    "comment",
    "reply",
    "resolve",
    "meta",
    "lock",
    "check-add",
    "check-set",
    "check-remove",
    "table-cell",
    "table-add-row",
    "table-remove-row",
    "table-add-column",
    "table-remove-column",
    "move",
    "order",
    "frame-order",
    "group",
    "ungroup",
    "align",
    "distribute",
    "tidy",
    "copy-style",
    "duplicate",
    "paste",
    "connected-copy",
    "mind-child",
    "template",
    "clear-blob",
];

/// The verbs that read, and those that apply raw ops or undo.
pub const OTHER_VERBS: &[&str] = &[
    "list",
    "read",
    "search",
    "fields",
    "templates",
    "references",
    "apply",
    "inverse",
];

/// Every verb's arguments, by name, for a host matching its own command line to these verbs. A name
/// ending in `?` may be left out. For a [`Verb`], the names are its variant's fields, and
/// `tests/host_cli.rs` holds them to the variants.
pub const ARGS: &[(&str, &[&str])] = &[
    ("add", &["kind", "fields", "at"]),
    ("update", &["id", "rev", "set", "unset"]),
    ("delete", &["id", "rev", "cascade"]),
    ("put-back", &["removed"]),
    ("connect", &["from", "to", "fields"]),
    ("react", &["id", "emoji", "on?"]),
    ("vote", &["id", "delta"]),
    ("comment", &["target?", "text", "point?"]),
    ("reply", &["id", "text"]),
    ("resolve", &["id", "rev", "resolved"]),
    ("meta", &["rev", "set", "unset"]),
    ("lock", &["ids", "locked"]),
    ("check-add", &["id", "text"]),
    ("check-set", &["id", "rev", "item", "text?", "done?"]),
    ("check-remove", &["id", "rev", "item"]),
    ("table-cell", &["id", "rev", "row", "column", "text"]),
    ("table-add-row", &["id", "after?"]),
    ("table-remove-row", &["id", "rev", "row"]),
    ("table-add-column", &["id", "after?", "width?"]),
    ("table-remove-column", &["id", "rev", "column"]),
    ("move", &["ids", "dx", "dy"]),
    ("order", &["ids", "to"]),
    ("frame-order", &["id", "to"]),
    ("group", &["ids"]),
    ("ungroup", &["ids"]),
    ("align", &["ids", "to"]),
    ("distribute", &["ids", "axis"]),
    ("tidy", &["ids"]),
    ("copy-style", &["from", "to"]),
    ("duplicate", &["ids", "dx", "dy"]),
    ("paste", &["objects", "x", "y"]),
    ("connected-copy", &["id", "side"]),
    ("mind-child", &["parent", "text"]),
    ("template", &["name", "x", "y"]),
    ("clear-blob", &["hash"]),
    ("list", &["kind?"]),
    ("read", &["id"]),
    ("search", &["text"]),
    ("fields", &["type?"]),
    ("templates", &[]),
    ("references", &["id?"]),
    ("apply", &["ops"]),
    ("inverse", &["before", "after", "group_id"]),
];

/// The arguments that are JSON, not text, by verb and name, and why: a whole value a command line
/// has no text form for. Every other argument in [`ARGS`] has one.
pub const JSON_ARGS: &[(&str, &str, &str)] = &[
    (
        "put-back",
        "removed",
        "a whole object a delete took, with its neighbours: one of the delete's removed, as \
         Removed::to_value gives it",
    ),
    (
        "paste",
        "objects",
        "whole objects, as read earlier: each as Object::to_value gives it",
    ),
    (
        "inverse",
        "before",
        "a whole board: the board before the change",
    ),
    (
        "inverse",
        "after",
        "a whole board: the board after the change",
    ),
    (
        "apply",
        "ops",
        "raw ops, as group_from_json and labelled_from_json read them",
    ),
];

/// A verb's result: a group to apply, and what a caller wants to know about it.
#[derive(Clone, Debug, PartialEq)]
pub struct Planned {
    /// The group, for [`crate::apply_group`] or [`crate::BoardFile::apply`].
    pub group: Group,
    /// The ids of new objects.
    pub ids: Vec<Id>,
    /// New ids by the caller's labels, for [`labelled_from_json`].
    pub labels: BTreeMap<String, Id>,
    /// What a delete takes, back to front, for [`Verb::PutBack`].
    pub removed: Vec<Removed>,
    /// Locked objects a delete takes along. Nobody loses a locked object without being told.
    pub removed_locked: Vec<Id>,
    /// References a delete leaves with no target.
    pub orphans: Vec<Reference>,
    /// Objects the verb left alone, and it says so here.
    pub skipped: Vec<Id>,
}

/// Why a verb can't be planned. The strings from [`VerbCode::as_str`] are stable. These are the
/// API's, not the file format's: a file never carries them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum VerbCode {
    /// This needs to know who's acting, or when.
    NeedsActor,
    /// Voting isn't on.
    VotingOff,
    /// The caller has no votes left.
    NoVotesLeft,
    /// This doesn't apply to that type of object.
    WrongType,
    /// Too few objects for this.
    TooFew,
    /// No template by that name.
    NoTemplate,
    /// A field name the format doesn't hold for that type. Raw ops keep it; a verb refuses it.
    UnknownField,
    /// A type the format doesn't know. Raw ops keep it; a verb refuses it.
    UnknownType,
    /// No check item, row or column by that id.
    NoSuchItem,
    /// The host's random bytes keep landing on ids in use.
    NoFreeId,
    /// A value this verb can't use.
    BadValue,
}

impl VerbCode {
    /// Every code.
    pub const ALL: &'static [VerbCode] = &[
        VerbCode::NeedsActor,
        VerbCode::VotingOff,
        VerbCode::NoVotesLeft,
        VerbCode::WrongType,
        VerbCode::TooFew,
        VerbCode::NoTemplate,
        VerbCode::UnknownField,
        VerbCode::UnknownType,
        VerbCode::NoSuchItem,
        VerbCode::NoFreeId,
        VerbCode::BadValue,
    ];

    /// The stable string for this code.
    pub fn as_str(self) -> &'static str {
        match self {
            VerbCode::NeedsActor => "needs_actor",
            VerbCode::VotingOff => "voting_off",
            VerbCode::NoVotesLeft => "no_votes_left",
            VerbCode::WrongType => "wrong_type",
            VerbCode::TooFew => "too_few",
            VerbCode::NoTemplate => "no_template",
            VerbCode::UnknownField => "unknown_field",
            VerbCode::UnknownType => "unknown_type",
            VerbCode::NoSuchItem => "no_such_item",
            VerbCode::NoFreeId => "no_free_id",
            VerbCode::BadValue => "bad_value",
        }
    }
}

/// What a verb's own refusal is about, as data: what the place that refused knew. Each
/// [`VerbCode`] sets the facts its meaning names, and leaves the others `None`:
///
/// | Code | Facts |
/// |---|---|
/// | [`VerbCode::WrongType`] | `id`, `type_name`: the object the verb doesn't apply to, and its type |
/// | [`VerbCode::TooFew`] | `min`: the fewest objects the verb takes |
/// | [`VerbCode::NoTemplate`] | `name`: the template asked for |
/// | [`VerbCode::UnknownField`] | `type_name`, `field`: the type, and the field it doesn't hold |
/// | [`VerbCode::UnknownType`] | `type_name`: the type asked for |
/// | [`VerbCode::NoSuchItem`] | `id`, `item`: the object, and the item id it doesn't hold |
/// | the others | none |
///
/// `non_exhaustive`: read its fields; a later fact isn't a breaking change.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Facts {
    /// The object it's about.
    pub id: Option<Id>,
    /// The type it's about: the object's, or the one asked for.
    pub type_name: Option<String>,
    /// A field name.
    pub field: Option<String>,
    /// A check item's, row's or column's id.
    pub item: Option<String>,
    /// A template's name.
    pub name: Option<String>,
    /// The fewest objects a verb takes.
    pub min: Option<u64>,
}

/// Why a verb can't be planned, or the conflict that stopped it.
///
/// `non_exhaustive`: a `match` on it needs a wildcard arm, so a later kind isn't a breaking change.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum VerbError {
    /// A conflict or a typed refusal from the board.
    Conflict(Conflict),
    /// The verb's own refusal.
    Refused {
        /// Why.
        code: VerbCode,
        /// A sentence for people. Not stable; match on `code`.
        detail: String,
        /// What it's about, as data ([`Facts`]), boxed so the error stays small.
        facts: Box<Facts>,
    },
}

impl From<Conflict> for VerbError {
    fn from(c: Conflict) -> VerbError {
        VerbError::Conflict(c)
    }
}

impl From<Refusal> for VerbError {
    fn from(r: Refusal) -> VerbError {
        VerbError::Conflict(Conflict::Refused(r))
    }
}

impl fmt::Display for VerbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerbError::Conflict(c) => write!(f, "{c:?}"),
            VerbError::Refused { code, detail, .. } => write!(f, "{}: {detail}", code.as_str()),
        }
    }
}

impl std::error::Error for VerbError {}

impl Facts {
    /// The object it's about, and its type.
    pub(crate) fn object(id: &Id, type_name: &str) -> Facts {
        Facts {
            id: Some(id.clone()),
            type_name: Some(type_name.to_string()),
            ..Facts::default()
        }
    }

    /// A type, and maybe a field it doesn't hold.
    pub(crate) fn type_field(type_name: &str, field: Option<&str>) -> Facts {
        Facts {
            type_name: Some(type_name.to_string()),
            field: field.map(str::to_string),
            ..Facts::default()
        }
    }

    /// The fewest objects a verb takes.
    pub(crate) fn min(min: u64) -> Facts {
        Facts {
            min: Some(min),
            ..Facts::default()
        }
    }
}

pub(crate) fn refuse(code: VerbCode, detail: impl Into<String>) -> VerbError {
    refuse_with(code, detail, Facts::default())
}

/// A verb's refusal, with what it's about.
pub(crate) fn refuse_with(code: VerbCode, detail: impl Into<String>, facts: Facts) -> VerbError {
    VerbError::Refused {
        code,
        detail: detail.into(),
        facts: Box::new(facts),
    }
}

pub(crate) fn s(text: &str) -> Value {
    Value::Str(text.to_string())
}

pub(crate) fn one(name: &str, v: Value) -> Fields {
    let mut f = Fields::new();
    f.insert(name.to_string(), v);
    f
}

/// Plans a verb on a board. Nothing changes until the group is applied.
pub fn plan(board: &Board, cx: &mut Ctx<'_>, verb: &Verb) -> Result<Planned, VerbError> {
    let mut p = Planner::new(board, cx);
    match verb {
        Verb::Add { kind, fields, at } => p.add(kind, fields, at)?,
        Verb::Update {
            id,
            rev,
            set,
            unset,
        } => p.update(id, *rev, set, unset)?,
        Verb::Delete { id, rev, cascade } => p.delete(id, *rev, *cascade)?,
        Verb::PutBack { removed } => p.put_back(removed)?,
        Verb::Connect { from, to, fields } => p.connect(from, to, fields)?,
        Verb::React { id, emoji, on } => p.react(id, emoji, *on)?,
        Verb::Vote { id, delta } => p.vote(id, *delta)?,
        Verb::Comment {
            target,
            text,
            point,
        } => p.comment(target.as_ref(), text, *point)?,
        Verb::Reply { id, text } => p.reply(id, text)?,
        Verb::Resolve { id, rev, resolved } => p.resolve(id, *rev, *resolved)?,
        Verb::Meta { rev, set, unset } => p.ops.push(Op::Meta {
            rev: *rev,
            set: set.clone(),
            unset: unset.clone(),
        }),
        Verb::Lock { ids, locked } => p.lock(ids, *locked)?,
        Verb::CheckAdd { id, text } => p.check_add(id, text)?,
        Verb::CheckSet {
            id,
            rev,
            item,
            text,
            done,
        } => p.check_set(id, *rev, item, text.as_deref(), *done)?,
        Verb::CheckRemove { id, rev, item } => p.check_remove(id, *rev, item)?,
        Verb::TableCell {
            id,
            rev,
            row,
            column,
            text,
        } => p.table_cell(id, *rev, row, column, text)?,
        Verb::TableAddRow { id, after } => p.table_add_row(id, after.as_deref())?,
        Verb::TableRemoveRow { id, rev, row } => p.table_remove_row(id, *rev, row)?,
        Verb::TableAddColumn { id, after, width } => {
            p.table_add_column(id, after.as_deref(), *width)?
        }
        Verb::TableRemoveColumn { id, rev, column } => p.table_remove_column(id, *rev, column)?,
        Verb::Move { ids, dx, dy } => p.move_by(ids, *dx, *dy)?,
        Verb::Order { ids, to } => p.order(ids, to)?,
        Verb::FrameOrder { id, to } => p.ops.push(Op::FrameOrder {
            id: id.clone(),
            to: to.clone(),
        }),
        Verb::Group { ids } => p.group(ids)?,
        Verb::Ungroup { ids } => p.ungroup(ids)?,
        Verb::Align { ids, to } => p.align(ids, *to)?,
        Verb::Distribute { ids, axis } => p.distribute(ids, *axis)?,
        Verb::Tidy { ids } => p.tidy(ids)?,
        Verb::CopyStyle { from, to } => p.copy_style(from, to)?,
        Verb::Duplicate { ids, dx, dy } => p.duplicate(ids, *dx, *dy)?,
        Verb::Paste { objects, x, y } => p.paste(objects, *x, *y)?,
        Verb::ConnectedCopy { id, side } => p.connected_copy(id, *side)?,
        Verb::MindChild { parent, text } => p.mind_child(parent, text)?,
        Verb::Template { name, x, y } => p.template(name, *x, *y)?,
        Verb::ClearBlob { hash } => p.ops.push(Op::ClearBlob { hash: *hash }),
    }
    Ok(p.finish())
}

/// What a verb builds while it plans.
pub(crate) struct Planner<'a, 'c> {
    pub(crate) board: &'a Board,
    pub(crate) cx: &'a mut Ctx<'c>,
    pub(crate) index: HashMap<&'a str, &'a Object>,
    pub(crate) ops: Vec<Op>,
    pub(crate) ids: Vec<Id>,
    pub(crate) labels: BTreeMap<String, Id>,
    pub(crate) removed: Vec<Removed>,
    pub(crate) removed_locked: Vec<Id>,
    pub(crate) orphans: Vec<Reference>,
    pub(crate) skipped: Vec<Id>,
    taken: BTreeSet<Id>,
}

impl<'a, 'c> Planner<'a, 'c> {
    pub(crate) fn new(board: &'a Board, cx: &'a mut Ctx<'c>) -> Planner<'a, 'c> {
        Planner {
            board,
            cx,
            index: board.objects.iter().map(|o| (o.id.as_str(), o)).collect(),
            ops: Vec::new(),
            ids: Vec::new(),
            labels: BTreeMap::new(),
            removed: Vec::new(),
            removed_locked: Vec::new(),
            orphans: Vec::new(),
            skipped: Vec::new(),
            taken: BTreeSet::new(),
        }
    }

    pub(crate) fn finish(self) -> Planned {
        let mut ops = Vec::with_capacity(self.ops.len() + 1);
        if let Some(Me {
            token,
            new: Some(person),
        }) = &self.cx.by
        {
            ops.push(Op::AddPerson {
                token: token.clone(),
                person: person.clone(),
            });
        }
        ops.extend(self.ops);
        Planned {
            group: Group {
                id: self.cx.group_id.clone(),
                by: self.cx.by.as_ref().map(|m| m.token.clone()),
                ops,
            },
            ids: self.ids,
            labels: self.labels,
            removed: self.removed,
            removed_locked: self.removed_locked,
            orphans: self.orphans,
            skipped: self.skipped,
        }
    }

    /// The object, or the conflict that says it's gone.
    pub(crate) fn obj(&self, id: &Id) -> Result<&'a Object, VerbError> {
        self.index
            .get(id.as_str())
            .copied()
            .ok_or_else(|| Conflict::Deleted { id: id.clone() }.into())
    }

    pub(crate) fn token(&self) -> Result<Id, VerbError> {
        match &self.cx.by {
            Some(me) => Ok(me.token.clone()),
            None => Err(refuse(
                VerbCode::NeedsActor,
                "this needs to know who's acting",
            )),
        }
    }

    /// `<prefix>-<10 hex>`, free on this board and in this plan.
    pub(crate) fn mint(&mut self, prefix: &str) -> Result<Id, VerbError> {
        for _ in 0..8 {
            let random = (self.cx.random)();
            let id = Id::new(&format!("{prefix}-{}", hex(&random[..5])))?;
            if !self.index.contains_key(id.as_str()) && self.taken.insert(id.clone()) {
                return Ok(id);
            }
        }
        Err(refuse(
            VerbCode::NoFreeId,
            "the random bytes keep repeating",
        ))
    }

    /// A sub-item's id: unique in its list, so a hit on the board's objects doesn't matter.
    pub(crate) fn mint_sub(&mut self, prefix: &str) -> Result<String, VerbError> {
        Ok(self.mint(prefix)?.as_str().to_string())
    }

    pub(crate) fn set_op(&mut self, id: &Id, rev: Sha256, set: Fields, unset: Vec<String>) {
        self.ops.push(Op::Set {
            id: id.clone(),
            rev,
            set,
            unset,
        });
    }

    /// Adds an object with a fresh id, stamped with who and when.
    pub(crate) fn add_new(
        &mut self,
        kind: &ObjectType,
        mut fields: Fields,
        at: &Position,
    ) -> Result<Id, VerbError> {
        let id = self.mint(id_prefix(kind))?;
        self.stamp(&mut fields);
        self.ops.push(Op::Add {
            object: Object {
                id: id.clone(),
                kind: kind.clone(),
                fields,
            },
            at: at.clone(),
        });
        self.ids.push(id.clone());
        Ok(id)
    }

    pub(crate) fn stamp(&self, fields: &mut Fields) {
        if let (Some(me), Some(at)) = (&self.cx.by, &self.cx.at) {
            let mut c = one("by", Value::Str(me.token.to_string()));
            c.insert("at".into(), Value::Str(at.clone()));
            fields.entry("created".to_string()).or_insert(Value::Obj(c));
        }
    }
}

/// The first letter of a new id, by type. Ids are opaque: this is only so they read well.
pub(crate) fn id_prefix(kind: &ObjectType) -> &'static str {
    match kind {
        ObjectType::Sticky => "s",
        ObjectType::Text => "t",
        ObjectType::Shape => "h",
        ObjectType::Connector => "c",
        ObjectType::Pen => "p",
        ObjectType::Frame => "f",
        ObjectType::Card => "k",
        ObjectType::Table => "b",
        ObjectType::Mind => "m",
        ObjectType::Comment => "q",
        ObjectType::Image => "i",
        ObjectType::Unknown(_) => "o",
    }
}

/// Refuses a type the format doesn't know, and a field name it doesn't hold for that type. Raw ops
/// keep both, as the format does. A verb refuses them, so a misspelt name is never kept unseen.
pub(crate) fn known_fields<'n>(
    kind: &ObjectType,
    names: impl IntoIterator<Item = &'n str>,
) -> Result<(), VerbError> {
    if matches!(kind, ObjectType::Unknown(_)) {
        return Err(refuse_with(
            VerbCode::UnknownType,
            format!("{:?} isn't a type this version knows", kind.as_str()),
            Facts::type_field(kind.as_str(), None),
        ));
    }
    for name in names {
        if schema::field_of(kind.as_str(), name).is_none() {
            return Err(refuse_with(
                VerbCode::UnknownField,
                format!("a {} has no field {name:?}", kind.as_str()),
                Facts::type_field(kind.as_str(), Some(name)),
            ));
        }
    }
    Ok(())
}

pub(crate) fn no_id_or_type(fields: &Fields) -> Result<(), VerbError> {
    for name in ["id", "type"] {
        if fields.contains_key(name) {
            return Err(Refusal::at(
                RefusalCode::FieldValue,
                format!("/{name}"),
                "the verb sets this itself",
            )
            .into());
        }
    }
    Ok(())
}
