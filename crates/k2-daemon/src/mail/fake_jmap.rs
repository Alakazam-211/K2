//! A fake mail engine for tests: a loopback HTTP server that answers the JMAP
//! the send path uses, routed by what each request asks (the sequential
//! `jmap::tests::spawn_mock_server` can't, since one submit makes a varying
//! number of calls). It stores uploads and answers downloads from them, so a
//! round trip through [`crate::mail::jmap::StalwartClient`] sees real bytes.
//! No real mail server, no real mail. tests/mail/stalwart-attachments-probe.py
//! runs the same shapes on a real (throwaway) Stalwart.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::{Arc, Mutex};

use crate::mail::jmap::StalwartClient;

#[derive(Default)]
pub(crate) struct FakeState {
    /// Blob id → bytes (uploads, and blobs a test seeds).
    pub blobs: HashMap<String, Vec<u8>>,
    /// The bytes of each upload, in order.
    pub uploads: Vec<Vec<u8>>,
    /// Stored messages `Email/get` answers (id → the JMAP Email object).
    pub emails: HashMap<String, serde_json::Value>,
    /// Each `Email/set` create object the engine was handed.
    pub created: Vec<serde_json::Value>,
    /// Each `EmailSubmission/set` envelope.
    pub envelopes: Vec<serde_json::Value>,
    /// The arguments of each `Email/get`.
    pub gets: Vec<serde_json::Value>,
    /// Answer every upload with this HTTP status instead of storing it.
    pub upload_status: Option<u16>,
    /// Each `Email/set` update (`accountId`, email id, the patch): marking
    /// read lands here.
    pub updates: Vec<(String, String, serde_json::Value)>,
    /// Account id → the email ids `Email/query` lists for it, newest first.
    pub inbox: HashMap<String, Vec<String>>,
    /// Each email id an `Email/set destroy` removed.
    pub destroyed: Vec<String>,
    /// `(account, mailbox id)` → the ids created into that mailbox, oldest
    /// first. An `Email/query` naming a mailbox with entries here lists
    /// them (newest first); any other mailbox lists `inbox`.
    pub members: HashMap<(String, String), Vec<String>>,
}

pub(crate) struct FakeJmap {
    pub state: Arc<Mutex<FakeState>>,
    pub port: u16,
}

impl FakeJmap {
    pub fn start() -> FakeJmap {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("addr").port();
        let state = Arc::new(Mutex::new(FakeState::default()));
        let st = state.clone();
        std::thread::spawn(move || {
            for sock in listener.incoming() {
                let Ok(sock) = sock else { return };
                let st = st.clone();
                std::thread::spawn(move || serve(sock, &st));
            }
        });
        FakeJmap { state, port }
    }

    pub fn client(&self) -> StalwartClient {
        StalwartClient::new(format!("http://127.0.0.1:{}", self.port), "k2-test-key")
    }
}

fn serve(mut sock: std::net::TcpStream, state: &Mutex<FakeState>) {
    let mut raw = Vec::new();
    let mut buf = [0u8; 65536];
    let (head_end, want) = loop {
        let n = match sock.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        raw.extend_from_slice(&buf[..n]);
        if let Some(h) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&raw[..h]).to_string();
            let want: usize = head
                .lines()
                .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
                .unwrap_or(0);
            break (h, want);
        }
    };
    while raw.len() < head_end + 4 + want {
        let n = match sock.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        raw.extend_from_slice(&buf[..n]);
    }
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let body = &raw[head_end + 4..];
    let mut first = head.lines().next().unwrap_or("").split(' ');
    let method = first.next().unwrap_or("");
    let path = first.next().unwrap_or("");
    let (status, reply) = route(method, path, body, state);
    let reason = if status == 200 { "OK" } else { "Error" };
    let _ = sock.write_all(
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            reply.len()
        )
        .as_bytes(),
    );
    let _ = sock.write_all(&reply);
}

fn route(method: &str, path: &str, body: &[u8], state: &Mutex<FakeState>) -> (u16, Vec<u8>) {
    let mut st = state.lock().unwrap_or_else(|p| p.into_inner());
    if method == "GET" && path.starts_with("/jmap/session") {
        let session = serde_json::json!({
            "capabilities": { "urn:ietf:params:jmap:core": { "maxSizeUpload": 50_000_000 } },
            "accounts": { "b": { "name": "admin@fake.test" } },
            "primaryAccounts": { "urn:ietf:params:jmap:mail": "b", "urn:stalwart:jmap": "b" },
            "username": "admin@fake.test",
            "apiUrl": "/jmap/",
            "uploadUrl": "/jmap/upload/{accountId}/",
            "downloadUrl": "/jmap/download/{accountId}/{blobId}/{name}?accept={type}",
            "state": "1",
        });
        return (200, session.to_string().into_bytes());
    }
    if method == "POST" && path.starts_with("/jmap/upload/") {
        if let Some(code) = st.upload_status {
            return (code, br#"{"type":"refused"}"#.to_vec());
        }
        let id = format!("up-{}", st.uploads.len() + 1);
        st.uploads.push(body.to_vec());
        st.blobs.insert(id.clone(), body.to_vec());
        return (200, serde_json::json!({ "blobId": id }).to_string().into_bytes());
    }
    if method == "GET" && path.starts_with("/jmap/download/") {
        let blob = path.split('/').nth(4).unwrap_or("");
        return match st.blobs.get(blob) {
            Some(b) => (200, b.clone()),
            None => (404, b"no such blob".to_vec()),
        };
    }
    if method == "POST" && path.starts_with("/jmap/") {
        let req: serde_json::Value = serde_json::from_slice(body).expect("JMAP request JSON");
        let calls = req["methodCalls"].as_array().expect("methodCalls");
        let mut out = Vec::new();
        for call in calls {
            let name = call[0].as_str().unwrap_or("");
            let args = &call[1];
            let tag = call[2].clone();
            let answer = match name {
                // No identity yet: the client creates one (its fallback path).
                "Identity/get" => serde_json::json!({ "list": [] }),
                "Identity/set" => serde_json::json!({ "created": { "k2": { "id": "ident-1" } } }),
                // By role, as Stalwart answers (`mb-<role>`); no role named
                // → the drafts mailbox (the submit path's).
                "Mailbox/query" => {
                    let role = args["filter"]["role"].as_str().unwrap_or("drafts");
                    serde_json::json!({ "ids": [format!("mb-{role}")] })
                }
                // The folder list (the mail reader's folders).
                "Mailbox/get" => serde_json::json!({ "list": [
                    { "id": "mb-inbox", "name": "Inbox", "role": "inbox" },
                    { "id": "mb-receipts", "name": "Receipts", "role": null },
                    { "id": "mb-sent", "name": "Sent", "role": "sent" },
                    { "id": "mb-drafts", "name": "Drafts", "role": "drafts" },
                    { "id": "mb-junk", "name": "Junk", "role": "junk" },
                    { "id": "mb-trash", "name": "Trash", "role": "trash" },
                    { "id": "mb-archive", "name": "Archive", "role": "archive" },
                ] }),
                "Email/query" => {
                    let account = args["accountId"].as_str().unwrap_or("");
                    let mailbox = args["filter"]["inMailbox"].as_str().unwrap_or("");
                    let ids: Vec<String> = match st.members.get(&(account.to_string(), mailbox.to_string())) {
                        Some(m) => m.iter().rev().cloned().collect(),
                        None => st.inbox.get(account).cloned().unwrap_or_default(),
                    };
                    serde_json::json!({ "ids": ids, "position": 0 })
                }
                "Email/set" => {
                    let account = args["accountId"].as_str().unwrap_or("").to_string();
                    let mut answer = serde_json::json!({});
                    if let Some(update) = args.get("update").and_then(|u| u.as_object()) {
                        let mut updated = serde_json::Map::new();
                        for (id, patch) in update {
                            st.updates.push((account.clone(), id.clone(), patch.clone()));
                            updated.insert(id.clone(), serde_json::Value::Null);
                        }
                        answer["updated"] = serde_json::Value::Object(updated);
                    }
                    if let Some(create) = args.get("create").and_then(|c| c.as_object()) {
                        let mut created = serde_json::Map::new();
                        for (k, v) in create {
                            st.created.push(v.clone());
                            // The first id stays `e-new` (older tests name it).
                            let id = match st.created.len() {
                                1 => "e-new".to_string(),
                                n => format!("e-new-{n}"),
                            };
                            let stored = stored_email(&st, &id, v);
                            let boxes: Vec<String> =
                                v["mailboxIds"].as_object().map(|m| m.keys().cloned().collect()).unwrap_or_default();
                            for mailbox in boxes {
                                st.members.entry((account.clone(), mailbox)).or_default().push(id.clone());
                            }
                            st.emails.insert(id.clone(), stored);
                            created.insert(k.clone(), serde_json::json!({ "id": id }));
                        }
                        answer["created"] = serde_json::Value::Object(created);
                    }
                    if let Some(destroy) = args.get("destroy").and_then(|d| d.as_array()) {
                        let mut gone = Vec::new();
                        let mut missing = serde_json::Map::new();
                        for id in destroy.iter().filter_map(|i| i.as_str()) {
                            if st.emails.remove(id).is_some() {
                                st.destroyed.push(id.to_string());
                                for members in st.members.values_mut() {
                                    members.retain(|m| m != id);
                                }
                                gone.push(serde_json::json!(id));
                            } else {
                                missing.insert(id.to_string(), serde_json::json!({ "type": "notFound" }));
                            }
                        }
                        answer["destroyed"] = serde_json::Value::Array(gone);
                        if !missing.is_empty() {
                            answer["notDestroyed"] = serde_json::Value::Object(missing);
                        }
                    }
                    answer
                }
                "EmailSubmission/set" => {
                    let create = args["create"].as_object().expect("EmailSubmission/set create");
                    let mut created = serde_json::Map::new();
                    for (k, v) in create {
                        st.envelopes.push(v["envelope"].clone());
                        created.insert(k.clone(), serde_json::json!({ "id": "s-new" }));
                    }
                    serde_json::json!({ "created": created })
                }
                "Email/get" => {
                    st.gets.push(args.clone());
                    let mut list = Vec::new();
                    let mut not_found = Vec::new();
                    for id in args["ids"].as_array().expect("Email/get ids") {
                        let id = id.as_str().unwrap_or("");
                        match st.emails.get(id) {
                            Some(e) => list.push(e.clone()),
                            None => not_found.push(id.to_string()),
                        }
                    }
                    serde_json::json!({ "list": list, "notFound": not_found })
                }
                other => panic!("fake JMAP: unexpected method {other}"),
            };
            out.push(serde_json::json!([name, answer, tag]));
        }
        return (200, serde_json::json!({ "methodResponses": out }).to_string().into_bytes());
    }
    (404, format!("fake JMAP: no route {method} {path}").into_bytes())
}

/// What `Email/get` serves for an Email the engine was asked to create: the
/// create object plus what a server adds. Its thread is the thread of the
/// message its `In-Reply-To` names (as Stalwart threads a reply), else its own.
fn stored_email(st: &FakeState, id: &str, create: &serde_json::Value) -> serde_json::Value {
    let mut e = create.clone();
    let irt = create["header:In-Reply-To:asText"].as_str().unwrap_or("").trim().to_string();
    let thread = st
        .emails
        .values()
        .find(|o| !irt.is_empty() && o["header:Message-ID:asText"].as_str().map(str::trim) == Some(irt.as_str()))
        .and_then(|o| o["threadId"].as_str().map(str::to_string))
        .unwrap_or_else(|| format!("t-{id}"));
    e["id"] = serde_json::json!(id);
    e["threadId"] = serde_json::json!(thread);
    e["receivedAt"] = serde_json::json!(format!("2026-11-10T12:00:{:02}Z", st.created.len() % 60));
    e["hasAttachment"] = serde_json::json!(false);
    for (key, empty) in [
        ("keywords", serde_json::json!({})),
        ("htmlBody", serde_json::json!([])),
        ("textBody", serde_json::json!([])),
        ("bodyValues", serde_json::json!({})),
        ("cc", serde_json::json!([])),
        ("header:References:asText", serde_json::Value::Null),
    ] {
        if e.get(key).is_none() {
            e[key] = empty;
        }
    }
    e["attachments"] = serde_json::json!([]);
    e["blobId"] = serde_json::json!(format!("blob-{id}"));
    e["header:Message-ID:asText"] = serde_json::json!(format!("<{id}@fake.test>"));
    e["header:Authentication-Results:asText:all"] = serde_json::json!([]);
    e
}
