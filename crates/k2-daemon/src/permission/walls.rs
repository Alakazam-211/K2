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
}

const fn ask(action: &'static str, floor: Floor) -> Target {
    Target::Ask { action, floor }
}

const fn code(status: u16, code: &'static str) -> WallMatch {
    WallMatch::Code { status, code }
}

const ANY: WallMethod = WallMethod::Any;
const POST: WallMethod = WallMethod::Post;

const fn row(target: Target, matches: WallMatch, prefix: &'static str, method: WallMethod) -> Row {
    Row { target, matches, prefix, method, when: &[], needs: None, params: &[], fixed: &[], body_has: None, optional: &[] }
}

/// "a person does this in Settings" for never / dropped walls a broad row
/// would otherwise claim.
const IN_SETTINGS: &str = "a person with the owner role does this, in Settings or from a terminal outside K2";

/// The box's firewall (0.46.6): the owner or an Admin, never an agent.
const FIREWALL_WHERE: &str =
    "the owner or an admin does this, in Settings → Network → Custom Domains → Firewall or with k2 firewall from a terminal outside K2";

/// K2's front on :443 (`k2 front`): the owner, an Admin or a shell tab.
const FRONT_WHERE: &str = "the owner or an admin does this, with k2 front over SSH or in a K2 shell tab";

/// The v1 card-able actions (PRD §5.2): the 17 Agent-tab switches, the 5
/// held-request actions, `garden.keep-mine` (K2-presented, no wall) and
/// the `to=off` twins (asked only, no wall), plus Garden access's three
/// (prd-garden-access-v1 G16, listed in every switch state): `garden.access`
/// and `garden.delete` (walled; K2 presents them too) and the
/// `garden.access-off` twin (asked only, no wall). Every [`Target::Ask`]
/// names one of them, never `garden.keep-mine` or a twin.
pub(crate) const V1_ACTIONS: [&str; 39] = [
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
    // 0.46.6 (Rosson 2026-10-28): let the agent read one of its own
    // workspace's secrets as a file or env (a handed-over or person-given
    // secret it may not widen itself).
    "vault.delivery",
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

/// The walls map (PRD §5.2 walls column, Appendix A, checked against the
/// off-goldens' captured refusals).
pub(crate) const ROWS: &[Row] = &[
    // ── compute ────────────────────────────────────────────────────────
    row(ask("compute.use", Floor::Admin), code(403, "compute_off"), "/cli/", ANY),
    // compute.grant and compute.agents are covered by compute.use (turning it
    // on grants every active node).
    row(ask("compute.use", Floor::Admin), code(403, "not_granted"), "/cli/compute/", ANY),
    row(ask("compute.use", Floor::Admin), code(403, "owner_only"), "/cli/compute/grants", ANY),
    row(ask("compute.use", Floor::Admin), code(403, "owner_only"), "/cli/compute/agents", ANY),
    row(ask("compute.operate", Floor::Admin), code(403, "operate_off"), "/cli/", ANY),
    // `owner_or_manager` routes: "Allow managing compute nodes" lets the
    // agent through. Owner both on and off.
    row(ask("compute.manage", Floor::Owner), code(403, "owner_only"), "/cli/compute/", ANY),
    // `need_owner` only: no switch admits an agent.
    row(Target::Cannot { r#where: IN_SETTINGS }, code(403, "owner_only"), "/cli/compute/nodes/remove", ANY),
    row(Target::Cannot { r#where: IN_SETTINGS }, code(403, "owner_only"), "/cli/compute/local/", ANY),
    Row {
        params: &[("request", &[ParamFrom::Body("/request"), ParamFrom::Body("/id"), ParamFrom::Query("request")])],
        ..row(ask("compute.admin-approve", Floor::Owner), code(403, "owner_only"), "/cli/compute/admin/approve", POST)
    },
    // Job tiers (0.46.5): `--tier ci` / `--gate` without a grant that
    // allows CI, and a compute manager setting `allowCi` on a grant.
    Row { params: &[("node", &[ParamFrom::Body("/node")])], ..row(ask("compute.ci", Floor::Admin), code(403, "ci_not_allowed"), "/cli/compute/", ANY) },
    // ── Sew (0.46.6) ───────────────────────────────────────────────────
    // An agent's own call to the owner handoff: agents never become Sew's
    // install owner; the owner opens it for themselves on a card.
    row(ask("sew.owner-handoff", Floor::Owner), code(403, "owner_only"), "/cli/sew/owner-handoff", POST),
    // An agent's (or a K2 terminal's) emergency rollback: a person rolls
    // Sew back across a schema change; the owner answers on a card.
    row(ask("sew.rollback-data-loss", Floor::Owner), code(403, "rollback_needs_person"), "/cli/sew/server/rollback", POST),
    // ── agents, projects ───────────────────────────────────────────────
    row(ask("agents.manage", Floor::Admin), code(403, "gated"), "/cli/sidecar/", ANY),
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
    // Every App hold (switch 2 off, a core not ticked, new caps, a --cmd
    // service…) already made a held request: asking attaches a card to that
    // same id (§3.2, the agent sheet's HELD rule).
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(
            ask("apps.approve", Floor::Owner),
            WallMatch::Held { status: 202, field: "status", value: "held" },
            "/cli/app/",
            ANY,
        )
    },
    // An agent's held `k2 publish run` (a `publish_run` App request, 0.46.6
    // NSI papercut 12: it printed no Ask-with line).
    Row {
        params: &[("request", &[ParamFrom::Response("/request/id")])],
        ..row(
            ask("apps.approve", Floor::Owner),
            WallMatch::Held { status: 202, field: "status", value: "held" },
            "/cli/publish/run",
            POST,
        )
    },
    Row {
        params: &[("request", &[ParamFrom::Body("/request"), ParamFrom::Body("/id"), ParamFrom::Query("id")])],
        ..row(ask("apps.approve", Floor::Owner), code(403, "owner_only"), "/cli/app/approve", POST)
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
    row(ask("vault.manage", Floor::Admin), code(403, "vault_manage_required"), "/cli/", ANY),
    // `k2 vault delivery NAME file|env` on a secret the agent may not widen
    // (2026-10-28): the refusal names the secret and the delivery.
    Row {
        needs: Some("/error/delivery"),
        params: &[("secret", &[ParamFrom::Response("/error/secret")]), ("delivery", &[ParamFrom::Response("/error/delivery")])],
        ..row(ask("vault.delivery", Floor::Admin), code(403, "vault_owner_confirm_required"), "/cli/vault/policy", POST)
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

/// The `--why` placeholder in every Ask-with command.
pub(crate) const WHY_PLACEHOLDER: &str = "<what you need it for>";

/// Generic `where` for a passport refusal no row names.
pub(crate) const WHERE_DEFAULT: &str = "a person does this, in Settings or from a terminal outside K2";

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

fn row_matches(r: &Row, req: &Refused<'_>, code: Option<&str>) -> bool {
    let hit = match r.matches {
        WallMatch::Code { status, code: c } => req.status == status && code == Some(c),
        WallMatch::Held { status, field, value } => {
            req.status == status && req.response.get(field).and_then(Value::as_str) == Some(value)
        }
    };
    hit && req.path.starts_with(r.prefix)
        && method_ok(r.method, req.method)
        && r.when.iter().all(|(k, v)| req.params.get(*k).map(String::as_str) == Some(*v))
        && r.needs.is_none_or(|p| req.response.pointer(p).is_some())
        && r.body_has.is_none_or(|(p, present)| body_value(req.body, p) == present)
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

/// The exact command an agent runs to ask (PRD §3.1).
pub(crate) fn ask_command(action: &str, params: &Map<String, Value>) -> String {
    let mut cmd = format!("k2 permission request {action}");
    for (k, v) in params {
        let v = v.as_str().map(shell_word).unwrap_or_else(|| format!("<{k}>"));
        cmd.push_str(&format!(" {k}={v}"));
    }
    cmd.push_str(&format!(" --why \"{WHY_PLACEHOLDER}\""));
    cmd
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
                let command = ask_command(action, &params);
                to_value(PermissionRefusal::Requestable {
                    action: action.to_string(),
                    params,
                    approvers: floor.label().to_string(),
                    command,
                })
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
    let candidate = matches!(code, 401 | 403)
        || ROWS
            .iter()
            .any(|r| matches!(r.matches, WallMatch::Held { status: s, .. } if s == code) && path.starts_with(r.prefix));
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
    let req = Refused { status: code, method, path, params, body: req_body, response: &response };
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
    if code < 200 || (code >= 300 && !matches!(code, 401 | 403)) {
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
pub(crate) const CELL_GATED_HINT: &str = "a person does this (the owner, or an admin where Settings allows), \
     not an agent: this route isn't open to agents. Use Settings or a terminal outside K2.";

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
    let req = Refused { status: 403, method, path, params, body: json.as_ref(), response: &empty };
    let row = ROWS
        .iter()
        .filter(|r| {
            r.prefix != "/cli/"
                && r.needs.is_none()
                && matches!(r.matches, WallMatch::Code { code: "owner_only" | "person_only", .. })
                && path.starts_with(r.prefix)
                && method_ok(r.method, method)
                && r.when.iter().all(|(k, v)| params.get(*k).map(String::as_str) == Some(*v))
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
        Refused { status, method, path, params, body, response }
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
            (403, "POST", "/cli/compute/grants/set", vec![], None, json!({"error":{"code":"owner_only"}}), "compute.use"),
            (403, "POST", "/cli/sidecar/new", vec![], None, json!({"error":{"code":"gated"}}), "agents.manage"),
            (403, "POST", "/cli/project-group/create", vec![], None, json!({"error":{"code":"agent_permission_off"},"ok":false}), "projects.manage"),
            (403, "POST", "/cli/app/exception", vec![], None, json!({"error":{"code":"apps_manage_off"}}), "apps.build"),
            (202, "POST", "/cli/app/build", vec![], None, json!({"status":"held","request":{"id":"w_1"}}), "apps.approve"),
            (202, "POST", "/cli/app/deploy", vec![], None, json!({"status":"held","request":{"id":"w_2"}}), "apps.approve"),
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
            (403, "POST", "/cli/zen/widget/sign", vec![], Some(json!({"widget":"board"})), json!({"ok":false,"error":"sign_needs_person","widget":"board","version":"ab".repeat(32),"card":null}), "zen.sign"),
            // Job tiers (prd-compute-job-tiers-v1 T13).
            (403, "POST", "/cli/compute/run", vec![], Some(json!({"node":"k2n4","tier":"ci"})), json!({"error":{"code":"ci_not_allowed","hint":"x"}}), "compute.ci"),
            // Mobile dev (0.46.6): `k2 sim boot` with "Allow mobile devices" off.
            (403, "POST", "/cli/sim/boot", vec![], Some(json!({})), json!({"error":{"code":"sim_switch_off","hint":"x"},"code":"sim_switch_off"}), "sim.use"),
            // 0.46.6: an agent moving its own pinned chat (both doors).
            (403, "POST", "/cli/agent/harness", vec![], Some(json!({"workspace":"sales","harness":"Claude"})), json!({"error":{"code":"harness_needs_person","harness":"claude"},"switched":false}), "agent.harness-switch"),
            (403, "POST", "/cli/workspace/swap-canonical", vec![], Some(json!({"project":"/x","provider":"claude"})), json!({"error":{"code":"harness_needs_person","harness":"claude"},"swapped":false}), "agent.harness-switch"),
            // 0.46.6: `k2 vault delivery NAME env` on a secret the agent can't widen.
            (403, "POST", "/cli/vault/policy", vec![], Some(json!({"name":"K2N12_ENROLL_CODE","delivery":"env"})), json!({"error":{"code":"vault_owner_confirm_required","hint":"x","secret":"K2N12_ENROLL_CODE","delivery":"env"}}), "vault.delivery"),
            // 0.46.6: an agent pausing / resuming another workspace's heartbeat.
            (403, "POST", "/cli/heartbeat/enable", vec![("name", "triage"), ("enabled", "false"), ("workspace", "companion")], None, json!({"error":{"code":"other_workspace","workspace":"companion","heartbeat":"triage","enabled":false}}), "heartbeat.pause-other"),
            (403, "POST", "/cli/heartbeat/enable", vec![("name", "triage"), ("enabled", "true"), ("workspace", "companion")], None, json!({"error":{"code":"other_workspace","workspace":"companion","heartbeat":"triage","enabled":true}}), "heartbeat.resume-other"),
            // 0.46.6: an agent's own reset (the owner's route) and a password
            // it may not set (a box login).
            (403, "POST", "/cli/skin/users/reset-link", vec![], Some(json!({"username":"adam","app":"agents.nsi","code":true})), json!({"ok":false,"error":{"code":"owner_only","hint":"x"}}), "apps.user-password-reset"),
            (403, "POST", "/cli/skin/users/password", vec![], Some(json!({"username":"adam","password":"hunter2-not-real"})), json!({"ok":false,"error":{"code":"owner_only","hint":"'adam' has a box login"}}), "apps.user-password-reset"),
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
        // (K2-presented) — PRD §5.2 walls column.
        for a in V1_ACTIONS {
            if matches!(a, "apps.manage-full" | "apps.core" | "garden.keep-mine" | "garden.access-off" | "zen.sign-off") {
                continue;
            }
            assert!(asked.contains(&a), "{a}: no captured wall teaches it");
        }
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
        assert_eq!(p["approvers"], "owner");
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
        let p = teach_of(403, "POST", "/cli/skin/users/reset-link", &[], Some(json!({"username":"adam","app":"agents.nsi","code":true})), owner_only()).unwrap();
        assert_eq!(action_of(&p), "apps.user-password-reset", "{p}");
        assert_eq!(p["params"], json!({"user":"adam","app":"agents.nsi","mode":"link"}), "{p}");
        assert_eq!(p["approvers"], super::super::actions::password_reset::FLOOR.label());
        let cmd = p["command"].as_str().unwrap();
        assert!(cmd.starts_with("k2 permission request apps.user-password-reset ") && cmd.contains(" user=adam") && cmd.contains(" app=agents.nsi") && cmd.contains(" mode=link"), "{cmd}");
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
                        ParamFrom::Body(k) | ParamFrom::Query(k) | ParamFrom::Response(k) => *k,
                    };
                    assert!(!key.to_ascii_lowercase().contains("password"), "{key}");
                }
            }
            assert_eq!(r.target, ask("apps.user-password-reset", super::super::actions::password_reset::FLOOR));
        }
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
            ("GET", "/cli/front/status", FRONT_WHERE),
            ("POST", "/cli/front/disable", FRONT_WHERE),
            ("POST", "/cli/front/passthrough/add", FIREWALL_WHERE),
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
        assert_eq!(V1_ACTIONS.len(), 39);
        // No two rows can claim the same refusal: the longest prefix must
        // decide alone.
        for (i, a) in ROWS.iter().enumerate() {
            for b in &ROWS[i + 1..] {
                let same = a.matches == b.matches
                    && a.prefix == b.prefix
                    && a.method == b.method
                    && a.when == b.when
                    && a.needs == b.needs
                    && a.body_has == b.body_has;
                assert!(!same, "two rows claim {:?} on {}", a.matches, a.prefix);
            }
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
}
