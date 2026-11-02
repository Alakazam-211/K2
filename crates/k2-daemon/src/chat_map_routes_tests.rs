//! Sew for Apps A6 on the App transport, in process: per-App filtering of
//! `chat.member` / `chat.whois`, the cap and pass floors.

use std::collections::HashMap;

use k2_core::skin::{self, link as passes};
use rusqlite::params;
use serde_json::{json, Value as J};

use super::*;

struct Fx {
    _db: k2_core::db::TestDbGuard,
    _home: crate::test_support::TempHome,
    jobs: String,
    other: String,
    own_ws: String,
    pass: String,
}

fn fx(tag: &str, caps: &[&str]) -> Fx {
    let home = crate::test_support::TempHome::new();
    let db = k2_core::db::scoped_for_test();
    let own_ws = uuid::Uuid::new_v4().to_string();
    let acme = format!("acme-{tag}");
    let (jobs, other) = {
        let h = k2_core::db::shared();
        let conn = h.lock();
        for (id, handle) in [(&own_ws, format!("jobs-{tag}")), (&acme, acme.clone())] {
            conn.execute("INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)", params![id, handle, format!("/tmp/cmr-{id}")])
                .unwrap();
        }
        let j = k2_core::published_services::insert(&conn, &own_ws, "jobs", "node s.js", "/tmp", 4500, "local", "running", "cmd", "").unwrap();
        let o = k2_core::published_services::insert(&conn, &acme, "other", "node s.js", "/tmp", 4501, "local", "running", "cmd", "").unwrap();
        (j.id, o.id)
    };
    let caps: Vec<String> = caps.iter().map(|c| c.to_string()).collect();
    let pass = passes::mint_remote_pass(&jobs, "jobs", &caps, &[own_ws.clone()], 3600).unwrap().raw;
    Fx { _db: db, _home: home, jobs, other, own_ws, pass }
}

fn map(ext: &str, member: &str, kind: &str, username: &str, state: &str) {
    k2_core::tool_hook::map_member_for_tests(ext, member, kind, username, state).unwrap();
}

fn call(f: &Fx, raw: &str, path: &str, post: bool, body: J, q: &[(&str, &str)]) -> (u16, J) {
    let pass = crate::apps::gateway::resolve_pass(raw);
    let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let _ = &f.jobs;
    let r = handle(path, pass, None, post, body.to_string().as_bytes(), &params);
    let n: u16 = r.status.split_whitespace().next().unwrap().parse().unwrap();
    (n, serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("not JSON ({e}): {}", r.body)))
}

fn code(v: &J) -> &str {
    v["error"]["code"].as_str().unwrap_or_else(|| panic!("no error.code: {v}"))
}

/// 0.46.8 `chat.space`: the App's own pass, no cap. Its bound Space and its
/// live manifest's channels; a channel's id once K2 has one; never another
/// App's.
#[test]
fn chat_space_is_the_apps_own_space_and_manifest_channels() {
    let f = fx("space", &["thread:read"]);
    let (st, v) = call(&f, &f.pass, SPACE, false, json!({}), &[]);
    assert_eq!(st, 200, "no cap needed: {v}");
    assert_eq!(
        v,
        json!({ "ok": true, "app": f.jobs, "spaceId": null, "spaces": [], "manifestChannels": [] }),
        "nothing bound, no manifest: the same shape, all empty"
    );
    let (st, v) = call(&f, &f.pass, SPACE, false, json!({}), &[("app", &f.other)]);
    assert_eq!((st, code(&v)), (404, "not_found"), "another App: the same not_found: {v}");
    let (st, v) = call(&f, &f.pass, SPACE, false, json!({}), &[("app", &f.jobs)]);
    assert_eq!((st, v["app"].clone()), (200, json!(f.jobs)), "its own id is fine: {v}");

    // A guest's session pass never reads it.
    let dana = skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    skin::set_principal_password(&dana.username, Some("s3cret-horse")).unwrap();
    let guest = skin::app_login::login_device(&dana.username, &f.jobs, &skin::app_login::Device { id: "d1".into(), name: "d".into() })
        .expect("login")
        .expect("device session");
    let (st, v) = call(&f, &guest.session.raw, SPACE, false, json!({}), &[]);
    assert_eq!((st, code(&v)), (403, "app_pass_required"), "{v}");

    // A live manifest with two channels; no Space yet: known, without ids.
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        k2_core::deploy::store::insert_app(
            &conn,
            &k2_core::deploy::store::NewApp {
                app_id: "jobs",
                service_id: &f.jobs,
                seed_id: None,
                owner_workspace: &f.own_ws,
                isolation: "process",
                memory_limit_bytes: None,
                front_port: 0,
                created_by: "test",
            },
        )
        .unwrap();
        let manifest = json!({
            "schema": 1, "kind": "app", "id": "jobs", "name": "Jobs", "version": "1.0.0",
            "chat": { "channels": [
                { "slug": "desk", "name": "Desk" },
                { "slug": "site-call", "name": "Site call", "type": "voice", "private": true, "roles": ["office"] }
            ] }
        });
        k2_core::deploy::store::upsert_seed(
            &conn,
            &k2_core::deploy::store::SeedRow {
                hash: "seedhash1".into(),
                seed_id: "jobs".into(),
                version: Some("1.0.0".into()),
                host_mode: "cmd".into(),
                manifest,
                created_with: json!({}),
                report: json!({}),
                signer: "test".into(),
                origin: "test".into(),
                built_for: None,
                built_from: None,
                source_ref: None,
                data_revision: 0,
                size_bytes: 1,
                created_at: 1,
                last_used_at: 1,
            },
        )
        .unwrap();
        k2_core::deploy::store::set_live(&conn, "jobs", Some("seedhash1"), None).unwrap();
    }
    let (st, v) = call(&f, &f.pass, SPACE, false, json!({}), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["spaceId"], J::Null);
    assert_eq!(
        v["manifestChannels"],
        json!([
            { "slug": "desk", "name": "Desk", "type": "text", "private": false, "spaceId": null, "channelId": null },
            { "slug": "site-call", "name": "Site call", "type": "voice", "private": true, "spaceId": null, "channelId": null }
        ])
    );

    // Bound, and K2 has one channel's id.
    k2_core::tool_hook::app_space_bind(&f.jobs, "5101", "owner-token").expect("bind");
    skin::with_skin_conn(|c| {
        c.execute(
            "INSERT INTO tool_app_space_channels (space_id, slug, app_id, source, channel_id, name, kind, private, state, ensure_tries)
             VALUES ('5101', 'desk', ?1, 'manifest', '77', 'Desk', 'text', 0, 'ready', 0)",
            params![f.jobs],
        )
        .map_err(|e| e.to_string())
    })
    .unwrap();
    let (st, v) = call(&f, &f.pass, SPACE, false, json!({}), &[]);
    assert_eq!(st, 200, "{v}");
    assert_eq!(v["spaceId"], json!("5101"));
    assert_eq!(
        v["spaces"],
        json!([{ "spaceId": "5101", "governed": false, "channels": [
            { "slug": "desk", "channelId": "77", "name": "Desk", "type": "text", "private": false, "source": "manifest", "state": "ready" }
        ] }])
    );
    assert_eq!(v["manifestChannels"][0]["channelId"], json!("77"));
    assert_eq!(v["manifestChannels"][0]["spaceId"], json!("5101"));
    assert_eq!(v["manifestChannels"][1]["channelId"], J::Null, "not made yet");

    // Two Spaces: spaceId is null, the list says which.
    k2_core::tool_hook::app_space_bind(&f.jobs, "5102", "owner-token").expect("bind a second");
    let (_, v) = call(&f, &f.pass, SPACE, false, json!({}), &[]);
    assert_eq!(v["spaceId"], J::Null, "{v}");
    assert_eq!(v["spaces"].as_array().map(Vec::len), Some(2), "{v}");
    // The other App sees none of this.
    let other_pass = passes::mint_remote_pass(&f.other, "other", &[], &[], 3600).unwrap().raw;
    let (st, v) = call(&f, &other_pass, SPACE, false, json!({}), &[]);
    assert_eq!((st, v["spaces"].clone(), v["manifestChannels"].clone()), (200, json!([]), json!([])), "{v}");
}

#[test]
fn member_answers_only_for_this_apps_users() {
    let f = fx("member", &["thread:read", "chat:read"]);
    let dana = skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    let eli = skin::add_principal_for_app("eli", None, &f.other, None, "test").unwrap();
    let fay = skin::add_principal_for_app("fay", None, &f.jobs, None, "test").unwrap();
    map(&format!("k2:principal:{}", dana.id), "1001", "person", "dana", "active");
    map(&format!("k2:principal:{}", eli.id), "1002", "person", "eli", "active");

    let (st, v) = call(&f, &f.pass, MEMBER, false, json!({}), &[("username", "dana")]);
    assert_eq!(st, 200, "{v}");
    assert_eq!((v["memberId"].clone(), v["principalId"].clone(), v["state"].clone()), (json!("1001"), json!(dana.id), json!("active")));
    assert_eq!(v["externalId"], json!(format!("k2:principal:{}", dana.id)));
    let (st, v) = call(&f, &f.pass, MEMBER, false, json!({}), &[("principal", &dana.id)]);
    assert_eq!((st, v["memberId"].clone()), (200, json!("1001")), "by principal id: {v}");
    // Another App's user (with a member!) and nobody: the same 404.
    for q in [("username", "eli"), ("principal", eli.id.as_str()), ("username", "ghost")] {
        let (st, v) = call(&f, &f.pass, MEMBER, false, json!({}), &[q]);
        assert_eq!((st, code(&v)), (404, "not_found"), "{q:?}: {v}");
    }
    // This App's user without a member yet.
    let (st, v) = call(&f, &f.pass, MEMBER, false, json!({}), &[("username", "fay")]);
    assert_eq!((st, code(&v)), (404, "no_chat_member"), "{v}");
    assert!(!fay.id.is_empty());
    // Naming another App is the 404 too.
    let (st, v) = call(&f, &f.pass, MEMBER, false, json!({}), &[("username", "dana"), ("app", &f.other)]);
    assert_eq!((st, code(&v)), (404, "not_found"), "{v}");
}

#[test]
fn whois_masks_everyone_who_isnt_this_apps_user() {
    let f = fx("whois", &["chat:read"]);
    let dana = skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    skin::set_principal_full_name("dana", Some("Dana Doe")).unwrap();
    let eli = skin::add_principal_for_app("eli", None, &f.other, None, "test").unwrap();
    let gone = skin::add_principal_for_app("gus", None, &f.jobs, None, "test").unwrap();
    map(&format!("k2:principal:{}", dana.id), "1", "person", "dana", "active");
    map(&format!("k2:principal:{}", eli.id), "2", "person", "eli", "active");
    map(&format!("k2:workspace:{}", f.own_ws), "3", "agent", "jobs-whois", "active");
    let other_ws = uuid::Uuid::new_v4().to_string();
    map(&format!("k2:workspace:{other_ws}"), "4", "agent", "secret-agent", "active");
    map("k2:owner", "5", "person", "rosson", "active");
    map(&format!("k2:principal:{}", gone.id), "6", "person", "gus", "active");
    k2_core::tool_hook::tombstone_member_for_tests(&format!("k2:principal:{}", gone.id)).unwrap();

    let (st, v) = call(&f, &f.pass, WHOIS, true, json!({"memberIds": ["1", "2", 3, "4", "5", "6", "99"]}), &[]);
    assert_eq!(st, 200, "{v}");
    let m = v["members"].as_array().unwrap();
    assert_eq!(m.len(), 7);
    assert_eq!(m[0]["isThisAppUser"], json!(true));
    assert_eq!((m[0]["principalId"].clone(), m[0]["username"].clone(), m[0]["displayName"].clone()), (json!(dana.id), json!("dana"), json!("Dana Doe")));
    // Another App's person: masked, no principal, name or App.
    assert_eq!(m[1], json!({"memberId": "2", "kind": "person", "isThisAppUser": false, "deleted": false}), "{v}");
    assert_eq!((m[2]["kind"].clone(), m[2]["username"].clone()), (json!("agent"), json!("jobs-whois")), "the App's own agent: {v}");
    assert_eq!(m[3], json!({"memberId": "4", "kind": "agent", "isThisAppUser": false, "deleted": false}), "another agent: {v}");
    assert_eq!(m[4], json!({"memberId": "5", "kind": "person", "isThisAppUser": false, "deleted": false}), "the owner: {v}");
    assert_eq!(m[5], json!({"memberId": "6", "kind": "person", "isThisAppUser": false, "deleted": true}), "a tombstone: {v}");
    assert_eq!(m[6]["kind"], json!("unknown"));
    let s = v.to_string();
    assert!(!s.contains(&eli.id) && !s.contains("secret-agent") && !s.contains("rosson"), "no leak: {s}");

    // Limits and shapes.
    let many: Vec<String> = (0..=ac::WHOIS_MAX).map(|i| i.to_string()).collect();
    let (st, v) = call(&f, &f.pass, WHOIS, true, json!({ "memberIds": many }), &[]);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    let (st, v) = call(&f, &f.pass, WHOIS, true, json!({"memberIds": ["a b"]}), &[]);
    assert_eq!((st, code(&v)), (400, "usage"), "{v}");
    let (st, _) = call(&f, &f.pass, WHOIS, false, json!({}), &[]);
    assert_eq!(st, 405, "whois is a POST");
}

#[test]
fn only_the_apps_own_pass_with_chat_read() {
    let f = fx("floors", &["thread:read"]);
    let dana = skin::add_principal_for_app("dana", None, &f.jobs, None, "test").unwrap();
    map(&format!("k2:principal:{}", dana.id), "1", "person", "dana", "active");
    let (st, v) = call(&f, &f.pass, MEMBER, false, json!({}), &[("username", "dana")]);
    assert_eq!((st, code(&v)), (403, "cap_not_granted"), "{v}");
    skin::set_principal_password("dana", Some("s3cret-horse-battery")).unwrap();
    let guest = skin::create_app_session_token("dana", &f.jobs).unwrap().raw;
    let (st, v) = call(&f, &guest, MEMBER, false, json!({}), &[("username", "dana")]);
    assert_eq!((st, code(&v)), (403, "app_pass_required"), "a guest's own pass: {v}");
    let (st, v) = call(&f, "k2skn_unknownunknownunknownunknown", WHOIS, true, json!({"memberIds": ["1"]}), &[]);
    assert_eq!((st, code(&v)), (401, "pass_invalid"), "{v}");
}
