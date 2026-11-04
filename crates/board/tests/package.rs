//! Open, edit and write: round trips, unknown parts, the guard, and what the writer refuses.

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::*;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};

const P1: &str = "p-0123456789abcdef";

fn lineage() -> Lineage {
    Lineage::from_bytes([7; 16])
}

fn sample_board(blob: &Sha256) -> Board {
    let text = format!(
        "{{\"frames\":[\"f-1\"],\"objects\":[{{\"id\":\"f-1\",\"title\":\"One\",\"type\":\"frame\"}},\
         {{\"id\":\"s-1\",\"reactions\":{{\"x\":[\"{P1}\"]}},\"text\":\"hello\",\"type\":\"sticky\",\"x\":0.1}},\
         {{\"id\":\"i-1\",\"src\":{{\"blob\":\"{blob}\"}},\"type\":\"image\"}},\
         {{\"id\":\"n-1\",\"newThing\":[1,2,{{\"a\":null}}],\"type\":\"future-type\"}}],\
         \"people\":{{\"{P1}\":{{\"name\":\"Ann\"}}}},\"settings\":{{\"title\":\"T\"}},\"later\":true}}"
    );
    Board::from_json(text.as_bytes(), &Limits::V1).expect("the sample board")
}

/// A file with a blob, a preview and a part of an unknown role.
fn sample_file() -> (Vec<u8>, Written) {
    let mut parts = MemParts::new();
    let h = parts.add_blob("image/png", b"image bytes".to_vec());
    parts.set_preview(b"a preview".to_vec());
    let mut extra = Fields::new();
    extra.insert("note".into(), Value::Str("kept".into()));
    parts.add(
        "extra/data.bin",
        "future-role",
        "application/x-thing",
        b"future".to_vec(),
        extra,
    );
    let board = sample_board(&h);
    let mut out = Vec::new();
    let w = write(
        &mut out,
        &WriteMeta::new(lineage()),
        &board,
        &mut parts,
        &Limits::V1,
    )
    .expect("write");
    (out, w)
}

fn open(bytes: &[u8]) -> Package<Cursor<Vec<u8>>> {
    Package::open(Cursor::new(bytes.to_vec()), &Limits::V1).expect("open")
}

fn rewrite(pkg: &mut Package<Cursor<Vec<u8>>>, board: &Board) -> (Vec<u8>, Written) {
    let meta = pkg.manifest().write_meta();
    let mut out = Vec::new();
    let w = write(&mut out, &meta, board, pkg, &Limits::V1).expect("rewrite");
    (out, w)
}

#[test]
fn our_files_round_trip_byte_exact() {
    let (first, w) = sample_file();
    assert_eq!(
        w.bytes,
        first.len() as u64,
        "Written.bytes isn't the file's size"
    );
    let mut pkg = open(&first);
    let board = pkg.board().clone();
    let (second, _) = rewrite(&mut pkg, &board);
    assert!(first == second, "an unchanged open and write changed bytes");
}

#[test]
fn an_unchanged_open_and_write_keeps_the_guard() {
    let (first, w) = sample_file();
    let mut pkg = open(&first);
    assert_eq!(
        pkg.package_guard(),
        w.package_guard,
        "open and write disagree on the guard"
    );
    let board = pkg.board().clone();
    let (_, again) = rewrite(&mut pkg, &board);
    assert_eq!(again.package_guard, w.package_guard, "the guard moved");
}

#[test]
fn guard_of_reads_the_same_guard_cheaply() {
    let (first, w) = sample_file();
    let g = package_guard_of(Cursor::new(first), &Limits::V1).expect("guard");
    assert_eq!(g, w.package_guard);
}

#[test]
fn guard_is_the_hash_of_the_canonical_manifest() {
    let (first, w) = sample_file();
    let pkg = open(&first);
    let canon = pkg.manifest().to_canonical_json().expect("canonical");
    assert_eq!(Sha256::of(&canon), w.package_guard);
}

#[test]
fn the_guard_trips_on_an_added_part() {
    let (first, w) = sample_file();
    let mut pkg = open(&first);
    let board = pkg.board().clone();
    let meta = pkg.manifest().write_meta();
    let mut overlay = Overlay::new(&mut pkg);
    overlay.add_blob("image/png", b"an unreferenced blob".to_vec());
    let mut out = Vec::new();
    let w2 = write(&mut out, &meta, &board, &mut overlay, &Limits::V1).expect("write");
    assert_ne!(
        w2.package_guard, w.package_guard,
        "an added blob didn't move the guard"
    );
    assert_eq!(
        w2.board_hash, w.board_hash,
        "board.json itself didn't change"
    );
}

#[test]
fn the_guard_trips_on_a_board_edit_and_on_a_new_base() {
    let (first, w) = sample_file();
    let mut pkg = open(&first);
    let mut board = pkg.board().clone();
    board
        .settings
        .insert("title".into(), Value::Str("New".into()));
    let (_, edited) = rewrite(&mut pkg, &board);
    assert_ne!(
        edited.package_guard, w.package_guard,
        "an edit didn't move the guard"
    );
    assert_ne!(
        edited.board_hash, w.board_hash,
        "an edit didn't move the board hash"
    );

    let mut pkg = open(&first);
    let board = pkg.board().clone();
    let mut meta = pkg.manifest().write_meta();
    meta.base = Some(Base::new(w.board_hash));
    let mut out = Vec::new();
    let based = write(&mut out, &meta, &board, &mut pkg, &Limits::V1).expect("write");
    assert_ne!(
        based.package_guard, w.package_guard,
        "a new base didn't move the guard"
    );
    let reopened = open(&out);
    assert_eq!(
        reopened.manifest().base,
        Some(Base::new(w.board_hash)),
        "base wasn't kept"
    );
    assert_eq!(
        reopened.manifest().lineage,
        lineage(),
        "lineage wasn't kept"
    );
}

#[test]
fn base_and_lineage_stay_put_across_saves() {
    let (first, w) = sample_file();
    let mut pkg = open(&first);
    let board = pkg.board().clone();
    let mut meta = pkg.manifest().write_meta();
    meta.base = Some(Base::new(w.board_hash));
    let mut with_base = Vec::new();
    let w2 = write(&mut with_base, &meta, &board, &mut pkg, &Limits::V1).expect("write");
    let mut pkg = open(&with_base);
    let board = pkg.board().clone();
    let (_, w3) = rewrite(&mut pkg, &board);
    assert_eq!(
        w3.package_guard, w2.package_guard,
        "a base didn't pass through unchanged"
    );
}

#[test]
fn unknown_parts_and_previews_survive_an_edit_unchanged() {
    let (first, _) = sample_file();
    let mut pkg = open(&first);
    let mut board = pkg.board().clone();
    board.objects[1]
        .fields
        .insert("text".into(), Value::Str("edited".into()));
    let (second, _) = rewrite(&mut pkg, &board);
    let mut again = open(&second);
    let future = again
        .manifest()
        .parts
        .iter()
        .find(|p| p.name == "extra/data.bin")
        .expect("the unknown part is listed")
        .clone();
    assert_eq!(future.role, "future-role");
    assert_eq!(future.media_type, "application/x-thing");
    assert_eq!(future.extra.get("note"), Some(&Value::Str("kept".into())));
    assert_eq!(again.read_part("extra/data.bin").expect("read"), b"future");
    assert_eq!(again.read_part("preview.png").expect("read"), b"a preview");
}

#[test]
fn overlay_changes_only_what_it_is_told() {
    let (first, _) = sample_file();
    let mut pkg = open(&first);
    let board = pkg.board().clone();
    let meta = pkg.manifest().write_meta();
    let mut overlay = Overlay::new(&mut pkg);
    overlay.set_preview(b"new preview".to_vec());
    overlay.remove("extra/data.bin");
    let mut out = Vec::new();
    write(&mut out, &meta, &board, &mut overlay, &Limits::V1).expect("write");
    let mut again = open(&out);
    assert_eq!(
        again.read_part("preview.png").expect("read"),
        b"new preview"
    );
    let r = again.read_part("extra/data.bin").expect_err("removed");
    assert_eq!(r.code, RefusalCode::PartMissing);
}

#[test]
fn removing_a_referenced_blob_is_refused() {
    let (first, _) = sample_file();
    let mut pkg = open(&first);
    let board = pkg.board().clone();
    let meta = pkg.manifest().write_meta();
    let h = board.blob_refs().into_iter().next().expect("a blob");
    let mut overlay = Overlay::new(&mut pkg);
    overlay.remove(&h.blob_name());
    let r = write(&mut Vec::new(), &meta, &board, &mut overlay, &Limits::V1).expect_err("refused");
    assert_eq!(r.code, RefusalCode::BlobMissing);
}

#[test]
fn a_reference_in_an_unknown_field_counts() {
    let h = Sha256::of(b"x");
    let text = format!(
        "{{\"frames\":[],\"objects\":[{{\"id\":\"a\",\"later\":{{\"deep\":[{{\"blob\":\"{h}\"}}]}},\"type\":\"sticky\"}}],\"people\":{{}},\"settings\":{{}}}}"
    );
    let b = Board::from_json(text.as_bytes(), &Limits::V1).expect("board");
    assert!(b.blob_refs().contains(&h), "the reference went unseen");
    let r = write(
        &mut Vec::new(),
        &WriteMeta::new(lineage()),
        &b,
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect_err("refused");
    assert_eq!(r.code, RefusalCode::BlobMissing);
}

#[test]
fn the_writer_computes_the_parts() {
    let (first, _) = sample_file();
    let pkg = open(&first);
    let m = pkg.manifest();
    assert_eq!(m.version, VERSION);
    assert_eq!(m.min_reader, MIN_READER);
    let board = m
        .parts
        .iter()
        .find(|p| p.name == "board.json")
        .expect("listed");
    assert_eq!(board.sha256, pkg.board_hash());
    assert_eq!(
        board.bytes,
        pkg.board().to_canonical_json().expect("json").len() as u64
    );
    let names: Vec<&str> = m.parts.iter().map(|p| p.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "parts aren't sorted by name");
}

#[test]
fn reserved_manifest_fields_are_refused_from_a_caller() {
    let mut meta = WriteMeta::new(lineage());
    meta.extra.insert("parts".into(), Value::Arr(Vec::new()));
    let r = write(
        &mut Vec::new(),
        &meta,
        &Board::empty(),
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect_err("refused");
    assert_eq!(r.code, RefusalCode::ManifestInvalid);
}

struct Liar(MemParts, Vec<u8>);
impl PartSource for Liar {
    fn list(&self) -> Vec<Part> {
        self.0.list()
    }
    fn read_part(&mut self, _name: &str) -> Result<Vec<u8>, Refusal> {
        Ok(self.1.clone())
    }
}

#[test]
fn a_source_that_lies_is_refused() {
    let mut mem = MemParts::new();
    mem.add_blob("image/png", b"abcd".to_vec());
    let mut same_size = Liar(mem.clone(), b"abcX".to_vec());
    let r = write(
        &mut Vec::new(),
        &WriteMeta::new(lineage()),
        &Board::empty(),
        &mut same_size,
        &Limits::V1,
    )
    .expect_err("refused");
    assert_eq!(r.code, RefusalCode::PartHash);
    let mut other_size = Liar(mem, b"abcdefg".to_vec());
    let r = write(
        &mut Vec::new(),
        &WriteMeta::new(lineage()),
        &Board::empty(),
        &mut other_size,
        &Limits::V1,
    )
    .expect_err("refused");
    assert_eq!(r.code, RefusalCode::PartDisagree);
}

#[test]
fn the_writer_holds_the_same_part_rules() {
    let write_with = |parts: &mut MemParts| {
        write(
            &mut Vec::new(),
            &WriteMeta::new(lineage()),
            &Board::empty(),
            parts,
            &Limits::V1,
        )
        .map(|_| ())
        .map_err(|r| r.code)
    };
    let mut bad_name = MemParts::new();
    bad_name.add(
        "../x",
        "other",
        "application/x",
        b"x".to_vec(),
        Fields::new(),
    );
    assert_eq!(write_with(&mut bad_name), Err(RefusalCode::ZipNameInvalid));
    let mut wrong_role = MemParts::new();
    wrong_role.add(
        "board.json",
        "other",
        "application/x",
        b"x".to_vec(),
        Fields::new(),
    );
    assert_eq!(write_with(&mut wrong_role), Err(RefusalCode::PartDuplicate));
    let mut second_board = MemParts::new();
    second_board.add(
        "b2.json",
        "board",
        "application/x",
        b"x".to_vec(),
        Fields::new(),
    );
    assert_eq!(write_with(&mut second_board), Err(RefusalCode::PartRole));
    let mut fold = MemParts::new();
    fold.add(
        "EXTRA",
        "other",
        "application/x",
        b"x".to_vec(),
        Fields::new(),
    );
    fold.add(
        "extra",
        "other",
        "application/x",
        b"x".to_vec(),
        Fields::new(),
    );
    assert_eq!(write_with(&mut fold), Err(RefusalCode::PartDuplicate));
    let mut named_wrong = MemParts::new();
    named_wrong.add(
        "blobs/sha256-abc",
        "blob",
        "image/png",
        b"x".to_vec(),
        Fields::new(),
    );
    assert_eq!(write_with(&mut named_wrong), Err(RefusalCode::PartHash));
}

#[test]
fn the_writer_holds_the_limits() {
    let tight = |edit: fn(&mut Limits)| {
        let mut l = Limits::V1;
        edit(&mut l);
        l
    };
    let mut parts = MemParts::new();
    parts.add_blob("image/png", b"abcdef".to_vec());
    parts.add_blob("image/png", b"ghijkl".to_vec());
    let attempt = |parts: &mut MemParts, l: Limits| {
        write(
            &mut Vec::new(),
            &WriteMeta::new(lineage()),
            &Board::empty(),
            parts,
            &l,
        )
        .map(|_| ())
        .map_err(|r| r.code)
    };
    assert_eq!(
        attempt(&mut parts, tight(|l| l.entries = 3)),
        Err(RefusalCode::LimitEntries)
    );
    assert_eq!(
        attempt(&mut parts, tight(|l| l.part_bytes = 5)),
        Err(RefusalCode::LimitPartBytes)
    );
    assert_eq!(
        attempt(&mut parts, tight(|l| l.board_json_bytes = 5)),
        Err(RefusalCode::LimitPartBytes)
    );
    assert_eq!(
        attempt(&mut parts, tight(|l| l.manifest_bytes = 5)),
        Err(RefusalCode::LimitPartBytes)
    );
    assert_eq!(
        attempt(&mut parts, tight(|l| l.total_bytes = 20)),
        Err(RefusalCode::LimitTotalBytes)
    );
    assert_eq!(attempt(&mut parts, Limits::V1), Ok(()));
}

#[test]
fn a_host_may_tighten_a_limit_but_not_raise_one() {
    let mut raised = Limits::V1;
    raised.objects += 1;
    let r = Package::open(Cursor::new(sample_file().0), &raised)
        .err()
        .expect("refused");
    assert_eq!(r.code, RefusalCode::LimitRaised);
    let r = Board::from_json(b"{}", &raised).expect_err("refused");
    assert_eq!(r.code, RefusalCode::LimitRaised);
    let r = write(
        &mut Vec::new(),
        &WriteMeta::new(lineage()),
        &Board::empty(),
        &mut MemParts::new(),
        &raised,
    )
    .expect_err("refused");
    assert_eq!(r.code, RefusalCode::LimitRaised);
    let r = package_guard_of(Cursor::new(sample_file().0), &raised).expect_err("refused");
    assert_eq!(r.code, RefusalCode::LimitRaised);
    for edit in [
        (|l: &mut Limits| l.entries += 1) as fn(&mut Limits),
        |l| l.manifest_bytes += 1,
        |l| l.board_json_bytes += 1,
        |l| l.part_bytes += 1,
        |l| l.total_bytes += 1,
        |l| l.ratio += 1,
        |l| l.ratio_floor += 1,
        |l| l.people += 1,
        |l| l.depth += 1,
        |l| l.values += 1,
    ] {
        let mut l = Limits::V1;
        edit(&mut l);
        assert_eq!(
            l.check_ceiling().expect_err("refused").code,
            RefusalCode::LimitRaised
        );
    }
    let mut tighter = Limits::V1;
    tighter.objects = 10;
    assert_eq!(tighter.check_ceiling(), Ok(()));
}

#[test]
fn a_one_field_edit_changes_only_that_field() {
    let (first, _) = sample_file();
    let pkg = open(&first);
    let before = pkg.board().to_canonical_json().expect("json");
    let mut board = pkg.board().clone();
    board.objects[1]
        .fields
        .insert("text".into(), Value::Str("HELLO".into()));
    let after = board.to_canonical_json().expect("json");
    assert_eq!(before.len(), after.len());
    let differing: Vec<usize> = (0..before.len())
        .filter(|i| before[*i] != after[*i])
        .collect();
    assert_eq!(
        differing.len(),
        5,
        "more than the field's five letters changed"
    );
    assert_eq!(differing[4] - differing[0], 4, "the changes aren't one run");
}

#[test]
fn numbers_keep_their_value_through_a_file() {
    let (first, _) = sample_file();
    let mut pkg = open(&first);
    assert_eq!(
        pkg.board().objects[1].fields.get("x"),
        Some(&Value::Num(0.1))
    );
    let board = pkg.board().clone();
    let (second, _) = rewrite(&mut pkg, &board);
    assert_eq!(
        open(&second).board().objects[1].fields.get("x"),
        Some(&Value::Num(0.1))
    );
}

#[test]
fn blobs_read_by_hash_with_their_media_type() {
    let (first, _) = sample_file();
    let mut pkg = open(&first);
    let h = Sha256::of(b"image bytes");
    let blob = pkg.read_blob(&h).expect("blob");
    assert_eq!(blob.media_type, "image/png");
    assert_eq!(blob.bytes, b"image bytes");
    let r = pkg.read_blob(&Sha256::of(b"none")).expect_err("refused");
    assert_eq!(r.code, RefusalCode::BlobMissing);
    pkg.verify_parts().expect("every part holds");
    let r = pkg.read_part("nope").expect_err("refused");
    assert_eq!(r.code, RefusalCode::PartMissing);
}

#[test]
fn a_changed_blob_is_refused_when_read_not_at_open() {
    let (mut file, _) = sample_file();
    let at = file
        .windows(11)
        .position(|w| w == b"image bytes")
        .expect("stored as is");
    file[at] = b'I';
    // The CRC catches it first; fix the file's CRC fields so the hash has to.
    let good = crc(b"image bytes").to_le_bytes();
    let bad = crc(b"Image bytes").to_le_bytes();
    let spots: Vec<usize> = (0..file.len() - 4)
        .filter(|i| file[*i..*i + 4] == good)
        .collect();
    assert_eq!(spots.len(), 2, "the CRC appears in two headers");
    for s in spots {
        file[s..s + 4].copy_from_slice(&bad);
    }
    let guard_before = package_guard_of(Cursor::new(file.clone()), &Limits::V1).expect("guard");
    let mut pkg = open(&file);
    assert_eq!(
        pkg.package_guard(),
        guard_before,
        "the guard reads only the manifest"
    );
    let r = pkg
        .read_blob(&Sha256::of(b"image bytes"))
        .expect_err("refused");
    assert_eq!(r.code, RefusalCode::PartHash);
    assert_eq!(
        pkg.verify_parts().expect_err("refused").code,
        RefusalCode::PartHash
    );
}

fn crc(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

#[test]
fn very_compressible_boards_still_read_back() {
    // Deflate would pass the ratio limit here, so the writer has to store this one.
    let mut board = Board::empty();
    board
        .settings
        .insert("blob".into(), Value::Str(" ".repeat(32 * 1024 * 1024)));
    let mut out = Vec::new();
    write(
        &mut out,
        &WriteMeta::new(lineage()),
        &board,
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect("write");
    let pkg = open(&out);
    assert_eq!(pkg.board(), &board);
}

#[test]
fn stored_output_is_deflated_when_it_helps() {
    let mut board = Board::empty();
    board
        .settings
        .insert("title".into(), Value::Str("abc ".repeat(40)));
    let mut out = Vec::new();
    let w = write(
        &mut out,
        &WriteMeta::new(lineage()),
        &board,
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect("write");
    let raw = board.to_canonical_json().expect("json").len() as u64;
    assert!(
        w.bytes < raw + 400,
        "board.json wasn't deflated: file {} bytes, board {raw}",
        w.bytes
    );
}

struct BrokenReader;
impl Read for BrokenReader {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("disk gone"))
    }
}
impl Seek for BrokenReader {
    fn seek(&mut self, _: SeekFrom) -> std::io::Result<u64> {
        Ok(1000)
    }
}
struct BrokenWriter;
impl Write for BrokenWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("disk full"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn input_and_output_failures_are_typed() {
    let r = Package::open(BrokenReader, &Limits::V1)
        .err()
        .expect("refused");
    assert_eq!(r.code, RefusalCode::IoError);
    let r = write(
        &mut BrokenWriter,
        &WriteMeta::new(lineage()),
        &Board::empty(),
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect_err("refused");
    assert_eq!(r.code, RefusalCode::IoError);
}

#[test]
fn strip_names_keeps_the_tokens() {
    let (first, _) = sample_file();
    let mut board = open(&first).into_board();
    let token = Id::new(P1).expect("id");
    assert_eq!(board.people[&token].name.as_deref(), Some("Ann"));
    let before = board.objects.clone();
    board.strip_names();
    assert_eq!(board.people[&token].name, None, "the name stayed");
    assert_eq!(board.objects, before, "objects changed");
    board
        .validate(&Limits::V1)
        .expect("still valid: reactions name a known token");
    let json = String::from_utf8(board.to_canonical_json().expect("json")).expect("utf8");
    assert!(!json.contains("Ann"), "the name is still in the file");
}

#[test]
fn person_tokens_come_from_the_hosts_random_bytes() {
    let a = mint_person_token([1, 2, 3, 4, 5, 6, 7, 8]);
    assert_eq!(a.as_str(), "p-0102030405060708");
    assert!(Id::new(a.as_str()).is_ok(), "a token must fit the id rule");
    assert_ne!(a, mint_person_token([1, 2, 3, 4, 5, 6, 7, 9]));
}

#[test]
fn absent_fields_stay_absent_and_defaults_come_from_the_schema() {
    let text = "{\"frames\":[],\"objects\":[{\"id\":\"s\",\"type\":\"sticky\"}],\"people\":{},\"settings\":{}}";
    let b = Board::from_json(text.as_bytes(), &Limits::V1).expect("board");
    assert!(b.objects[0].fields.is_empty(), "a default was written in");
    assert_eq!(b.to_canonical_json().expect("json"), text.as_bytes());
    assert_eq!(
        Board::default_of("sticky", "w"),
        Some(schema::Default::Num(200.0))
    );
    assert_eq!(
        Board::default_of("sticky", "x"),
        Some(schema::Default::Num(0.0))
    );
    assert_eq!(
        Board::default_of("pen", "w"),
        None,
        "a pen's box is derived, not a field"
    );
}

fn refs_board(objects: &str) -> Board {
    let text =
        format!("{{\"frames\":[],\"objects\":[{objects}],\"people\":{{}},\"settings\":{{}}}}");
    Board::from_json(text.as_bytes(), &Limits::V1).expect("board")
}

fn state_of(b: &Board, from: &str, field: &str) -> RefState {
    b.references()
        .into_iter()
        .find(|r| r.from.as_str() == from && r.field == field)
        .expect("that reference")
        .state
}

#[test]
fn references_resolve_to_states() {
    let b = refs_board(
        "{\"id\":\"s\",\"type\":\"sticky\"},\
         {\"from\":{\"id\":\"s\"},\"id\":\"c\",\"to\":{\"hint\":{\"x\":3,\"y\":4},\"id\":\"gone\"},\"type\":\"connector\"},\
         {\"id\":\"q\",\"target\":\"gone\",\"type\":\"comment\"},\
         {\"id\":\"q2\",\"target\":\"s\",\"type\":\"comment\"},\
         {\"frame\":\"s\",\"id\":\"w\",\"type\":\"text\"}",
    );
    assert_eq!(state_of(&b, "c", "from"), RefState::Attached);
    assert_eq!(state_of(&b, "c", "to"), RefState::Orphaned);
    assert_eq!(state_of(&b, "q", "target"), RefState::Orphaned);
    assert_eq!(state_of(&b, "q2", "target"), RefState::Attached);
    assert_eq!(
        state_of(&b, "w", "frame"),
        RefState::Suspect,
        "a frame reference to a sticky"
    );
    let orphan = b
        .references()
        .into_iter()
        .find(|r| r.field == "to")
        .expect("to");
    assert_eq!(orphan.hint, Some((3.0, 4.0)), "the last point was lost");
}

#[test]
fn a_parent_loop_is_cut_at_its_smallest_id() {
    let b = refs_board(
        "{\"id\":\"m3\",\"parent\":\"m1\",\"type\":\"mind\"},\
         {\"id\":\"m1\",\"parent\":\"m2\",\"type\":\"mind\"},\
         {\"id\":\"m2\",\"parent\":\"m3\",\"type\":\"mind\"},\
         {\"id\":\"m4\",\"parent\":\"m1\",\"type\":\"mind\"},\
         {\"id\":\"m5\",\"parent\":\"m5\",\"type\":\"mind\"}",
    );
    assert_eq!(
        state_of(&b, "m1", "parent"),
        RefState::Suspect,
        "the smallest id in the loop"
    );
    assert_eq!(state_of(&b, "m2", "parent"), RefState::Attached);
    assert_eq!(state_of(&b, "m3", "parent"), RefState::Attached);
    assert_eq!(
        state_of(&b, "m4", "parent"),
        RefState::Attached,
        "outside the loop"
    );
    assert_eq!(
        state_of(&b, "m5", "parent"),
        RefState::Suspect,
        "its own parent"
    );
}

#[test]
fn a_board_edited_in_memory_is_checked_before_a_write() {
    let mut b = Board::empty();
    b.objects.push(Object {
        id: Id::new("a").expect("id"),
        kind: ObjectType::Sticky,
        fields: Fields::new(),
    });
    b.objects.push(b.objects[0].clone());
    let r = write(
        &mut Vec::new(),
        &WriteMeta::new(lineage()),
        &b,
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect_err("refused");
    assert_eq!(r.code, RefusalCode::IdDuplicate);

    let mut b = Board::empty();
    b.settings.insert("later".into(), Value::Num(f64::NAN));
    let r = b.validate(&Limits::V1).expect_err("refused");
    assert_eq!(r.code, RefusalCode::JsonNumberRange);

    let mut b = Board::empty();
    b.objects.push(Object {
        id: Id::new("a").expect("id"),
        kind: ObjectType::Unknown("Not A Name".into()),
        fields: Fields::new(),
    });
    assert_eq!(
        b.validate(&Limits::V1).expect_err("refused").code,
        RefusalCode::TypeInvalid
    );

    let mut deep = Value::Null;
    for _ in 0..70 {
        deep = Value::Arr(vec![deep]);
    }
    let mut b = Board::empty();
    b.extra.insert("later".into(), deep);
    assert_eq!(
        b.validate(&Limits::V1).expect_err("refused").code,
        RefusalCode::LimitDepth
    );
}

#[test]
fn ids_follow_the_grammar() {
    for good in ["a", "0", "a.b_c-d", "s-0123456789", &"z".repeat(24)] {
        assert!(Id::new(good).is_ok(), "{good}");
    }
    for bad in ["", "A", "a b", "a/b", "\u{e9}", &"z".repeat(25)] {
        assert_eq!(
            Id::new(bad).expect_err(bad).code,
            RefusalCode::IdInvalid,
            "{bad}"
        );
    }
}

#[test]
fn sha256_text_forms() {
    let h = Sha256::of(b"test");
    let hex = "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08";
    assert_eq!(h.to_hex(), hex);
    assert_eq!(h.to_string(), format!("sha256-{hex}"));
    assert_eq!(h.blob_name(), format!("blobs/sha256-{hex}"));
    assert_eq!(Sha256::parse(&h.to_string()).expect("parses"), h);
    assert_eq!(Sha256::from_hex(hex), Some(h));
    assert_eq!(Sha256::from_hex(&hex.to_uppercase()), None);
    assert_eq!(
        Sha256::parse(hex).expect_err("no prefix").code,
        RefusalCode::FieldValue
    );
}

#[test]
fn a_frame_loop_is_suspect() {
    let text = "{\"frames\":[\"f1\",\"f2\"],\"objects\":[{\"frame\":\"f2\",\"id\":\"f1\",\"type\":\"frame\"},{\"frame\":\"f1\",\"id\":\"f2\",\"type\":\"frame\"}],\"people\":{},\"settings\":{}}";
    let b = Board::from_json(text.as_bytes(), &Limits::V1).expect("board");
    assert_eq!(state_of(&b, "f1", "frame"), RefState::Suspect);
    assert_eq!(state_of(&b, "f2", "frame"), RefState::Attached);
}

#[test]
fn only_a_lone_blob_member_is_a_reference() {
    let h = Sha256::of(b"x");
    let text = format!(
        "{{\"frames\":[],\"objects\":[{{\"id\":\"a\",\"later\":{{\"blob\":\"{h}\",\"other\":1}},\"type\":\"sticky\"}}],\"people\":{{}},\"settings\":{{}}}}"
    );
    let b = Board::from_json(text.as_bytes(), &Limits::V1).expect("board");
    assert!(b.blob_refs().is_empty(), "not a reference, but it counted");
}

#[test]
fn an_overlay_can_put_back_what_it_removed() {
    let (first, _) = sample_file();
    let mut pkg = open(&first);
    let board = pkg.board().clone();
    let meta = pkg.manifest().write_meta();
    let h = board.blob_refs().into_iter().next().expect("a blob");
    let mut overlay = Overlay::new(&mut pkg);
    overlay.remove(&h.blob_name());
    overlay.remove("preview.png");
    assert_eq!(
        overlay.read_part("preview.png").expect_err("removed").code,
        RefusalCode::PartMissing
    );
    overlay.add_blob("image/png", b"image bytes".to_vec());
    overlay.set_preview(b"a preview".to_vec());
    let mut out = Vec::new();
    write(&mut out, &meta, &board, &mut overlay, &Limits::V1).expect("write");
    assert!(out == first, "putting the same parts back changed the file");
}

#[test]
fn entry_order_does_not_depend_on_the_source() {
    let (first, _) = sample_file();
    let mut pkg = open(&first);
    let board = pkg.board().clone();
    let meta = pkg.manifest().write_meta();
    let mut overlay = Overlay::new(&mut pkg);
    overlay.set_preview(b"a preview".to_vec()); // listed last by the overlay
    let mut out = Vec::new();
    write(&mut out, &meta, &board, &mut overlay, &Limits::V1).expect("write");
    assert!(out == first, "the same content wrote different bytes");
}

#[test]
fn manifest_extras_pass_through() {
    let mut meta = WriteMeta::new(lineage());
    meta.extra.insert("future".into(), Value::Bool(true));
    let mut out = Vec::new();
    let w = write(
        &mut out,
        &meta,
        &Board::empty(),
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect("write");
    let mut pkg = open(&out);
    assert_eq!(pkg.manifest().extra.get("future"), Some(&Value::Bool(true)));
    let board = pkg.board().clone();
    let (_, again) = rewrite(&mut pkg, &board);
    assert_eq!(
        again.package_guard, w.package_guard,
        "an extra didn't pass through"
    );
}

fn people_board(tokens: usize, field: &str) -> String {
    let ids: Vec<String> = (0..tokens).map(|i| format!("p{i:06}")).collect();
    let people: Vec<String> = ids.iter().map(|t| format!("\"{t}\":{{}}")).collect();
    let value = match field {
        "votes" => format!(
            "\"votes\":{{{}}}",
            ids.iter()
                .map(|t| format!("\"{t}\":1"))
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => format!(
            "\"reactions\":{{\"x\":[{}]}}",
            ids.iter()
                .map(|t| format!("\"{t}\""))
                .collect::<Vec<_>>()
                .join(",")
        ),
    };
    format!(
        "{{\"frames\":[],\"objects\":[{{\"id\":\"a\",{value},\"type\":\"sticky\"}}],\"people\":{{{}}},\"settings\":{{}}}}",
        people.join(",")
    )
}

#[test]
fn counts_too_big_for_a_vector_file_are_held() {
    let code = |text: String| {
        Board::from_json(text.as_bytes(), &Limits::V1)
            .map(|_| ())
            .map_err(|r| r.code)
    };
    assert_eq!(code(people_board(10_000, "votes")), Ok(()));
    assert_eq!(
        code(people_board(10_001, "votes")),
        Err(RefusalCode::LimitField)
    );
    assert_eq!(code(people_board(10_000, "reactions")), Ok(()));
    assert_eq!(
        code(people_board(10_001, "reactions")),
        Err(RefusalCode::LimitField)
    );
    let reply =
        format!("{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{P1}\",\"id\":\"ID\",\"text\":\"\"}}");
    let replies = |n: usize| {
        let list: Vec<String> = (0..n)
            .map(|i| reply.replace("ID", &format!("r{i}")))
            .collect();
        format!(
            "{{\"frames\":[],\"objects\":[{{\"id\":\"q\",\"replies\":[{}],\"type\":\"comment\"}}],\"people\":{{\"{P1}\":{{}}}},\"settings\":{{}}}}",
            list.join(",")
        )
    };
    assert_eq!(code(replies(1000)), Ok(()));
    assert_eq!(code(replies(1001)), Err(RefusalCode::LimitField));
}

#[test]
fn a_dangling_reference_opens_reports_and_blocks_a_write() {
    let h = Sha256::of(b"not in the file");
    let text = format!(
        "{{\"frames\":[],\"objects\":[{{\"id\":\"i\",\"src\":{{\"blob\":\"{h}\"}},\"type\":\"image\"}}],\"people\":{{}},\"settings\":{{}}}}"
    );
    let forged = forged_package(&text);
    let mut pkg = Package::open(Cursor::new(forged), &Limits::V1).expect("it opens");
    assert_eq!(
        pkg.missing_blobs(),
        vec![h],
        "the missing blob isn't reported"
    );
    let board = pkg.board().clone();
    let meta = pkg.manifest().write_meta();
    let r = write(&mut Vec::new(), &meta, &board, &mut pkg, &Limits::V1).expect_err("refused");
    assert_eq!(r.code, RefusalCode::BlobMissing);
    let r = pkg.read_blob(&h).expect_err("refused");
    assert_eq!(r.code, RefusalCode::BlobMissing);
}

/// A whole file holding this board.json, stored, from a hand-written manifest and zip.
fn forged_package(board: &str) -> Vec<u8> {
    let hash = Sha256::of(board.as_bytes()).to_hex();
    let manifest = format!(
        "{{\"format\":\"k5board\",\"lineage\":\"{}\",\"min_reader\":1,\"parts\":[{{\"bytes\":{},\"media_type\":\"application/json\",\"name\":\"board.json\",\"role\":\"board\",\"sha256\":\"{hash}\"}}],\"version\":1}}",
        "0".repeat(32),
        board.len()
    );
    let entries = [
        ("manifest.json", manifest.as_bytes()),
        ("board.json", board.as_bytes()),
    ];
    let (mut out, mut central) = (Vec::new(), Vec::new());
    for (name, data) in entries {
        let offset = out.len() as u32;
        let mut common = Vec::new();
        common.extend_from_slice(&20u16.to_le_bytes());
        common.extend_from_slice(&[0; 4]);
        common.extend_from_slice(&[0, 0, 0x21, 0]);
        common.extend_from_slice(&crc(data).to_le_bytes());
        common.extend_from_slice(&(data.len() as u32).to_le_bytes());
        common.extend_from_slice(&(data.len() as u32).to_le_bytes());
        common.extend_from_slice(&(name.len() as u16).to_le_bytes());
        common.extend_from_slice(&[0; 2]);
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&common);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&common);
        central.extend_from_slice(&[0; 10]);
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let at = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&at.to_le_bytes());
    out.extend_from_slice(&[0; 2]);
    out
}

#[test]
fn a_write_never_lowers_the_versions() {
    let mut meta = WriteMeta::new(lineage());
    meta.version = 9;
    let mut out = Vec::new();
    write(
        &mut out,
        &meta,
        &Board::empty(),
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect("write");
    let pkg = open(&out);
    assert_eq!(pkg.manifest().version, 9, "the version came down");
    assert_eq!(
        pkg.manifest().write_meta().version,
        9,
        "write_meta dropped the version"
    );
    assert_eq!(pkg.manifest().write_meta().min_reader, 1);

    let mut low = WriteMeta::new(lineage());
    low.version = 0;
    low.min_reader = 0;
    let mut out = Vec::new();
    write(
        &mut out,
        &low,
        &Board::empty(),
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect("write");
    let pkg = open(&out);
    assert_eq!(
        pkg.manifest().version,
        VERSION,
        "below this crate's own version"
    );
    assert_eq!(
        pkg.manifest().min_reader,
        MIN_READER,
        "below this crate's own reader"
    );
}

#[test]
fn a_persons_kind_is_read_kept_and_survives_stripping_names() {
    let text = "{\"frames\":[],\"objects\":[],\"people\":{\"p-1\":{\"kind\":\"agent\",\"name\":\"Bot\"}},\"settings\":{}}";
    let mut b = Board::from_json(text.as_bytes(), &Limits::V1).expect("board");
    let token = Id::new("p-1").expect("id");
    assert_eq!(b.people[&token].kind.as_deref(), Some("agent"));
    assert!(b.people[&token].extra.is_empty(), "kind landed in extra");
    assert_eq!(b.to_canonical_json().expect("json"), text.as_bytes());
    b.strip_names();
    assert_eq!(b.people[&token].kind.as_deref(), Some("agent"));
}

#[test]
fn a_type_outside_the_grammar_is_refused_when_parsed() {
    assert_eq!(
        ObjectType::parse("Big Type").expect_err("refused").code,
        RefusalCode::TypeInvalid
    );
    assert_eq!(
        ObjectType::parse(&"a".repeat(41))
            .expect_err("refused")
            .code,
        RefusalCode::TypeInvalid
    );
    assert_eq!(
        ObjectType::parse("a-new-type").expect("kept"),
        ObjectType::Unknown("a-new-type".into())
    );
}

#[test]
fn revisions_ignore_position_and_see_every_field() {
    let text = "{\"frames\":[],\"objects\":[{\"id\":\"a\",\"later\":1,\"type\":\"sticky\"},{\"id\":\"b\",\"type\":\"text\"}],\"people\":{},\"settings\":{\"title\":\"T\"}}";
    let mut b = Board::from_json(text.as_bytes(), &Limits::V1).expect("board");
    let a = b.objects[0].rev().expect("rev");
    assert_eq!(
        a,
        Sha256::of(b"{\"id\":\"a\",\"later\":1,\"type\":\"sticky\"}"),
        "not the hash of the object's canonical JSON"
    );
    assert_eq!(
        b.objects[0].canonical_json().expect("json"),
        b"{\"id\":\"a\",\"later\":1,\"type\":\"sticky\"}"
    );
    assert_ne!(a, b.objects[1].rev().expect("rev"));
    b.objects.reverse();
    assert_eq!(
        b.objects[1].rev().expect("rev"),
        a,
        "position changed the rev"
    );
    b.objects[1].fields.insert("later".into(), Value::Num(2.0));
    assert_ne!(
        b.objects[1].rev().expect("rev"),
        a,
        "an unknown field didn't change the rev"
    );
    assert_eq!(
        b.settings_rev().expect("rev"),
        Sha256::of(b"{\"title\":\"T\"}")
    );
}

#[test]
fn open_checks_the_boards_hash_by_itself() {
    let board = "{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{}}";
    let mut file = forged_package(board);
    // One byte of board.json changed, with a CRC to match, and the manifest left as it was.
    let other = board.replace("frames", "framez");
    let (a, b) = (
        crc(board.as_bytes()).to_le_bytes(),
        crc(other.as_bytes()).to_le_bytes(),
    );
    let spots: Vec<usize> = (0..file.len() - 4)
        .filter(|i| file[*i..*i + 4] == a)
        .collect();
    assert_eq!(spots.len(), 2, "the CRC appears in two headers");
    for s in spots {
        file[s..s + 4].copy_from_slice(&b);
    }
    let at = file
        .windows(board.len())
        .position(|w| w == board.as_bytes())
        .expect("stored");
    file[at..at + board.len()].copy_from_slice(other.as_bytes());
    let r = Package::open(Cursor::new(file), &Limits::V1)
        .err()
        .expect("refused");
    assert_eq!(
        r.code,
        RefusalCode::PartHash,
        "open itself must check this, with no verify_parts"
    );
}

#[test]
fn a_raised_limit_is_refused_before_anything_is_read() {
    let mut raised = Limits::V1;
    raised.values += 1;
    let r = Package::open(Cursor::new(Vec::new()), &raised)
        .err()
        .expect("refused");
    assert_eq!(
        r.code,
        RefusalCode::LimitRaised,
        "the empty file was judged first"
    );
    let r = Board::empty().validate(&raised).expect_err("refused");
    assert_eq!(r.code, RefusalCode::LimitRaised);
}

#[test]
fn a_write_keeps_a_higher_min_reader() {
    let mut meta = WriteMeta::new(lineage());
    meta.min_reader = 5;
    let mut out = Vec::new();
    write(
        &mut out,
        &meta,
        &Board::empty(),
        &mut MemParts::new(),
        &Limits::V1,
    )
    .expect("write");
    let r = Package::open(Cursor::new(out), &Limits::V1)
        .err()
        .expect("this reader is too old for it");
    assert_eq!(r.code, RefusalCode::MinReaderTooHigh);
    assert_eq!(r.needs_reader, Some(5), "the file's min_reader came down");
}

fn many_bad(count: usize) -> String {
    let objects: Vec<String> = (0..count)
        .map(|i| format!("{{\"id\":\"Bad{i}\",\"type\":\"sticky\"}}"))
        .collect();
    format!(
        "{{\"frames\":[],\"objects\":[{}],\"people\":{{}},\"settings\":{{}}}}",
        objects.join(",")
    )
}

#[test]
fn a_refusal_counts_every_bad_id_past_the_cap() {
    let r = Board::from_json(many_bad(5000).as_bytes(), &Limits::V1).expect_err("refused");
    assert_eq!(r.code, RefusalCode::IdInvalid);
    assert_eq!(r.bad_ids_total, 5000, "the total stopped at the cap");
    assert_eq!(
        r.bad_ids.len(),
        schema::BAD_IDS_MAX,
        "the list isn't capped"
    );
    assert!(r.bad_ids_truncated, "no flag says the list stops short");
    assert_eq!(r.bad_ids[0], "Bad0");
    assert_eq!(r.bad_ids[999], "Bad999", "not the first in order");
}

#[test]
fn bad_ids_dont_depend_on_how_the_json_is_spelled() {
    let a = "{\"frames\":[\"F1\"],\"objects\":[{\"group\":\"G1\",\"id\":\"A1\",\"type\":\"sticky\"}],\"people\":{\"P1\":{}},\"settings\":{}}";
    let b = "{ \"settings\":{}, \"people\":{\"P1\":{}}, \"objects\":[{\"type\":\"sticky\",\"id\":\"A1\",\"group\":\"G1\"}], \"frames\":[\"F1\"] }";
    let (ra, rb) = (
        Board::from_json(a.as_bytes(), &Limits::V1).expect_err("refused"),
        Board::from_json(b.as_bytes(), &Limits::V1).expect_err("refused"),
    );
    assert_eq!(ra.bad_ids, vec!["F1", "G1", "A1", "P1"]);
    assert_eq!(ra.bad_ids, rb.bad_ids);
}

#[test]
fn a_package_and_a_board_in_memory_name_their_bad_ids() {
    let file = forged_package(&many_bad(3));
    let r = Package::open(Cursor::new(file), &Limits::V1)
        .err()
        .expect("refused");
    assert_eq!(
        r.bad_ids,
        vec!["Bad0", "Bad1", "Bad2"],
        "open dropped the list"
    );
    let mut b = Board::empty();
    let mut o = Object {
        id: Id::new("a").expect("id"),
        kind: ObjectType::Sticky,
        fields: Fields::new(),
    };
    o.fields.insert("group".into(), Value::Str("Team A".into()));
    o.fields.insert("frame".into(), Value::Str("Frm A".into()));
    b.objects.push(o);
    let r = b.validate(&Limits::V1).expect_err("refused");
    assert_eq!(r.code, RefusalCode::IdInvalid);
    assert_eq!(
        r.bad_ids,
        vec!["Frm A", "Team A"],
        "validate dropped the list"
    );
    assert_eq!(r.bad_ids_total, 2);
}

#[test]
fn other_refusals_name_no_ids() {
    let r = Board::from_json(b"[]", &Limits::V1).expect_err("refused");
    assert_eq!(r.code, RefusalCode::BoardShape);
    assert!(r.bad_ids.is_empty() && r.bad_ids_total == 0 && !r.bad_ids_truncated);
    // This one has a bad id too, and another fault is reached first.
    let text = "{\"frames\":[\"F 1\"],\"objects\":[1],\"people\":{},\"settings\":{}}";
    let r = Board::from_json(text.as_bytes(), &Limits::V1).expect_err("refused");
    assert_eq!(r.code, RefusalCode::BoardShape);
    assert!(
        r.bad_ids.is_empty(),
        "a refusal for something else named ids: {:?}",
        r.bad_ids
    );
}

#[test]
fn a_count_past_the_central_directory_is_refused_by_the_bound_that_checks_it() {
    // Both count fields say 3, the directory holds 2, so the third header would start at its end.
    let mut file = forged_package("{\"frames\":[],\"objects\":[],\"people\":{},\"settings\":{}}");
    let end = file.len() - 22;
    file[end + 8] = 3;
    file[end + 10] = 3;
    let result = std::panic::catch_unwind(|| {
        Package::open(Cursor::new(file), &Limits::V1)
            .err()
            .map(|r| r.code)
    });
    assert_eq!(
        result.ok(),
        Some(Some(RefusalCode::ZipMalformed)),
        "the decoder stopped without a refusal, or gave another one"
    );
}
