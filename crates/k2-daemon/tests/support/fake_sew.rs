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
//!   `external_id` → 404; else `200 {revoked: n}`. Sew 0.6.0 I8 (flag
//!   `app_governed_spaces`): `app_id` ends every session of that App; a fake
//!   without the flag refuses the field like an older Sew (400, recorded in
//!   `bad_bodies`).
//! - App settings (decision 13 + Sew 0.6.0 I7): `hidden_from_home`,
//!   `display_name`, `origin`; with `app_governed_spaces` also `max_level`
//!   (`member | moderator | admin`) and `uploads` (bool). `max_level` on the
//!   home-client App → 409 `conflict` reason `home_client_cannot_govern`.
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
//! - A9 (Sew 0.5.0, flag `app_channel_create`): `channels/create`
//!   (`app_id` must name the Space's App, else 409 `conflict` reason
//!   `app_mismatch`; an unbound Space → 409 `conflict` reason
//!   `space_unbound`; unknown → 404). Idempotent on (space, slug): a repeat
//!   answers `created: false` with the CURRENT name and visibility and adds
//!   listed members it lacks. Members: unknown → `not_provisioned`,
//!   deactivated, guest, outside the Space → `not_space_member`, banned →
//!   `skipped` member ids; the Space's owner is never in `added`.
//! - Sew 0.6.0 (flag `app_governed_spaces`, sew-55 v2.4; without the flag
//!   each route is unknown: 404 + `bad_paths`): I3 `spaces/{id}/settings
//!   {governance}` (`app` needs a governing link → 409 `no_governing_app`;
//!   box users other than the owner → 409 `box_users_present` +
//!   `details.external_ids`; the switch makes `app:<app>` the owner and a
//!   box-user former owner leaves); I4 single `spaces/{id}/members/{ext}/
//!   channels` and bulk `spaces/{id}/members/channels` (1–500, answered
//!   per item `{external_id, status?, error?, …}`): the answer is `{added,
//!   removed, unchanged, level, unknown_channel_ids, skipped}` (+ `queued:
//!   true` over 1,000 changes); 409 `app_mismatch` / `not_app_governed`;
//!   422 `box_user_not_allowed`, `level_above_ceiling` (the App's
//!   `max_level`, default moderator), `agent_level`, `member_not_provisioned`;
//!   `skipped`: `owner`, `deprovisioned`, `banned`; I6 `channels/bulk`
//!   (`ensure` with `channel_type_mismatch`, `rename`, `archive`,
//!   `unarchive`); `spaces/create` with `owner: "app:<app_id>"` (§3a; makes
//!   the service owner member; another App's → 409 `app_mismatch`).
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
    ChannelCreateRequest, ProvisionRequest, RevokeSessionsRequest, SpaceBindRequest,
    SpaceMemberRemoveRequest, SpaceMembersAddRequest, UpdatePrincipalRequest,
};

/// A Space (SW13 view).
#[derive(Debug, Clone, Default)]
pub struct Space {
    pub app_id: Option<String>,
    pub members: std::collections::BTreeSet<String>,
    pub banned: std::collections::BTreeSet<String>,
    /// A9: channels by slug.
    pub channels: BTreeMap<String, Channel>,
    /// Sew 0.5.0 (`space_unbind`): the App this Space was unbound from (an
    /// orphan: adds and channel creates answer 422 `space_orphaned`; removes work).
    pub orphaned_from: Option<String>,
    /// Sew 0.6.0 I3: `app` | `sew` (`None` = `sew`).
    pub governance: Option<String>,
    /// The Space owner's external id (`app:<app_id>` once App-governed).
    pub owner: Option<String>,
    /// Sew 0.6.0 I4: each person's level in an App-governed Space.
    pub levels: BTreeMap<String, String>,
}

/// A9: a text channel made over mgmt.
#[derive(Debug, Clone, Default)]
pub struct Channel {
    pub id: u64,
    pub name: String,
    pub private: bool,
    /// External ids.
    pub members: std::collections::BTreeSet<String>,
    /// Sew 0.6.0 I6: `text` | `voice` (empty = text). A voice channel is
    /// made in Sew (A9b); K2's `channels/create` makes text only.
    pub kind: String,
    pub archived: bool,
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
    /// Sew 0.6.0 A23: every owner pass a home-client write carried, in
    /// order (accepted or not), and the `jti`s spent.
    home_approvals: Vec<String>,
    spent_jti: Vec<String>,
    /// Sew 0.6.0 S55: Apps that govern a Space (never the home client).
    governing: Vec<String>,
    /// The status `features` (default none).
    features: Vec<String>,
    /// Sew 0.5.0: org policies K2 may read and set over mgmt (only
    /// `identity.managed_fields`; absent = Sew's default).
    policies: BTreeMap<String, Value>,
    /// `GET /mgmt/v1/policies` shows the default managed fields explicitly
    /// (instead of leaving the key out).
    policy_default_shown: bool,
    /// `GET /mgmt/v1/policies` answers 503.
    policy_get_fails: bool,
    /// Provisions that cleared a title or pronouns an existing member had
    /// (Sew 0.5.0's provision clears an absent one).
    profile_clears: Vec<String>,
    /// Sew 0.6.0: I4 bulk calls answered.
    bulk_calls: usize,
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
    /// Sew 0.5.0: an admin sets (or, `None`, resets) a policy behind K2.
    pub fn set_policy(&self, key: &str, v: Option<Value>) {
        self.with(|s| match v {
            Some(v) => {
                s.policies.insert(key.into(), v);
            }
            None => {
                s.policies.remove(key);
            }
        })
    }
    pub fn policy(&self, key: &str) -> Option<Value> {
        self.with(|s| s.policies.get(key).cloned())
    }
    /// Show the default `identity.managed_fields` in `GET /mgmt/v1/policies`.
    pub fn set_policy_default_shown(&self, on: bool) {
        self.with(|s| s.policy_default_shown = on)
    }
    /// `GET /mgmt/v1/policies` answers 503 while `on`.
    pub fn set_policy_get_fails(&self, on: bool) {
        self.with(|s| s.policy_get_fails = on)
    }
    /// Every provision that cleared a title or pronouns an existing member
    /// had (the clear is applied, as the real Sew 0.5.0 does).
    pub fn profile_clears(&self) -> Vec<String> {
        self.with(|s| s.profile_clears.clone())
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
            s.policies.clear();
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

    /// A voice channel at `slug` in a Space (made in Sew, not by K2).
    pub fn add_voice_channel(&self, space_id: &str, slug: &str) {
        self.with(|s| {
            s.next_id += 1;
            let id = 900_000 + s.next_id;
            let sp = s.spaces.get_mut(space_id).expect("the Space exists");
            sp.channels.insert(slug.into(), Channel { id, name: slug.into(), private: false, kind: "voice".into(), ..Default::default() });
        })
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

    /// Sew 0.6.0: who owns a Space now (`app:<app>` once App-governed).
    pub fn set_space_owner(&self, space_id: &str, owner: &str) {
        self.with(|s| {
            if let Some(sp) = s.spaces.get_mut(space_id) {
                sp.owner = Some(owner.to_string());
                sp.members.insert(owner.to_string());
            }
        });
    }

    /// Sew 0.6.0: someone joined a Space by hand (a box user, before the switch).
    pub fn add_space_member(&self, space_id: &str, ext: &str) {
        self.with(|s| {
            if let Some(sp) = s.spaces.get_mut(space_id) {
                sp.members.insert(ext.to_string());
            }
        });
    }

    /// Sew 0.6.0: `n` private channels made in Sew (by the App) as
    /// `job-<i>`; returns `(slug, id)`.
    pub fn add_channels(&self, space_id: &str, n: usize) -> Vec<(String, String)> {
        self.with(|s| {
            let mut out = Vec::new();
            for i in 0..n {
                s.next_id += 1;
                let id = 600_000 + s.next_id;
                let slug = format!("job-{i}");
                let sp = s.spaces.get_mut(space_id).expect("space");
                sp.channels.insert(slug.clone(), Channel { id, name: slug.clone(), private: true, kind: "text".into(), ..Default::default() });
                out.push((slug, id.to_string()));
            }
            out
        })
    }

    /// Sew 0.6.0: the private channel ids `ext` is in, and their level.
    pub fn member_channels(&self, space_id: &str, ext: &str) -> (std::collections::BTreeSet<String>, Option<String>) {
        self.with(|s| {
            let Some(sp) = s.spaces.get(space_id) else { return (Default::default(), None) };
            let ids = sp.channels.values().filter(|c| c.members.contains(ext)).map(|c| c.id.to_string()).collect();
            (ids, sp.levels.get(ext).cloned())
        })
    }

    /// Sew 0.6.0: I4 bulk calls answered so far.
    pub fn bulk_calls(&self) -> usize {
        self.with(|s| s.bulk_calls)
    }

    /// A moderator removed `ext` from a Space by hand (K2's ledger still
    /// says it is in).
    pub fn remove_from_space(&self, space_id: &str, ext: &str) {
        self.with(|s| {
            if let Some(sp) = s.spaces.get_mut(space_id) {
                sp.members.remove(ext);
            }
        });
    }

    /// A9: a channel made in a Space.
    pub fn channel(&self, space_id: &str, slug: &str) -> Option<Channel> {
        self.with(|s| s.spaces.get(space_id).and_then(|sp| sp.channels.get(slug).cloned()))
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

    /// Sew 0.6.0 A23: the owner passes home-client writes carried.
    pub fn home_approvals(&self) -> Vec<String> {
        self.with(|s| s.home_approvals.clone())
    }

    /// Sew 0.6.0 S55: `app` governs a Space.
    pub fn set_governing(&self, app: &str) {
        self.with(|s| s.governing.push(app.into()))
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
            "/mgmt/v1/policies" => policies_get(s),
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
        sew_wire::MGMT_CHANNELS_CREATE => channel_create(s, body),
        "/mgmt/v1/principals/avatar" => avatar_set(s, query, raw),
        "/mgmt/v1/principals/avatar/clear" => avatar_clear(s, body),
        "/mgmt/v1/home-client" => home_client(s, body),
        "/mgmt/v1/policies" => policies_set(s, body),
        sew_wire::MGMT_SPACES_UNBIND => space_unbind(s, body),
        // Sew 0.6.0 (App-governed Spaces): I3, I4 (bulk before single), I6.
        p if p.starts_with("/mgmt/v1/spaces/") && p.ends_with("/settings") => space_settings(s, p, body),
        p if p.starts_with("/mgmt/v1/spaces/") && p.ends_with("/members/channels") => member_channels_bulk(s, p, body),
        p if p.starts_with("/mgmt/v1/spaces/") && p.contains("/members/") && p.ends_with("/channels") => member_channels_one(s, p, body),
        sew_wire::MGMT_CHANNELS_BULK => channels_bulk(s, body),
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
    // Sew 0.6.0 A23 (flag home_client_owner_approval): the install owner's
    // approval on every write, checked first. Without the flag the body is
    // exactly {app_id} (Sew 0.5.0 denies unknown fields).
    let approval_flag = s.features.iter().any(|f| f == "home_client_owner_approval");
    let keys_ok = v.as_object().is_some_and(|o| o.keys().all(|k| k == "app_id" || (approval_flag && k == "approval")));
    if !keys_ok {
        return bad_body(s, "home-client", "unknown field (this Sew takes {app_id} only)", body);
    }
    if approval_flag {
        let refuse = || {
            err(
                403,
                "missing_permission",
                "changing the home client needs the current install owner's approval",
                json!({"reason": "owner_approval_required"}),
            )
        };
        let Some(a) = v.get("approval") else { return refuse() };
        if a.get("kind").and_then(Value::as_str) != Some("owner_pass") || a.as_object().map(|o| o.len()) != Some(2) {
            return bad_body(s, "home-client", "approval is {kind: owner_pass, pass}", body);
        }
        let pass = a.get("pass").and_then(Value::as_str).unwrap_or("").to_string();
        s.home_approvals.push(pass.clone());
        let claims = pass_claims(&pass);
        let jti = claims.get("jti").and_then(Value::as_str).unwrap_or("").to_string();
        let ok = claims.get("sub").and_then(Value::as_str) == Some("k2:owner")
            && claims.get("aud").and_then(Value::as_str) == Some(format!("sew:{}", s.install_id).as_str())
            && !jti.is_empty()
            && !s.spent_jti.contains(&jti);
        if !ok {
            return refuse();
        }
        s.spent_jti.push(jti);
    }
    if app.as_ref().is_some_and(|a| s.governing.contains(a)) {
        return err(
            409,
            "conflict",
            "the home client never governs a Space, and this App governs one",
            json!({"reason": "home_client_cannot_govern", "field": "app_id"}),
        );
    }
    s.home_client = Some(app.clone());
    (200, json!({"home_client": app}).to_string())
}

fn app_settings(s: &mut State, path: &str, body: &str) -> (u16, String) {
    let app = path.trim_start_matches("/mgmt/v1/apps/").trim_end_matches("/settings").to_string();
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return bad_body(s, "app settings", e, body),
    };
    // Any subset per call (Sew 2026-10-28); origin is a wss:// URL, or
    // null to clear it (Sew I7, App-governed Spaces). Sew 0.6.0 (I7, flag
    // `app_governed_spaces`): max_level and uploads.
    let governed = s.features.iter().any(|f| f == "app_governed_spaces");
    let mut known = vec!["hidden_from_home", "display_name", "origin"];
    if governed {
        known.extend(["max_level", "uploads"]);
    }
    let ok_keys = v.as_object().is_some_and(|o| !o.is_empty() && o.keys().all(|k| known.contains(&k.as_str())));
    let ok_types = v.get("hidden_from_home").is_none_or(Value::is_boolean)
        && v.get("origin").is_none_or(|o| o.is_null() || o.as_str().is_some_and(|o| o.starts_with("wss://")))
        && v.get("max_level").is_none_or(|l| matches!(l.as_str(), Some("member" | "moderator" | "admin")))
        && v.get("uploads").is_none_or(Value::is_boolean);
    if app.is_empty() || !ok_keys || !ok_types {
        return bad_body(s, "app settings", "fields: hidden_from_home (bool), display_name, origin (wss://), max_level, uploads (bool; app_governed_spaces)", body);
    }
    if v.get("max_level").is_some() && s.home_client.clone().flatten().as_deref() == Some(app.as_str()) {
        return err(409, "conflict", "the home client can't govern", json!({"reason": "home_client_cannot_govern"}));
    }
    let entry = s.app_settings.entry(app.clone()).or_insert_with(|| json!({}));
    for (k, x) in v.as_object().cloned().unwrap_or_default() {
        entry[k] = x;
    }
    (200, json!({"app_id": app, "applied": true}).to_string())
}

// ── Sew 0.5.0: the managed-fields policy and Space unbind ─────────────

const MANAGED_FIELDS: &str = "identity.managed_fields";
const DEFAULT_MANAGED: [&str; 3] = ["display_name", "title", "pronouns"];
const MANAGEABLE: [&str; 6] = ["display_name", "username", "email", "title", "pronouns", "avatar"];

fn policies_json(s: &State) -> Value {
    let mut p = serde_json::Map::new();
    p.insert("gifs.enabled".into(), json!(true));
    p.insert("messages.delete_own".into(), json!(true));
    for (k, v) in &s.policies {
        p.insert(k.clone(), v.clone());
    }
    if s.policy_default_shown && !p.contains_key(MANAGED_FIELDS) {
        p.insert(MANAGED_FIELDS.into(), json!(DEFAULT_MANAGED));
    }
    json!({ "policies": p })
}

fn policies_get(s: &mut State) -> (u16, String) {
    if s.policy_get_fails {
        return err(503, "unavailable", "injected", json!({}));
    }
    (200, policies_json(s).to_string())
}

/// `POST /mgmt/v1/policies {"changes": {...}}`. K2 only ever sets
/// `identity.managed_fields`, and only to a Sew that reports
/// `k2_managed_avatar` (an older Sew refuses the key: 400).
fn policies_set(s: &mut State, body: &str) -> (u16, String) {
    let v: Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(e) => return bad_body(s, "policies", e, body),
    };
    let Some(changes) = v.get("changes").and_then(Value::as_object).filter(|_| v.as_object().is_some_and(|o| o.len() == 1)) else {
        return bad_body(s, "policies", "expected {changes: {...}}", body);
    };
    if changes.keys().any(|k| k != MANAGED_FIELDS) {
        return bad_body(s, "policies", "K2 sets only identity.managed_fields", body);
    }
    if !s.features.iter().any(|f| f == "k2_managed_avatar") {
        return bad_body(s, "policies", "identity.managed_fields on a Sew without k2_managed_avatar", body);
    }
    match changes.get(MANAGED_FIELDS) {
        Some(Value::Null) => {
            s.policies.remove(MANAGED_FIELDS);
        }
        Some(Value::Array(a)) => {
            let names: Vec<&str> = a.iter().filter_map(Value::as_str).collect();
            let mut uniq = names.clone();
            uniq.sort_unstable();
            uniq.dedup();
            if names.len() != a.len() || uniq.len() != names.len() || names.iter().any(|n| !MANAGEABLE.contains(n)) {
                s.bad_bodies.push(format!("policies: duplicate or unknown managed field: {body}"));
                return err(400, "validation_failed", "managed fields: distinct known names", json!({"field": MANAGED_FIELDS}));
            }
            s.policies.insert(MANAGED_FIELDS.into(), Value::Array(a.clone()));
        }
        _ => return bad_body(s, "policies", "identity.managed_fields is a list or null", body),
    }
    (200, policies_json(s).to_string())
}

/// `POST /mgmt/v1/spaces/unbind {space_id, app_id}` (flag `space_unbind`).
fn space_unbind(s: &mut State, body: &str) -> (u16, String) {
    let req: sew_wire::SpaceUnbindRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "spaces/unbind", e, body),
    };
    if !s.features.iter().any(|f| f == "space_unbind") {
        s.bad_paths.push("POST /mgmt/v1/spaces/unbind on a Sew without space_unbind".into());
        return err(404, "not_found", "unknown route", json!({"reason": "unknown_route"}));
    }
    let Some(sp) = s.spaces.get_mut(&req.space_id) else {
        return err(404, "not_found", "no such space", json!({}));
    };
    if sp.app_id.is_none() && sp.orphaned_from.as_deref() == Some(req.app_id.as_str()) {
        return (200, json!({"space_id": req.space_id, "orphaned_from": req.app_id, "changed": false}).to_string());
    }
    if sp.app_id.as_deref() != Some(req.app_id.as_str()) {
        return err(409, "conflict", "app_id does not name the Space's App", json!({"reason": "app_mismatch"}));
    }
    sp.app_id = None;
    sp.orphaned_from = Some(req.app_id.clone());
    (200, json!({"space_id": req.space_id, "orphaned_from": req.app_id, "changed": true}).to_string())
}

fn orphaned(space_id: &str) -> (u16, String) {
    err(422, "invalid_state", &format!("Space {space_id} is unbound from its App"), json!({"reason": "space_orphaned"}))
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

// ── Sew 0.6.0: App-governed Spaces (sew-55 v2.4 I3, I4, I6, §3a) ───────
//
// Every route answers only with the flag `app_governed_spaces` (a Sew
// without it has none of them: 404 + `bad_paths`, so `assert_clean`
// catches K2 sending without the flag).

const GOVERNED: &str = "app_governed_spaces";

fn no_route(s: &mut State, what: &str) -> (u16, String) {
    s.bad_paths.push(format!("POST {what} (no {GOVERNED} flag)"));
    err(404, "not_found", "unknown route", json!({"reason": "unknown_route"}))
}

fn pct_decode(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&raw[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_default()
}

fn is_box_user(ext: &str) -> bool {
    ext == "k2:owner" || ext.starts_with("k2:connect:")
}

/// I3: `POST /mgmt/v1/spaces/{space_id}/settings {governance}`.
fn space_settings(s: &mut State, path: &str, body: &str) -> (u16, String) {
    if !s.features.iter().any(|f| f == GOVERNED) {
        return no_route(s, path);
    }
    let space = path.trim_start_matches("/mgmt/v1/spaces/").trim_end_matches("/settings").to_string();
    let req: sew_wire::SpaceSettingsRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "spaces/settings", e, body),
    };
    if req.governance != "app" && req.governance != "sew" {
        return bad_body(s, "spaces/settings", "governance app|sew", body);
    }
    let Some(sp) = s.spaces.get_mut(&space) else {
        return err(404, "not_found", "no such space", json!({}));
    };
    if req.governance == "app" {
        let Some(app) = sp.app_id.clone() else {
            return err(409, "conflict", "no governing App", json!({"reason": "no_governing_app"}));
        };
        let owner = sp.owner.clone();
        let present: Vec<String> =
            sp.members.iter().filter(|m| is_box_user(m) && Some(m.as_str()) != owner.as_deref()).cloned().collect();
        if !present.is_empty() {
            return err(409, "conflict", "box users present", json!({"reason": "box_users_present", "external_ids": present}));
        }
        // The service owner takes over; a box-user former owner leaves.
        if let Some(o) = owner.filter(|o| is_box_user(o)) {
            sp.members.remove(&o);
        }
        sp.owner = Some(format!("app:{app}"));
    }
    sp.governance = Some(req.governance.clone());
    (200, json!({"space_id": space, "governance": req.governance}).to_string())
}

/// One I4 item, applied: `Ok(answer)` or `Err((status, code, reason))`.
fn member_channels_item(s: &mut State, space: &str, app: &str, ext: &str, ids: &[String], level: &str) -> Result<Value, (u16, &'static str, &'static str)> {
    let ceiling = s.app_settings.get(app).and_then(|v| v.get("max_level")).and_then(Value::as_str).unwrap_or("moderator").to_string();
    let member = s.members.get(ext).cloned();
    let Some(sp) = s.spaces.get_mut(space) else { return Err((404, "not_found", "")) };
    if sp.app_id.as_deref() != Some(app) {
        return Err((409, "conflict", "app_mismatch"));
    }
    if sp.governance.as_deref() != Some("app") {
        return Err((409, "conflict", "not_app_governed"));
    }
    if is_box_user(ext) {
        return Err((422, "invalid_state", "box_user_not_allowed"));
    }
    if sp.owner.as_deref() == Some(ext) {
        return Ok(json!({"added": [], "removed": [], "unchanged": [], "level": level, "unknown_channel_ids": [], "skipped": "owner"}));
    }
    let rank = |l: &str| sew_wire::level_rank(l).unwrap_or(9);
    if rank(level) > rank(&ceiling) {
        return Err((422, "invalid_state", "level_above_ceiling"));
    }
    if ext.starts_with("k2:workspace:") && level != "member" {
        return Err((422, "invalid_state", "agent_level"));
    }
    let Some(m) = member else { return Err((422, "invalid_state", "member_not_provisioned")) };
    if m.deactivated {
        return Ok(json!({"added": [], "removed": [], "unchanged": [], "level": level, "unknown_channel_ids": [], "skipped": "deprovisioned"}));
    }
    if sp.banned.contains(ext) {
        return Ok(json!({"added": [], "removed": [], "unchanged": [], "level": level, "unknown_channel_ids": [], "skipped": "banned"}));
    }
    let known: BTreeMap<String, String> = sp.channels.iter().map(|(slug, c)| (c.id.to_string(), slug.clone())).collect();
    let unknown: Vec<String> = ids.iter().filter(|i| !known.contains_key(*i)).cloned().collect();
    if !unknown.is_empty() {
        return Ok(json!({"added": [], "removed": [], "unchanged": [], "level": level, "unknown_channel_ids": unknown, "skipped": null}));
    }
    let want: std::collections::BTreeSet<String> = ids.iter().cloned().collect();
    let (mut added, mut removed, mut unchanged) = (vec![], vec![], vec![]);
    for c in sp.channels.values_mut().filter(|c| c.private) {
        let id = c.id.to_string();
        match (want.contains(&id), c.members.contains(ext)) {
            (true, true) => unchanged.push(id),
            (true, false) => {
                c.members.insert(ext.to_string());
                added.push(id);
            }
            // "Exactly the list, plus the owner": a sync never removes the owner.
            (false, true) => {
                c.members.remove(ext);
                removed.push(id);
            }
            (false, false) => {}
        }
    }
    sp.members.insert(ext.to_string());
    sp.levels.insert(ext.to_string(), level.to_string());
    let queued = added.len() + removed.len() > sew_wire::MEMBER_CHANNELS_JOB_THRESHOLD;
    let mut a = json!({"added": added, "removed": removed, "unchanged": unchanged, "level": level, "unknown_channel_ids": [], "skipped": null});
    if queued {
        a["queued"] = json!(true);
    }
    Ok(a)
}

/// I4 single: `POST /mgmt/v1/spaces/{space_id}/members/{external_id}/channels`.
fn member_channels_one(s: &mut State, path: &str, body: &str) -> (u16, String) {
    if !s.features.iter().any(|f| f == GOVERNED) {
        return no_route(s, path);
    }
    let rest = path.trim_start_matches("/mgmt/v1/spaces/");
    let parts: Vec<&str> = rest.split('/').collect();
    let [space, "members", ext_enc, "channels"] = parts.as_slice() else {
        return bad_body(s, "member channels path", "shape", path);
    };
    let ext = pct_decode(ext_enc);
    if ext.contains('%') || ExternalId::parse(&ext).is_err() {
        return bad_body(s, "member channels path", "external id", path);
    }
    let req: sew_wire::MemberChannelsRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "member channels", e, body),
    };
    if req.channel_ids.len() > sew_wire::MEMBER_CHANNELS_MAX || sew_wire::level_rank(&req.level).is_none() {
        return bad_body(s, "member channels", "channel_ids ≤ 5000, level", body);
    }
    match member_channels_item(s, space, &req.app_id, &ext, &req.channel_ids, &req.level) {
        Ok(v) => (200, v.to_string()),
        Err((st, code, reason)) => err(st, code, "refused", json!({"reason": reason})),
    }
}

/// I4 bulk: `POST /mgmt/v1/spaces/{space_id}/members/channels`.
fn member_channels_bulk(s: &mut State, path: &str, body: &str) -> (u16, String) {
    if !s.features.iter().any(|f| f == GOVERNED) {
        return no_route(s, path);
    }
    let space = path.trim_start_matches("/mgmt/v1/spaces/").trim_end_matches("/members/channels").to_string();
    let req: sew_wire::MemberChannelsBulkRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "member channels bulk", e, body),
    };
    if req.items.is_empty() || req.items.len() > sew_wire::MEMBER_CHANNELS_BULK_MAX {
        return bad_body(s, "member channels bulk", "1–500 items", body);
    }
    if !s.spaces.contains_key(&space) {
        return err(404, "not_found", "no such space", json!({}));
    }
    s.bulk_calls += 1;
    let mut items = Vec::new();
    for it in &req.items {
        let ext = it.external_id.to_string();
        match member_channels_item(s, &space, &req.app_id, &ext, &it.channel_ids, &it.level) {
            Ok(mut v) => {
                v["external_id"] = json!(ext);
                items.push(v);
            }
            Err((st, code, reason)) => items.push(json!({"external_id": ext, "status": st, "error": {"code": code, "hint": "refused", "details": {"reason": reason}}})),
        }
    }
    (200, json!({"items": items}).to_string())
}

/// I6: `POST /mgmt/v1/channels/bulk`.
fn channels_bulk(s: &mut State, body: &str) -> (u16, String) {
    if !s.features.iter().any(|f| f == GOVERNED) {
        return no_route(s, sew_wire::MGMT_CHANNELS_BULK);
    }
    let req: sew_wire::ChannelsBulkRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "channels/bulk", e, body),
    };
    if req.ops.is_empty() || req.ops.len() > sew_wire::CHANNELS_BULK_MAX {
        return bad_body(s, "channels/bulk", "1–200 ops", body);
    }
    let Some(sp) = s.spaces.get(&req.space_id) else {
        return err(404, "not_found", "no such space", json!({}));
    };
    if sp.app_id.as_deref() != Some(req.app_id.as_str()) {
        return err(409, "conflict", "app_id does not name the Space's App", json!({"reason": "app_mismatch"}));
    }
    if sp.governance.as_deref() != Some("app") {
        return err(409, "conflict", "not App-governed", json!({"reason": "not_app_governed"}));
    }
    let mut results = Vec::new();
    for op in &req.ops {
        s.next_id += 1;
        let fresh = 800_000 + s.next_id;
        let sp = s.spaces.get_mut(&req.space_id).expect("space");
        let slug = op
            .slug
            .clone()
            .or_else(|| op.channel_id.as_ref().and_then(|id| sp.channels.iter().find(|(_, c)| c.id.to_string() == *id).map(|(k, _)| k.clone())));
        let Some(slug) = slug.filter(|x| sew_wire::is_channel_slug(x)) else {
            results.push(json!({"op": op.op, "error": {"code": "not_found", "hint": "no such channel", "details": {"reason": "not_found"}}}));
            continue;
        };
        let r = match op.op.as_str() {
            "ensure" => {
                let kind = op.kind.clone().unwrap_or_else(|| "text".into());
                match sp.channels.get(&slug) {
                    Some(c) if (if c.kind.is_empty() { "text" } else { c.kind.as_str() }) != kind => {
                        json!({"op": "ensure", "slug": slug, "error": {"code": "conflict", "hint": "type", "details": {"reason": "channel_type_mismatch"}}})
                    }
                    Some(c) => json!({"op": "ensure", "slug": slug, "channel_id": c.id.to_string(), "name": c.name, "created": false}),
                    None => {
                        let c = Channel { id: fresh, name: op.name.clone().unwrap_or_else(|| slug.clone()), private: op.private.unwrap_or(false), kind, ..Default::default() };
                        let v = json!({"op": "ensure", "slug": slug, "channel_id": c.id.to_string(), "name": c.name, "created": true});
                        sp.channels.insert(slug.clone(), c);
                        v
                    }
                }
            }
            "rename" | "archive" | "unarchive" => match sp.channels.get_mut(&slug) {
                Some(c) => {
                    match op.op.as_str() {
                        "rename" => c.name = op.name.clone().unwrap_or_default(),
                        "archive" => c.archived = true,
                        _ => c.archived = false,
                    }
                    json!({"op": op.op, "slug": slug, "channel_id": c.id.to_string(), "name": c.name})
                }
                None => json!({"op": op.op, "slug": slug, "error": {"code": "not_found", "hint": "no such channel", "details": {"reason": "not_found"}}}),
            },
            _ => return bad_body(s, "channels/bulk", "op", body),
        };
        results.push(r);
    }
    (200, json!({"results": results}).to_string())
}

fn space_create(s: &mut State, body: &str) -> (u16, String) {
    use sew_client::proto::mgmt::MgmtSpaceCreateRequest;
    // Sew 0.6.0 §3a: an App-owned Space names `owner: "app:<app_id>"`,
    // which the vendored sew-proto (0.3.x) can't parse.
    let raw: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    if raw.get("owner").and_then(Value::as_str).is_some_and(|o| o.starts_with("app:")) {
        if !s.features.iter().any(|f| f == GOVERNED) {
            return bad_body(s, "spaces/create", "an app: owner without the flag", body);
        }
        let owner = raw["owner"].as_str().unwrap_or_default().to_string();
        let app = raw.get("app_id").and_then(Value::as_str).unwrap_or_default().to_string();
        let allowed = ["slug", "name", "owner", "managers", "app_id", "hidden"];
        if !raw.as_object().is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str()))) || app.is_empty() {
            return bad_body(s, "spaces/create", "fields", body);
        }
        if owner != format!("app:{app}") {
            return err(409, "conflict", "another App's service owner", json!({"reason": "app_mismatch"}));
        }
        s.next_id += 1;
        let space_id = format!("{}", 7000 + s.next_id);
        if !s.members.contains_key(&owner) {
            s.next_id += 1;
            let id = s.next_id;
            s.members.insert(
                owner.clone(),
                Member {
                    member_id: id,
                    external_id: owner.clone(),
                    kind: MemberKind::Person,
                    agent_kind: None,
                    assistant_of: None,
                    guest: false,
                    username: format!("app-{}", &app[..8.min(app.len())]),
                    display_name: app.clone(),
                    email: None,
                    title: None,
                    pronouns: None,
                    deactivated: false,
                },
            );
        }
        s.spaces.insert(
            space_id.clone(),
            Space { app_id: Some(app.clone()), owner: Some(owner.clone()), governance: Some("sew".into()), ..Default::default() },
        );
        let slug = raw.get("slug").and_then(Value::as_str).unwrap_or_default();
        let name = raw.get("name").and_then(Value::as_str).unwrap_or_default();
        return (200, json!({"space_id": space_id, "slug": slug, "name": name, "owner_member_id": "1", "app_id": app, "created": true, "manager_member_ids": []}).to_string());
    }
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

/// A chat pass's claims, unverified (the test checks the signature
/// against K2's JWKS itself); `{}` when it isn't a JWT.
fn pass_claims(pass: &str) -> Value {
    use base64::Engine;
    pass.split('.')
        .nth(1)
        .and_then(|p| base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(p).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| json!({}))
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
    let mut s_clears = Vec::new();
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
            // Sew 0.5.0 too (provisioning.rs profile_of + set_profile_in),
            // whatever the managed-fields policy says. Each clear of a
            // title/pronouns the member had is recorded ([`FakeSew::profile_clears`]).
            if (m.title.is_some() && req.profile.title.is_none()) || (m.pronouns.is_some() && req.profile.pronouns.is_none()) {
                s_clears.push(format!("{ext}: title {:?} pronouns {:?} cleared by {body}", m.title, m.pronouns));
            }
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
    s.profile_clears.extend(s_clears);
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
    // Sew 0.5.0: an orphan rebinds to the App it came from; another App
    // waits for a person's `spaces/release`.
    if let Some(from) = sp.orphaned_from.clone() {
        if from != req.app_id {
            return err(409, "conflict", "orphaned from another App", json!({"reason": "space_bound_elsewhere"}));
        }
        sp.orphaned_from = None;
    }
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
    if sp.orphaned_from.is_some() {
        return orphaned(&req.space_id);
    }
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

fn channel_create(s: &mut State, body: &str) -> (u16, String) {
    let req: ChannelCreateRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "channels/create", e, body),
    };
    if !s.features.iter().any(|f| f == "app_channel_create") {
        // A Sew without A9 doesn't have the route at all.
        s.bad_paths.push("POST /mgmt/v1/channels/create (no app_channel_create flag)".into());
        return err(404, "not_found", "unknown route", json!({"reason": "unknown_route"}));
    }
    if !sew_wire::is_app_id(&req.app_id) || !sew_wire::is_channel_slug(&req.slug) || req.name.is_empty() || req.members.len() > 100 {
        return bad_body(s, "channels/create", "app_id, slug, name or members", body);
    }
    let mut seen = std::collections::BTreeSet::new();
    if !req.members.iter().all(|m| seen.insert(m.clone())) {
        return bad_body(s, "channels/create", "a member listed twice", body);
    }
    let members = s.members.clone();
    s.next_id += 1;
    let fresh_id = 900_000 + s.next_id;
    let Some(sp) = s.spaces.get_mut(&req.space_id) else {
        return err(404, "not_found", "no such space", json!({}));
    };
    if sp.orphaned_from.is_some() {
        return orphaned(&req.space_id);
    }
    match &sp.app_id {
        None => return err(409, "conflict", "the Space is bound to no App", json!({"reason": "space_unbound", "field": "space_id"})),
        Some(a) if *a != req.app_id => {
            return err(409, "conflict", "app_id does not name the Space's App", json!({"reason": "app_mismatch", "field": "app_id"}))
        }
        _ => {}
    }
    // Sew 0.6.0: a repeat on a slug of the other type (K2 sends no `type`:
    // text) writes nothing (0.5.0 answered 200).
    if sp.channels.get(&req.slug).is_some_and(|c| c.kind == "voice") {
        return err(
            409,
            "conflict",
            "a channel of another type already has this slug",
            json!({"reason": "channel_type_mismatch", "field": "type", "existing_type": "voice"}),
        );
    }
    let created = !sp.channels.contains_key(&req.slug);
    let space_members = sp.members.clone();
    let banned = sp.banned.clone();
    let ch = sp.channels.entry(req.slug.clone()).or_insert_with(|| Channel {
        id: fresh_id,
        name: req.name.clone(),
        private: req.private,
        members: Default::default(),
        ..Default::default()
    });
    let (mut added, mut already, mut skipped, mut not_added) = (vec![], vec![], vec![], vec![]);
    for ext in &req.members {
        let Some(m) = members.get(ext) else {
            not_added.push(json!({"member": ext, "reason": "not_provisioned"}));
            continue;
        };
        let r = json!({"external_id": ext, "member_id": m.member_id.to_string()});
        if m.deactivated {
            not_added.push(json!({"member": ext, "reason": "deactivated"}));
        } else if banned.contains(ext) {
            skipped.push(m.member_id.to_string());
        } else if m.guest {
            not_added.push(json!({"member": ext, "reason": "guest"}));
        } else if !space_members.contains(ext) {
            not_added.push(json!({"member": ext, "reason": "not_space_member"}));
        } else if ch.members.insert(ext.clone()) {
            added.push(r);
        } else {
            already.push(r);
        }
    }
    (
        200,
        json!({
            "channel_id": ch.id.to_string(), "space_id": req.space_id, "slug": req.slug, "name": ch.name,
            "private": ch.private, "created": created, "added": added, "already": already,
            "skipped": skipped, "not_added": not_added,
        })
        .to_string(),
    )
}

fn space_member_remove(s: &mut State, body: &str) -> (u16, String) {
    let req: SpaceMemberRemoveRequest = match serde_json::from_str(body) {
        Ok(r) => r,
        Err(e) => return bad_body(s, "spaces/members/remove", e, body),
    };
    let Some(sp) = s.spaces.get_mut(&req.space_id) else {
        return err(404, "not_found", "no such space", json!({}));
    };
    // Real Sew 0.5.0 (probed on a VM, 2026-10-28): a remove still works on
    // an orphan, named by the App it was unbound from (adds and channel
    // creates answer 422 space_orphaned).
    let owner_app = sp.app_id.clone().or_else(|| sp.orphaned_from.clone());
    if req.app_id.is_some() && owner_app != req.app_id {
        return err(409, "conflict", "app_id does not name the Space's App", json!({"reason": "app_mismatch"}));
    }
    if req.app_id.is_none() && sp.app_id.is_some() {
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
    if req.external_id.is_none() && req.k2_sid.is_none() && req.app_id.is_none() {
        return bad_body(s, "sessions/revoke", "send external_id, k2_sid, app_id", body);
    }
    if req.app_id.is_some() && !s.features.iter().any(|f| f == "app_governed_spaces") {
        return bad_body(s, "sessions/revoke", "unknown field app_id (this Sew has no app_governed_spaces)", body);
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
