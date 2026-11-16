//! The kit's view index of a fixed timeline, pinned whole as golden snapshots
//! (`tests/kit_view_golden/`, the kit's own pretty JSON, which an editor's mocks can read as it
//! is). Any difference from a golden fails, naming each path; a change made on purpose is
//! re-blessed (`kit_view_golden`, at its top).
//!
//! Markers aren't units of the view (they come with the keyed state), so each golden holds the
//! index and the markers as `marker list` prints them, and the keyed state a watcher's snapshot
//! carries (`state`: each root, its elements and fields named by address as the wire names
//! them, `@item:1 geom`), which holds what a player reads beyond the index: the timeline's
//! `stacking`, `end_from` and `drop_frame`, a track's `enabled` and `muted`, a media's `start`
//! and `origin`, an item's `retimed`. Each container of the index carries its track's label and
//! kind (`derived.kind_label`, `derived.kind`), each unit its start timecode (`derived.tc`).
//!
//! - `cut`: the timeline the live rows use (`live_support::cut`), as it opens.
//! - `cut-edited`: the same after [`EDITS`], an agent's verbs in turn, each on a fresh base: a
//!   move, a trim, a lift, an add, an enable (`--off`) and a marker add.
//! - `cut-trim-in`: the same timeline after a trim of a clip's start ([`TRIM_IN`]: its record
//!   start and its source in point both move), with the change frame a keyed watch gets for it
//!   (`ops_change` as the wire carries it: the ops, the view's unit delta with its geometry
//!   before and after, and the clip's new start timecode in `derived`; its client id, minted per
//!   session, as the label [`CLIENT`]), so an edit bracket and a head tile come from pinned data.

mod kit_view_golden;
mod live_support;

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::errors::Value as Json;
use k2_mpc_base::session::{ConnId, Session, WatchFrame, WatchOptions, KEYED_WATCH_FEATURE};
use k2_mpc_base::view_delta::UnitOp;
use k2_mpc_timeline::live::address_of_key;
use live_support::{content, cut, fx, run, run_based, stdout};

/// The edits of `cut-edited`, in order.
const EDITS: &[&[&str]] = &[
    &["clip", "move", "@item:5", "--to", "+12f"],
    &["clip", "trim", "@item:4", "--out", "-6f"],
    &["clip", "delete", "@item:6"],
    &[
        "clip",
        "add",
        "--media",
        "@med:3",
        "--track",
        "@trk:3",
        "--at",
        "01:00:02:00",
        "--in",
        "20f",
        "--dur",
        "10f",
        "--name",
        "C on V2",
    ],
    &["clip", "enable", "@item:1", "--off"],
    &[
        "marker",
        "add",
        "--on",
        "@item:4",
        "--at",
        "01:00:04:12",
        "--name",
        "cue",
    ],
];

/// The trim of `cut-trim-in`: Shot A's start, six frames later.
const TRIM_IN: &[&str] = &["clip", "trim", "@item:1", "--in", "+6f"];

/// What `cut-trim-in` pins in place of the change frame's client id (minted per session).
const CLIENT: &str = "client-id";

/// The golden's value: the live index as `c` reads it, `marker list`'s output, and the keyed
/// state ([`state`]).
fn view(s: &Session, c: ConnId) -> Json {
    let ix = s
        .view_index(c)
        .expect("the view index reads")
        .expect("the pack has a view index");
    let markers = run(s, c, &["marker", "list"]).expect("marker list runs");
    assert_eq!(markers.exit, 0, "marker list: {}", stdout(&markers));
    Json::obj([
        ("index", ix.to_value()),
        ("marker_list", Json::from(stdout(&markers))),
        ("state", state(s, c)),
    ])
}

/// The live doc's keyed state as `c` reads it, by root in the schema's order: a list as its
/// `[address, value]` pairs in order, a map as its entries by address (`@item:1 geom`; a
/// `timeline` key as it is), in key order.
fn state(s: &Session, c: ConnId) -> Json {
    let roots = content(s, c).into_iter().map(|(root, n)| {
        let v = match n {
            Neutral::List(l) => Json::List(
                l.iter()
                    .map(|(k, v)| {
                        Json::List(vec![
                            Json::from(address_of_key(&root, k.as_str())),
                            Json::from(v.as_str().unwrap_or("")),
                        ])
                    })
                    .collect(),
            ),
            Neutral::Map(m) => Json::Obj(
                m.iter()
                    .map(|(k, v)| {
                        (
                            address_of_key(&root, k),
                            Json::from(v.as_str().unwrap_or("")),
                        )
                    })
                    .collect(),
            ),
            _ => Json::Null,
        };
        (root, v)
    });
    Json::Obj(roots.collect())
}

fn golden(name: &str, got: &Json) {
    if let Err(e) = kit_view_golden::check(name, got) {
        panic!("{e}");
    }
}

#[test]
fn the_cut_view_is_the_golden() {
    let f = fx("golden-cut", |_| {});
    f.write(&cut());
    let s = f.open();
    let a = f.agent(&s);
    golden("cut", &view(&s, a));
}

#[test]
fn the_cut_view_after_edits_is_the_golden() {
    let f = fx("golden-edited", |_| {});
    f.write(&cut());
    let s = f.open();
    let a = f.agent(&s);
    for e in EDITS {
        let o = run_based(&s, a, e).unwrap_or_else(|err| panic!("{e:?}: {err}"));
        assert_eq!(o.exit, 0, "{e:?} exited {}: {}", o.exit, stdout(&o));
    }
    golden("cut-edited", &view(&s, a));
}

#[test]
fn the_trim_of_a_clip_start_and_its_change_frame_are_the_golden() {
    let f = fx("golden-trim-in", |_| {});
    f.write(&cut());
    let s = f.open();
    let (p, a) = (f.person(&s), f.agent(&s));
    let mut w = s
        .watch_keyed(p, &WatchOptions::new(&[KEYED_WATCH_FEATURE]))
        .expect("the watch opens")
        .expect("the timeline is watchable");
    s.watch_snapshot(&mut w).expect("the snapshot");
    w.try_frames();
    let o = run_based(&s, a, TRIM_IN).expect("the trim runs");
    assert_eq!(o.exit, 0, "{TRIM_IN:?} exited {}: {}", o.exit, stdout(&o));
    let frames = w.try_frames();
    let changes: Vec<&WatchFrame> = frames
        .iter()
        .filter(|f| matches!(f, WatchFrame::Change(_)))
        .collect();
    assert_eq!(
        changes.len(),
        1,
        "one change frame for the trim: {frames:?}"
    );
    let WatchFrame::Change(c) = changes[0] else {
        unreachable!("filtered to change frames")
    };
    // the sample keeps what it is for: the clip's geometry and source both change, and its
    // record start moves
    let deltas = c.deltas.as_ref().expect("the frame carries view deltas");
    let d = deltas
        .units
        .iter()
        .find(|u| u.key == "@item:1")
        .expect("a unit delta for @item:1");
    assert!(
        d.ops.contains(&UnitOp::MoveResize),
        "the trim moves and resizes the clip: {d:?}"
    );
    assert!(
        d.fields.iter().any(|f| f == "geom") && d.fields.iter().any(|f| f == "src"),
        "the trim changes geom and src: {d:?}"
    );
    let start = |g: &Option<String>| -> String {
        g.as_deref()
            .and_then(|g| g.split(',').next())
            .unwrap_or_default()
            .to_owned()
    };
    assert_ne!(
        start(&d.geom_before),
        start(&d.geom_after),
        "the record start moves: {d:?}"
    );
    // the frame's client id is the session's, minted per run: pinned as a label, once checked
    // to be a number
    let mut frame = changes[0].to_value();
    if let Json::Obj(m) = &mut frame {
        for (k, v) in m.iter_mut() {
            if k == "client" {
                assert!(
                    matches!(v, Json::UInt(_) | Json::Int(_)),
                    "the frame's client is a number: {}",
                    v.to_json()
                );
                *v = Json::from(CLIENT);
            }
        }
    }
    let mut got = view(&s, a);
    if let Json::Obj(m) = &mut got {
        m.push(("change".to_owned(), frame));
    }
    golden("cut-trim-in", &got);
}
