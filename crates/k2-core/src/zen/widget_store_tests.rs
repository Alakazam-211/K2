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
    // 0.46.1 (gap 6): 10 custom widgets per page (was 6); the 11th is refused.
    let customs = |n: usize| {
        let mut s = String::from("schema = 1\n");
        for i in 0..n {
            s.push_str(&format!("\n[[widget]]\nid = \"w{i}\"\nkind = \"custom\"\nwidget = \"w{i}\"\ncolumn = 0\n"));
        }
        s
    };
    assert!(check(&customs(10)).is_clean(), "{:?}", check(&customs(10)).errors);
    let c = check(&customs(11));
    assert_eq!(c.errors.len(), 1, "{:?}", c.errors);
    assert!(c.errors[0].message.contains("at most 10 custom widgets"));
    let ok = check(&ARCADE_PAGE.replace("home = \"Work\"", "home = \"Work\"\nconfig = { speed = 2, title = \"Hi\", sound = false }"));
    assert!(ok.is_clean(), "{:?}", ok.errors);
    let page = crate::zen::garden_page(&ok.layer, schema::BLANK_TEMPLATE_ID);
    assert_eq!(page["widgets"][0]["props"]["config"], json!({"speed": 2, "title": "Hi", "sound": false}));
    assert_eq!(page["widgets"][1]["props"]["agents"], "arcade", "the Conversation follows the custom widget");
}

/// 0.46.1 (gap 6): `weight` sizes widgets stacked in a column's body; the
/// page answer carries it; bands, menus, chrome and column edges refuse it.
#[test]
fn widget_weight_in_a_column_body() {
    let check = |src: &str| {
        schema::check_garden("gardens/g-test0001.toml", src, crate::zen::builtin_layer(), schema::BLANK_TEMPLATE_ID)
    };
    let stacked = ARCADE_PAGE.replace("column = 0\n[widget.props]\nhome", "column = 0\nweight = 3\n[widget.props]\nhome").replace(
        "kind = \"conversation\"\ncolumn = 1\n",
        "kind = \"conversation\"\ncolumn = 0\nweight = 1\n",
    );
    let c = check(&stacked);
    assert!(c.is_clean(), "{:?}", c.errors);
    let page = crate::zen::garden_page(&c.layer, schema::BLANK_TEMPLATE_ID);
    assert_eq!(page["widgets"][0]["weight"], json!(3), "{page}");
    assert_eq!(page["widgets"][1]["weight"], json!(1), "{page}");
    assert!(check(ARCADE_PAGE).is_clean());
    let plain = crate::zen::garden_page(&check(ARCADE_PAGE).layer, schema::BLANK_TEMPLATE_ID);
    assert!(plain["widgets"][0].get("weight").is_none(), "no weight unless the file sets it: {plain}");
    for (bad, why) in [("weight = 0", "1 to 12"), ("weight = 13", "1 to 12"), ("weight = 1.5", "whole number"), ("weight = \"big\"", "whole number")] {
        let c = check(&ARCADE_PAGE.replace("column = 0\n[widget.props]\nhome", &format!("column = 0\n{bad}\n[widget.props]\nhome")));
        assert_eq!(c.errors.len(), 1, "{bad}: {:?}", c.errors);
        assert!(c.errors[0].message.contains(why), "{bad}: {}", c.errors[0].message);
    }
    let band = "schema = 1\n\n[[widget]]\nid = \"rail\"\nkind = \"nav-rail\"\nslot = \"top\"\nweight = 2\n";
    let c = check(band);
    assert!(c.errors.iter().any(|e| e.message.contains("has no column share")), "{:?}", c.errors);
    let edge = "schema = 1\n\n[[widget]]\nid = \"rail\"\nkind = \"nav-rail\"\ncolumn = 0\nedge = \"top\"\nweight = 2\n";
    let c = check(edge);
    assert!(c.errors.iter().any(|e| e.message.contains("column edge is a row")), "{:?}", c.errors);
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
    // chrome is one ⋯ menu, at the top LEFT ("move the top right one to
    // the left"), holding the two required controls, so exactly one Garden
    // switcher; no usage chip, no theme control.
    let page = crate::zen::template_page(&diary.template_id).expect("diary page");
    let items = page["chrome"]["items"].as_array().expect("chrome items");
    let kinds: Vec<&str> = items.iter().filter_map(|i| i["kind"].as_str()).collect();
    assert_eq!(kinds, vec!["menu", "garden-switcher", "zen-toggle"], "{page}");
    let menu = &items[0];
    assert_eq!((menu["slot"].as_str(), menu["align"].as_str()), (Some("top"), Some("start")), "{menu}");
    assert_eq!(kinds.iter().filter(|k| **k == "garden-switcher").count(), 1, "one Garden switcher: {page}");
    assert_eq!(menu["props"]["icon"], "dots", "{menu}");
    for i in &items[1..] {
        assert_eq!((i["slot"].as_str(), i["menu"].as_str()), (Some("menu"), Some("more")), "{i}");
    }
    assert_eq!(page["menus"]["more"].as_array().map(Vec::len), Some(2), "the menu holds both: {page}");
    let content: Vec<&str> = page["widgets"].as_array().expect("widgets").iter().filter_map(|w| w["kind"].as_str()).collect();
    assert_eq!(content, vec!["custom"], "the Diary page is the Diary alone: {page}");
}

/// 0.45.7 (Rosson 2026-10-15): New Garden → Kanban makes a full-canvas
/// Garden whose one widget is the built-in board, working at once with
/// its Garden data and Thread caps; its ⋯ menu holds the Garden switcher,
/// the theme picker and the Zen toggle. Setup never adds it.
#[test]
fn a_kanban_garden_from_the_catalog_is_the_board_with_its_caps() {
    let (_t, f) = set_up("kanban");
    assert!(f.find_garden("Kanban").is_none(), "setup doesn't preinstall the Kanban");
    let g = f.new_garden("Kanban", Some("kanban"), None, None).expect("new from catalog");
    assert_eq!(g.template, crate::zen::garden_catalog::KANBAN_TEMPLATE_ID);
    let page = f.resolve(Some(&g.id)).expect("resolve kanban");
    assert_eq!(page["page"]["layout"]["canvas"], "full", "{page}");
    let widgets = page["page"]["widgets"].as_array().expect("widgets");
    assert_eq!(widgets.len(), 1, "the board alone: {page}");
    let w = &widgets[0];
    assert_eq!((w["kind"].as_str(), w["widget"].as_str()), (Some("custom"), Some(crate::zen::builtin_widgets::KANBAN_WIDGET_LATEST)), "{page}");
    assert_eq!(w["origin"], "local");
    assert_eq!(
        w["caps"],
        json!(["agents:read", "thread:read", "thread:post", "store:read", "store:write"]),
        "the board works with zero clicks: {w}"
    );
    let items = page["page"]["chrome"]["items"].as_array().expect("chrome items");
    let kinds: Vec<&str> = items.iter().filter_map(|i| i["kind"].as_str()).collect();
    assert_eq!(kinds, vec!["menu", "garden-switcher", "theme-picker", "zen-toggle"], "{page}");
    assert_eq!((items[0]["slot"].as_str(), items[0]["align"].as_str()), (Some("top"), Some("start")));
    assert!(!kinds.contains(&"usage"), "no usage chip, as the Diary");
}

/// 0.45.2 (Rosson 2026-10-08, "not haunted enough"): a Diary shows the
/// creepier study, and the first image is still compiled in for a theme
/// that names it (an own copy on an archived set keeps its look; the
/// lookup is `background_json`'s).
#[test]
fn the_diary_shows_the_creepier_study_and_the_first_one_still_ships() {
    use base64::Engine as _;
    let (_t, f) = set_up("haunted-2");
    let haunted = crate::zen::builtin_theme("haunted").expect("haunted ships");
    let now = haunted.background.expect("a background");
    assert_eq!(now.file, "haunted-background-2.webp");
    assert!(haunted.toml.contains("image = \"haunted-background-2.webp\""));
    let old: Vec<&str> = haunted.older_backgrounds.iter().map(|b| b.file).collect();
    assert_eq!(old, ["haunted-background.webp"]);
    for b in std::iter::once(&now).chain(haunted.older_backgrounds) {
        assert!(b.bytes.starts_with(b"RIFF") && &b.bytes[8..12] == b"WEBP", "{} is a WebP", b.file);
        assert!((b.bytes.len() as u64) < crate::zen::schema::MAX_BACKGROUND_BYTES, "{}", b.file);
    }
    let served = |v: &J| -> Vec<u8> {
        let url = v["theme"]["background"]["dataUrl"].as_str().unwrap_or_else(|| panic!("a background: {}", v["theme"]));
        let b64 = url.strip_prefix("data:image/webp;base64,").expect("a WebP data URL");
        base64::engine::general_purpose::STANDARD.decode(b64).expect("base64")
    };
    let diary = gid(&f, "Diary");
    assert_eq!(served(&f.resolve(Some(&diary)).expect("resolve")), now.bytes, "a new Diary shows the new study");
}

/// decision-k2-runtime-v1 stage 0 (d)/(e): runtime errors reach agents and
/// validate loops: pending until a window runs the saved version, failed
/// with its runtime errors, ok once it ran clean; a new save starts over.
#[test]
fn runtime_errors_are_logged_per_version_and_validate_loops_on_them() {
    let (_t, f) = set_up("runs");
    f.new_widget("agent-arcade", Some("hello")).expect("new widget");
    let unplaced = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!(unplaced["status"], "ok", "not placed: nothing to wait for: {unplaced}");
    assert_eq!(unplaced["runtime"]["placed"], false);
    write_garden(&f, &gid(&f, "Garden 2"), ARCADE_PAGE);
    let v = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!(v["status"], "pending", "{v}");
    assert_eq!(v["ok"], true, "pending is not an error: {v}");
    let hash = v["runtime"]["hash"].as_str().expect("hash").to_string();

    let r = f.record_widget_run("agent-arcade", &hash, "ready", None, None).expect("ready");
    assert_eq!(r["logged"], true);
    let again = f.record_widget_run("agent-arcade", &hash, "ready", None, None).expect("ready again");
    assert_eq!(again["logged"], false, "one ready per version");
    assert_eq!(f.validate_widget("agent-arcade").expect("validate")["status"], "ok");

    f.record_widget_run("agent-arcade", &hash, "error", Some("ReferenceError: draw is not defined"), Some("at about:srcdoc:3:1"))
        .expect("error");
    let v = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!((v["status"].as_str(), v["ok"].as_bool()), (Some("failed"), Some(false)), "{v}");
    let msgs: Vec<&str> = v["errors"].as_array().expect("errors").iter().map(|e| e["message"].as_str().unwrap_or("")).collect();
    assert_eq!(msgs[0], "1 runtime error(s) since your last save", "{v}");
    assert!(msgs[1].starts_with("runtime error #2: ReferenceError"), "{v}");
    let logged = f.widget_errors("agent-arcade", None).expect("errors");
    assert!(logged["errors"][0]["suggestion"].as_str().is_some_and(|s| s.contains("<script src> order")), "{logged}");
    assert!(v["errors"][1]["suggestion"].as_str().is_some_and(|s| s.contains("<script src> order")), "{v}");
    let (all_errors, _, _) = f.validate_all_widgets();
    assert!(all_errors.iter().any(|d| d.message.contains("runtime error")), "{all_errors:?}");

    let e = f.widget_errors("agent-arcade", None).expect("errors");
    assert_eq!(e["errors"].as_array().map(Vec::len), Some(1), "{e}");
    assert_eq!(e["errors"][0]["current"], true);
    assert_eq!(e["errors"][0]["stack"], "at about:srcdoc:3:1");
    assert_eq!(e["last"], 2);
    let none = f.widget_errors("agent-arcade", Some(2)).expect("since");
    assert_eq!(none["errors"].as_array().map(Vec::len), Some(0), "{none}");

    // A new save is a new version: its errors start at zero and it waits to run.
    let js = f.widget_dir("agent-arcade").join("hello.js");
    let text = fs::read_to_string(&js).expect("read js") + "\n// saved again\n";
    fs::write(&js, text).expect("write js");
    let v = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!(v["status"], "pending", "{v}");
    assert_ne!(v["runtime"]["hash"].as_str(), Some(hash.as_str()));
    let e = f.widget_errors("agent-arcade", None).expect("errors");
    assert_eq!(e["errors"][0]["current"], false, "the old version's error: {e}");
}

/// Rewrite every logged run row's `at` (the log is the widget's
/// `runtime.jsonl`), to make errors old.
fn age_runs(f: &ZenFiles, name: &str, at: &str) {
    let p = f.widget_history_dir(name).join("runtime.jsonl");
    let text = fs::read_to_string(&p).expect("runtime log");
    let out: String = text
        .lines()
        .map(|l| {
            let mut r: J = serde_json::from_str(l).expect("row");
            r["at"] = J::String(at.into());
            r.to_string() + "\n"
        })
        .collect();
    fs::write(&p, out).expect("write runtime log");
}

/// 0.46.3: validate reflects what runs now. Runtime errors on a widget no
/// Garden places, or older than a day, are warnings (exit 0), not errors;
/// a placed widget's fresh errors on its saved version still fail.
#[test]
fn only_a_placed_widgets_fresh_runtime_errors_fail_validate() {
    let (_t, f) = set_up("runs-placed");
    f.new_widget("agent-arcade", Some("hello")).expect("new widget");
    let hash = f.validate_widget("agent-arcade").expect("validate")["runtime"]["hash"].as_str().expect("hash").to_string();
    f.record_widget_run("agent-arcade", &hash, "error", Some("ReferenceError: draw is not defined"), None).expect("error");

    // Unplaced: a warning that says why, never an error.
    let v = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!((v["status"].as_str(), v["ok"].as_bool()), (Some("ok"), Some(true)), "{v}");
    assert_eq!((v["runtime"]["placed"].as_bool(), v["runtime"]["staleErrors"].as_u64(), v["runtime"]["errorsSinceSave"].as_u64()), (Some(false), Some(1), Some(0)), "{v}");
    let w = v["warnings"].as_array().expect("warnings");
    assert!(w.iter().any(|d| d["message"].as_str().is_some_and(|m| m.contains("1 old runtime error(s)") && m.contains("no Garden places"))), "{v}");
    let (all_errors, all_warnings, _) = f.validate_all_widgets();
    assert!(!all_errors.iter().any(|d| d.message.contains("runtime error")), "unplaced never fails validate: {all_errors:?}");
    assert!(all_warnings.iter().any(|d| d.message.contains("old runtime error")), "{all_warnings:?}");
    let all = f.validate(None).expect("validate all");
    let row = all["widgets"].as_array().expect("widgets").iter().find(|r| r["widget"] == "agent-arcade").expect("row").clone();
    assert_eq!((row["status"].as_str(), row["stale"].as_u64(), row["failing"].as_u64()), (Some("unplaced"), Some(1), Some(0)), "{all}");
    assert_eq!(all["staleRuntime"], 1, "{all}");
    let e = f.widget_errors("agent-arcade", None).expect("errors");
    assert_eq!((e["errors"][0]["counted"].as_bool(), e["errors"][0]["staleReason"].as_str()), (Some(false), Some("unplaced")), "{e}");

    // Placed: the same fresh error fails, naming where it is placed.
    write_garden(&f, &gid(&f, "Garden 2"), ARCADE_PAGE);
    let v = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!((v["status"].as_str(), v["ok"].as_bool()), (Some("failed"), Some(false)), "{v}");
    assert_eq!(v["runtime"]["placements"][0]["gardenName"], "Garden 2", "{v}");
    assert_eq!(v["runtime"]["placements"][0]["placement"], "arcade", "{v}");
    let all = f.validate(None).expect("validate all");
    assert_eq!(all["ok"], false, "{all}");
    let row = all["widgets"].as_array().expect("widgets").iter().find(|r| r["widget"] == "agent-arcade").expect("row").clone();
    assert_eq!((row["status"].as_str(), row["failing"].as_u64(), row["placed"].as_bool()), (Some("failed"), Some(1), Some(true)), "{all}");

    // A day later the same error is old: a warning.
    let later = chrono::Utc::now() + chrono::Duration::seconds(runs::RUNTIME_FRESH_SECS + 60);
    let rt = f.widget_runtime_at("agent-arcade", &hash, later);
    assert!(rt.failing.is_empty(), "{rt:?}");
    assert_eq!(rt.stale.len(), 1);
    assert_eq!(rt.stale[0]["staleReason"], runs::STALE_OLD);
    age_runs(&f, "agent-arcade", "2026-10-17T00:26:54.000Z");
    let v = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!((v["status"].as_str(), v["ok"].as_bool(), v["runtime"]["staleErrors"].as_u64()), (Some("pending"), Some(true), Some(1)), "placed, old error, never ran: {v}");
    assert!(v["warnings"].as_array().expect("w").iter().any(|d| d["message"].as_str().is_some_and(|m| m.contains("since 2026-10-17") && m.contains("older than 24 h"))), "{v}");

    // A fresh error on top fails again.
    f.record_widget_run("agent-arcade", &hash, "error", Some("TypeError: x is null"), None).expect("error");
    assert_eq!(f.validate_widget("agent-arcade").expect("validate")["status"], "failed");
}

/// 0.46.3: `k2 zen widget errors <name> --clear` drops what validate no
/// longer counts; `--all` drops every error; ready rows and the sequence
/// survive.
#[test]
fn clearing_runtime_errors_keeps_what_still_counts_and_the_sequence() {
    let (_t, f) = set_up("runs-clear");
    f.new_widget("agent-arcade", Some("hello")).expect("new widget");
    write_garden(&f, &gid(&f, "Garden 2"), ARCADE_PAGE);
    let hash = f.validate_widget("agent-arcade").expect("validate")["runtime"]["hash"].as_str().expect("hash").to_string();
    f.record_widget_run("agent-arcade", &hash, "ready", None, None).expect("ready");
    f.record_widget_run("agent-arcade", &hash, "error", Some("old one"), None).expect("error");
    age_runs(&f, "agent-arcade", "2026-10-17T00:00:00.000Z");
    f.record_widget_run("agent-arcade", &hash, "error", Some("fresh one"), None).expect("error");
    let other = "b".repeat(64);
    f.record_widget_run("agent-arcade", &other, "error", Some("older version"), None).expect("error");

    let c = f.clear_widget_errors("agent-arcade", false).expect("clear");
    assert_eq!((c["cleared"].as_u64(), c["left"].as_u64()), (Some(2), Some(1)), "the old one and the other version's go: {c}");
    let e = f.widget_errors("agent-arcade", None).expect("errors");
    let msgs: Vec<&str> = e["errors"].as_array().expect("errors").iter().filter_map(|r| r["message"].as_str()).collect();
    assert_eq!(msgs, vec!["fresh one"], "{e}");
    assert_eq!(e["last"], 4, "the sequence never goes back: {e}");
    let v = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!((v["status"].as_str(), v["runtime"]["ran"].as_bool()), (Some("failed"), Some(true)), "ready kept, fresh error still counts: {v}");
    let n = f.record_widget_run("agent-arcade", &hash, "error", Some("next"), None).expect("error");
    assert_eq!(n["seq"], 5, "{n}");

    let c = f.clear_widget_errors("agent-arcade", true).expect("clear all");
    assert_eq!((c["cleared"].as_u64(), c["left"].as_u64()), (Some(2), Some(0)), "{c}");
    let v = f.validate_widget("agent-arcade").expect("validate");
    assert_eq!(v["status"], "ok", "ran, no errors left: {v}");
    let none = f.clear_widget_errors("agent-arcade", false).expect("clear again");
    assert_eq!(none["cleared"], 0);
    assert!(matches!(f.clear_widget_errors("k2:diary@2", false), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.clear_widget_errors("nope", false), Err(ZenError::NotFound(_))));
}

#[test]
fn run_reports_refuse_builtins_bad_hashes_and_unknown_events() {
    let (_t, f) = set_up("runs-bad");
    f.new_widget("w", Some("hello")).expect("new widget");
    let h = "a".repeat(64);
    assert!(matches!(f.record_widget_run("k2:diary@2", &h, "ready", None, None), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.record_widget_run("w", "abc", "ready", None, None), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.record_widget_run("w", &h, "crash", None, None), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.record_widget_run("nope", &h, "ready", None, None), Err(ZenError::NotFound(_))));
    let long = "x".repeat(5000);
    f.record_widget_run("w", &h, "error", Some(&long), Some(&long)).expect("long error");
    let e = f.widget_errors("w", None).expect("errors");
    assert_eq!(e["errors"][0]["message"].as_str().map(|m| m.chars().count()), Some(runs::MAX_MESSAGE + 1));
    assert_eq!(e["errors"][0]["stack"].as_str().map(|m| m.chars().count()), Some(runs::MAX_STACK + 1));
}

/// App runtime S9 (AR23): widget versions write deploy-audit events
/// (created, the new last-good, reset). The last-good behaviour above is
/// unchanged; only these events are new.
#[test]
fn widget_versions_reach_the_deploy_audit() {
    use crate::deploy::widget_events::{set_sink, WidgetEvent};
    use std::sync::{Arc, Mutex};
    let (_t, f) = set_up("s9audit");
    let got: Arc<Mutex<Vec<WidgetEvent>>> = Arc::default();
    let g2 = got.clone();
    set_sink(Some(Box::new(move |e| {
        if e.widget() == "s9-audit" {
            g2.lock().unwrap().push(e.clone());
        }
    })));
    f.new_widget("s9-audit", Some("arcade")).expect("new");
    f.refresh().expect("refresh");
    let hash = f.widget_status("s9-audit").hash().to_string();
    f.refresh().expect("refresh again: same content, no new event");
    fs::write(f.widget_dir("s9-audit").join("index.html"), "<script src=\"https://cdn.example.test/x.js\"></script>").expect("break");
    f.refresh().expect("refresh");
    f.reset_widget("s9-audit", None).expect("reset");
    set_sink(None);
    let events = got.lock().unwrap().clone();
    assert_eq!(events.len(), 3, "{events:?}");
    assert_eq!(events[0], WidgetEvent::Created { widget: "s9-audit".into(), from: "arcade".into() });
    assert_eq!(events[1], WidgetEvent::LastGood { widget: "s9-audit".into(), hash });
    assert!(matches!(&events[2], WidgetEvent::Reset { kept: Some(_), .. }), "{:?}", events[2]);
}

/// 0.46.3 r10 (prd-garden-column-collapse-and-agent-list-v1 A2, §8): an
/// Agents widget beside a Board, showing the agents the Board lists.
const LIST_PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 60\nmin-width = 200\n[[layout.column]]\nsize = 40\nmin-width = 200\n\n[[widget]]\nid = \"board\"\nkind = \"custom\"\nwidget = \"agent-arcade\"\ncolumn = 0\n\n[[widget]]\nid = \"crew\"\nkind = \"agents\"\ncolumn = 1\n[widget.props]\nlist-from = \"board\"\n";

fn page_errors(f: &ZenFiles, g: &str, text: &str) -> Vec<J> {
    write_garden(f, g, text);
    let v = f.validate(Some(&ZenFile::Garden(g.into()))).expect("validate");
    v["errors"].as_array().expect("errors").clone()
}

fn one_error(errs: &[J], line: u64, needle: &str, what: &str) {
    assert_eq!(errs.len(), 1, "{what}: want exactly one error: {errs:?}");
    assert_eq!(errs[0]["line"].as_u64(), Some(line), "{what}: {}", errs[0]);
    let m = errs[0]["message"].as_str().expect("message");
    assert!(m.contains(needle), "{what}: {m:?} must contain {needle:?}");
}

#[test]
fn list_from_names_a_custom_widget_that_reads_agents() {
    let (_t, f) = set_up("listfrom");
    let g = gid(&f, "Garden 2");
    f.new_widget("agent-arcade", Some("hello")).expect("new");

    // Clean: the mode is derived as `list`, and list-from rides along.
    assert_eq!(page_errors(&f, &g, LIST_PAGE), Vec::<J>::new());
    let page = f.resolve(Some(&g)).expect("resolve");
    let crew = &page["page"]["widgets"][1];
    assert_eq!(crew["kind"], "agents", "{page}");
    assert_eq!(crew["props"]["mode"], "list", "list-from makes it a list: {crew}");
    assert_eq!(crew["props"]["list-from"], "board", "{crew}");
    // mode = "list" said out loud is the same page.
    let said = LIST_PAGE.replace("[widget.props]\n", "[widget.props]\nmode = \"list\"\n");
    assert_eq!(page_errors(&f, &g, &said), Vec::<J>::new());

    // A Board that doesn't ask for agents:read: one error at list-from.
    let m = f.widget_dir("agent-arcade").join("manifest.json");
    let text = fs::read_to_string(&m)
        .expect("read")
        .replace("\"caps\": [\"agents:read\"]", "\"caps\": []")
        .replace(",\n  \"reasons\": {\n    \"agents:read\": \"to say hello to the first agent in this Home\"\n  }", "");
    fs::write(&m, text).expect("write");
    assert_eq!(f.widget_status("agent-arcade").state, WidgetState::Ok);
    let errs = page_errors(&f, &g, LIST_PAGE);
    one_error(&errs, 23, "lists from custom widget 'board', which doesn't ask for agents:read", "board without agents:read");
    assert_eq!(errs[0]["col"].as_u64(), Some(13), "{}", errs[0]);
}

#[test]
fn list_from_refusals_each_at_their_line() {
    let (_t, f) = set_up("listfrom-bad");
    let g = gid(&f, "Garden 2");
    f.new_widget("agent-arcade", Some("hello")).expect("new");

    let nope = LIST_PAGE.replace("list-from = \"board\"", "list-from = \"nope\"");
    one_error(&page_errors(&f, &g, &nope), 23, "lists from 'nope', which is not a custom widget on this page (board)", "dangling list-from");
    // An Agents widget is never a list's source (only a custom widget calls k2.agents.show).
    let itself = LIST_PAGE.replace("list-from = \"board\"", "list-from = \"crew\"");
    one_error(&page_errors(&f, &g, &itself), 23, "lists from 'crew', which is not a custom widget", "list-from an Agents widget");
    // mode = "list" needs list-from.
    let bare = LIST_PAGE.replace("list-from = \"board\"", "mode = \"list\"");
    one_error(&page_errors(&f, &g, &bare), 23, "mode = \"list\" needs list-from", "list mode without list-from");
    // list-from is its own mode: not home, not agent.
    let home_mode = LIST_PAGE.replace("[widget.props]\n", "[widget.props]\nmode = \"home\"\n");
    one_error(&page_errors(&f, &g, &home_mode), 23, "drop mode = \"home\"", "list-from with mode home");
    let agent_mode = LIST_PAGE.replace("[widget.props]\n", "[widget.props]\nmode = \"agent\"\nagent = \"cortana\"\n");
    let errs = page_errors(&f, &g, &agent_mode);
    assert_eq!(errs.len(), 2, "mode and agent both clash with list-from: {errs:?}");
    assert!(errs.iter().any(|e| e["message"].as_str().is_some_and(|m| m.contains("drop mode = \"agent\""))), "{errs:?}");
    assert!(errs.iter().any(|e| e["message"].as_str().is_some_and(|m| m.starts_with("agent picks"))), "{errs:?}");
    for prop in ["home = \"Work\"", "home-picker = true"] {
        let clash = LIST_PAGE.replace("[widget.props]\n", &format!("[widget.props]\n{prop}\n"));
        let name = prop.split(' ').next().expect("name");
        one_error(&page_errors(&f, &g, &clash), 23, &format!("{name} picks a Home or an agent"), prop);
    }
    // home-picker = false says nothing new: no clash.
    let off = LIST_PAGE.replace("[widget.props]\n", "[widget.props]\nhome-picker = false\n");
    assert_eq!(page_errors(&f, &g, &off), Vec::<J>::new());
    // list-from is a widget id (text), never a list.
    let list = LIST_PAGE.replace("list-from = \"board\"", "list-from = [\"board\"]");
    one_error(&page_errors(&f, &g, &list), 23, "list-from must be a name or id", "list-from as a list");
}

/// CA5: the key is absent unless the file sets it (zen/get stays the same
/// for every page without a list).
#[test]
fn an_agents_widget_without_list_from_carries_no_list_key() {
    let (_t, f) = set_up("listfrom-absent");
    let g = gid(&f, "Garden 2");
    let plain = "schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nhome = \"Work\"\n";
    assert_eq!(page_errors(&f, &g, plain), Vec::<J>::new());
    let page = f.resolve(Some(&g)).expect("resolve");
    let a = &page["page"]["widgets"][0];
    assert_eq!(a["props"]["mode"], "home", "{a}");
    assert!(a["props"].get("list-from").is_none(), "no list-from unless set: {a}");
    let g1 = gid(&f, "Garden 1");
    let page = f.resolve(Some(&g1)).expect("resolve Garden 1");
    for w in page["page"]["widgets"].as_array().expect("widgets") {
        assert!(w["props"].get("list-from").is_none(), "{w}");
    }
}
