//! The reader on files built here (`k2_mpc_blend::testgen`, from a schema of ours): both header
//! forms, both pointer sizes and byte orders, the three ways a file is stored, the schema and
//! field lookup by name, pointer resolution where addresses repeat, what is kept and what is
//! digested, and every refusal by its type.

use k2_mpc_base::index::Digest;
use k2_mpc_blend::testgen::{gzip, zstd_frame, zstd_frames, zstd_window_frame, Body, FileBuilder};
use k2_mpc_blend::{read_blend, BlendFile, Compression, HeaderForm, ReadError, ReadLimits, Target};

const A: u64 = 0x1_0000_1000;
const B: u64 = 0x1_0000_2000;
const SHARED: u64 = 0x1_0000_3000;
const C: u64 = 0x1_0000_4000;

/// A small file: REND, an object `OBCube` with a DATA block of its own, a material `MAMat`
/// with its own DATA block at the same address, then GLOB at that address too.
fn file(form: HeaderForm, pointer_size: u8, big: bool) -> FileBuilder {
    let mut f = FileBuilder::new(
        form,
        pointer_size,
        big,
        if form == HeaderForm::Large { 501 } else { 403 },
    );
    f.schema.add_struct(
        "ID",
        &[
            ("void", "*next"),
            ("void", "*prev"),
            ("Library", "*lib"),
            ("char", "name[66]"),
            ("short", "flag"),
            ("int", "us"),
        ],
    );
    f.schema.add_struct(
        "Thing",
        &[
            ("ID", "id"),
            ("float", "loc[3]"),
            ("Link", "*other"),
            ("Link", "**arr"),
            ("int", "count"),
        ],
    );
    let ps = pointer_size;
    let id_name = f.schema.offset("ID", "name");
    let thing = |name: &str, loc: [f32; 3], other: u64, f: &FileBuilder| {
        let mut b = Body::new(f.schema.len_of("Thing"), big, ps);
        b.raw(f.schema.offset("Thing", "id") + id_name, name.as_bytes())
            .f32s(f.schema.offset("Thing", "loc"), &loc)
            .ptr(f.schema.offset("Thing", "other"), other)
            .i32(f.schema.offset("Thing", "count"), 3);
        b.bytes
    };
    let link = |next: u64, f: &FileBuilder| {
        let mut b = Body::new(f.schema.len_of("Link"), big, ps);
        b.ptr(0, next);
        b.bytes
    };
    f.raw(b"REND", 0, 0x10, 1, vec![1, 2, 3, 4]);
    let cube = thing("OBCube", [1.0, 2.0, 3.0], SHARED, &f);
    f.block(b"OB\0\0", "Thing", A, cube);
    let l = link(C, &f);
    f.block(b"DATA", "Link", SHARED, l);
    let l = link(0, &f);
    f.block(b"DATA", "Link", C, l);
    let mat = thing("MAMat", [0.5, 0.0, -1.0], SHARED, &f);
    f.block(b"MA\0\0", "Thing", B, mat);
    let l = link(A, &f);
    f.block(b"DATA", "Link", SHARED, l);
    f.raw(b"GLOB", 0, SHARED, 1, vec![9; 8]);
    f
}

fn read(bytes: &[u8]) -> Result<BlendFile, ReadError> {
    read_blend(&mut &bytes[..], &ReadLimits::default())
}

fn read_with(bytes: &[u8], limits: &ReadLimits) -> Result<BlendFile, ReadError> {
    read_blend(&mut &bytes[..], limits)
}

fn codes(f: &BlendFile) -> Vec<String> {
    f.blocks
        .iter()
        .map(|b| {
            String::from_utf8_lossy(&b.head.code)
                .trim_end_matches('\0')
                .to_owned()
        })
        .collect()
}

#[test]
fn a_classic_file_reads_block_by_block() {
    let built = file(HeaderForm::Classic, 8, false);
    let bytes = built.bytes();
    let f = read(&bytes).expect("check: the file reads");
    assert_eq!(f.header.form, HeaderForm::Classic);
    assert_eq!(
        (f.header.pointer_size, f.header.big_endian, f.header.version),
        (8, false, 403)
    );
    assert_eq!(f.compression, Compression::None);
    assert_eq!(
        f.raw_len,
        bytes.len() as u64,
        "read to the end block, header included"
    );
    assert_eq!(
        codes(&f),
        ["REND", "OB", "DATA", "DATA", "MA", "DATA", "GLOB", "DNA1", "ENDB"]
    );
    // owners: a data-block owns itself and the DATA after it; anything else resets
    let owners: Vec<Option<u32>> = f.blocks.iter().map(|b| b.owner).collect();
    assert_eq!(
        owners,
        [
            None,
            Some(1),
            Some(1),
            Some(1),
            Some(4),
            Some(4),
            None,
            None,
            None
        ]
    );
    // the schema, by name
    let thing = f
        .sdna
        .struct_index("Thing")
        .expect("Thing is in the schema");
    assert_eq!(f.blocks[1].head.sdna, thing);
    assert_eq!(f.sdna.struct_name(thing), Some("Thing"));
    let (name_at, name) = f.sdna.field_path(thing, "id.name").expect("id.name");
    assert_eq!((name_at, name.size, name.array_len), (24, 66, 66));
    let loc = f.sdna.field(thing, "loc").expect("loc");
    assert_eq!(
        (loc.offset, loc.size, f.sdna.type_name(loc.type_index)),
        (96, 12, "float")
    );
    let arr = f.sdna.field(thing, "arr").expect("arr");
    assert_eq!((arr.pointer_depth, arr.size), (2, 8));
    assert_eq!(
        f.sdna.field_any(thing, &["scale", "loc"]).map(|f| f.offset),
        Some(96)
    );
    assert!(f.sdna.field(thing, "nothing").is_none());
    // bodies are kept, at their offsets in the stream
    let body = f.body(1).expect("the object's body is kept");
    assert_eq!(&body[name_at as usize..name_at as usize + 6], b"OBCube");
    let off = f.blocks[1].head.body_offset as usize;
    assert_eq!(
        &bytes[off..off + body.len()],
        body,
        "the body offset is where the body is"
    );
    let e = f.endian();
    assert_eq!(e.f32(body, 96 + 4), Some(2.0));
    assert_eq!(
        e.i32(body, f.sdna.field(thing, "count").unwrap().offset as usize),
        Some(3)
    );
}

#[test]
fn every_form_reads_to_the_same_blocks() {
    let base = read(&file(HeaderForm::Classic, 8, false).bytes()).unwrap();
    for (form, ps, big) in [
        (HeaderForm::Classic, 4, false),
        (HeaderForm::Classic, 4, true),
        (HeaderForm::Classic, 8, true),
        (HeaderForm::Large, 8, false),
    ] {
        let f = read(&file(form, ps, big).bytes())
            .unwrap_or_else(|e| panic!("check: {form:?} {ps} {big} reads: {e}"));
        assert_eq!(
            (f.header.form, f.header.pointer_size, f.header.big_endian),
            (form, ps, big)
        );
        assert_eq!(codes(&f), codes(&base), "{form:?} {ps} {big}");
        let thing = f.sdna.struct_index("Thing").unwrap();
        let loc = f.sdna.field(thing, "loc").unwrap().offset as usize;
        let body = f.body(1).unwrap();
        let e = f.endian();
        assert_eq!(
            (e.f32(body, loc), e.f32(body, loc + 4), e.f32(body, loc + 8)),
            (Some(1.0), Some(2.0), Some(3.0)),
            "{form:?} {ps} {big}: numbers read in the file's byte order"
        );
        let other = f.sdna.field(thing, "other").unwrap().offset as usize;
        let p = e.ptr(body, other).unwrap();
        assert_eq!(
            p,
            if ps == 4 {
                SHARED as u32 as u64
            } else {
                SHARED
            }
        );
        let owners: Vec<Option<u32>> = f.blocks.iter().map(|b| b.owner).collect();
        let base_owners: Vec<Option<u32>> = base.blocks.iter().map(|b| b.owner).collect();
        assert_eq!(owners, base_owners);
    }
}

#[test]
fn compressed_files_read_as_the_plain_one() {
    for form in [HeaderForm::Classic, HeaderForm::Large] {
        let plain = file(form, 8, false).bytes();
        let want = read(&plain).unwrap();
        let mut stored = vec![(Compression::Gzip, gzip(&plain))];
        for frame in [1, 7, 333, 4096, plain.len()] {
            stored.push((Compression::Zstd, zstd_frames(&plain, frame)));
        }
        for (how, bytes) in stored {
            let f = read(&bytes).unwrap_or_else(|e| panic!("check: {how:?} reads: {e}"));
            assert_eq!(f.compression, how);
            assert_eq!(f.blocks, want.blocks, "{how:?}: the same blocks");
            assert_eq!(f.raw_len, want.raw_len);
            for i in 0..f.blocks.len() as u32 {
                assert_eq!(f.body(i), want.body(i), "{how:?}: block {i}'s body");
            }
        }
    }
}

#[test]
fn a_zstd_stream_is_refused_where_it_breaks() {
    let plain = file(HeaderForm::Classic, 8, false).bytes();
    let limits = ReadLimits::default();
    // a window over the limit, before anything is decoded
    let over = (limits.max_window_bytes.trailing_zeros() - 10 + 1) as u8;
    match read(&zstd_window_frame(over, false)) {
        Err(ReadError::Limit { what, limit }) => {
            assert_eq!(
                (what, limit),
                ("a zstd frame's window", limits.max_window_bytes)
            )
        }
        other => panic!("{other:?}"),
    }
    // a frame that needs a dictionary
    match read(&zstd_window_frame(4, true)) {
        Err(ReadError::Malformed { what }) => assert_eq!(what, "a zstd frame needs a dictionary"),
        other => panic!("{other:?}"),
    }
    // bytes after the last frame that are no frame
    let mut tail = zstd_frames(&plain, 4096);
    tail.extend(b"junk");
    match read(&tail) {
        Err(ReadError::Malformed { what }) => assert_eq!(what, "bytes after the last zstd frame"),
        other => panic!("{other:?}"),
    }
    // a frame cut short: decoding fails, never a short read taken as the end
    let mut cut = Vec::new();
    zstd_frame(&mut cut, &plain);
    cut.truncate(cut.len() - 10);
    assert!(
        matches!(
            read(&cut),
            Err(ReadError::Malformed { .. } | ReadError::Truncated { .. })
        ),
        "{:?}",
        read(&cut).err()
    );
    // a skippable frame alone: no file in it
    let mut only_skip = 0x184D_2A50u32.to_le_bytes().to_vec();
    only_skip.extend(0u32.to_le_bytes());
    match read(&only_skip) {
        Err(ReadError::Malformed { what }) => assert_eq!(what, "the zstd stream has no frame"),
        other => panic!("{other:?}"),
    }
    // a zstd stream that isn't a BLEND file inside
    match read(&zstd_frames(b"not a blend file at all", 8)) {
        Err(ReadError::NotBlend) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_damaged_gzip_stream_is_malformed_not_an_io_failure() {
    let mut g = gzip(&file(HeaderForm::Classic, 8, false).bytes());
    let mid = g.len() / 2;
    for b in &mut g[mid..mid + 16] {
        *b ^= 0x5a;
    }
    match read(&g) {
        Err(ReadError::Io(e)) => panic!("a damaged stream is not an I/O failure: {e}"),
        Err(_) => {}
        Ok(_) => panic!("a damaged gzip stream read"),
    }
}

#[test]
fn bytes_after_the_end_block_are_refused() {
    let mut bytes = file(HeaderForm::Classic, 8, false).bytes();
    bytes.push(0);
    assert_eq!(malformed(&bytes), "bytes after the end block");
    // an end block with a body
    let mut f = file(HeaderForm::Classic, 8, false);
    f.no_end = true;
    f.raw(b"ENDB", 0, 0, 0, vec![1, 2]);
    // the schema comes after the blocks: put the end block last by hand
    let mut bytes = f.bytes();
    let endb = bytes.windows(4).rposition(|w| w == b"ENDB").unwrap();
    let dna = bytes.windows(4).rposition(|w| w == b"DNA1").unwrap();
    assert!(endb < dna);
    let tail = bytes.split_off(dna);
    let mut moved = bytes[..endb].to_vec();
    moved.extend(tail);
    moved.extend(&bytes[endb..]);
    assert_eq!(malformed(&moved), "bytes after the end block");
}

#[test]
fn pointers_resolve_by_owner_and_never_guess() {
    let f = read(&file(HeaderForm::Classic, 8, false).bytes()).unwrap();
    // blocks: 0 REND, 1 OB (A), 2 DATA (SHARED, owner 1), 3 DATA (C, owner 1),
    // 4 MA (B), 5 DATA (SHARED, owner 4), 6 GLOB (SHARED)
    assert_eq!(f.resolve(0, 1), Target::Null);
    assert_eq!(f.resolve(A, 5), Target::Block(1), "a unique address");
    assert_eq!(f.resolve(C, 2), Target::Block(3));
    assert_eq!(f.resolve(0xdead_0000, 1), Target::Unresolved);
    assert_eq!(
        f.resolve(SHARED, 1),
        Target::Block(2),
        "the object's own DATA block"
    );
    assert_eq!(
        f.resolve(SHARED, 4),
        Target::Block(5),
        "the material's own DATA block"
    );
    assert_eq!(f.resolve(SHARED, 5), Target::Block(5));
    assert_eq!(
        f.resolve(SHARED, 0),
        Target::Ambiguous,
        "no owner, no data-block among them"
    );
    assert_eq!(f.shared_addresses(), 3);
}

#[test]
fn large_bodies_are_digested_and_dropped() {
    let mut built = file(HeaderForm::Classic, 8, false);
    let big: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
    built.raw(b"DATA", 0, 0x2_0000_0000, 1, big.clone());
    built.raw(b"DATA", 0, 0x2_0000_1000, 1, vec![5; 1000]);
    built.raw(b"DATA", 0, 0x2_0000_2000, 1, vec![6; 1000]);
    let bytes = built.bytes();
    // a block over the per-block limit
    let f = read(&bytes).unwrap();
    let i = f
        .blocks
        .iter()
        .position(|b| b.head.old == 0x2_0000_0000)
        .unwrap() as u32;
    assert!(
        f.body(i).is_none(),
        "a 100 kB block isn't kept under a 64 KiB limit"
    );
    assert_eq!(f.body_digest(i), Some(&Digest::of(&big)));
    assert!(f.body(i + 1).is_some() && f.body_digest(i + 1).is_none());
    // a small block over the budget: digested too; data-blocks always kept
    let thing_len = f.blocks[1].head.len;
    let kept_before: u64 = f.blocks[..i as usize]
        .iter()
        .filter(|b| &b.head.code != b"GLOB" && &b.head.code != b"REND")
        .map(|b| b.head.len)
        .sum();
    let tight = ReadLimits::new(
        1 << 30,
        1000,
        kept_before + 1000,
        64 << 10,
        1 << 20,
        1 << 20,
    );
    let f = read_with(&bytes, &tight).unwrap();
    assert!(f.body(i + 1).is_some(), "the first 1000 bytes fit");
    assert!(f.body(i + 2).is_none(), "the next 1000 don't");
    assert_eq!(f.body_digest(i + 2), Some(&Digest::of(&[6; 1000])));
    // a data-block that doesn't fit is refused, never read in part
    let none = ReadLimits::new(1 << 30, 1000, thing_len - 1, 64 << 10, 1 << 20, 1 << 20);
    match read_with(&bytes, &none) {
        Err(ReadError::Limit { what, .. }) => assert_eq!(what, "bytes kept of data-blocks"),
        other => panic!("{other:?}"),
    }
}

fn limit_of(bytes: &[u8], limits: ReadLimits) -> &'static str {
    match read_with(bytes, &limits) {
        Err(ReadError::Limit { what, .. }) => what,
        other => panic!("{other:?}"),
    }
}

#[test]
fn each_limit_refuses_by_name() {
    let bytes = file(HeaderForm::Classic, 8, false).bytes();
    let d = ReadLimits::default();
    let with = |f: &dyn Fn(&mut ReadLimits)| {
        let mut l = d;
        f(&mut l);
        l
    };
    assert_eq!(limit_of(&bytes, with(&|l| l.max_blocks = 3)), "blocks");
    assert_eq!(
        limit_of(&bytes, with(&|l| l.max_sdna_bytes = 16)),
        "the schema block"
    );
    assert_eq!(
        limit_of(&bytes, with(&|l| l.max_raw_bytes = bytes.len() as u64 - 1)),
        "bytes after decompression"
    );
    // exactly at each limit reads
    let blocks = read(&bytes).unwrap().blocks.len() as u64;
    read_with(
        &bytes,
        &with(&|l| {
            l.max_blocks = blocks;
            l.max_raw_bytes = bytes.len() as u64;
        }),
    )
    .expect("check: a file exactly at the limits reads");
}

fn malformed(bytes: &[u8]) -> &'static str {
    match read(bytes) {
        Err(ReadError::Malformed { what }) => what,
        other => panic!("{other:?}"),
    }
}

fn truncated(bytes: &[u8]) -> &'static str {
    match read(bytes) {
        Err(ReadError::Truncated { what }) => what,
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_broken_file_is_refused_by_type() {
    let good = file(HeaderForm::Classic, 8, false);
    let bytes = good.bytes();
    // not a BLEND file at all
    for not in [&b""[..], b"BLEN", b"PK\x03\x04 a zip", &[0u8; 64]] {
        assert!(matches!(read(not), Err(ReadError::NotBlend)), "{not:?}");
    }
    // an unknown header form
    let mut unknown = bytes.clone();
    unknown[8] = b'x';
    assert!(matches!(
        read(&unknown),
        Err(ReadError::Unknown { what: "byte order" })
    ));
    // cut short: in the header, in a block header, in a body, before the end block
    assert_eq!(truncated(&bytes[..10]), "the file header");
    assert_eq!(truncated(&bytes[..12 + 10]), "a block header");
    assert_eq!(truncated(&bytes[..12 + 24 + 2]), "a block's body");
    let mut no_end = good.clone();
    no_end.no_end = true;
    assert_eq!(
        truncated(&no_end.bytes()),
        "the file ends before its end block"
    );
    // no schema, or two
    let mut no_schema = good.clone();
    no_schema.no_schema = true;
    assert_eq!(
        malformed(&no_schema.bytes()),
        "the file has no schema block"
    );
    let mut two = good.clone();
    let dna = good.schema.encode(false);
    two.raw(b"DNA1", 0, 0, 1, dna);
    assert_eq!(malformed(&two.bytes()), "the file has two schema blocks");
    // a negative length in the classic form
    let mut neg = bytes.clone();
    neg[12 + 4..12 + 8].copy_from_slice(&(-5i32).to_le_bytes());
    assert_eq!(malformed(&neg), "a block header holds a negative number");
    // ...and in the newer one
    let large = file(HeaderForm::Large, 8, false).bytes();
    let mut neg = large.clone();
    neg[17 + 16..17 + 24].copy_from_slice(&(-1i64).to_le_bytes());
    assert_eq!(malformed(&neg), "a block header holds a negative number");
}

/// A file whose schema is `edit`ed after the usual one is built.
fn with_schema(edit: impl FnOnce(&mut FileBuilder)) -> Vec<u8> {
    let mut f = file(HeaderForm::Classic, 8, false);
    edit(&mut f);
    f.bytes()
}

/// The schema block's bytes replaced by `body`.
fn with_schema_bytes(body: Vec<u8>) -> Vec<u8> {
    let mut f = file(HeaderForm::Classic, 8, false);
    f.no_schema = true;
    f.raw(b"DNA1", 0, 0, 1, body);
    f.bytes()
}

#[test]
fn a_schema_that_breaks_its_own_rules_is_refused() {
    // fields that don't add up to the struct's length
    let bad_len = with_schema(|f| {
        f.schema
            .add_struct_raw("Odd", 7, &[("int", "a"), ("short", "b")]);
    });
    assert_eq!(
        malformed(&bad_len),
        "a struct's fields don't add up to its length"
    );
    // a struct embedding itself (with a length that adds up)
    let selfish = with_schema(|f| {
        f.schema
            .add_struct_raw("Selfish", 0, &[("Selfish", "inner")]);
    });
    assert_eq!(malformed(&selfish), "a struct embeds itself");
    // two structs embedding each other
    let pair = with_schema(|f| {
        f.schema.add_type("PairB", 0);
        f.schema.add_struct_raw("PairA", 0, &[("PairB", "b")]);
        f.schema.add_struct_raw("PairB", 0, &[("PairA", "a")]);
    });
    assert_eq!(malformed(&pair), "a struct embeds itself");
    // a field name that doesn't parse
    let bad_name = with_schema(|f| {
        f.schema.add_struct_raw("Named", 4, &[("int", "a[x]")]);
    });
    assert_eq!(malformed(&bad_name), "a field name doesn't parse");
    // a section missing, a count past the end, an index out of range
    let good = file(HeaderForm::Classic, 8, false).schema.encode(false);
    let mut no_tag = good.clone();
    no_tag[4..8].copy_from_slice(b"NAMX");
    assert_eq!(
        malformed(&with_schema_bytes(no_tag)),
        "the schema block lacks a section"
    );
    let mut huge = good.clone();
    huge[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        malformed(&with_schema_bytes(huge)),
        "a schema count is past the block's end"
    );
    let mut cut = good.clone();
    cut.truncate(cut.len() - 3);
    assert_eq!(
        malformed(&with_schema_bytes(cut)),
        "the schema block is cut short"
    );
    let strc = good.windows(4).position(|w| w == b"STRC").unwrap();
    let mut out_of_range = good.clone();
    // the first struct's type index
    out_of_range[strc + 8..strc + 10].copy_from_slice(&u16::MAX.to_le_bytes());
    assert_eq!(
        malformed(&with_schema_bytes(out_of_range)),
        "a struct's type is out of range"
    );
    let mut not_utf8 = good.clone();
    not_utf8[16] = 0xff;
    assert_eq!(
        malformed(&with_schema_bytes(not_utf8)),
        "a schema name isn't UTF-8"
    );
}

#[test]
fn deep_embedding_is_bounded() {
    let deep = with_schema(|f| {
        f.schema.add_struct("Deep0", &[("int", "x")]);
        for i in 1..40 {
            let prev = format!("Deep{}", i - 1);
            f.schema
                .add_struct(&format!("Deep{i}"), &[(&prev, "inner")]);
        }
    });
    assert_eq!(malformed(&deep), "structs nest too deep");
    let ok = with_schema(|f| {
        f.schema.add_struct("Deep0", &[("int", "x")]);
        for i in 1..20 {
            let prev = format!("Deep{}", i - 1);
            f.schema
                .add_struct(&format!("Deep{i}"), &[(&prev, "inner")]);
        }
    });
    let f = read(&ok).expect("check: 20 levels read");
    let d = f.sdna.struct_index("Deep19").unwrap();
    let path = vec!["inner"; 19].join(".") + ".x";
    assert_eq!(f.sdna.field_path(d, &path).map(|(o, _)| o), Some(0));
}
