//! A file on bytes: open, apply, guarded save, and the reading verbs.

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::*;
use std::io::Cursor;

fn id(s: &str) -> Id {
    Id::new(s).expect("an id")
}

fn text(s: &str) -> Value {
    Value::Str(s.to_string())
}

fn board() -> Board {
    let json = "{\"frames\":[],\"objects\":[\
{\"id\":\"s-1\",\"tags\":[\"Alpha\"],\"text\":\"hello\",\"type\":\"sticky\"},\
{\"id\":\"s-2\",\"later\":{\"keep\":1},\"text\":\"World\",\"type\":\"sticky\"},\
{\"from\":{\"id\":\"s-1\"},\"id\":\"c-1\",\"label\":\"Ties\",\"to\":{\"id\":\"s-2\"},\"type\":\"connector\"},\
{\"assignee\":\"Bo\",\"id\":\"k-1\",\"status\":\"Ready\",\"title\":\"Plan\",\"type\":\"card\"},\
{\"columns\":[{\"id\":\"a\"}],\"id\":\"b-1\",\"rows\":[{\"cells\":{\"a\":\"Cell words\"},\"id\":\"r\"}],\"type\":\"table\"},\
{\"id\":\"q-1\",\"replies\":[{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"p-1\",\"id\":\"x\",\"text\":\"Reply words\"}],\"target\":\"gone\",\"type\":\"comment\"}\
],\"people\":{\"p-1\":{\"name\":\"Ann\"}},\"settings\":{\"title\":\"T\"}}";
    Board::from_json(json.as_bytes(), &Limits::V1).expect("the board")
}

fn file_with(b: &Board, parts: &mut MemParts) -> Vec<u8> {
    let mut out = Vec::new();
    let meta = WriteMeta::new(Lineage::from_bytes([7; 16]));
    write(&mut out, &meta, b, parts, &Limits::V1).expect("a file");
    out
}

fn file() -> Vec<u8> {
    file_with(&board(), &mut MemParts::new())
}

fn open(bytes: &[u8]) -> BoardFile<Cursor<&[u8]>> {
    BoardFile::open(
        FileTarget::new("plan.k5board").expect("a path"),
        Cursor::new(bytes),
        &Limits::V1,
    )
    .expect("it opens")
}

fn rev(f: &BoardFile<Cursor<&[u8]>>, i: &str) -> Sha256 {
    f.board()
        .objects
        .iter()
        .find(|o| o.id.as_str() == i)
        .expect("that object")
        .rev()
        .expect("a rev")
}

fn set_text(f: &BoardFile<Cursor<&[u8]>>, i: &str, value: &str) -> Group {
    let set: Fields = [("text".to_string(), text(value))].into_iter().collect();
    Group {
        id: format!("g-{i}"),
        by: None,
        ops: vec![Op::Set {
            id: id(i),
            rev: rev(f, i),
            set,
            unset: Vec::new(),
        }],
    }
}

fn text_of(b: &Board, i: &str) -> Value {
    b.objects
        .iter()
        .find(|o| o.id.as_str() == i)
        .expect("that object")
        .fields["text"]
        .clone()
}

#[test]
fn a_save_writes_the_edit_and_passes_lineage_through() {
    let bytes = file();
    let mut f = open(&bytes);
    f.apply(&set_text(&f, "s-1", "changed"))
        .expect("it applies");
    assert!(f.is_changed());
    let mut out = Vec::new();
    let saved = f
        .save(Cursor::new(&bytes[..]), &mut out)
        .expect("it saves")
        .value;
    let Saved::Wrote { written, revs } = saved else {
        panic!("wanted a write, got {saved:?}");
    };
    let again = Package::open(Cursor::new(&out[..]), &Limits::V1).expect("it reads back");
    assert_eq!(text_of(again.board(), "s-1"), text("changed"));
    assert_eq!(again.package_guard(), written.package_guard);
    assert_eq!(again.manifest().lineage, Lineage::from_bytes([7; 16]));
    assert_eq!(written.bytes, out.len() as u64);
    let s1 = again.board().objects[0].rev().expect("a rev");
    assert_eq!(
        revs.get(&id("s-1")),
        Some(&s1),
        "the changed object's new rev"
    );
    assert_eq!(revs.len(), 1, "only what changed: {revs:?}");
}

#[test]
fn revs_list_an_added_object_and_leave_out_a_deleted_one() {
    let bytes = file();
    let mut f = open(&bytes);
    let del = Op::Del {
        id: id("s-2"),
        rev: rev(&f, "s-2"),
    };
    let add = Op::Add {
        object: Object {
            id: id("s-9"),
            kind: ObjectType::Sticky,
            fields: Fields::new(),
        },
        at: Position::Front,
    };
    let g = Group {
        id: "g".into(),
        by: None,
        ops: vec![del, add],
    };
    f.apply(&g).expect("it applies");
    let mut out = Vec::new();
    let Saved::Wrote { revs, .. } = f
        .save(Cursor::new(&bytes[..]), &mut out)
        .expect("saves")
        .value
    else {
        panic!("wanted a write");
    };
    let keys: Vec<&str> = revs.keys().map(Id::as_str).collect();
    assert_eq!(keys, vec!["s-9"]);
}

#[test]
fn an_unchanged_board_writes_nothing() {
    let bytes = file();
    let f = open(&bytes);
    assert!(!f.is_changed());
    let mut out = Vec::new();
    let saved = f
        .save(Cursor::new(&bytes[..]), &mut out)
        .expect("it saves")
        .value;
    assert_eq!(saved, Saved::Nothing);
    assert!(out.is_empty(), "wrote {} bytes", out.len());
}

#[test]
fn an_edit_and_its_reverse_write_nothing() {
    let bytes = file();
    let mut f = open(&bytes);
    let g = set_text(&f, "s-1", "changed");
    f.apply(&g).expect("first");
    let back = set_text(&f, "s-1", "hello");
    f.apply(&back).expect("back");
    assert!(!f.is_changed());
    let mut out = Vec::new();
    assert_eq!(
        f.save(Cursor::new(&bytes[..]), &mut out)
            .expect("saves")
            .value,
        Saved::Nothing
    );
    assert!(out.is_empty());
}

#[test]
fn a_stale_guard_refuses_and_writes_nothing() {
    let bytes = file();
    let mut f = open(&bytes);
    let read = f.guard();
    f.apply(&set_text(&f, "s-1", "mine")).expect("it applies");

    let mut theirs = board();
    theirs.settings.insert("title".into(), text("Theirs"));
    let on_disk = file_with(&theirs, &mut MemParts::new());
    assert!(
        !f.still_current(Cursor::new(&on_disk[..]))
            .expect("reads")
            .value
    );
    assert!(
        f.still_current(Cursor::new(&bytes[..]))
            .expect("reads")
            .value
    );

    let mut out = Vec::new();
    let err = f
        .save(Cursor::new(&on_disk[..]), &mut out)
        .expect_err("stale")
        .value;
    let now = package_guard_of(Cursor::new(&on_disk[..]), &Limits::V1).expect("a guard");
    assert_eq!(err, SaveError::Stale { read, on_disk: now });
    assert!(out.is_empty(), "wrote {} bytes", out.len());
}

#[test]
fn a_file_on_disk_that_will_not_read_is_a_refusal() {
    let bytes = file();
    let f = open(&bytes);
    let mut out = Vec::new();
    let err = f
        .save(Cursor::new(&b"not a zip"[..]), &mut out)
        .expect_err("refused")
        .value;
    assert!(matches!(err, SaveError::Refused(_)), "{err:?}");
    assert!(out.is_empty());
}

#[test]
fn a_stale_revision_changes_nothing() {
    let bytes = file();
    let mut f = open(&bytes);
    let first = set_text(&f, "s-1", "first");
    let stale = set_text(&f, "s-1", "second");
    f.apply(&first).expect("first");
    let err = f.apply(&stale).expect_err("stale").value;
    assert!(matches!(err, Conflict::Changed { .. }), "{err:?}");
    assert_eq!(text_of(f.board(), "s-1"), text("first"));
}

#[test]
fn a_held_group_keeps_its_ops_and_the_rest_go_on() {
    let bytes = file();
    let mut f = open(&bytes);
    let good1 = set_text(&f, "s-1", "one");
    let stale = Group {
        id: "stale".into(),
        by: None,
        ops: vec![Op::Del {
            id: id("s-2"),
            rev: Sha256::of(b"something else"),
        }],
    };
    let good2 = set_text(&f, "k-1", "two");
    let done = f.apply_all(&[good1, stale.clone(), good2]).value;
    assert_eq!(done.applied.len(), 2, "{done:?}");
    assert_eq!(done.applied[0].0, "g-s-1");
    assert_eq!(done.applied[1].0, "g-k-1");
    assert_eq!(done.held.len(), 1);
    assert_eq!(done.held[0].group, stale, "the edit is kept");
    assert!(
        matches!(done.held[0].conflict, Conflict::Changed { .. }),
        "{:?}",
        done.held[0].conflict
    );
    assert_eq!(text_of(f.board(), "s-1"), text("one"));
    assert_eq!(text_of(f.board(), "k-1"), text("two"));
    assert_eq!(text_of(f.board(), "s-2"), text("World"));
}

fn blob_field(h: &Sha256) -> Value {
    let mut f = Fields::new();
    f.insert("blob".into(), Value::Str(h.to_string()));
    Value::Obj(f)
}

fn add_image(hash: &Sha256) -> Group {
    let mut fields = Fields::new();
    fields.insert("src".into(), blob_field(hash));
    Group {
        id: "pic".into(),
        by: None,
        ops: vec![Op::Add {
            object: Object {
                id: id("i-1"),
                kind: ObjectType::Image,
                fields,
            },
            at: Position::Front,
        }],
    }
}

#[test]
fn a_staged_blob_is_written_when_the_board_refers_to_it() {
    let bytes = file();
    let mut f = open(&bytes);
    let h = f.add_blob("image/png", b"picture".to_vec()).value;
    let seen = f.read_blob(&h).map(|e| e.value).map_err(|e| e.value);
    assert_eq!(
        seen,
        Ok(Blob {
            media_type: "image/png".into(),
            bytes: b"picture".to_vec(),
        }),
        "a staged blob reads before the save"
    );
    f.apply(&add_image(&h)).expect("it applies");
    let mut out = Vec::new();
    f.save(Cursor::new(&bytes[..]), &mut out).expect("saves");
    let mut again = Package::open(Cursor::new(&out[..]), &Limits::V1).expect("reads");
    assert_eq!(
        again.read_blob(&h).expect("the blob").bytes,
        b"picture".to_vec()
    );
    again.verify_parts().expect("hashes");
}

#[test]
fn a_staged_blob_nothing_refers_to_is_not_written() {
    let bytes = file();
    let mut f = open(&bytes);
    let h = f.add_blob("image/png", b"unused".to_vec()).value;
    f.apply(&set_text(&f, "s-1", "changed"))
        .expect("it applies");
    let mut out = Vec::new();
    f.save(Cursor::new(&bytes[..]), &mut out).expect("saves");
    let again = Package::open(Cursor::new(&out[..]), &Limits::V1).expect("reads");
    let names: Vec<&str> = again
        .manifest()
        .parts
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert!(!names.contains(&h.blob_name().as_str()), "{names:?}");
}

#[test]
fn staging_a_blob_the_file_holds_keeps_the_files_listing() {
    let mut parts = MemParts::new();
    let h = parts.add_blob("image/png", b"picture".to_vec());
    let mut with_pic = board();
    apply_group(&mut with_pic, &add_image(&h)).expect("it applies");
    let bytes = file_with(&with_pic, &mut parts);
    let mut f = open(&bytes);
    f.add_blob("image/webp", b"picture".to_vec());
    f.apply(&set_text(&f, "s-1", "changed"))
        .expect("it applies");
    let mut out = Vec::new();
    f.save(Cursor::new(&bytes[..]), &mut out).expect("saves");
    let again = Package::open(Cursor::new(&out[..]), &Limits::V1).expect("reads");
    let part = again
        .manifest()
        .parts
        .iter()
        .find(|p| p.name == h.blob_name())
        .expect("the part");
    assert_eq!(part.media_type, "image/png");
}

#[test]
fn a_reference_with_no_blob_is_a_refusal_at_save() {
    let bytes = file();
    let mut f = open(&bytes);
    f.apply(&add_image(&Sha256::of(b"nowhere")))
        .expect("it applies");
    let mut out = Vec::new();
    let err = f
        .save(Cursor::new(&bytes[..]), &mut out)
        .expect_err("refused")
        .value;
    let SaveError::Refused(r) = err else {
        panic!("wanted a refusal, got {err:?}");
    };
    assert_eq!(r.code, RefusalCode::BlobMissing);
}

#[test]
fn an_unknown_part_and_a_preview_survive_a_save() {
    let mut parts = MemParts::new();
    parts.set_preview(b"preview".to_vec());
    parts.add(
        "extras/later.bin",
        "later",
        "application/octet-stream",
        b"keep".to_vec(),
        Fields::new(),
    );
    let bytes = file_with(&board(), &mut parts);
    let mut f = open(&bytes);
    f.apply(&set_text(&f, "s-1", "changed"))
        .expect("it applies");
    let mut out = Vec::new();
    f.save(Cursor::new(&bytes[..]), &mut out).expect("saves");
    let mut again = Package::open(Cursor::new(&out[..]), &Limits::V1).expect("reads");
    let names: Vec<&str> = again
        .manifest()
        .parts
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(
        names,
        ["board.json", "extras/later.bin", "preview.png"],
        "every part is kept"
    );
    assert_eq!(
        again.read_part("preview.png").expect("preview"),
        b"preview".to_vec()
    );
    assert_eq!(
        again.read_part("extras/later.bin").expect("kept"),
        b"keep".to_vec()
    );
}

#[test]
fn list_gives_order_revisions_and_a_filter() {
    let b = board();
    let all = verbs::list(&b, None).expect("a list");
    let ids: Vec<&str> = all.iter().map(|v| v.object.id.as_str()).collect();
    assert_eq!(ids, ["s-1", "s-2", "c-1", "k-1", "b-1", "q-1"]);
    assert_eq!(all[1].position, 1);
    assert_eq!(all[1].rev, b.objects[1].rev().expect("a rev"));
    let stickies = verbs::list(&b, Some(&ObjectType::Sticky)).expect("a list");
    let ids: Vec<&str> = stickies.iter().map(|v| v.object.id.as_str()).collect();
    assert_eq!(ids, ["s-1", "s-2"]);
    assert_eq!(
        stickies[1].position, 1,
        "its place on the board, not in the filter"
    );
}

#[test]
fn get_finds_references_both_ways() {
    let b = board();
    let view = verbs::read(&b, &id("s-1")).expect("reads").expect("there");
    assert_eq!(view.references.len(), 1, "{:?}", view.references);
    assert_eq!(view.references[0].from, id("c-1"));
    assert_eq!(view.references[0].state, RefState::Attached);
    let c = verbs::read(&b, &id("c-1")).expect("reads").expect("there");
    assert_eq!(c.references.len(), 2, "{:?}", c.references);
    let q = verbs::read(&b, &id("q-1")).expect("reads").expect("there");
    assert_eq!(q.references[0].state, RefState::Orphaned);
    assert_eq!(verbs::read(&b, &id("nope")).expect("reads"), None);
}

#[test]
fn a_kept_hint_reaches_the_json() {
    let json = "{\"frames\":[],\"objects\":[{\"from\":{\"hint\":{\"x\":1.5,\"y\":2},\"id\":\"gone\"},\"id\":\"c-1\",\"to\":{\"x\":0,\"y\":0},\"type\":\"connector\"}],\"people\":{},\"settings\":{}}";
    let b = Board::from_json(json.as_bytes(), &Limits::V1).expect("the board");
    let view = verbs::read(&b, &id("c-1")).expect("reads").expect("there");
    let Value::Obj(r) = verbs::reference_value(&view.references[0]) else {
        panic!("wanted an object");
    };
    let hint = r.get("hint").cloned();
    let want: Fields = [
        ("x".to_string(), Value::Num(1.5)),
        ("y".to_string(), Value::Num(2.0)),
    ]
    .into_iter()
    .collect();
    assert_eq!(hint, Some(Value::Obj(want)), "a kept hint reaches the json");
}

#[test]
fn a_view_as_json_holds_the_object_and_its_revision() {
    let b = board();
    let view = verbs::read(&b, &id("q-1")).expect("reads").expect("there");
    let Value::Obj(v) = view.to_value() else {
        panic!("wanted an object");
    };
    assert_eq!(
        v.get("rev"),
        Some(&Value::Str(view.rev.to_string())),
        "the view's json holds its revision"
    );
    assert_eq!(v["position"], Value::Num(5.0));
    let Value::Arr(refs) = &v["references"] else {
        panic!("wanted a list");
    };
    let Value::Obj(r) = &refs[0] else {
        panic!("wanted an object");
    };
    assert_eq!(r["state"], text("orphaned"));
    assert_eq!(r["field"], text("target"));
    let Value::Obj(o) = &v["object"] else {
        panic!("wanted an object");
    };
    assert_eq!(o["id"], text("q-1"));
}

fn found(b: &Board, q: &str) -> Vec<String> {
    verbs::search(b, q)
        .expect("a search")
        .iter()
        .map(|v| v.object.id.to_string())
        .collect()
}

#[test]
fn search_reads_every_text_a_person_can_search() {
    let b = board();
    assert_eq!(found(&b, "HELLO"), ["s-1"], "text, any case");
    assert_eq!(found(&b, "alpha"), ["s-1"], "tags");
    assert_eq!(found(&b, "ties"), ["c-1"], "a connector's label");
    assert_eq!(found(&b, "plan"), ["k-1"], "a title");
    assert_eq!(found(&b, "bo"), ["k-1"], "an assignee");
    assert_eq!(found(&b, "ready"), ["k-1"], "a status");
    assert_eq!(found(&b, "cell words"), ["b-1"], "a cell");
    assert_eq!(found(&b, "reply words"), ["q-1"], "a reply");
    assert_eq!(
        found(&b, "  hello "),
        ["s-1"],
        "spaces around it don't count"
    );
    assert!(found(&b, "   ").is_empty(), "an empty search finds nothing");
    let hit = verbs::search(&b, "ready").expect("a search");
    assert_eq!(hit[0].position, 3, "its place on the board");
    assert_eq!(hit[0].rev, b.objects[3].rev().expect("a rev"));
    assert!(found(&b, "nothing has this").is_empty());
}

#[test]
fn fields_gives_one_type_or_all_and_refuses_an_unknown_one() {
    let Value::Obj(all) = verbs::fields(None).expect("fields") else {
        panic!("wanted an object");
    };
    let Value::Obj(types) = &all["types"] else {
        panic!("wanted an object");
    };
    assert_eq!(types.len(), 11);
    assert!(matches!(all["common"], Value::Obj(_) | Value::Arr(_)));
    let Value::Obj(one) = verbs::fields(Some("pen")).expect("fields") else {
        panic!("wanted an object");
    };
    let Value::Obj(types) = &one["types"] else {
        panic!("wanted an object");
    };
    let names: Vec<&String> = types.keys().collect();
    assert_eq!(names, ["pen"]);
    let r = verbs::fields(Some("nope")).map(|_| ());
    assert_eq!(
        r.map_err(|e| match e {
            verbs::VerbError::Refused { code, .. } => code.as_str().to_string(),
            other => format!("{other:?}"),
        }),
        Err("unknown_type".to_string()),
        "a type the format doesn't know is the verbs' unknown_type"
    );
}

/// A path a host might give, with a space in front and letters past ASCII: the echo keeps it byte
/// for byte, and the crate never resolves it.
const PATH: &str = " Plans/Q3 board (\u{fc}).k5board";

fn target_at(path: &str) -> FileTarget {
    FileTarget::new(path).expect("a path")
}

fn on(path: &str, version: Option<Sha256>) -> On {
    On {
        kind: FORMAT,
        path: path.to_string(),
        version: version.map(|v| v.to_string()),
        mode: Mode::File,
    }
}

fn open_at(bytes: &[u8]) -> BoardFile<Cursor<&[u8]>> {
    BoardFile::open(target_at(PATH), Cursor::new(bytes), &Limits::V1).expect("it opens")
}

fn guard_of(bytes: &[u8]) -> Sha256 {
    package_guard_of(Cursor::new(bytes), &Limits::V1).expect("a guard")
}

#[test]
fn an_empty_target_is_refused() {
    assert_eq!(FileTarget::new(""), None, "an empty target is refused");
    assert_eq!(
        FileTarget::new(PATH).map(|t| t.path().to_string()),
        Some(PATH.to_string()),
        "a path is kept byte for byte"
    );
}

#[test]
fn every_answer_says_which_file_by_which_guard_in_file_mode() {
    let bytes = file();
    let want = on(PATH, Some(guard_of(&bytes)));
    let mut f = open_at(&bytes);
    assert_eq!(f.on(), want, "open: the file says what it is");
    assert_eq!(f.echo(()).on, want, "echo");
    let g = set_text(&f, "s-1", "changed");
    let stale = set_text(&f, "s-1", "again");
    assert_eq!(f.apply(&g).expect("it applies").on, want, "apply");
    assert_eq!(
        f.apply(&stale).expect_err("stale").on,
        want,
        "apply, refused"
    );
    assert_eq!(f.apply_all(&[stale]).on, want, "apply_all");
    let h = f.add_blob("image/png", b"picture".to_vec());
    assert_eq!(h.on, want, "add_blob");
    assert_eq!(f.read_blob(&h.value).expect("staged").on, want, "read_blob");
    assert_eq!(
        f.read_blob(&Sha256::of(b"nowhere"))
            .expect_err("no such blob")
            .on,
        want,
        "read_blob, refused"
    );
    assert_eq!(
        f.still_current(Cursor::new(&bytes[..])).expect("reads").on,
        want,
        "still_current"
    );
    assert_eq!(
        f.still_current(Cursor::new(&b"not a zip"[..]))
            .expect_err("refused")
            .on,
        want,
        "still_current, refused"
    );
    let base = f.board().clone();
    let mut ours = base.clone();
    ours.settings.insert("title".into(), text("Ours"));
    assert_eq!(
        f.merge(&base, &ours, &mut MemParts::new())
            .expect("a clean merge")
            .on,
        want,
        "merge"
    );
    let mut other = board();
    other
        .objects
        .iter_mut()
        .find(|o| o.id.as_str() == "s-1")
        .expect("s-1")
        .fields
        .insert("text".into(), text("other"));
    assert_eq!(
        f.merge(&board(), &other, &mut MemParts::new())
            .expect_err("a conflict")
            .on,
        want,
        "merge, refused"
    );
}

#[test]
fn the_version_moves_from_read_to_written_on_a_save() {
    let bytes = file();
    let read = guard_of(&bytes);
    let mut out = Vec::new();
    let f = open_at(&bytes);
    let nothing = f.save(Cursor::new(&bytes[..]), &mut out).expect("saves");
    assert_eq!(
        nothing.on,
        on(PATH, Some(read)),
        "nothing written: the guard read"
    );

    let mut f = open_at(&bytes);
    f.apply(&set_text(&f, "s-1", "changed"))
        .expect("it applies");
    let wrote = f.save(Cursor::new(&bytes[..]), &mut out).expect("saves");
    let written = guard_of(&out);
    assert_eq!(
        wrote.on,
        on(PATH, Some(written)),
        "a write: the guard written"
    );
    assert_ne!(written, read, "the guard moved");

    let mut theirs = board();
    theirs.settings.insert("title".into(), text("Theirs"));
    let on_disk = file_with(&theirs, &mut MemParts::new());
    let mut f = open_at(&bytes);
    f.apply(&set_text(&f, "s-1", "mine")).expect("it applies");
    let stale = f
        .save(Cursor::new(&on_disk[..]), &mut Vec::new())
        .expect_err("stale");
    assert_eq!(
        stale.on,
        on(PATH, Some(read)),
        "stale: the guard read, not the one on disk"
    );

    let mut f = open_at(&bytes);
    f.apply(&set_text(&f, "s-1", "mine")).expect("it applies");
    let refused = f
        .save(Cursor::new(&b"not a zip"[..]), &mut Vec::new())
        .expect_err("refused");
    assert_eq!(refused.on, on(PATH, Some(read)), "refused: the guard read");
}

#[test]
fn a_refused_open_echoes_the_target_with_no_version() {
    let e = BoardFile::open(target_at(PATH), Cursor::new(&b"not a zip"[..]), &Limits::V1)
        .err()
        .expect("refused");
    assert_eq!(
        e.on,
        on(PATH, None),
        "a refused open: the target as given, and no guard was read"
    );
}
