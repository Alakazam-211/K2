//! The catalog's daemon half (PRD rev 3.1 §5, §6.4, §12.3): one file per
//! E-lane area, each pairing its `k2_core::permission::catalog::ActionSpec`s
//! with `check` and `apply`.
//!
//! **Frozen at P0 (lane A1).** Every area file is pre-declared below and
//! exports `actions()` / `not_cardable()`; lanes fill only their own files
//! and never edit this one. Contract changes go through A1.
//!
//! | File | Lane | v1 rows |
//! |---|---|---|
//! | `switches.rs` | E1 | the Agent tab's switches (vault, DNS and mail included) + `to=off` |
//! | `connections.rs` | E1 | `connections.create`, `connections.add`; `connections.add-between` (0.46.18: connect two other workspaces, asked only) |
//! | `gardens.rs` | E1, Garden access A4 | `garden.keep-mine`; `garden.access` (+ `-off`), `garden.delete` |
//! | `held.rs` | E2 | `apps.approve`, `mail.approve`, `garden.proposal.approve`, `sources.approve` |
//! | `compute.rs` | E2 | `compute.admin-approve` (cut if late) |
//! | `sew.rs` | Sew (Rosson 2026-10-28; 0.46.11 K-a) | `sew.owner-handoff`, `sew.rollback-data-loss`, `sew.recording-enable` |
//! | `harness.rs` | 0.46.6 | `agent.harness-switch` (move the pinned chat to another harness) |
//! | `vault.rs` | 0.46.6 | `vault.delivery` (let the agent read one of its secrets as file or env), `vault.share-service` (give its own service or App one), `vault.use` (0.46.11: use a server's or another workspace's secret for a while) |
//! | `heartbeats.rs` | 0.46.6 | `heartbeat.pause-other`, `heartbeat.resume-other` (another workspace's heartbeat) |
//! | `app_login.rs` | 0.46.7 | `app.login-grant` (add or remove a login on the agent's own App) |
//! | `password_reset.rs` | 0.46.6 | `apps.user-password-reset` (reset an App user's password; the secret goes to the approver only) |
//! | `workbench.rs` | 0.46.7 | `git.land`, `sidecar.retire` (a person lands / retires an agent's worktree sidecar) |
//! | `publish.rs` | 0.46.7 | `publish.public`, `publish.pin` (who may open a published site) |
//! | `chat_cards.rs` | 0.46.7 | `apps.chat-key` (turn on or widen its App's chat key), `sew.space-governance` (switch who governs its App's Space) |
//! | `docs.rs` | Markdown multiplayer v1 | `docs.share` (MI25) |
//! | `chat_account.rs` | 0.46.8 | `chat.account` (the agent's own workspace's chat account) |
//! | `mail_stop.rs` | 0.46.8 | `mail.stop-sending` (turn off a workspace's agent email sending) |
//! | `box_login_grant.rs` | 0.46.8 | `apps.user-grant-box` (a box login's access to the agent's own App) |
//! | `chat_refresh.rs` | 0.46.8 | `chat.refresh` (a new pinned chat on the same harness, once or from now on) |
//! | `vault_incoming.rs` | 0.46.10 | `vault.incoming.accept` (accept or decline a secret another agent sent; K2 presents it) |
//! | `recording.rs` | 0.46.11 | `apps.recording` (its App requires recording, or its server reads transcripts; the owner only) |
//! | `vault_send.rs` | 0.46.11 | `vault.send` (a person sends a person-given secret to an agent on another server for the agent, once or from now on) |
//! | `vault_grant_server.rs` | 0.46.11 | `vault.grant-server` (share a server secret with the asking workspace, until removed) |
//! | `approve.rs` | 0.46.13 | `approve.request` (+ `-off`): let the agent ask people for signed approvals |
//! | `permission_approvers.rs` | 0.46.17 | `permission.approvers` (who agents' cards ask when the agent names no one) |
//! | `postal.rs` | 0.46.16 | `postal.invite`, `postal.pair.accept`, `postal.link-p5` (Postal friend requests both ways, and linking p5; K2 presents them) |
//! | `ci_preview.rs` | 0.46.17 | `ci.preview` (+ `-off`): turn on CI on K2 Compute for the server (`ciPreview`, the owner's) |
//! | `voice_recording.rs` | 0.46.17 | `voice.recording-start-level` (who starts recordings in the voice rooms), `voice.recording-dm` (recording in direct messages) |
//! | `focus_groups.rs` | 0.46.17 | `focus-groups.create`, `focus-groups.rename`, `focus-groups.assign` (a person creates, renames or fills a focus group for an agent) |
//! | `vault_owner_confirm.rs` | 0.46.17 | `vault.owner-confirm` (the owner answers their own terminal's vault confirm from the phone; K2 presents it) |
//! | `db_read.rs` | 0.46.17 | `db.read` (read-only access to another workspace's database: `k2 db grant <ws> --db <name> --level read`) |
//! | `llm_refresh.rs` | 0.46.17 | `llm.tokens-refresh` (a person rechecks the server's LLM tokens, as `k2 llm tokens refresh`) |
//!
//! Rules for every `apply` (§6.4, C15): call the Settings route's **entry**
//! with the answerer as the actor, so the route's own floor check runs
//! again at the click. Never the inner writer, never a second
//! implementation.

use k2_core::permission::catalog::{ActionSpec, NotCardableSpec};
use serde_json::{Map, Value};

pub(crate) mod app_login;
pub(crate) mod approve;
pub(crate) mod box_login_grant;
pub(crate) mod chat_account;
pub(crate) mod chat_cards;
pub(crate) mod chat_refresh;
pub(crate) mod ci_preview;
pub(crate) mod voice_recording;
pub(crate) mod compute;
pub(crate) mod db_read;
pub(crate) mod connections;
pub(crate) mod docs;
pub(crate) mod focus_groups;
pub(crate) mod gardens;
pub(crate) mod harness;
pub(crate) mod heartbeats;
pub(crate) mod held;
pub(crate) mod llm_refresh;
pub(crate) mod mail_stop;
pub(crate) mod password_reset;
pub(crate) mod permission_approvers;
pub(crate) mod postal;
pub(crate) mod publish;
pub(crate) mod recording;
pub(crate) mod sew;
pub(crate) mod switches;
pub(crate) mod test_guest_rooms;
pub(crate) mod vault;
pub(crate) mod vault_accept_from;
pub(crate) mod vault_incoming;
pub(crate) mod vault_grant_server;
pub(crate) mod vault_owner_confirm;
pub(crate) mod vault_send;
pub(crate) mod workbench;

/// The asking side, fixed at request time.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Ctx {
    /// The asking workspace id (always from the passport).
    pub workspace_id: String,
    /// Its handle, for the card words.
    pub workspace_handle: String,
    /// The action's values, as stored on the request.
    pub params: Map<String, Value>,
}

/// Who clicked, as the route entry's own caller (§6.4, §5.3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Answerer {
    /// The owner token (desktop app, terminal outside K2). Audited `via: owner-token`.
    OwnerToken,
    /// A Connect login with its role (`owner`, `admin`, `member`).
    Login { user: String, role: String },
}

impl Answerer {
    /// The confirm-token stamp the writer checks (`owner` / `connect:<user>`).
    pub fn stamp(&self) -> String {
        match self {
            Answerer::OwnerToken => "owner".to_string(),
            Answerer::Login { user, .. } => format!("connect:{user}"),
        }
    }
}

/// The result of `check`: at request time, at render (cached 2 s) and at
/// the click (§4.2, §6.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Check {
    /// The change is still needed and the values hold.
    Needed,
    /// The current Settings state already matches (`already_done`).
    AlreadyDone,
    /// Values that moved: widget version, held request decided elsewhere,
    /// workspace gone (`bad_params` at request time; `stale` at the click).
    Moved(String),
    /// The action isn't offered on this server right now, whatever the
    /// values: `not_available` (Garden access lists are off) or `retired`
    /// (Gardens use access lists now). 409 with `code` at request time;
    /// `stale` with `why` at the click; drawn as stored at render.
    Unavailable { code: &'static str, why: String },
}

/// The result of `apply`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Applied {
    /// Applied; the message goes on the card and into the agent's turn.
    Done { message: String },
    /// The route entry refused (floor, confirm, state): nothing applied,
    /// the request goes to `failed`.
    Refused { code: String, message: String },
}

impl Check {
    /// The texts with this server's names: a check may word a hint as a
    /// template ([`k2_core::brand::CLI_SLOT`], [`k2_core::brand::PRODUCT_SLOT`]),
    /// which the request's refusal sends beside the hint as `hintTemplate`
    /// (white-label C5). Everything else that reads a check (a card, the
    /// click, the cache, a raise) reads it through [`Registered::check_named`].
    pub(crate) fn filled(self) -> Check {
        match self {
            Check::Moved(t) => Check::Moved(k2_core::brand::fill_names(&t)),
            Check::Unavailable { code, why } => Check::Unavailable { code, why: k2_core::brand::fill_names(&why) },
            other => other,
        }
    }
}

impl Applied {
    /// The message with this server's names ([`Check::filled`]).
    pub(crate) fn filled(self) -> Applied {
        match self {
            Applied::Done { message } => Applied::Done { message: k2_core::brand::fill_names(&message) },
            Applied::Refused { code, message } => Applied::Refused { code, message: k2_core::brand::fill_names(&message) },
        }
    }
}

impl Registered {
    /// `check`, its texts with this server's names ([`Check::filled`]).
    pub(crate) fn check_named(&self, ctx: &Ctx) -> Check {
        (self.check)(ctx).filled()
    }

    /// `apply`, its message with this server's names ([`Applied::filled`]).
    pub(crate) fn apply_named(&self, ctx: &Ctx, who: &Answerer, confirm_token: Option<&str>) -> Applied {
        (self.apply)(ctx, who, confirm_token).filled()
    }
}

/// One catalog action with its daemon half.
pub(crate) struct Registered {
    pub spec: &'static ActionSpec,
    pub check: fn(&Ctx) -> Check,
    /// `confirm_token` is the Settings confirm the answer carried, if any.
    pub apply: fn(&Ctx, &Answerer, Option<&str>) -> Applied,
}

/// Every card-able action, in lane order.
pub(crate) fn all() -> Vec<Registered> {
    let mut out = Vec::new();
    out.extend(switches::actions());
    out.extend(connections::actions());
    out.extend(gardens::actions());
    out.extend(held::actions());
    out.extend(compute::actions());
    out.extend(sew::actions());
    out.extend(harness::actions());
    out.extend(vault::actions());
    out.extend(heartbeats::actions());
    out.extend(password_reset::actions());
    out.extend(workbench::actions());
    out.extend(app_login::actions());
    out.extend(publish::actions());
    out.extend(chat_cards::actions());
    out.extend(docs::actions());
    out.extend(chat_account::actions());
    out.extend(mail_stop::actions());
    out.extend(box_login_grant::actions());
    out.extend(chat_refresh::actions());
    out.extend(vault_incoming::actions());
    out.extend(recording::actions());
    out.extend(vault_send::actions());
    out.extend(vault_accept_from::actions());
    out.extend(test_guest_rooms::actions());
    out.extend(vault_grant_server::actions());
    out.extend(approve::actions());
    out.extend(permission_approvers::actions());
    out.extend(postal::actions());
    out.extend(ci_preview::actions());
    out.extend(voice_recording::actions());
    out.extend(focus_groups::actions());
    out.extend(db_read::actions());
    out.extend(vault_owner_confirm::actions());
    out.extend(llm_refresh::actions());
    out
}

/// Every row the lanes name as not card-able in v1 (§5.4, §9).
pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    let mut out = Vec::new();
    out.extend(switches::not_cardable());
    out.extend(connections::not_cardable());
    out.extend(gardens::not_cardable());
    out.extend(held::not_cardable());
    out.extend(compute::not_cardable());
    out.extend(sew::not_cardable());
    out.extend(harness::not_cardable());
    out.extend(vault::not_cardable());
    out.extend(heartbeats::not_cardable());
    out.extend(password_reset::not_cardable());
    out.extend(workbench::not_cardable());
    out.extend(app_login::not_cardable());
    out.extend(publish::not_cardable());
    out.extend(chat_cards::not_cardable());
    out.extend(docs::not_cardable());
    out.extend(chat_account::not_cardable());
    out.extend(mail_stop::not_cardable());
    out.extend(box_login_grant::not_cardable());
    out.extend(chat_refresh::not_cardable());
    out.extend(vault_incoming::not_cardable());
    out.extend(recording::not_cardable());
    out.extend(vault_send::not_cardable());
    out.extend(vault_accept_from::not_cardable());
    out.extend(test_guest_rooms::not_cardable());
    out.extend(vault_grant_server::not_cardable());
    out.extend(approve::not_cardable());
    out.extend(permission_approvers::not_cardable());
    out.extend(postal::not_cardable());
    out.extend(ci_preview::not_cardable());
    out.extend(voice_recording::not_cardable());
    out.extend(focus_groups::not_cardable());
    out.extend(db_read::not_cardable());
    out.extend(vault_owner_confirm::not_cardable());
    out.extend(llm_refresh::not_cardable());
    out
}

/// The registered action with this id.
pub(crate) fn find(id: &str) -> Option<Registered> {
    all().into_iter().find(|r| r.spec.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Holds for every lane as it fills its file: each spec validates, ids
    /// are unique, and no id is both card-able and not.
    #[test]
    fn the_registry_is_well_formed() {
        let all = all();
        for r in &all {
            k2_core::permission::catalog::validate(r.spec).unwrap_or_else(|e| panic!("{e}"));
        }
        let mut ids: Vec<&str> = all.iter().map(|r| r.spec.id).collect();
        ids.sort_unstable();
        let n = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), n, "duplicate action ids");
        for nc in not_cardable() {
            assert!(!ids.contains(&nc.id), "{} is both card-able and not", nc.id);
        }
        assert!(find("no.such-action").is_none());
    }

    #[test]
    fn the_answerer_stamp_matches_the_confirm_broker() {
        assert_eq!(Answerer::OwnerToken.stamp(), "owner");
        assert_eq!(Answerer::Login { user: "rosson".into(), role: "owner".into() }.stamp(), "connect:rosson");
    }

    /// A check worded as a template: everything that reads it gets this
    /// server's names, and the request's refusal also sends the template
    /// as `hintTemplate` (white-label C5). A text with no slot is as it was.
    #[test]
    fn a_check_s_template_takes_this_server_s_names() {
        let t = "ask with {cli} permission request a.b, from outside {product}";
        let want = k2_core::brand::fill_names(t);
        assert!(!want.contains('{'), "{want}");
        assert_eq!(Check::Moved(t.into()).filled(), Check::Moved(want.clone()));
        assert_eq!(Check::Unavailable { code: "c", why: t.into() }.filled(), Check::Unavailable { code: "c", why: want.clone() });
        assert_eq!(Applied::Refused { code: "c".into(), message: t.into() }.filled(), Applied::Refused { code: "c".into(), message: want.clone() });
        assert_eq!(Applied::Done { message: t.into() }.filled(), Applied::Done { message: want.clone() });
        let r = crate::permission::answer::named_err(400, "bad_params", t);
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!((v["error"]["hint"].as_str(), v["error"]["hintTemplate"].as_str()), (Some(want.as_str()), Some(t)), "{v}");
        let r = crate::permission::answer::named_err(400, "bad_params", "no names here");
        let v: Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!(v["error"]["hint"], "no names here");
        assert!(v["error"].get("hintTemplate").is_none(), "{v}");
    }
}
