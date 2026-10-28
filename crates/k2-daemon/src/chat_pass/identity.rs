//! Who a chat pass names (S28 §5 as amended by rev 134, Rosson's 10-15
//! rulings; K2 work item K7).
//!
//! The route reads exactly one credential, `Authorization: Bearer`, and
//! never `?token=` or a cookie.
//!
//! | Credential | `sub` | `sid` | provenance |
//! |---|---|---|---|
//! | owner token | `k2:owner` | `own_<32 hex SHA-256(token)>` | `owner-token` |
//! | App login session (`k2skn_`, `session=1`) + `chat` grant | `k2:principal:<id>` | `skn_<tokens.id>` | `app-session` |
//! | Connect login (any role) | `k2:connect:<uuid>` | `cu_<32 hex SHA-256(token)>` | `connect-login` |
//! | the owner's linked Connect login (`ownerAccounts`) | `k2:owner` | `cu_<32 hex SHA-256(token)>` | `connect-login` |
//! | owner handoff code (no bearer) | `k2:owner` | `own_<32 hex SHA-256(code)>` | `owner-handoff` |
//!
//! **Owner-linked login (Sew 2026-10-18).** A Connect login the owner
//! linked IS the owner person, so its pass names `k2:owner` (one person,
//! one member) with its own `cu_` sid: a `k2_sid` revoke ends just that
//! session, while an `external_id: k2:owner` revoke ends every owner
//! session, this one included (intended). It never gets a `connect` grant
//! or a `k2:connect` member.
//!
//! Refused: an App session without a `chat` grant (403 `no_chat_grant`),
//! platform app passes (`session=0`), agent passports, `k2sk_` API keys
//! (403 `credential_not_allowed`); anything unknown (401).
//!
//! **Owner-role Connect users ([`OWNER_CONNECT_GETS_K2_OWNER`]).** Rosson
//! (2026-10-15): K2 Connect owners and admins control the Sew install and
//! its management on the runtime; inside Sew the top level is delegated
//! through the App's own user roles. So an Owner-role Connect login gets
//! its OWN member identity (`k2:connect:<uuid>`), never `k2:owner`, and
//! its `sewRole` is `member` like every Connect login: K2 Connect Owner is
//! server control only, not Sew install admin (pending Rosson; the field
//! must not suggest admin). `k2:owner` comes only from the
//! box owner token (provenance `owner-token`, never person proof), a
//! person-confirmed owner handoff, or the owner's linked Connect login
//! (above; linking is the owner's act, not a role).
//!
//! **No pass is person proof** (KM13 §19a): see the module doc of
//! [`super`].

use sha2::{Digest, Sha256};

use k2_core::connect_users::Role;

/// The single switch for the Owner-role Connect ruling. `false` (decided,
/// Rosson 2026-10-15): own member identity, `sewRole` member. `true` would
/// mint `k2:owner` (with the same `cu_` sid) for an Owner-role Connect
/// login.
pub const OWNER_CONNECT_GETS_K2_OWNER: bool = false;

pub const OWNER_SUB: &str = "k2:owner";

/// How the identity was established. Shown in the mint response and the
/// audit; never a claim, never consent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provenance {
    /// The box owner token: whoever holds it (agents on shared-user boxes
    /// can read it). Attribution only.
    OwnerToken,
    /// An App (skin) login session for a principal.
    AppSession,
    /// A Connect login session.
    ConnectLogin,
    /// A single-use code a person confirmed at mint (vault confirm).
    OwnerHandoff,
}

impl Provenance {
    pub fn as_str(self) -> &'static str {
        match self {
            Provenance::OwnerToken => "owner-token",
            Provenance::AppSession => "app-session",
            Provenance::ConnectLogin => "connect-login",
            Provenance::OwnerHandoff => "owner-handoff",
        }
    }
}

/// What the identity may do in Sew, for K5's provisioning (the pass itself
/// carries identity, not authority).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SewRole {
    Member,
    /// Sew's install owner (`k2:owner`).
    InstallOwner,
}

impl SewRole {
    pub fn as_str(self) -> &'static str {
        match self {
            SewRole::Member => "member",
            SewRole::InstallOwner => "install-owner",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassIdentity {
    pub sub: String,
    pub sid: String,
    /// Audit `user` (never a secret): `owner-token`, `principal:<name>`,
    /// `user:<connect name>`, `handoff:<8 hex>`.
    pub actor: String,
    pub provenance: Provenance,
    pub sew_role: SewRole,
    /// The `caps` ceiling (subset of [`super::CHAT_CAPS`]); empty → no claim.
    pub caps: Vec<String>,
    /// App sessions: the App the login is bound to (`host` or a service id).
    pub app: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// 401 `unauthenticated`.
    Unauthenticated,
    /// 403 `credential_not_allowed` with a hint and an audit label.
    NotAllowed { hint: &'static str, actor: String },
    /// 403 `no_chat_grant`.
    NoChatGrant { actor: String },
    /// 403 `tied_pending_reset` (agent-app-users A3): an agent could know
    /// this person's credentials, and a chat grant is box-wide, so no chat
    /// pass until their own password reset drops the tie.
    TiedPendingReset { actor: String },
    /// 403 `no_chat_caps` (Sew for Apps A1): the App declares chat caps in
    /// its manifest, and this person's App role holds none of them that
    /// the owner approved for the App. Never an uncapped pass instead.
    NoChatCaps { actor: String },
    /// 503 `chat_unavailable` (a store failed).
    Unavailable(String),
}

fn sha_hex(s: &str) -> String {
    crate::es256::bytes_to_hex(&Sha256::digest(s.as_bytes()))
}

/// `own_<first 32 hex of SHA-256(owner token)>`.
pub fn owner_sid(owner_token: &str) -> String {
    format!("own_{}", &sha_hex(owner_token)[..32])
}

/// The App-role caps a pass may carry: the session snapshot's caps that
/// are chat caps (Sew for Apps A1: an App role lists them on any room
/// line; the snapshot is the union), in [`super::CHAT_CAPS`] order.
pub fn chat_caps_of(snapshot_caps: &[String]) -> Vec<String> {
    let mut out: Vec<String> = super::CHAT_CAPS
        .iter()
        .filter(|c| snapshot_caps.iter().any(|s| s == *c))
        .map(|c| c.to_string())
        .collect();
    out.dedup();
    out
}

/// What an App's own manifest says about chat (Sew for Apps A1, the
/// manifest ceiling): `effective = manifest ∩ owner-approved ∩ role`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppChatCeiling {
    /// No runtime App behind the login (the box login `host`, a published
    /// `--cmd` service, a test): the role's own chat caps stand.
    NotManaged,
    /// The App's live manifest names no chat cap (Hearth and Meet before
    /// they declare any): the role's own chat caps stand, as before A1.
    NotDeclared,
    /// The App asks for these chat caps and the owner approved them (a
    /// suspended App, or one whose chat caps wait for approval, is
    /// `Declared([])`).
    Declared(Vec<String>),
}

/// The caps a pass carries for a role's `role_chat` under the App's
/// ceiling. `Err(())` = the App declares chat caps and nothing is left:
/// refuse (`no_chat_caps`), because an empty list would mint a pass with
/// no `caps` claim, which Sew reads as no ceiling at all.
pub fn narrow_to_app(role_chat: &[String], ceiling: &AppChatCeiling) -> Result<Vec<String>, ()> {
    match ceiling {
        AppChatCeiling::NotManaged | AppChatCeiling::NotDeclared => Ok(role_chat.to_vec()),
        AppChatCeiling::Declared(allowed) => {
            let out: Vec<String> = role_chat.iter().filter(|c| allowed.iter().any(|a| a == *c)).cloned().collect();
            if out.is_empty() {
                Err(())
            } else {
                Ok(out)
            }
        }
    }
}

/// A Connect login's identity under the ruling `owner_connect_is_k2_owner`.
pub fn connect_identity(
    username: &str,
    role: Role,
    user_uuid: &str,
    session_token: &str,
    owner_connect_is_k2_owner: bool,
) -> PassIdentity {
    let sid = k2_core::connect_users::session_sid(session_token);
    let (sub, sew_role) = if role == Role::Owner && owner_connect_is_k2_owner {
        (OWNER_SUB.to_string(), SewRole::InstallOwner)
    } else {
        (format!("k2:connect:{user_uuid}"), SewRole::Member)
    };
    PassIdentity {
        sub,
        sid,
        actor: format!("user:{username}"),
        provenance: Provenance::ConnectLogin,
        sew_role,
        caps: Vec::new(),
        app: None,
    }
}

/// The owner's linked Connect login (`ownerAccounts`, Sew 2026-10-18): it
/// IS the owner person, so its pass names `k2:owner` (one person, one
/// member) with ITS Connect session as `sid` (`cu_…`), so revoking that
/// sid ends just that session. It keeps a Connect pass's own caps ceiling
/// (today none, as for every Connect login), never the owner token's. No
/// `connect` grant or provision is ever made for it.
pub fn owner_linked_identity(username: &str, session_token: &str) -> PassIdentity {
    PassIdentity {
        sub: OWNER_SUB.to_string(),
        sid: k2_core::connect_users::session_sid(session_token),
        actor: format!("user:{username}"),
        provenance: Provenance::ConnectLogin,
        sew_role: SewRole::InstallOwner,
        caps: Vec::new(),
        app: None,
    }
}

/// Classify a Bearer credential (S28 §5) with the default ruling.
pub fn classify(bearer: &str, owner_token: &str) -> Result<PassIdentity, Refusal> {
    classify_with(bearer, owner_token, OWNER_CONNECT_GETS_K2_OWNER)
}

/// [`classify`] with the Owner-role Connect ruling given (tests flip it).
pub fn classify_with(bearer: &str, owner_token: &str, owner_connect_is_k2_owner: bool) -> Result<PassIdentity, Refusal> {
    let tok = bearer.trim();
    if tok.is_empty() {
        return Err(Refusal::Unauthenticated);
    }
    if !owner_token.is_empty() && crate::routes::http::ct_eq_token(tok, owner_token) {
        return Ok(PassIdentity {
            sub: OWNER_SUB.into(),
            sid: owner_sid(tok),
            actor: "owner-token".into(),
            provenance: Provenance::OwnerToken,
            sew_role: SewRole::InstallOwner,
            caps: Vec::new(),
            app: None,
        });
    }
    if k2_core::skin::is_api_key_prefix(tok) {
        return Err(Refusal::NotAllowed { hint: "API keys can't get chat passes", actor: "api-key".into() });
    }
    if k2_core::skin::is_skin_token(tok) {
        let Some(pass) = k2_core::skin::resolve_skin_token(tok) else {
            return Err(Refusal::Unauthenticated);
        };
        let Some(principal_id) = pass.principal_id.clone().filter(|_| pass.session) else {
            return Err(Refusal::NotAllowed {
                hint: "platform app passes name no person; log the person in to the App first",
                actor: format!("app-token:{}", pass.username),
            });
        };
        let actor = format!("principal:{}", pass.username);
        match k2_core::skin::principal_has_chat_grant(&principal_id) {
            Ok(true) => {}
            Ok(false) => return Err(Refusal::NoChatGrant { actor }),
            Err(e) => return Err(Refusal::Unavailable(e)),
        }
        match k2_core::skin::ties::get(&principal_id) {
            Ok(None) => {}
            Ok(Some(_)) => return Err(Refusal::TiedPendingReset { actor }),
            Err(e) => return Err(Refusal::Unavailable(e)),
        }
        // A1: the role's chat caps under the App's own manifest ceiling.
        let ceiling = if pass.app == k2_core::skin::HOST_APP_ID {
            AppChatCeiling::NotManaged
        } else {
            crate::apps::grants::chat_ceiling(&pass.app).map_err(Refusal::Unavailable)?
        };
        let Ok(caps) = narrow_to_app(&chat_caps_of(&pass.caps), &ceiling) else {
            return Err(Refusal::NoChatCaps { actor });
        };
        return Ok(PassIdentity {
            sub: format!("k2:principal:{principal_id}"),
            sid: k2_core::sew_revoke::skin_sid(&pass.id),
            actor,
            provenance: Provenance::AppSession,
            sew_role: SewRole::Member,
            caps,
            app: Some(pass.app.clone()),
        });
    }
    if crate::session_token::validate_hook(tok).is_some() {
        return Err(Refusal::NotAllowed { hint: "agents never get chat passes", actor: "agent".into() });
    }
    if let Some(username) = k2_core::connect_users::validate_session(tok) {
        let role = k2_core::connect_users::role_for_user(&username).ok_or(Refusal::Unauthenticated)?;
        let uuid = match k2_core::connect_users::user_uuid(&username) {
            Ok(Some(u)) if !u.is_empty() => u,
            Ok(_) => return Err(Refusal::Unauthenticated),
            Err(e) => return Err(Refusal::Unavailable(e)),
        };
        // The owner's linked login IS the owner person: `k2:owner`.
        let people = k2_core::notify_prefs::People::load().map_err(Refusal::Unavailable)?;
        if people.is_linked(&username) {
            return Ok(owner_linked_identity(&username, tok));
        }
        return Ok(connect_identity(&username, role, &uuid, tok, owner_connect_is_k2_owner));
    }
    Err(Refusal::Unauthenticated)
}
