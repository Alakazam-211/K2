//! The board: ids, objects, people, and the reads that are computed, never stored.

use crate::bad_ids;
use crate::hash::{hex, unhex, Sha256};
use crate::json::{self, Fields, Value};
use crate::limits::Limits;
use crate::refusal::{Refusal, RefusalCode};
use crate::schema::{self, ID_MAX_BYTES};
use crate::validate;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;

/// An id: 1 to 24 bytes of `[0-9a-z._-]`. Opaque: never parse one for meaning.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Id(String);

impl Id {
    /// Checks the grammar.
    pub fn new(s: &str) -> Result<Id, Refusal> {
        let ok = (1..=ID_MAX_BYTES).contains(&s.len())
            && s.bytes()
                .all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'z' | b'.' | b'_' | b'-'));
        if ok {
            Ok(Id(s.to_string()))
        } else {
            Err(Refusal::new(
                RefusalCode::IdInvalid,
                format!("{s:?} is not 1 to {ID_MAX_BYTES} bytes of [0-9a-z._-]"),
            ))
        }
    }

    /// The id as text.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

/// Makes a person token from eight random bytes the host supplies. Random, so two copies of a
/// board never mint one token for two different people. Readers must not parse the form.
pub fn mint_person_token(random: [u8; 8]) -> Id {
    Id(format!("p-{}", hex(&random)))
}

/// A board's lineage: 128 bits the host supplies, which tell copies of one board apart.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Lineage([u8; 16]);

impl Lineage {
    /// From the host's random bytes.
    pub fn from_bytes(bytes: [u8; 16]) -> Lineage {
        Lineage(bytes)
    }

    /// 32 lower-case hex digits.
    pub fn to_hex(&self) -> String {
        hex(&self.0)
    }

    /// Reads 32 lower-case hex digits.
    pub fn from_hex(s: &str) -> Option<Lineage> {
        let mut out = [0u8; 16];
        unhex(s, &mut out)?;
        Some(Lineage(out))
    }
}

impl fmt::Debug for Lineage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

/// An object's type. A type this version doesn't know is kept as it came.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ObjectType {
    /// A sticky note.
    Sticky,
    /// A text block.
    Text,
    /// A shape.
    Shape,
    /// A connector.
    Connector,
    /// A pen stroke.
    Pen,
    /// A frame.
    Frame,
    /// A card.
    Card,
    /// A table.
    Table,
    /// A mind-map node.
    Mind,
    /// A comment.
    Comment,
    /// An image.
    Image,
    /// A type from a newer writer: kept, and a renderer shows a placeholder.
    Unknown(String),
}

impl ObjectType {
    /// Reads a type name, refusing one outside the grammar.
    pub fn parse(s: &str) -> Result<ObjectType, Refusal> {
        Ok(match s {
            "sticky" => ObjectType::Sticky,
            "text" => ObjectType::Text,
            "shape" => ObjectType::Shape,
            "connector" => ObjectType::Connector,
            "pen" => ObjectType::Pen,
            "frame" => ObjectType::Frame,
            "card" => ObjectType::Card,
            "table" => ObjectType::Table,
            "mind" => ObjectType::Mind,
            "comment" => ObjectType::Comment,
            "image" => ObjectType::Image,
            _ => {
                if !validate::is_name(s) {
                    return Err(Refusal::new(
                        RefusalCode::TypeInvalid,
                        format!("{s:?} is not a type name"),
                    ));
                }
                ObjectType::Unknown(s.to_string())
            }
        })
    }

    /// The type's name.
    pub fn as_str(&self) -> &str {
        match self {
            ObjectType::Sticky => "sticky",
            ObjectType::Text => "text",
            ObjectType::Shape => "shape",
            ObjectType::Connector => "connector",
            ObjectType::Pen => "pen",
            ObjectType::Frame => "frame",
            ObjectType::Card => "card",
            ObjectType::Table => "table",
            ObjectType::Mind => "mind",
            ObjectType::Comment => "comment",
            ObjectType::Image => "image",
            ObjectType::Unknown(s) => s,
        }
    }
}

/// One object on a board. `fields` holds known and unknown fields together; a field that's absent
/// stays absent, and [`schema::field_of`] gives its default.
#[derive(Clone, Debug, PartialEq)]
pub struct Object {
    /// Unique in the file.
    pub id: Id,
    /// Its type.
    pub kind: ObjectType,
    /// Everything else.
    pub fields: Fields,
}

impl Object {
    /// The object as JSON, with its id and type, and without its position.
    ///
    /// The tree keeps a number as it is in memory, so a -0.0 stays -0.0 here, and
    /// [`crate::json::to_canonical`] writes it as `0`, as the file does. A host printing this tree
    /// with its own JSON printer normalises -0.0 to 0, or prints through `to_canonical`.
    pub fn to_value(&self) -> Value {
        let mut f = self.fields.clone();
        f.insert("id".into(), Value::Str(self.id.as_str().to_string()));
        f.insert("type".into(), Value::Str(self.kind.as_str().to_string()));
        Value::Obj(f)
    }

    /// The object's canonical JSON, unknown fields included. Position isn't part of it.
    pub fn canonical_json(&self) -> Result<Vec<u8>, Refusal> {
        json::to_canonical(&self.to_value())
    }

    /// The object's revision: sha256 of its canonical JSON. A save names the revision it read.
    pub fn rev(&self) -> Result<Sha256, Refusal> {
        Ok(Sha256::of(&self.canonical_json()?))
    }
}

/// A person on a board's `people` table. The token is the key of the table.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Person {
    /// A display name, if the file carries one.
    pub name: Option<String>,
    /// `person` or `agent`, if the file says. Another name is kept.
    pub kind: Option<String>,
    /// Fields this version doesn't know.
    pub extra: Fields,
}

impl Person {
    /// The person as JSON.
    pub fn to_value(&self) -> Value {
        let mut f = self.extra.clone();
        if let Some(name) = &self.name {
            f.insert("name".into(), Value::Str(name.clone()));
        }
        if let Some(kind) = &self.kind {
            f.insert("kind".into(), Value::Str(kind.clone()));
        }
        Value::Obj(f)
    }
}

/// A board: the content of `board.json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    /// Board settings, known and unknown fields together.
    pub settings: Fields,
    /// Per-file person tokens.
    pub people: BTreeMap<Id, Person>,
    /// Back to front: array order is z-order.
    pub objects: Vec<Object>,
    /// Frame ids in presentation order.
    pub frames: Vec<Id>,
    /// Top-level fields this version doesn't know.
    pub extra: Fields,
}

/// A reference's state, computed on read and never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefState {
    /// The target is there and fits.
    Attached,
    /// The target is there but doesn't fit: a frame reference to a non-frame, or a parent chain
    /// that loops, cut at the member with the smallest id.
    Suspect,
    /// The target isn't there. A connector end keeps its hint; nothing moves or goes silently.
    Orphaned,
}

/// One reference between objects.
#[derive(Clone, Debug, PartialEq)]
pub struct Reference {
    /// The object holding the reference.
    pub from: Id,
    /// The field that holds it: `from`, `to`, `target`, `parent` or `frame`.
    pub field: &'static str,
    /// The object it names.
    pub target: Id,
    /// Its state.
    pub state: RefState,
    /// The last resolved point, if a connector end keeps one.
    pub hint: Option<(f64, f64)>,
}

fn shape(at: &str, what: &str) -> Refusal {
    Refusal::at(RefusalCode::BoardShape, at, what.to_string())
}

fn with_at(mut r: Refusal, at: String) -> Refusal {
    if r.at.is_none() {
        r.at = Some(at);
    }
    r
}

impl Board {
    /// A board with nothing on it.
    pub fn empty() -> Board {
        Board::default()
    }

    /// Reads and validates `board.json`, in any JSON spelling.
    pub fn from_json(bytes: &[u8], limits: &Limits) -> Result<Board, Refusal> {
        limits.check_ceiling()?;
        let value = json::parse(bytes, limits)?;
        let board = Board::from_value(value, limits).map_err(|r| {
            // from_value takes the parsed value apart; parsing again costs only on this path.
            match json::parse(bytes, limits) {
                Ok(again) => bad_ids::name_them(r, &again),
                Err(_) => r,
            }
        })?;
        board.validate(limits)?;
        Ok(board)
    }

    fn from_value(value: Value, limits: &Limits) -> Result<Board, Refusal> {
        let Value::Obj(mut top) = value else {
            return Err(shape("/", "board.json must be an object"));
        };
        let Some(Value::Obj(settings)) = top.remove("settings") else {
            return Err(shape("/settings", "must be an object"));
        };
        let Some(Value::Obj(people_in)) = top.remove("people") else {
            return Err(shape("/people", "must be an object"));
        };
        let Some(Value::Arr(objects_in)) = top.remove("objects") else {
            return Err(shape("/objects", "must be an array"));
        };
        let Some(Value::Arr(frames_in)) = top.remove("frames") else {
            return Err(shape("/frames", "must be an array"));
        };
        check_counts(objects_in.len(), people_in.len(), limits)?;

        let mut people = BTreeMap::new();
        for (token, v) in people_in {
            let at = format!("/people/{token}");
            let id = Id::new(&token).map_err(|r| with_at(r, at.clone()))?;
            let Value::Obj(mut f) = v else {
                return Err(shape(&at, "a person must be an object"));
            };
            let name = match f.remove("name") {
                None => None,
                Some(Value::Str(s)) => Some(s),
                Some(_) => {
                    return Err(Refusal::at(
                        RefusalCode::FieldKind,
                        format!("{at}/name"),
                        "must be text",
                    ))
                }
            };
            let kind = match f.remove("kind") {
                None => None,
                Some(Value::Str(s)) => Some(s),
                Some(_) => {
                    return Err(Refusal::at(
                        RefusalCode::FieldKind,
                        format!("{at}/kind"),
                        "must be text",
                    ))
                }
            };
            people.insert(
                id,
                Person {
                    name,
                    kind,
                    extra: f,
                },
            );
        }

        let mut objects = Vec::with_capacity(objects_in.len());
        for (i, v) in objects_in.into_iter().enumerate() {
            let at = format!("/objects/{i}");
            let Value::Obj(mut f) = v else {
                return Err(shape(&at, "an object must be a JSON object"));
            };
            let Some(Value::Str(id)) = f.remove("id") else {
                return Err(Refusal::at(
                    RefusalCode::IdInvalid,
                    format!("{at}/id"),
                    "every object needs an id",
                ));
            };
            let id = Id::new(&id).map_err(|r| with_at(r, format!("{at}/id")))?;
            let Some(Value::Str(kind)) = f.remove("type") else {
                return Err(Refusal::at(
                    RefusalCode::TypeInvalid,
                    format!("{at}/type"),
                    "every object needs a type",
                ));
            };
            let kind = ObjectType::parse(&kind).map_err(|r| with_at(r, format!("{at}/type")))?;
            objects.push(Object {
                id,
                kind,
                fields: f,
            });
        }

        let mut frames = Vec::with_capacity(frames_in.len());
        for (i, v) in frames_in.into_iter().enumerate() {
            let at = format!("/frames/{i}");
            let Value::Str(s) = v else {
                return Err(shape(&at, "a frame entry must be an id"));
            };
            frames.push(Id::new(&s).map_err(|r| with_at(r, at))?);
        }

        Ok(Board {
            settings,
            people,
            objects,
            frames,
            extra: top,
        })
    }

    /// The board as a JSON value.
    pub fn to_value(&self) -> Value {
        let mut top = self.extra.clone();
        top.insert("settings".into(), Value::Obj(self.settings.clone()));
        let people: Fields = self
            .people
            .iter()
            .map(|(id, p)| (id.as_str().to_string(), p.to_value()))
            .collect();
        top.insert("people".into(), Value::Obj(people));
        let objects = self.objects.iter().map(Object::to_value).collect();
        top.insert("objects".into(), Value::Arr(objects));
        let frames = self
            .frames
            .iter()
            .map(|id| Value::Str(id.as_str().to_string()))
            .collect();
        top.insert("frames".into(), Value::Arr(frames));
        Value::Obj(top)
    }

    /// The settings' revision: sha256 of their canonical JSON, as for an object.
    pub fn settings_rev(&self) -> Result<Sha256, Refusal> {
        Ok(Sha256::of(&json::to_canonical(&Value::Obj(
            self.settings.clone(),
        ))?))
    }

    /// The canonical bytes of `board.json`: what a tab hands to its renderer, and what a writer
    /// stores.
    pub fn to_canonical_json(&self) -> Result<Vec<u8>, Refusal> {
        json::to_canonical(&self.to_value())
    }

    /// Checks the whole board, after an edit and before a write.
    pub fn validate(&self, limits: &Limits) -> Result<(), Refusal> {
        limits.check_ceiling()?;
        check_counts(self.objects.len(), self.people.len(), limits)?;
        validate::validate_board(self).map_err(|r| {
            if r.code == RefusalCode::IdInvalid {
                bad_ids::name_them(r, &self.to_value())
            } else {
                r
            }
        })
    }

    /// Every reference between objects, with its state.
    pub fn references(&self) -> Vec<Reference> {
        let by_id: HashMap<&Id, &Object> = self.objects.iter().map(|o| (&o.id, o)).collect();
        let parents = |field: &str| -> HashMap<&Id, Id> {
            self.objects
                .iter()
                .filter_map(|o| match o.fields.get(field) {
                    Some(Value::Str(s)) => Id::new(s).ok().map(|t| (&o.id, t)),
                    _ => None,
                })
                .collect()
        };
        let loops_cut_at = |links: &HashMap<&Id, Id>| -> BTreeSet<Id> {
            // A member of a loop walks back to itself. Each loop is cut at its smallest id.
            let mut cut = BTreeSet::new();
            for start in links.keys() {
                let mut seen = vec![(*start).clone()];
                let mut at = links.get(*start);
                while let Some(next) = at {
                    if next == *start {
                        if let Some(smallest) = seen.iter().min() {
                            cut.insert(smallest.clone());
                        }
                        break;
                    }
                    if seen.contains(next) {
                        break;
                    }
                    seen.push(next.clone());
                    at = links.get(next);
                }
            }
            cut
        };
        let mind_links = parents("parent");
        let frame_links = parents("frame");
        let mind_cut = loops_cut_at(&mind_links);
        let frame_cut = loops_cut_at(&frame_links);

        let mut out = Vec::new();
        for o in &self.objects {
            for field in ["from", "to"] {
                if o.kind == ObjectType::Connector {
                    if let Some(Value::Obj(end)) = o.fields.get(field) {
                        if let Some(Value::Str(t)) = end.get("id") {
                            if let Ok(target) = Id::new(t) {
                                let state = if by_id.contains_key(&target) {
                                    RefState::Attached
                                } else {
                                    RefState::Orphaned
                                };
                                let hint = match end.get("hint") {
                                    Some(Value::Obj(h)) => match (h.get("x"), h.get("y")) {
                                        (Some(Value::Num(x)), Some(Value::Num(y))) => {
                                            Some((*x, *y))
                                        }
                                        _ => None,
                                    },
                                    _ => None,
                                };
                                out.push(Reference {
                                    from: o.id.clone(),
                                    field,
                                    target,
                                    state,
                                    hint,
                                });
                            }
                        }
                    }
                }
            }
            if o.kind == ObjectType::Comment {
                if let Some(Value::Str(t)) = o.fields.get("target") {
                    if let Ok(target) = Id::new(t) {
                        let state = if by_id.contains_key(&target) {
                            RefState::Attached
                        } else {
                            RefState::Orphaned
                        };
                        out.push(Reference {
                            from: o.id.clone(),
                            field: "target",
                            target,
                            state,
                            hint: None,
                        });
                    }
                }
            }
            if o.kind == ObjectType::Mind {
                if let Some(target) = mind_links.get(&o.id) {
                    let state = match by_id.get(target) {
                        None => RefState::Orphaned,
                        Some(_) if mind_cut.contains(&o.id) => RefState::Suspect,
                        Some(_) => RefState::Attached,
                    };
                    out.push(Reference {
                        from: o.id.clone(),
                        field: "parent",
                        target: target.clone(),
                        state,
                        hint: None,
                    });
                }
            }
            if let Some(target) = frame_links.get(&o.id) {
                let state = match by_id.get(target) {
                    None => RefState::Orphaned,
                    Some(t) if t.kind != ObjectType::Frame => RefState::Suspect,
                    Some(_) if frame_cut.contains(&o.id) => RefState::Suspect,
                    Some(_) => RefState::Attached,
                };
                out.push(Reference {
                    from: o.id.clone(),
                    field: "frame",
                    target: target.clone(),
                    state,
                    hint: None,
                });
            }
        }
        out
    }

    /// Every blob this board names, unknown fields included: any JSON object whose only member is
    /// `blob`, holding `sha256-<hex>`.
    pub fn blob_refs(&self) -> BTreeSet<Sha256> {
        fn walk(v: &Value, out: &mut BTreeSet<Sha256>) {
            match v {
                Value::Obj(f) => {
                    if f.len() == 1 {
                        if let Some(Value::Str(s)) = f.get("blob") {
                            if let Ok(h) = Sha256::parse(s) {
                                out.insert(h);
                            }
                        }
                    }
                    f.values().for_each(|x| walk(x, out));
                }
                Value::Arr(a) => a.iter().for_each(|x| walk(x, out)),
                _ => {}
            }
        }
        let mut out = BTreeSet::new();
        walk(&self.to_value(), &mut out);
        out
    }

    /// Removes every display name from the people table. Tokens and kinds stay, so per-person entries still
    /// mean one person each. Free text (a card's `assignee`, a comment) is content, and stays.
    pub fn strip_names(&mut self) {
        for p in self.people.values_mut() {
            p.name = None;
        }
    }

    /// The default of a known field of this object type, as the schema gives it.
    pub fn default_of(type_name: &str, field: &str) -> Option<schema::Default> {
        schema::field_of(type_name, field).and_then(|f| f.default)
    }
}

fn check_counts(objects: usize, people: usize, limits: &Limits) -> Result<(), Refusal> {
    if objects as u64 > limits.objects {
        return Err(Refusal::new(
            RefusalCode::LimitObjects,
            format!("{objects} objects, above {}", limits.objects),
        ));
    }
    if people as u64 > limits.people {
        return Err(Refusal::new(
            RefusalCode::LimitPeople,
            format!("{people} people, above {}", limits.people),
        ));
    }
    Ok(())
}
