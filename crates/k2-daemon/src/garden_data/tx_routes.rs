//! Garden data routes: tx/begin, tx/add, tx/commit, tx/abort for agents and
//! the person (contract §4.5; PRD §5.3.1, BG4). C0 → B2.
//!
//! The gates in `zen_data_routes::handle` already ran (credential kind,
//! person-only rows, POST-only, body cap). Handlers answer the 200 body or
//! a `DataError` (contract §4.6).
//!
//! One [`TxTable`] serves both doors (this one and the widget door,
//! `call.rs`): staged parts live in daemon memory, invisible to readers,
//! bound to their caller and one space. A part's ops are checked for key
//! syntax, value size and **grants** when staged; a failing part drops the
//! transaction. The commit reads the grants **again** (a revoke between
//! add and commit wins) and writes every op as one rev, or nothing.

use std::collections::HashMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use serde::Deserialize;
use serde_json::{json, Value as J};

use k2_core::garden_data::backend::{Op, StoreBackend};
use k2_core::garden_data::tx::{self, TxOwner, TxTable};
use k2_core::garden_data::{Actor, DataError, Space};

use crate::garden_data::agent_routes::{access, data_space};
use crate::garden_data::audit::{self, Counted};
use crate::garden_data::budget::{self, BudgetKey, Spend};
use crate::zen_data_routes::DataWho;

/// The daemon's open transactions (both doors). A restart drops them all.
pub fn table() -> MutexGuard<'static, TxTable> {
    static TABLE: OnceLock<Mutex<TxTable>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(TxTable::new())).lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Parse a POST body (deny_unknown_fields shapes) or `bad_request`.
pub(crate) fn parse<T: serde::de::DeserializeOwned>(body: &[u8], shape: &str) -> Result<T, DataError> {
    serde_json::from_slice(body).map_err(|e| DataError::BadRequest(format!("{shape}: {e}")))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BeginBody {
    widget: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct AddBody {
    tx_id: String,
    ops: Vec<Op>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct TxIdBody {
    tx_id: String,
}

fn owner_of(who: &DataWho) -> Result<TxOwner, DataError> {
    match who {
        DataWho::Owner => Ok(TxOwner::Owner),
        DataWho::Agent { project_id } | DataWho::Shell { project_id } => {
            Ok(TxOwner::Agent { project_id: project_id.clone() })
        }
        _ => Err(DataError::Refused { code: "apps_not_allowed", status: 403, hint: "not a Garden data caller".into() }),
    }
}

/// An agent's write budget key for a space; none for the person.
fn budget_key(who: &DataWho, space: &Space) -> Option<BudgetKey> {
    who.project_id().map(|p| BudgetKey::Agent { project_id: p.to_string(), space: space.as_str().to_string() })
}

/// Log a refusal one by one (contract §10) and hand it back.
fn refused(who: &DataWho, space: Option<&Space>, e: DataError) -> DataError {
    if matches!(e, DataError::NotGranted { .. } | DataError::BadTx) {
        let at = space.map(Space::as_str).unwrap_or("-");
        audit::line("zen.data.refused", &who.audit_label(), &format!("{at} code={}", e.code()));
    }
    e
}

pub fn handle(
    path: &str,
    who: &DataWho,
    body: &[u8],
    _params: &HashMap<String, String>,
) -> Result<J, DataError> {
    let owner = owner_of(who)?;
    match path {
        "/cli/zen/data/tx/begin" => {
            let b: BeginBody = parse(body, "tx/begin takes {widget}")?;
            let space = data_space(&b.widget)?;
            let acc = access(who, &space)?;
            if !acc.can_write_somewhere() {
                return Err(refused(who, Some(&space), DataError::NotGranted { key: None }));
            }
            let (tx_id, expires_at) = table().begin(owner, space, now())?;
            Ok(json!({ "txId": tx_id, "expiresAt": expires_at }))
        }
        "/cli/zen/data/tx/add" => {
            let b: AddBody = parse(body, "tx/add takes {txId, ops}")?;
            add(who, &owner, &b.tx_id, b.ops)
        }
        "/cli/zen/data/tx/commit" => {
            let b: TxIdBody = parse(body, "tx/commit takes {txId}")?;
            commit(who, &owner, &b.tx_id)
        }
        "/cli/zen/data/tx/abort" => {
            let b: TxIdBody = parse(body, "tx/abort takes {txId}")?;
            Ok(json!({ "aborted": table().abort(&b.tx_id, &owner) }))
        }
        _ => Err(DataError::NotFound("route_not_found")),
    }
}

fn add(who: &DataWho, owner: &TxOwner, tx_id: &str, ops: Vec<Op>) -> Result<J, DataError> {
    let space = table().space_of(tx_id, owner, now()).map_err(|e| refused(who, None, e))?;
    let acc = access(who, &space)?;
    if let Err(e) = tx::check_part(&ops, &acc) {
        // A failing part drops the whole transaction (PRD §5.3.1).
        table().abort(tx_id, owner);
        return Err(refused(who, Some(&space), e));
    }
    let st = table().add(tx_id, owner, ops, now())?;
    Ok(json!({ "ops": st.ops, "bytes": st.bytes, "expiresAt": st.expires_at }))
}

fn commit(who: &DataWho, owner: &TxOwner, tx_id: &str) -> Result<J, DataError> {
    let (space, ops) = table().take(tx_id, owner, now()).map_err(|e| refused(who, None, e))?;
    // Grants again: a revoke between add and commit wins, nothing written.
    let acc = access(who, &space)?;
    tx::check_writable(&ops, &acc).map_err(|e| refused(who, Some(&space), e))?;
    let bytes = tx::ops_bytes(&ops);
    if let Some(key) = budget_key(who, &space) {
        budget::spend(&key, Spend::Commit { ops: ops.len(), bytes })?;
    }
    let actor = who.actor().ok_or(DataError::PersonOnly)?;
    let rev = apply(&space, &ops, &actor, &who.audit_label(), None)?;
    Ok(json!({ "rev": rev, "ops": ops.len() }))
}

/// Write a batch or a committed transaction as one rev, push it, count it.
/// Shared with the widget door.
pub(crate) fn apply(space: &Space, ops: &[Op], actor: &Actor, label: &str, made_by: Option<&str>) -> Result<i64, DataError> {
    match crate::garden_data::store().apply(space, ops, actor) {
        Ok(rev) => {
            if space.is_data() {
                crate::garden_data::notify_changed(space.as_str(), rev);
            }
            audit::count(space.as_str(), label, made_by, Counted::Write { ops: ops.len(), bytes: tx::ops_bytes(ops) });
            Ok(rev)
        }
        Err(e) => {
            if matches!(e, DataError::Conflict { .. }) {
                audit::count(space.as_str(), label, made_by, Counted::Conflict);
            }
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bodies_are_strict() {
        assert!(parse::<AddBody>(br#"{"txId":"x","ops":[]}"#, "s").is_ok());
        assert_eq!(parse::<AddBody>(br#"{"txId":"x","ops":[],"extra":1}"#, "s").map(|_| ()).unwrap_err().code(), "bad_request");
        assert_eq!(parse::<TxIdBody>(br#"{"tx_id":"x"}"#, "s").map(|_| ()).unwrap_err().code(), "bad_request");
    }
}
