//! The corpus: boards full of what's easy to lose. Each goes through a file, field by field and verb
//! by verb. Nothing it holds may change unless an edit named it, and whatever a verb does has to read
//! back as it was written, and has to undo.

#[path = "support/verbs.rs"]
mod support;

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::verbs::{self, Layer, Side, Verb};
use k2_mpc_board::*;
use std::io::Cursor;
use std::path::PathBuf;
use support::*;

fn corpus() -> Vec<(String, Board, Vec<u8>)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../vectors");
    let Value::Obj(index) = json::parse(
        &std::fs::read(root.join("index.json")).expect("the index"),
        &Limits::V1,
    )
    .expect("JSON") else {
        panic!("an object");
    };
    let Value::Arr(cases) = &index["cases"] else {
        panic!("a list");
    };
    let mut out = Vec::new();
    for case in cases {
        let Value::Obj(c) = case else {
            panic!("an object");
        };
        if c["layer"] != text("fidelity") {
            continue;
        }
        let (Value::Str(name), Value::Str(input), Value::Str(canonical)) =
            (&c["name"], &c["input"], &c["canonical"])
        else {
            panic!("a case: {c:?}");
        };
        let bytes = std::fs::read(root.join(input)).expect("the input");
        let board = Board::from_json(&bytes, &Limits::V1).expect("the corpus reads");
        let want = std::fs::read(root.join(canonical)).expect("the canonical form");
        out.push((name.clone(), board, want));
    }
    assert!(out.len() >= 15, "only {} corpus cases", out.len());
    out
}

fn canon(b: &Board) -> Vec<u8> {
    b.to_canonical_json().expect("canonical")
}

/// A file holding a board, a preview and a part this version doesn't know. The corpus names a blob
/// by the hash of bytes it doesn't carry, so those references are cleared here: a write refuses a
/// dangling one, and the unit tests hold real blobs.
fn file_of(b: &Board) -> Vec<u8> {
    let mut parts = MemParts::new();
    parts.set_preview(b"preview".to_vec());
    parts.add(
        "extras/later.bin",
        "later",
        "application/x-later",
        b"keep".to_vec(),
        Fields::new(),
    );
    let mut held = b.clone();
    for h in b.blob_refs() {
        apply_group(
            &mut held,
            &Group {
                id: "clear".into(),
                by: None,
                ops: vec![Op::ClearBlob { hash: h }],
            },
        )
        .expect("clears");
    }
    let mut out = Vec::new();
    write(
        &mut out,
        &WriteMeta::new(Lineage::from_bytes([3; 16])),
        &held,
        &mut parts,
        &Limits::V1,
    )
    .expect("it writes");
    out
}

#[test]
fn every_case_reads_to_its_canonical_form_and_stays_there() {
    for (name, board, want) in corpus() {
        assert_eq!(canon(&board), want, "{name}: its canonical form");
        let again = Board::from_json(&want, &Limits::V1).expect("canonical reads");
        assert_eq!(again, board, "{name}: read from canonical");
        assert_eq!(canon(&again), want, "{name}: canonical twice");
    }
}

#[test]
fn a_file_keeps_everything_through_open_and_write() {
    for (name, board, _) in corpus() {
        let first = file_of(&board);
        let mut pkg = Package::open(Cursor::new(&first[..]), &Limits::V1).expect("it opens");
        let guard = pkg.package_guard();
        let held = pkg.board().clone();
        let meta = pkg.manifest().write_meta();
        let mut second = Vec::new();
        let written = write(&mut second, &meta, &held, &mut pkg, &Limits::V1).expect("it writes");
        assert_eq!(written.package_guard, guard, "{name}: the guard moved");
        assert_eq!(second, first, "{name}: the bytes moved");
        let mut again = Package::open(Cursor::new(&second[..]), &Limits::V1).expect("reopens");
        assert_eq!(again.board(), &held, "{name}: the board");
        assert_eq!(
            again.read_part("preview.png").expect("it's there"),
            b"preview".to_vec(),
            "{name}"
        );
        assert_eq!(
            again.read_part("extras/later.bin").expect("it's there"),
            b"keep".to_vec(),
            "{name}"
        );
        let part = again
            .manifest()
            .parts
            .iter()
            .find(|p| p.name == "extras/later.bin")
            .expect("listed");
        assert_eq!(part.role, "later", "{name}");
        assert_eq!(part.media_type, "application/x-later", "{name}");
        assert_eq!(
            again.manifest().lineage,
            Lineage::from_bytes([3; 16]),
            "{name}"
        );

        let f = BoardFile::open(
            FileTarget::new("plan.k5board").expect("a path"),
            Cursor::new(&second[..]),
            &Limits::V1,
        )
        .expect("it opens");
        let mut out = Vec::new();
        assert_eq!(
            f.save(Cursor::new(&second[..]), &mut out)
                .expect("saves")
                .value,
            Saved::Nothing,
            "{name}: an unchanged save"
        );
        assert!(out.is_empty(), "{name}");
    }
}

fn value_with(board: &Board, object: &str, name: &str, value: Option<&Value>) -> Vec<u8> {
    // What the board should be, built from its JSON with no help from the op code.
    let Value::Obj(mut top) = board.to_value() else {
        panic!("an object");
    };
    let Some(Value::Arr(objects)) = top.get_mut("objects") else {
        panic!("a list");
    };
    for o in objects {
        let Value::Obj(f) = o else {
            panic!("an object");
        };
        if f["id"] == text(object) {
            match value {
                Some(v) => {
                    f.insert(name.to_string(), v.clone());
                }
                None => {
                    f.remove(name);
                }
            }
        }
    }
    json::to_canonical(&Value::Obj(top)).expect("canonical")
}

/// A different value of the same kind, if there's an obvious one.
fn other_value(v: &Value) -> Option<Value> {
    match v {
        Value::Num(n) => Some(Value::Num(if *n == 41.5 { 42.5 } else { 41.5 })),
        Value::Bool(b) => Some(Value::Bool(!b)),
        Value::Str(s) if s != "changed" => Some(text("changed")),
        _ => None,
    }
}

/// An edit of one field changes that field's bytes and no others: not its neighbours, not the other
/// objects, not unknown things and not the order.
#[test]
fn an_edit_of_one_field_changes_only_that_field() {
    let mut applied = 0;
    let mut refused = 0;
    for (name, board, _) in corpus() {
        for o in &board.objects {
            for (field, value) in &o.fields {
                let r = o.rev().expect("a rev");
                let held = o.fields.get("locked") == Some(&Value::Bool(true))
                    && !["locked", "votes", "reactions"].contains(&field.as_str());
                let same = Group {
                    id: "g".into(),
                    by: None,
                    ops: vec![Op::Set {
                        id: o.id.clone(),
                        rev: r,
                        set: fields(&[(field, value.clone())]),
                        unset: Vec::new(),
                    }],
                };
                let mut next = board.clone();
                if let Ok(report) = apply_group(&mut next, &same) {
                    assert!(
                        report.is_noop(),
                        "{name}: {}.{field} to itself: {report:?}",
                        o.id
                    );
                    assert_eq!(
                        canon(&next),
                        canon(&board),
                        "{name}: {}.{field} to itself",
                        o.id
                    );
                }

                let unset = Group {
                    id: "g".into(),
                    by: None,
                    ops: vec![Op::Set {
                        id: o.id.clone(),
                        rev: r,
                        set: Fields::new(),
                        unset: vec![field.clone()],
                    }],
                };
                let mut next = board.clone();
                match apply_group(&mut next, &unset) {
                    Ok(_) => {
                        applied += 1;
                        assert_eq!(
                            canon(&next),
                            value_with(&board, o.id.as_str(), field, None),
                            "{name}: unset {}.{field}",
                            o.id
                        );
                    }
                    Err(Conflict::Refused(r)) => {
                        refused += 1;
                        assert_eq!(next, board, "{name}: refused, so nothing changes");
                        if held {
                            assert_eq!(r.code, RefusalCode::ObjectLocked, "{name}: {}", o.id);
                        }
                    }
                    Err(other) => panic!("{name}: unset {}.{field}: {other:?}", o.id),
                }
                assert!(
                    !held || next == board,
                    "{name}: a lock let {}.{field} go",
                    o.id
                );

                let Some(new) = other_value(value) else {
                    continue;
                };
                let set = Group {
                    id: "g".into(),
                    by: None,
                    ops: vec![Op::Set {
                        id: o.id.clone(),
                        rev: r,
                        set: fields(&[(field, new.clone())]),
                        unset: Vec::new(),
                    }],
                };
                let mut next = board.clone();
                match apply_group(&mut next, &set) {
                    Ok(_) => {
                        applied += 1;
                        assert_eq!(
                            canon(&next),
                            value_with(&board, o.id.as_str(), field, Some(&new)),
                            "{name}: set {}.{field}",
                            o.id
                        );
                    }
                    Err(Conflict::Refused(_)) => {
                        refused += 1;
                        assert_eq!(next, board, "{name}: refused, so nothing changes");
                    }
                    Err(other) => panic!("{name}: set {}.{field}: {other:?}", o.id),
                }
            }
        }
    }
    assert!(applied > 300, "only {applied} edits applied");
    assert!(
        refused > 0,
        "no edit was refused, so the locks went unchecked"
    );
}

/// Verbs to try on one object. Many won't fit: that's a typed answer.
fn verbs_for(b: &Board, o: &Object) -> Vec<Verb> {
    let t = verbs::ObjRev {
        id: o.id.clone(),
        rev: o.rev().expect("a rev"),
    };
    let i = &o.id;
    let mut v = vec![
        Verb::Update {
            id: i.clone(),
            rev: t.rev,
            set: fields(&[("rot", Value::Num(7.5))]),
            unset: Vec::new(),
        },
        Verb::Delete {
            id: i.clone(),
            rev: t.rev,
            cascade: true,
        },
        Verb::Delete {
            id: i.clone(),
            rev: t.rev,
            cascade: false,
        },
        Verb::Move {
            ids: vec![t.clone()],
            dx: 3.5,
            dy: -2.0,
        },
        Verb::Lock {
            ids: vec![t.clone()],
            locked: true,
        },
        Verb::Lock {
            ids: vec![t.clone()],
            locked: false,
        },
        Verb::Duplicate {
            ids: vec![i.clone()],
            dx: 24.0,
            dy: 24.0,
        },
        Verb::Paste {
            objects: vec![o.clone()],
            x: 5.0,
            y: 6.0,
        },
        Verb::React {
            id: i.clone(),
            emoji: "\u{2705}".into(),
            on: Some(true),
        },
        Verb::Vote {
            id: i.clone(),
            delta: 1,
        },
        Verb::Comment {
            target: Some(i.clone()),
            text: "look".into(),
            point: None,
        },
        Verb::Reply {
            id: i.clone(),
            text: "yes".into(),
        },
        Verb::Resolve {
            id: i.clone(),
            rev: t.rev,
            resolved: true,
        },
        Verb::CheckAdd {
            id: i.clone(),
            text: "new".into(),
        },
        Verb::TableAddRow {
            id: i.clone(),
            after: None,
        },
        Verb::TableAddColumn {
            id: i.clone(),
            after: None,
            width: Some(10.0),
        },
        Verb::ConnectedCopy {
            id: i.clone(),
            side: Side::Right,
        },
        Verb::MindChild {
            parent: i.clone(),
            text: "new".into(),
        },
        Verb::Connect {
            from: verbs::End::Object {
                id: i.clone(),
                anchor: None,
            },
            to: verbs::End::Point { x: 1.0, y: 2.0 },
            fields: Fields::new(),
        },
    ];
    for to in [Layer::Front, Layer::Back, Layer::Forward, Layer::Backward] {
        v.push(Verb::Order {
            ids: vec![i.clone()],
            to,
        });
    }
    if let Some(h) = b.blob_refs().into_iter().next() {
        v.push(Verb::ClearBlob { hash: h });
    }
    v
}

fn board_verbs(b: &Board) -> Vec<Verb> {
    let mut v: Vec<Verb> = verbs::TEMPLATES
        .iter()
        .map(|(name, _)| Verb::Template {
            name: (*name).to_string(),
            x: 100.0,
            y: 100.0,
        })
        .collect();
    v.push(Verb::Meta {
        rev: b.settings_rev().expect("a rev"),
        set: fields(&[("title", text("new"))]),
        unset: Vec::new(),
    });
    let boxed: Vec<verbs::ObjRev> = b
        .objects
        .iter()
        .filter(|o| o.kind != ObjectType::Connector)
        .take(5)
        .map(|o| named(b, o.id.as_str()))
        .collect();
    v.push(Verb::Group { ids: boxed.clone() });
    v.push(Verb::Ungroup { ids: boxed.clone() });
    for to in [verbs::AlignTo::Left, verbs::AlignTo::Middle] {
        v.push(Verb::Align {
            ids: boxed.clone(),
            to,
        });
    }
    v.push(Verb::Distribute {
        ids: boxed.clone(),
        axis: verbs::Axis::Horizontal,
    });
    v.push(Verb::Tidy { ids: boxed.clone() });
    if let Some(first) = boxed.first() {
        v.push(Verb::CopyStyle {
            from: first.id.clone(),
            to: boxed.clone(),
        });
    }
    v
}

/// Whatever a verb does is valid, reads back as written with what it didn't touch intact, and
/// undoes. Whatever it won't do is a typed answer, and the board is unchanged.
#[test]
fn every_verb_on_every_case_reads_back_and_undoes() {
    let mut done = 0;
    let mut declined = 0;
    for (name, plain, _) in corpus() {
        // Who acts has to be in the file.
        let mut board = plain.clone();
        let person = board
            .people
            .get(&id(ME))
            .cloned()
            .unwrap_or_else(|| Person {
                name: Some("Ann".into()),
                ..Person::default()
            });
        let add_me = Group {
            id: "me".into(),
            by: None,
            ops: vec![Op::AddPerson {
                token: id(ME),
                person,
            }],
        };
        apply_group(&mut board, &add_me).expect("a person");
        let mut all = board_verbs(&board);
        for o in &board.objects {
            all.extend(verbs_for(&board, o));
        }
        for verb in &all {
            let label = format!("{name}: {} {verb:?}", verb.name());
            let planned = match plan_as(&board, Who::Me, 4000, verb) {
                Ok(p) => p,
                Err(_) => {
                    declined += 1;
                    continue;
                }
            };
            let mut next = board.clone();
            match apply_group(&mut next, &planned.group) {
                Ok(_) => {}
                // A lock is the one thing a verb leaves to the op, and a move can go past the
                // largest number. Anything else it planned has to fit.
                Err(Conflict::Refused(r))
                    if matches!(
                        r.code,
                        RefusalCode::ObjectLocked | RefusalCode::JsonNumberRange
                    ) =>
                {
                    assert_eq!(next, board, "{label}: refused, so nothing changes");
                    declined += 1;
                    continue;
                }
                Err(other) => panic!("{label}: a plan that doesn't fit: {other:?}"),
            }
            done += 1;
            next.validate(&Limits::V1)
                .unwrap_or_else(|r| panic!("{label}: {r}"));
            let bytes = next.to_canonical_json().expect("canonical");
            let back = Board::from_json(&bytes, &Limits::V1)
                .unwrap_or_else(|r| panic!("{label}: reading it back: {r}"));
            assert_eq!(back, next, "{label}: reading it back");
            assert_eq!(back.extra, board.extra, "{label}: top-level unknowns");
            let undo = verbs::inverse(&board, &next, "undo")
                .unwrap_or_else(|r| panic!("{label}: an inverse: {r}"));
            let mut undone = next.clone();
            apply_group(&mut undone, &undo).unwrap_or_else(|e| panic!("{label}: undo: {e:?}"));
            if !matches!(verb, Verb::ClearBlob { .. }) {
                assert_eq!(
                    json::to_canonical(&content(&undone)).expect("canonical"),
                    json::to_canonical(&content(&board)).expect("canonical"),
                    "{label}: undo"
                );
            }
        }
    }
    assert!(done > 500, "only {done} verbs applied");
    assert!(declined > 100, "only {declined} verbs declined");
}

fn big(objects: Vec<String>, name: &str) -> Board {
    let text = format!(
        "{{\"frames\":[],\"objects\":[{}],\"people\":{{\"{ME}\":{{\"name\":\"Ann\"}}}},\"settings\":{{}}}}",
        objects.join(",")
    );
    Board::from_json(text.as_bytes(), &Limits::V1).unwrap_or_else(|r| panic!("{name}: {r}"))
}

/// The largest each field holds comes through a file unchanged: nothing's cut or rounded at the edge.
#[test]
fn the_largest_values_survive_a_file() {
    let emoji = "\u{1F600}".repeat(20_000);
    let cell = "\u{1F600}".repeat(2_000);
    let columns: Vec<String> = (0..30).map(|i| format!("{{\"id\":\"c{i}\"}}")).collect();
    let cells: Vec<String> = (0..30).map(|i| format!("\"c{i}\":\"{cell}\"")).collect();
    let rows: Vec<String> = (0..100)
        .map(|i| format!("{{\"cells\":{{{}}},\"id\":\"r{i}\"}}", cells.join(",")))
        .collect();
    let points: Vec<String> = (0..5000).map(|i| format!("[{i},{i}.5]")).collect();
    let replies: Vec<String> = (0..1000)
        .map(|i| format!("{{\"at\":\"{NOW}\",\"by\":\"{ME}\",\"id\":\"r{i}\",\"text\":\"t{i}\"}}"))
        .collect();
    let tags: Vec<String> = (0..200).map(|i| format!("\"t{i}\"")).collect();
    let checks: Vec<String> = (0..50)
        .map(|i| format!("{{\"done\":false,\"id\":\"c{i}\",\"text\":\"t\"}}"))
        .collect();
    let board = big(
        vec![
            format!("{{\"id\":\"t\",\"text\":\"{emoji}\",\"type\":\"text\"}}"),
            format!(
                "{{\"columns\":[{}],\"id\":\"b\",\"rows\":[{}],\"type\":\"table\"}}",
                columns.join(","),
                rows.join(",")
            ),
            format!(
                "{{\"id\":\"p\",\"points\":[{}],\"type\":\"pen\"}}",
                points.join(",")
            ),
            format!(
                "{{\"id\":\"q\",\"replies\":[{}],\"type\":\"comment\"}}",
                replies.join(",")
            ),
            format!(
                "{{\"id\":\"s\",\"tags\":[{}],\"type\":\"sticky\"}}",
                tags.join(",")
            ),
            format!(
                "{{\"checks\":[{}],\"id\":\"k\",\"type\":\"card\"}}",
                checks.join(",")
            ),
        ],
        "the largest",
    );
    let file = file_of(&board);
    let pkg = Package::open(Cursor::new(&file[..]), &Limits::V1).expect("it opens");
    assert_eq!(pkg.board(), &board, "the board through a file");
    assert_eq!(obj(pkg.board(), "t").fields["text"], text(&emoji));
    // One edit beside them, and the rest stays.
    let mut f = BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(&file[..]),
        &Limits::V1,
    )
    .expect("it opens");
    let g = Group {
        id: "g".into(),
        by: None,
        ops: vec![Op::Set {
            id: id("s"),
            rev: rev(&board, "s"),
            set: fields(&[("x", Value::Num(1.0))]),
            unset: Vec::new(),
        }],
    };
    f.apply(&g).expect("it applies");
    let mut out = Vec::new();
    f.save(Cursor::new(&file[..]), &mut out).expect("saves");
    let again = Package::open(Cursor::new(&out[..]), &Limits::V1).expect("reopens");
    assert_eq!(obj(again.board(), "b"), obj(&board, "b"));
    assert_eq!(obj(again.board(), "t"), obj(&board, "t"));
}

#[test]
fn many_objects_survive_a_file_in_order() {
    let objects: Vec<String> = (0..20_000)
        .rev()
        .map(|i| format!("{{\"id\":\"s-{i}\",\"type\":\"sticky\",\"x\":{i}.5}}"))
        .collect();
    let board = big(objects, "many");
    let file = file_of(&board);
    let pkg = Package::open(Cursor::new(&file[..]), &Limits::V1).expect("it opens");
    assert_eq!(order(pkg.board()), order(&board));
    assert_eq!(pkg.board(), &board);
}
