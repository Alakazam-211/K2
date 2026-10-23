//! Garden access, the daemon glue A1 owns (prd-garden-access-v1 rev 5).
//! The store is `k2_core::zen::garden_access`; the routes, resolver and
//! cards are A2/A4's.
//!
//! - [`actor_of`]: a Zen route's caller as History's actor label. A2–A6
//!   use it for every `record_event` / list change made from a route.
//! - [`on_garden_deleted`]: G8, called by `zen_routes` after a Garden
//!   delete succeeds: the list and made rows go to `access.json`, and (with
//!   the switch on) History gets a `deleted` line. With the switch never
//!   on there are no rows and nothing is written: 0.46.3 byte for byte.
//! - [`spawn_sweep`] / [`sweep_once`]: G29a, the daily History sweep (2
//!   years). A plain daemon thread, so it runs headless with no client
//!   attached, switch on or off (it only deletes).

use std::time::Duration;

use k2_core::zen::garden_access::{self as ga, Actor, EventKind, On};

use crate::sidecar_routes::Caller;

/// `~/.k2/garden-access-on`: written at boot (Garden access is always on
/// from 0.46.3). It also records that this install's one-time upgrade pass
/// ran: [`boot`] runs the pass only while the file is missing. The `k2`
/// CLI reads it before treating `k2 zen garden access|history` and `k2 zen
/// widget history` as verbs against this computer's daemon (no file: an
/// older daemon, so it asks nothing).
pub const SWITCH_MARK: &str = "garden-access-on";

/// Write (or, in tests of the retired path, remove) [`SWITCH_MARK`].
pub fn mark_switch(on: bool) {
    let path = k2_core::paths::k2_home().join(SWITCH_MARK);
    let r = if on {
        std::fs::create_dir_all(k2_core::paths::k2_home())
            .and_then(|_| std::fs::write(&path, "on\n"))
    } else if path.exists() {
        std::fs::remove_file(&path)
    } else {
        Ok(())
    };
    if let Err(e) = r {
        k2_core::log_debug!("[garden-access] switch mark {}: {e}", path.display());
    }
}

/// Daemon boot (0.46.3, Rosson 2026-10-23: no on/off switch). Garden access
/// is on in this process, always. The first boot of an install that hasn't
/// run the upgrade pass yet (no [`SWITCH_MARK`]: a 0.46.2 install, a fresh
/// one, or one whose switch was off in an earlier 0.46.3 build) runs it
/// now, the same work an off→on flip did: the G4 pass (with
/// `pass::GRANDFATHER`, each agent that built or edited in a Garden is
/// listed with an "added automatically at upgrade" History line), the old
/// Keep-as-mine/proposal cards close, every workspace's k2-zen skill is
/// rewritten, and one Access Audit line. Then the mark is written, so a
/// later boot runs nothing. Returns whether the pass ran.
pub fn boot() -> bool {
    ga::set_switch(true);
    let mark = k2_core::paths::k2_home().join(SWITCH_MARK);
    if mark.exists() {
        return false;
    }
    crate::settings_routes::garden_access_flipped(true, "k2");
    mark_switch(true);
    true
}

/// How often the History sweep runs.
pub const SWEEP_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// The first sweep waits this long after boot (never on the boot path).
pub const SWEEP_FIRST_AFTER: Duration = Duration::from_secs(10 * 60);

/// A Zen route's caller as History's `actor`: the owner token → `owner`; a
/// login → `login:<user>`; an agent passport → `agent:<workspace id>`; a
/// shell tab → `shell:<workspace id>` (the person typing). Anything else
/// (no route lets one write) → `k2`.
pub fn actor_of(owner: bool, caller: &Caller) -> Actor {
    match caller {
        Caller::Login { username } => Actor::Login(username.clone()),
        Caller::Agent(a) => Actor::Agent(a.project_id.clone()),
        Caller::Shell { project_id } => Actor::Shell(project_id.clone()),
        Caller::Owner => Actor::Owner,
        Caller::WrongCredential | Caller::Invalid if owner => Actor::Owner,
        Caller::WrongCredential | Caller::Invalid => Actor::K2,
    }
}

/// G8: Garden `garden` was just deleted (`ZenFiles::delete_garden`
/// succeeded). Mirrors its list and made rows to `access.json` and removes
/// them; with the switch on, adds a `deleted` History line. The delete has
/// already happened, so a store error is logged, not returned.
pub fn on_garden_deleted(owner: bool, caller: &Caller, garden: &str, name: &str) {
    let actor = actor_of(owner, caller);
    let now = chrono::Utc::now().timestamp();
    let files = k2_core::zen::ZenFiles::local();
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Err(e) = ga::archive_garden(&conn, &files, garden, &actor, now) {
        k2_core::log_debug!("[garden-access] archive of deleted Garden {garden} failed: {e}");
    }
    if ga::enabled() {
        let detail = serde_json::json!({ "name": name });
        if let Err(e) = ga::record_event(&conn, On::Garden(garden), EventKind::Deleted, &actor, &detail, now) {
            k2_core::log_debug!("[garden-access] History line for deleted Garden {garden} failed: {e}");
        }
    }
}

/// One History sweep at `now`: lines older than two years go. Returns how
/// many went.
pub fn sweep_once(now: i64) -> Result<usize, ga::AccessError> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    ga::prune_events(&conn, now)
}

/// Start the daily sweep (daemon boot). Headless: a plain thread, no
/// client needed.
pub fn spawn_sweep() {
    std::thread::Builder::new()
        .name("garden-history-sweep".into())
        .spawn(|| {
            std::thread::sleep(SWEEP_FIRST_AFTER);
            loop {
                if k2_core::db::try_shared().is_some() {
                    match sweep_once(chrono::Utc::now().timestamp()) {
                        Ok(0) => {}
                        Ok(n) => k2_core::log_debug!("[garden-access] History sweep: {n} line(s) older than 2 years removed"),
                        Err(e) => k2_core::log_debug!("[garden-access] History sweep failed: {e}"),
                    }
                }
                std::thread::sleep(SWEEP_EVERY);
            }
        })
        .map(|_| ())
        .unwrap_or_else(|e| k2_core::log_debug!("[garden-access] could not start the History sweep: {e}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sidecar_routes::AgentCaller;
    use serde_json::json;

    #[test]
    fn actor_labels_follow_the_caller() {
        let agent = Caller::Agent(AgentCaller { project_id: "w-1".into(), map_key: None, args: vec![], sandboxed: false });
        assert_eq!(actor_of(false, &agent).label(), "agent:w-1");
        assert_eq!(actor_of(false, &Caller::Shell { project_id: "w-2".into() }).label(), "shell:w-2");
        assert_eq!(actor_of(true, &Caller::Login { username: "ana".into() }).label(), "login:ana");
        assert_eq!(actor_of(true, &Caller::Owner).label(), "owner");
        assert_eq!(actor_of(true, &Caller::Invalid).label(), "owner");
        assert_eq!(actor_of(false, &Caller::Invalid).label(), "k2");
    }

    /// G29a: the daily sweep drops History older than two years, from the
    /// shared database the daemon uses, and keeps the rest.
    #[test]
    fn the_sweep_prunes_history_older_than_two_years() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let now = 1_900_000_000;
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            for (at, n) in [(now - ga::EVENTS_RETAIN_SECS - 1, 1), (now - ga::EVENTS_RETAIN_SECS, 2), (now - 60, 3)] {
                ga::record_event(&conn, On::Garden("g-sweep"), EventKind::Changed, &Actor::K2, &json!({ "n": n }), at)
                    .expect("event");
            }
        }
        assert_eq!(sweep_once(now).expect("sweep"), 1);
        assert_eq!(sweep_once(now).expect("sweep again"), 0);
        let h = k2_core::db::shared();
        let conn = h.lock();
        let left = ga::events_for_garden(&conn, "g-sweep", &[], 10, None).expect("history");
        let ns: Vec<i64> = left.iter().map(|e| e.detail["n"].as_i64().expect("n")).collect();
        assert_eq!(ns, vec![3, 2]);
        assert_eq!(SWEEP_EVERY, Duration::from_secs(86_400), "daily");
    }

    /// Upgrade from 0.46.2 (Rosson 2026-10-23: Garden access is always on,
    /// no switch). A 0.46.2-shaped install: a settings file with no
    /// `gardenAccess`, a database whose Garden access tables are empty (0219
    /// just ran), a Garden placing a widget an agent built, and an Access
    /// Audit edit by another agent. The first boot turns the flow on and
    /// runs the pass once: both agents are listed (`added_by = migration`)
    /// with an "added automatically at upgrade" History line each, the old
    /// audit line is imported, and the mark is written. A second boot runs
    /// nothing and adds nothing.
    #[test]
    fn an_upgrade_from_0462_grandfathers_editors_once_at_boot() {
        let home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let _env = k2_core::test_env::lock();
        let was = ga::set_switch(false);
        let mark = home.path().join(".k2").join(SWITCH_MARK);
        assert!(!mark.exists(), "0.46.2 had no mark");
        std::fs::create_dir_all(home.path().join(".k2")).expect("k2 home");
        let f = k2_core::zen::ZenFiles::local();
        f.setup().expect("zen setup");
        let g = f.find_garden("Garden 1").expect("Garden 1").1.id;
        f.new_widget("tally", None).expect("widget");
        std::fs::write(
            f.path_of(&k2_core::zen::ZenFile::Garden(g.clone())),
            "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 60\nmin-width = 200\n[[layout.column]]\nsize = 40\nmin-width = 200\n\n[[widget]]\nid = \"w1\"\nkind = \"custom\"\nwidget = \"tally\"\ncolumn = 0\n[widget.props]\nhome = \"Work\"\n\n[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 1\n[widget.props]\nagents = \"w1\"\n",
        )
        .expect("page");
        let hash = f.widget_status("tally").hash().to_string();
        assert_eq!(hash.len(), 64, "tally bundles clean");
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            for id in ["w-builder", "w-editor"] {
                conn.execute(
                    "INSERT INTO projects (id, name, path) VALUES (?1, ?1, '/tmp/' || ?1)",
                    rusqlite::params![id],
                )
                .expect("ws");
            }
            k2_core::garden_data::authors::stamp(
                &conn,
                "tally",
                &hash,
                &k2_core::garden_data::MadeBy::Agent { project_id: "w-builder".into() },
                k2_core::garden_data::authors::StampSource::Sign,
                k2_core::garden_data::authors::CallerKind::Agent,
                1,
            )
            .expect("stamp");
            for t in ["zen_garden_access", "zen_garden_events", "zen_garden_made"] {
                let n: i64 = conn.query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0)).expect("count");
                assert_eq!(n, 0, "{t}: empty after the 0219 migration");
            }
        }
        // 0.46.2's Manage-Gardens edit by another agent, in the Access Audit.
        k2_core::auth_audit::try_record(&k2_core::auth_audit::AuditEvent::new(
            "zen-garden-put",
            "agent:w-editor",
            format!("ok target=gardens/{g}.toml"),
            "-",
            "-",
            "cli",
        ))
        .expect("audit line");
        std::fs::write(home.path().join(".k2").join("settings.json"), r#"{"defaultAgent":"codex"}"#)
            .expect("0.46.2 settings file");
        assert!(k2_core::app_settings::load().garden_access, "a 0.46.2 file reads on");

        assert!(boot(), "the first boot runs the upgrade pass");
        assert!(ga::enabled(), "on in this process");
        assert_eq!(std::fs::read_to_string(&mark).expect("mark"), "on\n");
        let (rows, events) = {
            let h = k2_core::db::shared();
            let conn = h.lock();
            let rows = ga::list(&conn, &g).expect("list");
            let events = ga::events_for_garden(&conn, &g, &["tally"], 50, None).expect("history");
            (rows, events)
        };
        let mut listed: Vec<(String, &str)> = rows.iter().map(|r| (r.project_id.clone(), r.added_by.as_str())).collect();
        listed.sort();
        assert_eq!(
            listed,
            vec![("w-builder".to_string(), "migration"), ("w-editor".to_string(), "migration")],
            "the builder and the editor are grandfathered"
        );
        let added: Vec<&ga::Event> = events.iter().filter(|e| e.kind == EventKind::AccessAdded).collect();
        assert_eq!(added.len(), 2, "{events:?}");
        for e in &added {
            assert_eq!(e.actor, "migration", "{e:?}");
            assert_eq!(e.detail["note"], ga::pass::AUTO_NOTE, "{e:?}");
        }
        assert_eq!(events.iter().filter(|e| e.kind == EventKind::Imported).count(), 1, "{events:?}");
        let switch_lines = || {
            k2_core::auth_audit::read_all()
                .expect("audit")
                .into_iter()
                .filter(|e| e["event"] == ga::AUDIT_SWITCH_EVENT)
                .count()
        };
        assert_eq!(switch_lines(), 1, "one Access Audit line for the upgrade");

        // A later boot: nothing runs, nothing is added.
        assert!(!boot(), "the mark is there: no second pass");
        let h = k2_core::db::shared();
        let conn = h.lock();
        let n: i64 = conn.query_row("SELECT COUNT(*) FROM zen_garden_events", [], |r| r.get(0)).expect("count");
        assert_eq!(n as usize, events.len(), "a second boot adds no History");
        assert_eq!(ga::list(&conn, &g).expect("list").len(), 2);
        drop(conn);
        assert_eq!(switch_lines(), 1);
        ga::set_switch(was);
    }

    /// G8 through the real route: with the switch off and no rows, a Garden
    /// delete writes no `access.json` and no History (0.46.3 bytes); with
    /// rows, they are mirrored and removed, and with the switch on History
    /// gets `deleted`.
    #[test]
    fn a_garden_delete_mirrors_rows_and_writes_history_only_when_on() {
        let _home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let _env = k2_core::test_env::lock();
        let was = ga::set_switch(false);
        let f = k2_core::zen::ZenFiles::local();
        f.setup().expect("zen setup");
        let id = |name: &str| f.find_garden(name).unwrap_or_else(|| panic!("no {name}")).1.id;
        let (g1, g2) = (id("Garden 1"), id("Garden 2"));
        let del = |g: &str| {
            let r = crate::zen_routes::handle(
                "/cli/zen/garden/delete",
                true,
                &Caller::Owner,
                "test",
                &std::collections::HashMap::new(),
                json!({ "garden": g }).to_string().as_bytes(),
            );
            assert_eq!(r.status, "200 OK", "{}", r.body);
        };
        del(&g1);
        assert!(!ga::mirror_path(&f, &g1).exists(), "off, no rows: no access.json");
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            let n: i64 = conn.query_row("SELECT COUNT(*) FROM zen_garden_events", [], |r| r.get(0)).expect("count");
            assert_eq!(n, 0, "off: History stays empty");
            conn.execute("INSERT INTO projects (id, name, path) VALUES ('w-ed', 'ed', '/tmp/w-ed')", []).expect("ws");
            ga::add(&conn, &g2, "w-ed", &ga::Change::new(ga::Via::Settings, &Actor::Owner, 1)).expect("add");
        }
        ga::set_switch(true);
        del(&g2);
        ga::set_switch(was);
        assert!(ga::mirror_path(&f, &g2).exists(), "rows are mirrored");
        let h = k2_core::db::shared();
        let conn = h.lock();
        assert_eq!(ga::list(&conn, &g2).expect("list"), vec![]);
        let hist = ga::events_for_garden(&conn, &g2, &[], 10, None).expect("history");
        assert_eq!(hist[0].kind, EventKind::Deleted);
        assert_eq!(hist[0].actor, "owner");
        assert_eq!(hist[1].kind, EventKind::AccessRemoved);
        assert_eq!(hist[1].detail["via"], "garden_delete");
    }
}
