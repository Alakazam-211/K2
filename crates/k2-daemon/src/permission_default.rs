//! Permission cards on by default (0.46.11, Rosson 2026-11-10): "Have the
//! permission cards system turned on by default at the start so that users
//! can grant their agents easier access to permission cards from the
//! workspace settings and not have to go hunting to turn on that toggle."
//!
//! - A fresh server: `AppSettings::permission_cards` defaults on, so the
//!   boot sync opens the gate with no settings file at all.
//! - An existing server: [`boot`] runs the one-time pass
//!   (`app_settings::apply_permission_cards_default`). 0.46.3 to 0.46.10
//!   saved `permissionCards: false` on every settings write, so a stored
//!   off can't say whether a person chose it; the tie-breaker is the Access
//!   Audit ([`audit_says_turned_off`]). Turned on → one Access Audit line
//!   at boot, and [`notify`] files the owner's one notice once the
//!   database is up. The pass marks the file, so it never runs again, and
//!   an owner's later off sticks.
//!
//! Only the server switch moves. Each agent's own "Let this agent ask for
//! permission" stays as it was (off unless someone turned it on), and
//! every grant a card asks for stays off until a person allows that card.

use k2_core::app_settings::{self, PermissionCardsDefault};
use k2_core::permission::audit_event;

/// The owner's notice title (Tickets, owners only, with a push).
pub const NOTICE_TITLE: &str = "Permission cards are now on";

/// The notice body: what changed, what it doesn't do, where to turn it off.
pub const NOTICE_BODY: &str = "Permission cards are now on for this server (K2 0.46.11 turns them on by default). \
An agent you choose can ask you, on a card in its Thread, to change a setting it can't change itself; nothing changes until you or an admin allow that card. \
To choose an agent: Settings → Workspaces → <agent> → Permissions → \"Let this agent ask for permission\" (each agent's switch is still off until you turn it on). \
To turn cards off for the whole server: Settings → Access → Permission Cards, or `k2 permission beta off`.";

/// The Access Audit line the pass writes when it turns the switch on.
pub const AUDIT_OUTCOME: &str = "server on (on by default from 0.46.11)";

/// Who the pass writes as (Access Audit `user`, the notice's agent name).
const BY: &str = "k2";

/// True when the Access Audit's last server-switch line
/// (`permission-switch`, `server on|off…`) says the switch was turned off:
/// a person chose off (every flip a settings write or reset makes is
/// recorded by `settings_routes::permission_cards_flipped`). No line, or an
/// unreadable audit, is "can't tell" (false).
pub fn audit_says_turned_off() -> bool {
    let lines = match k2_core::auth_audit::read_all() {
        Ok(lines) => lines,
        Err(e) => {
            k2_core::log_debug!("[permission] default pass: the Access Audit can't be read ({e}); treated as no off on record");
            return false;
        }
    };
    lines
        .iter()
        .rev()
        .filter(|l| l["event"] == audit_event::SWITCH)
        .filter_map(|l| l["outcome"].as_str())
        .find(|o| o.starts_with("server "))
        .is_some_and(|o| o == "server off")
}

/// Daemon boot, before any route answers: the one-time pass, then the
/// gate from the setting (`K2_PERMISSION_CARDS=1` still forces it on).
/// Returns what the pass did; [`notify`] takes it once the database is up.
pub fn boot() -> PermissionCardsDefault {
    let outcome = match app_settings::apply_permission_cards_default(&audit_says_turned_off) {
        Ok(o) => o,
        Err(e) => {
            // Nothing was written: the stored value stands, and the pass
            // tries again on the next boot.
            k2_core::log_debug!("ERROR [permission] default pass: {e}");
            PermissionCardsDefault::AlreadyApplied
        }
    };
    if outcome == PermissionCardsDefault::TurnedOn {
        k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(audit_event::SWITCH, BY, AUDIT_OUTCOME, "-", "-", "cli"));
    }
    k2_core::permission::gate::set_server_switch(app_settings::load().permission_cards);
    outcome
}

/// After the database is up: the owner's one notice, only when [`boot`]
/// turned the switch on. Returns whether it filed one.
pub fn notify(outcome: PermissionCardsDefault) -> bool {
    if outcome != PermissionCardsDefault::TurnedOn {
        return false;
    }
    match crate::feedback_routes::file_owners_only_notice(k2_core::db::AUDIT_SENTINEL_IDS[0], BY, NOTICE_TITLE, NOTICE_BODY) {
        Ok(_) => true,
        Err(e) => {
            k2_core::log_debug!("ERROR [permission] the permission-cards notice: {e}");
            false
        }
    }
}
