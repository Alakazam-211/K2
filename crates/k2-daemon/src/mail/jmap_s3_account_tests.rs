use super::tests::{body_json, spawn_mock_server};
use super::*;

// ── Pure reply parsers (fixtures, no network) ───────────────────

#[test]
fn account_set_created_parses_id_and_surfaces_rejections() {
    // The REAL create reply shape (live 2026-07-10).
    let ok = serde_json::json!({ "accountId": "d", "created": { "k2": { "id": "e" } } });
    assert_eq!(parse_set_created_id("x:Account/set", &ok).expect("created"), "e");

    // Server-set id missing/blank → loud.
    let no_id = serde_json::json!({ "created": { "k2": { "quota": 1 } } });
    assert!(parse_set_created_id("x:Account/set", &no_id).is_err());
    let blank = serde_json::json!({ "created": { "k2": { "id": "  " } } });
    assert!(parse_set_created_id("x:Account/set", &blank).is_err());

    // The REAL wrong-shape rejection (credentials as a JSON array)
    // — notCreated surfaces the server's SetError verbatim.
    let rejected = serde_json::json!({
        "notCreated": { "k2": { "type": "invalidPatch",
                                "description": "Invalid value for object property",
                                "properties": ["credentials"] } },
    });
    let msg = parse_set_created_id("x:Account/set", &rejected).expect_err("must reject");
    assert!(msg.contains("invalidPatch"), "{msg}");

    // Neither → loud.
    assert!(parse_set_created_id("x:Account/set", &serde_json::json!({})).is_err());
}

#[test]
fn account_set_updated_requires_id_in_updated_map() {
    // RFC 8620 /set: updated maps id → null (the live reply shape).
    let ok = serde_json::json!({ "updated": { "f": null } });
    parse_set_updated("x:Account/set", "f", &ok).expect("updated");

    let rejected = serde_json::json!({
        "notUpdated": { "f": { "type": "forbidden" } },
    });
    let msg = parse_set_updated("x:Account/set", "f", &rejected).expect_err("must reject");
    assert!(msg.contains("forbidden"), "{msg}");

    // Someone ELSE updated / empty reply → loud, never a silent ok.
    let other = serde_json::json!({ "updated": { "zz": null } });
    assert!(parse_set_updated("x:Account/set", "f", &other).is_err());
    assert!(parse_set_updated("x:Account/set", "f", &serde_json::json!({})).is_err());
}

#[test]
fn account_get_name_parses_the_list_entry() {
    let args = serde_json::json!({
        "list": [{ "id": "e", "name": "research-bot" }],
        "notFound": [],
    });
    assert_eq!(parse_account_get_name("e", &args).expect("name"), "research-bot");
    assert!(parse_account_get_name("zz", &args).is_err());
}

#[test]
fn account_create_round_trip_against_loopback_mock() {
    let created_reply = serde_json::json!({
        "methodResponses": [["x:Account/set", {
            "created": { "k2": { "id": "e" } },
        }, "0"]],
    })
    .to_string();
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let (port, rx) = spawn_mock_server(vec![session, created_reply]);

    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let id = c
        .account_create("research-bot", "b", "s3cret-pw", 1_073_741_824, 10_000, None)
        .expect("create round-trip");
    assert_eq!(id, "e");

    let req1 = rx.recv().expect("first request recorded");
    assert!(req1.starts_with("GET /jmap/session"), "{req1}");

    // The JMAP envelope with the LIVE-VERIFIED create shape.
    let req2 = rx.recv().expect("second request recorded");
    assert!(req2.starts_with("POST /jmap/"), "{req2}");
    let body = body_json(&req2);
    assert_eq!(body["using"][1], "urn:stalwart:jmap");
    assert_eq!(body["methodCalls"][0][0], "x:Account/set");
    let create = &body["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["@type"], "User");
    assert_eq!(create["name"], "research-bot");
    assert_eq!(create["domainId"], "b");
    assert_eq!(create["credentials"]["0"]["@type"], "Password");
    assert_eq!(create["credentials"]["0"]["secret"], "s3cret-pw");
    assert_eq!(create["quotas"]["maxDiskQuota"], 1_073_741_824u64, "§12: 1 GB quota");
    assert_eq!(create["quotas"]["maxEmails"], 10_000, "§12: 10k message cap");
    assert!(
        create.get("permissions").is_none(),
        "no DAV policy → Stalwart's Inherit default: {create}"
    );
}

/// Calendars S2: a stored DAV policy rides the create call.
#[test]
fn account_create_carries_the_dav_policy_permissions() {
    let created_reply = serde_json::json!({
        "methodResponses": [["x:Account/set", { "created": { "k2": { "id": "e" } } }, "0"]],
    })
    .to_string();
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let (port, rx) = spawn_mock_server(vec![session, created_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let perms = serde_json::json!({
        "@type": "Merge",
        "enabledPermissions": {},
        "disabledPermissions": {"davFileGet": true},
    });
    c.account_create("bot", "b", "pw", 1, 1, Some(&perms)).expect("create");
    let _sess = rx.recv().expect("session");
    let body = body_json(&rx.recv().expect("set"));
    assert_eq!(body["methodCalls"][0][1]["create"]["k2"]["permissions"], perms);
}

/// Calendars S2: read = one x:Account/get for every id with the
/// policy fields; write = one x:Account/set update of `permissions`
/// only (no name/credentials/quotas in the patch).
#[test]
fn account_permissions_get_and_set_wire_shapes() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let get_reply = serde_json::json!({
        "methodResponses": [["x:Account/get", {
            "list": [{"id": "e", "@type": "User", "name": "bot", "roles": {"@type": "User"},
                      "permissions": {"@type": "Inherit"}}],
            "notFound": ["zz"],
        }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
    })
    .to_string();
    // The session is discovered once and cached by the client.
    let (port, rx) = spawn_mock_server(vec![session, get_reply, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let rows = c
        .account_get_permissions(&["e".to_string(), "zz".to_string()])
        .expect("get");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["permissions"]["@type"], "Inherit");
    let _sess = rx.recv().expect("session");
    let get = body_json(&rx.recv().expect("get"));
    assert_eq!(get["methodCalls"][0][0], "x:Account/get");
    assert_eq!(get["methodCalls"][0][1]["ids"], serde_json::json!(["e", "zz"]));
    assert_eq!(
        get["methodCalls"][0][1]["properties"],
        serde_json::json!(["@type", "name", "roles", "permissions"])
    );

    let perms = serde_json::json!({
        "@type": "Merge",
        "enabledPermissions": {},
        "disabledPermissions": {"davFileGet": true},
    });
    c.account_set_permissions("e", &perms).expect("set");
    let set = body_json(&rx.recv().expect("set"));
    assert_eq!(set["methodCalls"][0][0], "x:Account/set");
    let patch = &set["methodCalls"][0][1]["update"]["e"];
    assert_eq!(patch, &serde_json::json!({ "permissions": perms }));

    // Empty id list never dials. The URL is a test server that
    // answers 503, so a dial would fail fast and loudly (a dead port
    // such as :9 can hang where loopback RSTs are dropped).
    let down = k2_core::test_env::ErrorHttpServer::start(503);
    let idle = StalwartClient::new(down.url(), "k2-test-key");
    assert!(idle.account_get_permissions(&[]).expect("empty").is_empty());
}

#[test]
fn account_set_quotas_records_account_set_update() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.account_set_quotas("e", Some(3_221_225_472), Some(40_000))
        .expect("quota update");

    let _sess = rx.recv().expect("session");
    let req = rx.recv().expect("Account/set");
    assert!(req.starts_with("POST /jmap/"), "{req}");
    let body = body_json(&req);
    assert_eq!(body["methodCalls"][0][0], "x:Account/set");
    let patch = &body["methodCalls"][0][1]["update"]["e"]["quotas"];
    assert_eq!(patch["maxDiskQuota"], 3_221_225_472u64);
    assert_eq!(patch["maxEmails"], 40_000);
}

#[test]
fn account_set_quotas_zero_is_unlimited() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.account_set_quotas("e", Some(0), None).expect("0 = unlimited");
    let _sess = rx.recv().expect("session");
    let req = rx.recv().expect("Account/set");
    let body = body_json(&req);
    let patch = &body["methodCalls"][0][1]["update"]["e"]["quotas"];
    assert_eq!(patch["maxDiskQuota"], 0);
    assert!(patch.get("maxEmails").is_none(), "unset field omitted: {patch}");
}

#[test]
fn account_get_quotas_parses_the_list_entry() {
    let args = serde_json::json!({
        "list": [{ "id": "e", "quotas": { "maxDiskQuota": 1073741824u64, "maxEmails": 10000 } }],
        "notFound": [],
    });
    let q = parse_account_get_quotas("e", &args).expect("quotas");
    assert_eq!(q["maxDiskQuota"], 1_073_741_824u64);
    assert_eq!(q["maxEmails"], 10_000);
    assert!(parse_account_get_quotas("zz", &args).is_err());
}

/// Retire = rename (the live-verified v0.16 disable mechanism):
/// get the current name, rename to the retired local part;
/// idempotent when already retired.
#[test]
fn account_disable_renames_and_is_idempotent() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let get_reply = serde_json::json!({
        "methodResponses": [["x:Account/get", {
            "list": [{ "id": "e", "name": "research-bot" }], "notFound": [],
        }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session.clone(), get_reply, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.account_disable("e").expect("disable");
    let _sess = rx.recv().expect("req1");
    let _get = rx.recv().expect("req2");
    let set = body_json(&rx.recv().expect("req3"));
    let new_name = set["methodCalls"][0][1]["update"]["e"]["name"]
        .as_str()
        .expect("rename update");
    assert!(new_name.starts_with("research-bot-k2r-"), "{new_name}");

    // Already-retired name → no update call happens (idempotent).
    let retired_get = serde_json::json!({
        "methodResponses": [["x:Account/get", {
            "list": [{ "id": "e", "name": "research-bot-k2r-1760000000" }], "notFound": [],
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, retired_get]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.account_disable("e").expect("idempotent disable");
    let _sess = rx.recv().expect("req1");
    let _get = rx.recv().expect("req2");
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(),
        "no rename when already retired"
    );
}

#[test]
fn account_engine_errors_never_leak_the_password() {
    // A refused/failed create must surface the transport error
    // without the secret riding along (mirrors the S1 credential
    // rule). Closed port → immediate refusal, no live dial.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener); // port now closed
    let c = StalwartClient::new(format!("http://{addr}"), "k2-test-key");
    let err = c
        .account_create("bot", "dom-1", "super-secret-pw", 1, 1, None)
        .expect_err("closed port must fail");
    assert!(!err.contains("super-secret-pw"), "password leaked: {err}");
}

#[test]
fn domain_set_catchall_sends_null_not_empty_and_omits_aliases() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
            .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Domain/set", { "updated": { "dom-1": null } }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session.clone(), set_reply.clone()]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.domain_set_catchall("dom-1", None).expect("unset");
    let _sess = rx.recv().expect("session");
    let req = rx.recv().expect("Domain/set");
    let body = body_json(&req);
    assert_eq!(body["methodCalls"][0][0], "x:Domain/set");
    let patch = &body["methodCalls"][0][1]["update"]["dom-1"];
    assert!(patch["catchAllAddress"].is_null(), "{patch}");
    assert!(
        patch.get("aliases").is_none(),
        "Domain.aliases must never be written: {patch}"
    );

    let set_reply = serde_json::json!({
        "methodResponses": [["x:Domain/set", { "updated": { "dom-1": null } }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.domain_set_catchall("dom-1", Some("post@acme.dev"))
        .expect("set");
    let _sess = rx.recv().expect("session");
    let req = rx.recv().expect("Domain/set");
    let body = body_json(&req);
    let patch = &body["methodCalls"][0][1]["update"]["dom-1"];
    assert_eq!(patch["catchAllAddress"], "post@acme.dev");
    assert!(patch.get("aliases").is_none(), "{patch}");
}

#[test]
fn domain_get_catchall_parses_null_and_addr() {
    let null = serde_json::json!({
        "list": [{ "id": "dom-1", "catchAllAddress": null }],
        "notFound": [],
    });
    assert_eq!(parse_domain_get_catchall("dom-1", &null).expect("null"), None);
    let addr = serde_json::json!({
        "list": [{ "id": "dom-1", "catchAllAddress": "post@acme.dev" }],
    });
    assert_eq!(
        parse_domain_get_catchall("dom-1", &addr).expect("addr"),
        Some("post@acme.dev".into())
    );
    assert!(parse_domain_get_catchall("zz", &addr).is_err());
}

#[test]
fn account_set_aliases_is_index_keyed_not_array() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:Account/set", { "updated": { "e": null } }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.account_set_aliases(
        "e",
        &[EmailAlias {
            name: "sales".into(),
            domain_id: "dom-1".into(),
            enabled: true,
        }],
    )
    .expect("set aliases");
    let _sess = rx.recv().expect("session");
    let req = rx.recv().expect("Account/set");
    let body = body_json(&req);
    assert_eq!(body["methodCalls"][0][0], "x:Account/set");
    assert_ne!(body["methodCalls"][0][0], "x:User/set");
    let aliases = &body["methodCalls"][0][1]["update"]["e"]["aliases"];
    assert!(aliases.is_object(), "index-keyed object, not array: {aliases}");
    assert!(aliases.as_array().is_none(), "{aliases}");
    assert_eq!(aliases["0"]["name"], "sales");
    assert_eq!(aliases["0"]["domainId"], "dom-1");
    assert_eq!(aliases["0"]["enabled"], true);
}

#[test]
fn parse_email_aliases_index_keyed_and_array() {
    let keyed = serde_json::json!({
        "1": { "name": "info", "domainId": "d", "enabled": true },
        "0": { "name": "Sales", "domainId": "d", "enabled": true },
    });
    let got = parse_email_aliases(Some(&keyed));
    assert_eq!(got[0].name, "sales");
    assert_eq!(got[1].name, "info");
    let arr = serde_json::json!([{ "name": "x", "domainId": "d" }]);
    assert_eq!(parse_email_aliases(Some(&arr))[0].name, "x");
    assert!(parse_email_aliases(Some(&serde_json::Value::Null)).is_empty());
}

#[test]
fn k2_forward_script_keep_and_default() {
    let def = k2_forward_script("dest@acme.dev", false);
    assert!(def.contains("redirect \"dest@acme.dev\";"), "{def}");
    assert!(!def.contains(":copy"), "{def}");
    assert!(!def.contains("require"), "{def}");
    let keep = k2_forward_script("dest@acme.dev", true);
    assert!(keep.contains("require [\"copy\"];"), "{keep}");
    assert!(keep.contains("redirect :copy \"dest@acme.dev\";"), "{keep}");
    assert_eq!(
        parse_k2_forward_script(&def),
        Some(("dest@acme.dev".into(), false))
    );
    assert_eq!(
        parse_k2_forward_script(&keep),
        Some(("dest@acme.dev".into(), true))
    );
}

#[test]
fn sieve_k2_forward_set_declares_sieve_capability_and_name() {
    let session = serde_json::json!({
        "apiUrl": "/jmap/",
        "uploadUrl": "/jmap/upload/{accountId}/",
        "accounts": { "b": {} },
        "primaryAccounts": { "urn:stalwart:jmap": "b" },
    })
    .to_string();
    let blob = serde_json::json!({ "blobId": "Bfwd" }).to_string();
    let query = serde_json::json!({
        "methodResponses": [["SieveScript/query", { "ids": [] }, "0"]],
    })
    .to_string();
    let set = serde_json::json!({
        "methodResponses": [["SieveScript/set", {
            "created": { "k2": { "id": "s1" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        session.clone(),
        blob,
        session,
        query,
        set,
    ]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.sieve_k2_forward_set("acc-1", "dest@acme.dev", true)
        .expect("set forward");

    let blob_sess = rx.recv().expect("blob session");
    assert!(blob_sess.starts_with("GET /jmap/session"), "{blob_sess}");
    let blob_req = rx.recv().expect("blob POST");
    assert!(blob_req.contains("POST /jmap/upload/acc-1/"), "{blob_req}");
    assert!(
        blob_req.contains("require [\"copy\"];") && blob_req.contains("redirect :copy"),
        "{blob_req}"
    );
    let _disc = rx.recv().expect("sieve session");
    let q = body_json(&rx.recv().expect("query"));
    assert_eq!(q["methodCalls"][0][0], "SieveScript/query");
    assert_eq!(q["using"][1], "urn:ietf:params:jmap:sieve");
    assert_eq!(q["methodCalls"][0][1]["filter"]["name"], K2_FORWARD_SCRIPT);
    let set_req = body_json(&rx.recv().expect("set"));
    assert_eq!(set_req["methodCalls"][0][0], "SieveScript/set");
    assert_ne!(set_req["methodCalls"][0][0], "x:SieveScript/set");
    assert_ne!(set_req["methodCalls"][0][0], "x:SieveSystemScript/set");
    assert_eq!(set_req["using"][1], "urn:ietf:params:jmap:sieve");
    let create = &set_req["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["name"], "k2-forward");
    assert_eq!(create["blobId"], "Bfwd");
    assert_eq!(
        set_req["methodCalls"][0][1]["onSuccessActivateScript"],
        "#k2"
    );
}

#[test]
fn sieve_k2_forward_destroy_only_that_name() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
            .to_string();
    let query = serde_json::json!({
        "methodResponses": [["SieveScript/query", { "ids": ["s1"] }, "0"]],
    })
    .to_string();
    let destroy = serde_json::json!({
        "methodResponses": [["SieveScript/set", { "destroyed": ["s1"] }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, query, destroy]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.sieve_k2_forward_destroy("acc-1").expect("destroy");
    let _sess = rx.recv().expect("session");
    let q = body_json(&rx.recv().expect("query"));
    assert_eq!(q["methodCalls"][0][1]["filter"]["name"], "k2-forward");
    let d = body_json(&rx.recv().expect("destroy"));
    assert_eq!(d["methodCalls"][0][0], "SieveScript/set");
    assert_eq!(d["methodCalls"][0][1]["destroy"][0], "s1");
    assert!(
        d["methodCalls"][0][1].get("create").is_none(),
        "unset must not recreate: {d}"
    );
}
