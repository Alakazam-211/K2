//! The home database switch alone (prd-k2-db-rename-v1 DB-A8): runs exactly
//! what the daemon runs before its first open (the switch, then the open,
//! migrations and seeds of `init_home_database`) under `$HOME/.k2`, prints
//! the result as JSON and exits. No listener, no agents, no Apps, no mail,
//! no keychain: it is safe on a copy of a real database.
//!
//!   HOME=/tmp/x cargo run -p k2-core --example db_home_switch [-- --control]
//!
//! `--control` runs the same open with the switch deferred (today's pick),
//! for a control run's row-count deltas. `K2_TEST_DB_SWITCH_CRASH=<point>`
//! (debug builds) aborts the switch at `before_rename`, `after_rename` or
//! `after_link`. Exit 0 on success, 2 when the database would not open.

fn main() {
    let control = std::env::args().any(|a| a == "--control");
    let opts = k2_core::db::home_switch::SwitchOpts {
        defer: control.then(|| "control run (switch off)".to_string()),
        by: format!("db_home_switch harness {}", env!("CARGO_PKG_VERSION")),
    };
    match k2_core::db::init_home_database(&opts) {
        Ok((_, report)) => {
            let rep = report.expect("first init in this process");
            let out = serde_json::json!({
                "ok": true,
                "homeDb": k2_core::db::home_switch::status_json(),
                "path": rep.path.display().to_string(),
                "quarantined": rep.quarantined.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "notes": rep.notes,
            });
            println!("{out}");
        }
        Err(e) => {
            println!("{}", serde_json::json!({ "ok": false, "error": e.to_string() }));
            std::process::exit(2);
        }
    }
}
