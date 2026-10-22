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

/// `~/.k2/garden-access-on`: present only while the switch is on (A6). The
/// `k2` CLI reads it before treating `k2 zen garden access|history` and
/// `k2 zen widget history` as verbs, so with the switch off it asks this
/// daemon nothing and answers exactly as 0.46.3 did (off-golden
/// `cli_zen.txt`). The daemon stays the authority: with the file but the
/// switch off, those routes still answer as unknown.
pub const SWITCH_MARK: &str = "garden-access-on";

/// Make [`SWITCH_MARK`] match the switch (boot, and every flip). Off and
/// never on: one stat, nothing written.
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

    /// The CLI's switch mark follows every flip, and an off switch that was
    /// never on writes nothing under `~/.k2`.
    #[test]
    fn the_switch_mark_exists_only_while_the_switch_is_on() {
        let home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let mark = home.path().join(".k2").join(SWITCH_MARK);
        let was = ga::set_switch(false);
        mark_switch(false);
        assert!(!mark.exists(), "off, never on: nothing written");
        assert!(crate::settings_routes::sync_garden_access(true, "test"), "flipped on");
        assert_eq!(std::fs::read_to_string(&mark).expect("the mark while on"), "on\n");
        assert!(!crate::settings_routes::sync_garden_access(true, "test"), "already on");
        assert!(mark.exists());
        assert!(crate::settings_routes::sync_garden_access(false, "test"), "flipped off");
        assert!(!mark.exists(), "off removes the mark");
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
