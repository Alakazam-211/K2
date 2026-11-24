//! C · teach — the refusal wrapper (PRD rev 3.1 §3.1, §5.5; vs-live C2, C3,
//! C21).
//!
//! While the gate is on for the calling agent (both switches,
//! `k2_core::permission::gate::enabled_for`), every refusal it gets ends
//! with exactly one closing line on the CLI: `Ask with: k2 permission
//! request …` or `It can't be requested.` The daemon side of that is one
//! top-level `permission` object added to the refusal body:
//!
//! - a mapped wall (this file's [`ROWS`]): `{action, params, approvers,
//!   command}`;
//! - a machine-only refusal (a [`MACHINE_HINTS`] code): `{requestable:false,
//!   where:"at the machine", hint}` — a plain-text hint, never a card;
//! - any other 401/403 to a passport: `{requestable:false, where}`.
//!
//! **Off is today, byte for byte.** Nothing here runs unless
//! `gate::server_on()`; then only for a valid agent passport whose
//! workspace has its own switch on. People (owner token, logins), App
//! passes and bad tokens never get a `permission` object.
//!
//! **Where it hooks** (no handler is edited):
//! - TCP: `routes::http::send_response*` call [`teach_tcp`] with the
//!   request the dispatcher noted in `routes::pass_gate` (one task-local per
//!   request, set where the App-pass fence is set).
//! - Cell socket: the dispatch result ([`teach_cell`]) and, for a route that
//!   isn't an agent verb (today's codeless 403), [`cell_gated_body`]: a
//!   **valid** passport bound to that cell gets the teaching `owner_only`
//!   body, a bad token keeps today's text (C3).
//!
//! **The map.** One row per wall: the refusal's status and code (or the
//! held answer's field), the route prefix and method, optional conditions,
//! and where each value of the action comes from. The longest matching
//! prefix wins, so a specific wall (`/cli/sources/approve` →
//! `sources.approve`) beats its area's switch (`/cli/sources/` →
//! `gardens.manage`). Rows for routes even a switched-on agent can't use
//! (node removal, `compute local`, an apex attach) say "It can't be
//! requested."
//!
//! The E lanes register each action's `ActionSpec` (with its own `walls`);
//! the test [`tests::rows_agree_with_the_registry`] keeps this map and the
//! registry in step as they land.

use k2_core::brand::CLI_NAME;
use k2_core::{cli_name, product_name};
use std::borrow::Cow;
use std::collections::HashMap;

use k2_core::permission::catalog::{Floor, ParamFrom, WallMatch, WallMethod};
use k2_core::permission::request::{PermissionRefusal, WHERE_AT_MACHINE};
use serde_json::{Map, Value};

/// What a matching row teaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Target {
    /// Ask for this catalog action; its answer floor.
    Ask { action: &'static str, floor: Floor },
    /// Not requestable: a person does it, here.
    Cannot { r#where: &'static str },
}

/// One wall.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Row {
    pub target: Target,
    pub matches: WallMatch,
    /// Route prefix the refused request's path starts with.
    pub prefix: &'static str,
    pub method: WallMethod,
    /// Query/form params that must have these values (`action=add`).
    pub when: &'static [(&'static str, &'static str)],
    /// A JSON pointer that must exist in the refusal body (`mail.manage`'s
    /// `owner_only` carries `error.workspace`; the box-level one doesn't).
    pub needs: Option<&'static str>,
    /// `(key, sources)`: the first source with a value wins. A key with no
    /// value is printed as `key=<key>` for the agent to fill in.
    pub params: &'static [(&'static str, &'static [ParamFrom])],
    /// Values fixed by the wall itself (`db.agent-access level=write`).
    pub fixed: &'static [(&'static str, &'static str)],
    /// `(pointer, present)`: the refused request's JSON body must (or
    /// must not) have a value there (`reset-link` with `to` is the email
    /// mode, without it the link mode). Read for a match only, never as a
    /// value.
    pub body_has: Option<(&'static str, bool)>,
    /// Keys left out of the ask when no source has a value (the action
    /// picks one itself: `apps.user-password-reset app=`), instead of a
    /// `key=<key>` placeholder for the agent to fill in.
    pub optional: &'static [&'static str],
    /// `(pointer, value)`: the refused request's JSON body must have this
    /// string there (ASCII case aside), or a boolean spelled `true` /
    /// `false`: `app.login-grant` is `kind=app` only. Read for a match only.
    pub body_eq: &'static [(&'static str, &'static str)],
    /// A JSON pointer in the refused request's body that must name the
    /// asking agent's **own** workspace (a handle, name, path or id):
    /// `chat.account` is only ever its own account, and `mail.send-mode`
    /// only its own sending (0.46.8). Read for a match only.
    pub own_ws: Option<&'static str>,
    /// JSON pointers the refused request's body must have **no** value at:
    /// `compute.grant` is limits only, never `--ci` or `--gpu` (0.46.17).
    /// Read for a match only.
    pub body_lacks: &'static [&'static str],
}

const fn ask(action: &'static str, floor: Floor) -> Target {
    Target::Ask { action, floor }
}

const fn code(status: u16, code: &'static str) -> WallMatch {
    WallMatch::Code { status, code }
}

const CHAT_REFRESH_PARAMS: &[(&str, &[ParamFrom])] =
    &[("notes", &[ParamFrom::Response("/error/notes")]), ("mode", &[ParamFrom::Response("/error/mode")])];

/// `mail.address-cap` from `k2 hostmail config`'s body (0.46.18).
const MAIL_ADDRESS_CAP_PARAMS: &[(&str, &[ParamFrom])] =
    &[("target", &[ParamFrom::Body("/workspace")]), ("cap", &[ParamFrom::Body("/addressCap")])];

const ANY: WallMethod = WallMethod::Any;
const POST: WallMethod = WallMethod::Post;

const fn row(target: Target, matches: WallMatch, prefix: &'static str, method: WallMethod) -> Row {
    Row {
        target,
        matches,
        prefix,
        method,
        when: &[],
        needs: None,
        params: &[],
        fixed: &[],
        body_has: None,
        optional: &[],
        body_eq: &[],
        own_ws: None,
        body_lacks: &[],
    }
}

/// "a person does this in Settings" for never / dropped walls a broad row
/// would otherwise claim.
const IN_SETTINGS: &str = concat!("a person with the owner role does this, in Settings or from a terminal outside ", product_name!());

/// The box's firewall (0.46.6): the owner or an Admin, never an agent.
const FIREWALL_WHERE: &str =
    concat!("the owner or an admin does this, in Settings → Network → Custom Domains → Firewall or with ", cli_name!(), " firewall from a terminal outside ", product_name!());

/// `k2 desktop env` (0.46.12): the agent's own desktop only, one session of
/// a workspace at a time, and only while the Owner keeps virtual desktops on.
const DESKTOP_OWN_WHERE: &str = concat!(cli_name!(), " desktop env always gives you your own workspace's desktop: run it with no --desktop");
const DESKTOP_IN_USE_WHERE: &str =
    concat!("another session of your workspace holds the desktop: wait until it runs ", cli_name!(), " desktop env --release or its hold ends (the error says when)");
const DESKTOP_OFF_WHERE: &str =
    concat!("virtual desktops are off on this server: the owner turns them on in Settings → Remote Desktop or with ", cli_name!(), " desktop enable from a terminal outside ", product_name!());
const DESKTOP_ENV_WHERE: &str = concat!(cli_name!(), " desktop env is for agents: people open a Desktop tab, or use ", cli_name!(), " desktop start");

/// Remote desktop (0.46.11): people only; an agent can't see or drive the
/// screen, and can't ask to.
const DESKTOP_WHERE: &str =
    concat!("remote desktop is for people only: the owner turns it on and lets logins watch, in Settings → Remote Desktop or with ", cli_name!(), " desktop from a terminal outside ", product_name!());

/// K2's front on :443 (`k2 front`): the owner, an Admin or a shell tab.
const FRONT_WHERE: &str = concat!("the owner or an admin does this, with ", cli_name!(), " front over SSH or in a ", product_name!(), " shell tab");

/// The box's daily cap on new outside forward addresses: the owner's alone.
/// An agent must not be able to loosen its own limit, so mail.manage (the
/// `/cli/mail/` row) is not the answer here.
const FORWARD_CAP_WHERE: &str = concat!("the owner does this, with ", cli_name!(), " hostmail config --forward-new-per-day <n>");
const FORWARD_COPIES_DAY_WHERE: &str = concat!("the owner does this, with ", cli_name!(), " hostmail config --forward-copies-per-day <n>");
const FORWARD_COPIES_HOUR_WHERE: &str = concat!("the owner does this, with ", cli_name!(), " hostmail config --forward-copies-per-hour <n>");

/// Sew's home client (AG-21): the owner's alone, never an agent's; an
/// App's App-only mark off: the owner or an admin.
const HOME_CLIENT_WHERE: &str = concat!("the server's owner does this, with ", cli_name!(), " hostchat home client from a terminal outside ", product_name!(), " or in a ", product_name!(), " shell tab");
const HOME_APP_WHERE: &str =
    concat!("the owner or an admin turns App-only off, with ", cli_name!(), " hostchat home app from a terminal outside ", product_name!(), " or in a ", product_name!(), " shell tab");

/// The recording policies other than `recording.dm_enabled`: a body that
/// sets any of them is not the `voice.recording-dm` card's.
const RECORDING_DM_OTHERS: &[&str] =
    &["/changes/recording.enabled", "/changes/recording.require_consent_to_join", "/changes/recording.required", "/changes/recording.audio_max_days"];

/// The recording policies other than `recording.enabled`: a body that sets
/// any of them beside it is not the `sew.recording-enable` card's.
const RECORDING_ENABLED_OTHERS: &[&str] =
    &["/changes/recording.dm_enabled", "/changes/recording.require_consent_to_join", "/changes/recording.required", "/changes/recording.audio_max_days"];

/// The v1 card-able actions (PRD §5.2): the 17 Agent-tab switches, the 5
/// held-request actions, `garden.keep-mine` (K2-presented, no wall) and
/// the `to=off` twins (asked only, no wall), plus Garden access's three
/// (prd-garden-access-v1 G16, listed in every switch state): `garden.access`
/// and `garden.delete` (walled; K2 presents them too) and the
/// `garden.access-off` twin (asked only, no wall). Every [`Target::Ask`]
/// names one of them, never `garden.keep-mine` or a twin.
/// A slice, not a fixed-size array: two landings that each add an action
/// would both bump the length and merge cleanly to the wrong one. The test
/// below pins the count and that every id is listed once.
pub(crate) const V1_ACTIONS: &[&str] = &[
    "compute.use",
    "compute.operate",
    "compute.manage",
    "agents.manage",
    "projects.manage",
    "apps.build",
    "apps.manage-full",
    "apps.core",
    "chat.manage",
    "vault.manage",
    "gardens.manage",
    "dns.manage",
    "connections.create",
    "connections.add",
    "mail.manage",
    "mail.send-mode",
    "db.agent-access",
    "apps.approve",
    "mail.approve",
    "garden.proposal.approve",
    "sources.approve",
    "compute.admin-approve",
    "garden.keep-mine",
    "garden.access",
    "garden.access-off",
    "garden.delete",
    // 0.46.4 (Rosson 2026-10-26): "Let this agent manage its Apps' users".
    "apps.manage-users",
    // 0.46.5 S0 (Rosson 2026-10-26): sign a widget version whose manifest
    // changed (walled; K2 presents it too) and its twin (asked only).
    "zen.sign",
    "zen.sign-off",
    // 0.46.5 (prd-compute-job-tiers-v1 T13): CI jobs on one node.
    "compute.ci",
    // 0.46.6 (prd-mobile-dev-v1 M-A12): "Allow mobile devices".
    "sim.use",
    // 0.46.6 (prd-box-move-carry-v1 Q3): run a held box-move step.
    "vault.move-approve",
    // 0.46.6 (Rosson 2026-10-28): open Sew as its owner, for the owner.
    "sew.owner-handoff",
    // 0.46.6 (Rosson): move the agent's own pinned chat to another harness.
    "agent.harness-switch",
    "chat.refresh",
    // 0.46.6 (Rosson 2026-10-28): let the agent read one of its own
    // workspace's secrets as a file or env (a handed-over or person-given
    // secret it may not widen itself).
    "vault.delivery",
    // Let the agent give its own service or App a secret a person gave it.
    "vault.share-service",
    // An App's own agent: add or remove a login on its App.
    "app.login-grant",
    // 0.46.6: pause / resume another workspace's heartbeat.
    "heartbeat.pause-other",
    "heartbeat.resume-other",
    // 0.46.6 (Rosson 2026-10-28): the emergency Sew rollback across a
    // schema change, for the owner (chat since the upgrade leaves the
    // live chat; K2 backs it up first).
    "sew.rollback-data-loss",
    // 0.46.6 (Rosson 2026-10-29): reset one of the agent's Apps' users'
    // password; the approver alone sees the password or link.
    "apps.user-password-reset",
    // 0.46.7 Workbench W1 (Rosson 2026-10-29): a person lands an agent's
    // worktree sidecar on main, or retires it.
    "git.land",
    "sidecar.retire",
    // 0.46.7 (prd-publish-visibility-and-dev-mode-v1 PA-17): make a site
    // public (walled; K2 presents it) and put one behind a PIN (asked; the
    // person types it into K2's own form).
    "publish.public",
    "publish.pin",
    // 0.46.18: move a site's name another workspace on this server holds (a
    // server swap leaves the old workspace's) to the asking one (walled).
    "publish.transfer",
    // 0.46.7 App-governed Spaces: turn on or widen the agent's own App's
    // chat key, and switch who governs one of that App's Spaces.
    "apps.chat-key",
    "sew.space-governance",
    // 0.46.11 (Sew 0.9.0): its own App requires recording, or its server
    // reads transcripts (the owner only).
    "apps.recording",
    // Markdown multiplayer v1 (MI25): share a file or folder with another
    // workspace as live docs.
    "docs.share",
    // 0.46.8: add nodes to the agent's own compute node list, or remove
    // the list (narrowing it is the agent's own; widening is a person's).
    "compute.allow-nodes",
    "compute.allow-nodes-clear",
    // 0.46.18: a managing agent's change to ANOTHER workspace's list.
    "compute.allow-nodes-for",
    // prd-compute-blacksmith-class-v1 B2: add trusted branches to the
    // agent's own CI cache (removing one is the agent's own).
    "ci.cache-trust",
    // 0.46.8: the agent's own chat account (the `chat` grant), and turning
    // off a workspace's agent email sending (any workspace on the server).
    "chat.account",
    "mail.stop-sending",
    // 0.46.8: a box login's access to the agent's own App (the owner's on a
    // card; `app.login-grant` names it for a box login).
    "apps.user-grant-box",
    // 0.46.9 (prd-compute-self-serve-environments-v1 §13): install the
    // packages a filed request names, and set a node's package policy.
    "compute.packages",
    "compute.packages-policy",
    // 0.46.9 (GH#78): give the agent's own workspace's compute jobs
    // read access to GitHub repos (short-lived tokens, a person's grant).
    "compute.git-access",
    // 0.46.10: this server's GitHub credential, which every grant needs (a
    // person types it into K2's own form on the card).
    "compute.git-access-setup",
    // 0.46.11 (prd-compute-gpu-jobs-v1 S1): GPU jobs on one node.
    "compute.gpu",
    // 0.46.17: change the limits of the agent's own grant on one node
    // (`k2 compute grant` without --ci or --gpu).
    "compute.grant",
    // 0.46.11 (prd-vault-for-agents-v1 A6): use a server's or another
    // workspace's secret for a while.
    "vault.use",
    // 0.46.11 (K-a): install Sew's recorder and turn recording on (an
    // Owner or Admin, in person; the agent never installs it).
    "sew.recording-enable",
    // 0.46.17: who starts recordings in the voice rooms, and recording in
    // direct messages (an Owner or Admin, with a confirm).
    "voice.recording-start-level",
    "voice.recording-dm",
    // 0.46.11 (prd-org-chart-v1 §7): "Manage the org chart" (Owner).
    "organization.manage",
    // 0.46.11: a person sends a person-given secret to an agent on another
    // server for the agent (once, or lets it from now on).
    "vault.send",
    // 0.46.11: share a server secret with the asking workspace.
    "vault.grant-server",
    // 0.46.11 (Rosson 2026-11-10): "Read call transcripts" (Owner).
    "voice.transcripts-read",
    // 0.46.12 (prd-linux-virtual-desktop-v1 LD-38): "Allow a virtual desktop".
    "desktop.use",
    // 0.46.12 (Rosson): "Let this agent hire agents", split from
    // agents.manage (now its sidecars only).
    "agents.hire",
    // 0.46.13 (prd-signed-approvals-v1 S1): ask people for signed approvals.
    "approve.request",
    // 0.46.16 Postal (prd-postal-in-k2-v1 §5.5; Rosson 2026-11-18): every
    // step an agent takes to befriend another agent is its person's card.
    // K2 puts each up in the route's own answer; these lines find it.
    "postal.enable",
    "postal.claim",
    "postal.invite",
    "postal.pair.accept",
    "postal.link-p5",
    // 0.46.17: turn on CI on K2 Compute for the server (`ciPreview`, the
    // owner's). Its `ci.preview-off` twin is asked only.
    "ci.preview",
    // 0.46.17: a person rechecks the server's
    // LLM tokens for an agent (`k2 llm tokens refresh` is people only).
    "llm.tokens-refresh",
    // D1 (0.46.17): a person approves an agent's own review branch, and
    // makes its commit --merge and its plain commit.
    "git.review-approve",
    "git.commit-merge",
    "git.commit",
    // 0.46.17: a person creates or renames a focus group, or puts
    // a workspace in one, for an agent.
    "focus-groups.create",
    "focus-groups.rename",
    "focus-groups.assign",
    // 0.46.18: a person sets or clears a workspace's hosted mail address
    // cap for an agent (`k2 hostmail config --address-cap <n>|none`).
    "mail.address-cap",
];

/// `git.land`: the sidecar and its branch tip the refusal names.
const GIT_LAND_PARAMS: &[(&str, &[ParamFrom])] = &[
    ("sidecar", &[ParamFrom::Response("/error/sidecar"), ParamFrom::Body("/name")]),
    ("tip", &[ParamFrom::Response("/error/tip"), ParamFrom::Body("/expectTip")]),
];

/// `sidecar.retire`: the sidecar and the discard choice the refusal names.
const SIDECAR_RETIRE_PARAMS: &[(&str, &[ParamFrom])] = &[
    ("sidecar", &[ParamFrom::Response("/error/sidecar"), ParamFrom::Body("/name")]),
    ("discard", &[ParamFrom::Response("/error/discard")]),
];

/// `heartbeat.pause-other` / `-resume-other`: the workspace and heartbeat
/// the refusal names, else what the request named.
const HEARTBEAT_OTHER_PARAMS: &[(&str, &[ParamFrom])] = &[
    ("target", &[ParamFrom::Response("/error/workspace"), ParamFrom::Query("workspace")]),
    ("heartbeat", &[ParamFrom::Response("/error/heartbeat"), ParamFrom::Query("name")]),
];

/// `apps.user-password-reset`'s values from a refused reset: the person
/// (never the body's password) and, on a reset link, the App.
const PASSWORD_RESET_LINK_PARAMS: &[(&str, &[ParamFrom])] = &[
    ("user", &[ParamFrom::Body("/username"), ParamFrom::Body("/name")]),
    ("app", &[ParamFrom::Body("/app"), ParamFrom::Body("/appId")]),
];
const PASSWORD_RESET_SET_PARAMS: &[(&str, &[ParamFrom])] = &[("user", &[ParamFrom::Body("/username")])];

/// The password-reset card's floor, from its one constant (PR-A2).
const PASSWORD_RESET_FLOOR: Floor = super::actions::password_reset::FLOOR;

/// `chat.account`: the grants route's own floor (the owner).
const CHAT_ACCOUNT_FLOOR: Floor = super::actions::chat_account::FLOOR;

/// `app.login-grant`: the route's own floor, and its values from the
/// refused `k2 app grant add` body (never a password: there is none).
const LOGIN_GRANT_FLOOR: Floor = super::actions::app_login::FLOOR;
const LOGIN_GRANT_PARAMS: &[(&str, &[ParamFrom])] = &[
    ("user", &[ParamFrom::Body("/subjectId")]),
    ("app", &[ParamFrom::Body("/targetId")]),
    ("role", &[ParamFrom::Body("/roleId")]),
];

/// `k2 compute grant`'s values (0.46.17): the node and each limit it was
/// sent, `--max-time` as the duration typed (the CLI sends seconds).
const COMPUTE_GRANT_PARAMS: &[(&str, &[ParamFrom])] = &[
    ("node", &[ParamFrom::Body("/node")]),
    ("max-time", &[ParamFrom::BodySecs("/maxJobSecs")]),
    ("max-parallel", &[ParamFrom::Body("/maxParallel")]),
    ("max-queued", &[ParamFrom::Body("/maxQueued")]),
    ("max-disk", &[ParamFrom::Body("/maxDiskGb")]),
];

/// `compute.allow-nodes-for`'s values: the workspace and its one change.
const ALLOW_FOR_PARAMS: &[(&str, &[ParamFrom])] = &[
    ("target", &[ParamFrom::Response("/error/workspace")]),
    ("add", &[ParamFrom::Response("/error/add")]),
    ("remove", &[ParamFrom::Response("/error/remove")]),
    ("clear", &[ParamFrom::Response("/error/clear")]),
];

/// The `compute.allow-nodes-for` wall (each row adds the change it needs).
const ALLOW_FOR_ROW: Row = Row {
    params: ALLOW_FOR_PARAMS,
    optional: &["add", "remove", "clear"],
    ..row(ask("compute.allow-nodes-for", Floor::Admin), code(403, "allow_nodes_for_needs_person"), "/cli/compute/allow-nodes/set", POST)
};

/// The `compute.grant` wall: an agent's own grant change, limits only.
const COMPUTE_GRANT_ROW: Row = Row {
    params: COMPUTE_GRANT_PARAMS,
    optional: &["max-time", "max-parallel", "max-queued", "max-disk"],
    body_lacks: &["/allowCi", "/allowGpu", "/allowUserns", "/allowNoAddrFilter"],
    ..row(ask("compute.grant", Floor::Admin), code(403, "owner_only"), "/cli/compute/grants/set", POST)
};

/// `k2 settings set ciPreview true`'s body (0.46.17), and its route.
const CI_PREVIEW_ON: &[(&str, &str)] = &[("/ciPreview", "true")];
const CI_SETTINGS_ROUTE: &str = super::actions::ci_preview::SETTINGS_ROUTE;

/// What `POST /cli/settings/update` answers a passport over TCP
/// (`token_ok` refuses it before the handler): a flat `error` string.
pub(crate) const SETTINGS_TCP_REFUSAL: &str = "invalid or missing token";
/// That refusal's whole body (the dispatcher sends exactly this).
pub(crate) const SETTINGS_TCP_REFUSAL_BODY: &str = r#"{"error":"invalid or missing token"}"#;

/// `k2 compute access on`'s body: the compute toggle, turned on.
const COMPUTE_ACCESS_ON: &[(&str, &str)] = &[("/toggle", "compute"), ("/value", "true")];

/// The walls map (PRD §5.2 walls column, Appendix A, checked against the
/// off-goldens' captured refusals).
pub(crate) const ROWS: &[Row] = &[
    // ── published sites (0.46.7) ───────────────────────────────────────
    // An agent making its site public: K2 presents the card in the same answer.
    Row {
        params: &[("label", &[ParamFrom::Response("/label")]), ("reason", &[ParamFrom::Body("/reason")])],
        ..row(ask("publish.public", Floor::Admin), code(403, "public_needs_person"), "/cli/publish/visibility", POST)
    },
    // 0.46.18: a name another workspace on this server holds, on a claim, run
    // or start (409), and the same with --transfer (403, the owner's or an
    // Admin's): ask to move it to this workspace. Both bodies carry the label.
    Row {
        params: &[("label", &[ParamFrom::Response("/label"), ParamFrom::Query("label")])],
        ..row(ask("publish.transfer", Floor::Admin), code(409, crate::label_owner::CODE), "/cli/publish/", POST)
    },
    Row {
        params: &[("label", &[ParamFrom::Response("/label"), ParamFrom::Query("label")])],
        ..row(ask("publish.transfer", Floor::Admin), code(403, crate::label_owner::TRANSFER_CODE), "/cli/publish/", POST)
    },
    // ── compute ────────────────────────────────────────────────────────
    row(ask("compute.use", Floor::Admin), code(403, "compute_off"), "/cli/", ANY),
    // compute.agents is covered by compute.use (turning it on grants every
    // active node); so is a grant for another workspace, or one with --ci
    // or --gpu.
    row(ask("compute.use", Floor::Admin), code(403, "not_granted"), "/cli/compute/", ANY),
    row(ask("compute.use", Floor::Admin), code(403, "owner_only"), "/cli/compute/grants", ANY),
    // 0.46.17: the agent's own `k2 compute grant <node> --max-time …`
    // (limits only: no --ci, no --gpu) asks to change its own grant's
    // limits there, with the values it typed. The body names its own
    // workspace, or none (a passport acts for its own).
    Row { own_ws: Some("/workspace"), ..COMPUTE_GRANT_ROW },
    Row { body_has: Some(("/workspace", false)), ..COMPUTE_GRANT_ROW },
    row(ask("compute.use", Floor::Admin), code(403, "owner_only"), "/cli/compute/agents", ANY),
    // 0.46.9 (Rosson 2026-11-05): the agent's own `k2 compute access on`.
    // The switch's route refuses every passport (`forbidden`); turning on
    // its OWN workspace's "Allow compute nodes" asks for compute.use. The
    // body names that workspace, or none (a passport acts for its own).
    // Another workspace's, `off`, and every other toggle (`compute_manage`
    // among them) stay "It can't be requested."
    Row {
        body_eq: COMPUTE_ACCESS_ON,
        own_ws: Some("/workspace"),
        ..row(ask("compute.use", Floor::Admin), code(403, "forbidden"), "/cli/agent-access/set", POST)
    },
    Row {
        body_eq: COMPUTE_ACCESS_ON,
        body_has: Some(("/workspace", false)),
        ..row(ask("compute.use", Floor::Admin), code(403, "forbidden"), "/cli/agent-access/set", POST)
    },
    row(ask("compute.operate", Floor::Admin), code(403, "operate_off"), "/cli/", ANY),
    // `owner_or_manager` routes: "Allow managing compute nodes" lets the
    // agent through. Owner both on and off.
    row(ask("compute.manage", Floor::Owner), code(403, "owner_only"), "/cli/compute/", ANY),
    // `need_owner` only: no switch admits an agent.
    row(Target::Cannot { r#where: IN_SETTINGS }, code(403, "owner_only"), "/cli/compute/nodes/remove", ANY),
    // An agent always reaches its own workspace's caches; this refusal is
    // only for another workspace's. It answered compute.manage before, and
    // that card says "already on" when the switch is on (the loop).
    row(Target::Cannot { r#where: IN_SETTINGS }, code(403, "owner_only"), "/cli/compute/cache", ANY),
    row(Target::Cannot { r#where: IN_SETTINGS }, code(403, "owner_only"), "/cli/compute/local/", ANY),
    Row {
        params: &[("request", &[ParamFrom::Body("/request"), ParamFrom::Body("/id"), ParamFrom::Query("request")])],
        ..row(ask("compute.admin-approve", Floor::Owner), code(403, "owner_only"), "/cli/compute/admin/approve", POST)
    },
    // 0.46.9 (Rosson 2026-11-05): a held node request (`held: true`, its
    // `car_…` id) brings itself to the person's chat: a node admin
    // proposal, a policy widening, a share change, a wall on an older
    // node. Only an agent that may operate or manage nodes files one; the
    // card checks that again.
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(
            ask("compute.admin-approve", Floor::Owner),
            WallMatch::Held { status: 200, field: "held", value: "true" },
            "/cli/compute/admin/propose",
            POST,
        )
    },
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(
            ask("compute.admin-approve", Floor::Owner),
            WallMatch::Held { status: 200, field: "held", value: "true" },
            "/cli/compute/nodes/",
            POST,
        )
    },
    // Job tiers (0.46.5): `--tier ci` / `--gate` without a grant that
    // allows CI, and a compute manager setting `allowCi` on a grant.
    Row { params: &[("node", &[ParamFrom::Body("/node")])], ..row(ask("compute.ci", Floor::Admin), code(403, "ci_not_allowed"), "/cli/compute/", ANY) },
    // `--exclusive` reuses the grant's CI right (0.46.7, CE-13, D3).
    Row { params: &[("node", &[ParamFrom::Body("/node")])], ..row(ask("compute.ci", Floor::Admin), code(403, "exclusive_needs_grant"), "/cli/compute/run", POST) },
    // GPU jobs (0.46.11): `--gpu` where the node has a GPU for this server
    // but the grant lacks `--gpu` (the refusal names the node), and an
    // agent setting `allowGpu` on a grant.
    Row { params: &[("node", &[ParamFrom::Response("/error/node"), ParamFrom::Body("/node")])], ..row(ask("compute.gpu", Floor::Admin), code(403, "gpu_needs_grant"), "/cli/compute/", ANY) },
    // The workspace's compute node list (0.46.8): a job aimed at a node
    // off it (run, retry, prewarm; the refusal names the node), and the
    // agent's own list change that adds a node (`error.nodes`) or clears
    // the list (`error.clear`). Each refusal carries exactly one of the two.
    Row {
        params: &[("nodes", &[ParamFrom::Response("/error/nodes")])],
        ..row(ask("compute.allow-nodes", Floor::Admin), code(403, "node_not_allowed"), "/cli/compute/", ANY)
    },
    Row {
        params: &[("nodes", &[ParamFrom::Response("/error/nodes")])],
        needs: Some("/error/nodes"),
        ..row(ask("compute.allow-nodes", Floor::Admin), code(403, "allow_nodes_needs_person"), "/cli/compute/allow-nodes/set", POST)
    },
    Row {
        needs: Some("/error/clear"),
        ..row(ask("compute.allow-nodes-clear", Floor::Admin), code(403, "allow_nodes_needs_person"), "/cli/compute/allow-nodes/set", POST)
    },
    // 0.46.18: a managing agent's change to another workspace's list. The
    // refusal names the workspace and exactly one change (`error.add`,
    // `error.remove` or `error.clear`); a whole list at once names none
    // and can't be requested (the card takes one change).
    Row { needs: Some("/error/add"), ..ALLOW_FOR_ROW },
    Row { needs: Some("/error/remove"), ..ALLOW_FOR_ROW },
    Row { needs: Some("/error/clear"), ..ALLOW_FOR_ROW },
    // B2: the agent's own trusted-branch change that adds a branch (the
    // refusal names them in `error.branches`).
    Row {
        params: &[("branches", &[ParamFrom::Response("/error/branches")])],
        needs: Some("/error/branches"),
        ..row(ask("ci.cache-trust", Floor::Admin), code(403, "cache_trust_needs_person"), "/cli/ci/cache/trust/set", POST)
    },
    // Package requests (0.46.9, prd-compute-self-serve-environments-v1 §6):
    // a filed request held for this server's owner answers `pkgHeld: true`
    // (never `held`, which is a node admin request's), from the request
    // verb or a prewarm; a job refused `package_not_approved` names the
    // request it filed. Each teaches `compute.packages request=<cpr_…>`.
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(
            ask("compute.packages", Floor::Admin),
            WallMatch::Held { status: 200, field: "pkgHeld", value: "true" },
            "/cli/compute/nodes/packages/request",
            POST,
        )
    },
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(ask("compute.packages", Floor::Admin), WallMatch::Held { status: 200, field: "pkgHeld", value: "true" }, "/cli/compute/prewarm", POST)
    },
    Row {
        params: &[("request", &[ParamFrom::Response("/error/request")])],
        ..row(ask("compute.packages", Floor::Admin), code(403, "package_not_approved"), "/cli/compute/", ANY)
    },
    // The package policy route to an agent that may not operate nodes: the
    // owner sets the policy on a card (narrowing at once, widening held).
    Row {
        params: &[
            ("node", &[ParamFrom::Body("/node")]),
            ("server", &[ParamFrom::Body("/server")]),
            ("level", &[ParamFrom::Body("/level")]),
        ],
        ..row(ask("compute.packages-policy", Floor::Owner), code(403, "operate_off"), "/cli/compute/nodes/packages/policy", POST)
    },
    // Git access for compute jobs (0.46.9, GH#78): an agent's grant for
    // its own workspace teaches `compute.git-access repos=<the repos>`.
    Row {
        params: &[("repos", &[ParamFrom::Response("/error/repos")])],
        ..row(ask("compute.git-access", Floor::Admin), code(403, "git_access_needs_person"), "/cli/compute/git-access/grant", POST)
    },
    // 0.46.10: the same grant while the server has no GitHub credential,
    // and the agent's own `k2 compute git-access setup` (people only; the
    // `/cli/compute/` owner_only row would name compute.manage, which
    // can't set it), both teach `compute.git-access-setup`.
    row(ask("compute.git-access-setup", Floor::Admin), code(403, "git_access_needs_setup"), "/cli/compute/git-access/grant", POST),
    row(ask("compute.git-access-setup", Floor::Admin), code(403, "owner_only"), "/cli/compute/git-access/setup", POST),
    // ── CI on K2 Compute (0.46.17) ─────────────────────────────────────
    // Every CI surface answers 409 `ci_preview_off` while the server's
    // `ciPreview` is off (`k2 ci`, rosters, lanes, seats, signer pairing,
    // rightsize, stats…): the owner turns it on, on a card.
    row(ask("ci.preview", Floor::Owner), code(409, k2_core::compute::CI_PREVIEW_OFF), "/cli/", ANY),
    // The agent's own `k2 settings set ciPreview true`: agents never reach
    // `/cli/settings/*`. On the cell socket that is the teaching
    // `owner_only` (`cell_gated_body`); over TCP, `token_ok`'s flat
    // refusal. Only `ciPreview: true`: any other key (or `false`) stays
    // "It can't be requested".
    Row { body_eq: CI_PREVIEW_ON, ..row(ask("ci.preview", Floor::Owner), code(403, "owner_only"), CI_SETTINGS_ROUTE, POST) },
    Row { body_eq: CI_PREVIEW_ON, ..row(ask("ci.preview", Floor::Owner), code(403, SETTINGS_TCP_REFUSAL), CI_SETTINGS_ROUTE, POST) },
    // ── Sew (0.46.6)───────────────────────────────────────────────────
    // An agent's own call to the owner handoff: agents never become Sew's
    // install owner; the owner opens it for themselves on a card.
    row(ask("sew.owner-handoff", Floor::Owner), code(403, "owner_only"), "/cli/sew/owner-handoff", POST),
    // An agent's (or a K2 terminal's) emergency rollback: a person rolls
    // Sew back across a schema change; the owner answers on a card.
    row(ask("sew.rollback-data-loss", Floor::Owner), code(403, "rollback_needs_person"), "/cli/sew/server/rollback", POST),
    // 0.46.11 K-a: an agent's `k2 hostchat recording enable` (Manage Sew
    // or not past the switch): an Owner or Admin installs the recorder.
    row(ask("sew.recording-enable", Floor::Admin), code(403, "person_only"), "/cli/sew/recording/enable", POST),
    // 0.46.18 (HR-12, Sew 0.11.3): an agent's `k2 hostchat recording policy
    // set enabled on`, only when `changes` is that one key: the same card
    // (an Owner or Admin, with a confirm), which installs the recorder when
    // it isn't yet and turns the org policy on.
    Row {
        body_eq: &[("/changes/recording.enabled", "true")],
        body_lacks: RECORDING_ENABLED_OTHERS,
        ..row(ask("sew.recording-enable", Floor::Admin), code(403, "person_only"), crate::sew::recording_settings::POLICY, POST)
    },
    // 0.46.17: an agent's `k2 voice settings --start-level <level>` (its own
    // code; `--require-recording` stays the owner's, never a card).
    Row {
        params: &[("level", &[ParamFrom::Body("/startLevel")])],
        ..row(
            ask("voice.recording-start-level", Floor::Admin),
            code(403, crate::voice::routes::LEVEL_NEEDS_OWNER),
            crate::voice::routes::RECORDING_SETTINGS,
            POST,
        )
    },
    // 0.46.17: an agent's `k2 hostchat recording policy set dm_enabled on|off`,
    // only when `changes` is that one key (any other key in the same body:
    // no card, the person's own path).
    Row {
        body_eq: &[("/changes/recording.dm_enabled", "true")],
        body_lacks: RECORDING_DM_OTHERS,
        fixed: &[("dm", "on")],
        ..row(ask("voice.recording-dm", Floor::Admin), code(403, "person_only"), crate::sew::recording_settings::POLICY, POST)
    },
    Row {
        body_eq: &[("/changes/recording.dm_enabled", "false")],
        body_lacks: RECORDING_DM_OTHERS,
        fixed: &[("dm", "off")],
        ..row(ask("voice.recording-dm", Floor::Admin), code(403, "person_only"), crate::sew::recording_settings::POLICY, POST)
    },
    // The home client (AG-21): its sessions see every Space each person is
    // in, so no agent sets or clears it, Manage Sew or not: not requestable.
    row(Target::Cannot { r#where: HOME_CLIENT_WHERE }, code(403, "owner_only"), "/cli/sew/home/client", POST),
    // An App's App-only mark off widens the home client again.
    row(Target::Cannot { r#where: HOME_APP_WHERE }, code(403, "owner_only"), "/cli/sew/home/app", POST),
    // `k2 hostchat app-space governance <app> <space> app|sew` by an agent
    // ("Manage Sew" or not, AG-7): the App, the Space and the side come
    // from the refused call's own body.
    Row {
        params: &[("app", &[ParamFrom::Body("/app")]), ("space", &[ParamFrom::Body("/spaceId")]), ("to", &[ParamFrom::Body("/governance")])],
        ..row(ask("sew.space-governance", Floor::Admin), code(403, "person_only"), "/cli/sew/app-spaces/governance", POST)
    },
    // ── agents, projects ───────────────────────────────────────────────
    row(ask("agents.manage", Floor::Admin), code(403, "gated"), "/cli/sidecar/", ANY),
    // Workbench W1 (0.46.7): an agent's `k2 sidecar land` / `retire`; the
    // refusal names the sidecar, its tip and the discard choice.
    Row {
        params: GIT_LAND_PARAMS,
        ..row(ask("git.land", Floor::Admin), code(403, "land_needs_person"), "/cli/sidecar/land", POST)
    },
    Row {
        params: SIDECAR_RETIRE_PARAMS,
        ..row(ask("sidecar.retire", Floor::Admin), code(403, "retire_needs_person"), "/cli/sidecar/retire", POST)
    },
    // D1 (0.46.17): an agent's approve of its own review branch, and its
    // commit --merge, are a person's; the refusal names the branch.
    Row {
        params: &[("agent", &[ParamFrom::Response("/error/agent"), ParamFrom::Query("agent")]), ("branch", &[ParamFrom::Response("/error/branch"), ParamFrom::Query("branch")])],
        ..row(ask("git.review-approve", Floor::Admin), code(403, "merge_needs_person"), "/cli/review/approve", POST)
    },
    Row {
        params: &[("message", &[ParamFrom::Query("message")])],
        ..row(ask("git.commit-merge", Floor::Admin), code(403, "merge_needs_person"), "/cli/commit-merge", POST)
    },
    // 0.46.17: changing focus groups is a person's; the refusal
    // names the group and the workspace. A colour or order change carries
    // no `to`, so it teaches nothing (it can't be requested).
    Row {
        params: &[("name", &[ParamFrom::Response("/error/name")])],
        ..row(ask("focus-groups.create", Floor::Admin), code(403, "focus_group_needs_person"), "/cli/focus-groups/create", POST)
    },
    Row {
        needs: Some("/error/to"),
        params: &[("group", &[ParamFrom::Response("/error/group")]), ("to", &[ParamFrom::Response("/error/to")])],
        ..row(ask("focus-groups.rename", Floor::Admin), code(403, "focus_group_needs_person"), "/cli/focus-groups/update", POST)
    },
    Row {
        params: &[("target", &[ParamFrom::Response("/error/target")]), ("group", &[ParamFrom::Response("/error/group")])],
        ..row(ask("focus-groups.assign", Floor::Admin), code(403, "focus_group_needs_person"), "/cli/focus-groups/assign", POST)
    },
    // D1 (0.46.17): an agent's plain commit starts a person's agent preset.
    Row {
        params: &[("message", &[ParamFrom::Query("message")])],
        ..row(ask("git.commit", Floor::Admin), code(403, "commit_needs_person"), "/cli/commit", POST)
    },
    // `k2 sessions open` (prd-workspace-sessions-cli-v1 WS-X4): the same
    // switch as sidecar new. Its main-only refusal (`main_only`) can't be
    // requested.
    row(ask("agents.manage", Floor::Admin), code(403, "gated"), "/cli/sessions/open", POST),
    // 0.46.12: an agent's own `k2 agent hire` while "Let this agent hire
    // agents" is off. Its sidecar-only refusal (`main_only`) can't be
    // requested.
    row(ask("agents.hire", Floor::Admin), code(403, "hire_off"), "/cli/agents/hire", POST),
    // 0.46.13: `k2 approve request` while approve.request is off.
    row(ask("approve.request", Floor::Admin), code(403, "approve_request_off"), "/cli/approve/request", POST),
    // ── Postal (0.46.16, prd-postal-in-k2-v1 §5.5) ─────────────────────
    // Off: every Postal route refuses an agent with `postal_off`; the
    // server's owner turns it on.
    row(ask("postal.enable", Floor::Owner), code(403, "postal_off"), "/cli/postal/", ANY),
    // The route put the card up in the same answer (`asked: true`, exit 2):
    // the line asks for that same card (`already_pending`).
    row(ask("postal.enable", Floor::Owner), POSTAL_ASKED, "/cli/postal/enable", POST),
    Row {
        params: &[("address", &[ParamFrom::Body("/address")])],
        ..row(ask("postal.claim", Floor::Admin), POSTAL_ASKED, "/cli/postal/claim", POST)
    },
    Row {
        params: &[("to", &[ParamFrom::Body("/to")]), ("note", &[ParamFrom::Body("/why")])],
        ..row(ask("postal.invite", Floor::Admin), POSTAL_ASKED, "/cli/postal/invite", POST)
    },
    Row {
        params: &[("id", &[ParamFrom::Body("/id")])],
        ..row(ask("postal.pair.accept", Floor::Admin), POSTAL_ASKED, "/cli/postal/decide", POST)
    },
    row(ask("postal.link-p5", Floor::Owner), POSTAL_ASKED, "/cli/postal/link-p5", POST),
    // Another workspace's Postal address and friends: a person's.
    row(Target::Cannot { r#where: POSTAL_OTHER_WHERE }, code(403, "postal_own_workspace_only"), "/cli/postal/", ANY),
    row(ask("projects.manage", Floor::Admin), code(403, "agent_permission_off"), "/cli/", ANY),
    // `k2 agent harness` / `k2 workspace swap-canonical` on the agent's own
    // workspace (0.46.6): the refusal carries the normalized harness.
    Row {
        params: &[("harness", &[ParamFrom::Response("/error/harness"), ParamFrom::Body("/harness")])],
        ..row(ask("agent.harness-switch", Floor::Admin), code(403, "harness_needs_person"), "/cli/agent/harness", POST)
    },
    Row {
        params: &[("harness", &[ParamFrom::Response("/error/harness"), ParamFrom::Body("/provider")])],
        ..row(ask("agent.harness-switch", Floor::Admin), code(403, "harness_needs_person"), "/cli/workspace/swap-canonical", POST)
    },
    // `k2 agent refresh`, and swap-canonical on the same harness (0.46.8):
    // the refusal carries the kept note's hash; no note, no value.
    Row {
        params: CHAT_REFRESH_PARAMS,
        optional: &["notes", "mode"],
        ..row(ask("chat.refresh", Floor::Admin), code(403, "chat_refresh_needs_person"), "/cli/agent/refresh", POST)
    },
    Row {
        params: CHAT_REFRESH_PARAMS,
        optional: &["notes", "mode"],
        ..row(ask("chat.refresh", Floor::Admin), code(403, "chat_refresh_needs_person"), "/cli/workspace/swap-canonical", POST)
    },
    // `k2 heartbeat disable|enable <n> --workspace <another>` (0.46.6): the
    // refusal names the workspace and the heartbeat; `enabled=` (the CLI
    // sends true|false) picks pause or resume.
    Row {
        when: &[("enabled", "false")],
        params: HEARTBEAT_OTHER_PARAMS,
        ..row(ask("heartbeat.pause-other", Floor::Admin), code(403, "other_workspace"), "/cli/heartbeat/enable", POST)
    },
    Row {
        when: &[("enabled", "true")],
        params: HEARTBEAT_OTHER_PARAMS,
        ..row(ask("heartbeat.resume-other", Floor::Admin), code(403, "other_workspace"), "/cli/heartbeat/enable", POST)
    },
    // ── mobile devices (0.46.6) ────────────────────────────────────────
    row(ask("sim.use", Floor::Admin), code(403, "sim_switch_off"), "/cli/sim/", ANY),
    // ── Apps ───────────────────────────────────────────────────────────
    row(ask("apps.build", Floor::Owner), code(403, "apps_manage_off"), "/cli/", ANY),
    // App users (0225): the skin-manage door's refusal on a user route.
    row(ask("apps.manage-users", Floor::Owner), code(403, "app_users_off"), "/cli/", ANY),
    // 0.46.6: an agent's own `k2 app user reset` (the owner's route) or a
    // `k2 app user password` it may not do (a box login, a guest outside
    // its workspace): ask the owner to reset it on a card. `to` in the
    // body is the email mode. Never reads the body's password.
    Row {
        body_has: Some(("/to", true)),
        fixed: &[("mode", "email")],
        params: PASSWORD_RESET_LINK_PARAMS,
        optional: &["app"],
        ..row(ask("apps.user-password-reset", PASSWORD_RESET_FLOOR), code(403, "owner_only"), "/cli/skin/users/reset-link", POST)
    },
    Row {
        body_has: Some(("/to", false)),
        fixed: &[("mode", "link")],
        params: PASSWORD_RESET_LINK_PARAMS,
        optional: &["app"],
        ..row(ask("apps.user-password-reset", PASSWORD_RESET_FLOOR), code(403, "owner_only"), "/cli/skin/users/reset-link", POST)
    },
    Row {
        fixed: &[("mode", "password")],
        params: PASSWORD_RESET_SET_PARAMS,
        ..row(ask("apps.user-password-reset", PASSWORD_RESET_FLOOR), code(403, "owner_only"), "/cli/skin/users/password", POST)
    },
    // `k2 app grant add|rm … --kind app` by an App's own agent: the owner's
    // route (`owner_only`). Only kind=app with a principal; any other grant
    // stays "It can't be requested". A removal by grant id can't name the
    // person or the App: the agent fills them in from `k2 app grant list`.
    Row {
        body_eq: &[("/kind", "app"), ("/subjectKind", "principal")],
        fixed: &[("op", "add")],
        params: LOGIN_GRANT_PARAMS,
        optional: &["role"],
        ..row(ask("app.login-grant", LOGIN_GRANT_FLOOR), code(403, "owner_only"), "/cli/skin/grants", POST)
    },
    Row {
        body_has: Some(("/id", true)),
        fixed: &[("op", "remove")],
        params: &[("user", &[]), ("app", &[])],
        ..row(ask("app.login-grant", LOGIN_GRANT_FLOOR), code(403, "owner_only"), "/cli/skin/grants/delete", POST)
    },
    Row {
        body_eq: &[("/kind", "app"), ("/subjectKind", "principal")],
        fixed: &[("op", "remove")],
        params: &[("user", &[ParamFrom::Body("/subjectId")]), ("app", &[ParamFrom::Body("/targetId")])],
        ..row(ask("app.login-grant", LOGIN_GRANT_FLOOR), code(403, "owner_only"), "/cli/skin/grants/delete", POST)
    },
    // `k2 app grant <app> --kind chat-key` by an agent, turning the key on
    // or widening it: the route's `owner_only`. With cards on it names the
    // App and what was asked (`caps` comma-joined); without `app` (another
    // workspace's App, cards off) it isn't this card's wall.
    Row {
        needs: Some("/error/app"),
        params: &[
            ("app", &[ParamFrom::Response("/error/app")]),
            ("level", &[ParamFrom::Response("/error/level")]),
            ("caps", &[ParamFrom::Response("/error/caps")]),
            ("uploads", &[ParamFrom::Response("/error/uploads")]),
        ],
        optional: &["level", "caps", "uploads"],
        ..row(ask("apps.chat-key", Floor::Admin), code(403, "owner_only"), "/cli/app/chat-key", POST)
    },
    // 0.46.11 (Sew 0.9.0): `k2 app recording <app> set --required on` /
    // `--server-read on` by the App's own agent: the refusal names the App
    // and what it asked for; without `app` (another workspace's App, cards
    // off) it isn't this card's wall.
    Row {
        needs: Some("/error/app"),
        params: &[
            ("app", &[ParamFrom::Response("/error/app")]),
            ("required", &[ParamFrom::Response("/error/required")]),
            ("server-read", &[ParamFrom::Response("/error/serverRead")]),
        ],
        optional: &["required", "server-read"],
        ..row(ask("apps.recording", Floor::Owner), code(403, "owner_only"), "/cli/app/recording/set", POST)
    },
    // 0.46.8: an agent's own `k2 app grant add --kind chat --subject-kind
    // workspace --subject <its own workspace>` (the owner's route). Another
    // workspace's chat grant isn't this card's: it can't be requested.
    Row {
        body_eq: &[("/kind", "chat"), ("/subjectKind", "workspace")],
        own_ws: Some("/subjectId"),
        params: &[("scope", &[ParamFrom::Body("/scope")])],
        optional: &["scope"],
        ..row(ask("chat.account", CHAT_ACCOUNT_FLOOR), code(403, "owner_only"), "/cli/skin/grants", POST)
    },
    // Every App hold (switch 2 off, a core not ticked, new caps, a --cmd
    // service…) already made a held request: asking attaches a card to that
    // same id (§3.2, the agent sheet's HELD rule).
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(
            ask("apps.approve", Floor::Admin),
            WallMatch::Held { status: 202, field: "status", value: "held" },
            "/cli/app/",
            ANY,
        )
    },
    // An agent's held `k2 publish run` (a `publish_run` App request, 0.46.6
    // client papercut 12: it printed no Ask-with line).
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(
            ask("apps.approve", Floor::Admin),
            WallMatch::Held { status: 202, field: "status", value: "held" },
            "/cli/publish/run",
            POST,
        )
    },
    Row {
        params: &[("request", &[ParamFrom::Body("/request"), ParamFrom::Body("/id"), ParamFrom::Query("id")])],
        ..row(ask("apps.approve", Floor::Admin), code(403, "owner_only"), "/cli/app/approve", POST)
    },
    // ── Sew, vault ─────────────────────────────────────────────────────
    // A box-move step an agent ran is held for the owner (`mv_…`,
    // prd-box-move-carry-v1 Q3).
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(
            ask("vault.move-approve", Floor::Owner),
            WallMatch::Held { status: 202, field: "status", value: "held" },
            "/cli/migrate/secrets/",
            POST,
        )
    },
    row(ask("chat.manage", Floor::Admin), code(403, "chat_manage_off"), "/cli/", ANY),
    // 0.46.8 (Rosson 2026-11-02: the broad switch first). A `k2 chat` read
    // with no chat account: "Manage chat and voice" while it's off
    // (`details.chatManage`; `k2 chat space create` already stops at
    // `chat_manage_off`). With the switch on and the account still not
    // there: `chat.account`, for that read (`details.chatAccount`) and for
    // create (`chat_account_needed`, refused before any Sew call). Plain
    // `not_granted`, no Space access, is a person's: it can't be requested.
    Row {
        needs: Some("/error/details/chatManage"),
        ..row(ask("chat.manage", Floor::Admin), code(403, "not_granted"), "/cli/sew/chat/", ANY)
    },
    Row {
        fixed: &[("scope", "coworker")],
        ..row(ask("chat.account", CHAT_ACCOUNT_FLOOR), code(403, "chat_account_needed"), "/cli/sew/chat/manage/space/create", POST)
    },
    Row {
        needs: Some("/error/details/chatAccount"),
        fixed: &[("scope", "coworker")],
        ..row(ask("chat.account", CHAT_ACCOUNT_FLOOR), code(403, "not_granted"), "/cli/sew/chat/", ANY)
    },
    row(ask("vault.manage", Floor::Admin), code(403, "vault_manage_required"), "/cli/", ANY),
    // `k2 vault delivery NAME file|env` on a secret the agent may not widen
    // (2026-10-28): the refusal names the secret and the delivery.
    Row {
        needs: Some("/error/delivery"),
        params: &[("secret", &[ParamFrom::Response("/error/secret")]), ("delivery", &[ParamFrom::Response("/error/delivery")])],
        ..row(ask("vault.delivery", Floor::Admin), code(403, "vault_owner_confirm_required"), "/cli/vault/policy", POST)
    },
    // `k2 publish secret add` / `k2 vault grant --to service:` on a secret
    // the agent may not widen: the refusal names the secret, the service and
    // the delivery only when a card can do it (its own workspace's secret,
    // file or env); any other shape of this refusal isn't requestable.
    Row {
        needs: Some("/error/service"),
        params: &[
            ("secret", &[ParamFrom::Response("/error/secret")]),
            ("service", &[ParamFrom::Response("/error/service")]),
            ("as", &[ParamFrom::Response("/error/as")]),
            ("env_name", &[ParamFrom::Response("/error/envName")]),
            ("file_name", &[ParamFrom::Response("/error/fileName")]),
        ],
        optional: &["env_name", "file_name"],
        ..row(ask("vault.share-service", Floor::Admin), code(403, "vault_owner_confirm_required"), "/cli/vault/grant", POST)
    },
    // `k2 vault fetch|file|env-file --scope server|ws:<name>` on a secret the
    // agent can't see: the same refusal whether or not the name exists
    // (`vault_use_routes::out_of_reach`). `for=` starts at the shortest.
    Row {
        params: &[
            ("secret", &[ParamFrom::Response("/error/secret")]),
            ("from", &[ParamFrom::Response("/error/from")]),
            ("as", &[ParamFrom::Response("/error/as")]),
        ],
        fixed: &[("for", "1h")],
        ..row(ask("vault.use", Floor::Admin), code(403, crate::vault_use_routes::OUT_OF_REACH), "/cli/vault/", POST)
    },
    // `k2 vault send NAME --to agent::host` of a secret a person gave (R3,
    // 0.46.11): the refusal names the secret, the target and the note.
    Row {
        params: &[
            ("secret", &[ParamFrom::Response("/error/secret")]),
            ("to", &[ParamFrom::Response("/error/to")]),
            ("note", &[ParamFrom::Response("/error/note")]),
        ],
        optional: &["note"],
        ..row(ask("vault.send", Floor::Admin), code(403, "held"), "/cli/vault/send", POST)
    },
    // An agent's `k2 vault grant NAME --scope server --to ws:<itself>` of a
    // secret it can't see (0.46.11): one answer whether or not it exists.
    Row {
        params: &[("secret", &[ParamFrom::Response("/error/secret")]), ("as", &[ParamFrom::Response("/error/as")])],
        optional: &["as"],
        ..row(ask("vault.grant-server", Floor::Admin), code(403, crate::vault_routes::GRANT_OUT_OF_REACH), "/cli/vault/grant", POST)
    },
    // ── Gardens and sources ────────────────────────────────────────────
    row(ask("gardens.manage", Floor::Admin), code(403, "person_only"), "/cli/sources/", ANY),
    row(ask("gardens.manage", Floor::Admin), code(403, "person_only"), "/cli/zen/", ANY),
    // prd-garden-board-needs-v1 GH6 (R2): an agent's whole-space restore is
    // the person's own (`k2 zen data restore`, Settings → Local Data); the
    // switch wouldn't lift it, so nothing to ask for.
    row(Target::Cannot { r#where: WHERE_AT_MACHINE }, code(403, "person_only"), "/cli/zen/data/restore", POST),
    // The approve body is `{name, specHash}`: the card is bound to the spec
    // the agent saw (`spec=`, lane E2).
    Row {
        params: &[("source", &[ParamFrom::Body("/name")]), ("spec", &[ParamFrom::Body("/specHash")])],
        ..row(ask("sources.approve", Floor::Admin), code(403, "person_only"), "/cli/sources/approve", POST)
    },
    // A batch (`{items: [{name, specHash}…]}`) is one ask with comma lists;
    // the agent fills them in from `k2 sources pending`.
    Row {
        params: &[("source", &[]), ("spec", &[])],
        ..row(ask("sources.approve", Floor::Admin), code(403, "person_only"), "/cli/sources/batch-approve", POST)
    },
    Row {
        params: &[("proposal", &[ParamFrom::Body("/id"), ParamFrom::Body("/proposal"), ParamFrom::Query("id")])],
        ..row(
            ask("garden.proposal.approve", Floor::GardenOwner),
            code(403, "person_only"),
            "/cli/zen/proposals/approve",
            POST,
        )
    },
    // Garden access (prd-garden-access-v1 G11, G12, G16): an unlisted agent
    // on a Garden or widget write names the Garden (`garden`; null for the
    // computer-wide look, which any Garden's list opens). Deleting is the
    // person's: a passport's `garden/delete` is `garden_delete_person`, not
    // `person_only` (which teaches gardens.manage under /cli/zen/).
    Row {
        params: &[(
            "garden",
            &[ParamFrom::Response("/garden"), ParamFrom::Response("/error/garden"), ParamFrom::Body("/garden"), ParamFrom::Query("garden")],
        )],
        ..row(ask("garden.access", Floor::GardenOwner), code(403, "garden_access_needed"), "/cli/zen/", ANY)
    },
    Row {
        params: &[("garden", &[ParamFrom::Body("/garden"), ParamFrom::Response("/garden"), ParamFrom::Query("garden")])],
        ..row(ask("garden.delete", Floor::GardenOwner), code(403, "garden_delete_person"), "/cli/zen/garden/delete", POST)
    },
    // 0.46.5 S0: an agent's sign that can't cover a placed widget's version
    // (it isn't on a placing Garden's list) names the widget and the exact
    // version; K2 presents the zen.sign card in the same answer.
    Row {
        params: &[
            ("widget", &[ParamFrom::Response("/widget"), ParamFrom::Body("/widget")]),
            ("version", &[ParamFrom::Response("/version")]),
        ],
        ..row(ask("zen.sign", Floor::GardenOwner), code(403, "sign_needs_person"), "/cli/zen/widget/sign", POST)
    },
    // ── DNS, domains ───────────────────────────────────────────────────
    row(ask("dns.manage", Floor::Admin), code(403, "dns_manage_disabled"), "/cli/", ANY),
    // An apex attach/remove is owner/admin only for every agent, switch or
    // not (`dispatcher.rs` passport arm): not requestable.
    row(Target::Cannot { r#where: IN_SETTINGS }, code(403, "owner_only"), "/cli/domains", ANY),
    // ── the box's firewall (0.46.6) ────────────────────────────────────
    // Who can reach the box is box-level (`NeverReason::BoxLevel`): no
    // agent opens, narrows or passes through anything, so not requestable.
    row(Target::Cannot { r#where: FIREWALL_WHERE }, code(403, "owner_only"), "/cli/firewall/", POST),
    row(Target::Cannot { r#where: FIREWALL_WHERE }, code(403, "owner_only"), "/cli/front/passthrough", POST),
    // `k2 front status|enable|disable`: the same people, reads included.
    row(Target::Cannot { r#where: FRONT_WHERE }, code(403, "owner_only"), "/cli/front/", ANY),
    // ── remote desktop (0.46.11) ───────────────────────────────────────
    // People only, reads included: never requestable.
    row(Target::Cannot { r#where: DESKTOP_WHERE }, code(403, "desktop_people_only"), "/cli/desktop/", ANY),
    // `k2 desktop env` (0.46.12, LD-19): the agent's own switch is
    // requestable; the rest isn't (LD-29, LD-33, the Owner's switch).
    row(ask("desktop.use", Floor::Admin), code(403, "desktop_switch_off"), "/cli/desktop/env", POST),
    row(Target::Cannot { r#where: DESKTOP_OWN_WHERE }, code(403, "not_your_desktop"), "/cli/desktop/env", POST),
    row(Target::Cannot { r#where: DESKTOP_IN_USE_WHERE }, code(403, "desktop_in_use"), "/cli/desktop/env", POST),
    row(Target::Cannot { r#where: DESKTOP_OFF_WHERE }, code(403, "desktop_off"), "/cli/desktop/env", POST),
    row(Target::Cannot { r#where: DESKTOP_ENV_WHERE }, code(403, "desktop_env_agents_only"), "/cli/desktop/env", POST),
    // ── the Org Chart (0.46.11) ─────────────────────────────────────────
    // An agent without "Manage the org chart" on a manage verb.
    row(ask("organization.manage", Floor::Owner), code(403, "org_manage_off"), "/cli/organization/", ANY),
    // ── call transcripts (0.46.11) ──────────────────────────────────────
    // An agent without "Read call transcripts" on a recordings read.
    row(ask("voice.transcripts-read", Floor::Owner), code(403, "transcripts_read_off"), "/cli/voice/", ANY),
    // ── connections ────────────────────────────────────────────────────
    row(ask("connections.create", Floor::Admin), code(403, "agents_create_connections_disabled"), "/cli/", ANY),
    Row {
        when: &[("action", "add")],
        params: &[("to", &[ParamFrom::Query("target"), ParamFrom::Query("to"), ParamFrom::Body("/target")])],
        ..row(
            ask("connections.add", Floor::Admin),
            code(403, "agents_create_connections_disabled"),
            "/cli/connections",
            ANY,
        )
    },
    Row {
        params: &[(
            "to",
            &[
                ParamFrom::Query("workspace"),
                ParamFrom::Query("to"),
                ParamFrom::Query("target"),
                ParamFrom::Body("/workspace"),
                ParamFrom::Body("/to"),
            ],
        )],
        ..row(ask("connections.add", Floor::Admin), code(403, "not_connected"), "/cli/", ANY)
    },
    // ── live docs (MI25) ───────────────────────────────────────────────
    // Connected but not shared: the refusal names the workspace and the
    // workspace-relative path it checked.
    Row {
        params: &[
            ("from", &[ParamFrom::Response("/error/workspace")]),
            ("path", &[ParamFrom::Response("/error/path")]),
        ],
        ..row(ask("docs.share", Floor::Admin), code(403, "doc_not_shared"), "/cli/docs/", ANY)
    },
    // A board file in another workspace: the same share, the same refusal.
    Row {
        params: &[
            ("from", &[ParamFrom::Response("/error/workspace")]),
            ("path", &[ParamFrom::Response("/error/path")]),
        ],
        ..row(ask("docs.share", Floor::Admin), code(403, "doc_not_shared"), "/cli/board/file/", ANY)
    },
    // ── mail ───────────────────────────────────────────────────────────
    // The mail-manage surfaces' refusal names the workspace it checked; the
    // owner-level mutations (server install/uninstall, domain remove) don't.
    Row {
        needs: Some("/error/workspace"),
        ..row(ask("mail.manage", Floor::Admin), code(403, "owner_only"), "/cli/mail/", ANY)
    },
    Row {
        needs: Some("/error/workspace"),
        ..row(ask("mail.manage", Floor::Admin), code(403, "owner_only"), "/cli/hostmail/", ANY)
    },
    Row {
        fixed: &[("mode", "approval")],
        ..row(ask("mail.send-mode", Floor::Admin), code(403, "gated"), "/cli/mail/", ANY)
    },
    // 0.46.17: `k2 llm tokens refresh` by an agent (people only): a person
    // rechecks the token it named, or every token.
    Row {
        params: &[("token", &[ParamFrom::Body("/id")])],
        optional: &["token"],
        ..row(ask("llm.tokens-refresh", Floor::Admin), code(403, "owner_only"), "/cli/llm/accounts/refresh", POST)
    },
    // 0.46.8: `k2 mail sending <ws> off` by an agent (no "manage hosted
    // mail", or its own workspace with it): any workspace on the server,
    // its own too. Turning off only tightens; a person allows it.
    Row {
        body_eq: &[("/agentSend", "off")],
        params: &[("target", &[ParamFrom::Body("/workspace")])],
        ..row(ask("mail.stop-sending", Floor::Admin), code(403, "owner_only"), "/cli/mail/config/set", POST)
    },
    // `k2 mail sending <its own workspace> approval|on`: the agent's own
    // "Let this agent send email" switch. Another workspace's stays as it
    // was (the mail.manage row, or "It can't be requested").
    Row {
        body_eq: &[("/agentSend", "approval")],
        own_ws: Some("/workspace"),
        fixed: &[("mode", "approval")],
        ..row(ask("mail.send-mode", Floor::Admin), code(403, "owner_only"), "/cli/mail/config/set", POST)
    },
    Row {
        body_eq: &[("/agentSend", "on")],
        own_ws: Some("/workspace"),
        fixed: &[("mode", "on")],
        ..row(ask("mail.send-mode", Floor::Admin), code(403, "owner_only"), "/cli/mail/config/set", POST)
    },
    // 0.46.18: a workspace's address cap. `k2 hostmail config --workspace
    // <ws> --address-cap <n>|none` by an agent without "manage hosted mail"
    // (any workspace), or with it on its own workspace (the owner's cap),
    // alone: with agentSend or always-BCC in the same call it's theirs.
    Row {
        body_has: Some(("/addressCap", true)),
        body_lacks: &["/agentSend", "/alwaysBcc"],
        params: MAIL_ADDRESS_CAP_PARAMS,
        ..row(ask("mail.address-cap", Floor::Admin), code(403, "agent_send_path_only"), "/cli/mail/config/set", POST)
    },
    Row {
        body_has: Some(("/addressCap", true)),
        body_lacks: &["/agentSend", "/alwaysBcc"],
        own_ws: Some("/workspace"),
        params: MAIL_ADDRESS_CAP_PARAMS,
        ..row(ask("mail.address-cap", Floor::Admin), code(403, "owner_only"), "/cli/mail/config/set", POST)
    },
    // `k2 hostmail create` past the cap: the refusal names the workspace;
    // the agent fills in the cap it needs (the refusal never says one, so
    // the line reads `cap=<cap>`).
    Row {
        params: &[("target", &[ParamFrom::Response("/error/workspace")]), ("cap", &[ParamFrom::Body("/addressCap")])],
        ..row(ask("mail.address-cap", Floor::Admin), code(409, "cap_reached"), "/cli/mail/address/create", POST)
    },
    Row {
        params: &[("message", &[ParamFrom::Response("/id")])],
        ..row(
            ask("mail.approve", Floor::Admin),
            WallMatch::Held { status: 200, field: "status", value: "pending_approval" },
            "/cli/mail/",
            ANY,
        )
    },
    Row {
        params: &[("message", &[ParamFrom::Body("/id"), ParamFrom::Query("id")])],
        ..row(ask("mail.approve", Floor::Admin), code(403, "owner_only"), "/cli/mail/approvals/approve", POST)
    },
    row(Target::Cannot { r#where: FORWARD_CAP_WHERE }, code(403, "owner_only"), "/cli/mail/config/forward-new-per-day", POST),
    row(Target::Cannot { r#where: FORWARD_COPIES_DAY_WHERE }, code(403, "owner_only"), "/cli/mail/config/forward-copies-per-day", POST),
    row(Target::Cannot { r#where: FORWARD_COPIES_HOUR_WHERE }, code(403, "owner_only"), "/cli/mail/config/forward-copies-per-hour", POST),
    // ── database ───────────────────────────────────────────────────────
    Row {
        fixed: &[("level", "write")],
        ..row(ask("db.agent-access", Floor::Admin), code(403, "forbidden"), "/cli/db/", ANY)
    },
];

/// Machine-only refusal codes (PRD §5.5): a plain-text hint, no card.
/// `{node}` is the refused request's node, else "that machine".
pub(crate) const MACHINE_HINTS: &[(&str, &str)] = &[(
    "remote_admin_off",
    "At {node}, run: sudo k2-node admin level policy   (switch back after: sudo k2-node admin level off)",
)];

/// A Postal route that put its card up in the same answer.
const POSTAL_ASKED: WallMatch = WallMatch::Held { status: 200, field: "asked", value: "true" };

/// Another workspace's Postal: a person's, in that workspace's tab.
const POSTAL_OTHER_WHERE: &str = "a person does this, in that workspace's Postal tab (Settings → Workspaces → <name> → Postal)";

/// The `--why` placeholder in every Ask-with command.
pub(crate) const WHY_PLACEHOLDER: &str = "<what you need it for>";

/// Generic `where` for a passport refusal no row names.
pub(crate) const WHERE_DEFAULT: &str = concat!("a person does this, in Settings or from a terminal outside ", product_name!());

/// The refused request, as the wrapper sees it.
pub(crate) struct Refused<'a> {
    pub status: u16,
    pub method: &'a str,
    pub path: &'a str,
    /// Query + form params.
    pub params: &'a HashMap<String, String>,
    /// The request's JSON body, if it had one.
    pub body: Option<&'a Value>,
    /// The refusal (or held answer), parsed.
    pub response: &'a Value,
    /// The asking agent's workspace id (`own_ws` rows); empty = unknown,
    /// and no `own_ws` row matches.
    pub workspace_id: &'a str,
}

/// The status code of a `"403 Forbidden"` status line.
pub(crate) fn status_code(status: &str) -> u16 {
    status.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// The refusal's code: `error.code`, a flat `error` string, or `code`.
fn refusal_code(v: &Value) -> Option<&str> {
    match v.get("error") {
        Some(Value::Object(e)) => e.get("code").and_then(Value::as_str),
        Some(Value::String(s)) => Some(s.as_str()),
        _ => v.get("code").and_then(Value::as_str),
    }
}

fn method_ok(want: WallMethod, method: &str) -> bool {
    match want {
        WallMethod::Any => true,
        WallMethod::Get => method.eq_ignore_ascii_case("GET"),
        WallMethod::Post => method.eq_ignore_ascii_case("POST"),
    }
}

/// A flat `error` sentence with its code beside it (`{"code":
/// "label_owned_by_other_workspace","error":"label_owned_by_other_workspace:
/// 'shop' belongs to …"}`): that top-level `code` names the refusal too.
fn side_code(v: &Value) -> Option<&str> {
    match v.get("error") {
        Some(Value::String(_)) => v.get("code").and_then(Value::as_str),
        _ => None,
    }
}

fn row_matches(r: &Row, req: &Refused<'_>, code: Option<&str>) -> bool {
    let hit = match r.matches {
        WallMatch::Code { status, code: c } => req.status == status && (code == Some(c) || side_code(req.response) == Some(c)),
        WallMatch::Held { status, field, value } => {
            req.status == status
                && match req.response.get(field) {
                    Some(Value::String(s)) => s == value,
                    // `held: true` (a held compute node request, 0.46.9).
                    Some(Value::Bool(b)) => value == if *b { "true" } else { "false" },
                    _ => false,
                }
        }
    };
    hit && req.path.starts_with(r.prefix)
        && method_ok(r.method, req.method)
        && r.when.iter().all(|(k, v)| req.params.get(*k).map(String::as_str) == Some(*v))
        && r.needs.is_none_or(|p| req.response.pointer(p).is_some())
        && r.body_has.is_none_or(|(p, present)| body_value(req.body, p) == present)
        && body_eq_ok(r, req.body)
        && own_ws_ok(r, req.body, req.workspace_id)
        && r.body_lacks.iter().all(|p| !body_value(req.body, p))
}

/// An `own_ws` row: the body names the asking agent's own workspace.
fn own_ws_ok(r: &Row, body: Option<&Value>, workspace_id: &str) -> bool {
    let Some(pointer) = r.own_ws else { return true };
    let Some(token) = body.and_then(|b| b.pointer(pointer)).and_then(Value::as_str) else { return false };
    names_own_workspace(token, workspace_id)
}

/// `token` (a handle, name, path or id) resolves to `workspace_id`.
pub(crate) fn names_own_workspace(token: &str, workspace_id: &str) -> bool {
    let t = token.trim();
    if t.is_empty() || workspace_id.is_empty() {
        return false;
    }
    if t.eq_ignore_ascii_case(workspace_id) {
        return true;
    }
    let mine = {
        let db = k2_core::db::shared();
        let conn = db.lock();
        k2_core::db::schema::Project::get(&conn, workspace_id).ok().map(|p| p.path)
    };
    match (mine, crate::workspace_msg::resolve_workspace(t)) {
        (Some(mine), Some(theirs)) => mine == theirs,
        _ => false,
    }
}

fn body_eq_ok(r: &Row, body: Option<&Value>) -> bool {
    r.body_eq.iter().all(|(p, want)| match body.and_then(|b| b.pointer(p)) {
        Some(Value::String(v)) => v.trim().eq_ignore_ascii_case(want),
        // A JSON boolean matches "true" / "false" (`value` on a switch).
        Some(Value::Bool(v)) => *want == if *v { "true" } else { "false" },
        _ => false,
    })
}

/// The request body has a non-empty value at `pointer`.
fn body_value(body: Option<&Value>, pointer: &str) -> bool {
    match body.and_then(|b| b.pointer(pointer)) {
        None | Some(Value::Null) => false,
        Some(Value::String(s)) => !s.trim().is_empty(),
        Some(_) => true,
    }
}

/// The row that names this refusal: the longest matching prefix.
pub(crate) fn find_row(req: &Refused<'_>) -> Option<&'static Row> {
    let code = refusal_code(req.response);
    ROWS.iter().filter(|r| row_matches(r, req, code)).max_by_key(|r| r.prefix.len())
}

fn value_string(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn param_value(from: &ParamFrom, req: &Refused<'_>) -> Option<String> {
    match from {
        ParamFrom::Query(k) => req.params.get(*k).filter(|s| !s.trim().is_empty()).cloned(),
        ParamFrom::Body(p) => req.body.and_then(|b| b.pointer(p)).and_then(value_string),
        ParamFrom::Response(p) => req.response.pointer(p).and_then(value_string),
        ParamFrom::BodySecs(p) => req.body.and_then(|b| b.pointer(p)).and_then(Value::as_i64).map(k2_core::permission::catalog::secs_word),
    }
}

/// One `key=value` for the command line: shell-safe words as is, anything
/// else single-quoted (the CLI prints this verbatim for the agent to run).
fn shell_word(s: &str) -> String {
    let safe = !s.is_empty()
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b'/' | b':' | b'@' | b',' | b'+'));
    if safe {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}

/// The exact command an agent runs to ask (PRD §3.1), with this build's
/// command name.
pub(crate) fn ask_command(action: &str, params: &Map<String, Value>) -> String {
    ask_command_named(CLI_NAME, action, params)
}

/// [`ask_command`] with the command's name as a value.
fn ask_command_named(cli: &str, action: &str, params: &Map<String, Value>) -> String {
    format!("{cli} {}", ask_tail(action, params))
}

/// The Ask line after the command's name, quoted for a shell: the one
/// place it is built. A refusal's `permission` object sends it as `tail`,
/// so a CLI prints it after its own name.
pub(crate) fn ask_tail(action: &str, params: &Map<String, Value>) -> String {
    let mut tail = format!("permission request {action}");
    for (k, v) in params {
        let v = v.as_str().map(shell_word).unwrap_or_else(|| format!("<{k}>"));
        tail.push_str(&format!(" {k}={v}"));
    }
    tail.push_str(&format!(" --why \"{WHY_PLACEHOLDER}\""));
    tail
}

/// The `permission` object for this refusal, or `None` when it isn't a
/// refusal at all (a plain 2xx, a 400, a 404…). The caller has already
/// checked the gate and that the caller is a passport.
pub(crate) fn permission_for(req: &Refused<'_>) -> Option<Value> {
    if let Some(r) = find_row(req) {
        return Some(match r.target {
            Target::Ask { action, floor } => {
                let mut params = Map::new();
                for (k, v) in r.fixed {
                    params.insert((*k).to_string(), Value::String((*v).to_string()));
                }
                for (k, sources) in r.params {
                    let v = sources.iter().find_map(|f| param_value(f, req));
                    if v.is_none() && r.optional.contains(k) {
                        continue;
                    }
                    // An unknown value stays visible as `key=<key>` in the
                    // command; the params object carries null for it.
                    params.insert((*k).to_string(), v.map(Value::String).unwrap_or(Value::Null));
                }
                let tail = ask_tail(action, &params);
                to_value(PermissionRefusal::requestable(action, params, floor.label(), tail))
            }
            Target::Cannot { r#where } => to_value(PermissionRefusal::not_requestable(r#where)),
        });
    }
    if !matches!(req.status, 401 | 403) {
        return None;
    }
    let code = refusal_code(req.response);
    if let Some((_, hint)) = MACHINE_HINTS.iter().find(|(c, _)| Some(*c) == code) {
        let node = req
            .params
            .get("node")
            .cloned()
            .or_else(|| req.body.and_then(|b| b.pointer("/node")).and_then(value_string))
            .or_else(|| req.response.pointer("/error/node").and_then(value_string))
            .unwrap_or_else(|| "that machine".to_string());
        let mut v = to_value(PermissionRefusal::not_requestable(WHERE_AT_MACHINE));
        if let Value::Object(m) = &mut v {
            m.insert("hint".to_string(), Value::String(hint.replace("{node}", &node)));
        }
        return Some(v);
    }
    Some(to_value(PermissionRefusal::not_requestable(WHERE_DEFAULT)))
}

fn to_value(p: PermissionRefusal) -> Value {
    serde_json::to_value(p).expect("PermissionRefusal serializes")
}

/// Add `"permission": …` as the last top-level key of a JSON object body,
/// leaving every other byte as it was (pretty or compact). `None` when the
/// body isn't a JSON object or already has one.
pub(crate) fn add_permission(body: &str, permission: &Value) -> Option<String> {
    let trimmed = body.trim_end();
    if !trimmed.ends_with('}') {
        return None;
    }
    let close = trimmed.len() - 1;
    let head = &trimmed[..close];
    let inner_empty = head.trim_start().strip_prefix('{').is_some_and(|rest| rest.trim().is_empty());
    let tail = &body[close..];
    let sep = if inner_empty { "" } else { "," };
    Some(format!("{head}{sep}\"permission\":{permission}{tail}"))
}

/// A status some row answers on besides 401/403: a hold's 2xx, or a
/// refusal with its own status (`ci_preview_off` is a 409, 0.46.17). With
/// `path`, only rows whose prefix it starts with count.
fn walled_status(code: u16, path: Option<&str>) -> bool {
    ROWS.iter().any(|r| {
        let s = match r.matches {
            WallMatch::Code { status, .. } | WallMatch::Held { status, .. } => status,
        };
        s == code && path.is_none_or(|p| p.starts_with(r.prefix))
    })
}

/// The whole wrapper for one answer: `None` = send `body` unchanged.
fn teach(
    status: &str,
    method: &str,
    path: &str,
    params: &HashMap<String, String>,
    req_body: Option<&Value>,
    workspace_id: &str,
    body: &str,
) -> Option<String> {
    let code = status_code(status);
    let candidate = matches!(code, 401 | 403) || walled_status(code, Some(path));
    if !candidate || !body.trim_start().starts_with('{') {
        return None;
    }
    if !k2_core::permission::gate::enabled_for(workspace_id) {
        return None;
    }
    let response: Value = serde_json::from_str(body).ok()?;
    if !response.is_object() || response.get("permission").is_some() {
        return None;
    }
    // A held App answer whose card K2 already raised (0.46.4,
    // `apps::approvals::held_note`): nothing to ask; the answer names it.
    if response.pointer("/card/raised").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    // `outside_app_rooms` (0.46.8) already names its next step, the App's
    // guests-rooms grant, which raises its own card: no "It can't be
    // requested." under it.
    if response.pointer("/error/code").and_then(Value::as_str) == Some("outside_app_rooms") {
        return None;
    }
    let req = Refused { status: code, method, path, params, body: req_body, response: &response, workspace_id };
    let p = permission_for(&req)?;
    add_permission(body, &p)
}

/// TCP: the refusal wrapper inside `routes::http::send_response*`. Reads
/// the request the dispatcher noted in `routes::pass_gate`; a no-op unless
/// the gate is on **and** the caller is a valid agent passport.
pub(crate) fn teach_tcp<'a>(status: &str, body: &'a str) -> Cow<'a, str> {
    if !k2_core::permission::gate::server_on() {
        return Cow::Borrowed(body);
    }
    let code = status_code(status);
    if code < 200 || (code >= 300 && !matches!(code, 401 | 403) && !walled_status(code, None)) {
        return Cow::Borrowed(body);
    }
    let taught = crate::routes::pass_gate::with_request(|r| {
        let token = crate::routes::http::extract_token(&r.query).filter(|t| !t.is_empty())?;
        let v = crate::session_token::validate_hook(token)?;
        // Query wins over the form body, as in `read_request_params`.
        let mut params = r.form.clone();
        params.extend(crate::routes::http::parse_params(&r.path, &r.query));
        teach(status, &r.method, &r.path, &params, r.body.as_ref(), &v.principal.workspace_uuid, body)
    })
    .flatten();
    match taught {
        Some(s) => Cow::Owned(s),
        None => Cow::Borrowed(body),
    }
}

/// Cell socket: the refusal wrapper on an agent verb's result (the cell's
/// `:621` exit). `params` are the request's own (query + form), `req_body`
/// its raw bytes.
pub(crate) fn teach_cell(
    status: &str,
    method: &str,
    path: &str,
    params: &HashMap<String, String>,
    req_body: &[u8],
    workspace_id: &str,
    body: &str,
) -> Option<String> {
    if !k2_core::permission::gate::server_on() {
        return None;
    }
    let json = json_body(req_body);
    teach(status, method, path, params, json.as_ref(), workspace_id, body)
}

fn json_body(raw: &[u8]) -> Option<Value> {
    let s = std::str::from_utf8(raw).ok()?;
    if !s.trim_start().starts_with('{') {
        return None;
    }
    serde_json::from_str(s).ok()
}

/// The teaching hint on the cell's gated body.
pub(crate) const CELL_GATED_HINT: &str = concat!("a person does this (the owner, or an admin where Settings allows), \
     not an agent: this route isn't open to agents. Use Settings or a terminal outside ", product_name!(), ".");

/// Cell socket, a route that isn't an agent verb (`:488-495`; vs-live C3):
/// with the gate on for this **valid** passport, the codeless 403 becomes a
/// teaching `owner_only` body plus `permission`. `None` = today's text.
/// There is no refusal to read, so only the generic owner/person walls
/// (`owner_only`, `person_only`: what TCP answers a passport on a route
/// agents can't use) with a route-specific prefix and no refusal-body
/// condition can name an action here.
pub(crate) fn cell_gated_body(
    method: &str,
    path: &str,
    params: &HashMap<String, String>,
    req_body: &[u8],
    workspace_id: &str,
) -> Option<String> {
    if !k2_core::permission::gate::enabled_for(workspace_id) {
        return None;
    }
    let mut body = serde_json::json!({"error": {"code": "owner_only", "hint": CELL_GATED_HINT}});
    let json = json_body(req_body);
    let empty = Value::Object(Map::new());
    let req = Refused { status: 403, method, path, params, body: json.as_ref(), response: &empty, workspace_id };
    let row = ROWS
        .iter()
        .filter(|r| {
            r.prefix != "/cli/"
                && r.needs.is_none()
                && matches!(r.matches, WallMatch::Code { code: "owner_only" | "person_only", .. })
                && path.starts_with(r.prefix)
                && method_ok(r.method, method)
                && r.when.iter().all(|(k, v)| params.get(*k).map(String::as_str) == Some(*v))
                && body_eq_ok(r, json.as_ref())
                && own_ws_ok(r, json.as_ref(), workspace_id)
        })
        .max_by_key(|r| r.prefix.len());
    let permission = match row {
        Some(r) => {
            // Re-run the generic builder with this row's code as the refusal.
            let WallMatch::Code { code, .. } = r.matches else { unreachable!("filtered to Code rows") };
            let synthetic = serde_json::json!({"error": {"code": code}});
            let req = Refused { response: &synthetic, ..req };
            permission_for(&req).unwrap_or_else(|| to_value(PermissionRefusal::not_requestable(WHERE_DEFAULT)))
        }
        None => to_value(PermissionRefusal::not_requestable(WHERE_DEFAULT)),
    };
    body["permission"] = permission;
    Some(body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn req<'a>(
        status: u16,
        method: &'a str,
        path: &'a str,
        params: &'a HashMap<String, String>,
        body: Option<&'a Value>,
        response: &'a Value,
    ) -> Refused<'a> {
        Refused { status, method, path, params, body, response, workspace_id: "" }
    }

    fn teach_of(status: u16, method: &str, path: &str, q: &[(&str, &str)], body: Option<Value>, response: Value) -> Option<Value> {
        let params: HashMap<String, String> = q.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        permission_for(&req(status, method, path, &params, body.as_ref(), &response))
    }

    fn action_of(p: &Value) -> &str {
        p["action"].as_str().unwrap_or_else(|| panic!("not an Ask: {p}"))
    }

    /// Every refusal captured in the off-goldens for a v1 row teaches that
    /// row's action (bodies copied from `tests/golden/permission_off/daemon/walls/`).
    #[test]
    fn the_captured_walls_teach_their_actions() {
        let cases: Vec<(u16, &str, &str, Vec<(&str, &str)>, Option<Value>, Value, &str)> = vec![
            (403, "GET", "/cli/compute/nodes", vec![], None, json!({"error":{"code":"compute_off","hint":"x"}}), "compute.use"),
            (403, "POST", "/cli/compute/admin/propose", vec![], None, json!({"error":{"code":"operate_off"}}), "compute.operate"),
            (403, "POST", "/cli/compute/nodes/retire", vec![], None, json!({"error":{"code":"owner_only"}}), "compute.manage"),
            // 0.46.17: the agent's own grant change (no workspace named) asks
            // for its limits; revoking, or a grant with --ci, is compute.use's.
            (403, "POST", "/cli/compute/grants/set", vec![], None, json!({"error":{"code":"owner_only"}}), "compute.grant"),
            (403, "POST", "/cli/compute/grants/set", vec![], Some(json!({"node":"n1","allowCi":true})), json!({"error":{"code":"owner_only"}}), "compute.use"),
            (403, "POST", "/cli/compute/grants/revoke", vec![], None, json!({"error":{"code":"owner_only"}}), "compute.use"),
            (403, "POST", "/cli/sidecar/new", vec![], None, json!({"error":{"code":"gated"}}), "agents.manage"),
            (403, "POST", "/cli/sessions/open", vec![], None, json!({"error":{"code":"gated"}}), "agents.manage"),
            // 0.46.12: hiring is its own switch.
            (403, "POST", "/cli/agents/hire", vec![], None, json!({"error":{"code":"hire_off","hint":"x"},"ok":false}), "agents.hire"),
            (403, "POST", "/cli/project-group/create", vec![], None, json!({"error":{"code":"agent_permission_off"},"ok":false}), "projects.manage"),
            // 0.46.11: the Org Chart's manage verbs without the switch.
            (403, "POST", "/cli/organization/create", vec![], None, json!({"error":{"code":"org_manage_off","hint":"x"},"ok":false}), "organization.manage"),
            (403, "POST", "/cli/organization/line/add", vec![], None, json!({"error":{"code":"org_manage_off","hint":"x"},"ok":false}), "organization.manage"),
            // 0.46.11: a transcript read without "Read call transcripts".
            (403, "GET", "/cli/voice/transcript", vec![], None, json!({"error":{"code":"transcripts_read_off","hint":"x"},"ok":false}), "voice.transcripts-read"),
            (403, "POST", "/cli/app/exception", vec![], None, json!({"error":{"code":"apps_manage_off"}}), "apps.build"),
            // Rosson 2026-11-01: custom domains (Path B too) fall under the
            // broad App switch; there is no per-domain or web-server card.
            (403, "POST", "/cli/app/domain/add", vec![], None, json!({"error":{"code":"apps_manage_off"}}), "apps.build"),
            (403, "POST", "/cli/app/domain/check", vec![], None, json!({"error":{"code":"apps_manage_off"}}), "apps.build"),
            (403, "POST", "/cli/app/domain/rm", vec![], None, json!({"error":{"code":"apps_manage_off"}}), "apps.build"),
            (403, "POST", "/cli/app/domain/primary", vec![], None, json!({"error":{"code":"apps_manage_off"}}), "apps.build"),
            (403, "POST", "/cli/app/domain/external", vec![], None, json!({"error":{"code":"apps_manage_off"}}), "apps.build"),
            (202, "POST", "/cli/app/build", vec![], None, json!({"status":"held","request":{"id":"w_1"}}), "apps.approve"),
            (202, "POST", "/cli/app/deploy", vec![], None, json!({"status":"held","request":{"id":"w_2"}}), "apps.approve"),
            // 0.46.11: k2 app db connect's one card asks with its request id.
            (202, "POST", "/cli/app/db/connect", vec![], None, json!({"status":"held","request":{"id":"w_3"}}), "apps.approve"),
            (403, "POST", "/cli/sew/server/disable", vec![], None, json!({"error":{"code":"chat_manage_off"}}), "chat.manage"),
            (403, "POST", "/cli/vault/set", vec![], None, json!({"error":{"code":"vault_manage_required"}}), "vault.manage"),
            (403, "POST", "/cli/sources/pin", vec![], None, json!({"error":{"code":"person_only"}}), "gardens.manage"),
            (403, "POST", "/cli/zen/widget/preview", vec![], None, json!({"error":"person_only","message":"m","ok":false}), "gardens.manage"),
            (403, "POST", "/cli/sources/approve", vec![], Some(json!({"name":"fleet","specHash":"a1b2"})), json!({"error":{"code":"person_only"}}), "sources.approve"),
            (403, "POST", "/cli/sources/batch-approve", vec![], Some(json!({"items":[{"name":"fleet","specHash":"a1b2"}]})), json!({"error":{"code":"person_only"}}), "sources.approve"),
            (403, "POST", "/cli/zen/proposals/approve", vec![], Some(json!({"id":"zp_1"})), json!({"error":"person_only"}), "garden.proposal.approve"),
            (403, "GET", "/cli/dns/records", vec![], None, json!({"error":{"code":"dns_manage_disabled"},"ok":false}), "dns.manage"),
            (403, "POST", "/cli/relations/create", vec![], None, json!({"error":{"code":"agents_create_connections_disabled"}}), "connections.create"),
            (403, "POST", "/cli/connections", vec![("action","remove"),("target","sales")], None, json!({"error":{"code":"agents_create_connections_disabled"}}), "connections.create"),
            (403, "POST", "/cli/connections", vec![("action","add"),("target","pog-sales")], None, json!({"error":{"code":"agents_create_connections_disabled"}}), "connections.add"),
            (403, "POST", "/cli/workspace/msg", vec![("workspace","pog-sales")], None, json!({"error":{"code":"not_connected"}}), "connections.add"),
            (403, "GET", "/cli/mail/doctor", vec![], None, json!({"error":{"code":"owner_only","workspace":{"id":"w"}},"ok":false}), "mail.manage"),
            (403, "POST", "/cli/mail/send", vec![], None, json!({"error":{"code":"gated"},"ok":false}), "mail.send-mode"),
            (200, "POST", "/cli/mail/send", vec![], None, json!({"status":"pending_approval","id":"out_9","ok":true}), "mail.approve"),
            (403, "POST", "/cli/mail/approvals/approve", vec![], Some(json!({"id":"out_9"})), json!({"error":{"code":"owner_only","workspace":{"id":"w"}}}), "mail.approve"),
            (403, "POST", "/cli/db/create", vec![], None, json!({"error":{"code":"forbidden"},"ok":false}), "db.agent-access"),
            (403, "POST", "/cli/app/approve", vec![], Some(json!({"request":"ar_22"})), json!({"error":{"code":"owner_only"}}), "apps.approve"),
            (403, "POST", "/cli/compute/admin/approve", vec![], Some(json!({"request":"nar_1"})), json!({"error":{"code":"owner_only"}}), "compute.admin-approve"),
            // A box-move step an agent ran, held for the owner (0.46.6).
            (202, "POST", "/cli/migrate/secrets/export", vec![], None, json!({"ok":false,"status":"held","request":{"id":"mv_0123456789ab","step":"export"}}), "vault.move-approve"),
            (403, "POST", "/cli/sew/owner-handoff", vec![], None, json!({"ok":false,"error":{"code":"owner_only","hint":"agents never get Sew's install owner; ask a person to open Sew as owner"}}), "sew.owner-handoff"),
            (403, "POST", "/cli/sew/server/rollback", vec![], Some(json!({"acceptDataLoss":true})), json!({"ok":false,"error":{"code":"rollback_needs_person","hint":"a person rolls Sew back","to":"0.3.1","since":"2026-10-28 23:08 UTC","bundle":"20261028T230805Z"}}), "sew.rollback-data-loss"),
            // App users (0225): the skin-manage door's refusal on a user route.
            (403, "POST", "/cli/skin/users/password", vec![], None, json!({"ok":false,"error":{"code":"app_users_off"}}), "apps.manage-users"),
            // Garden access (A2's refusals, prd-garden-access-v1 G11–G12).
            (403, "POST", "/cli/zen/garden/put", vec![], Some(json!({"garden":"garden-2"})), json!({"ok":false,"error":"garden_access_needed","garden":"garden-2","card":null}), "garden.access"),
            (403, "POST", "/cli/zen/widget/folder/put", vec![], None, json!({"ok":false,"error":"garden_access_needed","garden":"garden-3","card":null}), "garden.access"),
            (403, "POST", "/cli/zen/garden/delete", vec![], Some(json!({"garden":"garden-2"})), json!({"ok":false,"error":"garden_delete_person","garden":"garden-2","card":null}), "garden.delete"),
            // 0.46.5 S0: a sign that can't cover a placed widget's new version.
            (403, "POST", "/cli/publish/visibility", vec![], Some(json!({"label":"docs","mode":"public","reason":"launch"})), json!({"status":"held","code":"public_needs_person","label":"docs"}), "publish.public"),
            // 0.46.18: `dropped_label_transfer`'s body (an agent's --transfer).
            (403, "POST", "/cli/publish/subdomain/claim", vec![("label","pogshop"),("transfer","1")], None, json!({"code":"transfer_owner_only","error":"transfer_owner_only: only the server owner or an admin can transfer 'pogshop'.","hint":"Pick another name, or ask the server owner to transfer this one.","label":"pogshop"}), "publish.transfer"),
            (403, "POST", "/cli/zen/widget/sign", vec![], Some(json!({"widget":"board"})), json!({"ok":false,"error":"sign_needs_person","widget":"board","version":"ab".repeat(32),"card":null}), "zen.sign"),
            // Job tiers (prd-compute-job-tiers-v1 T13).
            (403, "POST", "/cli/compute/run", vec![], Some(json!({"node":"k2n4","tier":"ci"})), json!({"error":{"code":"ci_not_allowed","hint":"x"}}), "compute.ci"),
            // 0.46.7 (CE-13): --exclusive without the grant's CI right.
            (403, "POST", "/cli/compute/run", vec![], Some(json!({"node":"k2n4","exclusive":true,"why":"bench"})), json!({"error":{"code":"exclusive_needs_grant","hint":"x"}}), "compute.ci"),
            // 0.46.11: --gpu where the grant lacks --gpu (auto: the node is in the refusal).
            (403, "POST", "/cli/compute/run", vec![], Some(json!({"gpu":{"vendor":"any","count":1}})), json!({"error":{"code":"gpu_needs_grant","hint":"x","node":"k2n11"}}), "compute.gpu"),
            // Mobile dev (0.46.6): `k2 sim boot` with "Allow mobile devices" off.
            (403, "POST", "/cli/sim/boot", vec![], Some(json!({})), json!({"error":{"code":"sim_switch_off","hint":"x"},"code":"sim_switch_off"}), "sim.use"),
            // 0.46.12 (L8): `k2 desktop env` with "Allow a virtual desktop" off.
            (403, "POST", "/cli/desktop/env", vec![], Some(json!({})), json!({"ok":false,"error":{"code":"desktop_switch_off","hint":"x"}}), "desktop.use"),
            // 0.46.6: an agent moving its own pinned chat (both doors).
            (403, "POST", "/cli/agent/harness", vec![], Some(json!({"workspace":"sales","harness":"Claude"})), json!({"error":{"code":"harness_needs_person","harness":"claude"},"switched":false}), "agent.harness-switch"),
            (403, "POST", "/cli/workspace/swap-canonical", vec![], Some(json!({"project":"/x","provider":"claude"})), json!({"error":{"code":"harness_needs_person","harness":"claude"},"swapped":false}), "agent.harness-switch"),
            // 0.46.8: an agent refreshing its own pinned chat (both doors).
            (403, "POST", "/cli/agent/refresh", vec![], Some(json!({"notes":"n"})), json!({"error":{"code":"chat_refresh_needs_person","notes":"ab"},"refreshed":false}), "chat.refresh"),
            (403, "POST", "/cli/workspace/swap-canonical", vec![], Some(json!({"project":"/x","provider":"codex"})), json!({"error":{"code":"chat_refresh_needs_person"},"swapped":false}), "chat.refresh"),
            // 0.46.6: `k2 vault delivery NAME env` on a secret the agent can't widen.
            (403, "POST", "/cli/vault/policy", vec![], Some(json!({"name":"K2N12_ENROLL_CODE","delivery":"env"})), json!({"error":{"code":"vault_owner_confirm_required","hint":"x","secret":"K2N12_ENROLL_CODE","delivery":"env"}}), "vault.delivery"),
            // `k2 app grant add … --kind app` by the App's own agent.
            (403, "POST", "/cli/skin/grants", vec![], Some(json!({"subjectKind":"principal","subjectId":"dana","kind":"app","targetId":"jobs","roleId":"viewer"})), json!({"error":{"code":"owner_only","hint":"x"}}), "app.login-grant"),
            // `k2 publish secret add svc NAME` on a secret a person gave.
            (403, "POST", "/cli/vault/grant", vec![], Some(json!({"name":"PIN","to":"service:svc","as":"file"})), json!({"error":{"code":"vault_owner_confirm_required","hint":"x","secret":"PIN","service":"svc","as":"file"}}), "vault.share-service"),
            // 0.46.11: a secret on the server or in another workspace it can't see.
            (403, "POST", "/cli/vault/fetch", vec![], None, json!({"error":{"code":"secret_out_of_reach","hint":"x","secret":"STRIPE","from":"server","as":"fetch"}}), "vault.use"),
            // 0.46.11: `k2 vault send NAME --to agent::host` of a secret a person gave.
            (403, "POST", "/cli/vault/send", vec![], None, json!({"error":{"code":"held","hint":"x","secret":"FLEET_DB","to":"argus::other.k2.dev"},"status":"held"}), "vault.send"),
            (403, "POST", "/cli/vault/grant", vec![], None, json!({"error":{"code":"grant_out_of_reach","hint":"x","secret":"SHARED_DB","as":"env"}}), "vault.grant-server"),
            // 0.46.7: an agent turning its own App's chat key on (cards on), and switching a Space's governance.
            (403, "POST", "/cli/app/chat-key", vec![], Some(json!({"app":"jobs"})), json!({"error":{"code":"owner_only","hint":"x","app":"jobs"}}), "apps.chat-key"),
            (403, "POST", "/cli/app/recording/set", vec![], Some(json!({"app":"jobs","serverRead":true})), json!({"error":{"code":"owner_only","hint":"x","app":"jobs","serverRead":"on"}}), "apps.recording"),
            (403, "POST", "/cli/sew/app-spaces/governance", vec![], Some(json!({"app":"jobs","spaceId":"5101","governance":"app"})), json!({"ok":false,"error":{"code":"person_only","hint":"x"}}), "sew.space-governance"),
            // 0.46.11 K-a: an agent's recorder install.
            (403, "POST", "/cli/sew/recording/enable", vec![], None, json!({"ok":false,"error":{"code":"person_only","hint":"x"}}), "sew.recording-enable"),
            // 0.46.18: an agent's `policy set enabled on` asks on the same card.
            (403, "POST", "/cli/sew/recording/policy", vec![], Some(json!({"changes":{"recording.enabled":true}})), json!({"ok":false,"error":{"code":"person_only","hint":"x"}}), "sew.recording-enable"),
            // 0.46.17: an agent's start level, and recording in direct messages on and off.
            (403, "POST", "/cli/voice/recording/settings", vec![], Some(json!({"startLevel":"member"})), json!({"ok":false,"code":"recording_level_needs_owner","hint":"x","error":{"code":"recording_level_needs_owner","hint":"x"}}), "voice.recording-start-level"),
            (403, "POST", "/cli/sew/recording/policy", vec![], Some(json!({"changes":{"recording.dm_enabled":true}})), json!({"ok":false,"error":{"code":"person_only","hint":"x"}}), "voice.recording-dm"),
            (403, "POST", "/cli/sew/recording/policy", vec![], Some(json!({"changes":{"recording.dm_enabled":false}})), json!({"ok":false,"error":{"code":"person_only","hint":"x"}}), "voice.recording-dm"),
            // 0.46.6: an agent pausing / resuming another workspace's heartbeat.
            (403, "POST", "/cli/heartbeat/enable", vec![("name", "triage"), ("enabled", "false"), ("workspace", "companion")], None, json!({"error":{"code":"other_workspace","workspace":"companion","heartbeat":"triage","enabled":false}}), "heartbeat.pause-other"),
            (403, "POST", "/cli/heartbeat/enable", vec![("name", "triage"), ("enabled", "true"), ("workspace", "companion")], None, json!({"error":{"code":"other_workspace","workspace":"companion","heartbeat":"triage","enabled":true}}), "heartbeat.resume-other"),
            // 0.46.7 Workbench W1: an agent's `k2 sidecar land` / `retire`.
            (403, "POST", "/cli/sidecar/land", vec![], Some(json!({"name":"fix","expectTip":"abc1234"})), json!({"error":{"code":"land_needs_person","hint":"x","sidecar":"fix","tip":"abc1234def"}}), "git.land"),
            (403, "POST", "/cli/sidecar/retire", vec![], Some(json!({"name":"fix","discard":false})), json!({"error":{"code":"retire_needs_person","hint":"x","sidecar":"fix","discard":"no"}}), "sidecar.retire"),
            // D1 (0.46.17): an agent approving its own review branch, and its commit --merge.
            (403, "POST", "/cli/review/approve", vec![("agent", "scout"), ("branch", "agent/scout/fix")], None, json!({"error":{"code":"merge_needs_person","hint":"x","agent":"scout","branch":"agent/scout/fix"}}), "git.review-approve"),
            (403, "POST", "/cli/commit-merge", vec![("message", "wip")], None, json!({"error":{"code":"merge_needs_person","hint":"x"}}), "git.commit-merge"),
            // 0.46.17: an agent's focus-group changes.
            (403, "POST", "/cli/focus-groups/create", vec![], Some(json!({"name":"Client work"})), json!({"error":{"code":"focus_group_needs_person","hint":"x","name":"Client work"}}), "focus-groups.create"),
            (403, "POST", "/cli/focus-groups/update", vec![], Some(json!({"id":"g1","name":"Ops"})), json!({"error":{"code":"focus_group_needs_person","hint":"x","group":"Infra","to":"Ops"}}), "focus-groups.rename"),
            (403, "POST", "/cli/focus-groups/assign", vec![], Some(json!({"projectId":"w","focusGroupId":null})), json!({"error":{"code":"focus_group_needs_person","hint":"x","target":"sales","group":"none"}}), "focus-groups.assign"),
            (403, "POST", "/cli/commit", vec![("message", "wip")], None, json!({"error":{"code":"commit_needs_person","hint":"x"}}), "git.commit"),
            // 0.46.6: an agent's own reset (the owner's route) and a password
            // it may not set (a box login).
            (403, "POST", "/cli/skin/users/reset-link", vec![], Some(json!({"username":"adam","app":"agents.acme","code":true})), json!({"ok":false,"error":{"code":"owner_only","hint":"x"}}), "apps.user-password-reset"),
            (403, "POST", "/cli/skin/users/password", vec![], Some(json!({"username":"adam","password":"hunter2-not-real"})), json!({"ok":false,"error":{"code":"owner_only","hint":"'adam' has a box login"}}), "apps.user-password-reset"),
            (403, "GET", "/cli/docs/read", vec![("workspace", "alpha"), ("path", "/abs/alpha/notes/plan.md")], None, json!({"ok":false,"error":{"code":"doc_not_shared","hint":"x","workspace":"alpha","path":"notes/plan.md"}}), "docs.share"),
            // 0.46.8: a job aimed at a node off the workspace's node list, and an agent clearing its list.
            (403, "POST", "/cli/compute/run", vec![], Some(json!({"node":"k2n2"})), json!({"error":{"code":"node_not_allowed","hint":"x","node":"k2n2-m1","nodes":"k2n2-m1"}}), "compute.allow-nodes"),
            (403, "POST", "/cli/compute/allow-nodes/set", vec![], Some(json!({"clear":true})), json!({"error":{"code":"allow_nodes_needs_person","hint":"x","clear":true}}), "compute.allow-nodes-clear"),
            (403, "POST", "/cli/compute/allow-nodes/set", vec![], Some(json!({"workspace":"k2-engine","add":"k2n13"})), json!({"error":{"code":"allow_nodes_for_needs_person","hint":"x","workspace":"k2-engine","add":"k2n13"}}), "compute.allow-nodes-for"),
            (403, "POST", "/cli/ci/cache/trust/set", vec![], Some(json!({"add":["rel"]})), json!({"error":{"code":"cache_trust_needs_person","hint":"x","workspace":"k2","branches":"rel"}}), "ci.cache-trust"),
            // 0.46.9 (GH#78): an agent's git-access grant.
            (403, "POST", "/cli/compute/git-access/grant", vec![], Some(json!({"github":"acme/app"})), json!({"error":{"code":"git_access_needs_person","hint":"x","workspace":"k2","repos":"acme/app"}}), "compute.git-access"),
            // 0.46.10: the grant with no credential yet, and the agent's own setup.
            (403, "POST", "/cli/compute/git-access/grant", vec![], Some(json!({"github":"acme/app"})), json!({"error":{"code":"git_access_needs_setup","hint":"x","workspace":"k2","repos":"acme/app"}}), "compute.git-access-setup"),
            (403, "POST", "/cli/compute/git-access/setup", vec![], Some(json!({"token":"x"})), json!({"error":{"code":"owner_only","hint":"x"}}), "compute.git-access-setup"),
            // 0.46.9: a filed package request, and the package policy to an agent that can't operate nodes.
            (200, "POST", "/cli/compute/nodes/packages/request", vec![], Some(json!({"nodes":["k2n7"],"names":["libgtk-3-dev"]})), json!({"pkgHeld":true,"request":{"id":"cpr_0123456789abcdef"}}), "compute.packages"),
            (403, "POST", "/cli/compute/run", vec![], Some(json!({"node":"k2n7"})), json!({"error":{"code":"package_not_approved","hint":"x","request":"cpr_0123456789abcdef","node":"k2n7"}}), "compute.packages"),
            (403, "POST", "/cli/compute/nodes/packages/policy", vec![], Some(json!({"node":"k2n7","server":"own","level":"official"})), json!({"error":{"code":"operate_off","hint":"x"}}), "compute.packages-policy"),
            // 0.46.8: `k2 chat space create --app` with no chat account (fix A),
            // and `k2 mail sending <ws> off` by an agent without mail-manage.
            (403, "POST", "/cli/sew/chat/manage/space/create", vec![], Some(json!({"slug":"jobs","app":"jobs"})), json!({"ok":false,"error":{"code":"chat_account_needed","hint":"x","details":{"workspace":"alpha"}}}), "chat.account"),
            (403, "POST", "/cli/mail/config/set", vec![], Some(json!({"workspace":"sales","agentSend":"off"})), json!({"ok":false,"error":{"code":"owner_only","hint":"x","workspace":{"name":"alpha","path":"/a","id":"w"}}}), "mail.stop-sending"),
            // 0.46.17: an agent's `k2 llm tokens refresh`.
            (403, "POST", "/cli/llm/accounts/refresh", vec![], Some(json!({"id":"acc_0123"})), json!({"ok":false,"error":{"code":"owner_only","hint":"x"}}), "llm.tokens-refresh"),
            // 0.46.13: `k2 approve request` without the permission.
            (403, "POST", "/cli/approve/request", vec![], Some(json!({"payload_b64":"AA=="})), json!({"ok":false,"error":{"code":"approve_request_off","hint":"x"}}), "approve.request"),
            // 0.46.17: CI on K2 Compute off (any CI verb), and the agent's
            // own `k2 settings set ciPreview true` (cell socket, then TCP).
            (409, "POST", "/cli/ci/run", vec![], Some(json!({"rev":"HEAD"})), json!({"error":{"code":"ci_preview_off","hint":"x"}}), "ci.preview"),
            (409, "GET", "/cli/compute/roster", vec![], None, json!({"error":{"code":"ci_preview_off","hint":"x"}}), "ci.preview"),
            (403, "POST", "/cli/settings/update", vec![], Some(json!({"ciPreview":true})), json!({"error":{"code":"owner_only","hint":"x"}}), "ci.preview"),
            (403, "POST", "/cli/settings/update", vec![], Some(json!({"ciPreview":true})), json!({"error":"invalid or missing token"}), "ci.preview"),
            // 0.46.16 Postal: off refuses an agent on every route; each
            // agent step answers with the card K2 put up (`asked: true`).
            (403, "GET", "/cli/postal/status", vec![], None, json!({"ok":false,"state":"off","error":{"code":"postal_off","hint":"x"}}), "postal.enable"),
            (403, "POST", "/cli/postal/invite", vec![], Some(json!({"to":"ops::beta.postal.bot","why":"x"})), json!({"ok":false,"state":"off","error":{"code":"postal_off","hint":"x"}}), "postal.enable"),
            (200, "POST", "/cli/postal/enable", vec![], Some(json!({})), json!({"ok":true,"asked":true,"action":"postal.enable","card":"c1"}), "postal.enable"),
            (200, "POST", "/cli/postal/claim", vec![], Some(json!({"address":"scout::initech.postal.bot"})), json!({"ok":true,"asked":true,"card":"c1"}), "postal.claim"),
            (200, "POST", "/cli/postal/invite", vec![], Some(json!({"to":"ops::beta.postal.bot","why":"to review PRs"})), json!({"ok":true,"asked":true,"card":"c1"}), "postal.invite"),
            (200, "POST", "/cli/postal/decide", vec![], Some(json!({"id":"f-1","decision":"accept"})), json!({"ok":true,"asked":true,"card":"c1"}), "postal.pair.accept"),
            (200, "POST", "/cli/postal/link-p5", vec![], Some(json!({})), json!({"ok":true,"asked":true,"card":"c1"}), "postal.link-p5"),
            // 0.46.18: a mint past the address cap, and an agent's
            // `k2 hostmail config --address-cap` without "manage hosted mail".
            (409, "POST", "/cli/mail/address/create", vec![], Some(json!({"localPart":"bot6"})), json!({"ok":false,"error":{"code":"cap_reached","hint":"x","workspace":"alpha","cap":5}}), "mail.address-cap"),
            (403, "POST", "/cli/mail/config/set", vec![], Some(json!({"workspace":"sales","addressCap":9})), json!({"ok":false,"error":{"code":"agent_send_path_only","hint":"x"}}), "mail.address-cap"),
        ];
        let mut asked: Vec<&str> = Vec::new();
        for (status, m, path, q, body, resp, want) in cases {
            let p = teach_of(status, m, path, &q, body, resp).unwrap_or_else(|| panic!("{path}: no permission"));
            assert_eq!(action_of(&p), want, "{m} {path}: {p}");
            assert!(p["command"].as_str().unwrap().starts_with(&format!("k2 permission request {want}")), "{p}");
            asked.push(want);
        }
        // Every walled v1 action is reachable from some captured refusal,
        // except the two whose holds attach to apps.approve, keep-mine
        // (K2-presented) — PRD §5.2 walls column — and apps.user-grant-box
        // (asked; `app.login-grant` and the box-login refusal name it).
        for &a in V1_ACTIONS {
            if matches!(a, "apps.manage-full" | "apps.core" | "garden.keep-mine" | "garden.access-off" | "zen.sign-off" | "publish.pin" | "apps.user-grant-box") {
                continue;
            }
            assert!(asked.contains(&a), "{a}: no captured wall teaches it");
        }
    }

    /// 0.46.17: an agent's start level and DM recording end with the exact
    /// Ask-with line for their card, with the value the agent sent.
    #[test]
    fn recording_lines_name_their_cards_with_the_value() {
        let cmd = |path: &str, body: Value, resp: Value| {
            teach_of(403, "POST", path, &[], Some(body), resp).map(|p| (p["command"].as_str().unwrap_or_default().to_string(), p["approvers"].clone()))
        };
        let level = json!({"ok":false,"code":"recording_level_needs_owner","hint":"x","error":{"code":"recording_level_needs_owner","hint":"x"}});
        for l in ["none", "moderator", "member"] {
            let (c, who) = cmd("/cli/voice/recording/settings", json!({ "startLevel": l }), level.clone()).expect("a card");
            assert_eq!(c, format!("k2 permission request voice.recording-start-level level={l} --why \"<what you need it for>\""));
            assert_eq!(who, json!("owner or admin"));
        }
        let person = json!({"ok":false,"error":{"code":"person_only","hint":"x"}});
        for (b, v) in [(true, "on"), (false, "off")] {
            let (c, _) = cmd("/cli/sew/recording/policy", json!({"changes": {"recording.dm_enabled": b}}), person.clone()).expect("a card");
            assert_eq!(c, format!("k2 permission request voice.recording-dm dm={v} --why \"<what you need it for>\""));
        }
        // The shared refusal of a K5 room change stays no card of these.
        let shared = json!({"ok":false,"code":"owner_or_admin_only","hint":"x","error":{"code":"owner_or_admin_only","hint":"x"}});
        let p = teach_of(403, "POST", "/cli/voice/recording/settings", &[], Some(json!({"required": true})), shared);
        assert!(p.as_ref().and_then(|p| p["action"].as_str()).is_none_or(|a| a != "voice.recording-start-level"), "{p:?}");
    }

    /// 0.46.16 Postal: each agent step's Ask-with line names its card with
    /// the values the agent sent; a plain answer teaches nothing; another
    /// workspace's Postal can't be requested.
    #[test]
    fn postal_lines_name_each_card_with_its_values() {
        let cmd = |status, path: &str, body: Value, resp: Value| {
            teach_of(status, "POST", path, &[], Some(body), resp).map(|p| p["command"].as_str().unwrap_or_default().to_string())
        };
        let asked = json!({"ok":true,"asked":true,"card":"c1"});
        assert_eq!(
            cmd(200, "/cli/postal/invite", json!({"to":"ops::beta.postal.bot","why":"to review PRs"}), asked.clone()).as_deref(),
            Some("k2 permission request postal.invite note='to review PRs' to=ops::beta.postal.bot --why \"<what you need it for>\"")
        );
        assert_eq!(
            cmd(200, "/cli/postal/claim", json!({"address":"scout::initech.postal.bot"}), asked.clone()).as_deref(),
            Some("k2 permission request postal.claim address=scout::initech.postal.bot --why \"<what you need it for>\"")
        );
        assert_eq!(
            cmd(200, "/cli/postal/decide", json!({"id":"f-1","decision":"accept"}), asked.clone()).as_deref(),
            Some("k2 permission request postal.pair.accept id=f-1 --why \"<what you need it for>\"")
        );
        assert_eq!(
            cmd(200, "/cli/postal/enable", json!({}), asked.clone()).as_deref(),
            Some("k2 permission request postal.enable --why \"<what you need it for>\"")
        );
        let off = json!({"ok":false,"state":"off","error":{"code":"postal_off","hint":"x"}});
        let p = teach_of(403, "POST", "/cli/postal/claim", &[], Some(json!({"address":"scout::initech.postal.bot"})), off).unwrap();
        assert_eq!((action_of(&p), p["approvers"].as_str()), ("postal.enable", Some("owner")), "{p}");
        // Done without a card (a person's own claim, an agent's block): nothing to ask.
        assert!(teach_of(200, "POST", "/cli/postal/claim", &[], None, json!({"ok":true,"handle":{}})).is_none());
        assert!(teach_of(200, "POST", "/cli/postal/block", &[], None, json!({"ok":true,"asked":true})).is_none(), "block puts no card up");
        let p = teach_of(403, "GET", "/cli/postal/status", &[], None, json!({"ok":false,"error":{"code":"postal_own_workspace_only"}})).unwrap();
        assert_eq!((p["requestable"].as_bool(), p["where"].as_str()), (Some(false), Some(POSTAL_OTHER_WHERE)), "{p}");
    }

    #[test]
    fn doc_not_shared_asks_for_the_resolved_workspace_and_path() {
        let p = teach_of(
            403,
            "GET",
            "/cli/docs/read",
            &[("workspace", "/abs/alpha"), ("path", "/abs/alpha/notes/plan.md")],
            None,
            json!({"ok":false,"error":{"code":"doc_not_shared","hint":"x","workspace":"alpha","path":"notes/plan.md"}}),
        )
        .expect("a permission object");
        assert_eq!(action_of(&p), "docs.share");
        let cmd = p["command"].as_str().expect("command");
        assert!(cmd.contains("from=alpha") && cmd.contains("path=notes/plan.md"), "{cmd}");
    }

    #[test]
    fn values_come_from_the_request_or_the_answer() {
        let p = teach_of(
            202,
            "POST",
            "/cli/app/build",
            &[],
            None,
            json!({"status":"held","request":{"id":"w_7f"}}),
        )
        .unwrap();
        assert_eq!(p["params"], json!({"request":"w_7f"}));
        assert_eq!(p["approvers"], "owner or admin", "an Owner or Admin approves (0.46.13)");
        assert_eq!(p["command"], "k2 permission request apps.approve request=w_7f --why \"<what you need it for>\"");

        let p = teach_of(403, "POST", "/cli/connections", &[("action", "add"), ("target", "pog-sales")], None, json!({"error":{"code":"agents_create_connections_disabled"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request connections.add to=pog-sales --why \"<what you need it for>\"");
        assert_eq!(p["approvers"], "owner or admin");

        let p = teach_of(403, "POST", "/cli/db/create", &[], None, json!({"error":{"code":"forbidden"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request db.agent-access level=write --why \"<what you need it for>\"");

        // A value K2 can't find stays a placeholder the agent fills in.
        let p = teach_of(403, "POST", "/cli/sources/approve", &[], None, json!({"error":{"code":"person_only"}})).unwrap();
        assert_eq!(p["params"], json!({"source": null, "spec": null}));
        assert_eq!(p["command"], "k2 permission request sources.approve source=<source> spec=<spec> --why \"<what you need it for>\"");

        // Odd characters are single-quoted.
        let p = teach_of(403, "POST", "/cli/workspace/msg", &[("workspace", "a b'c")], None, json!({"error":{"code":"not_connected"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request connections.add to='a b'\\''c' --why \"<what you need it for>\"");

        // 0.46.6: the harness comes from the refusal (normalized), not the
        // agent's spelling in the request.
        let p = teach_of(
            403,
            "POST",
            "/cli/agent/harness",
            &[],
            Some(json!({"workspace":"sales","harness":"Claude"})),
            json!({"error":{"code":"harness_needs_person","harness":"claude"}}),
        )
        .unwrap();
        assert_eq!(p["command"], "k2 permission request agent.harness-switch harness=claude --why \"<what you need it for>\"");
        assert_eq!(p["approvers"], "owner or admin");
    }

    /// 0.46.6: another workspace's heartbeat. The workspace (its handle) and
    /// the heartbeat come from the refusal, not the agent's spelling (a path,
    /// `ws/name`); `enabled=` picks pause or resume; the same code on
    /// another heartbeat route (show) names no card.
    #[test]
    fn heartbeat_other_teaches_the_workspace_and_heartbeat_it_was_refused_on() {
        let refusal = |enabled: bool| {
            json!({"error":{"code":"other_workspace","hint":"x","workspace":"companion","heartbeat":"triage","enabled":enabled}})
        };
        let p = teach_of(
            403,
            "POST",
            "/cli/heartbeat/enable",
            &[("name", "companion/triage"), ("enabled", "false"), ("workspace", "/Users/x/companion")],
            None,
            refusal(false),
        )
        .unwrap();
        assert_eq!(action_of(&p), "heartbeat.pause-other", "{p}");
        assert_eq!(p["params"], json!({"target": "companion", "heartbeat": "triage"}), "{p}");
        assert_eq!(p["approvers"], "owner or admin");
        let cmd = p["command"].as_str().unwrap();
        assert!(
            cmd.starts_with("k2 permission request heartbeat.pause-other ")
                && cmd.contains(" target=companion")
                && cmd.contains(" heartbeat=triage")
                && cmd.ends_with(" --why \"<what you need it for>\""),
            "{cmd}"
        );
        let p = teach_of(403, "POST", "/cli/heartbeat/enable", &[("name", "triage"), ("enabled", "true")], None, refusal(true)).unwrap();
        assert_eq!(action_of(&p), "heartbeat.resume-other", "{p}");
        // `other_workspace` from heartbeat show is no card's wall.
        let p = teach_of(403, "GET", "/cli/heartbeat/show", &[("name", "triage")], None, json!({"error":{"code":"other_workspace","hint":"x"}})).unwrap();
        assert_eq!(p["requestable"], false, "{p}");
    }

    /// 0.46.6: the password-reset card's walls. A reset link teaches
    /// `mode=link` (or `email` when the agent gave `to`) with the person and
    /// App; a refused password set teaches `mode=password` with the person
    /// only, and never reads the body's password; the off switch's code
    /// still teaches `apps.manage-users`; the approvers are the card's floor.
    #[test]
    fn the_password_reset_walls_name_the_person_and_the_mode_never_the_password() {
        let owner_only = || json!({"ok":false,"error":{"code":"owner_only","hint":"x"}});
        let p = teach_of(403, "POST", "/cli/skin/users/reset-link", &[], Some(json!({"username":"adam","app":"agents.acme","code":true})), owner_only()).unwrap();
        assert_eq!(action_of(&p), "apps.user-password-reset", "{p}");
        assert_eq!(p["params"], json!({"user":"adam","app":"agents.acme","mode":"link"}), "{p}");
        assert_eq!(p["approvers"], super::super::actions::password_reset::FLOOR.label());
        let cmd = p["command"].as_str().unwrap();
        assert!(cmd.starts_with("k2 permission request apps.user-password-reset ") && cmd.contains(" user=adam") && cmd.contains(" app=agents.acme") && cmd.contains(" mode=link"), "{cmd}");
        let p = teach_of(403, "POST", "/cli/skin/users/reset-link", &[], Some(json!({"username":"adam","to":"adam@example.test"})), owner_only()).unwrap();
        assert_eq!(p["params"]["mode"], "email", "{p}");
        assert!(p["params"].get("app").is_none(), "no App named: K2 picks it, so no placeholder: {p}");
        assert!(!p["command"].as_str().unwrap().contains("app="), "{p}");
        assert!(!p.to_string().contains("adam@example.test"), "the address never goes in the ask: {p}");
        // An empty `to` is no address.
        let p = teach_of(403, "POST", "/cli/skin/users/reset-link", &[], Some(json!({"username":"adam","to":""})), owner_only()).unwrap();
        assert_eq!(p["params"]["mode"], "link", "{p}");

        let secret = "correct-horse-not-real-9";
        let p = teach_of(403, "POST", "/cli/skin/users/password", &[], Some(json!({"username":"adam","password":secret})), owner_only()).unwrap();
        assert_eq!(action_of(&p), "apps.user-password-reset", "{p}");
        assert_eq!(p["params"], json!({"user":"adam","mode":"password"}), "{p}");
        assert!(!p.to_string().contains(secret), "the wall never reads the password: {p}");
        // The switch-off refusal on the same route still teaches the switch.
        let p = teach_of(403, "POST", "/cli/skin/users/password", &[], Some(json!({"username":"adam","password":secret})), json!({"ok":false,"error":{"code":"app_users_off"}})).unwrap();
        assert_eq!(action_of(&p), "apps.manage-users", "{p}");
        // No wall of this card reads a password anywhere.
        for r in ROWS.iter().filter(|r| matches!(r.target, Target::Ask { action: "apps.user-password-reset", .. })) {
            for (_, sources) in r.params {
                for f in *sources {
                    let key = match f {
                        ParamFrom::Body(k) | ParamFrom::Query(k) | ParamFrom::Response(k) | ParamFrom::BodySecs(k) => *k,
                    };
                    assert!(!key.to_ascii_lowercase().contains("password"), "{key}");
                }
            }
            assert_eq!(r.target, ask("apps.user-password-reset", super::super::actions::password_reset::FLOOR));
        }
    }

    /// Workbench W1 (0.46.7): an agent's Land / Retire. The sidecar and the
    /// tip come from the refusal (not the agent's `ws/name` spelling); the
    /// retire choice too. `gated` on the same routes is still agents.manage.
    #[test]
    fn workbench_land_and_retire_teach_the_sidecar_and_tip_they_were_refused_on() {
        let tip = "0123456789abcdef0123456789abcdef01234567";
        let p = teach_of(
            403,
            "POST",
            "/cli/sidecar/land",
            &[],
            Some(json!({"workspace": "/Users/x/k2", "name": "k2/fix", "expectTip": tip})),
            json!({"error":{"code":"land_needs_person","hint":"x","sidecar":"fix","address":"k2/fix","tip":tip}}),
        )
        .unwrap();
        assert_eq!(action_of(&p), "git.land", "{p}");
        assert_eq!(p["params"], json!({"sidecar": "fix", "tip": tip}), "{p}");
        assert_eq!(p["approvers"], "owner or admin");
        let cmd = p["command"].as_str().unwrap();
        assert!(cmd.starts_with("k2 permission request git.land ") && cmd.contains(" sidecar=fix"), "{cmd}");
        let p = teach_of(
            403,
            "POST",
            "/cli/sidecar/retire",
            &[],
            Some(json!({"name": "fix", "discard": true})),
            json!({"error":{"code":"retire_needs_person","hint":"x","sidecar":"fix","discard":"yes"}}),
        )
        .unwrap();
        assert_eq!(action_of(&p), "sidecar.retire", "{p}");
        assert_eq!(p["params"], json!({"sidecar": "fix", "discard": "yes"}), "{p}");
        let p = teach_of(403, "POST", "/cli/sidecar/land", &[], None, json!({"error":{"code":"gated","hint":"x"}})).unwrap();
        assert_eq!(action_of(&p), "agents.manage", "{p}");
    }

    /// `vault.share-service`: the secret, service and delivery come from the
    /// refusal, names only when the agent gave them. The same code without a
    /// `service` (a share with another workspace, a secret a card can't
    /// share) stays "It can't be requested"; on the policy route it stays
    /// `apps.chat-key`: the route names the App and what was asked only with
    /// cards on for the agent's own App; without `app` the same code is "It
    /// can't be requested" (revoke, another workspace's App). Optional values
    /// the agent didn't send stay out of the line.
    #[test]
    fn chat_key_teaches_only_the_refusal_that_names_the_app() {
        let body = json!({"app":"jobs","caps":["chat:read","voice:join"],"maxLevel":"admin"});
        let p = teach_of(403, "POST", "/cli/app/chat-key", &[], Some(body.clone()), json!({"error":{"code":"owner_only","hint":"x","app":"jobs","level":"admin","caps":"chat:read,voice:join"}})).unwrap();
        assert_eq!(action_of(&p), "apps.chat-key", "{p}");
        assert_eq!(p["params"], json!({"app":"jobs","level":"admin","caps":"chat:read,voice:join"}), "{p}");
        assert_eq!(p["approvers"], "owner or admin");
        assert_eq!(p["command"], "k2 permission request apps.chat-key app=jobs caps=chat:read,voice:join level=admin --why \"<what you need it for>\"");
        let p = teach_of(403, "POST", "/cli/app/chat-key", &[], Some(json!({"app":"jobs"})), json!({"error":{"code":"owner_only","app":"jobs"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request apps.chat-key app=jobs --why \"<what you need it for>\"");
        for (path, resp) in [
            ("/cli/app/chat-key", json!({"error":{"code":"owner_only","hint":"x"}})),
            ("/cli/app/chat-key/revoke", json!({"error":{"code":"owner_only","hint":"x"}})),
        ] {
            let p = teach_of(403, "POST", path, &[], Some(json!({"app":"jobs"})), resp).unwrap();
            assert_eq!(p["requestable"], false, "{path}: {p}");
        }
    }

    /// 0.46.18: a managing agent's change to another workspace's list
    /// teaches `compute.allow-nodes-for` with that workspace and exactly its
    /// one change; a whole list at once (no change named) can't be asked.
    #[test]
    fn another_workspaces_list_teaches_the_for_card_with_its_one_change() {
        let refusal = |extra: Value| {
            let mut e = json!({"code":"allow_nodes_for_needs_person","hint":"x","workspace":"k2-engine"});
            for (k, v) in extra.as_object().unwrap() {
                e[k] = v.clone();
            }
            json!({ "error": e })
        };
        for (extra, want) in [
            (json!({"add":"k2n13,k2n14"}), "k2 permission request compute.allow-nodes-for add=k2n13,k2n14 target=k2-engine --why \"<what you need it for>\""),
            (json!({"remove":"k2n2"}), "k2 permission request compute.allow-nodes-for remove=k2n2 target=k2-engine --why \"<what you need it for>\""),
            (json!({"clear":"true"}), "k2 permission request compute.allow-nodes-for clear=true target=k2-engine --why \"<what you need it for>\""),
        ] {
            let p = teach_of(403, "POST", "/cli/compute/allow-nodes/set", &[], Some(json!({"workspace":"k2-engine"})), refusal(extra.clone())).unwrap();
            assert_eq!(action_of(&p), "compute.allow-nodes-for", "{extra}: {p}");
            assert_eq!(p["command"], want, "{extra}: {p}");
            assert_eq!(p["approvers"], "owner or admin", "{p}");
        }
        let p = teach_of(403, "POST", "/cli/compute/allow-nodes/set", &[], Some(json!({"workspace":"k2-engine","nodes":"a"})), refusal(json!({}))).unwrap();
        assert_eq!(p["requestable"], false, "a whole list names no change: {p}");
    }

    /// The compute node list (0.46.8): a job off the list and an agent's
    /// widening change teach `compute.allow-nodes nodes=<the node>`; an
    /// agent's clear teaches `compute.allow-nodes-clear`.
    #[test]
    fn the_node_list_walls_name_the_node_or_the_clear() {
        let off = json!({"error":{"code":"node_not_allowed","hint":"x","node":"k2n2-m1","nodes":"k2n2-m1","allowed":["k2n9"]}});
        for (path, body) in [
            ("/cli/compute/run", json!({"node":"k2n2"})),
            ("/cli/compute/jobs/retry", json!({"job":"abcd1234"})),
            ("/cli/compute/prewarm", json!({"nodes":["k2n2"]})),
        ] {
            let p = teach_of(403, "POST", path, &[], Some(body), off.clone()).unwrap();
            assert_eq!(action_of(&p), "compute.allow-nodes", "{path}: {p}");
            assert_eq!(p["approvers"], "owner or admin");
            assert_eq!(p["command"], "k2 permission request compute.allow-nodes nodes=k2n2-m1 --why \"<what you need it for>\"", "{path}");
        }
        let add = json!({"error":{"code":"allow_nodes_needs_person","hint":"x","workspace":"k2","nodes":"k2n2-m1,k2n3-m1"}});
        let p = teach_of(403, "POST", "/cli/compute/allow-nodes/set", &[], Some(json!({"add":["k2n2-m1","k2n3-m1"]})), add).unwrap();
        assert_eq!(action_of(&p), "compute.allow-nodes", "{p}");
        assert_eq!(p["command"], "k2 permission request compute.allow-nodes nodes=k2n2-m1,k2n3-m1 --why \"<what you need it for>\"");
        let clear = json!({"error":{"code":"allow_nodes_needs_person","hint":"x","workspace":"k2","clear":true}});
        let p = teach_of(403, "POST", "/cli/compute/allow-nodes/set", &[], Some(json!({"clear":true})), clear).unwrap();
        assert_eq!(action_of(&p), "compute.allow-nodes-clear", "{p}");
        assert_eq!(p["command"], "k2 permission request compute.allow-nodes-clear --why \"<what you need it for>\"");
    }

    /// Git access (0.46.9, GH#78): an agent's grant teaches
    /// `compute.git-access repos=<the repos>`, for the owner or an Admin.
    #[test]
    fn the_git_access_wall_names_the_repos() {
        let body = json!({"error":{"code":"git_access_needs_person","hint":"x","workspace":"k2","repos":"acme/app,acme/lib"}});
        let p = teach_of(403, "POST", "/cli/compute/git-access/grant", &[], Some(json!({"github":"acme/app,acme/lib"})), body).unwrap();
        assert_eq!(action_of(&p), "compute.git-access", "{p}");
        assert_eq!(p["approvers"], "owner or admin");
        assert_eq!(p["command"], "k2 permission request compute.git-access repos=acme/app,acme/lib --why \"<what you need it for>\"");
    }

    /// 0.46.10: with no GitHub credential the grant teaches the setup card
    /// first (no values: the person types the credential into K2's form),
    /// and so does the agent's own `k2 compute git-access setup`, never
    /// `compute.manage` (the `/cli/compute/` owner_only row), which can't
    /// set a credential.
    #[test]
    fn no_credential_teaches_the_setup_card() {
        let setup = "k2 permission request compute.git-access-setup --why \"<what you need it for>\"";
        let body = json!({"error":{"code":"git_access_needs_setup","hint":"x","workspace":"k2","repos":"acme/app"}});
        let p = teach_of(403, "POST", "/cli/compute/git-access/grant", &[], Some(json!({"github":"acme/app"})), body).unwrap();
        assert_eq!((action_of(&p), &p["params"], p["command"].as_str()), ("compute.git-access-setup", &json!({}), Some(setup)), "{p}");
        assert_eq!(p["approvers"], "owner or admin");
        for req in [json!({"token": "x"}), json!({"app_id": "1", "key_pem": "x"}), json!({"clear": true})] {
            let p = teach_of(403, "POST", "/cli/compute/git-access/setup", &[], Some(req.clone()), json!({"error":{"code":"owner_only","hint":"x"}})).unwrap();
            assert_eq!(p["command"], setup, "{req}: {p}");
        }
        // Another workspace's job caches can't be asked for at all: the
        // compute.manage card this named answered "already on" (a loop).
        for path in ["/cli/compute/cache", "/cli/compute/cache/clear"] {
            let p = teach_of(403, "POST", path, &[], None, json!({"error":{"code":"owner_only","hint":"x"}})).unwrap();
            assert_eq!(p["requestable"], false, "{path}: {p}");
            assert_eq!(p.get("action"), None, "{path}: {p}");
        }
        // owner_only elsewhere under /cli/compute/ is still compute.manage's.
        let p = teach_of(403, "POST", "/cli/compute/nodes/pause", &[], None, json!({"error":{"code":"owner_only","hint":"x"}})).unwrap();
        assert_eq!(action_of(&p), "compute.manage", "{p}");
    }

    /// `sew.space-governance`: the App, the Space and the side come from the
    /// refused call's body; only `person_only` on that route teaches it.
    #[test]
    fn space_governance_teaches_from_the_refused_body() {
        let body = json!({"app":"jobs","spaceId":"5101","governance":"app"});
        let p = teach_of(403, "POST", "/cli/sew/app-spaces/governance", &[], Some(body.clone()), json!({"error":{"code":"person_only","hint":"x"}})).unwrap();
        assert_eq!(action_of(&p), "sew.space-governance", "{p}");
        assert_eq!(p["approvers"], "owner or admin");
        assert_eq!(p["command"], "k2 permission request sew.space-governance app=jobs space=5101 to=app --why \"<what you need it for>\"");
        // Manage Sew off is its own switch, not this card.
        let p = teach_of(403, "POST", "/cli/sew/app-spaces/governance", &[], Some(body), json!({"error":{"code":"chat_manage_off"}})).unwrap();
        assert_ne!(p["action"], "sew.space-governance", "{p}");
    }

    /// `vault.delivery`.
    #[test]
    fn share_service_teaches_only_the_refusal_that_names_a_service() {
        let p = teach_of(
            403,
            "POST",
            "/cli/vault/grant",
            &[],
            Some(json!({"name":"PIN","to":"service:svc","as":"env","envName":"THE_PIN"})),
            json!({"error":{"code":"vault_owner_confirm_required","hint":"x","secret":"PIN","service":"svc","as":"env","envName":"THE_PIN"}}),
        )
        .unwrap();
        assert_eq!(p["params"], json!({"secret":"PIN","service":"svc","as":"env","env_name":"THE_PIN"}), "{p}");
        assert_eq!(p["approvers"], "owner or admin");
        assert_eq!(p["command"], "k2 permission request vault.share-service as=env env_name=THE_PIN secret=PIN service=svc --why \"<what you need it for>\"");

        let p = teach_of(403, "POST", "/cli/vault/grant", &[], Some(json!({"name":"PIN","to":"service:svc"})), json!({"error":{"code":"vault_owner_confirm_required","secret":"PIN","service":"svc","as":"file"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request vault.share-service as=file secret=PIN service=svc --why \"<what you need it for>\"");

        let p = teach_of(403, "POST", "/cli/vault/grant", &[], Some(json!({"name":"PIN","to":"ws:sales"})), json!({"error":{"code":"vault_owner_confirm_required","hint":"x"}})).unwrap();
        assert_eq!(p["requestable"], false, "{p}");
        assert!(p.get("command").is_none(), "{p}");
    }

    /// `app.login-grant`: kind=app with a principal, add or remove, names
    /// the person, the App and the role as the agent gave them; another
    /// kind, or a workspace subject, stays "It can't be requested".
    #[test]
    fn login_grant_teaches_only_an_app_login() {
        let add = json!({"subjectKind":"principal","subjectId":"dana","kind":"App","targetId":"jobs","roleId":"viewer"});
        let p = teach_of(403, "POST", "/cli/skin/grants", &[], Some(add), json!({"error":{"code":"owner_only","hint":"x"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request app.login-grant app=jobs op=add role=viewer user=dana --why \"<what you need it for>\"", "{p}");
        assert_eq!(p["approvers"], "owner", "{p}");
        let rooms = json!({"subjectKind":"principal","subjectId":"dana","kind":"app","targetId":"jobs","rooms":["sales"]});
        let p = teach_of(403, "POST", "/cli/skin/grants", &[], Some(rooms), json!({"error":{"code":"owner_only"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request app.login-grant app=jobs op=add user=dana --why \"<what you need it for>\"", "the card names a role or nothing: {p}");
        let rm = json!({"subjectKind":"principal","subjectId":"dana","kind":"app","targetId":"jobs"});
        let p = teach_of(403, "POST", "/cli/skin/grants/delete", &[], Some(rm), json!({"error":{"code":"owner_only"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request app.login-grant app=jobs op=remove user=dana --why \"<what you need it for>\"", "{p}");
        let p = teach_of(403, "POST", "/cli/skin/grants/delete", &[], Some(json!({"id":"g_1"})), json!({"error":{"code":"owner_only"}})).unwrap();
        assert_eq!(p["command"], "k2 permission request app.login-grant app=<app> op=remove user=<user> --why \"<what you need it for>\"", "{p}");
        for body in [
            json!({"subjectKind":"principal","subjectId":"dana","kind":"mailbox","targetId":"x","scope":"y"}),
            json!({"subjectKind":"workspace","subjectId":"sales","kind":"app","targetId":"jobs"}),
            json!({"subjectKind":"principal","subjectId":"dana","kind":"chat"}),
        ] {
            let p = teach_of(403, "POST", "/cli/skin/grants", &[], Some(body.clone()), json!({"error":{"code":"owner_only"}})).unwrap();
            assert_eq!(p["requestable"], false, "{body}: {p}");
        }
        // The cell socket: the same rows, from the request body alone.
        let empty = HashMap::new();
        let b = br#"{"subjectKind":"principal","subjectId":"dana","kind":"app","targetId":"jobs","roleId":"viewer"}"#;
        let ws = "no-such-workspace";
        assert!(cell_gated_body("POST", "/cli/skin/grants", &empty, b, ws).is_none(), "the gate is off for an unknown workspace");
    }

    /// `sources.approve` teaches the source the agent tried (`name`) and
    /// the spec it reviewed (`specHash`); a batch teaches one ask with the
    /// values left for the agent to fill in. The area's switch still
    /// teaches the other sources routes.
    #[test]
    fn sources_approve_teaches_the_name_and_spec_it_was_refused_on() {
        let hash = "9f".repeat(32);
        let p = teach_of(
            403,
            "POST",
            "/cli/sources/approve",
            &[],
            Some(json!({"name": "fleet", "specHash": hash})),
            json!({"error":{"code":"person_only"}}),
        )
        .unwrap();
        assert_eq!(action_of(&p), "sources.approve", "{p}");
        assert_eq!(p["params"], json!({"source": "fleet", "spec": hash}));
        assert_eq!(p["approvers"], "owner or admin");
        assert_eq!(
            p["command"],
            format!("k2 permission request sources.approve source=fleet spec={hash} --why \"<what you need it for>\"")
        );

        let p = teach_of(
            403,
            "POST",
            "/cli/sources/batch-approve",
            &[],
            Some(json!({"items": [{"name": "fleet", "specHash": hash}, {"name": "ci", "specHash": hash}]})),
            json!({"error":{"code":"person_only"}}),
        )
        .unwrap();
        assert_eq!(action_of(&p), "sources.approve", "{p}");
        assert_eq!(p["params"], json!({"source": null, "spec": null}));
        assert_eq!(p["command"], "k2 permission request sources.approve source=<source> spec=<spec> --why \"<what you need it for>\"");

        // Another sources route still teaches the area's switch.
        let p = teach_of(403, "POST", "/cli/sources/secret", &[], None, json!({"error":{"code":"person_only"}})).unwrap();
        assert_eq!(action_of(&p), "gardens.manage", "{p}");
    }

    /// prd-garden-access-v1 G16: the two Garden walls teach their actions
    /// with the Garden the refusal named; the computer-wide look (no
    /// Garden) leaves `garden=` for the agent to fill in; `person_only`
    /// under /cli/zen/ still teaches gardens.manage, and the delete wall
    /// takes the delete route only.
    #[test]
    fn the_garden_walls_name_the_garden() {
        let p = teach_of(403, "POST", "/cli/zen/garden/put", &[], Some(json!({"garden":"Sales board"})),
            json!({"ok":false,"error":"garden_access_needed","garden":"garden-2","card":null})).unwrap();
        assert_eq!((action_of(&p), &p["params"], p["approvers"].as_str()), ("garden.access", &json!({"garden":"garden-2"}), Some("owner")), "{p}");
        assert_eq!(p["command"], "k2 permission request garden.access garden=garden-2 --why \"<what you need it for>\"");
        let p = teach_of(403, "POST", "/cli/zen/theme/set", &[], Some(json!({"name":"dusk"})),
            json!({"ok":false,"error":"garden_access_needed","garden":null,"card":null})).unwrap();
        assert_eq!((action_of(&p), &p["params"]), ("garden.access", &json!({"garden":null})), "{p}");
        assert_eq!(p["command"], "k2 permission request garden.access garden=<garden> --why \"<what you need it for>\"");
        let p = teach_of(403, "POST", "/cli/zen/garden/delete", &[], Some(json!({"garden":"garden-4"})),
            json!({"ok":false,"error":"garden_delete_person","garden":"garden-4"})).unwrap();
        assert_eq!((action_of(&p), &p["params"]), ("garden.delete", &json!({"garden":"garden-4"})), "{p}");
        // The delete code on another route is no wall of garden.delete's.
        let p = teach_of(403, "POST", "/cli/zen/garden/rename", &[], None, json!({"ok":false,"error":"garden_delete_person"})).unwrap();
        assert_eq!(p["requestable"], false, "{p}");
        // person_only under /cli/zen/ is still the switch's.
        let p = teach_of(403, "POST", "/cli/zen/widget/preview", &[], None, json!({"error":"person_only"})).unwrap();
        assert_eq!(action_of(&p), "gardens.manage", "{p}");
    }

    #[test]
    fn walls_no_switch_opens_say_it_cant_be_requested() {
        for (path, resp) in [
            ("/cli/compute/nodes/remove", json!({"error":{"code":"owner_only"}})),
            ("/cli/compute/local/enable", json!({"error":{"code":"owner_only"}})),
            ("/cli/domains", json!({"error":{"code":"owner_only"},"ok":false})),
            // mail's owner-level mutations carry no `error.workspace`.
            ("/cli/mail/server/uninstall", json!({"error":{"code":"owner_only"},"ok":false})),
            ("/cli/tunnel/start", json!({"error":"invalid or missing token"})),
            ("/cli/zen/widget/claim", json!({"error":"owner_only","ok":false})),
            ("/cli/front/enable", json!({"error":{"code":"owner_only"},"ok":false})),
        ] {
            let p = teach_of(403, "POST", path, &[], None, resp).unwrap();
            assert_eq!(p["requestable"], false, "{path}: {p}");
            assert!(p.get("action").is_none() && p.get("command").is_none(), "{path}: {p}");
            assert!(p["where"].as_str().is_some_and(|w| !w.is_empty()), "{path}: {p}");
        }
        // `k2 front status|disable` name the front's people (reads too); a
        // passthrough change keeps the firewall's.
        for (method, path, want) in [
            ("POST", "/cli/sew/home/client", HOME_CLIENT_WHERE),
            ("POST", "/cli/sew/home/app", HOME_APP_WHERE),
            ("GET", "/cli/front/status", FRONT_WHERE),
            ("POST", "/cli/front/disable", FRONT_WHERE),
            ("POST", "/cli/front/passthrough/add", FIREWALL_WHERE),
            // An agent can't raise the forward cap, mail.manage or not.
            ("POST", "/cli/mail/config/forward-new-per-day", FORWARD_CAP_WHERE),
            // Nor how much a mailbox may forward.
            ("POST", "/cli/mail/config/forward-copies-per-day", FORWARD_COPIES_DAY_WHERE),
            ("POST", "/cli/mail/config/forward-copies-per-hour", FORWARD_COPIES_HOUR_WHERE),
        ] {
            let p = teach_of(403, method, path, &[], None, json!({"error":{"code":"owner_only"},"ok":false})).unwrap();
            assert_eq!(p["requestable"], false, "{path}: {p}");
            assert_eq!(p["where"], want, "{path}: {p}");
        }
    }

    #[test]
    fn non_refusals_get_nothing() {
        for (status, resp) in [
            (200, json!({"ok":true})),
            (202, json!({"ok":true,"status":"queued"})),
            (400, json!({"error":{"code":"bad_args"}})),
            (404, json!({"error":"not found"})),
            (409, json!({"error":{"code":"conflict"}})),
        ] {
            assert!(teach_of(status, "POST", "/cli/compute/run", &[], None, resp.clone()).is_none(), "{status} {resp}");
        }
        // A held-shaped body on a route with no held wall is not a hold.
        assert!(teach_of(202, "POST", "/cli/compute/run", &[], None, json!({"status":"held"})).is_none());
    }

    #[test]
    fn a_machine_only_refusal_carries_its_plain_text_hint() {
        let p = teach_of(
            403,
            "POST",
            "/cli/compute/admin/propose",
            &[],
            Some(json!({"node":"z3mbpz"})),
            json!({"error":{"code":"remote_admin_off","hint":"z3mbpz only takes admin steps from someone at that Mac."}}),
        )
        .unwrap();
        assert_eq!(
            p,
            json!({"requestable": false, "where": "at the machine",
                   "hint": "At z3mbpz, run: sudo k2-node admin level policy   (switch back after: sudo k2-node admin level off)"})
        );
        let p = teach_of(403, "POST", "/cli/x", &[], None, json!({"error":{"code":"remote_admin_off"}})).unwrap();
        assert!(p["hint"].as_str().unwrap().starts_with("At that machine, run:"), "{p}");
    }

    #[test]
    fn every_row_is_well_formed() {
        for r in ROWS {
            assert!(r.prefix.starts_with("/cli/"), "{}", r.prefix);
            if let Target::Ask { action, .. } = r.target {
                assert!(V1_ACTIONS.contains(&action), "{action} is not a v1 action");
                assert_ne!(action, "garden.keep-mine", "keep-mine is K2-presented; it has no wall");
                assert!(!action.ends_with("-off"), "{action}: a twin is asked, never a wall");
            }
            if let WallMatch::Held { .. } = r.matches {
                assert!(matches!(r.target, Target::Ask { .. }), "a hold always names its request");
            }
        }
        for (code, hint) in MACHINE_HINTS {
            assert!(!code.is_empty() && hint.contains("{node}"));
            assert!(
                !ROWS.iter().any(|r| matches!(r.matches, WallMatch::Code { code: c, .. } if c == *code)),
                "{code} is machine-only and can't also be a wall"
            );
        }
        assert_eq!(V1_ACTIONS.len(), 89);
        let mut seen = std::collections::HashSet::new();
        for a in V1_ACTIONS {
            assert!(seen.insert(*a), "{a} is listed twice in V1_ACTIONS");
        }
        // No two rows can claim the same refusal: the longest prefix must
        // decide alone.
        for (i, a) in ROWS.iter().enumerate() {
            for b in &ROWS[i + 1..] {
                let same = a.matches == b.matches
                    && a.prefix == b.prefix
                    && a.method == b.method
                    && a.when == b.when
                    && a.needs == b.needs
                    && a.body_has == b.body_has
                    && a.body_eq == b.body_eq
                    && a.own_ws == b.own_ws
                    && a.body_lacks == b.body_lacks;
                assert!(!same, "two rows claim {:?} on {}", a.matches, a.prefix);
            }
        }
    }

    /// 0.46.17: `voice.recording-dm` is only `changes` with
    /// `recording.dm_enabled` alone; any other policy key in the same body
    /// is no card's (the person's own `k2 hostchat recording policy set`).
    #[test]
    fn the_dm_card_is_only_the_dm_key_alone() {
        let params = HashMap::new();
        let resp = json!({"ok": false, "error": {"code": "person_only", "hint": "x"}});
        let at = |body: Value| -> (Option<&'static str>, Vec<(&'static str, &'static str)>) {
            let req = Refused { status: 403, method: "POST", path: "/cli/sew/recording/policy", params: &params, body: Some(&body), response: &resp, workspace_id: "" };
            match find_row(&req) {
                Some(Row { target: Target::Ask { action, .. }, fixed, .. }) => (Some(*action), fixed.to_vec()),
                _ => (None, Vec::new()),
            }
        };
        assert_eq!(at(json!({"changes": {"recording.dm_enabled": true}})), (Some("voice.recording-dm"), vec![("dm", "on")]));
        assert_eq!(at(json!({"changes": {"recording.dm_enabled": false}})), (Some("voice.recording-dm"), vec![("dm", "off")]));
        for (other, v) in [
            ("recording.enabled", json!(true)),
            ("recording.require_consent_to_join", json!(false)),
            ("recording.required", json!("multi_person")),
            ("recording.audio_max_days", json!(30)),
        ] {
            let mut b = json!({"changes": {"recording.dm_enabled": true}});
            b["changes"][other] = v;
            assert_ne!(at(b).0, Some("voice.recording-dm"), "{other} beside dm_enabled is no card");
        }
        assert_ne!(at(json!({"changes": {"recording.enabled": true}})).0, Some("voice.recording-dm"));
    }

    /// 0.46.18 (HR-12): an agent's `policy set enabled on`, alone, is the
    /// `sew.recording-enable` card (an Owner or Admin, with a confirm);
    /// turning it off, or any other key beside it, is no card's.
    #[test]
    fn the_enable_card_is_only_recording_enabled_on_alone() {
        let params = HashMap::new();
        let resp = json!({"ok": false, "error": {"code": "person_only", "hint": "x"}});
        let at = |body: Value| -> Option<&'static str> {
            let req = Refused { status: 403, method: "POST", path: "/cli/sew/recording/policy", params: &params, body: Some(&body), response: &resp, workspace_id: "" };
            match find_row(&req) {
                Some(Row { target: Target::Ask { action, .. }, .. }) => Some(*action),
                _ => None,
            }
        };
        assert_eq!(at(json!({"changes": {"recording.enabled": true}})), Some("sew.recording-enable"));
        assert_eq!(at(json!({"changes": {"recording.enabled": false}})), None, "off is the person's own path");
        for (other, v) in [
            ("recording.dm_enabled", json!(true)),
            ("recording.require_consent_to_join", json!(false)),
            ("recording.required", json!("multi_person")),
            ("recording.audio_max_days", json!(30)),
        ] {
            let mut b = json!({"changes": {"recording.enabled": true}});
            b["changes"][other] = v;
            assert_eq!(at(b), None, "{other} beside enabled is no card");
        }
    }

    /// The E lanes register each action's spec with its own floor and
    /// walls; once registered, this map and the registry must agree.
    #[test]
    fn rows_agree_with_the_registry() {
        for reg in crate::permission::actions::all() {
            let id = reg.spec.id;
            let mine: Vec<&Row> = ROWS.iter().filter(|r| matches!(r.target, Target::Ask { action, .. } if action == id)).collect();
            for r in &mine {
                let Target::Ask { floor, .. } = r.target else { unreachable!() };
                assert_eq!(floor, reg.spec.floor, "{id}: the walls map and the catalog disagree on the floor");
            }
            for w in reg.spec.walls {
                assert!(
                    mine.iter().any(|r| r.matches == w.matches),
                    "{id}: the catalog lists wall {:?} that the walls map can't teach",
                    w.matches
                );
            }
        }
    }

    #[test]
    fn add_permission_keeps_every_other_byte() {
        let p = json!({"requestable": false, "where": "x"});
        assert_eq!(
            add_permission(r#"{"error":{"code":"owner_only"},"ok":false}"#, &p).unwrap(),
            r#"{"error":{"code":"owner_only"},"ok":false,"permission":{"requestable":false,"where":"x"}}"#
        );
        assert_eq!(add_permission("{}", &p).unwrap(), r#"{"permission":{"requestable":false,"where":"x"}}"#);
        assert_eq!(add_permission("{\n  \"a\": 1\n}\n", &p).unwrap(), "{\n  \"a\": 1\n,\"permission\":{\"requestable\":false,\"where\":\"x\"}}\n");
        assert!(add_permission("[1]", &p).is_none());
        assert!(add_permission("not json", &p).is_none());
        // The result still parses.
        let v: Value = serde_json::from_str(&add_permission("{\n  \"a\": 1\n}\n", &p).unwrap()).unwrap();
        assert_eq!(v["permission"]["where"], "x");
    }

    #[test]
    fn status_lines_parse() {
        assert_eq!(status_code("403 Forbidden"), 403);
        assert_eq!(status_code("202 Accepted"), 202);
        assert_eq!(status_code(""), 0);
    }

    /// 0.46.8: the two chat.account walls with no request body to read, the
    /// whoami flavour of `not_granted` only (a workspace with an account
    /// and no Space is a person's), and the stop-sending ask naming the
    /// workspace from the refused body.
    #[test]
    fn chat_account_and_stop_sending_teach_from_the_refusal() {
        let p = teach_of(
            403,
            "POST",
            "/cli/sew/chat/manage/space/create",
            &[],
            Some(json!({"slug":"jobs","name":"Jobs","app":"jobs"})),
            json!({"ok":false,"error":{"code":"chat_account_needed","hint":"x","details":{"workspace":"alpha"}}}),
        )
        .expect("taught");
        assert_eq!(p["command"], "k2 permission request chat.account scope=coworker --why \"<what you need it for>\"", "{p}");
        assert_eq!(p["approvers"], "owner", "{p}");
        let p = teach_of(
            403,
            "GET",
            "/cli/sew/chat/whoami",
            &[],
            None,
            json!({"ok":false,"error":{"code":"not_granted","hint":"x","details":{"chatAccount":false}}}),
        )
        .expect("taught");
        assert_eq!(p["command"], "k2 permission request chat.account scope=coworker --why \"<what you need it for>\"", "{p}");
        // The switch off: the broad switch first.
        let p = teach_of(
            403,
            "GET",
            "/cli/sew/chat/whoami",
            &[],
            None,
            json!({"ok":false,"error":{"code":"not_granted","hint":"x","details":{"chatManage":false}}}),
        )
        .expect("taught");
        assert_eq!(p["command"], "k2 permission request chat.manage --why \"<what you need it for>\"", "{p}");
        let p = teach_of(403, "GET", "/cli/sew/chat/whoami", &[], None, json!({"ok":false,"error":{"code":"not_granted","hint":"x"}}))
            .expect("a refusal");
        assert_eq!(p["requestable"], false, "Space access is a person's: {p}");
        let p = teach_of(
            403,
            "POST",
            "/cli/mail/config/set",
            &[],
            Some(json!({"workspace":"/Users/x/sales","agentSend":"off"})),
            json!({"ok":false,"error":{"code":"owner_only","hint":"x"}}),
        )
        .expect("taught");
        assert_eq!(p["command"], "k2 permission request mail.stop-sending target=/Users/x/sales --why \"<what you need it for>\"", "{p}");
        assert_eq!(p["approvers"], "owner or admin", "{p}");
    }

    /// 0.46.17: an agent's `k2 llm tokens refresh`
    /// is people only; its refusal ends with the recheck card's line, for the
    /// token it named or for every token, never "It can't be requested". The
    /// other token changes (switch, sign-in, remove) stay a person's.
    #[test]
    fn an_agents_token_refresh_teaches_the_recheck_card() {
        let refused = json!({"ok":false,"error":{"code":"owner_only","hint":"x"}});
        let p = teach_of(403, "POST", "/cli/llm/accounts/refresh", &[], Some(json!({"id":"acc_0123"})), refused.clone()).expect("taught");
        assert_eq!(p["command"], "k2 permission request llm.tokens-refresh token=acc_0123 --why \"<what you need it for>\"", "{p}");
        assert_eq!(p["approvers"], "owner or admin", "{p}");
        let p = teach_of(403, "POST", "/cli/llm/accounts/refresh", &[], Some(json!({})), refused.clone()).expect("taught");
        assert_eq!(p["command"], "k2 permission request llm.tokens-refresh --why \"<what you need it for>\"", "{p}");
        for path in ["/cli/llm/accounts/switch", "/cli/llm/accounts/login", "/cli/llm/accounts/remove"] {
            let p = teach_of(403, "POST", path, &[], Some(json!({"id":"acc_0123"})), refused.clone()).expect("a refusal");
            assert_eq!(p["requestable"], false, "{path}: {p}");
        }
    }

    /// 0.46.18: a name another workspace holds (a server swap): the claim's,
    /// run's and start's 409 and the --transfer 403 end with the
    /// publish.transfer line for that name; a 409 elsewhere, or one with
    /// another code, doesn't.
    #[test]
    fn a_label_another_workspace_holds_teaches_the_transfer_card() {
        let owned = json!({"code":"label_owned_by_other_workspace","error":"label_owned_by_other_workspace: 'shop' belongs to another workspace on this server, maintained by Sales (sales).","hint":"x","label":"shop","owner":{"agent":"Sales","kind":"agent"}});
        let want = "k2 permission request publish.transfer label=shop --why \"<what you need it for>\"";
        for path in ["/cli/publish/subdomain/claim", "/cli/publish/run", "/cli/publish/start"] {
            let p = teach_of(409, "POST", path, &[("label", "shop"), ("check", "1")], None, owned.clone()).expect("taught");
            assert_eq!(p["command"], want, "{path}: {p}");
            assert_eq!(p["approvers"], "owner or admin", "{p}");
        }
        let xfer = json!({"code":"transfer_owner_only","error":"transfer_owner_only: only the server owner or an admin can transfer 'shop'.","hint":"x","label":"shop"});
        let p = teach_of(403, "POST", "/cli/publish/subdomain/claim", &[("label", "shop"), ("transfer", "1")], None, xfer).expect("taught");
        assert_eq!(p["command"], want, "{p}");
        // The label from the form when the body has none.
        let bare = json!({"code":"transfer_owner_only","error":"transfer_owner_only: no"});
        let p = teach_of(403, "POST", "/cli/publish/subdomain/claim", &[("label", "shop")], None, bare).expect("taught");
        assert_eq!(p["command"], want, "{p}");
        // Not a publish route, or another 409: nothing of this card's.
        assert!(teach_of(409, "POST", "/cli/app/domain/add", &[], None, owned.clone()).is_none_or(|p| p["action"] != "publish.transfer"));
        let other = json!({"code":"label_needs_owner","error":"label_needs_owner: x","label":"shop"});
        assert!(teach_of(409, "POST", "/cli/publish/subdomain/claim", &[], None, other).is_none_or(|p| p["action"] != "publish.transfer"));
    }

    /// 0.46.9 (Rosson 2026-11-05): a held compute node request (`held:
    /// true`, its `car_…` id) teaches the ask that brings it to chat, on the
    /// writes that hold; anything else under compute stays as it was.
    #[test]
    fn a_held_compute_node_request_teaches_its_card() {
        let held = json!({"held": true, "request": {"id": "car_abc123", "state": "pending"}, "hint": "held for the owner"});
        let want = "k2 permission request compute.admin-approve request=car_abc123 --why \"<what you need it for>\"";
        for path in [
            "/cli/compute/admin/propose",
            "/cli/compute/nodes/policy/set",
            "/cli/compute/nodes/servers/limits",
            "/cli/compute/nodes/servers/add",
            "/cli/compute/nodes/wall",
        ] {
            let p = teach_of(200, "POST", path, &[], Some(json!({"node": "k2n1-m4"})), held.clone()).unwrap_or_else(|| panic!("{path}: not taught"));
            assert_eq!(action_of(&p), "compute.admin-approve", "{path}: {p}");
            assert_eq!(p["approvers"], "owner", "{path}: {p}");
            assert_eq!(p["command"], want, "{path}: {p}");
        }
        // Not held, held false, a read, or another family: nothing to teach.
        assert!(teach_of(200, "POST", "/cli/compute/nodes/policy/set", &[], None, json!({"ok": true, "changed": true})).is_none());
        assert!(teach_of(200, "POST", "/cli/compute/nodes/policy/set", &[], None, json!({"held": false})).is_none());
        assert!(teach_of(200, "GET", "/cli/compute/nodes/health", &[], None, held.clone()).is_none());
        assert!(teach_of(200, "POST", "/cli/compute/run", &[], None, held.clone()).is_none());
    }

    /// 0.46.9: a filed package request teaches `compute.packages` with its
    /// `cpr_…` id, from the hold (`pkgHeld`, the request verb or a prewarm)
    /// and from a job's `package_not_approved` refusal (run, test,
    /// prewarm); a node admin hold (`held`) still teaches
    /// `compute.admin-approve`, and a `pkgHeld` answer never does.
    #[test]
    fn a_package_request_teaches_its_own_card() {
        let id = "cpr_0123456789abcdef";
        let want = format!("k2 permission request compute.packages request={id} --why \"<what you need it for>\"");
        let held = json!({"pkgHeld": true, "request": {"id": id, "summary": "Install 1 package on k2n7 for agent tests: libgtk-3-dev"}});
        for path in ["/cli/compute/nodes/packages/request", "/cli/compute/prewarm"] {
            let p = teach_of(200, "POST", path, &[], Some(json!({"nodes":["k2n7"]})), held.clone()).unwrap_or_else(|| panic!("{path}: no permission"));
            assert_eq!(action_of(&p), "compute.packages", "{path}: {p}");
            assert_eq!(p["params"], json!({"request": id}), "{path}: {p}");
            assert_eq!(p["command"], want, "{path}");
            assert_eq!(p["approvers"], Floor::Admin.label(), "{path}: {p}");
        }
        let refused = json!({"error": {"code": "package_not_approved", "hint": "x", "request": id, "node": "k2n7", "packages": ["libgtk-3-dev"]}});
        for path in ["/cli/compute/run", "/cli/compute/test", "/cli/compute/prewarm"] {
            let p = teach_of(403, "POST", path, &[], Some(json!({"node":"k2n7"})), refused.clone()).unwrap_or_else(|| panic!("{path}: no permission"));
            assert_eq!(action_of(&p), "compute.packages", "{path}: {p}");
            assert_eq!(p["command"], want, "{path}");
        }
        // Nothing held here (only another server's owner was asked), or a
        // GET, or a 2xx elsewhere: nothing to ask.
        assert!(teach_of(200, "POST", "/cli/compute/nodes/packages/request", &[], None, json!({"pkgHeld": false, "request": {"id": id}})).is_none());
        assert!(teach_of(200, "GET", "/cli/compute/nodes/packages/request", &[], None, held.clone()).is_none());
        assert!(teach_of(200, "POST", "/cli/compute/run", &[], None, held.clone()).is_none());
        // A `pkgHeld` answer under /cli/compute/nodes/ never raises the node
        // admin card; a `held` one there still does.
        for path in ["/cli/compute/nodes/packages/request", "/cli/compute/nodes/packages/policy", "/cli/compute/nodes/wall"] {
            let p = teach_of(200, "POST", path, &[], None, held.clone());
            assert!(p.as_ref().is_none_or(|p| action_of(p) != "compute.admin-approve"), "{path}: {p:?}");
        }
        let node_hold = json!({"held": true, "request": {"id": "car_abc123"}});
        let p = teach_of(200, "POST", "/cli/compute/nodes/packages/policy", &[], None, node_hold).expect("a held policy widening");
        assert_eq!(action_of(&p), "compute.admin-approve", "{p}");
        assert_eq!(p["params"], json!({"request": "car_abc123"}), "{p}");
    }

    /// The package policy route to an agent that may not operate nodes asks
    /// the owner for that exact policy; elsewhere `operate_off` stays
    /// compute.operate.
    #[test]
    fn the_package_policy_wall_names_the_node_server_and_level() {
        let off = json!({"error": {"code": "operate_off", "hint": "x"}});
        let p = teach_of(403, "POST", "/cli/compute/nodes/packages/policy", &[], Some(json!({"node":"k2n7","server":"acme.k2.dev","level":"profiles"})), off.clone())
            .expect("a permission");
        assert_eq!(action_of(&p), "compute.packages-policy", "{p}");
        assert_eq!(p["approvers"], Floor::Owner.label(), "{p}");
        assert_eq!(
            p["command"],
            "k2 permission request compute.packages-policy level=profiles node=k2n7 server=acme.k2.dev --why \"<what you need it for>\""
        );
        let p = teach_of(403, "POST", "/cli/compute/admin/propose", &[], Some(json!({"node":"k2n7"})), off).expect("a permission");
        assert_eq!(action_of(&p), "compute.operate", "{p}");
    }

    /// 0.46.9 (Rosson 2026-11-05): the agent's own `k2 compute access on`
    /// asks for compute.use: with its own workspace named by any name, or
    /// none named (the CLI sends none from a K2 terminal). Never for another
    /// workspace, never for `off`, never for another toggle
    /// (`compute_manage` stays the owner's: "It can't be requested").
    #[test]
    fn compute_access_on_asks_for_compute_use_for_its_own_workspace_only() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let me = crate::permission::actions::held::tests::seed("wca");
        let other = crate::permission::actions::held::tests::seed("wcb");
        // The bytes the route answers every passport (the off golden).
        let refused = json!({"error":{"code":"forbidden","hint":"only the owner or an admin login may change agent access"}});
        let teach_as = |ws: &str, body: Value| {
            let params = HashMap::new();
            permission_for(&Refused {
                status: 403,
                method: "POST",
                path: "/cli/agent-access/set",
                params: &params,
                body: Some(&body),
                response: &refused,
                workspace_id: ws,
            })
            .expect("a passport's refusal always carries a permission object")
        };
        let want = "k2 permission request compute.use --why \"<what you need it for>\"";
        for body in [
            json!({"toggle":"compute","value":true}),
            json!({"toggle":"compute","value":true,"workspace":""}),
            json!({"toggle":"compute","value":true,"workspace":me.handle}),
            json!({"toggle":"compute","value":true,"workspace":me.id}),
        ] {
            let p = teach_as(&me.id, body.clone());
            assert_eq!(action_of(&p), "compute.use", "{body}: {p}");
            assert_eq!(p["params"], json!({}), "{body}: the workspace is never a value: {p}");
            assert_eq!(p["approvers"], "owner or admin", "{body}: {p}");
            assert_eq!(p["command"], want, "{body}: {p}");
        }
        for body in [
            // Another workspace's switch.
            json!({"toggle":"compute","value":true,"workspace":other.handle}),
            json!({"toggle":"compute","value":true,"workspace":other.id}),
            // Off is not this card.
            json!({"toggle":"compute","value":false}),
            json!({"toggle":"compute","value":false,"workspace":me.handle}),
            // "Allow managing compute nodes" stays the owner's own act.
            json!({"toggle":"compute_manage","value":true}),
            json!({"toggle":"compute_manage","value":true,"workspace":me.handle}),
            // Other switches through the same route are not this card.
            json!({"toggle":"compute_operate","value":true}),
            json!({"toggle":"permission_cards","value":true}),
            // Not the CLI's body.
            json!({"toggle":"compute","value":"on"}),
            json!({"value":true}),
        ] {
            let p = teach_as(&me.id, body.clone());
            assert_eq!(p["requestable"], false, "{body}: {p}");
            assert!(p.get("action").is_none() && p.get("command").is_none(), "{body}: {p}");
        }
        // With no known asker, a named workspace never matches.
        let p = teach_as("", json!({"toggle":"compute","value":true,"workspace":me.handle}));
        assert_eq!(p["requestable"], false, "{p}");
    }

    /// 0.46.17: the agent's own `k2 compute grant <node> --max-time 4h`
    /// asks for compute.grant with every limit it typed (max-time as the
    /// duration, the CLI sends seconds), for its own workspace named by any
    /// name or none. Another workspace's grant, and `--ci` / `--gpu` (on or
    /// off), stay compute.use's row.
    #[test]
    fn compute_grant_teaches_the_limits_typed_for_its_own_grant_only() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let me = crate::permission::actions::held::tests::seed("wgm");
        let other = crate::permission::actions::held::tests::seed("wgo");
        let refused = json!({"ok":false,"error":{"code":"owner_only","hint":"only the server's owner or an admin"}});
        let teach_as = |body: Value| {
            let params = HashMap::new();
            permission_for(&Refused {
                status: 403,
                method: "POST",
                path: "/cli/compute/grants/set",
                params: &params,
                body: Some(&body),
                response: &refused,
                workspace_id: &me.id,
            })
            .expect("a passport's refusal always carries a permission object")
        };
        let want = "k2 permission request compute.grant max-time=4h node=node-a --why \"<what you need it for>\"";
        for body in [
            json!({"node":"node-a","maxJobSecs":14400}),
            json!({"node":"node-a","workspace":"","maxJobSecs":14400}),
            json!({"node":"node-a","workspace":me.handle,"maxJobSecs":14400}),
            json!({"node":"node-a","workspace":me.id,"maxJobSecs":14400}),
            // The per-submitter cap isn't the card's; it's left off the line.
            json!({"node":"node-a","workspace":me.handle,"maxJobSecs":14400,"submitterMaxRunning":2}),
        ] {
            let p = teach_as(body.clone());
            assert_eq!(action_of(&p), "compute.grant", "{body}: {p}");
            assert_eq!(p["params"], json!({"node":"node-a","max-time":"4h"}), "{body}: the workspace is never a value: {p}");
            assert_eq!(p["approvers"], "owner or admin", "{body}: {p}");
            assert_eq!(p["command"], want, "{body}: {p}");
        }
        let p = teach_as(json!({"node":"node-a","maxJobSecs":5400,"maxParallel":2,"maxQueued":20,"maxDiskGb":120}));
        assert_eq!(
            p["command"],
            "k2 permission request compute.grant max-disk=120 max-parallel=2 max-queued=20 max-time=1h30m node=node-a --why \"<what you need it for>\"",
            "{p}"
        );
        // No limit typed: the card itself says to name one.
        let p = teach_as(json!({"node":"node-a"}));
        assert_eq!(p["command"], "k2 permission request compute.grant node=node-a --why \"<what you need it for>\"", "{p}");
        for body in [
            json!({"node":"node-a","workspace":other.handle,"maxJobSecs":14400}),
            json!({"node":"node-a","workspace":other.id,"maxJobSecs":14400}),
            json!({"node":"node-a","maxJobSecs":14400,"allowCi":true}),
            json!({"node":"node-a","maxJobSecs":14400,"allowCi":false}),
            json!({"node":"node-a","workspace":me.handle,"maxJobSecs":14400,"allowGpu":true}),
            json!({"node":"node-a","maxJobSecs":14400,"allowGpu":false}),
            json!({"node":"node-a","maxJobSecs":14400,"allowUserns":true}),
            json!({"node":"node-a","maxJobSecs":14400,"allowNoAddrFilter":true}),
        ] {
            let p = teach_as(body.clone());
            assert_eq!(action_of(&p), "compute.use", "{body}: never compute.grant: {p}");
        }
    }

    /// 0.46.8: `own_ws` rows teach only for the asking agent's own
    /// workspace, by any of its names. Another workspace's chat grant can't
    /// be requested; another workspace's sending on stays mail.manage's.
    #[test]
    fn own_workspace_rows_match_only_the_askers_workspace() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let me = crate::permission::actions::held::tests::seed("wom");
        let other = crate::permission::actions::held::tests::seed("woo");
        let mine_path = format!("/tmp/pc-e2-{}", me.handle);
        let owner_only = json!({"ok":false,"error":{"code":"owner_only","hint":"x"}});
        let teach_as = |ws: &str, path: &str, body: Value, response: &Value| {
            let params = HashMap::new();
            permission_for(&Refused { status: 403, method: "POST", path, params: &params, body: Some(&body), response, workspace_id: ws })
        };
        let upper = me.handle.to_uppercase();
        for token in [me.handle.as_str(), me.id.as_str(), mine_path.as_str(), upper.as_str()] {
            let body = json!({"subjectKind":"workspace","subjectId":token,"kind":"chat","targetId":"sew","scope":"coworker"});
            let p = teach_as(&me.id, "/cli/skin/grants", body, &owner_only).expect("taught");
            assert_eq!(p["command"], "k2 permission request chat.account scope=coworker --why \"<what you need it for>\"", "{token}: {p}");
        }
        // No scope in the body: the card picks its default.
        let p = teach_as(&me.id, "/cli/skin/grants", json!({"subjectKind":"workspace","subjectId":me.handle,"kind":"chat"}), &owner_only)
            .expect("taught");
        assert_eq!(p["command"], "k2 permission request chat.account --why \"<what you need it for>\"", "{p}");
        // Another workspace's chat grant: never this card.
        let body = json!({"subjectKind":"workspace","subjectId":other.handle,"kind":"chat","scope":"coworker"});
        let p = teach_as(&me.id, "/cli/skin/grants", body, &owner_only).expect("a refusal");
        assert_eq!(p["requestable"], false, "{p}");
        assert!(p.get("action").is_none(), "{p}");
        // With no known asker, an own-workspace row never matches.
        let body = json!({"subjectKind":"workspace","subjectId":me.handle,"kind":"chat"});
        assert_eq!(teach_as("", "/cli/skin/grants", body, &owner_only).expect("a refusal")["requestable"], false);

        // Its own sending, approval or on: the agent's own switch.
        for mode in ["approval", "on"] {
            let p = teach_as(&me.id, "/cli/mail/config/set", json!({"workspace": me.handle, "agentSend": mode}), &owner_only).expect("taught");
            assert_eq!(p["command"], format!("k2 permission request mail.send-mode mode={mode} --why \"<what you need it for>\""), "{p}");
        }
        // Another workspace's sending on: what it was (mail.manage, from the
        // refusal that names the asker's own toggle).
        let manage = json!({"ok":false,"error":{"code":"owner_only","hint":"x","workspace":{"name":me.handle,"path":mine_path,"id":me.id}}});
        let p = teach_as(&me.id, "/cli/mail/config/set", json!({"workspace": other.handle, "agentSend": "on"}), &manage).expect("taught");
        assert_eq!(action_of(&p), "mail.manage", "{p}");
        // Off: any workspace, its own too.
        for ws in [&other.handle, &me.handle] {
            let p = teach_as(&me.id, "/cli/mail/config/set", json!({"workspace": ws, "agentSend": "off"}), &owner_only).expect("taught");
            assert_eq!(p["command"], format!("k2 permission request mail.stop-sending target={ws} --why \"<what you need it for>\""), "{p}");
        }
    }

    /// 0.46.18: the address cap's three refusals end with the
    /// `mail.address-cap` card's line: `k2 hostmail create` past the cap
    /// (409, the workspace from the refusal, the cap for the agent to fill
    /// in), `k2 hostmail config --address-cap` on its own workspace
    /// (`owner_only`), and by an agent without "manage hosted mail"
    /// (`agent_send_path_only`). Bundled with agentSend or always-BCC it's
    /// theirs, and another workspace's `owner_only` stays mail.manage's.
    #[test]
    fn address_cap_refusals_teach_the_cap_card() {
        let _home = crate::test_support::TempHome::new();
        k2_core::db::init_for_tests();
        let me = crate::permission::actions::held::tests::seed("wac");
        let other = crate::permission::actions::held::tests::seed("wad");
        let teach_as = |status: u16, path: &str, body: Value, response: &Value| {
            let params = HashMap::new();
            permission_for(&Refused { status, method: "POST", path, params: &params, body: Some(&body), response, workspace_id: &me.id })
        };
        let full = |rest: &str| format!("{CLI_NAME} permission request mail.address-cap {rest} --why \"<what you need it for>\"");

        // Past the cap: the refusal names the workspace.
        let cap_reached = json!({"ok":false,"error":{"code":"cap_reached","hint":"x","workspace":me.handle,"cap":5}});
        let p = teach_as(409, "/cli/mail/address/create", json!({"project": me.handle, "localPart": "bot6"}), &cap_reached).expect("taught");
        assert_eq!(p["command"], full(&format!("cap=<cap> target={}", me.handle)), "{p}");
        assert_eq!(p["approvers"], "owner or admin", "{p}");
        // Another 409 on create (a taken name) is no card's.
        let exists = json!({"ok":false,"error":{"code":"exists","hint":"x"}});
        assert!(teach_as(409, "/cli/mail/address/create", json!({"localPart": "bot"}), &exists).is_none());

        // Its own workspace's cap (it manages email hosting): owner_only.
        let owner_only = json!({"ok":false,"error":{"code":"owner_only","hint":"x"}});
        for (cap, word) in [(json!(50), "50"), (json!("none"), "none"), (json!(0), "0")] {
            let p = teach_as(403, "/cli/mail/config/set", json!({"workspace": me.handle, "addressCap": cap}), &owner_only).expect("taught");
            assert_eq!(p["command"], full(&format!("cap={word} target={}", me.handle)), "{p}");
        }
        // Without the switch, any workspace.
        let path_only = json!({"ok":false,"error":{"code":"agent_send_path_only","hint":"x"}});
        for ws in [&me.handle, &other.handle] {
            let p = teach_as(403, "/cli/mail/config/set", json!({"workspace": ws, "addressCap": 9}), &path_only).expect("taught");
            assert_eq!(p["command"], full(&format!("cap=9 target={ws}")), "{p}");
        }
        // Bundled with agent mail policy: not this card.
        let p = teach_as(403, "/cli/mail/config/set", json!({"workspace": me.handle, "addressCap": 9, "agentSend": "on"}), &owner_only)
            .expect("taught");
        assert_eq!(action_of(&p), "mail.send-mode", "{p}");
        let p = teach_as(403, "/cli/mail/config/set", json!({"workspace": me.handle, "addressCap": 9, "alwaysBcc": "audit@shop.example"}), &owner_only)
            .expect("a refusal");
        assert_ne!(p["action"], "mail.address-cap", "{p}");
        // Another workspace's owner_only (the dispatcher's mail-manage wall,
        // which names the asker's own toggle): mail.manage, as before.
        let manage = json!({"ok":false,"error":{"code":"owner_only","hint":"x","workspace":{"name":me.handle,"id":me.id}}});
        let p = teach_as(403, "/cli/mail/config/set", json!({"workspace": other.handle, "addressCap": 9}), &manage).expect("taught");
        assert_eq!(action_of(&p), "mail.manage", "{p}");
    }

    /// 0.46.17: CI on K2 Compute. Every `ci_preview_off` (a 409) and the
    /// agent's own `k2 settings set ciPreview true` (either door) end with
    /// the owner's `ci.preview` card; nothing else on the settings route
    /// does, and the 409 reaches the wrapper on both doors.
    #[test]
    fn ci_preview_teaches_the_owners_card() {
        const ASK: &str = "k2 permission request ci.preview --why \"<what you need it for>\"";
        let off = json!({"error":{"code":"ci_preview_off","hint":"x"}});
        for (m, path) in [("POST", "/cli/ci/run"), ("GET", "/cli/ci/status"), ("POST", "/cli/compute/seats/request"), ("GET", "/cli/compute/rightsize")] {
            let p = teach_of(409, m, path, &[], None, off.clone()).unwrap_or_else(|| panic!("{path}: not taught"));
            assert_eq!((p["command"].as_str(), p["approvers"].as_str()), (Some(ASK), Some("owner")), "{path}: {p}");
            assert_eq!(p["params"], json!({}), "{p}");
        }
        // Another 409 is no refusal of this card's.
        assert!(teach_of(409, "POST", "/cli/ci/run", &[], None, json!({"error":{"code":"ci_not_a_repo"}})).is_none());
        // The wrapper lets a 409 through only where a row answers on it.
        assert!(walled_status(409, None) && walled_status(409, Some("/cli/ci/run")));
        assert!(!walled_status(409, Some("/v1/sandboxes")) && !walled_status(500, None));

        // The settings route: ciPreview true asks; false, another key, or
        // both keys at once with ciPreview false stay "It can't be requested".
        for refusal in [json!({"error":{"code":"owner_only","hint":"x"}}), json!({"error":"invalid or missing token"})] {
            let p = teach_of(403, "POST", "/cli/settings/update", &[], Some(json!({"ciPreview": true})), refusal.clone()).expect("taught");
            assert_eq!(p["command"], ASK, "{refusal}: {p}");
            for body in [json!({"ciPreview": false}), json!({"computePreview": true}), json!({"ciPreview": false, "computePreview": true})] {
                let p = teach_of(403, "POST", "/cli/settings/update", &[], Some(body.clone()), refusal.clone()).expect("a refusal");
                assert_eq!(p["requestable"], false, "{body}: {p}");
            }
        }
        // The TCP refusal's body is the code the row matches on.
        assert_eq!(serde_json::from_str::<Value>(SETTINGS_TCP_REFUSAL_BODY).expect("json")["error"], SETTINGS_TCP_REFUSAL);
        // The cell socket, with the gate off for this workspace: today's text.
        let empty = HashMap::new();
        assert!(cell_gated_body("POST", "/cli/settings/update", &empty, br#"{"ciPreview":true}"#, "no-such-workspace").is_none());
    }

    /// The Ask line with the command's name passed in: another brand's
    /// name comes out whole, the rest of the line as before. Pinned as
    /// literals; the tests above pin the default brand's lines.
    #[test]
    fn the_ask_line_takes_the_command_name_as_a_value() {
        let mut params = Map::new();
        params.insert("to".to_string(), json!("pog-sales"));
        assert_eq!(
            ask_command_named("xy", "connections.add", &params),
            "xy permission request connections.add to=pog-sales --why \"<what you need it for>\""
        );

        // A value that needs quoting is quoted the same way.
        let mut params = Map::new();
        params.insert("to".to_string(), json!("a b'c"));
        assert_eq!(
            ask_command_named("xy", "connections.add", &params),
            "xy permission request connections.add to='a b'\\''c' --why \"<what you need it for>\""
        );
    }

    /// The Ask line's tail names no command: the CLI puts its own name in
    /// front. Pinned as literals with no brand in them.
    #[test]
    fn the_ask_tail_is_the_line_after_the_name() {
        let mut params = Map::new();
        params.insert("to".to_string(), json!("a b'c"));
        assert_eq!(ask_tail("connections.add", &params), "permission request connections.add to='a b'\\''c' --why \"<what you need it for>\"");
        assert_eq!(ask_tail("compute.use", &Map::new()), "permission request compute.use --why \"<what you need it for>\"");
    }

    /// A refusal's `permission` object carries `tail` beside `command`, and
    /// `command` is this build's name, a space and the tail, for every
    /// action the walls map names.
    #[test]
    fn every_permission_object_carries_the_tail() {
        let p = teach_of(403, "POST", "/cli/connections", &[("action", "add"), ("target", "pog-sales")], None, json!({"error":{"code":"agents_create_connections_disabled"}})).unwrap();
        assert_eq!(p["tail"], "permission request connections.add to=pog-sales --why \"<what you need it for>\"", "{p}");
        let mut asks = 0;
        for row in ROWS {
            let Target::Ask { action, .. } = row.target else { continue };
            let mut params = Map::new();
            for (k, v) in row.fixed {
                params.insert((*k).to_string(), json!(v));
            }
            let p = to_value(PermissionRefusal::requestable(action, params.clone(), "owner", ask_tail(action, &params)));
            let tail = p["tail"].as_str().expect("tail");
            assert!(tail.starts_with(&format!("permission request {action}")), "{p}");
            assert_eq!(p["command"].as_str(), Some(format!("{CLI_NAME} {tail}").as_str()), "{p}");
            assert_eq!(p["command"].as_str(), Some(ask_command(action, &params).as_str()), "{p}");
            asks += 1;
        }
        assert!(asks > 50, "the walls map's actions: {asks}");
    }

    /// A command name in `text` other than this build's: one of `names`
    /// with no letter, digit, `_`, `.`, `/` or `-` before it, then a space
    /// and a lower-case letter. The brand gate's "reads as a command"
    /// (scripts/garden/cli-name-flip.py) is the model, written small.
    fn other_command<'a>(text: &str, names: &[&'a str]) -> Option<&'a str> {
        for name in names {
            for (i, _) in text.match_indices(name) {
                if text[..i].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')) {
                    continue;
                }
                let mut after = text[i + name.len()..].chars();
                if after.next() == Some(' ') && after.next().is_some_and(|c| c.is_ascii_lowercase()) {
                    return Some(name);
                }
            }
        }
        None
    }

    fn strings_of<'v>(v: &'v Value, out: &mut Vec<&'v str>) {
        match v {
            Value::String(s) => out.push(s),
            Value::Array(a) => a.iter().for_each(|x| strings_of(x, out)),
            Value::Object(m) => m.values().for_each(|x| strings_of(x, out)),
            _ => {}
        }
    }

    /// The reader the brand-free test below uses, on literals.
    #[test]
    fn brand_free_reader_sees_a_command_and_nothing_else() {
        let names = ["k5", "xy"];
        assert_eq!(other_command("ask with: k5 permission request a.b", &names), Some("k5"));
        assert_eq!(other_command("(xy app list)", &names), Some("xy"));
        assert_eq!(other_command("`k5 msg <workspace>`", &names), Some("k5"));
        for text in ["the K5 Space", "sudo k5-node admin", "proxy server", "k5so app", "a.k5 x", "k5", "k5 <id>", "xy App", "k2 app list"] {
            assert_eq!(other_command(text, &names), None, "{text}");
        }
    }

    /// The second judge: a test that names no brand. Every refusal row
    /// (what happened, the fix, its explain line, its text), the
    /// registry's index and study page, every wall's constant text, the
    /// Ask line of every action the walls map names, and every card's
    /// constant text hold no command name but this build's own. Under the
    /// default build it proves little; built with another brand
    /// (K2_BRAND_DIR=brand/xy) it shows this text moved with the brand.
    #[test]
    fn brand_free_permission_and_refusal_text() {
        use k2_core::brand::CLI_ALIASES;
        use k2_core::refusals;

        // Every command name a brand has, less this build's name and aliases.
        let others: Vec<&str> = ["k2", "k5", "xy"].into_iter().filter(|n| *n != CLI_NAME && !CLI_ALIASES.contains(n)).collect();
        assert_eq!(others.len(), 2, "this build's name is one of the known ones: {others:?}");

        let mut texts: Vec<(String, String)> = Vec::new();
        for r in refusals::all() {
            texts.push((format!("refusal {}: what happened", r.id), r.what.to_string()));
            texts.push((format!("refusal {}: the fix", r.id), r.fix.to_string()));
            texts.push((format!("refusal {}: explain", r.id), r.explain_cmd()));
            texts.push((format!("refusal {}: text", r.id), refusals::row_text(r)));
        }
        assert!(texts.len() > 400, "the registry's rows: {}", texts.len() / 4);
        texts.push(("the refusal index".to_string(), refusals::list_text()));
        texts.push(("the refusal study page".to_string(), refusals::study_page()));

        let mut asks = 0;
        for row in ROWS {
            match row.target {
                Target::Ask { action, .. } => {
                    let mut params = Map::new();
                    for (k, v) in row.fixed {
                        params.insert((*k).to_string(), json!(v));
                    }
                    for (k, _) in row.params {
                        params.entry((*k).to_string()).or_insert(Value::Null);
                    }
                    let cmd = ask_command(action, &params);
                    assert!(
                        cmd.contains(&format!(" permission request {action}")) && cmd.ends_with(" --why \"<what you need it for>\""),
                        "{cmd}"
                    );
                    texts.push((format!("wall {} {:?}: the Ask line", row.prefix, row.method), cmd));
                    asks += 1;
                }
                Target::Cannot { r#where } => texts.push((format!("wall {} {:?}: where", row.prefix, row.method), r#where.to_string())),
            }
        }
        assert!(asks > 50, "the walls map's actions: {asks}");
        texts.push(("the default where".to_string(), WHERE_DEFAULT.to_string()));
        for (code, hint) in MACHINE_HINTS {
            texts.push((format!("machine hint {code}"), (*hint).to_string()));
        }

        let cards = crate::permission::actions::all();
        assert!(cards.len() > 50, "the card registry: {}", cards.len());
        for r in cards {
            let spec = serde_json::to_value(r.spec).expect("a card's spec is JSON");
            let mut all = Vec::new();
            strings_of(&spec, &mut all);
            for s in all {
                texts.push((format!("card {}", r.spec.id), s.to_string()));
            }
        }
        for n in crate::permission::actions::not_cardable() {
            texts.push((format!("not a card {}: where", n.id), n.r#where.to_string()));
        }

        let found: Vec<String> =
            texts.iter().filter_map(|(at, text)| other_command(text, &others).map(|name| format!("{at} ({name}): {text}"))).collect();
        assert!(found.is_empty(), "text that names another command than this build's:\n{}", found.join("\n"));
    }
}
