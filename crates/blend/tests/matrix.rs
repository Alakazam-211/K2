//! The merge matrix: every pair of one agent edit (ours, through the writer) and one person
//! edit (theirs: the scene built again at other addresses, as an app saves it) on a small scene,
//! 8 x 11, with the outcome an oracle predicts from the merge's rules (`crate::merge`'s module
//! documentation), not from its code: both land; or the person's lands and the agent's is held,
//! for the predicted object, field and reason. Every merged file reads and indexes, and shows
//! each landed edit as its side made it.

use k2_mpc_base::index::{IndexLimits, INDEX_TEXT_MAX};
use k2_mpc_blend::merge::{merge3, ConflictWhy, Outcome};
use k2_mpc_blend::testgen::{ObjectSpec, SceneBuilder};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{
    index_blend, read_blend, read_scene, write, Change, Edit, ReadLimits, SceneModel,
};

/// One edit; `char` names the object (`A`, `B`, `C`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Loc(char),
    Scale(char),
    Rot(char),
    HideViewport(char),
    HideRender(char),
    Rename(char),
    /// The person deletes A.
    Delete,
    /// The person adds an object D.
    Add,
    /// The person renames B to the name the agent gives A.
    TakeName,
}

const AGENT: &[Op] = &[
    Op::Loc('A'),
    Op::Scale('A'),
    Op::Rot('B'),
    Op::HideViewport('A'),
    Op::HideRender('A'),
    Op::Rename('A'),
    Op::Rename('C'),
    Op::Loc('B'),
];

fn person_ops() -> Vec<Op> {
    let mut v = AGENT.to_vec();
    v.extend([Op::Delete, Op::Add, Op::TakeName]);
    v
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Who {
    Agent,
    Person,
}

impl Op {
    fn object(self) -> Option<char> {
        match self {
            Op::Loc(o) | Op::Scale(o) | Op::Rot(o) | Op::HideViewport(o) | Op::HideRender(o) => {
                Some(o)
            }
            Op::Rename(o) => Some(o),
            Op::Delete => Some('A'),
            Op::Add => None,
            Op::TakeName => Some('B'),
        }
    }
    /// The merge's field label.
    fn field(self) -> &'static str {
        match self {
            Op::Loc(_) => "loc",
            Op::Scale(_) => "scale",
            Op::Rot(_) => "rot",
            Op::HideViewport(_) => "hide_viewport",
            Op::HideRender(_) => "hide_render",
            Op::Rename(_) | Op::TakeName => "name",
            Op::Delete | Op::Add => "",
        }
    }
    /// The value as the index shows it.
    fn value(self, who: Who) -> String {
        let a = who == Who::Agent;
        match self {
            Op::Loc(_) => if a { "1,1,1" } else { "2,2,2" }.into(),
            Op::Scale(_) => if a { "3,3,3" } else { "4,4,4" }.into(),
            Op::Rot(_) => if a { "0.1,0,0" } else { "0.2,0,0" }.into(),
            Op::HideViewport(_) | Op::HideRender(_) => "true".into(),
            Op::Rename(o) => format!("{o}_{}", if a { "agent" } else { "person" }),
            Op::TakeName => "A_agent".into(),
            Op::Delete | Op::Add => String::new(),
        }
    }
}

/// The scene: A (mesh data A), B (mesh data B), C (no data, B's child); all Euler XYZ.
#[derive(Clone)]
struct Obj {
    name: String,
    loc: [f32; 3],
    rot: [f32; 3],
    scale: [f32; 3],
    restrict: i16,
}

#[derive(Clone)]
struct Spec {
    a: Option<Obj>,
    b: Obj,
    c: Obj,
    d: bool,
}

fn obj(name: &str) -> Obj {
    Obj {
        name: name.into(),
        loc: [0.0; 3],
        rot: [0.0; 3],
        scale: [1.0; 3],
        restrict: 0,
    }
}

fn base_spec() -> Spec {
    Spec {
        a: Some(obj("A")),
        b: obj("B"),
        c: obj("C"),
        d: false,
    }
}

fn build(s: &Spec, shift: u64) -> Vec<u8> {
    let mut b = SceneBuilder::modern();
    b.shift = shift;
    let mut names: Vec<&str> = Vec::new();
    if let Some(a) = &s.a {
        names.push(&a.name);
    }
    names.push(&s.b.name);
    names.push(&s.c.name);
    if s.d {
        names.push("D");
    }
    b.scene("Scene", &names, &[], None);
    let put = |b: &mut SceneBuilder, o: &Obj, data: Option<&str>, parent: Option<&str>| {
        b.object(
            &o.name,
            &ObjectSpec {
                data: data.map(str::to_owned),
                parent: parent.map(str::to_owned),
                loc: o.loc,
                rot: o.rot,
                scale: o.scale,
                restrict: o.restrict,
                rotmode: 1,
                ..ObjectSpec::default()
            },
        )
    };
    if let Some(a) = &s.a {
        put(&mut b, a, Some("ME:A"), None);
    }
    put(&mut b, &s.b, Some("ME:B"), None);
    put(&mut b, &s.c, None, Some(&s.b.name));
    if s.d {
        put(&mut b, &obj("D"), None, None);
    }
    b.mesh("A", &[], 3);
    b.mesh("B", &[], 3);
    b.bytes()
}

fn base() -> Vec<u8> {
    build(&base_spec(), 0)
}

fn floats(v: &str) -> [f32; 3] {
    let p: Vec<f32> = v.split(',').map(|x| x.parse().unwrap()).collect();
    [p[0], p[1], p[2]]
}

/// The agent's version: the writer on the base.
fn agent(op: Op) -> Vec<u8> {
    let key = format!("@ob:{}", op.object().unwrap());
    let v = op.value(Who::Agent);
    let change = match op {
        Op::Loc(_) => Change::Location(floats(&v)),
        Op::Scale(_) => Change::Scale(floats(&v)),
        Op::Rot(_) => Change::Rotation(floats(&v)),
        Op::HideViewport(_) => Change::Visibility {
            viewport: Some(true),
            select: None,
            render: None,
        },
        Op::HideRender(_) => Change::Visibility {
            viewport: None,
            select: None,
            render: Some(true),
        },
        Op::Rename(_) => Change::Name(v),
        other => panic!("not an agent edit: {other:?}"),
    };
    write(&base(), &[Edit::new(key, change)], &ReadLimits::default())
        .expect("check: the agent's write")
        .bytes
}

/// The person's save: the scene with `op`, built again at other addresses.
fn person(op: Op) -> Vec<u8> {
    let mut s = base_spec();
    let v = op.value(Who::Person);
    fn target(s: &mut Spec, o: char) -> &mut Obj {
        match o {
            'A' => s.a.as_mut().unwrap(),
            'B' => &mut s.b,
            _ => &mut s.c,
        }
    }
    match op {
        Op::Loc(o) => target(&mut s, o).loc = floats(&v),
        Op::Scale(o) => target(&mut s, o).scale = floats(&v),
        Op::Rot(o) => target(&mut s, o).rot = floats(&v),
        Op::HideViewport(o) => target(&mut s, o).restrict |= 1,
        Op::HideRender(o) => target(&mut s, o).restrict |= 4,
        Op::Rename(o) => target(&mut s, o).name = v,
        Op::Delete => s.a = None,
        Op::Add => s.d = true,
        Op::TakeName => s.b.name = v,
    }
    build(&s, 0x66_0000)
}

/// What the rules say a pair comes to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expect {
    Both,
    Held {
        object: String,
        field: &'static str,
        why: &'static str,
    },
}

/// The object's name in the person's save.
fn person_name(o: char, t: Op) -> String {
    match t {
        Op::Rename(x) if x == o => t.value(Who::Person),
        Op::TakeName if o == 'B' => t.value(Who::Person),
        _ => o.to_string(),
    }
}

/// The oracle, from the rules alone.
fn expect(o: Op, t: Op) -> Expect {
    let obj = o.object().unwrap();
    let same_target = t.object() == Some(obj) && t.field() == o.field();
    if same_target && o.value(Who::Agent) == t.value(Who::Person) {
        return Expect::Both; // the same change on both sides
    }
    if t == Op::Delete && obj == 'A' {
        return Expect::Held {
            object: "@ob:A".into(),
            field: o.field(),
            why: "deleted",
        };
    }
    if same_target {
        return Expect::Held {
            object: format!("@ob:{}", person_name(obj, t)),
            field: o.field(),
            why: "both",
        };
    }
    if o == Op::Rename('A') && t == Op::TakeName {
        return Expect::Held {
            object: "@ob:A".into(),
            field: "name",
            why: "refused",
        };
    }
    Expect::Both
}

fn model(bytes: &[u8]) -> SceneModel {
    let (_, raw) = decompress(bytes, &ReadLimits::default()).unwrap();
    read_scene(&read_blend(&mut &raw[..], &ReadLimits::default()).unwrap()).unwrap()
}

/// The value `field` (the merge's label) of `key` in `m`.
fn value(m: &SceneModel, key: &str, field: &str) -> Option<String> {
    let id = m.by_key(key)?;
    if field == "name" {
        return Some(id.name.clone());
    }
    let n = if field == "scale" { "size" } else { field };
    id.fields
        .iter()
        .chain(&id.named)
        .find(|(f, _)| f == n)
        .map(|(_, v)| v.clone())
}

/// Whether `op`, made by `who`, shows in the merged file, given the other side's op.
fn shows(m: &SceneModel, op: Op, who: Who, other: Op) -> bool {
    match op {
        Op::Delete => m.ids.iter().all(|i| i.code != "OB" || i.name != "A"),
        Op::Add => m.by_key("@ob:D").is_some(),
        Op::TakeName => m.by_key("@ob:A_agent").is_some_and(|i| {
            matches!(&i.detail, k2_mpc_blend::Detail::Object { data: Some(d), .. }
                if m.ids[*d].key.as_deref() == Some("@me:B"))
        }),
        _ => {
            let o = op.object().unwrap();
            // the object's name after both: a rename by its maker, else the other side's
            let name = match (op, who) {
                (Op::Rename(_), _) => op.value(who),
                (_, Who::Agent) => person_name(o, other),
                (_, Who::Person) => match other {
                    Op::Rename(x) if x == o => other.value(Who::Agent),
                    _ => person_name(o, op),
                },
            };
            let key = format!("@ob:{name}");
            match op {
                Op::Rename(_) => m.by_key(&key).is_some(),
                _ => value(m, &key, op.field()).as_deref() == Some(op.value(who).as_str()),
            }
        }
    }
}

#[test]
fn every_pair_of_edits_merges_as_the_rules_say() {
    let b = base();
    let limits = IndexLimits::new(10_000, 64, 64 << 20, INDEX_TEXT_MAX);
    let mut counts = [0usize; 2];
    let mut failures = Vec::new();
    let tops = person_ops();
    for &o in AGENT {
        let ours = agent(o);
        for &t in &tops {
            let theirs = person(t);
            let want = expect(o, t);
            let out = merge3(&b, &ours, &theirs, &ReadLimits::default());
            let got: Result<(), String> = match (&want, &out) {
                (Expect::Both, Outcome::Merged(m)) => {
                    counts[0] += 1;
                    let md = model(&m.bytes);
                    if !m.conflicts.is_empty() {
                        Err(format!("held {:?}", m.conflicts))
                    } else if !shows(&md, o, Who::Agent, t) {
                        Err("the agent's edit is missing".into())
                    } else if !shows(&md, t, Who::Person, o) {
                        Err("the person's edit is missing".into())
                    } else {
                        Ok(())
                    }
                }
                (Expect::Held { object, field, why }, Outcome::Merged(m)) => {
                    counts[1] += 1;
                    let md = model(&m.bytes);
                    let held: Vec<(String, &str, &str)> = m
                        .conflicts
                        .iter()
                        .map(|c| {
                            let w = match &c.why {
                                ConflictWhy::BothChanged => "both",
                                ConflictWhy::PersonDeleted => "deleted",
                                ConflictWhy::Refused(_) => "refused",
                                _ => "other",
                            };
                            (c.object.clone(), c.field, w)
                        })
                        .collect();
                    if held != [(object.clone(), *field, *why)] {
                        Err(format!("held {held:?}, not [({object}, {field}, {why})]"))
                    } else if !shows(&md, t, Who::Person, Op::Add) {
                        Err("the person's edit isn't in the file".into())
                    } else if m.bytes != theirs {
                        Err("something of the agent's landed".into())
                    } else {
                        Ok(())
                    }
                }
                (w, other) => Err(format!("{other:?}, expected {w:?}")),
            };
            if let Outcome::Merged(m) = &out {
                if let Err(e) = index_blend(&mut &m.bytes[..], &limits) {
                    failures.push(format!("{o:?} x {t:?}: the result doesn't index: {e}"));
                }
            }
            if let Err(e) = got {
                failures.push(format!("{o:?} x {t:?}: {e}"));
            }
        }
    }
    println!(
        "merge matrix: {} pairs: {} both land, {} held",
        AGENT.len() * tops.len(),
        counts[0],
        counts[1]
    );
    assert!(
        failures.is_empty(),
        "{} pairs failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(counts.iter().all(|c| *c > 0));
    assert_eq!(counts[0] + counts[1], 88);
}
