//! Tests for `widget_folder` (0.46.1 S3): staged folders are checked and
//! written whole, previews land on K2's own preview Garden only. Every Zen
//! folder is under the temp dir. Fail loudly.

use std::path::PathBuf;

use super::*;
use crate::zen::ZenFile;

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set_up(tag: &str) -> (Tmp, ZenFiles) {
    let d = std::env::temp_dir().join(format!("k2-zen-wfolder-{tag}-{}", uuid::Uuid::new_v4().simple()));
    let f = ZenFiles::new(d.join(".k2").join("zen"));
    assert!(f.root().starts_with(std::env::temp_dir()), "tests never touch the real Zen folder");
    f.setup().expect("setup");
    (Tmp(d), f)
}

const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR";

fn manifest(id: &str) -> String {
    format!("{{\n  \"schema\": 1,\n  \"id\": \"{id}\",\n  \"name\": \"Board\",\n  \"caps\": [\"agents:read\"]\n}}\n")
}

fn staged(id: &str, html: &str) -> Vec<StagedFile> {
    vec![
        StagedFile { name: "manifest.json".into(), text: Some(manifest(id)), base64: None },
        StagedFile { name: "index.html".into(), text: Some(html.into()), base64: None },
        StagedFile { name: "app.js".into(), text: Some("console.log('hi')\n".into()), base64: None },
        StagedFile {
            name: "logo.png".into(),
            text: None,
            base64: Some(base64::engine::general_purpose::STANDARD.encode(PNG)),
        },
    ]
}

const HTML: &str = "<!doctype html><html><body><p>v1</p><script src=\"app.js\"></script></body></html>\n";

#[test]
fn staged_files_follow_the_name_and_encoding_rules() {
    let files = staged_files(&staged("board", HTML)).expect("a good folder");
    assert_eq!(files.keys().cloned().collect::<Vec<_>>(), vec!["app.js", "index.html", "logo.png", "manifest.json"]);
    assert_eq!(files["logo.png"], PNG);
    let bad = |list: Vec<StagedFile>| match staged_files(&list) {
        Err(ZenError::BadRequest(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    };
    let one = |name: &str, text: Option<&str>, b64: Option<&str>| {
        vec![StagedFile { name: name.into(), text: text.map(str::to_string), base64: b64.map(str::to_string) }]
    };
    assert!(bad(vec![]).contains("no files"));
    assert!(bad(one("../x.js", Some("x"), None)).contains("can't be in a widget folder"));
    assert!(bad(one(".env", Some("x"), None)).contains("can't be in a widget folder"));
    assert!(bad(one("sub/a.js", Some("x"), None)).contains("can't be in a widget folder"));
    assert!(bad(one("a.js", None, Some("eA=="))).contains("send it as text"));
    assert!(bad(one("a.png", Some("x"), None)).contains("send it as base64"));
    assert!(bad(one("a.png", None, Some("@@@"))).contains("isn't valid base64"));
    let mut twice = staged("board", HTML);
    twice.push(StagedFile { name: "app.js".into(), text: Some("x".into()), base64: None });
    assert!(bad(twice).contains("twice"));
    let big: Vec<StagedFile> = (0..65).map(|i| StagedFile { name: format!("f{i}.js"), text: Some(String::new()), base64: None }).collect();
    assert!(bad(big).contains("at most 64 files"));
}

#[test]
fn staged_names_come_from_the_argument_then_the_manifest_then_the_folder() {
    let files = staged_files(&staged("board", HTML)).unwrap();
    assert_eq!(staged_name(Some("other"), &files, Some("dir")).unwrap(), "other");
    assert_eq!(staged_name(None, &files, Some("dir")).unwrap(), "board");
    let mut no_id = files.clone();
    no_id.insert("manifest.json".into(), b"{\"schema\": 1, \"name\": \"B\", \"caps\": []}".to_vec());
    assert_eq!(staged_name(None, &no_id, Some("dir-name")).unwrap(), "dir-name");
    assert!(matches!(staged_name(None, &no_id, None), Err(ZenError::BadRequest(_))));
    assert!(matches!(staged_name(Some("k2:diary"), &files, None), Err(ZenError::BadRequest(m)) if m.contains("built into K2")));
    assert!(matches!(staged_name(Some("Bad Name"), &files, None), Err(ZenError::BadRequest(_))));
}

#[test]
fn check_staged_reports_like_validate_and_never_pends() {
    let files = staged_files(&staged("board", HTML)).unwrap();
    let v = check_staged("board", &files);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(v["status"], "ok", "{v}");
    assert_eq!(v["files"], json!(["staged/board"]));
    assert!(v["bundleHash"].as_str().is_some_and(|h| h.len() == 64), "{v}");
    // The manifest's id must match the name it checks as.
    let v = check_staged("other", &files);
    assert_eq!((v["ok"].as_bool(), v["status"].as_str()), (Some(false), Some("failed")), "{v}");
    assert!(v["errors"].to_string().contains("doesn't match the folder"), "{v}");
    // No manifest at all.
    let mut no_manifest = files.clone();
    no_manifest.remove("manifest.json");
    let v = check_staged("board", &no_manifest);
    assert_eq!(v["ok"], false);
    assert!(v["errors"].to_string().contains("no manifest.json"), "{v}");
}

#[test]
fn a_folder_put_lands_whole_only_when_clean_and_prunes_only_on_request() {
    let (_t, f) = set_up("put");
    let files = staged_files(&staged("board", HTML)).unwrap();
    let v = f.put_widget_folder("board", &files, false).expect("first put");
    assert_eq!(v["written"], json!(["app.js", "index.html", "logo.png", "manifest.json"]), "{v}");
    assert_eq!(v["validate"]["ok"], true, "{v}");
    let dir = f.widget_dir("board");
    assert_eq!(fs::read(dir.join("logo.png")).unwrap(), PNG);

    // Same files again: nothing rewritten.
    let v = f.put_widget_folder("board", &files, false).expect("again");
    assert_eq!(v["changed"], false, "{v}");
    assert_eq!(v["written"], json!([]), "{v}");

    // A file the staged folder drops is kept without prune, removed with it;
    // README and dotfiles are never touched.
    fs::write(dir.join("README.md"), "notes").unwrap();
    fs::write(dir.join(".keep"), "").unwrap();
    let mut smaller = files.clone();
    smaller.remove("logo.png");
    let v = f.put_widget_folder("board", &smaller, false).expect("no prune");
    assert_eq!(v["kept"], json!(["logo.png"]), "{v}");
    assert!(dir.join("logo.png").exists());
    let v = f.put_widget_folder("board", &smaller, true).expect("prune");
    assert_eq!(v["removed"], json!(["logo.png"]), "{v}");
    assert!(!dir.join("logo.png").exists());
    assert!(dir.join("README.md").exists() && dir.join(".keep").exists(), "prune leaves README and dotfiles");

    // A folder with errors is refused and nothing is written.
    let mut broken = smaller.clone();
    broken.insert("manifest.json".into(), b"{ not json".to_vec());
    broken.insert("index.html".into(), b"<p>v2</p>".to_vec());
    match f.put_widget_folder("board", &broken, false) {
        Err(ZenError::BadRequest(m)) => assert!(m.contains("nothing was written"), "{m}"),
        other => panic!("a broken folder must be refused: {other:?}"),
    }
    assert_eq!(fs::read_to_string(dir.join("index.html")).unwrap(), HTML, "a bad folder never lands");

    // Built-ins are never written.
    assert!(matches!(f.put_widget_folder("k2:diary", &files, false), Err(ZenError::BadRequest(_))));
}

#[test]
fn a_folder_proposal_holds_code_changes_and_keeps_the_rest_by_hash() {
    let (_t, f) = set_up("changes");
    let files = staged_files(&staged("board", HTML)).unwrap();
    f.put_widget_folder("board", &files, false).expect("put");
    assert!(f.widget_folder_proposal("board", &files).unwrap().is_none(), "nothing changed");
    let mut next = files.clone();
    let v2 = "<p>v2</p><script src=\"app.js\"></script>";
    next.insert("index.html".into(), v2.as_bytes().to_vec());
    let p = f.widget_folder_proposal("board", &next).unwrap().expect("a change");
    assert_eq!(p.files.len(), 1);
    assert_eq!((p.files[0].name.as_str(), p.files[0].text.as_str()), ("index.html", v2));
    assert_eq!(p.files[0].base, text_hash(Some(HTML)), "the base is the file on disk");
    assert_eq!(p.keep.len(), files.len() - 1, "every other staged file is kept by hash");
    assert!(p.keep.iter().all(|k| k.hash == bytes_hash(files.get(&k.name).map(Vec::as_slice))));
    assert!(f.folder_proposal_stale("board", &p).unwrap().is_empty());
    let mut asset = files.clone();
    asset.insert("logo.png".into(), [PNG, b"x"].concat());
    assert!(matches!(f.widget_folder_proposal("board", &asset), Err(ZenError::BadRequest(m)) if m.contains("image or font")));
    // A kept file that moves makes it stale, and applying writes nothing.
    let kept = p.keep[0].name.clone();
    fs::write(f.widget_dir("board").join(&kept), b"moved").unwrap();
    assert_eq!(f.folder_proposal_stale("board", &p).unwrap(), vec![kept.clone()]);
    match f.apply_folder_proposal("board", &p).unwrap() {
        Err(stale) => assert_eq!(stale, vec![kept]),
        Ok(v) => panic!("a stale folder proposal must not apply: {v}"),
    }
    assert_eq!(fs::read_to_string(f.widget_dir("board").join("index.html")).unwrap(), HTML, "nothing written");
}

#[test]
fn a_preview_lands_on_k2s_preview_garden_only_and_clears() {
    let (_t, f) = set_up("preview");
    let before: Vec<(String, Option<String>)> = f
        .gardens()
        .into_iter()
        .map(|g| (g.id.clone(), fs::read_to_string(f.path_of(&ZenFile::Garden(g.id))).ok()))
        .collect();
    let mut files = staged_files(&staged("board", HTML)).unwrap();
    // A card that reads the sources its placement names.
    files.insert(
        "manifest.json".into(),
        b"{\"schema\": 1, \"id\": \"board\", \"name\": \"Board\", \"caps\": [\"sources:read\"], \"sources\": \"config\"}".to_vec(),
    );
    let config: BTreeMap<String, String> = [("sources".to_string(), "fleet, k2-ops-boxes".to_string())].into();
    let v = f.preview_widget_folder(None, None, &files, &config, false).expect("preview");
    assert_eq!(v["preview"], "preview-board", "{v}");
    assert_eq!(v["validate"]["ok"], true, "{v}");
    let gid = v["garden"]["id"].as_str().expect("garden id").to_string();
    assert_eq!(f.find_garden(PREVIEW_GARDEN).expect("preview garden").1.id, gid);
    // The person's Gardens are untouched.
    for (id, text) in &before {
        assert_ne!(id, &gid, "the preview never reuses a person's Garden");
        assert_eq!(&fs::read_to_string(f.path_of(&ZenFile::Garden(id.clone()))).ok(), text, "Garden {id} untouched");
    }
    // The manifest id follows the preview folder; the page places it with the config.
    let m: J = serde_json::from_str(&fs::read_to_string(f.widget_dir("preview-board").join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m["id"], "preview-board");
    let page = f.resolve(Some(&gid)).expect("resolve preview");
    let w = page["page"]["widgets"]
        .as_array()
        .and_then(|ws| ws.iter().find(|w| w["kind"] == "custom"))
        .cloned()
        .unwrap_or_else(|| panic!("no custom widget on the preview page: {page}"));
    assert_eq!(w["widget"], "preview-board", "{w}");
    assert_eq!(w["sourceAddresses"], json!(["fleet", "k2-ops-boxes"]), "{w}");

    // A second preview reuses the Garden.
    let n = f.gardens().len();
    f.preview_widget_folder(Some("board"), None, &files, &BTreeMap::new(), false).expect("preview again");
    assert_eq!(f.gardens().len(), n, "one preview Garden");

    // Bad config keys and values are refused.
    let bad: BTreeMap<String, String> = [("9x".to_string(), "v".to_string())].into();
    assert!(matches!(f.preview_widget_folder(None, None, &files, &bad, false), Err(ZenError::BadRequest(_))));
    let bad: BTreeMap<String, String> = [("k".to_string(), "a\nb".to_string())].into();
    assert!(matches!(f.preview_widget_folder(None, None, &files, &bad, false), Err(ZenError::BadRequest(_))));

    let v = f.clear_widget_previews().expect("clear");
    assert_eq!(v["garden"], gid.as_str(), "{v}");
    assert_eq!(v["removed"], json!(["preview-board"]), "{v}");
    assert!(f.find_garden(PREVIEW_GARDEN).is_none());
    assert!(!f.widget_dir("preview-board").exists());
    assert_eq!(f.gardens().len(), before.len(), "back to the person's Gardens");
}

#[test]
fn preview_names_stay_within_forty_characters() {
    assert_eq!(ZenFiles::preview_widget_name("board").unwrap(), "preview-board");
    assert_eq!(ZenFiles::preview_widget_name("preview-board").unwrap(), "preview-board");
    assert!(ZenFiles::preview_widget_name(&"a".repeat(33)).is_err());
    let page = ZenFiles::preview_page("preview-x", &[("title".to_string(), "say \"hi\" \\o/".to_string())].into());
    assert!(page.contains(r#"config = { title = "say \"hi\" \\o/" }"#), "{page}");
}
