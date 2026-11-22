//! The `/storage/v1` door against a real Postgres (PRD §12, lane D).
//!
//! `#[ignore]`: runs only where `REALPG_STORAGE=1` is set, with a Postgres
//! server's binaries (`REALPG_BIN`: `initdb`, `pg_ctl`, `postgres`; default
//! `/usr/lib/postgresql/17/bin`), as an ordinary user: a Linux compute job,
//! never the M5. It makes its own throwaway server under `/tmp` on a free
//! port and stops it at the end. Everything else is missing → it fails,
//! loudly; nothing is skipped.
//!
//! Run: `REALPG_STORAGE=1 cargo test -p k2-daemon --lib compat::storage_real_pg -- --ignored --nocapture --test-threads=1`
//!
//! The fixture makes what lane A makes when compat is turned on (the three
//! roles, `<db>_authenticator`, a minimal `auth.uid()` with Supabase's
//! definition), then K2's own `storage` schema through
//! `storage_bind::install_schema` as the migrator, then the reference
//! app's storage policy shape: the first path segment is the caller's
//! organisation.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::door_config::DoorConfig;
use super::storage_auth::Via;
use super::storage_bind::test_seams;
use super::storage_door;
use super::storage_pg::{Caller, DbRole, PgTarget};

fn bin() -> String {
    std::env::var("REALPG_BIN").unwrap_or_else(|_| "/usr/lib/postgresql/17/bin".into())
}

fn run(prog: &str, args: &[&str]) -> Result<String, String> {
    let out = k2_core::bg_proc::command(prog).args(args).env("LC_ALL", "C").output().map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)).trim().to_string())
    }
}

/// A throwaway server, stopped and removed on drop.
pub struct Server {
    pub root: PathBuf,
    pub port: u16,
}

impl Server {
    pub fn start() -> Server {
        assert_eq!(std::env::var("REALPG_STORAGE").as_deref(), Ok("1"), "REALPG_STORAGE=1 marks the throwaway job");
        assert!(Path::new(&format!("{}/initdb", bin())).exists(), "{}/initdb: install Postgres (REALPG_BIN)", bin());
        eprintln!("server: {}", run(&format!("{}/postgres", bin()), &["--version"]).expect("postgres --version"));
        let port = {
            let l = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("a free port");
            l.local_addr().expect("addr").port()
        };
        let root = PathBuf::from(format!("/tmp/k2st-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]));
        std::fs::create_dir_all(root.join("sock")).expect("server dir");
        let srv = Server { root, port };
        let data = srv.root.join("data").to_string_lossy().into_owned();
        run(&format!("{}/initdb", bin()), &["-D", &data, "-U", "postgres", "--auth=trust", "-E", "UTF8"]).expect("initdb");
        let opts = format!("-p {port} -k {} -c listen_addresses=127.0.0.1", srv.root.join("sock").display());
        let log = format!("{data}.log");
        run(&format!("{}/pg_ctl", bin()), &["-D", &data, "-o", &opts, "-l", &log, "-w", "start"]).expect("pg_ctl start");
        srv
    }

    pub fn target(&self, user: &str, db: &str) -> PgTarget {
        PgTarget { host: "127.0.0.1".into(), port: self.port, user: user.into(), password: "unused-trust".into(), dbname: db.into() }
    }

    pub async fn su(&self, db: &str, sql: &str) {
        let mut c = super::storage_pg::connect(&self.target("postgres", db)).await.expect("superuser connect");
        c.client().batch_execute(sql).await.unwrap_or_else(|e| panic!("superuser SQL failed: {e:?}\n{sql}"));
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let data = self.root.join("data").to_string_lossy().into_owned();
        let _ = run(&format!("{}/pg_ctl", bin()), &["-D", &data, "-m", "immediate", "-w", "stop"]);
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// What lane A makes when compat is turned on, for `db` (owned by
/// `<db>_migrator`).
pub fn lane_a_fixture(db: &str) -> String {
    format!(
        r#"
DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'anon') THEN CREATE ROLE anon NOLOGIN NOINHERIT; END IF; IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'authenticated') THEN CREATE ROLE authenticated NOLOGIN NOINHERIT; END IF; IF NOT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'service_role') THEN CREATE ROLE service_role NOLOGIN NOINHERIT BYPASSRLS; END IF; END $$;
CREATE ROLE {db}_migrator LOGIN;
CREATE ROLE {db}_authenticator LOGIN NOINHERIT;
GRANT anon, authenticated, service_role TO {db}_authenticator;
CREATE DATABASE {db} OWNER {db}_migrator;
REVOKE CONNECT ON DATABASE {db} FROM PUBLIC;
GRANT CONNECT ON DATABASE {db} TO {db}_migrator, {db}_authenticator;
"#
    )
}

/// `auth.uid()` as Supabase defines it (lane A's real one also checks
/// `session_user`), owned by the migrator.
const AUTH_SQL: &str = r#"
CREATE SCHEMA auth;
CREATE FUNCTION auth.uid() RETURNS uuid LANGUAGE sql STABLE AS
  $$ SELECT nullif(current_setting('request.jwt.claims', true)::jsonb ->> 'sub', '')::uuid $$;
CREATE FUNCTION auth.role() RETURNS text LANGUAGE sql STABLE AS
  $$ SELECT current_setting('request.jwt.claims', true)::jsonb ->> 'role' $$;
GRANT USAGE ON SCHEMA auth TO anon, authenticated, service_role;
GRANT EXECUTE ON ALL FUNCTIONS IN SCHEMA auth TO anon, authenticated, service_role;
"#;

const USER_A: &str = "0a0a0a0a-0000-4000-8000-00000000000a";
const USER_B: &str = "0b0b0b0b-0000-4000-8000-00000000000b";
const ORG1: &str = "11111111-1111-4111-8111-111111111111";
const ORG2: &str = "22222222-2222-4222-8222-222222222222";

/// The same fixture for the oracle (storage-api's own database).
pub fn app_sql_for_oracle() -> String {
    app_sql()
}

/// The reference app's policy shape: a member's organisation is the first
/// path segment. Public `avatars` reads for anyone.
fn app_sql() -> String {
    format!(
        r#"
CREATE TABLE public.members (user_id uuid PRIMARY KEY, org_id uuid NOT NULL);
INSERT INTO public.members VALUES ('{USER_A}', '{ORG1}'), ('{USER_B}', '{ORG2}');
CREATE FUNCTION public.user_org_ids() RETURNS SETOF text LANGUAGE sql STABLE SECURITY DEFINER AS
  $$ SELECT org_id::text FROM public.members WHERE user_id = auth.uid() $$;
GRANT EXECUTE ON FUNCTION public.user_org_ids() TO authenticated;
INSERT INTO storage.buckets (id, name, public) VALUES ('photos', 'photos', false);
INSERT INTO storage.buckets (id, name, public, allowed_mime_types) VALUES ('avatars', 'avatars', true, ARRAY['image/*']);
CREATE POLICY "org read" ON storage.objects FOR SELECT TO authenticated
  USING (bucket_id = 'photos' AND (storage.foldername(name))[1] IN (SELECT public.user_org_ids()));
CREATE POLICY "org insert" ON storage.objects FOR INSERT TO authenticated
  WITH CHECK (bucket_id = 'photos' AND (storage.foldername(name))[1] IN (SELECT public.user_org_ids()));
CREATE POLICY "org update" ON storage.objects FOR UPDATE TO authenticated
  USING (bucket_id = 'photos' AND (storage.foldername(name))[1] IN (SELECT public.user_org_ids()))
  WITH CHECK (bucket_id = 'photos' AND (storage.foldername(name))[1] IN (SELECT public.user_org_ids()));
CREATE POLICY "org delete" ON storage.objects FOR DELETE TO authenticated
  USING (bucket_id = 'photos' AND (storage.foldername(name))[1] IN (SELECT public.user_org_ids()));
CREATE POLICY "avatar read" ON storage.objects FOR SELECT TO anon, authenticated USING (bucket_id = 'avatars');
CREATE POLICY "avatar write" ON storage.objects FOR INSERT TO authenticated WITH CHECK (bucket_id = 'avatars');
CREATE POLICY "buckets read" ON storage.buckets FOR SELECT TO authenticated USING (true);
"#
    )
}

pub fn person(id: &str) -> Caller {
    Caller {
        role: DbRole::Authenticated,
        claims: json!({ "sub": id, "role": "authenticated", "aud": "authenticated", "email": format!("{id}@example.com") }),
        label: format!("guest:{id}"),
    }
}

/// Tests: the bearer → caller the door resolves (the credential rules are
/// `storage_auth`'s own tests).
fn callers() -> HashMap<String, Caller> {
    [
        ("Bearer test-a".to_string(), person(USER_A)),
        ("Bearer test-b".to_string(), person(USER_B)),
        ("Bearer test-svc".to_string(), Caller::service("service:test")),
    ]
    .into_iter()
    .collect()
}

/// One raw request through the door: `(HTTP status, headers, body)`.
pub async fn call(app_id: &str, raw: Vec<u8>) -> (u16, HashMap<String, String>, Vec<u8>) {
    let (head, len) = storage_door::door_head(&raw).expect("a door request head");
    let (client, mut server) = tokio::io::duplex(1 << 20);
    let app = app_id.to_string();
    let srv = tokio::spawn(async move {
        storage_door::serve(&mut server, &head, len, &app, Via::Public).await;
    });
    let writer = tokio::spawn(async move {
        let (mut rd, mut wr) = tokio::io::split(client);
        let w = async {
            let _ = wr.write_all(&raw).await;
        };
        let mut out = Vec::new();
        let r = rd.read_to_end(&mut out);
        let (_, got) = tokio::join!(w, r);
        got.expect("read the answer");
        out
    });
    let out = tokio::time::timeout(Duration::from_secs(60), writer).await.expect("the door answered in time").expect("client task");
    srv.await.expect("door task");
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").expect("an answer head") + 4;
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    let status: u16 = head.split_whitespace().nth(1).expect("status").parse().expect("status number");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|l| l.split_once(':').map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())))
        .collect();
    (status, headers, out[split..].to_vec())
}

pub fn req(method: &str, path: &str, auth: Option<&str>, extra: &[(&str, &str)], body: &[u8]) -> Vec<u8> {
    let mut h = format!("{method} /storage/v1{path} HTTP/1.1\r\nHost: jobs.box.k2.dev\r\n");
    if let Some(a) = auth {
        h.push_str(&format!("Authorization: Bearer {a}\r\n"));
    }
    for (k, v) in extra {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    if !extra.iter().any(|(k, _)| k.eq_ignore_ascii_case("transfer-encoding")) {
        h.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    h.push_str("\r\n");
    let mut v = h.into_bytes();
    v.extend_from_slice(body);
    v
}

fn j(b: &[u8]) -> Value {
    serde_json::from_slice(b).unwrap_or_else(|e| panic!("not JSON ({e}): {}", String::from_utf8_lossy(b)))
}

/// A fresh database with storage on, for `app_id`.
pub async fn storage_app(srv: &Server, app_id: &str, db: &str, anon: bool) -> DoorConfig {
    // One statement at a time: CREATE DATABASE can't run in the implicit
    // transaction a multi-statement query is.
    for stmt in lane_a_fixture(db).split(";\n").map(str::trim).filter(|x| !x.is_empty()) {
        srv.su("postgres", stmt).await;
    }
    test_seams::set_targets(db, srv.target(&format!("{db}_authenticator"), db), srv.target(&format!("{db}_migrator"), db));
    let mut mig = super::storage_pg::connect(&srv.target(&format!("{db}_migrator"), db)).await.expect("migrator");
    mig.client().batch_execute(AUTH_SQL).await.expect("auth fixture");
    super::storage_bind::install_schema(db).await.expect("install the storage schema");
    super::storage_bind::install_schema(db).await.expect("the install runs again cleanly");
    mig.client().batch_execute(&app_sql()).await.expect("the App's own storage policies load unchanged");
    let mut cfg = test_seams::config_for(app_id, db);
    cfg.anon = anon;
    test_seams::set_config(app_id, Some(cfg.clone()));
    storage_door::set_test_service(app_id, &format!("svc-{app_id}"));
    cfg
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs only with REALPG_STORAGE=1 and Postgres binaries (a Linux compute job)"]
async fn real_pg_storage_door() {
    let _home = crate::test_support::TempHome::new();
    let srv = Server::start();
    let app = "jobs";
    storage_app(&srv, app, "st_door", true).await;
    storage_door::set_test_callers(callers());
    let key = format!("{ORG1}/sub1/p1.jpg");
    let path = format!("/object/photos/{key}");

    // ── upload, duplicate, upsert ──
    let (st, _, body) = call(app, req("POST", &path, Some("test-a"), &[("Content-Type", "image/jpeg")], b"first bytes")).await;
    assert_eq!(st, 200, "A uploads into its org: {}", String::from_utf8_lossy(&body));
    let up = j(&body);
    assert_eq!(up["Key"], format!("photos/{key}"));
    let id = up["Id"].as_str().expect("Id").to_string();
    uuid::Uuid::parse_str(&id).expect("Id is a uuid");

    let (st, _, body) = call(app, req("POST", &path, Some("test-a"), &[("Content-Type", "image/jpeg")], b"again")).await;
    assert_eq!(st, 400, "a duplicate is HTTP 400, as Supabase");
    assert_eq!(
        j(&body),
        json!({"statusCode": "409", "code": "KeyAlreadyExists", "error": "Duplicate", "message": "The resource already exists"}),
        "the phone queue's duplicate answer, exactly"
    );

    let (st, _, body) = call(app, req("POST", &path, Some("test-a"), &[("Content-Type", "image/jpeg"), ("x-upsert", "true")], b"second bytes")).await;
    assert_eq!(st, 200, "upsert replaces: {}", String::from_utf8_lossy(&body));
    assert_eq!(j(&body)["Id"], id, "an upsert keeps the object's id");

    let (st, h, body) = call(app, req("GET", &path, Some("test-a"), &[], b"")).await;
    assert_eq!((st, body.as_slice()), (200, b"second bytes".as_slice()), "the new bytes");
    assert_eq!(h.get("content-type").map(String::as_str), Some("image/jpeg"));
    let md5 = {
        use md5::{Digest, Md5};
        super::storage_bytes::hex(&Md5::digest(b"second bytes"))
    };
    assert_eq!(h.get("etag").cloned(), Some(format!("\"{md5}\"")), "Supabase's ETag: the MD5 in quotes");
    let versions: Vec<_> = std::fs::read_dir(super::storage_bytes::ByteStore::for_app(app).object_path(&id, "00").expect("path").parent().expect("dir"))
        .expect("object folder")
        .collect();
    assert_eq!(versions.len(), 1, "the replaced version's bytes are gone");

    // ── policies: B is in another org ──
    let (st, _, body) = call(app, req("GET", &path, Some("test-b"), &[], b"")).await;
    assert_eq!(st, 400);
    assert_eq!(j(&body)["statusCode"], "404", "B can't see org1's object: {}", String::from_utf8_lossy(&body));
    let (st, _, body) = call(app, req("POST", &format!("/object/photos/{ORG1}/sub1/evil.jpg"), Some("test-b"), &[("Content-Type", "image/jpeg")], b"x")).await;
    assert_eq!(st, 400);
    assert_eq!(j(&body)["statusCode"], "403", "B can't write into org1: {}", String::from_utf8_lossy(&body));
    assert_eq!(j(&body)["message"], "new row violates row-level security policy");
    let (st, _, body) = call(app, req("POST", &format!("/object/photos/{ORG2}/s/b.jpg"), Some("test-b"), &[("Content-Type", "image/jpeg")], b"b's")).await;
    assert_eq!(st, 200, "B writes into its own org: {}", String::from_utf8_lossy(&body));

    // service_role sees everything; anon sees nothing private.
    let (st, _, body) = call(app, req("GET", &path, Some("test-svc"), &[], b"")).await;
    assert_eq!((st, body.as_slice()), (200, b"second bytes".as_slice()), "service_role bypasses RLS");
    let (st, _, body) = call(app, req("GET", &path, None, &[], b"")).await;
    assert_eq!((st, j(&body)["statusCode"].clone()), (400, json!("404")), "anon sees no private object");

    // ── list ──
    let list = |who: &'static str, prefix: String| async move {
        let (st, _, body) = call(app, req("POST", "/object/list/photos", Some(who), &[("Content-Type", "application/json")], json!({"prefix": prefix}).to_string().as_bytes())).await;
        assert_eq!(st, 200, "list: {}", String::from_utf8_lossy(&body));
        j(&body)
    };
    let top = list("test-a", ORG1.to_string()).await;
    assert_eq!(
        top,
        json!([{"name": "sub1", "id": null, "updated_at": null, "created_at": null, "last_accessed_at": null, "metadata": null,
                "version": null, "archived_at": null, "is_delete_marker": null, "is_versioned": null}]),
        "a folder"
    );
    let files = list("test-a", format!("{ORG1}/sub1")).await;
    assert_eq!(files.as_array().map(Vec::len), Some(1));
    assert_eq!(files[0]["name"], "p1.jpg");
    assert_eq!(files[0]["id"], id);
    assert_eq!(files[0]["metadata"]["size"], 12);
    assert_eq!(files[0]["metadata"]["mimetype"], "image/jpeg");
    assert!(files[0]["created_at"].as_str().is_some_and(|t| t.ends_with('Z') && t.contains('T')), "Supabase's timestamp form: {}", files[0]["created_at"]);
    assert_eq!(list("test-b", ORG1.to_string()).await, json!([]), "B lists nothing of org1");

    // ── multipart (supabase-js in a browser, the Dart client) ──
    let bnd = "XyZboundary";
    let mut mp = Vec::new();
    mp.extend_from_slice(format!("--{bnd}\r\nContent-Disposition: form-data; name=\"cacheControl\"\r\n\r\n3600\r\n").as_bytes());
    mp.extend_from_slice(format!("--{bnd}\r\nContent-Disposition: form-data; name=\"\"; filename=\"p2.jpg\"\r\nContent-Type: image/jpeg\r\n\r\n").as_bytes());
    mp.extend_from_slice(b"multi\r\npart bytes");
    mp.extend_from_slice(format!("\r\n--{bnd}--\r\n").as_bytes());
    let p2 = format!("/object/photos/{ORG1}/sub1/p2.jpg");
    let (st, _, body) = call(app, req("POST", &p2, Some("test-a"), &[("Content-Type", &format!("multipart/form-data; boundary={bnd}"))], &mp)).await;
    assert_eq!(st, 200, "multipart: {}", String::from_utf8_lossy(&body));
    let (st, h, body) = call(app, req("GET", &p2, Some("test-a"), &[], b"")).await;
    assert_eq!((st, body.as_slice()), (200, b"multi\r\npart bytes".as_slice()));
    assert_eq!(h.get("cache-control").map(String::as_str), Some("max-age=3600"));

    // ── chunked, and a range ──
    let p3 = format!("/object/photos/{ORG1}/sub1/p3.bin");
    let (st, _, body) = call(app, req("POST", &p3, Some("test-a"), &[("Transfer-Encoding", "chunked"), ("Content-Type", "application/octet-stream")], b"4\r\n0123\r\n6\r\n456789\r\n0\r\n\r\n")).await;
    assert_eq!(st, 200, "chunked: {}", String::from_utf8_lossy(&body));
    let (st, h, body) = call(app, req("GET", &p3, Some("test-a"), &[("Range", "bytes=2-5")], b"")).await;
    assert_eq!((st, body.as_slice()), (206, b"2345".as_slice()));
    assert_eq!(h.get("content-range").map(String::as_str), Some("bytes 2-5/10"));

    // ── signed URLs ──
    let (st, _, body) = call(app, req("POST", &format!("/object/sign/photos/{key}"), Some("test-a"), &[("Content-Type", "application/json")], br#"{"expiresIn": 3600}"#)).await;
    assert_eq!(st, 200, "sign: {}", String::from_utf8_lossy(&body));
    let signed = j(&body)["signedURL"].as_str().expect("signedURL").to_string();
    assert!(signed.starts_with(&format!("/object/sign/photos/{key}?token=")), "{signed}");
    // No credential at all: the token is the grant, whoever signed it has
    // no session in it.
    let (st, _, body) = call(app, req("GET", &signed, None, &[], b"")).await;
    assert_eq!((st, body.as_slice()), (200, b"second bytes".as_slice()), "the signed link, no session");
    let tampered = format!("{}x", signed);
    let (st, _, body) = call(app, req("GET", &tampered, None, &[], b"")).await;
    assert_eq!((st, j(&body)["error"].clone()), (400, json!("InvalidJWT")), "a tampered token");
    let other = signed.replace("p1.jpg", "p2.jpg");
    let (st, _, body) = call(app, req("GET", &other, None, &[], b"")).await;
    assert_eq!((st, j(&body)["error"].clone()), (400, json!("InvalidSignature")), "a token for another path");
    let (st, _, body) = call(app, req("POST", &format!("/object/sign/photos/{key}"), Some("test-b"), &[], br#"{"expiresIn": 60}"#)).await;
    assert_eq!((st, j(&body)["statusCode"].clone()), (400, json!("404")), "B can't sign what it can't read");
    // Expiry.
    let (_, _, body) = call(app, req("POST", &format!("/object/sign/photos/{key}"), Some("test-a"), &[], br#"{"expiresIn": 1}"#)).await;
    let short = j(&body)["signedURL"].as_str().expect("signedURL").to_string();
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let (st, _, body) = call(app, req("GET", &short, None, &[], b"")).await;
    assert_eq!((st, j(&body)["message"].clone()), (400, json!("The provided token has expired.")), "an expired link");
    // Many at once: one readable, one not.
    let (st, _, body) = call(
        app,
        req("POST", "/object/sign/photos", Some("test-a"), &[], json!({"expiresIn": 60, "paths": [key, format!("{ORG2}/s/b.jpg")]}).to_string().as_bytes()),
    )
    .await;
    assert_eq!(st, 200);
    let many = j(&body);
    assert!(many[0]["signedURL"].as_str().is_some() && many[0]["error"].is_null(), "{many}");
    assert!(many[1]["signedURL"].is_null() && many[1]["error"].as_str().is_some(), "{many}");
    // Rotation ends every outstanding link.
    super::storage_sign::rotate(app).expect("rotate");
    let (st, _, body) = call(app, req("GET", &signed, None, &[], b"")).await;
    assert_eq!((st, j(&body)["error"].clone()), (400, json!("InvalidJWT")), "a link from before the rotation");

    // ── signed upload URL ──
    let p4 = format!("{ORG1}/sub1/p4.jpg");
    let (st, _, body) = call(app, req("POST", &format!("/object/upload/sign/photos/{p4}"), Some("test-a"), &[], b"")).await;
    assert_eq!(st, 200, "upload sign: {}", String::from_utf8_lossy(&body));
    let up_url = j(&body)["url"].as_str().expect("url").to_string();
    let (st, _, body) = call(app, req("PUT", &up_url, None, &[("Content-Type", "image/jpeg")], b"via signed upload")).await;
    assert_eq!(st, 200, "the signed upload, no credential: {}", String::from_utf8_lossy(&body));
    let (st, _, body) = call(app, req("GET", &format!("/object/info/photos/{p4}"), Some("test-a"), &[], b"")).await;
    assert_eq!(st, 200);
    assert_eq!(j(&body)["size"], 17, "info: {}", String::from_utf8_lossy(&body));
    let mut su = super::storage_pg::connect(&srv.target("postgres", "st_door")).await.expect("su");
    let owner: Option<String> =
        su.client().query_one("SELECT owner::text FROM storage.objects WHERE name = $1", &[&p4]).await.expect("owner").get(0);
    assert_eq!(owner.as_deref(), Some(USER_A), "the token's owner owns the object");
    let (st, _, body) = call(app, req("POST", &format!("/object/upload/sign/photos/{ORG1}/x.jpg"), Some("test-b"), &[], b"")).await;
    assert_eq!((st, j(&body)["statusCode"].clone()), (400, json!("403")), "B can't get an upload link into org1");

    // ── public bucket ──
    let (st, _, body) = call(app, req("POST", "/object/avatars/u/a.png", Some("test-a"), &[("Content-Type", "image/png")], b"png!")).await;
    assert_eq!(st, 200, "avatar: {}", String::from_utf8_lossy(&body));
    let (st, _, body) = call(app, req("GET", "/object/public/avatars/u/a.png", None, &[], b"")).await;
    assert_eq!((st, body.as_slice()), (200, b"png!".as_slice()), "a public object, no credential");
    let (st, _, body) = call(app, req("GET", &format!("/object/public/photos/{key}"), None, &[], b"")).await;
    assert_eq!((st, j(&body)["error"].clone()), (400, json!("Bucket not found")), "a private bucket has no public URL");
    let (st, _, body) = call(app, req("POST", "/object/avatars/u/a.txt", Some("test-a"), &[("Content-Type", "text/plain")], b"no")).await;
    assert_eq!((st, j(&body)["statusCode"].clone()), (400, json!("415")), "the bucket's mime list");

    // ── move ──
    let (st, _, body) = call(
        app,
        req("POST", "/object/move", Some("test-a"), &[], json!({"bucketId": "photos", "sourceKey": format!("{ORG1}/sub1/p2.jpg"), "destinationKey": format!("{ORG1}/sub1/p2-moved.jpg")}).to_string().as_bytes()),
    )
    .await;
    assert_eq!(st, 200, "move: {}", String::from_utf8_lossy(&body));
    let (st, _, body) = call(app, req("GET", &format!("/object/photos/{ORG1}/sub1/p2-moved.jpg"), Some("test-a"), &[], b"")).await;
    assert_eq!((st, body.as_slice()), (200, b"multi\r\npart bytes".as_slice()));
    let (st, _, _) = call(app, req("GET", &p2, Some("test-a"), &[], b"")).await;
    assert_eq!(st, 400, "the old name is gone");
    let (st, _, body) = call(
        app,
        req("POST", "/object/move", Some("test-b"), &[], json!({"bucketId": "photos", "sourceKey": format!("{ORG2}/s/b.jpg"), "destinationKey": format!("{ORG1}/stolen.jpg")}).to_string().as_bytes()),
    )
    .await;
    assert_eq!((st, j(&body)["statusCode"].clone()), (400, json!("403")), "B can't move into org1 (WITH CHECK)");

    // ── remove ──
    let (st, _, body) = call(app, req("DELETE", "/object/photos", Some("test-b"), &[], json!({"prefixes": [key]}).to_string().as_bytes())).await;
    assert_eq!((st, j(&body)), (200, json!([])), "B removes nothing of org1");
    let (st, _, body) = call(app, req("DELETE", "/object/photos", Some("test-a"), &[], json!({"prefixes": [key]}).to_string().as_bytes())).await;
    assert_eq!(st, 200);
    let gone = j(&body);
    assert_eq!(gone.as_array().map(Vec::len), Some(1), "{gone}");
    assert_eq!(gone[0]["name"], key);
    assert!(!super::storage_bytes::ByteStore::for_app(app).has(&id, gone[0]["version"].as_str().expect("version")), "the bytes go with the row");
    let (st, _, body) = call(app, req("DELETE", &p3, Some("test-a"), &[], b"")).await;
    assert_eq!((st, j(&body)), (200, json!({"message": "Successfully deleted"})));
    let (st, _, body) = call(app, req("DELETE", &p3, Some("test-a"), &[], b"")).await;
    assert_eq!((st, j(&body)["statusCode"].clone()), (400, json!("404")));

    // ── buckets and unsupported parts ──
    let (st, _, body) = call(app, req("GET", "/bucket", Some("test-a"), &[], b"")).await;
    assert_eq!(st, 200);
    let mut ids: Vec<String> = j(&body).as_array().expect("buckets").iter().map(|b| b["id"].as_str().expect("bucket id").to_string()).collect();
    ids.sort();
    assert_eq!(ids, vec!["avatars".to_string(), "photos".to_string()], "the buckets the caller may see");
    let (st, _, body) = call(app, req("POST", "/bucket", Some("test-svc"), &[], br#"{"name": "docs", "public": false, "file_size_limit": "1MB"}"#)).await;
    assert_eq!((st, j(&body)), (200, json!({"name": "docs"})));
    let (st, _, body) = call(app, req("POST", "/bucket", Some("test-svc"), &[], br#"{"name": "docs"}"#)).await;
    assert_eq!((st, j(&body)["statusCode"].clone()), (400, json!("409")), "a taken bucket name");
    let (st, _, body) = call(app, req("POST", "/object/docs/big.bin", Some("test-svc"), &[], &vec![0u8; (1 << 20) + 1])).await;
    assert_eq!((st, j(&body)["statusCode"].clone()), (400, json!("413")), "over the bucket's file limit");
    let (st, _, body) = call(app, req("GET", "/render/image/authenticated/photos/x.png", Some("test-a"), &[], b"")).await;
    assert_eq!((st, j(&body)["error"].clone()), (400, json!("k2_unsupported")));

    // ── a JWT-shaped token whose claims K2 didn't write ──
    {
        use base64::Engine as _;
        let b64 = |v: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        let forged = format!(
            "{}.{}.c2lnbmF0dXJl",
            b64(json!({"alg": "HS256", "typ": "JWT"})),
            b64(json!({"sub": USER_A, "role": "service_role", "pass": "k2skn_made.up"}))
        );
        let (st, _, body) = call(app, req("GET", &format!("/object/photos/{ORG2}/s/b.jpg"), Some(&forged), &[], b"")).await;
        assert_eq!((st, j(&body)["error"].clone()), (400, json!("InvalidJWT")), "forged claims are never read: {}", String::from_utf8_lossy(&body));
    }

    // ── the anon lane off: no credential is 401 ──
    let mut cfg = test_seams::config(app).flatten().expect("config");
    cfg.anon = false;
    test_seams::set_config(app, Some(cfg));
    let (st, _, _) = call(app, req("GET", &format!("/object/photos/{ORG2}/s/b.jpg"), None, &[], b"")).await;
    assert_eq!(st, 401, "no anon lane");
    // Compat off: the door says so (the App path doesn't divert then).
    test_seams::set_config(app, None);
    let (st, _, _) = call(app, req("GET", "/bucket", Some("test-a"), &[], b"")).await;
    assert_eq!(st, 404);
    drop(srv);
}

// ── the import, against a stand-in Supabase project ──────────────────

/// One object at the stand-in source.
#[derive(Clone)]
struct SrcObj {
    id: String,
    bytes: Vec<u8>,
    etag: String,
    updated_at: String,
}

#[derive(Default)]
struct Mock {
    key: String,
    buckets: Vec<Value>,
    objects: std::collections::BTreeMap<(String, String), SrcObj>,
    /// Objects whose first full download is cut halfway.
    cut_once: std::collections::HashSet<String>,
    requests: usize,
    /// Object downloads (any range).
    downloads: usize,
    ranged: usize,
}

type MockRef = std::sync::Arc<std::sync::Mutex<Mock>>;

fn md5_hex(b: &[u8]) -> String {
    use md5::{Digest, Md5};
    super::storage_bytes::hex(&Md5::digest(b))
}

fn sha_hex(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    super::storage_bytes::hex(&Sha256::digest(b))
}

fn src_obj(bytes: Vec<u8>, t: &str) -> SrcObj {
    SrcObj { id: uuid::Uuid::new_v4().to_string(), etag: format!("\"{}\"", md5_hex(&bytes)), bytes, updated_at: t.into() }
}

fn decode_path(p: &str) -> String {
    storage_door::decode(p).expect("a decodable path")
}

/// Supabase's list for one level under `prefix`.
fn mock_list(m: &Mock, bucket: &str, prefix: &str, limit: usize, offset: usize) -> Value {
    let mut entries: std::collections::BTreeMap<String, Value> = Default::default();
    for ((b, name), o) in &m.objects {
        if b != bucket || !name.starts_with(prefix) {
            continue;
        }
        let rest = &name[prefix.len()..];
        match rest.split_once('/') {
            Some((folder, _)) => {
                entries.insert(folder.to_string(), json!({"name": folder, "id": null, "metadata": null}));
            }
            None => {
                entries.insert(
                    rest.to_string(),
                    json!({"name": rest, "id": o.id, "updated_at": o.updated_at, "created_at": "2026-01-02T03:04:05.000Z",
                           "metadata": {"eTag": o.etag, "size": o.bytes.len(), "mimetype": "image/jpeg", "cacheControl": "max-age=3600"}}),
                );
            }
        }
    }
    Value::Array(entries.into_values().skip(offset).take(limit).collect())
}

async fn mock_conn(mut s: tokio::net::TcpStream, m: MockRef) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        match s.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let hv = |k: &str| {
        head.lines().skip(1).find_map(|l| l.split_once(':').filter(|(n, _)| n.trim().eq_ignore_ascii_case(k)).map(|(_, v)| v.trim().to_string()))
    };
    let len: usize = hv("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while buf.len() < head_end + len {
        match s.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let body = buf[head_end..head_end + len].to_vec();
    let mut first = head.lines().next().unwrap_or("").split_whitespace();
    let (method, target) = (first.next().unwrap_or("").to_string(), first.next().unwrap_or("").to_string());
    let reply = |status: &str, ctype: &str, extra: &str, body: &[u8], send: usize| {
        let mut out = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\n{extra}Connection: close\r\n\r\n", body.len()).into_bytes();
        out.extend_from_slice(&body[..send.min(body.len())]);
        out
    };
    let out = {
        let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
        g.requests += 1;
        let key_ok = hv("apikey").as_deref() == Some(g.key.as_str()) && hv("authorization") == Some(format!("Bearer {}", g.key));
        if !key_ok {
            reply("401 Unauthorized", "application/json", "", br#"{"statusCode":"403","error":"Unauthorized","message":"bad key"}"#, usize::MAX)
        } else if method == "GET" && target == "/storage/v1/bucket" {
            reply("200 OK", "application/json", "", Value::Array(g.buckets.clone()).to_string().as_bytes(), usize::MAX)
        } else if let Some(b) = target.strip_prefix("/storage/v1/object/list/") {
            let v: Value = serde_json::from_slice(&body).expect("list body");
            let prefix = v["prefix"].as_str().unwrap_or("");
            let l = mock_list(&g, &decode_path(b), prefix, v["limit"].as_u64().unwrap_or(100) as usize, v["offset"].as_u64().unwrap_or(0) as usize);
            reply("200 OK", "application/json", "", l.to_string().as_bytes(), usize::MAX)
        } else if let Some(t) = target.strip_prefix("/storage/v1/object/authenticated/") {
            g.downloads += 1;
            let t = decode_path(t);
            let (b, n) = t.split_once('/').expect("bucket/name");
            match g.objects.get(&(b.to_string(), n.to_string())).cloned() {
                None => reply("400 Bad Request", "application/json", "", br#"{"statusCode":"404","error":"not_found","message":"Object not found"}"#, usize::MAX),
                Some(o) => match hv("range").and_then(|r| r.strip_prefix("bytes=").and_then(|x| x.strip_suffix('-')).and_then(|x| x.parse::<usize>().ok())) {
                    Some(from) if from < o.bytes.len() => {
                        g.ranged += 1;
                        let part = &o.bytes[from..];
                        let cr = format!("Content-Range: bytes {from}-{}/{}\r\n", o.bytes.len() - 1, o.bytes.len());
                        reply("206 Partial Content", "application/octet-stream", &cr, part, usize::MAX)
                    }
                    _ => {
                        let send = if g.cut_once.remove(n) { o.bytes.len() / 2 } else { usize::MAX };
                        reply("200 OK", "application/octet-stream", "", &o.bytes, send)
                    }
                },
            }
        } else {
            reply("404 Not Found", "text/plain", "", b"no", usize::MAX)
        }
    };
    let _ = s.write_all(&out).await;
    let _ = s.shutdown().await;
}

async fn mock_source(m: MockRef) -> String {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("mock source bind");
    let addr = l.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(mock_conn(s, m.clone()));
        }
    });
    format!("http://{addr}")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "runs only with REALPG_STORAGE=1 and Postgres binaries (a Linux compute job)"]
async fn real_pg_storage_import() {
    use super::storage_import::{self as imp, Opts, Source};
    let _home = crate::test_support::TempHome::new();
    let srv = Server::start();
    let app = "moved";
    storage_app(&srv, app, "st_import", false).await;
    let cfg = test_seams::config(app).flatten().expect("config");
    let ctx = super::storage_ops::Ctx {
        app_id: app.into(),
        target: super::storage_bind::authenticator_target(&cfg).expect("target"),
        bytes: super::storage_bytes::ByteStore::for_app(app),
    };

    // The source: two buckets, nested folders, a 3 MiB object that is cut
    // off once, an object whose eTag doesn't match its bytes, and a folder
    // placeholder.
    let m: MockRef = Default::default();
    let t0 = "2026-03-04T05:06:07.000Z";
    {
        let mut g = m.lock().unwrap();
        g.key = "src-service-role-key".into();
        g.buckets = vec![
            json!({"id": "photos", "name": "photos", "public": false, "file_size_limit": 52428800, "allowed_mime_types": null}),
            json!({"id": "logos", "name": "logos", "public": true, "file_size_limit": null, "allowed_mime_types": ["image/*"]}),
        ];
        for org in 0..3usize {
            for sub in 0..4usize {
                for i in 0..3usize {
                    let bytes: Vec<u8> = (0..(1000 + org * 997 + sub * 101 + i * 13)).map(|x| (x * 31 + org + sub + i) as u8).collect();
                    g.objects.insert(("photos".into(), format!("org{org}/sub{sub}/p{i}.jpg")), src_obj(bytes, t0));
                }
            }
        }
        g.objects.insert(("photos".into(), "org0/.emptyFolderPlaceholder".into()), src_obj(vec![], t0));
        let big: Vec<u8> = (0..3 * 1024 * 1024u32).map(|x| (x % 251) as u8).collect();
        g.objects.insert(("photos".into(), "org1/big.bin".into()), src_obj(big, t0));
        g.cut_once.insert("org1/big.bin".into());
        let mut bad = src_obj(b"these bytes".to_vec(), t0);
        bad.etag = format!("\"{}\"", md5_hex(b"other bytes"));
        g.objects.insert(("photos".into(), "org2/bad.bin".into()), bad);
        g.objects.insert(("logos".into(), "acme logo (1).png".into()), src_obj(b"logo".to_vec(), t0));
    }
    let total = m.lock().unwrap().objects.len();
    let base = mock_source(m.clone()).await;

    // A row the dump already carried (lane B): it keeps its id and owner.
    let dumped_id = "dddddddd-0000-4000-8000-000000000001";
    srv.su(
        "st_import",
        &format!("INSERT INTO storage.objects (id, bucket_id, name, owner, owner_id) VALUES ('{dumped_id}', 'photos', 'org0/sub0/p0.jpg', '{USER_A}', '{USER_A}')"),
    )
    .await;

    let src = Source { base: base.clone(), key: "src-service-role-key".into() };
    let opts = Opts { buckets: vec![], concurrency: 4 };
    let s1 = imp::run(ctx.clone(), src.clone(), opts.clone()).await.expect("first run");
    eprintln!("first run: {s1:?}");
    assert_eq!(s1.buckets, 2);
    assert_eq!(s1.planned, total, "every source object is planned");
    assert_eq!((s1.copied, s1.failed, s1.skipped), (total - 1, 1, 0), "all but the mismatched one: {:?}", s1.errors);
    assert_eq!(s1.status, "done_with_failures");
    assert!(s1.errors.iter().any(|e| e.contains("org2/bad.bin") && e.contains("MD5")), "the mismatch is named: {:?}", s1.errors);
    assert!(m.lock().unwrap().ranged >= 1, "the cut-off object resumed with Range");

    // Every byte checked: what is in the App hashes to the source's bytes.
    let src_objs = m.lock().unwrap().objects.clone();
    let rows = super::storage_ops::all_objects(&ctx).await.expect("rows");
    assert_eq!(rows.len(), total - 1);
    for (b, n, id, version, size) in &rows {
        let o = &src_objs[&(b.clone(), n.clone())];
        let p = ctx.bytes.object_path(id, version.as_deref().expect("a version")).expect("path");
        let got = std::fs::read(&p).unwrap_or_else(|e| panic!("{b}/{n}: {e}"));
        assert_eq!(sha_hex(&got), sha_hex(&o.bytes), "{b}/{n}: the bytes");
        assert_eq!(*size, Some(o.bytes.len() as i64), "{b}/{n}: metadata.size");
        if n == "org0/sub0/p0.jpg" {
            assert_eq!(id, dumped_id, "the dump's row keeps its id");
        } else {
            assert_eq!(id, &o.id, "{b}/{n}: the source's id is kept");
        }
    }
    let mut c = super::storage_pg::connect(&srv.target("postgres", "st_import")).await.expect("su");
    let owner: Option<String> = c.client().query_one("SELECT owner::text FROM storage.objects WHERE id = $1::text::uuid", &[&dumped_id]).await.expect("owner").get(0);
    assert_eq!(owner.as_deref(), Some(USER_A), "the dump's row keeps its owner");
    let public: bool = c.client().query_one("SELECT public FROM storage.buckets WHERE id = 'logos'", &[]).await.expect("bucket").get(0);
    assert!(public, "the bucket's settings came across");

    // The door serves an imported object.
    storage_door::set_test_callers(callers());
    let (st, _, body) = call(app, req("GET", "/object/public/logos/acme%20logo%20(1).png", None, &[], b"")).await;
    assert_eq!((st, body.as_slice()), (200, b"logo".as_slice()), "an imported public object");

    // Verify: clean but for the one that failed.
    let v = imp::verify(&ctx, Some(&src)).await.expect("verify");
    assert!(v.bad.is_empty(), "{v:?}");
    assert_eq!(v.missing, vec!["photos/org2/bad.bin".to_string()]);

    // Again: nothing changed, nothing copied.
    let before = m.lock().unwrap().downloads;
    let s2 = imp::run(ctx.clone(), src.clone(), opts.clone()).await.expect("second run");
    assert_eq!((s2.copied, s2.skipped, s2.failed), (0, total - 1, 1), "a rerun copies only what changed: {s2:?}");
    let fetched = m.lock().unwrap().downloads - before;
    assert_eq!(fetched, 1, "a rerun downloads only the object that failed before");

    // One object changes at the source, the bad one is fixed: two copies.
    {
        let mut g = m.lock().unwrap();
        let k = ("photos".to_string(), "org1/sub1/p1.jpg".to_string());
        let id = g.objects[&k].id.clone();
        let mut o = src_obj(b"new content".to_vec(), "2026-05-06T07:08:09.000Z");
        o.id = id;
        g.objects.insert(k, o);
        let k = ("photos".to_string(), "org2/bad.bin".to_string());
        let b = g.objects[&k].bytes.clone();
        g.objects.get_mut(&k).unwrap().etag = format!("\"{}\"", md5_hex(&b));
    }
    let s3 = imp::run(ctx.clone(), src.clone(), opts).await.expect("third run");
    assert_eq!((s3.copied, s3.failed, s3.status.as_str()), (2, 0, "done"), "{s3:?}");
    let v = imp::verify(&ctx, Some(&src)).await.expect("verify");
    assert!(v.bad.is_empty() && v.missing.is_empty(), "{v:?}");
    assert_eq!(v.checked, total);

    // Bytes changed on disk behind K2's back: verify says so.
    let (b, n, id, version, _) =
        super::storage_ops::all_objects(&ctx).await.expect("rows").into_iter().find(|r| r.1 == "org0/sub1/p2.jpg").expect("row");
    std::fs::write(ctx.bytes.object_path(&id, version.as_deref().unwrap()).unwrap(), b"tampered").expect("tamper");
    let v = imp::verify(&ctx, None).await.expect("verify");
    assert_eq!(v.bad, vec![format!("{b}/{n}: bytes don't match the copy's hash")]);

    // A wrong key is refused by the source, loudly.
    let bad = imp::run(ctx.clone(), Source { base, key: "wrong".into() }, Opts { buckets: vec![], concurrency: 2 }).await.expect("a run");
    assert_eq!(bad.status, "failed");
    assert!(bad.errors.iter().any(|e| e.contains("401")), "{:?}", bad.errors);
    drop(srv);
}
