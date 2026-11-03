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
                "Mailbox/query" => serde_json::json!({ "ids": ["mb-drafts"] }),
                "Email/set" => {
                    let create = args["create"].as_object().expect("Email/set create");
                    let mut created = serde_json::Map::new();
                    for (k, v) in create {
                        st.created.push(v.clone());
                        created.insert(k.clone(), serde_json::json!({ "id": "e-new" }));
                    }
                    serde_json::json!({ "created": created })
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
