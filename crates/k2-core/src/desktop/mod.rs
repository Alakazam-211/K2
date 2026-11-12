//! Remote desktop (`prd-remote-desktop-tab-v1`, D1): the truth behind the
//! Desktop tab. It runs headless; the daemon serves it (`/cli/desktop/*`)
//! and `k2-deskd` (D3+) captures and injects.
//!
//! - [`store`]: the switch and the hide-list, in this module's own table,
//!   **not** `AppSettings` (S1: `/cli/settings/update` takes Admin writes;
//!   this switch is the Owner's). Off until an Owner turns it on; on until
//!   an Owner turns it off (no TTL, Rosson 2026-11-10).
//! - [`policy`]: who may open, drive and manage ([`policy::access`]).
//!   Owners and Admins open (view and control); a Member only with a
//!   grant, and a Member grant is view only; Voice & Chat only never.
//! - [`grants`]: Member grants keyed by the login's stable id (R18).
//! - [`lease`]: one controller at a time; local input wins (3 s).
//! - [`audit`]: the access log, `~/.k2/desktop-audit.jsonl` (never frames
//!   or keystrokes; written before a viewer connects, R33).
//! - [`viewers`]: the live viewer registry (what `disable`, `revoke` and
//!   `kick` close).
//! - [`ticket`]: single-use stream tickets (R20).
//! - [`wire`], [`input`], [`keymap`]: the stream's messages, input rules
//!   and key table.
//!
//! **Threat model (v1, Rosson's ruling 2026-11-10).** Same-user code on
//! this machine (agents, Member or Admin terminals, `k2rs_` shells) can
//! reach the desktop: it can read the owner token, mint or forge a login,
//! or edit this table. That is accepted for v1 ("agent inception") and is
//! the 0.49 agent-isolation arc's job. What v1 guards is everyone off the
//! box: the route gates, the role and grant rules, POST-only writes, no
//! long-lived token in the stream URL, per-message control checks, grants
//! keyed by id, the access log, and a phone push to every paired Owner.

pub mod audit;
pub mod grants;
pub mod input;
pub mod keymap;
pub mod lease;
pub mod policy;
pub mod store;
pub mod ticket;
pub mod viewers;
pub mod virtual_desktop;
pub mod wire;

/// A machine's real screen session (every Mac; a Linux box someone is
/// logged into). Linux servers with no screen also have virtual desktops
/// ([`virtual_desktop`]).
pub const CONSOLE: &str = "console";

/// The `/boot-status` key of daemons that start virtual desktops (Linux
/// only, LD-16): clients show Set up, Start and the picker with it.
pub const VIRTUAL_FEATURE_KEY: &str = "virtual-desktop-v1";

/// The feature key in `/boot-status` (clients show + → Desktop with it).
pub const FEATURE_KEY: &str = "remote-desktop-v1";

/// The enable card (S14), returned by `enable` and shown before Turn on.
pub const ENABLE_CARD: &str = "Anyone you allow can see and do anything you can on this computer, including your mail, files and passwords. Owners and Admins can open it; Members only with your grant, and only to watch. It stays on until an Owner turns it off. On Linux, K2 can't hide password apps from the picture, and notifications (2FA codes too) show in it.";

/// The enable card for the `virtual` switch (LD-18): every virtual
/// desktop on the server, the shared one and each agent's.
pub const VIRTUAL_ENABLE_CARD: &str = "K2 will start a desktop on this server. Anyone you allow can see and use whatever is opened on it. Programs on it run as this server's K2 user. It stays on until an Owner turns it off.";

/// The machine's state as the deskd side reports it (`status.state`, the
/// stream's `state`). The tab adds `off`, `not_allowed` and `same_machine`
/// from the switch and the caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MachineState {
    /// deskd is up with its grants: a viewer can connect.
    Ready,
    /// Someone at this Mac has to allow K2 Screen once (TCC).
    NeedsPermission,
    /// No one is logged in on the screen.
    NoSession,
    /// No screen K2 can show (headless Linux).
    NoDisplay,
    /// The Mac is locked; K2 can't unlock it.
    Locked,
    /// Wayland: someone has to click Allow once.
    PortalAllow,
    /// K2 Screen and K2 don't match; restarting it.
    NeedsUpdate,
    /// K2 Screen is switched off under Login Items (macOS).
    BlockedInLoginItems,
    /// No K2 Screen in this build (or on this platform), with why.
    Unavailable(String),
}

impl MachineState {
    pub fn as_wire(&self) -> &'static str {
        match self {
            MachineState::Ready => "ready",
            MachineState::NeedsPermission => "needs_permission",
            MachineState::NoSession => "no_session",
            MachineState::NoDisplay => "no_display",
            MachineState::Locked => "locked",
            MachineState::PortalAllow => "portal_allow",
            MachineState::NeedsUpdate => "needs_update",
            MachineState::BlockedInLoginItems => "blocked_in_login_items",
            MachineState::Unavailable(_) => "unavailable",
        }
    }

    /// The tab's sentence for this state.
    pub fn message(&self) -> String {
        match self {
            MachineState::Ready => "Ready.".into(),
            MachineState::NeedsPermission => "Someone at this Mac has to allow K2 Screen once.".into(),
            MachineState::NoSession => "No one is logged in on this computer's screen.".into(),
            MachineState::NoDisplay => "This computer has no screen K2 can show.".into(),
            MachineState::Locked => "This Mac is locked. K2 can't unlock it.".into(),
            MachineState::PortalAllow => "Someone at this computer has to click Allow once.".into(),
            MachineState::NeedsUpdate => "K2 Screen and K2 don't match; restarting it.".into(),
            MachineState::BlockedInLoginItems => {
                "Turn K2 Screen on in System Settings → General → Login Items.".into()
            }
            MachineState::Unavailable(why) => why.clone(),
        }
    }
}

pub(crate) fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
}

pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_state_wire_names_are_stable() {
        let all = [
            (MachineState::Ready, "ready"),
            (MachineState::NeedsPermission, "needs_permission"),
            (MachineState::NoSession, "no_session"),
            (MachineState::NoDisplay, "no_display"),
            (MachineState::Locked, "locked"),
            (MachineState::PortalAllow, "portal_allow"),
            (MachineState::NeedsUpdate, "needs_update"),
            (MachineState::BlockedInLoginItems, "blocked_in_login_items"),
            (MachineState::Unavailable("x".into()), "unavailable"),
        ];
        for (s, w) in all {
            assert_eq!(s.as_wire(), w);
            assert!(!s.message().is_empty(), "{w}");
        }
        assert_eq!(MachineState::Unavailable("why".into()).message(), "why");
    }

    #[test]
    fn the_enable_card_is_honest() {
        for needle in ["anything you can", "passwords", "until an Owner turns it off", "only to watch"] {
            assert!(ENABLE_CARD.contains(needle), "{needle}");
        }
    }

    #[test]
    fn the_virtual_enable_card_is_honest() {
        for needle in ["start a desktop on this server", "Anyone you allow can see and use", "run as this server's K2 user", "until an Owner turns it off"] {
            assert!(VIRTUAL_ENABLE_CARD.contains(needle), "{needle}");
        }
        assert_ne!(VIRTUAL_ENABLE_CARD, ENABLE_CARD);
    }
}
