//! `--reply-all`, `--cc` and `--bcc` on a hosted inbox's reply draft and
//! reply, end to end through the real dispatcher and the fake mail engine
//! (the OA0 fixture: the agent's hosted inbox `bot@<domain>`).
//!
//! What each test pins:
//! - R1: a reply-all draft of a message with three Cc's carries them, adds
//!   the original's To, drops the inbox's own address and its alias from
//!   To and Cc (any case), collapses repeats, adds `--cc` / `--bcc`, and
//!   keeps the thread headers.
//! - R2: a plain reply draft with `--cc` adds just that copy.
//! - R3: a person's edit of that draft in the mail reader keeps its blind
//!   copies.
//! - R4: `k2 mail reply --reply-all --cc --bcc` (send level) builds the
//!   same recipients, threaded, under the workspace's own send gate.

use crate::mail::fake_jmap::FakeJmap;
use crate::mail::mailbox_guard_tests::{block, json, Fx, OWNER};
use crate::mail::send::OutboundStore as _;

fn passport(fx: &Fx) -> String {
    let sid = k2_core::session::SessionId::new();
    crate::session_token::mint_session_token(
        &sid,
        &sid.to_string(),
        crate::session_token::HookPrincipal {
            workspace_uuid: fx.ws_id.clone(),
            agent_address: "reply-all-agent".to_string(),
        },
        crate::session_token::CredMode::ApiKey,
        crate::session_token::Provider::Anthropic,
    )
}

/// A message to the agent's inbox from three people's thread: To = the
/// inbox (in capitals) + ada; Cc = bo, cy, di, the inbox itself, its alias
/// and ada again. The inbox's account has the alias `hello`.
fn seed_multi_party(fx: &Fx, id: &str) -> String {
    let mut st = fx.fake.state.lock().unwrap();
    let hello = format!("Hello@{}", fx.domain);
    st.aliases.insert("acc-agent".to_string(), vec!["hello".to_string()]);
    st.emails.insert(
        id.to_string(),
        serde_json::json!({
            "id": id,
            "threadId": format!("t-{id}"),
            "from": [{ "name": "Sender", "email": "sender@outside.example" }],
            "to": [
                { "name": "", "email": fx.agent_addr.to_ascii_uppercase() },
                { "name": "Ada", "email": "ada@example.com" },
            ],
            "cc": [
                { "name": "", "email": "bo@example.com" },
                { "name": "", "email": "cy@example.com" },
                { "name": "", "email": fx.agent_addr },
                { "name": "", "email": "di@example.com" },
                { "name": "", "email": hello },
                { "name": "", "email": "ADA@example.com" },
            ],
            "subject": "Plans",
            "receivedAt": "2026-11-01T10:00:00Z",
            "keywords": {},
            "hasAttachment": false,
            "textBody": [{ "partId": "1", "type": "text/plain" }],
            "htmlBody": [],
            "bodyValues": { "1": { "value": "What do you all think?" } },
            "attachments": [],
            "blobId": format!("blob-{id}"),
            "header:Message-ID:asText": format!("<{id}@outside.example>"),
            "header:References:asText": "<root@outside.example>",
            "header:Authentication-Results:asText:all": [],
        }),
    );
    st.inbox.entry("acc-agent".to_string()).or_default().push(id.to_string());
    crate::mail::messages::encode_message_id(&fx.agent_addr, id)
}

fn created(fake: &FakeJmap) -> Vec<serde_json::Value> {
    fake.state.lock().unwrap().created.clone()
}

fn emails(v: &serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Null => Vec::new(),
        serde_json::Value::Array(a) => a
            .iter()
            .map(|e| e["email"].as_str().unwrap_or_else(|| panic!("no email in {e}")).to_string())
            .collect(),
        other => panic!("not an address list: {other}"),
    }
}

fn set_gate(fx: &Fx, mode: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("UPDATE projects SET mail_agent_send = ?1 WHERE id = ?2", rusqlite::params![mode, fx.ws_id])
        .expect("set the workspace's send gate");
    conn.execute(
        "INSERT OR IGNORE INTO mail_domains (id, domain, send_mode, status, created_at) \
         VALUES (?1, ?2, 'direct', 'verified', 100)",
        rusqlite::params![format!("dom-ra-{}", fx.ws_id), fx.domain],
    )
    .expect("seed the sending domain");
}

/// R1 + R3.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r1_a_reply_all_draft_carries_the_ccs_without_our_own_addresses() {
    let fx = Fx::start().await;
    block(|| {
        let token = seed_multi_party(&fx, "em3");
        let pp = passport(&fx);
        let r = fx.post(
            &pp,
            "draft",
            serde_json::json!({
                "project": fx.ws_path, "id": token, "body": "Count me in.", "replyAll": true,
                "cc": ["eve@example.com", "Bo@Example.com"], "bcc": ["boss@example.com"],
            }),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let draft_id = json(&r)["draftId"].as_str().unwrap_or_else(|| panic!("no draftId: {}", r.body)).to_string();
        let made = created(&fx.fake);
        assert_eq!(made.len(), 1, "one draft: {made:?}");
        let e = &made[0];
        assert_eq!(emails(&e["to"]), ["sender@outside.example", "ada@example.com"], "{e}");
        assert_eq!(
            emails(&e["cc"]),
            ["bo@example.com", "cy@example.com", "di@example.com", "eve@example.com"],
            "the three Cc's, then --cc; no repeats: {e}"
        );
        assert_eq!(emails(&e["bcc"]), ["boss@example.com"], "{e}");
        let all = [emails(&e["to"]), emails(&e["cc"]), emails(&e["bcc"])].concat();
        let hello = format!("hello@{}", fx.domain);
        assert!(!all.iter().any(|a| a.eq_ignore_ascii_case(&fx.agent_addr) || a.eq_ignore_ascii_case(&hello)), "never to ourselves: {all:?}");
        assert_eq!(e["header:In-Reply-To:asText"], "<em3@outside.example>", "{e}");
        assert_eq!(e["header:References:asText"], "<root@outside.example> <em3@outside.example>", "{e}");
        assert_eq!(e["subject"], "Re: Plans", "{e}");
        assert_eq!(e["keywords"]["$draft"], true, "a draft, never sent: {e}");
        assert!(fx.fake.state.lock().unwrap().envelopes.is_empty(), "a draft is never submitted");

        // R3: the person edits only the text; the blind copy stays.
        let s = fx.post(OWNER, "reader/draft-save", serde_json::json!({ "id": draft_id, "body": "Count us in." }));
        assert_eq!(s.status, 200, "{}", s.body);
        let made = created(&fx.fake);
        assert_eq!(made.len(), 2, "the edit is a new draft: {made:?}");
        assert_eq!(emails(&made[1]["bcc"]), ["boss@example.com"], "the edit keeps the blind copy: {}", made[1]);
        assert_eq!(emails(&made[1]["cc"]), emails(&made[0]["cc"]), "{}", made[1]);
        assert_eq!(made[1]["header:In-Reply-To:asText"], "<em3@outside.example>", "{}", made[1]);
    });
}

/// R2.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r2_cc_on_a_plain_reply_draft_adds_just_that_copy() {
    let fx = Fx::start().await;
    block(|| {
        let token = seed_multi_party(&fx, "em4");
        let pp = passport(&fx);
        let r = fx.post(
            &pp,
            "draft",
            serde_json::json!({
                "project": fx.ws_path, "id": token, "body": "Just you two.",
                "cc": ["a@example.com", "b@example.com"],
            }),
        );
        assert_eq!(r.status, 200, "{}", r.body);
        let e = &created(&fx.fake)[0];
        assert_eq!(emails(&e["to"]), ["sender@outside.example"], "no reply-all: the sender only: {e}");
        assert_eq!(emails(&e["cc"]), ["a@example.com", "b@example.com"], "{e}");
        assert!(e.get("bcc").is_none(), "{e}");
        assert_eq!(e["header:In-Reply-To:asText"], "<em4@outside.example>", "{e}");

        let bad = fx.post(&pp, "draft", serde_json::json!({ "project": fx.ws_path, "id": token, "body": "x", "cc": ["not-an-address"] }));
        assert_eq!(bad.status, 400, "{}", bad.body);
        assert_eq!(json(&bad)["error"]["code"], "usage", "{}", bad.body);
        assert_eq!(created(&fx.fake).len(), 1, "a refused copy saves nothing");
    });
}

/// R4.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn r4_reply_all_on_a_send_level_inbox_builds_the_same_recipients_under_its_gate() {
    let fx = Fx::start().await;
    block(|| {
        let token = seed_multi_party(&fx, "em5");
        let pp = passport(&fx);
        let ask = serde_json::json!({
            "project": fx.ws_path, "id": token, "body": "All of you, thanks.", "replyAll": true,
            "cc": "eve@example.com", "bcc": ["boss@example.com"],
        });
        // Sending off: refused like any reply, nothing stored or sent.
        set_gate(&fx, "off");
        let off = fx.post(&pp, "reply", ask.clone());
        assert_eq!(off.status, 403, "{}", off.body);
        assert_eq!(json(&off)["error"]["code"], "gated", "{}", off.body);

        // Approval: held for the person, with every recipient on it.
        set_gate(&fx, "approval");
        let held = fx.post(&pp, "reply", ask);
        assert_eq!(held.status, 200, "{}", held.body);
        let v = json(&held);
        assert_eq!(v["status"], "pending_approval", "{}", held.body);
        let id = v["id"].as_str().unwrap_or_else(|| panic!("no id: {}", held.body));
        let stored = crate::mail::send::DbOutboundStore::default().load_message(id).expect("stored reply");
        assert_eq!(stored.from, fx.agent_addr, "From stays the receiving address");
        assert_eq!(stored.to, ["sender@outside.example", "ada@example.com"]);
        assert_eq!(stored.cc, ["bo@example.com", "cy@example.com", "di@example.com", "eve@example.com"]);
        assert_eq!(stored.bcc, ["boss@example.com"]);
        assert_eq!(stored.in_reply_to.as_deref(), Some("<em5@outside.example>"));
        assert_eq!(stored.references.as_deref(), Some("<root@outside.example> <em5@outside.example>"));
        assert_eq!(stored.subject, "Re: Plans");
        assert!(fx.fake.state.lock().unwrap().envelopes.is_empty(), "nothing leaves before the person approves");
    });
}
