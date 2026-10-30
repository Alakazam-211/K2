//! Who may do what (§8.2, §11.2, KV15, KV16, VS1-deferred).
//!
//! 0.45.2 rules (Rosson on the four must-fixes):
//! - The raw owner token keeps owner rights for loosening and reveal
//!   (VS1 is deferred). Every widening is audited. Reveal still needs an
//!   owner confirm (KV4/KV7). A future person-proof (signed Mac app
//!   channel, Connect password, mobile MFA) plugs in at [`HumanProof`].
//! - Agent passports **and** K2 shell tabs are agent-level (KV15).
//! - An agent can't loosen a secret a person gave it (§11.2). Tightening
//!   is always open.
//! - IT agents (`vault_manage`) may grant server secrets to *other*
//!   workspaces, never their own, and never loosen a secret their own
//!   workspace has a grant on.

use serde::{Deserialize, Serialize};

use crate::error::{Result, VaultError};
use crate::model::{Consumer, ConsumerKind, Delivery, Namespace, Scope, ScopeKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Admin,
    Member,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentActor {
    pub workspace_id: String,
    pub session_id: Option<String>,
    /// A plain K2 shell tab (a person typing in K2) — still agent-level.
    pub shell: bool,
    /// "Allow managing server-wide secrets" (0149).
    pub vault_manage: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    /// The owner token (desktop app, owner CLI outside K2, scripts).
    Owner,
    /// A Connect login with its role.
    Login { username: String, role: Role },
    /// A session passport.
    Agent(AgentActor),
    /// An app guest (answering a Thread card).
    Guest { username: String, workspace_id: String },
    /// A daemon subsystem (`mail`, `db`, `adopt`, …).
    System(String),
}

/// Proof that a person approved a reveal-class action. Today: an owner
/// confirm token (vault confirm password, Connect password, macOS
/// LocalAuthentication). The seam for a future person-proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HumanProof {
    None,
    Confirmed { method: String },
}

impl Actor {
    pub fn stamp(&self) -> String {
        match self {
            Actor::Owner => "owner".into(),
            Actor::Login { username, .. } => format!("connect:{username}"),
            Actor::Agent(a) if a.shell => format!("shell:{}", a.workspace_id),
            Actor::Agent(a) => match &a.session_id {
                Some(s) => format!("agent:{}/{}", a.workspace_id, s),
                None => format!("agent:{}", a.workspace_id),
            },
            Actor::Guest { username, .. } => format!("guest:{username}"),
            Actor::System(s) => format!("system:{s}"),
        }
    }

    /// Owner/Admin person (owner token counts in 0.45.2, VS1 deferred).
    pub fn is_admin_human(&self) -> bool {
        matches!(self, Actor::Owner | Actor::Login { role: Role::Owner | Role::Admin, .. })
    }

    pub fn is_member(&self) -> bool {
        matches!(self, Actor::Login { role: Role::Member, .. })
    }

    /// A value from this actor counts as human-given (§6.2).
    pub fn is_human(&self) -> bool {
        matches!(self, Actor::Owner | Actor::Login { .. } | Actor::Guest { .. })
    }

    pub fn agent(&self) -> Option<&AgentActor> {
        match self {
            Actor::Agent(a) => Some(a),
            _ => None,
        }
    }

    pub fn is_system(&self) -> bool {
        matches!(self, Actor::System(_))
    }

    /// Wildcard hosts only from people (VS4).
    pub fn may_add_wildcards(&self) -> bool {
        matches!(self, Actor::Owner | Actor::Login { .. })
    }

    /// Body injection only when a person set it (VS3).
    pub fn may_set_body_inject(&self) -> bool {
        matches!(self, Actor::Owner | Actor::Login { .. })
    }
}

/// The parts of a secret the rules look at.
#[derive(Debug, Clone)]
pub struct Facts<'a> {
    pub namespace: Namespace,
    pub scope: &'a Scope,
    pub created_by: &'a str,
    pub human_given: bool,
    /// Does the actor's own workspace hold an explicit grant on it?
    pub own_ws_has_grant: bool,
    /// Accepting is consent (Rosson 2026-10-30): set only when this secret
    /// is a copy another agent handed to this workspace **and a person
    /// accepted it** here (`k2 vault incoming accept`, the Thread card,
    /// Settings → Vault → Incoming). It is the ceiling the accept card
    /// showed, read from the vault's own MAC'd handoff row, never from the
    /// request. `None` for everything else, including copies that landed
    /// without a person (accept-from, an answered ask) and pending or
    /// declined ones.
    pub handoff_ceiling: Option<Delivery>,
}

fn denied(code: &'static str, hint: &str) -> VaultError {
    VaultError::forbidden(code, hint)
}

/// Did this agent's workspace create the secret?
fn created_by_agent_ws(created_by: &str, ws: &str) -> bool {
    created_by == format!("agent:{ws}")
        || created_by.starts_with(&format!("agent:{ws}/"))
        || created_by == format!("shell:{ws}")
}

pub fn can_create(actor: &Actor, ns: Namespace, scope: &Scope) -> Result<()> {
    if ns == Namespace::System {
        return if actor.is_system() {
            Ok(())
        } else {
            Err(denied("vault_system_namespace", "system secrets are made by K2 itself"))
        };
    }
    match actor {
        Actor::System(_) => Ok(()),
        Actor::Owner | Actor::Login { role: Role::Owner | Role::Admin, .. } => match scope.kind {
            ScopeKind::Server | ScopeKind::Workspace | ScopeKind::Session => Ok(()),
            _ => Err(denied("scope_not_allowed", "that scope isn't available")),
        },
        Actor::Login { .. } => match scope.kind {
            ScopeKind::Workspace | ScopeKind::Session => Ok(()),
            _ => Err(denied("scope_not_allowed", "Members keep secrets for a workspace or a chat; server-wide needs an Admin")),
        },
        Actor::Agent(a) => match scope.kind {
            ScopeKind::Workspace if scope.id == a.workspace_id => Ok(()),
            ScopeKind::Session if a.session_id.as_deref() == Some(scope.id.as_str()) => Ok(()),
            ScopeKind::Server if a.vault_manage => Ok(()),
            ScopeKind::Server => Err(denied(
                "vault_manage_required",
                "server-wide secrets need \"Allow managing server-wide secrets\" for this agent",
            )),
            _ => Err(denied("scope_not_allowed", "agents keep secrets for their own workspace or chat")),
        },
        Actor::Guest { .. } => match scope.kind {
            ScopeKind::Session => Ok(()),
            _ => Err(denied("scope_not_allowed", "guests can only give a secret for this chat")),
        },
    }
}

/// See a secret's metadata. `granted_to_actor`: an explicit grant names
/// the actor's workspace, session or `workspace:*`.
pub fn can_see(actor: &Actor, f: &Facts<'_>, granted_to_actor: bool) -> bool {
    if f.namespace == Namespace::System {
        return actor.is_admin_human() || actor.is_system();
    }
    match actor {
        Actor::System(_) => true,
        Actor::Owner | Actor::Login { role: Role::Owner | Role::Admin, .. } => true,
        Actor::Login { username, .. } => match f.scope.kind {
            ScopeKind::Session => f.created_by == format!("connect:{username}"),
            _ => true,
        },
        Actor::Agent(a) => match f.scope.kind {
            ScopeKind::Workspace => f.scope.id == a.workspace_id || granted_to_actor,
            ScopeKind::Session => a.session_id.as_deref() == Some(f.scope.id.as_str()) || granted_to_actor,
            ScopeKind::Server => granted_to_actor || a.vault_manage,
            _ => granted_to_actor,
        },
        Actor::Guest { username, .. } => f.created_by == format!("guest:{username}"),
    }
}

/// Accepting is consent (Rosson 2026-10-30, "The things you get in your
/// vault shouldn't be limited in this way"): after a person accepted a
/// handed-off secret into a workspace, that workspace's own agents may take
/// it at any delivery up to the ceiling the accept card showed, with no
/// second owner confirm. Only:
/// - an agent of the workspace the copy landed in (`scope` is that
///   workspace, and it is the agent's own);
/// - a copy a person accepted ([`Facts::handoff_ceiling`] is set);
/// - at or below that ceiling (never `api`: a landing ceiling is at most
///   `env`).
///
/// Every other rule stands: other workspaces, a higher ceiling, wider
/// sites, another injection and secrets the workspace didn't receive this
/// way still need the owner.
pub fn accepted_handoff_covers(actor: &Actor, f: &Facts<'_>, delivery: Delivery) -> bool {
    let Some(a) = actor.agent() else { return false };
    let Some(ceiling) = f.handoff_ceiling else { return false };
    f.namespace == Namespace::User
        && f.scope.kind == ScopeKind::Workspace
        && f.scope.id == a.workspace_id
        && !f.human_given
        && ceiling.level() <= Delivery::Env.level()
        && delivery != Delivery::None
        && delivery.level() <= ceiling.level()
}

/// Rotate, narrow, disable, destroy (tightening or same-level changes).
pub fn can_manage(actor: &Actor, f: &Facts<'_>) -> Result<()> {
    if f.namespace == Namespace::System {
        return if actor.is_system() {
            Ok(())
        } else {
            Err(denied("vault_system_namespace", "system secrets are managed by K2"))
        };
    }
    let ok = match actor {
        Actor::System(_) => true,
        Actor::Owner | Actor::Login { role: Role::Owner | Role::Admin, .. } => true,
        Actor::Login { username, .. } => f.created_by == format!("connect:{username}"),
        Actor::Agent(a) => match f.scope.kind {
            ScopeKind::Workspace => f.scope.id == a.workspace_id,
            ScopeKind::Session => a.session_id.as_deref() == Some(f.scope.id.as_str()),
            ScopeKind::Server => a.vault_manage || created_by_agent_ws(f.created_by, &a.workspace_id),
            _ => false,
        },
        Actor::Guest { .. } => false,
    };
    if ok {
        Ok(())
    } else {
        Err(denied("vault_not_yours", "only the secret's owner can change it"))
    }
}

/// A loosening change (wider hosts, higher delivery or ceiling, changed
/// injection, re-enable, a grant beyond the agent's own workspace).
pub fn can_loosen(actor: &Actor, f: &Facts<'_>) -> Result<()> {
    can_manage(actor, f)?;
    match actor {
        Actor::System(_) => Ok(()),
        Actor::Owner | Actor::Login { role: Role::Owner | Role::Admin, .. } => Ok(()),
        Actor::Login { .. } => Err(denied(
            "vault_admin_required",
            "widening a secret needs an Owner or Admin",
        )),
        Actor::Agent(a) => {
            if f.human_given {
                return Err(VaultError::owner_confirm_required(
                    "a person gave this secret, so an agent can't widen where it goes or who can read it",
                ));
            }
            if !created_by_agent_ws(f.created_by, &a.workspace_id) {
                return Err(VaultError::owner_confirm_required(
                    "agents can only widen secrets their own workspace created",
                ));
            }
            if a.vault_manage && f.scope.kind == ScopeKind::Server && f.own_ws_has_grant {
                return Err(VaultError::owner_confirm_required(
                    "an agent can't widen a server secret its own workspace uses",
                ));
            }
            Ok(())
        }
        Actor::Guest { .. } => Err(denied("vault_not_yours", "not allowed")),
    }
}

/// A grant (§8.2). `within_own` = the consumer is the agent's own
/// workspace or its own session, and the delivery doesn't exceed the
/// secret's default (so it widens nothing).
pub fn can_grant(actor: &Actor, f: &Facts<'_>, consumer: &Consumer, delivery: Delivery, default_delivery: Delivery) -> Result<()> {
    if f.namespace == Namespace::System {
        return if actor.is_system() {
            Ok(())
        } else {
            Err(denied("vault_system_namespace", "system secrets are granted by K2"))
        };
    }
    match actor {
        Actor::System(_) => Ok(()),
        Actor::Owner | Actor::Login { role: Role::Owner | Role::Admin, .. } => Ok(()),
        Actor::Login { .. } => Err(denied("vault_admin_required", "sharing a secret needs an Owner or Admin")),
        Actor::Guest { .. } => Err(denied("vault_not_yours", "not allowed")),
        Actor::Agent(a) => {
            // Compute secret-env (SE15, SV7): the value leaves this machine
            // for a node, so only a person grants it. This is the first
            // check, before the IT-agent branch below, which would
            // otherwise let "Allow managing server-wide secrets" send a
            // server secret to any other workspace's compute jobs.
            if consumer.kind == ConsumerKind::Compute {
                return Err(VaultError::owner_confirm_required(format!(
                    "only a person can let a secret leave this machine for a compute node; ask your owner to run: k2 vault grant <NAME> --to {} --as env",
                    consumer.label()
                )));
            }
            let own_ws = consumer.kind == ConsumerKind::Workspace && consumer.id == a.workspace_id;
            let all_ws = consumer.kind == ConsumerKind::Workspace && consumer.id == "*";
            let own_session = consumer.kind == ConsumerKind::Session && a.session_id.as_deref() == Some(consumer.id.as_str());
            // `k2 job` (item 5, Q14): a job in the agent's own workspace.
            let own_job = consumer.job_workspace() == Some(a.workspace_id.as_str());
            // A published service an agent can name is always one of its
            // own workspace's (the daemon resolves `service:<label>` in the
            // caller's workspace only), and the agent runs its code: a
            // grant to it is a grant to the agent (prd-env-to-vault-v1 R8).
            let own_service = consumer.kind == ConsumerKind::PublishedService;
            // IT agents: server secrets to OTHER workspaces only.
            if a.vault_manage && f.scope.kind == ScopeKind::Server {
                if own_ws || all_ws || own_session || own_job || own_service {
                    return Err(denied(
                        "vault_self_grant",
                        "an agent can't grant a server secret to its own workspace; ask the owner",
                    ));
                }
                if f.own_ws_has_grant && delivery.level() > default_delivery.level() {
                    return Err(VaultError::owner_confirm_required(
                        "an agent can't widen a server secret its own workspace uses",
                    ));
                }
                if f.human_given && delivery.level() > default_delivery.level() {
                    return Err(VaultError::owner_confirm_required(
                        "a person gave this secret; an agent can't raise how readable it is",
                    ));
                }
                return Ok(());
            }
            can_manage(actor, f)?;
            let within_own = (own_ws || own_session || own_job) && delivery.level() <= default_delivery.level();
            if within_own {
                return Ok(());
            }
            // Accepting is consent: the workspace a person accepted a
            // handed-off copy into takes it up to the accepted ceiling.
            if (own_ws || own_session || own_job) && accepted_handoff_covers(actor, f, delivery) {
                return Ok(());
            }
            can_loosen(actor, f)
        }
    }
}

/// Reveal (print a `user` value to a person): Owner/Admin with a confirm.
pub fn can_reveal(actor: &Actor, ns: Namespace, proof: &HumanProof) -> Result<()> {
    if ns == Namespace::System {
        return Err(denied("vault_system_namespace", "K2's own secrets are never shown, to anyone"));
    }
    if !actor.is_admin_human() {
        return Err(denied("vault_admin_required", "showing a secret needs an Owner or Admin"));
    }
    match proof {
        HumanProof::Confirmed { .. } => Ok(()),
        HumanProof::None => Err(denied(
            "vault_confirm_required",
            "showing a secret needs a confirm (Touch ID, your password, or the vault password)",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(ws: &str, manage: bool) -> Actor {
        Actor::Agent(AgentActor { workspace_id: ws.into(), session_id: Some("s1".into()), shell: false, vault_manage: manage })
    }

    fn facts<'a>(scope: &'a Scope, created_by: &'a str, human: bool) -> Facts<'a> {
        Facts { namespace: Namespace::User, scope, created_by, human_given: human, own_ws_has_grant: false, handoff_ceiling: None }
    }

    #[test]
    fn create_rules() {
        let ws = Scope::workspace("w1");
        can_create(&agent("w1", false), Namespace::User, &ws).expect("own ws");
        assert!(can_create(&agent("w2", false), Namespace::User, &ws).is_err());
        assert_eq!(can_create(&agent("w1", false), Namespace::User, &Scope::server()).expect_err("no").code, "vault_manage_required");
        can_create(&agent("w1", true), Namespace::User, &Scope::server()).expect("IT agent");
        can_create(&Actor::Owner, Namespace::User, &Scope::server()).expect("owner");
        let member = Actor::Login { username: "m".into(), role: Role::Member };
        assert!(can_create(&member, Namespace::User, &Scope::server()).is_err());
        can_create(&member, Namespace::User, &ws).expect("member ws");
        assert!(can_create(&Actor::Owner, Namespace::System, &ws).is_err());
        can_create(&Actor::System("mail".into()), Namespace::System, &Scope::engine("stalwart")).expect("system");
        let guest = Actor::Guest { username: "g".into(), workspace_id: "w1".into() };
        can_create(&guest, Namespace::User, &Scope::session("c1")).expect("guest chat");
        assert!(can_create(&guest, Namespace::User, &ws).is_err());
    }

    #[test]
    fn agents_cannot_loosen_human_given() {
        let ws = Scope::workspace("w1");
        let f = facts(&ws, "owner", true);
        can_manage(&agent("w1", false), &f).expect("can narrow/rotate");
        assert_eq!(can_loosen(&agent("w1", false), &f).expect_err("no").code, "vault_owner_confirm_required");
        can_loosen(&Actor::Owner, &f).expect("owner loosens (KV16, VS1 deferred)");
        let own = facts(&ws, "agent:w1/s1", false);
        can_loosen(&agent("w1", false), &own).expect("own secret");
        let other_ws_made = facts(&ws, "agent:w9/s1", false);
        assert!(can_loosen(&agent("w1", false), &other_ws_made).is_err());
    }

    #[test]
    fn shell_tabs_are_agent_level() {
        let shell = Actor::Agent(AgentActor { workspace_id: "w1".into(), session_id: None, shell: true, vault_manage: false });
        let ws = Scope::workspace("w1");
        assert!(can_loosen(&shell, &facts(&ws, "owner", true)).is_err());
        assert!(can_reveal(&shell, Namespace::User, &HumanProof::Confirmed { method: "x".into() }).is_err());
        assert_eq!(shell.stamp(), "shell:w1");
    }

    #[test]
    fn it_agent_grants() {
        let srv = Scope::server();
        let mut f = facts(&srv, "owner", true);
        let it = agent("it", true);
        can_grant(&it, &f, &Consumer::workspace("sales"), Delivery::Fetch, Delivery::Fetch).expect("other ws");
        assert_eq!(can_grant(&it, &f, &Consumer::workspace("it"), Delivery::Fetch, Delivery::Fetch).expect_err("self").code, "vault_self_grant");
        assert_eq!(can_grant(&it, &f, &Consumer::all_workspaces(), Delivery::Fetch, Delivery::Fetch).expect_err("all").code, "vault_self_grant");
        // R8: its own published service runs its code: a self-grant too.
        assert_eq!(
            can_grant(&it, &f, &Consumer::published_service("svc-1"), Delivery::Env, Delivery::Fetch).expect_err("own service").code,
            "vault_self_grant"
        );
        assert!(can_grant(&it, &f, &Consumer::workspace("sales"), Delivery::Env, Delivery::Fetch).is_err(), "human-given: no raise");
        f.own_ws_has_grant = true;
        f.human_given = false;
        assert!(can_grant(&it, &f, &Consumer::workspace("sales"), Delivery::Env, Delivery::Fetch).is_err());
        let plain = agent("sales", false);
        assert!(can_grant(&plain, &facts(&srv, "owner", true), &Consumer::workspace("x"), Delivery::Fetch, Delivery::Fetch).is_err());
    }

    /// SE15 / SV7: no agent grants to `compute:`, not even an IT agent
    /// granting a server secret to another workspace's compute jobs, nor an
    /// agent its own workspace's secret to its own compute jobs. People do.
    #[test]
    fn only_people_grant_to_compute() {
        let srv = Scope::server();
        let ws = Scope::workspace("w1");
        let it = agent("it", true);
        let other = Consumer::compute("sales", "*");
        let e = can_grant(&it, &facts(&srv, "agent:it/s1", false), &other, Delivery::Env, Delivery::Env).expect_err("IT agent");
        assert_eq!(e.code, "vault_owner_confirm_required");
        assert!(e.hint.contains("k2 vault grant") && e.hint.contains("compute:sales/*"), "{}", e.hint);
        // The same IT agent may still grant that server secret to the other workspace itself.
        can_grant(&it, &facts(&srv, "agent:it/s1", false), &Consumer::workspace("sales"), Delivery::Fetch, Delivery::Fetch).expect("IT grant to a workspace");
        let own = agent("w1", false);
        let e = can_grant(&own, &facts(&ws, "agent:w1/s1", false), &Consumer::compute("w1", "node-1"), Delivery::Env, Delivery::Env).expect_err("own");
        assert_eq!(e.code, "vault_owner_confirm_required");
        let shell = Actor::Agent(AgentActor { workspace_id: "w1".into(), session_id: None, shell: true, vault_manage: true });
        assert!(can_grant(&shell, &facts(&ws, "owner", true), &Consumer::compute("w1", "*"), Delivery::Env, Delivery::Env).is_err());
        let member = Actor::Login { username: "m".into(), role: Role::Member };
        assert_eq!(can_grant(&member, &facts(&ws, "owner", true), &Consumer::compute("w1", "*"), Delivery::Env, Delivery::Env).expect_err("member").code, "vault_admin_required");
        let guest = Actor::Guest { username: "g".into(), workspace_id: "w1".into() };
        assert!(can_grant(&guest, &facts(&ws, "owner", true), &Consumer::compute("w1", "*"), Delivery::Env, Delivery::Env).is_err());
        can_grant(&Actor::Owner, &facts(&ws, "owner", true), &Consumer::compute("w1", "node-1"), Delivery::Env, Delivery::Fetch).expect("owner");
        let admin = Actor::Login { username: "a".into(), role: Role::Admin };
        can_grant(&admin, &facts(&srv, "owner", true), &Consumer::compute("w1", "*"), Delivery::Env, Delivery::Fetch).expect("admin");
    }

    #[test]
    fn agent_grants_within_own_workspace() {
        let ws = Scope::workspace("w1");
        let f = facts(&ws, "owner", true);
        let a = agent("w1", false);
        can_grant(&a, &f, &Consumer::session("s1"), Delivery::Fetch, Delivery::Fetch).expect("own session, same level");
        assert!(can_grant(&a, &f, &Consumer::session("s1"), Delivery::Env, Delivery::Fetch).is_err(), "raise");
        assert!(can_grant(&a, &f, &Consumer::workspace("w2"), Delivery::Fetch, Delivery::Fetch).is_err(), "other ws");
    }

    /// Accepting is consent (2026-10-30): the policy edge, without a vault.
    #[test]
    fn an_accepted_handoff_covers_its_own_workspace_up_to_the_ceiling_only() {
        let ws = Scope::workspace("w1");
        let mut f = facts(&ws, "system:handoff", false);
        let a = agent("w1", false);
        let own = Consumer::workspace("w1");
        // Not accepted by a person (or not a handoff copy): as before.
        assert_eq!(can_grant(&a, &f, &own, Delivery::Env, Delivery::File).expect_err("no consent").code, "vault_owner_confirm_required");
        f.handoff_ceiling = Some(Delivery::Env);
        can_grant(&a, &f, &own, Delivery::Env, Delivery::File).expect("own workspace, at the ceiling");
        can_grant(&a, &f, &Consumer::session("s1"), Delivery::Env, Delivery::File).expect("own session");
        can_grant(&a, &f, &Consumer::job("w1", "collect"), Delivery::Env, Delivery::File).expect("own job");
        // Above the ceiling, to another workspace, to every workspace, to
        // a service, to a compute node, and for another workspace's agent.
        assert_eq!(can_grant(&a, &f, &own, Delivery::Api, Delivery::File).expect_err("api").code, "vault_owner_confirm_required");
        assert_eq!(can_grant(&a, &f, &Consumer::workspace("w2"), Delivery::File, Delivery::File).expect_err("w2").code, "vault_owner_confirm_required");
        assert_eq!(can_grant(&a, &f, &Consumer::all_workspaces(), Delivery::File, Delivery::File).expect_err("ws:*").code, "vault_owner_confirm_required");
        assert_eq!(can_grant(&a, &f, &Consumer::published_service("svc"), Delivery::Env, Delivery::File).expect_err("svc").code, "vault_owner_confirm_required");
        assert_eq!(can_grant(&a, &f, &Consumer::compute("w1", "n1"), Delivery::Env, Delivery::File).expect_err("compute").code, "vault_owner_confirm_required");
        assert!(can_grant(&agent("w2", false), &f, &Consumer::workspace("w2"), Delivery::File, Delivery::File).is_err(), "another workspace's agent");
        // A lower accepted ceiling caps it.
        f.handoff_ceiling = Some(Delivery::File);
        can_grant(&a, &f, &own, Delivery::File, Delivery::Fetch).expect("file");
        assert_eq!(can_grant(&a, &f, &own, Delivery::Env, Delivery::Fetch).expect_err("env above file").code, "vault_owner_confirm_required");
        // A ceiling above env (never written by a landing) covers nothing.
        f.handoff_ceiling = Some(Delivery::Api);
        assert!(!accepted_handoff_covers(&a, &f, Delivery::File));
        // A person-given secret, a server secret, people and guests: no.
        f.handoff_ceiling = Some(Delivery::Env);
        f.human_given = true;
        assert!(!accepted_handoff_covers(&a, &f, Delivery::File));
        f.human_given = false;
        let srv = Scope::server();
        let fs = Facts { scope: &srv, ..f.clone() };
        assert!(!accepted_handoff_covers(&agent("w1", true), &fs, Delivery::File));
        assert!(!accepted_handoff_covers(&Actor::Guest { username: "g".into(), workspace_id: "w1".into() }, &f, Delivery::File));
        // It never makes can_loosen pass (sites, ceiling, injection).
        assert_eq!(can_loosen(&a, &f).expect_err("loosen").code, "vault_owner_confirm_required");
    }

    #[test]
    fn reveal_needs_admin_and_confirm() {
        let c = HumanProof::Confirmed { method: "password".into() };
        can_reveal(&Actor::Owner, Namespace::User, &c).expect("ok");
        assert_eq!(can_reveal(&Actor::Owner, Namespace::User, &HumanProof::None).expect_err("no proof").code, "vault_confirm_required");
        assert!(can_reveal(&Actor::Owner, Namespace::System, &c).is_err());
        assert!(can_reveal(&agent("w", true), Namespace::User, &c).is_err());
        let m = Actor::Login { username: "m".into(), role: Role::Member };
        assert!(can_reveal(&m, Namespace::User, &c).is_err());
    }

    #[test]
    fn visibility() {
        let ws = Scope::workspace("w1");
        assert!(can_see(&agent("w1", false), &facts(&ws, "owner", true), false));
        assert!(!can_see(&agent("w2", false), &facts(&ws, "owner", true), false));
        assert!(can_see(&agent("w2", false), &facts(&ws, "owner", true), true));
        let srv = Scope::server();
        assert!(!can_see(&agent("w2", false), &facts(&srv, "owner", true), false));
        assert!(can_see(&agent("w2", true), &facts(&srv, "owner", true), false));
        let sys = Facts { namespace: Namespace::System, scope: &srv, created_by: "system:mail", human_given: false, own_ws_has_grant: false, handoff_ceiling: None };
        assert!(can_see(&Actor::Owner, &sys, false));
        assert!(!can_see(&agent("w1", true), &sys, false));
    }
}
