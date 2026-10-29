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
