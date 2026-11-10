//! OA0 (0.46.11): people's mailboxes through a named workspace, end to end
//! through the real dispatcher (`test_harness`), the fake mail engine and
//! Connect logins. prd-ocmail-admin-in-k2-v1 §7.3 and T10.
//!
//! One workspace holds three inboxes: an agent's hosted address, a person's
//! hosted mailbox (`pat`'s) and a linked inbox (always someone's own). The
//! "masked" checks compare each answer with the answer for the same request
//! naming an address K2 has never heard of: a refusal must not tell the two
//! apart.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

use crate::mail::fake_jmap::FakeJmap;
use crate::mail::messages::encode_message_id;
use crate::test_harness;

const OWNER: &str = "owner-token-oa0-mailbox-guard";

struct Resp {
    status: u16,
    body: String,
}

fn http(port: u16, method: &str, path_and_query: &str, body: Option<&str>) -> Resp {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to test daemon");
    stream.set_read_timeout(Some(Duration::from_secs(30))).expect("read timeout");
    let req = match body {
        Some(b) => format!(
            "{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{b}",
            b.len()
        ),
        None => format!("{method} {path_and_query} HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"),
    };
    stream.write_all(req.as_bytes()).expect("write request");
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => raw.extend_from_slice(&chunk[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                panic!("{method} {path_and_query}: no EOF within 30s; got {:?}", String::from_utf8_lossy(&raw))
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::UnexpectedEof) => break,
            Err(e) => panic!("read response: {e:?}"),
        }
    }
    let text = String::from_utf8_lossy(&raw).to_string();
    let status = text
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("{method} {path_and_query}: no status line in {text:?}"));
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    Resp { status, body }
}

fn json(r: &Resp) -> serde_json::Value {
    serde_json::from_str(&r.body).unwrap_or_else(|e| panic!("body is not JSON ({e}): {:?}", r.body))
}

fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// A temp `$HOME` for the connect-user store and the vault (restored on drop).
struct Home {
    prev: Option<std::ffi::OsString>,
    dir: std::path::PathBuf,
}

impl Home {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("k2-oa0-{}-{}", std::process::id(), uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join(".k2")).expect("temp HOME");
        let prev = std::env::var_os("HOME");
        std::env::set_var("HOME", &dir);
        Home { prev, dir }
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(p) => std::env::set_var("HOME", p),
            None => std::env::remove_var("HOME"),
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// An email the fake engine serves (`account` lists it in `Email/query`).
fn seed_email(fake: &FakeJmap, account: &str, id: &str, subject: &str) {
    let mut st = fake.state.lock().unwrap();
    st.emails.insert(
        id.to_string(),
        serde_json::json!({
            "id": id,
            "threadId": format!("t-{id}"),
            "from": [{ "name": "Sender", "email": "sender@outside.example" }],
            "to": [{ "name": "", "email": "someone@inside.example" }],
            "subject": subject,
            "receivedAt": "2026-11-01T10:00:00Z",
            "keywords": {},
            "hasAttachment": false,
            "textBody": [{ "partId": "1", "type": "text/plain" }],
            "htmlBody": [],
            "bodyValues": { "1": { "value": format!("body of {subject}") } },
            "attachments": [],
            "blobId": format!("blob-{id}"),
            "header:Message-ID:asText": format!("<{id}@outside.example>"),
            "header:References:asText": null,
            "header:Authentication-Results:asText:all": [],
        }),
    );
    st.inbox.entry(account.to_string()).or_default().push(id.to_string());
}

struct Fx {
    _home: Home,
    _key: k2_core::test_env::EnvVar,
    _mail: crate::mail::MailServerTestGuard,
    _row: crate::mail::MailServerRowCleanup,
    d: test_harness::TestDaemon,
    fake: FakeJmap,
    ws_id: String,
    ws_path: String,
    domain: String,
    agent_addr: String,
    person_addr: String,
    linked_addr: String,
    ghost_addr: String,
    agent_tok: String,
    person_tok: String,
    linked_tok: String,
    ghost_tok: String,
    member: String,
    admin: String,
    owner_login: String,
    pat: String,
}

impl Fx {
    async fn start() -> Fx {
        let home = Home::new();
        let key = k2_core::test_env::EnvVar::set("K2_TEST_MAIL_API_KEY", "k2-test-key");
        let mail = crate::mail::mail_server_test_lock();
        let row = crate::mail::MailServerRowCleanup;
        let d = test_harness::start(OWNER).await;
        let fake = FakeJmap::start();
        let short = &uuid::Uuid::new_v4().simple().to_string()[..10];
        let domain = format!("oa0{short}.example");
        let ws_id = uuid::Uuid::new_v4().to_string();
        let ws_path = home.dir.join(format!("ws-oa0-{short}")).to_string_lossy().to_string();
        std::fs::create_dir_all(&ws_path).expect("workspace dir");
        let agent_addr = format!("bot@{domain}");
        let person_addr = format!("pat@{domain}");
        let ghost_addr = format!("ghost@{domain}");
        let linked_addr = format!("pat.linked{short}@gmail.example");
        {
            let db = k2_core::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path, handle) VALUES (?1, ?2, ?3, ?2)",
                rusqlite::params![ws_id, format!("oa0{short}"), ws_path],
            )
            .expect("seed workspace");
            let _ = conn.execute("DELETE FROM mail_server WHERE id = 1", []);
            conn.execute(
                "INSERT INTO mail_server (id, status, pinned_version, hostname, api_url, api_key_ref, updated_at) \
                 VALUES (1, 'running', '0.16.25', 'mail.box.test', ?1, 'env:K2_TEST_MAIL_API_KEY', 1)",
                rusqlite::params![format!("http://127.0.0.1:{}", fake.port)],
            )
            .expect("mail_server row");
            for (addr, acct, person) in [(&agent_addr, "acc-agent", None), (&person_addr, "acc-pat", Some("pat"))] {
                conn.execute(
                    "INSERT INTO mail_addresses (id, address, domain_id, stalwart_account_id, owner_project_id, \
                     status, created_at, primary_level, primary_can_manage, primary_can_delete, person_mailbox, \
                     person_principal) VALUES (?1, ?2, 'dom-oa0', ?3, ?4, 'active', 100, 'send', 1, 1, ?5, ?6)",
                    rusqlite::params![
                        uuid::Uuid::new_v4().to_string(),
                        addr,
                        acct,
                        ws_id,
                        person.is_some() as i64,
                        person
                    ],
                )
                .expect("seed hosted address");
            }
            conn.execute(
                "INSERT INTO mail_external_inboxes (id, owner_project_id, email_address, host, port, username, \
                 created_at, primary_level) VALUES (?1, ?2, ?3, 'imap.gmail.example', 993, ?3, 100, 'send')",
                rusqlite::params![uuid::Uuid::new_v4().to_string(), ws_id, linked_addr],
            )
            .expect("seed linked inbox");
        }
        seed_email(&fake, "acc-agent", "ea1", "agent mail");
        seed_email(&fake, "acc-pat", "ep1", "pat private mail");
        let mut sessions = Vec::new();
        for (user, role) in [
            ("oa0member", k2_core::connect_users::Role::Member),
            ("oa0admin", k2_core::connect_users::Role::Admin),
            ("oa0owner", k2_core::connect_users::Role::Owner),
            ("pat", k2_core::connect_users::Role::Member),
        ] {
            k2_core::connect_users::add_user(user, "correct-horse-battery-staple-1").expect("add user");
            k2_core::connect_users::set_role(user, role).expect("set role");
            sessions.push(k2_core::connect_users::create_session(user));
        }
        Fx {
            _home: home,
            _key: key,
            _mail: mail,
            _row: row,
            d,
            fake,
            agent_tok: encode_message_id(&agent_addr, "ea1"),
            person_tok: encode_message_id(&person_addr, "ep1"),
            linked_tok: encode_message_id(&linked_addr, "uid:7:1"),
            ghost_tok: encode_message_id(&ghost_addr, "ep1"),
            ws_id,
            ws_path,
            domain,
            agent_addr,
            person_addr,
            linked_addr,
            ghost_addr,
            member: sessions[0].clone(),
            admin: sessions[1].clone(),
            owner_login: sessions[2].clone(),
            pat: sessions[3].clone(),
        }
    }

    fn get(&self, token: &str, route: &str, query: &[(&str, &str)]) -> Resp {
        self.get_on(self.d.port, token, route, query)
    }

    fn get_on(&self, port: u16, token: &str, route: &str, query: &[(&str, &str)]) -> Resp {
        let mut q = format!("token={}", enc(token));
        for (k, v) in query {
            q.push_str(&format!("&{k}={}", enc(v)));
        }
        http(port, "GET", &format!("/cli/mail/{route}?{q}"), None)
    }

    fn post(&self, token: &str, route: &str, body: serde_json::Value) -> Resp {
        http(self.d.port, "POST", &format!("/cli/mail/{route}?token={}", enc(token)), Some(&body.to_string()))
    }

    fn read(&self, token: &str, msg: &str) -> Resp {
        self.get(token, "read", &[("project", &self.ws_path), ("id", msg)])
    }

    /// Every `mail_reader_audit` row, oldest first, as JSON.
    fn audit(&self) -> Vec<serde_json::Value> {
        let db = k2_core::db::shared();
        let conn = db.lock();
        let mut stmt = conn
            .prepare(
                "SELECT actor, actor_role, inbox_source, address, inbox_kind, own, workspace_id, action, \
                 message_ref, outcome, status, ip_hint FROM mail_reader_audit ORDER BY id",
            )
            .expect("the mailbox access log table exists");
        let rows = stmt
            .query_map([], |r| {
                Ok(serde_json::json!({
                    "actor": r.get::<_, String>(0)?,
                    "role": r.get::<_, String>(1)?,
                    "source": r.get::<_, String>(2)?,
                    "address": r.get::<_, String>(3)?,
                    "kind": r.get::<_, String>(4)?,
                    "own": r.get::<_, i64>(5)?,
                    "workspace": r.get::<_, Option<String>>(6)?,
                    "action": r.get::<_, String>(7)?,
                    "messageRef": r.get::<_, Option<String>>(8)?,
                    "outcome": r.get::<_, String>(9)?,
                    "status": r.get::<_, Option<i64>>(10)?,
                    "ipHint": r.get::<_, Option<String>>(11)?,
                }))
            })
            .expect("query the access log");
        rows.map(|r| r.expect("access log row")).collect()
    }

    fn rows_for(&self, actor: &str, address: &str, action: &str) -> Vec<serde_json::Value> {
        self.audit()
            .into_iter()
            .filter(|r| r["actor"] == actor && r["address"] == address && r["action"] == action)
            .collect()
    }

    /// `Email/set` updates the fake engine saw for `account` (mark read).
    fn seen_updates(&self, account: &str) -> usize {
        self.fake.state.lock().unwrap().updates.iter().filter(|(a, _, _)| a == account).count()
    }

    fn email_gets(&self) -> usize {
        self.fake.state.lock().unwrap().gets.len()
    }

    /// The same text with the person's and the ghost's names folded to one
    /// placeholder, so a masked answer compares equal to the unknown one.
    fn fold(&self, s: &str) -> String {
        s.replace(&self.person_tok, "<ID>")
            .replace(&self.ghost_tok, "<ID>")
            .replace(&self.linked_tok, "<ID>")
            .replace(&self.person_addr, "<ADDR>")
            .replace(&self.ghost_addr, "<ADDR>")
            .replace(&self.linked_addr, "<ADDR>")
            .replace("out_oa0_person", "<OUT>")
            .replace("out_oa0_ghost", "<OUT>")
    }
}

fn block<R>(f: impl FnOnce() -> R) -> R {
    tokio::task::block_in_place(f)
}

/// T10: a Member reading a person's mailbox through `read?project=<ws>` gets
/// the masked not_found an unknown address gets; reading the agent inbox
/// still works and writes an access-log row first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn t10_member_read_of_a_person_mailbox_is_masked_and_an_agent_read_is_logged() {
    let fx = Fx::start().await;
    block(|| {
        let person = fx.read(&fx.member, &fx.person_tok);
        let ghost = fx.read(&fx.member, &fx.ghost_tok);
        assert_eq!(person.status, 404, "a Member can't open pat's mailbox: {}", person.body);
        assert_eq!(json(&person)["error"]["code"], "not_found", "{}", person.body);
        assert_eq!(fx.fold(&person.body), fx.fold(&ghost.body), "masked: same answer as an unknown address");
        assert!(!person.body.contains("pat private mail"), "{}", person.body);

        let agent = fx.read(&fx.member, &fx.agent_tok);
        assert_eq!(agent.status, 200, "the agent tab's Inbox keeps working: {}", agent.body);
        assert_eq!(json(&agent)["message"]["subject"], "agent mail", "{}", agent.body);

        let rows = fx.rows_for("user:oa0member", &fx.agent_addr, "read");
        assert_eq!(rows.len(), 1, "one row for the agent read: {:?}", fx.audit());
        assert_eq!(rows[0]["outcome"], "ok");
        assert_eq!(rows[0]["kind"], "agent");
        assert_eq!(rows[0]["role"], "member");
        assert_eq!(rows[0]["status"], 200);
        assert_eq!(rows[0]["workspace"], serde_json::json!(fx.ws_id));
        assert_eq!(rows[0]["messageRef"], serde_json::json!(fx.agent_tok));
        let refused = fx.rows_for("user:oa0member", &fx.person_addr, "read");
        assert_eq!(refused.len(), 1, "the refused try is logged too: {:?}", fx.audit());
        assert_eq!(refused[0]["outcome"], "refused");
        assert_eq!(refused[0]["kind"], "person");
        let text = serde_json::to_string(&fx.audit()).unwrap();
        assert!(!text.contains("agent mail") && !text.contains("body of"), "no message content in the log: {text}");
    });
}

/// A Member naming the workspace gets, on every route that reaches a
/// mailbox, the same answer for pat's mailbox (and the linked inbox) as for
/// an address K2 doesn't know, and no route opens it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_is_masked_on_every_route_that_reaches_a_person_mailbox() {
    let fx = Fx::start().await;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute(
            "INSERT INTO mail_outbound (id, owner_project_id, agent_name, from_address, to_json, subject, \
             status, created_at, updated_at) VALUES ('out_oa0_person', ?1, 'bot', ?2, '[\"x@y.example\"]', \
             'pat sent this', 'pending', 100, 100)",
            rusqlite::params![fx.ws_id, fx.person_addr],
        )
        .expect("seed outbound row");
    }
    block(|| {
        let p = fx.ws_path.as_str();
        let m = fx.member.as_str();
        let mut failures: Vec<String> = Vec::new();
        let mut table: Vec<String> = Vec::new();
        let mut same = |label: &str, person: Resp, ghost: Resp| {
            let ok = person.status == ghost.status && fx.fold(&person.body) == fx.fold(&ghost.body);
            table.push(format!("{label}: person {} / unknown {}", person.status, ghost.status));
            if !ok {
                failures.push(format!(
                    "{label}: person {} {} — unknown {} {}",
                    person.status, person.body, ghost.status, ghost.body
                ));
            }
        };
        // GET routes naming one inbox or one message.
        for (label, route, key, person, ghost) in [
            ("messages", "messages", "address", fx.person_addr.as_str(), fx.ghost_addr.as_str()),
            ("messages (linked)", "messages", "address", fx.linked_addr.as_str(), fx.ghost_addr.as_str()),
            ("read", "read", "id", fx.person_tok.as_str(), fx.ghost_tok.as_str()),
            ("read (linked)", "read", "id", fx.linked_tok.as_str(), fx.ghost_tok.as_str()),
            ("attachments", "attachments", "id", fx.person_tok.as_str(), fx.ghost_tok.as_str()),
            ("folder/list", "folder/list", "address", fx.person_addr.as_str(), fx.ghost_addr.as_str()),
            ("capabilities", "capabilities", "address", fx.person_addr.as_str(), fx.ghost_addr.as_str()),
            ("outbox", "outbox", "id", "out_oa0_person", "out_oa0_ghost"),
            ("calendar/list", "calendar/list", "address", fx.person_addr.as_str(), fx.ghost_addr.as_str()),
            ("calendar/events", "calendar/events", "address", fx.person_addr.as_str(), fx.ghost_addr.as_str()),
        ] {
            same(
                label,
                fx.get(m, route, &[("project", p), (key, person)]),
                fx.get(m, route, &[("project", p), (key, ghost)]),
            );
        }
        same(
            "wait",
            fx.get(m, "wait", &[("project", p), ("to", &fx.person_addr), ("timeout", "1")]),
            fx.get(m, "wait", &[("project", p), ("to", &fx.ghost_addr), ("timeout", "1")]),
        );
        same(
            "watch/inbox",
            fx.get(m, "watch/inbox", &[("address", &fx.person_addr)]),
            fx.get(m, "watch/inbox", &[("address", &fx.ghost_addr)]),
        );
        // POST routes.
        let tok_routes: [(&str, serde_json::Value); 7] = [
            ("move", serde_json::json!({ "folder": "Archive" })),
            ("flag", serde_json::json!({ "read": true })),
            ("archive", serde_json::json!({})),
            ("delete", serde_json::json!({})),
            ("reply", serde_json::json!({ "body": "hi" })),
            ("forward-message", serde_json::json!({ "to": "dest@else.example" })),
            ("draft", serde_json::json!({ "body": "draft reply" })),
        ];
        for (route, extra) in tok_routes {
            let body = |id: &str| {
                let mut v = serde_json::json!({ "project": p, "id": id });
                for (k, val) in extra.as_object().unwrap() {
                    v[k] = val.clone();
                }
                v
            };
            same(route, fx.post(m, route, body(&fx.person_tok)), fx.post(m, route, body(&fx.ghost_tok)));
        }
        same(
            "draft (linked)",
            fx.post(m, "draft", serde_json::json!({ "project": p, "id": fx.linked_tok, "body": "x" })),
            fx.post(m, "draft", serde_json::json!({ "project": p, "id": fx.ghost_tok, "body": "x" })),
        );
        let addr_routes: [(&str, serde_json::Value); 5] = [
            ("folder/create", serde_json::json!({ "name": "Snoop" })),
            ("folder/rename", serde_json::json!({ "from": "Inbox", "to": "Gone" })),
            ("address/delete", serde_json::json!({})),
            ("send", serde_json::json!({ "to": "x@y.example", "subject": "s", "body": "b" })),
            ("calendar/create", serde_json::json!({ "title": "t", "start": "2026-12-01T10:00:00Z", "end": "2026-12-01T11:00:00Z" })),
        ];
        for (route, extra) in addr_routes {
            let key = if route == "send" { "from" } else { "address" };
            let body = |addr: &str| {
                let mut v = serde_json::json!({ "project": p });
                v[key] = serde_json::json!(addr);
                for (k, val) in extra.as_object().unwrap() {
                    v[k] = val.clone();
                }
                v
            };
            same(route, fx.post(m, route, body(&fx.person_addr)), fx.post(m, route, body(&fx.ghost_addr)));
        }
        same(
            "send (linked)",
            fx.post(m, "send", serde_json::json!({ "project": p, "from": fx.linked_addr, "to": "x@y.example", "subject": "s", "body": "b" })),
            fx.post(m, "send", serde_json::json!({ "project": p, "from": fx.ghost_addr, "to": "x@y.example", "subject": "s", "body": "b" })),
        );
        same(
            "outbox/cancel",
            fx.post(m, "outbox/cancel", serde_json::json!({ "project": p, "id": "out_oa0_person" })),
            fx.post(m, "outbox/cancel", serde_json::json!({ "project": p, "id": "out_oa0_ghost" })),
        );
        same(
            "link/remove",
            fx.post(m, "link/remove", serde_json::json!({ "address": fx.linked_addr })),
            fx.post(m, "link/remove", serde_json::json!({ "address": fx.ghost_addr })),
        );
        same(
            "watch/inboxes",
            fx.post(m, "watch/inboxes", serde_json::json!({ "project": p, "add": [{ "address": fx.person_addr }] })),
            fx.post(m, "watch/inboxes", serde_json::json!({ "project": p, "add": [{ "address": fx.ghost_addr }] })),
        );

        // Lists naming the workspace leave pat's mailbox and the linked
        // inbox out, and keep the agent's.
        for (route, query) in [
            ("messages", vec![("project", p)]),
            ("inboxes", vec![("project", p)]),
            ("capabilities", vec![("project", p)]),
            ("address/list", vec![("project", p)]),
            ("outbox", vec![("project", p)]),
            ("watch", vec![("project", p)]),
        ] {
            let r = fx.get(m, route, &query);
            table.push(format!("{route} (list): {}", r.status));
            if r.status != 200
                || r.body.contains(&fx.person_addr)
                || r.body.contains(&fx.linked_addr)
                || r.body.contains("pat private mail")
                || r.body.contains("pat sent this")
            {
                failures.push(format!("{route} list shows a person's mailbox: {} {}", r.status, r.body));
            }
        }
        let list = fx.get(m, "messages", &[("project", p)]);
        assert!(list.body.contains("agent mail"), "the agent's mail still lists: {}", list.body);
        let inboxes = fx.get(m, "inboxes", &[("project", p)]);
        assert!(inboxes.body.contains(&fx.agent_addr), "the agent's inbox still lists: {}", inboxes.body);

        println!("OA0 route table:\n{}", table.join("\n"));
        assert!(failures.is_empty(), "{} route(s) let a Member reach a person's mailbox:\n{}", failures.len(), failures.join("\n"));
    });
    // Nothing reached pat's mail: no engine call ever named acc-pat.
    let st = fx.fake.state.lock().unwrap();
    assert!(st.updates.iter().all(|(a, _, _)| a != "acc-pat"), "no write on pat's mailbox: {:?}", st.updates);
    assert!(
        st.gets.iter().all(|g| g["accountId"] != "acc-pat"),
        "no read of pat's mailbox: {:?}",
        st.gets
    );
}

/// A Member on the agent's inbox: the list and the read work, and both are
/// logged (the list once per 10 minutes).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_on_an_agent_inbox_still_works_and_is_logged() {
    let fx = Fx::start().await;
    block(|| {
        for _ in 0..2 {
            let r = fx.get(&fx.member, "messages", &[("project", &fx.ws_path), ("address", &fx.agent_addr)]);
            assert_eq!(r.status, 200, "{}", r.body);
            assert_eq!(json(&r)["count"], 1, "{}", r.body);
        }
        let rows = fx.rows_for("user:oa0member", &fx.agent_addr, "messages");
        assert_eq!(rows.len(), 1, "a list is logged once per window: {:?}", fx.audit());
        assert_eq!(rows[0]["outcome"], "ok");
        assert_eq!(rows[0]["status"], 200);
        assert!(rows[0]["ipHint"].as_str().is_some_and(|h| h.starts_with("loopback ")), "{:?}", rows[0]);
        let r = fx.read(&fx.member, &fx.agent_tok);
        assert_eq!(r.status, 200, "{}", r.body);
        let r = fx.read(&fx.member, &fx.agent_tok);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(fx.rows_for("user:oa0member", &fx.agent_addr, "read").len(), 2, "every open is its own row");
    });
}

/// The owner token, an Owner login and an Admin login open pat's mailbox;
/// each open is logged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_and_admin_open_a_person_mailbox_and_are_logged() {
    let fx = Fx::start().await;
    block(|| {
        for (token, actor, role) in [
            (OWNER, "owner-token", "owner"),
            (fx.owner_login.as_str(), "user:oa0owner", "owner"),
            (fx.admin.as_str(), "user:oa0admin", "admin"),
        ] {
            let r = fx.read(token, &fx.person_tok);
            assert_eq!(r.status, 200, "{actor}: {}", r.body);
            assert_eq!(json(&r)["message"]["subject"], "pat private mail", "{actor}");
            let rows = fx.rows_for(actor, &fx.person_addr, "read");
            assert_eq!(rows.len(), 1, "{actor}: {:?}", fx.audit());
            assert_eq!(rows[0]["outcome"], "ok", "{actor}");
            assert_eq!(rows[0]["kind"], "person", "{actor}");
            assert_eq!(rows[0]["role"], role, "{actor}");
            assert_eq!(rows[0]["own"], 0, "{actor}");
            assert_eq!(rows[0]["status"], 200, "{actor}");
            let list = fx.get(token, "messages", &[("project", &fx.ws_path), ("address", &fx.person_addr)]);
            assert_eq!(list.status, 200, "{actor}: {}", list.body);
        }
        let inboxes = fx.get(&fx.admin, "inboxes", &[("project", &fx.ws_path)]);
        assert!(inboxes.body.contains(&fx.person_addr), "an Admin sees pat's mailbox: {}", inboxes.body);
    });
}

/// pat (a Member) opens their own mailbox; it's logged as their own.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_person_opens_their_own_mailbox() {
    let fx = Fx::start().await;
    block(|| {
        let r = fx.read(&fx.pat, &fx.person_tok);
        assert_eq!(r.status, 200, "pat reads their own mail: {}", r.body);
        assert_eq!(json(&r)["message"]["subject"], "pat private mail");
        let rows = fx.rows_for("user:pat", &fx.person_addr, "read");
        assert_eq!(rows.len(), 1, "{:?}", fx.audit());
        assert_eq!(rows[0]["own"], 1);
        assert_eq!(rows[0]["outcome"], "ok");
        let list = fx.get(&fx.pat, "messages", &[("project", &fx.ws_path), ("address", &fx.person_addr)]);
        assert_eq!(list.status, 200, "{}", list.body);
        // Someone else's person mailbox stays shut to pat: the linked one
        // has no person on record.
        let linked = fx.read(&fx.pat, &fx.linked_tok);
        let ghost = fx.read(&fx.pat, &fx.ghost_tok);
        assert_eq!(fx.fold(&linked.body), fx.fold(&ghost.body), "{} / {}", linked.body, ghost.body);
    });
}

/// No log, no access: with the log table gone, a read answers 503 and the
/// engine is never asked for the message.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_log_means_no_access() {
    let fx = Fx::start().await;
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        conn.execute_batch("DROP TABLE mail_reader_audit;").expect("drop the access log");
    }
    block(|| {
        let before = fx.email_gets();
        for token in [fx.member.as_str(), OWNER, fx.admin.as_str()] {
            let r = fx.read(token, &fx.agent_tok);
            assert_eq!(r.status, 503, "{}", r.body);
            assert_eq!(json(&r)["error"]["code"], "audit_unavailable", "{}", r.body);
            let l = fx.get(token, "messages", &[("project", &fx.ws_path)]);
            assert_eq!(l.status, 503, "{}", l.body);
        }
        assert_eq!(fx.email_gets(), before, "nothing was fetched");
    });
}

/// A read by anyone but the mailbox's own person or agent doesn't mark the
/// mail read; the person's own read and the agent's own read do.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reads_by_others_do_not_mark_mail_read() {
    let fx = Fx::start().await;
    block(|| {
        for token in [fx.member.as_str(), OWNER, fx.admin.as_str()] {
            let r = fx.read(token, &fx.agent_tok);
            assert_eq!(r.status, 200, "{}", r.body);
            assert_eq!(json(&r)["markedRead"], false, "{}", r.body);
        }
        assert_eq!(fx.seen_updates("acc-agent"), 0, "a person reading the agent's mail leaves it unread");
        for token in [OWNER, fx.admin.as_str(), fx.owner_login.as_str()] {
            let r = fx.read(token, &fx.person_tok);
            assert_eq!(r.status, 200, "{}", r.body);
        }
        assert_eq!(fx.seen_updates("acc-pat"), 0, "an Owner or Admin reading pat's mail leaves it unread");

        let r = fx.read(&fx.pat, &fx.person_tok);
        assert_eq!(r.status, 200, "{}", r.body);
        assert_eq!(json(&r)["markedRead"], true, "{}", r.body);
        assert_eq!(fx.seen_updates("acc-pat"), 1, "pat's own read marks it read");

        let sid = k2_core::session::SessionId::new();
        let passport = crate::session_token::mint_session_token(
            &sid,
            &sid.to_string(),
            crate::session_token::HookPrincipal {
                workspace_uuid: fx.ws_id.clone(),
                agent_address: "oa0-agent".to_string(),
            },
            crate::session_token::CredMode::ApiKey,
            crate::session_token::Provider::Anthropic,
        );
        let rows_before = fx.audit().len();
        let r = fx.get(&passport, "read", &[("id", &fx.agent_tok)]);
        assert_eq!(r.status, 200, "the agent reads its own mail: {}", r.body);
        assert_eq!(fx.seen_updates("acc-agent"), 1, "the agent's own read marks it read");
        assert_eq!(fx.audit().len(), rows_before, "an agent on its own workspace isn't logged: {:?}", fx.audit());
        // Agents keep their rule: a person's mailbox stays a person's call.
        let watch = fx.post(&passport, "watch/inboxes", serde_json::json!({ "add": [{ "address": fx.person_addr }] }));
        assert_eq!(watch.status, 403, "{}", watch.body);
    });
}

/// A Connect login through the tunnel (how a remote login arrives) is held
/// to the same rule, and its row says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_login_through_the_tunnel_is_held_to_the_same_rule() {
    let fx = Fx::start().await;
    block(|| {
        let tunnel = fx.d.tunnel_port;
        let person = fx.get_on(tunnel, &fx.member, "read", &[("project", &fx.ws_path), ("id", &fx.person_tok)]);
        let ghost = fx.get_on(tunnel, &fx.member, "read", &[("project", &fx.ws_path), ("id", &fx.ghost_tok)]);
        assert_eq!(person.status, 404, "{}", person.body);
        assert_eq!(fx.fold(&person.body), fx.fold(&ghost.body));
        let agent = fx.get_on(tunnel, &fx.member, "read", &[("project", &fx.ws_path), ("id", &fx.agent_tok)]);
        assert_eq!(agent.status, 200, "{}", agent.body);
        let rows = fx.rows_for("user:oa0member", &fx.agent_addr, "read");
        assert_eq!(rows.len(), 1, "{:?}", fx.audit());
        assert!(rows[0]["ipHint"].as_str().is_some_and(|h| h.starts_with("tunnel ")), "{:?}", rows[0]);
        assert!(fx.domain.ends_with(".example"));
    });
}
