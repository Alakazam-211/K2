//! Vault secrets that ride a "Clone to" / "Clone to this computer" move
//! (0.45.2, prd-k2-vault-v1 §13.3).
//!
//! Ruling: when the owner moves THEIR OWN workspace to THEIR OWN server,
//! the workspace's vault secrets travel with it. The renderer asks the
//! destination daemon for its carry recipient (`GET /cli/vault/recipient`)
//! and hands it to the source's `clone/bundle` / `clone/pack` as
//! `vault_target` plus `own_box_move: true`. The source seals that
//! workspace's `user` secrets (workspace scope only, never server scope)
//! into a carry bound to the destination vault; the renderer passes the
//! opaque `vault_carry_b64` to the destination's `clone/unpack`, which
//! imports it into the NEWLY registered project (its id, not a name
//! lookup — the destination may have renamed the folder).
//!
//! - A clone for another owner never carries (Q7): a `vault_target`
//!   without `own_box_move` is refused with a teaching code.
//! - The recovery key is never involved; values are re-encrypted box to
//!   box and never appear in any response.
//! - On the destination a carry problem never fails the unpack; it is
//!   reported next to the result.

use std::collections::HashMap;
use std::sync::Arc;

use base64::Engine as _;
use k2_vault::{LocalVault, VaultError};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::vault::actor::VaultCaller;
use crate::vault_use_routes::{apply_carry, may_carry, seal_carry, ItemTarget};

/// The destination vault, as its `GET /cli/vault/recipient` answered.
#[derive(Debug, Clone, Deserialize)]
pub struct VaultTarget {
    /// `age1…` carry recipient.
    pub recipient: String,
    /// The destination vault id the carry is bound to.
    pub vault_id: String,
}

/// Teaching code for a carry requested outside an own-server move.
pub const OWN_MOVE_ONLY: &str = "vault_carry_own_move_only";

/// A source request that names a destination vault must also say this is
/// the owner's own move. `Some(400)` when it doesn't.
pub fn refuse_unless_own_move(target: Option<&VaultTarget>, own_box_move: bool) -> Option<CliResponse> {
    if target.is_none() || own_box_move {
        return None;
    }
    Some(CliResponse {
        status: "400 Bad Request",
        content_type: "application/json",
        body: json!({
            "error": "Vault secrets only move when you move your own workspace to your own server. \
                      For a clone for someone else, leave the vault option off; they add their own secrets.",
            "code": OWN_MOVE_ONLY,
        })
        .to_string(),
    })
}

/// Everything the source side needs to seal a carry.
#[derive(Clone)]
pub struct CarryRequest {
    pub target: VaultTarget,
    /// Who asked (None when the token couldn't be mapped to a vault caller).
    pub caller: Option<VaultCaller>,
    /// This daemon's vault, or why it isn't open.
    pub vault: Result<Arc<LocalVault>, VaultError>,
}

/// The source side's answer: the opaque carry (when one was sealed) and a
/// names-only report for the response.
pub struct Sealed {
    pub carry_b64: Option<String>,
    pub report: Value,
}

fn skipped(reason: &str, note: &str) -> Sealed {
    Sealed {
        carry_b64: None,
        report: json!({ "sealed": false, "count": 0, "secrets": [], "reason": reason, "note": note }),
    }
}

/// projects.id of the registered workspace at `path` (canonical paths
/// compared, so a symlinked spelling matches).
pub fn project_id_for_path(path: &str) -> Option<String> {
    if let Some(id) = crate::canonical_session::lookup_project_id(path) {
        return Some(id);
    }
    let want = std::fs::canonicalize(path).ok()?;
    k2_core::projects_ops::projects_list()
        .ok()?
        .into_iter()
        .find(|p| std::fs::canonicalize(&p.path).map(|c| c == want).unwrap_or(false))
        .map(|p| p.id)
}

/// Seal the `user` secrets of the workspace at `project_path` (workspace
/// scope only) to the destination vault in `req`.
pub fn seal_for_workspace(req: &CarryRequest, project_path: &str) -> Sealed {
    let Some(c) = req.caller.as_ref() else {
        return skipped("unauthorized", "Couldn't tell who asked, so no vault secrets were moved.");
    };
    if let Err(e) = may_carry(c) {
        return skipped(e.code, "Moving vault secrets needs an Owner or Admin on this server; none were moved.");
    }
    let vault = match &req.vault {
        Ok(v) => v,
        Err(e) => return skipped(e.code, "The vault on the source server isn't open, so no vault secrets were moved."),
    };
    let Some(id) = project_id_for_path(project_path) else {
        return skipped("not_registered", "This folder isn't a registered workspace, so it has no vault secrets to move.");
    };
    let label = crate::vault::workspace_label(&id);
    let mut ws = HashMap::new();
    ws.insert(id, label);
    match seal_carry(vault, &req.target.recipient, &req.target.vault_id, &ws, &[], true, None) {
        Ok(s) if s.names.is_empty() => skipped("no_secrets", "This workspace has no vault secrets to move."),
        Ok(s) => {
            let names: Vec<&str> = s.names.iter().map(|(_, n)| n.as_str()).collect();
            crate::vault::auth_audit(
                "vault-carry",
                &c.actor.stamp(),
                &format!("ok clone export {} to {}", names.len(), s.target_fingerprint),
                "-",
            );
            Sealed {
                carry_b64: Some(base64::engine::general_purpose::STANDARD.encode(&s.file)),
                report: json!({ "sealed": true, "count": names.len(), "secrets": names }),
            }
        }
        Err(e) => skipped(e.code, &format!("No vault secrets were moved: {}", e.hint)),
    }
}

/// Destination side: open `carry_b64` and write its workspace secrets into
/// `project_id`. Never fails the caller; the outcome is
/// `{vault_imported, vault_skipped[, vault_error]}` (names only).
pub fn import_into_project(
    vault: Result<Arc<LocalVault>, VaultError>,
    caller: Option<&VaultCaller>,
    carry_b64: &str,
    project_id: &str,
) -> Value {
    let fail = |code: &str, hint: &str| {
        json!({ "vault_imported": [], "vault_skipped": [], "vault_error": { "code": code, "hint": hint } })
    };
    let Some(c) = caller else {
        return fail("unauthorized", "Couldn't tell who asked, so the vault secrets weren't added.");
    };
    if let Err(e) = may_carry(c) {
        return fail(e.code, "Adding moved vault secrets needs an Owner or Admin on this server.");
    }
    let vault = match vault {
        Ok(v) => v,
        Err(e) => return fail(e.code, "The vault on this server isn't open, so the moved secrets weren't added."),
    };
    let file = match base64::engine::general_purpose::STANDARD.decode(carry_b64.trim()) {
        Ok(f) => f,
        Err(_) => return fail("bad_carry", "The vault carry wasn't readable."),
    };
    let p = match vault.carry_open(&file) {
        Ok(p) => p,
        Err(e) => return fail(e.code, &e.hint),
    };
    if !p.own_box_move {
        return fail(OWN_MOVE_ONLY, "This carry wasn't made for moving your own workspace, so a clone doesn't add it.");
    }
    // By the new project's id, never a name (CL12); one workspace only.
    let first = p.items.iter().find(|i| i.scope_kind != "server").map(|i| i.scope_label.clone());
    let targets: Vec<ItemTarget> = p
        .items
        .iter()
        .map(|i| match i.scope_kind.as_str() {
            "server" => ItemTarget::Skip(format!("{}: server-wide secrets don't move with a workspace", i.name)),
            _ if Some(&i.scope_label) != first.as_ref() => ItemTarget::Skip(format!("{}: belongs to another workspace", i.name)),
            _ => ItemTarget::Workspace(project_id.to_string()),
        })
        .collect();
    match apply_carry(&vault, &p, &targets) {
        Ok((imported, skipped)) => {
            crate::vault::auth_audit(
                "vault-carry",
                &c.actor.stamp(),
                &format!("ok clone import {} from {}", imported.len(), p.source_vault_id),
                "-",
            );
            json!({ "vault_imported": imported, "vault_skipped": skipped })
        }
        Err(e) => fail(e.code, &e.hint),
    }
}
