//! Who may open, drive and manage a desktop (`prd-remote-desktop-tab-v1`
//! §4, with Rosson's answers of 2026-11-10).
//!
//! | Caller                         | View | Control | Manage |
//! |--------------------------------|------|---------|--------|
//! | Owner-role login               | yes  | yes     | yes    |
//! | Admin login                    | yes  | yes     | no     |
//! | Member login with a grant      | yes  | never   | no     |
//! | Member login without a grant   | no   | no      | no     |
//! | Voice & Chat only login        | no   | no      | no     |
//! | The owner daemon token         | no   | no      | yes    |
//! | A K2 shell tab's passport      | no   | no      | yes    |
//! | Agent passport, App pass, API key, `k2rs_` | refused before this (`desktop_people_only`) |
//!
//! - **The stream takes logins only** (Q2): the owner token and a shell
//!   tab manage (switch, policy, grants, kick) but never watch. Their only
//!   use would be this machine's own screen.
//! - **Same machine** (R19): a caller on this machine (loopback) never
//!   watches: viewing your own screen has no use, and refusing it closes a
//!   local path. The tab shows "You're on this computer."
//! - Management is the Owner's only (S1), never an Admin's.

use super::grants::Grant;
use super::wire::Mode;
use crate::connect_users::Role;

/// Who is asking (the daemon builds it from its caller classification).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Principal {
    /// The owner daemon token.
    OwnerToken,
    /// A plain K2 shell tab's passport (a person typing inside K2).
    Shell,
    /// A Connect login.
    Login { username: String, role: Role },
}

impl Principal {
    /// `status.you.role`.
    pub fn role_wire(&self) -> &'static str {
        match self {
            Principal::OwnerToken => "owner-token",
            Principal::Shell => "shell",
            Principal::Login { role, .. } => role.as_wire(),
        }
    }

    /// The actor for audit lines and `granted_by`.
    pub fn actor(&self) -> String {
        match self {
            Principal::OwnerToken => "owner".into(),
            Principal::Shell => "shell".into(),
            Principal::Login { username, .. } => format!("login:{username}"),
        }
    }
}

/// What a caller may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Access {
    pub may_view: bool,
    pub may_control: bool,
    pub may_manage: bool,
    pub same_machine: bool,
}

/// Decide. `grant` is the caller's live grant (already checked against
/// its login's current facts); `same_machine` is true for a loopback
/// caller and for the owner token and a shell tab (both are always on
/// this machine).
pub fn access(p: &Principal, grant: Option<&Grant>, same_machine: bool) -> Access {
    let base = match p {
        Principal::OwnerToken | Principal::Shell => {
            return Access { may_view: false, may_control: false, may_manage: true, same_machine: true };
        }
        Principal::Login { role: Role::Owner, .. } => Access { may_view: true, may_control: true, may_manage: true, same_machine },
        Principal::Login { role: Role::Admin, .. } => Access { may_view: true, may_control: true, may_manage: false, same_machine },
        Principal::Login { role: Role::Member, username } => {
            let granted = grant.is_some_and(|g| &g.user == username && g.role_at_grant == Role::Member);
            // A Member grant is view only, whatever a row says.
            Access { may_view: granted, may_control: false, may_manage: false, same_machine }
        }
        Principal::Login { role: Role::VoiceChat, .. } => Access { same_machine, ..Access::default() },
    };
    if same_machine {
        Access { may_view: false, may_control: false, ..base }
    } else {
        base
    }
}

/// The mode a viewer starts in: control only when it may and asked to.
pub fn start_mode(a: &Access, want_view: bool) -> Mode {
    if a.may_control && !want_view {
        Mode::Control
    } else {
        Mode::View
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(name: &str, role: Role) -> Principal {
        Principal::Login { username: name.into(), role }
    }

    fn grant_for(name: &str, mode: Mode) -> Grant {
        Grant {
            desktop: super::super::CONSOLE.into(),
            user: name.into(),
            user_id: format!("u-{name}"),
            role_at_grant: Role::Member,
            mode,
            granted_by: "owner".into(),
            granted_at: 1,
        }
    }

    #[test]
    fn owners_and_admins_open_and_drive_only_owners_manage() {
        let o = access(&login("o", Role::Owner), None, false);
        assert_eq!(o, Access { may_view: true, may_control: true, may_manage: true, same_machine: false });
        let a = access(&login("a", Role::Admin), None, false);
        assert_eq!(a, Access { may_view: true, may_control: true, may_manage: false, same_machine: false });
    }

    #[test]
    fn a_member_needs_a_grant_and_only_ever_watches() {
        let m = login("mel", Role::Member);
        assert_eq!(access(&m, None, false), Access::default(), "no grant, nothing");
        let g = grant_for("mel", Mode::View);
        assert_eq!(access(&m, Some(&g), false), Access { may_view: true, ..Access::default() });
        // Even a forged control row never lets a Member drive.
        let forged = grant_for("mel", Mode::Control);
        let a = access(&m, Some(&forged), false);
        assert!(a.may_view && !a.may_control && !a.may_manage, "{a:?}");
        // Someone else's grant isn't this Member's.
        assert!(!access(&m, Some(&grant_for("ann", Mode::View)), false).may_view);
        assert_eq!(start_mode(&a, false), Mode::View);
    }

    #[test]
    fn voice_chat_never_opens() {
        let v = login("v", Role::VoiceChat);
        assert_eq!(access(&v, None, false), Access::default());
        let mut g = grant_for("v", Mode::View);
        g.role_at_grant = Role::VoiceChat;
        assert_eq!(access(&v, Some(&g), false), Access::default());
    }

    #[test]
    fn the_owner_token_and_a_shell_manage_but_never_watch() {
        for p in [Principal::OwnerToken, Principal::Shell] {
            let a = access(&p, None, false);
            assert_eq!(a, Access { may_view: false, may_control: false, may_manage: true, same_machine: true }, "{p:?}");
        }
    }

    #[test]
    fn nobody_watches_from_the_same_machine() {
        let o = access(&login("o", Role::Owner), None, true);
        assert_eq!(o, Access { may_view: false, may_control: false, may_manage: true, same_machine: true });
        let g = grant_for("mel", Mode::View);
        assert!(!access(&login("mel", Role::Member), Some(&g), true).may_view);
        assert_eq!(start_mode(&o, false), Mode::View);
        let a = access(&login("a", Role::Admin), None, false);
        assert_eq!(start_mode(&a, false), Mode::Control);
        assert_eq!(start_mode(&a, true), Mode::View, "view=1 starts watching");
    }
}
