//! The deck's verbs on the kit's tier-0 path (`PptxPack::verbs`, `blob_verbs`), run by the kit
//! on a blob doc through `BlobSession::run_tool`: each gives the file verb's bytes, needs the
//! caller's own lease, lists in the kit's registry with its word, and a hold on a slide covers
//! its shapes. The kit's revert, undo and redo (`BlobSession::run_revert`) take one version's
//! change away from a deck through this pack's merge, keeping a later edit to another slide.

mod common;

use common::blob_rows::{
    argv, call_with, err_id, file_verb_equals_blob_verb, head_bytes, head_entry, head_version,
    join, leased, open, res, revert,
};
use common::live::fx;
use common::{deflated, entries, limits};
use k2_mpc_base::errors::{capabilities, Value};
use k2_mpc_base::index::NodeKey;
use k2_mpc_base::parity::Registry;
use k2_mpc_base::session::{ClaimPolicy, Config, ParticipantKind, RevertWhat};
use k2_mpc_base::tools::ToolOptions;
use k2_mpc_pptx::blob_verbs::VERB_SET;
use k2_mpc_pptx::testgen;
use k2_mpc_pptx::verbs::{apply, list, render, Edit};
use k2_mpc_pptx::PptxPack;

/// file_verb_equals_blob_verb: every case on every fixture deck, stored and deflated: the
/// kit's verb gives the file verb's bytes (its plan applied by the kit, its output read by the
/// deck's index with no node it didn't declare), or the file verb's refusal. Mutant:
/// `pptx_declares_too_little`.
#[test]
fn file_verb_equals_blob_verb_row() {
    if let Err(e) = file_verb_equals_blob_verb() {
        panic!("file_verb_equals_blob_verb: {e}");
    }
}

/// edit_verbs_need_the_callers_lease: an agent's edit verb with no lease of its own is refused
/// (`MPC-E718`) and changes nothing; with it, it lands.
#[test]
fn edit_verbs_need_the_callers_lease() {
    let f = fx("blob-lease", |_| {});
    let bytes = testgen::deck(&testgen::sample());
    let s = open(&f, "lease.pptx", &bytes);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let a = argv(&["shape", "move", "@sld:258/sp:4", "--x", "100"]);
    let refused = err_id(&call_with(&s, bot, &a, ToolOptions::new()));
    let unchanged = head_bytes(&s, bot) == bytes;
    s.acquire_lease(bot, None)
        .expect("the agent takes the lease");
    let landed = err_id(&call_with(&s, bot, &a, ToolOptions::new()));
    let changed = head_bytes(&s, bot) != bytes;
    assert_eq!(
        (refused, unchanged, landed, changed),
        (Some("MPC-E718".to_owned()), true, None, true),
        "edit_verbs_need_the_callers_lease: no lease of its own: E718 and nothing written; with \
         it: lands"
    );
}

/// slide_list_on_the_blob_path: `slide list --shapes` answers what the file verb's list says,
/// as text and as data, and writes nothing.
#[test]
fn slide_list_on_the_blob_path() {
    let f = fx("blob-list", |_| {});
    let bytes = testgen::deck(&testgen::sample());
    let s = open(&f, "list.pptx", &bytes);
    let ann = join(&s, "ann", ParticipantKind::Person);
    let before = s.status().expect("status").version;
    let a = argv(&["slide", "list", "--shapes"]);
    // as text (the verb's own output), then as data
    let stdout = s
        .run_tool(ann, &a, &ToolOptions::new())
        .ok()
        .map(|p| String::from_utf8_lossy(&p.stdout).into_owned());
    let o = call_with(&s, ann, &a, ToolOptions::new());
    let want = list(&bytes, &limits()).expect("lists");
    let rows = match res(&o).get("data").and_then(|d| d.get("slides")) {
        Some(Value::List(l)) => Some(l.len()),
        _ => None,
    };
    let after = s.status().expect("status").version;
    assert_eq!(
        (err_id(&o), stdout, rows, after),
        (None, Some(render(&want, true)), Some(want.len()), before),
        "slide_list_on_the_blob_path: the file verb's list, as text and data; no version"
    );
}

/// a_slide_hold_covers_its_shapes: under `ClaimPolicy::Respect`, a person's hold on a slide
/// refuses an agent's verb on a shape of it (`MPC-E301`, naming the holder); a shape on
/// another slide lands.
#[test]
fn a_slide_hold_covers_its_shapes() {
    let f = fx("blob-hold", |_| {});
    let bytes = testgen::deck(&testgen::sample());
    let s = open(&f, "hold.pptx", &bytes);
    let ann = join(&s, "ann", ParticipantKind::Person);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    s.claim_nodes(ann, &[NodeKey::parse("@sld:258").unwrap()], None, None)
        .expect("ann holds the slide");
    s.acquire_lease(bot, None)
        .expect("the agent takes the lease");
    let respect = || ToolOptions::new().policy(ClaimPolicy::Respect);
    let held = call_with(
        &s,
        bot,
        &argv(&["shape", "move", "@sld:258/sp:4", "--x", "100"]),
        respect(),
    );
    let by = res(&held)
        .get("error")
        .and_then(|e| e.get("details"))
        .and_then(|d| d.get("by"))
        .and_then(Value::as_str)
        .map(str::to_owned);
    let other = err_id(&call_with(
        &s,
        bot,
        &argv(&["shape", "text", "@sld:257/sp:2", "--text", "Aims"]),
        respect(),
    ));
    assert_eq!(
        (err_id(&held), by, other),
        (Some("MPC-E301".to_owned()), Some("ann".to_owned()), None),
        "a_slide_hold_covers_its_shapes: a slide's hold refuses an agent's change to a shape on \
         it; another slide's shape lands"
    );
}

/// the_kit_lists_the_deck_verbs: the kit's registry for the pack takes `slide list` and the
/// seven edits, each edit with its word in `capabilities`.
#[test]
fn the_kit_lists_the_deck_verbs() {
    let r = Registry::for_blob_pack(&PptxPack::default()).expect("the deck's verbs are taken");
    let caps = capabilities(&[&r], &Config::default());
    let rows = match caps.get("verbs") {
        Some(Value::List(l)) => l.clone(),
        other => panic!("capabilities has no verbs list: {other:?}"),
    };
    let mut missing = Vec::new();
    for v in VERB_SET {
        let row = rows
            .iter()
            .find(|row| row.get("name").and_then(Value::as_str) == Some(v.name));
        let word = row.and_then(|x| x.get("word")).and_then(Value::as_str);
        let ok = r.get(v.name).is_some() && (v.word.is_empty() || word == Some(v.word));
        if !ok {
            missing.push(v.name);
        }
    }
    assert_eq!(
        (VERB_SET.len(), missing),
        (8, Vec::<&str>::new()),
        "the_kit_lists_the_deck_verbs: slide list and seven edits, each in the registry and in \
         capabilities with its word"
    );
}

/// The agent's shape move on slide 258 (the version a revert takes away).
const MOVE: &[&str] = &["shape", "move", "@sld:258/sp:4", "--x", "100"];
/// The person's later edit, on another slide.
const LATER: &[&str] = &["shape", "text", "@sld:257/sp:2", "--text", "Aims"];

/// The person's later edit alone on the sample deck, by the file verb.
fn later_alone(bytes: &[u8]) -> Vec<u8> {
    let e = Edit::SetText {
        slide: 257,
        shape: 2,
        text: "Aims".into(),
    };
    apply(bytes, &e, &limits())
        .expect("the file verb sets the text")
        .bytes
}

/// revert_keeps_a_later_edit: the agent moves a shape (version K), then a person sets a shape's
/// text on another slide. Reverting K is the agent's version with the verb `revert` and
/// `reverts: K`, and its bytes are the person's edit alone on the deck as it was before K, by
/// the file verb; the `zip` crate (a reader sharing no code with ours) reads every part of it
/// as that deck's. Plant: `revert-takes-whole-version`.
#[test]
fn revert_keeps_a_later_edit() {
    let f = fx("blob-revert", |_| {});
    let bytes = testgen::deck(&testgen::sample());
    let s = open(&f, "revert.pptx", &bytes);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let ann = join(&s, "ann", ParticipantKind::Person);
    let moved = leased(&s, bot, &argv(MOVE));
    assert_eq!(err_id(&moved), None, "setup: the agent's move lands");
    let k = head_version(&s);
    let later = leased(&s, ann, &argv(LATER));
    assert_eq!(err_id(&later), None, "setup: the person's later edit lands");
    let want = later_alone(&bytes);
    let o = revert(&s, bot, RevertWhat::Version(k));
    let got = head_bytes(&s, bot);
    assert_eq!(
        (
            err_id(&o),
            head_entry(&s, bot),
            entries(&got) == entries(&want),
            got == want
        ),
        (None, (Some("revert".to_owned()), Some(k)), true, true),
        "revert_keeps_a_later_edit: K's move taken away, the later edit kept, byte for byte ({})",
        res(&o).to_json()
    );
}

/// revert_refused_when_the_shape_changed_since: the agent moves a shape (version K), then a
/// person moves the same shape. Reverting K is refused (`MPC-E731`), its conflicts naming the
/// shape, the person and the person's version; head and its bytes are unchanged. Plant:
/// `revert-overwrites`.
#[test]
fn revert_refused_when_the_shape_changed_since() {
    let f = fx("blob-revert-clash", |_| {});
    let bytes = testgen::deck(&testgen::sample());
    let s = open(&f, "clash.pptx", &bytes);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let ann = join(&s, "ann", ParticipantKind::Person);
    let moved = leased(&s, bot, &argv(MOVE));
    assert_eq!(err_id(&moved), None, "setup: the agent's move lands");
    let k = head_version(&s);
    let again = leased(
        &s,
        ann,
        &argv(&["shape", "move", "@sld:258/sp:4", "--x", "200"]),
    );
    assert_eq!(err_id(&again), None, "setup: the person's move lands");
    let h = head_version(&s);
    let before = head_bytes(&s, bot);
    let o = revert(&s, bot, RevertWhat::Version(k));
    let shape = match res(&o).get("conflicts") {
        Some(Value::List(l)) => l
            .iter()
            .find(|c| c.get("key").and_then(Value::as_str) == Some("@sld:258/sp:4"))
            .map(|c| {
                (
                    c.get("by").and_then(Value::as_str).map(str::to_owned),
                    c.get("change").and_then(Value::as_u64),
                )
            }),
        _ => None,
    };
    assert_eq!(
        (
            err_id(&o),
            shape,
            head_version(&s),
            head_bytes(&s, bot) == before
        ),
        (
            Some("MPC-E731".to_owned()),
            Some((Some("ann".to_owned()), Some(h))),
            h,
            true
        ),
        "revert_refused_when_the_shape_changed_since: E731 naming the shape, who and when; \
         nothing written ({})",
        res(&o).to_json()
    );
}

/// undo_and_redo_keep_a_later_edit: the agent moves a shape (version K), then a person sets a
/// shape's text on another slide. The agent's undo takes K away (the verb `undo`, `reverts:
/// K`): the bytes are the person's edit alone, by the file verb. Its redo takes the undo away
/// (the verb `redo`, `reverts` the undo's version): the bytes are those from before the undo.
/// Plant: `undo-latest-any`.
#[test]
fn undo_and_redo_keep_a_later_edit() {
    let f = fx("blob-undo", |_| {});
    let bytes = testgen::deck(&testgen::sample());
    let s = open(&f, "undo.pptx", &bytes);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let ann = join(&s, "ann", ParticipantKind::Person);
    let moved = leased(&s, bot, &argv(MOVE));
    assert_eq!(err_id(&moved), None, "setup: the agent's move lands");
    let k = head_version(&s);
    let later = leased(&s, ann, &argv(LATER));
    assert_eq!(err_id(&later), None, "setup: the person's later edit lands");
    let before_undo = head_bytes(&s, bot);
    let want = later_alone(&bytes);
    let u = revert(&s, bot, RevertWhat::Undo);
    let undo_v = head_version(&s);
    let undone = (err_id(&u), head_entry(&s, bot), head_bytes(&s, bot) == want);
    let r = revert(&s, bot, RevertWhat::Redo);
    let redone = (
        err_id(&r),
        head_entry(&s, bot),
        head_bytes(&s, bot) == before_undo,
    );
    assert_eq!(
        (undone, redone),
        (
            (None, (Some("undo".to_owned()), Some(k)), true),
            (None, (Some("redo".to_owned()), Some(undo_v)), true)
        ),
        "undo_and_redo_keep_a_later_edit: undo takes the agent's own move away, the later edit \
         kept; redo puts it back (undo {}; redo {})",
        res(&u).to_json(),
        res(&r).to_json()
    );
}

/// The person's later edit to another shape of the agent's slide (258): the picture moved.
const SIBLING: &[&str] = &["shape", "move", "@sld:258/sp:7", "--x", "900000"];

/// The sibling's edit alone on the sample deck, by the file verb.
fn sibling_alone(bytes: &[u8]) -> Vec<u8> {
    let e = Edit::MoveShape {
        slide: 258,
        shape: 7,
        x: Some(900_000),
        y: None,
        w: None,
        h: None,
    };
    apply(bytes, &e, &limits())
        .expect("the file verb moves the picture")
        .bytes
}

/// The keys a refusal's `conflicts` name.
fn conflict_keys(o: &k2_mpc_base::tools::ToolOutput) -> Vec<String> {
    match res(o).get("conflicts") {
        Some(Value::List(l)) => l
            .iter()
            .filter_map(|c| c.get("key").and_then(Value::as_str).map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// revert_keeps_a_sibling_edit: the agent moves a shape (version K), then a person moves
/// another shape of the same slide. The slide's record holds both shapes, but its own record
/// (the index's format 3) didn't change: reverting K is the agent's version with `reverts: K`,
/// its bytes the person's move alone on the deck as it was before K, by the file verb, and the
/// `zip` crate reads every part of it as that deck's. Plant: `container-always-counted`.
#[test]
fn revert_keeps_a_sibling_edit() {
    let f = fx("blob-revert-sibling", |_| {});
    let bytes = testgen::deck(&testgen::sample());
    let s = open(&f, "sibling.pptx", &bytes);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let ann = join(&s, "ann", ParticipantKind::Person);
    let moved = leased(&s, bot, &argv(MOVE));
    assert_eq!(err_id(&moved), None, "setup: the agent's move lands");
    let k = head_version(&s);
    let later = leased(&s, ann, &argv(SIBLING));
    assert_eq!(
        err_id(&later),
        None,
        "setup: the person's move of a sibling lands"
    );
    let want = sibling_alone(&bytes);
    let o = revert(&s, bot, RevertWhat::Version(k));
    let got = head_bytes(&s, bot);
    assert_eq!(
        (
            err_id(&o),
            head_entry(&s, bot),
            entries(&got) == entries(&want),
            got == want
        ),
        (None, (Some("revert".to_owned()), Some(k)), true, true),
        "revert_keeps_a_sibling_edit: K's move taken away, the later move of another shape on \
         the same slide kept, byte for byte ({})",
        res(&o).to_json()
    );
}

/// The deck `bytes` with the picture (shape 7) and the table (shape 8) of slide 258 swapped in
/// its shape tree: a change of the slide's own (its shapes' order), each shape's XML as it was.
fn picture_and_table_swapped(bytes: &[u8]) -> Vec<u8> {
    let parts = entries(bytes);
    let (name, xml) = parts
        .iter()
        .filter(|(n, _)| n.starts_with("ppt/slides/slide"))
        .map(|(n, c)| (n.clone(), String::from_utf8(c.clone()).expect("utf-8")))
        .find(|(_, x)| x.contains("descr=\"A small square\""))
        .expect("slide 258's part");
    let one = |open: &str, close: &str| {
        assert_eq!(
            (xml.matches(open).count(), xml.matches(close).count()),
            (1, 1),
            "setup: one {open} on slide 258"
        );
        let a = xml.find(open).expect("its start");
        let b = xml.find(close).expect("its end") + close.len();
        (a, b)
    };
    let (pa, pb) = one("<p:pic>", "</p:pic>");
    let (ga, gb) = one("<p:graphicFrame>", "</p:graphicFrame>");
    assert!(pb <= ga, "setup: the picture comes before the table");
    let swapped = format!(
        "{}{}{}{}{}",
        &xml[..pa],
        &xml[ga..gb],
        &xml[pb..ga],
        &xml[pa..pb],
        &xml[gb..]
    );
    let parts: Vec<(String, Vec<u8>)> = parts
        .into_iter()
        .map(|(n, c)| {
            if n == name {
                (n, swapped.clone().into_bytes())
            } else {
                (n, c)
            }
        })
        .collect();
    deflated(&parts)
}

/// revert_refused_when_the_slide_order_changed: the agent moves a shape (version K), then a
/// person's write swaps two other shapes of the same slide (the picture and the table; the
/// agent's shape and every shape's XML as they were). The slide's own record changed: reverting
/// K is refused (`MPC-E731`), its conflicts naming the slide; head and its bytes are unchanged.
/// Plant: `container-never-counted`.
#[test]
fn revert_refused_when_the_slide_order_changed() {
    let f = fx("blob-revert-order", |_| {});
    let bytes = testgen::deck(&testgen::sample());
    let s = open(&f, "order.pptx", &bytes);
    let bot = join(&s, "bot", ParticipantKind::Agent);
    let ann = join(&s, "ann", ParticipantKind::Person);
    let moved = leased(&s, bot, &argv(MOVE));
    assert_eq!(err_id(&moved), None, "setup: the agent's move lands");
    let k = head_version(&s);
    let swapped = picture_and_table_swapped(&head_bytes(&s, bot));
    s.write_bytes(ann, &swapped, k)
        .expect("setup: the person's write lands");
    if let Some(j) = s.index_job(None).expect("an index job") {
        j.run().expect("the index job runs");
    }
    let h = head_version(&s);
    let before = head_bytes(&s, bot);
    let o = revert(&s, bot, RevertWhat::Version(k));
    assert_eq!(
        (
            err_id(&o),
            conflict_keys(&o).contains(&"@sld:258".to_owned()),
            head_version(&s),
            head_bytes(&s, bot) == before
        ),
        (Some("MPC-E731".to_owned()), true, h, true),
        "revert_refused_when_the_slide_order_changed: the slide's own change since K refuses, \
         naming the slide; nothing written ({})",
        res(&o).to_json()
    );
}
