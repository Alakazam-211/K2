use super::tests::{body_json, spawn_mock_server};
use super::*;

#[test]
fn relay_route_names_are_domain_scoped_and_dot_free() {
    assert_eq!(relay_route_name("acme.dev"), "k2-relay-acme-dev");
    // Different domains never collide on a route name.
    assert_ne!(relay_route_name("a.dev"), relay_route_name("b.dev"));
}

/// The REAL default route expression (live 2026-07-10) — rewrite
/// keeps the local rule first, appends our sender-domain match,
/// and strips it again on clear.
#[test]
fn route_expression_rewrite_adds_and_removes_our_match() {
    let default_expr = serde_json::json!({
        "match": { "0": { "if": "is_local_domain(rcpt_domain)", "then": "'local'" } },
        "else": "'mx'",
    });
    let bound =
        rewrite_route_expression(&default_expr, "acme.dev", Some("k2-relay-acme-dev"))
            .expect("rewrite");
    assert_eq!(bound["match"]["0"]["if"], "is_local_domain(rcpt_domain)", "local rule first");
    assert_eq!(bound["match"]["1"]["if"], "sender_domain == 'acme.dev'");
    assert_eq!(bound["match"]["1"]["then"], "'k2-relay-acme-dev'");
    assert_eq!(bound["else"], "'mx'");

    // Re-apply is idempotent (the old match is dropped first).
    let rebound =
        rewrite_route_expression(&bound, "acme.dev", Some("k2-relay-acme-dev"))
            .expect("rewrite");
    assert_eq!(rebound, bound);

    // Clear removes only OUR match.
    let cleared = rewrite_route_expression(&bound, "acme.dev", None).expect("rewrite");
    assert_eq!(cleared, default_expr);

    // Missing else → loud.
    assert!(rewrite_route_expression(&serde_json::json!({}), "a.dev", None).is_err());
}

/// Apply round-trip: MtaRoute create + outbound-strategy rebind
/// (creds ride the create payload; the wire assertion is the ONLY
/// place the test looks for them).
#[test]
fn relay_route_apply_creates_route_and_binds_expression() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let routes_get = serde_json::json!({
        "methodResponses": [["x:MtaRoute/get", { "list": [], "notFound": [] }, "0"]],
    })
    .to_string();
    let route_set = serde_json::json!({
        "methodResponses": [["x:MtaRoute/set", {
            "created": { "k2": { "id": "R1" } },
        }, "0"]],
    })
    .to_string();
    let strategy_get = serde_json::json!({
        "methodResponses": [["x:MtaOutboundStrategy/get", {
            "list": [{
                "id": "singleton",
                "route": {
                    "match": { "0": { "if": "is_local_domain(rcpt_domain)", "then": "'local'" } },
                    "else": "'mx'",
                },
            }],
            "notFound": [],
        }, "0"]],
    })
    .to_string();
    let strategy_set = serde_json::json!({
        "methodResponses": [["x:MtaOutboundStrategy/set", {
            "updated": { "singleton": null },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        session,
        routes_get,
        route_set,
        strategy_get,
        strategy_set,
    ]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let route = RelayRoute {
        host: "smtp.mailgun.org".into(),
        port: 587,
        username: "postmaster@acme.dev".into(),
        password: "relay-pw".into(),
        implicit_tls: false,
    };
    c.relay_route_apply("acme.dev", Some(&route)).expect("apply");

    let _sess = rx.recv().expect("req1");
    let _routes = rx.recv().expect("req2");
    let set = body_json(&rx.recv().expect("req3"));
    assert_eq!(set["methodCalls"][0][0], "x:MtaRoute/set");
    let create = &set["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["@type"], "Relay");
    assert_eq!(create["name"], "k2-relay-acme-dev");
    assert_eq!(create["address"], "smtp.mailgun.org");
    assert_eq!(create["port"], 587);
    assert_eq!(create["protocol"], "smtp");
    assert_eq!(create["implicitTls"], false);
    assert_eq!(create["authUsername"], "postmaster@acme.dev");
    assert_eq!(create["authSecret"]["@type"], "Value");
    assert_eq!(create["authSecret"]["secret"], "relay-pw");

    let _sget = rx.recv().expect("req4");
    let sset = body_json(&rx.recv().expect("req5"));
    assert_eq!(sset["methodCalls"][0][0], "x:MtaOutboundStrategy/set");
    let expr = &sset["methodCalls"][0][1]["update"]["singleton"]["route"];
    assert_eq!(expr["match"]["1"]["if"], "sender_domain == 'acme.dev'");
    assert_eq!(expr["match"]["1"]["then"], "'k2-relay-acme-dev'");
}

/// Clear round-trip: expression unbound FIRST, then the route
/// destroyed (never a dangling expression reference).
#[test]
fn relay_route_clear_unbinds_then_destroys() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let routes_get = serde_json::json!({
        "methodResponses": [["x:MtaRoute/get", {
            "list": [{ "id": "R1", "name": "k2-relay-acme-dev" }],
            "notFound": [],
        }, "0"]],
    })
    .to_string();
    let strategy_get = serde_json::json!({
        "methodResponses": [["x:MtaOutboundStrategy/get", {
            "list": [{
                "id": "singleton",
                "route": {
                    "match": {
                        "0": { "if": "is_local_domain(rcpt_domain)", "then": "'local'" },
                        "1": { "if": "sender_domain == 'acme.dev'", "then": "'k2-relay-acme-dev'" },
                    },
                    "else": "'mx'",
                },
            }],
            "notFound": [],
        }, "0"]],
    })
    .to_string();
    let strategy_set = serde_json::json!({
        "methodResponses": [["x:MtaOutboundStrategy/set", {
            "updated": { "singleton": null },
        }, "0"]],
    })
    .to_string();
    let route_destroy = serde_json::json!({
        "methodResponses": [["x:MtaRoute/set", { "destroyed": ["R1"] }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![
        session,
        routes_get,
        strategy_get,
        strategy_set,
        route_destroy,
    ]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.relay_route_apply("acme.dev", None).expect("clear");

    let _sess = rx.recv().expect("req1");
    let _routes = rx.recv().expect("req2");
    let _sget = rx.recv().expect("req3");
    let sset = body_json(&rx.recv().expect("req4"));
    let expr = &sset["methodCalls"][0][1]["update"]["singleton"]["route"];
    assert!(
        expr["match"]["1"].is_null(),
        "our sender-domain match removed: {expr}"
    );
    let destroy = body_json(&rx.recv().expect("req5"));
    assert_eq!(destroy["methodCalls"][0][0], "x:MtaRoute/set");
    assert_eq!(destroy["methodCalls"][0][1]["destroy"][0], "R1");
}
