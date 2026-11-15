//! `k2 app requests` end to end in process: a visitor's request through the
//! App path (`router::serve_app_conn_with`, what the E2E listener, the App
//! TLS door and the front port run) lands in `app-requests.db` as one row;
//! K2's 503 and 502 give rows with K2's code; a query string, a cookie or
//! the visitor's address never reach the file; forwarding never waits on a
//! blocked writer; the writer prunes by age and by count. The route's
//! callers are `door_tests::the_request_log_is_the_owners_an_admins_and_the_apps_own_agents`.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use k2_core::app_requests::{self as store, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::*;

const VISITOR: &str = "203.0.113.9";

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

/// One connection on the App path for `app_id`, from `visitor` (as the
/// E2E listener passes it).
async fn via(app_id: &str, visitor: Option<IpAddr>) -> TcpStream {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    let id = app_id.to_string();
    tokio::spawn(async move {
        let (mut s, _) = l.accept().await.unwrap();
        crate::apps::router::serve_app_conn_with(&mut s, &id, crate::client_ip::Mode::Tunnel(visitor)).await;
    });
    TcpStream::connect(addr).await.unwrap()
}

/// Send `req` (any number of pipelined requests), close the sending side,
/// read everything back.
async fn ask(app_id: &str, visitor: Option<IpAddr>, req: &str) -> String {
    let mut c = via(app_id, visitor).await;
    c.write_all(req.as_bytes()).await.unwrap();
    c.shutdown().await.unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), c.read_to_end(&mut out)).await.expect("answered in time").unwrap();
    String::from_utf8_lossy(&out).to_string()
}

fn list(app: &str) -> Vec<Request> {
    let c = store::open(&store::db_path()).unwrap();
    store::list(&c, &store::Query { app_id: app, limit: 1000, ..Default::default() }).unwrap()
}

/// `app`'s rows once there are at least `n`, oldest first.
async fn rows_n(app: &str, n: usize) -> Vec<Request> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        flush(Duration::from_secs(2));
        let mut got = list(app);
        if got.len() >= n {
            got.reverse();
            return got;
        }
        assert!(Instant::now() < deadline, "want {n} rows for {app}, have {got:?}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Every byte of the file and its WAL.
fn file_bytes() -> Vec<u8> {
    let p = store::db_path();
    let mut all = std::fs::read(&p).unwrap();
    if let Ok(w) = std::fs::read(p.with_extension("db-wal")) {
        all.extend_from_slice(&w);
    }
    all
}

fn contains(hay: &[u8], needle: &str) -> bool {
    hay.windows(needle.len()).any(|w| w == needle.as_bytes())
}

/// Every column of every row, as text.
fn dump() -> String {
    let c = store::open(&store::db_path()).unwrap();
    let mut st = c.prepare("SELECT * FROM requests").unwrap();
    let n = st.column_count();
    let mut rows = st.query([]).unwrap();
    let mut out = String::new();
    while let Some(r) = rows.next().unwrap() {
        for i in 0..n {
            out.push_str(&format!("{:?}|", r.get_ref(i).unwrap()));
        }
        out.push('\n');
    }
    out
}

#[tokio::test]
async fn visitor_requests_through_the_app_path_land_one_row_each_without_query_cookie_or_address() {
    let _home = crate::test_support::TempHome::new();
    let id = "req-log-ok";
    let _ = crate::apps::router::switch(id, crate::client_ip::tests::echo_app().await);
    // Three requests on one keep-alive connection.
    let out = ask(
        id,
        Some(ip(VISITOR)),
        "GET /files/list?access_token=k2skn_QUERYSECRET HTTP/1.1\r\nHost: a\r\nCookie: s=COOKIESECRET\r\n\r\n\
         POST /upload?sig=QUERYSIG HTTP/1.1\r\nHost: a\r\nAuthorization: Bearer AUTHSECRET\r\nContent-Length: 4\r\n\r\nBODY\
         GET /done HTTP/1.1\r\nHost: a\r\n\r\n",
    )
    .await;
    assert_eq!(out.matches("HTTP/1.1 200 OK").count(), 3, "{out}");
    let got = rows_n(id, 3).await;
    assert_eq!(got.len(), 3, "{got:?}");
    assert_eq!(
        got.iter().map(|r| (r.method.as_str(), r.path.as_str(), r.status)).collect::<Vec<_>>(),
        vec![("GET", "/files/list", 200), ("POST", "/upload", 200), ("GET", "/done", 200)]
    );
    assert!(got.iter().all(|r| r.k2_code.is_none() && r.closed_early.is_none() && r.head_ms.is_some()), "{got:?}");
    assert!(got[1].bytes_in > 4 && got.iter().all(|r| r.bytes_out > 0), "{got:?}");
    // The visitor key groups the three and is not the address.
    let key = got[0].visitor.clone().expect("a visitor key");
    assert!(got.iter().all(|r| r.visitor.as_deref() == Some(key.as_str())), "{got:?}");
    assert_eq!(key.len(), 16);
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()) && !key.contains(VISITOR), "{key}");
    // Nothing a log must not keep is anywhere in the file.
    let rows = dump();
    let bytes = file_bytes();
    for secret in ["QUERYSECRET", "QUERYSIG", "access_token", "COOKIESECRET", "AUTHSECRET", "BODY", VISITOR] {
        assert!(!rows.contains(secret), "{secret} is in a row:\n{rows}");
        assert!(!contains(&bytes, secret), "{secret} is in app-requests.db");
    }
}

#[tokio::test]
async fn k2s_503_and_502_give_rows_with_k2s_code() {
    let _home = crate::test_support::TempHome::new();
    // Nothing live: K2's 503.
    let down = "req-log-down";
    let out = ask(down, Some(ip(VISITOR)), "POST /upload?sig=QS HTTP/1.1\r\nHost: a\r\nContent-Length: 0\r\n\r\n").await;
    assert!(out.starts_with("HTTP/1.1 503"), "{out}");
    let r = rows_n(down, 1).await.remove(0);
    assert_eq!((r.method.as_str(), r.path.as_str(), r.status), ("POST", "/upload", 503), "{r:?}");
    assert_eq!(r.k2_code.as_deref(), Some(k2_core::app_requests::codes::APP_NOT_RUNNING));
    assert_eq!(r.bytes_out, out.len() as u64);
    assert!(r.visitor.is_some() && r.deployment.is_none(), "{r:?}");

    // A backend port nobody listens on: K2's 502.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let gone = "req-log-dead";
    let _ = crate::apps::router::switch(gone, dead);
    let out = ask(gone, None, "GET /api/items?page=2 HTTP/1.1\r\nHost: a\r\n\r\n").await;
    assert!(out.starts_with("HTTP/1.1 502"), "{out}");
    let r = rows_n(gone, 1).await.remove(0);
    assert_eq!((r.method.as_str(), r.path.as_str(), r.status), ("GET", "/api/items", 502), "{r:?}");
    assert_eq!(r.k2_code.as_deref(), Some(k2_core::app_requests::codes::APP_NOT_ANSWERING));
    assert_eq!(r.visitor, None, "no vouched address, no key");
    assert!(!dump().contains("page=2"));
}

/// The hot path never waits on SQLite: with the file's write lock held
/// (the writer stuck in its insert), visitors are still answered at full
/// speed, and a full queue drops rows (counted) instead of blocking.
#[tokio::test]
async fn forwarding_never_waits_on_a_blocked_writer() {
    let _home = crate::test_support::TempHome::new();
    let id = "req-log-blocked";
    let _ = crate::apps::router::switch(id, crate::client_ip::tests::echo_app().await);
    // The writer has the file open.
    ask(id, None, "GET /warm HTTP/1.1\r\nHost: a\r\n\r\n").await;
    rows_n(id, 1).await;
    let lock = store::open(&store::db_path()).unwrap();
    lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let t0 = Instant::now();
    for i in 0..20 {
        let out = ask(id, None, &format!("GET /while-blocked/{i} HTTP/1.1\r\nHost: a\r\n\r\n")).await;
        assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    }
    let took = t0.elapsed();
    assert!(took < Duration::from_secs(3), "20 requests took {took:?} while the writer was blocked");
    let written: i64 = lock.query_row("SELECT COUNT(*) FROM requests WHERE path LIKE '/while-blocked/%'", [], |r| r.get(0)).unwrap();
    assert_eq!(written, 0, "the writer really was blocked");
    // Flood the queue: every call returns at once; past the cap rows drop.
    let before = dropped();
    let t1 = Instant::now();
    for _ in 0..(QUEUE_CAP + 2 * BATCH + 10) {
        record(Pending {
            app_id: std::sync::Arc::from("req-log-flood"),
            port: None,
            rec: crate::client_ip::RequestRecord {
                at_ms: now_ms(),
                method: "GET".into(),
                path: "/flood".into(),
                status: 200,
                k2_code: None,
                bytes_in: 1,
                bytes_out: 1,
                head_ms: Some(0),
                total_ms: 0,
                closed_early: None,
                ws: None,
                visitor: None,
            },
        });
    }
    assert!(t1.elapsed() < Duration::from_secs(2), "record() blocked: {:?}", t1.elapsed());
    assert!(dropped() > before, "a full queue drops and counts");
    lock.execute_batch("COMMIT").unwrap();
    drop(lock);
    // Queued before the flood: all twenty land once the lock is gone.
    let got = rows_n(id, 21).await;
    assert_eq!(got.iter().filter(|r| r.path.starts_with("/while-blocked/")).count(), 20, "{got:?}");
}

#[tokio::test]
async fn the_writer_prunes_by_age_and_by_count() {
    let _home = crate::test_support::TempHome::new();
    let day = 86_400_000i64;
    let now = now_ms();
    let row = |at_ms: i64, app: &str, path: &str| Request {
        at_ms,
        app_id: app.into(),
        deployment: None,
        method: "GET".into(),
        path: path.into(),
        status: 200,
        k2_code: None,
        bytes_in: 1,
        bytes_out: 1,
        head_ms: Some(1),
        total_ms: 1,
        closed_early: None,
        ws: None,
        visitor: None,
    };
    {
        let c = store::open(&store::db_path()).unwrap();
        let mut rows = vec![row(now - 8 * day, "prune-a", "/old"), row(now - 8 * day, "prune-b", "/old")];
        rows.extend((0..6).map(|i| row(now - 1000 + i, "prune-a", &format!("/n{i}"))));
        rows.push(row(now, "prune-b", "/new"));
        store::append(&c, &rows).unwrap();
    }
    // The daemon's numbers: only the 8-day-old rows go.
    assert_eq!(prune_now(now, store::KEEP_DAYS, store::MAX_ROWS_PER_APP), 2);
    assert_eq!(list("prune-a").len(), 6);
    assert_eq!(list("prune-b").iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), vec!["/new"]);
    // A smaller cap: the newest 3 of prune-a stay.
    assert_eq!(prune_now(now, store::KEEP_DAYS, 3), 3);
    assert_eq!(list("prune-a").iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), vec!["/n5", "/n4", "/n3"]);
    assert_eq!(list("prune-b").len(), 1);
}

#[test]
fn the_first_line_gives_method_and_a_safe_path_only() {
    assert_eq!(first_line(b"POST /up?t=1 HTTP/1.1\r\nCookie: a=b\r\n\r\n"), ("POST".into(), "/up".into()));
    assert_eq!(first_line(b"\x16\x03\x01binary"), (String::new(), String::new()));
    assert_eq!(first_line(b""), (String::new(), String::new()));
}
