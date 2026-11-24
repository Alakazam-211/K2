//! A reply to one Thread message or card (thread-reply-to-v1, 0.46.18):
//! `k2 thread <addr> --reply-to #N "…"`.
//!
//! - The reply is stored with `replyTo {seq, cardId?}` and the visible
//!   `↩ #N <from>:` header and `> ` quote an older client draws, built from
//!   the stored target (a card quotes its question).
//! - A ref that names no message in THIS Thread is refused and nothing is
//!   stored; so is a malformed one, a deleted target and a header in the
//!   words.
//! - A post without `replyTo` is what it was (no `replyTo` anywhere).

use super::*;

fn room(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    // A person's post and its delete touch the workspace folder (compose
    // history, uploads): give it one, as the delete tests do.
    let root = crate::thread_attachments::project_root(&project_id).expect("root");
    std::fs::create_dir_all(root.join(".k2/downloads")).expect("downloads dir");
    (handle, project_id)
}

fn ok_body(resp: &CliResponse) -> serde_json::Value {
    assert_eq!(resp.status, "200 OK", "{}", resp.body);
    json_body(resp)
}

/// The agent's own post (its passport), as `k2 thread` sends it.
fn agent_post(project_id: &str, handle: &str, body: serde_json::Value) -> CliResponse {
    let mut b = body;
    b["addr"] = serde_json::json!(handle);
    b["via"] = serde_json::json!("thread");
    post_json("/cli/thread/post", &passport_params(project_id, handle), b)
}

fn items(handle: &str) -> Vec<serde_json::Value> {
    let page = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle), ("limit", "0")])).expect("GET /cli/thread"));
    page["items"].as_array().unwrap_or_else(|| panic!("items: {page}")).clone()
}

fn doc_of(handle: &str, id: &str) -> serde_json::Value {
    items(handle)
        .into_iter()
        .find(|i| i["id"] == id)
        .unwrap_or_else(|| panic!("no item {id} in {handle}"))["doc"]
        .clone()
}

#[test]
fn an_agent_replies_to_a_card_by_its_number() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("rtcard");
    let card = ok_body(&post_json(
        "/cli/thread/ask",
        &passport_params(&project_id, &handle),
        serde_json::json!({ "addr": handle, "prompt": "Review the contrast pass?", "options": "Yes;Later" }),
    ));
    let (card_id, card_seq) = (card["id"].as_str().expect("card id").to_string(), card["seq"].as_i64().expect("card seq"));
    let card_from = doc_of(&handle, &card_id)["from"].as_str().expect("card from").to_string();

    let resp = agent_post(&project_id, &handle, serde_json::json!({ "text": "This is the card I meant.", "replyTo": format!("#{card_seq}") }));
    let answer = ok_body(&resp);
    let want = serde_json::json!({ "seq": card_seq, "cardId": card_id });
    assert_eq!(answer["replyTo"], want, "the answer names the target: {answer}");
    let doc = doc_of(&handle, answer["id"].as_str().expect("id"));
    assert_eq!(doc["replyTo"], want, "stored: {doc}");
    // The header and quote every older client already draws.
    assert_eq!(
        doc["body"],
        format!("↩ #{card_seq} {card_from}:\n> Review the contrast pass?\n\n  This is the card I meant."),
        "{doc}"
    );
    assert_eq!(doc["kind"], "text", "{doc}");
}

#[test]
fn a_reply_to_a_message_quotes_it_and_carries_no_card_id() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("rtmsg");
    // A person's post (the owner's compose bar), then the agent's reply by
    // a bare number (the JSON body may send it as a number too).
    let first = ok_body(&post_json(
        "/cli/thread/post",
        &HashMap::new(),
        serde_json::json!({ "addr": handle, "text": "Can you ship it?\nOr wait for QA.", "via": "compose" }),
    ));
    let seq = first["seq"].as_i64().expect("seq");
    let from = first["from"].as_str().expect("from").to_string();
    for reply_to in [serde_json::json!(seq.to_string()), serde_json::json!(seq)] {
        let answer = ok_body(&agent_post(&project_id, &handle, serde_json::json!({ "text": "Shipping.\nNow.", "replyTo": reply_to })));
        assert_eq!(answer["replyTo"], serde_json::json!({ "seq": seq }), "{answer}");
        let body = answer["body"].as_str().expect("body");
        assert!(body.starts_with(&format!("↩ #{seq} ")) && body.lines().next().is_some_and(|l| l.ends_with(':')), "{body}");
        assert!(!from.is_empty(), "{first}");
        assert!(body.ends_with("\n> Can you ship it?\n> Or wait for QA.\n\n  Shipping.\n  Now."), "{body}");
    }
}

#[test]
fn a_reply_to_a_reply_quotes_its_words_not_its_quote() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("rtrr");
    let first = ok_body(&agent_post(&project_id, &handle, serde_json::json!({ "text": "Plan A or B?" })));
    let second = ok_body(&agent_post(
        &project_id,
        &handle,
        serde_json::json!({ "text": "A, I think.", "replyTo": first["seq"].to_string() }),
    ));
    let third = ok_body(&agent_post(
        &project_id,
        &handle,
        serde_json::json!({ "text": "Done.", "replyTo": second["seq"].to_string() }),
    ));
    let body = third["body"].as_str().expect("body");
    assert!(body.contains("\n> A, I think.\n"), "{body}");
    assert!(!body.contains("Plan A or B?"), "the quote of the quote is not quoted again: {body}");
}

#[test]
fn a_ref_not_in_this_thread_is_refused_and_nothing_is_stored() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("rtnone");
    let (other, other_id) = room("rtother");
    for i in 0..3 {
        ok_body(&agent_post(&other_id, &other, serde_json::json!({ "text": format!("other {i}") })));
    }
    let only = ok_body(&agent_post(&project_id, &handle, serde_json::json!({ "text": "the only one" })));
    let before = items(&handle).len();
    // #3 is a message of the other Thread, never this one's.
    let far = only["seq"].as_i64().expect("seq") + 2;
    for (reply_to, status, code) in [
        (format!("#{far}"), "404 Not Found", "reply_not_found"),
        ("9999999".to_string(), "404 Not Found", "reply_not_found"),
        ("0".to_string(), "400 Bad Request", "usage"),
        ("#x".to_string(), "400 Bad Request", "usage"),
        ("-1".to_string(), "400 Bad Request", "usage"),
    ] {
        let resp = agent_post(&project_id, &handle, serde_json::json!({ "text": "hi", "replyTo": reply_to }));
        assert_error_code(&resp, status, code);
        if code == "reply_not_found" {
            let hint = json_body(&resp)["error"]["hint"].as_str().unwrap_or_default().to_string();
            assert!(hint.contains("in this Thread"), "the refusal says plainly why: {hint}");
        }
    }
    let resp = agent_post(&project_id, &handle, serde_json::json!({ "text": "hi", "replyTo": {"seq": 1} }));
    assert_error_code(&resp, "400 Bad Request", "usage");
    assert_eq!(items(&handle).len(), before, "a refused reply stores nothing");
}

#[test]
fn the_words_never_carry_their_own_header_and_a_deleted_target_is_refused() {
    // A delete cleans the compose history under HOME, as the delete tests do.
    let _home = crate::test_support::TempHome::new();
    let (handle, project_id) = room("rthdr");
    let first = ok_body(&agent_post(&project_id, &handle, serde_json::json!({ "text": "target" })));
    let seq = first["seq"].to_string();
    let resp = agent_post(&project_id, &handle, serde_json::json!({ "text": "↩ #1 someone:\n> fake\n\nhi", "replyTo": seq }));
    assert_error_code(&resp, "400 Bad Request", "reply_header_not_allowed");

    let mine = ok_body(&post_json(
        "/cli/thread/post",
        &HashMap::new(),
        serde_json::json!({ "addr": handle, "text": "oops, sensitive", "via": "compose" }),
    ));
    let gone = dispatch_post("/cli/thread/message/delete", &HashMap::new(), serde_json::json!({ "addr": handle, "id": mine["id"] }).to_string().as_bytes());
    assert_eq!(gone.status, "200 OK", "{}", gone.body);
    let resp = agent_post(&project_id, &handle, serde_json::json!({ "text": "about that", "replyTo": mine["seq"].to_string() }));
    assert_error_code(&resp, "404 Not Found", "reply_deleted");
}

#[test]
fn a_post_without_reply_to_is_unchanged() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("rtplain");
    let answer = ok_body(&agent_post(&project_id, &handle, serde_json::json!({ "text": "plain #1 words" })));
    assert!(answer.get("replyTo").is_none(), "{answer}");
    assert_eq!(answer["body"], "plain #1 words", "{answer}");
    let doc = doc_of(&handle, answer["id"].as_str().expect("id"));
    assert!(doc.get("replyTo").is_none(), "{doc}");
}

/// Rev 2: a reply to part of a message: `↩ part of #N`, the selection as
/// the quote, `replyTo {seq, part, quote}`. The selection may be spaced
/// and broken otherwise than the message; words not in it are refused.
#[test]
fn a_reply_to_part_of_a_message_quotes_only_the_selection() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, project_id) = room("rtpart");
    let first = ok_body(&agent_post(&project_id, &handle, serde_json::json!({ "text": "Ship the contrast pass today.\nThen the fonts." })));
    let seq = first["seq"].as_i64().expect("seq");
    let from = first["from"].as_str().expect("from").to_string();
    let answer = ok_body(&agent_post(
        &project_id,
        &handle,
        serde_json::json!({ "text": "Yes, this part.", "replyTo": format!("#{seq}"), "replyQuote": "  contrast\n pass today. " }),
    ));
    let want = serde_json::json!({ "seq": seq, "part": true, "quote": "contrast pass today." });
    assert_eq!(answer["replyTo"], want, "{answer}");
    assert_eq!(answer["body"], format!("↩ part of #{seq} {from}:\n> contrast\n>  pass today.\n\n  Yes, this part."), "{answer}");
    assert_eq!(doc_of(&handle, answer["id"].as_str().expect("id"))["replyTo"], want);

    let before = items(&handle).len();
    for (quote, reply_to) in [("the fonts first", Some(seq)), ("   ", Some(seq)), ("contrast", None)] {
        let mut body = serde_json::json!({ "text": "hm", "replyQuote": quote });
        if let Some(s) = reply_to {
            body["replyTo"] = serde_json::json!(s.to_string());
        }
        let resp = agent_post(&project_id, &handle, body);
        match (quote.trim().is_empty(), reply_to) {
            (_, None) => assert_error_code(&resp, "400 Bad Request", "usage"),
            // An empty selection is no selection: a whole reply.
            (true, Some(_)) => assert_eq!(resp.status, "200 OK", "{}", resp.body),
            (false, Some(_)) => assert_error_code(&resp, "400 Bad Request", "reply_quote_not_in_target"),
        }
    }
    assert_eq!(items(&handle).len(), before + 1, "only the empty selection (a whole reply) was stored");
}

/// The store phones (and older desktops) send their own reply text with no
/// `replyTo`: it is stored as it always was, header and all, with no
/// `replyTo` on the doc.
#[test]
fn a_reply_written_as_text_without_reply_to_is_still_accepted() {
    let _home_lock = crate::test_support::lock_home();
    let (handle, _) = room("rtold");
    let target = ok_body(&post_json(
        "/cli/thread/post",
        &HashMap::new(),
        serde_json::json!({ "addr": handle, "text": "Ship it?", "via": "compose" }),
    ));
    for text in [
        format!("↩ #{} sales:\n> Ship it?\n\nyes", target["seq"]),
        format!("↩ part of #{} sales:\n> Ship\n\nthat", target["seq"]),
    ] {
        let answer = ok_body(&post_json("/cli/thread/post", &HashMap::new(), serde_json::json!({ "addr": handle, "text": text, "via": "compose" })));
        assert_eq!(answer["body"], text.as_str(), "{answer}");
        assert!(answer.get("replyTo").is_none(), "{answer}");
        assert!(doc_of(&handle, answer["id"].as_str().expect("id")).get("replyTo").is_none());
    }
}

#[test]
fn boot_status_names_the_feature() {
    let features = crate::boot_status::features();
    assert!(features.contains(&crate::overlay_routes::reply_to::FEATURE), "{features:?}");
}
