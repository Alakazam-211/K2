//! `/cli/app/sdk/*` in process (0.46.6): who may vendor, the workspace
//! jail, `--sew` with and without the box's kit, the kit door.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use k2_core::connect_users::Role;
use rusqlite::params;
use serde_json::{json, Value as J};

use super::routes::Who;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    home: crate::test_support::TempHome,
    ws: String,
    ws_path: PathBuf,
    other_path: PathBuf,
}

fn fx(tag: &str) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    k2_core::deploy::set_enabled(true);
    let root = dunce::canonicalize(home.path()).unwrap();
    let ws = format!("ws-sdk-{tag}");
    let ws_path = root.join("work").join(&ws);
    let other_path = root.join("work").join("other");
    std::fs::create_dir_all(ws_path.join("app")).unwrap();
    std::fs::create_dir_all(other_path.join("app")).unwrap();
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?1, ?2, ?1)",
            params![ws, ws_path.to_string_lossy().to_string()],
        )
        .unwrap();
    }
    Fx { _db: db, home, ws, ws_path, other_path }
}

fn agent(ws: &str) -> Who {
    Who::Agent { project_id: ws.into(), session_id: format!("sess-{ws}") }
}

fn set_build(ws: &str, on: bool) {
    let h = k2_core::db::shared();
    let conn = h.lock();
    conn.execute("UPDATE projects SET agents_can_manage_skin = ?2 WHERE id = ?1", params![ws, on as i64]).unwrap();
}

fn post(path: &str, who: &Who, body: J) -> (u16, J) {
    let r = super::routes::handle(path, who.clone(), body.to_string().as_bytes(), &HashMap::new());
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn get(path: &str, who: &Who, q: &[(&str, &str)]) -> (u16, J) {
    let p: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let r = super::routes::handle(path, who.clone(), b"", &p);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("{path}: not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error code: {v}"))
}

fn fixture_kit() -> Vec<u8> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../k2-root-proto/testdata/sew/sew-app-kit-0.0.2.tar.gz");
    std::fs::read(&p).unwrap_or_else(|e| panic!("{}: {e}", p.display()))
}

fn s(p: &Path) -> String {
    p.to_string_lossy().to_string()
}

#[test]
fn the_owner_vendors_k2_server_into_an_app_folder() {
    let f = fx("owner");
    let app = f.ws_path.join("app");
    let (st, v) = post("/cli/app/sdk/add", &Who::Owner, json!({"folder": s(&app)}));
    assert_eq!(st, 200, "{v}");
    let file = format!("k2-server-{}.tgz", env!("CARGO_PKG_VERSION"));
    assert_eq!(v["vendored"][0]["file"], format!("vendor/{file}"));
    assert_eq!(v["packageJson"], "created");
    assert_eq!(std::fs::read(app.join("vendor").join(&file)).unwrap(), k2_core::app_sdk::k2_server::tarball());
    // A rerun changes nothing.
    let (st, v) = post("/cli/app/sdk/add", &Who::Owner, json!({"folder": s(&app)}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["packageJson"], "unchanged");
    assert_eq!(v["vendored"][0]["written"], false);
    // Status: current against this box.
    let (st, v) = get("/cli/app/sdk/status", &Who::Owner, &[("folder", &s(&app))]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["packages"][0]["state"], "current");
    assert_eq!(v["box"]["k2Server"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn an_agent_needs_build_and_run_apps_and_stays_in_its_workspace() {
    let f = fx("agent");
    let me = agent(&f.ws);
    let app = f.ws_path.join("app");
    // Switch 1 off: refused (never held), nothing written.
    set_build(&f.ws, false);
    let (st, v) = post("/cli/app/sdk/add", &me, json!({"folder": s(&app)}));
    assert_eq!((st, code(&v)), (403, "apps_manage_off"), "{v}");
    assert!(!app.join("vendor").exists());
    let (st, v) = get("/cli/app/sdk/status", &me, &[("folder", &s(&app))]);
    assert_eq!((st, code(&v)), (403, "apps_manage_off"), "{v}");
    // Switch 1 on: its own folder works.
    set_build(&f.ws, true);
    let (st, v) = post("/cli/app/sdk/add", &me, json!({"folder": s(&app)}));
    assert_eq!(st, 200, "{v}");
    // A folder outside its workspace is refused, and untouched.
    let outside = f.other_path.join("app");
    let (st, v) = post("/cli/app/sdk/add", &me, json!({"folder": s(&outside)}));
    assert_eq!((st, code(&v)), (403, "folder_outside_workspace"), "{v}");
    assert!(!outside.join("vendor").exists() && !outside.join("package.json").exists());
    // A `..` escape is the same folder after canonicalizing: refused.
    let sneaky = f.ws_path.join("app/../../other/app");
    let (st, v) = post("/cli/app/sdk/add", &me, json!({"folder": s(&sneaky)}));
    assert_eq!((st, code(&v)), (403, "folder_outside_workspace"), "{v}");
    // A relative path is refused.
    let (st, v) = post("/cli/app/sdk/add", &me, json!({"folder": "app"}));
    assert_eq!(st, 400, "{v}");
}

#[test]
fn a_member_or_a_wrong_credential_is_refused() {
    let f = fx("member");
    let app = f.ws_path.join("app");
    let member = Who::Login { username: "mia".into(), role: Role::Member };
    let (st, v) = post("/cli/app/sdk/add", &member, json!({"folder": s(&app)}));
    assert_eq!(st, 403, "{v}");
    let (st, v) = post("/cli/app/sdk/add", &Who::Wrong, json!({"folder": s(&app)}));
    assert_eq!((st, code(&v)), (403, "wrong_credential"), "{v}");
    assert!(!app.join("vendor").exists());
    // An Admin login and a shell tab may (a human; folders anywhere).
    let admin = Who::Login { username: "ada".into(), role: Role::Admin };
    assert_eq!(post("/cli/app/sdk/add", &admin, json!({"folder": s(&app)})).0, 200);
    let shell = Who::Shell { project_id: f.ws.clone(), session_id: "sh".into() };
    assert_eq!(post("/cli/app/sdk/add", &shell, json!({"folder": s(&f.other_path.join("app"))})).0, 200);
}

#[test]
fn sew_needs_the_kit_on_this_box() {
    let f = fx("sew");
    let app = f.ws_path.join("app");
    // The test pin (0.0.2) has a kit pinned, but none is kept yet.
    let (st, v) = post("/cli/app/sdk/add", &Who::Owner, json!({"folder": s(&app), "sew": true}));
    assert_eq!((st, code(&v)), (409, "sew_kit_missing"), "{v}");
    assert!(v["error"]["hint"].as_str().unwrap().contains("k2 app sdk kit"), "{v}");
    assert!(!app.join("vendor").exists(), "nothing vendored on a refusal");

    // A tampered kit is refused at the door and not kept.
    let mut bad = fixture_kit();
    let n = bad.len();
    bad[n - 9] ^= 0xff;
    let dl = f.home.path().join("Downloads");
    std::fs::create_dir_all(&dl).unwrap();
    std::fs::write(dl.join("sew-app-kit-0.0.2.tar.gz"), &bad).unwrap();
    let (st, v) = post("/cli/app/sdk/kit", &Who::Owner, json!({"path": s(&dl.join("sew-app-kit-0.0.2.tar.gz"))}));
    assert_eq!((st, code(&v)), (409, "sew_kit_tampered"), "{v}");
    assert!(!k2_core::app_sdk::kit_path("0.0.2").exists());

    // The real fixture is kept; --sew then vendors the five packages.
    std::fs::write(dl.join("sew-app-kit-0.0.2.tar.gz"), fixture_kit()).unwrap();
    let (st, v) = post("/cli/app/sdk/kit", &Who::Owner, json!({"path": s(&dl.join("sew-app-kit-0.0.2.tar.gz"))}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["version"], "0.0.2");
    let (st, v) = post("/cli/app/sdk/add", &Who::Owner, json!({"folder": s(&app), "sew": true}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["sew"], "0.0.2");
    assert_eq!(v["vendored"].as_array().unwrap().len(), 6);
    assert_eq!(v["overrides"], json!(["@sew/client", "@sew/e2ee"]));
    assert!(v["install"].as_str().unwrap().starts_with("npm i livekit-client@2.22.3 react@^19 react-dom@^19"), "{v}");
    let pj: J = serde_json::from_str(&std::fs::read_to_string(app.join("package.json")).unwrap()).unwrap();
    assert_eq!(pj["dependencies"]["@sew/voice"], "file:vendor/sew-voice-0.0.2.tgz");
    assert_eq!(pj["overrides"]["@sew/client"], "file:vendor/sew-client-0.0.2.tgz");
    let (_, v) = get("/cli/app/sdk/status", &Who::Owner, &[("folder", &s(&app))]);
    assert_eq!(v["box"]["sewKit"], "0.0.2");
    assert!(v["packages"].as_array().unwrap().iter().all(|r| r["state"] == "current"), "{v}");

    // A kit kept on the box that no longer matches its pin is refused.
    let kept = k2_core::app_sdk::kit_path("0.0.2");
    let mut perm = std::fs::metadata(&kept).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    perm.set_readonly(false);
    std::fs::set_permissions(&kept, perm).unwrap();
    std::fs::write(&kept, &bad).unwrap();
    let (st, v) = post("/cli/app/sdk/add", &Who::Owner, json!({"folder": s(&app), "sew": true}));
    assert_eq!((st, code(&v)), (409, "sew_kit_tampered"), "{v}");
}

#[test]
fn the_kit_door_keeps_an_agent_in_its_workspace() {
    let f = fx("kitjail");
    let me = agent(&f.ws);
    set_build(&f.ws, true);
    std::fs::write(f.other_path.join("sew-app-kit-0.0.2.tar.gz"), fixture_kit()).unwrap();
    let (st, v) = post("/cli/app/sdk/kit", &me, json!({"path": s(&f.other_path.join("sew-app-kit-0.0.2.tar.gz"))}));
    assert_eq!((st, code(&v)), (403, "folder_outside_workspace"), "{v}");
    std::fs::write(f.ws_path.join("sew-app-kit-0.0.2.tar.gz"), fixture_kit()).unwrap();
    let (st, v) = post("/cli/app/sdk/kit", &me, json!({"path": s(&f.ws_path.join("sew-app-kit-0.0.2.tar.gz"))}));
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["kept"], true);
    assert!(v.get("path").is_none(), "no host path for a passport");
}
