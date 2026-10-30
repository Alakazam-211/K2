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

fn caller_of(who: &Answerer) -> VaultCaller {
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
    let found = request.and_then(|id| crate::permission::store::get(id).ok().flatten()).filter(|r| r.action == SHARE_ACTION && r.is_answerable());
    Some(match found {
        None => Err("that card isn't waiting for an answer".to_string()),
        Some(r) => {
            let ctx = crate::permission::present::ctx_of(&r);
            let ctx = with_computed(SHARE_ACTION, &ctx).unwrap_or(ctx);
            Ok(crate::permission::present::fill("share {secret} with {service} as {how}", &ctx))
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

pub(crate) fn actions() -> Vec<Registered> {
    vec![Registered { spec: &DELIVERY, check, apply }, Registered { spec: &SHARE_SERVICE, check: check_share, apply: apply_share }]
}

pub(crate) fn not_cardable() -> Vec<&'static NotCardableSpec> {
    Vec::new()
}

#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;
