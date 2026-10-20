//! K15 tests: the vendored crates match their manifest; the pure mappings
//! (errors, messages, channels, permission bits, Sew-Actor, slugs,
//! addresses); and the real backend over the fake Sew mgmt socket
//! (`tests/support/fake_sew.rs`): act-as headers on a post and a resolve,
//! `Sew-Actor` + `Idempotency-Key` on an approved post, and SW11
//! spaces/create with the agent as manager. Never a real Sew.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use super::*;

#[path = "../../../tests/support/fake_sew.rs"]
#[allow(clippy::all)]
mod fake_sew;

const WS: &str = "aaaaaaaa-0000-4000-8000-0000000000a1";

// ── vendored crates ───────────────────────────────────────────────────

/// Every file under third_party/sew-proto and third_party/sew-client
/// matches `SEW-VENDOR.txt` byte for byte; nothing is missing or extra.
#[test]
fn vendored_sew_crates_match_the_manifest() {
    let third = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../third_party");
    let manifest = std::fs::read_to_string(third.join("SEW-VENDOR.txt")).expect("third_party/SEW-VENDOR.txt");
    let body = manifest.split("# manifest\n").nth(1).expect("a '# manifest' section");
    let mut want: HashMap<String, String> = HashMap::new();
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        let (sha, path) = line.split_once("  ").unwrap_or_else(|| panic!("bad manifest line {line:?}"));
        assert_eq!(sha.len(), 64, "bad sha in {line:?}");
        assert!(want.insert(path.to_string(), sha.to_string()).is_none(), "duplicate {path}");
    }
    assert!(want.len() > 100, "manifest lists only {} files", want.len());
    let mut seen = 0usize;
    for krate in ["sew-proto", "sew-client"] {
        let mut stack = vec![third.join(krate)];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("{}: {e}", d.display())) {
                let p = e.expect("dir entry").path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(&third).expect("under third_party").to_string_lossy().replace('\\', "/");
                let sha = format!("{:x}", Sha256::digest(std::fs::read(&p).expect("read vendored file")));
                match want.get(&rel) {
                    Some(w) => assert_eq!(&sha, w, "{rel} changed since it was vendored"),
                    None => panic!("{rel} is not in SEW-VENDOR.txt (extra file)"),
                }
                seen += 1;
            }
        }
    }
    assert_eq!(seen, want.len(), "files listed in SEW-VENDOR.txt are missing from third_party");
}

// ── pure mappings ─────────────────────────────────────────────────────

fn pmessage(v: Value) -> PMessage {
    serde_json::from_value(v).expect("a sew-proto Message")
}

fn base_message() -> Value {
    json!({
        "id": "501", "channel_id": "301", "author_id": "11", "body": "hello",
        "also_in_channel": false,
        "mentions": {"members": [], "roles": [], "groups": [], "channel": false, "here": false},
        "reactions": [], "attachments": [], "pinned": false, "seq": 9,
        "created_at": "2026-10-20T00:00:10.000Z",
    })
}

#[test]
fn messages_map_authors_threads_mentions_and_tombstones() {
    let mut authors = HashMap::new();
    authors.insert("11".to_string(), MemberRef { member_id: "11".into(), username: "rosson".into(), is_agent: false });
    let m = map_message(&pmessage(base_message()), &authors);
    assert_eq!((m.id.as_str(), m.channel_id.as_str(), m.author_username.as_str()), ("501", "301", "rosson"));
    assert!(!m.author_is_agent && !m.deleted && !m.mentions_me && !m.following);
    assert_eq!(m.created_at, 1_792_454_410, "unix seconds");
    assert_eq!((m.reply_to, m.reply_count, m.message_kind), (None, 0, None));

    let mut v = base_message();
    v["thread_root_id"] = json!("500");
    v["on_behalf_of"] = json!("12");
    v["mentions_me"] = json!(true);
    v["edited_at"] = json!("2026-10-20T00:00:20.000Z");
    let m = map_message(&pmessage(v), &HashMap::new());
    assert_eq!(m.reply_to.as_deref(), Some("500"));
    assert_eq!(m.on_behalf_of.as_deref(), Some("12"));
    assert!(m.mentions_me);
    assert_eq!(m.edited_at, Some(1_792_454_420));
    assert_eq!(m.author_username, "unknown", "an author the agent can't see stays unnamed");

    let mut v = base_message();
    v["thread"] = json!({"root_id": "501", "channel_id": "301", "reply_count": 3, "following": true});
    let m = map_message(&pmessage(v), &HashMap::new());
    assert_eq!((m.reply_count, m.following), (3, true));

    let mut v = base_message();
    v["deleted_at"] = json!("2026-10-20T00:01:00.000Z");
    let m = map_message(&pmessage(v), &HashMap::new());
    assert!(m.deleted && m.body.is_empty());
}

fn pchannel(v: Value) -> PChannel {
    serde_json::from_value(v).expect("a sew-proto Channel")
}

#[test]
fn channels_map_kind_privacy_and_send_permission() {
    let text = pchannel(json!({
        "id": "301", "space_id": "201", "type": "text", "visibility": "private", "slug": "general",
        "name": "General", "position": 0, "synced": true, "head_seq": 1, "dm_member_ids": [],
        "my_perms": "3", "created_at": "2026-10-20T00:00:00.000Z",
    }));
    let c = map_channel(&text, Some("hearth".into()));
    assert_eq!((c.slug.as_str(), c.name.as_str(), c.space_slug.as_deref()), ("general", "General", Some("hearth")));
    assert_eq!(c.kind, ChannelKind::Text);
    assert!(c.private && !c.archived);
    assert!(!c.can_send, "my_perms 3 = VIEW + READ_HISTORY, no SEND_MESSAGES");

    let dm = pchannel(json!({
        "id": "302", "type": "dm", "visibility": "private", "position": 0, "synced": true,
        "head_seq": 1, "dm_member_ids": ["11", "12"], "created_at": "2026-10-20T00:00:00.000Z",
    }));
    let c = map_channel(&dm, Some("ignored".into()));
    assert_eq!(c.kind, ChannelKind::Dm);
    assert_eq!(c.space_slug, None, "a DM is outside every Space");
    assert_eq!(c.dm_member_ids, vec!["11".to_string(), "12".to_string()]);
    assert_eq!(c.slug, "302", "a DM has no slug: its id stands in");
    assert!(c.can_send, "Sew leaves my_perms off DMs");
}

#[test]
fn overwrites_touch_only_view_and_send() {
    assert_eq!(perm_bits(&[ChannelPerm::View]).0, Perms::VIEW_CHANNEL.0);
    assert_eq!(perm_bits(&[ChannelPerm::Send, ChannelPerm::View]).0, Perms::VIEW_CHANNEL.0 | Perms::SEND_MESSAGES.0);
    let existing: POverwrite = serde_json::from_value(json!({
        "target_kind": "role", "target_id": "201",
        "allow": (Perms::ADD_REACTIONS.0 | Perms::SEND_MESSAGES.0).to_string(),
        "deny": Perms::MANAGE_MESSAGES.0.to_string(),
    }))
    .expect("a sew-proto Overwrite");
    let o = Overwrite { role_id: None, allow: vec![ChannelPerm::View], deny: vec![ChannelPerm::Send] };
    let (allow, deny) = merge_overwrite(Some(&existing), &o);
    assert_eq!(allow.0, Perms::ADD_REACTIONS.0 | Perms::VIEW_CHANNEL.0, "Sew's other bits stay; SEND moves to deny");
    assert_eq!(deny.0, Perms::MANAGE_MESSAGES.0 | Perms::SEND_MESSAGES.0);
    let both = Overwrite { role_id: Some("9".into()), allow: vec![ChannelPerm::Send], deny: vec![ChannelPerm::Send] };
    let (allow, deny) = merge_overwrite(None, &both);
    assert_eq!((allow.0, deny.0), (0, Perms::SEND_MESSAGES.0), "allow + deny of one bit is a deny");
}

#[test]
fn actor_slugs_and_addresses() {
    assert_eq!(actor_header("person:login:ada via vault-password").as_deref(), Some("person:login:ada.via.vault-password"));
    assert_eq!(actor_header("agent:ops").as_deref(), Some("agent:ops"));
    assert_eq!(actor_header("  ").as_deref(), None);
    assert_eq!(actor_header(&"x".repeat(300)).map(|s| s.len()), Some(128));
    assert_eq!(slugify("Release Notes!").unwrap().as_str(), "release-notes");
    assert_eq!(slugify("  ops  ").unwrap().as_str(), "ops");
    assert!(slugify("!!!").is_err());
    assert_eq!(build_addr("hearth", "general").unwrap().to_query_value(), "hearth/general");
    assert!(build_addr("hearth", "#301").unwrap().to_query_value().contains("%23301"));
    assert_eq!(build_addr("Hearth", "general").unwrap_err().status, 400, "a Space slug is lowercase");
    assert_eq!(parse_id("mem-x").unwrap_err().status, 404, "a non-Sew id is a masked 404");
}

#[test]
fn errors_keep_sews_envelope_and_masked_404s() {
    let e = decode_raw::<Value>(404, &[], br#"{"ok":false,"error":{"code":"not_found","hint":"channel not found","details":{}}}"#)
        .unwrap_err();
    assert_eq!((e.status, e.code.as_str(), e.hint.as_str()), (404, "not_found", "channel not found"));
    let e = decode_raw::<Value>(
        403,
        &[],
        br#"{"ok":false,"error":{"code":"missing_permission","hint":"no","details":{"reason":"SEND_MESSAGES"}}}"#,
    )
    .unwrap_err();
    assert_eq!((e.code.as_str(), e.reason.as_deref()), ("missing_permission", Some("SEND_MESSAGES")));
    let e = decode_raw::<Value>(
        429,
        &[("retry-after".into(), "3".into())],
        br#"{"ok":false,"error":{"code":"rate_limited","hint":"slow","details":{}}}"#,
    )
    .unwrap_err();
    assert_eq!(e.retry_after_ms, Some(3000));
    let e = decode_raw::<Value>(502, &[], b"<html>").unwrap_err();
    assert_eq!((e.status, e.code.as_str()), (502, "unexpected_status"));
    assert_eq!(decode_raw::<Value>(200, &[], br#"{"a":1}"#).unwrap(), json!({"a": 1}));

    let e = sew_error(ClientError::UnexpectedStatus { status: 502, body_snippet: String::new() });
    assert_eq!((e.status, e.code.as_str()), (502, "unexpected_status"));
    let e = sew_error(ClientError::InvalidInput { field: "body".into(), hint: "too long".into() });
    assert_eq!((e.status, e.code.as_str()), (400, "usage"));
    let e = sew_error(ClientError::Config("no socket".into()));
    assert_eq!((e.status, e.code.as_str()), (503, "unavailable"));
}

#[test]
fn bans_and_visibility_changes_answer_not_supported_on_sew_030() {
    let b = SewClientChat::new("/nonexistent/sew.sock", Arc::new(|_| Ok("t".into()))).unwrap();
    let a = ActAs { workspace_uuid: WS.into(), k2_session: "s1".into(), on_behalf_of: None, actor: None };
    for e in [
        b.ban(&a, "201", "11", None).unwrap_err(),
        b.unban(&a, "201", "11").unwrap_err(),
        b.channel_set_private(&a, "301", true).unwrap_err(),
    ] {
        assert_eq!((e.status, e.code.as_str()), (501, "not_supported"), "{e:?}");
    }
}

// ── the real client over the fake Sew mgmt socket ─────────────────────

async fn backend(home: &k2_core::test_env::TempHome) -> (fake_sew::FakeSew, Arc<SewClientChat>) {
    let sew = fake_sew::FakeSew::start(&home.path().join("s")).await;
    sew.seed(&format!("k2:workspace:{WS}"), fake_sew_kind_agent(), "alpha", "Alpha", false);
    sew.seed("k2:owner", fake_sew_kind_person(), "rosson", "Rosson", false);
    let b = SewClientChat::new(sew.socket.clone(), Arc::new(|_| Ok("fake-mgmt-token".into()))).unwrap();
    (sew, Arc::new(b))
}

fn fake_sew_kind_agent() -> k2_core::tool_hook::sew_wire::MemberKind {
    k2_core::tool_hook::sew_wire::MemberKind::Agent
}

fn fake_sew_kind_person() -> k2_core::tool_hook::sew_wire::MemberKind {
    k2_core::tool_hook::sew_wire::MemberKind::Person
}

fn act_as(actor: Option<&str>) -> ActAs {
    ActAs { workspace_uuid: WS.into(), k2_session: "sess-1".into(), on_behalf_of: None, actor: actor.map(str::to_string) }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn resolve_and_post_go_out_as_the_agent_with_its_key() {
    let home = k2_core::test_env::TempHome::short();
    let (sew, b) = backend(&home).await;
    let b2 = b.clone();
    let (ch, me, posted) = tokio::task::spawn_blocking(move || {
        let a = act_as(None);
        let ch = b2.resolve(&a, "hearth", "general").expect("resolve");
        let me = b2.me(&a).expect("me");
        let m = b2
            .create(
                &a,
                &NewMessage {
                    channel_id: ch.id.clone(),
                    body: "hi from alpha".into(),
                    reply_to: None,
                    broadcast: false,
                    idempotency_key: "k2c-key1".into(),
                },
            )
            .expect("post");
        (ch, me, m)
    })
    .await
    .unwrap();
    sew.assert_clean();
    assert_eq!((ch.id.as_str(), ch.space_slug.as_deref(), ch.slug.as_str()), ("301", Some("hearth"), "general"));
    assert!(ch.can_send);
    assert_eq!(me.username, "alpha");
    assert_eq!((posted.author_username.as_str(), posted.body.as_str()), ("alpha", "hi from alpha"));
    assert_eq!(sew.chat_posts().len(), 1);
    let calls = sew.calls();
    let api: Vec<_> = calls.iter().filter(|c| c.path.starts_with("/api/v1/")).collect();
    assert!(!api.is_empty(), "{}", sew.log());
    for c in &api {
        assert_eq!(c.act_as.as_deref(), Some(format!("k2:workspace:{WS}").as_str()), "{} {}", c.method, c.path);
        assert_eq!(c.via.as_deref(), Some(format!("k2; agent={WS}; k2_session=sess-1").as_str()), "{}", c.path);
    }
    let post = calls.iter().find(|c| c.method == "POST" && c.path == "/api/v1/channels/301/messages").expect("the post");
    assert_eq!(post.key.as_deref(), Some("k2c-key1"), "the client key is the Idempotency-Key");
    assert_eq!(post.actor, None, "no approval, no Sew-Actor");
    assert!(calls.iter().any(|c| c.path == "/api/v1/resolve"), "{}", sew.log());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approved_post_carries_sew_actor_and_a_hidden_channel_is_sews_404() {
    let home = k2_core::test_env::TempHome::short();
    let (sew, b) = backend(&home).await;
    let b2 = b.clone();
    let (miss, m) = tokio::task::spawn_blocking(move || {
        let a = act_as(Some("person:login:ada via vault-password"));
        let miss = b2.resolve(&a, "hearth", "secret").unwrap_err();
        let m = b2
            .create(
                &a,
                &NewMessage {
                    channel_id: "301".into(),
                    body: "approved".into(),
                    reply_to: None,
                    broadcast: false,
                    idempotency_key: "k2c-key2".into(),
                },
            )
            .expect("approved post");
        (miss, m)
    })
    .await
    .unwrap();
    sew.assert_clean();
    assert_eq!((miss.status, miss.code.as_str()), (404, "not_found"), "masked, unchanged");
    assert_eq!(m.body, "approved");
    let post = sew.calls().into_iter().find(|c| c.method == "POST").expect("the post");
    assert_eq!(post.actor.as_deref(), Some("person:login:ada.via.vault-password"));
    assert_eq!(post.key.as_deref(), Some("k2c-key2"));
    assert_eq!(post.act_as.as_deref(), Some(format!("k2:workspace:{WS}").as_str()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn space_create_is_a_mgmt_call_naming_the_agent_manager() {
    let home = k2_core::test_env::TempHome::short();
    let (sew, b) = backend(&home).await;
    let b2 = b.clone();
    let (sp, refused) = tokio::task::spawn_blocking(move || {
        let a = act_as(Some("agent:alpha"));
        let sp = b2
            .space_create(
                &a,
                &NewSpace {
                    slug: "ops".into(),
                    name: "Ops".into(),
                    owner_external_id: "k2:owner".into(),
                    idempotency_key: "k2c-space1".into(),
                },
            )
            .expect("space create");
        let refused = b2
            .space_create(
                &a,
                &NewSpace {
                    slug: "mine".into(),
                    name: "Mine".into(),
                    owner_external_id: format!("k2:workspace:{WS}"),
                    idempotency_key: "k2c-space2".into(),
                },
            )
            .unwrap_err();
        (sp, refused)
    })
    .await
    .unwrap();
    sew.assert_clean();
    assert_eq!((sp.id.as_str(), sp.slug.as_str(), sp.name.as_str()), ("202", "ops", "Ops"));
    assert_eq!(refused.status, 400, "Sew refuses an agent owner");
    let c = sew.calls().into_iter().find(|c| c.path == "/mgmt/v1/spaces/create").expect("create call");
    assert_eq!(c.act_as, None, "spaces/create is a mgmt call, never act-as");
    assert_eq!(c.actor.as_deref(), Some("agent:alpha"));
    assert_eq!(c.key.as_deref(), Some("k2c-space1"));
    assert_eq!(c.body["owner"], "k2:owner");
    assert_eq!(c.body["managers"], json!([format!("k2:workspace:{WS}")]));
}
