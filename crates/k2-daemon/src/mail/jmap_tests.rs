use super::*;
use std::io::{Read, Write};

/// The REAL bootstrap-mode session document served by Stalwart
/// v0.16.10 (captured live 2026-07-10, trimmed to the fields the
/// client reads — accounts key + apiUrl are verbatim).
const BOOTSTRAP_SESSION_FIXTURE: &str = r#"{
        "capabilities": { "urn:ietf:params:jmap:core": { "maxSizeUpload": 50000000 } },
        "accounts": { "d333333": { "name": "admin", "isPersonal": true, "isReadOnly": false } },
        "primaryAccounts": {
            "urn:ietf:params:jmap:mail": "d333333",
            "urn:stalwart:jmap": "d333333"
        },
        "username": "admin",
        "apiUrl": "/jmap/",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
        "uploadUrl": "/jmap/upload/{accountId}/",
        "state": "20f78199"
    }"#;

/// The REAL normal-mode session document shape: absolute URLs on
/// the MAIL HOSTNAME (captured live 2026-07-10) — the client must
/// rebase them onto its loopback base.
pub(crate) const NORMAL_SESSION_FIXTURE: &str = r#"{
        "capabilities": { "urn:ietf:params:jmap:core": {} },
        "accounts": { "b": { "name": "admin@k2livebox.test" } },
        "primaryAccounts": { "urn:ietf:params:jmap:mail": "b", "urn:stalwart:jmap": "b" },
        "username": "admin@k2livebox.test",
        "apiUrl": "https://mail.k2livebox.test/jmap/",
        "downloadUrl": "https://mail.k2livebox.test/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
        "state": "73a05484"
    }"#;

#[test]
fn session_api_url_rebases_absolute_and_relative_onto_base() {
    // Bootstrap mode: root-relative.
    let session: serde_json::Value =
        serde_json::from_str(BOOTSTRAP_SESSION_FIXTURE).expect("fixture JSON");
    assert_eq!(
        parse_session_api_url("http://127.0.0.1:8080", &session).expect("parsed"),
        "http://127.0.0.1:8080/jmap/"
    );
    // Normal mode: ABSOLUTE https url on the mail hostname —
    // only the path survives (live-verified behavior).
    let session: serde_json::Value =
        serde_json::from_str(NORMAL_SESSION_FIXTURE).expect("fixture JSON");
    assert_eq!(
        parse_session_api_url("http://127.0.0.1:8180", &session).expect("parsed"),
        "http://127.0.0.1:8180/jmap/"
    );
    assert_eq!(
        parse_session_download_url("http://127.0.0.1:8180", &session).expect("parsed"),
        "http://127.0.0.1:8180/jmap/download/{accountId}/{blobId}/{name}?accept={type}"
    );
}

#[test]
fn session_account_id_prefers_stalwart_primary_then_first_account() {
    let session: serde_json::Value =
        serde_json::from_str(NORMAL_SESSION_FIXTURE).expect("fixture JSON");
    assert_eq!(parse_session_account_id(&session).expect("parsed"), "b");
    // No primaryAccounts entry → the single accounts key.
    let bare = serde_json::json!({ "accounts": { "d333333": {} } });
    assert_eq!(parse_session_account_id(&bare).expect("parsed"), "d333333");
    // Neither → loud.
    assert!(parse_session_account_id(&serde_json::json!({})).is_err());
}

#[test]
fn missing_or_garbage_api_url_fails_loudly() {
    for session in [
        serde_json::json!({}),
        serde_json::json!({ "apiUrl": "" }),
        serde_json::json!({ "apiUrl": 42 }),
        serde_json::json!({ "capabilities": {} }),
    ] {
        let err = parse_session_api_url("http://127.0.0.1:8180", &session)
            .expect_err("must reject");
        assert!(err.contains("apiUrl"), "{err}");
    }
    // Neither absolute nor root-relative → loud, named value.
    let err = parse_session_api_url(
        "http://127.0.0.1:8180",
        &serde_json::json!({ "apiUrl": "jmap/" }),
    )
    .expect_err("must reject");
    assert!(err.contains("jmap/"), "{err}");
}

/// Constructor normalizes a trailing-slash base so path joins never
/// double the slash.
#[test]
fn client_construction_normalizes_base() {
    let c = StalwartClient::new("http://127.0.0.1:8180///", "k2-test-key");
    assert_eq!(c.base_url, "http://127.0.0.1:8180");
}

// ── S1 pure parsers (fixtures = REAL live-box replies) ──────────

/// The REAL `x:Bootstrap/set` reply (captured live 2026-07-10;
/// secret replaced).
#[test]
fn bootstrap_set_reply_yields_provisioned_admin_credentials() {
    let args = serde_json::json!({
        "accountId": "d333333",
        "updated": {
            "singleton": {
                "username": "admin@k2livebox.test",
                "secret": "REDACTED-16CHARS"
            }
        }
    });
    let creds = parse_bootstrap_updated(&args).expect("parsed");
    assert_eq!(creds.username, "admin@k2livebox.test");
    assert_eq!(creds.secret, "REDACTED-16CHARS");

    // The REAL first-attempt failure (perm denied writing the
    // config path) — notUpdated surfaces the SetError verbatim.
    let rejected = serde_json::json!({
        "notUpdated": { "singleton": {
            "type": "invalidProperties",
            "description": "Failed to save data store settings: Permission denied",
            "properties": ["dataStore"]
        } }
    });
    let err = parse_bootstrap_updated(&rejected).expect_err("must reject");
    assert!(err.contains("Permission denied"), "{err}");

    // Updated but WITHOUT credentials (external directory case) →
    // loud, never a silent success without an admin.
    let no_creds = serde_json::json!({ "updated": { "singleton": null } });
    assert!(parse_bootstrap_updated(&no_creds).is_err());
    assert!(parse_bootstrap_updated(&serde_json::json!({})).is_err());
}

/// The REAL default listener set (ids + names captured live).
#[test]
fn listeners_parse_and_port_plan_builds_the_right_set() {
    let reply = serde_json::json!({
        "list": [
            { "id": "iz1vbh9qaeqb", "name": "smtp", "bind": { "[::]:25": true }, "protocol": "smtp" },
            { "id": "iz1vbh9qafab", "name": "submissions", "bind": { "[::]:465": true } },
            { "id": "iz1vbh9qafqb", "name": "imaps", "bind": { "[::]:993": true } },
            { "id": "iz1vbh9sagab", "name": "pop3s", "bind": { "[::]:995": true } },
            { "id": "iz1vbh9sagqb", "name": "sieve", "bind": { "[::]:4190": true } },
            { "id": "iz1vbh9sahab", "name": "https", "bind": { "[::]:443": true } },
            { "id": "iz1vbh9sahqb", "name": "http", "bind": { "[::]:8080": true } },
        ],
        "notFound": []
    });
    let listeners = parse_listeners(&reply).expect("parsed");
    assert_eq!(listeners.len(), 7);
    assert_eq!(listeners[0], ListenerInfo { id: "iz1vbh9qaeqb".into(), name: "smtp".into() });
    // No list → loud; entry without id → loud.
    assert!(parse_listeners(&serde_json::json!({})).is_err());
    assert!(parse_listeners(&serde_json::json!({ "list": [{ "name": "x" }] })).is_err());
}

#[test]
fn set_clean_guard_rejects_any_partial_application() {
    expect_set_clean("x:NetworkListener/set", &serde_json::json!({
        "created": { "k2": { "id": "n1" } },
        "updated": { "l1": null },
        "destroyed": ["l2"],
    }))
    .expect("clean set");
    let partial = serde_json::json!({
        "destroyed": ["l2"],
        "notDestroyed": { "l3": { "type": "notFound" } },
    });
    let err = expect_set_clean("x:NetworkListener/set", &partial).expect_err("must reject");
    assert!(err.contains("notDestroyed"), "{err}");
}

#[test]
fn retired_local_part_marks_and_respects_the_64_char_budget() {
    let r = retired_local_part("research-bot", 1_760_000_000);
    assert_eq!(r, "research-bot-k2r-1760000000");
    assert!(r.contains(RETIRED_MARKER));
    let long = "a".repeat(64);
    let r = retired_local_part(&long, 1_760_000_000);
    assert_eq!(r.len(), 64, "budget respected");
    assert!(r.ends_with("-k2r-1760000000"));
}

// ── Loopback mock-server helpers (ephemeral port; the one allowed
//    form of network in tests) ───────────────────────────────────

pub(crate) fn spawn_mock_server(
    replies: Vec<String>,
) -> (u16, std::sync::mpsc::Receiver<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for reply in replies {
            let (mut sock, _) = match listener.accept() {
                Ok(s) => s,
                Err(_) => return,
            };
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = match sock.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                raw.extend_from_slice(&buf[..n]);
                if let Some(head_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
                    let want: usize = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + want {
                        break;
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&raw).to_string());
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.len(),
                reply
            );
            let _ = sock.write_all(resp.as_bytes());
        }
    });
    (port, rx)
}

pub(crate) fn body_json(req: &str) -> serde_json::Value {
    let start = req.find("\r\n\r\n").expect("body") + 4;
    serde_json::from_str(&req[start..]).expect("JSON body")
}

/// A box-wide proxy env never reaches K2's own Stalwart. The mgmt
/// client (session GET) and both blob clients (upload POST, download
/// GET) dial the loopback listener directly; reqwest's default would
/// send them, admin bearer key included, to HTTP(S)_PROXY/ALL_PROXY.
/// (Seed-11 shuffled run: a parallel test's fake proxy answered a
/// jmap call with no `methodResponses`.)
#[test]
fn stalwart_clients_ignore_proxy_env() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    let proxy = std::net::TcpListener::bind("127.0.0.1:0").expect("bind proxy");
    let proxy_port = proxy.local_addr().expect("proxy addr").port();
    let proxy_conns = Arc::new(AtomicUsize::new(0));
    let seen = proxy_conns.clone();
    std::thread::spawn(move || {
        for stream in proxy.incoming() {
            let Ok(mut sock) = stream else { return };
            seen.fetch_add(1, Ordering::SeqCst);
            let mut buf = [0u8; 4096];
            let _ = sock.read(&mut buf);
            let body = r#"{"via":"proxy"}"#;
            let _ = sock.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            );
        }
    });
    let p = format!("http://127.0.0.1:{proxy_port}");
    let _http = k2_core::test_env::EnvVar::set("HTTP_PROXY", &p);
    let _https = k2_core::test_env::EnvVar::set("HTTPS_PROXY", &p);
    let _all = k2_core::test_env::EnvVar::set("ALL_PROXY", &p);
    // A runner's NO_PROXY=127.0.0.1 would hide the bug; clear it.
    let _no1 = k2_core::test_env::EnvVar::remove("NO_PROXY");
    let _no2 = k2_core::test_env::EnvVar::remove("no_proxy");

    let (port, rx) = spawn_mock_server(vec![
        BOOTSTRAP_SESSION_FIXTURE.to_string(),
        BOOTSTRAP_SESSION_FIXTURE.to_string(),
        r#"{"blobId":"b-1"}"#.to_string(),
        BOOTSTRAP_SESSION_FIXTURE.to_string(),
        "blob-bytes".to_string(),
    ]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");

    let session = c.get_json(SESSION_PATH).expect("session GET goes direct");
    assert_eq!(session["username"], "admin", "{session}");
    let blob = c.blob_upload("d333333", b"hello").expect("blob upload goes direct");
    assert_eq!(blob, "b-1");
    let bytes = c
        .blob_download("d333333", "b-1", "x.txt", "text/plain")
        .expect("blob download goes direct");
    assert_eq!(bytes, b"blob-bytes");
    assert_eq!(proxy_conns.load(Ordering::SeqCst), 0, "a Stalwart call went through the env proxy");

    let reqs: Vec<String> = (0..5).map(|i| rx.recv().unwrap_or_else(|e| panic!("req {i}: {e}"))).collect();
    assert!(reqs[0].starts_with("GET /jmap/session "), "{}", reqs[0]);
    assert!(reqs[2].starts_with("POST /jmap/upload/d333333/ "), "{}", reqs[2]);
    assert!(reqs[4].starts_with("GET /jmap/download/d333333/b-1/x.txt"), "{}", reqs[4]);
    for r in &reqs {
        assert!(
            r.contains("authorization: Bearer k2-test-key") || r.contains("Authorization: Bearer k2-test-key"),
            "{r}"
        );
    }
}

/// Bootstrap round-trip: authenticate discovers /jmap/session with
/// BASIC auth, complete_bootstrap posts the x:Bootstrap/set update
/// (singleton id, DataStore RocksDb, hostname/domain) and yields
/// the provisioned admin credentials.
#[test]
fn bootstrap_round_trip_against_loopback_mock() {
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Bootstrap/set", {
            "accountId": "d333333",
            "updated": { "singleton": {
                "username": "admin@acme.dev", "secret": "s3cr3t16"
            } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) =
        spawn_mock_server(vec![BOOTSTRAP_SESSION_FIXTURE.to_string(), set_reply]);

    use crate::mail::supervisor::BootstrapApi;
    let mut api = StalwartBootstrap::new();
    api.authenticate(&format!("http://127.0.0.1:{port}"), "admin", "recovery-pw")
        .expect("authenticate");
    let creds = api
        .complete_bootstrap("mail.acme.dev", "acme.dev", false)
        .expect("bootstrap");
    assert_eq!(creds.username, "admin@acme.dev");
    assert_eq!(creds.secret, "s3cr3t16");

    let req1 = rx.recv().expect("req1");
    assert!(req1.starts_with("GET /jmap/session"), "{req1}");
    // Basic base64("admin:recovery-pw").
    assert!(
        req1.contains("authorization: Basic YWRtaW46cmVjb3ZlcnktcHc=")
            || req1.contains("Authorization: Basic YWRtaW46cmVjb3ZlcnktcHc="),
        "{req1}"
    );

    let req2 = rx.recv().expect("req2");
    assert!(req2.starts_with("POST /jmap/"), "{req2}");
    let v = body_json(&req2);
    assert_eq!(v["using"][1], "urn:stalwart:jmap");
    assert_eq!(v["methodCalls"][0][0], "x:Bootstrap/set");
    let args = &v["methodCalls"][0][1];
    assert_eq!(args["accountId"], "d333333", "session account id injected");
    let up = &args["update"]["singleton"];
    assert_eq!(up["serverHostname"], "mail.acme.dev");
    assert_eq!(up["defaultDomain"], "acme.dev");
    assert_eq!(up["requestTlsCertificate"], false);
    assert_eq!(up["generateDkimKeys"], true);
    assert_eq!(up["dataStore"]["@type"], "RocksDb");
    assert_eq!(up["dataStore"]["path"], "/var/lib/stalwart/data");
    // Using before authenticate fails loudly.
    let mut cold = StalwartBootstrap::new();
    let err = cold
        .complete_bootstrap("x", "y", false)
        .expect_err("must fail");
    assert!(err.contains("before authenticate"), "{err}");
}

/// Port-plan round-trip: one x:NetworkListener/set destroys the
/// §10 listeners, retargets http to the loopback mgmt bind, binds
/// https per plan, creates submission :587.
#[test]
fn listeners_apply_builds_the_port_plan_set() {
    let get_reply = serde_json::json!({
        "methodResponses": [["x:NetworkListener/get", {
            "accountId": "b",
            "list": [
                { "id": "L-smtp", "name": "smtp" },
                { "id": "L-subs", "name": "submissions" },
                { "id": "L-imaps", "name": "imaps" },
                { "id": "L-pop3s", "name": "pop3s" },
                { "id": "L-sieve", "name": "sieve" },
                { "id": "L-https", "name": "https" },
                { "id": "L-http", "name": "http" },
            ],
            "notFound": [],
        }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:NetworkListener/set", {
            "created": { "k2": { "id": "L-new" } },
            "updated": { "L-http": null, "L-https": null },
            "destroyed": ["L-pop3s", "L-sieve"],
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        get_reply,
        set_reply,
    ]);
    use crate::mail::supervisor::BootstrapApi;
    let mut api = StalwartBootstrap::new();
    api.authenticate(&format!("http://127.0.0.1:{port}"), "admin@k2livebox.test", "pw")
        .expect("authenticate");
    api.configure_listeners("http-01").expect("apply");

    let _sess = rx.recv().expect("req1");
    let _get = rx.recv().expect("req2");
    let set = rx.recv().expect("req3");
    let v = body_json(&set);
    assert_eq!(v["methodCalls"][0][0], "x:NetworkListener/set");
    let args = &v["methodCalls"][0][1];
    let destroy: Vec<&str> = args["destroy"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert_eq!(destroy, ["L-pop3s", "L-sieve"], "pop3s/sieve torn out; imaps kept");
    assert!(
        !destroy.contains(&"L-imaps"),
        "Mail.app IMAPS 993 must survive the port plan: {destroy:?}"
    );
    assert_eq!(args["create"]["imap"]["name"], "imap");
    assert_eq!(args["create"]["imap"]["bind"]["[::]:143"], true);
    assert_eq!(args["create"]["imap"]["tlsImplicit"], false);
    assert_eq!(
        args["create"]["imap"],
        crate::mail::imap_listeners::IMAP_STARTTLS.body(),
        "server-config creates imap from the shared template"
    );
    assert!(
        args["create"].get("imaps").is_none(),
        "imaps already in the fixture — not re-created"
    );
    assert_eq!(
        args["update"]["L-http"]["bind"]["127.0.0.1:8180"], true,
        "the 8080 listener becomes the loopback mgmt endpoint"
    );
    assert_eq!(
        args["update"]["L-https"]["bind"]["127.0.0.1:8443"], true,
        "http-01 plan keeps https loopback-only"
    );
    let sub = &args["create"]["k2"];
    assert_eq!(sub["name"], "submission");
    assert_eq!(sub["bind"]["[::]:587"], true);
    assert_eq!(sub["protocol"], "smtp");
    assert_eq!(sub["tlsImplicit"], false, "STARTTLS on 587");
    assert!(
        !destroy.contains(&"L-https"),
        "public /login on 443 is a follow-up — do not close https here: {destroy:?}"
    );
}

/// L12: bootstrap HTTP bind is 127.0.0.1:8080, never *:8080.
#[test]
fn bind_setup_http_loopback_is_127_not_world() {
    let get_reply = serde_json::json!({
        "methodResponses": [["x:NetworkListener/get", {
            "accountId": "b",
            "list": [
                { "id": "L-http", "name": "http", "bind": { "[::]:8080": true } },
                { "id": "L-https", "name": "https", "bind": { "[::]:443": true } },
            ],
            "notFound": [],
        }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:NetworkListener/set", {
            "updated": { "L-http": null },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        get_reply,
        set_reply,
    ]);
    use crate::mail::supervisor::BootstrapApi;
    let mut api = StalwartBootstrap::new();
    api.authenticate(&format!("http://127.0.0.1:{port}"), "admin@k2livebox.test", "pw")
        .expect("authenticate");
    api.bind_setup_http_loopback().expect("bind loopback 8080");

    let _sess = rx.recv().expect("req1");
    let _get = rx.recv().expect("req2");
    let set = rx.recv().expect("req3");
    let v = body_json(&set);
    assert_eq!(v["methodCalls"][0][0], "x:NetworkListener/set");
    let args = &v["methodCalls"][0][1];
    assert_eq!(
        args["update"]["L-http"]["bind"]["127.0.0.1:8080"], true,
        "bootstrap HTTP must bind loopback 8080: {args}"
    );
    assert!(
        args["update"]["L-http"]["bind"].get("[::]:8080").is_none(),
        "must not keep world [::]:8080: {args}"
    );
    assert!(
        args["update"]["L-http"]["bind"].get("0.0.0.0:8080").is_none(),
        "must not keep world 0.0.0.0:8080: {args}"
    );
    assert!(
        args["update"].get("L-https").is_none(),
        "do not close public /login on 443: {args}"
    );
}

/// Service-account + ApiKey round-trip: domain id resolved by
/// query; account is an Admin-role User with object-keyed
/// credentials (the live-verified list shape); the ApiKey rides
/// the TARGET accountId and pins the loopback.
#[test]
fn service_account_and_api_key_round_trip() {
    let domain_query = serde_json::json!({
        "methodResponses": [["x:Domain/query", { "ids": ["b"] }, "0"]],
    })
    .to_string();
    let account_set = serde_json::json!({
        "methodResponses": [["x:Account/set", {
            "created": { "k2": { "id": "d" } },
        }, "0"]],
    })
    .to_string();
    let key_set = serde_json::json!({
        "methodResponses": [["x:ApiKey/set", {
            "accountId": "d",
            "created": { "k2": { "id": "b", "secret": "API_once-shown" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        domain_query,
        account_set,
        key_set,
    ]);
    use crate::mail::supervisor::BootstrapApi;
    let mut api = StalwartBootstrap::new();
    api.authenticate(&format!("http://127.0.0.1:{port}"), "admin@k2livebox.test", "pw")
        .expect("authenticate");
    let account_id = api.create_service_account("k2livebox.test").expect("account");
    assert_eq!(account_id, "d");
    let secret = api.mint_api_key(&account_id).expect("key");
    assert_eq!(secret, "API_once-shown");

    let _sess = rx.recv().expect("req1");
    let q = body_json(&rx.recv().expect("req2"));
    assert_eq!(q["methodCalls"][0][0], "x:Domain/query");
    assert_eq!(q["methodCalls"][0][1]["filter"]["name"], "k2livebox.test");

    let a = body_json(&rx.recv().expect("req3"));
    assert_eq!(a["methodCalls"][0][0], "x:Account/set");
    let create = &a["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["@type"], "User");
    assert_eq!(create["name"], "k2-daemon");
    assert_eq!(create["domainId"], "b");
    assert_eq!(create["roles"]["@type"], "Admin");
    assert_eq!(
        create["credentials"]["0"]["@type"], "Password",
        "credentials are an INDEX-KEYED OBJECT (live-verified; an array is rejected)"
    );
    assert_eq!(create["credentials"]["0"]["secret"].as_str().unwrap().len(), 64);

    let k = body_json(&rx.recv().expect("req4"));
    assert_eq!(k["methodCalls"][0][0], "x:ApiKey/set");
    let args = &k["methodCalls"][0][1];
    assert_eq!(args["accountId"], "d", "the key rides the TARGET account");
    let create = &args["create"]["k2"];
    assert_eq!(create["allowedIps"]["127.0.0.1"], true, "pre-mortem #13: loopback-pinned");
    assert_eq!(create["permissions"]["@type"], "Inherit");
}

#[test]
fn http_error_replies_surface_status_without_credentials() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept");
        let mut buf = [0u8; 8192];
        let _ = stream.read(&mut buf);
        let body = r#"{"error":"unauthorized"}"#;
        let resp = format!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes()).expect("write");
    });
    let client = StalwartClient::new(format!("http://{addr}"), "bad-key");
    let err = client.get_json(SESSION_PATH).expect_err("401 is an error");
    server.join().expect("server thread");
    assert!(err.contains("401"), "{err}");
    assert!(!err.contains("bad-key"), "credential must never appear in errors: {err}");
}

/// Hostname retarget: `x:SystemSettings/set` singleton
/// `defaultHostname` — not Bootstrap/set, not a wipe.
#[test]
fn set_server_hostname_patches_system_settings_not_bootstrap() {
    let set_reply = serde_json::json!({
        "methodResponses": [["x:SystemSettings/set", {
            "accountId": "b",
            "updated": { "singleton": null },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![NORMAL_SESSION_FIXTURE.to_string(), set_reply]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    client
        .set_server_hostname("mail.lumon.io")
        .expect("set hostname");
    let _sess = rx.recv().expect("session");
    let req = rx.recv().expect("set");
    assert!(req.starts_with("POST /jmap/"), "{req}");
    let v = body_json(&req);
    assert_eq!(v["methodCalls"][0][0], "x:SystemSettings/set");
    assert_ne!(v["methodCalls"][0][0], "x:Bootstrap/set");
    let args = &v["methodCalls"][0][1];
    assert_eq!(args["update"]["singleton"]["defaultHostname"], "mail.lumon.io");
    assert!(
        args["update"]["singleton"].get("serverHostname").is_none(),
        "normal-mode field is defaultHostname, not Bootstrap serverHostname: {args}"
    );
    let err = client
        .set_server_hostname("  ")
        .expect_err("empty hostname must fail loud");
    assert!(err.contains("empty hostname"), "{err}");
}

/// Plant envelope: `x:Certificate/set` create uses Text/value +
/// Text/secret (not File paths), then SystemSettings
/// `defaultCertificateId`.
#[test]
fn certificate_create_envelope_is_text_pem_and_secret_key() {
    let chain = "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n";
    let key = "-----BEGIN PRIVATE KEY-----\nMIIB\n-----END PRIVATE KEY-----\n";
    let args = certificate_create_args(chain, key);
    let create = &args["create"]["k2"];
    assert_eq!(create["certificate"]["@type"], "Text");
    assert_eq!(create["certificate"]["value"], chain);
    assert!(
        create["certificate"].get("secret").is_none(),
        "certificate PEM is PublicText value, not secret: {create}"
    );
    assert_eq!(create["privateKey"]["@type"], "Text");
    assert_eq!(create["privateKey"]["secret"], key);
    assert!(
        create["privateKey"].get("value").is_none(),
        "privateKey is SecretText secret, not value: {create}"
    );
    assert!(
        create.get("filePath").is_none()
            && create["certificate"].get("filePath").is_none()
            && create["privateKey"].get("filePath").is_none(),
        "ProtectHome=yes: never File paths into ~/.k2/certs: {create}"
    );
    let env = registry_envelope("x:Certificate/set", args);
    assert_eq!(env["methodCalls"][0][0], "x:Certificate/set");
    assert_eq!(env["using"][1], "urn:stalwart:jmap");

    let patch = default_certificate_id_args("cert-plant-1");
    assert_eq!(
        patch["update"]["singleton"]["defaultCertificateId"],
        "cert-plant-1"
    );
}

#[test]
fn certificate_plant_sets_default_certificate_id() {
    let cert_reply = serde_json::json!({
        "methodResponses": [["x:Certificate/set", {
            "accountId": "b",
            "created": { "k2": { "id": "cert-plant-1" } },
        }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:SystemSettings/set", {
            "accountId": "b",
            "updated": { "singleton": null },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        cert_reply,
        set_reply,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let id = client
        .certificate_plant(
            "-----BEGIN CERTIFICATE-----\nLEAF\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n",
        )
        .expect("plant");
    assert_eq!(id, "cert-plant-1");
    let _sess = rx.recv().expect("session");
    let c = body_json(&rx.recv().expect("certificate/set"));
    assert_eq!(c["methodCalls"][0][0], "x:Certificate/set");
    let create = &c["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["certificate"]["@type"], "Text");
    assert_eq!(create["privateKey"]["@type"], "Text");
    assert!(
        create["privateKey"]["secret"].as_str().unwrap().contains("PRIVATE KEY"),
        "{create}"
    );
    let s = body_json(&rx.recv().expect("systemsettings/set"));
    assert_eq!(s["methodCalls"][0][0], "x:SystemSettings/set");
    assert_eq!(
        s["methodCalls"][0][1]["update"]["singleton"]["defaultCertificateId"],
        "cert-plant-1"
    );
}

#[test]
fn certificate_plant_surfaces_system_settings_method_error() {
    let cert_reply = serde_json::json!({
        "methodResponses": [["x:Certificate/set", {
            "created": { "k2": { "id": "cert-x" } },
        }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:SystemSettings/set", {
            "notUpdated": { "singleton": {
                "type": "invalidPatch",
                "description": "Failed to parse Id from string | Properties: defaultCertificateId",
            } },
        }, "0"]],
    })
    .to_string();
    let (port, _rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        cert_reply,
        set_reply,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let err = client
        .certificate_plant("-----BEGIN CERTIFICATE-----\nX\n-----END CERTIFICATE-----\n", "-----BEGIN PRIVATE KEY-----\nY\n-----END PRIVATE KEY-----\n")
        .expect_err("must fail loud");
    assert!(err.contains("invalidPatch"), "{err}");
    assert!(err.contains("defaultCertificateId"), "{err}");
}

/// CAL44: a per-name certificate is ONE `x:Certificate/set` create —
/// the client sends no `x:SystemSettings/set`, so
/// `defaultCertificateId` (the mail host's cert) never moves.
#[test]
fn certificate_add_never_touches_default_certificate_id() {
    let cert_reply = serde_json::json!({
        "methodResponses": [["x:Certificate/set", {
            "accountId": "b",
            "created": { "k2": { "id": "cert-aux-1" } },
        }, "0"]],
    })
    .to_string();
    // A second reply is queued so a stray SystemSettings/set would be
    // answered (and recorded) instead of hanging.
    let stray = serde_json::json!({
        "methodResponses": [["x:SystemSettings/set", {
            "updated": { "singleton": null },
        }, "0"]],
    })
    .to_string();
    let (port, rx) =
        spawn_mock_server(vec![NORMAL_SESSION_FIXTURE.to_string(), cert_reply, stray]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let id = client
        .certificate_add(
            "-----BEGIN CERTIFICATE-----\nAUX\n-----END CERTIFICATE-----\n",
            "-----BEGIN PRIVATE KEY-----\nKEY\n-----END PRIVATE KEY-----\n",
        )
        .expect("add");
    assert_eq!(id, "cert-aux-1");
    let _sess = rx.recv().expect("session");
    let c = body_json(&rx.recv().expect("certificate/set"));
    assert_eq!(c["methodCalls"].as_array().map(Vec::len), Some(1), "{c}");
    assert_eq!(c["methodCalls"][0][0], "x:Certificate/set");
    let create = &c["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["certificate"]["@type"], "Text");
    assert_eq!(create["privateKey"]["@type"], "Text");
    assert!(
        !c.to_string().contains("defaultCertificateId"),
        "a per-name cert must not carry defaultCertificateId: {c}"
    );
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(300)).is_err(),
        "certificate_add must not send a second request (SystemSettings/set)"
    );
    assert!(client.certificate_add("  ", "k").is_err(), "empty chain fails loud");
}

/// S0.10: the reload is `x:Action/set` create
/// `{"@type":"ReloadTlsCertificates"}`; a `notCreated` reply is an Err
/// (the caller then restarts).
#[test]
fn action_reload_tls_certificates_envelope_and_failure() {
    let ok = serde_json::json!({
        "methodResponses": [["x:Action/set", {
            "created": { "k2": { "id": "1760000000" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![NORMAL_SESSION_FIXTURE.to_string(), ok]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    client.action_reload_tls_certificates().expect("reload");
    let _sess = rx.recv().expect("session");
    let a = body_json(&rx.recv().expect("action/set"));
    assert_eq!(a["methodCalls"][0][0], "x:Action/set");
    assert_eq!(
        a["methodCalls"][0][1]["create"]["k2"],
        serde_json::json!({ "@type": "ReloadTlsCertificates" })
    );

    let bad = serde_json::json!({
        "methodResponses": [["x:Action/set", {
            "notCreated": { "k2": {
                "type": "validationFailed",
                "description": "Invalid certificate: No private keys found.",
            } },
        }, "0"]],
    })
    .to_string();
    let (port, _rx) = spawn_mock_server(vec![NORMAL_SESSION_FIXTURE.to_string(), bad]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let err = client.action_reload_tls_certificates().expect_err("notCreated must fail");
    assert!(err.contains("validationFailed") || err.contains("No private keys"), "{err}");
}

#[test]
fn default_certificate_state_present_unset_missing() {
    // Present: settings name cert-9 and Certificate/get lists it.
    let settings = |v: serde_json::Value| {
        serde_json::json!({
            "methodResponses": [["x:SystemSettings/get", {
                "list": [{ "id": "singleton", "defaultCertificateId": v }],
                "notFound": [],
            }, "0"]],
        })
        .to_string()
    };
    let listed = serde_json::json!({
        "methodResponses": [["x:Certificate/get", {
            "list": [{ "id": "cert-9", "notValidAfter": "2027-01-03T00:00:00Z" }],
            "notFound": [],
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        settings(serde_json::json!("cert-9")),
        listed,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    assert_eq!(
        client.default_certificate_state().expect("present"),
        DefaultCertificate::Present("cert-9".into())
    );
    let _sess = rx.recv().expect("session");
    let s = body_json(&rx.recv().expect("settings get"));
    assert_eq!(s["methodCalls"][0][0], "x:SystemSettings/get");
    assert_eq!(s["methodCalls"][0][1]["ids"], serde_json::json!(["singleton"]));
    let c = body_json(&rx.recv().expect("certificate get"));
    assert_eq!(c["methodCalls"][0][0], "x:Certificate/get");
    assert_eq!(c["methodCalls"][0][1]["ids"], serde_json::json!(["cert-9"]));

    // Unset: no second request.
    let (port, _rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        settings(serde_json::Value::Null),
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    assert_eq!(client.default_certificate_state().expect("unset"), DefaultCertificate::Unset);

    // Missing: the id dangles (Stalwart deleted the expired cert).
    let gone = serde_json::json!({
        "methodResponses": [["x:Certificate/get", {
            "list": [], "notFound": ["cert-old"],
        }, "0"]],
    })
    .to_string();
    let (port, _rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        settings(serde_json::json!("cert-old")),
        gone,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    assert_eq!(
        client.default_certificate_state().expect("missing"),
        DefaultCertificate::Missing("cert-old".into())
    );

    // Garbage: neither list nor notFound → loud.
    let err = parse_get_has_id("x:Certificate/get", "x", &serde_json::json!({})).expect_err("loud");
    assert!(err.contains("neither"), "{err}");
}

/// Cert renew: `x:Task/set` create AcmeRenewal for the mail
/// hostname's domain — no extra SAN names, no enable/bootstrap.
#[test]
fn renew_acme_records_task_create_without_enable() {
    let query_reply = serde_json::json!({
        "methodResponses": [["x:Domain/query", {
            "accountId": "b",
            "ids": ["dom-mail"],
        }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Task/set", {
            "accountId": "b",
            "created": { "k2": { "id": "task-acme-1" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        query_reply,
        cert_mgmt_get_reply("dom-mail", serde_json::json!({
            "@type": "Automatic",
            "acmeProviderId": "acme-p1",
            "subjectAlternativeNames": {},
        })),
        domain_set_updated_reply("dom-mail"),
        set_reply,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let id = client
        .renew_acme_for_mail_hostname("mail.acme.dev")
        .expect("renew");
    assert_eq!(id, "task-acme-1");
    let _sess = rx.recv().expect("session");
    let q = body_json(&rx.recv().expect("query"));
    assert_eq!(q["methodCalls"][0][0], "x:Domain/query");
    assert_eq!(q["methodCalls"][0][1]["filter"]["name"], "mail.acme.dev");
    // CAL43: the Domain's SAN list is locked BEFORE the renewal is
    // queued (the task carries no names; the order reads the Domain).
    let g = body_json(&rx.recv().expect("domain get"));
    assert_eq!(g["methodCalls"][0][0], "x:Domain/get");
    assert_eq!(g["methodCalls"][0][1]["ids"], serde_json::json!(["dom-mail"]));
    let lock = body_json(&rx.recv().expect("domain set"));
    assert_eq!(lock["methodCalls"][0][0], "x:Domain/set");
    assert_eq!(
        lock["methodCalls"][0][1]["update"]["dom-mail"]["certificateManagement"],
        serde_json::json!({
            "@type": "Automatic",
            "acmeProviderId": "acme-p1",
            "subjectAlternativeNames": { "mail.acme.dev": true },
        })
    );
    let t = body_json(&rx.recv().expect("task"));
    assert_eq!(t["methodCalls"][0][0], "x:Task/set");
    let create = &t["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["@type"], "AcmeRenewal");
    assert_eq!(create["domainId"], "dom-mail");
    assert!(
        create.get("subjectAlternativeNames").is_none(),
        "C8/C24: optional names must not ride the renew request: {create}"
    );
    assert!(
        create.get("names").is_none(),
        "C8/C24: no extra names field: {create}"
    );
    assert_ne!(t["methodCalls"][0][0], "x:Bootstrap/set");
}

#[test]
fn renew_acme_falls_back_to_default_domain_then_fails_loud() {
    let miss = serde_json::json!({
        "methodResponses": [["x:Domain/query", {
            "accountId": "b",
            "ids": [],
        }, "0"]],
    })
    .to_string();
    let hit = serde_json::json!({
        "methodResponses": [["x:Domain/query", {
            "accountId": "b",
            "ids": ["dom-parent"],
        }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Task/set", {
            "created": { "k2": { "id": "task-2" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        miss.clone(),
        hit,
        // Already locked: idempotent, no x:Domain/set.
        cert_mgmt_get_reply("dom-parent", serde_json::json!({
            "@type": "Automatic",
            "acmeProviderId": "acme-p1",
            "subjectAlternativeNames": { "MAIL.acme.dev.": true },
        })),
        set_reply,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    client
        .renew_acme_for_mail_hostname("mail.acme.dev")
        .expect("parent domain");
    let _sess = rx.recv().expect("session");
    let q1 = body_json(&rx.recv().expect("query host"));
    assert_eq!(q1["methodCalls"][0][1]["filter"]["name"], "mail.acme.dev");
    let q2 = body_json(&rx.recv().expect("query parent"));
    assert_eq!(q2["methodCalls"][0][1]["filter"]["name"], "acme.dev");
    let g = body_json(&rx.recv().expect("domain get"));
    assert_eq!(g["methodCalls"][0][0], "x:Domain/get");
    let t = body_json(&rx.recv().expect("task"));
    assert_eq!(t["methodCalls"][0][0], "x:Task/set", "no Domain/set when already locked");
    assert_eq!(t["methodCalls"][0][1]["create"]["k2"]["domainId"], "dom-parent");

    let (port2, _rx2) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        miss.clone(),
        miss,
    ]);
    let client2 = StalwartClient::new(format!("http://127.0.0.1:{port2}"), "k2-test-key");
    let err = client2
        .renew_acme_for_mail_hostname("mail.missing.test")
        .expect_err("no domain must fail loud");
    assert!(err.contains("mail.missing.test"), "{err}");
    assert!(err.contains("cannot retry ACME"), "{err}");
}

// ── CAL43: cert-name lock ───────────────────────────────────────

fn cert_mgmt_get_reply(id: &str, cm: serde_json::Value) -> String {
    serde_json::json!({
        "methodResponses": [["x:Domain/get", {
            "accountId": "b",
            "list": [{ "id": id, "certificateManagement": cm }],
            "notFound": [],
        }, "0"]],
    })
    .to_string()
}

fn domain_set_updated_reply(id: &str) -> String {
    serde_json::json!({
        "methodResponses": [["x:Domain/set", {
            "accountId": "b",
            "updated": { id: null },
        }, "0"]],
    })
    .to_string()
}

fn domain_query_reply(ids: &[&str]) -> String {
    serde_json::json!({
        "methodResponses": [["x:Domain/query", { "accountId": "b", "ids": ids }, "0"]],
    })
    .to_string()
}

/// The Domain update carries EXACTLY one SAN (the mail host) and
/// keeps the existing acmeProviderId — replacing Stalwart's
/// guided-setup default set.
#[test]
fn cert_name_lock_writes_exactly_the_mail_host_and_keeps_provider() {
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        domain_query_reply(&[]),
        domain_query_reply(&["dom-apex"]),
        cert_mgmt_get_reply("dom-apex", serde_json::json!({
            "@type": "Automatic",
            "acmeProviderId": "acme-p7",
            "subjectAlternativeNames": {
                "mta-sts.example.com": true,
                "autoconfig.example.com": true,
            },
        })),
        domain_set_updated_reply("dom-apex"),
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let out = client
        .lock_acme_cert_names_to_mail_host("Mail.Example.com.")
        .expect("lock");
    let CertNamesLock::Locked { mut previous } = out else {
        panic!("expected Locked, got {out:?}");
    };
    previous.sort();
    assert_eq!(
        previous,
        vec!["autoconfig.example.com".to_string(), "mta-sts.example.com".to_string()]
    );
    let _sess = rx.recv().expect("session");
    let q1 = body_json(&rx.recv().expect("query host"));
    assert_eq!(q1["methodCalls"][0][1]["filter"]["name"], "mail.example.com");
    let q2 = body_json(&rx.recv().expect("query apex"));
    assert_eq!(q2["methodCalls"][0][1]["filter"]["name"], "example.com");
    let g = body_json(&rx.recv().expect("get"));
    assert_eq!(
        g["methodCalls"][0][1]["properties"],
        serde_json::json!(["certificateManagement"])
    );
    let set = body_json(&rx.recv().expect("set"));
    assert_eq!(set["methodCalls"][0][0], "x:Domain/set");
    let update = set["methodCalls"][0][1]["update"]
        .as_object()
        .expect("update map");
    assert_eq!(update.len(), 1, "{update:?}");
    let cm = &update["dom-apex"]["certificateManagement"];
    assert_eq!(cm["@type"], "Automatic");
    assert_eq!(cm["acmeProviderId"], "acme-p7");
    let sans = cm["subjectAlternativeNames"].as_object().expect("SAN set object");
    assert_eq!(sans.len(), 1, "exactly one SAN: {sans:?}");
    assert_eq!(sans["mail.example.com"], true);
    // Only certificateManagement is patched — no other Domain field.
    assert_eq!(update["dom-apex"].as_object().expect("patch").len(), 1);
}

#[test]
fn cert_name_lock_is_idempotent_and_leaves_manual_alone() {
    // Already exactly [host]: no write.
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        domain_query_reply(&["dom-host"]),
        cert_mgmt_get_reply("dom-host", serde_json::json!({
            "@type": "Automatic",
            "acmeProviderId": "acme-p1",
            "subjectAlternativeNames": { "mail.example.com": true },
        })),
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    assert_eq!(
        client
            .lock_acme_cert_names_to_mail_host("mail.example.com")
            .expect("lock"),
        CertNamesLock::AlreadyLocked
    );
    let sent: Vec<String> = rx.iter().collect();
    assert_eq!(sent.len(), 3, "session + query + get only: {sent:#?}");
    assert!(!sent.iter().any(|r| r.contains("x:Domain/set")), "{sent:#?}");

    // Manual (Stalwart ACME off): no write.
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        domain_query_reply(&["dom-host"]),
        cert_mgmt_get_reply("dom-host", serde_json::json!({ "@type": "Manual" })),
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    assert_eq!(
        client
            .lock_acme_cert_names_to_mail_host("mail.example.com")
            .expect("lock"),
        CertNamesLock::NotAutomatic
    );
    let sent: Vec<String> = rx.iter().collect();
    assert_eq!(sent.len(), 3, "{sent:#?}");
    assert!(!sent.iter().any(|r| r.contains("x:Domain/set")), "{sent:#?}");
}

#[test]
fn cert_name_lock_surfaces_a_rejected_update() {
    let rejected = serde_json::json!({
        "methodResponses": [["x:Domain/set", {
            "notUpdated": { "dom-host": {
                "type": "invalidProperties",
                "description": "ACME provider not found",
            } },
        }, "0"]],
    })
    .to_string();
    let (port, _rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        domain_query_reply(&["dom-host"]),
        cert_mgmt_get_reply("dom-host", serde_json::json!({
            "@type": "Automatic",
            "acmeProviderId": "acme-p1",
        })),
        rejected,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let err = client
        .lock_acme_cert_names_to_mail_host("mail.example.com")
        .expect_err("must fail loud");
    assert!(err.contains("ACME provider not found"), "{err}");
}

/// A failed lock stops the renew: no AcmeRenewal is queued against
/// Stalwart's wide default name set.
#[test]
fn renew_does_not_queue_when_the_cert_name_lock_fails() {
    let rejected = serde_json::json!({
        "methodResponses": [["x:Domain/set", {
            "notUpdated": { "dom-host": { "type": "forbidden" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        domain_query_reply(&["dom-host"]),
        cert_mgmt_get_reply("dom-host", serde_json::json!({
            "@type": "Automatic",
            "acmeProviderId": "acme-p1",
            "subjectAlternativeNames": {},
        })),
        rejected,
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let err = client
        .renew_acme_for_mail_hostname("mail.example.com")
        .expect_err("lock failure must stop the renew");
    assert!(err.contains("CAL43"), "{err}");
    assert!(err.contains("forbidden"), "{err}");
    let sent: Vec<String> = rx.iter().collect();
    assert!(!sent.iter().any(|r| r.contains("x:Task/set")), "{sent:#?}");
}

/// K2-issuer boxes: the Manual write changes ONLY the Domain's
/// certificateManagement — never a Certificate, never
/// SystemSettings.defaultCertificateId, never a reload or restart.
#[test]
fn manual_switch_touches_only_certificate_management() {
    let args = cert_management_manual_args("dom-host");
    let update = args["update"].as_object().expect("update map");
    assert_eq!(update.len(), 1, "{args}");
    let patch = update["dom-host"].as_object().expect("patch");
    assert_eq!(patch.len(), 1, "only certificateManagement: {args}");
    assert_eq!(patch["certificateManagement"], serde_json::json!({ "@type": "Manual" }));
    assert!(args.get("create").is_none() && args.get("destroy").is_none(), "{args}");

    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        domain_set_updated_reply("dom-host"),
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    client.domain_set_cert_management_manual("dom-host").expect("manual");
    let _sess = rx.recv().expect("session");
    let set = body_json(&rx.recv().expect("set"));
    let calls = set["methodCalls"].as_array().expect("calls");
    assert_eq!(calls.len(), 1, "one call: {set}");
    assert_eq!(calls[0][0], "x:Domain/set");
    assert_eq!(calls[0][1]["update"], args["update"]);
    let rest: Vec<String> = rx.iter().collect();
    assert!(rest.is_empty(), "nothing after the one Domain/set: {rest:#?}");
    let all = format!("{set}");
    for never in ["x:Certificate", "x:SystemSettings", "defaultCertificateId", "x:Action"] {
        assert!(!all.contains(never), "{never} in {all}");
    }
}

#[test]
fn manual_switch_surfaces_a_rejected_update() {
    let rejected = serde_json::json!({
        "methodResponses": [["x:Domain/set", {
            "notUpdated": { "dom-host": { "type": "forbidden" } },
        }, "0"]],
    })
    .to_string();
    let (port, _rx) =
        spawn_mock_server(vec![NORMAL_SESSION_FIXTURE.to_string(), rejected]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let err = client
        .domain_set_cert_management_manual("dom-host")
        .expect_err("loud");
    assert!(err.contains("forbidden"), "{err}");
}

#[test]
fn acme_provider_ids_reads_the_list() {
    let reply = serde_json::json!({
        "methodResponses": [["x:AcmeProvider/get", {
            "accountId": "b",
            "list": [{ "id": "acme-p1" }, { "id": "acme-p2" }],
            "notFound": [],
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![NORMAL_SESSION_FIXTURE.to_string(), reply]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    assert_eq!(client.acme_provider_ids().expect("ids"), vec!["acme-p1", "acme-p2"]);
    let _sess = rx.recv().expect("session");
    let g = body_json(&rx.recv().expect("get"));
    assert_eq!(g["methodCalls"][0][0], "x:AcmeProvider/get");
    assert!(parse_get_list_ids("x:AcmeProvider/get", &serde_json::json!({})).is_err());
}

/// A Manual Domain is never flipped back by `cert renew`, and no
/// AcmeRenewal is queued for it: the error names the owner command.
#[test]
fn renew_refuses_a_manual_domain_without_flipping_it() {
    let (port, rx) = spawn_mock_server(vec![
        NORMAL_SESSION_FIXTURE.to_string(),
        domain_query_reply(&["dom-host"]),
        cert_mgmt_get_reply("dom-host", serde_json::json!({ "@type": "Manual" })),
    ]);
    let client = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let err = client
        .renew_acme_for_mail_hostname("mail.example.com")
        .expect_err("manual = nothing for Stalwart to renew");
    assert!(err.contains("Manual"), "{err}");
    assert!(err.contains("k2 hostmail cert owner --stalwart-acme"), "{err}");
    let sent: Vec<String> = rx.iter().collect();
    assert!(!sent.iter().any(|r| r.contains("x:Domain/set")), "{sent:#?}");
    assert!(!sent.iter().any(|r| r.contains("x:Task/set")), "{sent:#?}");
}

#[test]
fn cert_management_parser_reads_set_and_array_forms_and_refuses_junk() {
    let reply = |cm: serde_json::Value| {
        serde_json::json!({ "list": [{ "id": "d1", "certificateManagement": cm }] })
    };
    assert_eq!(
        parse_domain_get_cert_management("d1", &reply(serde_json::json!({ "@type": "Manual" })))
            .expect("manual"),
        CertManagement::Manual
    );
    assert_eq!(
        parse_domain_get_cert_management(
            "d1",
            &reply(serde_json::json!({
                "@type": "Automatic",
                "acmeProviderId": "p",
                "subjectAlternativeNames": { "a.example.com": true, "b.example.com": false },
            }))
        )
        .expect("set form"),
        CertManagement::Automatic {
            acme_provider_id: "p".into(),
            subject_alternative_names: vec!["a.example.com".into()],
        }
    );
    assert_eq!(
        parse_domain_get_cert_management(
            "d1",
            &reply(serde_json::json!({
                "@type": "Automatic",
                "acmeProviderId": "p",
                "subjectAlternativeNames": ["a.example.com"],
            }))
        )
        .expect("array form"),
        CertManagement::Automatic {
            acme_provider_id: "p".into(),
            subject_alternative_names: vec!["a.example.com".into()],
        }
    );
    assert_eq!(
        parse_domain_get_cert_management(
            "d1",
            &reply(serde_json::json!({ "@type": "SomethingNew" }))
        )
        .expect("unknown type"),
        CertManagement::Other("SomethingNew".into())
    );
    let no_provider = parse_domain_get_cert_management(
        "d1",
        &reply(serde_json::json!({ "@type": "Automatic" })),
    )
    .expect_err("no provider");
    assert!(no_provider.contains("acmeProviderId"), "{no_provider}");
    let missing = parse_domain_get_cert_management("d2", &reply(serde_json::json!({})))
        .expect_err("not in list");
    assert!(missing.contains("d2"), "{missing}");
    assert!(sans_locked_to(&["mail.example.com.".to_string()], "MAIL.example.com"));
    assert!(!sans_locked_to(&[], "mail.example.com"));
    assert!(!sans_locked_to(
        &["mail.example.com".to_string(), "autoconfig.example.com".to_string()],
        "mail.example.com"
    ));
}
