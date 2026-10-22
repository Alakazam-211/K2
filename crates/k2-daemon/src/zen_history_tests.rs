//! Garden History, slice A6 (prd-garden-access-v1 G29): the lines A6
//! writes (locked/unlocked, sync, lock_put_back, disk_edit) and the merge
//! of snapshots, stamps and decided proposals into the History routes.
//! Through the real handlers; temp HOME, scoped DB, the switch on for the
//! test (TempHome's env lock keeps it to this test). Fail loudly.

use std::collections::HashMap;

use serde_json::{json, Value as J};

use k2_core::garden_data::authors::{self, CallerKind, StampSource};
use k2_core::garden_data::MadeBy;
use k2_core::zen::garden_access::{self as ga, Actor, EventKind, On};
use k2_core::zen::{ZenFile, ZenFiles};

use crate::sidecar_routes::Caller;
use crate::zen_access_routes::{GARDEN_HISTORY_ROUTE, WIDGET_HISTORY_ROUTE};

const PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n";

struct SwitchOn;

impl Drop for SwitchOn {
    fn drop(&mut self) {
        ga::set_switch(false);
    }
}

struct Env {
    _switch: Option<SwitchOn>,
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    f: ZenFiles,
    g1: String,
}

/// Zen set up; Garden 1 places `board`; workspace `w-ed` (handle `editor`).
/// The baseline of what K2 saw is this state.
fn env_with(on: bool) -> Env {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let dir = home.path().join("ga-editor");
        std::fs::create_dir_all(&dir).expect("workspace dir");
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES ('w-ed', 'editor', ?1, 'editor')",
            rusqlite::params![dir.to_string_lossy().to_string()],
        )
        .expect("workspace row");
    }
    let f = ZenFiles::local();
    f.setup().expect("zen setup");
    f.new_widget("board", Some("hello")).expect("widget folder");
    let g1 = f.gardens()[0].id.clone();
    std::fs::write(f.path_of(&ZenFile::Garden(g1.clone())), PAGE).expect("garden page");
    let switch = on.then(|| {
        ga::set_switch(true);
        SwitchOn
    });
    super::forget_baseline();
    crate::zen_routes::refresh_and_emit().expect("refresh (the baseline)");
    Env { _switch: switch, _db: db, _home: home, f, g1 }
}

fn env() -> Env {
    env_with(true)
}

fn zen(path: &str, body: J) -> (String, J) {
    let r = crate::zen_routes::handle(path, true, &Caller::Owner, "test", &HashMap::new(), body.to_string().as_bytes());
    let v: J = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body));
    (r.status.to_string(), v)
}

fn history(path: &str, q: &[(&str, &str)]) -> J {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = crate::zen_access_routes::handle(path, false, true, &Caller::Owner, "test", &p, &[]);
    assert_eq!(r.status, "200 OK", "{path} {q:?}: {}", r.body);
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body))
}

fn items(v: &J) -> Vec<J> {
    v["items"].as_array().unwrap_or_else(|| panic!("no items: {v}")).clone()
}

fn events(garden: &str, widgets: &[&str]) -> Vec<ga::Event> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::events_for_garden(&conn, garden, widgets, 500, None).expect("events")
}

fn kinds(evs: &[ga::Event]) -> Vec<&'static str> {
    evs.iter().map(|e| e.kind.as_str()).collect()
}

fn page_with(accent: &str) -> String {
    format!("{PAGE}\n[colors.light]\naccent = \"{accent}\"\n")
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── the lines A6 writes ──────────────────────────────────────────────

#[test]
fn lock_unlock_and_design_updates_write_one_line_each() {
    let e = env();
    let (s, v) = zen("/cli/zen/garden/lock", json!({"garden": e.g1, "locked": true}));
    assert_eq!(s, "200 OK", "{v}");
    // Already locked: `changed: false`, nothing written.
    let (s, v) = zen("/cli/zen/garden/lock", json!({"garden": e.g1, "locked": true}));
    assert_eq!((s.as_str(), &v["changed"]), ("200 OK", &json!(false)), "{v}");
    let (s, v) = zen("/cli/zen/garden/lock", json!({"garden": e.g1, "locked": false}));
    assert_eq!(s, "200 OK", "{v}");
    // Design updates off then on: one `sync` line per answer that changed
    // something (Garden 1 starts with one of the two states).
    let mut changed = Vec::new();
    for on in [false, true] {
        let (s, v) = zen("/cli/zen/garden/sync", json!({"garden": e.g1, "part": "both", "sync": on}));
        assert_eq!(s, "200 OK", "{v}");
        if v["changed"] == json!(true) {
            changed.push(on);
        }
    }
    assert!(!changed.is_empty(), "one of off/on changed Garden 1's design updates");
    let evs = events(&e.g1, &[]);
    let mut want = vec!["sync"; changed.len()];
    want.extend(["unlocked", "locked"]);
    assert_eq!(kinds(&evs), want, "{evs:?}");
    assert!(evs.iter().all(|x| x.actor == "owner"), "{evs:?}");
    assert_eq!(evs[0].detail["sync"], json!(changed.last().copied()), "{evs:?}");
    assert_eq!(evs[0].detail["part"], json!("both"), "{evs:?}");
}

#[test]
fn an_edit_to_a_locked_garden_is_put_back_with_one_line_and_no_disk_edit() {
    let e = env();
    let (s, v) = zen("/cli/zen/garden/lock", json!({"garden": e.g1, "locked": true}));
    assert_eq!(s, "200 OK", "{v}");
    let path = e.f.path_of(&ZenFile::Garden(e.g1.clone()));
    std::fs::write(&path, page_with("#ff0000")).expect("hand edit");
    crate::zen_routes::refresh_from_watcher().expect("watcher refresh");
    assert_eq!(std::fs::read_to_string(&path).expect("page"), PAGE, "the lock put the page back");
    let evs = events(&e.g1, &[]);
    assert_eq!(kinds(&evs), vec!["lock_put_back", "locked"], "{evs:?}");
    assert_eq!(evs[0].actor, "disk");
}

#[test]
fn the_watcher_logs_edits_outside_k2_and_never_a_routes_own_write() {
    let e = env();
    // A hand edit of the page: one disk_edit line, actor disk.
    std::fs::write(e.f.path_of(&ZenFile::Garden(e.g1.clone())), page_with("#112233")).expect("hand edit");
    crate::zen_routes::refresh_from_watcher().expect("watcher refresh");
    let evs = events(&e.g1, &[]);
    assert_eq!(kinds(&evs), vec!["disk_edit"], "{evs:?}");
    assert_eq!(evs[0].actor, "disk");
    assert_eq!(evs[0].detail["file"], json!(format!("gardens/{}.toml", e.g1)));
    // Nothing moved: the next watcher refresh writes nothing.
    crate::zen_routes::refresh_from_watcher().expect("watcher refresh");
    assert_eq!(events(&e.g1, &[]).len(), 1);
    // The person's put through K2: a `changed` line, and the watcher's
    // refresh after it is not an edit outside K2.
    let (s, v) = zen("/cli/zen/garden/put", json!({"garden": e.g1, "text": page_with("#445566")}));
    assert_eq!(s, "200 OK", "{v}");
    crate::zen_routes::refresh_from_watcher().expect("watcher refresh");
    assert_eq!(kinds(&events(&e.g1, &[])), vec!["changed", "disk_edit"]);
    // A hand edit of a widget file: a widget-only disk_edit line.
    let html = e.f.widget_dir("board").join("index.html");
    let old = std::fs::read_to_string(&html).expect("index.html");
    std::fs::write(&html, format!("{old}\n<!-- by hand -->\n")).expect("widget hand edit");
    crate::zen_routes::refresh_from_watcher().expect("watcher refresh");
    let db = k2_core::db::shared();
    let w = ga::events_for_widget(&db.lock(), "board", 50, None).expect("widget events");
    assert_eq!(kinds(&w), vec!["disk_edit"], "{w:?}");
    assert_eq!((w[0].garden_id.as_deref(), w[0].actor.as_str()), (None, "disk"));
    assert_eq!(w[0].detail["hash"].as_str().map(str::len), Some(12), "{w:?}");
}

#[test]
fn with_the_switch_off_the_watcher_and_lock_write_no_history() {
    let e = env_with(false);
    std::fs::write(e.f.path_of(&ZenFile::Garden(e.g1.clone())), page_with("#abcdef")).expect("hand edit");
    crate::zen_routes::refresh_from_watcher().expect("watcher refresh");
    let (s, v) = zen("/cli/zen/garden/lock", json!({"garden": e.g1, "locked": true}));
    assert_eq!(s, "200 OK", "{v}");
    std::fs::write(e.f.path_of(&ZenFile::Garden(e.g1.clone())), page_with("#fedcba")).expect("hand edit");
    crate::zen_routes::refresh_from_watcher().expect("watcher refresh");
    let db = k2_core::db::shared();
    let n: i64 = db.lock().query_row("SELECT COUNT(*) FROM zen_garden_events", [], |r| r.get(0)).expect("count");
    assert_eq!(n, 0, "nothing writes History while the switch is off");
}

// ── the merge ────────────────────────────────────────────────────────

#[test]
fn a_change_line_links_its_version_and_a_version_no_line_covers_is_its_own_line() {
    let e = env();
    let (s, v) = zen("/cli/zen/garden/put", json!({"garden": e.g1, "text": page_with("#0a0b0c")}));
    assert_eq!(s, "200 OK", "{v}");
    let h = history(GARDEN_HISTORY_ROUTE, &[("garden", &e.g1)]);
    let all = items(&h);
    let changed: Vec<&J> = all.iter().filter(|i| i["source"] == "event" && i["kind"] == "changed").collect();
    assert_eq!(changed.len(), 1, "{h}");
    let snap = &changed[0]["snapshot"];
    assert_eq!(snap["file"], json!(format!("gardens/{}.toml", e.g1)), "the change links its version: {h}");
    // View this version reads that version's text: the page the put wrote.
    let name = snap["name"].as_str().expect("snapshot name");
    let view = e.f.snapshot_view(snap["file"].as_str().expect("file"), name).expect("snapshot view");
    assert_eq!(view["text"], json!(page_with("#0a0b0c")), "{view}");
    // The widget's version from setup had no line: "version saved".
    let saved: Vec<&J> = all.iter().filter(|i| i["source"] == "snapshot").collect();
    assert!(
        saved.iter().any(|i| i["widget"] == "board" && i["kind"] == "widget_changed" && i["actor"]["kind"] == "k2"),
        "{h}"
    );
    for i in &saved {
        assert!(i["id"].as_str().is_some_and(|id| id.starts_with("snapshot:")), "{i}");
        assert!(i["snapshot"]["name"].is_string(), "{i}");
    }
    // The Garden page's own setup version is covered by nothing earlier
    // than its change line only when they are 5 s apart; either way each
    // snapshot shows once (linked or as its own line), never twice.
    let mut seen = std::collections::BTreeSet::new();
    for i in &all {
        if let Some(n) = i["snapshot"]["name"].as_str() {
            assert!(seen.insert((i["snapshot"]["file"].to_string(), n.to_string())), "shown twice: {h}");
        }
    }
    // The filters apply to merged lines too.
    let acc = history(GARDEN_HISTORY_ROUTE, &[("garden", &e.g1), ("kind", "access")]);
    assert_eq!(items(&acc), Vec::<J>::new(), "{acc}");
}

#[test]
fn stamps_and_decided_proposals_from_before_the_first_line_are_merged() {
    let e = env();
    let hash = e.f.widget_status("board").hash().to_string();
    assert_eq!(hash.len(), 64, "board bundles clean");
    let t0 = now() - 10_000;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        authors::stamp(&conn, "board", &hash, &MadeBy::Agent { project_id: "w-ed".into() }, StampSource::Sign, CallerKind::Agent, t0)
            .expect("stamp");
        for (id, status, at) in [("p-old", "approved", t0 + 1), ("p-rej", "rejected", t0 + 2), ("p-pending", "pending", 0)] {
            conn.execute(
                "INSERT INTO zen_proposals (id, kind, target, file, content, base_hash, note, status, made_by, made_by_project,
                   made_by_session, created_at, decided_by, decided_at)
                 VALUES (?1, 'garden', ?2, NULL, 'x', 'h', NULL, ?3, 'agent:editor', 'w-ed', NULL, ?4, ?5, ?6)",
                rusqlite::params![
                    id,
                    e.g1,
                    status,
                    t0,
                    (status != "pending").then_some("owner"),
                    (status != "pending").then_some(at)
                ],
            )
            .expect("proposal row");
        }
    }
    let h = history(GARDEN_HISTORY_ROUTE, &[("garden", &e.g1)]);
    let all = items(&h);
    let stamp: Vec<&J> = all.iter().filter(|i| i["source"] == "stamp").collect();
    assert_eq!(stamp.len(), 1, "{h}");
    assert_eq!(
        (&stamp[0]["kind"], &stamp[0]["widget"], &stamp[0]["actor"]["kind"], &stamp[0]["actor"]["id"]),
        (&json!("signed"), &json!("board"), &json!("agent"), &json!("w-ed")),
        "{h}"
    );
    assert_eq!(stamp[0]["detail"]["hash"], json!(&hash[..12]));
    let props: Vec<(&str, &str)> = all
        .iter()
        .filter(|i| i["source"] == "proposal")
        .map(|i| (i["id"].as_str().expect("id"), i["detail"]["decision"].as_str().expect("decision")))
        .collect();
    assert_eq!(props, vec![("proposal:p-rej", "rejected"), ("proposal:p-old", "approved")], "pending ones aren't History: {h}");
    // Newest first across sources.
    let ats: Vec<i64> = all.iter().map(|i| i["at"].as_i64().expect("at")).collect();
    assert!(ats.windows(2).all(|w| w[0] >= w[1]), "{ats:?}");
    // The widget's own History has the stamp, not the Garden's proposals.
    let w = history(WIDGET_HISTORY_ROUTE, &[("widget", "board")]);
    let srcs: Vec<String> = items(&w).iter().map(|i| i["source"].as_str().expect("source").to_string()).collect();
    assert!(srcs.iter().any(|s| s == "stamp") && !srcs.iter().any(|s| s == "proposal"), "{w}");
    // A stamp written after the widget's first line is that line's, not merged.
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        ga::record_event(&conn, On::Widget("board"), EventKind::Signed, &Actor::Owner, &json!({}), t0 - 1).expect("an older line");
    }
    let w = history(WIDGET_HISTORY_ROUTE, &[("widget", "board")]);
    assert!(items(&w).iter().all(|i| i["source"] != "stamp"), "{w}");
}

#[test]
fn merged_lines_page_with_the_events_once_each() {
    let e = env();
    let base = now();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO zen_proposals (id, kind, target, file, content, base_hash, note, status, made_by, made_by_project,
               made_by_session, created_at, decided_by, decided_at)
             VALUES ('p-1', 'garden', ?1, NULL, 'x', 'h', NULL, 'approved', 'agent:editor', 'w-ed', NULL, ?2, 'owner', ?2)",
            rusqlite::params![e.g1, base - 1000],
        )
        .expect("proposal row");
        for at in [base + 100, base + 200, base + 300] {
            ga::record_event(&conn, On::Garden(&e.g1), EventKind::Renamed, &Actor::Owner, &json!({"to": at}), at).expect("line");
        }
    }
    let mut ids = Vec::new();
    let mut pages = Vec::new();
    let mut before: Option<String> = None;
    loop {
        let mut q = vec![("garden", e.g1.as_str()), ("limit", "1")];
        if let Some(b) = before.as_deref() {
            q.push(("before", b));
        }
        let v = history(GARDEN_HISTORY_ROUTE, &q);
        let page: Vec<String> = items(&v).iter().map(|i| i["id"].as_str().expect("id").to_string()).collect();
        ids.extend(page.iter().cloned());
        pages.push(page);
        match v["next"].as_str() {
            Some(n) => before = Some(n.to_string()),
            None => break,
        }
        assert!(pages.len() < 10, "paging ends: {pages:?}");
    }
    let mut uniq = ids.clone();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), ids.len(), "each line once: {pages:?}");
    assert!(ids.iter().any(|i| i == "proposal:p-1"), "{pages:?}");
    assert!(ids.iter().any(|i| i.starts_with("snapshot:")), "{pages:?}");
    // The three newest pages hold one event each; the older sources wait
    // for the page past the last event.
    for p in &pages[..3] {
        assert_eq!(p.len(), 1, "{pages:?}");
        assert!(p[0].starts_with("event:"), "{pages:?}");
    }
    assert!(pages.last().expect("last").iter().all(|i| !i.starts_with("event:")), "{pages:?}");
}

#[test]
fn the_computer_wide_look_merges_nothing() {
    let e = env();
    let (s, v) = zen("/cli/zen/theme/next", json!({}));
    assert_eq!(s, "200 OK", "{v}");
    let h = history(GARDEN_HISTORY_ROUTE, &[("garden", ga::COMPUTER_WIDE)]);
    let all = items(&h);
    assert!(!all.is_empty(), "{h}");
    assert!(all.iter().all(|i| i["source"] == "event"), "{h}");
    drop(e);
}
