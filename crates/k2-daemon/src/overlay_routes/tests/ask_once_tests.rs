//! Idempotent sends (0.46.15, `crate::send_once`): `k2 thread ask` carries
//! one `idempotency_key` per invocation, and a CLI that timed out asks
//! again with it. Through the real handler (`dispatch_post`, the TCP and
//! cell door): a repeat answers the first card and posts none; a new key
//! posts a new card; another agent's same key is its own card.

use super::*;

fn ws(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    (handle, project_id)
}

fn ask(params: &HashMap<String, String>, handle: &str, prompt: &str, key: &str) -> CliResponse {
    post_json(
        "/cli/thread/ask",
        params,
        serde_json::json!({ "addr": handle, "prompt": prompt, "options": "Ship;Wait", "from": handle, "idempotency_key": key }),
    )
}

/// The cards on `handle`'s Thread with this prompt.
fn cards(handle: &str, prompt: &str) -> Vec<serde_json::Value> {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle), ("limit", "0")])).expect("GET /cli/thread"));
    snap["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|i| i["doc"]["kind"] == "choice" && i["doc"]["choice"]["prompt"] == prompt)
        .cloned()
        .collect()
}

#[test]
fn a_repeat_key_answers_the_first_card_and_posts_no_second() {
    let (handle, project_id) = ws("askonce");
    let agent = passport_params(&project_id, &handle);
    let key = uuid::Uuid::new_v4().to_string();
    let prompt = format!("Ship it once? {}", uuid::Uuid::new_v4());

    let first = ask(&agent, &handle, &prompt, &key);
    assert_eq!(first.status, "200 OK", "{}", first.body);
    let again = ask(&agent, &handle, &prompt, &key);
    assert_eq!(again.status, "200 OK", "{}", again.body);
    let (first, again) = (json_body(&first), json_body(&again));
    let id = first["id"].as_str().unwrap_or_else(|| panic!("no card id: {first}"));
    assert_eq!(again["id"], id, "the repeat names the first card: {again}");
    assert_eq!(again["seq"], first["seq"], "{again}");
    assert_eq!(again["replayed"], true, "{again}");
    assert_eq!(cards(&handle, &prompt).len(), 1, "one card on the Thread");
}

#[test]
fn a_new_key_posts_a_new_card() {
    let (handle, project_id) = ws("asktwo");
    let agent = passport_params(&project_id, &handle);
    let prompt = format!("Ship it twice? {}", uuid::Uuid::new_v4());
    let a = ask(&agent, &handle, &prompt, &uuid::Uuid::new_v4().to_string());
    let b = ask(&agent, &handle, &prompt, &uuid::Uuid::new_v4().to_string());
    assert_eq!(a.status, "200 OK", "{}", a.body);
    assert_eq!(b.status, "200 OK", "{}", b.body);
    assert_ne!(json_body(&a)["id"], json_body(&b)["id"]);
    assert!(json_body(&b).get("replayed").is_none(), "{}", b.body);
    assert_eq!(cards(&handle, &prompt).len(), 2);
}

#[test]
fn another_agents_same_key_is_its_own_card() {
    let (mine, my_id) = ws("askmine");
    let (theirs, their_id) = ws("asktheirs");
    let key = uuid::Uuid::new_v4().to_string();
    let prompt = format!("Ship whose? {}", uuid::Uuid::new_v4());
    let a = ask(&passport_params(&my_id, &mine), &mine, &prompt, &key);
    let b = ask(&passport_params(&their_id, &theirs), &theirs, &prompt, &key);
    assert_eq!(a.status, "200 OK", "{}", a.body);
    assert_eq!(b.status, "200 OK", "{}", b.body);
    assert!(json_body(&b).get("replayed").is_none(), "another sender's key must not replay: {}", b.body);
    assert_eq!(cards(&mine, &prompt).len(), 1);
    assert_eq!(cards(&theirs, &prompt).len(), 1, "the second agent's card was posted");
}

#[test]
fn a_malformed_key_is_refused_and_posts_nothing() {
    let (handle, project_id) = ws("askbad");
    let prompt = format!("Ship bad? {}", uuid::Uuid::new_v4());
    let r = ask(&passport_params(&project_id, &handle), &handle, &prompt, "bad key!");
    assert_error_code(&r, "400 Bad Request", "usage");
    assert!(cards(&handle, &prompt).is_empty());
}
