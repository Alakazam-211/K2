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
//!   With `--app <existing App>` (0.46.8, a thin shell over a server App the
//!   box already runs): no web manifest, and the updater wired to that App's
//!   host with the download host's `latest.json` as a second endpoint. With
//!   `--voice` (0.46.8): the mic and camera keys (below).
//! - `static`: `k2/web.manifest.json` only.
//! - `node-api`: a small `@k2/server` backend in `k2/api/` (manifest, a
//!   health route, `host.checks`, and the parted-upload route: a phone sends
//!   parts of at most 16 MiB and the server calls `files.uploadChunk` per
//!   part, TC-27).
//!
//! **Every Tauri App (0.46.9, the Tauri App SDK):** the store files
//! ([`store`]): `src-tauri/PrivacyInfo.xcprivacy`, `src-tauri/Info.ios.plist`
//! (the scene manifest iOS 27 requires, the export answer), the Android
//! lines (printed), `src-tauri/src/k2_http.rs` (one HTTP client), and the
//! Cargo lines it lacks (printed: Tauri 2.12.5+, K2's patched wry). The
//! README says what user data the App stores and how to declare it.
//! `k2 app doctor --store` ([`doctor`]) checks a repo for all of it.
//!
//! **Never overwrites a file** (TA-17; the 10-27 file-overwrite bug): a file
//! that exists is `skipped` with a diff hint, and new files are created
//! with `create_new`. The Tauri config is edited only when it is
//! `tauri.conf.json` and only by inserting keys it lacks, with every other
//! byte kept; `tauri.conf.json5` and `Tauri.toml` get the snippet printed
//! instead (TA-17: Tauri reads all three).
//!
//! **`--voice`:** `src-tauri/Info.plist` (macOS: the usage keys and
//! `NSAppSleepDisabled`) and `Info.ios.plist` (Tauri merges both),
//! `src-tauri/Entitlements.plist` and `bundle.macOS.entitlements`, the first
//! window's `backgroundThrottling` and `additionalBrowserArgs` (WebView2), and
//! `src-tauri/src/k2_voice.rs` (the mic and camera for the App's own origin,
//! through Tauri 2.12's `on_permission_request`; the person adds the two lines
//! to `lib.rs`). A plist that exists is never edited: the keys it lacks are
//! printed. So are the Android lines: `gen/android` belongs to `tauri android
//! init`. `tests/app-template/tauri-voice-check` builds these files into a
//! real Tauri app and checks the webview.

pub mod ci;
pub mod doctor;
mod json_insert;
pub mod store;
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
    /// `--app` or `--voice` on a template other than `tauri`.
    pub const TAURI_ONLY: &str = "tauri_template_only";
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
    /// The App name the files use (`<name>-web`, `<name>-api`), or the
    /// existing App's id (`--app`).
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

/// A server App already on this box (`--app`), as the daemon read it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExistingApp {
    pub app_id: String,
    /// Its first public host (`None`: it has none yet).
    pub host: Option<String>,
    /// The stable channel's `latest.json` on the download host.
    pub fallback_endpoint: Option<String>,
    /// The id K2 Web mints the download-host token for.
    pub dl_id: Option<String>,
}

/// What a plan is asked for, past the template and folder.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options<'a> {
    /// `--name`, else the folder's name.
    pub name: Option<&'a str>,
    /// This box's K2 Connect subdomain, for the App's URL.
    pub sub: Option<&'a str>,
    /// `--app`: wire the shell to this App instead of a new `<name>-web`.
    pub existing: Option<&'a ExistingApp>,
    /// `--voice`: the mic and camera permission keys.
    pub voice: bool,
    /// The CI and release files' facts (C8): this server's controller, seat
    /// and workspace, as the daemon knows them (`None`: placeholders).
    pub ci: Option<&'a ci::CiFacts>,
}

/// Plan `template` in `folder` with no options past the name and box.
pub fn plan(folder: &Path, template: &str, name: Option<&str>, sub: Option<&str>) -> Result<Plan, TemplateError> {
    plan_with(folder, template, &Options { name, sub, ..Options::default() })
}

/// Plan `template` in `folder`.
pub fn plan_with(folder: &Path, template: &str, o: &Options) -> Result<Plan, TemplateError> {
    if !TEMPLATES.contains(&template) {
        return Err(err(codes::TEMPLATE_UNKNOWN, format!("--template is one of {} (not '{template}')", TEMPLATES.join(", "))));
    }
    if template != "tauri" && (o.existing.is_some() || o.voice) {
        return Err(err(codes::TAURI_ONLY, format!("--app and --voice go with --template tauri, not --template {template}")));
    }
    if let Some(e) = o.existing {
        if o.name.is_some() {
            return Err(err(codes::TAURI_ONLY, format!("--app names the App already: drop --name (the shell updates through {})", e.app_id)));
        }
        let mut p = Plan { template: template.to_string(), folder: folder.display().to_string(), name: e.app_id.clone(), files: Vec::new(), next: Vec::new(), notes: Vec::new() };
        if e.host.is_none() {
            p.notes.push(format!(
                "{} has no public host yet, so the files say <your-app-host>; give it one (k2 app domain add {} <label>), then put it in (k2 app updates list {} --json prints the URLs)",
                e.app_id, e.app_id, e.app_id
            ));
        }
        let host = e.host.clone().unwrap_or_else(|| "<your-app-host>".into());
        let t = Tauri { base: &e.app_id, app_id: &e.app_id, host: &host, existing: Some(e), voice: o.voice, ci: o.ci };
        plan_tauri(folder, &t, &mut p)?;
        return Ok(p);
    }
    let (name, sub) = (o.name, o.sub);
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
        "tauri" => plan_tauri(folder, &Tauri { base: &base, app_id: &app_id, host: &host_s, existing: None, voice: o.voice, ci: o.ci }, &mut p)?,
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

/// The Tauri template's inputs.
struct Tauri<'a> {
    base: &'a str,
    app_id: &'a str,
    host: &'a str,
    /// `--app`: a thin shell over this App; no web manifest.
    existing: Option<&'a ExistingApp>,
    voice: bool,
    ci: Option<&'a ci::CiFacts>,
}

/// The file `bundle.macOS.entitlements` names by default, from `src-tauri/`.
const ENTITLEMENTS: &str = "./Entitlements.plist";

fn plan_tauri(folder: &Path, t: &Tauri, p: &mut Plan) -> Result<(), TemplateError> {
    let (base, app_id, host) = (t.base, t.app_id, t.host);
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
    if t.existing.is_none() {
        new_file(folder, "k2/web.manifest.json", &text::web_manifest(app_id, base), p);
    }
    let endpoint = format!("https://{host}/_k2/updates/stable/{{{{target}}}}/{{{{arch}}}}/{{{{current_version}}}}");
    // A thin shell's second endpoint: the download host still answers when
    // the box is down.
    let fallback = t.existing.and_then(|e| e.fallback_endpoint.as_deref());
    let endpoints: Vec<&str> = std::iter::once(endpoint.as_str()).chain(fallback).collect();
    let updater = json!({ "endpoints": endpoints, "pubkey": "" });
    // `--voice`: where the entitlements file is, from the config (`None`: the
    // config names a path K2 won't write to; its keys are printed instead).
    let mut entitlements = Some(ENTITLEMENTS.to_string());
    if json_conf.is_file() {
        let before = std::fs::read_to_string(&json_conf).map_err(|e| err(codes::IO, format!("read src-tauri/tauri.conf.json: {e}")))?;
        match json_insert::wire_updater(&before, &updater) {
            Ok(w) => {
                let mut added = w.added;
                let mut after = w.text;
                if t.voice {
                    let cur = after.clone().unwrap_or_else(|| before.clone());
                    match json_insert::wire_entitlements(&cur, ENTITLEMENTS) {
                        Ok(json_insert::Wired { text: Some(edited), added: more }) => {
                            after = Some(edited);
                            added.extend(more);
                        }
                        Ok(json_insert::Wired { text: None, .. }) => {
                            entitlements = serde_json::from_str::<J>(&cur)
                                .ok()
                                .and_then(|v| v["bundle"]["macOS"]["entitlements"].as_str().map(str::to_string));
                        }
                        Err(why) => p.notes.push(format!(
                            "src-tauri/tauri.conf.json can't take bundle.macOS.entitlements ({why}): add \"entitlements\": \"{ENTITLEMENTS}\" under bundle.macOS by hand, or the mic and camera entitlements won't reach the build"
                        )),
                    }
                    let cur = after.clone().unwrap_or_else(|| before.clone());
                    match json_insert::wire_first_window(&cur, &text::voice_window_keys()) {
                        Ok(json_insert::Wired { text: Some(edited), added: more }) => {
                            after = Some(edited);
                            added.extend(more);
                        }
                        Ok(json_insert::Wired { text: None, .. }) => {}
                        Err(why) => p.notes.push(format!(
                            "src-tauri/tauri.conf.json: no window to give the call settings to ({why}). If your Rust code builds the window, pass its builder through k2_voice::voice_window (src-tauri/src/k2_voice.rs); else add backgroundThrottling \"disabled\" and additionalBrowserArgs (k2_voice::WEBVIEW2_ARGS) to your first window"
                        )),
                    }
                }
                match after {
                    Some(after) => p.files.push(FilePlan {
                        path: "src-tauri/tauri.conf.json".into(),
                        action: Action::Edit,
                        note: format!("adds {} (every other line kept)", added.join(", ")),
                        content: after,
                        before: Some(before),
                    }),
                    None => p.files.push(FilePlan {
                        path: "src-tauri/tauri.conf.json".into(),
                        action: Action::Skipped,
                        note: format!(
                            "already has plugins.updater and bundle.createUpdaterArtifacts{}: left as it is (make sure its endpoints are: {})",
                            if t.voice { " and the call settings" } else { "" },
                            endpoints.join(" , ")
                        ),
                        content: String::new(),
                        before: None,
                    }),
                }
            }
            Err(why) => p.files.push(FilePlan {
                path: "src-tauri/tauri.conf.json".into(),
                action: Action::Snippet,
                note: format!("not edited ({why}): add this by hand"),
                content: text::tauri_json_snippet(&updater, t.voice),
                before: None,
            }),
        }
    } else if let Some(f) = other_conf {
        let snippet = if f == "Tauri.toml" { text::tauri_toml_snippet(&updater, t.voice) } else { text::tauri_json_snippet(&updater, t.voice) };
        p.files.push(FilePlan {
            path: format!("src-tauri/{f}"),
            action: Action::Snippet,
            note: format!("K2 edits tauri.conf.json only: add this to {f} by hand"),
            content: snippet,
            before: None,
        });
    }
    let mut readme = match t.existing {
        Some(e) => text::tauri_readme_existing(app_id, host, e.fallback_endpoint.as_deref(), e.dl_id.as_deref()),
        None => text::tauri_readme(app_id, host),
    };
    if t.voice {
        readme.push_str(&text::voice_readme(host));
    }
    readme.push_str(&store::store_readme(t.voice));
    let project = base.to_string();
    readme.push_str(&ci::readme_section(app_id, &project));
    new_file(folder, "k2/README.md", &readme, p);
    let cargo_lines = plan_runtime(folder, p);
    plan_store(folder, base, t.voice, p);
    if t.voice {
        plan_voice(folder, base, host, entitlements.as_deref(), p);
    }
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
    // C8: release builds on K2 Compute (last, so earlier files keep their
    // places in the plan).
    plan_ci(folder, &project, t.ci, p);
    let token_step = |dl: Option<&str>| {
        format!(
            "ask K2 Web (the team that runs k2.dev, through your K2 contact) for this App's download-host token ({}), then k2 app updates token {app_id} --file <file>   (never on the command line)",
            dl.unwrap_or("k2 app updates token <app> names the id it is for")
        )
    };
    p.next = match t.existing {
        Some(e) => vec![
            format!("point the shell's window at the live site: app.windows[0].url = \"https://{host}/\" in src-tauri/tauri.conf.json"),
            "npx tauri signer generate -w ~/.tauri/myapp.key   (on your own Mac; keep the key there, or in the K2 vault)".into(),
            "put the printed public key in src-tauri/tauri.conf.json plugins.updater.pubkey".into(),
            format!("k2 app updates key {app_id} --set \"$(cat ~/.tauri/myapp.key.pub)\""),
            token_step(e.dl_id.as_deref()),
            format!("k2 app updates push {app_id} --channel beta --version <x.y.z> --artifact <platform>=<file>"),
            format!("k2 app updates promote {app_id} --from beta --to stable"),
            format!("k2 app status {app_id}"),
        ],
        None => vec![
            "npx tauri signer generate -w ~/.tauri/myapp.key   (on your own Mac; keep the key there, or in the K2 vault)".into(),
            "put the printed public key in src-tauri/tauri.conf.json plugins.updater.pubkey".into(),
            format!("k2 app install {app_id} --tunnel   (once: the App and its public URL)"),
            format!("k2 app updates key {app_id} --set \"$(cat ~/.tauri/myapp.key.pub)\""),
            token_step(None),
            format!("npm run build && k2 app deploy {app_id} dist --manifest k2/web.manifest.json"),
            format!("k2 app updates push {app_id} --channel beta --version <x.y.z> --artifact <platform>=<file>"),
            format!("k2 app updates promote {app_id} --from beta --to stable"),
            format!("k2 app status {app_id}"),
        ],
    };
    if t.voice {
        p.next.insert(0, VOICE_WIRE_STEP.into());
    }
    if cargo_lines {
        p.next.insert(0, CARGO_STEP.into());
    }
    p.next.push(DOCTOR_STEP.into());
    Ok(())
}

/// C8: the release build on K2 Compute: the two build scripts, `.k2/ci.toml`
/// (a check and `[release]`) and the signer policy's starting point. An
/// existing `.k2/ci.toml` without `[release]` gets the section printed.
fn plan_ci(folder: &Path, project: &str, facts: Option<&ci::CiFacts>, p: &mut Plan) {
    let none = ci::CiFacts::default();
    let f = facts.unwrap_or(&none);
    let bin = std::fs::read_to_string(folder.join("src-tauri/Cargo.toml")).ok().and_then(|t| package_name(&t)).unwrap_or_else(|| project.to_string());
    new_file(folder, "k2/ci/build-linux.sh", &ci::build_linux_sh(), p);
    new_file(folder, "k2/ci/build-macos.sh", &ci::build_macos_sh(), p);
    let toml = ci::ci_toml(project, f);
    let ci_path = folder.join(".k2/ci.toml");
    match std::fs::read_to_string(&ci_path) {
        Ok(t) if !t.contains("[release]") => {
            let section = toml.split_once("\n[release]\n").map(|(_, rest)| format!("[release]\n{rest}")).unwrap_or_default();
            p.files.push(FilePlan {
                path: ".k2/ci.toml".into(),
                action: Action::Snippet,
                note: "already there without [release]: K2 doesn't edit it; add this section (k2 study ci-release)".into(),
                content: section,
                before: None,
            });
        }
        _ => new_file(folder, ".k2/ci.toml", &toml, p),
    }
    new_file(folder, ".k2/signer.toml", &ci::signer_toml(project, &bin, f), p);
    if f.owner.is_none() {
        p.notes.push(format!(
            ".k2/ci.toml names no seat yet (owner = \"{}\"): put your node owner's host there once this server holds a seat (k2 compute seats), or k2 ci release refuses ci_no_seat",
            ci::PLACEHOLDER_OWNER
        ));
    }
    if f.controller_fp.is_none() || f.owner_key_fp.is_none() {
        p.notes.push(".k2/signer.toml has all-zero fingerprints where this server's or the owner's aren't known here: fill them in before a person approves it at the signer".into());
    }
}

/// `[package] name` of a `Cargo.toml` (the binary the bundle carries).
fn package_name(cargo_toml: &str) -> Option<String> {
    let v: toml::Value = cargo_toml.parse().ok()?;
    v.get("package")?.get("name")?.as_str().map(str::to_string)
}

/// The Cargo lines K2 printed (it never edits a Cargo.toml).
const CARGO_STEP: &str =
    "add the Cargo lines printed above (Tauri 2.12.5 or newer, K2's patched wry, reqwest for k2_http.rs), then cargo update -p tauri -p wry (k2/README.md, Runtime fixes)";

/// Last: the store checklist.
const DOCTOR_STEP: &str =
    "k2 app doctor --store   (before you submit to the App Store or Play: the privacy manifest, usage strings, export answer, Tauri and wry; k2/README.md, Store review)";

/// The runtime fixes every Tauri App needs: `k2_http.rs`, and the Cargo
/// lines a manifest lacks (printed: K2 never edits a Cargo.toml). True when
/// it printed Cargo lines.
fn plan_runtime(folder: &Path, p: &mut Plan) -> bool {
    new_file(folder, "src-tauri/src/k2_http.rs", store::K2_HTTP_RS, p);
    let crate_rel = "src-tauri/Cargo.toml";
    let Ok(crate_toml) = std::fs::read_to_string(folder.join(crate_rel)) else {
        p.notes.push(format!(
            "no {crate_rel}: when it exists, it needs tauri {} or newer and K2's patched wry (k2/README.md, Runtime fixes; k2 app doctor --store checks)",
            store::TAURI_FLOOR_STR
        ));
        return false;
    };
    let old = tauri_older_than(&crate_toml, store::TAURI_FLOOR);
    let reqwest = !doctor::has_dependency(&crate_toml, "reqwest");
    // [patch] applies only from the workspace root.
    let root_rel = match std::fs::read_to_string(folder.join("Cargo.toml")) {
        Ok(t) if doctor::is_workspace_root(&t) => "Cargo.toml",
        _ => crate_rel,
    };
    let same = root_rel == crate_rel;
    let root_toml = if same { crate_toml.clone() } else { std::fs::read_to_string(folder.join(root_rel)).unwrap_or_default() };
    let patch = match doctor::wry_patch(&root_toml) {
        doctor::WryPatch::K2 => None,
        doctor::WryPatch::Other(row) => {
            p.notes.push(format!(
                "{root_rel} patches wry its own way ({row}): K2 leaves it. Make sure it answers IPC from the main frame only (k2/README.md, Runtime fixes)"
            ));
            None
        }
        doctor::WryPatch::Missing => Some(!root_toml.contains("[patch.crates-io]")),
    };
    let mut why = Vec::new();
    if let Some(req) = &old {
        why.push(format!("tauri {req} is older than {}", store::TAURI_FLOOR_STR));
    }
    if reqwest {
        why.push("k2_http.rs needs reqwest".to_string());
    }
    if patch.is_some() && same {
        why.push("K2's patched wry".to_string());
    }
    let here = store::cargo_snippet(if same { patch } else { None }, old.is_some(), reqwest);
    let mut printed = false;
    if here.is_empty() {
        p.files.push(FilePlan {
            path: crate_rel.into(),
            action: Action::Skipped,
            note: format!("already asks for tauri {}+ and reqwest{}", store::TAURI_FLOOR_STR, if same { ", and patches wry" } else { "" }),
            content: String::new(),
            before: None,
        });
    } else {
        printed = true;
        p.files.push(FilePlan {
            path: crate_rel.into(),
            action: Action::Snippet,
            note: format!("K2 doesn't edit a Cargo.toml: add these ({})", why.join("; ")),
            content: here,
            before: None,
        });
    }
    if let (false, Some(header)) = (same, patch) {
        printed = true;
        p.files.push(FilePlan {
            path: root_rel.into(),
            action: Action::Snippet,
            note: "your workspace root, the only place [patch] works: add K2's patched wry".into(),
            content: store::cargo_snippet(Some(header), false, false),
            before: None,
        });
    }
    printed
}

/// What every Tauri App needs for the stores: the privacy manifest, the iOS
/// keys (`voice`: and the call ones), the Android lines.
fn plan_store(folder: &Path, name: &str, voice: bool, p: &mut Plan) {
    new_file(folder, "src-tauri/PrivacyInfo.xcprivacy", &store::privacy_manifest(voice), p);
    let ios = "src-tauri/Info.ios.plist";
    let what = if voice { "iOS's call keys, scene manifest and export answer" } else { "iOS's scene manifest and export answer" };
    plist_keys(folder, ios, &store::ios_keys(name, voice), what, p);
    if voice {
        let said = std::fs::read_to_string(folder.join(ios)).ok().and_then(|t| doctor::plist_bool(&t, "ITSAppUsesNonExemptEncryption"));
        if said == Some(false) {
            p.notes.push(format!(
                "{ios} says ITSAppUsesNonExemptEncryption is false, but calls are end-to-end encrypted: set it to true (k2/README.md, Export compliance)"
            ));
        }
    }
    let android = "src-tauri/gen/android/app/src/main/AndroidManifest.xml";
    let lines = store::android_permissions(voice);
    let has = std::fs::read_to_string(folder.join(android)).ok();
    let missing: Vec<&String> = lines.iter().filter(|l| has.as_deref().is_none_or(|t| !t.contains(android_name(l)))).collect();
    if missing.is_empty() {
        p.files.push(FilePlan { path: android.into(), action: Action::Skipped, note: "already has the permissions".into(), content: String::new(), before: None });
    } else {
        let ask = if voice { "ask for notifications, the mic and the camera at run time" } else { "ask for notifications at run time" };
        p.files.push(FilePlan {
            path: android.into(),
            action: Action::Snippet,
            note: format!("tauri android init writes this file and K2 doesn't edit it: put these inside <manifest>, and {ask}"),
            content: missing.iter().map(|l| format!("{l}\n")).collect(),
            before: None,
        });
    }
}

/// `--voice`: the one change K2 leaves to the person (it never edits Rust).
const VOICE_WIRE_STEP: &str =
    "src-tauri/src/lib.rs: add `mod k2_voice;` and `.on_permission_request(k2_voice::on_permission_request)` on tauri::Builder::default() (Tauri 2.12 or newer; k2/README.md, Calls)";

/// `--voice`: the mic and camera keys, per platform.
fn plan_voice(folder: &Path, name: &str, host: &str, entitlements: Option<&str>, p: &mut Plan) {
    let mut macos = text::voice_usage_keys(name);
    macos.extend(text::voice_macos_keys());
    plist_keys(folder, "src-tauri/Info.plist", &macos, "macOS's mic and camera keys", p);
    new_file(folder, "src-tauri/src/k2_voice.rs", &text::k2_voice_rs(host), p);
    let ents = text::voice_entitlements();
    // A path inside src-tauri/ K2 may write; one outside gets the keys printed.
    let inside = entitlements.filter(|e| !Path::new(e).is_absolute() && !e.split(['/', '\\']).any(|c| c == ".."));
    match inside {
        Some(e) => {
            let rel = format!("src-tauri/{}", e.trim_start_matches("./"));
            plist_keys(folder, &rel, &ents, "the macOS hardened runtime's mic and camera entitlements", p);
        }
        None => {
            let shown = entitlements.unwrap_or("your entitlements file");
            p.files.push(FilePlan {
                path: shown.to_string(),
                action: Action::Snippet,
                note: "bundle.macOS.entitlements names this file, and K2 doesn't write outside src-tauri/: add these by hand".into(),
                content: text::plist_entries(&ents),
                before: None,
            });
        }
    }
}

/// The `tauri` requirement in a `Cargo.toml` when the lowest 2.x it allows
/// is below `floor` (`"2"`, `"2.12"`, `"^2.10.1"` against 2.12.5); `None`
/// when it is new enough, absent or not plain (a repository or workspace
/// row). `"2"` allows 2.12.5 but a lock file may hold an older one: named.
pub(crate) fn tauri_older_than(cargo_toml: &str, floor: (u32, u32, u32)) -> Option<String> {
    let req = doctor::dependency_req(cargo_toml, "tauri")?;
    let v = doctor::lowest_version(&req)?;
    (v.0 == floor.0 && v < floor).then_some(req)
}

/// `android.permission.X` of a `<uses-permission>` line.
fn android_name(line: &str) -> &str {
    line.split('"').nth(1).unwrap_or(line)
}

/// A new plist with `entries`, or (it exists: never edited) the entries it
/// lacks to add by hand.
fn plist_keys(folder: &Path, rel: &str, entries: &[(String, String)], what: &str, p: &mut Plan) {
    let path = folder.join(rel);
    if !path.exists() {
        p.files.push(FilePlan { path: rel.to_string(), action: Action::Write, note: format!("new: {what}"), content: text::plist(entries), before: None });
        return;
    }
    let has = std::fs::read_to_string(&path).unwrap_or_default();
    let missing: Vec<(String, String)> = entries.iter().filter(|(k, _)| !has.contains(&format!("<key>{k}</key>"))).cloned().collect();
    if missing.is_empty() {
        p.files.push(FilePlan { path: rel.to_string(), action: Action::Skipped, note: format!("already has {what}"), content: String::new(), before: None });
    } else {
        p.files.push(FilePlan {
            path: rel.to_string(),
            action: Action::Snippet,
            note: "already there and K2 doesn't edit a plist: add these inside its <dict>".into(),
            content: text::plist_entries(&missing),
            before: None,
        });
    }
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
