//! `/cli/app/init` (prd-tauri-core-0467-v1 TC-13): `k2 app init --template
//! tauri|static|node-api [<folder>] [--name <app>] [--dry-run]`.
//!
//! POST `{folder, template, name?, dryRun?}`: the daemon plans the
//! template's files in `folder` (`k2_core::app_templates`) and, unless it
//! is a dry run, writes them. It never overwrites a file: one that exists
//! is `skipped`; `tauri.conf.json` only gains the keys it lacks; JSON5 and
//! TOML Tauri configs get the snippet printed instead.
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
pub const ROUTES: &[&str] = &[ROUTE];
pub const POST_ROUTES: &[&str] = &[ROUTE];

fn refusal(e: app_templates::TemplateError) -> Refusal {
    let status = match e.code {
        tpl_codes::TEMPLATE_UNKNOWN | tpl_codes::NAME_INVALID => 400,
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
    let plan = app_templates::plan(&folder, template, s(&v, "name"), Some(sub.as_str()).filter(|x| !x.is_empty())).map_err(refusal)?;
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
