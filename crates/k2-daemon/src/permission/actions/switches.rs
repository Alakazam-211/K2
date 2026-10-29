//! E1 — the Agent tab's switches (PRD rev 3.1 §5.2): `compute.use`,
//! `compute.operate`, `compute.manage`, `agents.manage`, `projects.manage`,
//! `apps.build`, `apps.manage-full`, `apps.core`, `chat.manage`,
//! `vault.manage`, `gardens.manage`, `dns.manage`, `mail.manage`,
//! `mail.send-mode` and `db.agent-access` (the two connection switches are in
//! `connections.rs`), and each one's `to=off` twin, `<id>-off`.
//!
//! **Table-driven.** One [`SWITCHES`] row per Agent-tab switch: the loosen
//! spec, its tighten twin, and the [`Writer`] that applies it. `check` reads
//! the current Settings state; `apply` calls **the Settings route's own
//! entry** with the answerer as the caller, so the route's floor (and the
//! writer's own owner-only raise rules) run again at the click (§6.4, C15):
//! - the 10 `/cli/agent-access/set` toggles → `sidecar_routes::access_entry`;
//! - `dns.manage` → `misc_routes::dns_manage_entry` (`/cli/dns-manage`);
//! - `mail.manage` → `mail_routes::mail_manage_entry` (`/cli/mail-manage`);
//! - `mail.send-mode` → `mail_routes::mail_send_mode_entry`
//!   (`/cli/mail/config/set {agentSend}`, what Settings writes);
//! - `db.agent-access` → `workspace_routes::agent_switch_entry`
//!   (`/cli/workspace/set {db_agent_access}`, what Settings writes).
//!
//! **The `to=off` twins.** An agent asks for `<id>-off` (a tightening: no
//! wall, asked only), and the "Turn off again" undo of an approved switch
//! runs the same twin (`Undo::Twin`). The twin's floor is Admin (an Admin
//! may always lower), except `compute.manage-off`: that switch is the
//! owner's both ways. Twins carry the switch's own values (`apps.core-off
//! core=`); a choice value (`mode=`, `level=`) is optional and ignored.
//!
//! **Floors** are the code's (Appendix A): Admin, except `compute.manage`
//! (Owner both ways) and the three Apps raises (`apps.build`,
//! `apps.manage-full`, `apps.core`: the owner raises).
//!
//! Vault and DNS stay in (PRD §14 Q11, K2's recommendation; Rosson may
//! strike them): striking one is deleting its row here.

use k2_core::connect_users::Role;
use k2_core::permission::catalog::{
    ActionSpec, Area, Direction, Expiry, Floor, Grant, NotCardable, NotCardableSpec, ParamKind, ParamSpec,
    Proof, SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use serde_json::{json, Value};

use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::cli_response::CliResponse;
use crate::sidecar_routes::Caller;

// ── shared helpers for the E1 entries (also used by connections.rs) ──

/// The Settings deep link and path every Agent-tab switch shares.
pub(crate) const AGENT_TAB: SettingsRef =
    SettingsRef { page: "workspaces/{ws}/agent", path: "Settings → Workspaces → {ws} → Agent" };

/// The route caller an answerer is: the owner token, or a Connect login.
pub(crate) fn as_caller(who: &Answerer) -> Caller {
    match who {
        Answerer::OwnerToken => Caller::Owner,
        Answerer::Login { user, .. } => Caller::Login { username: user.clone() },
    }
}

/// The route's own login floor (`route_policy`'s row for `path`, as
/// `role_gate` applies it on the Settings door), checked again at the
/// click with the login's **current** role. The owner token passes, as on
/// the Settings door. A refusal is the same body the Settings door gives
/// (`role_required`), so card and Settings stay one door (§6.4).
pub(crate) fn route_floor(who: &Answerer, path: &str) -> Result<Caller, CliResponse> {
    let user = match who {
        Answerer::OwnerToken => return Ok(Caller::Owner),
        Answerer::Login { user, .. } => user,
    };
    let Some(role) = k2_core::connect_users::role_for_user(user) else {
        return Err(CliResponse::forbidden());
    };
    let Some(floor) = crate::routes::route_policy::lookup(path).and_then(|r| r.post.or(r.get)) else {
        // Every entry names a classified route (unit-tested); fail closed.
        return Err(CliResponse::forbidden());
    };
    if floor.admits(role) {
        return Ok(Caller::Login { username: user.clone() });
    }
    Err(role_required(floor, role))
}

/// `route_policy`'s `role_required` body, byte for byte.
fn role_required(floor: crate::routes::route_policy::Floor, role: Role) -> CliResponse {
    let message = match floor {
        crate::routes::route_policy::Floor::NoLogin => {
            "No login can use this route. It takes the owner token on the server.".to_string()
        }
        other => format!("This needs an {} login on this server. You are signed in as {}.", other.as_wire(), role.as_wire()),
    };
    CliResponse {
        status: "403 Forbidden",
        content_type: "application/json",
        body: json!({
            "error": "role_required",
            "required": floor.as_wire(),
            "role": role.as_wire(),
            "message": message,
        })
        .to_string(),
    }
}

/// The owner token or an Owner/Admin login (the dispatcher's
/// `token_is_owner_or_admin`), with the login's current role.
pub(crate) fn owner_or_admin(who: &Answerer) -> bool {
    match who {
        Answerer::OwnerToken => true,
        Answerer::Login { user, .. } => {
            k2_core::connect_users::role_for_user(user).is_some_and(k2_core::connect_users::can_manage_users)
        }
    }
}

/// A route entry's answer as the card's `Applied`: any 2xx is done
/// (`message` is the card's line; empty = the core's default), anything
/// else is the route's refusal (its code and words).
pub(crate) fn applied(r: &CliResponse, message: impl Into<String>) -> Applied {
    if r.status.starts_with('2') {
        return Applied::Done { message: message.into() };
    }
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    let code = match v.get("error") {
        Some(Value::Object(e)) => e.get("code").and_then(Value::as_str),
        Some(Value::String(s)) => Some(s.as_str()),
        _ => v.get("code").and_then(Value::as_str),
    }
    .unwrap_or("refused")
    .to_string();
    let words = v
        .pointer("/error/hint")
        .or_else(|| v.get("message"))
        .or_else(|| v.get("hint"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("{} {}", r.status, r.body));
    Applied::Refused { code, message: words }
}

/// The workspace's path (path-keyed writers), or None when it is gone.
pub(crate) fn ws_path(workspace_id: &str) -> Option<String> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    k2_core::db::schema::Project::get(&conn, workspace_id).ok().map(|p| p.path)
}

/// One string value of the request (`core=`, `mode=`, `to=`).
pub(crate) fn param<'a>(ctx: &'a Ctx, key: &str) -> Option<&'a str> {
    ctx.params.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

pub(crate) const GONE: &str = "the workspace is gone";

/// The `Check` of a switch whose state is known: already in the asked-for
/// state → `AlreadyDone`.
pub(crate) fn check_state(want_on: bool, is_on: bool) -> Check {
    if want_on == is_on {
        Check::AlreadyDone
    } else {
        Check::Needed
    }
}

/// The card line for an applied tightening (the core's default line is
/// for loosening).
pub(crate) fn off_message(ctx: &Ctx) -> String {
    format!("Turned off. Turn it back on in Settings → Workspaces → {} → Agent.", ctx.workspace_handle)
}

// ── the table ───────────────────────────────────────────────────────

/// How a switch is written: the Settings route entry `apply` calls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Writer {
    /// `POST /cli/agent-access/set {toggle, workspace, value}`.
    Access(&'static str),
    /// `POST /cli/agent-access/set {toggle:"app_core", workspace, core, value}`.
    AppCore,
    /// `POST /cli/dns-manage {project, enable}`.
    Dns,
    /// `POST /cli/mail-manage {project, enable}`.
    MailManage,
    /// `POST /cli/mail/config/set {workspace, agentSend}`.
    MailSendMode,
    /// `POST /cli/workspace/set {project, fields:{db_agent_access}}`.
    DbAccess,
}

/// One Agent-tab switch: the loosen spec, its tighten twin, its writer.
pub(crate) struct Switch {
    pub on: ActionSpec,
    pub off: ActionSpec,
    pub writer: Writer,
}

const DURATION_ON: &str = "until someone turns it off";
const DURATION_OFF: &str = "until someone turns it on again";
const RETRY: &str = "run your command again";

const fn code_wall(code: &'static str, prefix: &'static str) -> Wall {
    Wall { matches: WallMatch::Code { status: 403, code }, route_prefix: prefix, method: WallMethod::Any, params: &[] }
}

#[allow(clippy::too_many_arguments)]
const fn on_spec(
    id: &'static str,
    off_id: &'static str,
    floor: Floor,
    params: &'static [ParamSpec],
    setting: &'static str,
    what: &'static str,
    consequences: &'static str,
    walls: &'static [Wall],
    coverage: WallCoverage,
) -> ActionSpec {
    ActionSpec {
        id,
        area: Area::Switches,
        params,
        floor,
        direction: Direction::Loosen,
        grant: Grant::Standing,
        proof: Proof::Click,
        expiry: Expiry::STANDARD,
        words: Words { setting, what_changes: what, consequences, scope: "{ws}", duration: DURATION_ON, retry: RETRY },
        undo: Undo::Twin { action: off_id },
        settings: AGENT_TAB,
        walls,
        coverage,
        machine_hints: &[],
    }
}

pub(crate) const fn off_spec(
    id: &'static str,
    floor: Floor,
    params: &'static [ParamSpec],
    setting: &'static str,
    what: &'static str,
    consequences: &'static str,
) -> ActionSpec {
    ActionSpec {
        id,
        area: Area::Switches,
        params,
        floor,
        direction: Direction::Tighten,
        grant: Grant::Standing,
        proof: Proof::Click,
        expiry: Expiry::STANDARD,
        words: Words { setting, what_changes: what, consequences, scope: "{ws}", duration: DURATION_OFF, retry: "" },
        undo: Undo::None,
        settings: AGENT_TAB,
        walls: &[],
        coverage: WallCoverage::None,
        machine_hints: &[],
    }
}

const CORE: &[ParamSpec] = &[ParamSpec { key: "core", kind: ParamKind::Core, required: true, list: false, choices: &[] }];
const MODE: &[ParamSpec] =
    &[ParamSpec { key: "mode", kind: ParamKind::Choice, required: true, list: false, choices: &["approval", "on"] }];
const MODE_OFF: &[ParamSpec] =
    &[ParamSpec { key: "mode", kind: ParamKind::Choice, required: false, list: false, choices: &["approval", "on"] }];
const LEVEL: &[ParamSpec] =
    &[ParamSpec { key: "level", kind: ParamKind::Choice, required: false, list: false, choices: &["write"] }];

/// The Agent tab's switches, in Settings order. Index = the const generic
/// of [`check_on`] / [`apply_on`] and their twins.
pub(crate) static SWITCHES: [Switch; 17] = [
    Switch {
        on: on_spec(
            "compute.use",
            "compute.use-off",
            Floor::Admin,
            &[],
            "Allow compute nodes",
            "Let {ws} run jobs on this server's compute nodes",
            "It may use every machine switched on here, with default limits, and machines added later. Jobs see only the code {ws} sends.",
            &[code_wall("compute_off", "/cli/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "compute.use-off",
            Floor::Admin,
            &[],
            "Allow compute nodes",
            "Stop {ws} running jobs on this server's compute nodes",
            "Its grants on every machine are removed. Queued and running jobs keep going; new runs are refused.",
        ),
        writer: Writer::Access("compute"),
    },
    Switch {
        on: on_spec(
            "compute.operate",
            "compute.operate-off",
            Floor::Admin,
            &[],
            "Allow this agent to operate compute nodes",
            "Let {ws} pause, drain, resume, narrow, update and restart compute nodes",
            "Root steps on a machine still wait for the owner every time.",
            &[code_wall("operate_off", "/cli/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "compute.operate-off",
            Floor::Admin,
            &[],
            "Allow this agent to operate compute nodes",
            "Stop {ws} operating compute nodes",
            "Pausing, draining and restarting machines goes back to the owner.",
        ),
        writer: Writer::Access("compute_operate"),
    },
    Switch {
        on: on_spec(
            "compute.manage",
            "compute.manage-off",
            Floor::Owner,
            &[],
            "Allow managing compute nodes",
            "Let {ws} add, confirm, rename, pause and retire compute machines",
            "These are the owner's steps on this server, so only the owner allows them.",
            &[code_wall("owner_only", "/cli/compute/")],
            WallCoverage::PrefixOnly,
        ),
        off: off_spec(
            "compute.manage-off",
            Floor::Owner,
            &[],
            "Allow managing compute nodes",
            "Stop {ws} managing compute machines",
            "Adding, confirming and retiring machines goes back to the owner.",
        ),
        writer: Writer::Access("compute_manage"),
    },
    Switch {
        on: on_spec(
            "agents.manage",
            "agents.manage-off",
            Floor::Admin,
            &[],
            "Allow hiring and managing agents",
            "Let {ws} open and stop sidecar agents",
            "Sidecars work in {ws}'s workspace with its settings. It doesn't cover hiring new agents.",
            &[code_wall("gated", "/cli/sidecar/"), code_wall("gated", "/cli/sessions/open")],
            WallCoverage::PrefixOnly,
        ),
        off: off_spec(
            "agents.manage-off",
            Floor::Admin,
            &[],
            "Allow hiring and managing agents",
            "Stop {ws} opening sidecar agents",
            "Sidecars already open keep running until someone stops them.",
        ),
        writer: Writer::Access("agents_manage"),
    },
    Switch {
        on: on_spec(
            "projects.manage",
            "projects.manage-off",
            Floor::Admin,
            &[],
            "Manage K2 projects",
            "Let {ws} create, rename and delete K2 projects and change who is in them",
            "It covers every project {ws} can see.",
            &[code_wall("agent_permission_off", "/cli/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "projects.manage-off",
            Floor::Admin,
            &[],
            "Manage K2 projects",
            "Stop {ws} changing K2 projects",
            "It can still read its projects and use their project chat.",
        ),
        writer: Writer::Access("projects_manage"),
    },
    Switch {
        on: on_spec(
            "apps.build",
            "apps.build-off",
            Floor::Owner,
            &[],
            "Build and run Apps",
            "Let {ws} build and run Apps",
            "It builds, installs, deploys, runs, stops and uninstalls its own Apps inside the default limits, moves its own --cmd services and labels in, and edits their roles and App passes. New caps, open services and bigger budgets still wait for the owner; their users are a separate switch.",
            &[code_wall("apps_manage_off", "/cli/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "apps.build-off",
            Floor::Admin,
            &[],
            "Build and run Apps",
            "Stop {ws} building and running Apps",
            "Its Apps keep running; every change is held for the owner. This also turns off managing them end to end.",
        ),
        writer: Writer::Access("apps_build"),
    },
    Switch {
        // No wall of its own: without switch 2 an agent's move is a held
        // request, which teaches `apps.approve` (lane C's walls map).
        on: on_spec(
            "apps.manage-full",
            "apps.manage-full-off",
            Floor::Owner,
            &[],
            "Manage its Apps end to end",
            "Let {ws} manage its Apps end to end",
            "On its own Apps: budgets and quotas past the defaults, outbound (egress) hosts, box services, App keys and clean mode off apply without waiting for the owner; it resumes an App it suspended itself and sends push. Loosening an App's wall still waits for the owner; users are a separate switch.",
            &[],
            WallCoverage::None,
        ),
        off: off_spec(
            "apps.manage-full-off",
            Floor::Admin,
            &[],
            "Manage its Apps end to end",
            "Stop {ws} managing its Apps end to end",
            "Those changes are held for the owner again.",
        ),
        writer: Writer::Access("apps_manage_full"),
    },
    Switch {
        // "Let this agent manage its Apps' users" (0225, Rosson 2026-10-26):
        // the owner's both ways. Its wall is the skin-manage door's
        // `app_users_off`; a test guest or the guests-rooms grant without
        // it is a held request whose card offers "Always allow" (this).
        on: on_spec(
            "apps.manage-users",
            "apps.manage-users-off",
            Floor::Owner,
            &[],
            "Let this agent manage its Apps' users",
            "Let {ws} manage the users of its own Apps",
            crate::apps::held_cards::SWITCH_COVERS,
            &[code_wall("app_users_off", "/cli/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "apps.manage-users-off",
            Floor::Owner,
            &[],
            "Let this agent manage its Apps' users",
            "Stop {ws} managing its Apps' users",
            "Its Apps' users stay as they are; new ones, passwords and rooms go back to the owner.",
        ),
        writer: Writer::Access("app_users"),
    },
    Switch {
        // A cap outside the ticked cores is a held request (apps.approve).
        on: on_spec(
            "apps.core",
            "apps.core-off",
            Floor::Owner,
            CORE,
            "App cores",
            "Let {ws}'s Apps use the {core} core",
            "Every App {ws} looks after may use {core} without asking the owner first.",
            &[],
            WallCoverage::None,
        ),
        off: off_spec(
            "apps.core-off",
            Floor::Admin,
            CORE,
            "App cores",
            "Stop {ws}'s Apps using the {core} core without asking",
            "New uses of {core} are held for the owner again.",
        ),
        writer: Writer::AppCore,
    },
    Switch {
        on: on_spec(
            "chat.manage",
            "chat.manage-off",
            Floor::Admin,
            &[],
            "Manage Sew",
            "Let {ws} run Sew on this server and manage its communities",
            "It may enable, upgrade, back up and roll back Sew here.",
            &[code_wall("chat_manage_off", "/cli/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "chat.manage-off",
            Floor::Admin,
            &[],
            "Manage Sew",
            "Stop {ws} running Sew",
            "Sew keeps running; managing it goes back to the owner and admins.",
        ),
        writer: Writer::Access("chat_manage"),
    },
    Switch {
        on: on_spec(
            "vault.manage",
            "vault.manage-off",
            Floor::Admin,
            &[],
            "Allow managing server-wide secrets",
            "Let {ws} share server secrets with other workspaces and look after them",
            "Never with itself, and it can't loosen a secret it uses. It never sees a secret's value.",
            &[code_wall("vault_manage_required", "/cli/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "vault.manage-off",
            Floor::Admin,
            &[],
            "Allow managing server-wide secrets",
            "Stop {ws} managing server-wide secrets",
            "Secrets it already shared stay shared until someone changes them.",
        ),
        writer: Writer::Access("vault_manage"),
    },
    Switch {
        on: on_spec(
            "gardens.manage",
            "gardens.manage-off",
            Floor::Admin,
            &[],
            "Manage Gardens and data sources",
            "Let {ws} write Gardens and widgets and approve data sources",
            "Its writes stop waiting as proposals. Locked Gardens stay locked.",
            &[code_wall("person_only", "/cli/sources/"), code_wall("person_only", "/cli/zen/")],
            WallCoverage::PrefixOnly,
        ),
        off: off_spec(
            "gardens.manage-off",
            Floor::Admin,
            &[],
            "Manage Gardens and data sources",
            "Stop {ws} writing Gardens directly",
            "Its Garden and widget changes become proposals again.",
        ),
        writer: Writer::Access("gardens_manage"),
    },
    Switch {
        on: on_spec(
            "dns.manage",
            "dns.manage-off",
            Floor::Admin,
            &[],
            "Allow agents to manage DNS for this workspace",
            "Let {ws} manage DNS records",
            "It may add and change records on this server's domains.",
            &[code_wall("dns_manage_disabled", "/cli/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "dns.manage-off",
            Floor::Admin,
            &[],
            "Allow agents to manage DNS for this workspace",
            "Stop {ws} managing DNS records",
            "Records it made stay. The server-wide DNS switch, when on, still lets it in.",
        ),
        writer: Writer::Dns,
    },
    Switch {
        on: on_spec(
            "mail.manage",
            "mail.manage-off",
            Floor::Admin,
            &[],
            "Allow agents to manage hosted mail on this host",
            "Let {ws} turn hosted mail on and off and add and check its domains",
            "Uninstalling, removing a domain, OAuth and access stay the owner's.",
            &[code_wall("owner_only", "/cli/mail/")],
            WallCoverage::PrefixOnly,
        ),
        off: off_spec(
            "mail.manage-off",
            Floor::Admin,
            &[],
            "Allow agents to manage hosted mail on this host",
            "Stop {ws} managing hosted mail",
            "Hosted mail keeps running; managing it goes back to the owner and admins.",
        ),
        writer: Writer::MailManage,
    },
    Switch {
        on: on_spec(
            "mail.send-mode",
            "mail.send-mode-off",
            Floor::Admin,
            MODE,
            "Let this agent send email",
            "Set {ws}'s email sending to {mode}",
            "With approval, each send waits for an owner or admin. On, it sends freely from its inboxes.",
            &[code_wall("gated", "/cli/mail/")],
            WallCoverage::PrefixOnly,
        ),
        off: off_spec(
            "mail.send-mode-off",
            Floor::Admin,
            MODE_OFF,
            "Let this agent send email",
            "Stop {ws} sending email",
            "Its sends are refused until someone turns sending back on.",
        ),
        writer: Writer::MailSendMode,
    },
    Switch {
        on: on_spec(
            "db.agent-access",
            "db.agent-access-off",
            Floor::Admin,
            LEVEL,
            "Allow this agent to create databases",
            "Let {ws} create new databases",
            "Create only: databases it already uses stay usable either way.",
            &[code_wall("forbidden", "/cli/db/")],
            WallCoverage::PrefixOnly,
        ),
        off: off_spec(
            "db.agent-access-off",
            Floor::Admin,
            LEVEL,
            "Allow this agent to create databases",
            "Stop {ws} creating new databases",
            "Databases it already has stay usable.",
        ),
        writer: Writer::DbAccess,
    },
    // 0.46.6 (prd-mobile-dev-v1 M-A12): "Allow mobile devices" (0233,
    // off by default). The licence stays the box owner's (M-A23).
    Switch {
        on: on_spec(
            "sim.use",
            "sim.use-off",
            Floor::Admin,
            &[],
            "Allow mobile devices",
            "Let {ws} boot and drive an Android emulator or iOS Simulator on this server",
            "It may run one device for its own workspace, within the server's device limits. People can watch and take over at any time.",
            &[code_wall("sim_switch_off", "/cli/sim/")],
            WallCoverage::Code,
        ),
        off: off_spec(
            "sim.use-off",
            Floor::Admin,
            &[],
            "Allow mobile devices",
            "Stop {ws} using mobile devices",
            "Its running device keeps running until it is stopped or idles out; new device commands are refused.",
        ),
        writer: Writer::Access("sim"),
    },
];

// ── state and apply, per writer ─────────────────────────────────────

/// Is the switch on now (for this request's values)? `Err` = the values
/// moved (workspace gone, an unknown core, switch 1 off under switch 2).
fn is_on(sw: &Switch, ctx: &Ctx, want_on: bool) -> Result<bool, String> {
    let ws = ctx.workspace_id.as_str();
    let path = || ws_path(ws).ok_or_else(|| GONE.to_string());
    match sw.writer {
        Writer::Access("compute") => {
            path()?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            Ok(k2_core::compute::store::agents_can_use_compute(&conn, ws))
        }
        Writer::Access("compute_operate") => {
            path()?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            Ok(k2_core::compute::admin::agents_can_operate_compute(&conn, ws))
        }
        Writer::Access("compute_manage") => path().map(|_| k2_core::workspace::settings::agents_can_manage_compute(ws)),
        Writer::Access("agents_manage") => path().map(|_| k2_core::workspace::settings::agents_can_manage_agents(ws)),
        Writer::Access("projects_manage") => path().map(|_| k2_core::workspace::settings::agents_can_manage_projects(ws)),
        Writer::Access("chat_manage") => path().map(|_| k2_core::workspace::settings::agents_can_manage_chat(ws)),
        Writer::Access("vault_manage") => path().map(|_| k2_core::workspace::settings::agents_can_manage_vault(ws)),
        Writer::Access("gardens_manage") => path().map(|_| k2_core::workspace::settings::agents_can_manage_gardens(ws)),
        Writer::Access("apps_build") => {
            path()?;
            Ok(crate::apps::access::level(ws) >= crate::apps::access::Level::Build)
        }
        Writer::Access("app_users") => path().map(|_| k2_core::workspace::settings::agents_can_manage_app_users(ws)),
        Writer::Access("sim") => {
            path()?;
            let db = k2_core::db::shared();
            let conn = db.lock();
            Ok(k2_core::sim::store::agents_can_use_sim(&conn, ws))
        }
        Writer::Access("apps_manage_full") => {
            path()?;
            let level = crate::apps::access::level(ws);
            if want_on && level == crate::apps::access::Level::Off {
                return Err(format!(
                    "\"Manage its Apps end to end\" sits under \"Build and run Apps\", which is off for {}: ask for apps.build first",
                    ctx.workspace_handle
                ));
            }
            Ok(level == crate::apps::access::Level::Full)
        }
        Writer::Access(other) => Err(format!("K2 doesn't know the switch {other}")),
        Writer::AppCore => {
            path()?;
            let core = param(ctx, "core").ok_or("name the core: core=<db|files|thread|mail|sew>")?;
            if !k2_core::workspace::settings::APP_CORES.contains(&core) {
                return Err(format!("unknown core '{core}' (cores: {})", k2_core::workspace::settings::APP_CORES.join(", ")));
            }
            Ok(crate::apps::cores::ticked(ws).iter().any(|c| c == core))
        }
        Writer::Dns => {
            let p = path()?;
            // On: the effective gate (the server-wide master lets it in
            // too). Off: this workspace's own switch.
            Ok(if want_on {
                k2_core::workspace::settings::dns_manage_allowed_for_path(&p)
            } else {
                k2_core::workspace::settings::get_dns_manage_enabled(&p)
            })
        }
        Writer::MailManage => path().map(|p| k2_core::workspace::settings::mail_manage_allowed_for_path(&p)),
        Writer::MailSendMode => {
            let p = path()?;
            let now = k2_core::workspace::settings::mail_agent_send_for_path(&p);
            if !want_on {
                return Ok(now != "off");
            }
            // "with approval" is already met by "on" (freely): asking for
            // approval never lowers a freer setting.
            Ok(match param(ctx, "mode") {
                Some("on") => now == "on",
                _ => now == "approval" || now == "on",
            })
        }
        Writer::DbAccess => path().map(|p| k2_core::workspace::settings::db_agent_access_for_path(&p) == "write"),
    }
}

fn check(sw: &Switch, ctx: &Ctx, want_on: bool) -> Check {
    match is_on(sw, ctx, want_on) {
        Ok(on) => check_state(want_on, on),
        Err(why) => Check::Moved(why),
    }
}

fn write(sw: &Switch, ctx: &Ctx, who: &Answerer, on: bool) -> CliResponse {
    let ws = ctx.workspace_id.as_str();
    let path = || ws_path(ws);
    match sw.writer {
        Writer::Access(toggle) => {
            crate::sidecar_routes::access_entry(who, &json!({ "toggle": toggle, "workspace": ws, "value": on }))
        }
        Writer::AppCore => crate::sidecar_routes::access_entry(
            who,
            &json!({ "toggle": "app_core", "workspace": ws, "core": param(ctx, "core").unwrap_or_default(), "value": on }),
        ),
        Writer::Dns => match path() {
            Some(p) => crate::misc_routes::dns_manage_entry(who, &p, on),
            None => CliResponse::bad_request(GONE),
        },
        Writer::MailManage => match path() {
            Some(p) => crate::mail_routes::mail_manage_entry(who, &p, on),
            None => CliResponse::bad_request(GONE),
        },
        Writer::MailSendMode => match path() {
            Some(p) => {
                let mode = if on { param(ctx, "mode").unwrap_or("approval") } else { "off" };
                crate::mail_routes::mail_send_mode_entry(who, &p, mode)
            }
            None => CliResponse::bad_request(GONE),
        },
        Writer::DbAccess => match path() {
            Some(p) => crate::workspace_routes::agent_switch_entry(who, &p, "db_agent_access", if on { "write" } else { "off" }),
            None => CliResponse::bad_request(GONE),
        },
    }
}

fn check_on<const I: usize>(ctx: &Ctx) -> Check {
    check(&SWITCHES[I], ctx, true)
}

fn check_off<const I: usize>(ctx: &Ctx) -> Check {
    check(&SWITCHES[I], ctx, false)
}

fn apply_on<const I: usize>(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    applied(&write(&SWITCHES[I], ctx, who, true), "")
}

fn apply_off<const I: usize>(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    applied(&write(&SWITCHES[I], ctx, who, false), off_message(ctx))
}

macro_rules! register {
    ($($i:literal),* $(,)?) => {
        vec![$(
            Registered { spec: &SWITCHES[$i].on, check: check_on::<$i>, apply: apply_on::<$i> },
            Registered { spec: &SWITCHES[$i].off, check: check_off::<$i>, apply: apply_off::<$i> },
        )*]
    };
}

pub(crate) fn actions() -> Vec<Registered> {
    register!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16)
}

// ── not card-able (§5.4, §9) ────────────────────────────────────────

const IN_SETTINGS: &str = "Settings, or a terminal outside K2";

/// Rows near the switches that aren't v1 cards: the two "no" rows of A1,
/// the rows a switch covers, and Rosson's rule that agents can't ask for
/// wider roles or clones (2026-10-21).
pub(crate) static NOT_CARDABLE: [NotCardableSpec; 11] = [
    NotCardableSpec {
        id: "heartbeat.wake-computer",
        area: Area::Switches,
        why: NotCardable::Dropped,
        r#where: "Settings → Workspaces → <agent> → Heartbeats (any login may turn wake on)",
    },
    NotCardableSpec {
        id: "workspace.skip-permissions",
        area: Area::Switches,
        why: NotCardable::Dropped,
        r#where: "a terminal outside K2 (k2 workspace api-skip-permissions)",
    },
    NotCardableSpec { id: "compute.grant", area: Area::Compute, why: NotCardable::Covered { by: "compute.use" }, r#where: IN_SETTINGS },
    NotCardableSpec { id: "compute.node.pause", area: Area::Compute, why: NotCardable::Dropped, r#where: "Settings → Compute" },
    NotCardableSpec { id: "domain.apex.add", area: Area::Domains, why: NotCardable::Covered { by: "dns.manage" }, r#where: IN_SETTINGS },
    NotCardableSpec { id: "vault.widen", area: Area::Vault, why: NotCardable::Dropped, r#where: "Settings → Vault" },
    NotCardableSpec { id: "vault.self-grant", area: Area::Vault, why: NotCardable::Dropped, r#where: "Settings → Vault" },
    NotCardableSpec { id: "mail.rules", area: Area::Mail, why: NotCardable::Dropped, r#where: "Settings → Mail" },
    NotCardableSpec { id: "mail.inbox-access", area: Area::Mail, why: NotCardable::Dropped, r#where: "Settings → Mail" },
    // Rosson (2026-10-21): agents can't ask for wider roles or clones.
    NotCardableSpec {
        id: "app.role-caps",
        area: Area::Apps,
        why: NotCardable::Dropped,
        r#where: "Settings → Apps → Roles (talk to the agent that manages the role)",
    },
    NotCardableSpec {
        id: "clone.approve",
        area: Area::Clone,
        why: NotCardable::Dropped,
        r#where: "Settings → Clone, by the person who runs the clone",
    },
];

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    NOT_CARDABLE.iter().collect()
}

/// The parity test's view: every switch's writer route (the Settings door)
/// with its route_policy path, so a test can walk both doors.
#[cfg(test)]
pub(crate) fn writer_route(w: Writer) -> &'static str {
    match w {
        Writer::Access(_) | Writer::AppCore => "/cli/agent-access/set",
        Writer::Dns => "/cli/dns-manage",
        Writer::MailManage => "/cli/mail-manage",
        Writer::MailSendMode => "/cli/mail/config/set",
        Writer::DbAccess => "/cli/workspace/set",
    }
}

#[cfg(test)]
#[path = "e1_tests.rs"]
mod e1_tests;
