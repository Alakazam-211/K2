//! `vault.delivery secret= delivery=file|env` (0.46.6, Rosson 2026-10-28:
//! "a person changing a setting gets a one-click card, not 'go run a
//! command'").
//!
//! An agent that needs to READ one of its own workspace's secrets (a file
//! or an environment variable) but may not widen it itself (a person gave
//! it, or it was handed over by another agent: `created_by` isn't its
//! workspace) gets `vault_owner_confirm_required` from `k2 vault delivery
//! NAME file|env` (`POST /cli/vault/policy`). That refusal is this card's
//! wall: with permission cards on, the agent's error ends with
//! `Ask with: k2 permission request vault.delivery secret=NAME delivery=env
//! --why "…"`, and the owner sees "Let <agent> read NAME as env?".
//!
//! - **Who answers:** the owner token, an Owner or an Admin login: exactly
//!   who `k2 vault delivery` lets widen a secret (`policy::can_loosen`).
//! - **Proof:** a click, the same as Settings → Vault and `k2 vault
//!   delivery` take today (no confirm on that route; a reveal still needs
//!   one).
//! - **On Allow:** `apply` runs the `/cli/vault/policy` route entry
//!   ([`crate::vault_routes::handle_on`]) as the person who clicked, with
//!   the delivery asked for (and the ceiling raised to it when it was
//!   lower), so the route's own checks run again at the click.
//! - Only `file` and `env`: `api` (print it into a transcript) stays a
//!   Settings act, and fetch never needs widening.
//! - **Values:** the secret's NAME only; never a value. The workspace
//!   comes from the passport.
//!
//! `vault.use secret= from= as= for=` (0.46.11, prd-vault-for-agents-v1
//! A6): "let me use X for 2 h". A secret on the server, or in another
//! workspace, that the agent can't see at all.
//!
//! - **Who asks:** any agent. The request is always accepted and the card
//!   always posted, whatever the name, so asking tells it nothing about what
//!   exists ([`check_use`] never reads the vault).
//! - **Who answers:** the owner token, an Owner or an Admin login, never an
//!   agent. Only they are told when the secret isn't there, and then only
//!   Deny is offered ([`use_missing`]).
//! - **Proof:** a click for `fetch` and `agent`, which the grantee can never
//!   read. For `file` and `env` the same request-bound confirm
//!   `vault.share-service` takes, turned into the route's `grant-read`.
//! - **On Allow:** `/cli/vault/grant`'s own entry, as the person who clicked,
//!   to the asking workspace (or its session) with an `expiresAt`. The route
//!   checks the ceiling and who may share again at the click.
//! - **Ends by itself:** an expired grant stops resolving and the minute pass
//!   takes back its files.
//!
//! `vault.share-service secret= service= as=file|env` (Rosson 2026-10-30):
//! the same for a secret the agent wants to give its own published service
//! or App. `k2 publish secret add` / `k2 vault grant --to service:`
//! (`POST /cli/vault/grant`) refuse it with `vault_owner_confirm_required`
//! when a person gave the secret; with cards on, that refusal ends with the
//! line to ask with, and the owner answers from a Thread instead of a
//! terminal outside K2.
//!
//! - **Who asks:** only for a secret in the asking workspace's own scope,
//!   and only for a service or App of that workspace ([`own_service`], by
//!   its name there; a service of that name elsewhere is never reached).
//!   Anything else is refused at request time, in the same words as a name
//!   that doesn't exist.
//! - **Who answers:** the owner token, an Owner or an Admin login: who the
//!   route lets share (`policy::can_grant`). Never an agent.
//! - **Proof:** the route takes a `grant-read` confirm from the owner token
//!   (VC-B2: a share the grantee can read) and none from a Connect login. The
//!   card asks every answerer for one, `vault-share-service`, bound to this
//!   request. So: the owner token's stays as strong, a login's is stronger,
//!   and neither carries over to another card. See [`apply_share`].
//! - **On Allow:** `apply` runs the `/cli/vault/grant` route entry as the
//!   person who clicked, by the secret's and the service's ids, so the
//!   route's own checks run again at the click. Once: the answer is a
//!   compare-and-set and the confirm token works once.
//! - **After it:** the agent restarts the service; a running process keeps
//!   what it got. An App gets the same grant through the guarded engine
//!   (`publish_env::granted` → `guarded::secret_items` → a Podman secret in
//!   `$K2_SECRETS_DIR`, or an env variable). A name that wall would drop is
//!   refused at request time, so a card never ends in a file the App can't
//!   see.

use std::collections::HashMap;

use k2_core::deploy::store::{self as app_store, AppRow};
use k2_core::permission::catalog::{
    ActionSpec, Area, ConfirmAction, Direction, Expiry, Floor, Grant, NotCardableSpec, ParamFrom, ParamKind, ParamSpec, Proof,
    SettingsRef, Undo, Wall, WallCoverage, WallMatch, WallMethod, Words,
};
use k2_core::published_services as ps;
use k2_vault::{Actor, ConsumerKind, Delivery, HumanProof, Namespace, Scope, SecretStatus, SecretStore, Selector};
use serde_json::{json, Value};

use super::held::{error_of, route_floor, text_param};
use super::{Answerer, Applied, Check, Ctx, Registered};
use crate::vault::actor::{Channel, VaultCaller};

/// The card's id.
pub(crate) const ACTION: &str = "vault.delivery";
/// `k2 vault delivery` / Settings → Vault's delivery control.
pub(crate) const ROUTE: &str = crate::vault_routes::POLICY;
/// The deliveries a card may widen to.
pub(crate) const CHOICES: [&str; 2] = ["file", "env"];

pub(crate) static DELIVERY: ActionSpec = ActionSpec {
    id: ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "secret", kind: ParamKind::Secret, required: true, list: false, choices: &[] },
        ParamSpec { key: "delivery", kind: ParamKind::Choice, required: true, list: false, choices: &CHOICES },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::Click,
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{secret} delivered as {delivery}",
        what_changes: "Let {ws} read {secret} as {delivery} (file: a 0600 file it reads; env: an environment variable)",
        consequences: "{ws} can read {secret}'s value from then on (and so can anything it runs); it can still never print it with k2 vault get. Lower it again in Settings → Vault, or with k2 vault delivery {secret} fetch.",
        scope: "{secret} in {ws}",
        duration: "until someone changes it",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "vault", path: "Settings → Vault" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "vault_owner_confirm_required" },
        route_prefix: ROUTE,
        method: WallMethod::Post,
        params: &[("secret", ParamFrom::Response("/error/secret")), ("delivery", ParamFrom::Response("/error/delivery"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

fn delivery_of(ctx: &Ctx) -> Result<Delivery, String> {
    let d = text_param(ctx, "delivery").ok_or("name the delivery: delivery=file or delivery=env")?;
    if !CHOICES.contains(&d) {
        return Err(format!("delivery={d}: a card widens to file or env only"));
    }
    Delivery::parse(d).map_err(|e| e.hint)
}

fn secret_of(ctx: &Ctx) -> Result<&str, String> {
    text_param(ctx, "secret").ok_or_else(|| "name the secret: secret=NAME".to_string())
}

fn selector(ctx: &Ctx, name: &str) -> Selector {
    Selector::Name { namespace: Namespace::User, scope: Scope::workspace(ctx.workspace_id.clone()), name: name.to_string() }
}

/// Still wanted: the asking workspace has that secret, and it doesn't
/// already reach it at least that way.
fn check(ctx: &Ctx) -> Check {
    let (name, want) = match (secret_of(ctx), delivery_of(ctx)) {
        (Ok(n), Ok(d)) => (n, d),
        (Err(e), _) | (_, Err(e)) => return Check::Moved(e),
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Check::Unavailable { code: "vault_not_open", why: e.hint },
    };
    match vault.meta(&selector(ctx, name), &Actor::System("permission-card".into())) {
        Ok(m) if m.default_delivery.level() >= want.level() => Check::AlreadyDone,
        Ok(_) => Check::Needed,
        Err(_) => Check::Moved(format!("{} has no secret named {name}", ctx.workspace_handle)),
    }
}

pub(crate) fn caller_of(who: &Answerer) -> VaultCaller {
    match who {
        Answerer::OwnerToken => VaultCaller { actor: Actor::Owner, channel: Channel::Plain, session_id: None },
        Answerer::Login { user, role } => VaultCaller {
            actor: Actor::Login {
                username: user.clone(),
                role: match role.as_str() {
                    "owner" => k2_vault::Role::Owner,
                    "admin" => k2_vault::Role::Admin,
                    _ => k2_vault::Role::Member,
                },
            },
            channel: Channel::Login,
            session_id: None,
        },
    }
}

/// Through `/cli/vault/policy`'s own entry, as the person who clicked.
fn apply(ctx: &Ctx, who: &Answerer, _confirm: Option<&str>) -> Applied {
    let (name, want) = match (secret_of(ctx), delivery_of(ctx)) {
        (Ok(n), Ok(d)) => (n.to_string(), d),
        (Err(e), _) | (_, Err(e)) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(ROUTE, who) {
        return refused;
    }
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    // The ceiling rises with it when it was lower (a delivery above the
    // ceiling is refused).
    let m = match vault.meta(&selector(ctx, &name), &Actor::System("permission-card".into())) {
        Ok(m) => m,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    // By id: exactly the asking workspace's secret of that name.
    let mut body = json!({ "id": m.id, "delivery": want.as_str() });
    if m.delivery_ceiling.level() < want.level() {
        body["ceiling"] = json!(want.as_str());
    }
    let r = crate::vault_routes::handle_on(&vault, ROUTE, Ok(caller_of(who)), body.to_string().as_bytes(), &HashMap::new(), "permission-card", None);
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "vault_refused");
        return Applied::Refused { code, message };
    }
    let how = match want {
        Delivery::File => format!("k2 vault file --secret {name} (a short-lived 0600 file)"),
        _ => format!("k2 vault env-file --secret {name} or k2 vault file --secret {name}; ${name} in new sessions"),
    };
    Applied::Done { message: format!("{name} is delivered to {} as {} now: {how}.", ctx.workspace_handle, want.as_str()) }
}

// ── vault.share-service ─────────────────────────────────────────────

/// The share card's id.
pub(crate) const SHARE_ACTION: &str = "vault.share-service";
/// `k2 publish secret add` / `k2 vault grant --to service:`.
pub(crate) const SHARE_ROUTE: &str = crate::vault_routes::GRANT;
/// The confirm the card takes (`ConfirmAction::VaultShareService`), bound to
/// the request id.
pub(crate) const SHARE_CONFIRM: &str = "vault-share-service";

pub(crate) static SHARE_SERVICE: ActionSpec = ActionSpec {
    id: SHARE_ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "secret", kind: ParamKind::Secret, required: true, list: false, choices: &[] },
        ParamSpec { key: "service", kind: ParamKind::Service, required: true, list: false, choices: &[] },
        ParamSpec { key: "as", kind: ParamKind::Choice, required: true, list: false, choices: &CHOICES },
        ParamSpec { key: "env_name", kind: ParamKind::DeliveryName, required: false, list: false, choices: &[] },
        ParamSpec { key: "file_name", kind: ParamKind::DeliveryName, required: false, list: false, choices: &[] },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    proof: Proof::SettingsConfirm { action: ConfirmAction::VaultShareService },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{secret} shared with {service}",
        what_changes: "Let {ws} give its service {service} the secret {secret} as {how}",
        consequences: "{service} can read the value whenever it runs ({where}), and so can anything it runs. {ws} decides what {service} runs, so this lets {ws} use the value too. It takes effect the next time {service} starts; {ws} restarts it. To take it back: k2 publish secret remove {service} {secret} --scope ws:{ws}, then restart it.",
        scope: "{secret} in {ws}, for {service}",
        duration: "until someone removes it",
        retry: "restart the service so it picks it up",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "vault", path: "Settings → Vault" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: "vault_owner_confirm_required" },
        route_prefix: SHARE_ROUTE,
        method: WallMethod::Post,
        params: &[
            ("secret", ParamFrom::Response("/error/secret")),
            ("service", ParamFrom::Response("/error/service")),
            ("as", ParamFrom::Response("/error/as")),
            ("env_name", ParamFrom::Response("/error/envName")),
            ("file_name", ParamFrom::Response("/error/fileName")),
        ],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// A published service or App of one workspace.
pub(crate) struct OwnService {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub app: Option<AppRow>,
}

/// `label` (a name or an id) among the services of `workspace_id` only. A
/// name is unique per workspace, not per box, so nothing here looks further.
pub(crate) fn own_service(workspace_id: &str, label: &str) -> Option<OwnService> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    let row = match ps::get_by_project_name(&conn, workspace_id, label).ok().flatten() {
        Some(r) => r,
        None => ps::get_by_id(&conn, label).ok().flatten().filter(|r| r.project_id == workspace_id)?,
    };
    let app = app_store::get_app_by_service(&conn, &row.id).ok().flatten();
    Some(OwnService { id: row.id, name: row.name, kind: row.kind, app })
}

/// One share's stored values, checked.
struct Share<'a> {
    secret: &'a str,
    service: &'a str,
    how: Delivery,
    env_name: Option<&'a str>,
    file_name: Option<&'a str>,
}

impl Share<'_> {
    /// Where the service finds it: the name the grant will deliver under.
    fn place(&self) -> String {
        match self.how {
            Delivery::Env => format!("${}", self.env_name.map(str::to_string).unwrap_or_else(|| k2_vault::delivery::default_env_name(self.secret))),
            _ => format!("$K2_SECRETS_DIR/{}", self.file_name.unwrap_or(self.secret)),
        }
    }
}

fn share_of(ctx: &Ctx) -> Result<Share<'_>, String> {
    let secret = secret_of(ctx)?;
    let service = text_param(ctx, "service").ok_or("name the service: service=<its k2 publish name>")?;
    let word = text_param(ctx, "as").ok_or("say how it gets it: as=file or as=env")?;
    if !CHOICES.contains(&word) {
        return Err(format!("as={word}: a card shares as file or env only"));
    }
    let how = Delivery::parse(word).map_err(|e| e.hint)?;
    let (env_name, file_name) = (text_param(ctx, "env_name"), text_param(ctx, "file_name"));
    if env_name.is_some() && how != Delivery::Env {
        return Err("env_name= goes with as=env".into());
    }
    if file_name.is_some() && how != Delivery::File {
        return Err("file_name= goes with as=file".into());
    }
    // The name it will arrive under, chosen or not: what the route refuses
    // at the click is refused here, when asked.
    match how {
        Delivery::Env => {
            let n = env_name.map(str::to_string).unwrap_or_else(|| k2_vault::delivery::default_env_name(secret));
            k2_vault::delivery::validate_env_name(&n).map_err(|e| e.hint)?;
        }
        _ => k2_vault::delivery::validate_file_name(file_name.unwrap_or(secret)).map_err(|e| e.hint)?,
    }
    Ok(Share { secret, service, how, env_name, file_name })
}

/// The card's words K2 computes from the stored values (`{how}`, `{where}`).
pub(crate) fn with_computed(action: &str, ctx: &Ctx) -> Option<Ctx> {
    if action == USE_ACTION {
        return Some(use_computed(ctx));
    }
    if action != SHARE_ACTION {
        return None;
    }
    let mut out = ctx.clone();
    let share = share_of(ctx).ok();
    let how = if text_param(ctx, "as") == Some("env") { "an environment variable" } else { "a file" };
    out.params.insert("how".into(), json!(how));
    out.params.insert("where".into(), json!(share.map(|s| s.place()).unwrap_or_default()));
    Some(out)
}

/// The macOS sheet's words for the card's confirm, from the request's stored
/// values, never from the client's label. `None`: not this confirm.
pub(crate) fn confirm_reason(action: &str, request: Option<&str>) -> Option<Result<String, String>> {
    if action != SHARE_CONFIRM {
        return None;
    }
    let (send, server) = (super::vault_send::ACTION, super::vault_grant_server::ACTION);
    let found = request
        .and_then(|id| crate::permission::store::get(id).ok().flatten())
        .filter(|r| [SHARE_ACTION, USE_ACTION, send, server].contains(&r.action.as_str()) && r.is_answerable());
    Some(match found {
        None => Err("that card isn't waiting for an answer".to_string()),
        Some(r) if r.action == send => Ok(super::vault_send::confirm_words(&crate::permission::present::ctx_of(&r))),
        Some(r) if r.action == server => Ok(super::vault_grant_server::confirm_words(&crate::permission::present::ctx_of(&r))),
        Some(r) => {
            let ctx = crate::permission::present::ctx_of(&r);
            let words = if r.action == USE_ACTION { "let {ws} read {secret} ({from}) as {as} {lasts}" } else { "share {secret} with {service} as {how}" };
            let ctx = with_computed(&r.action, &ctx).unwrap_or(ctx);
            Ok(crate::permission::present::fill(words, &ctx))
        }
    })
}

/// Would the guarded engine drop this name? It carries only names its wall
/// takes, and a grant it drops only shows in the App's log. Asks the same
/// function a start does. An unwalled App and a `--cmd` service have no wall.
fn wall_drops(svc: &OwnService, share: &Share) -> bool {
    let Some(app) = &svc.app else { return false };
    if crate::apps::supervisor::runs_unwalled(app) {
        return false;
    }
    let (env, files) = match share.how {
        Delivery::Env => {
            let n = share.env_name.map(str::to_string).unwrap_or_else(|| k2_vault::delivery::default_env_name(share.secret));
            (vec![(n, String::new())], vec![])
        }
        _ => (vec![], vec![(share.file_name.unwrap_or(share.secret).to_string(), Vec::new())]),
    };
    !crate::apps::guarded::secret_items(env, files).1.is_empty()
}

/// Still wanted, and something a card can do: the asking workspace has that
/// secret and that service, the secret allows that delivery, and the service
/// doesn't already get it that way.
fn check_share(ctx: &Ctx) -> Check {
    let share = match share_of(ctx) {
        Ok(s) => s,
        Err(e) => return Check::Moved(e),
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Check::Unavailable { code: "vault_not_open", why: e.hint },
    };
    let sel = selector(ctx, share.secret);
    let system = Actor::System("permission-card".into());
    let Ok(m) = vault.meta(&sel, &system) else {
        return Check::Moved(format!("{} has no secret named {}", ctx.workspace_handle, share.secret));
    };
    let Some(svc) = own_service(&ctx.workspace_id, share.service) else {
        return Check::Moved(format!("{} has no service or App named {}", ctx.workspace_handle, share.service));
    };
    if m.status != SecretStatus::Active {
        return Check::Moved(format!("{} is disabled", share.secret));
    }
    if svc.kind == ps::KIND_SKIN {
        return Check::Moved(format!("{} is a static site: no process of its own runs that could read a secret", svc.name));
    }
    if share.how.level() > m.delivery_ceiling.level() {
        return Check::Moved(format!(
            "{name} can be delivered at most as {ceiling}. Ask for that first: k2 permission request vault.delivery secret={name} delivery={how}",
            name = share.secret,
            ceiling = m.delivery_ceiling,
            how = share.how.as_str()
        ));
    }
    if wall_drops(&svc, &share) {
        return Check::Moved(format!(
            "{} runs in a container, and its wall doesn't carry the name {}: pick another with env_name= or file_name=",
            svc.name,
            share.place()
        ));
    }
    let grants = match vault.grants(&sel, &system) {
        Ok(g) => g,
        Err(e) => return Check::Unavailable { code: "vault_not_open", why: e.hint },
    };
    for g in grants.iter().filter(|g| g.consumer.kind == ConsumerKind::PublishedService && g.consumer.id == svc.id && g.expires_at.is_none()) {
        let same_place = match share.how {
            Delivery::Env => g.env_name.as_deref() == share.place().strip_prefix('$'),
            _ => g.file_name.as_deref().unwrap_or(share.secret) == share.file_name.unwrap_or(share.secret),
        };
        if g.delivery == share.how && same_place {
            return Check::AlreadyDone;
        }
        // At a start the highest delivery wins: a file grant beside an env
        // one would never arrive.
        if g.delivery == Delivery::Env && share.how == Delivery::File {
            return Check::Moved(format!(
                "{} already gets {} as env, which wins over a file. Remove that first: k2 publish secret remove {} {}",
                svc.name, share.secret, svc.name, share.secret
            ));
        }
    }
    Check::Needed
}

/// Through `/cli/vault/grant`'s own entry, as the person who clicked, by the
/// secret's and the service's ids: the owner's default scope is the server
/// and a service's name is unique per workspace only.
///
/// The card's confirm is taken first, bound to this request. The route takes
/// no confirm from a login, so that's the only one a login gives; from the
/// owner token the route wants its own `grant-read`, for this secret, minted
/// only from the confirm just taken, so it still decides for itself.
fn apply_share(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let share = match share_of(ctx) {
        Ok(s) => s,
        Err(e) => return Applied::Refused { code: "bad_params".into(), message: e },
    };
    if let Err(refused) = route_floor(SHARE_ROUTE, who) {
        return refused;
    }
    let Some(token) = token else {
        return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm it's you first".into() };
    };
    let Some(request) = super::super::answer::current_request() else {
        return Applied::Refused { code: "failed".into(), message: "no request is being answered".into() };
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    let m = match vault.meta(&selector(ctx, share.secret), &Actor::System("permission-card".into())) {
        Ok(m) => m,
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    let Some(svc) = own_service(&ctx.workspace_id, share.service) else {
        return Applied::Refused { code: "not_found".into(), message: format!("{} has no service or App named {}", ctx.workspace_handle, share.service) };
    };
    let stamp = who.stamp();
    let method = match vault.confirm.take(token, SHARE_CONFIRM, Some(&request), &stamp) {
        Ok(HumanProof::Confirmed { method }) => method,
        Ok(HumanProof::None) => return Applied::Refused { code: "vault_confirm_required".into(), message: "confirm again".into() },
        Err(e) => return Applied::Refused { code: e.code.into(), message: e.hint },
    };
    let mut body = json!({ "id": m.id, "to": format!("service:{}", svc.id), "as": share.how.as_str() });
    if let Some(n) = share.env_name {
        body["envName"] = json!(n);
    }
    if let Some(n) = share.file_name {
        body["fileName"] = json!(n);
    }
    if matches!(who, Answerer::OwnerToken) {
        body["confirmToken"] = json!(vault.confirm.mint(crate::vault_routes::GRANT_READ_CONFIRM_ACTION, Some(&m.id), &stamp, &method));
    }
    let r = crate::vault_routes::handle_on(&vault, SHARE_ROUTE, Ok(caller_of(who)), body.to_string().as_bytes(), &HashMap::new(), "permission-card", None);
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "vault_refused");
        return Applied::Refused { code, message };
    }
    let restart = match &svc.app {
        Some(_) => format!("k2 app restart {}", svc.name),
        None => format!("k2 publish stop {name}, then k2 publish start {name}", name = svc.name),
    };
    let how = if share.how == Delivery::Env { "an environment variable" } else { "a file" };
    Applied::Done { message: format!("{} gets {} as {how} ({}) from its next start: {restart}.", svc.name, share.secret, share.place()) }
}

// ── vault.use ───────────────────────────────────────────────────────

/// The time-boxed use card's id.
pub(crate) const USE_ACTION: &str = "vault.use";
/// The card ends in `/cli/vault/grant`, as the person who clicked.
pub(crate) const USE_ROUTE: &str = crate::vault_routes::GRANT;
/// How the secret reaches the agent. `api` stays a Settings act.
pub(crate) const USE_HOWS: [&str; 4] = ["fetch", "agent", "file", "env"];
/// How long. Anything longer stays a standing grant a person makes in
/// Settings (PRD Q5); `session` is the agent's own session (decision 4).
pub(crate) const USE_FOR: [&str; 5] = ["1h", "2h", "8h", "24h", "session"];
/// `for=session` still has an end: the most a session-scoped thing lives
/// in this vault (`vault::SESSION_CARD_TTL_MAX_SECS`), for a session whose
/// id outlives its process.
const SESSION_USE_CAP: &str = "7d";

/// `for=session`: the grant goes to the session that asked, so an identical
/// ask from another session must get its own card.
pub(crate) fn asks_for_own_session(action: &str, values: &serde_json::Map<String, Value>) -> bool {
    action == USE_ACTION && values.get("for").and_then(Value::as_str) == Some("session")
}

pub(crate) static USE: ActionSpec = ActionSpec {
    id: USE_ACTION,
    area: Area::Vault,
    params: &[
        ParamSpec { key: "secret", kind: ParamKind::OtherSecret, required: true, list: false, choices: &[] },
        ParamSpec { key: "from", kind: ParamKind::SecretFrom, required: true, list: false, choices: &[] },
        ParamSpec { key: "as", kind: ParamKind::Choice, required: true, list: false, choices: &USE_HOWS },
        ParamSpec { key: "for", kind: ParamKind::Choice, required: true, list: false, choices: &USE_FOR },
    ],
    floor: Floor::Admin,
    direction: Direction::Loosen,
    grant: Grant::OneTime,
    // The strict answer, for a value that reads as anything unexpected;
    // `use_proof` lowers it to a click for a delivery that can't be read.
    proof: Proof::SettingsConfirm { action: ConfirmAction::VaultShareService },
    expiry: Expiry::STANDARD,
    words: Words {
        setting: "{secret} ({from}) for {ws}",
        what_changes: "Let {ws} use {secret} ({from}) as {as} {lasts}",
        consequences: "{effect}",
        scope: "{secret} ({from}), for {ws}",
        duration: "{lasts}",
        retry: "run your command again",
    },
    undo: Undo::None,
    settings: SettingsRef { page: "vault", path: "Settings → Vault" },
    walls: &[Wall {
        matches: WallMatch::Code { status: 403, code: crate::vault_use_routes::OUT_OF_REACH },
        route_prefix: "/cli/vault/",
        method: WallMethod::Post,
        params: &[("secret", ParamFrom::Response("/error/secret")), ("from", ParamFrom::Response("/error/from")), ("as", ParamFrom::Response("/error/as"))],
    }],
    coverage: WallCoverage::Code,
    machine_hints: &[],
};

/// One `vault.use` request's stored values, checked.
struct UseAsk<'a> {
    secret: &'a str,
    from: &'a str,
    how: Delivery,
    span: &'a str,
}

fn use_of(ctx: &Ctx) -> Result<UseAsk<'_>, String> {
    let secret = secret_of(ctx)?;
    let from = text_param(ctx, "from").ok_or("say where it lives: from=server or from=ws:<workspace>")?;
    if from != "server" && from.strip_prefix("ws:").is_none_or(str::is_empty) {
        return Err(format!("from={from}: it is server or ws:<workspace>"));
    }
    let word = text_param(ctx, "as").ok_or("say how you'd use it: as=fetch, agent, file or env")?;
    if !USE_HOWS.contains(&word) {
        return Err(format!("as={word}: a card gives fetch, agent, file or env"));
    }
    let how = Delivery::parse(word).map_err(|e| e.hint)?;
    let span = text_param(ctx, "for").ok_or("say how long: for=1h, 2h, 8h, 24h or session")?;
    if !USE_FOR.contains(&span) {
        return Err(format!("for={span}: it is 1h, 2h, 8h, 24h or session"));
    }
    Ok(UseAsk { secret, from, how, span })
}

/// `from=` names the asking workspace itself: those secrets are its own to
/// use, and widening one is what `vault.delivery` is for.
fn is_own_workspace(ctx: &Ctx, from: &str) -> bool {
    from.strip_prefix("ws:").is_some_and(|w| w == ctx.workspace_handle || crate::vault::workspace_id(w).as_deref() == Some(ctx.workspace_id.as_str()))
}

/// Only what the values say, never what the vault holds: a name the agent
/// can't see must get the same answer whether or not it exists, so the
/// request is always accepted and only `apply` looks it up (V20).
fn check_use(ctx: &Ctx) -> Check {
    let ask = match use_of(ctx) {
        Ok(a) => a,
        Err(e) => return Check::Moved(e),
    };
    if is_own_workspace(ctx, ask.from) {
        return Check::Moved(format!(
            "{} is your own workspace: you already use its secrets. To read one that a person gave it: k2 permission request vault.delivery secret={} delivery=file",
            ask.from, ask.secret
        ));
    }
    Check::Needed
}

/// `Some(proof)` for `vault.use` when the answer is a click: a grant its
/// grantee can never read. Anything else stays the catalog's confirm.
pub(crate) fn use_proof(action: &str, ctx: &Ctx) -> Option<Proof> {
    (action == USE_ACTION && matches!(text_param(ctx, "as"), Some("fetch" | "agent"))).then_some(Proof::Click)
}

/// The wording of `for=` (`{lasts}`).
fn lasts_words(span: &str) -> String {
    if span == "session" {
        "until this session ends".to_string()
    } else {
        format!("for {span}")
    }
}

/// The card's computed words. They never depend on the vault, so the text
/// hash is the same at the ask and the click, and the card says the same
/// thing whether or not the secret exists.
fn use_computed(ctx: &Ctx) -> Ctx {
    let mut out = ctx.clone();
    let span = text_param(ctx, "for").unwrap_or("");
    let ws = &ctx.workspace_handle;
    let ends = if span == "session" {
        "It ends by itself when this session ends, or 7 days after you allow it at the latest.".to_string()
    } else {
        format!("It ends by itself {span} after you allow it.")
    };
    let reads = match text_param(ctx, "as") {
        Some("file" | "env") => format!("{ws} can read the value until then, and so can anything it runs."),
        Some("agent") => format!("{ws} can't read the value: K2's ssh agent signs with it."),
        _ => format!("{ws} can't read the value: K2 adds it to {ws}'s calls."),
    };
    out.params.insert("lasts".into(), json!(lasts_words(span)));
    out.params.insert("effect".into(), json!(format!("{ends} {reads} To end it sooner: Settings → Vault.")));
    out
}

/// The secret `from` names, or why it can't be reached.
fn use_source(from: &str, name: &str) -> Result<Selector, String> {
    let scope = match from.strip_prefix("ws:") {
        None => Scope::server(),
        Some(w) => match crate::vault::workspace_id(w) {
            Some(id) => Scope::workspace(id),
            None => return Err(format!("K2 has no workspace named {w}, so no secret {name} there; nothing to allow.")),
        },
    };
    Ok(Selector::Name { namespace: Namespace::User, scope, name: name.to_string() })
}

/// What an approver is told when the named secret isn't there.
fn missing_line(from: &str, name: &str) -> String {
    match from.strip_prefix("ws:") {
        None => format!("K2 has no server secret {name}; nothing to allow."),
        Some(w) => format!("K2 has no secret {name} in {w}; nothing to allow."),
    }
}

/// The secret a `vault.use` card names is usable, or the line an approver
/// reads instead. A vault fault isn't "missing": the click decides then.
fn use_target(vault: &k2_vault::LocalVault, from: &str, name: &str) -> Result<k2_vault::SecretMeta, Option<String>> {
    let sel = use_source(from, name).map_err(Some)?;
    match vault.meta(&sel, &Actor::System("permission-card".into())) {
        Ok(m) if m.status == SecretStatus::Active => Ok(m),
        Ok(_) => Err(Some(format!("{name} is disabled; nothing to allow."))),
        Err(e) if e.code == "not_found" => Err(Some(missing_line(from, name))),
        Err(_) => Err(None),
    }
}

/// For a person who can answer the card, and only them: the line that
/// replaces the card's consequences when the secret isn't there (with only
/// Deny offered). `None` for every other action, and when it's there.
pub(crate) fn use_missing(action: &str, request_id: &str) -> Option<String> {
    if action != USE_ACTION {
        return None;
    }
    let req = crate::permission::store::get_by_display_id(request_id).ok().flatten()?;
    let ctx = crate::permission::present::ctx_of(&req);
    let ask = use_of(&ctx).ok()?;
    let vault = crate::vault::get().ok()?;
    use_target(&vault, ask.from, ask.secret).err().flatten()
}

/// Through `/cli/vault/grant`'s own entry, as the person who clicked, to
/// the asking workspace (or its session) only: the agent names neither the
/// grantee nor anything else the route decides. The route checks the ceiling
/// and who may share again here.
fn apply_use(ctx: &Ctx, who: &Answerer, token: Option<&str>) -> Applied {
    let refused = |code: &str, message: String| Applied::Refused { code: code.into(), message };
    let ask = match use_of(ctx) {
        Ok(a) => a,
        Err(e) => return refused("bad_params", e),
    };
    if let Err(r) = route_floor(USE_ROUTE, who) {
        return r;
    }
    let Some(request) = super::super::answer::current_request() else {
        return refused("failed", "no request is being answered".into());
    };
    let vault = match crate::vault::get() {
        Ok(v) => v,
        Err(e) => return refused(e.code, e.hint),
    };
    let m = match use_target(&vault, ask.from, ask.secret) {
        Ok(m) => m,
        Err(line) => return refused("not_found", line.unwrap_or_else(|| "the vault can't answer right now; try again".into())),
    };
    let stamp = who.stamp();
    // A readable grant: the card's confirm, bound to this request.
    let proved = if ask.how.readable() {
        let Some(token) = token else {
            return refused("vault_confirm_required", "confirm it's you first".into());
        };
        match vault.confirm.take(token, SHARE_CONFIRM, Some(&request), &stamp) {
            Ok(HumanProof::Confirmed { method }) => Some(method),
            Ok(HumanProof::None) => return refused("vault_confirm_required", "confirm again".into()),
            Err(e) => return refused(e.code, e.hint),
        }
    } else {
        None
    };
    // The grantee: exactly the asking workspace, or its session.
    let (to, consumer) = if ask.span == "session" {
        let Some(sid) = crate::permission::store::get_by_display_id(&request).ok().flatten().and_then(|r| r.session_id) else {
            return refused("no_session", "this card was asked outside a session; ask again with a time (for=1h)".into());
        };
        (format!("session:{sid}"), k2_vault::Consumer::session(sid))
    } else {
        if crate::vault::workspace_id(&ctx.workspace_handle).as_deref() != Some(ctx.workspace_id.as_str()) {
            return refused("not_found", format!("K2 can't find the workspace {} any more", ctx.workspace_handle));
        }
        (format!("ws:{}", ctx.workspace_handle), k2_vault::Consumer::workspace(ctx.workspace_id.clone()))
    };
    let secs = match k2_vault::duration::parse(if ask.span == "session" { SESSION_USE_CAP } else { ask.span }) {
        Ok(s) => s,
        Err(e) => return refused(e.code, e.hint),
    };
    let ends = chrono::Utc::now() + chrono::Duration::seconds(secs);
    let until = ends.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    // A card never shortens access someone already gave.
    let system = Actor::System("permission-card".into());
    let existing = match vault.grants(&Selector::Id(m.id.clone()), &system) {
        Ok(g) => g,
        Err(e) => return refused(e.code, e.hint),
    };
    let longer = existing.iter().find(|g| {
        g.consumer == consumer
            && g.delivery == ask.how
            && g.expires_at.as_deref().is_none_or(|x| chrono::DateTime::parse_from_rfc3339(x).is_ok_and(|t| t.with_timezone(&chrono::Utc) >= ends))
    });
    if let Some(g) = longer {
        let until = g.expires_at.as_deref().map(|x| format!("until {x}")).unwrap_or_else(|| "until someone removes it".to_string());
        return Applied::Done { message: format!("{} already has {} as {}, {until}, so nothing changed.", ctx.workspace_handle, ask.secret, ask.how.as_str()) };
    }
    let mut body = json!({ "id": m.id, "to": to, "as": ask.how.as_str(), "expiresAt": until });
    if let (Some(method), Answerer::OwnerToken) = (&proved, who) {
        body["confirmToken"] = json!(vault.confirm.mint(crate::vault_routes::GRANT_READ_CONFIRM_ACTION, Some(&m.id), &stamp, method));
    }
    let r = crate::vault_routes::handle_on(&vault, USE_ROUTE, Ok(caller_of(who)), body.to_string().as_bytes(), &HashMap::new(), "permission-card", None);
    let v: Value = serde_json::from_str(&r.body).unwrap_or(Value::Null);
    if !r.status.starts_with("200") {
        let (code, message) = error_of(&v, "vault_refused");
        return Applied::Refused { code, message };
    }
    let name = ask.secret;
    let scope = ask.from;
    let how = match ask.how {
        Delivery::Fetch => format!("k2 vault fetch --secret {name} --scope {scope} <url> (K2 adds it; you never see it)"),
        Delivery::Agent => "ssh and git sign with it on $SSH_AUTH_SOCK in new sessions".to_string(),
        Delivery::File => format!("k2 vault file {name} --scope {scope} (a short-lived 0600 file)"),
        _ => format!("k2 vault env-file {name} --scope {scope}, or ${} in new sessions", k2_vault::delivery::default_env_name(name)),
    };
    let ending = if ask.span == "session" { format!("until this session ends (at the latest {until})") } else { format!("until {until}") };
    Applied::Done { message: format!("You can use {name} ({scope}) as {} {ending}: {how}.", ask.how.as_str()) }
}

pub(crate) fn actions() -> Vec<Registered> {
    vec![
        Registered { spec: &DELIVERY, check, apply },
        Registered { spec: &SHARE_SERVICE, check: check_share, apply: apply_share },
        Registered { spec: &USE, check: check_use, apply: apply_use },
    ]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;
