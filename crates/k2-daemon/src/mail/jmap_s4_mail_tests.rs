use super::tests::{body_json, spawn_mock_server};
use super::*;

// ── Pure fixture parsers (no network) ───────────────────────────

/// A realistic `Email/get` summaries reply entry set: one unread
/// with attachment, one read.
fn summaries_fixture() -> serde_json::Value {
    serde_json::json!({
        "accountId": "acc-1",
        "state": "s1",
        "list": [
            {
                "id": "M1",
                "threadId": "T1",
                "from": [{ "name": "GitHub", "email": "noreply@github.com" }],
                "to": [{ "name": null, "email": "bot@acme.dev" }],
                "subject": "Verify your device",
                "receivedAt": "2026-07-08T10:15:00Z",
                "keywords": {},
                "hasAttachment": true
            },
            {
                "id": "M2",
                "threadId": "T2",
                "from": [{ "email": "news@example.com" }],
                "to": [{ "email": "bot@acme.dev" }],
                "subject": "Weekly digest",
                "receivedAt": "2026-07-07T09:00:00Z",
                "keywords": { "$seen": true },
                "hasAttachment": false
            }
        ],
        "notFound": []
    })
}

#[test]
fn summaries_parse_envelope_unread_and_attachments() {
    let list = parse_email_summaries(&summaries_fixture()).expect("parsed");
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].id, "M1");
    assert_eq!(list[0].thread_id.as_deref(), Some("T1"));
    assert_eq!(list[0].from[0].name.as_deref(), Some("GitHub"));
    assert_eq!(list[0].from[0].email, "noreply@github.com");
    assert_eq!(list[0].subject, "Verify your device");
    assert!(list[0].unread, "no $seen keyword = unread");
    assert!(list[0].has_attachment);
    assert!(!list[1].unread, "$seen = read");
    // No list at all → loud; an entry without id → loud.
    assert!(parse_email_summaries(&serde_json::json!({})).is_err());
    let bad = serde_json::json!({ "list": [{ "subject": "x" }] });
    assert!(parse_email_summaries(&bad).is_err());
}

/// A realistic full `Email/get` reply: multipart text+html with
/// bodyValues, one attachment, auth-results headers, blobId.
fn full_fixture() -> serde_json::Value {
    serde_json::json!({
        "accountId": "acc-1",
        "list": [{
            "id": "M1",
            "blobId": "B-raw",
            "threadId": "T1",
            "from": [{ "name": "GitHub", "email": "noreply@github.com" }],
            "to": [{ "email": "bot@acme.dev" }],
            "cc": [{ "name": "Ops", "email": "ops@acme.dev" }],
            "subject": "Verify your device",
            "receivedAt": "2026-07-08T10:15:00Z",
            "keywords": {},
            "hasAttachment": true,
            "textBody": [
                { "partId": "1", "type": "text/plain" },
                { "partId": "3", "type": "text/plain" }
            ],
            "htmlBody": [{ "partId": "2", "type": "text/html" }],
            "bodyValues": {
                "1": { "value": "Your code is 424242.", "isTruncated": false },
                "2": { "value": "<p>Your code is <b>424242</b>.</p>", "isTruncated": false },
                "3": { "value": "-- footer", "isTruncated": false }
            },
            "attachments": [
                { "blobId": "B1", "name": "invite.ics", "type": "text/calendar", "size": 512 },
                { "name": "no-blob.bin", "type": "application/octet-stream", "size": 9 }
            ],
            "header:Authentication-Results:asText:all": [
                " mail.acme.dev; spf=pass smtp.mailfrom=github.com; dkim=pass header.d=github.com; dmarc=pass"
            ]
        }],
        "notFound": []
    })
}

#[test]
fn full_parse_assembles_bodies_attachments_and_auth_headers() {
    let full = parse_email_full("M1", &full_fixture())
        .expect("parsed")
        .expect("present");
    assert_eq!(full.summary.id, "M1");
    assert_eq!(full.blob_id.as_deref(), Some("B-raw"));
    assert_eq!(
        full.text.as_deref(),
        Some("Your code is 424242.\n-- footer"),
        "multiple text parts concatenate in order"
    );
    assert!(full.html.as_deref().unwrap().contains("<b>424242</b>"));
    assert_eq!(full.cc[0].email, "ops@acme.dev");
    assert_eq!(full.attachments.len(), 1, "blob-less part is skipped");
    assert_eq!(full.attachments[0].blob_id, "B1");
    assert_eq!(full.attachments[0].filename.as_deref(), Some("invite.ics"));
    assert_eq!(full.attachments[0].mime, "text/calendar");
    assert_eq!(full.attachments[0].size, 512);
    assert!(full.auth_results[0].contains("spf=pass"));

    // notFound → Ok(None) (the route masks it).
    let nf = serde_json::json!({ "list": [], "notFound": ["M9"] });
    assert!(parse_email_full("M9", &nf).expect("parsed").is_none());
    // Neither list nor notFound → loud.
    assert!(parse_email_full("M9", &serde_json::json!({ "list": [] })).is_err());
}

#[test]
fn email_set_updated_parses_ok_and_rejections() {
    let ok = serde_json::json!({ "updated": { "M1": null } });
    parse_email_set_updated("M1", &ok).expect("updated");
    let rejected = serde_json::json!({
        "notUpdated": { "M1": { "type": "notFound" } },
    });
    let msg = parse_email_set_updated("M1", &rejected).expect_err("must reject");
    assert!(msg.contains("notFound"), "{msg}");
    assert!(parse_email_set_updated("M1", &serde_json::json!({})).is_err());
}

#[test]
fn download_url_discovery_and_expansion() {
    let session = serde_json::json!({
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}"
    });
    let template = parse_session_download_url("http://127.0.0.1:8180", &session)
        .expect("template");
    let url = expand_download_url(&template, "acc 1", "B/1", "réport.pdf", "application/pdf");
    assert_eq!(
        url,
        "http://127.0.0.1:8180/jmap/download/acc%201/B%2F1/r%C3%A9port.pdf?accept=application%2Fpdf"
    );
    // Missing/garbage downloadUrl → loud.
    assert!(parse_session_download_url("http://x", &serde_json::json!({})).is_err());
    assert!(
        parse_session_download_url("http://x", &serde_json::json!({ "downloadUrl": "jmap/" }))
            .is_err()
    );
}

#[test]
fn reply_context_parses_headers_and_masks_not_found() {
    let args = serde_json::json!({
        "list": [{
            "id": "M1",
            "threadId": "T1",
            "from": [{ "name": "GitHub", "email": "noreply@github.com" }],
            "subject": "Verify your device",
            "header:Message-ID:asText": " <abc@github.com> ",
            "header:References:asText": "<r1@x> <r2@x>",
            "header:Authentication-Results:asText:all": [
                "mx; spf=pass; dkim=pass; dmarc=fail"
            ],
        }]
    });
    let ctx = parse_reply_context("M1", &args)
        .expect("parses")
        .expect("found");
    assert_eq!(ctx.from[0].email, "noreply@github.com");
    assert_eq!(ctx.subject, "Verify your device");
    assert_eq!(ctx.thread_id.as_deref(), Some("T1"));
    assert_eq!(ctx.message_id.as_deref(), Some("<abc@github.com>"), "trimmed");
    assert_eq!(ctx.references.as_deref(), Some("<r1@x> <r2@x>"));
    assert_eq!(ctx.auth_results.len(), 1);

    // notFound → Ok(None); neither → loud.
    let nf = serde_json::json!({ "notFound": ["M1"], "list": [] });
    assert!(parse_reply_context("M1", &nf).expect("parses").is_none());
    assert!(parse_reply_context("M1", &serde_json::json!({ "list": [] })).is_err());
}

#[test]
fn identity_matcher_is_case_insensitive_and_none_when_absent() {
    let args = serde_json::json!({
        "list": [
            { "id": "I1", "email": "other@acme.dev" },
            { "id": "I2", "email": "Bot@ACME.dev", "name": "Bot" },
        ]
    });
    assert_eq!(parse_identity_for(&args, "bot@acme.dev").as_deref(), Some("I2"));
    assert_eq!(parse_identity_for(&args, "nobody@acme.dev"), None);
    assert_eq!(parse_identity_for(&serde_json::json!({}), "x@y.z"), None);
}

/// The session discovery plus one method answer, for the watcher's calls.
fn watch_server(answer: serde_json::Value) -> StalwartClient {
    let (port, _rx) = spawn_mock_server(vec![
        r#"{"apiUrl": "/jmap/", "accounts": {"b": {}}, "primaryAccounts": {"urn:stalwart:jmap": "b"}}"#.to_string(),
        serde_json::json!({ "methodResponses": [answer], "sessionState": "s" }).to_string(),
    ]);
    StalwartClient::new(format!("http://127.0.0.1:{port}"), "k")
}

#[test]
fn email_changes_returns_the_created_ids_and_the_new_state() {
    let c = watch_server(serde_json::json!(["Email/changes", { "newState": "s2", "created": ["M1", "M2"], "updated": ["U"], "destroyed": [] }, "0"]));
    let ch = c.email_changes("acct", "s1", 256).expect("changes");
    assert_eq!(ch.new_state, "s2");
    assert_eq!(ch.created, vec!["M1".to_string(), "M2".to_string()]);
}

#[test]
fn a_state_too_old_to_change_from_says_so_for_the_fallback() {
    let c = watch_server(serde_json::json!(["error", { "type": "cannotCalculateChanges", "description": "too old" }, "0"]));
    let e = c.email_changes("acct", "s1", 256).expect_err("no changes");
    assert!(e.contains("cannotCalculateChanges"), "{e}");
}

#[test]
fn the_watcher_get_keeps_only_inbox_mail() {
    let c = watch_server(serde_json::json!(["Email/get", { "list": [
        { "id": "M1", "from": [{ "email": "a@x.test" }], "subject": "in", "receivedAt": "2026-06-10T10:00:00Z",
          "mailboxIds": { "INBOX": true }, "header:Message-ID:asText": " <m1@x> ",
          "header:Auto-Submitted:asText": "auto-generated",
          "header:Authentication-Results:asText:all": ["mx; dmarc=pass"] },
        { "id": "M2", "from": [{ "email": "b@x.test" }], "subject": "junk", "receivedAt": "2026-06-10T10:00:00Z",
          "mailboxIds": { "JUNK": true } },
    ] }, "0"]));
    let got = c.email_get_watch("acct", &["M1".to_string(), "M2".to_string()], "INBOX").expect("get");
    assert_eq!(got.len(), 1, "junk is not news");
    assert_eq!(got[0].rfc_message_id, "<m1@x>");
    assert!(got[0].auto_submitted);
    assert_eq!(got[0].auth_lines, vec!["mx; dmarc=pass".to_string()]);
}

#[test]
fn a_one_time_link_submission_destroys_its_stored_copy() {
    let rcpts = vec!["ada@example.test".to_string()];
    let keep = submission_request("acc", "id1", "mb1", "bot@x.test", &rcpts, serde_json::json!({ "subject": "s" }), false);
    assert!(keep["methodCalls"][1][1].get("onSuccessDestroyEmail").is_none(), "{keep}");
    let gone = submission_request("acc", "id1", "mb1", "bot@x.test", &rcpts, serde_json::json!({ "subject": "s" }), true);
    assert_eq!(gone["methodCalls"][1][1]["onSuccessDestroyEmail"], serde_json::json!(["#k2sub"]), "{gone}");
    assert_eq!(gone["methodCalls"][1][1]["create"]["k2sub"]["emailId"], "#k2out", "{gone}");
    assert_eq!(gone["methodCalls"][0][1]["create"]["k2out"]["mailboxIds"], serde_json::json!({ "mb1": true }));
}

#[test]
fn submission_reply_requires_both_creates() {
    // Happy path: both created (the REAL live reply carried ids +
    // sendAt on the submission).
    let ok = serde_json::json!({
        "methodResponses": [
            ["Email/set", { "created": { "k2out": { "id": "eaaaaab", "threadId": "b" } } }, "0"],
            ["EmailSubmission/set", { "created": { "k2sub": {
                "id": "b", "sendAt": "2026-07-10T03:51:02Z", "undoStatus": "final"
            } } }, "1"],
        ]
    });
    parse_submission_created(&ok).expect("both created");

    // Email/set notCreated surfaces the server's SetError.
    let rejected = serde_json::json!({
        "methodResponses": [
            ["Email/set", { "notCreated": { "k2out": {
                "type": "invalidProperties", "description": "bad body"
            } } }, "0"],
        ]
    });
    let err = parse_submission_created(&rejected).expect_err("must reject");
    assert!(err.contains("invalidProperties"), "{err}");

    // EmailSubmission/set notCreated (e.g. forbiddenFrom) is loud.
    let sub_rejected = serde_json::json!({
        "methodResponses": [
            ["Email/set", { "created": { "k2out": { "id": "M9" } } }, "0"],
            ["EmailSubmission/set", { "notCreated": { "k2sub": {
                "type": "forbiddenFrom"
            } } }, "1"],
        ]
    });
    let err = parse_submission_created(&sub_rejected).expect_err("must reject");
    assert!(err.contains("forbiddenFrom"), "{err}");

    // A method-level JMAP error is loud; a half-answered reply
    // (Email/set only) is loud too — a message created but never
    // submitted must not read as success.
    let jmap_err = serde_json::json!({
        "methodResponses": [["error", { "type": "unknownMethod" }, "0"]]
    });
    assert!(parse_submission_created(&jmap_err).is_err());
    let half = serde_json::json!({
        "methodResponses": [
            ["Email/set", { "created": { "k2out": { "id": "M9" } } }, "0"],
        ]
    });
    let err = parse_submission_created(&half).expect_err("must reject");
    assert!(err.contains("EmailSubmission/set"), "{err}");
    assert!(parse_submission_created(&serde_json::json!({})).is_err());
}

// ── The ONE loopback mock round-trip for the S4 read path ────────
// (127.0.0.1 only, house rule — locks the wire shape end to end:
// discovery (cached once per client) → mail envelope + accountId
// injection → query → get → mark-seen → blob download.)

#[test]
fn mail_read_round_trip_against_loopback_mock() {
    let session = r#"{"apiUrl": "/jmap/", "accounts": {"d": {}}}"#.to_string();
    let query_reply = serde_json::json!({
        "methodResponses": [["Email/query", { "ids": ["M1"] }, "0"]],
    })
    .to_string();
    let get_reply = serde_json::json!({
        "methodResponses": [["Email/get", summaries_fixture(), "0"]],
    })
    .to_string();
    let seen_reply = serde_json::json!({
        "methodResponses": [["Email/set", { "updated": { "M1": null } }, "0"]],
    })
    .to_string();
    // ONE session fetch (cached on the client), then three calls.
    let (port, rx) = spawn_mock_server(vec![
        session,
        query_reply,
        get_reply,
        seen_reply,
    ]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");

    let ids = c
        .email_query_ids("acc-7", serde_json::json!({ "inMailbox": "IB" }), 20, 0)
        .expect("query");
    assert_eq!(ids, vec!["M1".to_string()]);
    let summaries = c.email_get_summaries("acc-7", &ids).expect("get");
    assert_eq!(summaries.len(), 2);
    c.email_mark_seen("acc-7", "M1").expect("seen");

    // Request 1: discovery (the NEW /jmap/session path). Request 2:
    // the Email/query envelope — mail capability + the INJECTED
    // accountId (the delegated-read seam, live-verified).
    let disc = rx.recv().expect("req1");
    assert!(disc.starts_with("GET /jmap/session"), "{disc}");
    let q = rx.recv().expect("req2");
    assert!(q.starts_with("POST /jmap/"), "{q}");
    let v = body_json(&q);
    assert_eq!(v["using"][1], "urn:ietf:params:jmap:mail");
    assert_eq!(v["methodCalls"][0][0], "Email/query");
    assert_eq!(v["methodCalls"][0][1]["accountId"], "acc-7");
    assert_eq!(v["methodCalls"][0][1]["filter"]["inMailbox"], "IB");
    assert_eq!(
        v["methodCalls"][0][1]["sort"][0]["property"], "receivedAt");
    assert_eq!(v["methodCalls"][0][1]["sort"][0]["isAscending"], false);
    assert_eq!(v["methodCalls"][0][1]["limit"], 20);

    let g = rx.recv().expect("req3");
    let v = body_json(&g);
    assert_eq!(v["methodCalls"][0][0], "Email/get");
    assert_eq!(v["methodCalls"][0][1]["accountId"], "acc-7");
    let props = v["methodCalls"][0][1]["properties"].as_array().unwrap();
    assert!(
        !props.iter().any(|p| p == "bodyValues"),
        "summaries must never fetch bodies"
    );

    let s = rx.recv().expect("req4");
    let v = body_json(&s);
    assert_eq!(v["methodCalls"][0][0], "Email/set");
    assert_eq!(v["methodCalls"][0][1]["update"]["M1"]["keywords/$seen"], true);
}

#[test]
fn blob_download_uses_discovered_template_with_auth() {
    let session = serde_json::json!({
        "apiUrl": "/jmap/",
        "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
    })
    .to_string();
    // Reply 2 is served as JSON content-type but blob_download only
    // reads bytes — fine for the wire assertion.
    let (port, rx) = spawn_mock_server(vec![session, "PDFBYTES".to_string()]);
    let c = StalwartClient::new(format!("http://127.0.0.1:{port}"), "k2-test-key");
    let bytes = c
        .blob_download("acc-7", "B1", "report.pdf", "application/pdf")
        .expect("blob");
    assert_eq!(bytes, b"PDFBYTES");
    let disc = rx.recv().expect("req1");
    assert!(disc.starts_with("GET /jmap/session"), "{disc}");
    let dl = rx.recv().expect("req2");
    assert!(
        dl.starts_with("GET /jmap/download/acc-7/B1/report.pdf?accept=application%2Fpdf"),
        "{dl}"
    );
    assert!(
        dl.contains("authorization: Bearer k2-test-key")
            || dl.contains("Authorization: Bearer k2-test-key"),
        "{dl}"
    );
}
