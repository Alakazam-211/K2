//! The three-way merge on files built here: the agent's versions come from the writer, the
//! person's saves are the scene built again at other addresses (an app's save moves everything
//! in memory), with their own changes. Disjoint changes both land; a field both changed keeps
//! the person's and holds the agent's; renames follow the carried key rule; an agent version
//! that is more than field changes is a whole-file conflict; and the pack's guards.

use k2_mpc_base::session::{BlobMerge, BlobPack};
use k2_mpc_blend::merge::{merge3, ConflictWhy, Merged, Outcome};
use k2_mpc_blend::testgen::{gzip, ObjectSpec, SceneBuilder};
use k2_mpc_blend::write::decompress;
use k2_mpc_blend::{
    merge_known_base, read_blend, read_scene, write, BlendPack, Change, Edit, ReadLimits,
    SceneModel,
};

/// What the person may have changed.
#[derive(Clone)]
struct Person {
    shift: u64,
    names: [&'static str; 3],
    a_loc: [f32; 3],
    a_scale: [f32; 3],
    a_restrict: i16,
    keep_a: bool,
}

impl Default for Person {
    fn default() -> Self {
        Self {
            shift: 0,
            names: ["A", "B", "C"],
            a_loc: [0.0; 3],
            a_scale: [1.0; 3],
            a_restrict: 0,
            keep_a: true,
        }
    }
}

/// A scene: A (mesh data A), B (mesh data B), C (no data, parent B).
fn scene(p: &Person) -> Vec<u8> {
    let mut b = SceneBuilder::modern();
    b.shift = p.shift;
    let [a, bb, c] = p.names;
    let mut objs = vec![bb, c];
    if p.keep_a {
        objs.insert(0, a);
    }
    b.scene("Scene", &objs, &[], None);
    if p.keep_a {
        b.object(
            a,
            &ObjectSpec {
                data: Some("ME:A".into()),
                loc: p.a_loc,
                scale: p.a_scale,
                restrict: p.a_restrict,
                rotmode: 1,
                ..ObjectSpec::default()
            },
        );
    }
    b.object(
        bb,
        &ObjectSpec {
            data: Some("ME:B".into()),
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.object(
        c,
        &ObjectSpec {
            parent: Some(bb.into()),
            rotmode: 1,
            ..ObjectSpec::default()
        },
    );
    b.mesh("A", &[], 3);
    b.mesh("B", &[], 3);
    b.bytes()
}

fn base() -> Vec<u8> {
    scene(&Person::default())
}

/// The person's save: the scene built again at other addresses, with `p`'s changes.
fn person(p: Person) -> Vec<u8> {
    scene(&Person {
        shift: 0x55_0000,
        ..p
    })
}

fn agent(edits: &[Edit]) -> Vec<u8> {
    write(&base(), edits, &ReadLimits::default())
        .expect("check: the agent's write")
        .bytes
}

fn merged(ours: &[u8], theirs: &[u8]) -> Merged {
    match merge3(&base(), ours, theirs, &ReadLimits::default()) {
        Outcome::Merged(m) => m,
        other => panic!("{other:?}"),
    }
}

fn model(bytes: &[u8]) -> SceneModel {
    let (_, raw) = decompress(bytes, &ReadLimits::default()).unwrap();
    read_scene(&read_blend(&mut &raw[..], &ReadLimits::default()).unwrap()).unwrap()
}

fn value(bytes: &[u8], key: &str, name: &str) -> Option<String> {
    let m = model(bytes);
    let id = m.by_key(key)?;
    id.fields
        .iter()
        .chain(&id.named)
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.clone())
}

fn e(key: &str, change: Change) -> Edit {
    Edit::new(key, change)
}

#[test]
fn disjoint_changes_both_land_on_the_persons_save() {
    let ours = agent(&[e("@ob:A", Change::Location([1.0, 2.0, 3.0]))]);
    let theirs = person(Person {
        a_scale: [2.0; 3],
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
    assert_eq!(m.applied.len(), 1);
    assert_eq!(
        value(&m.bytes, "@ob:A", "loc").as_deref(),
        Some("1,2,3"),
        "the agent's"
    );
    assert_eq!(
        value(&m.bytes, "@ob:A", "size").as_deref(),
        Some("2,2,2"),
        "the person's"
    );
    // the person's save is the file: only the agent's field differs from it
    let changed: Vec<usize> = (0..theirs.len())
        .filter(|&i| theirs[i] != m.bytes[i])
        .collect();
    assert!(
        !changed.is_empty() && changed.len() <= 12,
        "{} bytes",
        changed.len()
    );
}

#[test]
fn a_field_both_changed_keeps_the_persons_and_holds_the_agents() {
    let ours = agent(&[e("@ob:A", Change::Location([1.0, 1.0, 1.0]))]);
    let theirs = person(Person {
        a_loc: [9.0, 9.0, 9.0],
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert_eq!(value(&m.bytes, "@ob:A", "loc").as_deref(), Some("9,9,9"));
    assert_eq!(m.conflicts.len(), 1);
    let c = &m.conflicts[0];
    assert_eq!((c.object.as_str(), c.field), ("@ob:A", "loc"));
    assert_eq!(c.why, ConflictWhy::BothChanged);
    assert_eq!(c.base.as_deref(), Some("0,0,0"));
    assert_eq!(c.person.as_deref(), Some("9,9,9"));
    assert_eq!(c.agent, "1,1,1");
    assert!(m.applied.is_empty());
    assert_eq!(m.bytes, theirs, "nothing of the agent's landed");
    // the same clash, the same id
    assert_eq!(merged(&ours, &theirs).conflicts[0].id, c.id);
    // the same change on both sides is no conflict
    let same = person(Person {
        a_loc: [1.0, 1.0, 1.0],
        ..Person::default()
    });
    let m = merged(&ours, &same);
    assert!(m.conflicts.is_empty() && m.applied.is_empty());
}

#[test]
fn each_visibility_bit_is_its_own_field() {
    let ours = agent(&[e(
        "@ob:A",
        Change::Visibility {
            viewport: Some(true),
            select: None,
            render: None,
        },
    )]);
    let theirs = person(Person {
        a_restrict: 4,
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
    assert_eq!(
        value(&m.bytes, "@ob:A", "restrictflag").as_deref(),
        Some("5")
    );
}

#[test]
fn an_agent_change_to_an_object_the_person_deleted_is_held() {
    let ours = agent(&[e("@ob:A", Change::Scale([3.0; 3]))]);
    let theirs = person(Person {
        keep_a: false,
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert_eq!(m.conflicts.len(), 1);
    let c = &m.conflicts[0];
    assert_eq!(c.why, ConflictWhy::PersonDeleted);
    assert_eq!(
        (c.object.as_str(), c.field, c.person.as_deref()),
        ("@ob:A", "scale", None)
    );
    assert_eq!(m.bytes, theirs);
}

#[test]
fn renames_follow_the_carried_key_rule() {
    // the person renamed A (its data stays): the agent's change lands on the renamed object
    let ours = agent(&[e("@ob:A", Change::Location([5.0, 0.0, 0.0]))]);
    let theirs = person(Person {
        names: ["Hero", "B", "C"],
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
    assert_eq!(value(&m.bytes, "@ob:Hero", "loc").as_deref(), Some("5,0,0"));
    assert_eq!(m.applied[0].object, "@ob:Hero");
    // an object without data is matched by its parent: `C` renamed to `C2` (`B`'s child)
    let ours = agent(&[e("@ob:C", Change::Scale([4.0; 3]))]);
    let theirs = person(Person {
        names: ["A", "B", "C2"],
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert_eq!(value(&m.bytes, "@ob:C2", "size").as_deref(), Some("4,4,4"));
    // the agent renamed A, the person moved it: both land
    let ours = agent(&[e("@ob:A", Change::Name("Agent Name".into()))]);
    let theirs = person(Person {
        a_loc: [1.0, 2.0, 3.0],
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert!(m.conflicts.is_empty(), "{:?}", m.conflicts);
    assert_eq!(
        value(&m.bytes, "@ob:Agent%20Name", "loc").as_deref(),
        Some("1,2,3")
    );
    // both renamed A, differently: the person's name stays
    let theirs = person(Person {
        names: ["Person Name", "B", "C"],
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert_eq!(m.conflicts.len(), 1);
    assert_eq!(m.conflicts[0].why, ConflictWhy::BothChanged);
    assert!(model(&m.bytes).by_key("@ob:Person%20Name").is_some());
    // the agent's new name is one the person gave another object: held, the writer's reason
    let ours = agent(&[e("@ob:A", Change::Name("X".into()))]);
    let theirs = person(Person {
        names: ["A", "X", "C"],
        ..Person::default()
    });
    let m = merged(&ours, &theirs);
    assert_eq!(m.conflicts.len(), 1, "{:?}", m.conflicts);
    assert!(matches!(&m.conflicts[0].why, ConflictWhy::Refused(w) if w.contains("another object")));
}

#[test]
fn an_agent_version_that_is_more_than_field_changes_is_a_whole_file_conflict() {
    let ours = person(Person {
        a_loc: [1.0; 3],
        ..Person::default()
    });
    let theirs = person(Person {
        a_scale: [2.0; 3],
        shift: 0x99_0000,
        ..Person::default()
    });
    match merge3(&base(), &ours, &theirs, &ReadLimits::default()) {
        Outcome::Conflict(why) => assert!(why.contains("more than the fields"), "{why}"),
        other => panic!("{other:?}"),
    }
    let ours = agent(&[e("@ob:A", Change::Location([1.0; 3]))]);
    match merge3(&base(), &ours, b"not a scene", &ReadLimits::default()) {
        Outcome::Unsupported(_) => {}
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_compressed_save_stays_compressed() {
    let ours = agent(&[e("@ob:B", Change::Location([0.5; 3]))]);
    let theirs = gzip(&person(Person {
        a_scale: [2.0; 3],
        ..Person::default()
    }));
    let m = merged(&ours, &theirs);
    assert_eq!(m.bytes[..2], [0x1f, 0x8b]);
    assert_eq!(
        value(&m.bytes, "@ob:B", "loc").as_deref(),
        Some("0.5,0.5,0.5")
    );
    assert_eq!(value(&m.bytes, "@ob:A", "size").as_deref(), Some("2,2,2"));
}

#[test]
fn the_merge_is_a_pure_function_and_short_cuts_are_exact() {
    let b = base();
    let ours = agent(&[e("@ob:A", Change::Location([1.0; 3]))]);
    let theirs = person(Person::default());
    let l = ReadLimits::default();
    assert_eq!(
        merge3(&b, &ours, &theirs, &l),
        merge3(&b, &ours, &theirs, &l)
    );
    let only = |o: Outcome| match o {
        Outcome::Merged(m) => (m.bytes, m.conflicts.len()),
        other => panic!("{other:?}"),
    };
    assert_eq!(only(merge3(&b, &ours, &ours, &l)), (ours.clone(), 0));
    assert_eq!(only(merge3(&b, &ours, &b, &l)), (ours.clone(), 0));
    assert_eq!(only(merge3(&b, &b, &theirs, &l)), (theirs.clone(), 0));
}

/// The pack's answer, without the bytes (the merged file's, when merged).
fn answer(m: BlobMerge) -> (&'static str, Vec<u8>) {
    match m {
        BlobMerge::Merged(bytes) => ("merged", bytes),
        BlobMerge::Conflict => ("conflict", Vec::new()),
        _ => ("unsupported", Vec::new()),
    }
}

#[test]
fn the_pack_keeps_both_on_every_save_until_the_kit_knows_the_base() {
    let pack = BlendPack::default();
    assert!(
        !pack.can_merge(),
        "the kit needn't read three versions to keep both"
    );
    let b = base();
    let theirs = person(Person {
        a_loc: [9.0; 3],
        ..Person::default()
    });
    let said = |ours: &[u8], theirs: &[u8]| answer(pack.merge3_bytes(&b, ours, theirs)).0;
    // the agent wrote nothing since the base: the app's save may predate an agent's write
    assert_eq!(said(&b, &theirs), "conflict");
    // both moved A
    let clash = agent(&[e("@ob:A", Change::Location([1.0; 3]))]);
    assert_eq!(said(&clash, &theirs), "conflict");
    // disjoint changes: the base may be a kit write the app never read, so these keep
    // both too until the kit says what the app had open
    let disjoint = agent(&[e("@ob:B", Change::Location([1.0; 3]))]);
    assert_eq!(said(&disjoint, &theirs), "conflict");
    // one side's bytes again is no merge
    let again = answer(pack.merge3_bytes(&b, &disjoint, &disjoint));
    assert!(again == ("merged", disjoint), "{}", again.0);
}

#[test]
fn on_a_known_base_disjoint_changes_merge_and_a_clash_keeps_both() {
    let l = ReadLimits::default();
    let b = base();
    let theirs = person(Person {
        a_loc: [9.0; 3],
        ..Person::default()
    });
    // both moved A: the kit keeps the agent's version beside the save, never the agent's side
    // in pack memory only
    let ours = agent(&[e("@ob:A", Change::Location([1.0; 3]))]);
    assert_eq!(
        answer(merge_known_base(&b, &ours, &theirs, &l)).0,
        "conflict"
    );
    // disjoint changes merge
    let ours = agent(&[e("@ob:B", Change::Location([1.0; 3]))]);
    let (said, bytes) = answer(merge_known_base(&b, &ours, &theirs, &l));
    assert_eq!(said, "merged");
    assert_eq!(value(&bytes, "@ob:A", "loc").as_deref(), Some("9,9,9"));
    assert_eq!(value(&bytes, "@ob:B", "loc").as_deref(), Some("1,1,1"));
    // on the app's own base, an agent that changed nothing leaves the save as it is
    let same = answer(merge_known_base(&b, &b, &theirs, &l));
    assert!(same == ("merged", theirs), "{}", same.0);
}

#[test]
fn the_pack_keeps_both_on_a_stale_save_after_a_kit_write() {
    // the kit wrote `v1` (the agent moved A) to the file; `v2` (the agent moved B) waits to
    // be written; the app, still on the base it opened, scales A and saves
    let v1 = agent(&[e("@ob:A", Change::Location([1.0; 3]))]);
    let v2 = write(
        &v1,
        &[e("@ob:B", Change::Location([5.0; 3]))],
        &ReadLimits::default(),
    )
    .expect("check: the agent's write")
    .bytes;
    let theirs = person(Person {
        a_scale: [2.0; 3],
        ..Person::default()
    });
    // the kit's base is what the file held, `v1`: merged against it, the save reads as the
    // person moving A back
    let (said, bytes) = answer(BlendPack::default().merge3_bytes(&v1, &v2, &theirs));
    if said == "merged" {
        panic!(
            "merged, the kit's write of A lost: A at {:?}",
            value(&bytes, "@ob:A", "loc")
        );
    }
    assert_eq!(said, "conflict");
}
