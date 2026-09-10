//! `/cli/publish/*` — daemon-owned published services.
//!
//! GET: list, logs, leftovers. POST: run/start/stop/rm + subdomain
//! claim/unclaim (self-only 0074 stamp). Mutating routes are POST-only
//! (GET twins 405). Not under `/cli/tunnel/` (owner-only deny).

use std::collections::HashMap;
use std::path::PathBuf;

use k2_core::db::schema::{SelfStamp, SelfUnclaim, SubdomainWorkspace};
use k2_core::tunnel::config::SUBDOMAIN_HOST;

use crate::cli::{need_project, opt_param, str_param};
use crate::cli_response::CliResponse;
use crate::publish_runtime::{self, PublishError, RunSpec};

const FOREIGN_STICKER: &str = "not yours; the current agent must `k2 publish transfer`";

fn resolve_project_id(project: &str) -> Result<String, String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT id FROM projects WHERE path = ?1 OR id = ?1",
        rusqlite::params![project],
        |r| r.get(0),
    )
    .map_err(|_| format!("no registered workspace matches {project:?}"))
}

fn resolve_project_path(project_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.query_row(
        "SELECT path FROM projects WHERE id = ?1",
        rusqlite::params![project_id],
        |r| r.get(0),
    )
    .ok()
}

fn err_to_resp(e: PublishError) -> CliResponse {
    CliResponse {
        status: e.status,
        content_type: "application/json",
        body: serde_json::json!({ "error": e.message }).to_string(),
    }
}

fn boolish(params: &HashMap<String, String>, keys: &[&str]) -> bool {
    for k in keys {
        if let Some(v) = params.get(*k) {
            if matches!(v.as_str(), "1" | "true" | "on" | "yes") {
                return true;
            }
        }
    }
    false
}

fn parse_port(params: &HashMap<String, String>) -> Result<u16, CliResponse> {
    let raw = str_param(params, "port");
    if raw.trim().is_empty() {
        return Err(CliResponse::bad_request("Missing port"));
    }
    raw.parse::<u16>()
        .map_err(|_| CliResponse::bad_request("port must be an integer 1–65535"))
        .and_then(|p| {
            if p == 0 {
                Err(CliResponse::bad_request("Missing port"))
            } else {
                Ok(p)
            }
        })
}

/// GET dispatcher: list + logs + leftovers + 405 twins for POST-only paths.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> Option<CliResponse> {
    if !path.starts_with("/cli/publish/") {
        return None;
    }
    Some(match path {
        "/cli/publish/list" => handle_list(params),
        "/cli/publish/logs" => handle_logs(params),
        "/cli/publish/leftovers" => handle_leftovers(params),
        "/cli/publish/run"
        | "/cli/publish/start"
        | "/cli/publish/stop"
        | "/cli/publish/rm"
        | "/cli/publish/subdomain/claim"
        | "/cli/publish/subdomain/unclaim" => CliResponse::method_not_allowed(),
        _ => CliResponse::not_found(),
    })
}

/// POST dispatcher.
pub fn dispatch_post(path: &str, params: &HashMap<String, String>) -> CliResponse {
    match path {
        "/cli/publish/run" => handle_run(params),
        "/cli/publish/start" => handle_start(params),
        "/cli/publish/stop" => handle_stop(params),
        "/cli/publish/rm" => handle_rm(params),
        "/cli/publish/subdomain/claim" => handle_subdomain_claim(params),
        "/cli/publish/subdomain/unclaim" => handle_subdomain_unclaim(params),
        _ => CliResponse::not_found(),
    }
}

fn forbidden_sticker() -> CliResponse {
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: serde_json::json!({ "error": FOREIGN_STICKER }).to_string(),
    }
}

/// Same rules as renderer `nestedPublicUrl`: https public_url host prefix,
/// else `https://<label>.<primary>.k2.dev`, else null. Never fabricates.
fn nested_public_url(label: &str, primary: &str, public_url: Option<&str>) -> Option<String> {
    let clean_label = label.trim();
    if clean_label.is_empty() {
        return None;
    }
    let url = public_url.unwrap_or("").trim().trim_end_matches('/');
    if let Some(host) = url.strip_prefix("https://") {
        if !host.is_empty() {
            return Some(format!("https://{clean_label}.{host}"));
        }
    }
    let clean_primary = primary.trim();
    if !clean_primary.is_empty() {
        return Some(format!(
            "https://{clean_label}.{clean_primary}.{SUBDOMAIN_HOST}"
        ));
    }
    None
}

/// GET /cli/publish/leftovers?project= — this workspace's 0074 labels
/// unioned with the in-memory tunnel cache. Never calls Connect /
/// `refresh_once`. Empty cache still returns stickers.
fn handle_leftovers(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let stored = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match SubdomainWorkspace::list_for_project(&conn, &project_id) {
            Ok(rows) => rows,
            Err(e) => return CliResponse::internal_error(e.to_string()),
        }
    };
    let cache = k2_core::tunnel::subdomains::current();
    let public_url = k2_core::tunnel::tunnel_status().public_url;
    let leftovers: Vec<serde_json::Value> = stored
        .into_iter()
        .map(|(label, stored_target)| {
            let target = match cache.targets.get(&label) {
                Some(t) if !t.is_empty() => t.clone(),
                _ => stored_target,
            };
            let url = nested_public_url(&label, &cache.primary, public_url.as_deref());
            serde_json::json!({
                "label": label,
                "target": target,
                "url": url,
            })
        })
        .collect();
    CliResponse::ok_json(serde_json::json!({ "leftovers": leftovers }).to_string())
}

/// POST /cli/publish/subdomain/claim — self-only 0074 stamp. INSERT if
/// unattributed or already this workspace; foreign → 403, no rewrite.
/// Does not call `SubdomainWorkspace::claim` (REPLACE / steal).
fn handle_subdomain_claim(params: &HashMap<String, String>) -> CliResponse {
    let label = str_param(params, "label");
    if label.trim().is_empty() {
        return CliResponse::bad_request("Missing label");
    }
    if k2_core::skin::is_reserved_nested_label(&label) {
        return CliResponse::bad_request(k2_core::skin::reserved_nested_label_error(&label));
    }
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let target = opt_param(params, "target");
    let outcome = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match SubdomainWorkspace::self_stamp(&conn, &label, &project_id, target.as_deref()) {
            Ok(o) => o,
            Err(e) => return CliResponse::bad_request(e.to_string()),
        }
    };
    match outcome {
        SelfStamp::Foreign { .. } => forbidden_sticker(),
        SelfStamp::Written => {
            crate::session_events::emit_tunnel_subdomains_changed();
            CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "label": label.trim().to_ascii_lowercase(),
                    "projectId": project_id,
                })
                .to_string(),
            )
        }
    }
}

/// POST /cli/publish/subdomain/unclaim — drop THIS workspace's sticker.
/// Foreign 0074 → 403, no rewrite.
fn handle_subdomain_unclaim(params: &HashMap<String, String>) -> CliResponse {
    let label = str_param(params, "label");
    if label.trim().is_empty() {
        return CliResponse::bad_request("Missing label");
    }
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let outcome = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        match SubdomainWorkspace::self_unclaim(&conn, &label, &project_id) {
            Ok(o) => o,
            Err(e) => return CliResponse::bad_request(e.to_string()),
        }
    };
    match outcome {
        SelfUnclaim::Foreign { .. } => forbidden_sticker(),
        SelfUnclaim::Removed => {
            crate::session_events::emit_tunnel_subdomains_changed();
            CliResponse::ok_json(
                serde_json::json!({
                    "success": true,
                    "label": label.trim().to_ascii_lowercase(),
                    "removed": true,
                })
                .to_string(),
            )
        }
        SelfUnclaim::Absent => CliResponse::ok_json(
            serde_json::json!({
                "success": true,
                "label": label.trim().to_ascii_lowercase(),
                "removed": false,
            })
            .to_string(),
        ),
    }
}

fn handle_list(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::list(&project_id) {
        Ok(services) => {
            CliResponse::ok_json(serde_json::json!({ "services": services }).to_string())
        }
        Err(e) => err_to_resp(e),
    }
}

fn handle_logs(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let mut n: usize = 200;
    if let Some(lines) = opt_param(params, "lines") {
        if let Ok(v) = lines.parse::<usize>() {
            n = v.max(1);
        }
    }
    if let Some(tail) = opt_param(params, "tail") {
        if let Ok(v) = tail.parse::<usize>() {
            n = v.max(1);
        }
    }
    let text = publish_runtime::read_log_tail(&project_id, &name, n);
    CliResponse::ok_json(
        serde_json::json!({
            "name": name,
            "text": text,
        })
        .to_string(),
    )
}

fn handle_run(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let cmd = str_param(params, "cmd");
    let skin = boolish(params, &["skin"]);
    let skin_root = opt_param(params, "skinRoot")
        .or_else(|| opt_param(params, "skin_root"))
        .unwrap_or_default();
    let cwd_explicit = opt_param(params, "cwd").is_some();
    if skin && cwd_explicit {
        return CliResponse::bad_request("--cwd and --skin are mutually exclusive");
    }
    if skin && !cmd.trim().is_empty() {
        return CliResponse::bad_request("--cmd and --skin are mutually exclusive");
    }
    let port = match parse_port(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let no_tunnel = boolish(params, &["noTunnel", "no_tunnel"]);
    let cwd = opt_param(params, "cwd")
        .map(PathBuf::from)
        .or_else(|| resolve_project_path(&project_id).map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(&project));
    match publish_runtime::run(RunSpec {
        project_id,
        name,
        cmd,
        cwd,
        port,
        no_tunnel,
        replace_spec: true,
        skin,
        skin_root,
        cwd_explicit,
    }) {
        Ok(svc) => {
            CliResponse::ok_json(serde_json::to_string(&svc).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => err_to_resp(e),
    }
}

fn handle_start(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::start(&project_id, &name) {
        Ok(svc) => {
            CliResponse::ok_json(serde_json::to_string(&svc).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => err_to_resp(e),
    }
}

fn handle_stop(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::stop(&project_id, &name) {
        Ok(svc) => {
            CliResponse::ok_json(serde_json::to_string(&svc).unwrap_or_else(|_| "{}".into()))
        }
        Err(e) => err_to_resp(e),
    }
}

fn handle_rm(params: &HashMap<String, String>) -> CliResponse {
    let project = match need_project(params) {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    let name = str_param(params, "name");
    if name.trim().is_empty() {
        return CliResponse::bad_request("Missing name");
    }
    let keep = boolish(params, &["keepHostname", "keep_hostname"]);
    let project_id = match resolve_project_id(&project) {
        Ok(id) => id,
        Err(e) => return CliResponse::bad_request(e),
    };
    match publish_runtime::rm(&project_id, &name, keep) {
        Ok(()) => CliResponse::ok_json(serde_json::json!({ "ok": true, "name": name }).to_string()),
        Err(e) => err_to_resp(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_chain_405s_mutating_publish_routes() {
        let p = HashMap::new();
        for path in [
            "/cli/publish/run",
            "/cli/publish/start",
            "/cli/publish/stop",
            "/cli/publish/rm",
            "/cli/publish/subdomain/claim",
            "/cli/publish/subdomain/unclaim",
        ] {
            let resp = dispatch(path, &p).expect("GET twin must exist");
            assert_eq!(resp.status, "405 Method Not Allowed", "path={path}");
        }
    }

    #[test]
    fn get_list_requires_project() {
        let resp = dispatch("/cli/publish/list", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "400 Bad Request");
    }

    #[test]
    fn unknown_publish_path_is_404() {
        let resp = dispatch("/cli/publish/nope", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "404 Not Found");
    }

    fn cache_lock() -> std::sync::MutexGuard<'static, ()> {
        static M: std::sync::Mutex<()> = std::sync::Mutex::new(());
        M.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn seed_ws(id: &str, path: &str, name: &str) {
        let _ = k2_core::db::init_for_tests();
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT OR REPLACE INTO projects (id, path, name) VALUES (?1, ?2, ?3)",
            rusqlite::params![id, path, name],
        )
        .expect("seed workspace");
    }

    fn leftovers_json(project: &str) -> serde_json::Value {
        let mut p = HashMap::new();
        p.insert("project".into(), project.into());
        let resp = dispatch("/cli/publish/leftovers", &p).unwrap();
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        serde_json::from_str(&resp.body).expect("leftovers json")
    }

    #[test]
    fn leftovers_get_returns_0074_when_cache_empty_and_is_workspace_scoped() {
        let _g = cache_lock();
        let pid = std::process::id();
        let docs = format!("docs-left-{pid}");
        let sales = format!("sales-left-{pid}");
        let portal = format!("portal-left-{pid}");
        seed_ws(&docs, "/tmp/docs-left", "Documents");
        seed_ws(&sales, "/tmp/sales-left", "Sales");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            SubdomainWorkspace::self_stamp(&conn, &portal, &docs, None).unwrap();
        }
        k2_core::tunnel::subdomains::store(Default::default());

        let docs_body = leftovers_json(&docs);
        let rows = docs_body["leftovers"].as_array().expect("leftovers[]");
        assert!(
            rows.iter()
                .any(|r| r["label"] == portal && r["target"] == ""),
            "Documents leftover must list {portal} with unknown target; body={docs_body}"
        );
        assert!(docs_body["leftovers"][0].get("url").is_some());

        let sales_body = leftovers_json(&sales);
        let sales_rows = sales_body["leftovers"].as_array().expect("leftovers[]");
        assert!(
            !sales_rows.iter().any(|r| r["label"] == portal),
            "Sales must not inherit Documents leftover; body={sales_body}"
        );

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = SubdomainWorkspace::unclaim(&conn, &portal);
        }
    }

    #[test]
    fn leftovers_get_overlays_cache_target_and_url_when_primary_known() {
        let _g = cache_lock();
        let pid = std::process::id();
        let docs = format!("docs-ovl-{pid}");
        let portal = format!("portal-ovl-{pid}");
        let primary = format!("prim-ovl-{pid}");
        seed_ws(&docs, "/tmp/docs-ovl", "Documents");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            SubdomainWorkspace::self_stamp(&conn, &portal, &docs, None).unwrap();
        }
        let mut targets = std::collections::HashMap::new();
        targets.insert(portal.clone(), "localhost:3000".to_string());
        k2_core::tunnel::subdomains::store(k2_core::tunnel::subdomains::SubdomainMap {
            primary: primary.clone(),
            targets,
        });

        let body = leftovers_json(&docs);
        let row = body["leftovers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["label"] == portal)
            .expect("portal row");
        assert_eq!(row["target"], "localhost:3000");
        let url = row["url"].as_str().expect("url when primary known");
        assert!(
            url.contains(&portal),
            "public URL must include the leftover label; url={url} primary={primary}"
        );
        assert!(
            url.starts_with("https://"),
            "public URL must be https; url={url}"
        );

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = SubdomainWorkspace::unclaim(&conn, &portal);
        }
        k2_core::tunnel::subdomains::store(Default::default());
    }

    #[test]
    fn leftovers_get_keeps_last_target_when_cache_empty() {
        let _g = cache_lock();
        let pid = std::process::id();
        let docs = format!("docs-lt-{pid}");
        let portal = format!("portal-lt-{pid}");
        seed_ws(&docs, "/tmp/docs-lt", "Documents");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            SubdomainWorkspace::self_stamp(&conn, &portal, &docs, Some("localhost:3000")).unwrap();
        }
        k2_core::tunnel::subdomains::store(Default::default());
        let body = leftovers_json(&docs);
        let row = body["leftovers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["label"] == portal)
            .expect("portal row");
        assert_eq!(row["target"], "localhost:3000");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let _ = SubdomainWorkspace::unclaim(&conn, &portal);
        }
    }

    #[test]
    fn nested_public_url_matches_renderer_rules() {
        assert_eq!(
            nested_public_url("staging", "rosson", Some("https://rosson.k2.dev")),
            Some("https://staging.rosson.k2.dev".into())
        );
        assert_eq!(
            nested_public_url("staging", "rosson", None),
            Some("https://staging.rosson.k2.dev".into())
        );
        assert_eq!(nested_public_url("staging", "", None), None);
        assert_eq!(
            nested_public_url("", "rosson", Some("https://x.k2.dev")),
            None
        );
    }

    fn claim_params(project: &str, label: &str, target: Option<&str>) -> HashMap<String, String> {
        let mut p = HashMap::new();
        p.insert("project".into(), project.into());
        p.insert("label".into(), label.into());
        if let Some(t) = target {
            p.insert("target".into(), t.into());
        }
        p
    }

    #[test]
    fn subdomain_claim_is_self_only_and_persists_target() {
        let pid = std::process::id();
        let docs = format!("docs-claim-{pid}");
        let sales = format!("sales-claim-{pid}");
        let label = format!("claim-{pid}");
        seed_ws(&docs, "/tmp/docs-claim", "Documents");
        seed_ws(&sales, "/tmp/sales-claim", "Sales");

        let resp = dispatch_post(
            "/cli/publish/subdomain/claim",
            &claim_params(&docs, &label, Some("localhost:3000")),
        );
        assert_eq!(resp.status, "200 OK", "body={}", resp.body);
        let body: serde_json::Value = serde_json::from_str(&resp.body).unwrap();
        assert_eq!(body["success"], true);
        assert_eq!(body["projectId"], docs);

        let steal = dispatch_post(
            "/cli/publish/subdomain/claim",
            &claim_params(&sales, &label, Some("localhost:9")),
        );
        assert_eq!(steal.status, "403 Forbidden", "body={}", steal.body);
        assert!(
            steal.body.contains("k2 publish transfer"),
            "body={}",
            steal.body
        );

        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            let map = SubdomainWorkspace::map(&conn).unwrap();
            assert_eq!(map.get(&label).map(String::as_str), Some(docs.as_str()));
            let rows = SubdomainWorkspace::list_for_project(&conn, &docs).unwrap();
            assert_eq!(
                rows.iter()
                    .find(|(l, _)| l == &label)
                    .map(|(_, t)| t.as_str()),
                Some("localhost:3000")
            );
            let _ = SubdomainWorkspace::unclaim(&conn, &label);
        }
    }

    #[test]
    fn subdomain_unclaim_drops_own_sticker_not_foreign() {
        let pid = std::process::id();
        let docs = format!("docs-unclaim-{pid}");
        let sales = format!("sales-unclaim-{pid}");
        let label = format!("unclaim-{pid}");
        seed_ws(&docs, "/tmp/docs-unclaim", "Documents");
        seed_ws(&sales, "/tmp/sales-unclaim", "Sales");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            SubdomainWorkspace::self_stamp(&conn, &label, &docs, Some("localhost:3000")).unwrap();
        }
        let steal = dispatch_post(
            "/cli/publish/subdomain/unclaim",
            &claim_params(&sales, &label, None),
        );
        assert_eq!(steal.status, "403 Forbidden", "body={}", steal.body);
        let ok = dispatch_post(
            "/cli/publish/subdomain/unclaim",
            &claim_params(&docs, &label, None),
        );
        assert_eq!(ok.status, "200 OK", "body={}", ok.body);
        let body: serde_json::Value = serde_json::from_str(&ok.body).unwrap();
        assert_eq!(body["removed"], true);
        let gone = leftovers_json(&docs);
        assert!(
            !gone["leftovers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r["label"] == label),
            "successful unclaim must drop the leftover; body={gone}"
        );
    }

    #[test]
    fn leftovers_get_requires_project() {
        let resp = dispatch("/cli/publish/leftovers", &HashMap::new()).unwrap();
        assert_eq!(resp.status, "400 Bad Request");
    }
}
