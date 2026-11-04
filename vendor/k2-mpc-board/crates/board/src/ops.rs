//! Edits: one group of ops applied to a board, all or nothing. The compare is inside: every op that
//! edits an object or the settings names the revision it read, and a group that doesn't fit changes
//! nothing. Order is the one exception to "never last-writer-wins": see [`Position`].
//! A locked object follows one rule for everyone: see [`Op::Set`].

use crate::hash::Sha256;
use crate::json::{Fields, Value};
use crate::limits::Limits;
use crate::model::{Board, Id, Object, ObjectType, Person};
use crate::refusal::{Refusal, RefusalCode};
use crate::schema;
use crate::validate;
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// Where an object goes in z-order, or a frame in presentation order.
///
/// Position isn't part of a revision, or every insert would make false conflicts. So a move never
/// conflicts: the result is deterministic, nothing is lost or duplicated, and when one object is
/// moved on both sides the later save's position stands, as [`Report::moved`] says.
#[derive(Clone, Debug, PartialEq)]
pub enum Position {
    /// The top of the list.
    Front,
    /// The bottom of the list.
    Back,
    /// Directly above the first of these that's still there, nearest neighbour first. If none is
    /// there, the bottom.
    After(Vec<Id>),
}

/// One edit.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// Adds an object. An id in use is [`Conflict::Exists`].
    Add {
        /// The object.
        object: Object,
        /// Where it goes.
        at: Position,
    },
    /// Changes the named fields of an object, and only those. A stale `rev` is
    /// [`Conflict::Changed`]; an object that's gone is [`Conflict::Deleted`].
    ///
    /// `reactions` and `votes` change only the entries of the group's `by` token; with no `by` they
    /// are replaced whole. `checks`, `replies`, `rows` and `columns` merge by id: an item keeps the
    /// sub-fields the caller didn't send, and the caller's list sets the order and which items stay.
    ///
    /// A locked object refuses this with `object_locked`, as it does `Del`, `Z` and `FrameOrder`,
    /// unless the op only changes `locked` to false, `votes` or `reactions`. The state follows the
    /// group, so "unlock, then edit" in one group passes and "lock, then edit" doesn't.
    Set {
        /// The object.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
        /// Fields to set.
        set: Fields,
        /// Fields to remove.
        unset: Vec<String>,
    },
    /// Deletes an object, and its place in `frames`. A stale `rev` is [`Conflict::Changed`]; an
    /// object already gone is a no-op, listed in [`Report::already_gone`]. What referred to it
    /// becomes orphaned; nothing else is deleted.
    Del {
        /// The object.
        id: Id,
        /// The revision the caller read.
        rev: Sha256,
    },
    /// Changes the board's settings. A stale `rev` is [`Conflict::Changed`].
    Meta {
        /// The revision the caller read, from [`Board::settings_rev`].
        rev: Sha256,
        /// Fields to set.
        set: Fields,
        /// Fields to remove.
        unset: Vec<String>,
    },
    /// Moves an object in z-order. An object that's gone is [`Conflict::Deleted`].
    Z {
        /// The object.
        id: Id,
        /// Where it goes.
        to: Position,
    },
    /// Moves a frame in presentation order. An object that's gone is [`Conflict::Deleted`].
    FrameOrder {
        /// The frame.
        id: Id,
        /// Where it goes.
        to: Position,
    },
    /// Adds a person. The same person again is a no-op; a different one under that token is
    /// [`Conflict::Exists`].
    AddPerson {
        /// Their token, from [`crate::mint_person_token`].
        token: Id,
        /// The person.
        person: Person,
    },
    /// Clears every reference to a blob: a known blob field is removed, and a reference inside an
    /// unknown field becomes `null`. It names no revision and ignores locks: it's how a host makes
    /// a file with a missing picture saveable again.
    ClearBlob {
        /// The blob.
        hash: Sha256,
    },
}

/// One gesture's ops: all of them apply, or none.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// The caller's id for this group.
    pub id: String,
    /// The token of whoever is acting, for `reactions` and `votes`.
    pub by: Option<Id>,
    /// The ops, in order.
    pub ops: Vec<Op>,
}

/// What a conflict is about.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    /// An object.
    Object(Id),
    /// The board's settings.
    Settings,
    /// A person.
    Person(Id),
}

/// Why a group changed nothing.
#[derive(Clone, Debug, PartialEq)]
pub enum Conflict {
    /// It changed since the caller read it. `theirs` is what's there now.
    Changed {
        /// What changed.
        target: Target,
        /// What's there now, as JSON.
        theirs: Value,
    },
    /// The object is gone.
    Deleted {
        /// The object.
        id: Id,
    },
    /// The id or token is in use. `theirs` is what's there now.
    Exists {
        /// What's there.
        target: Target,
        /// What's there now, as JSON.
        theirs: Value,
    },
    /// The result wouldn't be valid, or the object is locked.
    Refused(Refusal),
}

/// Which order a move changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderList {
    /// Z-order.
    Objects,
    /// Presentation order.
    Frames,
}

/// A move that changed the order. The later save's position stands, so a host can show this.
#[derive(Clone, Debug, PartialEq)]
pub struct Moved {
    /// What moved.
    pub id: Id,
    /// In which list.
    pub list: OrderList,
    /// What's directly below it now.
    pub after: Option<Id>,
    /// False when the first neighbour named was gone, so it landed by a further one, or at the
    /// bottom.
    pub exact: bool,
}

/// What a group did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// Objects added.
    pub added: Vec<Id>,
    /// Objects whose fields changed.
    pub changed: Vec<Id>,
    /// Objects deleted.
    pub deleted: Vec<Id>,
    /// `Del` ops whose object was already gone.
    pub already_gone: Vec<Id>,
    /// Moves that changed an order.
    pub moved: Vec<Moved>,
    /// Whether the settings changed.
    pub settings_changed: bool,
    /// People added.
    pub people_added: Vec<Id>,
    /// Blobs whose references were cleared.
    pub blobs_cleared: Vec<Sha256>,
}

impl Report {
    /// True when the board didn't change, so nothing needs writing.
    pub fn is_noop(&self) -> bool {
        self.added.is_empty()
            && self.changed.is_empty()
            && self.deleted.is_empty()
            && self.moved.is_empty()
            && !self.settings_changed
            && self.people_added.is_empty()
            && self.blobs_cleared.is_empty()
    }
}

/// Every refusal code [`apply_group`] can refuse a group with, as [`Conflict::Refused`]: its own
/// checks, a revision's canonical JSON, and the check of the result. A host that maps a refusal by
/// the call it came from reads this; `tests/group_refusals.rs` makes a group refuse with each one,
/// and the raw-ops fuzz target holds every refusal it sees to it. A new raise site lists its code here.
pub const GROUP_REFUSALS: &[RefusalCode] = &[
    // here: a locked object, a frame move of what isn't one, an id or type set, the counts
    RefusalCode::ObjectLocked,
    RefusalCode::FramesMismatch,
    RefusalCode::FieldValue,
    RefusalCode::LimitObjects,
    RefusalCode::LimitPeople,
    // a revision's canonical JSON, and the check of the result (validate.rs)
    RefusalCode::JsonNumberRange,
    RefusalCode::LimitDepth,
    RefusalCode::FieldKind,
    RefusalCode::LimitField,
    RefusalCode::FieldNameInvalid,
    RefusalCode::PersonUnknown,
    RefusalCode::IdDuplicate,
    RefusalCode::CellColumnUnknown,
    RefusalCode::TypeInvalid,
    RefusalCode::IdInvalid,
];

/// Applies one group to a board, all or nothing: on a conflict the board is unchanged.
///
/// Every op is judged against what's there before anything is written to the board, then the whole
/// result is checked, and only then is it committed. The cost is one id index per group, plus the
/// work of the ops. Limits are the format's: a tighter host limit applies at [`crate::write`].
pub fn apply_group(board: &mut Board, group: &Group) -> Result<Report, Conflict> {
    let mut stage = Stage::new(board);
    for op in &group.ops {
        stage.op(op, group.by.as_ref())?;
    }
    stage.check()?;
    let plan = stage.into_plan();
    Ok(plan.commit(board))
}

const LOCK_FREE: [&str; 3] = ["locked", "votes", "reactions"];

enum Step {
    Add(Id, ObjectType, Position),
    Del(Id),
    Z(Id, Position),
    Frame(Id, Position),
}

struct Stage<'b> {
    board: &'b Board,
    index: HashMap<&'b str, usize>,
    /// The current state of everything an op touched; `None` means gone.
    touched: BTreeMap<Id, Option<Object>>,
    added: BTreeSet<Id>,
    steps: Vec<Step>,
    settings: Option<Fields>,
    people: Option<BTreeMap<Id, Person>>,
    extra: Option<Fields>,
    report: Report,
}

struct Plan {
    steps: Vec<Step>,
    finals: HashMap<Id, Object>,
    settings: Option<Fields>,
    people: Option<BTreeMap<Id, Person>>,
    extra: Option<Fields>,
    report: Report,
}

fn refused(r: Refusal) -> Conflict {
    Conflict::Refused(r)
}

fn is_locked(o: &Object) -> bool {
    matches!(o.fields.get("locked"), Some(Value::Bool(true)))
}

fn locked(id: &Id) -> Conflict {
    // A code raised here is in GROUP_REFUSALS (tests/group_refusals.rs).
    refused(Refusal::at(
        RefusalCode::ObjectLocked,
        format!("/objects/{id}"),
        "this object is locked: unlock it first",
    ))
}

impl<'b> Stage<'b> {
    fn new(board: &'b Board) -> Stage<'b> {
        Stage {
            board,
            index: board
                .objects
                .iter()
                .enumerate()
                .map(|(i, o)| (o.id.as_str(), i))
                .collect(),
            touched: BTreeMap::new(),
            added: BTreeSet::new(),
            steps: Vec::new(),
            settings: None,
            people: None,
            extra: None,
            report: Report::default(),
        }
    }

    fn start(&self, id: &Id) -> Option<&'b Object> {
        self.index.get(id.as_str()).map(|i| &self.board.objects[*i])
    }

    fn current(&self, id: &Id) -> Option<&Object> {
        match self.touched.get(id) {
            Some(state) => state.as_ref(),
            None => self.start(id),
        }
    }

    /// A stale revision is a conflict. An object added in this group has none to compare, and a
    /// second op on one object names the revision the group started from.
    fn check_rev(&self, id: &Id, rev: &Sha256) -> Result<(), Conflict> {
        if self.added.contains(id) {
            return Ok(());
        }
        if let Some(start) = self.start(id) {
            // A code raised here is in GROUP_REFUSALS (tests/group_refusals.rs).
            if start.rev().map_err(refused)? != *rev {
                return Err(Conflict::Changed {
                    target: Target::Object(id.clone()),
                    theirs: start.to_value(),
                });
            }
        }
        Ok(())
    }

    fn op(&mut self, op: &Op, by: Option<&Id>) -> Result<(), Conflict> {
        match op {
            Op::Add { object, at } => self.add(object, at),
            Op::Set {
                id,
                rev,
                set,
                unset,
            } => self.set(id, rev, set, unset, by),
            Op::Del { id, rev } => self.del(id, rev),
            Op::Meta { rev, set, unset } => self.meta(rev, set, unset),
            Op::Z { id, to } => {
                let o = self
                    .current(id)
                    .ok_or_else(|| Conflict::Deleted { id: id.clone() })?;
                if is_locked(o) {
                    return Err(locked(id));
                }
                self.steps.push(Step::Z(id.clone(), to.clone()));
                Ok(())
            }
            Op::FrameOrder { id, to } => {
                let o = self
                    .current(id)
                    .ok_or_else(|| Conflict::Deleted { id: id.clone() })?;
                if o.kind != ObjectType::Frame {
                    // A code raised here is in GROUP_REFUSALS (tests/group_refusals.rs).
                    return Err(refused(Refusal::at(
                        RefusalCode::FramesMismatch,
                        format!("/objects/{id}"),
                        "this object isn't a frame",
                    )));
                }
                if is_locked(o) {
                    return Err(locked(id));
                }
                self.steps.push(Step::Frame(id.clone(), to.clone()));
                Ok(())
            }
            Op::AddPerson { token, person } => self.add_person(token, person),
            Op::ClearBlob { hash } => self.clear_blob(hash),
        }
    }

    fn add(&mut self, object: &Object, at: &Position) -> Result<(), Conflict> {
        if let Some(there) = self.current(&object.id) {
            return Err(Conflict::Exists {
                target: Target::Object(object.id.clone()),
                theirs: there.to_value(),
            });
        }
        self.touched.insert(object.id.clone(), Some(object.clone()));
        self.added.insert(object.id.clone());
        self.steps.push(Step::Add(
            object.id.clone(),
            object.kind.clone(),
            at.clone(),
        ));
        self.report.added.push(object.id.clone());
        Ok(())
    }

    fn set(
        &mut self,
        id: &Id,
        rev: &Sha256,
        set: &Fields,
        unset: &[String],
        by: Option<&Id>,
    ) -> Result<(), Conflict> {
        let current = self
            .current(id)
            .ok_or_else(|| Conflict::Deleted { id: id.clone() })?;
        self.check_rev(id, rev)?;
        let names = set.keys().chain(unset.iter());
        for name in names {
            if name == "id" || name == "type" {
                // A code raised here is in GROUP_REFUSALS (tests/group_refusals.rs).
                return Err(refused(Refusal::at(
                    RefusalCode::FieldValue,
                    format!("/objects/{id}/{name}"),
                    "an object's id and type can't be changed",
                )));
            }
        }
        if is_locked(current) {
            // The same rule as a person's: a locked object can be unlocked, voted on and reacted
            // to, and nothing else.
            let free_set = set.iter().all(|(k, v)| {
                LOCK_FREE.contains(&k.as_str()) && (k != "locked" || *v == Value::Bool(false))
            });
            let free_unset = unset.iter().all(|k| LOCK_FREE.contains(&k.as_str()));
            if !free_set || !free_unset {
                return Err(locked(id));
            }
        }
        let mut next = current.clone();
        for (name, value) in set {
            set_field(&mut next.fields, name, value, by);
        }
        for name in unset {
            unset_field(&mut next.fields, name, by);
        }
        self.touched.insert(id.clone(), Some(next));
        Ok(())
    }

    fn del(&mut self, id: &Id, rev: &Sha256) -> Result<(), Conflict> {
        let Some(current) = self.current(id) else {
            self.report.already_gone.push(id.clone());
            return Ok(());
        };
        self.check_rev(id, rev)?;
        if is_locked(current) {
            return Err(locked(id));
        }
        self.touched.insert(id.clone(), None);
        self.added.remove(id);
        self.steps.push(Step::Del(id.clone()));
        self.report.deleted.push(id.clone());
        Ok(())
    }

    fn meta(&mut self, rev: &Sha256, set: &Fields, unset: &[String]) -> Result<(), Conflict> {
        // A code raised here is in GROUP_REFUSALS (tests/group_refusals.rs).
        if self.board.settings_rev().map_err(refused)? != *rev {
            return Err(Conflict::Changed {
                target: Target::Settings,
                theirs: Value::Obj(self.board.settings.clone()),
            });
        }
        let settings = self
            .settings
            .get_or_insert_with(|| self.board.settings.clone());
        for (name, value) in set {
            settings.insert(name.clone(), value.clone());
        }
        for name in unset {
            settings.remove(name);
        }
        Ok(())
    }

    fn add_person(&mut self, token: &Id, person: &Person) -> Result<(), Conflict> {
        let people = self.people.as_ref().unwrap_or(&self.board.people);
        if let Some(there) = people.get(token) {
            if there == person {
                return Ok(());
            }
            return Err(Conflict::Exists {
                target: Target::Person(token.clone()),
                theirs: there.to_value(),
            });
        }
        self.people
            .get_or_insert_with(|| self.board.people.clone())
            .insert(token.clone(), person.clone());
        self.report.people_added.push(token.clone());
        Ok(())
    }

    fn clear_blob(&mut self, hash: &Sha256) -> Result<(), Conflict> {
        let text = hash.to_string();
        let mut cleared = false;
        let mut staged = Vec::new();
        for o in &self.board.objects {
            if self.touched.contains_key(&o.id) {
                continue;
            }
            let mut next = o.clone();
            if clear_object(&mut next, &text) {
                staged.push(next);
            }
        }
        for state in self.touched.values_mut().flatten() {
            cleared |= clear_object(state, &text);
        }
        for o in staged {
            cleared = true;
            self.touched.insert(o.id.clone(), Some(o));
        }
        let mut settings = self
            .settings
            .clone()
            .unwrap_or_else(|| self.board.settings.clone());
        if clear_fields(&mut settings, &text) {
            cleared = true;
            self.settings = Some(settings);
        }
        let mut extra = self
            .extra
            .clone()
            .unwrap_or_else(|| self.board.extra.clone());
        if clear_fields(&mut extra, &text) {
            cleared = true;
            self.extra = Some(extra);
        }
        let mut people = self
            .people
            .clone()
            .unwrap_or_else(|| self.board.people.clone());
        let mut people_cleared = false;
        for p in people.values_mut() {
            people_cleared |= clear_fields(&mut p.extra, &text);
        }
        if people_cleared {
            cleared = true;
            self.people = Some(people);
        }
        if cleared {
            self.report.blobs_cleared.push(*hash);
        }
        Ok(())
    }

    /// The whole result, before anything is written to the board.
    ///
    /// A code raised here, or by the checks it calls, is in GROUP_REFUSALS
    /// (tests/group_refusals.rs).
    fn check(&self) -> Result<(), Conflict> {
        let people = self.people.as_ref().unwrap_or(&self.board.people);
        let count = self.report.added.len() as i64 - self.report.deleted.len() as i64
            + self.board.objects.len() as i64;
        if count > Limits::V1.objects as i64 {
            return Err(refused(Refusal::new(
                RefusalCode::LimitObjects,
                format!("{count} objects, above {}", Limits::V1.objects),
            )));
        }
        if people.len() as u64 > Limits::V1.people {
            return Err(refused(Refusal::new(
                RefusalCode::LimitPeople,
                format!("{} people, above {}", people.len(), Limits::V1.people),
            )));
        }
        for token in &self.report.people_added {
            if let Some(p) = people.get(token) {
                validate::check_person(token, p).map_err(refused)?;
            }
        }
        for o in self.touched.values().flatten() {
            validate::check_one_object(o, &format!("/objects/{}", o.id), people)
                .map_err(refused)?;
        }
        if let Some(settings) = &self.settings {
            validate::check_settings(settings, people).map_err(refused)?;
        }
        Ok(())
    }

    fn into_plan(self) -> Plan {
        let mut report = self.report;
        let mut finals = HashMap::new();
        for (id, state) in self.touched {
            let Some(next) = state else { continue };
            if self.added.contains(&id) {
                finals.insert(id, next);
            } else if self.index.get(id.as_str()).map(|i| &self.board.objects[*i]) != Some(&next) {
                report.changed.push(id.clone());
                finals.insert(id, next);
            }
        }
        report.settings_changed = self
            .settings
            .as_ref()
            .is_some_and(|s| *s != self.board.settings);
        Plan {
            steps: self.steps,
            finals,
            settings: self.settings,
            people: self.people,
            extra: self.extra,
            report,
        }
    }
}

impl Plan {
    /// Nothing here can fail: every op has been judged.
    fn commit(mut self, board: &mut Board) -> Report {
        for step in self.steps {
            match step {
                Step::Add(id, kind, at) => {
                    let (place, _) = place(&board.objects, |o| &o.id, &id, &at);
                    if kind == ObjectType::Frame {
                        board.frames.push(id.clone());
                    }
                    board.objects.insert(
                        place,
                        Object {
                            id,
                            kind,
                            fields: Fields::new(),
                        },
                    );
                }
                Step::Del(id) => {
                    board.objects.retain(|o| o.id != id);
                    board.frames.retain(|f| *f != id);
                }
                Step::Z(id, to) => {
                    if let Some(moved) = move_item(&mut board.objects, |o| &o.id, &id, &to) {
                        self.report.moved.push(Moved {
                            id,
                            list: OrderList::Objects,
                            after: moved.0,
                            exact: moved.1,
                        });
                    }
                }
                Step::Frame(id, to) => {
                    if let Some(moved) = move_item(&mut board.frames, |f| f, &id, &to) {
                        self.report.moved.push(Moved {
                            id,
                            list: OrderList::Frames,
                            after: moved.0,
                            exact: moved.1,
                        });
                    }
                }
            }
        }
        if !self.finals.is_empty() {
            for o in &mut board.objects {
                if let Some(next) = self.finals.remove(&o.id) {
                    *o = next;
                }
            }
        }
        if let Some(settings) = self.settings {
            board.settings = settings;
        }
        if let Some(people) = self.people {
            board.people = people;
        }
        if let Some(extra) = self.extra {
            board.extra = extra;
        }
        self.report
    }
}

/// Where an item goes, and whether that's where it was asked to go.
fn place<T>(list: &[T], id_of: impl Fn(&T) -> &Id, id: &Id, to: &Position) -> (usize, bool) {
    match to {
        Position::Front => (list.len(), true),
        Position::Back => (0, true),
        Position::After(names) => {
            for (i, name) in names.iter().filter(|n| *n != id).enumerate() {
                if let Some(at) = list.iter().position(|x| id_of(x) == name) {
                    return (at + 1, i == 0);
                }
            }
            (0, false)
        }
    }
}

/// Moves an item. Returns what's directly below it now and whether the move was exact, or `None` if
/// it stayed where it was.
fn move_item<T>(
    list: &mut Vec<T>,
    id_of: impl Fn(&T) -> &Id,
    id: &Id,
    to: &Position,
) -> Option<(Option<Id>, bool)> {
    let from = list.iter().position(|x| id_of(x) == id)?;
    let item = list.remove(from);
    let (at, exact) = place(list, &id_of, id, to);
    list.insert(at, item);
    if at == from {
        return None;
    }
    let below = at.checked_sub(1).map(|i| id_of(&list[i]).clone());
    Some((below, exact))
}

/// A list field that merges by id.
fn merges_by_id(name: &str) -> bool {
    matches!(name, "checks" | "replies" | "rows" | "columns")
}

fn set_field(fields: &mut Fields, name: &str, value: &Value, by: Option<&Id>) {
    let merged = match (name, by) {
        ("reactions", Some(token)) => merge_reactions(fields.get(name), value, token),
        ("votes", Some(token)) => merge_votes(fields.get(name), value, token),
        _ if merges_by_id(name) => Some(merge_by_id(fields.get(name), value)),
        _ => Some(value.clone()),
    };
    match merged {
        Some(v) => {
            fields.insert(name.to_string(), v);
        }
        None => {
            fields.remove(name);
        }
    }
}

fn unset_field(fields: &mut Fields, name: &str, by: Option<&Id>) {
    let empty = Value::Obj(Fields::new());
    if by.is_some() && (name == "reactions" || name == "votes") {
        // With a caller, this takes away only the caller's own entries.
        set_field(fields, name, &empty, by);
    } else {
        fields.remove(name);
    }
}

/// The caller's entries come from their value and everyone else's stay. `None` if nothing is left.
fn merge_reactions(existing: Option<&Value>, value: &Value, token: &Id) -> Option<Value> {
    let Value::Obj(incoming) = value else {
        return Some(value.clone()); // the wrong kind: validation refuses it, with its reason
    };
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if let Some(Value::Obj(old)) = existing {
        for (key, list) in old {
            if let Value::Arr(items) = list {
                let kept: BTreeSet<String> = items
                    .iter()
                    .filter_map(|v| match v {
                        Value::Str(s) if s != token.as_str() => Some(s.clone()),
                        _ => None,
                    })
                    .collect();
                out.insert(key.clone(), kept);
            }
        }
    }
    for (key, list) in incoming {
        let has = matches!(list, Value::Arr(items)
            if items.iter().any(|v| matches!(v, Value::Str(s) if s == token.as_str())));
        if has {
            out.entry(key.clone())
                .or_default()
                .insert(token.as_str().to_string());
        }
    }
    let result: Fields = out
        .into_iter()
        .filter(|(_, people)| !people.is_empty())
        .map(|(key, people)| {
            (
                key,
                Value::Arr(people.into_iter().map(Value::Str).collect()),
            )
        })
        .collect();
    if result.is_empty() {
        None
    } else {
        Some(Value::Obj(result))
    }
}

/// The caller's entry comes from their value and everyone else's stay. `None` if nothing is left.
fn merge_votes(existing: Option<&Value>, value: &Value, token: &Id) -> Option<Value> {
    let Value::Obj(incoming) = value else {
        return Some(value.clone()); // the wrong kind: validation refuses it, with its reason
    };
    let mut out = match existing {
        Some(Value::Obj(old)) => old.clone(),
        _ => Fields::new(),
    };
    match incoming.get(token.as_str()) {
        Some(v) => {
            out.insert(token.as_str().to_string(), v.clone());
        }
        None => {
            out.remove(token.as_str());
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(Value::Obj(out))
    }
}

/// The caller's list sets the order and which items stay. An item keeps the sub-fields the caller
/// didn't send.
fn merge_by_id(existing: Option<&Value>, value: &Value) -> Value {
    let (Some(Value::Arr(old)), Value::Arr(incoming)) = (existing, value) else {
        return value.clone();
    };
    let id_of = |v: &Value| match v {
        Value::Obj(f) => match f.get("id") {
            Some(Value::Str(s)) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    };
    let by_id: HashMap<String, &Fields> = old
        .iter()
        .filter_map(|v| match (id_of(v), v) {
            (Some(i), Value::Obj(f)) => Some((i, f)),
            _ => None,
        })
        .collect();
    let items = incoming
        .iter()
        .map(|item| match (id_of(item), item) {
            (Some(i), Value::Obj(f)) => match by_id.get(&i) {
                Some(prior) => {
                    let mut merged = (*prior).clone();
                    merged.extend(f.iter().map(|(k, v)| (k.clone(), v.clone())));
                    Value::Obj(merged)
                }
                None => item.clone(),
            },
            _ => item.clone(),
        })
        .collect();
    Value::Arr(items)
}

fn is_ref(v: &Value, text: &str) -> bool {
    matches!(v, Value::Obj(f) if f.len() == 1
        && matches!(f.get("blob"), Some(Value::Str(s)) if s == text))
}

/// Turns references to this blob into `null`, anywhere in a value.
fn clear_value(v: &mut Value, text: &str) -> bool {
    if is_ref(v, text) {
        *v = Value::Null;
        return true;
    }
    let mut any = false;
    match v {
        Value::Obj(f) => {
            for x in f.values_mut() {
                any |= clear_value(x, text);
            }
        }
        Value::Arr(a) => {
            for x in a.iter_mut() {
                any |= clear_value(x, text);
            }
        }
        _ => {}
    }
    any
}

fn clear_fields(fields: &mut Fields, text: &str) -> bool {
    let mut any = false;
    for v in fields.values_mut() {
        any |= clear_value(v, text);
    }
    any
}

/// A known blob field is removed, since `null` isn't a value for it. Elsewhere it becomes `null`.
fn clear_object(o: &mut Object, text: &str) -> bool {
    let type_name = o.kind.as_str().to_string();
    let known: Vec<String> = o
        .fields
        .iter()
        .filter(|(name, v)| {
            is_ref(v, text)
                && schema::field_of(&type_name, name).is_some_and(|f| f.kind == schema::Kind::Blob)
        })
        .map(|(name, _)| name.clone())
        .collect();
    for name in &known {
        o.fields.remove(name);
    }
    clear_fields(&mut o.fields, text) || !known.is_empty()
}
