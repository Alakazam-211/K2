//! Who is calling a vault route, in vault terms.
//!
//! - Owner token → `Actor::Owner` (owner rights in 0.45.2; VS1 deferred).
//! - Connect login → `Actor::Login` with its role.
//! - Agent passport **and** K2 shell-tab passport → `Actor::Agent`
//!   (agent-level, KV15), with the workspace's `vault_manage` switch.
//! - App passes, API cells, bad tokens → refused.
//!
//! Plus the **person channel** (VS2): some steps (recovery key,
//! vault confirm password) are for people only. A Connect login is a
//! person; so is a `k2 vault …` run on the vault socket from a terminal
//! outside K2 (peer not one of the daemon's own processes, see
//! `human.rs`). The raw owner token over TCP is not.

use k2_vault::{Actor, AgentActor, Role, VaultError};

use crate::sidecar_routes::Caller;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    /// TCP or cell socket with no proof of a person.
    Plain,
    /// A Connect login (password at login, cookie session).
    Login,
    /// The vault socket, from a terminal outside K2's own processes.
    Tty,
}

impl Channel {
    pub fn is_person(&self) -> bool {
        matches!(self, Channel::Login | Channel::Tty)
    }

}

/// A vault caller: the actor plus how it reached us.
#[derive(Debug, Clone)]
pub struct VaultCaller {
    pub actor: Actor,
    pub channel: Channel,
    /// The calling session's id (passports), for session scope and
    /// placeholders.
    pub session_id: Option<String>,
}

impl VaultCaller {
    pub fn is_passport(&self) -> bool {
        matches!(self.actor, Actor::Agent(_))
    }

    pub fn workspace_id(&self) -> Option<&str> {
        self.actor.agent().map(|a| a.workspace_id.as_str())
    }
}

fn role_of(r: k2_core::connect_users::Role) -> Role {
    use k2_core::connect_users::Role as R;
    match r {
        R::Owner => Role::Owner,
        R::Admin => Role::Admin,
        _ => Role::Member,
    }
}

/// The `vault_manage` switch for a workspace (0149).
pub fn vault_manage(project_id: &str) -> bool {
    k2_core::workspace::settings::agents_can_manage_vault(project_id)
}

/// Map a validated caller. `session_id` is the passport's session.
pub fn from_caller(c: Caller, session_id: Option<String>) -> Result<VaultCaller, VaultError> {
    let (actor, channel) = match c {
        Caller::Owner => (Actor::Owner, Channel::Plain),
        Caller::Login { username } => {
            let role = k2_core::connect_users::role_for_user(&username)
                .ok_or_else(|| VaultError::new("unauthorized", 401, "that login no longer exists"))?;
            (Actor::Login { username, role: role_of(role) }, Channel::Login)
        }
        Caller::Shell { project_id } => (
            Actor::Agent(AgentActor {
                vault_manage: vault_manage(&project_id),
                workspace_id: project_id,
                session_id: session_id.clone(),
                shell: true,
            }),
            Channel::Plain,
        ),
        Caller::Agent(a) => (
            Actor::Agent(AgentActor {
                vault_manage: vault_manage(&a.project_id),
                workspace_id: a.project_id,
                session_id: session_id.clone(),
                shell: false,
            }),
            Channel::Plain,
        ),
        Caller::WrongCredential => {
            return Err(VaultError::forbidden(
                "wrong_credential",
                "app passes and API keys can't use the vault; agents and people can",
            ))
        }
        Caller::Invalid => return Err(VaultError::new("unauthorized", 401, "invalid or missing token")),
    };
    Ok(VaultCaller { actor, channel, session_id })
}

/// TCP caller: like `sidecar_routes::caller_from_tcp`, keeping the
/// passport's session id.
pub fn from_tcp(path: &str, query: &str, bearer: Option<&str>, owner_token: &str) -> Result<VaultCaller, VaultError> {
    let presented = crate::routes::http::extract_token(query)
        .filter(|s| !s.is_empty())
        .or_else(|| bearer.map(str::trim).filter(|s| !s.is_empty()))
        .unwrap_or("");
    let session_id = if presented.is_empty() || crate::routes::http::ct_eq_token(presented, owner_token) {
        None
    } else {
        crate::session_token::require_hook(presented, path).map(|v| v.session_id)
    };
    let c = crate::sidecar_routes::caller_from_tcp(path, query, bearer, owner_token);
    from_caller(c, session_id)
}
