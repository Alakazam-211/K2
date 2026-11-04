//! What the fuzz targets check. Every decoder takes any bytes: it gives a typed refusal or a value
//! that holds up. A violated property stops the run here, and that's a finding.
//!
//! A find is fixed with a hand-written regression test. Fuzzer inputs stay out of this repo.

use k2_mpc_board::json::{self, Fields, Value};
use k2_mpc_board::verbs::{self, AlignTo, Axis, Ctx, End, Layer, Side, Verb};
use k2_mpc_board::*;
use std::io::Cursor;

/// A smaller ceiling than the format's, so a run spends its time on variety.
fn limits() -> Limits {
    let mut l = Limits::V1;
    l.board_json_bytes = 1 << 20;
    l.part_bytes = 1 << 20;
    l.total_bytes = 1 << 22;
    l.values = 1 << 16;
    l.entries = 64;
    l.objects = 1 << 10;
    l
}

/// Parsing: what's accepted prints, and printing is a fixed point.
pub fn json_value(data: &[u8]) {
    let l = limits();
    let Ok(v) = json::parse(data, &l) else { return };
    let printed = json::to_canonical(&v).expect("an accepted value prints");
    let again = json::parse(&printed, &l).expect("canonical bytes read back");
    assert_eq!(
        json::to_canonical(&again).expect("it prints"),
        printed,
        "canonical isn't a fixed point"
    );
}

/// `board.json`: what's accepted is valid, prints, reads back the same, and its computed reads work.
pub fn board_json(data: &[u8]) {
    let l = limits();
    let Ok(board) = Board::from_json(data, &l) else {
        return;
    };
    board.validate(&l).expect("an accepted board is valid");
    let printed = board.to_canonical_json().expect("it prints");
    let again = Board::from_json(&printed, &l).expect("canonical bytes read back");
    assert_eq!(again, board, "the board changed through canonical bytes");
    let _ = board.references();
    let _ = board.blob_refs();
    verbs::list(&board, None).expect("a list");
}

/// A package: the guard agrees however it's read, and a write of what opened reads back the same,
/// with the same guard when the file was canonical.
pub fn package_open(data: &[u8]) {
    let l = limits();
    let Ok(mut pkg) = Package::open(Cursor::new(data), &l) else {
        return;
    };
    let guard = package_guard_of(Cursor::new(data), &l).expect("what opened has a guard");
    assert_eq!(guard, pkg.package_guard(), "two ways to the guard differ");
    if pkg.verify_parts().is_err() || !pkg.missing_blobs().is_empty() {
        return;
    }
    let board = pkg.board().clone();
    let meta = pkg.manifest().write_meta();
    let mut out = Vec::new();
    let Ok(written) = write(&mut out, &meta, &board, &mut pkg, &l) else {
        return; // something in the file is a limit the writer holds too, typed
    };
    let again = Package::open(Cursor::new(&out[..]), &l).expect("what's written opens");
    assert_eq!(again.board(), &board, "the board changed through a write");
    assert_eq!(
        again.package_guard(),
        written.package_guard,
        "the written guard differs"
    );
}

/// The header read: each call answers on any bytes, and wherever an open accepts the file, what it
/// gives is what the open gives. The messages carry no input: a finding shows a location and a kind.
pub fn header(data: &[u8]) {
    let l = limits();
    let header = read_header(Cursor::new(data), &l);
    let preview = read_preview(Cursor::new(data), &l, l.part_bytes);
    let summary = read_summary(Cursor::new(data), &l);
    let Ok(mut pkg) = Package::open(Cursor::new(data), &l) else {
        return;
    };
    let guard = pkg.package_guard();
    assert!(
        matches!(&header, Ok(h) if h.guard == guard && !h.checked),
        "the header differs from the open"
    );
    let title = match pkg.board().settings.get("title") {
        Some(Value::Str(t)) => Some(t.clone()),
        _ => None,
    };
    let count = pkg.board().objects.len() as u64;
    assert!(
        matches!(&summary, Ok(s) if s.guard == guard && s.objects == count && s.title == title),
        "the summary differs from the open"
    );
    let shown = pkg
        .manifest()
        .parts
        .iter()
        .find(|p| p.role == "preview")
        .map(|p| p.name.clone());
    let same = match (shown, preview) {
        (None, Ok(None)) => true,
        (Some(name), got) => match (pkg.read_part(&name), got) {
            (Ok(bytes), Ok(Some(p))) => p.bytes == bytes,
            (Err(r), Err(e)) => r.code == e.code,
            _ => false,
        },
        (None, _) => false,
    };
    assert!(same, "the preview differs from the open");
}

fn fixture() -> Board {
    let text = br#"{"frames":["f-1","f-2"],"later":{"keep":1},"objects":[
{"id":"f-1","title":"One","type":"frame","w":500,"h":500},
{"id":"f-2","locked":true,"type":"frame"},
{"id":"s-1","group":"g-1","reactions":{"y":["p-1","p-2"]},"text":"hi","type":"sticky","votes":{"p-2":1},"x":10,"y":10},
{"group":"g-1","id":"s-2","later":[1,{"a":null}],"type":"sticky","x":40,"y":10},
{"from":{"id":"s-1"},"id":"c-1","to":{"id":"s-2"},"type":"connector"},
{"from":{"id":"s-2"},"id":"c-2","locked":true,"to":{"x":1,"y":2},"type":"connector"},
{"checks":[{"done":false,"id":"c1","text":"t"}],"id":"k-1","title":"T","type":"card"},
{"columns":[{"id":"a"},{"id":"b"}],"id":"b-1","rows":[{"cells":{"a":"x"},"id":"r"}],"type":"table"},
{"id":"m-1","type":"mind"},{"id":"m-2","parent":"m-1","type":"mind"},
{"id":"q-1","replies":[{"at":"2026-11-03T10:00:00Z","by":"p-1","id":"r","text":"t"}],"target":"s-1","type":"comment"},
{"id":"p-9","points":[[0,0],[5,7]],"type":"pen"},
{"id":"i-1","src":{"blob":"sha256-9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08"},"type":"image"}
],"people":{"p-1":{"kind":"agent","name":"Ann"},"p-2":{"name":"Bo"}},"settings":{"title":"T","voting":{"active":true,"perPerson":3}}}"#;
    Board::from_json(text, &Limits::V1).expect("the fixture")
}

/// The board without `people` and top-level unknowns: what `inverse` restores.
fn content(b: &Board) -> Vec<u8> {
    let Value::Obj(mut v) = b.to_value() else {
        panic!("a board is an object");
    };
    v.remove("people");
    json::to_canonical(&Value::Obj(v)).expect("it prints")
}

/// One group on one board: all or nothing; a result that holds up; an inverse that takes it back.
fn check_group(before: &Board, group: &Group) {
    let l = Limits::V1;
    let mut after = before.clone();
    match apply_group(&mut after, group) {
        Err(c) => {
            assert_eq!(
                &after, before,
                "a group that didn't apply changed the board"
            );
            no_impossible_pair(&c);
            if let Conflict::Refused(r) = &c {
                assert!(
                    GROUP_REFUSALS.contains(&r.code),
                    "a group was refused with {}, which GROUP_REFUSALS doesn't list",
                    r.code
                );
            }
        }
        Ok(report) => {
            after
                .validate(&l)
                .expect("an applied group leaves a valid board");
            let printed = after.to_canonical_json().expect("it prints");
            let again = Board::from_json(&printed, &l).expect("it reads back");
            assert_eq!(again, after, "the result changed through canonical bytes");
            if report.is_noop() {
                assert_eq!(
                    content(&after),
                    content(before),
                    "a no-op report with a change"
                );
            }
            let undo = verbs::inverse(before, &after, "undo").expect("an inverse");
            let mut undone = after.clone();
            apply_group(&mut undone, &undo).expect("the inverse applies");
            let (Value::Obj(a), Value::Obj(b)) = (undone.to_value(), before.to_value()) else {
                panic!("boards are objects");
            };
            if a.get("later") == b.get("later") {
                assert_eq!(
                    content(&undone),
                    content(before),
                    "the inverse missed something"
                );
            }
        }
    }
}

/// The two conflicts no op gives: a change to a person (no op edits one) and an add of the settings
/// (no op adds them). A host's bridge has no error row for either, so one here is a finding.
fn no_impossible_pair(c: &Conflict) {
    match c {
        Conflict::Changed {
            target: Target::Person(_),
            ..
        } => panic!("a group gave Changed on a person: {c:?}"),
        Conflict::Exists {
            target: Target::Settings,
            ..
        } => panic!("a group gave Exists on the settings: {c:?}"),
        _ => {}
    }
}

/// Raw ops: strict reading, then all or nothing on a board with every kind of thing on it.
pub fn raw_ops(data: &[u8]) {
    let Ok(group) = verbs::group_from_json(data, &limits()) else {
        return;
    };
    check_group(&fixture(), &group);
}

struct Bytes<'a>(&'a [u8]);

impl Bytes<'_> {
    fn next(&mut self) -> u8 {
        match self.0.split_first() {
            Some((b, rest)) => {
                self.0 = rest;
                *b
            }
            None => 0,
        }
    }

    fn id(&mut self, b: &Board) -> Id {
        let n = usize::from(self.next());
        match b.objects.get(n % (b.objects.len() + 1)) {
            Some(o) => o.id.clone(),
            None => Id::new("gone").expect("an id"),
        }
    }

    fn num(&mut self) -> f64 {
        f64::from(self.next() as i8) * 2.5
    }

    fn revved(&mut self, b: &Board) -> verbs::ObjRev {
        let id = self.id(b);
        let rev = match b.objects.iter().find(|o| o.id == id) {
            // Now and then a stale one.
            Some(o) if self.next() % 8 != 0 => o.rev().expect("a rev"),
            _ => Sha256::of(b"stale"),
        };
        verbs::ObjRev { id, rev }
    }

    fn several(&mut self, b: &Board) -> Vec<verbs::ObjRev> {
        let n = self.next() % 5;
        (0..n).map(|_| self.revved(b)).collect()
    }
}

/// A verb chosen by the bytes, on a board with every kind of thing on it.
fn pick(bytes: &mut Bytes<'_>, b: &Board) -> Verb {
    let t = bytes.revved(b);
    let id = t.id.clone();
    let rev = t.rev;
    match bytes.next() % 33 {
        0 => Verb::Add {
            kind: ObjectType::Sticky,
            fields: Fields::new(),
            at: Position::Front,
        },
        1 => Verb::Add {
            kind: ObjectType::Table,
            fields: Fields::new(),
            at: Position::Back,
        },
        2 => {
            let mut set = Fields::new();
            set.insert("x".into(), Value::Num(bytes.num()));
            Verb::Update {
                id,
                rev,
                set,
                unset: vec!["text".to_string()],
            }
        }
        3 => Verb::Delete {
            id,
            rev,
            cascade: true,
        },
        4 => Verb::Delete {
            id,
            rev,
            cascade: false,
        },
        5 => Verb::Connect {
            from: End::Object { id, anchor: None },
            to: End::Point {
                x: bytes.num(),
                y: bytes.num(),
            },
            fields: Fields::new(),
        },
        6 => Verb::React {
            id,
            emoji: "y".into(),
            on: [None, Some(true), Some(false)][usize::from(bytes.next() % 3)],
        },
        7 => Verb::Vote {
            id,
            delta: i32::from(bytes.next() as i8) % 4,
        },
        8 => Verb::Comment {
            target: Some(id),
            text: "t".into(),
            point: None,
        },
        9 => Verb::Reply {
            id,
            text: "t".into(),
        },
        10 => Verb::Resolve {
            id,
            rev,
            resolved: true,
        },
        11 => Verb::Lock {
            ids: bytes.several(b),
            locked: bytes.next() % 2 == 0,
        },
        12 => Verb::CheckAdd {
            id,
            text: "t".into(),
        },
        13 => Verb::CheckSet {
            id,
            rev,
            item: "c1".into(),
            text: None,
            done: Some(true),
        },
        14 => Verb::CheckRemove {
            id,
            rev,
            item: "c1".into(),
        },
        15 => Verb::TableCell {
            id,
            rev,
            row: "r".into(),
            column: "b".into(),
            text: "t".into(),
        },
        16 => Verb::TableAddRow {
            id,
            after: Some("r".into()),
        },
        17 => Verb::TableRemoveRow {
            id,
            rev,
            row: "r".into(),
        },
        18 => Verb::TableAddColumn {
            id,
            after: None,
            width: None,
        },
        19 => Verb::TableRemoveColumn {
            id,
            rev,
            column: "a".into(),
        },
        20 => Verb::Move {
            ids: bytes.several(b),
            dx: bytes.num(),
            dy: bytes.num(),
        },
        21 => {
            let to = match bytes.next() % 5 {
                0 => Layer::Front,
                1 => Layer::Back,
                2 => Layer::Forward,
                3 => Layer::Backward,
                _ => Layer::After(vec![bytes.id(b)]),
            };
            Verb::Order {
                ids: vec![id, bytes.id(b)],
                to,
            }
        }
        22 => Verb::Group {
            ids: bytes.several(b),
        },
        23 => Verb::Ungroup {
            ids: bytes.several(b),
        },
        24 => Verb::Align {
            ids: bytes.several(b),
            to: match bytes.next() % 6 {
                0 => AlignTo::Left,
                1 => AlignTo::Center,
                2 => AlignTo::Right,
                3 => AlignTo::Top,
                4 => AlignTo::Middle,
                _ => AlignTo::Bottom,
            },
        },
        25 => Verb::Distribute {
            ids: bytes.several(b),
            axis: if bytes.next() % 2 == 0 {
                Axis::Horizontal
            } else {
                Axis::Vertical
            },
        },
        26 => Verb::Tidy {
            ids: bytes.several(b),
        },
        27 => Verb::CopyStyle {
            from: id,
            to: bytes.several(b),
        },
        28 => Verb::Duplicate {
            ids: vec![id, bytes.id(b)],
            dx: bytes.num(),
            dy: bytes.num(),
        },
        29 => Verb::ConnectedCopy {
            id,
            side: match bytes.next() % 4 {
                0 => Side::Top,
                1 => Side::Right,
                2 => Side::Bottom,
                _ => Side::Left,
            },
        },
        30 => Verb::MindChild {
            parent: id,
            text: "t".into(),
        },
        31 => {
            let name = verbs::TEMPLATES[usize::from(bytes.next()) % verbs::TEMPLATES.len()].0;
            Verb::Template {
                name: name.to_string(),
                x: bytes.num(),
                y: bytes.num(),
            }
        }
        _ => {
            let object = b.objects[usize::from(bytes.next()) % b.objects.len()].clone();
            Verb::Paste {
                objects: vec![object],
                x: bytes.num(),
                y: bytes.num(),
            }
        }
    }
}

/// Up to eight verbs in a row, each on what the last one left. A verb gives a typed answer or a
/// group, and every group holds up.
pub fn verbs(data: &[u8]) {
    let mut bytes = Bytes(data);
    let mut board = fixture();
    for _ in 0..8 {
        if bytes.0.is_empty() {
            break;
        }
        let verb = pick(&mut bytes, &board);
        let mut seed = u64::from(bytes.next()) << 8;
        let mut random = move || {
            seed += 1;
            seed.to_le_bytes()
        };
        let mut cx = Ctx {
            group_id: "g".to_string(),
            by: Some(verbs::me(&board, Some("Ann"), Some("agent"), [1; 8])),
            at: Some("2026-11-03T12:00:00Z".to_string()),
            random: &mut random,
        };
        let Ok(planned) = verbs::plan(&board, &mut cx, &verb) else {
            continue;
        };
        check_group(&board, &planned.group);
        let _ = apply_group(&mut board, &planned.group);
    }
}
