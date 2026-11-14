//! Speed pass P4: what `GET /cli/projects/list` costs on the wire, through
//! the REAL dispatcher (`k2_daemon::test_harness::start`).
//!
//! - ETag round trip: a 200 carries a weak `ETag`; the same request with
//!   that tag (header, or the `if_none_match` query parameter a web page
//!   uses) answers `304 Not Modified` with no body; a change to the list
//!   answers 200 with a new tag.
//! - gzip only for requests from off this machine: the tunnel listener and
//!   a local reverse proxy (`X-Forwarded-For`) get a gzip body that decodes
//!   to the same JSON; plain loopback never does.
//! - Byte budget for 200 workspaces of 3 rows each: the plain JSON, the
//!   gzip body off this machine and the 304. The ceilings live here (P0's
//!   budget test keeps its own); lower them when the list gets smaller.

use std::io::{Read, Write};
use std::net::TcpStream as StdTcpStream;
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use k2_daemon::test_harness;

static TEST_LOCK: StdMutex<()> = StdMutex::new(());

fn lock() -> k2_core::test_env::SerialGuard {
    k2_core::test_env::serial(&TEST_LOCK)
}

const OWNER_TOKEN: &str = "owner-token-projects-list-wire";

/// Ceilings for 200 workspaces × 3 rows (measured, plus about 20%).
const BUDGET_PLAIN_BYTES: usize = 400_000;
const BUDGET_GZIP_BYTES: usize = 40_000;

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
        if self.header("content-encoding").as_deref() == Some("gzip") {
            let mut s = String::new();
            flate2::read::GzDecoder::new(&self.body[..]).read_to_string(&mut s).expect("a gzip body");
            s
        } else {
            String::from_utf8(self.body.clone()).expect("a UTF-8 body")
        }
    }
}

/// One GET on a fresh connection; reads exactly the framed answer.
fn get(port: u16, path_and_query: &str, extra: &[&str]) -> Resp {
    let mut stream = StdTcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream.set_read_timeout(Some(Duration::from_secs(20))).expect("set read timeout");
    let mut lines = String::new();
    for h in extra {
        lines.push_str(h);
        lines.push_str("\r\n");
    }
    let req = format!("GET {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\n{lines}Connection: close\r\n\r\n");
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

fn list_path(extra_query: &str) -> String {
    format!("/cli/projects/list?token={OWNER_TOKEN}{extra_query}")
}

fn seed(workspaces: usize) {
    k2_core::db::init_for_tests();
    let run = uuid::Uuid::new_v4().simple().to_string();
    let db = k2_core::db::shared();
    let conn = db.lock();
    for i in 0..workspaces {
        let id = format!("wire-{run}-{i}");
        k2_core::db::schema::Project::create(&conn, &id, &format!("Workspace {i}"), &format!("/tmp/{id}"), "#3366ff", i as i64, 0, None, None)
            .expect("seed project");
        for w in 0..3 {
            k2_core::db::schema::Workspace::create(&conn, &format!("{id}-w{w}"), &id, None, "branch", Some("main"), "main", w, None)
                .expect("seed workspace");
        }
    }
}

fn rename_one() {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE projects SET name = name || ' renamed' WHERE id = (SELECT id FROM projects WHERE id LIKE 'wire-%' LIMIT 1)",
        [],
    )
    .expect("rename");
}

fn guard_agent_spawns() {
    let dir = std::env::temp_dir().join(format!("k2-wire-shim-empty-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("shim dir");
    std::env::set_var("K2_TEST_AGENT_SHIM_DIR", &dir);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn etag_round_trip_answers_304_until_the_list_changes() {
    let _g = lock();
    let _home = k2_core::test_env::TempHome::new();
    guard_agent_spawns();
    seed(5);
    let d = test_harness::start(OWNER_TOKEN).await;
    let port = d.port;
    tokio::task::spawn_blocking(move || {
        let first = get(port, &list_path(""), &[]);
        assert_eq!(first.status, 200);
        let tag = first.header("etag").expect("projects/list carries an ETag");
        assert!(tag.starts_with("W/\""), "a weak tag: {tag}");
        let rows: serde_json::Value = serde_json::from_str(&first.text()).expect("JSON");
        assert!(rows.as_array().expect("array").len() >= 5);

        // Header form (CLI, phone): 304, no body, the same tag.
        let again = get(port, &list_path(""), &[&format!("If-None-Match: {tag}")]);
        assert_eq!(again.status, 304, "unchanged list: {}", again.headers);
        assert!(again.body.is_empty(), "a 304 has no body");
        assert_eq!(again.header("etag").as_deref(), Some(tag.as_str()));

        // Query form (a web page, no preflight).
        let enc: String = tag.bytes().map(|b| format!("%{b:02X}")).collect();
        let q = get(port, &list_path(&format!("&if_none_match={enc}")), &[]);
        assert_eq!(q.status, 304, "query form: {}", q.headers);
        assert!(q.body.is_empty());

        // The list changes: a full answer with a new tag.
        rename_one();
        let changed = get(port, &list_path(""), &[&format!("If-None-Match: {tag}")]);
        assert_eq!(changed.status, 200);
        let new_tag = changed.header("etag").expect("a tag");
        assert_ne!(new_tag, tag);
        assert!(changed.text().contains(" renamed"));

        // A route without an ETag ignores the header.
        let other = get(port, &format!("/cli/projects/active?token={OWNER_TOKEN}"), &[&format!("If-None-Match: {tag}")]);
        assert_ne!(other.status, 304);
        assert!(other.header("etag").is_none());
    })
    .await
    .expect("client thread");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gzip_only_for_requests_from_off_this_machine() {
    let _g = lock();
    let _home = k2_core::test_env::TempHome::new();
    guard_agent_spawns();
    seed(40);
    let d = test_harness::start(OWNER_TOKEN).await;
    let (port, tunnel) = (d.port, d.tunnel_port);
    tokio::task::spawn_blocking(move || {
        let ae = "Accept-Encoding: gzip, deflate, br";
        let local = get(port, &list_path(""), &[ae]);
        assert_eq!(local.status, 200);
        assert!(local.header("content-encoding").is_none(), "loopback is never gzip: {}", local.headers);
        let plain = local.text();

        let proxied = get(port, &list_path(""), &[ae, "X-Forwarded-For: 203.0.113.7"]);
        assert_eq!(proxied.status, 200, "{}", proxied.headers);
        assert_eq!(proxied.header("content-encoding").as_deref(), Some("gzip"), "{}", proxied.headers);
        assert_eq!(proxied.header("vary").as_deref(), Some("Accept-Encoding"));
        assert!(proxied.body.len() < plain.len() / 3, "gzip {} of {}", proxied.body.len(), plain.len());
        assert_eq!(proxied.text(), plain, "decodes to the same JSON");

        let tun = get(tunnel, &list_path(""), &[ae]);
        assert_eq!(tun.status, 200, "{}", tun.headers);
        assert_eq!(tun.header("content-encoding").as_deref(), Some("gzip"), "{}", tun.headers);
        assert_eq!(tun.text(), plain);

        // Off this machine without gzip in Accept-Encoding: plain.
        let no_ae = get(tunnel, &list_path(""), &[]);
        assert!(no_ae.header("content-encoding").is_none());
        assert_eq!(no_ae.text(), plain);
    })
    .await
    .expect("client thread");
}

/// The byte budget for a large server (200 workspaces × 3 rows).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn projects_list_byte_budget() {
    let _g = lock();
    let _home = k2_core::test_env::TempHome::new();
    guard_agent_spawns();
    seed(200);
    let d = test_harness::start(OWNER_TOKEN).await;
    let (port, tunnel) = (d.port, d.tunnel_port);
    tokio::task::spawn_blocking(move || {
        let plain = get(port, &list_path(""), &[]);
        let tag = plain.header("etag").expect("a tag");
        let gz = get(tunnel, &list_path(""), &["Accept-Encoding: gzip"]);
        let not_modified = get(tunnel, &list_path(""), &["Accept-Encoding: gzip", &format!("If-None-Match: {tag}")]);
        println!("projects/list, 200 workspaces x 3 rows:");
        println!("  plain JSON (loopback)     {:>8} bytes", plain.body.len());
        println!("  gzip (off this machine)   {:>8} bytes", gz.body.len());
        println!("  304 (unchanged)           {:>8} bytes", not_modified.body.len());
        assert!(plain.body.len() <= BUDGET_PLAIN_BYTES, "plain {} > {BUDGET_PLAIN_BYTES}", plain.body.len());
        assert_eq!(gz.header("content-encoding").as_deref(), Some("gzip"));
        assert!(gz.body.len() <= BUDGET_GZIP_BYTES, "gzip {} > {BUDGET_GZIP_BYTES}", gz.body.len());
        assert_eq!(not_modified.status, 304);
        assert_eq!(not_modified.body.len(), 0);
    })
    .await
    .expect("client thread");
}
