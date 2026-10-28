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
//! - sessions/revoke (S28 §12): `k2_sid` and/or `external_id`; an unknown
//!   `external_id` → 404; else `200 {revoked: n}`.
//! - provision is DECLARATIVE (A8, live Sew): absent optional profile
//!   fields (`email`, `title`, `pronouns`) are cleared; `update` takes a
//!   ProfilePatch (omitted = kept).
//! - `GET /mgmt/v1/principals` filters by `external_id` and carries
//!   `email`/`title`/`pronouns` (SW14) unless `set_list_profile_fields(false)`
//!   (a pre-SW14 Sew). Provision has no `skipped` field (HC2 is on member-add).
//! - SW13 (K31): `spaces/bind` (once; the same App again is a no-op,
//!   another → 409 `conflict`), `spaces/members/add` (`app_id` must name
//!   the Space's App, else 409 `app_mismatch`; unprovisioned →
//!   `not_added: not_provisioned`, guests → `guest`, deactivated →
//!   `deactivated`, banned → `skipped` member ids), `spaces/members/remove`.
//!   Spaces exist only after [`FakeSew::add_space`]; unknown → 404.
//! - Unknown paths → 404 and recorded in `bad_paths`.
//!
//! S30 K15 (`k2 chat`'s real backend, Sew 0.3.0 act-as): a small `/api/v1`
//! slice — `me`, `spaces`, `resolve`, `channels/{id}`, `members?ids=`,
//! `channels/{id}/messages` (POST) — answered only under act-as
//! (`Sew-Act-As` + `Sew-Via`, both parsed with the vendored `sew-proto`
//! grammar; a bad or missing pair is recorded in `bad_headers` and refused),
//! and `POST /mgmt/v1/spaces/create` (SW11; never under act-as; an agent
//! owner is 400). One fixed Space `hearth` (id 201) with `general` (id 301);
//! the acting member is whoever `seed` gave that external id.

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
    ProvisionRequest, RevokeSessionsRequest, SpaceBindRequest, SpaceMemberRemoveRequest,
    SpaceMembersAddRequest, UpdatePrincipalRequest,
};

/// A Space (SW13 view).
#[derive(Debug, Clone, Default)]
pub struct Space {
    pub app_id: Option<String>,
    pub members: std::collections::BTreeSet<String>,
    pub banned: std::collections::BTreeSet<String>,
}

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
    /// `Sew-Act-As` / `Sew-Via` (S30 K15 act-as calls).
    pub act_as: Option<String>,
    pub via: Option<String>,
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
    /// Every `sessions/revoke` body that answered 200 (S28 §12).
    revoked: Vec<RevokeSessionsRequest>,
    /// S30 K15: act-as header pairs Sew would refuse.
    bad_headers: Vec<String>,
    /// S30 K15: messages posted through `/api/v1` (id, author, body).
    chat_posts: Vec<(u64, u64, String)>,
    /// SW13 Spaces by id.
    spaces: BTreeMap<String, Space>,
    /// Sew for Apps A3: the picture K2 pushed per external id (sha256 hex
    /// of the raw bytes, length).
    avatars: BTreeMap<String, (String, usize)>,
    /// Sew for Apps D13: `/mgmt/v1/apps/{app_id}/settings` bodies by App,
    /// and the install's home client.
    app_settings: BTreeMap<String, Value>,
    home_client: Option<Option<String>>,
    /// The status `features` (default none).
    features: Vec<String>,
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
    /// The feature flags `GET /mgmt/v1/status` reports (Sew 2026-10-28).
    pub fn set_features(&self, f: &[&str]) {
        self.with(|s| s.features = f.iter().map(|x| x.to_string()).collect())
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
            s.avatars.clear();
            s.app_settings.clear();
            s.home_client = None;
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

    /// SW13: a Space (unbound) exists.
    pub fn add_space(&self, space_id: &str) {
        self.with(|s| {
            s.spaces.insert(space_id.to_string(), Space::default());
        });
    }

    /// SW13: a Space already bound to an App (as `spaces/create` with
    /// `app_id` leaves it).
    pub fn add_app_space(&self, space_id: &str, app_id: &str) {
        self.with(|s| {
            s.spaces.insert(space_id.to_string(), Space { app_id: Some(app_id.to_string()), ..Default::default() });
        });
    }

    /// SW13: ban `ext` in a Space (HC2: adds come back `skipped`).
    pub fn ban_in_space(&self, space_id: &str, ext: &str) {
        self.with(|s| {
            if let Some(sp) = s.spaces.get_mut(space_id) {
                sp.banned.insert(ext.to_string());
                sp.members.remove(ext);
            }
        });
    }

    pub fn space(&self, space_id: &str) -> Option<Space> {
        self.with(|s| s.spaces.get(space_id).cloned())
    }

    /// A3: the picture Sew holds for `ext` (sha256 hex, length).
    pub fn avatar(&self, ext: &str) -> Option<(String, usize)> {
        self.with(|s| s.avatars.get(ext).cloned())
    }

    /// D13: the last settings body K2 sent for `app`.
    pub fn app_settings(&self, app: &str) -> Option<Value> {
        self.with(|s| s.app_settings.get(app).cloned())
    }

    /// D13: the home client K2 set (`None` = never sent).
    pub fn home_client(&self) -> Option<Option<String>> {
        self.with(|s| s.home_client.clone())
    }

    pub fn member(&self, ext: &str) -> Option<Member> {
        self.with(|s| s.members.get(ext).cloned())
    }
    pub fn members(&self) -> Vec<Member> {
        self.with(|s| s.members.values().cloned().collect())
    }
    pub fn revoked(&self) -> Vec<RevokeSessionsRequest> {
        self.with(|s| s.revoked.clone())
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
            assert!(
                s.bad_headers.is_empty(),
                "act-as headers Sew refuses: {:?}",
                s.bad_headers
            );
        })
    }

    /// S30 K15: messages posted through `/api/v1` (id, author member id, body).
    pub fn chat_posts(&self) -> Vec<(u64, u64, String)> {
        self.with(|s| s.chat_posts.clone())
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
    let (head, body, raw) = loop {
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
                buf[end + 4..end + 4 + len].to_vec(),
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
    let act = (header(&head, "sew-act-as"), header(&head, "sew-via"));
    let ctype = header(&head, "content-type");

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
            &act,
            &body,
            (&raw, ctype.as_deref()),
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
    act: &(Option<String>, Option<String>),
    body: &str,
    raw: (&[u8], Option<&str>),
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
            act_as: act.0.clone(),
            via: act.1.clone(),
        });
    };
    if auth != Some(&format!("Bearer {}", s.token)) {
        let (st, b) = err(401, "unauthenticated", "bad mgmt token", json!({}));
        record(s, st, false);
        return (st, b, String::new(), false);
    }
    // S30 K15: `/api/v1` only under act-as; `/mgmt` never.
    let acting = match chat_acting(s, path, act) {
        Ok(a) => a,
        Err((st, b)) => {
            record(s, st, false);
            return (st, b, String::new(), false);
        }
    };
    if let Some(a) = actor {
        if !sew_client::proto::mgmt::is_sew_actor(a) {
            s.bad_headers.push(format!("Sew-Actor {a:?}"));
        }
    }
    if method == "GET" {
        if let Some(me) = acting {
            let (st, b) = chat_get(s, me, path, query);
            record(s, st, false);
            return (st, b, String::new(), false);
        }
        let (st, b) = match path {
            sew_wire::MGMT_STATUS => (
                200,
                json!({
                    "version": "0.0.0-fake", "features": s.features.clone(), "schema_version": 1, "install_id": s.install_id,
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
        sew_wire::MGMT_SESSIONS_REVOKE => sessions_revoke(s, body),
        sew_wire::MGMT_SPACES_BIND => space_bind(s, body),
        sew_wire::MGMT_SPACE_MEMBERS_ADD => space_members_add(s, body),
        sew_wire::MGMT_SPACE_MEMBERS_REMOVE => space_member_remove(s, body),
        "/mgmt/v1/principals/avatar" => avatar_set(s, query, raw),
        "/mgmt/v1/principals/avatar/clear" => avatar_clear(s, body),
        "/mgmt/v1/home-client" => home_client(s, body),
        p if p.starts_with("/mgmt/v1/apps/") && p.ends_with("/settings") => app_settings(s, p, body),
        p if acting.is_some() => chat_post(s, acting.expect("acting"), p, body),
        sew_client::proto::routes::MGMT_SPACES_CREATE => space_create(s, body),
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

// ── Sew for Apps A3: pictures; D13: home client and App settings ──────

fn query_param<'a>(query: &'a str, k: &str) -> Option<&'a str> {
    query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| v))
}

fn avatar_set(s: &mut State, query: &str, raw: (&[u8], Option<&str>)) -> (u16, String) {
    let ext = query_param(query, "external_id").unwrap_or("").to_string();
    if raw.1 != Some("image/jpeg") || raw.0.is_empty() || !matches!(query_param(query, "force"), Some("true" | "false")) {
        s.bad_bodies.push(format!("avatar {ext}: type {:?}, {} bytes, query {query}", raw.1, raw.0.len()));
        return err(400, "validation_failed", "a JPEG body and force=true|false", json!({}));
    }
    let Some(m) = s.members.get(&ext) else {
        return err(404, "not_found", "no such member", json!({}));
    };
    let id = m.member_id;
    let h = Sha256::digest(raw.0);
    let hex: String = h.iter().map(|b| format!("{b:02x}")).collect();
    s.avatars.insert(ext, (hex, raw.0.len()));
    (200, json!({"member_id": id.to_string(), "applied": true}).to_string())
}

fn avatar_clear(s: &mut State, body: &str) -> (u16, String) {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return bad_body(s, "avatar clear", e, body),
    };
    let ext = v["external_id"].as_str().unwrap_or("").to_string();
    if v.as_object().map_or(true, |o| o.keys().any(|k| k != "external_id" && k != "force")) {
        return bad_body(s, "avatar clear", "unknown field", body);
    }
    let Some(m) = s.members.get(&ext) else {
        return err(404, "not_found", "no such member", json!({}));
    };
    let id = m.member_id;
    let had = s.avatars.remove(&ext).is_some();
    (200, json!({"member_id": id.to_string(), "applied": had}).to_string())
}

fn home_client(s: &mut State, body: &str) -> (u16, String) {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return bad_body(s, "home-client", e, body),
    };
    let app = match v.get("app_id") {
        Some(Value::Null) => None,
        Some(Value::String(a)) => Some(a.clone()),
        _ => return bad_body(s, "home-client", "app_id is a string or null", body),
    };
    s.home_client = Some(app.clone());
    (200, json!({"home_client": app}).to_string())
}

fn app_settings(s: &mut State, path: &str, body: &str) -> (u16, String) {
    let app = path.trim_start_matches("/mgmt/v1/apps/").trim_end_matches("/settings").to_string();
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return bad_body(s, "app settings", e, body),
    };
    // Any subset per call (Sew 2026-10-28); origin is a wss:// URL.
    let ok_keys = v.as_object().is_some_and(|o| !o.is_empty() && o.keys().all(|k| ["hidden_from_home", "display_name", "origin"].contains(&k.as_str())));
    let ok_types = v.get("hidden_from_home").is_none_or(Value::is_boolean)
        && v.get("origin").is_none_or(|o| o.as_str().is_some_and(|o| o.starts_with("wss://")));
    if app.is_empty() || !ok_keys || !ok_types {
        return bad_body(s, "app settings", "fields: hidden_from_home (bool), display_name, origin (wss://)", body);
    }
    let entry = s.app_settings.entry(app.clone()).or_insert_with(|| json!({}));
    for (k, x) in v.as_object().cloned().unwrap_or_default() {
        entry[k] = x;
    }
    (200, json!({"app_id": app, "applied": true}).to_string())
}

// ── S30 K15: the act-as `/api/v1` slice and SW11 spaces/create ─────────

const TS: &str = "2026-10-20T00:00:00.000Z";
const SPACE_ID: &str = "201";
const CHANNEL_ID: &str = "301";

/// `Ok(Some(member id))` for a valid act-as pair on `/api/v1`, `Ok(None)`
/// for a plain mgmt call, `Err` for what Sew refuses (recorded).
fn chat_acting(
    s: &mut State,
    path: &str,
    act: &(Option<String>, Option<String>),
) -> Result<Option<u64>, (u16, String)> {
    use sew_client::proto::mgmt::{ActAsTarget, SewVia};
    let api = path.starts_with("/api/v1/");
    match (api, act) {
        (false, (None, None)) => Ok(None),
        (false, _) => {
            s.bad_headers.push(format!("act-as on mgmt {path}"));
            Err(err(400, "bad_request", "act-as is for /api/v1 only", json!({})))
        }
        (true, (Some(t), Some(v))) => {
            let target: Result<ActAsTarget, _> = t.parse();
            let via: Result<SewVia, _> = v.parse();
            let (Ok(ActAsTarget::External(ext)), Ok(_)) = (target, via) else {
                s.bad_headers.push(format!("Sew-Act-As {t:?} / Sew-Via {v:?}"));
                return Err(err(400, "bad_request", "bad act-as headers", json!({})));
            };
            let ext = ext.to_string();
            match s.members.get(&ext) {
                Some(m) if !m.deactivated => Ok(Some(m.member_id)),
                _ => Err(err(403, "act_as_unknown", "no active member has that external id", json!({}))),
            }
        }
        (true, _) => {
            s.bad_headers.push(format!("{path} without act-as"));
            Err(err(401, "unauthenticated", "the service token needs act-as on /api/v1", json!({})))
        }
    }
}

fn member_json(m: &Member) -> Value {
    json!({
        "id": m.member_id.to_string(),
        "kind": if m.kind == MemberKind::Agent { "agent" } else { "person" },
        "install_role": "member",
        "username": m.username,
        "display_name": m.display_name,
        "deactivated": m.deactivated,
        "created_at": TS,
    })
}

fn channel_json() -> Value {
    json!({
        "id": CHANNEL_ID, "space_id": SPACE_ID, "type": "text", "visibility": "public",
        "slug": "general", "name": "general", "position": 0, "synced": true, "head_seq": 1,
        "dm_member_ids": [], "my_perms": "7", "created_at": TS,
    })
}

fn chat_get(s: &mut State, me: u64, path: &str, query: &str) -> (u16, String) {
    let q = |k: &str| {
        query.split('&').find_map(|kv| kv.split_once('=').filter(|(a, _)| *a == k).map(|(_, v)| v.to_string()))
    };
    match path {
        "/api/v1/me" => {
            let m = s.members.values().find(|m| m.member_id == me).expect("acting member").clone();
            (200, member_json(&m).to_string())
        }
        "/api/v1/spaces" => (
            200,
            json!({"items": [{"id": SPACE_ID, "slug": "hearth", "name": "Hearth", "owner_id": "1",
                "member_count": 2, "created_at": TS}], "has_more": false})
            .to_string(),
        ),
        "/api/v1/resolve" => match q("addr").as_deref() {
            // Sew's own address form: `<space>/#<channel>`, `#` as %23 on
            // the wire. A bare `hearth/general` isn't an address Sew parses.
            Some("hearth/%23general") | Some("hearth/#general") => {
                (200, json!({"space_id": SPACE_ID, "channel_id": CHANNEL_ID}).to_string())
            }
            _ => err(404, "not_found", "channel not found", json!({})),
        },
        "/api/v1/channels/301" => (200, channel_json().to_string()),
        "/api/v1/spaces/201/channels" => (200, json!({"items": [channel_json()], "has_more": false}).to_string()),
        "/api/v1/members" => {
            let ids: Vec<String> = q("ids").map(|l| l.replace("%2C", ",").split(',').map(str::to_string).collect()).unwrap_or_default();
            let items: Vec<Value> =
                s.members.values().filter(|m| ids.contains(&m.member_id.to_string())).map(member_json).collect();
            (200, json!({"items": items, "has_more": false}).to_string())
        }
        _ => {
            s.bad_paths.push(format!("GET {path}"));
            err(404, "not_found", "unknown route", json!({"reason": "unknown_route"}))
        }
    }
}

fn chat_post(s: &mut State, me: u64, path: &str, body: &str) -> (u16, String) {
    use sew_client::proto::api::messages::CreateMessageRequest;
    if path != "/api/v1/channels/301/messages" {
        s.bad_paths.push(format!("POST {path}"));
        return err(404, "not_found", "unknown route", json!({"reason": "unknown_route"}));
    }
    let req: CreateMessageRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "create message", e, body),
    };
    let id = 400 + s.chat_posts.len() as u64 + 1;
    s.chat_posts.push((id, me, req.body.clone()));
    (
        200,
        json!({
            "id": id.to_string(), "channel_id": CHANNEL_ID, "author_id": me.to_string(), "body": req.body,
            "also_in_channel": false,
            "mentions": {"members": [], "roles": [], "groups": [], "channel": false, "here": false},
            "reactions": [], "attachments": [], "pinned": false, "seq": id, "created_at": TS,
        })
        .to_string(),
    )
}

fn space_create(s: &mut State, body: &str) -> (u16, String) {
    use sew_client::proto::mgmt::MgmtSpaceCreateRequest;
    let req: MgmtSpaceCreateRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "spaces/create", e, body),
    };
    let owner = req.owner.to_string();
    if owner.starts_with("k2:workspace:") {
        return err(400, "validation_failed", "an agent never owns a Space", json!({"field": "owner"}));
    }
    let owner_id = s.members.get(&owner).map(|m| m.member_id).unwrap_or(1);
    let managers: Vec<String> = req
        .managers
        .iter()
        .filter_map(|e| s.members.get(&e.to_string()).map(|m| m.member_id.to_string()))
        .collect();
    (
        200,
        json!({
            "space_id": "202", "slug": req.slug.as_str(), "name": req.name,
            "owner_member_id": owner_id.to_string(), "created": true,
            "manager_member_ids": managers,
        })
        .to_string(),
    )
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

fn space_bind(s: &mut State, body: &str) -> (u16, String) {
    let req: SpaceBindRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "spaces/bind", e, body),
    };
    if !sew_wire::is_app_id(&req.app_id) {
        return bad_body(s, "spaces/bind", "app_id grammar", body);
    }
    let Some(sp) = s.spaces.get_mut(&req.space_id) else {
        return err(404, "not_found", "no such space", json!({}));
    };
    match &sp.app_id {
        Some(a) if *a != req.app_id => {
            return err(409, "conflict", "bound to another App", json!({"reason": "space_bound"}));
        }
        _ => sp.app_id = Some(req.app_id.clone()),
    }
    (200, json!({"space_id": req.space_id, "slug": "s", "name": "S", "owner_member_id": "1", "app_id": req.app_id, "created": false}).to_string())
}

fn space_members_add(s: &mut State, body: &str) -> (u16, String) {
    let req: SpaceMembersAddRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "spaces/members/add", e, body),
    };
    if req.external_ids.is_empty() || req.external_ids.len() > 100 {
        return bad_body(s, "spaces/members/add", "1–100 principals", body);
    }
    let members = s.members.clone();
    let Some(sp) = s.spaces.get_mut(&req.space_id) else {
        return err(404, "not_found", "no such space", json!({}));
    };
    if sp.app_id != req.app_id {
        return err(409, "conflict", "app_id does not name the Space's App", json!({"reason": "app_mismatch"}));
    }
    let (mut added, mut already, mut skipped, mut not_added) = (vec![], vec![], vec![], vec![]);
    for ext in &req.external_ids {
        let e = ext.to_string();
        let Some(m) = members.get(&e) else {
            not_added.push(json!({"external_id": e, "reason": "not_provisioned"}));
            continue;
        };
        let r = json!({"external_id": e, "member_id": m.member_id.to_string()});
        if m.deactivated {
            not_added.push(json!({"external_id": e, "reason": "deactivated"}));
        } else if m.guest {
            not_added.push(json!({"external_id": e, "reason": "guest"}));
        } else if sp.banned.contains(&e) {
            skipped.push(m.member_id.to_string());
        } else if sp.members.insert(e) {
            added.push(r);
        } else {
            already.push(r);
        }
    }
    (200, json!({"added": added, "already": already, "skipped": skipped, "not_added": not_added}).to_string())
}

fn space_member_remove(s: &mut State, body: &str) -> (u16, String) {
    let req: SpaceMemberRemoveRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "spaces/members/remove", e, body),
    };
    let Some(sp) = s.spaces.get_mut(&req.space_id) else {
        return err(404, "not_found", "no such space", json!({}));
    };
    if sp.app_id != req.app_id {
        return err(409, "conflict", "app_id does not name the Space's App", json!({"reason": "app_mismatch"}));
    }
    let removed = sp.members.remove(&req.external_id.to_string());
    (200, json!({"removed": removed}).to_string())
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

/// `POST /mgmt/v1/sessions/revoke` (S28 §12, live Sew `revoke_sessions`):
/// one of `k2_sid` / `external_id` (else 400); an unknown `external_id`
/// is 404; anything else `200 {revoked: n}` (the fake holds no sessions,
/// so `n` is how many revokes it has seen for that selector before).
fn sessions_revoke(s: &mut State, body: &str) -> (u16, String) {
    let req: RevokeSessionsRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "sessions/revoke", e, body),
    };
    if req.external_id.is_none() && req.k2_sid.is_none() {
        return bad_body(s, "sessions/revoke", "send external_id, k2_sid, or both", body);
    }
    if let Some(ext) = &req.external_id {
        if !s.members.contains_key(&ext.to_string()) {
            return err(404, "not_found", "no such member", json!({}));
        }
    }
    let n = s.revoked.iter().filter(|r| **r == req).count();
    s.revoked.push(req);
    (200, json!({"revoked": n}).to_string())
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
