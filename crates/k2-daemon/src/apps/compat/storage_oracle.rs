//! The oracle diff (PRD §12 "Oracle tests (S4, S5)", lane D): the same
//! storage requests to K2's `/storage/v1` door and to a real Supabase
//! Storage (storage-api, file backend), loaded with the same fixture, and
//! every difference in status and body printed. A test oracle only: it is
//! never shipped and never runs on a box.
//!
//! `#[ignore]`: runs only from `scripts/test/storage-oracle.sh` on a Linux
//! compute job, which starts Postgres and storage-api and sets
//! `ORACLE_URL` (storage-api's base, e.g. `http://127.0.0.1:5000`),
//! `ORACLE_DB_URL` (its database, as a superuser), `ORACLE_JWT_SECRET`,
//! plus `REALPG_STORAGE=1` and `REALPG_BIN` for K2's own throwaway server.
//!
//! Values that differ by nature are normalised before comparing: ids and
//! versions (uuids), timestamps, and signed tokens. The agreed list of
//! differences ([`AGREED`]) starts empty, as the PRD says: any other
//! difference fails the test, and every one is printed first.

use std::collections::HashMap;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::storage_real_pg::{call, person, req, storage_app, Server};
use k2_core::brand::PRODUCT_NAME;

/// Differences K2 and Supabase agree to have: `(step, what)`. Empty.
const AGREED: &[(&str, &str)] = &[];

const USER_A: &str = "0a0a0a0a-0000-4000-8000-00000000000a";
const USER_B: &str = "0b0b0b0b-0000-4000-8000-00000000000b";
const ORG1: &str = "11111111-1111-4111-8111-111111111111";
const ORG2: &str = "22222222-2222-4222-8222-222222222222";

fn env(k: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| panic!("{k} is not set: run scripts/test/storage-oracle.sh"))
}

/// A Supabase JWT for one identity.
fn jwt(secret: &str, who: &str) -> String {
    use jsonwebtoken::{encode, EncodingKey, Header};
    let now = k2_core::deploy::now();
    let claims = match who {
        "a" => json!({"sub": USER_A, "role": "authenticated", "aud": "authenticated", "email": format!("{USER_A}@example.com"), "iat": now, "exp": now + 3600}),
        "b" => json!({"sub": USER_B, "role": "authenticated", "aud": "authenticated", "email": format!("{USER_B}@example.com"), "iat": now, "exp": now + 3600}),
        "svc" => json!({"role": "service_role", "iat": now, "exp": now + 3600}),
        _ => json!({"role": "anon", "iat": now, "exp": now + 3600}),
    };
    encode(&Header::default(), &claims, &EncodingKey::from_secret(secret.as_bytes())).expect("jwt")
}

/// One request to storage-api: `(status, headers, body)`.
async fn oracle(base: &str, raw: Vec<u8>) -> (u16, HashMap<String, String>, Vec<u8>) {
    let hostport = base.trim_start_matches("http://").trim_end_matches('/').to_string();
    let mut s = tokio::net::TcpStream::connect(&hostport).await.unwrap_or_else(|e| panic!("storage-api at {hostport}: {e}"));
    s.write_all(&raw).await.expect("send");
    let mut out = Vec::new();
    s.read_to_end(&mut out).await.expect("answer");
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").expect("an answer head") + 4;
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    let status: u16 = head.split_whitespace().nth(1).expect("status").parse().expect("status number");
    let headers: HashMap<String, String> = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    let mut body = out[split..].to_vec();
    if headers.get("transfer-encoding").is_some_and(|t| t.contains("chunked")) {
        body = dechunk(&body);
    }
    (status, headers, body)
}

fn dechunk(mut b: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(i) = b.windows(2).position(|w| w == b"\r\n") else { break };
        let size = usize::from_str_radix(String::from_utf8_lossy(&b[..i]).split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        if size == 0 {
            break;
        }
        let start = i + 2;
        out.extend_from_slice(&b[start..(start + size).min(b.len())]);
        b = &b[(start + size + 2).min(b.len())..];
    }
    out
}

/// Ids, versions, times and tokens out; everything else as it is.
fn norm(v: &Value) -> Value {
    match v {
        Value::String(s) => {
            if uuid::Uuid::parse_str(s).is_ok() {
                return json!("<uuid>");
            }
            if s.len() >= 20 && s.as_bytes().get(4) == Some(&b'-') && s.contains('T') && (s.ends_with('Z') || s.contains('+')) {
                return json!("<time>");
            }
            if let Some(i) = s.find("token=") {
                return json!(format!("{}token=<token>", &s[..i]));
            }
            if s.matches('.').count() == 2 && s.len() > 60 && !s.contains(' ') {
                return json!("<token>");
            }
            v.clone()
        }
        Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
        Value::Object(m) => {
            let mut out = serde_json::Map::new();
            for (k, x) in m {
                let x = match k.as_str() {
                    "lastModified" | "version" => json!("<v>"),
                    _ => norm(x),
                };
                out.insert(k.clone(), x);
            }
            Value::Object(out)
        }
        other => other.clone(),
    }
}

fn body_of(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or_else(|_| json!({ "bytes": String::from_utf8_lossy(b) }))
}

/// One step of the script.
struct Step {
    name: &'static str,
    who: &'static str,
    method: &'static str,
    path: String,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
    /// Download answers: these headers must match too.
    compare_headers: &'static [&'static str],
}

fn step(name: &'static str, who: &'static str, method: &'static str, path: String, headers: Vec<(&'static str, String)>, body: &[u8]) -> Step {
    Step { name, who, method, path, headers, body: body.to_vec(), compare_headers: &[] }
}

fn dl(name: &'static str, who: &'static str, path: String, headers: Vec<(&'static str, String)>) -> Step {
    Step { name, who, method: "GET", path, headers, body: vec![], compare_headers: &["content-type", "cache-control", "content-range", "etag"] }
}

/// The fixture on storage-api's database: the same as K2's
/// (`storage_real_pg::app_sql`), with `auth.uid()` as Supabase's own.
async fn load_oracle_fixture(db_url: &str) {
    let (c, conn) = tokio_postgres::connect(db_url, tokio_postgres::NoTls).await.expect("the oracle's database");
    tokio::spawn(conn);
    c.batch_execute(
        "DROP SCHEMA IF EXISTS auth CASCADE; DROP TABLE IF EXISTS public.members CASCADE; \
         SET storage.allow_delete_query = 'true'; DELETE FROM storage.objects; DELETE FROM storage.buckets;",
    )
    .await
    .expect("reset the oracle");
    let auth = r#"
CREATE SCHEMA auth;
CREATE FUNCTION auth.uid() RETURNS uuid LANGUAGE sql STABLE AS
  $$ SELECT nullif(current_setting('request.jwt.claims', true)::jsonb ->> 'sub', '')::uuid $$;
GRANT USAGE ON SCHEMA auth TO anon, authenticated, service_role;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA auth TO anon, authenticated, service_role;
"#;
    c.batch_execute(auth).await.expect("auth fixture on the oracle");
    c.batch_execute(&super::storage_real_pg::app_sql_for_oracle()).await.expect("the app's storage policies on the oracle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs only from scripts/test/storage-oracle.sh (Postgres + storage-api on a compute node)"]
async fn oracle_storage_diff() {
    let (base, db_url, secret) = (env("ORACLE_URL"), env("ORACLE_DB_URL"), env("ORACLE_JWT_SECRET"));
    let _home = crate::test_support::TempHome::new();
    load_oracle_fixture(&db_url).await;
    let srv = Server::start();
    let app = "oracle";
    storage_app(&srv, app, "st_oracle", true).await;
    super::storage_door::set_test_callers(
        [("Bearer test-a".to_string(), person(USER_A)), ("Bearer test-b".to_string(), person(USER_B)), ("Bearer test-svc".to_string(), super::storage_pg::Caller::service("svc"))]
            .into_iter()
            .collect(),
    );

    let p1 = format!("{ORG1}/sub1/p1.jpg");
    let p2 = format!("{ORG1}/sub1/p2.jpg");
    let p4 = format!("{ORG1}/sub1/p4.jpg");
    let jpeg = vec![("Content-Type", "image/jpeg".to_string())];
    let jsonh = vec![("Content-Type", "application/json".to_string())];
    let bnd = "XyZboundary";
    let mut mp = Vec::new();
    mp.extend_from_slice(format!("--{bnd}\r\nContent-Disposition: form-data; name=\"cacheControl\"\r\n\r\n3600\r\n").as_bytes());
    mp.extend_from_slice(format!("--{bnd}\r\nContent-Disposition: form-data; name=\"\"; filename=\"p2.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").as_bytes());
    mp.extend_from_slice(b"multi\r\npart bytes");
    mp.extend_from_slice(format!("\r\n--{bnd}--\r\n").as_bytes());

    let steps = vec![
        step("upload", "a", "POST", format!("/object/photos/{p1}"), jpeg.clone(), b"first bytes"),
        step("upload duplicate", "a", "POST", format!("/object/photos/{p1}"), jpeg.clone(), b"again"),
        step("upload upsert", "a", "POST", format!("/object/photos/{p1}"), vec![("Content-Type", "image/jpeg".into()), ("x-upsert", "true".into())], b"second bytes"),
        dl("download", "a", format!("/object/photos/{p1}"), vec![]),
        dl("download authenticated/", "a", format!("/object/authenticated/photos/{p1}"), vec![]),
        dl("download, other org", "b", format!("/object/photos/{p1}"), vec![]),
        step("upload, other org", "b", "POST", format!("/object/photos/{ORG1}/sub1/evil.jpg"), jpeg.clone(), b"x"),
        step("upload, own org", "b", "POST", format!("/object/photos/{ORG2}/s/b.jpg"), jpeg.clone(), b"b's"),
        dl("download as service", "svc", format!("/object/photos/{p1}"), vec![]),
        dl("download as anon", "anon", format!("/object/photos/{p1}"), vec![]),
        step("list folder", "a", "POST", "/object/list/photos".into(), jsonh.clone(), json!({"prefix": ORG1}).to_string().as_bytes()),
        step("list files", "a", "POST", "/object/list/photos".into(), jsonh.clone(), json!({"prefix": format!("{ORG1}/sub1")}).to_string().as_bytes()),
        step("list, other org", "b", "POST", "/object/list/photos".into(), jsonh.clone(), json!({"prefix": ORG1}).to_string().as_bytes()),
        step("upload multipart", "a", "POST", format!("/object/photos/{p2}"), vec![("Content-Type", format!("multipart/form-data; boundary={bnd}"))], &mp),
        dl("download multipart", "a", format!("/object/photos/{p2}"), vec![]),
        dl("range", "a", format!("/object/photos/{p1}"), vec![("Range", "bytes=2-5".into())]),
        step("info", "a", "GET", format!("/object/info/photos/{p1}"), vec![], b""),
        step("sign", "a", "POST", format!("/object/sign/photos/{p1}"), jsonh.clone(), br#"{"expiresIn": 3600}"#),
        step("sign, other org", "b", "POST", format!("/object/sign/photos/{p1}"), jsonh.clone(), br#"{"expiresIn": 60}"#),
        step("sign many", "a", "POST", "/object/sign/photos".into(), jsonh.clone(), json!({"expiresIn": 60, "paths": [p1, format!("{ORG2}/s/b.jpg")]}).to_string().as_bytes()),
        step("signed download, bad token", "none", "GET", format!("/object/sign/photos/{p1}?token=eyJhbGciOiJIUzI1NiJ9.eyJ1cmwiOiJ4In0.bad"), vec![], b""),
        step("upload sign", "a", "POST", format!("/object/upload/sign/photos/{p4}"), vec![], b""),
        step("upload sign, other org", "b", "POST", format!("/object/upload/sign/photos/{ORG1}/x.jpg"), vec![], b""),
        step("avatar upload", "a", "POST", "/object/avatars/u/a.png".into(), vec![("Content-Type", "image/png".into())], b"png!"),
        dl("public object", "none", "/object/public/avatars/u/a.png".into(), vec![]),
        step("public, private bucket", "none", "GET", format!("/object/public/photos/{p1}"), vec![], b""),
        step("bucket mime", "a", "POST", "/object/avatars/u/a.txt".into(), vec![("Content-Type", "text/plain".into())], b"no"),
        step("move", "a", "POST", "/object/move".into(), jsonh.clone(), json!({"bucketId": "photos", "sourceKey": p2, "destinationKey": format!("{ORG1}/sub1/p2-moved.jpg")}).to_string().as_bytes()),
        dl("moved", "a", format!("/object/photos/{ORG1}/sub1/p2-moved.jpg"), vec![]),
        dl("old name", "a", format!("/object/photos/{p2}"), vec![]),
        step("move into other org", "b", "POST", "/object/move".into(), jsonh.clone(), json!({"bucketId": "photos", "sourceKey": format!("{ORG2}/s/b.jpg"), "destinationKey": format!("{ORG1}/stolen.jpg")}).to_string().as_bytes()),
        step("remove, other org", "b", "DELETE", "/object/photos".into(), jsonh.clone(), json!({"prefixes": [p1]}).to_string().as_bytes()),
        step("remove", "a", "DELETE", "/object/photos".into(), jsonh.clone(), json!({"prefixes": [p1]}).to_string().as_bytes()),
        step("delete missing", "a", "DELETE", format!("/object/photos/{p1}"), vec![], b""),
        step("buckets", "a", "GET", "/bucket".into(), vec![], b""),
        step("bucket create", "svc", "POST", "/bucket".into(), jsonh.clone(), br#"{"name": "docs", "public": false, "file_size_limit": 1048576}"#),
        step("bucket taken", "svc", "POST", "/bucket".into(), jsonh.clone(), br#"{"name": "docs"}"#),
        step("over the limit", "svc", "POST", "/object/docs/big.bin".into(), vec![("Content-Type", "application/octet-stream".into())], &vec![0u8; (1 << 20) + 1]),
        dl("missing object", "a", format!("/object/photos/{ORG1}/nope.jpg"), vec![]),
        step("bad key", "a", "POST", format!("/object/photos/{ORG1}/a%23b.jpg"), jpeg.clone(), b"x"),
        step("no bucket", "a", "POST", "/object/nobucket/x.jpg".into(), jpeg.clone(), b"x"),
    ];

    let mut diffs: Vec<String> = Vec::new();
    let mut agreed_hit = 0;
    for s in &steps {
        let headers: Vec<(&str, &str)> = s.headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
        let k2_auth = match s.who {
            "a" => Some("test-a"),
            "b" => Some("test-b"),
            "svc" => Some("test-svc"),
            _ => None,
        };
        let (ks, kh, kb) = call(app, req(s.method, &s.path, k2_auth, &headers, &s.body)).await;
        let mut oh = headers.clone();
        let tok = jwt(&secret, s.who);
        let anon = jwt(&secret, "anon");
        if s.who != "none" {
            oh.push(("apikey", anon.as_str()));
        }
        let mut raw = req(s.method, &s.path, (s.who != "none").then_some(tok.as_str()), &oh, &s.body);
        // storage-api closes after the answer like K2 does.
        let pos = raw.windows(4).position(|w| w == b"\r\n\r\n").expect("head") + 2;
        let _ = raw.splice(pos..pos, b"Connection: close\r\n".iter().copied());
        // storage-api serves at its root; Supabase's gateway strips
        // `/storage/v1` in front of it.
        let line_end = raw.windows(2).position(|w| w == b"\r\n").expect("request line");
        let line = String::from_utf8_lossy(&raw[..line_end]).replacen(" /storage/v1/", " /", 1);
        let _ = raw.splice(..line_end, line.into_bytes());
        let (os, oh2, ob) = oracle(&base, raw).await;
        let (kj, oj) = (norm(&body_of(&kb)), norm(&body_of(&ob)));
        let mut what = Vec::new();
        if ks != os {
            what.push(format!("status {PRODUCT_NAME} {ks} vs Supabase {os}"));
        }
        if kj != oj {
            what.push(format!("body\n      {PRODUCT_NAME}:       {kj}\n      Supabase: {oj}"));
        }
        for h in s.compare_headers {
            if kh.get(*h) != oh2.get(*h) {
                what.push(format!("header {h}: {PRODUCT_NAME} {:?} vs Supabase {:?}", kh.get(*h), oh2.get(*h)));
            }
        }
        for w in what {
            let first = w.lines().next().unwrap_or("").to_string();
            if AGREED.iter().any(|(n, x)| *n == s.name && first.starts_with(x)) {
                agreed_hit += 1;
                continue;
            }
            diffs.push(format!("[{}] {}", s.name, w));
        }
    }
    eprintln!("oracle: {} steps, {} differences ({} agreed)", steps.len(), diffs.len(), agreed_hit);
    for d in &diffs {
        eprintln!("DIFF {d}");
    }
    drop(srv);
    assert!(diffs.is_empty(), "{} difference(s) from Supabase Storage (listed above)", diffs.len());
}
