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
            (".k2/compute.toml".to_string(), Action::Edit),
        ]
    );
    assert!(p.files[2].note.contains("different"), "a diff hint: {}", p.files[2].note);
    assert!(p.next.iter().any(|n| n.contains("k2 app deploy my-tauri-app-web dist --manifest k2/web.manifest.json")), "{:?}", p.next);
    // A dry run is the plan alone: nothing written yet.
    assert!(!d.0.join("k2/web.manifest.json").exists());
    let shown = plan_json(&p, true);
    assert!(shown["files"][0]["content"].as_str().unwrap().contains("\"mode\": \"static\""));
    assert!(plan_json(&p, false)["files"][0].get("content").is_none(), "content only on a dry run");
    let a = apply(&d.0, &p).expect("apply");
    assert_eq!(a.written, ["k2/web.manifest.json"]);
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
    // A second run writes nothing.
    let p2 = plan(&d.0, "tauri", None, Some("box")).expect("plan again");
    assert!(p2.files.iter().all(|f| f.action == Action::Skipped), "{:?}", actions(&p2));
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
    assert!(p2.files.iter().all(|f| f.action == Action::Skipped), "second run: {:?}", actions(&p2));
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
    let android = "src-tauri/gen/android/app/src/main/AndroidManifest.xml";
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
    for perm in ["RECORD_AUDIO", "MODIFY_AUDIO_SETTINGS", "CAMERA"] {
        assert!(a.content.contains(&format!("android.permission.{perm}")), "{}", a.content);
    }
    apply(&d.0, &p).unwrap();
    for f in ["src-tauri/Info.plist", "src-tauri/Info.ios.plist"] {
        let t = d.read(f);
        assert!(t.starts_with("<?xml") && t.contains("<key>NSMicrophoneUsageDescription</key>") && t.contains("<key>NSCameraUsageDescription</key>"), "{f}: {t}");
    }
    let t = d.read("src-tauri/Entitlements.plist");
    assert!(t.contains("<key>com.apple.security.device.audio-input</key>\n  <true/>") && t.contains("<key>com.apple.security.device.camera</key>"), "{t}");
    let conf: J = serde_json::from_str(&d.read("src-tauri/tauri.conf.json")).unwrap();
    assert_eq!(conf["bundle"]["macOS"]["entitlements"], "./Entitlements.plist");
    assert_eq!(conf["bundle"]["createUpdaterArtifacts"], true);
    assert_eq!(conf["bundle"]["active"], true, "kept");
    assert!(d.read("k2/README.md").contains("## Calls"));

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
