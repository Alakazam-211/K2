use super::*;

#[test]
fn a_sender_account_has_no_credential_and_its_logins_off() {
    let v = sender_account_object("noreply", "d1");
    assert!(v.get("credentials").is_none(), "no credential at all (E3): {v}");
    assert_eq!(v["@type"], "User");
    assert_eq!(v["quotas"]["maxEmails"], 1000);
    let off = v["permissions"]["disabledPermissions"].as_object().expect("map");
    for p in ["authenticate", "authenticateWithAlias", "imapAuthenticate", "pop3Authenticate", "sieveAuthenticate", "emailSend"] {
        assert_eq!(off.get(p), Some(&serde_json::json!(true)), "{p}");
    }
    for bad in ["authenticateOauth", "smtpAuthenticate", "jmapAuthenticate", "davAuthenticate", "emailReceive"] {
        assert!(off.get(bad).is_none(), "{bad} is not a 0.16.25 name or must stay on");
    }
}

#[test]
fn the_envelope_carries_ret_hdrs_and_the_copy_is_destroyed() {
    let params = serde_json::json!({ "RET": "HDRS", "ENVID": "enl_1" });
    let body = notify_submission_request(
        "acct",
        "ident",
        "drafts",
        "noreply@brand.test",
        &["a@x.test".to_string()],
        serde_json::json!({ "subject": "s" }),
        Some(&params),
        true,
    );
    let sub = &body["methodCalls"][1][1];
    assert_eq!(sub["create"]["k2sub"]["envelope"]["mailFrom"]["email"], "noreply@brand.test");
    assert_eq!(sub["create"]["k2sub"]["envelope"]["mailFrom"]["parameters"], params);
    assert_eq!(sub["onSuccessDestroyEmail"], serde_json::json!(["#k2sub"]));
    assert_eq!(body["methodCalls"][0][1]["create"]["k2out"]["mailboxIds"]["drafts"], true);
    let plain = notify_submission_request("a", "i", "m", "f@x.test", &["a@x.test".to_string()], serde_json::json!({}), None, false);
    assert!(plain["methodCalls"][1][1]["create"]["k2sub"]["envelope"]["mailFrom"].get("parameters").is_none());
    assert!(plain["methodCalls"][1][1].get("onSuccessDestroyEmail").is_none());
}

#[test]
fn a_refused_submission_names_the_draft_to_destroy() {
    let reply = serde_json::json!({ "methodResponses": [
        ["Email/set", { "created": { "k2out": { "id": "e42" } } }, "0"],
        ["EmailSubmission/set", { "notCreated": { "k2sub": { "type": "forbiddenFrom" } } }, "1"],
    ]});
    assert!(parse_submission_created(&reply).is_err());
    assert_eq!(submitted_email_id(&reply).as_deref(), Some("e42"));
    assert!(looks_like_dsn("MAILER-DAEMON@box.test", "Failed to deliver message"));
    assert!(looks_like_dsn("x@y.test", "Undelivered Mail Returned to Sender"));
    assert!(!looks_like_dsn("person@y.test", "Re: your order"));
}
