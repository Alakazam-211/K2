//! K-M2 (0.46.1): the reserved `meet` label for the Meet App.
//!
//! `meet.<box>.k2.dev` is reserved (`k2_core::skin::MEET_NESTED_LABEL`): no
//! workspace, `k2 publish` or `k2 app install meet` may claim it. It binds
//! the box-level way, like Sew's `voice` label: K2 points the nested label
//! at the App's **front port** with the box's tunnel token
//! ([`super::ops::SewOps::nested_label`], which refuses a label a workspace
//! holds), never through the user publish routes.
//!
//! - `k2 hostchat meet bind <app-id>` (`POST /cli/sew/meet/bind {app}`):
//!   the App must be installed here. Its front listener routes every
//!   connection to its live instance, so deploys and rollbacks need no
//!   re-point. The App gets `https://meet.<box>.k2.dev` in
//!   `K2_APP_ORIGIN(S)` on its next start (restart it once).
//! - `k2 hostchat meet unbind` (`POST /cli/sew/meet/unbind`): deletes the
//!   nested label and the record.
//! - The record is `~/.k2/sew/meet.json` (`{app, frontPort, url, boundAt}`);
//!   `GET /cli/sew/status` shows it as `meet`.
//!
//! Manage Sew gate (owner/admin, or an agent whose workspace has "Manage
//! Sew"). Sew itself need not be installed: Meet can run standalone.

use serde_json::{json, Value};

use super::supervisor::{no, Env, R};

pub const RECORD: &str = "meet";
pub const MEET_LABEL: &str = k2_core::skin::MEET_NESTED_LABEL;

/// The bound App and its public URL, from a record.
fn parse(v: &Value) -> Option<(String, Option<String>)> {
    let app = v.get("app")?.as_str()?.to_string();
    Some((app, v.get("url").and_then(Value::as_str).map(str::to_string)))
}

/// `https://meet.<label>.k2.dev`.
pub fn meet_url(label: &str) -> String {
    format!("https://{MEET_LABEL}.{label}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST)
}

/// The App bound to `meet` on this box right now, read from the live
/// record (`None` = none). For `K2_APP_ORIGINS`.
pub fn bound_live() -> Option<(String, Option<String>)> {
    let text = std::fs::read_to_string(super::ops::record_path(RECORD)).ok()?;
    parse(&serde_json::from_str(&text).ok()?)
}

/// `status.meet`: `{app, url, frontPort}` or null.
pub fn status(env: &Env) -> Value {
    env.ops.record_load(RECORD).unwrap_or(Value::Null)
}

pub fn bind(env: &Env, app_id: &str) -> R<Value> {
    let app_id = app_id.trim().to_ascii_lowercase();
    if app_id.is_empty() {
        return Err(no("bad_request", 400, "name the App to bind (k2 hostchat meet bind <app-id>)"));
    }
    let app = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::deploy::store::get_app(&conn, &app_id).map_err(|e| no("db_error", 500, e.to_string()))?
    };
    let app = app.ok_or_else(|| no("app_not_found", 404, format!("no App '{app_id}' on this box (k2 app install {app_id} first)")))?;
    let port = app
        .front_port
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .ok_or_else(|| no("app_not_ready", 409, format!("App '{app_id}' has no front port yet (k2 app status {app_id})")))?;
    let label = env
        .ops
        .box_label()
        .ok_or_else(|| no("no_label", 409, "the meet label needs K2 Connect on this box (k2 connect status)"))?;
    if let Some((other, _)) = env.ops.record_load(RECORD).as_ref().and_then(parse) {
        if other != app_id {
            return Err(no(
                "meet_bound",
                409,
                format!("meet.{label}.k2.dev is bound to App '{other}'; unbind it first (k2 hostchat meet unbind)"),
            ));
        }
    }
    env.ops
        .nested_label(MEET_LABEL, Some(&format!("127.0.0.1:{port}")))
        .map_err(|e| no("meet_label_failed", 409, format!("the meet label could not be pointed at App '{app_id}': {e}")))?;
    let url = meet_url(&label);
    let rec = json!({"app": app_id, "frontPort": port, "url": url, "boundAt": env.ops.now()});
    env.ops.record_save(RECORD, Some(&rec)).map_err(|e| no("io_error", 500, e))?;
    super::supervisor::changed("meet-bind");
    Ok(json!({
        "ok": true,
        "app": app_id,
        "url": url,
        "frontPort": port,
        "hint": format!("restart the App once so it gets {url} in K2_APP_ORIGINS: k2 app restart {app_id}"),
    }))
}

pub fn unbind(env: &Env) -> R<Value> {
    let had = env.ops.record_load(RECORD).as_ref().and_then(parse);
    env.ops
        .nested_label(MEET_LABEL, None)
        .map_err(|e| no("meet_label_failed", 409, format!("the meet label could not be removed: {e}")))?;
    env.ops.record_save(RECORD, None).map_err(|e| no("io_error", 500, e))?;
    super::supervisor::changed("meet-unbind");
    Ok(json!({"ok": true, "unbound": had.map(|(a, _)| a)}))
}
