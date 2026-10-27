//! Thread export / import outside a clone (prd-clone-carries-threads-and-
//! vault-v1 §7.1, slice T4): repair and manual moves, on slice B's core
//! (`k2_core::overlay::transfer`).
//!
//! ```text
//! POST /cli/thread/export   {ws, sinceDays?}   → {path, summary, participants}
//!                           file ~/.k2/clone-tmp/threads-<handle>-<ts>.json (0600)
//! POST /cli/thread/import   {path, to}         → ImportReport (names, counts)
//! GET  /cli/thread/imports?ws=                 → overlay_imports rows (names only),
//!                                                each with addr + readOnly (0.46.1)
//! ```
//!
//! Export/import are Owner/Admin people and clone doors (the clone gate's
//! `thread-export` / `thread-import`); agent passports never reach them
//! (DENY on the passport allowlist). Imports are idempotent per line and
//! land read-only, exactly like a clone's. The export file holds Thread
//! text (re-redacted by the export), never a vault value.

use std::collections::{HashMap, HashSet};

use crate::cli_response::CliResponse;
use crate::compute_routes::{no, refusal_response, ws_by_token, Who, R};
use k2_core::connect_users::Role;
use k2_core::overlay::transfer as t;
use serde_json::{json, Value as J};

pub const EXPORT: &str = "/cli/thread/export";
pub const IMPORT: &str = "/cli/thread/import";
pub const IMPORTS: &str = "/cli/thread/imports";
pub const ROUTES: &[&str] = &[EXPORT, IMPORT, IMPORTS];
pub const POST_ROUTES: &[&str] = &[EXPORT, IMPORT];
pub const MAX_BODY: usize = 64 * 1024;

pub fn is_route(p: &str) -> bool {
    ROUTES.contains(&p)
}

pub fn is_post_route(p: &str) -> bool {
    POST_ROUTES.contains(&p)
}

/// Serve one route (blocking). The dispatcher has already run the clone
/// gate for export/import.
pub fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>, home: &std::path::Path) -> CliResponse {
    let r = match path {
        EXPORT => export(who, body, home),
        IMPORT => import(who, body, home),
        IMPORTS => imports(who, params),
        _ => Err(no("not_found", 404, "no such route")),
    };
    match r {
        Ok(v) => CliResponse::ok_json(v.to_string()),
        Err(e) => refusal_response(&e),
    }
}

fn person_admin(who: &Who) -> R<()> {
    match who {
        Who::Owner | Who::Login { role: Role::Owner | Role::Admin, .. } => Ok(()),
        Who::Agent { .. } | Who::Shell { .. } => Err(no("person_only", 403, "Thread export/import is for a person (the owner or an admin)")),
        _ => Err(no("forbidden", 403, "only the owner or an admin exports or imports Threads")),
    }
}

fn body_json(body: &[u8]) -> R<J> {
    serde_json::from_slice(body).map_err(|e| no("bad_request", 400, format!("invalid JSON body: {e}")))
}

fn s<'a>(v: &'a J, k: &str) -> Option<&'a str> {
    v.get(k).and_then(J::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn ws(token: Option<&str>) -> R<crate::compute_routes::Ws> {
    let t = token.ok_or_else(|| no("bad_request", 400, "name the workspace (ws)"))?;
    ws_by_token(t).ok_or_else(|| no("bad_request", 404, format!("no workspace '{t}'")))
}

fn export(who: &Who, body: &[u8], home: &std::path::Path) -> R<J> {
    person_admin(who)?;
    let v = body_json(body)?;
    let w = ws(s(&v, "ws").or_else(|| s(&v, "addr")))?;
    let choice = match v.get("sinceDays").and_then(J::as_u64) {
        Some(d) if (1..=3650).contains(&d) => t::ThreadsChoice::SinceDays(d as u32),
        Some(_) => return Err(no("bad_request", 400, "sinceDays is 1–3650")),
        None => t::ThreadsChoice::All,
    };
    let export_id = uuid::Uuid::new_v4().to_string();
    let export = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        t::export_project_threads(&conn, &w.id, &t::ExportOptions::new(export_id, t::default_source_host(), choice))
            .map_err(|e| no("export_failed", 500, e))?
    };
    let bytes = export.to_json().map_err(|e| no("export_failed", 500, e))?;
    let dir = home.join(".k2").join("clone-tmp");
    std::fs::create_dir_all(&dir).map_err(|e| no("export_failed", 500, format!("clone-tmp: {e}")))?;
    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let path = dir.join(format!("threads-{}-{ts}.json", w.handle.replace('/', "_")));
    write_private(&path, &bytes).map_err(|e| no("export_failed", 500, format!("write {}: {e}", path.display())))?;
    Ok(json!({
        "ok": true,
        "path": path.to_string_lossy(),
        "workspace": w.handle,
        "summary": export.summary(bytes.len()),
        "participants": export.participants(),
    }))
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    std::fs::write(path, bytes)
}

fn import(who: &Who, body: &[u8], home: &std::path::Path) -> R<J> {
    person_admin(who)?;
    let v = body_json(body)?;
    let path = s(&v, "path").ok_or_else(|| no("bad_request", 400, "path of a k2-threads-export/1 file"))?;
    let w = ws(s(&v, "to"))?;
    // Only files this server's export wrote or a person put in clone-tmp.
    let canon = dunce::canonicalize(&path).map_err(|e| no("bad_request", 400, format!("{path}: {e}")))?;
    let tmp = dunce::canonicalize(&home.join(".k2").join("clone-tmp")).unwrap_or_else(|_| home.join(".k2").join("clone-tmp"));
    if !canon.starts_with(&tmp) {
        return Err(no("bad_request", 400, format!("put the file in {} first", tmp.display())));
    }
    let meta = std::fs::metadata(&canon).map_err(|e| no("bad_request", 400, e.to_string()))?;
    if meta.len() as usize > t::MAX_EXPORT_BYTES {
        return Err(no("bad_request", 413, "the file is over the 64 MiB Thread export cap"));
    }
    let bytes = std::fs::read(&canon).map_err(|e| no("bad_request", 400, e.to_string()))?;
    let export = t::ThreadsExport::parse(&bytes).map_err(|e| no("bad_request", 400, e))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let report = t::import_threads(
        &conn,
        &export,
        &t::ImportContext {
            project_id: w.id.clone(),
            clone_handle: None,
            travelled_sessions: HashSet::new(),
            copied_secret_names: HashSet::new(),
        },
    )
    .map_err(|e| no("import_failed", 400, e))?;
    drop(conn);
    // An import writes Thread lines with no frame: move `rev` for each
    // conversation it wrote into (catchup-unchanged-v1).
    crate::overlay_ws::note_import(&report);
    Ok(json!({ "ok": true, "workspace": w.handle, "report": report }))
}

fn imports(who: &Who, params: &HashMap<String, String>) -> R<J> {
    if matches!(who, Who::Wrong | Who::Invalid) {
        return Err(no("forbidden", 403, "invalid or missing token"));
    }
    let w = ws(params.get("ws").map(String::as_str).map(str::trim).filter(|s| !s.is_empty()))?;
    let db = k2_core::db::shared();
    let conn = db.lock();
    let rows = t::list_imports(&conn, &w.id).map_err(|e| no("db", 500, e))?;
    let rows: Vec<J> = rows.iter().map(|r| import_row_json(&conn, &w.id, r)).collect();
    Ok(json!({ "ok": true, "workspace": w.handle, "imports": rows }))
}

/// One import row as the clients list it (0.46.1 "Imported Threads"): the
/// stored row plus where to read it now. `addr` is the address that opens
/// the Thread: the `import:<id>` key for history with no session of its own
/// (a detached sidecar, a main Thread still pending its pin), else the
/// conversation's current address (the workspace for the main Thread,
/// `ws/<handle>` for a sidecar), null when it has none. `readOnly` = the
/// whole Thread is imported history (an `import:` key); a canonical or
/// sidecar import is a live Thread whose imported lines alone are read-only.
fn import_row_json(conn: &rusqlite::Connection, project_id: &str, r: &t::ImportRow) -> J {
    let mut v = serde_json::to_value(r).unwrap_or(J::Null);
    let read_only = t::is_import_key(&r.local_conversation_id);
    let addr = if read_only {
        Some(r.local_conversation_id.clone())
    } else {
        k2_core::workspace_session_handles::current_address_for(conn, project_id, &r.local_conversation_id)
            .ok()
            .flatten()
    };
    v["addr"] = json!(addr);
    v["readOnly"] = json!(read_only);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_then_import_round_trips_and_refuses_passports() {
        let home = crate::test_support::TempHome::new();
        let _db = k2_core::db::scoped_for_test();
        let (src, dst) = (format!("cc-t4-src-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]), format!("cc-t4-dst-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        {
            let h = k2_core::db::shared();
            let conn = h.lock();
            for ws in [&src, &dst] {
                conn.execute(
                    "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)",
                    rusqlite::params![ws, format!("/tmp/k2-cc-t4/{ws}")],
                )
                .expect("project");
            }
            let conv = format!("conv-{src}");
            k2_core::db::schema::WorkspaceSession::upsert(&conn, &format!("ws-{src}"), &src, None, Some(&conv), "claude", "system", "running")
                .expect("pin");
            for i in 0..3 {
                k2_core::overlay::post_thread(&conn, &conv, &src, &src, &src, &format!("line {i}"), "thread").expect("post");
            }
        }
        let h = std::path::PathBuf::from(std::env::var("HOME").expect("HOME"));
        let agent = Who::Agent { project_id: src.clone(), session_id: "s".into() };
        let r = handle(EXPORT, &agent, json!({"ws": src}).to_string().as_bytes(), &HashMap::new(), &h);
        assert_eq!(r.status, "403 Forbidden", "{}", r.body);
        let r = handle(EXPORT, &Who::Owner, json!({"ws": src}).to_string().as_bytes(), &HashMap::new(), &h);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: J = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["summary"]["docs"], 3, "{v}");
        let path = v["path"].as_str().expect("path").to_string();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).expect("meta").permissions().mode() & 0o777, 0o600);
        }
        let r = handle(IMPORT, &Who::Owner, json!({"path": path, "to": dst}).to_string().as_bytes(), &HashMap::new(), &h);
        assert_eq!(r.status, "200 OK", "{}", r.body);
        let v: J = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["report"]["docsImported"], 3, "{v}");
        // catchup-unchanged-v1: imported lines have no frame; the import
        // moves `rev` for each conversation it wrote into.
        let locals: Vec<String> = v["report"]["conversations"]
            .as_array()
            .expect("conversations")
            .iter()
            .filter(|c| c["imported"].as_u64().unwrap_or(0) > 0)
            .map(|c| c["localConversationId"].as_str().expect("local").to_string())
            .collect();
        assert!(!locals.is_empty(), "{v}");
        let counts: Vec<usize> = locals.iter().map(|c| crate::overlay_ws::test_bumps::for_conversation(c).len()).collect();
        assert!(counts.iter().all(|n| *n == 1), "one bump per written conversation: {counts:?}");
        let again = handle(IMPORT, &Who::Owner, json!({"path": path, "to": dst}).to_string().as_bytes(), &HashMap::new(), &h);
        let v2: J = serde_json::from_str(&again.body).expect("json");
        assert_eq!(v2["report"]["docsImported"], 0, "re-import is a no-op: {v2}");
        let counts2: Vec<usize> = locals.iter().map(|c| crate::overlay_ws::test_bumps::for_conversation(c).len()).collect();
        assert_eq!(counts, counts2, "a no-op re-import moves nothing");
        let mut q = HashMap::new();
        q.insert("ws".to_string(), dst.clone());
        let l = handle(IMPORTS, &Who::Owner, b"", &q, &h);
        let lv: J = serde_json::from_str(&l.body).expect("json");
        assert!(!lv["imports"].as_array().expect("rows").is_empty(), "{lv}");
        // 0.46.1: each row says where to read it. dst has no pin yet, so its
        // main Thread waits as `import:<id>`: whole-Thread read-only.
        let row = &lv["imports"][0];
        assert_eq!(row["role"], "pending_canonical", "{lv}");
        let key = row["localConversationId"].as_str().expect("key").to_string();
        assert!(key.starts_with("import:"), "{lv}");
        assert_eq!(row["addr"], key.as_str(), "{lv}");
        assert_eq!(row["readOnly"], true, "{lv}");
        // Once dst has a pin the import settles onto it: the workspace
        // address opens it and the Thread is live (only its imported lines
        // are read-only).
        {
            let h2 = k2_core::db::shared();
            let conn = h2.lock();
            k2_core::db::schema::WorkspaceSession::upsert(&conn, &format!("ws-{dst}"), &dst, None, Some(&format!("conv-{dst}")), "claude", "system", "running")
                .expect("pin");
            t::settle_pending(&conn, &dst).expect("settle");
        }
        let l = handle(IMPORTS, &Who::Owner, b"", &q, &h);
        let lv: J = serde_json::from_str(&l.body).expect("json");
        let row = &lv["imports"][0];
        assert_eq!(row["role"], "canonical", "{lv}");
        assert_eq!(row["localConversationId"], format!("conv-{dst}").as_str(), "{lv}");
        assert_eq!(row["addr"], dst.as_str(), "{lv}");
        assert_eq!(row["readOnly"], false, "{lv}");
        // Outside clone-tmp is refused.
        let r = handle(IMPORT, &Who::Owner, json!({"path": "/etc/hosts", "to": dst}).to_string().as_bytes(), &HashMap::new(), &h);
        assert_eq!(r.status, "400 Bad Request", "{}", r.body);
        drop(home);
    }
}
