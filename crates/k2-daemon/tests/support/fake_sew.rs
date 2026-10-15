//! Fake Sew mgmt server for the S27 grant-hook tests (§20.1).
//!
//! A `tokio::net::UnixListener` in a temp dir speaking HTTP/1.1, built from
//! the wire types in `k2_core::tool_hook::sew_wire` (the TEMP mirror of
//! `sew-proto`; when K1 vendors `sew-proto`, swap the imports). Every
//! request body is parsed with the `deny_unknown_fields` request type and
//! validated; a parse/validate failure answers `400 validation_failed` AND
//! is recorded in `bad_bodies` so the test fails loudly.
//!
//! Semantics encoded (contracts rev 13 + S27-A5..A9, HC2):
//! - `Authorization: Bearer <token>` (else 401), `Idempotency-Key` on every
//!   POST with the key grammar (else 400).
//! - Idempotency map: same key + same body → stored response with
//!   `Idempotent-Replayed: true`; same key + other body → 409
//!   `idempotency_conflict`.
//! - provision = upsert of state (reactivates; converts agent kind, A9);
//!   username unique among ACTIVE members → 409 `conflict`,
//!   `details.field = "profile.username"`, `reason = "username_taken"` (A6);
//!   assistant whose person is missing → 422 `assistant_person_unknown`,
//!   deactivated → 422 `assistant_person_inactive` (A7).
//! - deprovision of an unknown id → 404; of the owner → 422 `is_owner`;
//!   deactivating a person deactivates assistants pinned to them.
//! - owner/set naming the current owner → 200 `changed:false` (G6).
//! - provision is DECLARATIVE (A8, live Sew): absent optional profile
//!   fields (`email`, `title`, `pronouns`) are cleared; `update` takes a
//!   ProfilePatch (omitted = kept).
//! - `GET /mgmt/v1/principals` filters by `external_id` and carries
//!   `email`/`title`/`pronouns` (SW14) unless `set_list_profile_fields(false)`
//!   (a pre-SW14 Sew). Provision has no `skipped` field (HC2 is on member-add).
//! - Unknown paths → 404 and recorded in `bad_paths`.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinHandle;

use k2_core::tool_hook::sew_wire::{
    self, AgentKind, DeprovisionRequest, ExternalId, MemberKind, OwnerSetRequest, Patch,
    ProvisionRequest, UpdatePrincipalRequest,
};

#[derive(Debug, Clone)]
pub struct Member {
    pub member_id: u64,
    pub external_id: String,
    pub kind: MemberKind,
    pub agent_kind: Option<AgentKind>,
    pub assistant_of: Option<String>,
    pub guest: bool,
    pub username: String,
    pub display_name: String,
    pub email: Option<String>,
    pub title: Option<String>,
    pub pronouns: Option<String>,
    pub deactivated: bool,
}

#[derive(Debug, Clone)]
pub struct Call {
    pub method: String,
    pub path: String,
    pub key: Option<String>,
    pub actor: Option<String>,
    pub body: Value,
    pub status: u16,
    pub replayed: bool,
}

#[derive(Debug, Clone)]
enum Fault {
    Fail {
        status: u16,
        code: String,
        reason: String,
    },
    ApplyThenDrop,
    RateLimit {
        secs: u64,
    },
}

#[derive(Default)]
struct State {
    token: String,
    install_id: String,
    provider: String,
    members: BTreeMap<String, Member>,
    next_id: u64,
    owner: Option<String>,
    idem: HashMap<String, (String, u16, String)>,
    calls: Vec<Call>,
    faults: VecDeque<Fault>,
    /// Pre-SW14 Sew when false: the list omits email/title/pronouns.
    list_profile_fields: bool,
    bad_paths: Vec<String>,
    bad_bodies: Vec<String>,
}

pub struct FakeSew {
    pub socket: PathBuf,
    pub token_file: PathBuf,
    state: Arc<Mutex<State>>,
    task: Option<JoinHandle<()>>,
}

impl FakeSew {
    /// Start under `dir` (keep it short: `SUN_LEN`).
    pub async fn start(dir: &Path) -> FakeSew {
        std::fs::create_dir_all(dir).expect("fake sew dir");
        let socket = dir.join("mgmt.sock");
        let token_file = dir.join("mgmt.token");
        std::fs::write(&token_file, "fake-mgmt-token\n").expect("token file");
        let state = Arc::new(Mutex::new(State {
            token: "fake-mgmt-token".into(),
            install_id: "1001".into(),
            provider: "k2".into(),
            next_id: 1,
            list_profile_fields: true,
            ..Default::default()
        }));
        let mut f = FakeSew {
            socket,
            token_file,
            state,
            task: None,
        };
        f.up();
        f
    }

    /// (Re)bind the socket.
    pub fn up(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let listener = UnixListener::bind(&self.socket).expect("bind fake sew");
        let state = self.state.clone();
        self.task = Some(tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else {
                    return;
                };
                let state = state.clone();
                tokio::spawn(async move {
                    serve(stream, state).await;
                });
            }
        }));
    }

    /// Stop listening and remove the socket (Sew down).
    pub fn down(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        let _ = std::fs::remove_file(&self.socket);
    }

    fn with<T>(&self, f: impl FnOnce(&mut State) -> T) -> T {
        f(&mut self.state.lock().unwrap())
    }

    pub fn fail_next(&self, n: usize, status: u16, code: &str, reason: &str) {
        self.with(|s| {
            for _ in 0..n {
                s.faults.push_back(Fault::Fail {
                    status,
                    code: code.into(),
                    reason: reason.into(),
                });
            }
        })
    }
    pub fn apply_then_drop_connection_next(&self) {
        self.with(|s| s.faults.push_back(Fault::ApplyThenDrop))
    }
    pub fn rate_limit_next(&self, secs: u64) {
        self.with(|s| s.faults.push_back(Fault::RateLimit { secs }))
    }
    pub fn set_install_id(&self, id: &str) {
        self.with(|s| s.install_id = id.into())
    }
    pub fn set_provider(&self, p: &str) {
        self.with(|s| s.provider = p.into())
    }
    pub fn set_token(&self, t: &str) {
        self.with(|s| s.token = t.into())
    }
    pub fn set_list_profile_fields(&self, on: bool) {
        self.with(|s| s.list_profile_fields = on)
    }
    /// A person edits their Sew-local profile (or an admin sets an agent's
    /// email in Sew).
    pub fn set_sew_profile(
        &self,
        ext: &str,
        email: Option<&str>,
        title: Option<&str>,
        pronouns: Option<&str>,
    ) {
        self.with(|s| {
            let m = s.members.get_mut(ext).expect("member to edit");
            m.email = email.map(str::to_string);
            m.title = title.map(str::to_string);
            m.pronouns = pronouns.map(str::to_string);
        })
    }
    /// Wipe members (a fresh install: new id, no owner).
    pub fn reinstall(&self, id: &str) {
        self.with(|s| {
            s.install_id = id.into();
            s.members.clear();
            s.owner = None;
            s.idem.clear();
        })
    }
    /// Seed a member directly (drift fixtures).
    pub fn seed(&self, ext: &str, kind: MemberKind, username: &str, display: &str, guest: bool) {
        self.with(|s| {
            let id = s.next_id;
            s.next_id += 1;
            s.members.insert(
                ext.into(),
                Member {
                    member_id: id,
                    external_id: ext.into(),
                    kind,
                    agent_kind: (kind == MemberKind::Agent).then_some(AgentKind::Coworker),
                    assistant_of: None,
                    guest,
                    username: username.into(),
                    display_name: display.into(),
                    email: None,
                    title: None,
                    pronouns: None,
                    deactivated: false,
                },
            );
        })
    }
    pub fn set_display_name(&self, ext: &str, name: &str) {
        self.with(|s| {
            if let Some(m) = s.members.get_mut(ext) {
                m.display_name = name.into();
            }
        })
    }

    /// Edit members directly (drift fixtures).
    pub fn with_members(&self, f: impl FnOnce(&mut BTreeMap<String, Member>)) {
        self.with(|s| f(&mut s.members))
    }

    pub fn member(&self, ext: &str) -> Option<Member> {
        self.with(|s| s.members.get(ext).cloned())
    }
    pub fn members(&self) -> Vec<Member> {
        self.with(|s| s.members.values().cloned().collect())
    }
    pub fn owner(&self) -> Option<String> {
        self.with(|s| s.owner.clone())
    }
    pub fn calls(&self) -> Vec<Call> {
        self.with(|s| s.calls.clone())
    }
    pub fn posts(&self) -> Vec<Call> {
        self.calls()
            .into_iter()
            .filter(|c| c.method == "POST")
            .collect()
    }
    /// POSTs that changed state (not replays, not faults).
    pub fn applied(&self, path: &str, ext: &str) -> usize {
        self.calls()
            .iter()
            .filter(|c| {
                c.method == "POST"
                    && c.path == path
                    && !c.replayed
                    && (200..300).contains(&c.status)
                    && c.body["external_id"] == ext
            })
            .count()
    }
    /// The full call log, for failure messages.
    pub fn log(&self) -> String {
        self.calls()
            .iter()
            .map(|c| {
                format!(
                    "{} {} {} key={:?} actor={:?} replayed={} {}",
                    c.method, c.path, c.status, c.key, c.actor, c.replayed, c.body
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    /// Fail loudly on any request the real Sew would not take.
    pub fn assert_clean(&self) {
        self.with(|s| {
            assert!(
                s.bad_paths.is_empty(),
                "unknown mgmt paths: {:?}",
                s.bad_paths
            );
            assert!(
                s.bad_bodies.is_empty(),
                "bodies sew-proto refuses: {:?}",
                s.bad_bodies
            );
        })
    }

    /// Bounded wait (≤ 5 s) for `pred` over the fake's state.
    pub async fn wait_for(&self, what: &str, pred: impl Fn(&FakeSew) -> bool) {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(5) {
            if pred(self) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!(
            "timed out waiting for {what}; fake Sew call log:\n{}",
            self.log()
        );
    }
}

impl Drop for FakeSew {
    fn drop(&mut self) {
        self.down();
    }
}

fn err(status: u16, code: &str, hint: &str, details: Value) -> (u16, String) {
    (
        status,
        json!({"ok": false, "error": {"code": code, "hint": hint, "details": details}}).to_string(),
    )
}

fn sha(s: &str) -> String {
    format!("{:x}", Sha256::digest(s.as_bytes()))
}

async fn serve(mut stream: UnixStream, state: Arc<Mutex<State>>) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    let (head, body) = loop {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            let len = header(&head, "content-length")
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            while buf.len() < end + 4 + len {
                match stream.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => buf.extend_from_slice(&chunk[..n]),
                }
            }
            break (
                head,
                String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).to_string(),
            );
        }
    };
    let mut lines = head.split("\r\n");
    let req = lines.next().unwrap_or_default().to_string();
    let mut parts = req.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    let (path, query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let (path, query) = (path.to_string(), query.to_string());
    let key = header(&head, "idempotency-key");
    let actor = header(&head, "sew-actor");
    let auth = header(&head, "authorization");

    let (status, out, extra, drop_conn) = {
        let mut s = state.lock().unwrap();
        handle(
            &mut s,
            &method,
            &path,
            &query,
            auth.as_deref(),
            key.as_deref(),
            actor.as_deref(),
            &body,
        )
    };
    if drop_conn {
        return; // applied, but the response is lost
    }
    let resp = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nConnection: close\r\n{extra}Content-Length: {}\r\n\r\n{out}",
        out.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

fn header(head: &str, name: &str) -> Option<String> {
    head.lines().skip(1).find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim()
            .eq_ignore_ascii_case(name)
            .then(|| v.trim().to_string())
    })
}

#[allow(clippy::too_many_arguments)]
fn handle(
    s: &mut State,
    method: &str,
    path: &str,
    query: &str,
    auth: Option<&str>,
    key: Option<&str>,
    actor: Option<&str>,
    body: &str,
) -> (u16, String, String, bool) {
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let record = |s: &mut State, status: u16, replayed: bool| {
        s.calls.push(Call {
            method: method.into(),
            path: path.into(),
            key: key.map(str::to_string),
            actor: actor.map(str::to_string),
            body: parsed.clone(),
            status,
            replayed,
        });
    };
    if auth != Some(&format!("Bearer {}", s.token)) {
        let (st, b) = err(401, "unauthenticated", "bad mgmt token", json!({}));
        record(s, st, false);
        return (st, b, String::new(), false);
    }
    if method == "GET" {
        let (st, b) = match path {
            sew_wire::MGMT_STATUS => (
                200,
                json!({
                    "version": "0.0.0-fake", "schema_version": 1, "install_id": s.install_id,
                    "provider": s.provider, "db": {}, "jobs": {}, "search": {}, "keys": {},
                    "sessions": 0, "sockets": 0
                })
                .to_string(),
            ),
            sew_wire::MGMT_PRINCIPALS => list(s, query),
            _ => {
                s.bad_paths.push(format!("GET {path}"));
                err(
                    404,
                    "not_found",
                    "unknown route",
                    json!({"reason": "unknown_route"}),
                )
            }
        };
        record(s, st, false);
        return (st, b, String::new(), false);
    }
    // POST
    let Some(key) = key.filter(|k| sew_wire::is_idempotency_key(k)) else {
        let (st, b) = err(400, "bad_request", "Idempotency-Key required", json!({}));
        record(s, st, false);
        return (st, b, String::new(), false);
    };
    if let Some(a) = actor {
        assert!(
            sew_wire::is_sew_actor(a),
            "Sew-Actor {a:?} breaks the grammar"
        );
    }
    let hash = sha(body);
    if let Some((h, st, b)) = s.idem.get(key).cloned() {
        if h == hash {
            record(s, st, true);
            return (st, b, "Idempotent-Replayed: true\r\n".into(), false);
        }
        let (st, b) = err(
            409,
            "idempotency_conflict",
            "key reused with another body",
            json!({}),
        );
        record(s, st, false);
        return (st, b, String::new(), false);
    }
    let mut drop_conn = false;
    match s.faults.pop_front() {
        Some(Fault::Fail {
            status,
            code,
            reason,
        }) => {
            let (st, b) = err(status, &code, "injected", json!({"reason": reason}));
            record(s, st, false);
            return (st, b, String::new(), false);
        }
        Some(Fault::RateLimit { secs }) => {
            let (st, b) = err(
                429,
                "rate_limited",
                "slow down",
                json!({"retry_after_ms": secs * 1000}),
            );
            record(s, st, false);
            return (st, b, format!("Retry-After: {secs}\r\n"), false);
        }
        Some(Fault::ApplyThenDrop) => drop_conn = true,
        None => {}
    }
    let (st, b) = match path {
        sew_wire::MGMT_PROVISION => provision(s, body),
        sew_wire::MGMT_UPDATE => update(s, body),
        sew_wire::MGMT_DEPROVISION => deprovision(s, body),
        sew_wire::MGMT_OWNER_SET => owner_set(s, body),
        _ => {
            s.bad_paths.push(format!("POST {path}"));
            err(
                404,
                "not_found",
                "unknown route",
                json!({"reason": "unknown_route"}),
            )
        }
    };
    if (200..300).contains(&st) || st == 404 || st == 409 || st == 422 {
        s.idem.insert(key.to_string(), (hash, st, b.clone()));
    }
    record(s, st, false);
    (st, b, String::new(), drop_conn)
}

fn bad_body(s: &mut State, what: &str, e: impl std::fmt::Display, body: &str) -> (u16, String) {
    s.bad_bodies.push(format!("{what}: {e}: {body}"));
    err(400, "validation_failed", &e.to_string(), json!({}))
}

fn active_username_holder<'a>(s: &'a State, username: &str, except: &str) -> Option<&'a Member> {
    s.members
        .values()
        .find(|m| !m.deactivated && m.username == username && m.external_id != except)
}

fn provision(s: &mut State, body: &str) -> (u16, String) {
    let req: ProvisionRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "provision", e, body),
    };
    if let Err(e) = sew_wire::validate_provision(&req) {
        return bad_body(s, "provision", e, body);
    }
    if !req.external_id.is_known() {
        return bad_body(s, "provision", "unknown external id form", body);
    }
    let ext = req.external_id.to_string();
    if let Some(person) = &req.assistant_of {
        match s.members.get(&person.to_string()) {
            None => {
                return err(
                    422,
                    "invalid_state",
                    "person not provisioned",
                    json!({"reason": "assistant_person_unknown", "field": "assistant_of"}),
                )
            }
            Some(p) if p.deactivated => {
                return err(
                    422,
                    "invalid_state",
                    "person deactivated",
                    json!({"reason": "assistant_person_inactive", "field": "assistant_of"}),
                )
            }
            Some(_) => {}
        }
    }
    if active_username_holder(s, &req.profile.username, &ext).is_some() {
        return err(
            409,
            "conflict",
            "username taken",
            json!({"field": "profile.username", "reason": "username_taken"}),
        );
    }
    let (created, reactivated);
    let id = match s.members.get_mut(&ext) {
        Some(m) => {
            created = false;
            reactivated = m.deactivated;
            m.deactivated = false;
            m.guest = req.guest;
            m.agent_kind = req.agent_kind;
            m.assistant_of = req.assistant_of.as_ref().map(ExternalId::to_string);
            m.username = req.profile.username.clone();
            m.display_name = req.profile.display_name.clone();
            // Declarative: absent clears (live Sew, S9 §6.4 step 4).
            m.email = req.profile.email.clone();
            m.title = req.profile.title.clone();
            m.pronouns = req.profile.pronouns.clone();
            m.member_id
        }
        None => {
            created = true;
            reactivated = false;
            let id = s.next_id;
            s.next_id += 1;
            s.members.insert(
                ext.clone(),
                Member {
                    member_id: id,
                    external_id: ext.clone(),
                    kind: req.kind,
                    agent_kind: req.agent_kind,
                    assistant_of: req.assistant_of.as_ref().map(ExternalId::to_string),
                    guest: req.guest,
                    username: req.profile.username.clone(),
                    display_name: req.profile.display_name.clone(),
                    email: req.profile.email.clone(),
                    title: req.profile.title.clone(),
                    pronouns: req.profile.pronouns.clone(),
                    deactivated: false,
                },
            );
            id
        }
    };
    (
        200,
        json!({"member_id": id.to_string(), "created": created, "reactivated": reactivated, "changed": !created})
            .to_string(),
    )
}

fn update(s: &mut State, body: &str) -> (u16, String) {
    let req: UpdatePrincipalRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "update", e, body),
    };
    let ext = req.external_id.to_string();
    if let Some(u) = &req.profile.username {
        if active_username_holder(s, u, &ext).is_some() {
            return err(
                409,
                "conflict",
                "username taken",
                json!({"field": "profile.username", "reason": "username_taken"}),
            );
        }
    }
    let Some(m) = s.members.get_mut(&ext) else {
        return err(404, "not_found", "no such member", json!({}));
    };
    if let Some(d) = req.profile.display_name {
        m.display_name = d;
    }
    if let Some(u) = req.profile.username {
        m.username = u;
    }
    match req.profile.email {
        Patch::Keep => {}
        Patch::Clear => m.email = None,
        Patch::Set(e) => m.email = Some(e),
    }
    for (slot, patch) in [
        (&mut m.title, req.profile.title),
        (&mut m.pronouns, req.profile.pronouns),
    ] {
        match patch {
            Patch::Keep => {}
            Patch::Clear => *slot = None,
            Patch::Set(v) => *slot = Some(v),
        }
    }
    (
        200,
        json!({"member_id": m.member_id.to_string()}).to_string(),
    )
}

fn deprovision(s: &mut State, body: &str) -> (u16, String) {
    let req: DeprovisionRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "deprovision", e, body),
    };
    let ext = req.external_id.to_string();
    if s.owner.as_deref() == Some(ext.as_str()) {
        return err(422, "invalid_state", "owner", json!({"reason": "is_owner"}));
    }
    let Some(m) = s.members.get_mut(&ext) else {
        return err(404, "not_found", "no such member", json!({}));
    };
    m.deactivated = true;
    let id = m.member_id;
    for a in s.members.values_mut() {
        if a.assistant_of.as_deref() == Some(ext.as_str()) {
            a.deactivated = true;
        }
    }
    (
        200,
        json!({"member_id": id.to_string(), "sessions_revoked": 0}).to_string(),
    )
}

fn owner_set(s: &mut State, body: &str) -> (u16, String) {
    let req: OwnerSetRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "owner_set", e, body),
    };
    let ext = req.external_id.to_string();
    let Some(m) = s.members.get(&ext) else {
        return err(
            422,
            "invalid_state",
            "not eligible",
            json!({"reason": "target_not_eligible"}),
        );
    };
    if m.deactivated || m.guest || m.kind != MemberKind::Person {
        return err(
            422,
            "invalid_state",
            "not eligible",
            json!({"reason": "target_not_eligible"}),
        );
    }
    let id = m.member_id.to_string();
    match s.owner.clone() {
        Some(o) if o == ext => (200, json!({"owner_id": id, "changed": false}).to_string()),
        Some(_) => err(
            403,
            "missing_permission",
            "approval needed",
            json!({"reason": "owner_approval_required"}),
        ),
        None => {
            s.owner = Some(ext);
            (200, json!({"owner_id": id, "changed": true}).to_string())
        }
    }
}

fn list(s: &State, query: &str) -> (u16, String) {
    let mut after: u64 = 0;
    let mut limit: usize = 100;
    let mut only: Option<String> = None;
    for kv in query.split('&') {
        match kv.split_once('=') {
            Some(("after", v)) => after = v.parse().unwrap_or(0),
            Some(("limit", v)) => limit = v.parse().unwrap_or(100),
            Some(("external_id", v)) => {
                let raw = v.replace("%3A", ":").replace("%3a", ":");
                assert!(
                    ExternalId::parse(&raw).is_ok(),
                    "external_id filter {raw:?} is not an external id"
                );
                only = Some(raw);
            }
            Some((k, _)) => panic!("unknown principals query key {k:?}"),
            None => {}
        }
    }
    assert!((1..=500).contains(&limit), "limit {limit} outside 1–500");
    let mut all: Vec<&Member> = s
        .members
        .values()
        .filter(|m| m.member_id > after)
        .filter(|m| only.as_deref().is_none_or(|o| o == m.external_id))
        .collect();
    all.sort_by_key(|m| m.member_id);
    let has_more = all.len() > limit;
    let items: Vec<Value> = all
        .into_iter()
        .take(limit)
        .map(|m| {
            let role = if s.owner.as_deref() == Some(m.external_id.as_str()) {
                "owner"
            } else if m.guest {
                "guest"
            } else {
                "member"
            };
            let mut v = json!({
                "member_id": m.member_id.to_string(), "external_id": m.external_id,
                "kind": m.kind.as_str(), "install_role": role, "username": m.username,
                "display_name": m.display_name, "deactivated": m.deactivated,
                "created_at": "2026-10-15T00:00:00Z"
            });
            if let Some(k) = m.agent_kind {
                v["agent_kind"] = json!(k.as_str());
            }
            if let Some(a) = &m.assistant_of {
                v["assistant_of"] = json!(a);
            }
            if s.list_profile_fields {
                v["email"] = json!(m.email);
                v["title"] = json!(m.title);
                v["pronouns"] = json!(m.pronouns);
            }
            v
        })
        .collect();
    (
        200,
        json!({"items": items, "has_more": has_more}).to_string(),
    )
}
