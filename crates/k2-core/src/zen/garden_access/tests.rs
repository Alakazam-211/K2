//! Garden access store, History, G7, G8 and the G4 pass (prd-garden-access-v1
//! rev 5, slice A1). Every test runs under a temp `$HOME` (the Access Audit
//! and the Zen folder live there) and a fresh in-memory database. Fail
//! loudly.

use std::path::PathBuf;

use rusqlite::{params, Connection};
use serde_json::{json, Value as J};

use super::pass::{self, GardenFacts, PlacedWidget, Reason};
use super::*;
use crate::garden_data::authors::{self, CallerKind, StampSource};
use crate::garden_data::MadeBy;
use crate::test_env::TempHome;
use crate::zen::{ZenFile, ZenFiles};

const T0: i64 = 1_760_000_000;

fn db() -> Connection {
    let conn = Connection::open_in_memory().expect("memory db");
    crate::db::run_migrations(&conn).expect("migrations");
    conn
}

fn workspace(conn: &Connection, id: &str) {
    conn.execute(
        "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?3)",
        params![id, format!("/tmp/ga/{id}"), format!("h-{id}")],
    )
    .expect("workspace row");
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).expect("count")
}

fn zen(home: &TempHome) -> ZenFiles {
    let f = ZenFiles::new(home.path().join(".k2").join("zen"));
    f.setup().expect("zen setup");
    f
}

fn gid(f: &ZenFiles, name: &str) -> String {
    f.find_garden(name).unwrap_or_else(|| panic!("no Garden {name}: {:?}", f.gardens())).1.id
}

fn page_with(widget: &str) -> String {
    format!(
        "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 60\nmin-width = 200\n[[layout.column]]\nsize = 40\nmin-width = 200\n\n[[widget]]\nid = \"w1\"\nkind = \"custom\"\nwidget = \"{widget}\"\ncolumn = 0\n[widget.props]\nhome = \"Work\"\n\n[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 1\n[widget.props]\nagents = \"w1\"\n"
    )
}

fn place(f: &ZenFiles, garden: &str, widget: &str) {
    std::fs::write(f.path_of(&ZenFile::Garden(garden.into())), page_with(widget)).expect("write garden page");
}

fn audit_lines(event: &str) -> Vec<J> {
    crate::auth_audit::read_all().expect("audit").into_iter().filter(|e| e["event"] == event).collect()
}

// ── migration ────────────────────────────────────────────────────────

#[test]
fn migration_0219_makes_four_empty_tables_and_no_sight_tables() {
    let conn = db();
    for t in ["zen_garden_access", "zen_garden_made", "zen_garden_access_asks", "zen_garden_events"] {
        assert_eq!(count(&conn, t), 0, "{t} exists and starts empty");
    }
    for gone in ["zen_garden_sight", "zen_widget_legacy_reach", "zen_garden_access_log"] {
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1", params![gone], |r| r.get(0))
            .expect("sqlite_master");
        assert_eq!(n, 0, "{gone} must not exist (rev 4/5)");
    }
    let idx: Vec<String> = conn
        .prepare("SELECT name FROM sqlite_master WHERE type = 'index' AND name LIKE 'zen_garden_%' ORDER BY name")
        .expect("prepare")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("rows");
    for want in ["zen_garden_access_by_project", "zen_garden_events_by_garden", "zen_garden_events_by_widget"] {
        assert!(idx.iter().any(|i| i == want), "index {want} missing: {idx:?}");
    }
    // An event names a Garden or a widget; a bad kind is refused by SQL.
    let none = conn.execute(
        "INSERT INTO zen_garden_events (garden_id, widget, at, actor, kind, detail_json) VALUES (NULL, NULL, 1, 'k2', 'changed', '{}')",
        [],
    );
    assert!(none.is_err(), "an event with neither Garden nor widget is refused");
    let bad = conn.execute(
        "INSERT INTO zen_garden_events (garden_id, at, actor, kind, detail_json) VALUES ('g-1', 1, 'k2', 'claimed', '{}')",
        [],
    );
    assert!(bad.is_err(), "an unknown kind is refused");
    // The asks key includes kind, which defaults to 'access'.
    conn.execute("INSERT INTO zen_garden_access_asks (garden_id, project_id) VALUES ('g-1', 'w-1')", []).expect("ask");
    conn.execute("INSERT INTO zen_garden_access_asks (garden_id, project_id, kind) VALUES ('g-1', 'w-1', 'delete')", [])
        .expect("delete ask beside the access ask");
    let kinds: String = conn
        .query_row("SELECT group_concat(kind, ',') FROM (SELECT kind FROM zen_garden_access_asks ORDER BY kind)", [], |r| r.get(0))
        .expect("kinds");
    assert_eq!(kinds, "access,delete");
    let other = conn.execute("INSERT INTO zen_garden_access_asks (garden_id, project_id, kind) VALUES ('g-1', 'w-1', 'edit')", []);
    assert!(other.is_err(), "ask kind is access or delete");
}

// ── the switch ───────────────────────────────────────────────────────

#[test]
fn the_switch_defaults_off_and_set_switch_returns_the_previous_value() {
    let _env = crate::test_env::lock();
    assert!(!enabled(), "gardenAccess must default OFF");
    assert!(!set_switch(true), "was off");
    assert!(enabled());
    assert!(set_switch(false), "was on");
    assert!(!enabled());
    assert_eq!(SETTING_KEY, "gardenAccess");
}

// ── the list ─────────────────────────────────────────────────────────

#[test]
fn add_and_remove_write_rows_history_and_access_audit() {
    let _home = TempHome::new();
    let conn = db();
    workspace(&conn, "w-ana");
    workspace(&conn, "w-ops");
    let owner = Actor::Owner;
    let c = Change::new(Via::Settings, &owner, T0).route("/cli/zen/garden/access");
    assert!(add(&conn, "g-fleet", "w-ana", &c).expect("add"));
    assert!(!add(&conn, "g-fleet", "w-ana", &c).expect("add again"), "a second add is a no-op");
    let card = Actor::Login("rosson".into());
    assert!(add(&conn, "g-fleet", "w-ops", &Change::new(Via::Card, &card, T0 + 5).card("card-1")).expect("card add"));
    assert!(add(&conn, "g-sales", "w-ana", &Change::new(Via::Migration, &Actor::Migration, T0 + 6)).expect("migration"));

    let rows = list(&conn, "g-fleet").expect("list");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].project_id, "w-ana");
    assert_eq!(rows[0].added_by, AddedBy::Owner);
    assert_eq!(rows[1].added_by, AddedBy::Card);
    assert_eq!(rows[1].card_id.as_deref(), Some("card-1"));
    assert!(is_listed(&conn, "g-fleet", "w-ops").expect("listed"));
    assert!(!is_listed(&conn, "g-sales", "w-ops").expect("not listed"));
    assert!(listed_anywhere(&conn, "w-ops").expect("anywhere"));
    assert_eq!(gardens_of(&conn, "w-ana").expect("gardens"), vec!["g-fleet", "g-sales"]);
    assert!(listed_on_all(&conn, &["g-fleet", "g-sales"], "w-ana").expect("all"));
    assert!(!listed_on_all(&conn, &["g-fleet", "g-sales"], "w-ops").expect("not all"));
    assert!(!listed_on_all(&conn, &[], "w-ana").expect("empty"), "a draft is no Garden's");
    assert_eq!(all_rows(&conn).expect("all rows").len(), 3);

    // Errors: unknown workspace, a door that doesn't add, a bad Garden id.
    assert!(matches!(add(&conn, "g-fleet", "w-gone", &c), Err(AccessError::BadRequest(_))));
    assert!(matches!(add(&conn, "g-fleet", "w-ops", &Change::new(Via::Twin, &owner, T0)), Err(AccessError::BadRequest(_))));
    assert!(matches!(add(&conn, "../x", "w-ops", &c), Err(AccessError::BadRequest(_))));

    let twin = Change::new(Via::Twin, &card, T0 + 9).card("card-2");
    assert!(remove(&conn, "g-fleet", "w-ops", &twin).expect("remove"));
    assert!(!remove(&conn, "g-fleet", "w-ops", &twin).expect("remove again"), "nothing to remove");
    assert!(!is_listed(&conn, "g-fleet", "w-ops").expect("gone"));
    assert!(!listed_anywhere(&conn, "w-ops").expect("nowhere"));

    let h = events_for_garden(&conn, "g-fleet", &[], 50, None).expect("history");
    let kinds: Vec<(EventKind, &str)> = h.iter().map(|e| (e.kind, e.actor.as_str())).collect();
    assert_eq!(
        kinds,
        vec![
            (EventKind::AccessRemoved, "login:rosson"),
            (EventKind::AccessAdded, "login:rosson"),
            (EventKind::AccessAdded, "owner"),
        ],
        "newest first, one line per real change: {h:?}"
    );
    assert_eq!(h[0].detail, json!({"workspace": "w-ops", "via": "twin", "cardId": "card-2"}));
    assert_eq!(h[2].detail, json!({"workspace": "w-ana", "via": "settings", "route": "/cli/zen/garden/access"}));
    assert_eq!(h[0].garden_id.as_deref(), Some("g-fleet"));
    assert_eq!(h[0].widget, None);

    let audit = audit_lines(AUDIT_EVENT);
    assert_eq!(audit.len(), 4, "one Access Audit line per change: {audit:?}");
    assert_eq!(audit[0]["user"], "owner");
    assert_eq!(audit[0]["outcome"], "ok add garden=g-fleet workspace=w-ana via=settings route=/cli/zen/garden/access");
    assert_eq!(audit[3]["outcome"], "ok remove garden=g-fleet workspace=w-ops via=twin card=card-2");
}

// ── History ──────────────────────────────────────────────────────────

#[test]
fn history_lines_name_a_garden_a_widget_a_placement_or_the_computer_and_page() {
    let _home = TempHome::new();
    let conn = db();
    let me = Actor::Agent("w-ana".into());
    let a = record_event(&conn, On::Garden("g-1"), EventKind::Renamed, &me, &json!({"from": "A", "to": "B"}), T0)
        .expect("garden line");
    let b = record_event(&conn, On::Widget("board"), EventKind::WidgetChanged, &me, &json!({"hash": "abcdef012345"}), T0 + 1)
        .expect("widget line");
    let c = record_event(&conn, On::GardenWidget("g-1", "board"), EventKind::Signed, &Actor::Owner, &json!({}), T0 + 2)
        .expect("placement line");
    let d = record_event(&conn, On::Computer, EventKind::Theme, &Actor::Shell("w-ana".into()), &json!({"to": "basic"}), T0 + 3)
        .expect("computer-wide line");
    record_event(&conn, On::Widget("other"), EventKind::WidgetChanged, &me, &json!({}), T0 + 4).expect("other widget");

    let g = events_for_garden(&conn, "g-1", &["board"], 50, None).expect("garden history");
    assert_eq!(g.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, b, a], "the Garden's lines plus its placed widgets'");
    let g_unplaced = events_for_garden(&conn, "g-1", &[], 50, None).expect("garden history");
    assert_eq!(g_unplaced.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, a], "an unplaced widget's code lines don't show");
    let w = events_for_widget(&conn, "board", 50, None).expect("widget history");
    assert_eq!(w.iter().map(|e| e.id).collect::<Vec<_>>(), vec![c, b]);
    let star = events_for_garden(&conn, COMPUTER_WIDE, &[], 50, None).expect("computer history");
    assert_eq!(star.len(), 1);
    assert_eq!(star[0].id, d);
    assert_eq!(star[0].actor, "shell:w-ana");
    // Paging: older than the first page's last id.
    let page1 = events_for_garden(&conn, "g-1", &["board"], 2, None).expect("page 1");
    assert_eq!(page1.len(), 2);
    let page2 = events_for_garden(&conn, "g-1", &["board"], 2, Some(page1[1].id)).expect("page 2");
    assert_eq!(page2.iter().map(|e| e.id).collect::<Vec<_>>(), vec![a]);

    // Refused: a non-object detail, an oversize detail, a bad widget name.
    assert!(matches!(
        record_event(&conn, On::Garden("g-1"), EventKind::Changed, &me, &json!("x"), T0),
        Err(AccessError::BadRequest(_))
    ));
    let big = json!({ "note": "x".repeat(MAX_DETAIL_BYTES) });
    assert!(matches!(
        record_event(&conn, On::Garden("g-1"), EventKind::Changed, &me, &big, T0),
        Err(AccessError::BadRequest(_))
    ));
    assert!(matches!(
        record_event(&conn, On::Widget("../etc"), EventKind::Changed, &me, &json!({}), T0),
        Err(AccessError::BadRequest(_))
    ));
    // Every kind round-trips through the CHECK list and the parser.
    for k in EventKind::ALL {
        record_event(&conn, On::Garden("g-kinds"), k, &Actor::K2, &json!({}), T0).unwrap_or_else(|e| panic!("{k:?}: {e}"));
        assert_eq!(EventKind::parse(k.as_str()), Some(k));
    }
    assert_eq!(events_for_garden(&conn, "g-kinds", &[], 100, None).expect("kinds").len(), EventKind::ALL.len());
    // Actor labels round-trip.
    for a in [
        Actor::Owner,
        Actor::Login("ana".into()),
        Actor::Agent("w-1".into()),
        Actor::Shell("w-1".into()),
        Actor::K2,
        Actor::Disk,
        Actor::Migration,
    ] {
        assert_eq!(Actor::parse(&a.label()), Some(a.clone()), "{a:?}");
    }
    assert_eq!(Actor::parse("agent:"), None);
    assert_eq!(Actor::parse("robot:x"), None);
}

#[test]
fn the_two_year_sweep_prunes_only_older_lines() {
    let _home = TempHome::new();
    let conn = db();
    let now = T0 + 3 * 365 * 24 * 3600;
    let edge = now - EVENTS_RETAIN_SECS;
    let me = Actor::K2;
    record_event(&conn, On::Garden("g-1"), EventKind::Changed, &me, &json!({"n": 1}), edge - 1).expect("too old");
    record_event(&conn, On::Widget("board"), EventKind::WidgetChanged, &me, &json!({"n": 2}), edge - 100).expect("too old");
    record_event(&conn, On::Garden("g-1"), EventKind::Changed, &me, &json!({"n": 3}), edge).expect("exactly 2 years");
    record_event(&conn, On::Garden("g-2"), EventKind::Changed, &me, &json!({"n": 4}), now).expect("new");
    assert_eq!(EVENTS_RETAIN_SECS, 730 * 24 * 3600, "two years is 730 days");
    assert_eq!(prune_events(&conn, now).expect("prune"), 2);
    let left: Vec<i64> = conn
        .prepare("SELECT json_extract(detail_json, '$.n') FROM zen_garden_events ORDER BY id")
        .expect("prepare")
        .query_map([], |r| r.get(0))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("rows");
    assert_eq!(left, vec![3, 4]);
    assert_eq!(prune_events(&conn, now).expect("again"), 0, "a second sweep finds nothing");
}

// ── made and asks ────────────────────────────────────────────────────

#[test]
fn made_is_written_once_and_counts_toward_the_budget() {
    let conn = db();
    assert!(record_made(&conn, "g-1", "w-ana", T0).expect("made"));
    assert!(!record_made(&conn, "g-1", "w-ops", T0 + 1).expect("again"), "a Garden is made once");
    assert_eq!(made(&conn, "g-1").expect("made").map(|m| m.made_by_project), Some("w-ana".into()));
    assert_eq!(made(&conn, "g-2").expect("none"), None);
    record_made(&conn, "g-2", "w-ana", T0 + 100).expect("made 2");
    record_made(&conn, "g-3", "w-ana", T0 - 90_000).expect("yesterday");
    assert_eq!(made_since(&conn, "w-ana", T0 - 86_400).expect("budget"), 2);
    assert_eq!(MADE_PER_DAY, 5);
}

#[test]
fn asks_are_per_kind_and_quiet_for_a_day_after_not_now() {
    let conn = db();
    assert!(may_ask(None, T0), "never asked → may ask");
    record_ask(&conn, "g-1", "w-ana", AskKind::Access, "card-a", T0).expect("ask");
    let row = ask_state(&conn, "g-1", "w-ana", AskKind::Access).expect("state").expect("row");
    assert_eq!(row.card_id.as_deref(), Some("card-a"));
    assert!(!may_ask(Some(&row), T0 + 10), "a card is waiting");
    assert_eq!(ask_state(&conn, "g-1", "w-ana", AskKind::Delete).expect("state"), None, "kinds are separate");
    record_ask(&conn, "g-1", "w-ana", AskKind::Delete, "card-d", T0).expect("delete ask");
    record_not_now(&conn, "g-1", "w-ana", AskKind::Access, T0 + 20).expect("not now");
    let row = ask_state(&conn, "g-1", "w-ana", AskKind::Access).expect("state").expect("row");
    assert_eq!((row.card_id.as_deref(), row.last_not_now_at, row.last_ask_at), (None, Some(T0 + 20), Some(T0)));
    assert!(!may_ask(Some(&row), T0 + 20 + NOT_NOW_QUIET_SECS - 1), "quiet for 24 h");
    assert!(may_ask(Some(&row), T0 + 20 + NOT_NOW_QUIET_SECS), "then may ask again");
    let del = ask_state(&conn, "g-1", "w-ana", AskKind::Delete).expect("state").expect("row");
    assert_eq!(del.card_id.as_deref(), Some("card-d"), "Not now on access leaves the delete ask");
    clear_ask_card(&conn, "g-1", "w-ana", AskKind::Delete).expect("clear");
    let del = ask_state(&conn, "g-1", "w-ana", AskKind::Delete).expect("state").expect("row");
    assert!(may_ask(Some(&del), T0 + 1));
}

// ── G7 ───────────────────────────────────────────────────────────────

#[test]
fn deleting_a_workspace_drops_its_rows_and_keeps_history() {
    let _home = TempHome::new();
    let _db = crate::db::scoped_for_test();
    {
        let h = crate::db::shared();
        let conn = h.lock();
        workspace(&conn, "w-gone");
        workspace(&conn, "w-stay");
        let owner = Actor::Owner;
        for g in ["g-a", "g-b"] {
            add(&conn, g, "w-gone", &Change::new(Via::Settings, &owner, T0)).expect("add");
        }
        add(&conn, "g-a", "w-stay", &Change::new(Via::Settings, &owner, T0)).expect("add");
        record_made(&conn, "g-a", "w-gone", T0).expect("made");
        record_ask(&conn, "g-b", "w-gone", AskKind::Access, "c-1", T0).expect("ask");
        record_ask(&conn, "g-b", "w-stay", AskKind::Delete, "c-2", T0).expect("ask");
    }
    crate::projects_ops::projects_delete("w-gone").expect("delete workspace");
    let h = crate::db::shared();
    let conn = h.lock();
    assert_eq!(gardens_of(&conn, "w-gone").expect("gardens"), Vec::<String>::new());
    assert_eq!(gardens_of(&conn, "w-stay").expect("gardens"), vec!["g-a"], "others keep their rows");
    assert_eq!(made(&conn, "g-a").expect("made"), None);
    assert_eq!(ask_state(&conn, "g-b", "w-gone", AskKind::Access).expect("ask"), None);
    assert!(ask_state(&conn, "g-b", "w-stay", AskKind::Delete).expect("ask").is_some());
    let hist = events_for_garden(&conn, "g-a", &[], 50, None).expect("history");
    assert_eq!(hist.len(), 3, "both adds and the removal stay: {hist:?}");
    assert_eq!(hist[0].kind, EventKind::AccessRemoved);
    assert_eq!(hist[0].actor, "k2");
    assert_eq!(hist[0].detail, json!({"workspace": "w-gone", "via": "workspace_delete"}));
}

// ── G8 ───────────────────────────────────────────────────────────────

#[test]
fn garden_delete_mirrors_rows_to_history_and_restore_puts_them_back() {
    let home = TempHome::new();
    let f = zen(&home);
    let conn = db();
    workspace(&conn, "w-ana");
    workspace(&conn, "w-ops");
    let g = gid(&f, "Garden 2");
    // A Garden with no rows: deleting it writes no access.json (0.46.3 bytes).
    let g1 = gid(&f, "Garden 1");
    f.delete_garden(&g1).expect("delete garden 1");
    assert_eq!(archive_garden(&conn, &f, &g1, &Actor::Owner, T0).expect("archive empty"), None);
    assert!(!mirror_path(&f, &g1).exists(), "no rows → no file");

    let owner = Actor::Owner;
    add(&conn, &g, "w-ana", &Change::new(Via::Settings, &owner, T0)).expect("add");
    add(&conn, &g, "w-ops", &Change::new(Via::Card, &owner, T0 + 1).card("card-9")).expect("add");
    record_made(&conn, &g, "w-ana", T0 - 5).expect("made");
    record_ask(&conn, &g, "w-ops", AskKind::Delete, "c-del", T0).expect("ask");
    f.delete_garden(&g).expect("delete garden 2");
    let path = archive_garden(&conn, &f, &g, &owner, T0 + 10).expect("archive").expect("a file");
    assert_eq!(path, mirror_path(&f, &g));
    assert!(path.ends_with(PathBuf::from(format!("gardens/{g}.toml/access.json"))), "{}", path.display());
    let archived: Archived = serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("parse");
    assert_eq!(archived.access.len(), 2);
    assert_eq!(archived.made.as_ref().map(|m| m.made_by_project.as_str()), Some("w-ana"));
    assert_eq!(list(&conn, &g).expect("list"), vec![], "rows left the table");
    assert_eq!(made(&conn, &g).expect("made"), None);
    assert_eq!(ask_state(&conn, &g, "w-ops", AskKind::Delete).expect("ask"), None);
    let before = events_for_garden(&conn, &g, &[], 50, None).expect("history");
    assert_eq!(before.len(), 4, "2 adds + 2 removes, all kept: {before:?}");
    assert!(before[..2].iter().all(|e| e.kind == EventKind::AccessRemoved && e.detail["via"] == "garden_delete"));

    // A workspace deleted since is skipped on restore.
    conn.execute("DELETE FROM projects WHERE id = 'w-ops'", []).expect("drop w-ops");
    assert_eq!(restore_garden(&conn, &f, &g, &owner, T0 + 20).expect("restore"), 1);
    let rows = list(&conn, &g).expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0].project_id.as_str(), rows[0].added_by, rows[0].added_at), ("w-ana", AddedBy::Owner, T0));
    assert_eq!(made(&conn, &g).expect("made").map(|m| m.made_at), Some(T0 - 5));
    let after = events_for_garden(&conn, &g, &[], 50, None).expect("history");
    assert_eq!(after[0].kind, EventKind::AccessAdded);
    assert_eq!(after[0].detail["via"], "garden_restore");
    assert_eq!(restore_garden(&conn, &f, &g, &owner, T0 + 30).expect("again"), 0, "restore is idempotent");
    assert_eq!(restore_garden(&conn, &f, "g-never", &owner, T0).expect("no file"), 0);
}

// ── G4 pass ──────────────────────────────────────────────────────────

fn hash_of(f: &ZenFiles, widget: &str) -> String {
    let h = f.widget_status(widget).hash().to_string();
    assert_eq!(h.len(), 64, "{widget} bundles clean");
    h
}

fn audit_rec(event: &str, user: &str, outcome: &str, ts: &str) -> J {
    json!({ "ts": ts, "event": event, "user": user, "outcome": outcome, "ingress": "-", "ip": "-", "client": "cli" })
}

#[test]
fn the_pass_lists_every_builder_and_editor_records_made_and_imports_once() {
    let home = TempHome::new();
    let f = zen(&home);
    let conn = db();
    for w in ["w-maker", "w-editor", "w-proposer", "w-newer", "w-idle"] {
        workspace(&conn, w);
    }
    // w-idle holds the Manage Gardens switch but edited nothing (G6).
    conn.execute("UPDATE projects SET agents_can_manage_gardens = 1 WHERE id = 'w-idle'", []).expect("switch");
    let g1 = gid(&f, "Garden 1");
    let g2 = gid(&f, "Garden 2");
    f.new_widget("alpha", None).expect("alpha");
    f.new_widget("beta", None).expect("beta");
    f.new_widget("gamma", None).expect("gamma");
    place(&f, &g1, "alpha");
    place(&f, &g2, "beta");
    let (ha, hb, hg) = (hash_of(&f, "alpha"), hash_of(&f, "beta"), hash_of(&f, "gamma"));
    let agent = |p: &str| MadeBy::Agent { project_id: p.into() };
    authors::stamp(&conn, "alpha", &ha, &agent("w-maker"), StampSource::Sign, CallerKind::Agent, T0).expect("stamp");
    authors::stamp(&conn, "beta", &hb, &MadeBy::Owner, StampSource::New, CallerKind::Owner, T0).expect("stamp");
    // gamma is placed nowhere: its maker isn't a candidate anywhere.
    authors::stamp(&conn, "gamma", &hg, &agent("w-idle"), StampSource::Sign, CallerKind::Agent, T0).expect("stamp");
    // An approved widget proposal for beta (placed in Garden 2), and a
    // rejected one for Garden 1 that counts for nothing.
    conn.execute_batch(&format!(
        "INSERT INTO zen_proposals (id, kind, target, file, content, base_hash, status, made_by, made_by_project, created_at, decided_by, decided_at)
           VALUES ('p-1', 'widget', 'beta', 'index.html', 'x', 'b', 'approved', 'agent:h-w-proposer', 'w-proposer', 1, 'owner', 2),
                  ('p-2', 'garden', '{g1}', NULL, 'x', 'b', 'rejected', 'agent:h-w-newer', 'w-newer', 1, 'owner', 2);"
    ))
    .expect("proposals");
    let audit = vec![
        audit_rec("zen-garden-put", "agent:h-w-editor", &format!("ok target=gardens/{g1}.toml"), "2026-10-01T10:00:00.000Z"),
        audit_rec("zen-widget-folder-put", "shell:w-editor", "ok target=widgets/beta", "2026-10-01T10:01:00.000Z"),
        audit_rec("zen-garden-new", "agent:h-w-newer", &format!("ok target={g2} session=s-1"), "2026-10-02T09:00:00.000Z"),
        // Not an ok outcome, an unknown agent, a non-Zen event: ignored.
        audit_rec("zen-garden-put", "agent:h-w-proposer", "denied target=gardens/x.toml", "2026-10-03T00:00:00.000Z"),
        audit_rec("zen-garden-put", "agent:h-nobody", &format!("ok target=gardens/{g1}.toml"), "2026-10-03T00:00:00.000Z"),
        audit_rec("login", "ana", "ok", "2026-10-03T00:00:00.000Z"),
    ];
    let facts = pass::gather(&f);
    let g1_facts = facts.iter().find(|g| g.id == g1).expect("Garden 1 facts");
    assert_eq!(g1_facts.placed, vec![PlacedWidget { widget: "alpha".into(), hash: ha.clone() }]);

    let report = pass::run_with(&conn, &facts, &audit, T0 + 1000).expect("pass");
    assert!(pass::GRANDFATHER);
    let mut on1: Vec<String> = list(&conn, &g1).expect("list").into_iter().map(|r| r.project_id).collect();
    on1.sort();
    assert_eq!(on1, vec!["w-editor", "w-maker"], "Garden 1: alpha's maker and the audit editor");
    let mut on2: Vec<String> = list(&conn, &g2).expect("list").into_iter().map(|r| r.project_id).collect();
    on2.sort();
    assert_eq!(on2, vec!["w-editor", "w-proposer"], "Garden 2: beta's folder editor and approved proposer");
    assert!(!listed_anywhere(&conn, "w-idle").expect("idle"), "G6: a switch holder that edited nothing isn't listed");
    assert!(!listed_anywhere(&conn, "w-newer").expect("newer"), "making a Garden isn't editing it");
    assert!(all_rows(&conn).expect("rows").iter().all(|r| r.added_by == AddedBy::Migration && r.added_at == T0 + 1000));
    assert_eq!(report.listed, 4);
    let maker = report.candidates.iter().find(|c| c.project_id == "w-maker").expect("maker candidate");
    assert_eq!(maker.reasons, vec![Reason::LiveMaker { widget: "alpha".into() }]);
    assert!(maker.listed && !maker.already && !maker.removed_by_person);

    // Each auto-add has one History line, "added automatically at upgrade".
    let h1 = events_for_garden(&conn, &g1, &["alpha"], 50, None).expect("history");
    let adds: Vec<&Event> = h1.iter().filter(|e| e.kind == EventKind::AccessAdded).collect();
    assert_eq!(adds.len(), 2);
    for e in &adds {
        assert_eq!(e.actor, "migration");
        assert_eq!(e.detail["note"], pass::AUTO_NOTE);
        assert_eq!(e.detail["via"], "migration");
    }
    // Made: Garden 2, by w-newer, at the audit time, with a `made` line.
    let m = made(&conn, &g2).expect("made").expect("row");
    assert_eq!((m.made_by_project.as_str(), m.made_at), ("w-newer", 1_790_931_600));
    assert_eq!(report.made, 1);
    let h2 = events_for_garden(&conn, &g2, &["beta"], 50, None).expect("history");
    assert!(h2.iter().any(|e| e.kind == EventKind::Made && e.actor == "agent:w-newer"), "{h2:?}");
    // Imported: the ok zen-* lines naming each Garden (2 for G1, the
    // unknown agent's included; 2 for G2), never the login or a denial.
    let imported = |h: &[Event]| h.iter().filter(|e| e.kind == EventKind::Imported).count();
    assert_eq!(imported(&h1), 2, "{h1:?}");
    assert_eq!(imported(&h2), 2, "{h2:?}");
    assert_eq!(report.imported, 4);
    assert_eq!(audit_lines(crate::zen::garden_access::AUDIT_PASS_EVENT).len(), 1);

    // A second run (and off→on→off→on) adds nothing new.
    let again = pass::run_with(&conn, &pass::gather(&f), &audit, T0 + 2000).expect("pass again");
    assert_eq!((again.listed, again.made, again.imported), (0, 0, 0));
    assert!(again.candidates.iter().all(|c| c.already && !c.listed));
    assert_eq!(count(&conn, "zen_garden_access"), 4);
    let total_events = count(&conn, "zen_garden_events");
    pass::run_with(&conn, &pass::gather(&f), &audit, T0 + 3000).expect("third");
    assert_eq!(count(&conn, "zen_garden_events"), total_events, "History grows by nothing");

    // The person's removal sticks: a later off→on never re-lists that
    // agent on that Garden (K2's own removals don't count).
    remove(&conn, &g1, "w-maker", &Change::new(Via::Settings, &Actor::Owner, T0 + 4000)).expect("remove");
    let rerun = pass::run_with(&conn, &pass::gather(&f), &audit, T0 + 5000).expect("rerun");
    assert_eq!(rerun.listed, 0, "removed by the person: not re-added");
    let c = rerun.candidates.iter().find(|c| c.project_id == "w-maker").expect("still a candidate");
    assert!(c.removed_by_person && !c.listed && !c.already);
    assert!(!is_listed(&conn, &g1, "w-maker").expect("listed"));
}

#[test]
fn the_pass_skips_unsigned_and_broken_widgets_and_gone_makers() {
    let home = TempHome::new();
    let f = zen(&home);
    let conn = db();
    workspace(&conn, "w-live");
    let g1 = gid(&f, "Garden 1");
    f.new_widget("solo", None).expect("solo");
    place(&f, &g1, "solo");
    // Unsigned: nobody is a candidate.
    let facts = vec![GardenFacts { id: g1.clone(), placed: vec![PlacedWidget { widget: "solo".into(), hash: hash_of(&f, "solo") }] }];
    let r = pass::run_with(&conn, &facts, &[], T0).expect("pass");
    assert_eq!((r.listed, r.candidates.len()), (0, 0));
    // Stamped by a workspace that is gone: maker_of reads Unclaimed.
    authors::stamp(
        &conn,
        "solo",
        &hash_of(&f, "solo"),
        &MadeBy::Agent { project_id: "w-deleted".into() },
        StampSource::Sign,
        CallerKind::Agent,
        T0,
    )
    .expect("stamp");
    let r = pass::run_with(&conn, &facts, &[], T0).expect("pass");
    assert_eq!(r.listed, 0);
    // A broken folder (empty hash) contributes nothing either.
    let broken = vec![GardenFacts { id: g1.clone(), placed: vec![PlacedWidget { widget: "solo".into(), hash: String::new() }] }];
    assert_eq!(pass::run_with(&conn, &broken, &[], T0).expect("pass").candidates.len(), 0);
    assert_eq!(count(&conn, "zen_garden_access"), 0);
}

#[test]
fn audit_targets_parse_only_ok_outcomes() {
    assert_eq!(pass::audit_target("ok target=gardens/g-1.toml session=s"), Some("gardens/g-1.toml"));
    assert_eq!(pass::audit_target("ok target=widgets/board/index.html"), Some("widgets/board/index.html"));
    assert_eq!(pass::audit_target("denied target=gardens/g-1.toml"), None);
    assert_eq!(pass::audit_target("ok target="), None);
}
