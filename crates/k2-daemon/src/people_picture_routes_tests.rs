//! `/cli/people/picture*` (0.46.8): set, replace and clear; who may change
//! whose (the names' rule); agents read only; bad pictures refused; the
//! `person_picture_changed` event.

use super::*;
use crate::session_events::SessionEvent;
use crate::test_support::TempHome;
use base64::Engine as _;
use k2_core::connect_users;

struct Fixture {
    _home: TempHome,
    _db: k2_core::db::TestDbGuard,
}

fn fixture() -> Fixture {
    let home = TempHome::new();
    let db = k2_core::db::scoped_for_test();
    Fixture { _home: home, _db: db }
}

fn add(name: &str, role: Role) {
    connect_users::add_user(name, "password123").expect("add_user");
    if role != Role::Member {
        connect_users::set_role(name, role).expect("set_role");
    }
}

fn person(login: &str) -> Caller {
    Caller::Person(if login == OWNER { Actor::owner_token() } else { Actor::login(login) })
}

fn get(caller: &Caller, who: Option<&str>) -> CliResponse {
    let mut params = HashMap::new();
    if let Some(w) = who {
        params.insert("person".to_string(), w.to_string());
    }
    handle(false, PICTURE_PATH, b"", caller, &params)
}

fn post(path: &str, body: Value, caller: &Caller) -> CliResponse {
    handle(true, path, body.to_string().as_bytes(), caller, &HashMap::new())
}

fn ok(resp: CliResponse) -> Value {
    assert_eq!(resp.status, "200 OK", "body={}", resp.body);
    let v: Value = serde_json::from_str(&resp.body).expect("json");
    assert_eq!(v["ok"], true, "{v}");
    v
}

fn refused(resp: CliResponse, status: &str, code: &str) {
    assert_eq!(resp.status, status, "body={}", resp.body);
    let v: Value = serde_json::from_str(&resp.body).expect("json");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(v["error"]["code"], code, "{v}");
    assert!(!v["error"]["hint"].as_str().expect("hint").is_empty(), "a refusal says why: {v}");
}

/// A `w`×`h` PNG data URL, coloured by `seed`.
fn png(w: u32, h: u32, seed: u8) -> String {
    let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, seed, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).expect("encode");
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(out.into_inner()))
}

fn picture_events(rx: &mut tokio::sync::broadcast::Receiver<SessionEvent>, who: &str) -> Vec<Option<String>> {
    let mut out = Vec::new();
    while let Ok(ev) = rx.try_recv() {
        if let SessionEvent::PersonPictureChanged { person, image_hash } = ev {
            if person == who {
                out.push(image_hash);
            }
        }
    }
    out
}

fn hash_of(caller: &Caller, who: &str) -> Value {
    ok(get(caller, Some(who)))["imageHash"].clone()
}

#[test]
fn a_person_sets_replaces_and_clears_their_own_picture_and_each_emits() {
    let _f = fixture();
    add("mo", Role::Member);
    let mo = person("mo");
    let mut rx = crate::session_events::subscribe();

    let src = png(600, 300, 1);
    let set = ok(post(PICTURE_PATH, json!({ "image": src }), &mo));
    assert_eq!(set["person"], "mo");
    let first = set["imageHash"].as_str().expect("hash").to_string();
    let url = set["imageUrl"].as_str().expect("url");
    assert!(url.starts_with("data:image/jpeg;base64,"), "K2 serves its own JPEG, not the upload: {}", &url[..40]);
    assert_ne!(url, src, "the upload's bytes are never served");
    let jpeg = base64::engine::general_purpose::STANDARD.decode(&url["data:image/jpeg;base64,".len()..]).expect("b64");
    let decoded = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).expect("a real JPEG");
    assert!(decoded.width() <= 256 && decoded.height() <= 256, "resized: {}x{}", decoded.width(), decoded.height());
    assert!(jpeg.len() <= k2_core::workspace::agent_look::PROFILE_MAX_BYTES);
    assert_eq!(picture_events(&mut rx, "mo"), vec![Some(first.clone())], "one event for the set");

    // Everyone reads the same thing: the list and the one person.
    let listed = ok(get(&person(OWNER), None));
    assert_eq!(listed["people"], json!([{ "person": "mo", "logins": ["mo"], "imageHash": first }]));
    let one = ok(get(&person(OWNER), Some("mo")));
    assert_eq!(one["imageUrl"], set["imageUrl"]);

    let replaced = ok(post(PICTURE_PATH, json!({ "image": png(64, 64, 2) }), &mo));
    let second = replaced["imageHash"].as_str().expect("hash").to_string();
    assert_ne!(second, first, "a new picture, a new hash");
    assert_eq!(hash_of(&mo, "mo"), json!(second));
    assert_eq!(picture_events(&mut rx, "mo"), vec![Some(second)]);

    let cleared = ok(post(PICTURE_CLEAR_PATH, json!({}), &mo));
    assert_eq!(cleared["imageHash"], Value::Null);
    assert_eq!(hash_of(&mo, "mo"), Value::Null);
    assert_eq!(ok(get(&mo, Some("mo")))["imageUrl"], Value::Null);
    assert_eq!(ok(get(&mo, None))["people"], json!([]));
    assert_eq!(picture_events(&mut rx, "mo"), vec![None], "the clear emits with no hash");
}

#[test]
fn a_member_cannot_change_anyone_elses_picture() {
    let _f = fixture();
    add("mo", Role::Member);
    add("ann", Role::Member);
    add("ada", Role::Admin);
    let mut rx = crate::session_events::subscribe();
    let mo = person("mo");
    for target in ["ann", "ada", OWNER] {
        refused(post(PICTURE_PATH, json!({ "person": target, "image": png(8, 8, 3) }), &mo), "403 Forbidden", "role_required");
        refused(post(PICTURE_CLEAR_PATH, json!({ "person": target }), &mo), "403 Forbidden", "role_required");
        assert_eq!(hash_of(&mo, target), Value::Null, "{target}'s picture is unchanged");
        assert!(picture_events(&mut rx, target).is_empty(), "a refusal emits nothing");
    }
    refused(post(PICTURE_PATH, json!({ "person": "nobody", "image": png(8, 8, 3) }), &mo), "404 Not Found", "unknown_person");
}

#[test]
fn an_admin_changes_members_and_admins_but_not_an_owner_and_the_owner_changes_anyone() {
    let _f = fixture();
    add("mo", Role::Member);
    add("ada", Role::Admin);
    add("eve", Role::Admin);
    add("olga", Role::Owner);
    let ada = person("ada");
    let mut rx = crate::session_events::subscribe();

    let v = ok(post(PICTURE_PATH, json!({ "person": "mo", "image": png(16, 16, 4) }), &ada));
    assert_eq!(v["person"], "mo");
    assert_eq!(picture_events(&mut rx, "mo").len(), 1);
    ok(post(PICTURE_PATH, json!({ "person": "eve", "image": png(16, 16, 5) }), &ada));
    ok(post(PICTURE_CLEAR_PATH, json!({ "person": "mo" }), &ada));
    assert_eq!(hash_of(&ada, "mo"), Value::Null);

    // Admin Access's rule: an Admin can't act on an Owner, nor on the owner.
    refused(post(PICTURE_PATH, json!({ "person": "olga", "image": png(16, 16, 6) }), &ada), "403 Forbidden", "role_required");
    refused(post(PICTURE_PATH, json!({ "person": OWNER, "image": png(16, 16, 6) }), &ada), "403 Forbidden", "role_required");

    // The owner token and an Owner-role login change anyone's.
    ok(post(PICTURE_PATH, json!({ "person": "olga", "image": png(16, 16, 7) }), &person(OWNER)));
    ok(post(PICTURE_PATH, json!({ "person": "ada", "image": png(16, 16, 8) }), &person("olga")));
    ok(post(PICTURE_PATH, json!({ "person": OWNER, "image": png(16, 16, 9) }), &person("olga")));
    assert!(hash_of(&ada, OWNER).is_string());
    ok(post(PICTURE_CLEAR_PATH, json!({}), &person(OWNER)));
    assert_eq!(hash_of(&ada, OWNER), Value::Null);
}

#[test]
fn an_agent_reads_pictures_but_every_write_is_refused_by_name() {
    let _f = fixture();
    add("mo", Role::Member);
    ok(post(PICTURE_PATH, json!({ "image": png(8, 8, 10) }), &person("mo")));
    let mut rx = crate::session_events::subscribe();
    let agent = Caller::Agent;
    assert!(ok(get(&agent, Some("mo")))["imageUrl"].as_str().expect("an agent reads it").starts_with("data:image/jpeg"));
    assert_eq!(ok(get(&agent, None))["people"].as_array().expect("people").len(), 1);
    refused(post(PICTURE_PATH, json!({ "image": png(8, 8, 11) }), &agent), "403 Forbidden", "person_only");
    refused(post(PICTURE_PATH, json!({ "person": "mo", "image": png(8, 8, 11) }), &agent), "403 Forbidden", "person_only");
    refused(post(PICTURE_CLEAR_PATH, json!({ "person": "mo" }), &agent), "403 Forbidden", "person_only");
    assert!(hash_of(&agent, "mo").is_string(), "the picture is still there");
    assert!(picture_events(&mut rx, "mo").is_empty());
}

#[test]
fn bad_pictures_are_refused_by_name_and_store_nothing() {
    let _f = fixture();
    add("mo", Role::Member);
    let mo = person("mo");
    let b64 = |b: &[u8]| base64::engine::general_purpose::STANDARD.encode(b);
    refused(post(PICTURE_PATH, json!({}), &mo), "400 Bad Request", "usage");
    refused(post(PICTURE_PATH, json!({ "image": "https://example.com/me.png" }), &mo), "400 Bad Request", "usage");
    let svg = format!("data:image/svg+xml;base64,{}", b64(b"<svg xmlns='http://www.w3.org/2000/svg'/>"));
    refused(post(PICTURE_PATH, json!({ "image": svg }), &mo), "415 Unsupported Media Type", "type_refused");
    // Declared PNG, but JPEG bytes.
    let jpeg = {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(4, 4).write_to(&mut out, image::ImageFormat::Jpeg).expect("encode");
        out.into_inner()
    };
    refused(post(PICTURE_PATH, json!({ "image": format!("data:image/png;base64,{}", b64(&jpeg)) }), &mo), "400 Bad Request", "type_mismatch");
    // A PNG header with nothing behind it.
    let broken = [&b"\x89PNG\r\n\x1a\n"[..], &[0u8; 32]].concat();
    refused(post(PICTURE_PATH, json!({ "image": format!("data:image/png;base64,{}", b64(&broken)) }), &mo), "400 Bad Request", "bad_image");
    // Over the 2 MiB upload cap.
    let big = vec![0u8; k2_core::workspace::agent_look::MAX_INPUT_IMAGE_BYTES + 1];
    refused(post(PICTURE_PATH, json!({ "image": format!("data:image/png;base64,{}", b64(&big)) }), &mo), "413 Payload Too Large", "too_large");
    // Over the 4096 px decoder side.
    refused(post(PICTURE_PATH, json!({ "image": png(4097, 1, 12) }), &mo), "400 Bad Request", "bad_image");
    assert_eq!(hash_of(&mo, "mo"), Value::Null, "nothing was stored");
}

#[test]
fn owner_linked_logins_share_the_owners_picture() {
    let _f = fixture();
    add("rosson", Role::Owner);
    ok(crate::notify_routes::handle(
        true,
        crate::notify_routes::OWNER_ACCOUNTS_PATH,
        json!({ "accounts": ["rosson"] }).to_string().as_bytes(),
        &Actor::owner_token(),
    ));
    // The linked login sets its own picture: that IS the owner's.
    let v = ok(post(PICTURE_PATH, json!({ "image": png(8, 8, 13) }), &person("rosson")));
    assert_eq!(v["person"], OWNER);
    assert_eq!(hash_of(&person(OWNER), OWNER), v["imageHash"]);
    assert_eq!(hash_of(&person(OWNER), "rosson"), v["imageHash"]);
    assert_eq!(
        ok(get(&Caller::Agent, None))["people"],
        json!([{ "person": "owner", "logins": ["owner", "rosson"], "imageHash": v["imageHash"] }])
    );
    ok(post(PICTURE_CLEAR_PATH, json!({}), &person(OWNER)));
}

#[test]
fn a_removed_login_takes_its_picture_and_the_name_added_again_starts_empty() {
    let _f = fixture();
    add("mo", Role::Member);
    ok(post(PICTURE_PATH, json!({ "image": png(8, 8, 14) }), &person("mo")));
    connect_users::remove_user("mo").expect("remove");
    refused(get(&Caller::Agent, Some("mo")), "404 Not Found", "unknown_person");
    let db = k2_core::db::shared();
    assert!(k2_core::person_picture::hashes(&db.lock()).expect("hashes").is_empty(), "the row went with the login");
    add("mo", Role::Member);
    assert_eq!(hash_of(&Caller::Agent, "mo"), Value::Null, "a new login under the old name has no picture");
}

#[test]
fn the_clear_path_is_post_only() {
    let _f = fixture();
    let r = handle(false, PICTURE_CLEAR_PATH, b"", &person(OWNER), &HashMap::new());
    assert_eq!(r.status, "405 Method Not Allowed");
    assert!(crate::routes::route_policy::get_refused(PICTURE_CLEAR_PATH, ""), "the wire answers 405 before auth");
    assert!(!crate::routes::route_policy::get_refused(PICTURE_PATH, ""), "the picture path reads with GET");
}
