//! `k2 app doctor --store` (0.46.9, the Tauri App SDK): checks a Tauri App's
//! repo for what App Store and Play review, and iOS 27, need ([`super::store`]
//! says why each one matters). Read only: it never writes.
//!
//! Each finding names the file, the problem and the fix. `error`: review
//! rejects the build, or the App crashes or makes a false declaration.
//! `warn`: a gap a reviewer or a person will hit. Platform checks run only
//! when the repo has that platform's files (`src-tauri/Info.ios.plist` or
//! `gen/apple` for iOS, `gen/android` for Android); the report says what it
//! skipped.

use std::path::{Path, PathBuf};

use serde::Serialize;

use super::store;
use super::{codes, err, TemplateError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warn,
}

/// One thing to fix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Finding {
    pub id: &'static str,
    pub level: Level,
    /// `ios`, `android`, `macos` or `all`.
    pub platform: &'static str,
    /// The file it is about, relative to the folder.
    pub path: String,
    pub problem: String,
    pub fix: String,
}

/// The whole check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub folder: String,
    /// Calls are checked too.
    pub voice: bool,
    /// Why `voice` is what it is.
    pub voice_why: String,
    /// What the lock file builds (`None`: no lock file).
    pub tauri: Option<String>,
    pub tao: Option<String>,
    /// wry's version and where it comes from.
    pub wry: Option<String>,
    pub findings: Vec<Finding>,
    /// The checks that ran and passed.
    pub passed: Vec<&'static str>,
    /// What was skipped, and why.
    pub notes: Vec<String>,
    /// No `error` finding.
    pub ok: bool,
}

/// Check `folder` (a Tauri v2 repo's root). `voice`: `None` works it out
/// from the repo (k2_voice.rs, a mic usage string, Sew's voice plugin).
pub fn check(folder: &Path, voice: Option<bool>) -> Result<Report, TemplateError> {
    let tauri_dir = folder.join("src-tauri");
    let conf = ["tauri.conf.json", "tauri.conf.json5", "Tauri.toml"].into_iter().find(|f| tauri_dir.join(f).is_file());
    if conf.is_none() {
        return Err(err(
            codes::NOT_A_TAURI_REPO,
            format!("{} has no src-tauri/tauri.conf.json (nor .json5 or Tauri.toml): run this in a Tauri v2 app's repo root, or pass the folder", folder.display()),
        ));
    }
    let read = |rel: &str| std::fs::read_to_string(folder.join(rel)).ok();
    let cargo = read("src-tauri/Cargo.toml");
    let info_mac = read("src-tauri/Info.plist");
    let info_ios = read("src-tauri/Info.ios.plist");
    let (voice, voice_why) = match voice {
        Some(true) => (true, "--voice".to_string()),
        Some(false) => (false, "--no-voice".to_string()),
        None => {
            let why = if folder.join("src-tauri/src/k2_voice.rs").is_file() {
                Some("src-tauri/src/k2_voice.rs is there")
            } else if [&info_mac, &info_ios].iter().any(|t| t.as_deref().is_some_and(|t| t.contains("NSMicrophoneUsageDescription"))) {
                Some("an Info plist asks for the microphone")
            } else if cargo.as_deref().is_some_and(|t| has_dependency(t, "tauri-plugin-sew-voice")) {
                Some("Cargo.toml has tauri-plugin-sew-voice")
            } else {
                None
            };
            match why {
                Some(w) => (true, format!("{w} (--no-voice to skip the call checks)")),
                None => (false, "no sign of calls (--voice to check them anyway)".to_string()),
            }
        }
    };
    let mut c = Checker { findings: Vec::new(), passed: Vec::new(), notes: Vec::new() };

    // ── Tauri, tao, wry ──
    let lock = ["src-tauri/Cargo.lock", "Cargo.lock"].into_iter().find_map(|f| read(f).map(|t| (f, t)));
    let locked = |name: &str| lock.as_ref().and_then(|(_, t)| locked_package(t, name));
    let tauri = locked("tauri");
    let tao = locked("tao");
    let wry = locked("wry");
    match (&cargo, &tauri) {
        (None, _) => c.add(Finding {
            id: "cargo_missing",
            level: Level::Error,
            platform: "all",
            path: "src-tauri/Cargo.toml".into(),
            problem: "there is no src-tauri/Cargo.toml".into(),
            fix: "run this in the repo root of a Tauri v2 app (npx tauri init)".into(),
        }),
        (Some(_), Some((v, _))) => match parse_version(v) {
            Some(have) if have < store::TAURI_FLOOR => c.add(tauri_too_old(&format!("Cargo.lock builds tauri {v}"), have, lock.as_ref().map(|l| l.0).unwrap_or("Cargo.lock"))),
            _ => c.pass("tauri_version"),
        },
        (Some(t), None) => match super::tauri_older_than(t, store::TAURI_FLOOR) {
            Some(req) => {
                let have = lowest_version(&req).unwrap_or((2, 0, 0));
                c.add(tauri_too_old(&format!("src-tauri/Cargo.toml asks for tauri {req}"), have, "src-tauri/Cargo.toml"))
            }
            None => {
                c.pass("tauri_version");
                c.notes.push("no Cargo.lock: the Tauri, tao and wry versions come from Cargo.toml alone (build once, then check again)".into());
            }
        },
    }
    if let Some((v, src)) = &tao {
        let from_git = src.starts_with("git+");
        match parse_version(v) {
            Some(have) if have < store::TAO_SCENE_FIXED && !from_git => c.add(Finding {
                id: "tao_scene_crash",
                level: Level::Error,
                platform: "ios",
                path: lock.as_ref().map(|l| l.0).unwrap_or("Cargo.lock").into(),
                problem: format!("tao {v} crashes at launch on iOS 27 (its scene delegate, tao#1245)"),
                fix: format!("move to Tauri {} or newer, which brings tao 0.37 (cargo update -p tauri)", store::TAURI_FLOOR_STR),
            }),
            _ => c.pass("tao_scene"),
        }
    }
    let root_rel = match read("Cargo.toml") {
        Some(t) if is_workspace_root(&t) => "Cargo.toml",
        _ => "src-tauri/Cargo.toml",
    };
    let root_patch = read(root_rel).map(|t| wry_patch(&t)).unwrap_or(WryPatch::Missing);
    let wry_k2 = match &wry {
        Some((_, src)) => src.contains("Alakazam-211/K2"),
        None => root_patch == WryPatch::K2,
    };
    if wry_k2 {
        c.pass("wry_patched");
    } else if cargo.is_some() {
        let own = match &root_patch {
            WryPatch::Other(row) => format!(" ({root_rel} patches it its own way: {row})"),
            _ => String::new(),
        };
        c.add(Finding {
            id: "wry_unpatched",
            level: Level::Warn,
            platform: "all",
            path: root_rel.into(),
            problem: format!(
                "wry is not K2's patched copy{own}: on macOS and iOS every frame of a page (an iframe too) can call your Tauri commands, and a nil URL can abort the app"
            ),
            fix: format!("add to [patch.crates-io] in {root_rel}: {} then cargo update -p wry (k2/README.md, Runtime fixes)", store::wry_patch_row()),
        });
    }

    // ── iOS ──
    let apple = folder.join("src-tauri/gen/apple");
    if info_ios.is_none() && !apple.is_dir() {
        c.notes.push("iOS skipped: no src-tauri/Info.ios.plist and no src-tauri/gen/apple (k2 app init --template tauri writes the first; npx tauri ios init the second)".into());
    } else {
        check_ios(folder, &apple, info_ios.as_deref(), voice, &mut c);
    }

    // ── macOS (calls) ──
    if voice {
        check_macos(folder, conf, info_mac.as_deref(), &mut c);
    }

    // ── Android ──
    let android_rel = "src-tauri/gen/android/app/src/main/AndroidManifest.xml";
    match read(android_rel) {
        None => c.notes.push("Android skipped: no src-tauri/gen/android yet (npx tauri android init)".into()),
        Some(m) => check_android(android_rel, &m, voice, &mut c),
    }

    let ok = !c.findings.iter().any(|f| f.level == Level::Error);
    let show = |p: &Option<(String, String)>| p.as_ref().map(|(v, s)| if s.is_empty() { v.clone() } else { format!("{v} ({})", source_word(s)) });
    Ok(Report {
        folder: folder.display().to_string(),
        voice,
        voice_why,
        tauri: tauri.map(|t| t.0),
        tao: show(&tao),
        wry: show(&wry),
        findings: c.findings,
        passed: c.passed,
        notes: c.notes,
        ok,
    })
}

struct Checker {
    findings: Vec<Finding>,
    passed: Vec<&'static str>,
    notes: Vec<String>,
}

impl Checker {
    fn add(&mut self, f: Finding) {
        self.findings.push(f);
    }
    fn pass(&mut self, id: &'static str) {
        self.passed.push(id);
    }
}

fn tauri_too_old(what: &str, have: (u32, u32, u32), path: &str) -> Finding {
    let mut why = Vec::new();
    if have < store::TAURI_DEADLOCK_FIXED {
        why.push("the 2.10 deadlock");
    }
    if have < (2, 12, 0) {
        why.push("a launch crash on iOS 27 (tao before 0.37)");
    }
    if have < (2, 12, 4) {
        why.push("an IPC request with an empty path aborts the app (tauri#16197)");
    }
    if why.is_empty() {
        why.push("the release K2 tests Apps on");
    }
    Finding {
        id: "tauri_too_old",
        level: Level::Error,
        platform: "all",
        path: path.into(),
        problem: format!("{what}: {}", why.join("; ")),
        fix: format!("tauri = \"{}\" in src-tauri/Cargo.toml, then cargo update -p tauri", store::TAURI_FLOOR_STR),
    }
}

/// `registry+…` or `git+…` as a word.
fn source_word(src: &str) -> String {
    if src.contains("Alakazam-211/K2") {
        "K2's patched copy".into()
    } else if src.starts_with("registry+") {
        "crates.io".into()
    } else if let Some(g) = src.strip_prefix("git+") {
        g.split('#').next().unwrap_or(g).to_string()
    } else {
        src.to_string()
    }
}

fn check_ios(folder: &Path, apple: &Path, info_ios: Option<&str>, voice: bool, c: &mut Checker) {
    let rel = |p: &Path| p.strip_prefix(folder).unwrap_or(p).display().to_string().replace('\\', "/");
    // The privacy manifest.
    let manifests = find_files(&folder.join("src-tauri"), "PrivacyInfo.xcprivacy");
    match manifests.first() {
        None => c.add(Finding {
            id: "privacy_manifest_missing",
            level: Level::Error,
            platform: "ios",
            path: "src-tauri/PrivacyInfo.xcprivacy".into(),
            problem: "no privacy manifest: App Store Connect rejects an upload whose app uses APIs that need a reason (every Rust app reads file metadata) without one".into(),
            fix: "k2 app init --template tauri writes src-tauri/PrivacyInfo.xcprivacy (it never overwrites a file); then add it to the iOS target (k2/README.md, Store review)".into(),
        }),
        Some(m) => {
            let t = std::fs::read_to_string(m).unwrap_or_default();
            let mut need = vec![("NSPrivacyAccessedAPICategoryFileTimestamp", "C617.1", "file timestamps (Rust's std reads file metadata)", Level::Error)];
            if voice {
                need.push(("NSPrivacyAccessedAPICategorySystemBootTime", "35F9.1", "system boot time (the native call plugin's audio)", Level::Warn));
            }
            let mut all = true;
            for (cat, reason, what, level) in need {
                if !t.contains(cat) {
                    all = false;
                    c.add(Finding {
                        id: "privacy_reason_missing",
                        level,
                        platform: "ios",
                        path: rel(m),
                        problem: format!("the privacy manifest doesn't declare {what}"),
                        fix: format!("add a dict to NSPrivacyAccessedAPITypes: NSPrivacyAccessedAPIType {cat}, reasons [{reason}] (k2 app init --template tauri --dry-run shows the whole file)"),
                    });
                }
            }
            if all {
                c.pass("privacy_manifest");
            }
            // Tauri doesn't copy it into the app: the Xcode target must.
            if apple.is_dir() {
                let in_apple = manifests.iter().any(|p| p.starts_with(apple));
                let named = ["project.yml"].iter().map(|f| apple.join(f)).chain(find_files(apple, "project.pbxproj")).any(|p| {
                    std::fs::read_to_string(&p).is_ok_and(|t| t.contains("PrivacyInfo.xcprivacy"))
                });
                if in_apple || named {
                    c.pass("privacy_manifest_in_app");
                } else {
                    c.add(Finding {
                        id: "privacy_manifest_not_in_app",
                        level: Level::Error,
                        platform: "ios",
                        path: "src-tauri/gen/apple/project.yml".into(),
                        problem: format!("{} isn't in the iOS target, so the app ships without it (Tauri doesn't copy it in)", rel(m)),
                        fix: "under your <app>_iOS target's sources: in src-tauri/gen/apple/project.yml add  - path: ../../PrivacyInfo.xcprivacy  with  buildPhase: resources  (or add it to the target in Xcode)".into(),
                    });
                }
            } else {
                c.notes.push("no src-tauri/gen/apple yet: after npx tauri ios init, run this again to check the privacy manifest is in the iOS target".into());
            }
        }
    }
    // The Info keys: Info.ios.plist, and what tauri ios init generated.
    let mut texts: Vec<(String, String)> = Vec::new();
    if let Some(t) = info_ios {
        texts.push(("src-tauri/Info.ios.plist".into(), t.to_string()));
    }
    if apple.is_dir() {
        for p in find_files(apple, "Info.plist").into_iter().chain(std::iter::once(apple.join("project.yml"))) {
            // An app extension's Info.plist isn't the app's.
            if p.components().any(|c| c.as_os_str().to_string_lossy().contains("Extension") || c.as_os_str().to_string_lossy().contains("Service")) {
                continue;
            }
            if let Ok(t) = std::fs::read_to_string(&p) {
                texts.push((rel(&p), t));
            }
        }
    }
    let has = |key: &str| texts.iter().any(|(_, t)| t.contains(&format!("<key>{key}</key>")) || t.contains(&format!("{key}:")));
    let ios_path = "src-tauri/Info.ios.plist".to_string();
    if has("UIApplicationSceneManifest") {
        c.pass("ios_scene_manifest");
    } else {
        c.add(Finding {
            id: "ios_scene_manifest_missing",
            level: Level::Error,
            platform: "ios",
            path: ios_path.clone(),
            problem: "no UIApplicationSceneManifest: an app built with the iOS 27 SDK must use the scene lifecycle, or it won't launch".into(),
            fix: "add UIApplicationSceneManifest to src-tauri/Info.ios.plist (k2 app init --template tauri --dry-run prints it), with Tauri 2.12.5 or newer".into(),
        });
    }
    let export: Option<bool> = texts.iter().find_map(|(_, t)| plist_bool(t, "ITSAppUsesNonExemptEncryption").or_else(|| yaml_bool(t, "ITSAppUsesNonExemptEncryption")));
    match (export, voice) {
        (None, _) => c.add(Finding {
            id: "export_answer_missing",
            level: Level::Warn,
            platform: "ios",
            path: ios_path.clone(),
            problem: "no ITSAppUsesNonExemptEncryption: App Store Connect holds every build for the export question".into(),
            fix: format!("add ITSAppUsesNonExemptEncryption to src-tauri/Info.ios.plist: {} (k2/README.md, Export compliance)", if voice { "<true/> (calls are end-to-end encrypted)" } else { "<false/> if HTTPS is the App's only encryption" }),
        }),
        (Some(false), true) => c.add(Finding {
            id: "export_answer_wrong",
            level: Level::Error,
            platform: "ios",
            path: ios_path.clone(),
            problem: "ITSAppUsesNonExemptEncryption is false, but calls are end-to-end encrypted: that is a false export declaration".into(),
            fix: "set it to <true/>, answer App Store Connect's questions as standard encryption, and see k2/README.md, Export compliance (the US annual report, France's ANSSI declaration)".into(),
        }),
        _ => c.pass("export_answer"),
    }
    if voice {
        let mut usage_ok = true;
        for (key, what) in [("NSMicrophoneUsageDescription", "microphone"), ("NSCameraUsageDescription", "camera")] {
            if !has(key) {
                usage_ok = false;
                c.add(Finding {
                    id: "usage_string_missing",
                    level: Level::Error,
                    platform: "ios",
                    path: ios_path.clone(),
                    problem: format!("no {key}: iOS ends the app the moment a call asks for the {what}"),
                    fix: format!("add {key} to src-tauri/Info.ios.plist with a sentence saying why (Apple shows it to people)"),
                });
            }
        }
        if usage_ok {
            c.pass("ios_usage_strings");
        }
        let audio = texts.iter().any(|(_, t)| t.contains("UIBackgroundModes") && (t.contains("<string>audio</string>") || t.contains("- audio")));
        if audio {
            c.pass("ios_background_audio");
        } else {
            c.add(Finding {
                id: "background_audio_missing",
                level: Level::Error,
                platform: "ios",
                path: ios_path,
                problem: "UIBackgroundModes lacks audio: a call stops when the person leaves the app".into(),
                fix: "add UIBackgroundModes [audio] to src-tauri/Info.ios.plist; add voip only with PushKit and CallKit".into(),
            });
        }
    }
}

fn check_macos(folder: &Path, conf: Option<&str>, info_mac: Option<&str>, c: &mut Checker) {
    let mut ok = true;
    for key in ["NSMicrophoneUsageDescription", "NSCameraUsageDescription"] {
        if !info_mac.is_some_and(|t| t.contains(&format!("<key>{key}</key>"))) {
            ok = false;
            c.add(Finding {
                id: "usage_string_missing",
                level: Level::Error,
                platform: "macos",
                path: "src-tauri/Info.plist".into(),
                problem: format!("no {key}: without it WKWebView has no navigator.mediaDevices, so no call"),
                fix: format!("add {key} to src-tauri/Info.plist (k2 app init --template tauri --voice prints the keys it lacks)"),
            });
        }
    }
    // The entitlements file tauri.conf.json names (default ./Entitlements.plist).
    let named = (conf == Some("tauri.conf.json"))
        .then(|| std::fs::read_to_string(folder.join("src-tauri/tauri.conf.json")).ok())
        .flatten()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .and_then(|v| v["bundle"]["macOS"]["entitlements"].as_str().map(str::to_string));
    let ent_rel = format!("src-tauri/{}", named.as_deref().unwrap_or("./Entitlements.plist").trim_start_matches("./"));
    let ent = std::fs::read_to_string(folder.join(&ent_rel)).ok();
    if ent.as_deref().is_some_and(|t| t.contains("com.apple.security.device.audio-input")) {
        if ok {
            c.pass("macos_call_keys");
        }
    } else {
        c.add(Finding {
            id: "entitlement_missing",
            level: Level::Error,
            platform: "macos",
            path: ent_rel,
            problem: "no com.apple.security.device.audio-input entitlement: a signed, notarised build can't open the microphone".into(),
            fix: "add com.apple.security.device.audio-input and .camera, and name the file in bundle.macOS.entitlements (k2 app init --template tauri --voice)".into(),
        });
    }
}

fn check_android(path: &str, manifest: &str, voice: bool, c: &mut Checker) {
    let has = |p: &str| manifest.contains(&format!("android.permission.{p}\""));
    let mut want: Vec<(&str, Level, &str)> = vec![("POST_NOTIFICATIONS", Level::Warn, "Android 13+ shows no notification without it")];
    if voice {
        want.extend([
            ("RECORD_AUDIO", Level::Error, "a call can't open the microphone"),
            ("CAMERA", Level::Warn, "a video call can't open the camera"),
            ("FOREGROUND_SERVICE_MICROPHONE", Level::Warn, "a call stops when the App goes to the background (it needs a microphone foreground service)"),
        ]);
    }
    let mut ok = true;
    for (perm, level, why) in want {
        if !has(perm) {
            ok = false;
            c.add(Finding {
                id: "android_permission_missing",
                level,
                platform: "android",
                path: path.into(),
                problem: format!("no {perm}: {why}"),
                fix: format!("add <uses-permission android:name=\"android.permission.{perm}\" /> inside <manifest>"),
            });
        }
    }
    let fgs = manifest.contains("android.permission.FOREGROUND_SERVICE_");
    if fgs && !manifest.contains("foregroundServiceType") {
        ok = false;
        c.add(Finding {
            id: "android_service_missing",
            level: Level::Warn,
            platform: "android",
            path: path.into(),
            problem: "foreground-service permissions but no <service> with android:foregroundServiceType: Play rejects types the App doesn't use".into(),
            fix: "add the call service (from your call plugin) with foregroundServiceType microphone|camera, or remove the unused permissions; Play's declaration needs a video of each type in use".into(),
        });
    }
    if ok {
        c.pass("android_permissions");
    }
}

/// Files named `name` under `dir` (not in build output), nearest first.
fn find_files(dir: &Path, name: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![(dir.to_path_buf(), 0u32)];
    while let Some((d, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        let mut entries: Vec<_> = rd.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let n = e.file_name().to_string_lossy().to_string();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < 6 && !matches!(n.as_str(), "target" | "node_modules" | "build" | "Pods" | ".git" | "Externals") && !n.ends_with(".xcarchive") {
                    stack.push((p, depth + 1));
                }
            } else if n == name {
                out.push(p);
            }
        }
    }
    out.sort_by_key(|p| p.components().count());
    out
}

// ── small parsers, shared with the template ──

/// `<key>KEY</key>` followed by `<true/>` or `<false/>`.
pub fn plist_bool(text: &str, key: &str) -> Option<bool> {
    let at = text.find(&format!("<key>{key}</key>"))?;
    let rest = text[at..].split_once("</key>")?.1.trim_start();
    if rest.starts_with("<true/>") {
        Some(true)
    } else if rest.starts_with("<false/>") {
        Some(false)
    } else {
        None
    }
}

/// `KEY: true` in an XcodeGen project.yml.
fn yaml_bool(text: &str, key: &str) -> Option<bool> {
    text.lines().find_map(|l| {
        let v = l.trim().strip_prefix(key)?.strip_prefix(':')?.trim().trim_matches('"').to_ascii_lowercase();
        match v.as_str() {
            "true" | "yes" => Some(true),
            "false" | "no" => Some(false),
            _ => None,
        }
    })
}

/// Every dependency table of a parsed Cargo.toml (`[dependencies]`,
/// `[target.*.dependencies]`; not dev or build).
fn dependency_tables(v: &toml::Value) -> Vec<&toml::value::Table> {
    let mut out: Vec<&toml::value::Table> = v.get("dependencies").and_then(toml::Value::as_table).into_iter().collect();
    if let Some(t) = v.get("target").and_then(toml::Value::as_table) {
        for spec in t.values() {
            if let Some(d) = spec.get("dependencies").and_then(toml::Value::as_table) {
                out.push(d);
            }
        }
    }
    out
}

/// `name`'s version requirement in a Cargo.toml (`None`: absent, or a
/// repository, path or workspace row).
pub fn dependency_req(cargo_toml: &str, name: &str) -> Option<String> {
    let v: toml::Value = toml::from_str(cargo_toml).ok()?;
    dependency_tables(&v).into_iter().find_map(|t| match t.get(name)? {
        toml::Value::String(s) => Some(s.clone()),
        toml::Value::Table(row) if row.contains_key("git") || row.contains_key("path") || row.contains_key("workspace") => None,
        toml::Value::Table(row) => row.get("version").and_then(toml::Value::as_str).map(str::to_string),
        _ => None,
    })
}

/// `name` is a dependency of any kind.
pub fn has_dependency(cargo_toml: &str, name: &str) -> bool {
    toml::from_str::<toml::Value>(cargo_toml).is_ok_and(|v| dependency_tables(&v).iter().any(|t| t.contains_key(name)))
}

/// A Cargo.toml with a `[workspace]` table.
pub fn is_workspace_root(cargo_toml: &str) -> bool {
    toml::from_str::<toml::Value>(cargo_toml).is_ok_and(|v| v.get("workspace").is_some())
}

/// What a workspace root's `[patch.crates-io]` does with wry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WryPatch {
    /// K2's patched copy.
    K2,
    /// Some other patch (the row as written).
    Other(String),
    Missing,
}

pub fn wry_patch(cargo_toml: &str) -> WryPatch {
    let Ok(v) = toml::from_str::<toml::Value>(cargo_toml) else { return WryPatch::Missing };
    match v.get("patch").and_then(|p| p.get("crates-io")).and_then(|p| p.get("wry")) {
        None => WryPatch::Missing,
        Some(row) if row.get("git").and_then(toml::Value::as_str).is_some_and(|g| g.contains("Alakazam-211/K2")) => WryPatch::K2,
        Some(row) => WryPatch::Other(row.to_string()),
    }
}

/// The lowest version a requirement allows (`"2.12"` is 2.12.0; `"2"` is
/// 2.0.0); `None` when it isn't a plain lower bound.
pub fn lowest_version(req: &str) -> Option<(u32, u32, u32)> {
    let r = semver::VersionReq::parse(req.trim()).ok()?;
    let c = r.comparators.first()?;
    use semver::Op;
    if !matches!(c.op, Op::Caret | Op::Tilde | Op::Exact | Op::GreaterEq | Op::Wildcard) {
        return None;
    }
    Some((u32::try_from(c.major).ok()?, c.minor.unwrap_or(0) as u32, c.patch.unwrap_or(0) as u32))
}

/// `"2.12.5"` → (2, 12, 5); a pre-release counts as its release.
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = semver::Version::parse(v.trim()).ok()?;
    Some((u32::try_from(v.major).ok()?, u32::try_from(v.minor).ok()?, u32::try_from(v.patch).ok()?))
}

/// `(version, source)` of the first `name` in a Cargo.lock (source empty
/// for a path package).
fn locked_package(lock: &str, name: &str) -> Option<(String, String)> {
    let v: toml::Value = toml::from_str(lock).ok()?;
    v.get("package")?.as_array()?.iter().find_map(|p| {
        (p.get("name")?.as_str()? == name)
            .then(|| (p.get("version").and_then(toml::Value::as_str).unwrap_or("").to_string(), p.get("source").and_then(toml::Value::as_str).unwrap_or("").to_string()))
    })
}

