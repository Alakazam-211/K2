//! `/cli/app/sdk/*` (0.46.6): `k2 app sdk add|status|kit`.
//!
//! - POST `/cli/app/sdk/add {folder, sew?}`: vendor `@k2/server` (this K2's
//!   version) and, with `sew`, Sew's five client packages into
//!   `<folder>/vendor/`, and point package.json at them
//!   (`k2_core::app_sdk::add`).
//! - GET `/cli/app/sdk/status?folder=`: what the folder vendored against
//!   what this box carries.
//! - POST `/cli/app/sdk/kit {path}`: keep Sew's App kit (`sew-app-kit-<v>
//!   .tar.gz`, the pinned version, verified against its pinned sha256) for
//!   `--sew`. The Sew install and upgrade path keeps it too, when the kit
//!   sits beside the release tarball (`sew::supervisor`).
//!
//! Who: whoever may build and deploy from that folder — the owner, an
//! Owner/Admin login, a K2 shell tab, an agent with "Build and run Apps"
//! (`need_build`, the deploy check; without the switch an agent is refused,
//! never held: a file copy isn't worth a card). An agent's folder must be
//! inside its own workspace (`resolve_folder`, the build jail).

use std::collections::HashMap;

use k2_core::app_sdk::{self, codes as sdk_codes, SewKit};
use k2_core::deploy::codes;
use serde_json::{json, Value as J};

use super::routes::{json_body, need_build, no, resolve_folder, s, Refusal, Who, R};

pub const ROUTES: &[&str] = &["/cli/app/sdk/add", "/cli/app/sdk/status", "/cli/app/sdk/kit"];
pub const POST_ROUTES: &[&str] = &["/cli/app/sdk/add", "/cli/app/sdk/kit"];

pub(crate) fn handle(path: &str, who: &Who, body: &[u8], params: &HashMap<String, String>) -> R<J> {
    match path {
        "/cli/app/sdk/add" => add(who, body),
        "/cli/app/sdk/status" => status(who, params),
        "/cli/app/sdk/kit" => kit(who, body),
        _ => Err(no("not_found", 404, "no such App route")),
    }
}

fn refusal(e: app_sdk::SdkError) -> Refusal {
    let status = match e.code {
        sdk_codes::SEW_KIT_MISSING | sdk_codes::SEW_KIT_UNAVAILABLE | sdk_codes::SEW_KIT_TAMPERED => 409,
        _ => 422,
    };
    no(e.code, status, e.hint)
}

fn folder_of(who: &Who, raw: Option<&str>) -> R<std::path::PathBuf> {
    let raw = raw.ok_or_else(|| no(codes::BAD_REQUEST, 400, "folder is required (the App folder, an absolute path)"))?;
    let p = resolve_folder(who, raw)?;
    if !p.is_dir() {
        return Err(no(codes::BAD_REQUEST, 400, format!("{} is not a folder", p.display())));
    }
    Ok(p)
}

/// The box's Sew kit for `--sew`: the pinned Sew release's, kept by K2.
fn box_kit() -> R<SewKit> {
    let pin = k2_root_proto::sew::pinned().version;
    let Some(k) = k2_root_proto::sew::app_kit(pin) else {
        return Err(no(
            sdk_codes::SEW_KIT_UNAVAILABLE,
            409,
            format!(
                "--sew needs Sew {}+ (its release carries the App kit); this K2 pins Sew {pin}, which has none. \
                 Vendor K2's SDK alone: k2 app sdk add <folder> (without --sew)",
                app_sdk::sew_kit::MIN_SEW
            ),
        ));
    };
    match app_sdk::load_kit(k.version, k.sha256) {
        Ok(Some(kit)) => Ok(kit),
        Ok(None) => Err(no(
            sdk_codes::SEW_KIT_MISSING,
            409,
            format!(
                "Sew's App kit {v} isn't on this box yet. k2 hostchat enable and upgrade fetch it with Sew's release \
                 (or take it from beside --tarball). To add it now: {}",
                crate::sew::fetch::ReleaseFile::app_kit(pin, k).by_hand("k2 app sdk kit"),
                v = k.version,
            ),
        )),
        Err(e) => Err(refusal(e)),
    }
}

/// The Sew kit version kept on this box (for status), if any.
fn box_kit_version() -> Option<String> {
    let pin = k2_root_proto::sew::pinned().version;
    let k = k2_root_proto::sew::app_kit(pin)?;
    app_sdk::load_kit(k.version, k.sha256).ok().flatten().map(|kit| kit.version)
}

fn add(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let folder = folder_of(who, s(&v, "folder"))?;
    let sew = if v.get("sew").and_then(J::as_bool).unwrap_or(false) { Some(box_kit()?) } else { None };
    let r = app_sdk::add(&folder, sew.as_ref()).map_err(refusal)?;
    k2_core::log_debug!(
        "[apps] sdk add {} by {}: {} ({} removed)",
        folder.display(),
        super::routes::actor(who),
        r.vendored.iter().map(|x| format!("{}@{}", x.name, x.version)).collect::<Vec<_>>().join(", "),
        r.removed.len()
    );
    let mut out = serde_json::to_value(&r).map_err(|e| no("failed", 500, e.to_string()))?;
    out["ok"] = json!(true);
    Ok(out)
}

fn status(who: &Who, params: &HashMap<String, String>) -> R<J> {
    need_build(who)?;
    let folder = folder_of(who, params.get("folder").map(String::as_str).map(str::trim).filter(|x| !x.is_empty()))?;
    let sew = box_kit_version();
    let rows = app_sdk::status(&folder, sew.as_deref()).map_err(refusal)?;
    let pin = k2_root_proto::sew::pinned().version;
    Ok(json!({
        "ok": true,
        "folder": folder.display().to_string(),
        "box": {
            "k2Server": app_sdk::k2_server::version(),
            "sewKit": sew,
            "sewPin": pin,
            "sewKitPinned": k2_root_proto::sew::app_kit(pin).is_some(),
        },
        "packages": rows,
    }))
}

fn kit(who: &Who, body: &[u8]) -> R<J> {
    need_build(who)?;
    let v = json_body(body)?;
    let raw = s(&v, "path").ok_or_else(|| no(codes::BAD_REQUEST, 400, "path is required (sew-app-kit-<v>.tar.gz, an absolute path)"))?;
    let path = resolve_folder(who, raw)?;
    let pin = k2_root_proto::sew::pinned().version;
    let k = k2_root_proto::sew::app_kit(pin).ok_or_else(|| {
        no(
            sdk_codes::SEW_KIT_UNAVAILABLE,
            409,
            format!("this K2 pins Sew {pin}, whose release has no App kit (kits start with Sew {})", app_sdk::sew_kit::MIN_SEW),
        )
    })?;
    let meta = std::fs::metadata(&path).map_err(|e| no(codes::BAD_REQUEST, 400, format!("{}: {e}", path.display())))?;
    if !meta.is_file() || meta.len() > app_sdk::sew_kit::MAX_KIT as u64 {
        return Err(no(codes::BAD_REQUEST, 400, format!("{} is not a Sew App kit file", path.display())));
    }
    let bytes = std::fs::read(&path).map_err(|e| no(codes::BAD_REQUEST, 400, format!("{}: {e}", path.display())))?;
    let kit = app_sdk::store_kit(&bytes, k.version, k.sha256).map_err(refusal)?;
    Ok(json!({
        "ok": true,
        "version": kit.version,
        "sha256": kit.sha256,
        "packages": kit.packages.iter().map(|p| json!({"name": p.name, "version": p.version, "sha256": p.sha256})).collect::<Vec<_>>(),
        "kept": true,
    }))
}
