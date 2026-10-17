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
/// `k2:workspace:<uuid>`, `k2:owner`, `local:<handle>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ExternalId {
    K2Principal(String),
    K2Workspace(String),
    K2Owner,
    Local(String),
    /// Not in sew-proto: a form this mirror does not know yet (e.g. SW2's
    /// `k2:connect:<uuid>`, or a Meet link guest `link:<channel>:<id>`,
    /// K-M6). Only produced by lenient deserialization, so a newer Sew's
    /// member list still parses; reconcile ignores it. **Keep this lenient
    /// arm when K1 swaps in the vendored sew-proto**, whose `ExternalId`
    /// refuses unknown forms.
    Other(String),
}

impl ExternalId {
    pub fn parse(s: &str) -> Result<ExternalId, String> {
        let bad = || {
            format!(
                "invalid external id {s:?}: expected k2:principal:<uuid>, k2:workspace:<uuid>, k2:owner or local:<handle>"
            )
        };
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
}

impl fmt::Display for ExternalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExternalId::K2Principal(u) => write!(f, "k2:principal:{u}"),
            ExternalId::K2Workspace(u) => write!(f, "k2:workspace:{u}"),
            ExternalId::K2Owner => f.write_str("k2:owner"),
            ExternalId::Local(h) => write!(f, "local:{h}"),
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
/// (S27-A12). K2 sends exactly one of the two fields
/// ([`crate::sew_revoke::SewSessionRevoke::payload`]).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RevokeSessionsRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_id: Option<ExternalId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub k2_sid: Option<String>,
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
            "k2:owner".to_string(),
            "local:alice".to_string(),
        ] {
            let id = ExternalId::parse(&raw).expect("parses");
            assert_eq!(id.to_string(), raw);
            let json = serde_json::to_string(&id).unwrap();
            assert_eq!(json, format!("\"{raw}\""));
            assert_eq!(serde_json::from_str::<ExternalId>(&json).unwrap(), id);
        }
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
        let other: ExternalId =
            serde_json::from_str("\"k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e\"").unwrap();
        assert!(!other.is_known());
        assert_eq!(
            other.to_string(),
            "k2:connect:0f8fad5b-d9cb-469f-a165-70867728950e"
        );
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
