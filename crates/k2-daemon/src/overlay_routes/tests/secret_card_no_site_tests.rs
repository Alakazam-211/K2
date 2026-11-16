//! A secret card with no site. Not every secret is for a website (a
//! computer's admin password, an ssh passphrase), and a card that K2 would
//! send to a site it doesn't know can't be saved. So: an agent that names no
//! site and no delivery gets the secret as a file it reads (`k2 vault file
//! NAME`); `--for` without `--delivery` stays fetch; an explicit fetch or
//! proxy with no site is refused when the agent asks, in one line; and a
//! card stored by an older K2 (fetch, no site) can still be saved as a file.

use super::*;
use k2_vault::SecretStore as _;

const PW: &str = "k2test-no-site-card-pw-71c3";

fn ws(prefix: &str) -> (String, String) {
    let handle = format!("{prefix}{}", &uuid::Uuid::new_v4().to_string()[..8]);
    let (project_id, _) = seed(&handle);
    pin(&project_id, &uuid::Uuid::new_v4().to_string());
    (handle, project_id)
}

fn ask(handle: &str, name: &str, extra: serde_json::Value) -> CliResponse {
    let mut body = serde_json::json!({ "addr": handle, "name": name, "from": "k2" });
    for (k, v) in extra.as_object().expect("object") {
        body[k] = v.clone();
    }
    post_json("/cli/thread/secret", &HashMap::new(), body)
}

fn card_doc(handle: &str, id: &str) -> serde_json::Value {
    let snap = json_body(&dispatch("/cli/thread", &params_of(&[("addr", handle)])).expect("GET"));
    snap["items"].as_array().expect("items").iter().find(|i| i["id"] == id).expect("card")["doc"]["secret"].clone()
}

fn save(handle: &str, id: &str, extra: serde_json::Value) -> CliResponse {
    let mut body = serde_json::json!({ "addr": handle, "id": id, "secret": PW, "disclosed": "1" });
    for (k, v) in extra.as_object().expect("object") {
        body[k] = v.clone();
    }
    post_json("/cli/thread/answer", &HashMap::new(), body)
}

fn sel(project_id: &str, name: &str) -> k2_vault::Selector {
    k2_vault::Selector::user(k2_vault::Scope::workspace(project_id.to_string()), name)
}

#[test]
fn no_site_and_no_delivery_asks_for_a_file() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlns");
    let posted = ask(&handle, "MAC_ADMIN", serde_json::json!({ "kind": "password", "prompt": "The admin password for this Mac" }));
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let body = json_body(&posted);
    assert_eq!(body["sends"], "MAC_ADMIN goes to you as the file $K2_SECRETS_DIR/MAC_ADMIN (you can read it)", "{body}");
    let id = body["id"].as_str().expect("id").to_string();
    assert_eq!(vault.request_get(&id).expect("request").proposed_delivery, k2_vault::Delivery::File);
    let doc = card_doc(&handle, &id);
    assert_eq!(doc["delivery"], "file", "{doc}");

    // The person saves it as it stands: no site needed.
    let set = save(&handle, &id, serde_json::json!({}));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert!(!set.body.contains(PW));
    let m = vault.meta(&sel(&project_id, "MAC_ADMIN"), &k2_vault::Actor::Owner).expect("meta");
    assert_eq!(m.default_delivery, k2_vault::Delivery::File);
    assert!(m.hosts.is_empty(), "{:?}", m.hosts);
}

#[test]
fn a_site_without_a_delivery_stays_fetch() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, _) = ws("ovlnsf");
    let posted = ask(&handle, "STRIPE_KEY", serde_json::json!({ "for": "api.stripe.com" }));
    assert_eq!(posted.status, "200 OK", "{}", posted.body);
    let body = json_body(&posted);
    let id = body["id"].as_str().expect("id").to_string();
    assert_eq!(vault.request_get(&id).expect("request").proposed_delivery, k2_vault::Delivery::Fetch);
    assert!(body["sends"].as_str().expect("sends").starts_with("K2 will send STRIPE_KEY to api.stripe.com"), "{body}");
}

#[test]
fn fetch_proxy_or_a_sending_with_no_site_is_refused_when_asked() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, _) = ws("ovlnsr");
    for extra in [
        serde_json::json!({ "delivery": "fetch" }),
        serde_json::json!({ "delivery": "proxy" }),
        serde_json::json!({ "inject": { "dst": "basic" } }),
    ] {
        let refused = ask(&handle, "NO_SITE", extra.clone());
        assert_error_code(&refused, "400 Bad Request", "card_needs_site");
        let hint = json_body(&refused)["error"]["hint"].as_str().expect("hint").to_string();
        assert!(hint.contains("--for HOST"), "{extra}: {hint}");
        assert!(hint.contains("--delivery file"), "{extra}: {hint}");
        assert!(!hint.contains('\n'), "one line: {hint}");
    }
    assert_eq!(vault.pending_requests_in(&resolve_addr(&handle).map_err(|r| r.body).expect("addr").conversation_id).expect("count"), 0, "no card went up");
}

#[test]
fn a_stored_fetch_card_with_no_site_saves_as_a_file() {
    let _home = crate::test_support::TempHome::new();
    let vault = crate::vault::ensure_test_vault();
    let (handle, project_id) = ws("ovlnso");
    // What an older K2 stored: fetch, and no site.
    let resolved = resolve_addr(&handle).map_err(|r| r.body).expect("addr");
    let proposal = overlay::SecretProposal {
        scope: Some("workspace".into()),
        delivery: Some("fetch".into()),
        kind: Some("password".into()),
        ..Default::default()
    };
    let item = create_secret_card(&vault, &resolved, "k2", None, "OLD_ADMIN", Some("an admin password"), proposal, None, None, true).map_err(|r| r.body).expect("card");
    assert_eq!(vault.request_get(&item.id).expect("request").proposed_delivery, k2_vault::Delivery::Fetch);

    // As it stands it can't go anywhere.
    let as_is = save(&handle, &item.id, serde_json::json!({}));
    assert_error_code(&as_is, "400 Bad Request", "hosts_required");
    assert!(!as_is.body.contains(PW));
    // The card's one click: the same Save, as a file for the agent.
    let set = save(&handle, &item.id, serde_json::json!({ "delivery": "file" }));
    assert_eq!(set.status, "200 OK", "{}", set.body);
    assert!(!set.body.contains(PW));
    let m = vault.meta(&sel(&project_id, "OLD_ADMIN"), &k2_vault::Actor::Owner).expect("meta");
    assert_eq!(m.default_delivery, k2_vault::Delivery::File);
}
