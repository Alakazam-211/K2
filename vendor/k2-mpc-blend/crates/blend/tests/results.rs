//! What an agent gets back from each verb as data, and from each refusal, pinned whole as golden
//! files in `tests/results_golden/` (the kit's pretty JSON; re-bless with `MPC_BLESS_GOLDEN=1`,
//! `kit_view_golden` at its top). Every scene is generated here (`k2_mpc_blend::testgen`).
//!
//! - **Live verbs:** the call's result (`mpc.result/1`, the kit's, asked for with `--json`) of
//!   every verb on the outliner scene, a list's first page, and a refusal of each code these
//!   calls can reach. Each call runs on a fresh fixture, so a golden doesn't depend on another
//!   call. Each result's `data` is the verb's own (`data_schema` `typed`, `data_spec`
//!   `blend.<verb>/1`); each refusal's `error` is its entry, by id, and names no shape.
//! - **File verbs:** each verb's answer as data (`verbs::Typed`, `data_spec`
//!   `blend.file.<verb>/1`) and each refusal's report (`VerbError::report`), as a host builds
//!   its result from them.
//! - **The live tier's decline** of a file from a release outside the checked ones: its code,
//!   its message, the kit's report and the pack's entry.
//!
//! Beside the goldens: every refusal code the verbs print is one of the pack's registered
//! entries, every writer, reader and mesh-read refusal maps to an entry with the verb's own exit,
//! the entries' texts name the releases the writer checks, and the kit's registry takes the pack.

// only `check_in` is used here
#[allow(dead_code)]
mod kit_view_golden;
mod live_support;
mod support;

use std::collections::BTreeSet;

use k2_mpc_base::errors::{ErrorEntry, ErrorReport, Refusal, Value as Json};
use k2_mpc_base::parity::Registry;
use k2_mpc_base::session::{ConnId, Pack, Session};
use k2_mpc_blend::live::errors::{self, by_code, checked_releases, ERRORS};
use k2_mpc_blend::live::{verbs as live_verbs, BlendLive};
use k2_mpc_blend::meshread::{MeshReadError, MeshReadLimits};
use k2_mpc_blend::testgen::{CameraSpec, MeshLayouts, MeshSpec, ObjectSpec, SceneBuilder};
use k2_mpc_blend::verbs::{
    apply, list, meshes, meshes_typed, Typed, Verb, VerbError, DATA_VERSION, VERBS,
};
use k2_mpc_blend::write::{WRITE_FROM, WRITE_TO};
use k2_mpc_blend::{HeaderForm, ReadError, ReadLimits, WriteError};
use live_support::{content, fx, obj_named, rows, run, run_based, scene};
use support::{studio, Studio};

/// The golden folder.
const DIR: &str = "results_golden";

/// The live reads, by golden name; `{Name}` is the address of the object named `Name`.
const READS: &[(&str, &[&str])] = &[
    ("scene-list-page", &["scene", "list", "--limit", "2"]),
    (
        "collection-show-page",
        &["collection", "show", "@coll:2", "--limit", "2"],
    ),
    ("object-show", &["object", "show", "{Cube}"]),
];

/// The live edits, by golden name, each on its own base.
const EDITS: &[(&str, &[&str])] = &[
    (
        "object-place",
        &[
            "object", "place", "{Cube}", "--loc", "1,2,3", "--scale", "2,2,2",
        ],
    ),
    (
        "object-visibility",
        &["object", "visibility", "{Lamp}", "--render", "off"],
    ),
    (
        "object-rename",
        &[
            "object",
            "rename",
            "{Cube}",
            "--name",
            "Box",
            "--break-links",
        ],
    ),
];

/// The live refusals, by golden name, with the code each must give.
const REFUSALS: &[(&str, &str, &[&str])] = &[
    (
        "refused-not-editable",
        "blend_not_editable",
        &["object", "place", "{Far}", "--loc", "1,0,0"],
    ),
    (
        "refused-driven",
        "blend_driven",
        &["object", "place", "{Walker}", "--loc", "1,0,0"],
    ),
    (
        "refused-rotation-mode",
        "blend_rotation_mode",
        &["object", "place", "{Spin}", "--rot", "0,0,1"],
    ),
    (
        "refused-name",
        "blend_name",
        &[
            "object",
            "rename",
            "{Cube}",
            "--name",
            "Lamp",
            "--break-links",
        ],
    ),
    (
        "refused-breaks-links",
        "blend_breaks_links",
        &["object", "rename", "{Cube}", "--name", "Box"],
    ),
    (
        "refused-not-found",
        "blend_not_found",
        &["object", "show", "@obj:999"],
    ),
];

/// A field of a JSON object.
fn get<'a>(v: &'a Json, k: &str) -> Option<&'a Json> {
    v.get(k)
}

fn text(v: Option<&Json>) -> Option<&str> {
    v.and_then(Json::as_str)
}

/// `argv` with each `{Name}` the address of the live object named `Name`.
fn resolve(s: &Session, c: ConnId, argv: &[&str]) -> Vec<String> {
    let doc = content(s, c);
    argv.iter()
        .map(
            |a| match a.strip_prefix('{').and_then(|x| x.strip_suffix('}')) {
                Some(name) => {
                    let k = obj_named(&doc, name).unwrap_or_else(|| panic!("no object {name}"));
                    format!("@obj:{}", &k[1..])
                }
                None => (*a).to_owned(),
            },
        )
        .collect()
}

/// `argv` with `--json`, as an agent on a fresh fixture of the outliner scene (an edit on its
/// own base): the call's result.
fn result_of(tag: &str, argv: &[&str], edit: bool) -> Json {
    let f = fx(tag, |_| {});
    f.write(&scene());
    let s = f.open();
    let a = f.agent(&s);
    let mut all = resolve(&s, a, argv);
    all.push("--json".to_owned());
    let all: Vec<&str> = all.iter().map(String::as_str).collect();
    let out = if edit {
        run_based(&s, a, &all)
    } else {
        run(&s, a, &all)
    }
    .unwrap_or_else(|e| panic!("{argv:?}: the call runs: {e:?}"));
    out.result()
        .unwrap_or_else(|| panic!("{argv:?}: --json gives a result"))
        .clone()
}

/// The shape's name a live verb's data must have: `blend.<verb>/1`, its words joined by `-`.
fn live_spec(argv: &[&str]) -> String {
    format!("blend.{}/{}", argv[..2].join("-"), live_verbs::DATA_VERSION)
}

/// A shape's name as the kit takes it (`StructOutput::data_spec`): 1 to 64 bytes of
/// `[a-z0-9._-]`, starting with a letter or a digit.
fn spec_name_ok(name: &str) -> bool {
    let ok = |c: u8| c.is_ascii_lowercase() || c.is_ascii_digit();
    !name.is_empty()
        && name.len() <= 64
        && ok(name.as_bytes()[0])
        && name
            .bytes()
            .all(|c| ok(c) || matches!(c, b'.' | b'_' | b'-'))
}

#[test]
fn every_refusal_code_the_verbs_print_is_registered() {
    let mut used = BTreeSet::new();
    for src in [
        include_str!("../src/live/verbs.rs"),
        include_str!("../src/verbs.rs"),
    ] {
        for (i, _) in src.match_indices("\"blend_") {
            let rest = &src[i + 1..];
            let end = rest.find('"').expect("a closing quote");
            used.insert(&rest[..end]);
        }
    }
    assert!(used.len() >= 15, "the verbs' codes are found: {used:?}");
    let registered: BTreeSet<&str> = ERRORS.iter().map(|e| e.name).collect();
    let missing: Vec<&&str> = used.iter().filter(|c| !registered.contains(**c)).collect();
    assert!(missing.is_empty(), "codes with no entry: {missing:?}");
    let ids: BTreeSet<&str> = ERRORS.iter().map(|e| e.id).collect();
    assert_eq!(ids.len(), ERRORS.len(), "each entry its own id");
    for (i, e) in ERRORS.iter().enumerate() {
        assert_eq!(
            e.id,
            format!("BLEND-E{:03}", i + 1),
            "ids in order, none skipped"
        );
        assert!(
            !e.message.is_empty() && !e.next.is_empty(),
            "{}: its texts",
            e.id
        );
    }
}

#[test]
fn the_kits_registry_takes_the_pack_its_verbs_and_its_entries() {
    let r = Registry::for_pack(&BlendLive::default()).expect("the registry takes the pack");
    assert_eq!(r.errors().len(), ERRORS.len());
    for v in live_verbs::VERBS {
        assert!(r.get(v.name()).is_some(), "{} is in the registry", v.name());
    }
    // each next step names a verb the pack, its file verbs or the kit's index verbs have
    let mut known: BTreeSet<&str> = live_verbs::VERBS.iter().map(|v| v.name()).collect();
    known.extend(VERBS.iter().map(|v| v.name));
    known.insert("object list");
    for e in ERRORS {
        if let Some(step) = e.step {
            assert!(
                known.contains(step.verb),
                "{}: {} is a verb",
                e.id,
                step.verb
            );
        }
    }
}

/// Every refusal the writer, the reader and the mesh read give a file verb is an entry, with
/// the verb's own exit; a usage error, a stale base and the session's are the kit's.
#[test]
fn every_file_verb_refusal_is_an_entry_with_its_exit() {
    let k = || "@ob:Cube".to_owned();
    let all: Vec<VerbError> = vec![
        WriteError::Read(ReadError::NotBlend).into(),
        WriteError::Read(ReadError::Unknown { what: "version" }).into(),
        WriteError::Read(ReadError::Truncated { what: "a block" }).into(),
        WriteError::Read(ReadError::Malformed { what: "a block" }).into(),
        WriteError::Read(ReadError::Limit {
            what: "blocks",
            limit: 1,
        })
        .into(),
        WriteError::Version { version: 502 }.into(),
        WriteError::NotFound(k()).into(),
        WriteError::Ambiguous(k()).into(),
        WriteError::NotAnObject(k()).into(),
        WriteError::Linked(k()).into(),
        WriteError::Field { field: "loc" }.into(),
        WriteError::RotationMode {
            mode: "quaternion".into(),
        }
        .into(),
        WriteError::Name { why: "empty" }.into(),
        WriteError::Value.into(),
        WriteError::Check { why: "a byte" }.into(),
        MeshReadError::TooLarge {
            what: "meshes",
            got: 2,
            max: 1,
        }
        .into(),
        MeshReadError::Invalid { what: "a value" }.into(),
        MeshReadError::Pairing { what: "a count" }.into(),
    ];
    let declines = [
        "not_blend",
        "unknown_form",
        "unreadable",
        "live_too_large",
        "live_version",
        "live_ambiguous",
        "live_value",
    ];
    let mut seen = BTreeSet::new();
    for e in all.into_iter().chain(
        declines
            .iter()
            .map(|c| MeshReadError::Declined { code: c }.into()),
    ) {
        let entry = e.entry().unwrap_or_else(|| panic!("{e:?}: an entry"));
        assert_eq!(
            entry.exit,
            e.exit(),
            "{e:?}: the entry's exit is the verb's"
        );
        let r = e.report("object place");
        assert_eq!(r.id(), entry.id, "{e:?}: reported by its entry");
        assert_eq!(text(r.detail("code")), Some(entry.name), "{e:?}: its code");
        seen.insert(entry.name);
    }
    for e in [
        VerbError::Usage("give a field".into()),
        VerbError::Stale {
            expected: 1,
            actual: 2,
        },
    ] {
        assert!(e.entry().is_none(), "{e:?}: the kit's");
        assert!(e.report("object place").id().starts_with("MPC-E"), "{e:?}");
    }
    let file: BTreeSet<&str> = ERRORS
        .iter()
        .filter(|e| e.verbs.first() == Some(&"@file"))
        .map(|e| e.name)
        .collect();
    let missing: Vec<&&str> = file
        .iter()
        .filter(|n| !seen.contains(**n) && **n != "blend_not_found")
        .collect();
    assert!(
        missing.is_empty(),
        "file entries no refusal reaches: {missing:?}"
    );
}

/// The texts name the releases the writer checks, from `WRITE_FROM` and `WRITE_TO`; a file
/// from another release is declined live in the wording people read, which never says to save
/// back to an older release.
#[test]
fn a_file_from_another_release_is_declined_in_its_wording() {
    let range = checked_releases();
    assert_eq!(
        range,
        format!(
            "{} to {}",
            errors::release(WRITE_FROM),
            errors::release(WRITE_TO)
        )
    );
    for e in [errors::LIVE_VERSION, errors::FILE_VERSION] {
        assert!(
            e.message.contains(&format!("({range})")),
            "{}: {}",
            e.id,
            e.message
        );
        assert!(
            e.next.contains(&format!("from {range} if you have one")),
            "{}",
            e.id
        );
    }
    let Err(d) = BlendLive::default().decode(&old_release()) else {
        panic!("a 2.80 file is declined live");
    };
    assert_eq!(d.code, "live_version");
    assert_eq!(
        d.message,
        format!(
            "Nothing was read: this file was saved by a newer (or older) release of the program \
             that wrote it (format 2.80) than this pack has checked ({range})."
        )
    );
    let entry = by_code(d.code).expect("live_version is registered");
    assert_eq!(
        entry.next,
        format!(
            "Its saves are being checked; until then, open it read-only as a file, or save a \
             copy with a release from {range} if you have one."
        )
    );
}

/// A file saved by the release before the checked ones (2.80).
fn old_release() -> Vec<u8> {
    let mut b = SceneBuilder::new(HeaderForm::Classic, 8, false, 280, false);
    b.scene("Scene", &["A"], &[], None);
    b.object("A", &ObjectSpec::default());
    b.bytes()
}

#[test]
fn a_live_refusal_is_its_registered_entry() {
    if let Err(e) = rows::row_refusal_is_its_registered_entry() {
        panic!("row failed at its step: {}", e.replace('\n', " | "));
    }
}

#[test]
fn each_live_reads_result_is_its_data_pinned() {
    let mut failures = Vec::new();
    for (name, argv) in READS {
        let r = result_of(name, argv, false);
        assert_eq!(
            text(get(&r, "data_schema")),
            Some("typed"),
            "{name}: typed data"
        );
        assert_eq!(get(&r, "ok"), Some(&Json::Bool(true)), "{name}: ok");
        assert_eq!(
            text(get(&r, "data_spec")),
            Some(live_spec(argv).as_str()),
            "{name}: the data's shape, named"
        );
        if let Err(e) = kit_view_golden::check_in(DIR, name, &r) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_live_lists_next_page_is_data() {
    let r = result_of("next-page", &["scene", "list", "--limit", "2"], false);
    let next = get(&r, "page").and_then(|p| p.get("next"));
    assert_eq!(text(next.and_then(|n| n.get("verb"))), Some("scene list"));
    let args: Vec<&str> = match next.and_then(|n| n.get("args")) {
        Some(Json::List(v)) => v.iter().filter_map(Json::as_str).collect(),
        other => panic!("page.next.args: {other:?}"),
    };
    assert_eq!(args, ["--limit", "2", "--after", "@coll:2"]);
}

#[test]
fn each_live_edits_result_is_its_data_and_change_pinned() {
    let mut failures = Vec::new();
    for (name, argv) in EDITS {
        let r = result_of(name, argv, true);
        assert_eq!(
            text(get(&r, "data_schema")),
            Some("typed"),
            "{name}: typed data"
        );
        assert_eq!(get(&r, "ok"), Some(&Json::Bool(true)), "{name}: ok");
        assert_eq!(
            text(get(&r, "data_spec")),
            Some(live_spec(argv).as_str()),
            "{name}: the data's shape, named"
        );
        assert_eq!(text(get(&r, "kind")), Some("edit"), "{name}: an edit");
        if let Err(e) = kit_view_golden::check_in(DIR, name, &r) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn each_live_refusal_is_its_registered_entry_pinned() {
    let mut failures = Vec::new();
    for (name, code, argv) in REFUSALS {
        let edit = argv[1] != "show";
        let r = result_of(name, argv, edit);
        assert_eq!(get(&r, "ok"), Some(&Json::Bool(false)), "{name}: refused");
        assert_eq!(
            get(&r, "data_spec"),
            Some(&Json::Null),
            "{name}: names no shape"
        );
        let error = get(&r, "error").unwrap_or_else(|| panic!("{name}: an error"));
        let entry = by_code(code).unwrap_or_else(|| panic!("{code} is registered"));
        assert_eq!(
            text(get(error, "id")),
            Some(entry.id),
            "{name}: its entry's id"
        );
        if let Err(e) = kit_view_golden::check_in(DIR, name, &r) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A file verb's answer as a host puts it in a result: the verb, the shape's name and the data.
fn typed_value(verb: &str, t: &Typed) -> Json {
    assert_eq!(t.version, DATA_VERSION);
    assert!(spec_name_ok(t.name), "{}: a shape's name", t.name);
    assert_eq!(
        t.name,
        format!("blend.file.{}", verb.replace(' ', "-")),
        "{verb}: its shape's name"
    );
    Json::obj([
        ("verb", Json::from(verb)),
        ("data_spec", Json::from(t.spec())),
        ("data", t.data.clone()),
    ])
}

/// A file verb's refusal as a host reports it: the verb, its exit and the report.
fn refusal_value(verb: &str, e: &VerbError) -> Json {
    let r = e.report(verb);
    assert_eq!(
        r.exit(),
        e.exit(),
        "{verb}: {e}: the report's exit is the verb's"
    );
    Json::obj([
        ("verb", Json::from(verb)),
        ("exit", Json::from(e.exit())),
        ("error", report_value(&r)),
    ])
}

/// A report as pinned: the pack's whole; the kit's by its id, exit, message and details (the
/// rest is the kit's registry, pinned in the kit).
fn report_value(r: &ErrorReport) -> Json {
    if !r.id().starts_with("MPC-E") {
        return r.to_value();
    }
    let details = r.details().iter().map(|(k, v)| (*k, v.clone()));
    Json::obj([
        ("id", Json::from(r.id())),
        ("exit", Json::from(r.exit())),
        ("message", Json::from(r.message())),
        ("details", Json::obj(details)),
    ])
}

/// A scene the mesh read draws: a cube's mesh, used by a placed object and by one hidden in
/// viewports, an empty, the scene's camera, and an orthographic camera (left out).
fn mesh_scene() -> Vec<u8> {
    let positions: Vec<[f32; 3]> = (0..8u8)
        .map(|i| {
            [
                f32::from(i & 1),
                f32::from((i >> 1) & 1),
                f32::from((i >> 2) & 1),
            ]
        })
        .collect();
    let faces = vec![
        vec![0, 2, 3, 1],
        vec![4, 5, 7, 6],
        vec![0, 1, 5, 4],
        vec![2, 6, 7, 3],
        vec![0, 4, 6, 2],
        vec![1, 3, 7, 5],
    ];
    let mesh = |loc: [f32; 3], restrict: i16| ObjectSpec {
        data: Some("ME:Cube".into()),
        ty: 1,
        rotmode: 1,
        loc,
        restrict,
        ..ObjectSpec::default()
    };
    let mut b = SceneBuilder::meshes(403, MeshLayouts::LAYERS);
    b.file_global(Some("S"));
    b.scene(
        "S",
        &["Cube", "Ghost", "Empty", "Cam", "Ortho"],
        &[],
        Some("Cam"),
    );
    b.object("Cube", &mesh([1.0, 2.0, 3.0], 0));
    b.object("Ghost", &mesh([0.0, 0.0, 0.0], 1));
    b.object(
        "Empty",
        &ObjectSpec {
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    let cam = |data: &str| ObjectSpec {
        data: Some(format!("CA:{data}")),
        ty: 11,
        rotmode: 1,
        loc: [0.0, -5.0, 1.0],
        ..ObjectSpec::default()
    };
    b.object("Cam", &cam("Cam"));
    b.object("Ortho", &cam("Ortho"));
    b.camera_with(
        "Cam",
        &CameraSpec {
            lens: 50.0,
            ..CameraSpec::default()
        },
    );
    b.camera_with(
        "Ortho",
        &CameraSpec {
            lens: 50.0,
            ty: 1,
            ..CameraSpec::default()
        },
    );
    b.mesh_with(
        "Cube",
        &MeshSpec {
            positions,
            faces,
            ..MeshSpec::default()
        },
        MeshLayouts::LAYERS,
    );
    b.bytes()
}

fn place(object: &str, loc: Option<[f32; 3]>, rot: Option<[f32; 3]>) -> Verb {
    Verb::Place {
        object: object.into(),
        loc,
        rot,
        scale: None,
    }
}

#[test]
fn each_file_verbs_answer_is_its_data_pinned() {
    let l = ReadLimits::default();
    let b = studio(&Studio::default());
    let mut got: Vec<(&str, Json)> = vec![(
        "file-scene-list",
        typed_value(
            "scene list",
            &list(&b, &l).expect("the studio lists").typed(),
        ),
    )];
    for (name, verb, v) in [
        (
            "file-object-place",
            "object place",
            Verb::Place {
                object: "@ob:Cube".into(),
                loc: Some([1.0, 2.0, 3.0]),
                rot: None,
                scale: Some([2.0; 3]),
            },
        ),
        (
            "file-object-visibility",
            "object visibility",
            Verb::Visibility {
                object: "@ob:Light".into(),
                viewport: Some(true),
                select: None,
                render: Some(false),
            },
        ),
        (
            "file-object-rename",
            "object rename",
            Verb::Rename {
                object: "@ob:Light".into(),
                name: "Key Light".into(),
            },
        ),
    ] {
        apply(&b, &v, &l).unwrap_or_else(|e| panic!("{name}: the verb writes: {e}"));
        got.push((name, typed_value(verb, &v.typed())));
    }
    let m = meshes(&mesh_scene(), &MeshReadLimits::default()).expect("the scene draws");
    assert_eq!(
        m.report.drawn.len(),
        2,
        "the cube and its hidden twin are drawn"
    );
    assert_eq!(m.report.cameras.len(), 1, "the scene's camera is drawn");
    assert_eq!(
        m.report.skipped.len(),
        1,
        "the orthographic camera is left out"
    );
    got.push((
        "file-scene-meshes",
        typed_value("scene meshes", &meshes_typed(&m)),
    ));
    let mut failures = Vec::new();
    for (name, v) in &got {
        if let Err(e) = kit_view_golden::check_in(DIR, name, v) {
            failures.push(e);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn each_file_verb_refusal_is_its_entry_pinned() {
    let l = ReadLimits::default();
    let b = studio(&Studio::default());
    let refuse = |bytes: &[u8], v: Verb| apply(bytes, &v, &l).unwrap_err();
    let cases: Vec<(&str, &str, Option<&ErrorEntry>, VerbError)> = vec![
        (
            "file-refused-not-found",
            "object place",
            Some(&errors::NOT_FOUND_ENTRY),
            refuse(&b, place("@ob:Nobody", Some([0.0; 3]), None)),
        ),
        (
            "file-refused-rotation-mode",
            "object place",
            Some(&errors::ROTATION_MODE),
            refuse(&b, place("@ob:Cube", None, Some([0.1, 0.0, 0.0]))),
        ),
        (
            "file-refused-name",
            "object rename",
            Some(&errors::NAME),
            refuse(
                &b,
                Verb::Rename {
                    object: "@ob:Cube".into(),
                    name: "Light".into(),
                },
            ),
        ),
        (
            "file-refused-linked",
            "object place",
            Some(&errors::NOT_EDITABLE),
            refuse(&b, place("@li:lib/ma:Shared", Some([0.0; 3]), None)),
        ),
        (
            "file-refused-version",
            "object place",
            Some(&errors::FILE_VERSION),
            refuse(&old_release(), place("@ob:A", Some([0.0; 3]), None)),
        ),
        (
            "file-refused-not-blend",
            "scene list",
            Some(&errors::FILE_NOT_BLEND),
            list(b"hello", &l).unwrap_err(),
        ),
        (
            "file-refused-usage",
            "object place",
            None,
            refuse(&b, place("@ob:Cube", None, None)),
        ),
    ];
    let mut failures = Vec::new();
    for (name, verb, want, e) in &cases {
        assert_eq!(e.entry().map(|x| x.id), want.map(|x| x.id), "{name}: {e}");
        if let Err(x) = kit_view_golden::check_in(DIR, name, &refusal_value(verb, e)) {
            failures.push(x);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn a_live_decline_is_pinned_with_its_entry() {
    let Err(d) = BlendLive::default().decode(&old_release()) else {
        panic!("a 2.80 file is declined live");
    };
    let entry = by_code(d.code).expect("the decline is registered");
    let v = Json::obj([
        ("code", Json::from(d.code)),
        ("message", Json::from(d.message.as_str())),
        ("kit_report", report_value(&d.report())),
        (
            "entry",
            Json::obj([
                ("id", Json::from(entry.id)),
                ("name", Json::from(entry.name)),
                ("exit", Json::from(entry.exit)),
                ("message", Json::from(entry.message)),
                ("next", Json::from(entry.next)),
                ("next_step", entry.step.map_or(Json::Null, |s| s.to_value())),
                ("verbs", Json::list(entry.verbs.iter().copied())),
            ]),
        ),
    ]);
    if let Err(e) = kit_view_golden::check_in(DIR, "declined-live-version", &v) {
        panic!("{e}");
    }
}
