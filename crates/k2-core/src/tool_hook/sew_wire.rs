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
                "invalid external id {s:?}: expected k2:principal:<uuid>, k2:workspace:<uuid>, k2:connect:<uuid>, k2:owner, app:<app id> or local:<handle>"
            )
        };
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
}

impl fmt::Display for ExternalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExternalId::K2Principal(u) => write!(f, "k2:principal:{u}"),
            ExternalId::K2Workspace(u) => write!(f, "k2:workspace:{u}"),
            ExternalId::K2Connect(u) => write!(f, "k2:connect:{u}"),
            ExternalId::K2Owner => f.write_str("k2:owner"),
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

/// I3 body: only `governance` (K2 never sends the other PRIV1 fields).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpaceSettingsRequest {
    /// [`GOVERNANCE_APP`] | [`GOVERNANCE_SEW`].
    pub governance: String,
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

/// `skipped` values of an I4 answer.
pub mod skipped {
    /// Banned in that Space: bans win; K2 holds the item.
    pub const BANNED: &str = "banned";
    /// Deprovisioned: a sync never reactivates (R8); K2 holds the item.
    pub const DEPROVISIONED: &str = "deprovisioned";
    /// The Space owner (the App's service owner): a sync never changes it.
    pub const OWNER: &str = "owner";
}

/// I4 answer for one person, read leniently:
/// `{added, removed, unchanged, level, unknown_channel_ids, skipped}`, plus
/// `queued: true` when the item ran as a Sew job (over
/// [`MEMBER_CHANNELS_JOB_THRESHOLD`] channel changes).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberChannelsResponse {
    #[serde(default)]
    pub added: Value,
    #[serde(default)]
    pub removed: Value,
    #[serde(default)]
    pub unchanged: Value,
    #[serde(default)]
    pub level: Option<String>,
    #[serde(default)]
    pub unknown_channel_ids: Vec<String>,
    #[serde(default)]
    pub skipped: Option<String>,
    #[serde(default)]
    pub queued: bool,
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

/// One answered I4 bulk item: the result, or that item's own error (with
/// its status: 409, 422 …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberChannelsBulkItem {
    pub external_id: ExternalId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ItemError>,
    #[serde(flatten)]
    pub result: MemberChannelsResponse,
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
    /// 409: an owner naming another App's service owner.
    pub const APP_MISMATCH: &str = "app_mismatch";
    /// 422: a DM to an App's service owner.
    pub const SERVICE_MEMBER: &str = "service_member";
    /// 409: I6 `ensure` of a slug that exists with another type.
    pub const CHANNEL_TYPE_MISMATCH: &str = "channel_type_mismatch";
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
}
