use super::*;

fn mem() -> Connection {
    let c = Connection::open_in_memory().unwrap();
    migrate(&c).unwrap();
    c
}

fn req(at_ms: i64, app: &str, method: &str, path: &str, status: u16) -> Request {
    Request {
        at_ms,
        app_id: app.into(),
        deployment: Some("d-1".into()),
        method: method.into(),
        path: path.into(),
        status,
        k2_code: None,
        bytes_in: 10,
        bytes_out: 20,
        head_ms: Some(3),
        total_ms: 4,
        closed_early: None,
        ws: None,
        visitor: None,
    }
}

fn q(app: &str) -> Query<'_> {
    Query { app_id: app, limit: 1000, ..Default::default() }
}

fn count(c: &Connection, app: &str) -> i64 {
    c.query_row("SELECT COUNT(*) FROM requests WHERE app_id = ?1", [app], |r| r.get(0)).unwrap()
}

#[test]
fn schema_is_versioned_and_a_newer_file_is_refused() {
    let c = mem();
    let v: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(v, SCHEMA_VERSION);
    migrate(&c).unwrap();
    c.execute_batch("PRAGMA user_version = 99").unwrap();
    assert!(migrate(&c).is_err(), "a newer schema is refused");
}

#[test]
fn open_creates_an_owner_only_file_beside_the_audit() {
    let dir = std::env::temp_dir().join(format!("k2-app-requests-{}", uuid::Uuid::new_v4().simple()));
    let p = dir.join("apps").join("app-requests.db");
    let c = open(&p).unwrap();
    append(&c, &[req(1, "jobs", "GET", "/", 200)]).unwrap();
    drop(c);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let c = open(&p).unwrap();
    assert_eq!(list(&c, &q("jobs")).unwrap().len(), 1, "reopened, the row is there");
    drop(c);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(db_path().ends_with("apps/app-requests.db"), "{}", db_path().display());
    assert_eq!(db_path().parent(), crate::app_audit::db_path().parent(), "beside app-audit.db");
}

#[test]
fn safe_path_keeps_no_query_fragment_params_or_host() {
    assert_eq!(safe_path("/upload?token=k2skn_abc&x=1"), "/upload");
    assert_eq!(safe_path("/a/b#frag"), "/a/b");
    assert_eq!(safe_path("/cart;jsessionid=ABC123/items"), "/cart/items");
    assert_eq!(safe_path("http://evil.example:8080/x/y?q=1"), "/x/y");
    assert_eq!(safe_path("https://h.example"), "/");
    assert_eq!(safe_path("*"), "*");
    assert_eq!(safe_path("/invite/k2inv_secretsecret/accept"), "/invite/[redacted]/accept");
    assert_eq!(safe_path("/s/eyJhbGciOiJIUzI1NiJ9.e30.sig"), "/s/[redacted]");
    assert_eq!(safe_path("/a\u{0007}b"), "/ab");
    let long = format!("/{}", "a".repeat(2000));
    let got = safe_path(&long);
    assert_eq!(got.chars().count(), MAX_PATH_CHARS + 1);
    assert!(got.ends_with('…'));
}

#[test]
fn visitor_keys_group_one_day_and_never_show_the_address() {
    let k1 = [7u8; 32];
    let k2 = [8u8; 32];
    let ip: IpAddr = "203.0.113.7".parse().unwrap();
    let a = visitor_key(&k1, ip);
    assert_eq!(a.len(), 16);
    assert!(a.bytes().all(|b| b.is_ascii_hexdigit()), "{a}");
    assert!(!a.contains('.') && !a.contains("203.0.113"), "{a}");
    assert_eq!(a, visitor_key(&k1, ip), "the same visitor groups");
    assert_eq!(a, visitor_key(&k1, "::ffff:203.0.113.7".parse().unwrap()), "mapped v4 is the same visitor");
    assert_ne!(a, visitor_key(&k1, "203.0.113.8".parse().unwrap()), "another visitor");
    assert_ne!(a, visitor_key(&k2, ip), "another day's key");
}

#[test]
fn list_filters_by_time_status_path_and_method_newest_first() {
    let c = mem();
    append(
        &c,
        &[
            req(1_000, "jobs", "GET", "/", 200),
            req(2_000, "jobs", "POST", "/upload", 413),
            req(3_000, "jobs", "POST", "/upload/big", 502),
            req(4_000, "jobs", "GET", "/api/x", 404),
            req(5_000, "other", "POST", "/upload", 500),
        ],
    )
    .unwrap();
    let all = list(&c, &q("jobs")).unwrap();
    assert_eq!(all.iter().map(|r| r.at_ms).collect::<Vec<_>>(), vec![4_000, 3_000, 2_000, 1_000], "newest first, one App");
    let st = |s: &str| list(&c, &Query { status: StatusFilter::parse(s), ..q("jobs") }).unwrap().iter().map(|r| r.status).collect::<Vec<_>>();
    assert_eq!(st("4xx"), vec![404, 413]);
    assert_eq!(st("5XX"), vec![502]);
    assert_eq!(st("200"), vec![200]);
    assert_eq!(StatusFilter::parse("6xx"), None);
    assert_eq!(StatusFilter::parse("abc"), None);
    let p = list(&c, &Query { path_prefix: Some("/upload"), ..q("jobs") }).unwrap();
    assert_eq!(p.iter().map(|r| r.path.as_str()).collect::<Vec<_>>(), vec!["/upload/big", "/upload"]);
    let m = list(&c, &Query { method: Some("post"), ..q("jobs") }).unwrap();
    assert_eq!(m.len(), 2);
    let t = list(&c, &Query { since_ms: Some(2_000), until_ms: Some(3_000), ..q("jobs") }).unwrap();
    assert_eq!(t.iter().map(|r| r.at_ms).collect::<Vec<_>>(), vec![3_000, 2_000]);
    let lim = list(&c, &Query { limit: 1, ..q("jobs") }).unwrap();
    assert_eq!(lim.len(), 1);
    let tot = totals(&c, &q("jobs")).unwrap();
    assert_eq!((tot.requests, tot.ok, tot.client_errors, tot.server_errors, tot.bytes_in), (4, 1, 2, 1, 40));
}

#[test]
fn prune_drops_rows_past_the_age_and_past_the_per_app_count() {
    let c = mem();
    let day = 86_400_000i64;
    let now = 100 * day;
    // Two Apps: old rows for both, and more than the cap for one.
    let mut rows = vec![req(now - 8 * day, "jobs", "GET", "/old", 200), req(now - 8 * day, "other", "GET", "/old", 200)];
    for i in 0..10 {
        rows.push(req(now - day + i, "jobs", "GET", &format!("/r{i}"), 200));
    }
    rows.push(req(now - day, "other", "GET", "/new", 200));
    append(&c, &rows).unwrap();
    let gone = prune(&c, now, KEEP_DAYS, 4).unwrap();
    assert_eq!(gone, 2 + 6, "two past 7 days, six past the cap");
    assert_eq!(count(&c, "jobs"), 4);
    assert_eq!(count(&c, "other"), 1);
    let kept: Vec<String> = list(&c, &q("jobs")).unwrap().into_iter().map(|r| r.path).collect();
    assert_eq!(kept, vec!["/r9", "/r8", "/r7", "/r6"], "the newest are kept");
    assert!(list(&c, &q("other")).unwrap().iter().all(|r| r.path != "/old"));
    // Under both limits: nothing goes.
    assert_eq!(prune(&c, now, KEEP_DAYS, 4).unwrap(), 0);
}

#[test]
fn the_numbers_are_the_documented_ones() {
    assert_eq!(KEEP_DAYS, 7);
    assert_eq!(MAX_ROWS_PER_APP, 200_000);
}
