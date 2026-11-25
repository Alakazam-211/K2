//! `--label WORD` in auto-placement (0.46.18): only nodes that carry every
//! asked label **for this server** (`k2_core::compute::server_labels`),
//! and the placement line names them. Kept apart so compute_place.rs stays
//! under its line limit.

use super::*;

const WS: &str = "ws-me";

/// Three granted nodes with an offer each (offline: the stored offer).
fn seed(conn: &rusqlite::Connection) {
    for n in ["node-a", "node-b", "node-c"] {
        store::insert_pending_node(conn, n, n, &format!("fp-{n}"), "pem", &BTreeMap::new(), "owner", "123456", 1).expect("node");
        store::confirm_node(conn, n, "owner", 2).expect("confirm");
        store::set_grant(conn, n, WS, &store::GrantLimits::default(), "owner", 3).expect("grant");
        store::store_offer(conn, n, &tests::cand(n, 30, 32, 0, 2).offer, 4);
    }
}

fn label(conn: &rusqlite::Connection, node: &str, labels: &[&str]) {
    let add: Vec<String> = labels.iter().map(|s| s.to_string()).collect();
    k2_core::compute::server_labels::change(conn, node, &add, &[], "owner", 5).expect("label");
}

fn want(labels: &[&str]) -> Want {
    let mut w = tests::want(1);
    w.ws_id = WS.into();
    w.filter.server_labels = labels.iter().map(|s| s.to_string()).collect();
    w
}

fn names(r: &CandReport) -> Vec<String> {
    let mut v: Vec<String> = r.cands.iter().map(|c| c.node.name.clone()).collect();
    v.sort();
    v
}

#[test]
fn only_nodes_with_every_asked_label_are_candidates() {
    let _db = k2_core::db::scoped_for_test();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        seed(&conn);
        label(&conn, "node-a", &["release"]);
        label(&conn, "node-b", &["release", "for:0.46.18"]);
    }
    let now = compute::now();
    // No label asked: every node.
    assert_eq!(names(&candidates_report_raw(&want(&[]), &[], now)), ["node-a", "node-b", "node-c"]);
    // One label: the two that carry it; the third is skipped for it.
    let r = candidates_report_raw(&want(&["release"]), &[], now);
    assert_eq!(names(&r), ["node-a", "node-b"]);
    assert_eq!(r.skipped.get(skip::SERVER_LABEL), Some(&1), "{:?}", r.skipped);
    // Two labels: only the node with both.
    let r = candidates_report_raw(&want(&["for:0.46.18", "release"]), &[], now);
    assert_eq!(names(&r), ["node-b"]);
    assert_eq!(r.skipped.get(skip::SERVER_LABEL), Some(&2));
    // A label no node carries: none, and the words say why.
    let w = want(&["gpu"]);
    let r = candidates_report_raw(&w, &[], now);
    assert!(r.cands.is_empty());
    assert_eq!(skip_words(skip::SERVER_LABEL, 3, &w), "3 without the label gpu here");
    assert_eq!(skip_words(skip::SERVER_LABEL, 1, &want(&["a", "b"])), "1 without the labels a, b here");
}

/// Another server using the same nodes has its own database: none of
/// this server's labels exist there, so a job asking for one finds none.
#[test]
fn another_servers_labels_never_count_here() {
    {
        let _db = k2_core::db::scoped_for_test();
        let db = k2_core::db::shared();
        let conn = db.lock();
        seed(&conn);
        label(&conn, "node-a", &["release"]);
        drop(conn);
        assert_eq!(names(&candidates_report_raw(&want(&["release"]), &[], compute::now())), ["node-a"], "server-a: its own label");
    }
    let _db = k2_core::db::scoped_for_test();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        seed(&conn);
    }
    let r = candidates_report_raw(&want(&["release"]), &[], compute::now());
    assert!(r.cands.is_empty(), "server-b: node-a carries none of server-a's labels: {:?}", names(&r));
    assert_eq!(r.skipped.get(skip::SERVER_LABEL), Some(&3));
}

#[test]
fn the_placement_line_names_the_matched_labels() {
    let _db = k2_core::db::scoped_for_test();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        seed(&conn);
        label(&conn, "node-b", &["release", "for:0.46.18"]);
    }
    let w = want(&["for:0.46.18", "release"]);
    let r = candidates_report_raw(&w, &[], compute::now());
    let c = &r.cands[0];
    let ex = Explain { policy: compute::PlacementPolicy::V2, cands: &r.cands, skipped: &r.skipped, project_key: None, commit: None };
    let rec = placement_record(c, Tier::Offline, &w, &ex, 1);
    assert_eq!(rec.node, "node-b");
    assert!(rec.why.starts_with("labels for:0.46.18, release · "), "{}", rec.why);
    assert_eq!(labels_words(&want(&["release"])).as_deref(), Some("label release"));
    assert_eq!(labels_words(&want(&[])), None);
    // No label asked: the line is as before.
    let plain = want(&[]);
    let rec = placement_record(c, Tier::Offline, &plain, &ex, 1);
    assert!(!rec.why.contains("label"), "{}", rec.why);
}
