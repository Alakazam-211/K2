//! Icon revisions instead of pictures (`project-icons-rev-v1`, 0.46.17).
//!
//! The phone app refreshes the workspace and project lists often, and every
//! workspace row carried its icon as a full data URL each time.
//!
//! - **Lists.** `GET /cli/projects/list` and `GET /cli/project-group/list`
//!   answer exactly as before unless asked `?icons=rev`. Then every row has
//!   `iconRev` (null = no icon) and no `iconUrl`. A repeated `icons` key or
//!   any value other than `rev` is a 400.
//! - **One icon.** `GET /cli/projects/get-icon` and
//!   `GET /cli/project-group/icon` carry `iconRev` in their JSON, and the
//!   wire writer tags the answer `ETag: "<iconRev>"` and answers a matching
//!   `If-None-Match` (the rev bare, quoted or weak, alone or in a list)
//!   with `304 Not Modified` and no body
//!   ([`crate::response_wire::ICON_ETAG_ROUTES`]). A 200 keeps every field
//!   it had; `iconRev` is the one added key.
//!
//! What `iconRev` is (opaque to clients; it changes exactly when the icon
//! does):
//! - **a workspace**: [`k2_core::projects_ops::project_icon_rev`], the first
//!   16 hex characters of the SHA-256 of the icon string get-icon returns
//!   (the stored icon URL, else the one detected from the repo's icon file).
//!   The default list's `iconRev` and a Thread push's `avatarRev` are the
//!   same value; only "no icon" is written as null here instead of `""`.
//! - **a project**: its `iconKey` ([`k2_core::project_groups::icon_key`],
//!   FNV-1a 64 of the stored data URL plus its length), which the list
//!   already carries and which already changes exactly when the icon does.
//!
//! Access is unchanged: these functions only reshape what the caller's
//! route already allowed it to read, and add no row.

use std::collections::HashMap;

use serde_json::Value;

use crate::cli_response::CliResponse;
use crate::project_group_routes as groups;
use k2_core::{project_groups, projects_ops};

/// The boot-status feature key.
pub const FEATURE: &str = "project-icons-rev-v1";

/// The query key that asks for revisions.
pub const PARAM: &str = "icons";

/// What a list sends for each row's icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icons {
    /// No `icons` key: the current answer, byte for byte.
    Urls,
    /// `icons=rev`: `iconRev` (null = none) and no `iconUrl`.
    Rev,
}

/// The request's [`Icons`]; `Err` (a 400 hint) for a repeated key or an
/// unknown value.
pub fn mode(params: &HashMap<String, String>) -> Result<Icons, String> {
    if crate::response_wire::current().repeats(PARAM) {
        return Err(format!("'{PARAM}' is given more than once: send it once, as {PARAM}=rev"));
    }
    match params.get(PARAM).map(String::as_str) {
        None => Ok(Icons::Urls),
        Some("rev") => Ok(Icons::Rev),
        Some(other) => Err(format!("unknown {PARAM} value '{other}': the only value is 'rev'")),
    }
}

/// A rev as the wire carries it under `?icons=rev` and on the icon routes:
/// `""` (no icon) is null.
pub fn rev_value(rev: Option<&str>) -> Value {
    match rev {
        Some(r) if !r.is_empty() => Value::String(r.to_string()),
        _ => Value::Null,
    }
}

/// A project's `iconRev` from its `iconKey` (`<fnv hex>-<length>`): null for
/// no icon and for an empty stored icon (length 0), which is no picture, so
/// the phone never asks for one.
pub fn group_rev(icon_key: Option<&str>) -> Value {
    rev_value(icon_key.filter(|k| k.rsplit_once('-').map(|(_, len)| len) != Some("0")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rev_is_a_string_or_null_and_never_for_no_picture() {
        assert_eq!(rev_value(Some("ab12")), Value::String("ab12".into()));
        assert_eq!(rev_value(Some("")), Value::Null);
        assert_eq!(rev_value(None), Value::Null);
        let key = project_groups::icon_key("data:image/png;base64,AAAA");
        assert_eq!(group_rev(Some(&key)), Value::String(key.clone()));
        // A 10-byte icon's key ends in "-10": still a picture.
        let ten = project_groups::icon_key("0123456789");
        assert!(ten.ends_with("-10"), "{ten}");
        assert_eq!(group_rev(Some(&ten)), Value::String(ten.clone()));
        // An empty stored icon is no picture.
        assert_eq!(group_rev(Some(&project_groups::icon_key(""))), Value::Null);
        assert_eq!(group_rev(None), Value::Null);
    }

    #[test]
    fn icons_takes_rev_once_or_nothing() {
        let p = |v: &str| HashMap::from([(PARAM.to_string(), v.to_string())]);
        assert_eq!(mode(&HashMap::new()), Ok(Icons::Urls));
        assert_eq!(mode(&p("rev")), Ok(Icons::Rev));
        for bad in ["", "urls", "REV", "rev "] {
            assert!(mode(&p(bad)).is_err(), "{bad:?}");
        }
    }
}

/// `GET /cli/projects/list`.
pub fn projects_list(params: &HashMap<String, String>) -> CliResponse {
    let mode = match mode(params) {
        Ok(m) => m,
        Err(e) => return CliResponse::bad_request(e),
    };
    // The read pool (speed pass P1): this route writes nothing.
    let rows = match projects_ops::projects_list_with_workspaces_read() {
        Ok(rows) => rows,
        Err(e) => return CliResponse::bad_request(e),
    };
    let body = match mode {
        Icons::Urls => serde_json::to_string(&rows),
        Icons::Rev => serde_json::to_string(&rows.into_iter().map(rev_row).collect::<Vec<Value>>()),
    };
    match body {
        Ok(s) => CliResponse::ok_json(s),
        Err(e) => CliResponse::internal_error(format!("serialize response: {e}")),
    }
}

/// One workspace row under `?icons=rev`.
fn rev_row(mut row: projects_ops::ProjectWithWorkspaces) -> Value {
    // Drop the picture before serializing: no copy of a large data URL.
    row.project.icon_url = None;
    let mut v = serde_json::to_value(&row).unwrap_or_else(|_| serde_json::json!({ "id": row.project.id }));
    if let Some(o) = v.as_object_mut() {
        o.remove("iconUrl");
        o.insert("iconRev".to_string(), rev_value(Some(&row.icon_rev)));
    }
    v
}

/// The get-icon answer: the current `found` / `dataUrl` plus `iconRev`.
#[derive(serde::Serialize)]
struct ProjectIconAnswer {
    #[serde(flatten)]
    icon: projects_ops::IconResult,
    #[serde(rename = "iconRev")]
    icon_rev: Value,
}

/// `GET /cli/projects/get-icon?path=&project_id=`.
pub fn project_icon(params: &HashMap<String, String>) -> CliResponse {
    let path = params.get("path").cloned().unwrap_or_default();
    if path.is_empty() {
        return CliResponse::bad_request("Missing 'path' parameter");
    }
    let project_id = params.get("project_id").filter(|s| !s.is_empty()).cloned();
    match projects_ops::projects_get_icon(&path, project_id.as_deref()) {
        Ok(icon) => {
            let icon_rev = rev_value(Some(&projects_ops::icon_rev_of(icon.data_url.as_deref())));
            match serde_json::to_string(&ProjectIconAnswer { icon, icon_rev }) {
                Ok(s) => CliResponse::ok_json(s),
                Err(e) => CliResponse::internal_error(format!("serialize response: {e}")),
            }
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

/// `GET /cli/project-group/list`: all projects with member counts, PoC and
/// pin/sort metadata, in nav order, plus the caller's read state.
pub fn group_list(params: &HashMap<String, String>) -> CliResponse {
    let mode = match mode(params) {
        Ok(m) => m,
        Err(e) => return groups::usage_error(e),
    };
    match project_groups::list_groups() {
        Ok(list) => {
            let mut rows = groups::with_read_state(list);
            if mode == Icons::Rev {
                for row in &mut rows {
                    if let Some(o) = row.as_object_mut() {
                        o.remove("iconUrl");
                        let rev = group_rev(o.get("iconKey").and_then(Value::as_str));
                        o.insert("iconRev".to_string(), rev);
                    }
                }
            }
            CliResponse::ok_json(serde_json::json!({ "ok": true, "groups": rows }).to_string())
        }
        Err(e) => groups::core_error(e),
    }
}

/// `GET /cli/project-group/icon?group=<id|name|prefix>`: the project's icon
/// data URL, served outside the list/show answers (data URLs run ~10-50KB).
/// `found:false` = unset (clients fall back to initials and the stable
/// color).
pub fn group_icon(params: &HashMap<String, String>) -> CliResponse {
    let selector = params.get("group").cloned().unwrap_or_default();
    if selector.is_empty() {
        return groups::usage_error("missing group (a project name, id, or unique prefix)");
    }
    let group_id = match project_groups::resolve_group(&selector) {
        Ok(id) => id,
        Err(e) => return groups::resolve_error_response(&selector, e),
    };
    match project_groups::get_group_icon(&group_id) {
        Ok(icon) => {
            // 0.46.1: the same key list/show carry, so the client caches the
            // blob under it; `iconRev` is that key (null = no icon).
            let key = icon.as_deref().map(project_groups::icon_key);
            CliResponse::ok_json(
                serde_json::json!({
                    "ok": true,
                    "groupId": group_id,
                    "found": icon.is_some(),
                    "iconRev": group_rev(key.as_deref()),
                    "iconKey": key,
                    "dataUrl": icon,
                })
                .to_string(),
            )
        }
        Err(e) => groups::core_error(e),
    }
}
