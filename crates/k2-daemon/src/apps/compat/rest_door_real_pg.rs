//! `/rest/v1` end to end against a real Postgres: HTTP in on an App's
//! public host (and its link), the caller resolved, the grammar run through
//! the pool as that caller, RLS deciding. `#[ignore]`: REALPG_COMPAT=1 on a
//! compute node.

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

use super::super::door::tests::{cfg_for, fx, Fx};
use super::super::door::{self, publishable_key, set_test_config, SESSION_COOKIE};
use crate::apps::router::FirstRequest;
use super::super::pool::{set_test_target, Target};
use k2_rest::testkit;

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Value,
}

impl Reply {
    fn header(&self, n: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(n)).map(|(_, v)| v.as_str())
    }
}

async fn read_reply(c: &mut DuplexStream) -> Reply {
    let mut buf = Vec::new();
    let mut b = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = tokio::time::timeout(std::time::Duration::from_secs(20), c.read(&mut b)).await.expect("an answer in time").unwrap();
        assert!(n > 0, "connection closed before an answer: {}", String::from_utf8_lossy(&buf));
        buf.extend_from_slice(&b[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let status: u16 = lines.next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.parse().unwrap()).unwrap_or(0);
    while buf.len() < head_end + len {
        let n = c.read(&mut b).await.unwrap();
        assert!(n > 0, "body cut short");
        buf.extend_from_slice(&b[..n]);
    }
    let raw = &buf[head_end..head_end + len];
    let body = if raw.is_empty() { Value::Null } else { serde_json::from_slice(raw).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(raw).into_owned())) };
    Reply { status, headers, body }
}

fn request(method: &str, target: &str, headers: &[(&str, &str)], body: &str) -> String {
    let mut s = format!("{method} {target} HTTP/1.1\r\nHost: shop.example\r\n");
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() {
        s.push_str(&format!("Content-Type: application/json\r\nContent-Length: {}\r\n", body.len()));
    }
    s.push_str("\r\n");
    s.push_str(body);
    s
}

struct Rig {
    f: Fx,
    pk: String,
    cookie: String,
}

fn rig(tag: &str) -> Rig {
    assert_eq!(std::env::var("REALPG_COMPAT").as_deref(), Ok("1"), "REALPG_COMPAT=1 marks the throwaway job");
    let f = fx(tag);
    let s = testkit::shared();
    let db = s.fresh_db(&format!("door_{tag}"));
    s.su(&db, &[include_str!("../../../../k2-rest/src/fixture.sql")]);
    s.su(
        &db,
        &[&format!(
            "CREATE TABLE public.notes (id serial PRIMARY KEY, owner uuid NOT NULL DEFAULT auth.uid(), body text NOT NULL);
             ALTER TABLE public.notes ENABLE ROW LEVEL SECURITY;
             CREATE POLICY own ON public.notes FOR ALL TO authenticated USING (auth.uid() = owner) WITH CHECK (auth.uid() = owner);
             GRANT ALL ON public.notes TO authenticated, service_role;
             GRANT USAGE ON SEQUENCE public.notes_id_seq TO authenticated, service_role;
             INSERT INTO public.notes (owner, body) VALUES ('{}', 'mine 1'), ('{}', 'mine 2'), ('99999999-9999-4999-8999-999999999999', 'theirs');",
            f.ann_id, f.ann_id
        )],
    );
    let mut cfg = cfg_for(&f.app, &db);
    cfg.anon = true;
    set_test_config(&f.app, Some((cfg, f.svc.clone())));
    set_test_target(
        &f.app,
        Target { host: "127.0.0.1".into(), port: s.port, db: db.clone(), user: format!("{db}_authenticator"), password: testkit::AUTHENTICATOR_PASSWORD.into(), size: 4 },
    );
    let pk = publishable_key(&f.svc);
    let cookie = format!("{SESSION_COOKIE}={}", f.web);
    Rig { f, pk, cookie }
}

/// One request through the link's door.
async fn link_one(ctx: door::Ctx, raw: String) -> Reply {
    let (mut client, mut server) = tokio::io::duplex(1 << 20);
    client.write_all(raw.as_bytes()).await.unwrap();
    let (head, len) = super::super::http::complete_head(raw.as_bytes()).unwrap();
    // On the test's own thread: its scoped K2 records go with the work.
    let (kept, reply) = tokio::join!(door::serve_one(&mut server, &head, len, &ctx), read_reply(&mut client));
    assert!(kept, "kept alive");
    reply
}

/// One connection on the App's public host: send `reqs` one after another
/// (keep-alive), read each answer.
async fn public(r: &Rig, reqs: Vec<String>) -> Vec<Reply> {
    let (mut client, mut server) = tokio::io::duplex(1 << 20);
    let app = r.f.app.clone();
    let task = tokio::spawn(async move { door::first_request(&mut server, &app, &app, None, None).await });
    let mut out = Vec::new();
    for q in reqs {
        client.write_all(q.as_bytes()).await.unwrap();
        out.push(read_reply(&mut client).await);
    }
    drop(client);
    let _ = task.await;
    out
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1"]
async fn real_pg_the_public_host_serves_supabase_js_shapes_as_the_caller() {
    let r = rig("pub");
    let pk = r.pk.as_str();
    let ck = r.cookie.as_str();
    let origin = "https://shop.example";
    let replies = public(
        &r,
        vec![
            // The browser, signed in: RLS shows only Ann's notes.
            request("GET", "/rest/v1/notes?select=body&order=id.asc", &[("apikey", pk), ("Authorization", &format!("Bearer {pk}")), ("Cookie", ck)], ""),
            // A write with the cookie: the owner comes from auth.uid().
            request("POST", "/rest/v1/notes?select=body,owner", &[("apikey", pk), ("Cookie", ck), ("Origin", origin), ("Prefer", "return=representation")], r#"{"body":"new"}"#),
            // A write for someone else is refused by the policy: 403.
            request("POST", "/rest/v1/notes", &[("apikey", pk), ("Cookie", ck), ("Origin", origin)], r#"{"body":"forged","owner":"99999999-9999-4999-8999-999999999999"}"#),
            // Signed out: the anon lane (on here) reads what anon may.
            request("GET", "/rest/v1/projects?select=id,owner:profiles!owner_id(name)&id=lt.3&order=id", &[("apikey", pk)], ""),
            // anon has no grant on notes: Postgres's 42501, 401 for anon.
            request("GET", "/rest/v1/notes", &[("apikey", pk)], ""),
            // count + head, as supabase-js sends `{count: 'exact', head: true}`.
            request("HEAD", "/rest/v1/projects?select=*", &[("apikey", pk), ("Prefer", "count=exact")], ""),
            // A refused part of the grammar.
            request("GET", "/rest/v1/projects?title=fts.alpha", &[("apikey", pk)], ""),
            // rpc.
            request("POST", "/rest/v1/rpc/add", &[("apikey", pk)], r#"{"a":1,"b":2}"#),
        ],
    )
    .await;
    assert_eq!((replies[0].status, replies[0].body.clone()), (200, json!([{"body": "mine 1"}, {"body": "mine 2"}])), "{}", replies[0].body);
    assert_eq!(replies[1].status, 201, "{}", replies[1].body);
    assert_eq!(replies[1].body, json!([{"body": "new", "owner": r.f.ann_id}]));
    assert_eq!((replies[2].status, replies[2].body["code"].as_str()), (403, Some("42501")), "{}", replies[2].body);
    assert_eq!(replies[3].body, json!([{"id": 1, "owner": {"name": "Ann"}}, {"id": 2, "owner": {"name": "Bo"}}]), "{}", replies[3].body);
    assert_eq!((replies[4].status, replies[4].body["code"].as_str()), (401, Some("42501")), "{}", replies[4].body);
    assert_eq!((replies[5].status, replies[5].header("Content-Range")), (200, Some("0-4/5")));
    assert_eq!(replies[5].body, Value::Null, "HEAD has no body");
    assert_eq!((replies[6].status, replies[6].body["code"].as_str()), (400, Some("K2REST001")));
    assert_eq!((replies[7].status, replies[7].body.clone()), (200, json!(3)));
    assert!(super::unsupported(&r.f.app).iter().any(|(m, n)| m.contains("full-text") && *n >= 1), "{:?}", super::unsupported(&r.f.app));
    set_test_config(&r.f.app, None);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1"]
async fn real_pg_the_link_takes_the_server_key_and_the_persons_pass() {
    let r = rig("link");
    let (cfg, svc) = door::config_for(&r.f.app).expect("config");
    let ctx = door::Ctx { cfg, service_id: svc, ingress: door::Ingress::Link };
    let one = |raw: String| link_one(ctx.clone(), raw);
    let sk = r.f.service_key.clone();
    // The server key: service_role sees every note.
    let a = one(request("GET", "/rest/v1/notes?select=body&order=id", &[("apikey", &sk), ("Authorization", &format!("Bearer {sk}"))], "")).await;
    assert_eq!(a.body, json!([{"body": "mine 1"}, {"body": "mine 2"}, {"body": "theirs"}]), "{}", a.body);
    // Server code as the person: the pass it read from the cookie.
    let a = one(request("GET", "/rest/v1/notes?select=body&order=id", &[("apikey", &r.pk), ("Authorization", &format!("Bearer {}", r.f.web))], "")).await;
    assert_eq!(a.body, json!([{"body": "mine 1"}, {"body": "mine 2"}]), "{}", a.body);
    // A schema change shows on the next request (a miss reloads the cache).
    testkit::shared().su(&ctx.cfg.database, &["CREATE TABLE public.late (id int PRIMARY KEY)", "GRANT SELECT ON public.late TO service_role"]);
    tokio::time::sleep(super::MISS_RELOAD).await;
    let a = one(request("GET", "/rest/v1/late", &[("apikey", &sk)], "")).await;
    assert_eq!((a.status, a.body.clone()), (200, json!([])), "{}", a.body);
    set_test_config(&r.f.app, None);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real Postgres: REALPG_COMPAT=1"]
async fn real_pg_door_requests_and_app_requests_share_a_connection() {
    let r = rig("mix");
    let (mut client, mut server) = tokio::io::duplex(1 << 20);
    let app = r.f.app.clone();
    let task = tokio::spawn(async move { door::first_request(&mut server, &app, &app, None, None).await });
    let q = request("GET", "/rest/v1/orgs?select=name&order=id&limit=1", &[("apikey", &r.pk)], "");
    let app_req = "GET /dashboard HTTP/1.1\r\nHost: shop.example\r\n\r\n";
    client.write_all(q.as_bytes()).await.unwrap();
    let a = read_reply(&mut client).await;
    assert_eq!(a.body, json!([{"name": "Acme"}]));
    client.write_all(q.as_bytes()).await.unwrap();
    let a = read_reply(&mut client).await;
    assert_eq!(a.status, 200);
    client.write_all(app_req.as_bytes()).await.unwrap();
    match task.await.unwrap() {
        FirstRequest::Pass(b) => assert_eq!(String::from_utf8(b).unwrap(), app_req),
        FirstRequest::Answered => panic!("the App's request must go to the App"),
    }
    set_test_config(&r.f.app, None);
}
