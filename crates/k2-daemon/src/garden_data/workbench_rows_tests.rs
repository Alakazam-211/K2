//! The Workbench's Garden rows (catalog v40), through
//! `zen_data_routes::handle` so the door's gates run too: a temp HOME with
//! one Garden placing `board`, a workspace that is a real git repo, and a
//! sidecar working in its own worktree (`.worktrees/k2-fix`, branch
//! `k2/fix`, one commit ahead of `main` plus an uncommitted change).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use k2_core::garden_data::authors::{self, CallerKind, StampSource};
use k2_core::garden_data::MadeBy;
use k2_core::zen::{ZenFile, ZenFiles};

use crate::garden_data::agent_routes::tests::{env, Env};
use crate::zen_data_routes::{handle, DataWho};

const APP: &str = "POST /cli/zen/call HTTP/1.1\r\nHost: 127.0.0.1\r\nOrigin: tauri://localhost\r\n\r\n";

const PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n";

const ALL: [&str; 6] = ["agents:read", "git:read", "git:notes", "compute:read", "agents:start", "agents:stop"];

const SIDECAR: &str = "wb-room/fix::local";

struct Bench {
    _env: Env,
    f: ZenFiles,
    garden: String,
    repo: PathBuf,
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "user.name=Bench", "-c", "user.email=bench@example.com", "-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"])
        .args(args)
        .output()
        .expect("git runs");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn manifest(caps: &[&str], tag: &str) -> String {
    let caps: Vec<String> = caps.iter().map(|c| format!("\"{c}\"")).collect();
    format!("{{\n  \"schema\": 1,\n  \"name\": \"Bench {tag}\",\n  \"entry\": \"index.html\",\n  \"caps\": [{}]\n}}\n", caps.join(", "))
}

fn bench(caps: &[&str]) -> Bench {
    let e = env();
    let f = ZenFiles::local();
    f.setup().expect("zen setup");
    f.new_widget("board", Some("hello")).expect("widget folder");
    let garden = f.gardens()[0].id.clone();
    std::fs::write(f.path_of(&ZenFile::Garden(garden.clone())), PAGE).expect("garden page");

    let repo = k2_core::paths::k2_home().join("wb-room");
    std::fs::create_dir_all(&repo).expect("repo dir");
    git(&repo, &["init", "-q", "-b", "main"]);
    std::fs::write(repo.join("b.txt"), "one\ntwo\n").expect("b");
    std::fs::write(repo.join(".gitignore"), ".worktrees/\n").expect("ignore");
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "base"]);
    let repo = dunce::canonicalize(&repo).expect("canonical");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES ('w-wb', 'Workbench room', ?1, 'wb-room')",
            rusqlite::params![repo.to_string_lossy()],
        )
        .expect("workspace");
    }
    // A worktree sidecar as W1's `sidecar new --worktree` leaves it, minus
    // the process: W1's own worktree, workspaces row, name and 0251 stamp.
    let c = k2_core::worktree_task::create(&repo, "fix", None).expect("W1 makes the worktree");
    let wt = std::path::PathBuf::from(&c.path);
    std::fs::write(wt.join("a.txt"), "hello <img src=x onerror=alert(1)>\n").expect("a");
    git(&wt, &["add", "a.txt"]);
    git(&wt, &["commit", "-q", "-m", "Add a greeting"]);
    std::fs::write(wt.join("b.txt"), "one\nTWO\n").expect("b changed");
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Workspace::create(&conn, "wsrow-wb-fix", "w-wb", None, "worktree", Some(&c.branch), &c.branch, 5, Some(&c.path))
            .expect("workspaces row");
        conn.execute(
            "INSERT INTO workspace_tab_sessions (project_id, pane_group_id, agent_name, session_id, command, args_json, cwd, last_seen_at, created_by) \
             VALUES ('w-wb', 'pane-wb-fix', 'tab-pane-wb-fix', 'conv-wb-fix', 'claude', '[]', ?1, unixepoch(), 'owner')",
            rusqlite::params![c.path],
        )
        .expect("sidecar tab row");
        k2_core::sidecar::bind_name(&conn, "w-wb", "claude", "conv-wb-fix", "fix", "fix").expect("bind name");
        k2_core::worktree_task::store::stamp_new(&conn, "w-wb", "pane-wb-fix", "wsrow-wb-fix", &c.branch, &c.dir, c.base.onto.as_deref())
            .expect("0251 stamp");
    }
    let b = Bench { _env: e, f, garden, repo };
    b.set(caps);
    b
}

impl Bench {
    fn hash(&self) -> String {
        let h = self.f.garden_widget_status(&self.garden, "board").hash().to_string();
        assert_eq!(h.len(), 64, "board bundles clean");
        h
    }

    fn set(&self, caps: &[&str]) {
        let tag = uuid::Uuid::new_v4().simple().to_string();
        std::fs::write(self.f.widget_dir("board").join("manifest.json"), manifest(caps, &tag[..8])).expect("manifest");
        let index = self.f.widget_dir("board").join("index.html");
        let html = std::fs::read_to_string(&index).expect("index.html");
        std::fs::write(&index, format!("{html}\n<!-- version {tag} -->\n")).expect("index.html");
        let db = k2_core::db::shared();
        let conn = db.lock();
        authors::stamp(&conn, "board", &self.hash(), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 1).expect("stamp");
    }

    fn raw(&self, verb: &str, args: Value, token: Option<&str>) -> (u16, Value) {
        let mut body = json!({ "garden": self.garden, "placement": "b1", "hash": self.hash(), "verb": verb, "args": args });
        if let Some(t) = token {
            body["confirmToken"] = json!(t);
        }
        let r = handle("/cli/zen/call", DataWho::Owner, body.to_string().as_bytes(), &HashMap::new(), APP);
        let code: u16 = r.status.split(' ').next().expect("status").parse().expect("code");
        let text = r.body.clone();
        assert!(!text.contains(&*self.repo.to_string_lossy()), "{verb}: a host path in a Garden answer: {text}");
        (code, serde_json::from_str(&text).unwrap_or_else(|e| panic!("{verb}: {e}: {text}")))
    }

    fn call(&self, verb: &str, args: Value) -> Value {
        let (code, v) = self.raw(verb, args, None);
        assert_eq!(code, 200, "{verb}: {v}");
        assert_eq!(v["ok"], true, "{v}");
        no_banned_fields(&v["result"], verb);
        v["result"].clone()
    }

    fn refuse(&self, verb: &str, args: Value) -> (String, Value) {
        let (code, v) = self.raw(verb, args, None);
        assert_ne!(code, 200, "{verb} should refuse: {v}");
        let c = v["error"]["code"].as_str().unwrap_or_else(|| panic!("{verb}: no error code: {v}")).to_string();
        (c, v["error"].clone())
    }
}

fn no_banned_fields(v: &Value, at: &str) {
    match v {
        Value::Object(o) => {
            for (k, x) in o {
                assert!(!k2_core::contract::catalog().banned_fields.iter().any(|b| b == k), "{at}: banned field {k}: {v}");
                no_banned_fields(x, &format!("{at}.{k}"));
            }
        }
        Value::Array(a) => a.iter().for_each(|x| no_banned_fields(x, at)),
        _ => {}
    }
}

#[test]
fn each_row_needs_its_caps() {
    let b = bench(&["store:read"]);
    for (verb, args) in [
        ("work.list", json!([])),
        ("git.snapshot", json!([{ "address": SIDECAR }])),
        ("git.diffFile", json!([{ "address": SIDECAR, "file": "a.txt" }])),
        ("git.log", json!([{ "address": SIDECAR }])),
        ("compute.jobs", json!([{ "address": SIDECAR }])),
        ("git.note", json!([{ "address": SIDECAR, "notes": [{ "file": "a.txt", "body": "x" }] }])),
        ("agents.stop", json!([{ "address": SIDECAR }])),
        ("git.land", json!([{ "address": SIDECAR }])),
        ("agents.settle", json!([{ "address": SIDECAR }])),
        ("agents.start", json!([{ "workspace": "wb-room::local", "name": "x", "brief": "y" }])),
    ] {
        assert_eq!(b.refuse(verb, args).0, "cap_not_granted", "{verb}");
    }
    // Land and settle need git:read too (alsoCaps).
    b.set(&["agents:start"]);
    assert_eq!(b.refuse("git.land", json!([{ "address": SIDECAR }])).0, "cap_not_granted");
}

#[test]
fn reads_answer_a_worktree_agent_in_relative_paths() {
    let b = bench(&ALL);
    let rows = b.call("work.list", json!([]));
    let row = rows.as_array().expect("rows").iter().find(|r| r["address"] == SIDECAR).unwrap_or_else(|| panic!("no fix row: {rows}"));
    assert_eq!(row["thread"], "wb-room/fix");
    assert_eq!(row["workspace"], "wb-room");
    assert_eq!(row["workspaceLabel"], "Workbench room");
    assert_eq!(row["worktree"], ".worktrees/k2-fix");
    assert_eq!(row["branch"], "k2/fix");
    assert_eq!(row["base"], "main");
    assert_eq!(row["ahead"], 1);
    assert_eq!(row["dirty"], 1, "b.txt is changed and not committed");
    assert_eq!(row["live"], false);
    assert_eq!(row["taskState"], "ready", "W1: a commit not on main → review → ready");

    let s = b.call("git.snapshot", json!([{ "address": SIDECAR }]));
    assert_eq!(s["branch"], "k2/fix");
    assert_eq!(s["base"]["ref"], "main");
    assert_eq!(s["base"]["ahead"], 1);
    let files: Vec<(String, String)> = s["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| (f["file"].as_str().expect("file").to_string(), f["group"].as_str().expect("group").to_string()))
        .collect();
    assert!(files.contains(&("a.txt".into(), "base".into())), "{files:?}");
    assert!(files.iter().any(|(f, g)| f == "b.txt" && g != "base"), "{files:?}");

    let d = b.call("git.diffFile", json!([{ "address": SIDECAR, "file": "a.txt", "base": "main" }]));
    assert_eq!(d["old"]["exists"], false);
    assert_eq!(d["new"]["text"], "hello <img src=x onerror=alert(1)>\n", "text crosses as data");
    let d = b.call("git.diffFile", json!([{ "address": SIDECAR, "file": "b.txt" }]));
    assert_eq!(d["old"]["text"], "one\ntwo\n");
    assert_eq!(d["new"]["text"], "one\nTWO\n");

    let log = b.call("git.log", json!([{ "address": SIDECAR }]));
    assert_eq!(log["base"], "main");
    assert_eq!(log["ahead"], 1);
    assert_eq!(log["commits"][0]["subject"], "Add a greeting");
    assert_eq!(log["commits"].as_array().expect("commits").len(), 1);

    assert_eq!(b.call("compute.jobs", json!([{ "address": SIDECAR }])), json!([]));

    // The workspace's main agent works on main: nothing ahead.
    let main = b.call("git.log", json!([{ "address": "wb-room::local" }]));
    assert_eq!(main["ahead"], 0);
}

#[test]
fn only_this_computers_agents_and_plain_refs() {
    let b = bench(&ALL);
    for address in ["nowhere::local", "wb-room/nope::local", "wb-room::other.k2.dev", "wb-room/fix/x::local"] {
        assert_eq!(b.refuse("git.snapshot", json!([{ "address": address }])).0, "not_bound", "{address}");
    }
    for base in ["--upload-pack=touch /tmp/x", "main..HEAD", "a b"] {
        assert_eq!(b.refuse("git.log", json!([{ "address": SIDECAR, "base": base }])).0, "bad_args", "{base}");
    }
    assert_eq!(b.refuse("git.snapshot", json!([{ "address": SIDECAR, "path": "/" }])).0, "bad_request", "unknown argument");
    assert_eq!(b.refuse("git.diffFile", json!([{ "address": SIDECAR, "file": "../../etc/passwd" }])).0, "bad_args");
}

#[test]
fn land_settle_and_start_ask_k2s_sheet_once_then_run_w1s_routes() {
    let b = bench(&ALL);
    let args = json!([{ "address": SIDECAR }]);
    let (code, err) = b.refuse("git.land", args.clone());
    assert_eq!(code, "confirm_required");
    assert_eq!(err["change"]["kind"], "land");
    assert_eq!(err["change"]["branch"], "k2/fix");
    assert_eq!(err["change"]["base"], "main");
    assert_eq!(err["change"]["commits"][0]["subject"], "Add a greeting");
    assert_eq!(err["change"]["files"], json!([{ "file": "a.txt", "status": "A" }]));
    assert_eq!(err["change"]["uncommitted"], 1, "b.txt stays in the worktree");
    let token = err["secretId"].as_str().expect("secretId").to_string();

    // Another address can't use this answer, and a made-up one is declined.
    let (_, other) = b.raw("git.land", json!([{ "address": "wb-room::local" }]), Some(&token));
    assert_eq!(other["error"]["code"], "bad_args", "the main agent has nothing to land: {other}");
    let (_, forged) = b.raw("git.land", args.clone(), Some("not-a-token"));
    assert_eq!(forged["error"]["code"], "declined");
    // The person's press on the sheet: W1 cherry-picks onto main.
    let (c, v) = b.raw("git.land", args.clone(), Some(&token));
    assert_eq!(c, 200, "{v}");
    assert_eq!(v["result"]["landed"], true, "{v}");
    assert_eq!(v["result"]["commits"], 1);
    assert_eq!(v["result"]["cancelled"], false);
    assert!(b.repo.join("a.txt").is_file(), "a.txt is on main now");
    // One answer, once.
    let (_, again) = b.raw("git.land", args.clone(), Some(&token));
    assert_eq!(again["error"]["code"], "declined");

    // Land's auto-retire refused (b.txt is uncommitted): Retire refuses too,
    // Discard asks K2's sheet, then W1 retires it with discard.
    assert_eq!(b.refuse("agents.settle", json!([{ "address": SIDECAR }])).0, "conflict");
    let (code, err) = b.refuse("agents.settle", json!([{ "address": SIDECAR, "discard": true }]));
    assert_eq!(code, "confirm_required");
    assert_eq!(err["change"]["kind"], "settle");
    assert_eq!(err["change"]["uncommitted"], 1);
    // A token for discard isn't one for retire.
    let (_, v) = b.raw("agents.settle", json!([{ "address": SIDECAR }]), err["secretId"].as_str());
    assert_eq!(v["error"]["code"], "declined");
    let (code, err) = b.refuse("agents.settle", json!([{ "address": SIDECAR, "discard": true }]));
    assert_eq!(code, "confirm_required");
    let (c, v) = b.raw("agents.settle", json!([{ "address": SIDECAR, "discard": true }]), err["secretId"].as_str());
    assert_eq!(c, 200, "{v}");
    assert_eq!(v["result"], json!({ "settled": true, "discarded": true, "cancelled": false }));
    assert!(!b.repo.join(".worktrees/k2-fix").exists(), "the worktree went to the Trash");
    let row = b.call("work.list", json!([])).as_array().expect("rows").iter().find(|r| r["address"] == SIDECAR).cloned().expect("row stays");
    assert_eq!(row["taskState"], "retired", "{row}");

    let start = json!([{ "workspace": "wb-room::local", "name": "Fix Login", "brief": "Make login keep the cookie." }]);
    let (code, err) = b.refuse("agents.start", start);
    assert_eq!(code, "confirm_required");
    assert_eq!(err["change"]["name"], "fix-login");
    assert_eq!(err["change"]["branch"], "k2/fix-login");
    assert_eq!(err["change"]["base"], "main");
    assert_eq!(b.refuse("agents.start", json!([{ "workspace": SIDECAR, "name": "x", "brief": "y" }])).0, "bad_args");
}

#[test]
fn stop_reaches_w1s_interrupt() {
    let b = bench(&ALL);
    // Asleep: W1 answers interrupted false, the process isn't touched.
    assert_eq!(b.call("agents.stop", json!([{ "address": SIDECAR }])), json!({ "stopped": false }));
    assert_eq!(b.refuse("agents.stop", json!([{ "address": "wb-room::local" }])).0, "bad_args");
}

#[test]
fn notes_land_on_the_worktree_and_go_to_the_sidecars_own_thread() {
    let b = bench(&ALL);
    let v = b.call("git.note", json!([{ "address": SIDECAR, "notes": [{ "file": "a.txt", "line": 1, "body": "Keep it short" }], "send": false }]));
    assert_eq!(v, json!({ "added": 1, "sent": 0 }));
    assert_eq!(b.refuse("git.note", json!([{ "address": SIDECAR, "notes": [] }])).0, "bad_args");
    assert_eq!(b.refuse("git.note", json!([{ "address": SIDECAR, "notes": [{ "file": "a.txt", "line": 0, "body": "x" }] }])).0, "bad_args");

    // `/cli/git/notes/send` with `to`: the sidecar's address, never another workspace's.
    let wt = b.repo.join(".worktrees/k2-fix");
    let seen: std::sync::Arc<Mutex<Vec<String>>> = std::sync::Arc::new(Mutex::new(Vec::new()));
    let heard = seen.clone();
    let deliver = move |req: &crate::git_notes_routes::DeliverRequest<'_>| {
        heard.lock().expect("lock").push(req.target.to_string());
        crate::git_notes_routes::DeliverResult { success: true, delivery_id: Some("m_1".into()), reason: None, hint: None }
    };
    let caller = crate::git_notes_routes::human_caller(None, None);
    let bad = json!({ "path": wt, "to": "someone-else/fix" });
    let r = crate::git_notes_routes::dispatch_post("/cli/git/notes/send", bad.to_string().as_bytes(), &caller, &deliver);
    assert_eq!(r.status, "400 Bad Request", "{}", r.body);
    let good = json!({ "path": wt, "to": "wb-room/fix" });
    let r = crate::git_notes_routes::dispatch_post("/cli/git/notes/send", good.to_string().as_bytes(), &caller, &deliver);
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert_eq!(*seen.lock().expect("lock"), vec!["wb-room/fix".to_string()]);
}

#[test]
fn the_reach_route_lists_sidecars_without_git_or_paths() {
    let b = bench(&ALL);
    let r = crate::garden_data::workbench_rows::agents_route();
    assert_eq!(r.status, "200 OK", "{}", r.body);
    assert!(!r.body.contains(&*b.repo.to_string_lossy()), "no host path: {}", r.body);
    let v: Value = serde_json::from_str(&r.body).expect("json");
    let row = v["agents"].as_array().expect("agents").iter().find(|x| x["address"] == SIDECAR).cloned().unwrap_or_else(|| panic!("{v}"));
    assert_eq!(row["worktree"], ".worktrees/k2-fix");
    assert_eq!(row["branch"], Value::Null, "no git work on the reach path");
}
