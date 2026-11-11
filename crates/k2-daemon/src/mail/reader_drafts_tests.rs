//! Drafted replies in the mail reader (Rosson, 2026-11-10): an agent's draft
//! reply sits at the top of its thread, plainly unsent, and the person edits,
//! sends or discards it. End to end through the real dispatcher, the fake
//! mail engine and Connect logins (the OA0/OA1 fixture: one workspace with an
//! agent's hosted inbox, pat's hosted mailbox and a linked inbox).
//!
//! What each test pins:
//! - D1: `k2 mail draft <id>` on a hosted message saves a real draft in the
//!   mailbox's Drafts folder with its thread headers, the `Re:` subject and
//!   the original sender as the recipient.
//! - D2: the reader's thread route lists the draft first with `draft: true`,
//!   who drafted it and when, and opening it marks nothing read.
//! - D3: Save replaces the draft in the mailbox and records who edited it.
//! - D4: Send runs the workspace's own send gate (refused while sending is
//!   off, the draft kept), then the draft becomes a sent message in the
//!   thread, threaded onto the original.
//! - D5: Discard removes it from the mailbox and the thread.
//! - D6: a Member can't see or act on another person's drafts (the same
//!   masked answer an unknown draft gets).
//! - D7: agents can't use the reader's draft routes.

use crate::mail::fake_jmap::FakeJmap;
use crate::mail::mailbox_guard_tests::{block, json, Fx, Resp, OWNER};

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
            agent_address: "drafts-agent".to_string(),
        },
        crate::session_token::CredMode::ApiKey,
        crate::session_token::Provider::Anthropic,
    )
}

/// The agent drafts a reply to its own inbox's message `ea1` ("agent mail",
/// from sender@outside.example, Message-ID `<ea1@outside.example>`).
fn agent_drafts(fx: &Fx, body: &str) -> String {
    let pp = passport(fx);
    let r = fx.post(&pp, "draft", serde_json::json!({ "project": fx.ws_path, "id": fx.agent_tok, "body": body }));
    assert_eq!(r.status, 200, "the agent's reply draft: {}", r.body);
    let v = json(&r);
    let id = v["draftId"].as_str().unwrap_or_else(|| panic!("no draftId in {}", r.body)).to_string();
    assert!(id.starts_with("d_"), "{id}");
    id
}

/// The Email objects the engine was asked to create (drafts and sends).
fn created(fake: &FakeJmap) -> Vec<serde_json::Value> {
    fake.state.lock().unwrap().created.clone()
}

fn destroyed(fake: &FakeJmap) -> Vec<String> {
    fake.state.lock().unwrap().destroyed.clone()
}

fn thread(fx: &Fx, token: &str) -> Vec<serde_json::Value> {
    let r = rget(fx, token, "thread", &[("id", &fx.agent_tok)]);
    assert_eq!(r.status, 200, "{}", r.body);
    json(&r)["messages"].as_array().cloned().unwrap_or_else(|| panic!("no messages: {}", r.body))
}

fn drafts_in(msgs: &[serde_json::Value]) -> Vec<serde_json::Value> {
    msgs.iter().filter(|m| m["draft"] == true).cloned().collect()
}

fn set_gate(fx: &Fx, mode: &str) {
    let db = k2_core::db::shared();
    let conn = db.lock();
    conn.execute("UPDATE projects SET mail_agent_send = ?1 WHERE id = ?2", rusqlite::params![mode, fx.ws_id])
        .expect("set the workspace's send gate");
}

/// The lines the drafting agent was told about `draft_id` (its session line).
fn told(fx: &Fx, draft_id: &str) -> Vec<String> {
    crate::mail::drafts::TOLD
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .iter()
        .filter(|(project, text)| project == &fx.ws_id && text.contains(draft_id))
        .map(|(_, text)| text.clone())
        .collect()
}

/// The agent's own view of its drafts (`k2 mail drafts`).
fn agent_view(fx: &Fx, draft_id: &str) -> serde_json::Value {
    let pp = passport(fx);
    let r = fx.get(&pp, "drafts", &[("project", &fx.ws_path)]);
    assert_eq!(r.status, 200, "k2 mail drafts: {}", r.body);
    json(&r)["drafts"]
        .as_array()
        .unwrap_or_else(|| panic!("no drafts array: {}", r.body))
        .iter()
        .find(|d| d["id"] == serde_json::json!(draft_id))
        .cloned()
        .unwrap_or_else(|| panic!("the agent doesn't see {draft_id}: {}", r.body))
}

/// D1: an agent's reply draft on a hosted inbox is a real draft in the
/// Drafts folder, threaded onto the message it answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d1_an_agent_reply_draft_carries_its_thread_headers() {
    let fx = Fx::start().await;
    block(|| {
        let id = agent_drafts(&fx, "Thanks, we'll ship Friday.");
        let made = created(&fx.fake);
        assert_eq!(made.len(), 1, "one Email/set create: {made:?}");
        let e = &made[0];
        assert_eq!(e["mailboxIds"], serde_json::json!({ "mb-drafts": true }), "in Drafts: {e}");
        assert_eq!(e["keywords"]["$draft"], true, "a draft, not a message: {e}");
        assert_eq!(e["header:In-Reply-To:asText"], "<ea1@outside.example>", "{e}");
        assert!(
            e["header:References:asText"].as_str().unwrap_or("").contains("<ea1@outside.example>"),
            "References names the original: {e}"
        );
        assert_eq!(e["subject"], "Re: agent mail", "{e}");
        assert_eq!(e["to"][0]["email"], "sender@outside.example", "back to the sender: {e}");
        assert_eq!(e["from"][0]["email"], serde_json::json!(fx.agent_addr), "{e}");
        assert_eq!(e["bodyValues"]["k2body"]["value"], "Thanks, we'll ship Friday.", "{e}");
        assert!(fx.fake.state.lock().unwrap().envelopes.is_empty(), "a draft is never submitted");
        assert_eq!(agent_view(&fx, &id)["status"], "waiting", "nobody has touched it yet");
    });
}

/// D2: the thread lists the draft first, marked unsent, with its author.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d2_the_thread_lists_the_draft_first_marked_unsent() {
    let fx = Fx::start().await;
    block(|| {
        let id = agent_drafts(&fx, "Draft body for the thread");
        for token in [OWNER, fx.admin.as_str()] {
            let msgs = thread(&fx, token);
            assert_eq!(msgs.len(), 2, "the draft and the original: {msgs:?}");
            let d = &msgs[0];
            assert_eq!(d["draft"], true, "the draft comes first: {d}");
            assert_eq!(d["draftId"], serde_json::json!(id), "{d}");
            assert_eq!(d["author"]["kind"], "agent", "{d}");
            assert!(d["author"]["name"].as_str().is_some_and(|n| !n.is_empty()), "who drafted it: {d}");
            assert!(d["editedAt"].as_i64().is_some_and(|t| t > 0), "when it was last edited: {d}");
            assert_eq!(d["editable"], true, "{d}");
            assert_eq!(d["subject"], "Re: agent mail", "{d}");
            assert_eq!(d["text"], "Draft body for the thread", "{d}");
            assert_eq!(d["to"][0]["address"], "sender@outside.example", "{d}");
            assert_eq!(msgs[1]["draft"], false, "the original is a message: {}", msgs[1]);
            assert_eq!(msgs[1]["subject"], "agent mail");
        }
        // Opening the draft is logged like any other reader open, and names it.
        let open = rget(&fx, &fx.admin, "draft-open", &[("id", &id)]);
        assert_eq!(open.status, 200, "{}", open.body);
        assert_eq!(json(&open)["draft"]["text"], "Draft body for the thread");
        let rows = fx.rows_for("user:oa0admin", &fx.agent_addr, "reader/draft-open");
        assert_eq!(rows.len(), 1, "{:?}", fx.audit());
        assert_eq!(rows[0]["messageRef"], serde_json::json!(id));
        let text = serde_json::to_string(&fx.audit()).unwrap();
        assert!(!text.contains("Draft body"), "no draft text in the log: {text}");
        assert_eq!(fx.seen_updates("acc-agent"), 0, "opening never marks anything read");
    });
}

/// D3: Save replaces the draft in the mailbox, keeps its thread headers and
/// records who edited it; the agent sees that a person edited it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d3_editing_a_draft_saves_it() {
    let fx = Fx::start().await;
    block(|| {
        let id = agent_drafts(&fx, "first words");
        let s = rpost(
            &fx,
            &fx.admin,
            "draft-save",
            serde_json::json!({
                "id": id,
                "to": ["ada@example.com"],
                "cc": ["bo@example.com"],
                "subject": "Re: agent mail (edited)",
                "body": "better words",
            }),
        );
        assert_eq!(s.status, 200, "{}", s.body);
        let made = created(&fx.fake);
        assert_eq!(made.len(), 2, "the edit is a new draft: {made:?}");
        let e = &made[1];
        assert_eq!(e["keywords"]["$draft"], true, "{e}");
        assert_eq!(e["mailboxIds"], serde_json::json!({ "mb-drafts": true }), "{e}");
        assert_eq!(e["header:In-Reply-To:asText"], "<ea1@outside.example>", "the thread headers stay: {e}");
        assert_eq!(e["to"], serde_json::json!([{ "email": "ada@example.com" }]), "{e}");
        assert_eq!(e["cc"], serde_json::json!([{ "email": "bo@example.com" }]), "{e}");
        assert_eq!(destroyed(&fx.fake).len(), 1, "the old copy is gone: {:?}", destroyed(&fx.fake));

        let open = rget(&fx, OWNER, "draft-open", &[("id", &id)]);
        assert_eq!(open.status, 200, "{}", open.body);
        let d = &json(&open)["draft"];
        assert_eq!(d["text"], "better words", "{d}");
        assert_eq!(d["subject"], "Re: agent mail (edited)", "{d}");
        assert_eq!(d["to"][0]["address"], "ada@example.com", "{d}");
        assert_eq!(d["cc"][0]["address"], "bo@example.com", "{d}");
        assert_eq!(d["author"]["kind"], "agent", "still the agent's draft: {d}");
        assert_eq!(d["editedBy"], "oa0admin", "{d}");
        let msgs = thread(&fx, OWNER);
        assert_eq!(drafts_in(&msgs).len(), 1, "one draft, not two: {msgs:?}");
        assert_eq!(msgs[0]["text"], "better words");

        let rows = fx.rows_for("user:oa0admin", &fx.agent_addr, "reader/draft-save");
        assert_eq!(rows.len(), 1, "{:?}", fx.audit());
        assert_eq!(rows[0]["status"], 200);
        let text = serde_json::to_string(&fx.audit()).unwrap();
        assert!(!text.contains("ada@example.com") && !text.contains("better words"), "no addresses or text in the log: {text}");
        let mine = agent_view(&fx, &id);
        assert_eq!(mine["status"], "edited", "{mine}");
        assert_eq!(mine["editedBy"], "oa0admin", "{mine}");
        let lines = told(&fx, &id);
        assert_eq!(lines.len(), 1, "the agent hears of the first edit once: {lines:?}");
        assert!(lines[0].contains("was edited by oa0admin") && lines[0].contains("k2 mail drafts"), "{lines:?}");
        assert!(!lines[0].contains("better words") && !lines[0].contains("ada@example.com"), "no mail text in the line: {lines:?}");
    });
}

/// D4: Send goes through the workspace's own send gate and the outbox. While
/// sending is off it is refused and the draft stays; once on, the draft
/// leaves the Drafts folder and shows in the thread as a sent message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d4_send_goes_through_governance_and_becomes_a_sent_message() {
    let fx = Fx::start().await;
    block(|| {
        let id = agent_drafts(&fx, "agent's words");
        let values = serde_json::json!({
            "id": id,
            "to": ["ada@example.com"],
            "subject": "Re: agent mail",
            "body": "Rosson's final words",
        });
        set_gate(&fx, "off");
        let off = rpost(&fx, OWNER, "draft-send", values.clone());
        let direct = fx.post(
            OWNER,
            "send",
            serde_json::json!({ "project": fx.ws_path, "from": fx.agent_addr, "to": ["ada@example.com"], "subject": "x", "body": "y" }),
        );
        assert_eq!(off.status, direct.status, "the reader's send meets the same gate: {} / {}", off.body, direct.body);
        assert_eq!(json(&off)["error"]["code"], json(&direct)["error"]["code"], "{} / {}", off.body, direct.body);
        assert!(fx.fake.state.lock().unwrap().envelopes.is_empty(), "nothing left while sending is off");
        assert_eq!(drafts_in(&thread(&fx, OWNER)).len(), 1, "the refused draft stays");
        assert!(destroyed(&fx.fake).is_empty(), "{:?}", destroyed(&fx.fake));

        set_gate(&fx, "on");
        // A hosted send also needs its domain hosted here, sending direct
        // (the send path refuses fail-closed otherwise).
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT OR IGNORE INTO mail_domains (id, domain, send_mode, status, created_at) \
                 VALUES (?1, ?2, 'direct', 'verified', 100)",
                rusqlite::params![format!("dom-d4-{}", fx.ws_id), fx.domain],
            )
            .expect("seed the sending domain");
        }
        let on = rpost(&fx, OWNER, "draft-send", values);
        assert_eq!(on.status, 200, "{}", on.body);
        let v = json(&on);
        assert_eq!(v["status"], "submitted", "{}", on.body);
        assert_eq!(v["draftStatus"], "sent", "{}", on.body);
        let st = fx.fake.state.lock().unwrap();
        assert_eq!(st.envelopes.len(), 1, "one submission: {:?}", st.envelopes);
        assert_eq!(st.envelopes[0]["rcptTo"][0]["email"], "ada@example.com", "{:?}", st.envelopes);
        let sent = st.created.last().cloned().expect("the sent message");
        drop(st);
        assert_eq!(sent["header:In-Reply-To:asText"], "<ea1@outside.example>", "threaded onto the original: {sent}");
        assert_eq!(sent["bodyValues"]["k2body"]["value"], "Rosson's final words", "{sent}");
        assert!(sent["keywords"].get("$draft").is_none(), "what left is not a draft: {sent}");
        assert_eq!(destroyed(&fx.fake).len(), 1, "the draft left the Drafts folder: {:?}", destroyed(&fx.fake));

        let msgs = thread(&fx, OWNER);
        assert!(drafts_in(&msgs).is_empty(), "no draft on top any more: {msgs:?}");
        assert!(
            msgs.iter().any(|m| m["draft"] == false && m["subject"] == "Re: agent mail" && m["text"] == "Rosson's final words"),
            "the sent reply shows in the thread: {msgs:?}"
        );
        let gone = rget(&fx, OWNER, "draft-open", &[("id", &id)]);
        assert_eq!(gone.status, 404, "a sent draft is no longer a draft: {}", gone.body);
        assert_eq!(fx.rows_for("owner-token", &fx.agent_addr, "reader/draft-send").len(), 2, "{:?}", fx.audit());
        let mine = agent_view(&fx, &id);
        assert_eq!(mine["status"], "sent", "{mine}");
        assert!(mine["outboundId"].as_str().is_some_and(|o| !o.is_empty()), "the outbox row: {mine}");
        let lines = told(&fx, &id);
        assert_eq!(lines.len(), 1, "told once, after the send that went out: {lines:?}");
        assert!(lines[0].contains("was sent by Owner"), "{lines:?}");
    });
}

/// D5: Discard removes the draft from the mailbox and the thread.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d5_discard_removes_the_draft() {
    let fx = Fx::start().await;
    block(|| {
        let id = agent_drafts(&fx, "not this one");
        let r = rpost(&fx, &fx.admin, "draft-discard", serde_json::json!({ "id": id }));
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(json(&r)["draftStatus"], "discarded", "{}", r.body);
        assert_eq!(destroyed(&fx.fake).len(), 1, "{:?}", destroyed(&fx.fake));
        assert!(drafts_in(&thread(&fx, OWNER)).is_empty(), "gone from the thread");
        assert_eq!(rget(&fx, OWNER, "draft-open", &[("id", &id)]).status, 404);
        let again = rpost(&fx, &fx.admin, "draft-discard", serde_json::json!({ "id": id }));
        assert_eq!(again.status, 404, "discarding twice: {}", again.body);
        assert!(fx.fake.state.lock().unwrap().envelopes.is_empty(), "nothing was sent");
        // One row: the second ask names a draft that no longer is one, so it
        // reaches no inbox (like an unknown message id).
        assert_eq!(fx.rows_for("user:oa0admin", &fx.agent_addr, "reader/draft-discard").len(), 1, "{:?}", fx.audit());
        let mine = agent_view(&fx, &id);
        assert_eq!(mine["status"], "discarded", "{mine}");
        assert_eq!(mine["stateBy"], "oa0admin", "{mine}");
        let lines = told(&fx, &id);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].contains("was discarded by oa0admin"), "{lines:?}");
    });
}

/// D6: a Member can't see or act on another person's drafts: every draft
/// route answers what an unknown draft gets.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d6_a_member_cant_see_another_persons_drafts() {
    let fx = Fx::start().await;
    block(|| {
        let made = rpost(&fx, &fx.pat, "draft", serde_json::json!({ "id": fx.person_tok, "body": "pat's own reply" }));
        assert_eq!(made.status, 200, "pat drafts in their own mailbox: {}", made.body);
        let id = json(&made)["draftId"].as_str().unwrap_or_else(|| panic!("no draftId: {}", made.body)).to_string();
        let own = rget(&fx, &fx.pat, "thread", &[("id", &fx.person_tok)]);
        assert_eq!(own.status, 200, "{}", own.body);
        let first = &json(&own)["messages"][0];
        assert_eq!(first["draft"], true, "{first}");
        assert_eq!(first["author"]["kind"], "person", "{first}");
        assert_eq!(first["author"]["name"], "pat", "{first}");

        let ghost = "d_00000000000000000000000000000000";
        let fold = |s: &str| s.replace(&id, "<D>").replace(ghost, "<D>");
        let mut failures = Vec::new();
        let pairs = vec![
            (rget(&fx, &fx.member, "draft-open", &[("id", &id)]), rget(&fx, &fx.member, "draft-open", &[("id", ghost)])),
            (
                rpost(&fx, &fx.member, "draft-save", serde_json::json!({ "id": id, "body": "x" })),
                rpost(&fx, &fx.member, "draft-save", serde_json::json!({ "id": ghost, "body": "x" })),
            ),
            (
                rpost(&fx, &fx.member, "draft-send", serde_json::json!({ "id": id, "to": ["ada@example.com"], "subject": "s", "body": "x" })),
                rpost(&fx, &fx.member, "draft-send", serde_json::json!({ "id": ghost, "to": ["ada@example.com"], "subject": "s", "body": "x" })),
            ),
            (
                rpost(&fx, &fx.member, "draft-discard", serde_json::json!({ "id": id })),
                rpost(&fx, &fx.member, "draft-discard", serde_json::json!({ "id": ghost })),
            ),
        ];
        for (a, g) in &pairs {
            if a.status != 404 || fold(&a.body) != fold(&g.body) {
                failures.push(format!("{} {} / {} {}", a.status, a.body, g.status, g.body));
            }
        }
        let list = rget(&fx, &fx.member, "drafts", &[("inbox", &fx.person_addr)]);
        let list_ghost = rget(&fx, &fx.member, "drafts", &[("inbox", &fx.ghost_addr)]);
        if list.status != 404 || fx.fold(&list.body) != fx.fold(&list_ghost.body) {
            failures.push(format!("drafts list: {} {} / {} {}", list.status, list.body, list_ghost.status, list_ghost.body));
        }
        assert!(failures.is_empty(), "a Member reached pat's draft:\n{}", failures.join("\n"));
        assert!(fx.fake.state.lock().unwrap().envelopes.is_empty(), "nothing was sent");
        assert!(destroyed(&fx.fake).is_empty(), "nothing was removed");
        let still = rget(&fx, &fx.pat, "draft-open", &[("id", &id)]);
        assert_eq!(still.status, 200, "pat's draft is untouched: {}", still.body);
        assert_eq!(json(&still)["draft"]["text"], "pat's own reply");
        let mine = rget(&fx, &fx.pat, "drafts", &[("inbox", &fx.person_addr)]);
        assert_eq!(mine.status, 200, "{}", mine.body);
        assert_eq!(json(&mine)["drafts"][0]["id"], serde_json::json!(id), "{}", mine.body);
    });
}

/// D7: an agent passport is refused on every reader draft route, before any
/// engine call or log row.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn d7_agents_cant_use_the_reader_draft_routes() {
    let fx = Fx::start().await;
    block(|| {
        let id = agent_drafts(&fx, "mine");
        let pp = passport(&fx);
        let rows_before = fx.audit().len();
        let made_before = created(&fx.fake).len();
        for (route, q) in [("draft-open", vec![("id", id.as_str())]), ("drafts", vec![("inbox", fx.agent_addr.as_str())])] {
            let r = rget(&fx, &pp, route, &q);
            assert_eq!(r.status, 403, "{route}: {}", r.body);
            assert_eq!(json(&r)["error"]["code"], "owner_only", "{route}: {}", r.body);
        }
        for route in ["draft-save", "draft-send", "draft-discard"] {
            let r = rpost(&fx, &pp, route, serde_json::json!({ "id": id, "to": ["ada@example.com"], "subject": "s", "body": "x" }));
            assert_eq!(r.status, 403, "{route}: {}", r.body);
            assert_eq!(json(&r)["error"]["code"], "owner_only", "{route}: {}", r.body);
        }
        assert_eq!(fx.audit().len(), rows_before, "no row for an agent");
        assert_eq!(created(&fx.fake).len(), made_before, "no engine write for an agent");
        assert!(destroyed(&fx.fake).is_empty());
        assert!(fx.fake.state.lock().unwrap().envelopes.is_empty());
    });
}
