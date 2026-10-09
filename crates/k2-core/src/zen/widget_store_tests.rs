//! Tests for `widget_store` (prd-zen-user-widgets-v2 TUW1.3, TUW1.5,
//! TUW2.1 core side; the Diary preinstall, Rosson 2026-10-08). Every Zen
//! folder is under the temp dir; fixtures use made-up names only.

use super::*;
use crate::zen::widget_access::{Pause, PauseReason, WidgetPauses};

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A Zen folder at `<tmp>/.k2/zen`, set up. Never the real `~/.k2`.
fn set_up(tag: &str) -> (Tmp, ZenFiles) {
    let d = std::env::temp_dir().join(format!("k2-zen-wstore-{tag}-{}", uuid::Uuid::new_v4().simple()));
    let f = ZenFiles::new(d.join(".k2").join("zen"));
    assert!(f.root().starts_with(std::env::temp_dir()), "tests never touch the real Zen folder");
    f.setup().expect("setup");
    (Tmp(d), f)
}

fn gid(f: &ZenFiles, name: &str) -> String {
    f.find_garden(name).unwrap_or_else(|| panic!("no Garden {name}")).1.id
}

fn write_garden(f: &ZenFiles, id: &str, text: &str) {
    fs::write(f.path_of(&ZenFile::Garden(id.into())), text).expect("write garden");
}

const ARCADE_PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 60\nmin-width = 200\n[[layout.column]]\nsize = 40\nmin-width = 200\n\n[[widget]]\nid = \"arcade\"\nkind = \"custom\"\nwidget = \"agent-arcade\"\ncolumn = 0\n[widget.props]\nhome = \"Work\"\n\n[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 1\n[widget.props]\nagents = \"arcade\"\n";

/// Rosson 2026-10-08: the Diary is preinstalled on a new computer, once.
#[test]
fn setup_preinstalls_the_diary_once_and_a_deleted_one_never_returns() {
    let (_t, f) = set_up("diary");
    let names: Vec<String> = f.gardens().into_iter().map(|g| g.name).collect();
    assert_eq!(names, vec!["Garden 1", "Garden 2", "Diary"]);
    let diary = f.find_garden("Diary").expect("diary").1;
    assert_eq!(diary.template, crate::zen::garden_catalog::DIARY_TEMPLATE_ID);
    let page = f.resolve(Some(&diary.id)).expect("resolve diary");
    let w = &page["page"]["widgets"][0];
    assert_eq!((w["kind"].as_str(), w["widget"].as_str()), (Some("custom"), Some("k2:diary@2")), "{page}");
    assert_eq!(w["source"], "user");
    assert_eq!(w["origin"], "local");
    assert_eq!(w["caps"], json!(["agents:read", "thread:read", "thread:post"]), "the Diary works with zero clicks: {w}");
    assert_eq!(w["paused"], J::Null);
    assert!(w.get("grant").is_none(), "no grants any more: {w}");
    f.setup().expect("setup again");
    assert_eq!(f.gardens().len(), 3, "setup again adds nothing");
    f.delete_garden(&diary.id).expect("delete diary");
    f.setup().expect("setup after delete");
    let names: Vec<String> = f.gardens().into_iter().map(|g| g.name).collect();
    assert_eq!(names, vec!["Garden 1", "Garden 2"], "a deleted Diary never comes back on its own");
    // Loadable again from the catalog by its short name.
    let g = f.new_garden("My diary", Some("diary"), None, None).expect("new from catalog");
    assert_eq!(g.template, "k2.diary@2");
    assert!(matches!(f.new_garden("x", Some("dashboard"), None, None), Err(ZenError::BadRequest(_))));
}

/// An existing list is never appended to (R5): a list made before the
/// catalog existed gets no Diary from setup.
#[test]
fn an_existing_list_never_gets_the_diary() {
    let d = std::env::temp_dir().join(format!("k2-zen-wstore-old-{}", uuid::Uuid::new_v4().simple()));
    let _t = Tmp(d.clone());
    let f = ZenFiles::new(d.join(".k2").join("zen"));
    fs::create_dir_all(f.gardens_dir()).expect("mkdir");
    fs::write(
        f.root().join("gardens.json"),
        "{\"version\":1,\"gardens\":[{\"id\":\"g-test0001\",\"name\":\"Mine\",\"template\":\"k2.texting@1\"}]}",
    )
    .expect("list");
    let out = f.setup().expect("setup");
    assert!(!out.created_default);
    let names: Vec<String> = f.gardens().into_iter().map(|g| g.name).collect();
    assert_eq!(names, vec!["Mine"]);
}

/// TUW1.3: last good, broken, history and reset.
#[test]
fn last_good_serves_through_errors_and_reset_restores_code() {
    let (_t, f) = set_up("lastgood");
    let out = f.new_widget("agent-arcade", Some("arcade")).expect("new");
    assert_eq!(out["files"].as_array().map(Vec::len), Some(4));
    assert!(matches!(f.new_widget("agent-arcade", None), Err(ZenError::WidgetExists(_))));
    f.refresh().expect("refresh");
    let good = f.widget_status("agent-arcade");
    assert_eq!(good.state, WidgetState::Ok, "{:?}", good.errors);
    let hash = good.hash().to_string();
    assert_eq!(f.widget_snapshots("agent-arcade").len(), 1);

    let js = f.widget_dir("agent-arcade").join("arcade.js");
    let original = fs::read(&js).expect("read js");
    fs::write(f.widget_dir("agent-arcade").join("index.html"), "<script src=\"https://cdn.example.test/x.js\"></script>")
        .expect("break");
    f.refresh().expect("refresh");
    let st = f.widget_status("agent-arcade");
    assert_eq!(st.state, WidgetState::Errors);
    assert_eq!(st.hash(), hash, "the last good bundle stays live");
    assert_eq!(st.errors.len(), 1, "{:?}", st.errors);
    assert_eq!(f.widget_bundle("agent-arcade").expect("bundle").hash(), hash);
    assert_eq!(f.widget_snapshots("agent-arcade").len(), 1, "a broken save is no snapshot");

    let r = f.reset_widget("agent-arcade", None).expect("reset");
    assert!(r["snapshot"].is_string(), "the broken code was kept first: {r}");
    assert_eq!(fs::read(&js).expect("read"), original, "byte for byte");
    assert_eq!(f.widget_status("agent-arcade").state, WidgetState::Ok);
    let snaps = f.widget_snapshots("agent-arcade");
    assert_eq!(snaps.iter().filter(|s| s.clean).count(), 1);
    assert_eq!(snaps.iter().filter(|s| !s.clean).count(), 1);
    let h = f.widget_history("agent-arcade").expect("history");
    assert_eq!(h["files"][0]["lastGood"]["hash"], hash.as_str());

    // A first-ever broken folder is broken, and the bundle route says so.
    fs::create_dir_all(f.widget_dir("broken")).expect("mkdir");
    fs::write(f.widget_dir("broken").join("manifest.json"), "{").expect("w");
    f.refresh().expect("refresh");
    assert_eq!(f.widget_status("broken").state, WidgetState::Broken);
    assert!(matches!(f.widget_bundle("broken"), Err(ZenError::WidgetBroken(_))));
    assert!(matches!(f.widget_bundle("nope"), Err(ZenError::UnknownWidget(_))));
    let v = f.validate(None).expect("validate");
    assert_eq!(v["ok"], false);
    assert!(v["files"].as_array().expect("files").iter().any(|x| x == "widgets/broken"));
}

#[test]
fn the_refresh_fingerprint_moves_once_per_change() {
    let (_t, f) = set_up("fp");
    let a = f.refresh().expect("fp");
    f.new_widget("clock", None).expect("new");
    let b = f.refresh().expect("fp");
    assert_ne!(a, b, "a new widget folder moves the fingerprint");
    assert_eq!(b, f.refresh().expect("fp"), "nothing changed, nothing moves");
    fs::write(f.widget_dir("clock").join("hello.js"), "k2.ready()\n").expect("edit");
    let c = f.refresh().expect("fp");
    assert_ne!(b, c, "a JS save moves it");
    fs::write(f.widget_dir("clock").join("hello.js"), "k2.ready()\n").expect("same bytes");
    assert_eq!(c, f.refresh().expect("fp"), "the same bytes move nothing");
    let m = f.widget_dir("clock").join("manifest.json");
    let text = fs::read_to_string(&m).expect("manifest").replace("\"Hello\"", "\"Hello again\"");
    fs::write(&m, text).expect("rename");
    let c2 = f.refresh().expect("fp");
    assert_ne!(c, c2, "a manifest-only change (the name) moves it");
    let c = c2;
    let mut pauses = WidgetPauses::empty();
    pauses.insert("g-test0001", "clock", Pause { at: "2026-10-08T12:00:00Z".into(), reason: PauseReason::Runaway });
    assert_ne!(c, f.refresh_with(&pauses).expect("fp"), "a runaway pause is in the fingerprint");
    assert_eq!(c, f.refresh().expect("fp"), "and its resume moves it back");
}

/// UW38 as changed 2026-10-08: a custom placement in `get` runs with every
/// Garden-safe cap it asks for, no grant; a runaway pause shows on it.
#[test]
fn resolve_fills_custom_widgets_with_their_caps_and_pause() {
    let (_t, f) = set_up("resolve");
    f.new_widget("agent-arcade", Some("arcade")).expect("new");
    let g = gid(&f, "Garden 2");
    write_garden(&f, &g, ARCADE_PAGE);
    f.refresh().expect("refresh");
    let page = f.resolve(Some(&g)).expect("resolve");
    assert_eq!(page["errors"], json!([]), "{page}");
    let w = &page["page"]["widgets"][0];
    assert_eq!(w["kind"], "custom");
    assert_eq!(w["name"], "Agent Arcade");
    assert_eq!(w["state"], "ok");
    assert_eq!(w["requested"], json!(["agents:read", "thread:read", "thread:post"]));
    assert_eq!(w["caps"], json!(["agents:read", "thread:read", "thread:post"]), "a new widget works with zero clicks");
    assert_eq!(w["origin"], "local");
    assert_eq!(w["paused"], J::Null);
    assert!(w.get("grant").is_none(), "{w}");
    assert_eq!(w["props"], json!({"home": "Work", "config": {}}));
    assert!(w["hash"].as_str().is_some_and(|h| h.len() == 64));

    let mut pauses = WidgetPauses::empty();
    pauses.insert(&g, "arcade", Pause { at: "2026-10-08T12:00:00Z".into(), reason: PauseReason::Runaway });
    let page = f.resolve_with(Some(&g), &pauses).expect("resolve");
    let w = &page["page"]["widgets"][0];
    assert_eq!(w["paused"], json!({"at": "2026-10-08T12:00:00Z", "reason": "runaway"}), "{w}");
    assert_eq!(w["caps"], json!(["agents:read", "thread:read", "thread:post"]), "a pause never takes caps away");
    let listed = f.widgets_json(&pauses).expect("widgets");
    let arcade = listed["widgets"].as_array().expect("list").iter().find(|x| x["name"] == "agent-arcade").cloned();
    let placement = arcade.expect("arcade listed")["placements"][0].clone();
    assert_eq!(placement["placement"], "arcade", "{listed}");
    assert_eq!(placement["paused"]["reason"], "runaway", "{listed}");
    assert!(placement.get("grant").is_none(), "{listed}");

    // The placement's home prop changes: nothing to re-ask.
    write_garden(&f, &g, &ARCADE_PAGE.replace("home = \"Work\"", "home = \"Play\""));
    let page = f.resolve(Some(&g)).expect("resolve");
    assert_eq!(page["page"]["widgets"][0]["caps"], json!(["agents:read", "thread:read", "thread:post"]));
}

/// TUW1.5 (store side): a missing folder is an error at its line without
/// rolling the page back; a Conversation following a widget that doesn't
/// ask for agents:read is an error.
#[test]
fn placement_findings_need_the_folder() {
    let (_t, f) = set_up("placement");
    let g = gid(&f, "Garden 2");
    write_garden(&f, &g, ARCADE_PAGE);
    let v = f.validate(Some(&ZenFile::Garden(g.clone()))).expect("validate");
    let errs = v["errors"].as_array().expect("errors").clone();
    assert_eq!(errs.len(), 1, "{v}");
    assert_eq!((errs[0]["line"].as_u64(), errs[0]["col"].as_u64()), (Some(15), Some(10)));
    assert!(errs[0]["message"].as_str().is_some_and(|m| m.contains("no widget folder 'agent-arcade'")));
    let page = f.resolve(Some(&g)).expect("resolve");
    assert_eq!(page["page"]["widgets"][0]["state"], "broken", "the page still shows the placement: {page}");

    f.new_widget("agent-arcade", Some("hello")).expect("new");
    let m = f.widget_dir("agent-arcade").join("manifest.json");
    let text = fs::read_to_string(&m)
        .expect("read")
        .replace("\"caps\": [\"agents:read\"]", "\"caps\": []")
        .replace(",\n  \"reasons\": {\n    \"agents:read\": \"to say hello to the first agent in this Home\"\n  }", "");
    fs::write(&m, text).expect("write");
    let st = f.widget_status("agent-arcade");
    assert_eq!(st.state, WidgetState::Ok, "{:?}", st.errors);
    let v = f.validate(Some(&ZenFile::Garden(g))).expect("validate");
    let errs = v["errors"].as_array().expect("errors").clone();
    assert_eq!(errs.len(), 1, "{v}");
    assert!(errs[0]["message"].as_str().is_some_and(|m| m.contains("doesn't ask for agents:read")));
    assert_eq!(errs[0]["line"].as_u64(), Some(25));
}

/// TUW1.5 (schema side).
#[test]
fn custom_placements_in_the_schema() {
    let check = |src: &str| {
        schema::check_garden("gardens/g-test0001.toml", src, crate::zen::builtin_layer(), schema::BLANK_TEMPLATE_ID)
    };
    let one = |src: &str, line: usize, has: &str| {
        let c = check(src);
        assert_eq!(c.errors.len(), 1, "{src}\n{:?}", c.errors);
        assert_eq!(c.errors[0].line, line, "{:?}", c.errors[0]);
        assert!(c.errors[0].message.contains(has), "{}", c.errors[0].message);
    };
    assert!(check(ARCADE_PAGE).is_clean(), "{:?}", check(ARCADE_PAGE).errors);
    one(&ARCADE_PAGE.replace("widget = \"agent-arcade\"\n", ""), 13, "needs widget =");
    one(&ARCADE_PAGE.replace("widget = \"agent-arcade\"", "widget = \"k2:diary\""), 15, "with its version");
    one(
        &ARCADE_PAGE.replace("column = 0\n[widget.props]", "column = 0\ncaps = [\"agents:read\"]\n[widget.props]"),
        17,
        "can't grant caps",
    );
    one(&ARCADE_PAGE.replace("home = \"Work\"", "home = \"Work\"\nsize = 3"), 19, "unknown key 'size'");
    let big = format!("home = \"Work\"\nconfig = {{ blob = \"{}\" }}", "x".repeat(4100));
    one(&ARCADE_PAGE.replace("home = \"Work\"", &big), 19, "4 KB");
    one(&ARCADE_PAGE.replace("home = \"Work\"", "config = { list = [1] }"), 18, "config.list");
    one(
        &ARCADE_PAGE.replace(
            "kind = \"custom\"\nwidget = \"agent-arcade\"\ncolumn = 0",
            "kind = \"custom\"\nslot = \"top\"\nwidget = \"agent-arcade\"",
        ),
        15,
        "can't go in the top band",
    );
    let mut seven = String::from("schema = 1\n");
    for i in 0..7 {
        seven.push_str(&format!("\n[[widget]]\nid = \"w{i}\"\nkind = \"custom\"\nwidget = \"w{i}\"\ncolumn = 0\n"));
    }
    let c = check(&seven);
    assert_eq!(c.errors.len(), 1, "{:?}", c.errors);
    assert!(c.errors[0].message.contains("at most 6 custom widgets"));
    let ok = check(&ARCADE_PAGE.replace("home = \"Work\"", "home = \"Work\"\nconfig = { speed = 2, title = \"Hi\", sound = false }"));
    assert!(ok.is_clean(), "{:?}", ok.errors);
    let page = crate::zen::garden_page(&ok.layer, schema::BLANK_TEMPLATE_ID);
    assert_eq!(page["widgets"][0]["props"]["config"], json!({"speed": 2, "title": "Hi", "sound": false}));
    assert_eq!(page["widgets"][1]["props"]["agents"], "arcade", "the Conversation follows the custom widget");
}

#[test]
fn catalog_templates_check_clean_and_carry_both_controls() {
    for e in crate::zen::garden_catalog::garden_catalog() {
        let c = crate::zen::check_template_chrome(&e.template_id).expect("chrome");
        assert!(c.is_clean(), "{}: {:?}", e.template_id, c.errors);
        let page = crate::zen::template_page(&e.template_id).expect("page");
        let kinds: Vec<&str> =
            page["controls"].as_array().into_iter().flatten().filter_map(|c| c["kind"].as_str()).collect();
        for r in crate::zen::REQUIRED_CONTROLS {
            assert!(kinds.contains(r), "{} lacks {r}", e.template_id);
        }
    }
    let diary =
        crate::zen::garden_catalog::current_entries().into_iter().find(|e| e.meta.short == "diary").expect("diary");
    assert!(diary.meta.new_users, "Rosson 2026-10-08: preinstalled");
    // Rosson 2026-10-08: nothing on the Diary that isn't haunted. Its only
    // chrome is one ⋯ menu at the top right holding the two required
    // controls; no usage chip, no theme control.
    let page = crate::zen::template_page(&diary.template_id).expect("diary page");
    let items = page["chrome"]["items"].as_array().expect("chrome items");
    let kinds: Vec<&str> = items.iter().filter_map(|i| i["kind"].as_str()).collect();
    assert_eq!(kinds, vec!["menu", "garden-switcher", "zen-toggle"], "{page}");
    let menu = &items[0];
    assert_eq!((menu["slot"].as_str(), menu["align"].as_str()), (Some("top"), Some("end")), "{menu}");
    assert_eq!(menu["props"]["icon"], "dots", "{menu}");
    for i in &items[1..] {
        assert_eq!((i["slot"].as_str(), i["menu"].as_str()), (Some("menu"), Some("more")), "{i}");
    }
    assert_eq!(page["menus"]["more"].as_array().map(Vec::len), Some(2), "the menu holds both: {page}");
    let content: Vec<&str> = page["widgets"].as_array().expect("widgets").iter().filter_map(|w| w["kind"].as_str()).collect();
    assert_eq!(content, vec!["custom"], "the Diary page is the Diary alone: {page}");
}
