//! The writer on files built here: each fixed-size change patches only its own field (every
//! other byte, and every block, exactly as it was), reads back, and survives every header form
//! and both compressed storages; and each refusal by its type, with nothing written in part.

use k2_mpc_blend::testgen::{gzip, zstd_frames, ObjectSpec, SceneBuilder};
use k2_mpc_blend::write::{decompress, WRITE_TO};
use k2_mpc_blend::{
    read_blend, read_scene, write, Change, Compression, Edit, HeaderForm, ReadLimits, SceneModel,
    WriteError, Written,
};

fn build(form: HeaderForm, ps: u8, big: bool, version: u16) -> Vec<u8> {
    let mut b = SceneBuilder::new(form, ps, big, version, false);
    b.library("lib", "//lib.blend");
    b.scene("Scene", &["Probe", "Other", "Quat"], &[], None);
    b.object(
        "Probe",
        &ObjectSpec {
            loc: [1.0, 2.0, 3.0],
            restrict: 1024,
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.object(
        "Other",
        &ObjectSpec {
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.object("Quat", &ObjectSpec::default());
    b.object(
        "Far",
        &ObjectSpec {
            lib: Some("lib".into()),
            ..ObjectSpec::default()
        },
    );
    b.material("Steel", [0.5; 3], None);
    b.bytes()
}

fn plain() -> Vec<u8> {
    build(HeaderForm::Classic, 8, false, 403)
}

fn limits() -> ReadLimits {
    ReadLimits::default()
}

fn model(bytes: &[u8]) -> SceneModel {
    let (_, raw) = decompress(bytes, &limits()).expect("check: decompresses");
    let f = read_blend(&mut &raw[..], &limits()).expect("check: reads");
    read_scene(&f).expect("check: models")
}

fn field(m: &SceneModel, key: &str, name: &str) -> Option<String> {
    let id = m.by_key(key).unwrap_or_else(|| panic!("no {key}"));
    id.fields
        .iter()
        .chain(&id.named)
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

fn ok(bytes: &[u8], edits: &[Edit]) -> Written {
    write(bytes, edits, &limits()).unwrap_or_else(|e| panic!("check: writes: {e}"))
}

fn err(bytes: &[u8], edits: &[Edit]) -> WriteError {
    match write(bytes, edits, &limits()) {
        Err(e) => e,
        Ok(_) => panic!("written"),
    }
}

/// The positions where `a` and `b` differ.
fn diff(a: &[u8], b: &[u8]) -> Vec<usize> {
    assert_eq!(a.len(), b.len(), "same length");
    (0..a.len()).filter(|&i| a[i] != b[i]).collect()
}

fn vis(viewport: Option<bool>, select: Option<bool>, render: Option<bool>) -> Change {
    Change::Visibility {
        viewport,
        select,
        render,
    }
}

#[test]
fn each_change_patches_only_its_field() {
    let input = plain();
    let cases: Vec<(Change, &str, &str)> = vec![
        (Change::Location([4.0, -5.5, 6.25]), "loc", "4,-5.5,6.25"),
        (Change::Rotation([0.5, 0.0, -1.0]), "rot", "0.5,0,-1"),
        (Change::Scale([2.0, 2.0, 0.5]), "size", "2,2,0.5"),
        (vis(Some(true), None, None), "restrictflag", "1025"),
        (vis(None, Some(true), Some(true)), "restrictflag", "1030"),
        (
            vis(Some(false), Some(false), Some(false)),
            "restrictflag",
            "1024",
        ),
    ];
    for (change, name, want) in cases {
        let w = ok(&input, &[Edit::new("@ob:Probe", change.clone())]);
        assert_eq!(w.compression, Compression::None);
        assert_eq!(w.patched.len(), 1);
        let span = &w.patched[0];
        assert_eq!(span.field, name);
        let changed = diff(&input, &w.bytes);
        // clearing bits that aren't set changes nothing
        assert_eq!(changed.is_empty(), want == "1024", "{change:?}");
        assert!(
            changed.iter().all(|&i| (span.offset as usize
                ..(span.offset + u64::from(span.len)) as usize)
                .contains(&i)),
            "{change:?}: only the field's bytes change"
        );
        let m = model(&w.bytes);
        assert_eq!(
            field(&m, "@ob:Probe", name).as_deref(),
            Some(want),
            "{change:?}"
        );
        // every other object as it was
        let before = model(&input);
        for k in ["@ob:Other", "@ob:Quat", "@ma:Steel"] {
            assert_eq!(m.by_key(k), before.by_key(k), "{k}");
        }
    }
    // the named bits read back too
    let w = ok(
        &input,
        &[Edit::new("@ob:Probe", vis(Some(true), None, Some(true)))],
    );
    let m = model(&w.bytes);
    assert_eq!(
        field(&m, "@ob:Probe", "hide_viewport").as_deref(),
        Some("true")
    );
    assert_eq!(
        field(&m, "@ob:Probe", "hide_render").as_deref(),
        Some("true")
    );
    assert_eq!(
        field(&m, "@ob:Probe", "hide_select").as_deref(),
        Some("false")
    );
    assert_eq!(
        field(&m, "@ob:Probe", "other_bits").as_deref(),
        Some("1024")
    );
    assert_eq!(m.by_key("@ob:Probe").unwrap().hidden, Some(true));
}

#[test]
fn a_rename_writes_the_name_field_and_keeps_its_length() {
    let input = plain();
    let w = ok(
        &input,
        &[Edit::new("@ob:Probe", Change::Name("Sensor 2".into()))],
    );
    let m = model(&w.bytes);
    assert!(m.by_key("@ob:Probe").is_none());
    let s = m.by_key("@ob:Sensor%202").expect("the new key");
    assert_eq!(s.name, "Sensor 2");
    let span = &w.patched[0];
    assert_eq!(span.field, "name");
    let at = span.offset as usize;
    let field = &w.bytes[at..at + span.len as usize];
    assert_eq!(&field[..11], b"OBSensor 2\0");
    assert!(
        field[11..].iter().all(|&b| b == 0),
        "the rest of the field zeroed"
    );
    assert_eq!(field.len(), 66, "the field's length kept");
    // its own name again: allowed, and a no-op
    let same = ok(
        &input,
        &[Edit::new("@ob:Probe", Change::Name("Probe".into()))],
    );
    assert_eq!(same.bytes, input);
    // the longest name that fits: 63 bytes
    let long = "x".repeat(63);
    let w = ok(
        &input,
        &[Edit::new("@ob:Probe", Change::Name(long.clone()))],
    );
    assert_eq!(
        model(&w.bytes)
            .ids
            .iter()
            .filter(|i| i.name == long)
            .count(),
        1
    );
}

#[test]
fn a_list_of_edits_names_objects_as_the_input_has_them() {
    let input = plain();
    let w = ok(
        &input,
        &[
            Edit::new("@ob:Probe", Change::Name("Moved".into())),
            Edit::new("@ob:Probe", Change::Location([9.0, 9.0, 9.0])),
            Edit::new("@ob:Other", Change::Name("Probe".into())),
            Edit::new("@ob:Probe", Change::Location([7.0, 8.0, 9.0])),
            Edit::new("@ob:Other", vis(Some(true), None, None)),
        ],
    );
    let m = model(&w.bytes);
    assert_eq!(
        field(&m, "@ob:Moved", "loc").as_deref(),
        Some("7,8,9"),
        "the last value wins"
    );
    assert_eq!(
        field(&m, "@ob:Probe", "hide_viewport").as_deref(),
        Some("true")
    );
    assert_eq!(
        field(&m, "@ob:Probe", "loc").as_deref(),
        Some("0,0,0"),
        "Other's own location"
    );
    // a name freed earlier in the list may be taken
    assert_eq!(w.patched.len(), 4, "names, Probe's location, Other's flag");
}

#[test]
fn every_form_and_both_compressions_write() {
    for (form, ps, big, version) in [
        (HeaderForm::Classic, 8, false, 306),
        (HeaderForm::Classic, 4, true, 405),
        (HeaderForm::Large, 8, false, 501),
    ] {
        let input = build(form, ps, big, version);
        let edits = [
            Edit::new("@ob:Probe", Change::Location([1.5, 2.5, 3.5])),
            Edit::new("@ob:Probe", Change::Name("P".into())),
            Edit::new("@ob:Probe", vis(None, Some(true), None)),
        ];
        let w = ok(&input, &edits);
        let m = model(&w.bytes);
        assert_eq!(
            field(&m, "@ob:P", "loc").as_deref(),
            Some("1.5,2.5,3.5"),
            "{form:?} {ps} {big}"
        );
        assert_eq!(field(&m, "@ob:P", "hide_select").as_deref(), Some("true"));
        for (how, stored) in [
            (Compression::Gzip, gzip(&input)),
            (Compression::Zstd, zstd_frames(&input, 4000)),
        ] {
            let c = ok(&stored, &edits);
            assert_eq!(c.compression, how);
            let (h, raw) = decompress(&c.bytes, &limits()).unwrap();
            assert_eq!(h, how);
            assert_eq!(raw, w.bytes, "{how:?}: the same file inside");
        }
    }
}

#[test]
fn the_writer_is_a_pure_function() {
    let input = plain();
    let edits = [
        Edit::new("@ob:Probe", Change::Scale([3.0, 3.0, 3.0])),
        Edit::new("@ob:Other", Change::Rotation([0.1, 0.2, 0.3])),
    ];
    assert_eq!(ok(&input, &edits), ok(&input, &edits));
    let z = zstd_frames(&input, 999);
    assert_eq!(ok(&z, &edits), ok(&z, &edits));
}

#[test]
fn each_refusal_is_typed_and_writes_nothing() {
    let input = plain();
    let one = |e: Edit| err(&input, &[e]);
    assert!(matches!(
        one(Edit::new("@ob:Ghost", Change::Location([0.0; 3]))),
        WriteError::NotFound(k) if k == "@ob:Ghost"
    ));
    assert!(matches!(
        one(Edit::new("@ma:Steel", Change::Location([0.0; 3]))),
        WriteError::NotAnObject(_)
    ));
    assert!(matches!(
        one(Edit::new("@li:lib/ob:Far", Change::Location([0.0; 3]))),
        WriteError::Linked(_)
    ));
    match one(Edit::new("@ob:Quat", Change::Rotation([1.0, 0.0, 0.0]))) {
        WriteError::RotationMode { mode } => assert_eq!(mode, "quaternion"),
        other => panic!("{other:?}"),
    }
    assert!(matches!(
        one(Edit::new(
            "@ob:Probe",
            Change::Location([f32::NAN, 0.0, 0.0])
        )),
        WriteError::Value
    ));
    assert!(matches!(
        one(Edit::new(
            "@ob:Probe",
            Change::Scale([f32::INFINITY, 1.0, 1.0])
        )),
        WriteError::Value
    ));
    for (name, why) in [
        ("", "it is empty"),
        ("a\0b", "it holds a NUL"),
        (&"x".repeat(64) as &str, "it doesn't fit the name field"),
        ("Other", "another object has it"),
    ] {
        match one(Edit::new("@ob:Probe", Change::Name(name.into()))) {
            WriteError::Name { why: w } => assert_eq!(w, why, "{name:?}"),
            other => panic!("{name:?}: {other:?}"),
        }
    }
    // nothing written in part: a good edit, then a bad one
    assert!(matches!(
        err(
            &input,
            &[
                Edit::new("@ob:Probe", Change::Location([1.0; 3])),
                Edit::new("@ob:Ghost", Change::Location([1.0; 3])),
            ]
        ),
        WriteError::NotFound(_)
    ));
    // versions outside the written ones
    for version in [305, WRITE_TO + 1, 273] {
        let old = build(HeaderForm::Classic, 8, false, version);
        match err(&old, &[Edit::new("@ob:Probe", Change::Location([1.0; 3]))]) {
            WriteError::Version { version: v } => assert_eq!(v, version),
            other => panic!("{other:?}"),
        }
    }
    // a field the file's schema doesn't have: `loc` renamed in the schema
    let mut no_loc = input.clone();
    let at = no_loc
        .windows(7)
        .position(|w| w == b"loc[3]\0")
        .expect("the field name in the schema");
    no_loc[at + 2] = b'x';
    match err(
        &no_loc,
        &[Edit::new("@ob:Probe", Change::Location([1.0; 3]))],
    ) {
        WriteError::Field { field } => assert_eq!(field, "loc"),
        other => panic!("{other:?}"),
    }
    // ...and the other fields still write
    ok(&no_loc, &[Edit::new("@ob:Probe", Change::Scale([1.0; 3]))]);
    // a file that isn't one
    assert!(matches!(
        err(
            b"not a scene",
            &[Edit::new("@ob:Probe", Change::Location([1.0; 3]))]
        ),
        WriteError::Read(_)
    ));
}
