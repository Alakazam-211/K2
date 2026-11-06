//! The daemon's half of `k2_core::sew_chat::install`: which Sew install the
//! main DB's chat ids (agent Spaces, hub cursors) belong to. The grant-hook
//! sweep calls [`sync`] with the install id Sew reports; a purge calls
//! [`forget`]. Both log once when they move anything.

use k2_core::sew_chat::install::{self, InstallReport};

/// When the current install was made (`sew_server.installed_at`).
fn installed_at() -> Option<i64> {
    crate::sew::store::load().and_then(|r| r.installed_at)
}

fn log(rep: &InstallReport, what: &str) {
    if !rep.changed_anything() {
        return;
    }
    let slugs: Vec<&str> = rep.stale.iter().map(|s| s.slug.as_str()).collect();
    k2_core::log_debug!(
        "[sew-install] {what}: {} agent Space(s) from an earlier install set aside ({}), {} hub cursor(s) reset. \
         k2 voice repair lists them and how to make them again",
        rep.stale.len(),
        slugs.join(", "),
        rep.cursors_reset
    );
}

/// Sew reports install `install`: set aside what an earlier install left.
pub fn sync(install: &str) -> Result<InstallReport, String> {
    let at = installed_at();
    let now = chrono::Utc::now().timestamp();
    let rep = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        install::note_install(&conn, install, at, now)?
    };
    log(&rep, &format!("Sew install {install}"));
    Ok(rep)
}

/// A purge: every agent Space but the K5 Space, every hub cursor and the
/// stamp go; the K5 Space is checked before anything is sent to it again.
pub fn forget(why: &str) -> Result<InstallReport, String> {
    let now = chrono::Utc::now().timestamp();
    let rep = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        install::forget(&conn, why, now)?
    };
    k2_core::tool_hook::admin_space_forget_install(why)?;
    log(&rep, why);
    Ok(rep)
}

/// Every stale Space's record (`k2 voice repair`, `k2 hostchat doctor`).
pub fn stale_spaces() -> Result<Vec<install::StaleSpace>, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    install::stale_spaces(&conn)
}
