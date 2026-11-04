//! Test 2: every refusal code and variant maps to exactly one row, as ruled; the two conflicts the
//! board crate can't give are a host bug, never a row. Test 6: each row's exit, as ruled.

mod support;

#[path = "../../crates/board/tests/support/group_cases.rs"]
mod group_cases;

use group_cases::group_cases;
use k2_mpc_board_bridge::board::json::{Fields, Value};
use k2_mpc_board_bridge::board::verbs::{self, VerbCode, VerbError};
use k2_mpc_board_bridge::board::*;
use k2_mpc_board_bridge::kit::errors::lookup;
use k2_mpc_board_bridge::rows::{self, Clash, Phase, TargetKind};
use k2_mpc_board_bridge::{report, HostBug};
use std::collections::BTreeSet;
use support::*;

const FILE: [Phase; 2] = [Phase::Open, Phase::GuardRead];

/// The ruled row of a format refusal (§7.2, and the rulings of 11-04): the file's rows when it
/// was read from the file, the caller's when it was in their input or a write.
fn ruled(code: &str, phase: Phase) -> &'static str {
    let file = FILE.contains(&phase);
    let input_json = if phase == Phase::Ops {
        "MPC-E849"
    } else {
        "MPC-E847"
    };
    match code {
        c if c.starts_with("zip") => "MPC-E842",
        c if c.starts_with("manifest_") || c.starts_with("part_") => "MPC-E843",
        "min_reader_too_high" => "MPC-E844",
        "blob_missing" => "MPC-E841",
        "object_locked" => "MPC-E835",
        "io_error" => "MPC-E857",
        "limit_raised" => "MPC-E848",
        c if c.starts_with("limit_") => {
            if file {
                "MPC-E846"
            } else {
                "MPC-E848"
            }
        }
        c if c.starts_with("json_") || c == "board_shape" => {
            if file {
                "MPC-E845"
            } else {
                input_json
            }
        }
        "id_invalid"
        | "id_duplicate"
        | "type_invalid"
        | "field_name_invalid"
        | "field_kind"
        | "field_value"
        | "person_unknown"
        | "frames_mismatch"
        | "cell_column_unknown" => {
            if file {
                "MPC-E845"
            } else {
                "MPC-E847"
            }
        }
        other => panic!("a refusal code the ruling doesn't name: {other}"),
    }
}

#[test]
fn every_refusal_code_in_every_phase_has_its_ruled_row() {
    let mut seen = BTreeSet::new();
    for code in RefusalCode::ALL {
        for phase in Phase::ALL {
            let row = rows::refusal_row(*code, *phase)
                .unwrap_or_else(|b| panic!("{code} in {phase:?}: no row ({b})"));
            assert_eq!(row.id, ruled(code.as_str(), *phase), "{code} in {phase:?}");
            seen.insert(row.id);
        }
    }
    let want: BTreeSet<&str> = [
        "MPC-E835", "MPC-E841", "MPC-E842", "MPC-E843", "MPC-E844", "MPC-E845", "MPC-E846",
        "MPC-E847", "MPC-E848", "MPC-E849", "MPC-E857",
    ]
    .into();
    assert_eq!(seen, want, "the rows format refusals reach");
}

#[test]
fn a_refusal_report_keeps_the_exact_code_where_its_row_groups_codes() {
    for code in RefusalCode::ALL {
        for phase in Phase::ALL {
            let r = report::refusal_report(&refusal(*code, None), *phase);
            let row = ruled(code.as_str(), *phase);
            match code.as_str() {
                // these rows name their own facts, which a bare refusal lacks
                "blob_missing" | "object_locked" | "min_reader_too_high" => {
                    assert!(r.is_err(), "{code}: a bare refusal lacks the row's facts");
                }
                _ => {
                    let r = r.unwrap_or_else(|b| panic!("{code} in {phase:?}: {b}"));
                    assert_eq!(r.id(), row, "{code} in {phase:?}");
                    if lookup(row)
                        .expect("check: the row")
                        .message
                        .contains("{code}")
                    {
                        assert_eq!(
                            r.detail("code").and_then(|v| v.as_str()),
                            Some(code.as_str()),
                            "{code}: details.code"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn every_verb_code_has_its_row() {
    let want = |c: VerbCode| match c {
        VerbCode::NeedsActor => "MPC-E855",
        VerbCode::VotingOff => "MPC-E836",
        VerbCode::NoVotesLeft => "MPC-E837",
        VerbCode::WrongType => "MPC-E853",
        VerbCode::TooFew => "MPC-E854",
        VerbCode::NoTemplate => "MPC-E839",
        VerbCode::UnknownField => "MPC-E850",
        VerbCode::UnknownType => "MPC-E851",
        VerbCode::NoSuchItem => "MPC-E840",
        VerbCode::NoFreeId => "MPC-E856",
        VerbCode::BadValue => "MPC-E852",
    };
    let mut seen = BTreeSet::new();
    for c in VerbCode::ALL {
        let row = rows::verb_code_row(*c).unwrap_or_else(|b| panic!("{}: {b}", c.as_str()));
        assert_eq!(row.id, want(*c), "{}", c.as_str());
        assert!(seen.insert(row.id), "{}: a row two codes share", c.as_str());
    }
}

fn person() -> Value {
    Value::Obj(Fields::new())
}

#[test]
fn every_conflict_has_its_row_by_target() {
    let changed = |t| Conflict::Changed {
        target: t,
        theirs: person(),
    };
    let exists = |t| Conflict::Exists {
        target: t,
        theirs: person(),
    };
    for (c, want) in [
        (changed(Target::Object(id("s-1"))), "MPC-E830"),
        (changed(Target::Settings), "MPC-E858"),
        (exists(Target::Object(id("s-1"))), "MPC-E831"),
        (exists(Target::Person(id(ME))), "MPC-E859"),
        (Conflict::Deleted { id: id("s-1") }, "MPC-E838"),
        (
            Conflict::Refused(refusal(RefusalCode::FieldValue, None)),
            "MPC-E847",
        ),
    ] {
        let row = rows::conflict_row(&c, Phase::Input).unwrap_or_else(|b| panic!("{c:?}: {b}"));
        assert_eq!(row.id, want, "{c:?}");
        let r = report::conflict_report(&c, Phase::Input).unwrap_or_else(|b| panic!("{c:?}: {b}"));
        assert_eq!(r.id(), want, "{c:?}: the report");
    }
}

/// The two conflicts the board crate can't give have no row: a host bug, from every way in.
#[test]
fn the_impossible_pairs_are_a_host_bug_never_a_row() {
    let pairs = [
        (
            Conflict::Changed {
                target: Target::Person(id(ME)),
                theirs: person(),
            },
            "changed",
            "person",
        ),
        (
            Conflict::Exists {
                target: Target::Settings,
                theirs: person(),
            },
            "exists",
            "settings",
        ),
    ];
    for (c, clash, target) in pairs {
        let bug = HostBug::ImpossibleConflict { clash, target };
        assert_eq!(
            rows::conflict_row(&c, Phase::Input),
            Err(bug.clone()),
            "{c:?}: the row"
        );
        assert_eq!(
            report::conflict_report(&c, Phase::Input),
            Err(bug.clone()),
            "{c:?}: the report"
        );
        let held = Held {
            group: Group {
                id: "g".into(),
                by: None,
                ops: vec![],
            },
            conflict: c.clone(),
        };
        assert_eq!(
            report::held_report(&[held]),
            Err(bug.clone()),
            "{c:?}: held"
        );
        let e = VerbError::Conflict(c.clone());
        assert_eq!(
            report::verb_error_report(&e, "apply", Phase::Input),
            Err(bug),
            "{c:?}: a verb's"
        );
    }
}

#[test]
fn merges_and_saves_have_their_rows() {
    let conflicts = MergeError::Conflicts(Box::default());
    assert_eq!(
        report::merge_report(&conflicts).map(|r| r.id()),
        Ok("MPC-E833"),
        "a merge conflict"
    );
    let refused = MergeError::Refused(refusal(RefusalCode::PartHash, Some("blobs/x")));
    assert_eq!(
        report::merge_report(&refused).map(|r| r.id()),
        Ok("MPC-E843"),
        "a copy's part"
    );
    let refused = MergeError::Refused(refusal(RefusalCode::FieldValue, None));
    assert_eq!(
        report::merge_report(&refused).map(|r| r.id()),
        Ok("MPC-E847"),
        "a merged board that isn't valid"
    );
    let stale = SaveError::Stale {
        read: Sha256::of(b"a"),
        on_disk: Sha256::of(b"b"),
    };
    assert_eq!(
        report::save_report(&stale).map(|r| r.id()),
        Ok("MPC-E832"),
        "stale"
    );
    for (code, want) in [
        (RefusalCode::LimitTotalBytes, "MPC-E848"),
        (RefusalCode::IoError, "MPC-E857"),
        (RefusalCode::JsonNumberRange, "MPC-E847"),
    ] {
        let e = SaveError::Refused(refusal(code, None));
        assert_eq!(
            report::save_report(&e).map(|r| r.id()),
            Ok(want),
            "{code} on write"
        );
    }
    assert_eq!(
        report::no_object_report(&id("nope")).map(|r| r.id()),
        Ok("MPC-E838"),
        "read finds nothing"
    );
    assert_eq!(
        report::io_report("disk full").map(|r| r.id()),
        Ok("MPC-E857"),
        "the host's own I/O"
    );
}

/// Every op by name, matched with no wildcard arm: a new op doesn't compile until it's here.
fn op_name(op: &Op) -> &'static str {
    match op {
        Op::Add { .. } => "add",
        Op::Set { .. } => "set",
        Op::Del { .. } => "del",
        Op::Meta { .. } => "meta",
        Op::Z { .. } => "z",
        Op::FrameOrder { .. } => "frame_order",
        Op::AddPerson { .. } => "add_person",
        Op::ClearBlob { .. } => "clear_blob",
    }
}

/// One op of each kind, made to conflict where it can.
fn conflicting_ops(b: &Board) -> Vec<Op> {
    vec![
        Op::Add {
            object: obj(b, "s-1").clone(),
            at: Position::Front,
        },
        Op::Set {
            id: id("s-1"),
            rev: stale(),
            set: fields(&[("text", text("x"))]),
            unset: vec![],
        },
        Op::Del {
            id: id("s-1"),
            rev: stale(),
        },
        Op::Meta {
            rev: stale(),
            set: Fields::new(),
            unset: vec![],
        },
        Op::Z {
            id: id("gone"),
            to: Position::Front,
        },
        Op::FrameOrder {
            id: id("gone"),
            to: Position::Front,
        },
        Op::AddPerson {
            token: id(ME),
            person: Person {
                name: Some("Someone else".into()),
                kind: None,
                extra: Fields::new(),
            },
        },
        Op::ClearBlob {
            hash: Sha256::of(b"nothing refers to this"),
        },
    ]
}

fn clash_of(c: &Conflict) -> Option<(Clash, TargetKind)> {
    match c {
        Conflict::Changed { target, .. } => Some((Clash::Changed, rows::target_kind(target))),
        Conflict::Exists { target, .. } => Some((Clash::Exists, rows::target_kind(target))),
        Conflict::Deleted { .. } | Conflict::Refused(_) => None,
    }
}

const IMPOSSIBLE: [(Clash, TargetKind); 2] = [
    (Clash::Changed, TargetKind::Person),
    (Clash::Exists, TargetKind::Settings),
];

#[test]
fn the_op_table_names_every_op_and_never_an_impossible_pair() {
    let b = board();
    let ops = conflicting_ops(&b);
    let names: Vec<&str> = ops.iter().map(op_name).collect();
    assert_eq!(
        names,
        [
            "add",
            "set",
            "del",
            "meta",
            "z",
            "frame_order",
            "add_person",
            "clear_blob"
        ],
        "one op of each kind"
    );
    for op in &ops {
        for pair in rows::op_clashes(op) {
            assert!(
                !IMPOSSIBLE.contains(pair),
                "{}: the table names an impossible pair {pair:?}",
                op_name(op)
            );
        }
    }
    let want: [&[(Clash, TargetKind)]; 8] = [
        &[(Clash::Exists, TargetKind::Object)],
        &[(Clash::Changed, TargetKind::Object)],
        &[(Clash::Changed, TargetKind::Object)],
        &[(Clash::Changed, TargetKind::Settings)],
        &[],
        &[],
        &[(Clash::Exists, TargetKind::Person)],
        &[],
    ];
    for (op, want) in ops.iter().zip(want) {
        assert_eq!(rows::op_clashes(op), want, "{}", op_name(op));
    }
}

/// The pack, driven over every op: each conflict it gives is one the table names, and never an
/// impossible pair. A later op that edits a person or adds settings fails here.
#[test]
fn the_pack_gives_only_the_conflicts_the_table_names() {
    let b = board();
    for op in conflicting_ops(&b) {
        let mut next = b.clone();
        let g = Group {
            id: "g".into(),
            by: Some(id(ME)),
            ops: vec![op.clone()],
        };
        if let Err(c) = apply_group(&mut next, &g) {
            if let Some(pair) = clash_of(&c) {
                assert!(
                    !IMPOSSIBLE.contains(&pair),
                    "{}: the pack gave {pair:?}",
                    op_name(&op)
                );
                assert!(
                    rows::op_clashes(&op).contains(&pair),
                    "{}: the pack gave {pair:?}, which the table doesn't name",
                    op_name(&op)
                );
            }
        }
    }
    // and through the friendly verbs: each verb's group, made stale, gives a pair the table names
    let all = [
        verbs::Verb::Meta {
            rev: stale(),
            set: fields(&[("title", text("x"))]),
            unset: vec![],
        },
        verbs::Verb::Update {
            id: id("s-1"),
            rev: stale(),
            set: fields(&[("text", text("x"))]),
            unset: vec![],
        },
    ];
    for v in all {
        let p = plan_with(&b, true, false, &v).expect("check: a stale verb plans");
        let mut next = b.clone();
        let c = apply_group(&mut next, &p.group).expect_err("a stale rev conflicts");
        let pair = clash_of(&c).expect("check: a stale rev is a clash");
        let named = p
            .group
            .ops
            .iter()
            .any(|op| rows::op_clashes(op).contains(&pair));
        assert!(named, "{}: {pair:?} is in the table", v.name());
        assert!(!IMPOSSIBLE.contains(&pair), "{}: {pair:?}", v.name());
    }
}

#[test]
fn every_verb_a_board_row_names_is_a_pack_verb() {
    let pack: BTreeSet<&str> = verbs::WRITE_VERBS
        .iter()
        .chain(verbs::OTHER_VERBS)
        .copied()
        .collect();
    for id in board_rows() {
        let e = lookup(&id).expect("check: the kit holds every board row");
        assert_eq!(e.verbs.first(), Some(&"@file"), "{id}: a file row");
        for v in &e.verbs[1..] {
            assert!(
                *v == "(host)" || pack.contains(v),
                "{id}: {v} isn't a board verb"
            );
        }
    }
}

/// Test 6: each row's exit, as ruled (§7.2; E858 and E859, 11-04).
#[test]
fn every_row_exits_as_ruled() {
    let ruled = [
        ("MPC-E830", 1),
        ("MPC-E831", 1),
        ("MPC-E832", 1),
        ("MPC-E833", 1),
        ("MPC-E834", 1),
        ("MPC-E835", 5),
        ("MPC-E836", 5),
        ("MPC-E837", 5),
        ("MPC-E838", 4),
        ("MPC-E839", 4),
        ("MPC-E840", 4),
        ("MPC-E841", 4),
        ("MPC-E842", 4),
        ("MPC-E843", 4),
        ("MPC-E844", 4),
        ("MPC-E845", 4),
        ("MPC-E846", 4),
        ("MPC-E847", 2),
        ("MPC-E848", 2),
        ("MPC-E849", 2),
        ("MPC-E850", 2),
        ("MPC-E851", 2),
        ("MPC-E852", 2),
        ("MPC-E853", 2),
        ("MPC-E854", 2),
        ("MPC-E855", 2),
        ("MPC-E856", 1),
        ("MPC-E857", 1),
        ("MPC-E858", 1),
        ("MPC-E859", 1),
    ];
    let samples = samples();
    for (row, exit) in ruled {
        let (_, r) = samples
            .iter()
            .find(|(i, _)| *i == row)
            .expect("check: a sample of every row");
        let r = r.as_ref().expect("check: the bridge reports the sample");
        assert_eq!((r.id(), r.exit()), (row, exit), "{row}'s exit");
    }
}

/// A refused group's row is its refusal's, by the call it came from, through conflict_row too: a
/// host asking for the row alone gets the same row the report gives.
#[test]
fn a_refused_groups_row_takes_its_phase() {
    let c = Conflict::Refused(refusal(RefusalCode::JsonSyntax, Some("/ops/0")));
    for (phase, want) in [
        (Phase::Ops, "MPC-E849"),
        (Phase::Input, "MPC-E847"),
        (Phase::Open, "MPC-E845"),
    ] {
        let row = rows::conflict_row(&c, phase).unwrap_or_else(|b| panic!("{phase:?}: {b}"));
        assert_eq!(row.id, want, "{phase:?}");
        let r = report::conflict_report(&c, phase).unwrap_or_else(|b| panic!("{phase:?}: {b}"));
        assert_eq!(r.id(), row.id, "{phase:?}: the row and the report agree");
    }
}

/// A refused group's row is its refusal's row, for every code and every phase.
#[test]
fn a_refused_groups_row_is_its_refusals_for_every_code_and_phase() {
    for code in RefusalCode::ALL {
        for phase in Phase::ALL {
            let c = Conflict::Refused(refusal(*code, None));
            assert_eq!(
                rows::conflict_row(&c, *phase).map(|e| e.id),
                rows::refusal_row(*code, *phase).map(|e| e.id),
                "{code} in {phase:?}"
            );
        }
    }
}

/// The codes a group can be refused with (the board crate's `GROUP_REFUSALS`), each with its ruled
/// row at the two calls a group comes in: the caller's input, and `apply`'s raw ops.
const GROUP_ROWS: [(RefusalCode, &str, &str); 15] = [
    (RefusalCode::ObjectLocked, "MPC-E835", "MPC-E835"),
    (RefusalCode::FramesMismatch, "MPC-E847", "MPC-E847"),
    (RefusalCode::FieldValue, "MPC-E847", "MPC-E847"),
    (RefusalCode::LimitObjects, "MPC-E848", "MPC-E848"),
    (RefusalCode::LimitPeople, "MPC-E848", "MPC-E848"),
    (RefusalCode::JsonNumberRange, "MPC-E847", "MPC-E849"),
    (RefusalCode::LimitDepth, "MPC-E848", "MPC-E848"),
    (RefusalCode::FieldKind, "MPC-E847", "MPC-E847"),
    (RefusalCode::LimitField, "MPC-E848", "MPC-E848"),
    (RefusalCode::FieldNameInvalid, "MPC-E847", "MPC-E847"),
    (RefusalCode::PersonUnknown, "MPC-E847", "MPC-E847"),
    (RefusalCode::IdDuplicate, "MPC-E847", "MPC-E847"),
    (RefusalCode::CellColumnUnknown, "MPC-E847", "MPC-E847"),
    (RefusalCode::TypeInvalid, "MPC-E847", "MPC-E847"),
    (RefusalCode::IdInvalid, "MPC-E847", "MPC-E847"),
];

#[test]
fn each_code_a_group_can_give_has_its_ruled_rows() {
    let listed: Vec<RefusalCode> = GROUP_ROWS.iter().map(|(c, _, _)| *c).collect();
    assert_eq!(
        GROUP_REFUSALS,
        listed.as_slice(),
        "every code the board crate says a group can give, and no other"
    );
    let cases = group_cases();
    let made: Vec<RefusalCode> = cases.iter().map(|c| c.code).collect();
    assert_eq!(made, listed, "one real group for each listed code");
    for (case, (code, input, ops)) in cases.iter().zip(GROUP_ROWS) {
        // the refusal the board crate gives, as it gives it
        let mut b = case.board.clone();
        let c = apply_group(&mut b, &case.group).expect_err("the case's group is refused");
        assert!(
            matches!(&c, Conflict::Refused(r) if r.code == code),
            "{code}: the group is refused with it: {c:?}"
        );
        for (phase, want) in [(Phase::Input, input), (Phase::Ops, ops)] {
            let r = report::conflict_report(&c, phase)
                .unwrap_or_else(|b| panic!("{code} in {phase:?}: {b}"));
            assert_eq!(r.id(), want, "{code} in {phase:?}");
        }
    }
}

/// A locked object's refusal with no where to name the object can't fill its row: a host bug, not a
/// row with a hole. The board crate always says where (`group_cases`' object_locked case).
#[test]
fn a_locked_refusal_with_no_where_is_a_host_bug() {
    let c = Conflict::Refused(refusal(RefusalCode::ObjectLocked, None));
    for phase in [Phase::Input, Phase::Ops] {
        assert_eq!(
            report::conflict_report(&c, phase),
            Err(HostBug::MissingFact {
                row: "MPC-E835",
                key: "id".into(),
            }),
            "{phase:?}"
        );
    }
}

/// A number past 2^53 in a host's Rust op: the group is refused, and its row is the caller's input
/// (MPC-E847) from a verb, raw ops' (MPC-E849) from `apply`.
#[test]
fn a_number_past_two_to_the_53_takes_the_row_of_the_call() {
    let b = board();
    let ops = vec![Op::Add {
        object: Object {
            id: id("s-9"),
            kind: ObjectType::Sticky,
            fields: fields(&[("later", Value::Num(1e300))]),
        },
        at: Position::Front,
    }];
    let c = conflict_of(&b, ops);
    assert!(
        matches!(&c, Conflict::Refused(r) if r.code == RefusalCode::JsonNumberRange),
        "the group is refused for its number: {c:?}"
    );
    for (phase, want) in [(Phase::Input, "MPC-E847"), (Phase::Ops, "MPC-E849")] {
        let r = report::conflict_report(&c, phase).unwrap_or_else(|b| panic!("{phase:?}: {b}"));
        assert_eq!(r.id(), want, "{phase:?}");
        assert_eq!(
            rows::conflict_row(&c, phase).map(|e| e.id),
            Ok(want),
            "{phase:?}: the row alone"
        );
    }
}
