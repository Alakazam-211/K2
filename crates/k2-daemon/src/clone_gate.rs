//! The one clone gate (prd-clone-carries-threads-and-vault-v1 §7.6, CL1,
//! CL2, CL4).
//!
//! Before 0.46.0 every clone door admitted any Connect login (`token_ok`,
//! Member floor). The gate narrows that with two owner-set switches, both
//! **on by default** (Rosson's D2, 2026-10-16), so nothing changes until
//! the owner restricts it:
//!
//! - Settings → General → "Allow cloning agents to people's personal
//!   computers" (`allowClonePersonalComputers`, Owner-only key).
//! - Settings → Admin Access → per person "Can clone"
//!   (`connect-users.json` `can_clone`, `POST /cli/users/set-can-clone`).
//!
//! | Caller | Rule |
//! |---|---|
//! | owner token, Owner-role login | always allowed |
//! | Admin / Member login | needs "Can clone" on this server |
//! | agent or K2-shell passport | refused: agents propose (`/cli/clone-proposal/*`) |
//! | App pass, API key, anything else | refused |
//!
//! Doors that hand a bundle to a person's computer — `pack` (pull to the
//! requester) and `bundle` (it can be downloaded with `fs/read-range`) —
//! also need the master switch for Admin/Member logins. `unpack` gates who
//! may land a clone on THIS server. Copying secrets additionally needs an
//! Owner/Admin vault confirm (D1, `clone-carry`, which also covers sending
//! `.env` files as files, D4), checked by the vault slice (`clone_vault`).
//!
//! Every decision is written to the auth audit as `clone-gate` (names
//! only). Every `/cli/clone/*` arm in the dispatcher and the proposal
//! approve call [`check`] first; `tests::every_clone_arm_calls_the_gate`
//! walks the dispatcher source so a new door can't skip it (CL4).

use crate::cli_response::CliResponse;
use crate::compute_routes::Who;
use k2_core::connect_users::Role;

/// The clone doors the gate knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Door {
    Preview,
    Bundle,
    Pack,
    PackStatus,
    PackCleanup,
    Unpack,
    ProposalApprove,
    ThreadExport,
    ThreadImport,
}

impl Door {
    pub fn as_str(self) -> &'static str {
        match self {
            Door::Preview => "preview",
            Door::Bundle => "bundle",
            Door::Pack => "pack",
            Door::PackStatus => "pack-status",
            Door::PackCleanup => "pack-cleanup",
            Door::Unpack => "unpack",
            Door::ProposalApprove => "proposal-approve",
            Door::ThreadExport => "thread-export",
            Door::ThreadImport => "thread-import",
        }
    }

    /// Doors whose result can leave this server for a person's computer.
    fn reaches_personal_computer(self) -> bool {
        matches!(self, Door::Bundle | Door::Pack)
    }

    /// The `/cli/clone/*` path → door map (exact paths only).
    pub fn for_path(path: &str) -> Option<Door> {
        Some(match path {
            "/cli/clone/preview" => Door::Preview,
            "/cli/clone/bundle" => Door::Bundle,
            "/cli/clone/pack" => Door::Pack,
            "/cli/clone/pack-status" => Door::PackStatus,
            "/cli/clone/pack-cleanup" => Door::PackCleanup,
            "/cli/clone/unpack" => Door::Unpack,
            "/cli/thread/export" => Door::ThreadExport,
            "/cli/thread/import" => Door::ThreadImport,
            // Slice A's vault half of a clone (names-only plan, the
            // clone-carry confirm, a standalone seal, an import, the
            // same-box copy).
            "/cli/clone/vault/plan" | "/cli/clone/vault/confirm" => Door::Preview,
            "/cli/clone/vault/export" => Door::Bundle,
            "/cli/clone/vault/import" | "/cli/clone/vault/copy" => Door::Unpack,
            _ => return None,
        })
    }
}

pub mod codes {
    pub const NOT_ALLOWED: &str = "clone_not_allowed";
    pub const PERSONAL_OFF: &str = "clone_personal_computers_off";
    pub const AGENTS_PROPOSE: &str = "clone_agents_propose";
    pub const WRONG_CREDENTIAL: &str = "clone_wrong_credential";
}

/// A gate refusal (`403`). `hint` is the teaching line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Denied {
    pub code: &'static str,
    pub hint: String,
}

impl Denied {
    /// `{"error": hint, "code", "hint"}`: `error` stays a string so older
    /// clients (which show `error` verbatim) print the teaching line.
    pub fn response(&self) -> CliResponse {
        CliResponse {
            status: "403 Forbidden",
            content_type: "application/json",
            body: serde_json::json!({ "error": self.hint, "code": self.code, "hint": self.hint }).to_string(),
        }
    }
}

/// The pure decision. `personal_on` = the master switch; `can_clone` =
/// the login's stored "Can clone" (only consulted for Admin/Member).
pub fn decide(who: &Who, door: Door, personal_on: bool, can_clone: impl Fn(&str) -> bool) -> Result<(), Denied> {
    match who {
        Who::Owner | Who::Login { role: Role::Owner, .. } => Ok(()),
        Who::Login { username, .. } => {
            if !can_clone(username) {
                return Err(Denied {
                    code: codes::NOT_ALLOWED,
                    hint: format!(
                        "cloning isn't allowed for '{username}' on this server: the owner turns on \"Can clone\" for you in \
                         Settings → Admin Access (or k2 users set-can-clone {username} on)"
                    ),
                });
            }
            if door.reaches_personal_computer() && !personal_on {
                return Err(Denied {
                    code: codes::PERSONAL_OFF,
                    hint: "this server doesn't allow cloning agents to people's personal computers (Settings → General, \
                           owner only). Ask the owner to clone it for you."
                        .to_string(),
                });
            }
            Ok(())
        }
        Who::Agent { .. } | Who::Shell { .. } => Err(Denied {
            code: codes::AGENTS_PROPOSE,
            hint: "agents can't clone; propose it and a person approves: k2 clone propose --to <host|here> --why \"…\""
                .to_string(),
        }),
        Who::Wrong | Who::Invalid | Who::Tenant(_) => Err(Denied {
            code: codes::WRONG_CREDENTIAL,
            hint: "cloning needs the owner or a Connect login with \"Can clone\" (App passes and API keys can't clone)"
                .to_string(),
        }),
    }
}

/// The live gate: reads the master switch and the login's "Can clone" now
/// (no cache, so a change applies to the next call), audits the decision,
/// and returns the 403 to send on refusal.
pub fn check(who: &Who, door: Door, ingress: &str) -> Result<(), CliResponse> {
    let personal_on = k2_core::app_settings::load().allow_clone_personal_computers;
    let r = decide(who, door, personal_on, k2_core::connect_users::can_clone);
    let outcome = match &r {
        Ok(()) => format!("allow {}", door.as_str()),
        Err(d) => format!("deny {} {}", door.as_str(), d.code),
    };
    k2_core::auth_audit::record(&k2_core::auth_audit::AuditEvent::new(
        "clone-gate",
        &who.label(),
        outcome,
        ingress,
        "-",
        "clone",
    ));
    r.map_err(|d| d.response())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn login(name: &str, role: Role) -> Who {
        Who::Login { username: name.into(), role }
    }

    const DOORS: &[Door] = &[
        Door::Preview,
        Door::Bundle,
        Door::Pack,
        Door::PackStatus,
        Door::PackCleanup,
        Door::Unpack,
        Door::ProposalApprove,
        Door::ThreadExport,
        Door::ThreadImport,
    ];

    #[test]
    fn owner_token_and_owner_login_pass_every_door_whatever_the_switches() {
        for door in DOORS {
            for personal in [true, false] {
                assert_eq!(decide(&Who::Owner, *door, personal, |_| false), Ok(()), "{door:?} {personal}");
                assert_eq!(decide(&login("boss", Role::Owner), *door, personal, |_| false), Ok(()), "{door:?} {personal}");
            }
        }
    }

    #[test]
    fn admin_and_member_need_can_clone_on_every_door() {
        for role in [Role::Admin, Role::Member] {
            for door in DOORS {
                let r = decide(&login("al", role), *door, true, |_| false);
                assert_eq!(r.expect_err("refused").code, codes::NOT_ALLOWED, "{role:?} {door:?}");
                assert_eq!(decide(&login("al", role), *door, true, |u| u == "al"), Ok(()), "{role:?} {door:?}");
            }
        }
    }

    #[test]
    fn master_switch_off_blocks_only_the_personal_computer_doors_for_logins() {
        for role in [Role::Admin, Role::Member] {
            for door in DOORS {
                let r = decide(&login("al", role), *door, false, |_| true);
                if matches!(door, Door::Bundle | Door::Pack) {
                    assert_eq!(r.expect_err("refused").code, codes::PERSONAL_OFF, "{role:?} {door:?}");
                } else {
                    assert_eq!(r, Ok(()), "{role:?} {door:?}");
                }
            }
        }
    }

    #[test]
    fn passports_and_other_credentials_never_pass() {
        let agent = Who::Agent { project_id: "p".into(), session_id: "s".into() };
        let shell = Who::Shell { project_id: "p".into(), session_id: "s".into() };
        for door in DOORS {
            assert_eq!(decide(&agent, *door, true, |_| true).expect_err("agent").code, codes::AGENTS_PROPOSE);
            assert_eq!(decide(&shell, *door, true, |_| true).expect_err("shell").code, codes::AGENTS_PROPOSE);
            assert_eq!(decide(&Who::Wrong, *door, true, |_| true).expect_err("wrong").code, codes::WRONG_CREDENTIAL);
            assert_eq!(decide(&Who::Invalid, *door, true, |_| true).expect_err("invalid").code, codes::WRONG_CREDENTIAL);
        }
    }

    #[test]
    fn refusal_body_keeps_error_a_string_for_old_clients() {
        let d = decide(&login("al", Role::Member), Door::Bundle, true, |_| false).expect_err("refused");
        let r = d.response();
        assert_eq!(r.status, "403 Forbidden");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert!(v["error"].as_str().expect("string error").contains("Can clone"), "{v}");
        assert_eq!(v["code"], codes::NOT_ALLOWED);
    }

    #[test]
    fn live_check_reads_settings_and_users_from_disk() {
        crate::test_support::with_temp_home(|| {
            k2_core::connect_users::add_user("mem", "password1").expect("add");
            let m = login("mem", Role::Member);
            // Defaults (D2): switch ON, Can clone ON → allowed.
            assert!(check(&m, Door::Pack, "test").is_ok());
            k2_core::connect_users::set_can_clone("mem", false).expect("off");
            let r = check(&m, Door::Unpack, "test").expect_err("can_clone off");
            assert!(r.body.contains(codes::NOT_ALLOWED), "{}", r.body);
            k2_core::connect_users::set_can_clone("mem", true).expect("on");
            let mut s = k2_core::app_settings::load();
            s.allow_clone_personal_computers = false;
            k2_core::app_settings::save(&s).expect("save");
            let r = check(&m, Door::Pack, "test").expect_err("switch off");
            assert!(r.body.contains(codes::PERSONAL_OFF), "{}", r.body);
            assert!(check(&m, Door::Unpack, "test").is_ok(), "unpack is not a personal-computer door");
            assert!(check(&Who::Owner, Door::Pack, "test").is_ok());
        });
    }

    /// CL4: every dispatcher arm that reaches a clone handler calls the
    /// gate first. Walks the dispatcher source: each line that calls
    /// `crate::clone_routes::handle_clone_*` or the proposal approve must
    /// sit after a `clone_gate::check` in the same match arm.
    #[test]
    fn every_clone_arm_calls_the_gate() {
        let src = include_str!("routes/dispatcher.rs");
        let mut last_gate: Option<usize> = None;
        let mut last_arm: usize = 0;
        let mut calls = 0;
        for (i, line) in src.lines().enumerate() {
            let t = line.trim_start();
            // A new match arm at the dispatcher's arm indentation.
            if line.starts_with("        ") && !line.starts_with("         ") && (t.starts_with("p if") || t.starts_with('"')) {
                last_arm = i;
            }
            if t.contains("clone_gate::check(") || t.contains("clone_gate_check(") {
                last_gate = Some(i);
            }
            if t.contains("clone_routes::handle_clone_")
                || t.contains("clone_preview::handle(")
                || t.contains("clone_vault::handle(")
                || t.contains("thread_transfer_routes::handle(")
            {
                calls += 1;
                let gated = last_gate.is_some_and(|g| g > last_arm);
                assert!(gated, "dispatcher.rs:{} reaches a clone handler without clone_gate::check in its arm", i + 1);
            }
        }
        // bundle, unpack, pack, pack-cleanup, pack-status, preview, vault/*,
        // thread export/import.
        assert!(calls >= 8, "expected every clone door in the dispatcher, saw {calls}");
        // The per-cell socket never serves `/cli/clone/*` (deny prefix).
        assert!(!crate::session_token::is_agent_verb("/cli/clone/bundle"));
        assert!(!crate::session_token::is_agent_verb("/cli/clone/preview"));
        assert!(!include_str!("cell_server.rs").contains("clone_routes::handle_clone_"));
    }
}
