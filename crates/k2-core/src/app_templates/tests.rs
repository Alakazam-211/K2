//! TC-13: each template on a fixture repo; a second run writes nothing;
//! JSON5 and TOML configs get the snippet; a non-Tauri folder is refused;
//! the files written validate as Apps.

use super::*;

/// A temp folder, removed on drop.
struct Dir(PathBuf);

impl Dir {
    fn new(tag: &str) -> Self {
        let p = crate::test_env::unique_temp_path(tag).join("my-tauri-app");
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
    fn write(&self, rel: &str, body: &str) {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }
    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.0.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.0.parent().unwrap());
    }
}

const ANDROID: &str = "src-tauri/gen/android/app/src/main/AndroidManifest.xml";

const CONF: &str = "{\n  \"productName\": \"My App\",\n  \"identifier\": \"com.you.app\",\n  \"bundle\": {\n    \"active\": true\n  }\n}\n";

fn actions(p: &Plan) -> Vec<(String, Action)> {
    p.files.iter().map(|f| (f.path.clone(), f.action.clone())).collect()
}

#[test]
fn tauri_writes_the_k2_side_once_and_never_overwrites() {
    let d = Dir::new("tpl-tauri");
    d.write("src-tauri/tauri.conf.json", CONF);
    d.write(".k2/compute.toml", "[presets.daemon]\nmem = \"7G\"\n");
    d.write("k2/README.md", "my own notes\n");
    let p = plan(&d.0, "tauri", None, Some("box")).expect("plan");
    assert_eq!(p.name, "my-tauri-app");
    assert_eq!(
        actions(&p),
        vec![
            ("k2/web.manifest.json".to_string(), Action::Write),
            ("src-tauri/tauri.conf.json".to_string(), Action::Edit),
            ("k2/README.md".to_string(), Action::Skipped),
            ("src-tauri/src/k2_http.rs".to_string(), Action::Write),
            ("src-tauri/PrivacyInfo.xcprivacy".to_string(), Action::Write),
            ("src-tauri/Info.ios.plist".to_string(), Action::Write),
            (ANDROID.to_string(), Action::Snippet),
            (".k2/compute.toml".to_string(), Action::Edit),
            ("k2/ci/build-linux.sh".to_string(), Action::Write),
            ("k2/ci/build-macos.sh".to_string(), Action::Write),
            (".k2/ci.toml".to_string(), Action::Write),
            (".k2/signer.toml".to_string(), Action::Write),
        ]
    );
    assert!(p.notes.iter().any(|n| n.contains("no src-tauri/Cargo.toml")), "no Cargo.toml: said so: {:?}", p.notes);
    assert!(p.next.last().unwrap().starts_with("k2 app doctor --store"), "{:?}", p.next);
    assert!(p.files[2].note.contains("different"), "a diff hint: {}", p.files[2].note);
    assert!(p.next.iter().any(|n| n.contains("k2 app deploy my-tauri-app-web dist --manifest k2/web.manifest.json")), "{:?}", p.next);
    // A dry run is the plan alone: nothing written yet.
    assert!(!d.0.join("k2/web.manifest.json").exists());
    let shown = plan_json(&p, true);
    assert!(shown["files"][0]["content"].as_str().unwrap().contains("\"mode\": \"static\""));
    assert!(plan_json(&p, false)["files"][0].get("content").is_none(), "content only on a dry run");
    let a = apply(&d.0, &p).expect("apply");
    assert_eq!(
        a.written,
        [
            "k2/web.manifest.json",
            "src-tauri/src/k2_http.rs",
            "src-tauri/PrivacyInfo.xcprivacy",
            "src-tauri/Info.ios.plist",
            "k2/ci/build-linux.sh",
            "k2/ci/build-macos.sh",
            ".k2/ci.toml",
            ".k2/signer.toml"
        ]
    );
    assert_eq!(a.edited, ["src-tauri/tauri.conf.json", ".k2/compute.toml"]);
    assert_eq!(d.read("k2/README.md"), "my own notes\n", "never overwritten");
    let conf: J = serde_json::from_str(&d.read("src-tauri/tauri.conf.json")).unwrap();
    assert_eq!(
        conf["plugins"]["updater"]["endpoints"][0],
        "https://my-tauri-app-web.box.k2.dev/_k2/updates/stable/{{target}}/{{arch}}/{{current_version}}"
    );
    assert_eq!(conf["plugins"]["updater"]["pubkey"], "");
    assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
    assert!(d.read("src-tauri/tauri.conf.json").starts_with("{\n  \"productName\": \"My App\",\n  \"identifier\""), "order kept");
    assert!(d.read(".k2/compute.toml").starts_with("[presets.daemon]\nmem = \"7G\"\n"));
    assert!(d.read(".k2/compute.toml").contains("[presets.tauri-linux]"));
    // A second run writes nothing (the Android lines are still printed:
    // gen/android doesn't exist yet).
    let p2 = plan(&d.0, "tauri", None, Some("box")).expect("plan again");
    assert!(p2.files.iter().all(|f| f.action == Action::Skipped || f.path == ANDROID), "{:?}", actions(&p2));
    assert!(p2.files.iter().find(|f| f.path == "k2/web.manifest.json").unwrap().note.contains("the same as the template"));
    let a2 = apply(&d.0, &p2).unwrap();
    assert!(a2.written.is_empty() && a2.edited.is_empty());
    // An edit planned against a file that changed since is refused.
    let d3 = Dir::new("tpl-race");
    d3.write("src-tauri/tauri.conf.json", CONF);
    let p3 = plan(&d3.0, "tauri", Some("race"), None).unwrap();
    d3.write("src-tauri/tauri.conf.json", "{}\n");
    assert_eq!(apply(&d3.0, &p3).unwrap_err().code, codes::CHANGED_UNDER_US);
    assert!(p3.notes[0].contains("<your-app-host>"), "no Connect name: said so");
}

#[test]
fn json5_and_toml_configs_get_a_snippet_and_a_non_tauri_folder_is_refused() {
    let d = Dir::new("tpl-toml");
    d.write("src-tauri/Tauri.toml", "[package]\nproduct-name = \"x\"\n");
    let p = plan(&d.0, "tauri", Some("Toml App!"), Some("box")).unwrap();
    assert_eq!(p.name, "toml-app");
    let f = p.files.iter().find(|f| f.path == "src-tauri/Tauri.toml").expect("the toml");
    assert_eq!(f.action, Action::Snippet);
    assert!(f.content.contains("[plugins.updater]") && f.content.contains("create-updater-artifacts = true"), "{}", f.content);
    assert!(plan_json(&p, false)["files"][1]["content"].as_str().is_some(), "a snippet is always shown");
    apply(&d.0, &p).unwrap();
    assert_eq!(d.read("src-tauri/Tauri.toml"), "[package]\nproduct-name = \"x\"\n", "TOML is never edited");
    let d5 = Dir::new("tpl-json5");
    d5.write("src-tauri/tauri.conf.json5", "{ productName: 'x' }");
    let p = plan(&d5.0, "tauri", None, None).unwrap();
    assert_eq!(p.files[1].action, Action::Snippet);
    let none = Dir::new("tpl-none");
    assert_eq!(plan(&none.0, "tauri", None, None).unwrap_err().code, codes::NOT_A_TAURI_REPO);
    assert_eq!(plan(&none.0, "rails", None, None).unwrap_err().code, codes::TEMPLATE_UNKNOWN);
    assert_eq!(plan(&none.0, "static", Some("--"), None).unwrap_err().code, codes::NAME_INVALID);
}

fn jobs_app() -> ExistingApp {
    ExistingApp {
        app_id: "jobs".into(),
        host: Some("jobs.example.com".into()),
        fallback_endpoint: Some("https://dl.k2.dev/dev.k2.box.jobs/stable/latest.json".into()),
        dl_id: Some("dev.k2.box.jobs".into()),
    }
}

/// 0.46.8: a shell over a server App already on the box: no web manifest,
/// the feed on that App's host, the download host second, no `-web` or
/// `app install` anywhere. Never overwrites, a second run writes nothing.
#[test]
fn app_wires_the_shell_to_an_existing_app_and_skips_the_web_manifest() {
    let d = Dir::new("tpl-existing");
    d.write("src-tauri/tauri.conf.json", CONF);
    let e = jobs_app();
    let o = Options { existing: Some(&e), ..Options::default() };
    let p = plan_with(&d.0, "tauri", &o).expect("plan");
    assert_eq!(p.name, "jobs");
    assert!(p.files.iter().all(|f| f.path != "k2/web.manifest.json"), "{:?}", actions(&p));
    let next = p.next.join("\n");
    assert!(!next.contains("-web") && !next.contains("k2 app install") && !next.contains("k2 app deploy"), "{next}");
    assert!(next.contains("k2 app updates token jobs --file") && next.contains("dev.k2.box.jobs"), "the token step: {next}");
    assert!(next.contains("https://jobs.example.com/"), "the window's URL: {next}");
    apply(&d.0, &p).expect("apply");
    let conf: J = serde_json::from_str(&d.read("src-tauri/tauri.conf.json")).expect("still JSON");
    assert_eq!(
        conf["plugins"]["updater"]["endpoints"],
        json!(["https://jobs.example.com/_k2/updates/stable/{{target}}/{{arch}}/{{current_version}}", "https://dl.k2.dev/dev.k2.box.jobs/stable/latest.json"])
    );
    assert_eq!(conf["plugins"]["updater"]["pubkey"], "");
    assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
    let readme = d.read("k2/README.md");
    assert!(!readme.contains("-web") && !readme.contains("k2 app install"), "{readme}");
    assert!(readme.contains("k2 app updates token jobs --file") && readme.contains("latest.json"), "{readme}");
    assert!(!d.0.join("k2/web.manifest.json").exists());
    let p2 = plan_with(&d.0, "tauri", &o).expect("plan again");
    assert!(p2.files.iter().all(|f| f.action == Action::Skipped || f.path == ANDROID), "second run: {:?}", actions(&p2));
    assert!(p2.files[0].note.contains("jobs.example.com"), "it says which endpoints to check: {}", p2.files[0].note);

    // No public host yet: a placeholder, and how to get one.
    let d2 = Dir::new("tpl-existing-nohost");
    d2.write("src-tauri/tauri.conf.json", CONF);
    let e = ExistingApp { host: None, ..jobs_app() };
    let p = plan_with(&d2.0, "tauri", &Options { existing: Some(&e), ..Options::default() }).unwrap();
    assert!(p.notes[0].contains("<your-app-host>") && p.notes[0].contains("k2 app domain add jobs"), "{:?}", p.notes);

    // Refused: another template, or --name beside it.
    let e = jobs_app();
    let o = Options { existing: Some(&e), ..Options::default() };
    assert_eq!(plan_with(&d.0, "static", &o).unwrap_err().code, codes::TAURI_ONLY);
    assert_eq!(plan_with(&d.0, "node-api", &o).unwrap_err().code, codes::TAURI_ONLY);
    assert_eq!(plan_with(&d.0, "tauri", &Options { name: Some("x"), ..o }).unwrap_err().code, codes::TAURI_ONLY);
    // A folder with no Tauri config: the same refusal as ever.
    let none = Dir::new("tpl-existing-none");
    assert_eq!(plan_with(&none.0, "tauri", &o).unwrap_err().code, codes::NOT_A_TAURI_REPO);
}

/// The plain template still names the token step (it was missing).
#[test]
fn the_plain_template_says_the_download_token_step() {
    let d = Dir::new("tpl-token");
    d.write("src-tauri/tauri.conf.json", CONF);
    let p = plan(&d.0, "tauri", None, Some("box")).unwrap();
    assert!(p.next.iter().any(|n| n.contains("k2 app updates token my-tauri-app-web --file")), "{:?}", p.next);
    apply(&d.0, &p).unwrap();
    assert!(d.read("k2/README.md").contains("k2 app updates token my-tauri-app-web --file"));
}

/// 0.46.8 `--voice`: the mic and camera keys on each platform, never over
/// a plist that is there.
#[test]
fn voice_writes_the_keys_and_prints_what_it_wont_edit() {
    let d = Dir::new("tpl-voice");
    d.write("src-tauri/tauri.conf.json", CONF);
    let o = Options { voice: true, sub: Some("box"), ..Options::default() };
    let p = plan_with(&d.0, "tauri", &o).unwrap();
    let android = ANDROID;
    let got = actions(&p);
    for (path, want) in [
        ("src-tauri/Info.plist", Action::Write),
        ("src-tauri/Info.ios.plist", Action::Write),
        ("src-tauri/Entitlements.plist", Action::Write),
        ("src-tauri/tauri.conf.json", Action::Edit),
        (android, Action::Snippet),
    ] {
        assert!(got.iter().any(|(f, a)| f == path && *a == want), "{path}: {got:?}");
    }
    let a = p.files.iter().find(|f| f.path == android).unwrap();
    for perm in ["RECORD_AUDIO", "MODIFY_AUDIO_SETTINGS", "CAMERA", "POST_NOTIFICATIONS", "FOREGROUND_SERVICE", "FOREGROUND_SERVICE_MICROPHONE", "FOREGROUND_SERVICE_CAMERA"] {
        assert!(a.content.contains(&format!("android.permission.{perm}\"")), "{perm}: {}", a.content);
    }
    assert!(!a.content.contains("PHONE_CALL") && !a.content.contains("MEDIA_PROJECTION"), "only with a Telecom service or screen share: {}", a.content);
    apply(&d.0, &p).unwrap();
    for f in ["src-tauri/Info.plist", "src-tauri/Info.ios.plist"] {
        let t = d.read(f);
        assert!(t.starts_with("<?xml") && t.contains("<key>NSMicrophoneUsageDescription</key>") && t.contains("<key>NSCameraUsageDescription</key>"), "{f}: {t}");
    }
    assert!(d.read("src-tauri/Info.plist").contains("<key>NSAppSleepDisabled</key>\n  <true/>"), "macOS: no App Nap");
    let ios = d.read("src-tauri/Info.ios.plist");
    assert!(!ios.contains("NSAppSleepDisabled"), "a macOS key");
    assert_eq!(doctor::plist_bool(&ios, "ITSAppUsesNonExemptEncryption"), Some(true), "calls are end-to-end encrypted: {ios}");
    assert!(ios.contains("<key>UIBackgroundModes</key>\n  <array>\n    <string>audio</string>\n  </array>"), "{ios}");
    assert!(!ios.contains("<string>voip</string>"), "voip only with PushKit");
    assert!(ios.contains("<key>UIApplicationSceneManifest</key>") && ios.contains("<string>TaoSceneDelegate</string>"), "{ios}");
    let privacy = d.read("src-tauri/PrivacyInfo.xcprivacy");
    assert!(privacy.contains("NSPrivacyAccessedAPICategoryFileTimestamp") && privacy.contains("NSPrivacyAccessedAPICategorySystemBootTime"), "{privacy}");
    let t = d.read("src-tauri/Entitlements.plist");
    assert!(t.contains("<key>com.apple.security.device.audio-input</key>\n  <true/>") && t.contains("<key>com.apple.security.device.camera</key>"), "{t}");
    let conf: J = serde_json::from_str(&d.read("src-tauri/tauri.conf.json")).unwrap();
    assert_eq!(conf["bundle"]["macOS"]["entitlements"], "./Entitlements.plist");
    assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
    assert_eq!(conf["bundle"]["active"], true, "kept");
    let readme = d.read("k2/README.md");
    assert!(readme.contains("## Calls") && readme.contains("wss://my-tauri-app-web.box.k2.dev") && readme.contains("'wasm-unsafe-eval'"), "{readme}");
    assert!(readme.contains("### Export compliance (calls are end-to-end encrypted)") && readme.contains("ANSSI") && readme.contains("### Calls in the store"), "{readme}");
    let rs = d.read("src-tauri/src/k2_voice.rs");
    assert!(rs.contains("pub const APP_ORIGINS: &[&str] = &[\"https://my-tauri-app-web.box.k2.dev\"];"), "{rs}");
    assert!(rs.contains(&format!("pub const WEBVIEW2_ARGS: &str = \"{}\";", text::WEBVIEW2_VOICE_ARGS)));
    // No window in this config (one made in Rust): said, and the Rust helper named.
    assert!(p.notes.iter().any(|n| n.contains("k2_voice::voice_window")), "{:?}", p.notes);
    assert!(p.next[0].contains(".on_permission_request(k2_voice::on_permission_request)"), "{:?}", p.next);

    // Again: nothing to write, the plists are left alone.
    let p2 = plan_with(&d.0, "tauri", &o).unwrap();
    assert!(p2.files.iter().all(|f| matches!(f.action, Action::Skipped | Action::Snippet)), "{:?}", actions(&p2));
    assert_eq!(p2.files.iter().filter(|f| f.action == Action::Snippet).map(|f| f.path.as_str()).collect::<Vec<_>>(), [android]);

    // A plist with one key: only the other is printed, and it stays as it is.
    let d2 = Dir::new("tpl-voice-own");
    d2.write("src-tauri/tauri.conf.json", "{\n  \"bundle\": {\n    \"macOS\": {\n      \"entitlements\": \"./mine.plist\"\n    }\n  }\n}\n");
    d2.write("src-tauri/Info.plist", "<plist><dict><key>NSMicrophoneUsageDescription</key><string>mine</string></dict></plist>");
    d2.write("src-tauri/mine.plist", "<plist><dict></dict></plist>");
    d2.write(android, "<manifest><uses-permission android:name=\"android.permission.RECORD_AUDIO\" /></manifest>");
    let p = plan_with(&d2.0, "tauri", &o).unwrap();
    let f = |path: &str| p.files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path}: {:?}", actions(&p)));
    assert_eq!(f("src-tauri/Info.plist").action, Action::Snippet);
    assert!(f("src-tauri/Info.plist").content.contains("NSCameraUsageDescription") && !f("src-tauri/Info.plist").content.contains("NSMicrophone"));
    let mine = f("src-tauri/mine.plist");
    assert_eq!(mine.action, Action::Snippet, "the file the config names");
    assert!(mine.content.contains("audio-input"));
    assert!(!p.files.iter().any(|f| f.path == "src-tauri/Entitlements.plist"), "{:?}", actions(&p));
    let a = f(android);
    assert!(!a.content.contains("RECORD_AUDIO") && a.content.contains("android.permission.CAMERA"), "{}", a.content);
    apply(&d2.0, &p).unwrap();
    assert!(d2.read("src-tauri/Info.plist").contains("<string>mine</string>") && !d2.read("src-tauri/Info.plist").contains("NSCamera"), "never edited");

    // A config that names a file outside src-tauri/: printed, never written.
    let d3 = Dir::new("tpl-voice-outside");
    d3.write("src-tauri/tauri.conf.json", "{\"bundle\":{\"macOS\":{\"entitlements\":\"../../outside.plist\"}}}");
    let p = plan_with(&d3.0, "tauri", &o).unwrap();
    let out = p.files.iter().find(|f| f.path == "../../outside.plist").expect("printed");
    assert_eq!(out.action, Action::Snippet);
    apply(&d3.0, &p).unwrap();
    assert!(!d3.0.join("../../outside.plist").exists());

    // TOML: the entitlements line is in the snippet; the plists are written.
    let d4 = Dir::new("tpl-voice-toml");
    d4.write("src-tauri/Tauri.toml", "[package]\nproduct-name = \"x\"\n");
    let p = plan_with(&d4.0, "tauri", &o).unwrap();
    let t = p.files.iter().find(|f| f.path == "src-tauri/Tauri.toml").unwrap();
    assert!(t.content.contains("[bundle.macOS]\nentitlements = \"./Entitlements.plist\""), "{}", t.content);
    assert!(p.files.iter().any(|f| f.path == "src-tauri/Entitlements.plist" && f.action == Action::Write));

    // Only for the Tauri template.
    assert_eq!(plan_with(&d.0, "static", &o).unwrap_err().code, codes::TAURI_ONLY);
}

/// C8: the CI files carry this server's facts when the daemon has them, a
/// placeholder (and a note) when it doesn't; an existing `.k2/ci.toml`
/// without `[release]` gets the section printed, never edited.
#[test]
fn tauri_writes_the_release_build_files() {
    let d = Dir::new("tpl-ci");
    d.write("src-tauri/tauri.conf.json", CONF);
    d.write("src-tauri/Cargo.toml", "[package]\nname = \"acme-shell\"\nversion = \"0.1.0\"\n\n[lib]\nname = \"acme_lib\"\n");
    let facts = ci::CiFacts {
        controller_fp: Some("a".repeat(64)),
        workspace_id: Some("ws-1".into()),
        owner: Some("owner.k2.dev".into()),
        lane: Some("ci-standard".into()),
        owner_key_fp: Some("b".repeat(64)),
        github_repo: Some("acme/shell".into()),
    };
    let o = Options { name: Some("acme"), sub: Some("box"), ci: Some(&facts), ..Options::default() };
    let p = plan_with(&d.0, "tauri", &o).unwrap();
    let file = |path: &str| p.files.iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path}: {:?}", actions(&p))).clone();
    let ci_toml = file(".k2/ci.toml");
    assert!(ci_toml.content.contains("owner = \"owner.k2.dev\"  # this server's seat") && ci_toml.content.contains("project = \"acme\""), "{}", ci_toml.content);
    let signer = file(".k2/signer.toml");
    assert!(signer.content.contains(&"a".repeat(64)) && signer.content.contains(&"b".repeat(64)) && signer.content.contains("repo = \"acme/shell\""), "{}", signer.content);
    // The binary is the package's, not the lib's.
    assert!(signer.content.contains("[assets.\"acme-shell.AppImage\"]") && signer.content.contains("\"k2/out/acme-shell\""), "{}", signer.content);
    assert!(!p.notes.iter().any(|n| n.contains("names no seat")), "{:?}", p.notes);
    assert!(file("k2/README.md").content.contains("## Release builds on K2 Compute"));
    // Unknown here: placeholders, and notes that say so.
    let p = plan_with(&d.0, "tauri", &Options { name: Some("acme"), ..Options::default() }).unwrap();
    assert!(p.notes.iter().any(|n| n.contains("names no seat")) && p.notes.iter().any(|n| n.contains("all-zero fingerprints")), "{:?}", p.notes);
    // An existing ci.toml without [release]: a snippet, the file untouched.
    d.write(".k2/ci.toml", "version = 1\n[checks.unit]\nargv = [\"true\"]\n");
    let p = plan_with(&d.0, "tauri", &o).unwrap();
    let c = p.files.iter().find(|f| f.path == ".k2/ci.toml").unwrap();
    assert_eq!(c.action, Action::Snippet);
    assert!(c.content.starts_with("[release]\n") && c.content.contains("[release.builds.linux-x86_64]"), "{}", c.content);
    let both = format!("version = 1\n[checks.unit]\nargv = [\"true\"]\n{}", c.content);
    crate::compute::proto::ci_config::CiConfig::parse(&both).expect("the printed section completes the file");
}

/// The release-build check app (`tests/app-template/tauri-ci-check`, built
/// on compute nodes by its run.sh) holds exactly the CI files the template
/// writes (with no seat known: the placeholders). `K2_WRITE_CI_CHECK=1`
/// rewrites them instead (after a template change).
#[test]
fn ci_files_match_the_checked_app() {
    const CHECK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/app-template/tauri-ci-check");
    let d = Dir::new("tpl-ci-check");
    let read = |rel: &str| std::fs::read_to_string(format!("{CHECK}/{rel}")).unwrap_or_else(|e| panic!("{rel}: {e}"));
    d.write("src-tauri/tauri.conf.json", &read("src-tauri/tauri.conf.json"));
    d.write("src-tauri/Cargo.toml", &read("src-tauri/Cargo.toml"));
    let p = plan_with(&d.0, "tauri", &Options { name: Some("k2-ci-check"), ..Options::default() }).unwrap();
    let write = std::env::var("K2_WRITE_CI_CHECK").as_deref() == Ok("1");
    for f in ["k2/ci/build-linux.sh", "k2/ci/build-macos.sh", ".k2/ci.toml", ".k2/signer.toml"] {
        let planned = p.files.iter().find(|x| x.path == f).unwrap_or_else(|| panic!("{f} not planned"));
        assert_eq!(planned.action, Action::Write, "{f}");
        if write {
            let path = format!("{CHECK}/{f}");
            std::fs::create_dir_all(std::path::Path::new(&path).parent().unwrap()).unwrap();
            std::fs::write(&path, &planned.content).unwrap();
        } else {
            assert_eq!(planned.content, read(f), "{f}: the template and the checked app differ (K2_WRITE_CI_CHECK=1 rewrites them)");
        }
    }
}

/// The check app (`tests/app-template/tauri-voice-check`, built and run on a
/// Mac by its run.sh) holds exactly what `--voice` writes, so what it proves
/// is what App builders get.
#[test]
fn voice_files_match_the_checked_app() {
    const CHECK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/app-template/tauri-voice-check/src-tauri");
    let base = r#"{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "voice-check",
  "version": "0.1.0",
  "identifier": "dev.k2.voice-check",
  "build": {
    "frontendDist": "../page"
  },
  "app": {
    "windows": [
      {
        "title": "voice-check",
        "create": false,
        "url": "http://127.0.0.1/",
        "width": 480,
        "height": 360
      }
    ],
    "security": {
      "csp": null
    }
  },
  "bundle": {
    "active": false,
    "icon": ["icons/icon.png", "icons/icon.ico"]
  }
}
"#;
    let d = Dir::new("tpl-voice-check");
    d.write("src-tauri/tauri.conf.json", base);
    let read = |rel: &str| std::fs::read_to_string(format!("{CHECK}/{rel}")).unwrap_or_else(|e| panic!("{rel}: {e}"));
    // Its real Cargo.toml: it asks for what the template asks for (Tauri
    // 2.12.5+, reqwest, K2's patched wry), so nothing is printed for it.
    d.write("src-tauri/Cargo.toml", &read("Cargo.toml"));
    let o = Options { voice: true, name: Some("voice-check"), ..Options::default() };
    let p = plan_with(&d.0, "tauri", &o).unwrap();
    let cargo = p.files.iter().find(|f| f.path == "src-tauri/Cargo.toml").expect("the Cargo.toml line");
    assert_eq!(cargo.action, Action::Skipped, "the check app carries the template's Cargo lines: {}", cargo.content);
    assert!(!p.next.iter().any(|n| n.contains("Cargo lines")), "{:?}", p.next);
    let conf = p.files.iter().find(|f| f.path == "src-tauri/tauri.conf.json").unwrap();
    assert!(conf.note.contains("app.windows[0].backgroundThrottling, app.windows[0].additionalBrowserArgs"), "{}", conf.note);
    apply(&d.0, &p).unwrap();
    for f in ["Info.plist", "Info.ios.plist", "PrivacyInfo.xcprivacy", "Entitlements.plist", "src/k2_voice.rs", "src/k2_http.rs"] {
        assert_eq!(d.read(&format!("src-tauri/{f}")), read(f), "{f}: the template and the checked app differ");
    }
    let got: J = serde_json::from_str(&d.read("src-tauri/tauri.conf.json")).unwrap();
    let want: J = serde_json::from_str(&read("tauri.conf.json")).unwrap();
    assert_eq!(got, want, "tauri.conf.json: the template and the checked app differ");
}

#[test]
fn old_tauri_is_named() {
    for (toml, want) in [
        ("tauri = \"2.11\"", Some("2.11")),
        ("tauri = { version = \"^2.10.1\", features = [] }", Some("^2.10.1")),
        ("tauri = { version = \"2\" }", Some("2")),
        ("tauri = \"2.12\"", Some("2.12")),
        ("tauri = \"2.12.4\"", Some("2.12.4")),
        ("tauri = \"2.12.5\"", None),
        ("tauri = { version = \"2.13.0\" }", None),
        ("tauri = \"3.0.0-alpha.5\"", None),
        ("tauri-build = \"2.0\"", None),
        ("tauri.workspace = true", None),
        ("tauri = { git = \"https://example.com/t\" }", None),
    ] {
        assert_eq!(super::tauri_older_than(&format!("[dependencies]\n{toml}\n"), store::TAURI_FLOOR).as_deref(), want, "{toml}");
    }
    let target = "[target.'cfg(target_os = \"ios\")'.dependencies]\ntauri = \"2.10\"\n";
    assert_eq!(super::tauri_older_than(target, store::TAURI_FLOOR).as_deref(), Some("2.10"), "a target table counts");
}

#[test]
fn static_and_node_api_files_validate_as_apps() {
    let d = Dir::new("tpl-static");
    let p = plan(&d.0, "static", Some("docs"), Some("box")).unwrap();
    assert_eq!(actions(&p), vec![("k2/web.manifest.json".to_string(), Action::Write)]);
    apply(&d.0, &p).unwrap();
    // As `k2 app deploy docs-web dist --manifest k2/web.manifest.json` packs it.
    d.write("dist/index.html", "<!doctype html><div id=app></div>");
    std::fs::copy(d.0.join("k2/web.manifest.json"), d.0.join("dist/manifest.json")).unwrap();
    let (r, m) = crate::deploy::validate::validate_tree(&d.0.join("dist"), &Default::default());
    assert!(r.ok, "{r:?}");
    assert_eq!(m.unwrap().id, "docs-web");
    let n = Dir::new("tpl-node");
    let p = plan(&n.0, "node-api", Some("jobs"), Some("box")).unwrap();
    assert_eq!(p.files.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(), ["k2/api/manifest.json", "k2/api/package.json", "k2/api/server.mjs", "k2/api/README.md"]);
    apply(&n.0, &p).unwrap();
    let server = n.read("k2/api/server.mjs");
    assert!(server.contains("16 * 1024 * 1024") && server.contains("files.uploadChunk"), "the parted-upload route (TC-27)");
    let (r, m) = crate::deploy::validate::validate_tree(&n.0.join("k2/api"), &Default::default());
    assert!(r.ok, "{r:?}");
    assert_eq!(m.unwrap().id, "jobs-api");
    assert_eq!(UPLOAD_PART_MAX, 16 << 20);
}

#[test]
fn names_become_app_ids() {
    assert_eq!(name_from("My Tauri App").as_deref(), Some("my-tauri-app"));
    assert_eq!(name_from("  __x__ ").as_deref(), Some("x"));
    assert_eq!(name_from("9lives"), None, "a letter first");
    assert_eq!(name_from("").as_deref(), None);
    assert_eq!(name_from(&"a".repeat(40)).map(|s| s.len()), Some(28), "room for -web in 32");
}

/// 0.46.9: every Tauri App gets the store files and the README's store and
/// data sections; without calls the export answer is "no" and there is no
/// background audio.
#[test]
fn tauri_writes_the_store_files_for_every_app() {
    let d = Dir::new("tpl-store");
    d.write("src-tauri/tauri.conf.json", CONF);
    let p = plan(&d.0, "tauri", None, Some("box")).unwrap();
    apply(&d.0, &p).unwrap();
    let ios = d.read("src-tauri/Info.ios.plist");
    assert_eq!(doctor::plist_bool(&ios, "ITSAppUsesNonExemptEncryption"), Some(false), "HTTPS only: {ios}");
    assert!(ios.contains("<key>UIApplicationSceneManifest</key>\n  <dict>\n    <key>UIApplicationSupportsMultipleScenes</key>\n    <true/>"), "{ios}");
    for no in ["UIBackgroundModes", "NSMicrophoneUsageDescription"] {
        assert!(!ios.contains(no), "{no} only with --voice: {ios}");
    }
    let privacy = d.read("src-tauri/PrivacyInfo.xcprivacy");
    assert!(privacy.contains("<string>NSPrivacyAccessedAPICategoryFileTimestamp</string>") && privacy.contains("<string>C617.1</string>"), "{privacy}");
    assert!(!privacy.contains("NSPrivacyAccessedAPICategorySystemBootTime"), "boot time only with calls");
    assert!(privacy.contains("<key>NSPrivacyTracking</key>\n  <false/>"), "{privacy}");
    for t in ["EmailAddress", "UserID", "DeviceID"] {
        assert!(privacy.contains(&format!("<string>NSPrivacyCollectedDataType{t}</string>")), "{t}: {privacy}");
    }
    let http = d.read("src-tauri/src/k2_http.rs");
    assert!(http.contains(".http1_only()") && http.contains("OnceLock<reqwest::Client>"), "{http}");
    let readme = d.read("k2/README.md");
    for want in [
        "## Runtime fixes (every Tauri App)",
        store::wry_patch_row().as_str(),
        "## Store review (App Store and Play)",
        "path: ../../PrivacyInfo.xcprivacy",
        "`ITSAppUsesNonExemptEncryption` is `false`",
        "## What user data your App stores and why",
        "**What you add is yours to declare.**",
        "k2 app user remove",
    ] {
        assert!(readme.contains(want), "README lacks {want}");
    }
    assert!(!readme.contains("ANSSI") && !readme.contains("### Calls in the store"), "the call sections only with --voice");
    let a = p.files.iter().find(|f| f.path == ANDROID).unwrap();
    assert_eq!(a.content, "<uses-permission android:name=\"android.permission.POST_NOTIFICATIONS\" />\n");
    // Each new plist is well formed: one <dict> per </dict>, as Apple reads them.
    for f in ["src-tauri/Info.ios.plist", "src-tauri/PrivacyInfo.xcprivacy"] {
        let t = d.read(f);
        assert_eq!(t.matches("<dict>").count(), t.matches("</dict>").count(), "{f}");
        assert_eq!(t.matches("<array>").count(), t.matches("</array>").count(), "{f}");
    }
    // A plist of the person's own that says "no encryption", with --voice:
    // never edited, but said.
    let d2 = Dir::new("tpl-store-own");
    d2.write("src-tauri/tauri.conf.json", CONF);
    d2.write("src-tauri/Info.ios.plist", "<plist><dict><key>ITSAppUsesNonExemptEncryption</key>\n<false/></dict></plist>");
    let p = plan_with(&d2.0, "tauri", &Options { voice: true, ..Options::default() }).unwrap();
    assert!(p.notes.iter().any(|n| n.contains("ITSAppUsesNonExemptEncryption is false")), "{:?}", p.notes);
    let f = p.files.iter().find(|f| f.path == "src-tauri/Info.ios.plist").unwrap();
    assert_eq!(f.action, Action::Snippet);
    assert!(f.content.contains("UIBackgroundModes") && !f.content.contains("ITSAppUsesNonExemptEncryption"), "{}", f.content);
}

/// The Cargo lines: printed, never written, on the manifest where [patch]
/// works (the workspace root).
#[test]
fn tauri_prints_the_cargo_lines_it_lacks() {
    let d = Dir::new("tpl-cargo");
    d.write("src-tauri/tauri.conf.json", CONF);
    let own = "[package]\nname = \"x\"\n\n[dependencies]\ntauri = { version = \"2.11\", features = [] }\n";
    d.write("src-tauri/Cargo.toml", own);
    let p = plan(&d.0, "tauri", None, None).unwrap();
    let c = p.files.iter().find(|f| f.path == "src-tauri/Cargo.toml").unwrap();
    assert_eq!(c.action, Action::Snippet);
    assert!(c.content.contains("tauri = { version = \"2.12.5\" }") && c.content.contains("reqwest = { version = \"0.12\""), "{}", c.content);
    assert!(c.content.contains(&format!("[patch.crates-io]\n{}", store::wry_patch_row())), "{}", c.content);
    assert!(c.note.contains("tauri 2.11 is older than 2.12.5"), "{}", c.note);
    assert!(p.next[0].contains("Cargo lines"), "{:?}", p.next);
    apply(&d.0, &p).unwrap();
    assert_eq!(d.read("src-tauri/Cargo.toml"), own, "never edited");

    // A workspace root: the patch goes there, the dependencies stay in
    // src-tauri. An existing [patch.crates-io] gets the row alone.
    let w = Dir::new("tpl-cargo-ws");
    w.write("src-tauri/tauri.conf.json", CONF);
    w.write("Cargo.toml", "[workspace]\nmembers = [\"src-tauri\"]\n\n[patch.crates-io]\nfoo = { path = \"foo\" }\n");
    w.write("src-tauri/Cargo.toml", "[dependencies]\ntauri = \"2.12.5\"\nreqwest = \"0.12\"\n");
    let p = plan(&w.0, "tauri", None, None).unwrap();
    let inner = p.files.iter().find(|f| f.path == "src-tauri/Cargo.toml").unwrap();
    assert_eq!(inner.action, Action::Skipped, "{}", inner.content);
    let root = p.files.iter().find(|f| f.path == "Cargo.toml").expect("the root");
    assert_eq!(root.action, Action::Snippet);
    assert!(root.content.contains("# under your [patch.crates-io]:\nwry = ") && !root.content.contains("\n[patch.crates-io]"), "{}", root.content);

    // K2's wry already there: nothing about wry. Another wry patch: kept, said.
    let k = Dir::new("tpl-cargo-k2");
    k.write("src-tauri/tauri.conf.json", CONF);
    k.write("src-tauri/Cargo.toml", &format!("[dependencies]\ntauri = \"2.12\"\nreqwest = \"0.12\"\n\n[patch.crates-io]\n{}\n", store::wry_patch_row()));
    let p = plan(&k.0, "tauri", None, None).unwrap();
    let c = p.files.iter().find(|f| f.path == "src-tauri/Cargo.toml").unwrap();
    assert!(c.content.contains("2.12.5") && !c.content.contains("wry"), "{}", c.content);
    let o = Dir::new("tpl-cargo-other");
    o.write("src-tauri/tauri.conf.json", CONF);
    o.write("src-tauri/Cargo.toml", "[dependencies]\ntauri = \"2.12.5\"\nreqwest = \"0.12\"\n\n[patch.crates-io]\nwry = { path = \"vendor/wry\" }\n");
    let p = plan(&o.0, "tauri", None, None).unwrap();
    assert_eq!(p.files.iter().find(|f| f.path == "src-tauri/Cargo.toml").unwrap().action, Action::Skipped);
    assert!(p.notes.iter().any(|n| n.contains("patches wry its own way") && n.contains("vendor/wry")), "{:?}", p.notes);
}

fn kinds(r: &doctor::Report) -> Vec<(&'static str, doctor::Level)> {
    r.findings.iter().map(|f| (f.id, f.level)).collect()
}

/// `k2 app doctor --store` on a repo that has none of it, then on the same
/// repo after `k2 app init --template tauri --voice` and the hand steps.
#[test]
fn store_doctor_names_each_gap_and_passes_a_finished_repo() {
    use doctor::Level::{Error, Warn};
    let d = Dir::new("tpl-doctor");
    assert_eq!(doctor::check(&d.0, None).unwrap_err().code, codes::NOT_A_TAURI_REPO);
    d.write("src-tauri/tauri.conf.json", CONF);
    d.write("src-tauri/Cargo.toml", "[dependencies]\ntauri = \"2.10\"\n");
    d.write("src-tauri/Cargo.lock", &lock("2.10.3", "0.35.3", "registry+https://github.com/rust-lang/crates.io-index"));
    d.write("src-tauri/Info.plist", "<plist><dict><key>NSMicrophoneUsageDescription</key><string>calls</string></dict></plist>");
    d.write("src-tauri/Info.ios.plist", "<plist><dict><key>ITSAppUsesNonExemptEncryption</key>\n<false/></dict></plist>");
    d.write(ANDROID, "<manifest><uses-permission android:name=\"android.permission.FOREGROUND_SERVICE_MICROPHONE\" /></manifest>");
    let r = doctor::check(&d.0, None).unwrap();
    assert!(r.voice && r.voice_why.contains("microphone"), "calls found: {}", r.voice_why);
    assert_eq!(r.tauri.as_deref(), Some("2.10.3"));
    assert_eq!(r.wry.as_deref(), Some("0.57.0 (crates.io)"));
    let k = kinds(&r);
    for want in [
        ("tauri_too_old", Error),
        ("tao_scene_crash", Error),
        ("wry_unpatched", Warn),
        ("privacy_manifest_missing", Error),
        ("ios_scene_manifest_missing", Error),
        ("export_answer_wrong", Error),
        ("usage_string_missing", Error),
        ("background_audio_missing", Error),
        ("entitlement_missing", Error),
        ("android_permission_missing", Error),
        ("android_service_missing", Warn),
    ] {
        assert!(k.contains(&want), "{want:?} in {k:?}");
    }
    assert!(!r.ok);
    let old = r.findings.iter().find(|f| f.id == "tauri_too_old").unwrap();
    assert!(old.problem.contains("2.10 deadlock") && old.problem.contains("iOS 27") && old.fix.contains("2.12.5"), "{old:?}");
    assert!(r.notes.iter().any(|n| n.contains("gen/apple")), "{:?}", r.notes);
    // --no-voice: the call checks don't run.
    let quiet = doctor::check(&d.0, Some(false)).unwrap();
    assert!(!kinds(&quiet).iter().any(|(id, _)| matches!(*id, "usage_string_missing" | "background_audio_missing" | "entitlement_missing")), "{:?}", kinds(&quiet));
    assert!(quiet.passed.contains(&"export_answer"), "false is right without calls: {:?}", kinds(&quiet));

    // Fixed: a fresh repo through init --voice, then the hand steps.
    let f = Dir::new("tpl-doctor-done");
    f.write("src-tauri/tauri.conf.json", CONF);
    f.write("src-tauri/Cargo.toml", &format!("[dependencies]\ntauri = \"2.12.5\"\nreqwest = \"0.12\"\n\n[patch.crates-io]\n{}\n", store::wry_patch_row()));
    let p = plan_with(&f.0, "tauri", &Options { voice: true, ..Options::default() }).unwrap();
    apply(&f.0, &p).unwrap();
    f.write("src-tauri/Cargo.lock", &lock("2.12.5", "0.37.1", &format!("git+{}?rev={}#{}", store::K2_REPO, store::WRY_PATCH_REV, store::WRY_PATCH_REV)));
    f.write("src-tauri/gen/apple/project.yml", "targets:\n  app_iOS:\n    sources:\n      - path: Sources\n      - path: ../../PrivacyInfo.xcprivacy\n        buildPhase: resources\n");
    let lines: String = store::android_permissions(true).join("\n");
    f.write(ANDROID, &format!("<manifest>{lines}<application><service android:name=\".Call\" android:foregroundServiceType=\"microphone|camera\" /></application></manifest>"));
    let r = doctor::check(&f.0, None).unwrap();
    assert!(r.voice_why.contains("k2_voice.rs"), "{}", r.voice_why);
    assert!(r.findings.is_empty(), "a finished repo: {:?}", r.findings);
    assert!(r.ok);
    assert_eq!(r.wry.as_deref(), Some("0.57.0 (K2's patched copy)"));
    for id in ["tauri_version", "tao_scene", "wry_patched", "privacy_manifest", "privacy_manifest_in_app", "ios_scene_manifest", "export_answer", "ios_usage_strings", "ios_background_audio", "macos_call_keys", "android_permissions"] {
        assert!(r.passed.contains(&id), "{id} in {:?}", r.passed);
    }
    // The manifest not in the iOS target: said.
    f.write("src-tauri/gen/apple/project.yml", "targets:\n  app_iOS:\n    sources:\n      - path: Sources\n");
    let r = doctor::check(&f.0, None).unwrap();
    assert_eq!(kinds(&r), [("privacy_manifest_not_in_app", Error)]);
}

/// A Cargo.lock with tauri, tao and wry (wry from `wry_source`).
fn lock(tauri: &str, tao: &str, wry_source: &str) -> String {
    let reg = "registry+https://github.com/rust-lang/crates.io-index";
    format!(
        "version = 4\n\n[[package]]\nname = \"tao\"\nversion = \"{tao}\"\nsource = \"{reg}\"\n\n[[package]]\nname = \"tauri\"\nversion = \"{tauri}\"\nsource = \"{reg}\"\n\n[[package]]\nname = \"wry\"\nversion = \"0.57.0\"\nsource = \"{wry_source}\"\n"
    )
}

#[test]
fn small_parsers() {
    assert_eq!(doctor::lowest_version("2"), Some((2, 0, 0)));
    assert_eq!(doctor::lowest_version("^2.12.5"), Some((2, 12, 5)));
    assert_eq!(doctor::lowest_version("~2.11"), Some((2, 11, 0)));
    assert_eq!(doctor::lowest_version("<2.12"), None);
    assert_eq!(doctor::plist_bool("<key>A</key>\n  <true/>", "A"), Some(true));
    assert_eq!(doctor::plist_bool("<key>A</key><string>x</string>", "A"), None);
    assert!(doctor::is_workspace_root("[workspace]\nmembers = []\n"));
    assert!(!doctor::is_workspace_root("[package]\nname = \"x\"\n"));
    assert_eq!(doctor::wry_patch("[patch.crates-io]\nwry = \"0.57\"\n"), doctor::WryPatch::Other("\"0.57\"".into()));
    assert_eq!(doctor::wry_patch("not toml ["), doctor::WryPatch::Missing);
}
