use super::tests::{body_json, spawn_mock_server};
use super::*;

#[test]
fn envelope_carries_registry_using_and_single_method_call() {
    let env = registry_envelope("x:Domain/set", serde_json::json!({"destroy": ["d1"]}));
    assert_eq!(env["using"][0], "urn:ietf:params:jmap:core");
    assert_eq!(env["using"][1], "urn:stalwart:jmap");
    assert_eq!(env["methodCalls"][0][0], "x:Domain/set");
    assert_eq!(env["methodCalls"][0][1]["destroy"][0], "d1");
    assert_eq!(env["methodCalls"][0][2], "0");
}

#[test]
fn method_response_unwraps_matching_method_and_surfaces_errors() {
    let ok = serde_json::json!({
        "methodResponses": [["x:Domain/set", { "created": {} }, "0"]],
        "sessionState": "s1",
    });
    let args = parse_method_response("x:Domain/set", &ok).expect("unwrapped");
    assert!(args.get("created").is_some());

    // JMAP-level error → named type + description (the REAL shape
    // a wrong method name earns on the live box).
    let err = serde_json::json!({
        "methodResponses": [["error", {
            "type": "unknownMethod",
            "description": "Domain/set is not known",
        }, "0"]],
    });
    let msg = parse_method_response("x:Domain/set", &err).expect_err("must reject");
    assert!(msg.contains("unknownMethod"), "{msg}");

    // Not a JMAP response at all → loud.
    let msg = parse_method_response("x:Domain/set", &serde_json::json!({"ok": true}))
        .expect_err("must reject");
    assert!(msg.contains("no methodResponses"), "{msg}");

    // A different method answering → loud.
    let odd = serde_json::json!({
        "methodResponses": [["x:Domain/get", {}, "0"]],
    });
    let msg = parse_method_response("x:Domain/set", &odd).expect_err("must reject");
    assert!(msg.contains("x:Domain/get"), "{msg}");
}

/// The REAL create reply (live 2026-07-10): id only — no zone file
/// rides the create.
#[test]
fn domain_set_created_parses_id_and_rejections() {
    let args = serde_json::json!({ "accountId": "d", "created": { "k2": { "id": "c" } } });
    assert_eq!(
        parse_set_created_id("x:Domain/set", &args).expect("created"),
        "c"
    );

    let rejected = serde_json::json!({
        "notCreated": { "k2": { "type": "alreadyExists",
                                 "description": "domain exists" } },
    });
    let msg = parse_set_created_id("x:Domain/set", &rejected).expect_err("must reject");
    assert!(msg.contains("alreadyExists"), "{msg}");
    assert!(msg.contains("domain exists"), "{msg}");

    assert!(parse_set_created_id("x:Domain/set", &serde_json::json!({})).is_err());
}

#[test]
fn domain_set_destroyed_requires_id_in_destroyed_list() {
    let ok = serde_json::json!({ "destroyed": ["dom-1", "dom-2"] });
    parse_set_destroyed("x:Domain/set", "dom-1", &ok).expect("destroyed");

    // The REAL refusal shape when DkimSignatures still link the
    // domain (live-verified: `objectIsLinked`).
    let rejected = serde_json::json!({
        "notDestroyed": { "dom-1": {
            "type": "objectIsLinked",
            "linkedObjects": [{ "object": "DkimSignature", "id": "iz1v70naabqa" }],
        } },
    });
    let msg =
        parse_set_destroyed("x:Domain/set", "dom-1", &rejected).expect_err("must reject");
    assert!(msg.contains("objectIsLinked"), "{msg}");

    let silent = serde_json::json!({ "destroyed": [] });
    assert!(parse_set_destroyed("x:Domain/set", "dom-1", &silent).is_err());
}

/// A trimmed REAL dnsZoneFile (live 2026-07-10) — MX + SPF + DKIM +
/// DMARC lines exactly as served.
const ZONE_FIXTURE: &str = "v1-ed25519-20260710._domainkey.k2livebox.test. IN TXT \"v=DKIM1; k=ed25519; h=sha256; p=1bt0i...\"\nk2livebox.test. IN TXT \"v=spf1 mx -all\"\nk2livebox.test. IN MX 10 mail.k2livebox.test.\n_dmarc.k2livebox.test. IN TXT \"v=DMARC1; p=reject; rua=mailto:postmaster@k2livebox.test\"\n";

#[test]
fn domain_get_zonefile_finds_our_id() {
    let args = serde_json::json!({
        "list": [
            { "id": "dom-other", "dnsZoneFile": "wrong.zone" },
            { "id": "b", "dnsZoneFile": ZONE_FIXTURE },
        ],
        "notFound": [],
    });
    let zone = parse_domain_get_zonefile("b", &args).expect("found");
    assert!(zone.contains("IN MX 10 mail.k2livebox.test."));
    assert!(zone.contains("_domainkey"));
    // Missing id / empty zone file → loud.
    assert!(parse_domain_get_zonefile("dom-9", &args).is_err());
    let empty = serde_json::json!({ "list": [{ "id": "b", "dnsZoneFile": "  " }] });
    assert!(parse_domain_get_zonefile("b", &empty).is_err());
}

/// Full `domain_create` round-trip: session discovery (once,
/// cached), the adopt-check query (empty → create), then the
/// x:Domain/set create envelope with the §6.1 args in the
/// live-verified shapes.
#[test]
fn domain_create_round_trip_against_loopback_mock() {
    let query_reply = serde_json::json!({
        "methodResponses": [["x:Domain/query", { "ids": [] }, "0"]],
    })
    .to_string();
    let created_reply = serde_json::json!({
        "methodResponses": [["x:Domain/set", {
            "accountId": "b",
            "created": { "k2": { "id": "c" } },
        }, "0"]],
    })
    .to_string();
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
            .to_string();
    let (port, rx) = spawn_mock_server(vec![session, query_reply, created_reply]);

    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let created = c.domain_create("acme.dev").expect("create round-trip");
    assert_eq!(created.id, "c");
    assert_eq!(created.dns_zone_file, None, "zone file is a separate get");

    let req1 = rx.recv().expect("first request recorded");
    assert!(req1.starts_with("GET /jmap/session"), "{req1}");
    assert!(req1.contains("authorization: Bearer k2-test-key")
        || req1.contains("Authorization: Bearer k2-test-key"), "{req1}");

    let q = body_json(&rx.recv().expect("adopt-check query"));
    assert_eq!(q["methodCalls"][0][0], "x:Domain/query");
    assert_eq!(q["methodCalls"][0][1]["filter"]["name"], "acme.dev");

    let req2 = rx.recv().expect("create request recorded");
    assert!(req2.starts_with("POST /jmap/"), "{req2}");
    let body = body_json(&req2);
    assert_eq!(body["methodCalls"][0][0], "x:Domain/set");
    assert_eq!(body["methodCalls"][0][1]["accountId"], "b");
    let create = &body["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["name"], "acme.dev");
    assert_eq!(create["isEnabled"], true);
    assert_eq!(create["dkimManagement"]["@type"], "Automatic");
    assert_eq!(create["subAddressing"]["@type"], "Enabled");
    assert!(create["catchAllAddress"].is_null(), "catch-all OFF by default");
    assert_eq!(create["dnsManagement"]["@type"], "Manual", "K2 never controls user DNS");
    assert_eq!(create["certificateManagement"]["@type"], "Manual");
}

/// The guided-setup default domain already exists in Stalwart —
/// `domain_create` ADOPTS it (query hit → no create call).
#[test]
fn domain_create_adopts_an_existing_domain() {
    let query_reply = serde_json::json!({
        "methodResponses": [["x:Domain/query", { "ids": ["b"] }, "0"]],
    })
    .to_string();
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
            .to_string();
    let (port, rx) = spawn_mock_server(vec![session, query_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let created = c.domain_create("k2livebox.test").expect("adopt");
    assert_eq!(created.id, "b");
    let _sess = rx.recv().expect("req1");
    let _query = rx.recv().expect("req2");
    assert!(
        rx.recv_timeout(std::time::Duration::from_millis(200)).is_err(),
        "no create call when adopting"
    );
}

/// domain_delete cascades the DkimSignature children first (the
/// live-verified objectIsLinked rule).
#[test]
fn domain_delete_cascades_dkim_signatures() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#
            .to_string();
    let dkim_query = serde_json::json!({
        "methodResponses": [["x:DkimSignature/query", {
            "ids": ["dk1", "dk2"],
        }, "0"]],
    })
    .to_string();
    let dkim_destroy = serde_json::json!({
        "methodResponses": [["x:DkimSignature/set", {
            "destroyed": ["dk1", "dk2"],
        }, "0"]],
    })
    .to_string();
    let domain_destroy = serde_json::json!({
        "methodResponses": [["x:Domain/set", { "destroyed": ["c"] }, "0"]],
    })
    .to_string();
    let (port, rx) =
        spawn_mock_server(vec![session, dkim_query, dkim_destroy, domain_destroy]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.domain_delete("c").expect("cascade delete");

    let _sess = rx.recv().expect("req1");
    let q = body_json(&rx.recv().expect("req2"));
    assert_eq!(q["methodCalls"][0][0], "x:DkimSignature/query");
    assert_eq!(q["methodCalls"][0][1]["filter"]["domainId"], "c");
    let d = body_json(&rx.recv().expect("req3"));
    assert_eq!(d["methodCalls"][0][0], "x:DkimSignature/set");
    assert_eq!(d["methodCalls"][0][1]["destroy"][0], "dk1");
    let dd = body_json(&rx.recv().expect("req4"));
    assert_eq!(dd["methodCalls"][0][0], "x:Domain/set");
    assert_eq!(dd["methodCalls"][0][1]["destroy"][0], "c");
}
