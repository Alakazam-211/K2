//! The merge matrix: every pair of one agent edit (ours, through the verbs) and one person edit
//! (theirs, the same verbs standing in for an app's save) on the sample deck, with the outcome
//! an oracle written from the merge rules predicts, not from the merge's code.

use k2_mpc_pptx::merge::{merge3, Outcome};
use k2_mpc_pptx::slide::{placeholder_text, read_shapes, ShapeInfo};
use k2_mpc_pptx::testgen;
use k2_mpc_pptx::verbs::{apply, Edit, LayoutSel, Place};
use k2_mpc_pptx::xmlx::parse_xml;
use k2_mpc_pptx::Pptx;

use super::limits;

/// One edit, made by either side with that side's own values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Set the text of shape `.1` on slide `.0`.
    Text(u32, u32),
    /// Move shape `.1` on slide `.0`.
    Move(u32, u32),
    /// Delete shape `.1` on slide `.0`.
    Delete(u32, u32),
    /// Add a text box on a slide.
    TextBox(u32),
    /// Add a slide (and title it).
    AddSlide,
    /// Set a slide's speaker notes.
    Notes(u32),
    /// Slide 259 first.
    MoveFirst,
    /// Slide 256 after 258.
    MoveLater,
}

pub const OPS: &[Op] = &[
    Op::Text(258, 4),
    Op::Text(257, 3),
    Op::Move(258, 4),
    Op::Move(258, 5),
    Op::Delete(258, 8),
    Op::Delete(258, 7),
    Op::TextBox(257),
    Op::TextBox(258),
    Op::AddSlide,
    Op::Notes(257),
    Op::Notes(256),
    Op::MoveFirst,
    Op::MoveLater,
];

/// Which side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Agent,
    Person,
}

impl Who {
    fn word(self) -> &'static str {
        match self {
            Who::Agent => "agent",
            Who::Person => "person",
        }
    }
    fn x(self) -> i64 {
        match self {
            Who::Agent => 111,
            Who::Person => 222,
        }
    }
}

/// The base: the sample deck.
pub fn base() -> Vec<u8> {
    testgen::deck(&testgen::sample())
}

/// The base with `op` made by `who`.
pub fn make(op: Op, who: Who) -> Vec<u8> {
    let l = limits();
    let b = base();
    let w = who.word();
    let run = |bytes: &[u8], e: Edit| apply(bytes, &e, &l).unwrap_or_else(|x| panic!("{e:?}: {x}"));
    match op {
        Op::Text(s, id) => {
            run(
                &b,
                Edit::SetText {
                    slide: s,
                    shape: id,
                    text: format!("{w} text"),
                },
            )
            .bytes
        }
        Op::Move(s, id) => {
            run(
                &b,
                Edit::MoveShape {
                    slide: s,
                    shape: id,
                    x: Some(who.x()),
                    y: None,
                    w: None,
                    h: None,
                },
            )
            .bytes
        }
        Op::Delete(s, id) => {
            run(
                &b,
                Edit::DeleteShape {
                    slide: s,
                    shape: id,
                },
            )
            .bytes
        }
        Op::TextBox(s) => {
            run(
                &b,
                Edit::AddTextBox {
                    slide: s,
                    at: (who.x(), 0, 1000, 1000),
                    text: format!("{w} box"),
                },
            )
            .bytes
        }
        Op::AddSlide => {
            let a = run(
                &b,
                Edit::AddSlide {
                    layout: LayoutSel::Name("Title and Content".into()),
                    place: None,
                },
            );
            let addr = a.address.expect("an address");
            let id: u32 = addr.trim_start_matches("@sld:").parse().unwrap();
            run(
                &a.bytes,
                Edit::SetText {
                    slide: id,
                    shape: 2,
                    text: format!("{w} slide"),
                },
            )
            .bytes
        }
        Op::Notes(s) => {
            run(
                &b,
                Edit::SetNotes {
                    slide: s,
                    text: format!("{w} notes"),
                },
            )
            .bytes
        }
        Op::MoveFirst => {
            run(
                &b,
                Edit::MoveSlide {
                    slide: 259,
                    place: Place::First,
                },
            )
            .bytes
        }
        Op::MoveLater => {
            run(
                &b,
                Edit::MoveSlide {
                    slide: 256,
                    place: Place::After(258),
                },
            )
            .bytes
        }
    }
}

/// What the oracle expects of a pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Expect {
    /// Both edits land.
    Both,
    /// The person's edit lands; the agent's is held at this address.
    Held(String),
    /// The kit keeps both files.
    Whole,
}

/// The unit an edit touches: a shape is the unit; a slide's order is one unit; a new notes
/// slide is one per slide.
fn unit(op: Op) -> Option<String> {
    match op {
        Op::Text(s, id) | Op::Move(s, id) | Op::Delete(s, id) => Some(format!("@sld:{s}/sp:{id}")),
        Op::Notes(257) => Some("@notes:257/sp:3".into()),
        Op::Notes(s) => Some(format!("new notes of {s}")),
        Op::MoveFirst | Op::MoveLater => Some("order".into()),
        Op::TextBox(_) | Op::AddSlide => None,
    }
}

/// The oracle, from the merge rules: different units both land; one unit both sides changed
/// the same way lands once; differently, the person's lands and the agent's is held; except
/// that both reordering differently and both making notes for one slide are whole-file
/// conflicts.
pub fn expect(o: Op, t: Op) -> Expect {
    match (unit(o), unit(t)) {
        (Some(a), Some(b)) if a == b => {
            if o == t && matches!(o, Op::Delete(..) | Op::MoveFirst | Op::MoveLater) {
                Expect::Both
            } else if a == "order" || a.starts_with("new notes") {
                Expect::Whole
            } else {
                Expect::Held(a)
            }
        }
        _ => Expect::Both,
    }
}

fn find(v: &[ShapeInfo], id: u32) -> Option<ShapeInfo> {
    v.iter().find_map(|s| {
        if s.id == Some(id) {
            Some(s.clone())
        } else {
            find(&s.children, id)
        }
    })
}

/// Whether `bytes` shows `op` as `who` made it.
pub fn shows(bytes: &[u8], op: Op, who: Who) -> bool {
    let l = limits();
    let p = Pptx::open(bytes, &l).expect("opens");
    let shapes_of = |s: u32| -> Vec<ShapeInfo> {
        let r = p.deck.slide(s).expect("slide");
        read_shapes(&parse_xml(p.text(&r.part).unwrap(), &l).unwrap())
    };
    let ids: Vec<u32> = p.deck.slides.iter().map(|s| s.id).collect();
    let w = who.word();
    match op {
        Op::Text(s, id) => {
            find(&shapes_of(s), id).and_then(|x| x.text) == Some(format!("{w} text"))
        }
        Op::Move(s, id) => {
            find(&shapes_of(s), id).and_then(|x| x.xfrm).map(|x| x.x) == Some(who.x())
        }
        Op::Delete(s, id) => find(&shapes_of(s), id).is_none(),
        Op::TextBox(s) => shapes_of(s)
            .iter()
            .any(|x| x.text.as_deref() == Some(&format!("{w} box"))),
        Op::AddSlide => ids.iter().any(|s| {
            placeholder_text(&shapes_of(*s), &["title"]).as_deref() == Some(&format!("{w} slide"))
        }),
        Op::Notes(s) => {
            let Some(n) = p.deck.slide(s).and_then(|r| r.notes.clone()) else {
                return false;
            };
            let doc = parse_xml(p.text(&n).unwrap(), &l).unwrap();
            placeholder_text(&read_shapes(&doc), &["body"]) == Some(format!("{w} notes"))
        }
        Op::MoveFirst => ids.first() == Some(&259),
        Op::MoveLater => {
            let at = |x| ids.iter().position(|i| *i == x);
            at(256) > at(258)
        }
    }
}

/// One pair's result.
pub fn run(o: Op, t: Op) -> (Vec<u8>, Vec<u8>, Outcome) {
    let (ob, tb) = (make(o, Who::Agent), make(t, Who::Person));
    let out = merge3(&base(), &ob, &tb, &limits());
    (ob, tb, out)
}
