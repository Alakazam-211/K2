//! sew-db against its Postgres package ([`crate::pg_stale`]): the status
//! alarm and the upgrade's preflight.

use serde_json::json;

use super::supervisor::{no, Env, R};
use k2_core::brand::CLI_NAME;

/// sew-db runs an older Postgres than the installed package (it started
/// before the package was upgraded). `k2 hostchat upgrade` refuses until
/// it is restarted.
pub const ALARM_SEW_DB_OUTDATED: &str = "k2_sew_db_postgres_outdated";

/// `sew_db_outdated` (409): the upgrade's preflight. K2 refuses rather
/// than restart sew-db itself: the root helper has no verb that restarts
/// sew-db alone, and a restart K2 started would take chat down before the
/// upgrade's backup with nobody watching. A person restarts it and runs
/// the upgrade again.
pub fn need_sew_db_current(env: &Env) -> R<()> {
    match crate::pg_stale::check_sew_db(env.ops.pg_stale_ops()) {
        Some(f) if f.is_stale() => {
            let mut fail = no(
                "sew_db_outdated",
                409,
                format!(
                    "nothing changed: sew-db runs an older Postgres than the installed {} ({}): it started before \
                     the package was upgraded, so the upgrade's migrations would fail to load the new libraries. \
                     Restart sew-db first: sudo systemctl restart sew-db.service, then {CLI_NAME} hostchat restart, then \
                     run the upgrade again",
                    f.package.as_deref().unwrap_or("postgresql package"),
                    f.installed_version.as_deref().unwrap_or("version unknown"),
                ),
            );
            fail.extra = Some(json!({"sewDbPostgres": f.json()}));
            Err(fail)
        }
        _ => Ok(()),
    }
}
