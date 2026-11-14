//! `k2 compute run --cache NAME --cache-required`: the job runs only on
//! the node that holds its cache, pinned there however long it waits.
//! Without the flag the cache is a preference (CE-6, `compute_place`):
//! auto placement ranks that node first and goes elsewhere with an empty
//! cache when it's busy for long. Which node holds a cache is
//! [`k2_core::compute::store::cache_home`].

use k2_core::compute::{self as compute, store};

use crate::compute_place::Want;
use crate::compute_routes::{b, no, pinned_now, s, Who, Ws, R};

const BAD_REQUEST: &str = compute::codes::BAD_REQUEST;

/// `--cache-required` with no node named and no `--same-node-as`: the
/// active node that holds this workspace's cache, which the job is pinned
/// to. `None` without the flag, or when no node holds it yet (the job is
/// then placed as usual and starts the cache there).
pub(crate) fn cache_pin(ws: &Ws, v: &serde_json::Value, cache: Option<&str>) -> Option<store::NodeRow> {
    let name = cache.filter(|_| b(v, "cacheRequired") == Some(true) && s(v, "node").is_none() && s(v, "sameNodeAs").is_none())?;
    home(ws, name).filter(|n| n.state == "active")
}

fn home(ws: &Ws, name: &str) -> Option<store::NodeRow> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    store::cache_home(&conn, &ws.id, name).and_then(|id| store::node_by_id(&conn, &id))
}

/// The submit checks, and the node the job is pinned to ([`cache_pin`]):
/// `--cache-required` needs `--cache`, and a job pinned to the cache's
/// node refuses an `--avoid` or `--only` that leaves that node out (a
/// standing list that doesn't is fine).
pub(crate) fn check_submit(ws: &Ws, v: &serde_json::Value, cache: Option<&str>, avoid: &[String], only: &[String]) -> R<Option<store::NodeRow>> {
    if b(v, "cacheRequired") == Some(true) && cache.is_none() {
        return Err(no(BAD_REQUEST, 400, "--cache-required goes with --cache NAME: it makes the job wait for the node that holds that cache"));
    }
    let pin = cache_pin(ws, v, cache);
    let (Some(n), Some(name)) = (pin.as_ref(), cache) else { return Ok(pin) };
    let flag = if avoid.contains(&n.id) {
        "your --avoid"
    } else if !only.is_empty() && !only.contains(&n.id) {
        "your --only list"
    } else {
        return Ok(pin);
    };
    Err(no(
        BAD_REQUEST,
        400,
        format!("--cache-required: cache {name} is on {}, which {flag} leaves out. Drop --cache-required to run elsewhere with an empty cache, or let the job use {}", n.name, n.name),
    ))
}

/// A node named with `--cache-required`: that node must hold the cache
/// (or no node holds it yet).
pub(crate) fn check_named(ws: &Ws, v: &serde_json::Value, cache: Option<&str>, named: &store::NodeRow) -> R<()> {
    let Some(name) = cache.filter(|_| b(v, "cacheRequired") == Some(true)) else { return Ok(()) };
    match home(ws, name) {
        Some(h) if h.id != named.id => Err(no(
            BAD_REQUEST,
            400,
            format!(
                "--cache-required: cache {name} is on {}, not {}. Leave the node off (K2 sends it to {}), or drop --cache-required to start an empty cache on {}",
                h.name, named.name, h.name, named.name
            ),
        )),
        _ => Ok(()),
    }
}

/// Placement for `--cache-required` (no node named, no `--same-node-as`):
/// the cache's node, pinned, when one holds it (on the workspace's node
/// list; `--now` only if it can start there now). `None`: placed as usual.
pub(crate) fn placed(who: &Who, ws: &Ws, v: &serde_json::Value, want: &Want, now: i64) -> R<Option<(store::NodeRow, store::Placement)>> {
    let (Some(n), Some(name)) = (cache_pin(ws, v, want.cache.as_deref()), want.cache.as_deref()) else { return Ok(None) };
    crate::compute_allow::check_node(ws, &n, &format!("--cache-required (cache {name} is there): node"))?;
    if b(v, "now") == Some(true) {
        pinned_now(who, ws, want, &n, now)?;
    }
    let mut rec = crate::compute_place::pinned_record(&n.name, now);
    rec.why = format!("holds cache {name} (--cache-required: the job waits for this node)");
    Ok(Some((n, rec)))
}
