//! `/cli/app/catalog*` (0.46.9, prd-one-click-suite-apps-v1 R20/R41,
//! vs-live A15): the suite App catalog and the one-click install.
//!
//! - `GET /cli/app/catalog` — every entry, whether this box can run it now
//!   (and if not, why and what to do), its default address, what is
//!   installed from it and the jobs in flight.
//! - `GET /cli/app/catalog/show?entry=sew/hearth[&version=]` — one entry:
//!   what it will do, what deploying approves, its versions.
//! - `GET /cli/app/catalog/check-address?entry=&address=[&id=][&workspace=]`
//!   — the live address check (R13): grammar, reserved, taken, the cached
//!   Pro tier (A17: never a network call per keystroke). A hint only; the
//!   install repeats every check.
//! - `POST /cli/app/catalog/install {entry, id?, address?, variant?,
//!   version?, workspace?, dryRun?, rebindMeet?}` — the job (or, with
//!   `dryRun`, the plan and nothing changed).
//! - `GET /cli/app/catalog/job?id=|app=` — a job's steps and state.
//! - `POST /cli/app/catalog/retry {job}` — a failed job whose step can be
//!   retried goes on from that step.
//!
//! Address rules (R7, R10, R13, R14): with no `address` the entry's family
//! label (`chat`, `meet`) when free; taken, `address_taken` names the holder
//! and a `suggestion` (the look's own `altLabel`, else `<installId>-<family>`,
//! never a silent number); `address: auto` takes the suggestion. A reserved
//! label is only ever its own family's. An ordinary label runs `k2 app
//! domain add`'s checks. Custom domains at deploy are a later slice (1b).
//!
//! Who: agents need "Build and run Apps" (switch 1) and act for their own
//! workspace; Members read; `rebindMeet` is a person's call.

use std::collections::HashMap;

use k2_core::deploy::{self, codes};
use k2_core::suite::jobs::{self, JobRow};
use k2_core::suite::{self, codes as sc, Catalog, Entry, Pin};
use serde_json::{json, Value as J};

use super::catalog_job::{self as job, Hooks};
use super::routes::{b, db_err, json_body, need_build, need_credential, no, s, target_ws, Refusal, Who, R};
use k2_core::brand::{CLI_NAME, LOGIN_NAME, PRODUCT_NAME};
use k2_core::{cli_name, product_name};

pub const LIST: &str = "/cli/app/catalog";
pub const SHOW: &str = "/cli/app/catalog/show";
pub const CHECK_ADDRESS: &str = "/cli/app/catalog/check-address";
pub const JOB: &str = "/cli/app/catalog/job";
pub const INSTALL: &str = "/cli/app/catalog/install";
pub const RETRY: &str = "/cli/app/catalog/retry";

pub const ROUTES: &[&str] = &[LIST, SHOW, CHECK_ADDRESS, JOB, INSTALL, RETRY];
pub const POST_ROUTES: &[&str] = &[INSTALL, RETRY];

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    let cat = suite::catalog().map_err(|e| no(sc::CATALOG_INVALID, 500, e))?;
    let facts = Facts::live();
    match path {
        LIST => list(who, cat, &facts),
        SHOW => show(who, cat, params, &facts),
        CHECK_ADDRESS => check_address(who, cat, params, &facts),
        JOB => job_route(who, params),
        INSTALL => json_body(body).and_then(|v| install_with(who, &v, cat, &facts, &job::Live)),
        RETRY => json_body(body).and_then(|v| retry(who, &v)),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

// ── what the box can host ────────────────────────────────────────────

/// This box, as the catalog needs it. Live: [`Facts::live`]; tests build one.
#[derive(Debug, Clone)]
pub(crate) struct Facts {
    pub linux: bool,
    /// Sew: its identity (`k2` | `standalone`) and version, or why not.
    pub sew: Result<(String, Option<String>), Why>,
    /// The cached Pro reading (`None` = unknown; never a network call).
    pub pro: Option<bool>,
}

/// Why something can't happen (yet), in two parts: `text`, a plain
/// sentence for people (Settings, the phone, the web), and `hint`, the
/// command an agent or the CLI runs, when there is one. People never see
/// the command; agents get both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Why {
    pub code: &'static str,
    pub text: String,
    pub hint: Option<String>,
}

impl Why {
    pub(crate) fn new(code: &'static str, text: impl Into<String>, hint: Option<&str>) -> Why {
        Why { code, text: text.into(), hint: hint.map(str::to_string) }
    }

    /// `{code, text, hint}` (a catalog reason).
    pub(crate) fn json(&self) -> J {
        json!({ "code": self.code, "text": self.text, "hint": self.hint })
    }

    /// The same as a refusal: its `hint` is the sentence with the command
    /// (the CLI's and agents' line), and `text` rides along for people.
    pub(crate) fn refusal(&self, status: u16) -> Refusal {
        let hint = match &self.hint {
            Some(cmd) => format!("{} ({cmd})", self.text),
            None => self.text.clone(),
        };
        let mut r = no(self.code, status, hint);
        r.extra = Some(json!({ "text": self.text }));
        r
    }
}

impl Facts {
    pub(crate) fn live() -> Facts {
        let sew = match crate::sew::store::load() {
            None => Err(Why::new("sew_not_installed", "Needs chat hosting on this server: turn it on in Settings → Cores → Chat / Voice", Some(concat!(cli_name!(), " hostchat enable")))),
            Some(r) if r.status == "removed" => {
                Err(Why::new("sew_not_installed", "Chat hosting was removed from this server: turn it on again in Settings → Cores → Chat / Voice", Some(concat!(cli_name!(), " hostchat enable"))))
            }
            Some(r) if !matches!(r.status.as_str(), "running" | "degraded") => {
                Err(Why::new("sew_not_running", format!("Chat hosting isn't running on this server (it is {})", r.status.replace('_', " ")), Some(concat!(cli_name!(), " hostchat status"))))
            }
            Some(r) => Ok((crate::sew::supervisor::row_provider(&r), r.installed_version.clone())),
        };
        let tier = crate::sew::supervisor::live().ops.plan_tier();
        let pro = match tier {
            k2_core::tunnel::subdomains::TierView::Unknown => None,
            t => Some(t.is_pro()),
        };
        Facts { linux: crate::sew::sew_supported(), sew, pro }
    }
}


/// Why this box can't deploy `e` now; empty = it can.
pub(crate) fn reasons(e: &Entry, f: &Facts) -> Vec<Why> {
    let mut out = Vec::new();
    if !f.linux {
        out.push(Why::new("needs_linux", "Needs a Linux server: the chat server it uses runs only on Linux", None));
    }
    match &f.sew {
        Err(why) => out.push(why.clone()),
        Ok((identity, _)) => {
            if let Ok(v) = e.version(None) {
                if suite::pick_variant(v, identity, None).is_err() {
                    out.push(Why::new(
                        sc::VARIANT_MISMATCH,
                        format!("{} {} has no build for the way this server's chat signs people in ({identity} sign-in)", e.name, v.version),
                        None,
                    ));
                }
            }
        }
    }
    if let Err((code, text)) = e.version(None) {
        out.push(Why::new(code, text, None));
    }
    if f.pro == Some(false) {
        let at = url_of(&e.default_label).map(|u| u.trim_start_matches("https://").to_string()).unwrap_or_else(|| k2_core::hosts::box_host(format_args!("{}.<your server>", e.default_label)));
        out.push(Why::new(sc::PRO_REQUIRED, format!("An address like {at} needs {LOGIN_NAME} Pro: see {}", k2_core::hosts::DASHBOARD_URL), None));
    }
    out
}

fn url_of(label: &str) -> Option<String> {
    let sub = super::origins::box_subdomain();
    (!sub.is_empty()).then(|| format!("https://{label}.{sub}.{}", k2_core::tunnel::config::SUBDOMAIN_HOST))
}

fn lineage_of(entry: &str) -> R<Vec<jobs::Lineage>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    jobs::lineage_of_entry(&conn, entry).map_err(db_err)
}

fn active_jobs() -> R<Vec<JobRow>> {
    let db = k2_core::db::shared();
    let conn = db.lock();
    jobs::active(&conn).map_err(db_err)
}

/// What approving a box service lets the App do, in plain words. A box
/// service is a door from the App's walled space to one service on this
/// box's loopback: `sew-api` reaches Sew's client API (chat), `sew-signal`
/// Sew's voice signalling (calls).
pub(crate) fn box_service_why(name: &str) -> String {
    match name {
        "sew-api" => "This App can talk to the chat service on this server, for chat".into(),
        "sew-signal" => "This App can talk to the call service on this server, for voice and video calls".into(),
        other => format!("This App can reach the {other} service on this server"),
    }
}

/// What approving a cap lets the App do. An App's chat caps are the most
/// its signed-in people's chat passes may do through it (their own roles
/// still apply).
pub(crate) fn cap_why(cap: &str) -> String {
    match cap {
        "chat:read" => "People signed in to this App may read chat through it".into(),
        "chat:post" => "People signed in to this App may post in chat through it".into(),
        "voice:join" => "People signed in to this App may join voice calls through it".into(),
        "voice:video" => "People signed in to this App may turn on video in calls".into(),
        other => format!("This App may {}", crate::permission::actions::app_login::cap_words(other)),
    }
}

/// What deploying it does and approves (R3 parts 1 and 4), from the pin.
fn will_do(e: &Entry, pin: Option<&Pin>) -> (Vec<String>, Vec<J>) {
    let mut lines = e.explain.clone();
    let mut approves = Vec::new();
    if let Some(p) = pin {
        if p.variant == suite::VARIANT_K2 {
            lines.push("People sign in with their logins for this App on this server".into());
        } else {
            lines.push("People sign in with this server's own chat logins".into());
        }
        for b in &p.seed.box_services {
            approves.push(json!({ "kind": "boxService", "name": b, "why": box_service_why(b) }));
        }
        for c in &p.seed.caps {
            approves.push(json!({ "kind": "cap", "name": c, "why": cap_why(c) }));
        }
    }
    (lines, approves)
}

fn entry_json(e: &Entry, f: &Facts) -> R<J> {
    let v = e.version(None).ok();
    let pin = v.and_then(|v| f.sew.as_ref().ok().and_then(|(id, _)| suite::pick_variant(v, id, None).ok()).and_then(|(name, _)| Pin::of(e, v, name)));
    let (lines, approves) = will_do(e, pin.as_ref());
    let reasons: Vec<J> = reasons(e, f).iter().map(Why::json).collect();
    let installed: Vec<J> = lineage_of(&e.entry)?.iter().map(|l| json!({ "app": l.app_id, "version": l.version, "variant": l.variant, "verified": !l.forked })).collect();
    let jobs: Vec<J> = active_jobs()?.iter().filter(|j| j.entry == e.entry).map(job::job_view).collect();
    let one_taken = e.one_per_box && (!installed.is_empty() || !jobs.is_empty());
    Ok(json!({
        "entry": e.entry,
        "name": e.name,
        "publisher": e.publisher,
        "summary": e.summary,
        "license": e.license,
        // Only an entry with a pinned, published version whose files K2
        // checks by sha256; `null` until then (no badge).
        "verified": e.verified().then_some(suite::VERIFIED_WORDS),
        "family": e.family,
        "defaultAddress": { "label": e.default_label, "url": url_of(&e.default_label) },
        "onePerBox": e.one_per_box,
        "version": v.map(|v| v.version.clone()),
        "versions": e.versions.iter().map(|v| v.version.clone()).collect::<Vec<_>>(),
        "variant": pin.as_ref().map(|p| p.variant.clone()),
        "willDo": lines,
        "approves": approves,
        "setup": pin.as_ref().map(|p| p.setup.clone()).unwrap_or_default(),
        "canDeploy": reasons.is_empty() && !one_taken,
        "reasons": reasons,
        "alreadyDeployed": one_taken.then(|| installed.first().and_then(|i| i["app"].as_str().map(str::to_string)).unwrap_or_else(|| "(installing)".into())),
        "installed": installed,
        "jobs": jobs,
    }))
}

fn list(who: &Who, cat: &Catalog, f: &Facts) -> R<J> {
    need_credential(who)?;
    let entries: Vec<J> = cat.entries.iter().map(|e| entry_json(e, f)).collect::<R<_>>()?;
    Ok(json!({ "ok": true, "schema": cat.schema, "entries": entries, "trust": concat!("compiled into this ", product_name!(), " release (sha256 pins)") }))
}

fn entry_of<'a>(cat: &'a Catalog, raw: Option<&str>) -> R<&'a Entry> {
    let id = raw.ok_or_else(|| no(codes::BAD_REQUEST, 400, concat!("name the catalog entry (like sew/hearth; ", cli_name!(), " app catalog lists them)")))?;
    cat.entry(id).ok_or_else(|| {
        let have: Vec<&str> = cat.entries.iter().map(|e| e.entry.as_str()).collect();
        no(sc::ENTRY_NOT_FOUND, 404, format!("no catalog entry '{id}' (this {PRODUCT_NAME} has: {})", have.join(", ")))
    })
}

fn param<'a>(params: &'a HashMap<String, String>, k: &str) -> Option<&'a str> {
    params.get(k).map(String::as_str).map(str::trim).filter(|x| !x.is_empty())
}

fn show(who: &Who, cat: &Catalog, params: &HashMap<String, String>, f: &Facts) -> R<J> {
    need_credential(who)?;
    let e = entry_of(cat, param(params, "entry"))?;
    let mut v = entry_json(e, f)?;
    if let Some(asked) = param(params, "version") {
        let ver = e.version(Some(asked)).map_err(|(c, h)| no(c, 404, h))?;
        v["version"] = json!(ver.version);
    }
    v["ok"] = json!(true);
    v["installId"] = json!(e.install_id);
    v["altLabel"] = json!(e.alt_label);
    v["steps"] = json!(jobs::STEPS);
    v["pins"] = json!(e.versions);
    Ok(v)
}

// ── addresses ────────────────────────────────────────────────────────

/// Who holds a family (suite) label now: an App, a job installing to it,
/// a workspace's grandfathered claim or a published service of that name.
fn family_holder(label: &str) -> Option<String> {
    if let Some(app) = job::suite_label_holder(label) {
        return Some(format!("the App {app}"));
    }
    if let Some(j) = active_jobs().ok()?.into_iter().find(|j| j.address == label) {
        return Some(format!("the App {} (installing now)", j.app_id));
    }
    let db = k2_core::db::shared();
    let conn = db.lock();
    if let Ok(Some(_)) = k2_core::db::schema::SubdomainWorkspace::owner_of(&conn, label) {
        return Some("a workspace's earlier claim on this server".into());
    }
    conn.query_row("SELECT name FROM published_services WHERE name = ?1 LIMIT 1", rusqlite::params![label], |r| r.get::<_, String>(0))
        .ok()
        .map(|n| format!("the published service {n}"))
}

/// Why an ordinary label can't be this install's address (`None` = free).
fn ordinary_problem(label: &str, install_id: &str, ws: &str) -> Option<Refusal> {
    if let Some(j) = active_jobs().ok().and_then(|v| v.into_iter().find(|j| j.address == label)) {
        return Some(no(sc::ADDRESS_TAKEN, 409, format!("'{label}' is the address the App {} is installing to now", j.app_id)));
    }
    super::domains::label_check(install_id, ws, label).err()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Address {
    pub kind: &'static str,
    pub label: String,
}

fn taken_refusal(e: &Entry, label: &str, holder: String, install_id: &str, ws: &str) -> Refusal {
    let sug = suggestion(e, install_id, ws);
    let mut r = no(
        sc::ADDRESS_TAKEN,
        409,
        format!(
            "'{label}' is used by {holder}. Pick another address (suggested: {sug}): {CLI_NAME} app install --catalog {} --address {sug}, or --address auto to take the suggestion",
            e.entry
        ),
    );
    // `text`: the same in plain words for people (Settings shows it; the
    // suggestion has its own button there).
    let text = format!("'{label}' is already used by {holder}. Try {sug} instead");
    r.extra = Some(json!({ "holder": holder, "suggestion": sug, "address": label, "text": text }));
    r
}

fn suggestion(e: &Entry, install_id: &str, ws: &str) -> String {
    suite::suggestion(e, install_id, &|l| family_holder(l).is_some() || ordinary_problem(l, install_id, ws).is_some())
}

/// The address an install gets (R10, R13, R14): `asked` = what was typed
/// (`None` = nothing: the family label, never a silent pick).
pub(crate) fn resolve_address(e: &Entry, install_id: &str, ws: &str, asked: Option<&str>) -> R<Address> {
    let family = e.default_label.as_str();
    let typed = match asked {
        None => None,
        Some(raw) => Some(suite::normalize_address(raw).map_err(|m| no(sc::ADDRESS_INVALID, 400, m))?),
    };
    match typed {
        // Nothing typed, or the family label itself.
        None => match family_holder(family) {
            None => Ok(Address { kind: jobs::ADDRESS_SUITE_LABEL, label: family.into() }),
            Some(h) => Err(taken_refusal(e, family, h, install_id, ws)),
        },
        Some(Some(l)) if l == family => match family_holder(family) {
            None => Ok(Address { kind: jobs::ADDRESS_SUITE_LABEL, label: l }),
            Some(h) => Err(taken_refusal(e, family, h, install_id, ws)),
        },
        // `auto`: the family label when free, else the suggestion.
        Some(None) => match family_holder(family) {
            None => Ok(Address { kind: jobs::ADDRESS_SUITE_LABEL, label: family.into() }),
            Some(_) => Ok(Address { kind: jobs::ADDRESS_LABEL, label: suggestion(e, install_id, ws) }),
        },
        Some(Some(l)) if k2_core::skin::is_reserved_nested_label(&l) => Err(no(
            sc::ADDRESS_RESERVED,
            400,
            format!("'{l}' is reserved for {} (only an App of that family takes it); pick an ordinary address like {}", k2_core::skin::reserved_nested_label(&l).map(|r| r.purpose).unwrap_or("K2"), suggestion(e, install_id, ws)),
        )),
        Some(Some(l)) => match ordinary_problem(&l, install_id, ws) {
            None => Ok(Address { kind: jobs::ADDRESS_LABEL, label: l }),
            Some(r) if r.code == super::domains::RESERVED => Err(no(sc::ADDRESS_RESERVED, 400, r.hint)),
            Some(r) => Err(taken_refusal(e, &l, r.hint, install_id, ws)),
        },
    }
}

fn app_taken(id: &str) -> bool {
    let db = k2_core::db::shared();
    let conn = db.lock();
    deploy::store::get_app(&conn, id).ok().flatten().is_some()
        || conn.query_row("SELECT 1 FROM published_services WHERE name = ?1", rusqlite::params![id], |_| Ok(())).is_ok()
}

/// The install id (R7): `id` when given (checked), else the entry's,
/// numbered for a second copy.
fn install_id(e: &Entry, asked: Option<&str>) -> R<String> {
    let busy = |id: &str| app_taken(id) || active_jobs().map(|v| v.iter().any(|j| j.app_id == id)).unwrap_or(false);
    match asked {
        None => Ok(suite::install_id_for(e, &busy)),
        Some(raw) => {
            let id = raw.trim().to_ascii_lowercase();
            if let Some((code, msg)) = deploy::app_id_problem(&id) {
                return Err(no(code, 400, msg));
            }
            if k2_root_proto::apps::AppId::parse(&id).is_none() {
                return Err(no(codes::APP_ID_INVALID, 400, format!("App id '{id}' is too long or has '--' in it: at most 32 characters")));
            }
            if busy(&id) {
                return Err(no(codes::APP_EXISTS, 409, format!("App id '{id}' is taken on this box ({CLI_NAME} app list)")));
            }
            Ok(id)
        }
    }
}

fn check_address(who: &Who, cat: &Catalog, params: &HashMap<String, String>, f: &Facts) -> R<J> {
    need_credential(who)?;
    let e = entry_of(cat, param(params, "entry"))?;
    let ws = target_ws(who, param(params, "workspace"))?;
    let id = install_id(e, param(params, "id"))?;
    let res = resolve_address(e, &id, &ws.id, param(params, "address"));
    let tier = match f.pro {
        Some(true) => json!({ "pro": true }),
        Some(false) => json!({ "pro": false, "code": sc::PRO_REQUIRED, "hint": format!("a {PRODUCT_NAME} address needs {LOGIN_NAME} Pro ({})", k2_core::hosts::DASHBOARD_URL) }),
        None => json!({ "pro": null, "hint": "Pro is checked when you deploy" }),
    };
    Ok(match res {
        Ok(a) => json!({ "ok": f.pro != Some(false), "app": id, "address": { "kind": a.kind, "label": a.label, "url": url_of(&a.label) }, "tier": tier }),
        Err(r) => {
            let mut err = json!({ "code": r.code, "hint": r.hint });
            if let Some(J::Object(m)) = r.extra {
                for (k, v) in m {
                    err[k] = v;
                }
            }
            json!({ "ok": false, "app": id, "error": err, "tier": tier })
        }
    })
}

// ── install ──────────────────────────────────────────────────────────

pub(crate) fn install_with(who: &Who, v: &J, cat: &Catalog, f: &Facts, h: &dyn Hooks) -> R<J> {
    need_build(who)?;
    let e = entry_of(cat, s(v, "entry"))?;
    if s(v, "domain").is_some() {
        // TODO(1b): a custom domain at deploy (R16/R17, A18).
        return Err(no(
            "custom_domain_later",
            400,
            concat!("a custom domain at deploy comes in a later ", product_name!(), ": deploy with a ", product_name!(), " address, then add yours with ", cli_name!(), " app domain add <app> <host>"),
        ));
    }
    if !f.linux {
        return Err(Why::new("needs_linux", "Needs a Linux server: the chat server it uses runs only on Linux", None).refusal(409));
    }
    let (identity, sew_version) = f.sew.clone().map_err(|w| w.refusal(409))?;
    let version = e.version(s(v, "version")).map_err(|(c, h)| no(c, 409, h))?;
    let (variant, _) = suite::pick_variant(version, &identity, s(v, "variant")).map_err(|(c, h)| no(c, 409, h))?;
    let pin = Pin::of(e, version, variant).ok_or_else(|| no(sc::VARIANT_MISMATCH, 409, "no such variant"))?;
    if let Some(why) = suite::sew_floor_unmet(&pin, sew_version.as_deref()) {
        return Err(no("requires_unmet", 409, why));
    }
    let ws = target_ws(who, s(v, "workspace"))?;
    if e.one_per_box {
        let existing = lineage_of(&e.entry)?.into_iter().map(|l| l.app_id).next().or_else(|| {
            active_jobs().ok().and_then(|v| v.into_iter().find(|j| j.entry == e.entry)).map(|j| format!("{} (installing now)", j.app_id))
        });
        if let Some(app) = existing {
            let mut r = no(sc::ONE_PER_BOX, 409, format!("{} is one per box and is already deployed as {app} ({CLI_NAME} app status {app})", e.name));
            r.extra = Some(json!({ "existing": app }));
            return Err(r);
        }
    }
    let app_id = install_id(e, s(v, "id"))?;
    let addr = resolve_address(e, &app_id, &ws.id, s(v, "address"))?;
    let rebind = b(v, "rebindMeet");
    if rebind && !job::is_person(who) {
        return Err(no(codes::FORBIDDEN, 403, concat!("releasing Sew's Meetings Space from a removed install is a person's call (the owner runs it from a terminal outside ", product_name!(), " or Settings)")));
    }
    let dry = b(v, "dryRun");
    let (lines, approves) = will_do(e, Some(&pin));
    let mut waits = Vec::new();
    if (addr.kind == jobs::ADDRESS_SUITE_LABEL || !pin.setup.is_empty()) && !h.may_manage_sew(who) {
        let (text, hint) = job::sew_rights_wait(&e.name, &ws.id);
        waits.push(json!({ "reason": jobs::WAIT_SEW_RIGHTS, "text": text, "hint": hint }));
    }
    let plan = json!({
        "entry": e.entry,
        "name": e.name,
        "version": pin.version,
        "variant": pin.variant,
        "app": app_id,
        "workspace": ws.id,
        "address": { "kind": addr.kind, "label": addr.label, "url": url_of(&addr.label) },
        "willDo": lines,
        "approves": approves,
        "setup": pin.setup,
        "steps": jobs::STEPS,
        "waits": waits,
        "files": { "tag": pin.tag, "file": pin.seed.file, "sha256": pin.seed.sha256, "size": pin.seed.size, "seedHash": pin.seed.seed_hash },
    });
    if dry {
        let tier = match f.pro {
            Some(true) => json!("pro"),
            Some(false) => json!("not pro: the install would be refused with pro_required"),
            None => json!("unknown: checked at deploy"),
        };
        return Ok(json!({ "ok": true, "dryRun": true, "plan": plan, "tier": tier, "note": "nothing changed" }));
    }
    // A17: the up-front Pro check (live), before anything exists.
    h.pro_check()?;
    let now = deploy::now();
    let row = JobRow {
        id: format!("sj-{}", uuid::Uuid::new_v4().simple()),
        entry: e.entry.clone(),
        version: pin.version.clone(),
        variant: pin.variant.clone(),
        app_id: app_id.clone(),
        workspace_id: ws.id.clone(),
        address_kind: addr.kind.into(),
        address: addr.label.clone(),
        actor: job::actor_of(who),
        who_json: job::who_json(who).to_string(),
        pin_json: serde_json::to_string(&pin).map_err(|e| no("db_error", 500, e.to_string()))?,
        options: json!({ "rebindMeet": rebind }),
        step: jobs::STEP_DOWNLOAD.into(),
        state: jobs::STATE_RUNNING.into(),
        wait_reason: None,
        wait_question: None,
        wait_requests: vec![],
        wait_hint: None,
        error_code: None,
        error_hint: None,
        seed_hash: None,
        deployment_id: None,
        space_id: None,
        steps: vec![],
        step_started_at: now,
        created_at: now,
        updated_at: now,
        finished_at: None,
    };
    {
        let db = k2_core::db::shared();
        let conn = db.lock();
        jobs::insert(&conn, &row).map_err(db_err)?;
    }
    super::routes::audit_human(&app_id, "suite.install.requested", who, json!({ "job": row.id, "entry": e.entry, "version": pin.version, "variant": pin.variant, "address": addr.label, "sha256": pin.seed.sha256 }));
    job::kick();
    Ok(json!({ "ok": true, "job": job::job_view(&row), "plan": plan }))
}

// ── jobs ─────────────────────────────────────────────────────────────

fn may_see(who: &Who, j: &JobRow) -> bool {
    match super::routes::agent_ws(who) {
        Some(ws) => ws == j.workspace_id,
        None => true,
    }
}

fn job_route(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_credential(who)?;
    let db = k2_core::db::shared();
    let rows = {
        let conn = db.lock();
        match (param(params, "id"), param(params, "app")) {
            (Some(id), _) => jobs::get(&conn, id).map_err(db_err)?.into_iter().collect::<Vec<_>>(),
            (None, Some(app)) => jobs::list(&conn, Some(&app.to_ascii_lowercase())).map_err(db_err)?,
            (None, None) => jobs::list(&conn, None).map_err(db_err)?,
        }
    };
    let rows: Vec<JobRow> = rows.into_iter().filter(|j| may_see(who, j)).collect();
    if let Some(id) = param(params, "id") {
        let j = rows.first().ok_or_else(|| no("job_not_found", 404, format!("no install job '{id}' ({CLI_NAME} app catalog job)")))?;
        return Ok(json!({ "ok": true, "job": job::job_view(j) }));
    }
    Ok(json!({ "ok": true, "jobs": rows.iter().map(job::job_view).collect::<Vec<_>>() }))
}

fn retry(who: &Who, v: &J) -> R<J> {
    need_build(who)?;
    let id = s(v, "job").ok_or_else(|| no(codes::BAD_REQUEST, 400, concat!("name the job (", cli_name!(), " app catalog job)")))?;
    let db = k2_core::db::shared();
    let mut j = {
        let conn = db.lock();
        jobs::get(&conn, id).map_err(db_err)?
    }
    .filter(|j| may_see(who, j))
    .ok_or_else(|| no("job_not_found", 404, format!("no install job '{id}'")))?;
    let code = j.error_code.clone().unwrap_or_default();
    if j.state != jobs::STATE_FAILED || !job::RETRYABLE.contains(&code.as_str()) {
        return Err(no("not_retryable", 409, format!("job {id} is {} ({}); only a failed {} step can be retried", j.state, if code.is_empty() { "-" } else { &code }, job::RETRYABLE.join("/"))));
    }
    let now = deploy::now();
    j.state = jobs::STATE_RUNNING.into();
    j.error_code = None;
    j.error_hint = None;
    j.finished_at = None;
    j.step_started_at = now;
    j.updated_at = now;
    {
        let conn = db.lock();
        jobs::save(&conn, &j).map_err(db_err)?;
    }
    super::routes::audit_human(&j.app_id, "suite.install.retry", who, json!({ "job": j.id, "step": j.step }));
    job::kick();
    Ok(json!({ "ok": true, "job": job::job_view(&j) }))
}

#[cfg(test)]
#[path = "catalog_routes_tests.rs"]
mod tests;
