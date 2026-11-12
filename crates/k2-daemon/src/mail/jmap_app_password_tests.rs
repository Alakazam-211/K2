use super::tests::{body_json, spawn_mock_server};
use super::*;

const SESSION: &str = r#"{"apiUrl": "/jmap/", "accounts": {"svc": {}}, "primaryAccounts": {"urn:stalwart:jmap": "svc"}}"#;

#[test]
fn parse_create_yields_server_set_secret_once() {
    let ok = serde_json::json!({
        "accountId": "mbox",
        "created": { "k2": { "id": "ap1", "secret": "app_once-shown" } },
    });
    let created = parse_app_password_created(&ok).expect("created");
    assert_eq!(created.id, "ap1");
    assert_eq!(created.secret, "app_once-shown");

    let no_secret = serde_json::json!({ "created": { "k2": { "id": "ap1" } } });
    let err = parse_app_password_created(&no_secret).expect_err("secret required");
    assert!(err.contains("secret"), "{err}");

    let cap = serde_json::json!({
        "notCreated": { "k2": { "type": "overQuota", "description": "maxAppPasswords" } },
    });
    let err = parse_app_password_created(&cap).expect_err("cap");
    assert!(err.contains("overQuota"), "{err}");
    assert!(err.contains("maxAppPasswords"), "{err}");
}

#[test]
fn parse_list_never_keeps_secret() {
    let args = serde_json::json!({
        "list": [{
            "id": "ap1",
            "description": "Mail.app",
            "createdAt": "2026-09-18T00:00:00Z",
            "secret": "app_MUST_NOT_LEAK",
        }],
    });
    let rows = parse_app_password_list(&args);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "ap1");
    assert_eq!(rows[0].description, "Mail.app");
    assert_eq!(rows[0].created_at, "2026-09-18T00:00:00Z");
    let dumped = serde_json::to_string(&serde_json::json!({
        "id": rows[0].id,
        "label": rows[0].description,
        "createdAt": rows[0].created_at,
    }))
    .unwrap();
    assert!(
        !dumped.contains("app_MUST_NOT_LEAK"),
        "list JSON must not contain the secret: {dumped}"
    );
    assert!(!dumped.contains("secret"), "{dumped}");
}

#[test]
fn create_recorded_jmap_copies_api_key_envelope() {
    let created = serde_json::json!({
        "methodResponses": [["x:AppPassword/set", {
            "accountId": "mbox-1",
            "created": { "k2": { "id": "ap1", "secret": "app_once-shown" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), created]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let got = c.app_password_create("mbox-1", "  ").expect("create");
    assert_eq!(got.id, "ap1");
    assert_eq!(got.secret, "app_once-shown");
    assert!(got.secret.starts_with("app_"), "0.16.3+ prefix: {}", got.secret);

    let _sess = rx.recv().expect("session");
    let req = body_json(&rx.recv().expect("set"));
    assert_eq!(req["using"][1], "urn:stalwart:jmap");
    assert_eq!(req["methodCalls"][0][0], "x:AppPassword/set");
    let args = &req["methodCalls"][0][1];
    assert_eq!(
        args["accountId"], "mbox-1",
        "must address the mailbox, not the supervisor session"
    );
    let create = &args["create"]["k2"];
    assert_eq!(create["description"], "k2", "empty label defaults to k2");
    assert_eq!(create["permissions"]["@type"], "Inherit");
    assert_eq!(create["allowedIps"], serde_json::json!({}));
    assert!(
        create.get("secret").is_none(),
        "do not invent the secret: {create}"
    );
    assert!(
        args.get("update").is_none(),
        "must not patch Account.credentials: {args}"
    );
}

#[test]
fn create_on_wrong_account_is_engine_error() {
    let created = serde_json::json!({
        "methodResponses": [["x:AppPassword/set", {
            "accountId": "svc",
            "created": { "k2": { "id": "ap1", "secret": "app_wrong-account" } },
        }, "0"]],
    })
    .to_string();
    let (port, _rx) = spawn_mock_server(vec![SESSION.to_string(), created]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let err = c
        .app_password_create("mbox-1", "k2")
        .expect_err("must not hand supervisor secret to Mail.app");
    assert!(err.contains("svc"), "{err}");
    assert!(err.contains("mbox-1"), "{err}");
    assert!(
        !err.contains("app_wrong-account"),
        "must not leak the foreign secret: {err}"
    );
}

#[test]
fn list_get_omits_secret_property() {
    let query = serde_json::json!({
        "methodResponses": [["x:AppPassword/query", { "ids": ["ap1"] }, "0"]],
    })
    .to_string();
    let get = serde_json::json!({
        "methodResponses": [["x:AppPassword/get", {
            "list": [{ "id": "ap1", "description": "Mail.app", "createdAt": "2026-09-18T00:00:00Z" }],
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), query, get]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let rows = c.app_password_list("mbox-1").expect("list");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "ap1");
    let _sess = rx.recv().expect("session");
    let q = body_json(&rx.recv().expect("query"));
    assert_eq!(q["methodCalls"][0][0], "x:AppPassword/query");
    assert_eq!(q["methodCalls"][0][1]["accountId"], "mbox-1");
    assert!(
        q["methodCalls"][0][1].get("filter").is_none(),
        "v1 query has no expiresAt filter: {}",
        q["methodCalls"][0][1]
    );
    let g = body_json(&rx.recv().expect("get"));
    assert_eq!(g["methodCalls"][0][0], "x:AppPassword/get");
    let props = g["methodCalls"][0][1]["properties"]
        .as_array()
        .expect("properties");
    let props: Vec<&str> = props.iter().filter_map(|v| v.as_str()).collect();
    assert_eq!(props, ["id", "description", "createdAt"]);
    assert!(!props.contains(&"secret"));
}

#[test]
fn destroy_recorded_jmap() {
    let destroy = serde_json::json!({
        "methodResponses": [["x:AppPassword/set", { "destroyed": ["ap1"] }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), destroy]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.app_password_destroy("mbox-1", "ap1").expect("destroy");
    let _sess = rx.recv().expect("session");
    let req = body_json(&rx.recv().expect("set"));
    assert_eq!(req["methodCalls"][0][0], "x:AppPassword/set");
    assert_eq!(req["methodCalls"][0][1]["accountId"], "mbox-1");
    assert_eq!(req["methodCalls"][0][1]["destroy"][0], "ap1");
}

/// 0.46.6: the mailbox API key revoke (password rotation) — query
/// and destroy ride the mailbox's `accountId` on the service key.
#[test]
fn api_key_query_and_destroy_address_the_mailbox() {
    let query = serde_json::json!({
        "methodResponses": [["x:ApiKey/query", { "ids": ["k1", "k2"] }, "0"]],
    })
    .to_string();
    let destroyed = serde_json::json!({
        "methodResponses": [["x:ApiKey/set", { "destroyed": ["k1", "k2"] }, "0"]],
    })
    .to_string();
    let partial = serde_json::json!({
        "methodResponses": [["x:ApiKey/set", {
            "destroyed": ["k1"],
            "notDestroyed": { "k2": { "type": "forbidden" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), query, destroyed, partial]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    assert_eq!(c.api_key_query_ids("mbox-1").expect("query"), vec!["k1", "k2"]);
    let ids = vec!["k1".to_string(), "k2".to_string()];
    c.api_keys_destroy("mbox-1", &ids).expect("destroy");
    let err = c.api_keys_destroy("mbox-1", &ids).expect_err("a key left behind is an error");
    assert!(err.contains("k2") || err.contains("forbidden"), "{err}");
    // Nothing to destroy: no call at all (the mock has no reply left).
    c.api_keys_destroy("mbox-1", &[]).expect("empty is a no-op");

    let _sess = rx.recv().expect("session");
    let q = body_json(&rx.recv().expect("query"));
    assert_eq!(q["methodCalls"][0][0], "x:ApiKey/query");
    assert_eq!(q["methodCalls"][0][1]["accountId"], "mbox-1");
    let d = body_json(&rx.recv().expect("destroy"));
    assert_eq!(d["methodCalls"][0][0], "x:ApiKey/set");
    assert_eq!(d["methodCalls"][0][1]["accountId"], "mbox-1");
    assert_eq!(d["methodCalls"][0][1]["destroy"], serde_json::json!(["k1", "k2"]));
    assert!(d["methodCalls"][0][1].get("create").is_none(), "{d}");
}

#[test]
fn account_set_password_patches_credentials_0_only() {
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Account/set", { "updated": { "mbox-1": null } }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![SESSION.to_string(), set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.account_set_password("mbox-1", "new-primary-secret")
        .expect("rotate");
    let _sess = rx.recv().expect("session");
    let req = body_json(&rx.recv().expect("set"));
    assert_eq!(req["methodCalls"][0][0], "x:Account/set");
    let update = &req["methodCalls"][0][1]["update"]["mbox-1"];
    let creds = &update["credentials"];
    assert!(creds.is_object(), "never JSON-array credentials: {creds}");
    assert_eq!(creds["0"]["@type"], "Password");
    assert_eq!(creds["0"]["secret"], "new-primary-secret");
    assert!(creds.get("1").is_none(), "only index-0: {creds}");
    assert_ne!(creds["0"]["@type"], "AppPassword");
}

/// Request shape only (a recorded mock). ⚠ LIVE 2026-10-28 (0.16.10 and
/// 0.16.25): the real server drops app passwords on this write — see
/// `account_set_password` and tests/mail/stalwart-apikey-probe.py.
#[test]
fn app_password_survives_primary_rotate_recorded_jmap() {
    let created = serde_json::json!({
        "methodResponses": [["x:AppPassword/set", {
            "accountId": "mbox-1",
            "created": { "k2": { "id": "ap1", "secret": "app_lives" } },
        }, "0"]],
    })
    .to_string();
    let rotate = serde_json::json!({
        "methodResponses": [["x:Account/set", { "updated": { "mbox-1": null } }, "0"]],
    })
    .to_string();
    let query = serde_json::json!({
        "methodResponses": [["x:AppPassword/query", { "ids": ["ap1"] }, "0"]],
    })
    .to_string();
    let get = serde_json::json!({
        "methodResponses": [["x:AppPassword/get", {
            "list": [{ "id": "ap1", "description": "k2", "createdAt": "2026-09-18T00:00:00Z" }],
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        SESSION.to_string(),
        created,
        rotate,
        query,
        get,
    ]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let created = c.app_password_create("mbox-1", "k2").expect("create");
    assert_eq!(created.id, "ap1");
    c.account_set_password("mbox-1", "rotated-primary")
        .expect("rotate primary");
    let rows = c.app_password_list("mbox-1").expect("list after rotate");
    assert!(
        rows.iter().any(|r| r.id == "ap1"),
        "rotate must not wipe app passwords: {rows:?}"
    );

    let _sess = rx.recv().expect("session");
    let _create = rx.recv().expect("create");
    let rotate_req = body_json(&rx.recv().expect("rotate"));
    let creds = &rotate_req["methodCalls"][0][1]["update"]["mbox-1"]["credentials"];
    assert!(creds.is_object(), "{creds}");
    assert_eq!(creds["0"]["@type"], "Password");
    assert!(creds.get("1").is_none(), "{creds}");
    let _query = rx.recv().expect("query");
    let _get = rx.recv().expect("get");
}
