//! `k2 app init --template <name>` (prd-tauri-core-0467-v1 TC-13): the K2
//! side of an App, written into a repo. Pure planning ([`plan`]) and a
//! careful writer ([`apply`]); the daemon's `/cli/app/init` route jails the
//! folder and calls both.
//!
//! - `tauri`, into an existing Tauri v2 repo: `k2/web.manifest.json` (a
//!   static App for the web build), the updater wiring in
//!   `src-tauri/tauri.conf.json` (`plugins.updater.endpoints` on the App's
//!   label, an empty `pubkey`, `bundle.createUpdaterArtifacts: true`),
//!   `k2/README.md` with the next five commands, and a `tauri-linux`
//!   compute preset when the repo's `.k2/compute.toml` has presets.
//! - `static`: `k2/web.manifest.json` only.
//! - `node-api`: a small `@k2/server` backend in `k2/api/` (manifest, a
//!   health route, `host.checks`, and the parted-upload route: a phone sends
//!   parts of at most 16 MiB and the server calls `files.uploadChunk` per
//!   part, TC-27).
//!
//! **Never overwrites a file** (TA-17; the 10-27 file-overwrite bug): a file
//! that exists is `skipped` with a diff hint, and new files are created
//! with `create_new`. The Tauri config is edited only when it is
//! `tauri.conf.json` and only by inserting keys it lacks, with every other
//! byte kept; `tauri.conf.json5` and `Tauri.toml` get the snippet printed
//! instead (TA-17: Tauri reads all three).

mod json_insert;
mod text;

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{json, Value as J};

/// The templates `k2 app init` knows.
pub const TEMPLATES: &[&str] = &["tauri", "static", "node-api"];

/// Largest phone upload part the `node-api` template accepts (TC-27).
pub const UPLOAD_PART_MAX: u64 = 16 << 20;

pub mod codes {
    /// `--template tauri` in a folder with no Tauri config.
    pub const NOT_A_TAURI_REPO: &str = "not_a_tauri_repo";
    /// Not one of [`super::TEMPLATES`].
    pub const TEMPLATE_UNKNOWN: &str = "template_unknown";
    /// `--name` (or the folder's name) can't be an App id.
    pub const NAME_INVALID: &str = "app_id_invalid";
    /// A file changed between the plan and the write.
    pub const CHANGED_UNDER_US: &str = "init_changed";
    pub const IO: &str = "io_error";
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TemplateError {
    pub code: &'static str,
    pub message: String,
}

fn err(code: &'static str, message: impl Into<String>) -> TemplateError {
    TemplateError { code, message: message.into() }
}

/// What happens to one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Action {
    /// A new file.
    Write,
    /// An existing file gains keys or a section (every other byte kept).
    Edit,
    /// It exists: left as it is.
    Skipped,
    /// K2 doesn't edit this format: the change is printed to make by hand.
    Snippet,
}

/// One file of a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePlan {
    /// Relative to the folder, `/` separated.
    pub path: String,
    pub action: Action,
    /// Why (skipped: same as the template / differs; edit: what it adds).
    pub note: String,
    /// The whole new content (write, edit) or the snippet (snippet). Empty
    /// for skipped.
    pub content: String,
    /// For an edit: the file as it was planned against (the writer checks
    /// it hasn't changed).
    #[serde(skip)]
    pub before: Option<String>,
}

/// A whole plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub template: String,
    pub folder: String,
    /// The App name the files use (`<name>-web`, `<name>-api`).
    pub name: String,
    pub files: Vec<FilePlan>,
    /// The commands to run next, in order.
    pub next: Vec<String>,
    pub notes: Vec<String>,
}

/// An App id base from `raw` (a folder name or `--name`): lower-case
/// letters, digits and `-`, a letter first, at most 28 characters so
/// `<name>-web` fits the guarded engine's 32.
pub fn name_from(raw: &str) -> Option<String> {
    let mut out = String::new();
    for c in raw.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    let out: String = out.trim_matches('-').chars().take(28).collect::<String>().trim_end_matches('-').to_string();
    (out.starts_with(|c: char| c.is_ascii_lowercase()) && !out.contains("--")).then_some(out)
}

/// The App's public host for `app_id` on box `sub` (`None` without K2
/// Connect: the files use a placeholder and say so).
fn host_for(app_id: &str, sub: Option<&str>) -> Option<String> {
    sub.filter(|s| !s.trim().is_empty()).map(|s| format!("{app_id}.{}.{}", s.trim(), crate::tunnel::config::SUBDOMAIN_HOST))
}

/// Plan `template` in `folder`. `name`: `--name`, else the folder's name;
/// `sub`: this box's K2 Connect subdomain, for the App's URL.
pub fn plan(folder: &Path, template: &str, name: Option<&str>, sub: Option<&str>) -> Result<Plan, TemplateError> {
    if !TEMPLATES.contains(&template) {
        return Err(err(codes::TEMPLATE_UNKNOWN, format!("--template is one of {} (not '{template}')", TEMPLATES.join(", "))));
    }
    let raw = name.map(str::to_string).unwrap_or_else(|| folder.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
    let base = name_from(&raw).ok_or_else(|| {
        err(codes::NAME_INVALID, format!("'{raw}' can't name an App: pass --name <name> (lower-case letters, digits and -, a letter first, up to 28)"))
    })?;
    let suffix = if template == "node-api" { "api" } else { "web" };
    let app_id = format!("{base}-{suffix}");
    if let Some((_, msg)) = crate::deploy::app_id_problem(&app_id) {
        return Err(err(codes::NAME_INVALID, format!("{msg} (from --name {base})")));
    }
    let host = host_for(&app_id, sub);
    let mut p = Plan { template: template.to_string(), folder: folder.display().to_string(), name: base.clone(), files: Vec::new(), next: Vec::new(), notes: Vec::new() };
    if host.is_none() {
        p.notes.push(format!(
            "this box has no K2 Connect name yet, so the files say <your-app-host> where {app_id}'s host goes; put it in once the App has one (k2 app list shows its URL)"
        ));
    }
    let host_s = host.clone().unwrap_or_else(|| "<your-app-host>".into());
    match template {
        "tauri" => plan_tauri(folder, &base, &app_id, &host_s, &mut p)?,
        "static" => {
            new_file(folder, "k2/web.manifest.json", &text::web_manifest(&app_id, &base), &mut p);
            p.next = vec![
                "build the site (its output folder, like dist/)".into(),
                format!("k2 app install {app_id} --tunnel   (once: the App and its public URL)"),
                format!("k2 app deploy {app_id} dist --manifest k2/web.manifest.json"),
                format!("k2 app list   (its URL: https://{host_s}/)"),
            ];
        }
        _ => {
            for (rel, content) in text::node_api_files(&app_id, &base) {
                new_file(folder, &rel, &content, &mut p);
            }
            p.next = vec![
                "cd k2/api && npm install".into(),
                "k2 app sdk add k2/api".into(),
                format!("k2 app install {app_id} --tunnel   (once: the App and its public URL)"),
                format!("k2 app deploy {app_id} k2/api"),
                format!("k2 app list   (its URL: https://{host_s}/)"),
            ];
        }
    }
    Ok(p)
}

/// A new file, or `skipped` when one is there (never overwritten).
fn new_file(folder: &Path, rel: &str, content: &str, p: &mut Plan) {
    let path = folder.join(rel);
    if path.exists() {
        let same = std::fs::read_to_string(&path).is_ok_and(|t| t == content);
        let note = if same {
            "already there, the same as the template".to_string()
        } else {
            format!("already there and different: left as it is (k2 app init --template {} --dry-run shows the template's version)", p.template)
        };
        p.files.push(FilePlan { path: rel.to_string(), action: Action::Skipped, note, content: String::new(), before: None });
    } else {
        p.files.push(FilePlan { path: rel.to_string(), action: Action::Write, note: "new".into(), content: content.to_string(), before: None });
    }
}

fn plan_tauri(folder: &Path, base: &str, app_id: &str, host: &str, p: &mut Plan) -> Result<(), TemplateError> {
    let tauri_dir = folder.join("src-tauri");
    let json_conf = tauri_dir.join("tauri.conf.json");
    let other_conf = ["tauri.conf.json5", "Tauri.toml"].into_iter().find(|f| tauri_dir.join(f).is_file());
    if !json_conf.is_file() && other_conf.is_none() {
        return Err(err(
            codes::NOT_A_TAURI_REPO,
            format!(
                "{} has no src-tauri/tauri.conf.json (nor .json5 or Tauri.toml): run this in a Tauri v2 app's repo root, or pass the folder; \
                 for a plain web build use --template static",
                folder.display()
            ),
        ));
    }
    new_file(folder, "k2/web.manifest.json", &text::web_manifest(app_id, base), p);
    let endpoint = format!("https://{host}/_k2/updates/stable/{{{{target}}}}/{{{{arch}}}}/{{{{current_version}}}}");
    let updater = json!({ "endpoints": [endpoint], "pubkey": "" });
    if json_conf.is_file() {
        let before = std::fs::read_to_string(&json_conf).map_err(|e| err(codes::IO, format!("read src-tauri/tauri.conf.json: {e}")))?;
        match json_insert::wire_updater(&before, &updater) {
            Ok(json_insert::Wired { text: Some(after), added }) => p.files.push(FilePlan {
                path: "src-tauri/tauri.conf.json".into(),
                action: Action::Edit,
                note: format!("adds {} (every other line kept)", added.join(", ")),
                content: after,
                before: Some(before),
            }),
            Ok(json_insert::Wired { text: None, .. }) => p.files.push(FilePlan {
                path: "src-tauri/tauri.conf.json".into(),
                action: Action::Skipped,
                note: "already has plugins.updater and bundle.createUpdaterArtifacts: left as it is (check its endpoints point at the App's /_k2/updates/…)".into(),
                content: String::new(),
                before: None,
            }),
            Err(why) => p.files.push(FilePlan {
                path: "src-tauri/tauri.conf.json".into(),
                action: Action::Snippet,
                note: format!("not edited ({why}): add this by hand"),
                content: text::tauri_json_snippet(&updater),
                before: None,
            }),
        }
    } else if let Some(f) = other_conf {
        let snippet = if f == "Tauri.toml" { text::tauri_toml_snippet(&updater) } else { text::tauri_json_snippet(&updater) };
        p.files.push(FilePlan {
            path: format!("src-tauri/{f}"),
            action: Action::Snippet,
            note: format!("K2 edits tauri.conf.json only: add this to {f} by hand"),
            content: snippet,
            before: None,
        });
    }
    new_file(folder, "k2/README.md", &text::tauri_readme(app_id, host), p);
    // CE-3: a preset stub when the repo already keeps compute presets.
    let compute = folder.join(".k2/compute.toml");
    if let Ok(t) = std::fs::read_to_string(&compute) {
        if t.contains("[presets.") && !t.contains("[presets.tauri-linux]") {
            let mut after = t.clone();
            if !after.ends_with('\n') {
                after.push('\n');
            }
            after.push_str(text::COMPUTE_PRESET);
            p.files.push(FilePlan {
                path: ".k2/compute.toml".into(),
                action: Action::Edit,
                note: "adds [presets.tauri-linux] at the end".into(),
                content: after,
                before: Some(t),
            });
        }
    }
    p.next = vec![
        "npx tauri signer generate -w ~/.tauri/myapp.key   (on your own Mac; keep the key there, or in the K2 vault)".into(),
        "put the printed public key in src-tauri/tauri.conf.json plugins.updater.pubkey".into(),
        format!("k2 app install {app_id} --tunnel   (once: the App and its public URL)"),
        format!("k2 app updates key {app_id} --set \"$(cat ~/.tauri/myapp.key.pub)\""),
        format!("npm run build && k2 app deploy {app_id} dist --manifest k2/web.manifest.json"),
        format!("k2 app updates push {app_id} --channel beta --version <x.y.z> --artifact <platform>=<file>"),
        format!("k2 app updates promote {app_id} --from beta --to stable"),
        format!("k2 app status {app_id}"),
    ];
    Ok(())
}

/// What [`apply`] did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Applied {
    pub written: Vec<String>,
    pub edited: Vec<String>,
    pub skipped: Vec<String>,
    pub snippets: Vec<String>,
}

/// Carry out `plan` in `folder`. New files are created only if still
/// absent (`create_new`); an edit only if the file is still as planned.
/// Stops at the first problem (files already written stay).
pub fn apply(folder: &Path, plan: &Plan) -> Result<Applied, TemplateError> {
    let mut a = Applied { written: Vec::new(), edited: Vec::new(), skipped: Vec::new(), snippets: Vec::new() };
    for f in &plan.files {
        let path: PathBuf = folder.join(&f.path);
        match f.action {
            Action::Write => {
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir).map_err(|e| err(codes::IO, format!("{}: {e}", f.path)))?;
                }
                use std::io::Write;
                let mut file = match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(file) => file,
                    Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                        a.skipped.push(f.path.clone());
                        continue;
                    }
                    Err(e) => return Err(err(codes::IO, format!("{}: {e}", f.path))),
                };
                file.write_all(f.content.as_bytes()).map_err(|e| err(codes::IO, format!("{}: {e}", f.path)))?;
                a.written.push(f.path.clone());
            }
            Action::Edit => {
                let now = std::fs::read_to_string(&path).map_err(|e| err(codes::IO, format!("{}: {e}", f.path)))?;
                if Some(&now) != f.before.as_ref() {
                    return Err(err(codes::CHANGED_UNDER_US, format!("{} changed while K2 planned the edit: run k2 app init again", f.path)));
                }
                std::fs::write(&path, &f.content).map_err(|e| err(codes::IO, format!("{}: {e}", f.path)))?;
                a.edited.push(f.path.clone());
            }
            Action::Skipped => a.skipped.push(f.path.clone()),
            Action::Snippet => a.snippets.push(f.path.clone()),
        }
    }
    Ok(a)
}

/// The plan as the route's JSON (content shown only for a dry run, and
/// always for a snippet).
pub fn plan_json(p: &Plan, dry_run: bool) -> J {
    let files: Vec<J> = p
        .files
        .iter()
        .map(|f| {
            let mut v = json!({ "path": f.path, "action": f.action, "note": f.note });
            if (dry_run && f.action != Action::Skipped) || f.action == Action::Snippet {
                v["content"] = json!(f.content);
            }
            v
        })
        .collect();
    json!({ "template": p.template, "folder": p.folder, "name": p.name, "files": files, "next": p.next, "notes": p.notes })
}

#[cfg(test)]
mod tests;
