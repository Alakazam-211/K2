use super::tests::{body_json, spawn_mock_server};
use super::*;

fn session() -> String {
    r#"{"apiUrl":"/jmap/","accounts":{"b":{}},"primaryAccounts":{"urn:stalwart:jmap":"b","urn:ietf:params:jmap:mail":"b"}}"#.to_string()
}

fn method_ok(method: &str, args: serde_json::Value) -> String {
    serde_json::json!({ "methodResponses": [[method, args, "0"]] }).to_string()
}

#[test]
fn mailing_list_create_sends_recipients_set_object_not_array() {
    let created = method_ok(
        "x:MailingList/set",
        serde_json::json!({ "created": { "k2": { "id": "ml1" } } }),
    );
    let (port, rx) = spawn_mock_server(vec![session(), created]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let recips = recipients_set_object(&["a@acme.dev".into(), "b@acme.dev".into()]);
    let id = c
        .mailing_list_create("team", "dom-1", &recips)
        .expect("create");
    assert_eq!(id, "ml1");
    let _sess = rx.recv().expect("session");
    let body = body_json(&rx.recv().expect("set"));
    assert_eq!(body["methodCalls"][0][0], "x:MailingList/set");
    let create = &body["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["name"], "team");
    assert_eq!(create["domainId"], "dom-1");
    assert!(create["recipients"].is_object(), "{}", create);
    assert_eq!(create["recipients"]["a@acme.dev"], true);
    assert!(create["recipients"].as_array().is_none());
    assert_eq!(create["aliases"], serde_json::json!({}));
    assert!(
        create.get("@type").is_none(),
        "list is not an Account: {create}"
    );
}

#[test]
fn mta_stage_data_set_authenticated_as_match() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:MtaStageData/set", {
            "updated": { "singleton": null },
        }, "0"]],
    })
    .to_string();
    let script = rewrite_data_script_footer(
        &serde_json::json!({"else": "false"}),
        Some("k2-footer"),
    );
    assert_eq!(script["match"]["0"]["if"], "!is_empty(authenticated_as)");
    assert_eq!(script["match"]["0"]["then"], "'k2-footer'");
    assert_eq!(script["else"], "false");
    assert!(data_script_points_at_footer(&script, "k2-footer"));
    let cleared = rewrite_data_script_footer(&script, None);
    assert_eq!(cleared, serde_json::json!({"else": "false"}));
    assert!(!data_script_points_at_footer(&cleared, "k2-footer"));

    let (port, rx) = spawn_mock_server(vec![session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.mta_stage_data_set_script(script).expect("set");
    let _sess = rx.recv().expect("session");
    let set = body_json(&rx.recv().expect("MtaStageData/set"));
    assert_eq!(set["methodCalls"][0][0], "x:MtaStageData/set");
    let patch = &set["methodCalls"][0][1]["update"]["singleton"]["script"];
    assert_eq!(patch["match"]["0"]["if"], "!is_empty(authenticated_as)");
    assert_eq!(patch["match"]["0"]["then"], "'k2-footer'");
}

#[test]
fn sieve_scripts_destroy_deactivates() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["SieveScript/set", {
            "destroyed": ["s1"],
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.sieve_scripts_destroy("acct-e", &["s1".to_string()])
        .expect("destroy");
    let _sess = rx.recv().expect("session");
    let set = body_json(&rx.recv().expect("set"));
    assert_eq!(set["using"][1], "urn:ietf:params:jmap:sieve");
    assert_eq!(set["methodCalls"][0][0], "SieveScript/set");
    let args = &set["methodCalls"][0][1];
    assert_eq!(args["destroy"][0], "s1");
    assert!(args["onSuccessActivateScript"].is_null(), "{args}");
}

#[test]
fn mailing_list_destroy_is_set_destroy() {
    let destroyed = method_ok(
        "x:MailingList/set",
        serde_json::json!({ "destroyed": ["ml1"] }),
    );
    let (port, rx) = spawn_mock_server(vec![session(), destroyed]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.mailing_list_destroy("ml1").expect("destroy");
    let _sess = rx.recv().expect("session");
    let body = body_json(&rx.recv().expect("set"));
    assert_eq!(body["methodCalls"][0][1]["destroy"][0], "ml1");
}

#[test]
fn queued_message_retry_patches_next_retry_drop_destroys() {
    let updated = method_ok(
        "x:QueuedMessage/set",
        serde_json::json!({ "updated": { "q1": null } }),
    );
    let destroyed = method_ok(
        "x:QueuedMessage/set",
        serde_json::json!({ "destroyed": ["q1"] }),
    );
    let (port, rx) = spawn_mock_server(vec![session(), updated, destroyed]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.queued_message_retry("q1", "2026-09-18T00:00:00Z")
        .expect("retry");
    c.queued_message_destroy("q1").expect("drop");
    let _s1 = rx.recv().expect("s1");
    let retry = body_json(&rx.recv().expect("retry"));
    assert_eq!(retry["methodCalls"][0][0], "x:QueuedMessage/set");
    assert_eq!(
        retry["methodCalls"][0][1]["update"]["q1"]["nextRetry"],
        "2026-09-18T00:00:00Z"
    );
    assert!(
        retry["methodCalls"][0][1].get("destroy").is_none(),
        "retry is not destroy"
    );
    let drop = body_json(&rx.recv().expect("drop"));
    assert_eq!(drop["methodCalls"][0][1]["destroy"][0], "q1");
}

#[test]
fn memory_lookup_then_invalidate_caches() {
    let q = method_ok("x:MemoryLookupKey/query", serde_json::json!({ "ids": [] }));
    let created = method_ok(
        "x:MemoryLookupKey/set",
        serde_json::json!({ "created": { "k2": { "id": "lk1" } } }),
    );
    let action = method_ok(
        "x:Action/set",
        serde_json::json!({ "created": { "k2": { "id": "act1" } } }),
    );
    let (port, rx) = spawn_mock_server(vec![session(), q, created, action]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.memory_lookup_query_namespace("trusted-domains")
        .expect("probe");
    c.memory_lookup_key_create("trusted-domains", "spam.example")
        .expect("create");
    c.action_invalidate_caches().expect("invalidate");
    let _s = rx.recv().expect("session");
    let qbody = body_json(&rx.recv().expect("query"));
    assert_eq!(qbody["methodCalls"][0][0], "x:MemoryLookupKey/query");
    let cbody = body_json(&rx.recv().expect("create"));
    let create = &cbody["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["namespace"], "trusted-domains");
    assert_eq!(create["key"], "spam.example");
    assert_eq!(create["isGlobPattern"], false);
    let abody = body_json(&rx.recv().expect("action"));
    assert_eq!(
        abody["methodCalls"][0][1]["create"]["k2"]["@type"],
        "InvalidateCaches"
    );
}

#[test]
fn unknown_spam_method_fails_loud() {
    let err = serde_json::json!({
        "methodResponses": [["error", {
            "type": "unknownMethod",
            "description": "x:Spam/train is not known",
        }, "0"]],
    })
    .to_string();
    let (port, _rx) = spawn_mock_server(vec![session(), err]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let e = c
        .memory_lookup_query_namespace("nope")
        .expect_err("unknownMethod");
    assert!(e.contains("unknownMethod"), "{e}");
}

#[test]
fn recipients_set_roundtrip() {
    let v = recipients_set_object(&["A@B.test".into()]);
    assert_eq!(v["A@B.test"], true);
    assert_eq!(recipients_addrs(&serde_json::json!({"x@y.z": true})), vec!["x@y.z"]);
}

#[test]
fn parse_blocked_ip_get_fixture() {
    let args = serde_json::json!({
        "list": [{
            "id": "bip1",
            "address": "65.130.10.89",
            "reason": "authFailure",
            "createdAt": "2026-09-18T12:00:00Z",
            "expiresAt": null,
        }]
    });
    let rows = parse_ip_list_get(&args);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "bip1");
    assert_eq!(rows[0].address, "65.130.10.89");
    assert_eq!(rows[0].reason.as_deref(), Some("authFailure"));
    assert!(rows[0].expires_at.is_null());
}

#[test]
fn parse_security_get_object_and_null() {
    let with_rate = serde_json::json!({
        "list": [{
            "id": "singleton",
            "authBanRate": { "count": 100, "period": 86400000 },
        }]
    });
    let s = parse_security_get(&with_rate).expect("object");
    assert_eq!(
        s.auth_ban_rate,
        Some(AuthBanRate {
            count: 100,
            period: 86400000
        })
    );
    let nulled = serde_json::json!({
        "list": [{ "id": "singleton", "authBanRate": null }]
    });
    let s = parse_security_get(&nulled).expect("null");
    assert_eq!(s.auth_ban_rate, None);
}

#[test]
fn reload_actions_are_typed_not_invalidate() {
    let action = method_ok(
        "x:Action/set",
        serde_json::json!({ "created": { "k2": { "id": "act1" } } }),
    );
    let (port, rx) = spawn_mock_server(vec![session(), action.clone(), action]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.action_reload_blocked_ips().expect("reload blocked");
    c.action_reload_settings().expect("reload settings");
    let _s = rx.recv().expect("session");
    let blocked = body_json(&rx.recv().expect("blocked"));
    assert_eq!(
        blocked["methodCalls"][0][1]["create"]["k2"]["@type"],
        "ReloadBlockedIps"
    );
    assert_ne!(
        blocked["methodCalls"][0][1]["create"]["k2"]["@type"],
        "InvalidateCaches"
    );
    let settings = body_json(&rx.recv().expect("settings"));
    assert_eq!(
        settings["methodCalls"][0][1]["create"]["k2"]["@type"],
        "ReloadSettings"
    );
}

#[test]
fn security_set_auth_ban_rate_never_touches_period() {
    let set_reply = method_ok(
        "x:Security/set",
        serde_json::json!({ "updated": { "singleton": null } }),
    );
    let (port, rx) = spawn_mock_server(vec![session(), set_reply.clone(), set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.security_set_auth_ban_rate(None).expect("null rate");
    let rate = AuthBanRate {
        count: 100,
        period: 86400000,
    };
    c.security_set_auth_ban_rate(Some(&rate)).expect("restore");
    let _s = rx.recv().expect("session");
    let null_body = body_json(&rx.recv().expect("null set"));
    let update = &null_body["methodCalls"][0][1]["update"]["singleton"];
    assert!(update.get("authBanRate").unwrap().is_null());
    assert!(
        update.get("authBanPeriod").is_none(),
        "never write authBanPeriod: {update}"
    );
    let restore = body_json(&rx.recv().expect("restore set"));
    let update = &restore["methodCalls"][0][1]["update"]["singleton"];
    assert_eq!(update["authBanRate"]["count"], 100);
    assert_eq!(update["authBanRate"]["period"], 86400000);
    assert!(update.get("authBanPeriod").is_none());
}
