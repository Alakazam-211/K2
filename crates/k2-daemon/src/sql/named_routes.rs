//! `GET|POST /cli/db/clusters` (0.46.11, prd-named-pg-clusters-v1): named
//! Postgres clusters.
//!
//! - `GET` (any caller the route policy lets in): every cluster K2 records
//!   (`k2 db clusters --json`), with `?health=1` each K2 cluster's health
//!   block (§8, what k2-watch reads). Never SQL on a foreign cluster.
//! - `POST {op: "create", major, name, port?, walArchive?, project}`:
//!   `k2 db cluster create <M> --name <n>`.
//! - `POST {op: "restart", name, project}`: `k2 db cluster restart <n>`.
//! - `POST {op: "create-db", cluster, name?, id?, major?, project}`:
//!   `k2 db create --cluster <n>`.
//!
//! Who: the owner (token or Owner login) for any workspace, or a local,
//! non-sandboxed agent with "Allow this agent to manage Postgres versions"
//! for its own workspace's clusters (`versions_access::decide`, §6). An
//! agent's `create-db` also needs `db_agent_access = write`, as `k2 db
//! create` does.

use std::collections::HashMap;

use serde_json::{json, Value};

use crate::cli_response::CliResponse;
use crate::sql::cluster::{self, Cluster};
use crate::sql::sysops::SystemOps;
use crate::sql::versions_access::{self, VersionsAct, Who};

fn err(status: &'static str, code: &str, hint: impl Into<String>) -> CliResponse {
    CliResponse {
        status,
        content_type: "application/json",
        body: json!({ "ok": false, "error": { "code": code, "hint": hint.into() } }).to_string(),
    }
}

fn status_of(code: u16) -> &'static str {
    match code {
        400 => "400 Bad Request",
        403 => "403 Forbidden",
        404 => "404 Not Found",
        _ => "409 Conflict",
    }
}

fn ensure_err(e: cluster::EnsureError) -> CliResponse {
    err(e.status(), e.code(), e.hint())
}

#[derive(Debug, Default, serde::Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct Body {
    op: String,
    project: String,
    major: Option<u32>,
    name: Option<String>,
    port: Option<u16>,
    wal_archive: bool,
    cluster: Option<String>,
    id: Option<String>,
}

/// The workspace the caller acts for: an agent's own, or (the owner) the
/// one the body names.
fn workspace_for(who: &Who, claim: &str) -> Result<(String, String), CliResponse> {
    match who {
        Who::Agent { project_id, .. } => {
            let path = {
                let db = k2_core::db::shared();
                let conn = db.lock();
                k2_core::db::schema::Project::get(&conn, project_id).map(|p| p.path)
            }
            .map_err(|_| err("404 Not Found", "not_found", "the agent's workspace isn't registered"))?;
            Ok((path, project_id.clone()))
        }
        _ => crate::sql::identity::resolve_caller(claim),
    }
}

fn gate(who: &Who, act: VersionsAct, project: &str) -> Result<(), CliResponse> {
    if matches!(who, Who::Member { .. } | Who::Other) {
        return Err(err(
            "403 Forbidden",
            versions_access::OWNER_ONLY,
            "named Postgres clusters are the owner's, or an agent's with \"Allow this agent to manage Postgres versions\" for its own workspace",
        ));
    }
    let switch = match who {
        Who::Agent { project_id, .. } => versions_access::switch_on(project_id),
        _ => false,
    };
    versions_access::decide(who, act, Some(project), false, switch).map_err(|r| err(status_of(r.status), r.code, r.hint))
}

/// `k2 db clusters [--health]`: the records (after the helper's view when
/// it can be read), each K2 one with its health when asked.
pub fn list(ops: &dyn SystemOps, health: bool) -> Value {
    let reconciled = cluster::reconcile(ops);
    let clusters = match &reconciled {
        Ok(v) => v.clone(),
        Err(_) => cluster::list(),
    };
    let rows: Vec<Value> = clusters
        .iter()
        .map(|c| {
            let mut v = c.json();
            v["label"] = json!(c.label());
            v["named"] = json!(c.is_named());
            if health && c.holds_k2_databases() {
                v["health"] = cluster::health_of(ops, c);
            }
            v
        })
        .collect();
    let mut out = json!({ "ok": true, "clusters": rows });
    if let Err(e) = reconciled {
        out["helperView"] = json!(e);
    }
    out
}

/// `GET|POST /cli/db/clusters`.
pub fn handle(ops: &dyn SystemOps, who: Who, is_post: bool, body: &[u8], params: &HashMap<String, String>) -> CliResponse {
    if !crate::sql::sql_supported() && !cfg!(test) {
        return err("409 Conflict", "unsupported", "Postgres runs on Linux deployments only");
    }
    if !is_post {
        if matches!(who, Who::Other) {
            return err("403 Forbidden", "forbidden", "reading K2's Postgres clusters needs the owner, a login or an agent passport");
        }
        let health = params.get("health").is_some_and(|v| v == "1" || v == "true");
        return CliResponse::ok_json(list(ops, health).to_string());
    }
    if matches!(who, Who::Member { .. } | Who::Other) {
        return err(
            "403 Forbidden",
            versions_access::OWNER_ONLY,
            "named Postgres clusters are the owner's, or an agent's with \"Allow this agent to manage Postgres versions\" for its own workspace",
        );
    }
    let b: Body = match serde_json::from_slice(body) {
        Ok(b) => b,
        Err(e) => return err("400 Bad Request", "usage", format!("invalid JSON body: {e}")),
    };
    let (path, project) = match workspace_for(&who, &b.project) {
        Ok(v) => v,
        Err(r) => return r,
    };
    match b.op.as_str() {
        "create" => {
            if let Err(r) = gate(&who, VersionsAct::NamedClusterCreate, &project) {
                return r;
            }
            let (Some(major), Some(name)) = (b.major, b.name.as_deref()) else {
                return err("400 Bad Request", "usage", "k2 db cluster create <major> --name <name> [--port <p>] [--wal-archive]");
            };
            match cluster::ensure_named_cluster(ops, major, name.trim(), b.port, b.wal_archive, &project) {
                Ok(c) => {
                    audit(&who, "cluster-create", &c);
                    CliResponse::ok_json(json!({ "ok": true, "cluster": cluster_answer(&c) }).to_string())
                }
                Err(e) => ensure_err(e),
            }
        }
        "restart" => {
            let Some(spec) = b.name.as_deref().or(b.cluster.as_deref()) else {
                return err("400 Bad Request", "usage", "k2 db cluster restart <name>");
            };
            let owner = matches!(who, Who::Owner);
            let c = match cluster::named_target(spec.trim(), (!owner).then_some(project.as_str())) {
                Ok(c) => c,
                Err(e) => return ensure_err(e),
            };
            if let Err(r) = gate(&who, VersionsAct::NamedClusterRestart, c.owner_project.as_deref().unwrap_or(&project)) {
                return r;
            }
            match cluster::restart_named(ops, &c) {
                Ok(v) => {
                    audit(&who, "cluster-restart", &c);
                    CliResponse::ok_json(json!({ "ok": true, "cluster": c.label(), "restart": v }).to_string())
                }
                Err(e) => ensure_err(e),
            }
        }
        "create-db" => {
            let Some(spec) = b.cluster.as_deref().map(str::trim).filter(|s| !s.is_empty()) else {
                return err("400 Bad Request", "usage", "k2 db create --cluster <name> [--name <db>] [--id <id>]");
            };
            if let Err(r) = gate(&who, VersionsAct::CreateOnNamedCluster, &project) {
                return r;
            }
            if matches!(who, Who::Agent { .. }) {
                let mode = k2_core::workspace::settings::db_agent_access_for_path(&path);
                if mode != "write" {
                    return err(
                        "403 Forbidden",
                        "forbidden",
                        format!("workspace db_agent_access is '{mode}' (need write to create new databases) — ask your human"),
                    );
                }
            }
            let secrets = crate::sql::secrets::FileSecretStore::default();
            let cap = k2_core::workspace::settings::db_active_cap_for_path(&path);
            match crate::sql::ops::create_database_in_cluster(
                ops,
                &secrets,
                &project,
                cap,
                b.id.as_deref(),
                b.name.as_deref(),
                b.major,
                spec,
            ) {
                Ok(v) => CliResponse::ok_json(crate::sql::routes::without_passwords_pub(v).to_string()),
                Err(e) => err(e.status(), e.code(), e.hint()),
            }
        }
        other => err("400 Bad Request", "usage", format!("unknown op {other:?} (create, restart, create-db)")),
    }
}

fn cluster_answer(c: &Cluster) -> Value {
    let mut v = c.json();
    v["label"] = json!(c.label());
    v["named"] = json!(true);
    if c.wal_archive {
        v["walSpool"] = json!(k2_root_proto::pg_named::wal_spool(c.major, &c.name));
    }
    v
}

fn audit(who: &Who, what: &str, c: &Cluster) {
    eprintln!("[db] {what} {} (port {}) by {}", c.label(), c.port, who.label());
}
