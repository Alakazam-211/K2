//! Wave 0 foundation: **caller-asserted workspace must never be identity**.
//!
//! Privileged agent surfaces (mail, DNS, grants) resolve *who the caller is*
//! from a validated [`HookPrincipal`] (minted at spawn / scoped-session
//! passport), never from the client's `project=` / `K2_PROJECT_PATH` claim.
//!
//! Body/query `project=` may still name a *target resource* (e.g. msg
//! recipient, access-grant grantee) when the principal is allowed to act
//! on it — that is a separate check. This helper is only for **self-identity**.
//!
//! # Integration (DNS / mail / future surfaces)
//!
//! ```ignore
//! use crate::caller_workspace::{resolve_caller_workspace, CallerWorkspace};
//! use crate::session_token::HookPrincipal;
//!
//! // Scoped session (cell UDS or validated hook token):
//! let ws = resolve_caller_workspace(Some(&principal), claim /* ignored */)?;
//! // Owner/app path (no principal): claim is required identity:
//! let ws = resolve_caller_workspace(None, Some(project_claim))?;
//! // Use ws.workspace_uuid for grants / catalog; ws.path for FS jail.
//! ```
//!
//! ## Residual risk (owner token)
//!
//! Until Phase 2 deprecates ambient owner tokens on agent-looking paths,
//! a request authenticated only with the daemon **owner** token and a
//! client-supplied `project=` still resolves identity from that claim.
//! Scoped principals (this helper's primary path) are closed. See module
//! tests + mail regression tests.

use std::cell::RefCell;

use crate::session_token::HookPrincipal;

/// Resolved caller workspace for identity-sensitive surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallerWorkspace {
    /// Canonical on-disk path of the workspace.
    pub path: String,
    /// `projects.id` — the stable workspace UUID used by grants / mail rows.
    pub workspace_uuid: String,
}

/// Why identity resolution failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveCallerError {
    /// No principal and no usable project claim.
    MissingClaim,
    /// Claim/principal did not match a registered workspace.
    NotFound(String),
    /// Path resolved but has no `projects` row (should be rare).
    Unregistered(String),
    /// Principal present but `workspace_uuid` empty / unusable.
    UnresolvablePrincipal,
}

impl ResolveCallerError {
    pub fn hint(&self) -> String {
        match self {
            Self::MissingClaim => {
                "missing 'project' (workspace name | path | UUID) — or present a scoped session token"
                    .to_string()
            }
            Self::NotFound(q) => format!("workspace not found: {q}"),
            Self::Unregistered(path) => format!("workspace not registered: {path}"),
            Self::UnresolvablePrincipal => {
                "scoped session principal has no resolvable workspace".to_string()
            }
        }
    }
}

/// Resolve the caller's workspace for **self-identity**.
///
/// * When `principal` is `Some`, identity is **always**
///   `principal.workspace_uuid` (minted at spawn). `project_claim` is
///   ignored for identity (CLI may still send `K2_PROJECT_PATH`; it must
///   not escalate).
/// * When `principal` is `None` (owner / connect-user / legacy agent with
///   ambient owner token), resolve `project_claim` via name | path | uuid
///   — the pre-Wave-0 behavior. **Residual spoof risk** if the credential
///   is the shared owner token.
pub fn resolve_caller_workspace(
    principal: Option<&HookPrincipal>,
    project_claim: Option<&str>,
) -> Result<CallerWorkspace, ResolveCallerError> {
    if let Some(p) = principal {
        return resolve_from_principal(p);
    }
    let claim = project_claim.map(str::trim).filter(|s| !s.is_empty());
    let Some(claim) = claim else {
        return Err(ResolveCallerError::MissingClaim);
    };
    resolve_from_claim(claim)
}

fn resolve_from_principal(principal: &HookPrincipal) -> Result<CallerWorkspace, ResolveCallerError> {
    let uuid = principal.workspace_uuid.trim();
    if uuid.is_empty() {
        return Err(ResolveCallerError::UnresolvablePrincipal);
    }
    // Path from the mint-time workspace uuid — never from client claim.
    let Some(path) = crate::workspace_msg::resolve_workspace(uuid) else {
        return Err(ResolveCallerError::UnresolvablePrincipal);
    };
    // Defense in depth: path → id must round-trip to the principal's uuid.
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    match project_id {
        Some(id) if id == uuid => Ok(CallerWorkspace {
            path,
            workspace_uuid: id,
        }),
        Some(_) => {
            // DB drift / collision: trust the principal uuid only if the
            // row is still addressable by uuid (resolve_workspace already
            // keyed on id). Prefer principal uuid as identity.
            Ok(CallerWorkspace {
                path,
                workspace_uuid: uuid.to_string(),
            })
        }
        None => Err(ResolveCallerError::Unregistered(path)),
    }
}

fn resolve_from_claim(claim: &str) -> Result<CallerWorkspace, ResolveCallerError> {
    let Some(path) = crate::workspace_msg::resolve_workspace(claim) else {
        return Err(ResolveCallerError::NotFound(claim.to_string()));
    };
    let project_id = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::workspace::agent_identity::resolve_project_id(&conn, &path)
    };
    match project_id {
        Some(id) => Ok(CallerWorkspace {
            path,
            workspace_uuid: id,
        }),
        None => Err(ResolveCallerError::Unregistered(path)),
    }
}

// ── Request-scoped principal (mail / DNS handlers) ─────────────────────
//
// POST handlers read identity from JSON bodies and do not share the
// params map `stamp_principal` mutates. The dispatcher / cell server
// installs the validated principal for the duration of a spawn_blocking
// handler so every `resolve_caller*` call sees it without threading
// `Option<HookPrincipal>` through every signature.

thread_local! {
    static REQUEST_PRINCIPAL: RefCell<Option<HookPrincipal>> = const { RefCell::new(None) };
}

/// Run `f` with `principal` as the request-scoped caller identity.
/// Always clears the slot afterward (including on panic via `Drop`).
pub fn with_request_principal<R>(
    principal: Option<HookPrincipal>,
    f: impl FnOnce() -> R,
) -> R {
    struct ClearOnDrop;
    impl Drop for ClearOnDrop {
        fn drop(&mut self) {
            REQUEST_PRINCIPAL.with(|c| *c.borrow_mut() = None);
        }
    }
    let _guard = ClearOnDrop;
    REQUEST_PRINCIPAL.with(|c| *c.borrow_mut() = principal);
    f()
}

/// The principal installed by [`with_request_principal`], if any.
pub fn request_principal() -> Option<HookPrincipal> {
    REQUEST_PRINCIPAL.with(|c| c.borrow().clone())
}

/// Marker key written by [`stamp_principal`] so param-based handlers can
/// detect a server-stamped identity without the request-scoped slot
/// (e.g. pure unit tests of param maps).
pub const PRINCIPAL_BOUND_KEY: &str = "principal_bound";

/// Stamp server-side caller identity into a params map.
///
/// Overwrites `from`, `project_id`, and (when the principal's workspace
/// resolves) `project` / `project_path`. Sets [`PRINCIPAL_BOUND_KEY`].
/// Recipient/routing args (`workspace` / `target`) are left untouched.
///
/// Shared by the cell UDS server and the HTTP mail/DNS arms so identity
/// stamping is not unix-only.
pub fn stamp_principal(
    params: &mut std::collections::HashMap<String, String>,
    principal: &HookPrincipal,
) {
    params.insert("from".to_string(), principal.agent_address.clone());
    params.insert("project_id".to_string(), principal.workspace_uuid.clone());
    params.insert(PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());

    let uuid = principal.workspace_uuid.trim();
    if uuid.is_empty() {
        params.remove("project");
        params.remove("project_path");
        return;
    }
    match crate::workspace_msg::resolve_workspace(uuid) {
        Some(path) => {
            params.insert("project".to_string(), path.clone());
            params.insert("project_path".to_string(), path);
        }
        None => {
            // Fail closed: never let a body-supplied operand survive an
            // unresolvable principal.
            params.remove("project");
            params.remove("project_path");
        }
    }
}

/// Build a principal from a stamped param map (`principal_bound=1` +
/// `project_id` + optional `from`), else fall back to
/// [`request_principal`].
pub fn principal_from_params(
    params: &std::collections::HashMap<String, String>,
) -> Option<HookPrincipal> {
    if params.get(PRINCIPAL_BOUND_KEY).map(String::as_str) == Some("1") {
        if let Some(uuid) = params
            .get("project_id")
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
        {
            return Some(HookPrincipal {
                workspace_uuid: uuid.to_string(),
                agent_address: params
                    .get("from")
                    .cloned()
                    .unwrap_or_default(),
            });
        }
    }
    request_principal()
}

/// Convenience for mail/DNS: principal from params/slot + optional claim.
pub fn resolve_caller_workspace_from_params(
    params: &std::collections::HashMap<String, String>,
) -> Result<CallerWorkspace, ResolveCallerError> {
    let principal = principal_from_params(params);
    let claim = params
        .get("project")
        .or_else(|| params.get("project_path"))
        .map(String::as_str);
    resolve_caller_workspace(principal.as_ref(), claim)
}

/// Convenience when only a body claim string is available (POST JSON).
/// Uses [`request_principal`] when installed.
pub fn resolve_caller_workspace_claim(
    project_claim: Option<&str>,
) -> Result<CallerWorkspace, ResolveCallerError> {
    resolve_caller_workspace(request_principal().as_ref(), project_claim)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn seed_project(id: &str, path: &str, name: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, path, name],
        )
        .expect("seed project");
    }

    fn cleanup(id: &str) {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let _ = conn.execute("DELETE FROM projects WHERE id = ?1", rusqlite::params![id]);
    }

    #[test]
    fn principal_wins_over_spoofed_project_claim() {
        let _ = k2_core::db::init_for_tests();
        let a_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        let b_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        let a_path = "/tmp/k2-caller-ws-A";
        let b_path = "/tmp/k2-caller-ws-B";
        seed_project(a_id, a_path, "ws-A");
        seed_project(b_id, b_path, "ws-B");

        let principal = HookPrincipal {
            workspace_uuid: a_id.to_string(),
            agent_address: "agent-A".to_string(),
        };
        // Client claims B (the live bug: K2_PROJECT_PATH=B while sitting in A).
        let got = resolve_caller_workspace(Some(&principal), Some(b_path))
            .expect("principal must resolve");
        assert_eq!(got.workspace_uuid, a_id);
        assert_eq!(got.path, a_path);

        cleanup(a_id);
        cleanup(b_id);
    }

    #[test]
    fn claim_only_still_resolves_for_owner_path() {
        let _ = k2_core::db::init_for_tests();
        let id = "cccccccc-cccc-cccc-cccc-cccccccccccc";
        let path = "/tmp/k2-caller-ws-C";
        seed_project(id, path, "ws-C");

        let got = resolve_caller_workspace(None, Some(path)).expect("claim resolves");
        assert_eq!(got.workspace_uuid, id);
        assert_eq!(got.path, path);

        // Residual: owner token + B's path still works — documented.
        let b_id = "dddddddd-dddd-dddd-dddd-dddddddddddd";
        let b_path = "/tmp/k2-caller-ws-D";
        seed_project(b_id, b_path, "ws-D");
        let spoof = resolve_caller_workspace(None, Some(b_path)).expect("claim-only residual");
        assert_eq!(
            spoof.workspace_uuid, b_id,
            "RESIDUAL: without principal, project= is still identity (owner-token path)"
        );

        cleanup(id);
        cleanup(b_id);
    }

    #[test]
    fn missing_claim_without_principal_errors() {
        let _ = k2_core::db::init_for_tests();
        assert_eq!(
            resolve_caller_workspace(None, None),
            Err(ResolveCallerError::MissingClaim)
        );
        assert_eq!(
            resolve_caller_workspace(None, Some("  ")),
            Err(ResolveCallerError::MissingClaim)
        );
    }

    #[test]
    fn empty_principal_uuid_fails_closed() {
        let _ = k2_core::db::init_for_tests();
        let p = HookPrincipal {
            workspace_uuid: String::new(),
            agent_address: "x".to_string(),
        };
        assert_eq!(
            resolve_caller_workspace(Some(&p), Some("/tmp/anything")),
            Err(ResolveCallerError::UnresolvablePrincipal)
        );
    }

    #[test]
    fn stamped_params_override_spoofed_project() {
        let _ = k2_core::db::init_for_tests();
        let a_id = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
        let b_id = "ffffffff-ffff-ffff-ffff-ffffffffffff";
        let a_path = "/tmp/k2-caller-ws-E";
        let b_path = "/tmp/k2-caller-ws-F";
        seed_project(a_id, a_path, "ws-E");
        seed_project(b_id, b_path, "ws-F");

        let mut params = HashMap::new();
        params.insert(PRINCIPAL_BOUND_KEY.to_string(), "1".to_string());
        params.insert("project_id".to_string(), a_id.to_string());
        params.insert("from".to_string(), "agent-E".to_string());
        // Spoofed claim still present (what a hostile raw request might send).
        params.insert("project".to_string(), b_path.to_string());

        let got = resolve_caller_workspace_from_params(&params).expect("stamped");
        assert_eq!(got.workspace_uuid, a_id);
        assert_eq!(got.path, a_path);

        cleanup(a_id);
        cleanup(b_id);
    }

    #[test]
    fn request_principal_slot_covers_body_claim_path() {
        let _ = k2_core::db::init_for_tests();
        let a_id = "12121212-1212-1212-1212-121212121212";
        let b_id = "34343434-3434-3434-3434-343434343434";
        let a_path = "/tmp/k2-caller-ws-G";
        let b_path = "/tmp/k2-caller-ws-H";
        seed_project(a_id, a_path, "ws-G");
        seed_project(b_id, b_path, "ws-H");

        let principal = HookPrincipal {
            workspace_uuid: a_id.to_string(),
            agent_address: "agent-G".to_string(),
        };
        let got = with_request_principal(Some(principal), || {
            // Body claims B — must not win.
            resolve_caller_workspace_claim(Some(b_path))
        })
        .expect("slot principal");
        assert_eq!(got.workspace_uuid, a_id);
        assert!(request_principal().is_none(), "slot cleared after with_*");

        cleanup(a_id);
        cleanup(b_id);
    }
}
