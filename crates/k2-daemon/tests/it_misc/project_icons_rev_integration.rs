//! `project-icons-rev-v1` (0.46.17) through the REAL dispatcher
//! (`k2_daemon::test_harness::start`):
//!
//! - the default lists are unchanged (a key snapshot, and the workspace list
//!   byte for byte against the core read);
//! - `?icons=rev` gives each row `iconRev` (null = no icon) and no `iconUrl`,
//!   on the workspace list and the project list, and the rev moves when the
//!   icon does;
//! - the icon routes carry `iconRev`, tag it `ETag: "<iconRev>"`, answer a
//!   matching `If-None-Match` with an empty 304 and a stale one with the
//!   picture;
//! - a repeated or unknown `icons` is a 400;
//! - the parameter shows no caller a row it couldn't already read.
//!
//! Fail loudly: every assertion names what it saw; no defaults.

use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::projects_ops;
use k2_daemon::test_harness;
use serde_json::{json, Value};

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> k2_core::test_env::SerialGuard {
    k2_core::test_env::serial(&TEST_LOCK)
}

const OWNER: &str = "owner-token-project-icons-rev";
const PW: &str = "password123-icons";
const ICON_A: &str = "data:image/png;base64,AAAAicon-a";
const ICON_B: &str = "data:image/png;base64,BBBBicon-b";

/// Every top-level key of a default `GET /cli/projects/list` row, sorted.
/// A new `projects` column adds one here on purpose.
const PROJECT_ROW_KEYS: &[&str] = &[
    "agentDisplayName",
    "agentEnabled",
    "agentMode",
    "agentsCanCreateConnections",
    "agentsCanHireAgents",
    "agentsCanManageAgents",
    "agentsCanManageAppUsers",
    "agentsCanManageAppsFull",
    "agentsCanManageChat",
    "agentsCanManageCompute",
    "agentsCanManageGardens",
    "agentsCanManageProjects",
    "agentsCanManageSkin",
    "agentsCanManageVault",
    "agentsCanOperateCompute",
    "agentsCanRefreshChat",
    "agentsCanUpgradePostgres",
    "agentsCanUseCompute",
    "allowRemoteInstruct",
    "appCores",
    "color",
    "completionSoundEnabled",
    "createdAt",
    "dbAgentAccess",
    "defaultAgent",
    "defaultModel",
    "designModeEnabled",
    "dnsManageEnabled",
    "focusGroupId",
    "forceModelOnResume",
    "handle",
    "heartbeatEnabled",
    "heartbeatLastFire",
    "heartbeatMode",
    "heartbeatSchedule",
    "hideApiSessions",
    "iconRev",
    "iconUrl",
    "id",
    "lastInteractionAt",
    "lastOpenedAt",
    "mailManageEnabled",
    "manuallyActive",
    "mobileTabEnabled",
    "name",
    "path",
    "pinned",
    "stateId",
    "tabOrder",
    "workspaces",
    "worktreeMode",
];

struct Resp {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

impl Resp {
    fn header(&self, name: &str) -> Option<String> {
        self.headers.split("\r\n").skip(1).find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim().eq_ignore_ascii_case(name).then(|| v.trim().to_string())
        })
    }

    fn text(&self) -> String {
        String::from_utf8(self.body.clone()).expect("a UTF-8 body")
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap_or_else(|e| panic!("not JSON ({e}): {:?}", self.text()))
    }
}

/// One request on a fresh connection; reads exactly the framed answer.
fn call(port: u16, method: &str, path_and_query: &str, extra: &[&str], body: Option<&str>) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream.set_read_timeout(Some(Duration::from_secs(20))).expect("set read timeout");
    let mut lines = String::new();
    for h in extra {
        lines.push_str(h);
        lines.push_str("\r\n");
    }
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n{lines}Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n{lines}Connection: close\r\n\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).expect("read the answer");
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("a header block");
    let headers = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = raw[split + 4..].to_vec();
    let status = headers
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .expect("a status line");
    let resp = Resp { status, headers, body };
    if let Some(len) = resp.header("content-length") {
        assert_eq!(len.parse::<usize>().expect("a number"), resp.body.len(), "Content-Length frames the body");
    }
    resp
}

fn get(port: u16, path_and_query: &str, extra: &[&str]) -> Resp {
    call(port, "GET", path_and_query, extra, None)
}

fn guard_agent_spawns() {
    let dir = std::env::temp_dir().join(format!("k2-icons-rev-shim-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("shim dir");
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &dir);
}

/// Two workspaces under a unique prefix: `<run>-a` with [`ICON_A`] stored,
/// `<run>-b` with none (its folder does not exist, so nothing is detected).
fn seed_projects() -> (String, String) {
    k2_core::db::init_for_tests();
    let run = format!("icons-{}", uuid::Uuid::new_v4().simple());
    let (a, b) = (format!("{run}-a"), format!("{run}-b"));
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        for (i, id) in [&a, &b].into_iter().enumerate() {
            k2_core::db::schema::Project::create(&conn, id, &format!("Icons {i}"), &format!("/tmp/{id}-missing"), "#3366ff", i as i64, 0, None, None)
                .expect("seed project");
        }
    }
    projects_ops::projects_set_icon(&a, ICON_A).expect("set icon a");
    (a, b)
}

fn rows_by_id(list: &Value) -> std::collections::BTreeMap<String, Value> {
    list.as_array()
        .unwrap_or_else(|| panic!("projects/list is an array: {list}"))
        .iter()
        .map(|r| (r["id"].as_str().unwrap_or_else(|| panic!("row id: {r}")).to_string(), r.clone()))
        .collect()
}

fn keys(row: &Value) -> BTreeSet<String> {
    row.as_object().unwrap_or_else(|| panic!("row is an object: {row}")).keys().cloned().collect()
}

fn list(port: u16, token: &str, extra: &str) -> Resp {
    get(port, &format!("/cli/projects/list?token={token}{extra}"), &[])
}

fn group_list(port: u16, token: &str, extra: &str) -> Resp {
    get(port, &format!("/cli/project-group/list?token={token}{extra}"), &[])
}

fn groups_by_id(resp: &Resp) -> std::collections::BTreeMap<String, Value> {
    assert_eq!(resp.status, 200, "project-group/list: {}", resp.text());
    let v = resp.json();
    assert_eq!(v["ok"], true, "{v}");
    v["groups"]
        .as_array()
        .unwrap_or_else(|| panic!("groups array: {v}"))
        .iter()
        .map(|g| (g["id"].as_str().unwrap_or_else(|| panic!("group id: {g}")).to_string(), g.clone()))
        .collect()
}

fn add_login(port: u16, user: &str, role: &str) -> String {
    let body = json!({ "username": user, "password": PW, "mustChangePassword": false, "role": role }).to_string();
    let r = call(port, "POST", &format!("/cli/users/add?token={OWNER}"), &[], Some(&body));
    assert_eq!(r.status, 200, "users/add {user} ({role}): {}", r.text());
    let login = json!({ "username": user, "password": PW }).to_string();
    let r = call(port, "POST", "/cli/auth/login", &[], Some(&login));
    assert_eq!(r.status, 200, "login {user}: {}", r.text());
    r.json()["token"].as_str().unwrap_or_else(|| panic!("login token: {}", r.text())).to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_list_is_unchanged_and_rev_list_sends_revisions() {
    let _g = lock();
    let _home = k2_core::test_env::TempHome::new();
    guard_agent_spawns();
    let (a, b) = seed_projects();
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    tokio::task::spawn_blocking(move || {
        // Default: byte for byte what the core read serializes to.
        let plain = list(port, OWNER, "");
        assert_eq!(plain.status, 200, "{}", plain.text());
        let want = serde_json::to_string(&projects_ops::projects_list_with_workspaces_read().expect("core read"))
            .expect("serialize");
        assert_eq!(plain.text(), want, "the default list is the core read, unchanged");
        let rows = rows_by_id(&plain.json());
        let (ra, rb) = (&rows[&a], &rows[&b]);
        let snapshot: BTreeSet<String> = PROJECT_ROW_KEYS.iter().map(|k| k.to_string()).collect();
        assert_eq!(keys(ra), snapshot, "default row keys: {ra}");
        assert_eq!(keys(rb), snapshot, "default row keys: {rb}");
        let rev_a = projects_ops::icon_rev_of(Some(ICON_A));
        assert_eq!(rev_a.len(), projects_ops::ICON_REV_LEN);
        assert_eq!(ra["iconUrl"], ICON_A);
        assert_eq!(ra["iconRev"], rev_a.as_str(), "default iconRev is a string");
        assert_eq!(rb["iconUrl"], Value::Null);
        assert_eq!(rb["iconRev"], "", "default: no icon is the empty string");

        // ?icons=rev: iconRev (null = none), no iconUrl, nothing else changes.
        let rev = list(port, OWNER, "&icons=rev");
        assert_eq!(rev.status, 200, "{}", rev.text());
        assert!(!rev.text().contains(ICON_A), "no data URL rides the rev list");
        let rrows = rows_by_id(&rev.json());
        assert_eq!(rrows.keys().collect::<Vec<_>>(), rows.keys().collect::<Vec<_>>(), "same rows");
        let mut no_url = snapshot.clone();
        no_url.remove("iconUrl");
        for (id, row) in &rrows {
            assert_eq!(keys(row), no_url, "rev row keys for {id}: {row}");
            let mut plain_row = rows[id].clone();
            let o = plain_row.as_object_mut().expect("object");
            o.remove("iconUrl");
            o.remove("iconRev");
            let mut rev_row = row.clone();
            rev_row.as_object_mut().expect("object").remove("iconRev");
            assert_eq!(rev_row, plain_row, "only the icon fields differ for {id}");
        }
        assert_eq!(rrows[&a]["iconRev"], rev_a.as_str());
        assert_eq!(rrows[&b]["iconRev"], Value::Null, "no icon is null under icons=rev");

        // The rev moves with the icon, and back to null when it is cleared.
        projects_ops::projects_set_icon(&a, ICON_B).expect("change icon");
        let moved = rows_by_id(&list(port, OWNER, "&icons=rev").json());
        let rev_b = projects_ops::icon_rev_of(Some(ICON_B));
        assert_ne!(rev_b, rev_a);
        assert_eq!(moved[&a]["iconRev"], rev_b.as_str(), "the rev changes when the icon changes");
        projects_ops::projects_clear_icon(&a).expect("clear icon");
        let cleared = rows_by_id(&list(port, OWNER, "&icons=rev").json());
        assert_eq!(cleared[&a]["iconRev"], Value::Null, "cleared, and no icon file to detect");

        // An empty stored icon is no picture: null, never a rev to fetch.
        projects_ops::projects_set_icon(&b, "").expect("store an empty icon");
        let empty = rows_by_id(&list(port, OWNER, "&icons=rev").json());
        assert_eq!(empty[&b]["iconRev"], Value::Null, "{}", empty[&b]);
        for (id, row) in &empty {
            assert!(row["iconRev"].is_string() || row["iconRev"].is_null(), "iconRev for {id}: {row}");
        }
    })
    .await
    .expect("client thread");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_icon_tags_its_rev_and_answers_304() {
    let _g = lock();
    let _home = k2_core::test_env::TempHome::new();
    guard_agent_spawns();
    let (a, b) = seed_projects();
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    tokio::task::spawn_blocking(move || {
        let icon_path = |id: &str| format!("/cli/projects/get-icon?token={OWNER}&path=%2Ftmp%2F{id}-missing&project_id={id}");
        let listed = rows_by_id(&list(port, OWNER, "&icons=rev").json());

        let first = get(port, &icon_path(&a), &[]);
        assert_eq!(first.status, 200, "{}", first.text());
        let v = first.json();
        assert_eq!(v["found"], true, "{v}");
        assert_eq!(v["dataUrl"], ICON_A, "{v}");
        let rev = v["iconRev"].as_str().unwrap_or_else(|| panic!("iconRev: {v}")).to_string();
        assert_eq!(listed[&a]["iconRev"], rev.as_str(), "the icon's rev is the list's");
        let tag = first.header("etag").unwrap_or_else(|| panic!("an ETag: {}", first.headers));
        assert_eq!(tag, format!("\"{rev}\""), "the ETag is the iconRev");

        // Today's fields, unchanged; iconRev is the one added key.
        let want_keys: BTreeSet<String> = ["found", "dataUrl", "iconRev"].iter().map(|k| k.to_string()).collect();
        assert_eq!(keys(&v), want_keys, "{v}");

        let again = get(port, &icon_path(&a), &[&format!("If-None-Match: {tag}")]);
        assert_eq!(again.status, 304, "{}", again.headers);
        assert!(again.body.is_empty(), "a 304 has no body: {:?}", again.text());
        assert_eq!(again.header("etag").as_deref(), Some(tag.as_str()));
        // Bare, weak and listed forms of the same rev.
        for inm in [rev.clone(), format!("W/\"{rev}\""), format!("\"old\", {rev}"), format!("x, W/\"{rev}\"")] {
            let r = get(port, &icon_path(&a), &[&format!("If-None-Match: {inm}")]);
            assert_eq!(r.status, 304, "If-None-Match: {inm}: {}", r.headers);
            assert!(r.body.is_empty(), "{inm}");
        }

        let stale = get(port, &icon_path(&a), &["If-None-Match: \"0000000000000000\""]);
        assert_eq!(stale.status, 200, "{}", stale.headers);
        assert_eq!(stale.json()["dataUrl"], ICON_A, "a stale tag gets the picture");

        // A new icon: the old tag gets the new picture and the new tag.
        projects_ops::projects_set_icon(&a, ICON_B).expect("change icon");
        let changed = get(port, &icon_path(&a), &[&format!("If-None-Match: {tag}")]);
        assert_eq!(changed.status, 200, "{}", changed.headers);
        let cv = changed.json();
        assert_eq!(cv["dataUrl"], ICON_B);
        let new_rev = projects_ops::icon_rev_of(Some(ICON_B));
        assert_eq!(cv["iconRev"], new_rev.as_str());
        assert_eq!(changed.header("etag"), Some(format!("\"{new_rev}\"")));

        // No icon: iconRev null, no tag, never a 304.
        let none = get(port, &icon_path(&b), &["If-None-Match: *"]);
        assert_eq!(none.status, 200, "{}", none.headers);
        let nv = none.json();
        assert_eq!(nv["found"], false, "{nv}");
        assert_eq!(nv["iconRev"], Value::Null, "{nv}");
        assert!(none.header("etag").is_none(), "{}", none.headers);
    })
    .await
    .expect("client thread");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_group_list_and_icon_do_the_same() {
    let _g = lock();
    let _home = k2_core::test_env::TempHome::new();
    guard_agent_spawns();
    k2_core::db::init_for_tests();
    let run = uuid::Uuid::new_v4().simple().to_string();
    let with = k2_core::project_groups::create_group(&format!("icons-with-{run}")).expect("create group").id;
    let without = k2_core::project_groups::create_group(&format!("icons-without-{run}")).expect("create group").id;
    k2_core::project_groups::set_group_icon(&with, Some(ICON_A)).expect("set group icon");
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    tokio::task::spawn_blocking(move || {
        // Default: unchanged (no iconRev; the iconKey it always had).
        let plain = groups_by_id(&group_list(port, OWNER, ""));
        let key_a = k2_core::project_groups::icon_key(ICON_A);
        assert_eq!(plain[&with]["iconKey"], key_a.as_str());
        assert_eq!(plain[&without]["iconKey"], Value::Null);
        for (id, row) in &plain {
            assert!(row.get("iconRev").is_none(), "default group row {id} has no iconRev: {row}");
            assert!(row.get("iconUrl").is_none(), "{row}");
        }

        // ?icons=rev: iconRev per row (null = none), no iconUrl, same rows.
        let rev = groups_by_id(&group_list(port, OWNER, "&icons=rev"));
        assert_eq!(rev.keys().collect::<Vec<_>>(), plain.keys().collect::<Vec<_>>(), "same groups");
        for (id, row) in &rev {
            let mut k = keys(row);
            assert!(k.remove("iconRev"), "rev row {id} has iconRev: {row}");
            assert_eq!(k, keys(&plain[id]), "only iconRev is added for {id}");
            assert!(row.get("iconUrl").is_none(), "{row}");
            assert!(row["iconRev"].is_string() || row["iconRev"].is_null(), "iconRev is a string or null: {row}");
        }
        assert_eq!(rev[&with]["iconRev"], key_a.as_str());
        assert_eq!(rev[&without]["iconRev"], Value::Null);

        // The icon route: iconRev, ETag, 304, stale → the picture.
        let icon_path = |g: &str| format!("/cli/project-group/icon?token={OWNER}&group={g}");
        let first = get(port, &icon_path(&with), &[]);
        assert_eq!(first.status, 200, "{}", first.text());
        let v = first.json();
        assert_eq!(v["iconRev"], key_a.as_str(), "{v}");
        assert_eq!(v["iconKey"], key_a.as_str(), "{v}");
        assert_eq!(v["dataUrl"], ICON_A, "{v}");
        let tag = first.header("etag").unwrap_or_else(|| panic!("an ETag: {}", first.headers));
        assert_eq!(tag, format!("\"{key_a}\""));
        let want_keys: BTreeSet<String> =
            ["ok", "groupId", "found", "iconKey", "dataUrl", "iconRev"].iter().map(|k| k.to_string()).collect();
        assert_eq!(keys(&v), want_keys, "today's fields, plus iconRev: {v}");
        assert_eq!(v["ok"], true);
        assert_eq!(v["groupId"], with.as_str());
        assert_eq!(v["found"], true);
        let again = get(port, &icon_path(&with), &[&format!("If-None-Match: {tag}")]);
        assert_eq!(again.status, 304, "{}", again.headers);
        assert!(again.body.is_empty(), "a 304 has no body: {:?}", again.text());
        for inm in [key_a.clone(), format!("W/\"{key_a}\""), format!("\"old\",{key_a}")] {
            let r = get(port, &icon_path(&with), &[&format!("If-None-Match: {inm}")]);
            assert_eq!(r.status, 304, "If-None-Match: {inm}: {}", r.headers);
            assert!(r.body.is_empty(), "{inm}");
        }
        let stale = get(port, &icon_path(&with), &["If-None-Match: \"stale\""]);
        assert_eq!(stale.status, 200);
        assert_eq!(stale.json()["dataUrl"], ICON_A);

        // A new icon moves the rev on the list and the route.
        k2_core::project_groups::set_group_icon(&with, Some(ICON_B)).expect("change group icon");
        let key_b = k2_core::project_groups::icon_key(ICON_B);
        assert_ne!(key_b, key_a);
        let moved = groups_by_id(&group_list(port, OWNER, "&icons=rev"));
        assert_eq!(moved[&with]["iconRev"], key_b.as_str(), "the rev changes when the icon changes");
        let changed = get(port, &icon_path(&with), &[&format!("If-None-Match: {tag}")]);
        assert_eq!(changed.status, 200, "{}", changed.headers);
        assert_eq!(changed.json()["dataUrl"], ICON_B);
        assert_eq!(changed.header("etag"), Some(format!("\"{key_b}\"")));

        // No icon: null, no tag.
        let none = get(port, &icon_path(&without), &["If-None-Match: *"]);
        assert_eq!(none.status, 200, "{}", none.headers);
        assert_eq!(none.json()["iconRev"], Value::Null);
        assert!(none.header("etag").is_none(), "{}", none.headers);
    })
    .await
    .expect("client thread");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_repeated_or_unknown_icons_param_is_refused() {
    let _g = lock();
    let _home = k2_core::test_env::TempHome::new();
    guard_agent_spawns();
    seed_projects();
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    tokio::task::spawn_blocking(move || {
        for extra in ["&icons=rev&icons=rev", "&icons=rev&icons=urls", "&icons=urls", "&icons=", "&icons=REV"] {
            let r = list(port, OWNER, extra);
            assert_eq!(r.status, 400, "projects/list{extra}: {}", r.text());
            let msg = r.json()["error"].as_str().unwrap_or_else(|| panic!("an error line: {}", r.text())).to_string();
            assert!(msg.contains("icons"), "{extra}: {msg}");

            let g = group_list(port, OWNER, extra);
            assert_eq!(g.status, 400, "project-group/list{extra}: {}", g.text());
            let v = g.json();
            assert_eq!(v["ok"], false, "{v}");
            assert_eq!(v["error"]["code"], "usage", "{v}");
        }
        // Once, and a repeat of another key, still answer.
        assert_eq!(list(port, OWNER, "&icons=rev").status, 200);
        assert_eq!(list(port, OWNER, "&x=1&x=2").status, 200);
        assert_eq!(group_list(port, OWNER, "&icons=rev").status, 200);
    })
    .await
    .expect("client thread");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn icons_rev_shows_no_caller_a_row_it_could_not_read() {
    let _g = lock();
    let _home = k2_core::test_env::TempHome::new();
    guard_agent_spawns();
    let (a, _) = seed_projects();
    let run = uuid::Uuid::new_v4().simple().to_string();
    let group_name = format!("icons-scope-{run}");
    let group = k2_core::project_groups::create_group(&group_name).expect("create group").id;
    k2_core::project_groups::set_group_icon(&group, Some(ICON_A)).expect("set group icon");
    let d = test_harness::start(OWNER).await;
    let port = d.port;
    tokio::task::spawn_blocking(move || {
        let member = add_login(port, &format!("m{}", &run[..8]), "member");
        let voice = add_login(port, &format!("v{}", &run[..8]), "voice_chat");

        // A Member: the same rows with and without the parameter.
        let plain = rows_by_id(&list(port, &member, "").json());
        let rev = rows_by_id(&list(port, &member, "&icons=rev").json());
        assert_eq!(rev.keys().collect::<Vec<_>>(), plain.keys().collect::<Vec<_>>());
        let gplain = groups_by_id(&group_list(port, &member, ""));
        let grev = groups_by_id(&group_list(port, &member, "&icons=rev"));
        assert_eq!(grev.keys().collect::<Vec<_>>(), gplain.keys().collect::<Vec<_>>());

        // A Voice & Chat only login reads neither list nor icon, either way.
        for extra in ["", "&icons=rev"] {
            for r in [list(port, &voice, extra), group_list(port, &voice, extra)] {
                assert_eq!(r.status, 403, "voice_chat{extra}: {}", r.text());
                let t = r.text();
                assert!(!t.contains(&a) && !t.contains(&group_name) && !t.contains("iconRev"), "{t}");
            }
        }
        let p = get(port, &format!("/cli/projects/get-icon?token={voice}&path=%2Ftmp%2F{a}-missing&project_id={a}"), &[]);
        assert_eq!(p.status, 403, "{}", p.text());
        assert!(p.header("etag").is_none() && !p.text().contains(ICON_A), "{}", p.text());
        let gi = get(port, &format!("/cli/project-group/icon?token={voice}&group={group}"), &[]);
        assert_eq!(gi.status, 403, "{}", gi.text());
        assert!(gi.header("etag").is_none() && !gi.text().contains(ICON_A), "{}", gi.text());

        // No credential at all: refused, with or without the parameter.
        for extra in ["", "&icons=rev"] {
            assert_eq!(list(port, "wrong", extra).status, 403, "bad token{extra}");
            assert_eq!(group_list(port, "wrong", extra).status, 403, "bad token{extra}");
        }
    })
    .await
    .expect("client thread");
}
