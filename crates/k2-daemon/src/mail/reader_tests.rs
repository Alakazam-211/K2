//! The mail reader (`/cli/mail/reader/*`, prd-ocmail-admin-in-k2-v1 OA1–OA9)
//! end to end through the real dispatcher, the fake mail engine and Connect
//! logins. Same fixture as the OA0 tests: one workspace with an agent's
//! hosted inbox, pat's hosted mailbox and a linked inbox; Member, Admin,
//! Owner and pat logins.
//!
//! The PRD's T1–T9 (T11, the notice to the person, was dropped by Rosson's
//! 2026-11-10 answer: log every access, notify nobody), plus the log route,
//! the OA9 writes and the method guards.

use crate::mail::fake_jmap::FakeJmap;
use crate::mail::mailbox_guard_tests::{block, json, Fx, Resp, OWNER};
use crate::mail::messages::encode_message_id;

fn rget(fx: &Fx, token: &str, route: &str, query: &[(&str, &str)]) -> Resp {
    fx.get(token, &format!("reader/{route}"), query)
}

fn rpost(fx: &Fx, token: &str, route: &str, body: serde_json::Value) -> Resp {
    fx.post(token, &format!("reader/{route}"), body)
}

fn passport(fx: &Fx) -> String {
    let sid = k2_core::session::SessionId::new();
    crate::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        crate::session_token::HookPrincipal {
            workspace_uuid: fx.ws_id.clone(),
            agent_address: "reader-agent".to_string(),
        },
        crate::session_token::CredMode::ApiKey,
        crate::session_token::Provider::Anthropic,
    )
}

/// A message the fake engine serves for `account` (newest last pushed).
#[allow(clippy::too_many_arguments)]
fn seed_full(
    fake: &FakeJmap,
    account: &str,
    id: &str,
    thread: &str,
    subject: &str,
    received: &str,
    html: Option<&str>,
    attachment: Option<(&str, &[u8])>,
) {
    let mut st = fake.state.lock().unwrap();
    let mut body_values = serde_json::json!({ "1": { "value": format!("text of {subject}") } });
    let mut html_body = serde_json::json!([]);
    if let Some(h) = html {
        body_values["2"] = serde_json::json!({ "value": h });
        html_body = serde_json::json!([{ "partId": "2", "type": "text/html" }]);
    }
    let mut attachments = serde_json::json!([]);
    if let Some((name, bytes)) = attachment {
        let blob = format!("att-{id}");
        st.blobs.insert(blob.clone(), bytes.to_vec());
        attachments = serde_json::json!([{ "blobId": blob, "name": name, "type": "text/plain", "size": bytes.len() }]);
    }
    st.emails.insert(
        id.to_string(),
        serde_json::json!({
            "id": id,
            "threadId": thread,
            "from": [{ "name": "Sender", "email": "sender@outside.example" }],
            "to": [{ "name": "", "email": "someone@inside.example" }],
            "subject": subject,
            "receivedAt": received,
            "keywords": {},
            "hasAttachment": attachment.is_some(),
            "textBody": [{ "partId": "1", "type": "text/plain" }],
            "htmlBody": html_body,
            "bodyValues": body_values,
            "attachments": attachments,
            "blobId": format!("blob-{id}"),
            "header:Message-ID:asText": format!("<{id}@outside.example>"),
            "header:References:asText": null,
            "header:Authentication-Results:asText:all": [],
        }),
    );
    st.inbox.entry(account.to_string()).or_default().push(id.to_string());
}

fn reader_rows(fx: &Fx, actor: &str, address: &str, action: &str) -> Vec<serde_json::Value> {
    fx.rows_for(actor, address, action)
}

fn set_linked_by(fx: &Fx, person: Option<&str>) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute(
        "UPDATE mail_external_inboxes SET linked_by_principal = ?1 WHERE email_address = ?2",
        rusqlite::params![person, fx.linked_addr],
    )
    .expect("the linked inbox records who linked it (0288)");
}

fn addresses(r: &Resp) -> Vec<String> {
    json(r)["inboxes"]
        .as_array()
        .unwrap_or_else(|| panic!("no inboxes array: {}", r.body))
        .iter()
        .map(|i| i["address"].as_str().expect("address").to_string())
        .collect()
}

/// T7: an agent passport gets 403 owner_only on every reader route, before
/// any engine call or log row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t7_agents_are_refused_on_every_reader_route() {
    let fx = Fx::start().await;
    block(|| {
        let pp = passport(&fx);
        let rows_before = fx.audit().len();
        let gets_before = fx.email_gets();
        for (route, q) in [
            ("inboxes", vec![]),
            ("folders", vec![("inbox", fx.agent_addr.as_str())]),
            ("messages", vec![("inbox", fx.agent_addr.as_str())]),
            ("message", vec![("id", fx.agent_tok.as_str())]),
            ("thread", vec![("id", fx.agent_tok.as_str())]),
            ("attachment", vec![("id", fx.agent_tok.as_str()), ("part", "1")]),
            ("log", vec![]),
        ] {
            let r = rget(&fx, &pp, route, &q);
            assert_eq!(r.status, 403, "{route}: {}", r.body);
            assert_eq!(json(&r)["error"]["code"], "owner_only", "{route}: {}", r.body);
            assert!(json(&r)["error"]["hint"].as_str().unwrap_or("").contains("k2 mail"), "{route}: {}", r.body);
        }
        for route in ["move", "flag", "archive", "delete", "reply", "draft", "send"] {
            let r = rpost(&fx, &pp, route, serde_json::json!({ "id": fx.agent_tok, "inbox": fx.agent_addr, "body": "x", "read": true, "folder": "Archive", "to": ["a@b.example"], "subject": "s" }));
            assert_eq!(r.status, 403, "{route}: {}", r.body);
            assert_eq!(json(&r)["error"]["code"], "owner_only", "{route}: {}", r.body);
        }
        assert_eq!(fx.audit().len(), rows_before, "no row for an agent: {:?}", fx.audit());
        assert_eq!(fx.email_gets(), gets_before, "no engine call for an agent");
        assert_eq!(fx.seen_updates("acc-agent"), 0);
    });
}

/// T1: opening a message or a thread in the reader never marks it read,
/// for anyone, the mailbox's own person included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t1_the_reader_only_peeks() {
    let fx = Fx::start().await;
    block(|| {
        for token in [OWNER, fx.admin.as_str(), fx.owner_login.as_str()] {
            for tok in [&fx.agent_tok, &fx.person_tok] {
                let r = rget(&fx, token, "message", &[("id", tok)]);
                assert_eq!(r.status, 200, "{}", r.body);
                assert_eq!(json(&r)["markedRead"], false, "{}", r.body);
                let t = rget(&fx, token, "thread", &[("id", tok)]);
                assert_eq!(t.status, 200, "{}", t.body);
            }
        }
        let own = rget(&fx, &fx.pat, "message", &[("id", &fx.person_tok)]);
        assert_eq!(own.status, 200, "pat opens their own mail: {}", own.body);
        assert_eq!(json(&own)["message"]["subject"], "pat private mail");
        assert_eq!(fx.seen_updates("acc-agent"), 0, "nobody's open marked the agent's mail read");
        assert_eq!(fx.seen_updates("acc-pat"), 0, "not even pat's own open in the reader");
    });
}

/// T2: no log, no access. With the log gone every reader read answers 503
/// and the engine is never asked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t2_no_log_means_the_reader_opens_nothing() {
    let fx = Fx::start().await;
    struct RestoreLog;
    impl Drop for RestoreLog {
        fn drop(&mut self) {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute_batch(include_str!("../../../k2-core/drizzle_sql/0284_mail_reader_audit.sql"))
                .expect("restore the access log");
        }
    }
    let _restore = RestoreLog;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute_batch("DROP TABLE mail_reader_audit;").expect("drop the access log");
    }
    block(|| {
        let before = fx.email_gets();
        for token in [OWNER, fx.admin.as_str(), fx.pat.as_str()] {
            for (route, q) in [
                ("message", vec![("id", fx.person_tok.as_str())]),
                ("messages", vec![("inbox", fx.person_addr.as_str())]),
                ("inboxes", vec![]),
            ] {
                let r = rget(&fx, token, route, &q);
                assert_eq!(r.status, 503, "{route}: {}", r.body);
                assert_eq!(json(&r)["error"]["code"], "audit_unavailable", "{route}: {}", r.body);
            }
            let w = rpost(&fx, token, "flag", serde_json::json!({ "id": fx.person_tok, "read": true }));
            assert_eq!(w.status, 503, "a write is logged first too: {}", w.body);
        }
        assert_eq!(fx.email_gets(), before, "nothing was fetched");
        assert_eq!(fx.seen_updates("acc-pat"), 0, "nothing was changed");
    });
}

/// T3: every open writes its own row (actor, role, inbox, the message id,
/// the reason), and no row carries content.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t3_every_open_is_its_own_row_with_the_reason() {
    let fx = Fx::start().await;
    block(|| {
        let a = rget(&fx, &fx.admin, "message", &[("id", &fx.person_tok), ("reason", "  checking a bounce  ")]);
        assert_eq!(a.status, 200, "{}", a.body);
        let b = rget(&fx, &fx.admin, "message", &[("id", &fx.person_tok)]);
        assert_eq!(b.status, 200, "{}", b.body);
        let rows = reader_rows(&fx, "user:oa0admin", &fx.person_addr, "reader/message");
        assert_eq!(rows.len(), 2, "one row per open: {:?}", fx.audit());
        for r in &rows {
            assert_eq!(r["role"], "admin");
            assert_eq!(r["kind"], "person");
            assert_eq!(r["outcome"], "ok");
            assert_eq!(r["status"], 200);
            assert_eq!(r["messageRef"], serde_json::json!(fx.person_tok));
            assert_eq!(r["workspace"], serde_json::json!(fx.ws_id));
        }
        let log = rget(&fx, &fx.admin, "log", &[("inbox", &fx.person_addr)]);
        assert_eq!(log.status, 200, "{}", log.body);
        let reasons: Vec<serde_json::Value> = json(&log)["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .filter(|r| r["action"] == "reader/message")
            .map(|r| r["reason"].clone())
            .collect();
        assert!(reasons.contains(&serde_json::json!("checking a bounce")), "the reason is kept, trimmed: {reasons:?}");
        let text = serde_json::to_string(&fx.audit()).unwrap();
        assert!(!text.contains("pat private mail") && !text.contains("text of") && !text.contains("body of"), "{text}");
        // Listing pages logs once per 10 minutes.
        for _ in 0..3 {
            let l = rget(&fx, &fx.admin, "messages", &[("inbox", &fx.person_addr)]);
            assert_eq!(l.status, 200, "{}", l.body);
        }
        assert_eq!(reader_rows(&fx, "user:oa0admin", &fx.person_addr, "reader/messages").len(), 1);
    });
}

/// T4: a Member sees only their own mailbox. pat lists only pat's mailbox;
/// another Member lists nothing; naming any other inbox answers the same
/// masked not_found as an address K2 has never heard of.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t4_a_member_opens_only_their_own_mailbox() {
    let fx = Fx::start().await;
    block(|| {
        let pat = rget(&fx, &fx.pat, "inboxes", &[]);
        assert_eq!(pat.status, 200, "{}", pat.body);
        assert_eq!(addresses(&pat), vec![fx.person_addr.clone()], "{}", pat.body);
        assert_eq!(json(&pat)["inboxes"][0]["own"], true);
        assert_eq!(json(&pat)["viewer"]["opensAny"], false);
        let other = rget(&fx, &fx.member, "inboxes", &[]);
        assert_eq!(other.status, 200, "{}", other.body);
        assert!(addresses(&other).is_empty(), "a Member with no mailbox sees none: {}", other.body);
        assert!(fx.rows_for("user:oa0member", &fx.agent_addr, "reader/inboxes").is_empty(), "no row per hidden inbox");

        let mut failures = Vec::new();
        for (who, token) in [("member", fx.member.as_str()), ("pat", fx.pat.as_str())] {
            for addr in [&fx.agent_addr, &fx.linked_addr] {
                for route in ["messages", "folders"] {
                    let a = rget(&fx, token, route, &[("inbox", addr)]);
                    let g = rget(&fx, token, route, &[("inbox", &fx.ghost_addr)]);
                    if a.status != 404 || fx.fold(&a.body).replace(addr.as_str(), "<ADDR>") != fx.fold(&g.body) {
                        failures.push(format!("{who} {route} {addr}: {} {} / {} {}", a.status, a.body, g.status, g.body));
                    }
                }
            }
            for tok in [&fx.agent_tok, &fx.linked_tok] {
                for route in ["message", "thread"] {
                    let a = rget(&fx, token, route, &[("id", tok)]);
                    let g = rget(&fx, token, route, &[("id", &fx.ghost_tok)]);
                    let a_body = fx.fold(&a.body).replace(fx.agent_tok.as_str(), "<ID>");
                    if a.status != 404 || a_body != fx.fold(&g.body) {
                        failures.push(format!("{who} {route}: {} {} / {} {}", a.status, a.body, g.status, g.body));
                    }
                }
            }
        }
        assert!(failures.is_empty(), "a Member reached an inbox that isn't theirs:\n{}", failures.join("\n"));
        let own = rget(&fx, &fx.pat, "messages", &[("inbox", &fx.person_addr)]);
        assert_eq!(own.status, 200, "{}", own.body);
        assert!(own.body.contains("pat private mail"), "{}", own.body);
        let refused = fx.rows_for("user:oa0member", &fx.agent_addr, "reader/messages");
        assert_eq!(refused.len(), 1, "a refused direct try is logged: {:?}", fx.audit());
        assert_eq!(refused[0]["outcome"], "refused");
    });
    let st = fx.fake.state.lock().unwrap();
    assert!(st.gets.iter().all(|g| g["accountId"] != "acc-agent"), "no Member read reached the agent's mail");
}

/// T5 (Rosson's answer 1): the Owner and Admins open anyone's mailbox, no
/// switch and no reason needed, and see every inbox on the server.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t5_owner_and_admins_open_every_inbox_without_a_switch() {
    let fx = Fx::start().await;
    block(|| {
        for (token, actor) in [(OWNER, "owner-token"), (fx.owner_login.as_str(), "user:oa0owner"), (fx.admin.as_str(), "user:oa0admin")] {
            let list = rget(&fx, token, "inboxes", &[]);
            assert_eq!(list.status, 200, "{actor}: {}", list.body);
            let got = addresses(&list);
            for want in [&fx.agent_addr, &fx.person_addr, &fx.linked_addr] {
                assert!(got.contains(want), "{actor} sees {want}: {}", list.body);
            }
            assert_eq!(json(&list)["viewer"]["opensAny"], true, "{actor}");
            let kinds: Vec<(String, String, String)> = json(&list)["inboxes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|i| (i["address"].as_str().unwrap().to_string(), i["kind"].as_str().unwrap().to_string(), i["source"].as_str().unwrap().to_string()))
                .collect();
            assert!(kinds.contains(&(fx.agent_addr.clone(), "agent".into(), "hosted".into())), "{kinds:?}");
            assert!(kinds.contains(&(fx.person_addr.clone(), "person".into(), "hosted".into())), "{kinds:?}");
            assert!(kinds.contains(&(fx.linked_addr.clone(), "person".into(), "linked".into())), "{kinds:?}");
            let r = rget(&fx, token, "message", &[("id", &fx.person_tok)]);
            assert_eq!(r.status, 200, "{actor}: {}", r.body);
            assert_eq!(json(&r)["message"]["subject"], "pat private mail");
            let rows = reader_rows(&fx, actor, &fx.person_addr, "reader/message");
            assert_eq!(rows.len(), 1, "{actor}: {:?}", fx.audit());
            assert_eq!(rows[0]["own"], 0);
            let folders = rget(&fx, token, "folders", &[("inbox", &fx.person_addr)]);
            assert_eq!(folders.status, 200, "{actor}: {}", folders.body);
            let f = json(&folders);
            assert_eq!(f["folders"][0]["name"], "Inbox", "Inbox first: {}", folders.body);
            assert_eq!(f["folders"][0]["role"], "inbox");
            assert!(f["folders"].as_array().unwrap().iter().any(|x| x["name"] == "Receipts" && x["role"].is_null()), "{}", folders.body);
        }
        // A linked inbox opens past the reader rule for an Admin: the call
        // reaches the linked backend (no stored credential in this fixture).
        let linked = rget(&fx, &fx.admin, "messages", &[("inbox", &fx.linked_addr)]);
        assert_ne!(linked.status, 404, "an Admin isn't masked out of a linked inbox: {}", linked.body);
        assert_eq!(reader_rows(&fx, "user:oa0admin", &fx.linked_addr, "reader/messages")[0]["outcome"], "ok");
    });
}

/// T6: a linked inbox is a person's mailbox. With nobody on record it is
/// the Owner's and Admins' only; once it records who linked it (0288), that
/// person sees it as their own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t6_a_linked_inbox_is_its_linkers_own() {
    let fx = Fx::start().await;
    block(|| {
        let before = rget(&fx, &fx.pat, "inboxes", &[]);
        assert!(!addresses(&before).contains(&fx.linked_addr), "nobody on record: not pat's: {}", before.body);
        set_linked_by(&fx, Some("pat"));
        let after = rget(&fx, &fx.pat, "inboxes", &[]);
        assert!(addresses(&after).contains(&fx.linked_addr), "pat linked it: {}", after.body);
        let row = json(&after)["inboxes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["address"] == serde_json::json!(fx.linked_addr))
            .cloned()
            .expect("the linked inbox");
        assert_eq!(row["own"], true);
        assert_eq!(row["person"], "pat");
        assert_eq!(row["kind"], "person");
        let other = rget(&fx, &fx.member, "inboxes", &[]);
        assert!(!addresses(&other).contains(&fx.linked_addr), "still not another Member's: {}", other.body);
        // The agent tab's rule (OA0) follows the same record.
        let tab = fx.read(&fx.pat, &fx.linked_tok);
        assert_ne!(tab.status, 404, "pat reaches their own linked inbox through the workspace too: {}", tab.body);
    });
}

/// T8: no reader answer carries a credential, username, host, account id
/// or server URL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t8_reader_answers_carry_no_secrets() {
    let fx = Fx::start().await;
    seed_full(&fx.fake, "acc-agent", "eh1", "t-eh1", "html mail", "2026-11-02T10:00:00Z", Some("<p>hi</p>"), Some(("a.txt", b"hello")));
    block(|| {
        let tok = encode_message_id(&fx.agent_addr, "eh1");
        let answers = vec![
            rget(&fx, OWNER, "inboxes", &[]),
            rget(&fx, OWNER, "folders", &[("inbox", &fx.agent_addr)]),
            rget(&fx, OWNER, "messages", &[("inbox", &fx.agent_addr)]),
            rget(&fx, OWNER, "message", &[("id", &tok)]),
            rget(&fx, OWNER, "thread", &[("id", &tok)]),
            rget(&fx, OWNER, "attachment", &[("id", &tok), ("part", "1")]),
            rget(&fx, OWNER, "log", &[]),
        ];
        const BANNED: &[&str] = &[
            "password", "token", "secret", "appPassword", "username", "accountId", "account_id",
            "stalwartAccountId", "jmapUrl", "apiUrl", "apiKey", "host", "smtpHost", "vaultRef", "refreshToken",
        ];
        fn walk(v: &serde_json::Value, path: &str, bad: &mut Vec<String>) {
            match v {
                serde_json::Value::Object(m) => {
                    for (k, val) in m {
                        if BANNED.iter().any(|b| b.eq_ignore_ascii_case(k)) {
                            bad.push(format!("{path}.{k}"));
                        }
                        walk(val, &format!("{path}.{k}"), bad);
                    }
                }
                serde_json::Value::Array(a) => {
                    for (i, val) in a.iter().enumerate() {
                        walk(val, &format!("{path}[{i}]"), bad);
                    }
                }
                _ => {}
            }
        }
        let mut bad = Vec::new();
        let engine_url = format!("127.0.0.1:{}", fx.fake.port);
        for r in &answers {
            assert_eq!(r.status, 200, "{}", r.body);
            walk(&json(r), "$", &mut bad);
            for value in ["acc-agent", "acc-pat", "k2-test-key", engine_url.as_str(), "imap.gmail.example", OWNER] {
                if r.body.contains(value) {
                    bad.push(format!("value {value} in {}", r.body));
                }
            }
        }
        assert!(bad.is_empty(), "secret-shaped fields in reader answers: {bad:?}");
        let att = json(&answers[5]);
        assert_eq!(att["filename"], "a.txt");
        assert_eq!(att["dataBase64"], "aGVsbG8=", "the attachment's bytes");
    });
}

/// T9 end to end: hostile HTML comes back cleaned, with no agent markers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t9_message_html_is_cleaned_in_the_daemon() {
    let fx = Fx::start().await;
    let hostile = r#"<p onclick="x()">Hi</p><script>alert(1)</script><img src="https://track.example/p.gif"><a href="https://site.example/">site</a><div style="background:url(http://x.example/b.png)">b</div>"#;
    seed_full(&fx.fake, "acc-agent", "ex1", "t-ex1", "hostile", "2026-11-02T10:00:00Z", Some(hostile), None);
    block(|| {
        let r = rget(&fx, OWNER, "message", &[("id", &encode_message_id(&fx.agent_addr, "ex1"))]);
        assert_eq!(r.status, 200, "{}", r.body);
        let m = &json(&r)["message"];
        let html = m["html"].as_str().expect("html");
        for banned in ["<script", "onclick", "track.example", "url(", " href="] {
            assert!(!html.contains(banned), "{banned} survived: {html}");
        }
        assert!(html.contains(r#"data-k2-link="https://site.example/""#), "{html}");
        assert_eq!(m["remoteContentBlocked"], true);
        let text = m["text"].as_str().expect("text");
        assert!(!text.contains("BEGIN EXTERNAL EMAIL"), "no agent markers for a person: {text}");
        assert!(text.contains("text of hostile"), "{text}");
    });
}

/// A thread comes back oldest first, peeked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_thread_gathers_its_messages_oldest_first() {
    let fx = Fx::start().await;
    seed_full(&fx.fake, "acc-agent", "c2", "t-conv", "second", "2026-11-03T10:00:00Z", None, None);
    seed_full(&fx.fake, "acc-agent", "c1", "t-conv", "first", "2026-11-02T10:00:00Z", None, None);
    seed_full(&fx.fake, "acc-agent", "c3", "t-conv", "third", "2026-11-04T10:00:00Z", None, None);
    block(|| {
        let r = rget(&fx, &fx.admin, "thread", &[("id", &encode_message_id(&fx.agent_addr, "c2"))]);
        assert_eq!(r.status, 200, "{}", r.body);
        let subjects: Vec<String> = json(&r)["messages"]
            .as_array()
            .expect("messages")
            .iter()
            .map(|m| m["subject"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(subjects, vec!["first", "second", "third"]);
        assert_eq!(fx.seen_updates("acc-agent"), 0);
    });
}

/// OA2: the log. The Owner and Admins read every row; a person the rows of
/// their own mailboxes; another Member none.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_log_shows_each_viewer_what_it_may_see() {
    let fx = Fx::start().await;
    block(|| {
        assert_eq!(rget(&fx, &fx.admin, "message", &[("id", &fx.person_tok)]).status, 200);
        assert_eq!(rget(&fx, OWNER, "message", &[("id", &fx.agent_tok)]).status, 200);
        let all = rget(&fx, &fx.admin, "log", &[]);
        assert_eq!(all.status, 200, "{}", all.body);
        let rows = json(&all)["rows"].as_array().cloned().expect("rows");
        assert!(rows.iter().any(|r| r["address"] == serde_json::json!(fx.agent_addr) && r["actor"] == "owner-token"), "{}", all.body);
        assert!(rows.iter().any(|r| r["address"] == serde_json::json!(fx.person_addr) && r["actor"] == "user:oa0admin"), "{}", all.body);
        let ids: Vec<i64> = rows.iter().map(|r| r["id"].as_i64().unwrap()).collect();
        let mut sorted = ids.clone();
        sorted.sort_by(|a, b| b.cmp(a));
        assert_eq!(ids, sorted, "newest first");

        let pat = rget(&fx, &fx.pat, "log", &[]);
        assert_eq!(pat.status, 200, "{}", pat.body);
        let pat_rows = json(&pat)["rows"].as_array().cloned().expect("rows");
        assert!(!pat_rows.is_empty(), "pat sees who opened their mailbox: {}", pat.body);
        assert!(pat_rows.iter().all(|r| r["address"] == serde_json::json!(fx.person_addr)), "only pat's mailbox: {}", pat.body);
        assert!(pat_rows.iter().any(|r| r["actor"] == "user:oa0admin"));

        let member = rget(&fx, &fx.member, "log", &[]);
        assert_eq!(member.status, 200, "{}", member.body);
        assert_eq!(json(&member)["count"], 0, "{}", member.body);

        let by_actor = rget(&fx, OWNER, "log", &[("actor", "oa0admin")]);
        assert!(json(&by_actor)["rows"].as_array().unwrap().iter().all(|r| r["actor"] == "user:oa0admin"), "{}", by_actor.body);
    });
}

/// OA9: a reader write runs the k2 mail verb as the inbox's own workspace,
/// so its governance applies unchanged, and it is logged like a read.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oa9_writes_follow_the_inbox_workspace_rules_and_are_logged() {
    let fx = Fx::start().await;
    block(|| {
        // flag: the only way the reader changes read state.
        let f = rpost(&fx, &fx.admin, "flag", serde_json::json!({ "id": fx.person_tok, "read": true, "reason": "tidy up" }));
        assert_eq!(f.status, 200, "{}", f.body);
        assert_eq!(fx.seen_updates("acc-pat"), 1, "the explicit flag marked it read");
        let rows = reader_rows(&fx, "user:oa0admin", &fx.person_addr, "reader/flag");
        assert_eq!(rows.len(), 1, "{:?}", fx.audit());
        assert_eq!(rows[0]["outcome"], "ok");
        assert_eq!(rows[0]["status"], 200);
        let m = rpost(&fx, OWNER, "move", serde_json::json!({ "id": fx.agent_tok, "folder": "Archive" }));
        assert_eq!(m.status, 200, "{}", m.body);
        assert_eq!(json(&m)["folder"], "Archive");

        // A Member can't act on an inbox that isn't theirs (masked), even
        // though the agent tab lets them read it.
        let w = rpost(&fx, &fx.member, "flag", serde_json::json!({ "id": fx.agent_tok, "read": true }));
        let g = rpost(&fx, &fx.member, "flag", serde_json::json!({ "id": fx.ghost_tok, "read": true }));
        assert_eq!(w.status, 404, "{}", w.body);
        assert_eq!(fx.fold(&w.body).replace(fx.agent_tok.as_str(), "<ID>"), fx.fold(&g.body));

        // send: the same answer the workspace's own send gives, gate off
        // (the default) and on.
        for mode in ["off", "on"] {
            {
                let db = k2_core::db::shared();
                let conn = db.lock();
                conn.execute("UPDATE projects SET mail_agent_send = ?1 WHERE id = ?2", rusqlite::params![mode, fx.ws_id])
                    .expect("set the workspace's send gate");
            }
            let msg = serde_json::json!({ "to": ["dest@else.example"], "subject": format!("hello {mode}"), "body": "hi" });
            let mut via_reader = msg.clone();
            via_reader["inbox"] = serde_json::json!(fx.agent_addr);
            via_reader["subject"] = serde_json::json!(format!("hello {mode} from the reader"));
            let mut direct = msg.clone();
            direct["project"] = serde_json::json!(fx.ws_path);
            direct["from"] = serde_json::json!(fx.agent_addr);
            let r = rpost(&fx, OWNER, "send", via_reader);
            let d = fx.post(OWNER, "send", direct);
            assert_eq!(r.status, d.status, "gate {mode}: reader {} / direct {}", r.body, d.body);
            assert_eq!(json(&r)["error"]["code"], json(&d)["error"]["code"], "gate {mode}: {} / {}", r.body, d.body);
            assert_eq!(reader_rows(&fx, "owner-token", &fx.agent_addr, "reader/send").len(), if mode == "off" { 1 } else { 2 });
        }
        // The inbox's level still rules: a read-level inbox can't send.
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute("UPDATE mail_addresses SET primary_level = 'read' WHERE address = ?1", rusqlite::params![fx.agent_addr])
                .expect("lower the level");
        }
        let low = rpost(&fx, OWNER, "send", serde_json::json!({ "inbox": fx.agent_addr, "to": ["d@e.example"], "subject": "s", "body": "b" }));
        assert_eq!(low.status, 403, "{}", low.body);
        assert_eq!(json(&low)["error"]["code"], "not_allowed", "{}", low.body);
        let att = rpost(&fx, OWNER, "send", serde_json::json!({ "inbox": fx.agent_addr, "to": ["d@e.example"], "subject": "s", "body": "b", "attachments": ["x.txt"] }));
        assert_eq!(att.status, 400, "{}", att.body);
    });
}

/// Writes are POST only and reads GET only; a Voice & Chat only login gets
/// the role refusal.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn methods_and_roles_are_guarded() {
    let fx = Fx::start().await;
    block(|| {
        let g = rget(&fx, OWNER, "flag", &[("id", &fx.agent_tok), ("read", "true")]);
        assert_eq!(g.status, 405, "GET on a write: {}", g.body);
        let p = rpost(&fx, OWNER, "message", serde_json::json!({ "id": fx.agent_tok }));
        assert_eq!(p.status, 405, "POST on a read: {}", p.body);
        assert_eq!(fx.seen_updates("acc-agent"), 0);
        k2_core::connect_users::add_user("oa0voice", "correct-horse-battery-staple-1").expect("add user");
        k2_core::connect_users::set_role("oa0voice", k2_core::connect_users::Role::VoiceChat).expect("set role");
        let voice = k2_core::connect_users::create_session("oa0voice");
        let v = rget(&fx, &voice, "inboxes", &[]);
        assert_eq!(v.status, 403, "{}", v.body);
        assert_eq!(json(&v)["error"], "role_required", "{}", v.body);
        let none = rget(&fx, "not-a-token", "inboxes", &[]);
        assert_eq!(none.status, 403, "{}", none.body);
    });
}
