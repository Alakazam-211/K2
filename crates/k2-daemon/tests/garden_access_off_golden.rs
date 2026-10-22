//! Garden access (prd-garden-access-v1 §3 G0a, §8): "off = 0.46.3" — the
//! daemon half of the switch-off goldens under
//! `tests/golden/garden_access_off/`.
//!
//! Captured from integ/0.46.3-r9, BEFORE any Garden access code existed.
//! While `gardenAccess` is off (the default) these must stay byte for byte:
//!
//! - **zen_bodies.txt**: what the person (owner token) and an agent (its
//!   passport over TCP, as the CLI sends it) get from the Zen routes: setup,
//!   the Garden list, status, sync, a custom widget made and placed, the page
//!   payload, the widget list and show, an agent's widget and Garden writes
//!   (proposals at 0.46.3), the agent's `garden/new`, rename, delete, reset,
//!   theme, preview and sign, the proposals list, the person's sign and
//!   delete, and the Garden access routes (`garden/access`, `/ask`,
//!   `garden/history`, `widget/history`, `gardens/deleted`, `garden/restore`,
//!   `snapshot`) answering as routes with no row did at 0.46.3.
//! - **boot_status_features.txt**: `/boot-status` key names and `features`.
//! - **skill_k2_zen.txt**: the k2-zen skill a set-up computer writes.
//!
//! G0a's named exceptions (`exceptions.txt`) are asserted here directly, not
//! through a golden: `gardenAccess: false` in `/cli/settings/get` (item 3)
//! and migration 0219's four tables existing and EMPTY after every act above
//! (item 2). Item 1 (the Settings switch row) is the renderer golden; item 4
//! (the catalog rows) is `catalog_rows.txt` (`src/garden_access_off_golden_tests.rs`).
//!
//! Normalizer (documented; it replaces values, never drops a field or line):
//! the temp `$HOME`, Garden ids (`<G1>`…, in list order), the workspace id,
//! session id, passport and owner token, the port, the daemon version, any
//! other UUID, Zen snapshot stamps (`20261022T101010123Z` → `<STAMP>`), RFC
//! 3339 times (`<TIME>`), proposal ids (`zp-…` → `<PROPOSAL>`), and in JSON
//! bodies the VALUES of [`VOLATILE_KEYS`] (clocks) and a 16-hex `version`
//! (a page version hash over content and save times). A `proposals` list is
//! sorted by (kind, target, file): same-second proposals tie on the clock.
//!
//! Modes: plain `cargo test -p k2-daemon --test garden_access_off_golden`
//! compares and fails loudly on any difference or missing fixture.
//! `GARDEN_ACCESS_GOLDEN_BLESS=1` writes the goldens — only ever from a base
//! WITHOUT Garden access code.
//!
//! No vault, keychain, real `~/.k2` or running daemon is touched.

#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_core::session::SessionId;
use k2_daemon::session_token::{CredMode, HookPrincipal, Provider};
use k2_daemon::test_harness;
use rusqlite::params;
use serde_json::Value;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());
const OWNER: &str = "owner-token-garden-access-off-golden";

/// The four tables of migration 0219 (G0a item 2: they exist and stay empty
/// while the switch is off).
const TABLES_0219: &[&str] = &["zen_garden_access", "zen_garden_made", "zen_garden_access_asks", "zen_garden_events"];

/// A custom widget placed in a Garden (the page payload carries it).
fn page(widget: &str) -> String {
    format!(
        "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"main\"\nkind = \"custom\"\nwidget = \"{widget}\"\ncolumn = 0\n"
    )
}

// ── env ──────────────────────────────────────────────────────────────

struct Env {
    home: PathBuf,
    prev_home: Option<std::ffi::OsString>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for Env {
    fn drop(&mut self) {
        match self.prev_home.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup() -> Env {
    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    for (k, _) in std::env::vars() {
        if k.starts_with("K2_") || k.starts_with("K2SO_") {
            std::env::remove_var(k);
        }
    }
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let home = PathBuf::from(format!("/tmp/k2gao{:x}", nanos % 0xffff_ffff));
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(&home).unwrap();
    let env = Env { prev_home: std::env::var_os("HOME"), home: home.clone(), _guard: guard };
    std::env::set_var("HOME", &home);
    env
}

// ── golden files ─────────────────────────────────────────────────────

fn golden_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/garden_access_off")
}

fn blessing() -> bool {
    std::env::var("GARDEN_ACCESS_GOLDEN_BLESS").as_deref() == Ok("1")
}

struct Goldens {
    failures: Vec<String>,
}

impl Goldens {
    fn check(&mut self, rel: &str, got: &str) {
        assert!(rel.ends_with(".txt"), "golden {rel:?} must end in .txt");
        let path = golden_root().join(rel);
        if blessing() {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, got).unwrap();
            return;
        }
        match std::fs::read_to_string(&path) {
            Err(e) => self.failures.push(format!("{rel}: no golden ({e}); capture it from a base without Garden access")),
            Ok(want) if want == got => {}
            Ok(want) => self.failures.push(format!("{rel} differs from 0.46.3\n--- want\n{want}\n--- got\n{got}")),
        }
    }

    fn finish(self) {
        assert!(self.failures.is_empty(), "{} off-golden failure(s):\n\n{}", self.failures.len(), self.failures.join("\n\n"));
    }
}

// ── normalizer ───────────────────────────────────────────────────────

struct Norm {
    subs: Vec<(String, String)>,
}

impl Norm {
    fn apply(&self, s: &str) -> String {
        let mut t = s.to_string();
        let mut subs = self.subs.clone();
        subs.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        for (from, to) in &subs {
            if !from.is_empty() {
                t = t.replace(from.as_str(), to);
            }
        }
        let t = mask_runs(&t);
        replace_uuids(&t)
    }
}

/// Zen snapshot stamps (`YYYYMMDDTHHMMSSmmmZ`), RFC 3339 times and proposal
/// ids (`zp-<alnum>`): per-run values, replaced in place.
fn mask_runs(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    let digits = |from: usize, n: usize| from + n <= b.len() && b[from..from + n].iter().all(u8::is_ascii_digit);
    while i < b.len() {
        // 20261022T101010123Z
        if digits(i, 8) && b.get(i + 8) == Some(&b'T') && digits(i + 9, 9) && b.get(i + 18) == Some(&b'Z') {
            out.push_str("<STAMP>");
            i += 19;
            continue;
        }
        // 2026-10-22T10:10:10(.123)?(Z|+00:00)
        if digits(i, 4)
            && b.get(i + 4) == Some(&b'-')
            && digits(i + 5, 2)
            && b.get(i + 7) == Some(&b'-')
            && digits(i + 8, 2)
            && b.get(i + 10) == Some(&b'T')
            && digits(i + 11, 2)
            && b.get(i + 13) == Some(&b':')
        {
            let mut j = i + 11;
            while j < b.len() && (b[j].is_ascii_digit() || b":.+-Z".contains(&b[j])) {
                j += 1;
            }
            out.push_str("<TIME>");
            i = j;
            continue;
        }
        if b[i..].starts_with(b"zp-") && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let mut j = i + 3;
            while j < b.len() && (b[j].is_ascii_alphanumeric()) {
                j += 1;
            }
            if j > i + 3 {
                out.push_str("<PROPOSAL>");
                i = j;
                continue;
            }
        }
        let c = s[i..].chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

fn replace_uuids(s: &str) -> String {
    let b = s.as_bytes();
    let shape = [8usize, 4, 4, 4, 12];
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    'outer: while i < b.len() {
        let mut j = i;
        for (n, len) in shape.iter().enumerate() {
            for _ in 0..*len {
                if j >= b.len() || !b[j].is_ascii_hexdigit() {
                    let c = s[i..].chars().next().unwrap();
                    out.push(c);
                    i += c.len_utf8();
                    continue 'outer;
                }
                j += 1;
            }
            if n < 4 {
                if j >= b.len() || b[j] != b'-' {
                    let c = s[i..].chars().next().unwrap();
                    out.push(c);
                    i += c.len_utf8();
                    continue 'outer;
                }
                j += 1;
            }
        }
        let before_ok = i == 0 || !b[i - 1].is_ascii_hexdigit();
        let after_ok = j >= b.len() || !b[j].is_ascii_hexdigit();
        if before_ok && after_ok {
            out.push_str("<UUID>");
            i = j;
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// Keys whose VALUES are clocks; the key itself is kept.
const VOLATILE_KEYS: &[&str] =
    &["at", "createdAt", "created_at", "updatedAt", "updated_at", "decidedAt", "decided_at", "mtime", "modifiedAt", "signedAt", "lastSeen"];

fn mask_volatile(v: &mut Value) {
    match v {
        Value::Object(m) => {
            for (k, x) in m.iter_mut() {
                if VOLATILE_KEYS.contains(&k.as_str()) && !x.is_null() {
                    *x = Value::String(format!("<{k}>"));
                } else if k == "version" && x.as_str().is_some_and(|s| s.len() == 16 && s.bytes().all(|b| b.is_ascii_hexdigit())) {
                    // A page version: a hash over content and save times.
                    *x = Value::String("<version-hash>".into());
                } else {
                    mask_volatile(x);
                }
            }
        }
        Value::Array(a) => a.iter_mut().for_each(mask_volatile),
        _ => {}
    }
}

// ── transport ────────────────────────────────────────────────────────

struct Resp {
    status_line: String,
    content_type: String,
    body: String,
}

fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect()
}

fn tcp_blocking(port: u16, method: &str, pq: &str, body: Option<String>) -> Resp {
    let mut s = StdTcpStream::connect(("127.0.0.1", port)).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
    let raw = match body {
        Some(b) => format!(
            "{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {pq} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    s.write_all(raw.as_bytes()).unwrap();
    let mut out = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match s.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionReset => break,
            Err(e) => panic!("{method} {pq}: {e}"),
        }
    }
    let text = String::from_utf8_lossy(&out).to_string();
    let status_line = text.lines().next().unwrap_or_else(|| panic!("no status line: {text:?}")).trim_end().to_string();
    assert!(status_line.starts_with("HTTP/1."), "not an HTTP response: {text:?}");
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let content_type = head
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-type").then(|| v.trim().to_string())
        })
        .unwrap_or_default();
    Resp { status_line, content_type, body: body.to_string() }
}

async fn tcp(port: u16, method: &str, path_q: &str, token: &str, body: Option<&str>) -> Resp {
    let sep = if path_q.contains('?') { '&' } else { '?' };
    let pq = format!("{path_q}{sep}token={}", enc(token));
    let method = method.to_string();
    let body = body.map(str::to_string);
    tokio::task::spawn_blocking(move || tcp_blocking(port, &method, &pq, body)).await.unwrap()
}

/// A `proposals` list orders by `created_at` then the random id, so two
/// proposals made in the same second come back in either order: sort them by
/// (kind, target, file). Nothing is dropped.
fn sort_proposals(v: &mut Value) {
    if let Some(list) = v.get_mut("proposals").and_then(Value::as_array_mut) {
        let key = |p: &Value| {
            (
                p["kind"].as_str().unwrap_or_default().to_string(),
                p["target"].as_str().unwrap_or_default().to_string(),
                p["file"].as_str().unwrap_or_default().to_string(),
            )
        };
        list.sort_by_key(key);
    }
}

fn json_text(body: &str) -> String {
    match serde_json::from_str::<Value>(body) {
        Ok(mut v) => {
            sort_proposals(&mut v);
            mask_volatile(&mut v);
            serde_json::to_string_pretty(&v).unwrap()
        }
        Err(_) => body.to_string(),
    }
}

// ── the test ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn garden_access_off_is_today_byte_for_byte() {
    let env = setup();
    let d = test_harness::start(OWNER).await;
    let port = d.port;

    // One agent workspace and its passport (an agent in a K2 terminal).
    let ws_path = env.home.join("ws-gao-billing");
    std::fs::create_dir_all(&ws_path).unwrap();
    let ws_id = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
            params![ws_id, "gao-billing", ws_path.to_string_lossy()],
        )
        .unwrap();
    }
    let sid = SessionId::new();
    let passport = k2_daemon::session_token::mint_session_token(
        &sid,
        "pane-gao",
        HookPrincipal { workspace_uuid: ws_id.clone(), agent_address: "gao-billing".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );

    let mut log = String::new();
    let mut subs: Vec<(String, String)> = vec![
        (env.home.to_string_lossy().to_string(), "<HOME>".into()),
        (ws_id.clone(), "<WS_ID>".into()),
        (sid.to_string(), "<SESSION_ID>".into()),
        (passport.clone(), "<PASSPORT>".into()),
        (OWNER.into(), "<OWNER_TOKEN>".into()),
        (format!("127.0.0.1:{port}"), "127.0.0.1:<PORT>".into()),
        (format!("\"version\":\"{}\"", env!("CARGO_PKG_VERSION")), "\"version\":\"<VERSION>\"".into()),
    ];

    // `{G1}`… in a path or body stand for the Garden ids, in list order.
    let mut gardens: Vec<String> = Vec::new();
    macro_rules! case {
        ($who:expr, $method:expr, $path:expr, $body:expr) => {{
            let fill = |s: &str| {
                let mut s = s.to_string();
                for (i, g) in gardens.iter().enumerate() {
                    s = s.replace(&format!("{{G{}}}", i + 1), g);
                }
                s
            };
            let path: String = fill($path);
            let body: Option<String> = $body;
            let body: Option<String> = body.map(|b| fill(&b));
            let (label, token) = match $who {
                "owner" => ("owner token", OWNER),
                _ => ("agent passport (tcp)", passport.as_str()),
            };
            let r = tcp(port, $method, &path, token, body.as_deref()).await;
            log.push_str(&format!(
                "## {label}\n> {} {}\n> body: {}\n{}\ncontent-type: {}\n{}\n\n",
                $method,
                path,
                body.as_deref().unwrap_or(""),
                r.status_line,
                r.content_type,
                json_text(&r.body)
            ));
            r
        }};
    }
    let s = |x: &str| Some(x.to_string());

    // ── the person sets Zen up and builds ───────────────────────────
    case!("owner", "POST", "/cli/zen/setup", s("{}"));
    let r = tcp(port, "GET", "/cli/zen/gardens", OWNER, None).await;
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("gardens JSON ({e}): {}", r.body));
    for g in v["gardens"].as_array().unwrap_or_else(|| panic!("no gardens: {v}")) {
        gardens.push(g["id"].as_str().unwrap_or_else(|| panic!("no id: {g}")).to_string());
    }
    assert!(gardens.len() >= 2, "setup makes Garden 1 and Garden 2 at least: {v}");
    let made_at_setup = gardens.len();
    case!("owner", "GET", "/cli/zen/gardens", None);
    case!("owner", "GET", "/cli/zen/status", None);
    case!("owner", "GET", "/cli/zen/sync", None);
    case!("owner", "POST", "/cli/zen/widget/new", s(r#"{"name":"board"}"#));
    let put = serde_json::to_string(&serde_json::json!({ "garden": "{G2}", "text": page("board") })).unwrap();
    case!("owner", "POST", "/cli/zen/garden/put", Some(put));
    case!("owner", "GET", "/cli/zen/get?garden={G2}", None);
    case!("owner", "GET", "/cli/zen/widgets", None);
    case!("owner", "GET", "/cli/zen/widget/show?widget=board", None);

    // ── an agent ────────────────────────────────────────────────────
    case!("agent", "GET", "/cli/zen/gardens", None);
    case!("agent", "POST", "/cli/zen/widget/new", s(r#"{"name":"agent-board"}"#));
    case!("agent", "POST", "/cli/zen/widget/put", s(r#"{"widget":"board","file":"hello.js","text":"// changed by an agent\n"}"#));
    let agent_put = serde_json::to_string(&serde_json::json!({ "garden": "{G2}", "text": page("agent-board"), "note": "swap the board" })).unwrap();
    case!("agent", "POST", "/cli/zen/garden/put", Some(agent_put));
    case!("agent", "POST", "/cli/zen/garden/new", s(r#"{"name":"Agent page"}"#));
    case!("agent", "POST", "/cli/zen/garden/rename", s(r#"{"garden":"{G2}","name":"Renamed by agent"}"#));
    case!("agent", "POST", "/cli/zen/garden/reorder", s(r#"{"garden":"{G2}","to":1}"#));
    case!("agent", "POST", "/cli/zen/reset", s(r#"{"garden":"{G2}"}"#));
    case!("agent", "POST", "/cli/zen/theme/set", s(r#"{"name":"paper"}"#));
    case!("agent", "POST", "/cli/zen/widget/preview", s(r#"{"clear":true}"#));
    case!("agent", "POST", "/cli/zen/widget/sign", s(r#"{"widget":"agent-board"}"#));
    case!("agent", "POST", "/cli/zen/widget/claim", s(r#"{"widget":"agent-board"}"#));
    case!("agent", "POST", "/cli/zen/garden/lock", s(r#"{"garden":"{G2}","locked":true}"#));
    // Last of the agent's acts: at 0.46.3 a passport's garden/delete deletes.
    case!("agent", "POST", "/cli/zen/garden/delete", s(r#"{"garden":"{G1}"}"#));

    // ── the person reads and decides ────────────────────────────────
    case!("owner", "GET", "/cli/zen/proposals", None);
    case!("owner", "POST", "/cli/zen/widget/sign", s(r#"{"widget":"agent-board"}"#));
    case!("owner", "GET", "/cli/zen/widgets", None);
    case!("owner", "POST", "/cli/zen/garden/new", s(r#"{"name":"Spare"}"#));
    let r = tcp(port, "GET", "/cli/zen/gardens", OWNER, None).await;
    let v: Value = serde_json::from_str(&r.body).unwrap();
    for g in v["gardens"].as_array().unwrap() {
        let id = g["id"].as_str().unwrap().to_string();
        if !gardens.contains(&id) {
            gardens.push(id);
        }
    }
    assert_eq!(gardens.len(), made_at_setup + 1, "the person's new Garden: {v}");
    let spare = format!("{{G{}}}", gardens.len());
    case!("owner", "POST", "/cli/zen/garden/delete", Some(format!(r#"{{"garden":"{spare}"}}"#)));
    case!("owner", "GET", "/cli/zen/gardens", None);

    // ── the Garden access routes, as at 0.46.3 (no rows) ────────────
    for who in ["owner", "agent"] {
        case!(who, "GET", "/cli/zen/garden/access?garden={G2}", None);
        case!(who, "POST", "/cli/zen/garden/access", s(r#"{"garden":"{G2}","agent":"gao-billing","allow":true}"#));
        case!(who, "POST", "/cli/zen/garden/access/ask", s(r#"{"garden":"{G2}"}"#));
        case!(who, "GET", "/cli/zen/garden/history?garden={G2}", None);
        case!(who, "GET", "/cli/zen/widget/history?widget=board", None);
        case!(who, "GET", "/cli/zen/gardens/deleted", None);
        case!(who, "POST", "/cli/zen/garden/restore", Some(format!(r#"{{"garden":"{spare}"}}"#)));
        case!(who, "GET", "/cli/zen/snapshot?file=zen.toml&name=x.toml", None);
    }

    for (i, g) in gardens.iter().enumerate() {
        subs.push((g.clone(), format!("<G{}>", i + 1)));
    }
    let norm = Norm { subs };
    let mut goldens = Goldens { failures: Vec::new() };
    goldens.check("zen_bodies.txt", &norm.apply(&log));

    // ── /boot-status: key names and the features list ───────────────
    let r = tcp_blocking(port, "GET", "/boot-status", None);
    let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("/boot-status JSON ({e}): {}", r.body));
    let obj = v.as_object().unwrap_or_else(|| panic!("/boot-status is not an object: {}", r.body));
    let mut keys: Vec<&String> = obj.keys().collect();
    keys.sort();
    let features: Vec<&str> = v["features"]
        .as_array()
        .unwrap_or_else(|| panic!("/boot-status has no features array: {}", r.body))
        .iter()
        .map(|f| f.as_str().unwrap_or_else(|| panic!("feature not a string: {f}")))
        .collect();
    let text = format!(
        "{}\ncontent-type: {}\nkeys: {}\nfeatures:\n{}\n",
        r.status_line,
        r.content_type,
        keys.iter().map(|k| k.as_str()).collect::<Vec<_>>().join(", "),
        features.iter().map(|f| format!("  {f}")).collect::<Vec<_>>().join("\n")
    );
    goldens.check("boot_status_features.txt", &norm.apply(&text));

    // ── the k2-zen skill on a set-up computer ───────────────────────
    let dot = k2_core::workspace_dot_dir(&ws_path);
    assert!(k2_core::workspace::skill_regen::ensure_zen_skill(&dot, true), "the k2-zen skill is written");
    let skill = std::fs::read_to_string(dot.join("skills/k2-zen/SKILL.md")).expect("k2-zen SKILL.md");
    goldens.check("skill_k2_zen.txt", &norm.apply(&skill));

    // ── G0a's named exceptions, asserted directly ───────────────────
    if !blessing() {
        // Item 3: the people-facing switch in /cli/settings/get, off.
        let r = tcp(port, "GET", "/cli/settings/get", OWNER, None).await;
        let v: Value = serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("settings/get JSON ({e}): {}", r.body));
        assert_eq!(v.get("gardenAccess"), Some(&Value::Bool(false)), "gardenAccess is false by default: {}", r.body);
        // Item 2: 0219's tables exist and nothing above wrote them.
        let db = k2_core::db::shared();
        let conn = db.lock();
        for t in TABLES_0219 {
            let n: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {t}"), [], |r| r.get(0))
                .unwrap_or_else(|e| panic!("migration 0219 table {t}: {e}"));
            assert_eq!(n, 0, "{t} has {n} row(s) with the switch off");
        }
        // The list of exceptions itself is a fixture, so it can't drift silently.
        let ex = std::fs::read_to_string(golden_root().join("exceptions.txt")).expect("exceptions.txt");
        for needle in ["Settings → Gardens", "0219", "gardenAccess", "garden.access", "garden.access-off", "garden.delete", "not_available"] {
            assert!(ex.contains(needle), "exceptions.txt must name {needle:?}");
        }
    }
    goldens.finish();
}
