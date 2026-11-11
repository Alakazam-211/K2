//! Save as template, add and remove through the `/cli` doors (T6): who may
//! call them, the contract's error codes, POST-only. Real shared test DB,
//! temp HOME and hire root. Secrets here are fake, secret-shaped strings.

use std::collections::HashMap;
use std::path::Path;

use serde_json::{json, Value};

use crate::compute_routes::Who;
use crate::hire_routes::handle_cli;
use crate::hire_routes::test_support::ensure_test_root;
use k2_core::connect_users::Role;

use super::{CLI_ADD, CLI_DIFF, CLI_REMOVE, CLI_SAVE, CLI_UPGRADE};

fn uniq(prefix: &str) -> String {
    format!("{prefix}-{}", &uuid::Uuid::new_v4().simple().to_string()[..10])
}

fn code(s: &str) -> u16 {
    s.split_whitespace().next().and_then(|n| n.parse().ok()).expect("status")
}

fn post(path: &str, who: Who, body: Value) -> (u16, Value) {
    let r = handle_cli(path, who, true, body.to_string().as_bytes(), &HashMap::new(), None);
    (code(r.status), serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("json {e}: {}", r.body)))
}

/// A writer agent hired by the owner: `(workspace id, path, handle)`.
fn hire() -> (String, String, String) {
    ensure_test_root();
    let (st, out) = post(crate::hire_routes::CLI_HIRE, Who::Owner, json!({ "template": "writer", "name": uniq("Max") }));
    assert_eq!(st, 200, "{out}");
    (out["id"].as_str().unwrap().to_string(), out["path"].as_str().unwrap().to_string(), out["handle"].as_str().unwrap().to_string())
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.to_string(), session_id: "s".into() }
}

#[test]
fn template_save_route_follows_the_contract() {
    let _home = k2_core::test_env::TempHome::new();
    let (ws, path, handle) = hire();
    let id = uniq("my-writer");

    // A dry run: the review body, nothing written.
    let (st, plan) = post(CLI_SAVE, Who::Owner, json!({ "workspace": path, "id": id, "dryRun": true }));
    assert_eq!(st, 200, "{plan}");
    assert_eq!((plan["ok"].as_bool(), plan["dryRun"].as_bool(), plan["version"].as_u64()), (Some(true), Some(true), Some(1)));
    assert!(plan["author"].as_str().unwrap().starts_with("user:"), "{plan}");
    assert!(plan["hash"].as_str().unwrap().starts_with("sha256:"), "{plan}");
    assert_eq!(plan["files"][0]["path"], "template.toml");
    assert!(plan["files"][0]["bytes"].as_u64().unwrap() > 0);
    for k in ["catalog", "suggest", "heartbeats", "skipped"] {
        assert!(plan[k].is_array(), "{k}: {plan}");
    }
    assert!(!k2_core::hire::template::library_dir().join(&id).exists());

    // The save by handle; then the same author again is v2.
    let (st, v1) = post(CLI_SAVE, Who::Owner, json!({ "workspace": handle, "id": id, "name": "My writer", "summary": "Writes posts." }));
    assert_eq!((st, v1["version"].as_u64()), (200, Some(1)), "{v1}");
    let (_, v2) = post(CLI_SAVE, Who::Owner, json!({ "workspace": path, "id": id }));
    assert_eq!(v2["version"], 2, "{v2}");

    // The agent saves its own workspace, as agent:<handle>; another author's id is taken.
    let (st, e) = post(CLI_SAVE, agent(&ws), json!({ "workspace": path, "id": id }));
    assert_eq!((st, e["error"]["code"].as_str()), (409, Some("id_taken")), "{e}");
    let mine = uniq("agent-made");
    let (st, out) = post(CLI_SAVE, agent(&ws), json!({ "workspace": path, "id": mine }));
    assert_eq!(st, 200, "{out}");
    assert_eq!(out["author"], format!("agent:{}", crate::compute_routes::ws_handle(&ws)));
    // …but never another workspace.
    let (other_ws, _, _) = hire();
    let (st, e) = post(CLI_SAVE, agent(&other_ws), json!({ "workspace": path, "id": uniq("x") }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("own_workspace_only")), "{e}");

    // The error table.
    let cases = [
        (json!({ "workspace": path, "id": "Not A Slug" }), 400, "bad_id"),
        (json!({ "workspace": path, "id": "writer" }), 409, "id_reserved"),
        (json!({ "workspace": path, "id": "worker" }), 409, "id_reserved"),
        (json!({ "workspace": "/no/such/agent", "id": uniq("x") }), 404, "no_workspace"),
    ];
    for (body, want_st, want_code) in cases {
        let (st, e) = post(CLI_SAVE, Who::Owner, body.clone());
        assert_eq!((st, e["error"]["code"].as_str()), (want_st, Some(want_code)), "{body}: {e}");
        assert!(e["error"]["message"].is_string() && e["error"]["hint"].is_string(), "{e}");
    }

    // A secret: 422 naming the file and line, never the secret.
    let fake = format!("sk-ant-api03-{}", "FAKEfake12".repeat(3));
    std::fs::write(Path::new(&path).join(".k2/wiki/Keys.md"), format!("# Keys\n\n{fake}\n")).unwrap();
    let (st, e) = post(CLI_SAVE, Who::Owner, json!({ "workspace": path, "id": uniq("x"), "wiki": ["Keys"] }));
    assert_eq!((st, e["error"]["code"].as_str()), (422, Some("secret_found")), "{e}");
    assert_eq!((e["error"]["file"].as_str(), e["error"]["line"].as_u64()), (Some(".k2/wiki/Keys.md"), Some(3)), "{e}");
    assert!(!e.to_string().contains(&fake));

    // A Member login is refused; GET is 405.
    let member = Who::Login { username: "pat".into(), role: Role::Member };
    let (st, e) = post(CLI_SAVE, member, json!({ "workspace": path, "id": uniq("x") }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("role_required")), "{e}");
    let admin = Who::Login { username: "ada".into(), role: Role::Admin };
    let (st, out) = post(CLI_SAVE, admin, json!({ "workspace": path, "id": uniq("ada"), "dryRun": true }));
    assert_eq!((st, out["author"].as_str()), (200, Some("user:ada")), "{out}");
    for p in [CLI_SAVE, CLI_ADD, CLI_REMOVE, CLI_UPGRADE, CLI_DIFF] {
        let r = handle_cli(p, Who::Owner, false, b"", &HashMap::new(), None);
        assert_eq!(r.status, "405 Method Not Allowed", "{p}");
        assert!(crate::routes::route_policy::post_allowed(p), "{p} is in route_policy as POST");
    }

    // Remove: an agent only what it saved; a person any; built-ins never.
    let (st, e) = post(CLI_REMOVE, agent(&ws), json!({ "id": id }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("not_author")), "{e}");
    let (st, out) = post(CLI_REMOVE, agent(&ws), json!({ "id": mine }));
    assert_eq!(st, 200, "{out}");
    let (st, out) = post(CLI_REMOVE, Who::Owner, json!({ "id": id, "version": 1 }));
    assert_eq!((st, out["remaining"].clone()), (200, json!([2])), "{out}");
    let (st, e) = post(CLI_REMOVE, Who::Owner, json!({ "id": "writer" }));
    assert_eq!((st, e["error"]["code"].as_str()), (409, Some("id_reserved")), "{e}");
}

#[test]
fn template_add_route_holds_an_agent_to_its_workspace() {
    let _home = k2_core::test_env::TempHome::new();
    let (ws, path, _) = hire();
    let id = uniq("from-folder");
    let make = |dir: &Path| {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("template.toml"),
            format!("schema = 1\nid = \"{id}\"\nversion = 1\nname = \"Folder\"\nsummary = \"From a folder.\"\nauthor = \"user:sam\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("ROLE.md"), "You are {{name}}.\n").unwrap();
    };
    // Outside its workspace: refused for an agent.
    let outside = k2_core::test_env::stable_temp_dir().join(uniq("tpl"));
    make(&outside);
    let (st, e) = post(CLI_ADD, agent(&ws), json!({ "dir": outside }));
    assert_eq!((st, e["error"]["code"].as_str()), (403, Some("own_workspace_only")), "{e}");
    // Inside: the agent is the author.
    let inside = Path::new(&path).join("tpl");
    make(&inside);
    let (st, out) = post(CLI_ADD, agent(&ws), json!({ "dir": inside }));
    assert_eq!(st, 200, "{out}");
    assert!(out["author"].as_str().unwrap().starts_with("agent:"), "{out}");
    // A person adds from anywhere; another author's id is taken.
    let (st, e) = post(CLI_ADD, Who::Owner, json!({ "dir": outside }));
    assert_eq!((st, e["error"]["code"].as_str()), (409, Some("id_taken")), "{e}");
    let (st, e) = post(CLI_ADD, Who::Owner, json!({ "dir": "relative/dir" }));
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("usage")), "{e}");
    let _ = std::fs::remove_dir_all(&outside);
}

/// T5 through the doors: upgrade and diff, an agent on its own workspace
/// only, a workspace with no template, two library versions diffed.
#[test]
fn template_upgrade_and_diff_routes() {
    let _home = k2_core::test_env::TempHome::new();
    let (ws, path, _) = hire();
    // A current built-in: nothing to do.
    let (st, r) = post(CLI_UPGRADE, Who::Owner, json!({ "workspace": path, "dryRun": true }));
    assert_eq!(st, 200, "{r}");
    assert_eq!((r["dryRun"].as_bool(), r["changed"].as_bool(), r["from"].clone()), (Some(true), Some(false), r["to"].clone()), "{r}");
    let (st, r) = post(CLI_UPGRADE, agent(&ws), json!({ "workspace": path }));
    assert_eq!(st, 200, "an agent upgrades its own workspace: {r}");
    let (st, d) = post(CLI_DIFF, agent(&ws), json!({ "workspace": path }));
    assert_eq!((st, d["id"].as_str()), (200, Some("writer")), "{d}");
    let (other, _, _) = hire();
    for p in [CLI_UPGRADE, CLI_DIFF] {
        let (st, e) = post(p, agent(&other), json!({ "workspace": path }));
        assert_eq!((st, e["error"]["code"].as_str()), (403, Some("own_workspace_only")), "{p}: {e}");
    }
    // A workspace no template made.
    let plain = ensure_test_root().join(uniq("plain"));
    let (st, out) = post(crate::hire_routes::CLI_HIRE, Who::Owner, json!({ "path": plain }));
    assert_eq!(st, 200, "{out}");
    let (st, e) = post(CLI_UPGRADE, Who::Owner, json!({ "workspace": plain }));
    assert_eq!((st, e["error"]["code"].as_str()), (404, Some("no_template")), "{e}");
    // Two library versions.
    let id = uniq("two");
    for _ in 0..2 {
        let (st, out) = post(CLI_SAVE, Who::Owner, json!({ "workspace": path, "id": id }));
        assert_eq!(st, 200, "{out}");
    }
    let (st, d) = post(CLI_DIFF, Who::Owner, json!({ "id": id, "from": 1, "to": "v2" }));
    assert_eq!(st, 200, "{d}");
    assert!(d["files"].as_array().unwrap().iter().any(|f| f["path"] == "template.toml" && f["status"] == "changed"), "{d}");
    let (st, e) = post(CLI_DIFF, Who::Owner, json!({ "id": id }));
    assert_eq!((st, e["error"]["code"].as_str()), (400, Some("usage")), "{e}");
}
