//! `/cli/app/init` (prd-tauri-core-0467-v1 TC-13): `k2 app init --template
//! tauri|static|node-api [<folder>] [--name <app>] [--dry-run]`.
//!
//! POST `{folder, template, name?, app?, voice?, dryRun?}`: the daemon plans
//! the template's files in `folder` (`k2_core::app_templates`) and, unless
//! it is a dry run, writes them. It never overwrites a file: one that exists
//! is `skipped`; `tauri.conf.json` only gains the keys it lacks; JSON5 and
//! TOML Tauri configs get the snippet printed instead.
//!
//! `app` (0.46.8, `--app`): a thin shell over an App already on this box,
//! one the caller may reach and K2 fronts (the update feed's own rules).
//!
//! Who: as `k2 app sdk add` (whoever may build from that folder: the owner,
//! an Owner/Admin login, a K2 shell tab, an agent with "Build and run
//! Apps"); an agent's folder must be inside its own workspace
//! (`resolve_folder`, the build jail).

use k2_core::app_templates::{self, codes as tpl_codes};
use k2_core::deploy::codes;
use serde_json::{json, Value as J};

use super::routes::{json_body, need_build, no, resolve_folder, s, Refusal, Who, R};

pub const ROUTE: &str = "/cli/app/init";
/// 0.46.9: `k2 app doctor --store` (read only; POST for the folder).
pub const STORE_ROUTE: &str = "/cli/app/doctor/store";
pub const ROUTES: &[&str] = &[ROUTE, STORE_ROUTE];
pub const POST_ROUTES: &[&str] = &[ROUTE, STORE_ROUTE];

fn refusal(e: app_templates::TemplateError) -> Refusal {
    let status = match e.code {
        tpl_codes::TEMPLATE_UNKNOWN | tpl_codes::NAME_INVALID | tpl_codes::TAURI_ONLY => 400,
        tpl_codes::NOT_A_TAURI_REPO | tpl_codes::CHANGED_UNDER_US => 409,
        _ => 500,
    };
    no(e.code, status, e.message)
}

/// POST `/cli/app/init`.
pub(crate) fn route(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let raw = s(&v, "folder").ok_or_else(|| no(codes::BAD_REQUEST, 400, "folder is required (an absolute path)"))?;
    let folder = resolve_folder(who, raw)?;
    if !folder.is_dir() {
        return Err(no(codes::BAD_REQUEST, 400, format!("{} is not a folder", folder.display())));
    }
    let template = s(&v, "template").ok_or_else(|| {
        no(codes::BAD_REQUEST, 400, format!("template is required: {}", app_templates::TEMPLATES.join(", ")))
    })?;
    let dry = v.get("dryRun").and_then(J::as_bool).unwrap_or(false);
    let sub = super::origins::box_subdomain();
    let voice = v.get("voice").and_then(J::as_bool).unwrap_or(false);
    let existing = match s(&v, "app") {
        // Before the App is looked up, so the template's own refusal wins.
        Some(_) if template != "tauri" => {
            return Err(no(tpl_codes::TAURI_ONLY, 400, format!("--app goes with --template tauri, not --template {template}")));
        }
        Some(raw) => Some(super::updates_routes::shell_target(who, raw)?),
        None => None,
    };
    let facts = (template == "tauri").then(|| ci_facts(&folder));
    let options = app_templates::Options { name: s(&v, "name"), sub: Some(sub.as_str()).filter(|x| !x.is_empty()), existing: existing.as_ref(), voice, ci: facts.as_ref() };
    let plan = app_templates::plan_with(&folder, template, &options).map_err(refusal)?;
    let mut out = app_templates::plan_json(&plan, dry);
    out["ok"] = json!(true);
    out["dryRun"] = json!(dry);
    if !dry {
        let applied = app_templates::apply(&folder, &plan).map_err(refusal)?;
        k2_core::log_debug!(
            "[apps] init {} {} by {}: {} written, {} edited",
            template,
            folder.display(),
            super::routes::actor(who),
            applied.written.len(),
            applied.edited.len()
        );
        out["applied"] = json!(applied);
    }
    Ok(out)
}

/// What this server knows for the Tauri template's CI files (C8): its
/// controller fingerprint (compute on), the workspace the folder is in, its
/// one active client seat (CI on compute on; several or none: the files
/// get a placeholder), and the repo's GitHub `owner/repo`. Read only.
fn ci_facts(folder: &std::path::Path) -> app_templates::ci::CiFacts {
    let mut f = app_templates::ci::CiFacts::default();
    if k2_core::compute::enabled() {
        f.controller_fp = crate::compute_admin::trust_controller_key().ok().map(|k| k.fingerprint());
    }
    {
        let h = k2_core::db::shared();
        let conn = h.lock();
        let rows: Vec<(String, String)> = conn
            .prepare("SELECT id, path FROM projects")
            .and_then(|mut st| st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).map(|rows| rows.filter_map(Result::ok).collect()))
            .unwrap_or_default();
        f.workspace_id = rows.into_iter().filter(|(_, p)| !p.is_empty() && folder.starts_with(p)).max_by_key(|(_, p)| p.len()).map(|(id, _)| id);
        if k2_core::compute::ci_enabled() {
            let active: Vec<k2_core::compute::trust::SeatRow> =
                k2_core::compute::trust::seats(&conn, Some("client")).into_iter().filter(|s| s.state == "active").collect();
            if let [one] = active.as_slice() {
                f.owner = Some(one.owner_id.clone());
                f.lane = Some(one.lane.clone());
                f.owner_key_fp = one.owner_key_fp.clone();
            }
        }
    }
    let o = crate::ci_engine::ops();
    if let Ok(root) = o.repo_root(folder) {
        f.github_repo = o.remote_slug(&root, "origin");
    }
    f
}

/// POST `/cli/app/doctor/store` `{folder, voice?}` (0.46.9): what App Store
/// and Play review, and iOS 27, need from a Tauri App's repo
/// (`k2_core::app_templates::doctor`). It only reads the folder.
/// `voice`: true or false; absent, the check works it out from the repo.
/// Who: as `/cli/app/init` (an agent only inside its own workspace).
pub(crate) fn store_route(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let raw = s(&v, "folder").ok_or_else(|| no(codes::BAD_REQUEST, 400, "folder is required (an absolute path)"))?;
    let folder = resolve_folder(who, raw)?;
    if !folder.is_dir() {
        return Err(no(codes::BAD_REQUEST, 400, format!("{} is not a folder", folder.display())));
    }
    let voice = v.get("voice").and_then(J::as_bool);
    let report = app_templates::doctor::check(&folder, voice).map_err(refusal)?;
    Ok(json!(report))
}
