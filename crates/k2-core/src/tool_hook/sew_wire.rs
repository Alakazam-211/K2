//! Sew mgmt wire types K2's grant hook needs (S27).
//!
//! **TEMP — replace with vendored `sew-proto` (work item K1) when Sew's SW8
//! lands.** `sew-proto` is not vendorable yet (git deps; CI cannot fetch the
//! private SSH URL, S27-A4), so this module mirrors exactly the slice of
//! `sew-proto` the hook uses, field for field and with the same serde
//! attributes. Every item names its source so the swap is mechanical:
//! delete this file, add `sew-proto` under `third_party/`, and change
//! `use crate::tool_hook::sew_wire::X` to `use sew_proto::…::X`.
//!
//! | Here | `sew-proto` |
//! |---|---|
//! | `MGMT_*`, header names | `routes.rs`, `http.rs` |
//! | [`ExternalId`] | `external_id.rs` (`ExternalId`) |
//! | [`MemberKind`], [`AgentKind`], [`InstallRole`] | `api/members.rs` |
//! | [`ProfileInput`] … [`OwnerSetResponse`], [`RevokeSessionsRequest`] | `mgmt.rs` |
//! | [`Patch`], [`Page`] | `http.rs` |
//! | [`Status`] (subset) | `mgmt.rs` `Status` |
//! | [`MgmtPrincipal`] | `mgmt.rs` `MgmtPrincipal` (S27-A5; `email`/`title`/`pronouns` = SW14) |
//! | [`ErrorBody`], [`ApiError`], [`codes`] | `error.rs` |
//! | [`reasons`] | `mgmt.rs` `reasons` |
//! | [`AddMembersOutcome`] | `api/spaces.rs` `AddSpaceMembersResponse` / `api/channels.rs` `AddChannelMembersResponse` (HC2 `skipped`) |
//! | [`ServerSpaceCreateRequest`] … [`MediaPathResponse`] | Sew 0.7.0 (sew-56 v1.1d §2.2, K2 PRD v1.9 §6.6): SW-1, SW-3, SW-4, SW-11, not released yet |
//!
//! Differences on purpose (all of them additive-safe for K2 as a client):
//! - Ids (`Id`) and timestamps (`Timestamp`) are kept as `String`: K2 never
//!   does arithmetic on them, and both are strings on the wire.
//! - [`Status`] keeps only the fields K2 reads and ignores the rest.
//! - `ProfileInput.custom_fields` / `ProfilePatch.custom_fields` (S18) are
//!   omitted: K2 never sends provider custom fields.
//! - `OwnerSetRequest.approval` is an opaque JSON value: K2 never sends one
//!   (first owner on an empty install, or a no-op replay, G6).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

// ── routes (`sew_proto::routes`) ─────────────────────────────────────

pub const MGMT_STATUS: &str = "/mgmt/v1/status";
pub const MGMT_PRINCIPALS: &str = "/mgmt/v1/principals";
pub const MGMT_PROVISION: &str = "/mgmt/v1/principals/provision";
pub const MGMT_UPDATE: &str = "/mgmt/v1/principals/update";
pub const MGMT_DEPROVISION: &str = "/mgmt/v1/principals/deprovision";
pub const MGMT_OWNER_SET: &str = "/mgmt/v1/owner/set";
/// S28's `sew_session_revoke` op (S27-A12), delivered by the worker.
pub const MGMT_SESSIONS_REVOKE: &str = "/mgmt/v1/sessions/revoke";

// ── headers (`sew_proto::http`) ──────────────────────────────────────

pub const IDEMPOTENCY_KEY: &str = "Idempotency-Key";
pub const IDEMPOTENT_REPLAYED: &str = "Idempotent-Replayed";
pub const RETRY_AFTER: &str = "Retry-After";
/// Names the K2 caller behind a mgmt call. **Attribution only** (KM13
/// §19a): on shared-user boxes agents can read the owner token, so this is
/// never proof a person acted. Sew records it as `details.k2_actor` and
/// labels it "reported by K2".
pub const SEW_ACTOR: &str = "Sew-Actor";

/// Page size K2 asks for when listing principals (`PRINCIPALS_LIMIT_MAX`).
pub const PRINCIPALS_LIMIT_MAX: u32 = 500;

// ── grammar helpers ──────────────────────────────────────────────────

/// `sew_proto::addr::is_valid_handle`: `^[a-z0-9_-]{2,64}$`.
pub fn is_valid_handle(s: &str) -> bool {
    (2..=64).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// `sew_proto::mgmt::is_sew_actor`: 1–128 chars of `[A-Za-z0-9_.:@-]`.
pub fn is_sew_actor(s: &str) -> bool {
    (1..=128).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'.' | b':' | b'@' | b'-'))
}

/// `Idempotency-Key` grammar (S1 §6.7): `[A-Za-z0-9_-]{1,64}`.
pub fn is_idempotency_key(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// Lowercase hyphenated uuid (the `ExternalId` grammar's `<uuid>`).
pub fn is_lower_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(c),
        })
}

// ── ExternalId (`sew_proto::external_id`) ────────────────────────────

/// Who a member is outside Sew. Wire forms: `k2:principal:<uuid>`,
/// `k2:workspace:<uuid>`, `k2:connect:<uuid>` (SW2: a Connect login, by
/// its never-reused K7 id), `k2:owner`, `local:<handle>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExternalId {
    K2Principal(String),
    K2Workspace(String),
    /// A K2 Connect login (its `connect_users` id). Provisioned only from
    /// a `connect` chat grant (written by the owner, or by the login's own
    /// first chat-pass mint), never by role (rev 147).
    K2Connect(String),
    K2Owner,
    Local(String),
    /// App-governed Spaces (Sew 0.6.0 §3a, flag `app_governed_spaces`): an
    /// App's **service owner**, `app:<app_id>` (one part, no user). It owns
    /// the App's Spaces, never signs in, and is hidden as a person. K2 names
    /// it only as a Space owner (`spaces/create`) and in its deprovision on
    /// uninstall; it is never a chat grant subject. A two-part
    /// `app:<app>:<user>` (App-native users, not in 0.6.0) stays
    /// [`ExternalId::Other`] when read, and is never sent.
    AppService(String),
    /// Sew 0.7.0 SW-1 (flag `server_governed_space`): K2's **service
    /// owner**, exactly `k2:server`. It owns the box's one K2 Admin Space
    /// (governance `server`), never signs in (a pass for it answers 401),
    /// is hidden as a person and is never provisioned. Sew makes it on the
    /// first `spaces/create` that names it.
    K2Server,
    /// Not in sew-proto: a form this mirror does not know yet (e.g. a
    /// Meet link guest `link:<channel>:<id>`, K-M6). Only produced by
    /// lenient deserialization, so a newer Sew's member list still parses;
    /// reconcile ignores it. **Keep this lenient arm when K1 swaps in the
    /// vendored sew-proto**, whose `ExternalId` refuses unknown forms.
    Other(String),
}

impl ExternalId {
    pub fn parse(s: &str) -> Result<ExternalId, String> {
        let bad = || {
            format!(
                "invalid external id {s:?}: expected k2:principal:<uuid>, k2:workspace:<uuid>, k2:connect:<uuid>, k2:owner, k2:server, app:<app id> or local:<handle>"
            )
        };
        if s == K2_SERVER {
            return Ok(ExternalId::K2Server);
        }
        if let Some(u) = s.strip_prefix("k2:connect:") {
            return if is_lower_uuid(u) {
                Ok(ExternalId::K2Connect(u.to_string()))
            } else {
                Err(bad())
            };
        }
        if s == "k2:owner" {
            return Ok(ExternalId::K2Owner);
        }
        if let Some(u) = s.strip_prefix("k2:principal:") {
            return if is_lower_uuid(u) {
                Ok(ExternalId::K2Principal(u.to_string()))
            } else {
                Err(bad())
            };
        }
        if let Some(u) = s.strip_prefix("k2:workspace:") {
            return if is_lower_uuid(u) {
                Ok(ExternalId::K2Workspace(u.to_string()))
            } else {
                Err(bad())
            };
        }
        if let Some(a) = s.strip_prefix("app:") {
            return if is_app_id(a) && a != crate::skin::HOST_APP_ID {
                Ok(ExternalId::AppService(a.to_string()))
            } else {
                Err(bad())
            };
        }
        if let Some(h) = s.strip_prefix("local:") {
            return if is_valid_handle(h) {
                Ok(ExternalId::Local(h.to_string()))
            } else {
                Err(bad())
            };
        }
        Err(bad())
    }

    /// A form sew-proto knows (not [`ExternalId::Other`]).
    pub fn is_known(&self) -> bool {
        !matches!(self, ExternalId::Other(_))
    }

    /// `k2:workspace:*`.
    pub fn is_agent(&self) -> bool {
        matches!(self, ExternalId::K2Workspace(_))
    }

    /// A box user: the owner (`k2:owner`) or a Connect login
    /// (`k2:connect:`). Never a member of an App-governed Space, never
    /// synced (Rosson 2026-10-30, ruling 1).
    pub fn is_box_user(&self) -> bool {
        matches!(self, ExternalId::K2Owner | ExternalId::K2Connect(_))
    }

    /// The service owner of App `app_id` (`app:<app_id>`).
    pub fn app_service(app_id: &str) -> Result<ExternalId, String> {
        ExternalId::parse(&format!("app:{app_id}"))
    }

    /// A service owner (`app:<app_id>` or `k2:server`): owns Spaces, never
    /// signs in, never provisioned or synced by K2 as a person.
    pub fn is_service_owner(&self) -> bool {
        matches!(self, ExternalId::AppService(_) | ExternalId::K2Server)
    }

    /// SW-1: an id Sew refuses in a `server` Space with `422 invalid_state`
    /// `app_user_not_allowed` (`k2:principal:` App users and `app:` ids).
    /// K2 never sends one there.
    pub fn is_app_user_or_app(&self) -> bool {
        matches!(self, ExternalId::K2Principal(_) | ExternalId::AppService(_))
    }
}

impl fmt::Display for ExternalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExternalId::K2Principal(u) => write!(f, "k2:principal:{u}"),
            ExternalId::K2Workspace(u) => write!(f, "k2:workspace:{u}"),
            ExternalId::K2Connect(u) => write!(f, "k2:connect:{u}"),
            ExternalId::K2Owner => f.write_str("k2:owner"),
            ExternalId::K2Server => f.write_str(K2_SERVER),
            ExternalId::Local(h) => write!(f, "local:{h}"),
            ExternalId::AppService(a) => write!(f, "app:{a}"),
            ExternalId::Other(raw) => f.write_str(raw),
        }
    }
}

impl Serialize for ExternalId {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ExternalId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Ok(ExternalId::parse(&raw).unwrap_or(ExternalId::Other(raw)))
    }
}

// ── enums (`sew_proto::api::members`) ────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemberKind {
    Person,
    Agent,
}

impl MemberKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MemberKind::Person => "person",
            MemberKind::Agent => "agent",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Coworker,
    Assistant,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Coworker => "coworker",
            AgentKind::Assistant => "assistant",
        }
    }
}

/// Install-wide role. `Unknown` keeps a newer Sew's role from failing the
/// whole list parse (sew-proto's `unit_enum!` has the same fallback).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallRole {
    Owner,
    Admin,
    Member,
    Guest,
    #[serde(other)]
    Unknown,
}

// ── Patch / Page (`sew_proto::http`) ─────────────────────────────────

/// Tri-state update field: absent = `Keep`, `null` = `Clear`, a value =
/// `Set`. Fields use `#[serde(default, skip_serializing_if = "Patch::is_keep")]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub enum Patch<T> {
    #[default]
    Keep,
    Clear,
    Set(T),
}

impl<T> Patch<T> {
    pub fn is_keep(&self) -> bool {
        matches!(self, Patch::Keep)
    }
    pub fn as_set(&self) -> Option<&T> {
        match self {
            Patch::Set(v) => Some(v),
            _ => None,
        }
    }
}

impl<T: Serialize> Serialize for Patch<T> {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Patch::Keep | Patch::Clear => s.serialize_none(),
            Patch::Set(v) => s.serialize_some(v),
        }
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Patch<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match Option::<T>::deserialize(d)? {
            Some(v) => Patch::Set(v),
            None => Patch::Clear,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub has_more: bool,
}

// ── provisioning (`sew_proto::mgmt`) ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileInput {
    /// 1–100 characters.
    pub display_name: String,
    /// Handle, `[a-z0-9_-]{2,64}`.
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pronouns: Option<String>,
}

/// `POST /mgmt/v1/principals/provision`. Idempotent by `external_id`; an
/// upsert of state on an existing member (G3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisionRequest {
    pub external_id: ExternalId,
    pub kind: MemberKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_of: Option<ExternalId>,
    #[serde(default)]
    pub guest: bool,
    pub profile: ProfileInput,
}

/// `sew_proto::mgmt::ProvisionRequest::validate` (the rules K2 can break).
/// The fake Sew runs it on every body; K2 runs it before enqueue as a belt.
pub fn validate_provision(r: &ProvisionRequest) -> Result<(), String> {
    match r.kind {
        MemberKind::Agent => {
            let Some(agent_kind) = r.agent_kind else {
                return Err("agent_kind: agent_kind is required for agents".into());
            };
            if !r.external_id.is_agent() {
                return Err(
                    "external_id: an agent's external_id must be k2:workspace:<uuid>".into(),
                );
            }
            if r.guest {
                return Err("guest: agents cannot be guests".into());
            }
            match (agent_kind, &r.assistant_of) {
                (AgentKind::Assistant, None) => {
                    return Err("assistant_of: assistant_of is required for assistants".into())
                }
                (AgentKind::Coworker, Some(_)) => {
                    return Err("assistant_of: assistant_of is only for assistants".into())
                }
                _ => {}
            }
            if r.assistant_of.as_ref().is_some_and(ExternalId::is_agent) {
                return Err("assistant_of: an assistant belongs to a person".into());
            }
        }
        MemberKind::Person => {
            if r.agent_kind.is_some() {
                return Err("agent_kind: agent_kind is only for agents".into());
            }
            if r.assistant_of.is_some() {
                return Err("assistant_of: assistant_of is only for assistants".into());
            }
            if r.external_id.is_agent() {
                return Err(
                    "external_id: a person's external_id must not be k2:workspace:*".into(),
                );
            }
        }
    }
    let n = r.profile.display_name.chars().count();
    if !(1..=100).contains(&n) {
        return Err("profile.display_name: must be 1–100 characters".into());
    }
    if !is_valid_handle(&r.profile.username) {
        return Err("profile.username: must be [a-z0-9_-]{2,64}".into());
    }
    if let Some(e) = &r.profile.email {
        if e.is_empty() || e.len() > 254 || !e.contains('@') {
            return Err("profile.email: must be an address of at most 254 bytes".into());
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProvisionResponse {
    pub member_id: String,
    pub created: bool,
    #[serde(default)]
    pub reactivated: bool,
    #[serde(default)]
    pub changed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub email: Patch<String>,
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub title: Patch<String>,
    #[serde(default, skip_serializing_if = "Patch::is_keep")]
    pub pronouns: Patch<String>,
}

/// `POST /mgmt/v1/principals/update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatePrincipalRequest {
    pub external_id: ExternalId,
    pub profile: ProfilePatch,
}

/// `POST /mgmt/v1/principals/deprovision`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeprovisionRequest {
    pub external_id: ExternalId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeprovisionResponse {
    pub member_id: String,
    pub sessions_revoked: u64,
}

/// `POST /mgmt/v1/sessions/revoke` — S28's `sew_session_revoke` op
/// (S27-A12). K2 sends exactly one of the fields
/// ([`crate::sew_revoke::SewSessionRevoke::payload`]). `app_id` is Sew
/// 0.6.0's I8 (flag `app_governed_spaces`): every session whose `app`
/// claim is that App.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeSessionsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<ExternalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k2_sid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
}

/// `POST /mgmt/v1/owner/set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerSetRequest {
    pub external_id: ExternalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval: Option<Value>,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OwnerSetResponse {
    pub owner_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous_owner_id: Option<String>,
    pub changed: bool,
}

/// `GET /mgmt/v1/status`, the fields K2 reads. Unknown fields ignored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    #[serde(default)]
    pub version: String,
    pub install_id: String,
    /// `k2` | `standalone`.
    pub provider: String,
    /// Feature flags (Sew 2026-10-28): sorted, never renamed. K2 gates on
    /// these, never on `version` ([`super::features`]). Absent on an older
    /// Sew: none.
    #[serde(default)]
    pub features: Vec<String>,
    /// Sew 0.7.0: `status.voice` (K2 reads only `media_path`, SW-11).
    /// Absent on an older Sew.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<StatusVoice>,
}

/// `status.voice`, the part K2 reads. Unknown fields ignored.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusVoice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_path: Option<MediaPathStatus>,
}

/// SW-11 `status.voice.media_path {state, since, media_path_reason}`.
/// Sew keeps it in memory and starts `up` (K2 re-asserts `down`, M11).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPathStatus {
    /// [`MEDIA_PATH_UP`] | [`MEDIA_PATH_DOWN`].
    #[serde(default)]
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_path_reason: Option<String>,
}

/// One row of `GET /mgmt/v1/principals` (S27-A5: no `guest` bool, no
/// nested profile, no email; guest = `install_role == guest`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MgmtPrincipal {
    pub member_id: String,
    pub external_id: ExternalId,
    pub kind: MemberKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_kind: Option<AgentKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assistant_of: Option<ExternalId>,
    pub install_role: InstallRole,
    pub username: String,
    pub display_name: String,
    pub deactivated: bool,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deactivated_at: Option<String>,
    /// SW14 (coming): read leniently. `None` = the field is absent (an
    /// older Sew): UNKNOWN, so K2 never clears what it cannot see.
    /// `Some(None)` = Sew says there is none. `Some(Some(v))` = the value.
    #[serde(
        default,
        deserialize_with = "known",
        skip_serializing_if = "Option::is_none"
    )]
    pub email: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "known",
        skip_serializing_if = "Option::is_none"
    )]
    pub title: Option<Option<String>>,
    #[serde(
        default,
        deserialize_with = "known",
        skip_serializing_if = "Option::is_none"
    )]
    pub pronouns: Option<Option<String>>,
}

/// Absent → `None` (via `default`); present `null` → `Some(None)`.
fn known<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Ok(Some(Option::<String>::deserialize(d)?))
}

pub type MgmtPrincipalPage = Page<MgmtPrincipal>;

/// `GET /mgmt/v1/principals?external_id=<ext>&limit=1` (PrincipalsQuery):
/// one member by external id, deactivated included.
pub fn principal_lookup_path(ext: &ExternalId) -> String {
    format!("{MGMT_PRINCIPALS}?external_id={ext}&limit=1")
}

/// HC2: the batch Space / channel member-add responses
/// (`AddSpaceMembersResponse`, `AddChannelMembersResponse`, and SW13's
/// mgmt per-Space add, empty until HC2 lands). A ban never fails the
/// batch; banned targets come back in `skipped`. `principals/provision`
/// has NO `skipped` field — S27 does not read one there. K5/K31's
/// Space-add op reads it here and records the member `held`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddMembersOutcome {
    #[serde(default)]
    pub added: Vec<String>,
    #[serde(default)]
    pub already: Vec<String>,
    /// Absent on a Sew without HC2: read as empty.
    #[serde(default)]
    pub skipped: Vec<String>,
}

impl AddMembersOutcome {
    /// Lenient read of a 2xx member-add body (`None` if it is not one).
    pub fn parse(body: &[u8]) -> Option<AddMembersOutcome> {
        serde_json::from_slice(body).ok()
    }

    /// Whether `member_id` was skipped (HC2: banned; K2 must not re-add).
    pub fn skipped_member(&self, member_id: &str) -> bool {
        self.skipped.iter().any(|s| s == member_id)
    }
}

// ── App Spaces over mgmt (`sew_proto::mgmt::spaces`, SW13) ───────────

/// `POST`: bind a Space to an App, once (the same App again is a no-op).
pub const MGMT_SPACES_BIND: &str = "/mgmt/v1/spaces/bind";
/// `POST`: add principals to a Space (any Space, not only the default).
pub const MGMT_SPACE_MEMBERS_ADD: &str = "/mgmt/v1/spaces/members/add";
/// `POST`: remove one principal from a Space.
pub const MGMT_SPACE_MEMBERS_REMOVE: &str = "/mgmt/v1/spaces/members/remove";
/// Longest App id Sew takes, bytes.
pub const APP_ID_MAX_BYTES: usize = 64;

/// Sew's App id grammar: 1–64 bytes of `[a-z0-9][a-z0-9._-]*`.
pub fn is_app_id(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=APP_ID_MAX_BYTES).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(*c, b'.' | b'_' | b'-')
        })
}

/// A Sew id as K2 stores it (`Id` is a string on the wire): 1–64 bytes of
/// `[A-Za-z0-9_-]`. K2 never builds one; it only checks what it is given.
pub fn is_sew_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
}

/// `MgmtSpaceBindRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceBindRequest {
    pub space_id: String,
    pub app_id: String,
}

/// `MgmtSpaceMembersAddRequest` (1–100 distinct principals).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceMembersAddRequest {
    pub space_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    pub external_ids: Vec<ExternalId>,
}

/// `MgmtSpaceMemberRemoveRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceMemberRemoveRequest {
    pub space_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    pub external_id: ExternalId,
}

/// `MgmtSpaceMemberRef`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceMemberRef {
    pub external_id: ExternalId,
    pub member_id: String,
}

/// `MgmtSpaceNotAdded` (`reason`: `not_provisioned` | `deactivated` |
/// `assistant` | `guest`; read as a string so a newer reason still parses).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceNotAdded {
    pub external_id: ExternalId,
    pub reason: String,
}

/// `MgmtSpaceNotAddedReason::NotProvisioned`: the member has no Sew
/// account yet (its provision is still queued). The only reason K2 retries.
pub const NOT_ADDED_NOT_PROVISIONED: &str = "not_provisioned";

/// `MgmtSpaceMembersAddResponse`, read leniently (every list defaults to
/// empty; `skipped` = banned member ids, HC2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceMembersAddResponse {
    #[serde(default)]
    pub added: Vec<SpaceMemberRef>,
    #[serde(default)]
    pub already: Vec<SpaceMemberRef>,
    #[serde(default)]
    pub skipped: Vec<String>,
    #[serde(default)]
    pub not_added: Vec<SpaceNotAdded>,
}

/// `MgmtSpaceMemberRemoveResponse`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceMemberRemoveResponse {
    #[serde(default)]
    pub removed: bool,
}

/// `POST` (Sew 0.5.0, flag `space_unbind`): unbind a Space from its App.
/// The Space becomes an orphan of that App: reads and export still work,
/// adds and channel creates answer 422 `invalid_state` reason [`reasons::SPACE_ORPHANED`]; removes still work.
pub const MGMT_SPACES_UNBIND: &str = "/mgmt/v1/spaces/unbind";
/// `POST` (Sew 0.5.0) `{space_id, confirm: true}`: forget an orphan's App
/// so another App may bind it (until then a bind elsewhere answers 409
/// reason [`reasons::SPACE_BOUND_ELSEWHERE`]). A person's call: K2 never
/// sends it on its own.
pub const MGMT_SPACES_RELEASE: &str = "/mgmt/v1/spaces/release";

/// `MgmtSpaceUnbindRequest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceUnbindRequest {
    pub space_id: String,
    pub app_id: String,
}

/// `MgmtSpaceUnbindResponse` (also the answer to `spaces/release`), read
/// leniently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceUnbindResponse {
    pub space_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orphaned_from: Option<String>,
    #[serde(default)]
    pub changed: bool,
}

// ── channels over mgmt (`sew_proto::mgmt::channels`, Sew A9, 0.5.0) ──

/// `POST`: a text channel in an App-bound Space, made as the Space's owner.
/// Idempotent on `(space_id, slug)`. Gated on the feature flag
/// [`super::features::APP_CHANNEL_CREATE`].
pub const MGMT_CHANNELS_CREATE: &str = "/mgmt/v1/channels/create";
/// Most members one channel create adds.
pub const CHANNEL_MEMBERS_MAX: usize = 100;
/// `MgmtChannelNotAddedReason::NotSpaceMember`: add them to the Space first.
pub const CHANNEL_NOT_SPACE_MEMBER: &str = "not_space_member";

/// `MgmtChannelCreateRequest`. `members` are member ids or external ids
/// (K2 sends external ids), each already in the Space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelCreateRequest {
    pub space_id: String,
    pub app_id: String,
    pub slug: String,
    pub name: String,
    pub private: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
}

/// `MgmtChannelNotAdded` (`member` as the request named it; `reason` read
/// as a string so a newer reason still parses).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelNotAdded {
    pub member: String,
    pub reason: String,
}

/// `MgmtChannelCreateResponse`, read leniently (every list defaults to
/// empty). `name` and `private` are the channel's CURRENT values (a repeat
/// never changes them); `skipped` = banned member ids (HC2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelCreateResponse {
    pub channel_id: String,
    pub space_id: String,
    pub slug: String,
    pub name: String,
    pub private: bool,
    pub created: bool,
    #[serde(default)]
    pub added: Vec<SpaceMemberRef>,
    #[serde(default)]
    pub already: Vec<SpaceMemberRef>,
    #[serde(default)]
    pub skipped: Vec<String>,
    #[serde(default)]
    pub not_added: Vec<ChannelNotAdded>,
}

/// A Sew channel slug: `[a-z0-9][a-z0-9-]{0,79}` (`sew_proto::addr::Slug`).
pub fn is_channel_slug(s: &str) -> bool {
    let b = s.as_bytes();
    (1..=80).contains(&b.len())
        && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit())
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

// ── App-governed Spaces (Sew 0.6.0, flag `app_governed_spaces`) ──────
//
// sew-55 v2.4 I3, I4, I6: K2's mirror of Sew's documented shapes (Sew
// 0.6.0 is not released; K2's fake Sew speaks exactly these). Requests are
// `deny_unknown_fields` like every K2 request type; answers are read
// leniently, so a newer field never fails a read.

/// I3: `POST /mgmt/v1/spaces/{space_id}/settings` (PRIV1) gains
/// `governance`.
pub fn space_settings_path(space_id: &str) -> String {
    format!("/mgmt/v1/spaces/{space_id}/settings")
}

/// I4 single: `POST /mgmt/v1/spaces/{space_id}/members/{external_id}/channels`
/// (the external id percent-encoded in the path).
pub fn member_channels_path(space_id: &str, ext: &ExternalId) -> String {
    format!("/mgmt/v1/spaces/{space_id}/members/{}/channels", pct_path(&ext.to_string()))
}

/// I4 bulk: `POST /mgmt/v1/spaces/{space_id}/members/channels`.
pub fn member_channels_bulk_path(space_id: &str) -> String {
    format!("/mgmt/v1/spaces/{space_id}/members/channels")
}

/// I6: `POST /mgmt/v1/channels/bulk`.
pub const MGMT_CHANNELS_BULK: &str = "/mgmt/v1/channels/bulk";

/// Percent-encode one path segment (RFC 3986 unreserved bytes kept).
pub fn pct_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Governance values (I3).
pub const GOVERNANCE_APP: &str = "app";
pub const GOVERNANCE_SEW: &str = "sew";

/// I3 body: `governance`, and the Space's App when it has one (K2 never
/// sends the other PRIV1 field). Sew's settings route answers 409
/// `app_mismatch` unless `app_id` names the Space's App, absent only for a
/// Space that is not bound.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceSettingsRequest {
    /// [`GOVERNANCE_APP`] | [`GOVERNANCE_SEW`].
    pub governance: String,
    /// The bound App's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
}

/// Levels in an App-governed Space (I4), lowest first.
pub const LEVELS: [&str; 3] = ["member", "moderator", "admin"];

/// Rank of a level (`member` 0 … `admin` 2); `None` = not a level.
pub fn level_rank(level: &str) -> Option<usize> {
    LEVELS.iter().position(|l| *l == level)
}

/// I4 single body: the person's FULL list of private channels in the
/// Space (0–5,000) and their level.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberChannelsRequest {
    pub app_id: String,
    pub channel_ids: Vec<String>,
    pub level: String,
}

/// Most channel ids one I4 item carries.
pub const MEMBER_CHANNELS_MAX: usize = 5_000;
/// Most items one I4 bulk call carries.
pub const MEMBER_CHANNELS_BULK_MAX: usize = 500;
/// An item with more channel changes than this runs as a Sew job and
/// answers `queued` (I4, R5).
pub const MEMBER_CHANNELS_JOB_THRESHOLD: usize = 1_000;

/// One I4 bulk item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberChannelsItem {
    pub external_id: ExternalId,
    pub channel_ids: Vec<String>,
    pub level: String,
}

/// I4 bulk body (1–500 items).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MemberChannelsBulkRequest {
    pub app_id: String,
    pub items: Vec<MemberChannelsItem>,
}

/// `skipped` values of an I4 answer (`MgmtMemberChannelsSkip`: exactly one,
/// and only when the item changed nothing for its person).
pub mod skipped {
    /// Banned in that Space: bans win; K2 holds the item.
    pub const BANNED: &str = "banned";
    /// Deprovisioned: a sync never reactivates (R8); K2 holds the item.
    pub const DEPROVISIONED: &str = "deprovisioned";
    /// The Space owner (the App's service owner): a sync never changes it.
    pub const OWNER: &str = "owner";
    /// Sew A11: the Space is at its App's `members_per_space_max`, so the
    /// person could not join. K2 holds the item until room may have freed.
    pub const QUOTA_EXCEEDED: &str = "quota_exceeded";
}

/// I4 answer for one person (Sew 0.6.0 `MgmtMemberChannelsResponse`), read
/// leniently: `{external_id, member_id, added, removed, unchanged, level?,
/// unknown_channel_ids, skipped?, queued, job_id?}`. `level` is absent when
/// `skipped`. `unknown_channel_ids` lists ids that are not live private
/// channels of the Space (a public channel's id too); the rest of the list
/// applied. `queued: true` (with `job_id`) when the item ran as a Sew job
/// (over [`MEMBER_CHANNELS_JOB_THRESHOLD`] channel changes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberChannelsResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    #[serde(default)]
    pub added: Value,
    #[serde(default)]
    pub removed: Value,
    #[serde(default)]
    pub unchanged: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(default)]
    pub unknown_channel_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
    #[serde(default)]
    pub queued: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job_id: Option<String>,
}

/// An error inside a per-item answer (bulk I4, I6), read leniently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemError {
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub hint: String,
    #[serde(default)]
    pub details: Map<String, Value>,
}

impl ItemError {
    /// `details.reason`, else the code.
    pub fn reason(&self) -> &str {
        self.details.get("reason").and_then(Value::as_str).unwrap_or(self.code.as_str())
    }
}

/// One answered I4 bulk item (Sew 0.6.0 `MgmtMemberChannelsItemResult`):
/// `{external_id, status, answer}` on success (status 200), else
/// `{external_id, status, error}` with the error body a single sync would
/// answer (409, 422 …). The answer is nested, never flattened.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberChannelsBulkItem {
    pub external_id: ExternalId,
    pub status: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answer: Option<MemberChannelsResponse>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ItemError>,
}

/// I4 bulk answer, one entry per item.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberChannelsBulkResponse {
    #[serde(default)]
    pub items: Vec<MemberChannelsBulkItem>,
}

/// One I6 op: `ensure` | `rename` | `archive` | `unarchive`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelOp {
    pub op: String,
    /// `ensure` names the channel by slug (the others may too).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// `rename`/`archive`/`unarchive` name it by id when K2 knows it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `text` | `voice` (`ensure`).
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private: Option<bool>,
    /// Voice: 0–99 (0 = no limit).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_limit: Option<u32>,
}

/// I6 body (1–200 ops).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelsBulkRequest {
    pub space_id: String,
    pub app_id: String,
    pub ops: Vec<ChannelOp>,
}

/// Most ops one I6 call carries.
pub const CHANNELS_BULK_MAX: usize = 200;

/// One answered I6 op (in request order), read leniently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelOpResult {
    #[serde(default)]
    pub op: String,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub channel_id: Option<String>,
    /// The channel's CURRENT name (an `ensure` never renames).
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ItemError>,
}

/// I6 answer, one entry per op.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelsBulkResponse {
    #[serde(default)]
    pub results: Vec<ChannelOpResult>,
}

// ── K2 voice rooms: Sew 0.7.0 (sew-56 v1.1d §2.2, frozen 11-01) ───────
//
// The shared interface, word for word (K2 PRD v1.9 §6.6 2.2). Sew 0.7.0 is
// not released; K2's fake Sew speaks exactly these. Feature flags:
// [`super::features::SERVER_GOVERNED_SPACE`] (SW-1),
// [`super::features::MGMT_VOICE_STATES`] (SW-3),
// [`super::features::VOICE_CHANNEL_CLOSING`] (SW-4),
// [`super::features::VOICE_MEDIA_PATH`] (SW-11).
//
// Sew's own types carry one `app_id: Option<String>` on the member-channels
// sync and `channels/bulk`; this mirror keeps the App-Space types as they
// are (`app_id` required) and adds `Server*` twins with no `app_id` field,
// so a server-Space body can never carry one by mistake.

/// SW-1: K2's service owner (`ExternalId::K2Server`).
pub const K2_SERVER: &str = "k2:server";
/// SW-1: the one server-governed Space's slug, reserved by Sew (any other
/// create naming it answers `422 validation_failed` `slug_reserved`).
pub const ADMIN_SPACE_SLUG: &str = "k5-admin";
/// SW-1 governance (beside [`GOVERNANCE_APP`] / [`GOVERNANCE_SEW`]). Only
/// `spaces/create` sets it; `spaces/{id}/settings` can't switch to or from
/// it (`409 server_governance_fixed`).
pub const GOVERNANCE_SERVER: &str = "server";

/// `POST`: create a Space over mgmt (SW11; SW-1 adds `governance`).
pub const MGMT_SPACES_CREATE: &str = "/mgmt/v1/spaces/create";

/// SW-1 `MgmtSpaceCreateRequest` as K2 sends it for the Admin Space:
/// `{"slug": "k5-admin", "name", "owner": "k2:server", "governance":
/// "server"}`. No `app_id` (sending one answers `409 app_mismatch`), no
/// managers, not hidden. sew-56 v1.2: the same body again (any
/// Idempotency-Key) replays `200` with `created: false` and the Space's id;
/// `409 server_space_exists` (`details.space_id`) is for a create that
/// differs from the Space the install has. K2 takes both as "the Space is
/// there".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSpaceCreateRequest {
    pub slug: String,
    /// 1–100 characters: the server's display name, else its label.
    pub name: String,
    pub owner: ExternalId,
    pub governance: String,
}

impl ServerSpaceCreateRequest {
    /// The K2 Admin Space named `name`.
    pub fn admin(name: &str) -> ServerSpaceCreateRequest {
        ServerSpaceCreateRequest {
            slug: ADMIN_SPACE_SLUG.into(),
            name: name.into(),
            owner: ExternalId::K2Server,
            governance: GOVERNANCE_SERVER.into(),
        }
    }

    /// The rules Sew checks that K2 could break.
    pub fn validate(&self) -> Result<(), String> {
        if self.slug != ADMIN_SPACE_SLUG {
            return Err(format!("slug: a server Space's slug is {ADMIN_SPACE_SLUG}"));
        }
        if self.owner != ExternalId::K2Server {
            return Err(format!("owner: a server Space's owner is {K2_SERVER}"));
        }
        if self.governance != GOVERNANCE_SERVER {
            return Err("governance: only server is accepted on create".into());
        }
        let n = self.name.chars().count();
        if !(1..=100).contains(&n) {
            return Err("name: must be 1–100 characters".into());
        }
        Ok(())
    }
}

/// `MgmtSpaceCreateResponse` as today plus SW-1's `governance`, read
/// leniently.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpaceCreateResponse {
    pub space_id: String,
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub owner_member_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
    #[serde(default)]
    pub created: bool,
    /// SW-1: `server` for the Admin Space (absent on an older Sew).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub governance: Option<String>,
}

/// Levels in a `server` Space (SW-1 §5): `member` and `moderator` only
/// (`admin` answers `422 invalid_state` `level_above_ceiling`).
pub const LEVEL_MEMBER: &str = "member";
pub const LEVEL_MODERATOR: &str = "moderator";
pub const SERVER_LEVELS: [&str; 2] = [LEVEL_MEMBER, LEVEL_MODERATOR];

/// SW-1 single level sync in a `server` Space: I4's body with `app_id`
/// omitted (`POST /mgmt/v1/spaces/{id}/members/{ext}/channels`). The
/// Admin Space has no private channels, so K2 sends `channel_ids: []`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerMemberChannelsRequest {
    pub channel_ids: Vec<String>,
    pub level: String,
}

/// SW-1 bulk level sync in a `server` Space (`POST /mgmt/v1/spaces/{id}/
/// members/channels`, 1–500 items, `app_id` omitted). Answered as
/// [`MemberChannelsBulkResponse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerMemberChannelsBulkRequest {
    pub items: Vec<MemberChannelsItem>,
}

/// SW-1 rooms: I6 `channels/bulk` in a `server` Space with `app_id`
/// omitted. Answered as [`ChannelsBulkResponse`]; per-op errors as I6.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerChannelsBulkRequest {
    pub space_id: String,
    pub ops: Vec<ChannelOp>,
}

/// SW-1 (sew-56 v1.2): `POST /mgmt/v1/channels/create` in a `server` Space
/// with `app_id` omitted (the App-Space twin is [`ChannelCreateRequest`]).
/// K2 makes its rooms with `channels/bulk`; this is here so the fake Sew
/// and any caller speak the same body. `type` is the channel type as in
/// I6 (`voice` for a room; absent = text) [unverified against Sew's A9b
/// field name until 0.7.0's sew-proto is diffed].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerChannelCreateRequest {
    pub space_id: String,
    pub slug: String,
    pub name: String,
    pub private: bool,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub members: Vec<String>,
}

/// I6 op kinds K2 sends for rooms.
pub mod channel_ops {
    pub const ENSURE: &str = "ensure";
    pub const RENAME: &str = "rename";
    pub const ARCHIVE: &str = "archive";
    pub const UNARCHIVE: &str = "unarchive";
}

/// A voice channel (`ChannelOp.kind`). K2 never asks for `voice:video`.
pub const CHANNEL_TYPE_VOICE: &str = "voice";

// SW-3: voice states for the daemon (flag `mgmt_voice_states`).

/// `GET /mgmt/v1/spaces/{space_id}/voice/states`.
pub fn voice_states_path(space_id: &str) -> String {
    format!("/mgmt/v1/spaces/{space_id}/voice/states")
}

/// Longest `wait` the feed takes, seconds (`wait=0..30`).
pub const VOICE_CHANGES_WAIT_MAX_S: u32 = 30;
/// Most changes one feed read returns (`limit=1..500`).
pub const VOICE_CHANGES_LIMIT_MAX: u32 = 500;

/// `GET /mgmt/v1/spaces/{space_id}/voice/changes?after=<cursor>&wait=<s>&limit=<n>`.
/// `wait` and `limit` are clamped to the contract's ranges. The cursor is
/// opaque (`<epoch>.<seq>` today), percent-encoded. The caller's request
/// timeout must be longer than `wait` (not the 10 s / 15 s mgmt default).
/// sew-56 v1.3: `after` is required (start from `voice/states`' `cursor`);
/// a cursor Sew never handed out is `400 validation_failed`; a gone one is
/// `410 gone` `cursor_expired` with `details.head`.
pub fn voice_changes_path(space_id: &str, after: &str, wait_s: u32, limit: u32) -> String {
    format!(
        "/mgmt/v1/spaces/{space_id}/voice/changes?after={}&wait={}&limit={}",
        pct_path(after),
        wait_s.min(VOICE_CHANGES_WAIT_MAX_S),
        limit.clamp(1, VOICE_CHANGES_LIMIT_MAX)
    )
}

/// `MgmtVoiceState`: one person in a voice channel of the Space. Read
/// leniently. `external_id` is Sew's (`k2:owner`, `k2:connect:<uuid>`);
/// K2 maps the uuid to its login.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MgmtVoiceState {
    pub channel_id: String,
    pub member_id: String,
    pub external_id: Option<ExternalId>,
    /// `connecting` | `connected`.
    #[serde(default)]
    pub connection: String,
    #[serde(default)]
    pub self_mute: bool,
    #[serde(default)]
    pub self_deaf: bool,
    #[serde(default)]
    pub server_mute: bool,
    #[serde(default)]
    pub server_deaf: bool,
    #[serde(default)]
    pub joined_at: String,
}

/// `{"states": [MgmtVoiceState…], "cursor": "<opaque>"}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceStatesResponse {
    #[serde(default)]
    pub states: Vec<MgmtVoiceState>,
    pub cursor: String,
}

/// `MgmtVoiceChange.kind` values (read as a string so a newer kind parses).
pub mod voice_change {
    pub const JOINED: &str = "joined";
    pub const UPDATED: &str = "updated";
    pub const LEFT: &str = "left";
    pub const CHANNEL_CLOSING: &str = "channel_closing";
    pub const CHANNEL_ARCHIVED: &str = "channel_archived";
}

/// `MgmtVoiceChange {seq, kind, channel_id, member_id?, external_id?,
/// state?, reason?}`. `reason` is the leave reason (e.g.
/// [`leave_reasons::MEDIA_PATH_DOWN`], [`leave_reasons::CHANNEL_ARCHIVED`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MgmtVoiceChange {
    pub seq: u64,
    pub kind: String,
    pub channel_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<ExternalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<MgmtVoiceState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// `{"changes": [MgmtVoiceChange…], "next": "<cursor>"}`. On `410 gone`
/// `cursor_expired` (`details.head`), re-read `voice/states`.
///
/// sew-56 v1.3, for the follower: **apply changes idempotently** (a change
/// already reflected in `states` may appear again in the feed), and a
/// **move** arrives as `left {reason: moved}` then `joined` (the channel
/// changes; never one `updated`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceChangesResponse {
    #[serde(default)]
    pub changes: Vec<MgmtVoiceChange>,
    pub next: String,
}

/// Leave reasons K2 reads (Sew's `LeaveReason`, snake_case).
pub mod leave_reasons {
    /// Archiving a voice channel ends its calls.
    pub const CHANNEL_ARCHIVED: &str = "channel_archived";
    /// SW-11 (code 28): K2 reported the media path down.
    pub const MEDIA_PATH_DOWN: &str = "media_path_down";
    /// `spaces/members/remove` ends the person's voice in that Space.
    pub const REMOVED_FROM_SPACE: &str = "removed_from_space";
    /// sew-56 v1.3: the `left` half of a move (`left {reason: moved}`, then
    /// `joined` in the new channel).
    pub const MOVED: &str = "moved";
}

// SW-4: closing a voice channel (flag `voice_channel_closing`).

/// `POST /mgmt/v1/channels/{channel_id}/closing`.
pub fn channel_closing_path(channel_id: &str) -> String {
    format!("/mgmt/v1/channels/{channel_id}/closing")
}

/// `{"closing": true|false}` (no `disconnect` flag: archiving already ends
/// calls, so "Close now" is notice → 10 s → archive).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelClosingRequest {
    pub closing: bool,
}

/// `{"closing": bool, "archived": bool}`: an empty room is archived at
/// once (`archived: true`); a live one stays until its last person leaves.
/// sew-56 v1.3: closing an already archived room answers `{closing: true,
/// archived: true}` (a safe retry); a DM or unknown id is `404`; people
/// already in a closing room may still swap devices; the auto-archive's
/// audit actor is the mgmt service (on the call) or `system` (last leave or
/// sweep), reason `closed_empty`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelClosingResponse {
    #[serde(default)]
    pub closing: bool,
    #[serde(default)]
    pub archived: bool,
}

// SW-11: voice unavailable while the media path is down (flag
// `voice_media_path`; idempotent; never through the S27 outbox).

/// `POST /mgmt/v1/voice/media-path`.
pub const MGMT_VOICE_MEDIA_PATH: &str = "/mgmt/v1/voice/media-path";
pub const MEDIA_PATH_UP: &str = "up";
pub const MEDIA_PATH_DOWN: &str = "down";
/// The `reason` K2 sends with `down` (M11).
pub const MEDIA_PATH_REASON_VOICE_LANE_DOWN: &str = "voice_lane_down";
/// Longest `reason`, characters.
pub const MEDIA_PATH_REASON_MAX_CHARS: usize = 64;

/// `{"state": "up"|"down", "reason"?: string}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaPathRequest {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl MediaPathRequest {
    pub fn up() -> MediaPathRequest {
        MediaPathRequest { state: MEDIA_PATH_UP.into(), reason: None }
    }
    pub fn down(reason: &str) -> MediaPathRequest {
        MediaPathRequest { state: MEDIA_PATH_DOWN.into(), reason: Some(reason.into()) }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.state != MEDIA_PATH_UP && self.state != MEDIA_PATH_DOWN {
            return Err("state: up or down".into());
        }
        if self.reason.as_ref().is_some_and(|r| r.chars().count() > MEDIA_PATH_REASON_MAX_CHARS) {
            return Err("reason: at most 64 characters".into());
        }
        Ok(())
    }
}

/// `{"state", "since", "ended_calls"}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaPathResponse {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub since: String,
    #[serde(default)]
    pub ended_calls: u64,
}

/// `details.engine` while the path is down (beside
/// `down|draining|syncing|error`). sew-56 v1.3: joins and moves (DM call
/// joins too) are refused `503` with `details.reason = voice_unavailable`
/// ([`reasons::VOICE_UNAVAILABLE`]), `details.engine = media_path_down` and
/// K2's text in `details.media_path_reason`. Branch on `details.reason`,
/// not the error code.
pub const ENGINE_MEDIA_PATH_DOWN: &str = "media_path_down";

/// sew-56 v1.3 SW-11: what a join or move answers while the media path is
/// down: `Some(media_path_reason)` (K2's text, maybe empty) when `status`
/// and `err` are that refusal.
pub fn media_path_refusal(status: u16, err: &ApiError) -> Option<String> {
    (status == 503
        && err.detail_str("reason") == Some(reasons::VOICE_UNAVAILABLE)
        && err.detail_str("engine") == Some(ENGINE_MEDIA_PATH_DOWN))
    .then(|| err.detail_str("media_path_reason").unwrap_or_default().to_string())
}

// ── errors (`sew_proto::error`) ──────────────────────────────────────

/// `{"ok": false, "error": {code, hint, details}}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ErrorBody {
    pub ok: bool,
    pub error: ApiError,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub hint: String,
    #[serde(default)]
    pub details: Map<String, Value>,
}

impl ApiError {
    pub fn detail_str(&self, key: &str) -> Option<&str> {
        self.details.get(key).and_then(Value::as_str)
    }
}

/// Stable error codes K2 branches on (`sew_proto::error::Code`).
pub mod codes {
    pub const BAD_REQUEST: &str = "bad_request";
    pub const VALIDATION_FAILED: &str = "validation_failed";
    pub const UNAUTHENTICATED: &str = "unauthenticated";
    pub const NOT_FOUND: &str = "not_found";
    pub const CONFLICT: &str = "conflict";
    pub const IDEMPOTENCY_CONFLICT: &str = "idempotency_conflict";
    pub const INVALID_STATE: &str = "invalid_state";
    pub const RATE_LIMITED: &str = "rate_limited";
    pub const INTERNAL: &str = "internal";
    pub const UNAVAILABLE: &str = "unavailable";
    /// 403 (sew-56 v1.2: SW-1's client-route refusals carry
    /// `details.reason` `app_session_not_allowed` / `app_user_not_allowed`;
    /// Sew has no `forbidden` code).
    pub const MISSING_PERMISSION: &str = "missing_permission";
    /// 410 (SW-3 `cursor_expired`).
    pub const GONE: &str = "gone";
}

/// `details.reason` values K2 branches on (`sew_proto::mgmt::reasons`).
pub mod reasons {
    pub const USERNAME_TAKEN: &str = "username_taken";
    pub const IS_OWNER: &str = "is_owner";
    /// 422: the assistant's person is not provisioned yet. K2 retries ≤ 1 h.
    pub const ASSISTANT_PERSON_UNKNOWN: &str = "assistant_person_unknown";
    /// 422: the assistant's person is deactivated. K2 stops (blocked).
    pub const ASSISTANT_PERSON_INACTIVE: &str = "assistant_person_inactive";
    pub const PERSON_IS_GUEST: &str = "person_is_guest";
    pub const PERSON_HAS_ASSISTANTS: &str = "person_has_assistants";
    /// 422 `invalid_state` (Sew 0.5.0): a write to a Space unbound from its
    /// App (an orphan). Reads and export still work.
    pub const SPACE_ORPHANED: &str = "space_orphaned";
    /// 409 (Sew 0.5.0): a bind to another App while the Space is an orphan
    /// of one; `spaces/release` first.
    pub const SPACE_BOUND_ELSEWHERE: &str = "space_bound_elsewhere";
    // App-governed Spaces (Sew 0.6.0, sew-55 v2.4 §10).
    /// 409 on the switch to `app`: box users other than the owner are in
    /// the Space (`details.external_ids`); K2 removes them, then switches.
    pub const BOX_USERS_PRESENT: &str = "box_users_present";
    /// 422: a box user named in a sync item (K2 never sends one).
    pub const BOX_USER_NOT_ALLOWED: &str = "box_user_not_allowed";
    /// 409: a sync or channel op in a Space that is still Sew-governed.
    pub const NOT_APP_GOVERNED: &str = "not_app_governed";
    /// 409: the switch to `app` without a governing link.
    pub const NO_GOVERNING_APP: &str = "no_governing_app";
    /// 422: a level above the App's `max_level`.
    pub const LEVEL_ABOVE_CEILING: &str = "level_above_ceiling";
    /// 422: an agent at a level other than `member`.
    pub const AGENT_LEVEL: &str = "agent_level";
    /// 422: a sync item for a person Sew has no member for. K2 provisions
    /// them again, then syncs again (never a member made by the sync).
    pub const MEMBER_NOT_PROVISIONED: &str = "member_not_provisioned";
    /// 422: a structure edit in an App-governed Space.
    pub const GOVERNED_BY_APP: &str = "governed_by_app";
    /// 409: an owner naming another App's service owner. Sew 0.7.0 SW-1
    /// (sew-56 v1.2): also an `app_id` sent on a server create.
    pub const APP_MISMATCH: &str = "app_mismatch";
    /// 422: a DM to an App's service owner.
    pub const SERVICE_MEMBER: &str = "service_member";
    /// 403 `missing_permission` (Sew 0.6.0 A23, flag
    /// `home_client_owner_approval`): a home-client write without the
    /// install owner's approval, or with one Sew did not accept.
    pub const OWNER_APPROVAL_REQUIRED: &str = "owner_approval_required";
    /// 409 `conflict` (Sew 0.6.0 S55): the home client never governs a
    /// Space, so an App that governs one can't become it.
    pub const HOME_CLIENT_CANNOT_GOVERN: &str = "home_client_cannot_govern";
    /// 409 `conflict` (Sew 0.6.0): `channels/create`, or an I6 `ensure`, on a
    /// slug that holds a channel of the other type (`details.existing_type`);
    /// nothing written.
    pub const CHANNEL_TYPE_MISMATCH: &str = "channel_type_mismatch";
    // K2 voice rooms (Sew 0.7.0, sew-56 v1.1d §2.2).
    /// SW-1 409 `conflict`: the install already has its server Space;
    /// `details.space_id` names it (K2 reads it back).
    pub const SERVER_SPACE_EXISTS: &str = "server_space_exists";
    /// SW-1 409 `conflict`: a non-server Space already holds slug
    /// `k5-admin`. K2 reports it (blocked).
    pub const SLUG_TAKEN: &str = "slug_taken";
    /// SW-1 409 `conflict`: a standalone install (no K2 identity).
    pub const IDENTITY_NOT_K2: &str = "identity_not_k2";
    /// SW-1 400 `validation_failed` (sew-56 v1.2; Sew's validation_failed
    /// is always 400): `k5-admin` on any other Space's create.
    pub const SLUG_RESERVED: &str = "slug_reserved";
    /// SW-1 422 `invalid_state` (mgmt) / 403 `missing_permission` (client
    /// routes, sew-56 v1.2): a `k2:principal:` or `app:` id in a `server`
    /// Space. (`app_id` sent on a server create is 409 [`APP_MISMATCH`].)
    pub const APP_USER_NOT_ALLOWED: &str = "app_user_not_allowed";
    /// SW-1 403 `missing_permission` (sew-56 v1.2): any session with an
    /// `app` claim (Hearth too).
    pub const APP_SESSION_NOT_ALLOWED: &str = "app_session_not_allowed";
    /// SW-1 422 `invalid_state`: a client structure edit in a `server` Space.
    pub const GOVERNED_BY_SERVER: &str = "governed_by_server";
    /// SW-1 409 `conflict`: `spaces/{id}/settings` to or from `server`.
    pub const SERVER_GOVERNANCE_FIXED: &str = "server_governance_fixed";
    /// SW-1 401 (logs only): a pass for `k2:server`.
    pub const SERVICE_SUB_REFUSED: &str = "service_sub_refused";
    /// SW-3 410 `gone`: the feed cursor is gone (`details.head`); re-read
    /// `voice/states`.
    pub const CURSOR_EXPIRED: &str = "cursor_expired";
    /// SW-4 409 `conflict`: un-closing an archived room.
    pub const CHANNEL_ARCHIVED: &str = "channel_archived";
    /// SW-4 422 `invalid_state`: `closing` on a channel that isn't voice.
    pub const NOT_VOICE_CHANNEL: &str = "not_voice_channel";
    /// SW-4 409 `conflict`: a join or move into a closing room.
    pub const CHANNEL_CLOSING: &str = "channel_closing";
    /// SW-11: `details.engine` of the `503` join refusal (and the leave
    /// reason) while the media path is down.
    pub const MEDIA_PATH_DOWN: &str = "media_path_down";
    /// SW-11 (sew-56 v1.3): `details.reason` of the `503` join/move refusal
    /// while voice is unavailable ([`super::media_path_refusal`]).
    pub const VOICE_UNAVAILABLE: &str = "voice_unavailable";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_id_round_trips_and_refuses_bad_forms() {
        let u = "0f8fad5b-d9cb-469f-a165-70867728950e";
        for raw in [
            format!("k2:principal:{u}"),
            format!("k2:workspace:{u}"),
            format!("k2:connect:{u}"),
            "k2:owner".to_string(),
            "local:alice".to_string(),
        ] {
            let id = ExternalId::parse(&raw).expect("parses");
            assert_eq!(id.to_string(), raw);
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, format!("\"{raw}\""));
            assert_eq!(serde_json::from_str::<ExternalId>(&json).unwrap(), id);
        }
        assert_eq!(
            ExternalId::parse(&format!("k2:connect:{u}")).unwrap(),
            ExternalId::K2Connect(u.into())
        );
        for bad in [
            "k2:principal:p1",
            "k2:principal:0F8FAD5B-D9CB-469F-A165-70867728950E",
            "k2:user:alice",
            "k2:connect:alice",
            "local:A",
            "",
        ] {
            assert!(ExternalId::parse(bad).is_err(), "{bad:?} must be refused");
        }
        // Lenient on the way IN (a newer Sew's list must still parse).
        let other: ExternalId = serde_json::from_str("\"link:4242:AB3DEFGHIJKLMNOP\"").unwrap();
        assert!(!other.is_known());
        assert_eq!(other.to_string(), "link:4242:AB3DEFGHIJKLMNOP");
    }

    #[test]
    fn patch_serializes_keep_absent_clear_null_set_value() {
        let p = ProfilePatch {
            display_name: Some("Ann".into()),
            email: Patch::Clear,
            ..Default::default()
        };
        let v: Value = serde_json::to_value(&p).unwrap();
        assert_eq!(v, serde_json::json!({"display_name":"Ann","email":null}));
        let back: ProfilePatch = serde_json::from_value(v).unwrap();
        assert_eq!(back.email, Patch::Clear);
        assert_eq!(back.title, Patch::Keep);
    }

    #[test]
    fn provision_request_denies_unknown_fields_like_sew_proto() {
        let raw = r#"{"external_id":"k2:owner","kind":"person","guest":false,
            "profile":{"display_name":"Owner","username":"owner"},"extra":1}"#;
        assert!(serde_json::from_str::<ProvisionRequest>(raw).is_err());
    }

    #[test]
    fn grammar_helpers_match_sew_proto() {
        assert!(is_valid_handle("ab"));
        assert!(is_valid_handle(&"a".repeat(64)));
        assert!(!is_valid_handle("a"));
        assert!(!is_valid_handle(&"a".repeat(65)));
        assert!(!is_valid_handle("a.b"));
        assert!(is_sew_actor("user:alice@x.y-z_1"));
        assert!(!is_sew_actor("user alice"));
        assert!(!is_sew_actor(&"a".repeat(129)));
        assert!(is_idempotency_key(&format!("k2ob_{}", "a".repeat(32))));
        assert!(!is_idempotency_key("k2ob:x"));
    }

    #[test]
    fn mgmt_principal_profile_fields_are_read_leniently() {
        let base = r#""member_id":"7","external_id":"k2:owner","kind":"person",
            "install_role":"owner","username":"owner","display_name":"Owner",
            "deactivated":false,"created_at":"2026-10-15T00:00:00Z""#;
        // Pre-SW14 Sew: fields absent → unknown.
        let old: MgmtPrincipal = serde_json::from_str(&format!("{{{base}}}")).unwrap();
        assert_eq!((old.email, old.title, old.pronouns), (None, None, None));
        // SW14: null = none, a value = the value.
        let new: MgmtPrincipal = serde_json::from_str(&format!(
            r#"{{{base},"email":null,"title":"CTO","pronouns":"she/her"}}"#
        ))
        .unwrap();
        assert_eq!(new.email, Some(None));
        assert_eq!(new.title, Some(Some("CTO".into())));
        assert_eq!(new.pronouns, Some(Some("she/her".into())));
        assert_eq!(
            principal_lookup_path(&ExternalId::K2Owner),
            "/mgmt/v1/principals?external_id=k2:owner&limit=1"
        );
    }

    #[test]
    fn hc2_skipped_is_read_from_member_add_outcomes_only() {
        let o =
            AddMembersOutcome::parse(br#"{"added":["1"],"already":[],"skipped":["9"]}"#).unwrap();
        assert!(o.skipped_member("9"));
        assert!(!o.skipped_member("1"));
        // A Sew without HC2: no `skipped` → nobody skipped.
        let pre = AddMembersOutcome::parse(br#"{"added":["1"],"already":["2"]}"#).unwrap();
        assert!(pre.skipped.is_empty());
    }

    #[test]
    fn mgmt_principal_unknown_role_does_not_fail_the_page() {
        let raw = r#"{"items":[{"member_id":"7","external_id":"k2:owner","kind":"person",
            "install_role":"superuser","username":"owner","display_name":"Owner",
            "deactivated":false,"created_at":"2026-10-15T00:00:00Z"}],"has_more":false}"#;
        let page: MgmtPrincipalPage = serde_json::from_str(raw).unwrap();
        assert_eq!(page.items[0].install_role, InstallRole::Unknown);
    }

    /// Sew 0.6.0's exact I4 bulk answer (`sew-proto` `mgmt/member_channels.rs`,
    /// as `serde_json` writes `MgmtMemberChannelsBulkResponse`): each item is
    /// `{external_id, status, answer}` or `{external_id, status, error}`; the
    /// answer is nested (K2 read it flattened before 0.46.7's S8, so every
    /// answer read as an empty success).
    #[test]
    fn i4_bulk_items_nest_the_answer_or_carry_the_error() {
        let a = "k2:principal:0f8fad5b-d9cb-469f-a165-70867728950e";
        let b = "k2:principal:1f8fad5b-d9cb-469f-a165-70867728950e";
        let c = "k2:principal:2f8fad5b-d9cb-469f-a165-70867728950e";
        let raw = format!(
            r#"{{"items":[
            {{"external_id":"{a}","status":200,"answer":{{"external_id":"{a}","member_id":"41","added":["301"],"removed":[],"unchanged":["302"],"level":"member","unknown_channel_ids":["299"],"queued":false}}}},
            {{"external_id":"{b}","status":200,"answer":{{"external_id":"{b}","member_id":"42","added":[],"removed":[],"unchanged":[],"unknown_channel_ids":[],"skipped":"quota_exceeded","queued":false}}}},
            {{"external_id":"{c}","status":422,"error":{{"code":"invalid_state","hint":"no provisioned member has this external id: provision it first","details":{{"reason":"member_not_provisioned","field":"external_id"}}}}}}
            ]}}"#
        );
        let r: MemberChannelsBulkResponse = serde_json::from_str(&raw).expect("Sew's bulk answer");
        assert_eq!(r.items.len(), 3);
        let one = r.items[0].answer.as_ref().expect("an answer");
        assert_eq!((r.items[0].status, r.items[0].error.is_none()), (200, true));
        assert_eq!((one.member_id.as_deref(), one.level.as_deref()), (Some("41"), Some("member")));
        assert_eq!(one.unknown_channel_ids, vec!["299".to_string()], "a public or unknown id, the rest applied");
        assert_eq!(one.added, serde_json::json!(["301"]));
        let two = r.items[1].answer.as_ref().expect("an answer");
        assert_eq!((two.skipped.as_deref(), two.level.as_deref()), (Some(skipped::QUOTA_EXCEEDED), None), "skipped: no level");
        let three = &r.items[2];
        assert!(three.answer.is_none());
        let e = three.error.as_ref().expect("the item's own error");
        assert_eq!((three.status, e.code.as_str(), e.reason()), (422, codes::INVALID_STATE, reasons::MEMBER_NOT_PROVISIONED));
        assert_eq!(e.details.get("field").and_then(Value::as_str), Some("external_id"));
        // A queued item: empty lists, job_id names the job.
        let q: MemberChannelsResponse = serde_json::from_str(&format!(
            r#"{{"external_id":"{a}","member_id":"41","added":[],"removed":[],"unchanged":[],"level":"member","unknown_channel_ids":[],"queued":true,"job_id":"9001"}}"#
        ))
        .expect("a queued answer");
        assert_eq!((q.queued, q.job_id.as_deref()), (true, Some("9001")));
        // What K2 stores as a row's result round-trips (skipped fields stay out).
        let back: MemberChannelsResponse = serde_json::from_str(&serde_json::to_string(two).unwrap()).unwrap();
        assert_eq!(&back, two);
        assert!(!serde_json::to_string(two).unwrap().contains("\"level\""));
    }

    /// SW-1 (sew-56 v1.1d §2.2): the create body word for word, `k2:server`
    /// as an id, and the server-Space twins carry no `app_id`.
    #[test]
    fn sw1_server_space_wire_is_word_for_word() {
        let req = ServerSpaceCreateRequest::admin("sewlab");
        req.validate().expect("valid");
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            serde_json::json!({"slug": "k5-admin", "name": "sewlab", "owner": "k2:server", "governance": "server"})
        );
        assert!(
            serde_json::from_str::<ServerSpaceCreateRequest>(
                r#"{"slug":"k5-admin","name":"x","owner":"k2:server","governance":"server","app_id":"a"}"#
            )
            .is_err(),
            "an app_id never rides a server create"
        );
        let mut bad = req.clone();
        bad.owner = ExternalId::K2Owner;
        assert!(bad.validate().is_err());
        let mut bad = req.clone();
        bad.slug = "hearth".into();
        assert!(bad.validate().is_err());
        let mut bad = req;
        bad.name = String::new();
        assert!(bad.validate().is_err());

        let id = ExternalId::parse("k2:server").unwrap();
        assert_eq!(id, ExternalId::K2Server);
        assert_eq!(id.to_string(), "k2:server");
        assert!(id.is_service_owner() && !id.is_box_user() && !id.is_agent());
        assert!(ExternalId::parse("k2:server:x").is_err());
        assert!(ExternalId::K2Principal("0f8fad5b-d9cb-469f-a165-70867728950e".into()).is_app_user_or_app());
        assert!(!ExternalId::K2Owner.is_app_user_or_app());

        // The answer as today plus `governance`; older Sew: absent.
        let r: SpaceCreateResponse = serde_json::from_str(
            r#"{"space_id":"77","slug":"k5-admin","name":"sewlab","owner_member_id":"5","created":true,"governance":"server","manager_member_ids":[]}"#,
        )
        .unwrap();
        assert_eq!((r.space_id.as_str(), r.governance.as_deref(), r.created), ("77", Some("server"), true));
        // sew-56 v1.2: the same body replays with created: false.
        let replay: SpaceCreateResponse = serde_json::from_str(
            r#"{"space_id":"77","slug":"k5-admin","name":"sewlab","owner_member_id":"5","created":false,"governance":"server"}"#,
        )
        .unwrap();
        assert_eq!((replay.space_id.as_str(), replay.created), ("77", false));
        // sew-56 v1.2 codes: no `forbidden` (403 is missing_permission).
        assert_eq!(codes::MISSING_PERMISSION, "missing_permission");
        let create = ServerChannelCreateRequest {
            space_id: "77".into(),
            slug: "room-1".into(),
            name: "Room 1".into(),
            private: false,
            kind: Some(CHANNEL_TYPE_VOICE.into()),
            members: vec![],
        };
        assert_eq!(
            serde_json::to_value(&create).unwrap(),
            serde_json::json!({"space_id": "77", "slug": "room-1", "name": "Room 1", "private": false, "type": "voice"})
        );

        let lv = ServerMemberChannelsRequest { channel_ids: vec![], level: LEVEL_MODERATOR.into() };
        assert_eq!(serde_json::to_value(&lv).unwrap(), serde_json::json!({"channel_ids": [], "level": "moderator"}));
        assert!(serde_json::from_str::<ServerMemberChannelsRequest>(r#"{"app_id":"a","channel_ids":[],"level":"member"}"#).is_err());
        let bulk = ServerMemberChannelsBulkRequest {
            items: vec![MemberChannelsItem { external_id: ExternalId::K2Owner, channel_ids: vec![], level: LEVEL_MODERATOR.into() }],
        };
        assert_eq!(
            serde_json::to_value(&bulk).unwrap(),
            serde_json::json!({"items": [{"external_id": "k2:owner", "channel_ids": [], "level": "moderator"}]})
        );
        let rooms = ServerChannelsBulkRequest {
            space_id: "77".into(),
            ops: vec![ChannelOp {
                op: channel_ops::ENSURE.into(),
                slug: Some("room-1".into()),
                channel_id: None,
                name: Some("Room 1".into()),
                kind: Some(CHANNEL_TYPE_VOICE.into()),
                private: Some(false),
                user_limit: Some(25),
            }],
        };
        assert_eq!(
            serde_json::to_value(&rooms).unwrap(),
            serde_json::json!({"space_id": "77", "ops": [{"op": "ensure", "slug": "room-1", "name": "Room 1", "type": "voice", "private": false, "user_limit": 25}]})
        );
        assert_eq!(SERVER_LEVELS, ["member", "moderator"]);
    }

    /// SW-3: the feed path, the snapshot and a change, as sew-56 names them.
    #[test]
    fn sw3_voice_states_and_changes_parse() {
        assert_eq!(voice_states_path("77"), "/mgmt/v1/spaces/77/voice/states");
        assert_eq!(
            voice_changes_path("77", "3.41", 99, 0),
            "/mgmt/v1/spaces/77/voice/changes?after=3.41&wait=30&limit=1",
            "wait ≤ 30, limit ≥ 1"
        );
        assert_eq!(voice_changes_path("77", "", 20, 900), "/mgmt/v1/spaces/77/voice/changes?after=&wait=20&limit=500");
        let u = "0f8fad5b-d9cb-469f-a165-70867728950e";
        let st: VoiceStatesResponse = serde_json::from_str(&format!(
            r#"{{"states":[{{"channel_id":"901","member_id":"5","external_id":"k2:connect:{u}","connection":"connected",
               "self_mute":true,"self_deaf":false,"server_mute":false,"server_deaf":false,"joined_at":"2026-11-01T00:00:00Z"}}],
               "cursor":"1.0"}}"#
        ))
        .unwrap();
        assert_eq!(st.cursor, "1.0");
        assert_eq!(st.states[0].external_id, Some(ExternalId::K2Connect(u.into())));
        assert!(st.states[0].self_mute && st.states[0].connection == "connected");
        let ch: VoiceChangesResponse = serde_json::from_str(
            r#"{"changes":[
                {"seq":42,"kind":"left","channel_id":"901","member_id":"5","external_id":"k2:owner","reason":"media_path_down"},
                {"seq":43,"kind":"channel_closing","channel_id":"902"}],
               "next":"1.43"}"#,
        )
        .unwrap();
        assert_eq!(ch.next, "1.43");
        assert_eq!(
            (ch.changes[0].kind.as_str(), ch.changes[0].reason.as_deref()),
            (voice_change::LEFT, Some(leave_reasons::MEDIA_PATH_DOWN))
        );
        assert_eq!((ch.changes[1].kind.as_str(), ch.changes[1].member_id.as_deref()), (voice_change::CHANNEL_CLOSING, None));
    }

    /// SW-4 and SW-11 bodies and answers; the status's `voice.media_path`.
    #[test]
    fn sw4_closing_and_sw11_media_path_wire() {
        assert_eq!(channel_closing_path("901"), "/mgmt/v1/channels/901/closing");
        assert_eq!(serde_json::to_value(ChannelClosingRequest { closing: true }).unwrap(), serde_json::json!({"closing": true}));
        assert!(
            serde_json::from_str::<ChannelClosingRequest>(r#"{"closing":true,"disconnect":true}"#).is_err(),
            "no disconnect flag"
        );
        let r: ChannelClosingResponse = serde_json::from_str(r#"{"closing":true,"archived":true}"#).unwrap();
        assert!(r.closing && r.archived);

        let down = MediaPathRequest::down(MEDIA_PATH_REASON_VOICE_LANE_DOWN);
        down.validate().unwrap();
        assert_eq!(serde_json::to_value(&down).unwrap(), serde_json::json!({"state": "down", "reason": "voice_lane_down"}));
        assert_eq!(serde_json::to_value(MediaPathRequest::up()).unwrap(), serde_json::json!({"state": "up"}));
        assert!(MediaPathRequest { state: "sideways".into(), reason: None }.validate().is_err());
        assert!(MediaPathRequest::down(&"x".repeat(65)).validate().is_err());
        let a: MediaPathResponse =
            serde_json::from_str(r#"{"state":"down","since":"2026-11-01T00:00:00Z","ended_calls":2}"#).unwrap();
        assert_eq!((a.state.as_str(), a.ended_calls), ("down", 2));

        let st: Status = serde_json::from_str(
            r#"{"version":"0.7.0","install_id":"1","provider":"k2","features":[],
               "voice":{"media_path":{"state":"down","since":"2026-11-01T00:00:00Z","media_path_reason":"voice_lane_down"},"engine":"up"}}"#,
        )
        .unwrap();
        let mp = st.voice.and_then(|v| v.media_path).expect("media_path");
        assert_eq!((mp.state.as_str(), mp.media_path_reason.as_deref()), ("down", Some("voice_lane_down")));
        let old: Status = serde_json::from_str(r#"{"install_id":"1","provider":"k2"}"#).unwrap();
        assert!(old.voice.is_none(), "an older Sew has no voice block");
        // sew-56 v1.3: the join refusal while the path is down.
        let e: ErrorBody = serde_json::from_str(
            r#"{"ok":false,"error":{"code":"unavailable","hint":"voice is unavailable",
               "details":{"reason":"voice_unavailable","engine":"media_path_down","media_path_reason":"voice_lane_down"}}}"#,
        )
        .unwrap();
        assert_eq!(media_path_refusal(503, &e.error).as_deref(), Some("voice_lane_down"));
        assert_eq!(media_path_refusal(409, &e.error), None);
        let other: ErrorBody = serde_json::from_str(
            r#"{"ok":false,"error":{"code":"unavailable","hint":"x","details":{"reason":"voice_unavailable","engine":"draining"}}}"#,
        )
        .unwrap();
        assert_eq!(media_path_refusal(503, &other.error), None, "another engine state");
        assert_eq!(leave_reasons::MOVED, "moved");
    }
}
