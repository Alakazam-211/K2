//! Permission cards lane A3 (prd-agent-settings-cards-v1 rev 3.1 §4.2, §4.6,
//! §17.1): one card, projected per viewer at every door that serves Thread
//! docs, headless on the real dispatcher (temp `$HOME`, loopback, real
//! sockets):
//!
//! - `GET /cli/thread` (page and `after_seq` delta), `POST
//!   /cli/thread/catchup`, `WS /cli/overlay/events` and `WS
//!   /cli/thread/stream` honour `cards=2` for people only: the approver
//!   gets the approve view, a member below the floor the wait view with a
//!   note box, an old client (no `cards=2`) and an agent passport the
//!   text fallback;
//! - `GET /cli/thread/latest`'s preview is the fallback line for that
//!   viewer;
//! - read state: the card is waiting only for people at its floor, and an
//!   App guest's `waiting` / `waitingTotal` never count it (nor does the
//!   guest see it on any read).
//!
//! The card is written with the same core call `present_action_card` uses
//! (A1 posts the real ones), with the stored view of the phone contract's
//! §17.11.1.

#![recursion_limit = "256"]

#[path = "support/app_fixture.rs"]
mod app_fixture;

use std::time::Duration;

use app_fixture::*;
use futures_util::StreamExt;
use k2_core::connect_users::{self, Role};
use k2_core::db::schema::WorkspaceSession;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

type Sock = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PW: &str = "s3cret-horse-staple";
const FRAME_WAIT: Duration = Duration::from_secs(10);
const SETTING: &str = "Allow compute nodes";

struct Room {
    ws: Ws,
    conv: String,
    /// A Connect Member login (the asker, below the admin floor).
    member: String,
    member_name: String,
    /// A Connect Admin login (at the floor).
    admin: String,
    guest: String,
}

fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..6].to_string()
}

async fn owner_ok(port: u16, path: &str, body: Value) -> Value {
    let r = post(port, path, OWNER, body).await;
    assert_eq!(r.status, 200, "{path}: {}", r.body);
    r.json()
}

fn login(name: &str, role: Role) -> String {
    connect_users::add_user(name, PW).expect("add_user");
    connect_users::set_role(name, role).expect("set_role");
    connect_users::create_session(name)
}

async fn room(env: &Env, port: u16) -> Room {
    let t = tag();
    let ws = seed_ws(env, &format!("ppj{t}"));
    let conv = uuid::Uuid::new_v4().to_string();
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        WorkspaceSession::upsert(&conn, &format!("ws-{conv}"), &ws.id, None, Some(&conv), "claude", "system", "running").expect("pin");
    }
    let member_name = format!("pm{t}");
    let member = login(&member_name, Role::Member);
    let admin = login(&format!("pa{t}"), Role::Admin);
    let role = format!("pr{t}");
    let user = format!("pg{t}");
    owner_ok(port, "/cli/skin/roles", json!({ "name": role })).await;
    owner_ok(port, "/cli/skin/roles/room", json!({ "name": role, "handle": ws.handle, "caps": ["thread:read"] })).await;
    owner_ok(port, "/cli/skin/users", json!({ "username": user })).await;
    owner_ok(port, "/cli/skin/users/password", json!({ "username": user, "password": PW })).await;
    owner_ok(port, "/cli/skin/roles/assign", json!({ "username": user, "role": role })).await;
    let body = json!({ "username": user, "password": PW }).to_string();
    let r = tokio::task::spawn_blocking(move || req_blocking(port, "POST", "/cli/skin/login", Some(body))).await.unwrap();
    assert_eq!(r.status, 200, "guest login: {}", r.body);
    let guest = r.json()["token"].as_str().expect("guest token").to_string();
    Room { ws, conv, member, member_name, admin, guest }
}

/// A pending permission card on the room's pinned Chat (admin floor, asked
/// while working with the member), posted and published as A1 does.
fn post_card(r: &Room) -> String {
    let rid = format!("pr_{}", tag());
    let h = &r.ws.handle;
    let action: k2_core::overlay::CardAction = serde_json::from_value(json!({
        "id": "permission", "params": { "request": rid }, "dedupe": format!("compute.use:{rid}"),
        "view": {"v":1,"kind":"permission","requestId":rid,"state":"pending",
            "viewer":{"login":"owner","role":"owner","via":"owner-token","mode":"approve","canAnswer":true,"isAsker":false},
            "action":"compute.use","setting":SETTING,
            "whatChanges":format!("Let {h} run jobs on this server's compute nodes."),
            "direction":"loosen","grant":"standing","scope":h,"duration":"until someone turns it off",
            "consequences":"Every active machine; default limits.",
            "values":[{"key":"workspace","value":h}],
            "why":{"by":h,"text":"Run the test suite on a mini"},
            "notes":[],"askedFor":r.member_name,"askers":null,
            "approvers":{"floor":"admin","label":"owner or admin","names":["owner"]},
            "proof":{"kind":"click"},"expiresAt":k2_core::overlay::now_secs() + 3600,"settingsPage":format!("workspaces/{h}/agent"),
            "noteBox":{"allowed":false,"max":500,"mine":null},"answer":null,"undo":null}
    }))
    .expect("a permission action");
    let db = k2_core::db::shared();
    let conn = db.lock();
    let (item, links) = k2_core::overlay::post_choice_with_action(
        &conn,
        &r.conv,
        &r.ws.id,
        h,
        h,
        &format!("{h} asks: Let {h} run jobs on this server's compute nodes ({SETTING})."),
        vec!["Allow".to_string(), "Deny".to_string()],
        false,
        Some(k2_core::overlay::now_secs() + 3600),
        Some(action),
    )
    .expect("post the card");
    drop(conn);
    k2_daemon::overlay_ws::emit_links(&links, &item.doc);
    item.id
}

async fn post_text(port: u16, ws: &Ws, text: &str) -> String {
    let r = post(port, "/cli/thread/post", OWNER, json!({ "addr": ws.handle, "text": text })).await;
    assert_eq!(r.status, 200, "{}", r.body);
    r.json()["id"].as_str().expect("id").to_string()
}

/// The card's `doc` in a read's `items`.
fn card_in<'a>(items: &'a Value, id: &str) -> &'a Value {
    &items.as_array().unwrap_or_else(|| panic!("items: {items}")).iter().find(|i| i["id"] == id).unwrap_or_else(|| panic!("card {id} in {items}"))["doc"]
}

/// The approve view (cards=2, at the floor).
fn assert_approve(doc: &Value, login: &str, what: &str) {
    assert_eq!(doc["kind"], "choice", "{what}: {doc}");
    let v = &doc["choice"]["action"]["view"];
    assert_eq!((v["viewer"]["login"].as_str(), v["viewer"]["mode"].as_str(), v["viewer"]["canAnswer"].as_bool()), (Some(login), Some("approve"), Some(true)), "{what}: {doc}");
    assert_eq!(doc["choice"]["options"], json!([{"label":"Allow"},{"label":"Deny"}]), "{what}");
}

/// The wait view (cards=2, below the floor): no options, a note box.
fn assert_wait(doc: &Value, login: &str, what: &str) {
    let v = &doc["choice"]["action"]["view"];
    assert_eq!(
        (v["viewer"]["login"].as_str(), v["viewer"]["mode"].as_str(), v["viewer"]["isAsker"].as_bool()),
        (Some(login), Some("wait"), Some(true)),
        "{what}: {doc}"
    );
    assert_eq!(doc["choice"]["options"], json!([]), "{what}");
    assert_eq!(v["noteBox"], json!({"allowed":true,"max":500,"mine":null}), "{what}");
}

/// The fallback (no cards=2, or an agent): text, no choice, no action.
fn assert_text(doc: &Value, body: &str, what: &str) {
    assert_eq!((doc["kind"].as_str(), doc["body"].as_str()), (Some("text"), Some(body)), "{what}: {doc}");
    assert!(doc.get("choice").is_none(), "{what}: {doc}");
}

fn asker_line(r: &Room) -> String {
    format!("Asked an admin to turn on {SETTING} for you. You can keep working with {} meanwhile.", r.ws.handle)
}

fn agent_line(r: &Room) -> String {
    format!("{} asked an admin to turn on {SETTING}.", r.ws.handle)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_plain_read_delta_and_catchup_project_per_viewer() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;
    let card = post_card(&r);
    let h = enc(&r.ws.handle);
    let passport = passport(&r.ws);

    for base in [format!("addr={h}&limit=20"), format!("addr={h}&after_seq=0&limit=20")] {
        let q2 = format!("{base}&cards=2");
        let read = |tok: String, q: String| async move {
            let resp = get(port, "/cli/thread", &tok, &q).await;
            assert_eq!(resp.status, 200, "{q}: {}", resp.body);
            resp.json()
        };
        assert_approve(card_in(&read(OWNER.into(), q2.clone()).await["items"], &card), "owner", &q2);
        let admin_login = connect_users::validate_session(&r.admin).expect("admin session");
        assert_approve(card_in(&read(r.admin.clone(), q2.clone()).await["items"], &card), &admin_login, &q2);
        assert_wait(card_in(&read(r.member.clone(), q2.clone()).await["items"], &card), &r.member_name, &q2);
        assert_text(card_in(&read(r.member.clone(), base.clone()).await["items"], &card), &asker_line(&r), &base);
        // An agent passport always gets the fallback, whatever it sends.
        assert_text(card_in(&read(passport.clone(), q2.clone()).await["items"], &card), &agent_line(&r), "passport");
        // The owner on an old build: a plain choice card, action {id, params}.
        let old = read(OWNER.into(), base.clone()).await;
        let c = card_in(&old["items"], &card);
        assert_eq!((c["kind"].as_str(), c["choice"]["action"].get("view")), (Some("choice"), None), "{c}");
        // A guest never gets the card (K2's rule).
        let g = read(r.guest.clone(), q2.clone()).await;
        assert!(!g.to_string().contains(&card), "a guest saw the card: {g}");
    }

    for (want, label) in [(json!({ "addr": r.ws.handle, "afterSeq": 0 }), "after_seq"), (json!({ "addr": r.ws.handle, "newest": true }), "newest")] {
        let cu = |tok: String, cards: Option<u8>| {
            let mut body = json!({ "threads": [want.clone()] });
            if let Some(c) = cards {
                body["cards"] = json!(c);
            }
            async move {
                let resp = post(port, "/cli/thread/catchup", &tok, body).await;
                assert_eq!(resp.status, 200, "{}", resp.body);
                resp.json()
            }
        };
        assert_approve(card_in(&cu(OWNER.into(), Some(2)).await["threads"][0]["items"], &card), "owner", label);
        assert_wait(card_in(&cu(r.member.clone(), Some(2)).await["threads"][0]["items"], &card), &r.member_name, label);
        assert_text(card_in(&cu(r.member.clone(), None).await["threads"][0]["items"], &card), &asker_line(&r), label);
        let g = cu(r.guest.clone(), Some(2)).await;
        assert!(!g.to_string().contains(&card), "a guest saw the card: {g}");
    }
    drop(env);
}

async fn next_text(sock: &mut Sock, what: &str) -> Value {
    loop {
        let m = tokio::time::timeout(FRAME_WAIT, sock.next())
            .await
            .unwrap_or_else(|_| panic!("a frame in time: {what}"))
            .unwrap_or_else(|| panic!("socket open: {what}"))
            .unwrap_or_else(|e| panic!("frame ok ({what}): {e}"));
        match m {
            Message::Text(t) => {
                let f: Value = serde_json::from_str(&t).unwrap_or_else(|e| panic!("JSON frame ({e}): {t}"));
                if f["collection"] == "activity" || f["collection"] == "read" {
                    continue;
                }
                return f;
            }
            Message::Ping(_) | Message::Pong(_) => continue,
            other => panic!("unexpected frame {other:?} ({what})"),
        }
    }
}

/// The frame carrying `id` (overlay socket: `{id, doc}`; stream:
/// `{kind:"thread_message", id, doc}`).
async fn frame_for(sock: &mut Sock, id: &str, what: &str) -> Value {
    loop {
        let f = next_text(sock, what).await;
        if f["id"] == id {
            return f;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn both_sockets_project_each_frame_for_their_viewer() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;
    let overlay = |token: String, cards: bool| {
        let extra = if cards { "&cards=2" } else { "" };
        let url = format!("ws://127.0.0.1:{port}/cli/overlay/events?conversation={}&token={}{extra}", r.conv, enc(&token));
        async move { tokio_tungstenite::connect_async(url).await.expect("overlay socket").0 }
    };
    let stream = |token: String, cards: bool| {
        let extra = if cards { "&cards=2" } else { "" };
        let url = format!("ws://127.0.0.1:{port}/cli/thread/stream?rooms=all&token={}{extra}", enc(&token));
        async move { tokio_tungstenite::connect_async(url).await.expect("thread stream").0 }
    };
    let mut o_owner = overlay(OWNER.into(), true).await;
    let mut o_member = overlay(r.member.clone(), true).await;
    let mut o_member_old = overlay(r.member.clone(), false).await;
    let mut s_owner = stream(OWNER.into(), true).await;
    let mut s_member = stream(r.member.clone(), true).await;
    let mut s_member_old = stream(r.member.clone(), false).await;
    for (s, who) in [(&mut s_owner, "owner"), (&mut s_member, "member"), (&mut s_member_old, "member old")] {
        loop {
            if next_text(s, who).await["kind"] == "inbox_subscribed" {
                break;
            }
        }
    }
    let card = post_card(&r);

    assert_approve(&frame_for(&mut o_owner, &card, "overlay owner").await["doc"], "owner", "overlay owner");
    assert_wait(&frame_for(&mut o_member, &card, "overlay member").await["doc"], &r.member_name, "overlay member");
    assert_text(&frame_for(&mut o_member_old, &card, "overlay old").await["doc"], &asker_line(&r), "overlay old");
    let f = frame_for(&mut s_owner, &card, "stream owner").await;
    assert_eq!(f["kind"], "thread_message");
    assert_approve(&f["doc"], "owner", "stream owner");
    assert_wait(&frame_for(&mut s_member, &card, "stream member").await["doc"], &r.member_name, "stream member");
    assert_text(&frame_for(&mut s_member_old, &card, "stream old").await["doc"], &asker_line(&r), "stream old");
    // A plain post still goes out unchanged to everyone.
    let marker = post_text(port, &r.ws, "marker").await;
    assert_eq!(frame_for(&mut o_member, &marker, "marker").await["doc"]["body"], "marker");
    drop(env);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn latest_previews_and_read_state_follow_the_viewer() {
    let env = setup(&[]);
    let d = k2_daemon::test_harness::start(OWNER).await;
    let port = d.port;
    let r = room(&env, port).await;
    // Everyone has read the room up to here.
    let before = post_text(port, &r.ws, "before").await;
    for tok in [OWNER.to_string(), r.member.clone(), r.admin.clone(), r.guest.clone()] {
        let resp = post(port, "/cli/thread/read", &tok, json!({ "conversationId": r.conv })).await;
        assert_eq!(resp.status, 200, "read: {}", resp.body);
    }
    let card = post_card(&r);
    let q = format!("addrs={}&cards=2", enc(&r.ws.handle));

    let row = |v: Value| v["items"][0].clone();
    let owner = row(get(port, "/cli/thread/latest", OWNER, &q).await.json());
    assert_eq!(owner["kind"], "choice", "{owner}");
    assert!(owner["preview"].as_str().unwrap().starts_with(&format!("Asked: {} asks:", r.ws.handle)), "{owner}");
    let member = row(get(port, "/cli/thread/latest", &r.member, &q).await.json());
    assert_eq!((member["kind"].as_str(), member["preview"].as_str()), (Some("text"), Some(asker_line(&r).as_str())), "{member}");
    let pass = passport(&r.ws);
    let agent = row(get(port, "/cli/thread/latest", &pass, &q).await.json());
    assert_eq!(agent["preview"], agent_line(&r), "{agent}");
    let guest = get(port, "/cli/thread/latest", &r.guest, &q).await;
    assert_eq!(row(guest.json())["preview"], "before", "{}", guest.body);
    assert!(!guest.body.contains(&card));
    let _ = before;

    // Read state (§4.6): waiting only at the floor; a member reads it as an
    // unread line; an App guest learns nothing.
    let unread = |tok: String| async move {
        let resp = get(port, "/cli/thread/unread", &tok, "").await;
        assert_eq!(resp.status, 200, "{}", resp.body);
        resp.json()
    };
    let conv_row = |u: &Value| -> Value {
        u["byConversation"].as_array().and_then(|a| a.iter().find(|x| x["id"] == r.conv.as_str()).cloned()).unwrap_or(Value::Null)
    };
    let o = unread(OWNER.into()).await;
    assert_eq!((conv_row(&o)["waiting"].as_i64(), conv_row(&o)["unread"].as_i64()), (Some(1), Some(0)), "owner: {o}");
    // Totals span every room in this process's database (other tests'
    // cards too), so people at the floor are checked per conversation.
    assert!(o["waitingTotal"].as_i64().is_some_and(|n| n >= 1), "{o}");
    let a = unread(r.admin.clone()).await;
    assert_eq!(conv_row(&a)["waiting"].as_i64(), Some(1), "admin: {a}");
    let m = unread(r.member.clone()).await;
    assert_eq!((conv_row(&m)["waiting"].as_i64(), conv_row(&m)["unread"].as_i64(), m["waitingTotal"].as_i64()), (Some(0), Some(1), Some(0)), "member: {m}");
    let g = unread(r.guest.clone()).await;
    assert_eq!((g["waitingTotal"].as_i64(), g["total"].as_i64()), (Some(0), Some(0)), "a guest never counts an action card: {g}");
    let gr = conv_row(&g);
    if !gr.is_null() {
        assert_eq!((gr["waiting"].as_i64(), gr["unread"].as_i64()), (Some(0), Some(0)), "guest row: {g}");
    }
    assert!(!g.to_string().contains(&card));
    drop(env);
}
