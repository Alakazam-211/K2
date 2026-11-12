use super::tests::{body_json, spawn_mock_server};
use super::*;

#[test]
fn sieve_envelope_is_rfc_9661_not_registry() {
    let env = sieve_envelope("SieveScript/set", serde_json::json!({"accountId": "e"}));
    assert_eq!(env["using"][0], "urn:ietf:params:jmap:core");
    assert_eq!(env["using"][1], "urn:ietf:params:jmap:sieve");
    assert_eq!(env["using"].as_array().map(|a| a.len()), Some(2));
    assert_ne!(env["using"][1], "urn:stalwart:jmap");
    assert_eq!(env["methodCalls"][0][0], "SieveScript/set");
    assert_ne!(env["methodCalls"][0][0], "x:SieveScript/set");
    assert_ne!(env["methodCalls"][0][0], "VacationResponse/set");
}

#[test]
fn sieve_script_put_k2_inactive_doesn_t_activate_it() {
    let session =
        r#"{"apiUrl": "/jmap/", "uploadUrl": "http://127.0.0.1/upload/{accountId}", "accounts": {"d": {}}, "primaryAccounts": {"urn:ietf:params:jmap:core": "d"}}"#
            .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["SieveScript/set", { "updated": { "s1": null } }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.sieve_script_put_k2_inactive("acct-e", Some("s1"), "blob-k2").expect("SieveScript/set");
    let _sess = rx.recv().expect("session");
    let set = body_json(&rx.recv().expect("SieveScript/set"));
    let args = &set["methodCalls"][0][1];
    assert_eq!(args["update"]["s1"]["blobId"], "blob-k2");
    assert!(args.get("onSuccessActivateScript").is_none(), "{args}");
}

#[test]
fn sieve_script_put_k2_records_blob_then_set_with_sieve_using() {
    let session =
        r#"{"apiUrl": "/jmap/", "uploadUrl": "http://127.0.0.1/upload/{accountId}", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let upload = r#"{"blobId":"blob-k2"}"#.to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["SieveScript/set", {
            "created": { "k2": { "id": "s1" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session.clone(), upload, session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let blob = c
        .blob_upload("acct-e", b"require [\"vacation\"];")
        .expect("upload");
    assert_eq!(blob, "blob-k2");
    c.sieve_script_put_k2("acct-e", None, &blob, &[])
        .expect("SieveScript/set");

    let _sess = rx.recv().expect("session");
    let up = rx.recv().expect("blob POST");
    assert!(up.contains("POST "), "{up}");
    let _sess2 = rx.recv().expect("session 2");
    let set = body_json(&rx.recv().expect("SieveScript/set"));
    assert_eq!(set["using"][1], "urn:ietf:params:jmap:sieve");
    assert_eq!(set["methodCalls"][0][0], "SieveScript/set");
    assert_ne!(set["methodCalls"][0][0], "x:SieveScript/set");
    assert_ne!(set["methodCalls"][0][0], "VacationResponse/set");
    let args = &set["methodCalls"][0][1];
    assert_eq!(args["accountId"], "acct-e");
    assert_eq!(args["create"]["k2"]["name"], "k2");
    assert_eq!(args["create"]["k2"]["blobId"], "blob-k2");
    assert_eq!(args["onSuccessActivateScript"], "#k2");
}

#[test]
fn sieve_script_put_k2_update_destroys_legacy_forward() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["SieveScript/set", {
            "updated": { "s1": null },
            "destroyed": ["fwd1"],
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.sieve_script_put_k2("acct-e", Some("s1"), "blob-2", &["fwd1".to_string()])
        .expect("update");
    let _sess = rx.recv().expect("session");
    let set = body_json(&rx.recv().expect("set"));
    let args = &set["methodCalls"][0][1];
    assert_eq!(args["update"]["s1"]["blobId"], "blob-2");
    assert_eq!(args["destroy"][0], "fwd1");
    assert_eq!(args["onSuccessActivateScript"], "s1");
    assert_eq!(set["methodCalls"][0][0], "SieveScript/set");
}

#[test]
fn system_script_put_is_registry_k2_footer() {
    let session =
        r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}, "primaryAccounts": {"urn:stalwart:jmap": "d"}}"#
            .to_string();
    let get_reply = serde_json::json!({
        "methodResponses": [["x:SieveSystemScript/get", { "list": [], "notFound": [] }, "0"]],
    })
    .to_string();
    let set_reply = serde_json::json!({
        "methodResponses": [["x:SieveSystemScript/set", {
            "created": { "k2": { "id": "sys1" } },
        }, "0"]],
    })
    .to_string();
    let (port, rx) = spawn_mock_server(vec![session, get_reply, set_reply]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    c.system_script_put("k2-footer", "require [\"variables\"];")
        .expect("put");
    let _sess = rx.recv().expect("session");
    let get = body_json(&rx.recv().expect("get"));
    assert_eq!(get["methodCalls"][0][0], "x:SieveSystemScript/get");
    assert_eq!(get["using"][1], "urn:stalwart:jmap");
    let set = body_json(&rx.recv().expect("set"));
    assert_eq!(set["methodCalls"][0][0], "x:SieveSystemScript/set");
    let create = &set["methodCalls"][0][1]["create"]["k2"];
    assert_eq!(create["name"], "k2-footer");
    assert_eq!(create["isActive"], true);
    assert!(
        create["contents"].as_str().unwrap().contains("require"),
        "{create}"
    );
}
