//! The phone door and a Garden's mobile layout (contract-gardens-phone rev 2
//! §A–§C, §H): the merged hello, the height rule, `frameVersion`, what a
//! Garden with no `page.phone` still answers, and saves kept apart from the
//! desktop's. Temp HOME, a scoped DB, the door's own fixture. Fail loudly.

use serde_json::{json, Value as J};

use k2_core::zen::ZenFile;

use super::*;

const COLUMN: &str = "schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n";

/// `b1` with a desktop `config`, `b2` under it, both of widget `board`.
pub(super) const TWO: &str = "schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\
[[widget]]\nid = \"b1\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n[widget.props.config]\nsource = \"fleet\"\nzoom = 1\nbubble = true\n\
[[widget]]\nid = \"b2\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 0\n";

pub(super) fn write(fx: &Fx, text: &str) {
    std::fs::write(fx.d.f.path_of(&ZenFile::Garden(fx.d.garden.clone())), text).expect("write the Garden");
}

fn page_of(fx: &Fx) -> J {
    ok_of(PAGE, &fx.phone, &[("garden", &fx.d.garden)], J::Null)
}

fn frame_of(fx: &Fx, placement: &str) -> J {
    ok_of(FRAME, &fx.phone, &[("garden", &fx.d.garden), ("placement", placement)], J::Null)
}

/// `frameVersion` on the page's row, which the frame answer must repeat.
fn fv(fx: &Fx, placement: &str) -> String {
    let page = page_of(fx);
    let row = page["remote"]["widgets"].as_array().expect("rows").iter().find(|r| r["id"] == placement).cloned().unwrap_or_else(|| panic!("no row {placement}: {page}"));
    let on_page = row["frameVersion"].as_str().unwrap_or_else(|| panic!("no frameVersion: {row}")).to_string();
    assert_eq!(frame_of(fx, placement)["frameVersion"], json!(on_page), "the frame repeats the page's");
    on_page
}

// ── hello: view, config, height ─────────────────────────────────────

#[test]
fn the_hello_always_says_phone_and_the_phone_adds_only_the_viewport() {
    let fx = fx(&["store:read", "store:write"]);
    let hello = &frame_of(&fx, "b1")["hello"];
    assert_eq!(hello["view"], "phone", "{hello}");
    assert!(hello.get("viewport").is_none(), "the viewport is the phone's to add: {hello}");
}

/// `config` merges: the phone's key wins, the rest stays, so a canvas's
/// sources stay bound. A replace would have lost `source` and `bubble`.
#[test]
fn hello_config_is_the_phone_s_merged_over_the_desktop_s() {
    let fx = fx(&["store:read", "store:write"]);
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n[phone.screen.widget.config]\nzoom = 2\nextra = \"x\"\n"));
    let hello = &frame_of(&fx, "b1")["hello"];
    assert_eq!(hello["config"], json!({ "source": "fleet", "zoom": 2, "bubble": true, "extra": "x" }), "{hello}");
    // The page's own desktop config is untouched; the row is there to read.
    let page = page_of(&fx);
    assert_eq!(page["page"]["widgets"][0]["props"]["config"], json!({ "source": "fleet", "zoom": 1, "bubble": true }));
    assert_eq!(page["page"]["phone"]["screens"][0]["widgets"][0]["config"], json!({ "zoom": 2, "extra": "x" }));
}

#[test]
fn a_placement_with_no_phone_config_gets_the_desktop_s() {
    let fx = fx(&["store:read", "store:write"]);
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n"));
    assert_eq!(frame_of(&fx, "b1")["hello"]["config"], json!({ "source": "fleet", "zoom": 1, "bubble": true }));
}

#[test]
fn height_comes_from_the_row_then_its_screen() {
    let fx = fx(&["store:read", "store:write"]);
    let h = |fx: &Fx, p: &str| frame_of(fx, p)["hello"].get("height").cloned();
    // Alone on its screen, no height: fill. Stacked, no height: auto.
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b2\"\n"));
    assert_eq!(h(&fx, "b1"), Some(json!("fill")));
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n[[phone.screen.widget]]\nid = \"b2\"\n"));
    assert_eq!(h(&fx, "b1"), Some(json!("auto")));
    assert_eq!(h(&fx, "b2"), Some(json!("auto")));
    // What the row says wins.
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\nheight = 420\n[[phone.screen.widget]]\nid = \"b2\"\nheight = \"auto\"\n"));
    assert_eq!(h(&fx, "b1"), Some(json!(420)));
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\nheight = 420\n"));
    assert_eq!(h(&fx, "b1"), Some(json!(420)), "a px height alone on its screen");
}

/// A widget left out of `page.phone`: the frame still answers, with no height.
#[test]
fn a_placement_the_layout_leaves_out_gets_no_height_but_still_answers() {
    let fx = fx(&["store:read", "store:write"]);
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n"));
    let f = frame_of(&fx, "b2");
    assert!(f["hello"].get("height").is_none(), "{}", f["hello"]);
    assert_eq!(f["hello"]["view"], "phone");
    let page = page_of(&fx);
    let ids: Vec<&str> = page["remote"]["widgets"].as_array().expect("rows").iter().filter_map(|r| r["id"].as_str()).collect();
    assert_eq!(ids, ["b1", "b2"], "remote.widgets keeps every placement");
    assert!(page["remote"]["widgets"][1]["frameVersion"].is_string(), "{page}");
}

/// No `page.phone`: one column per screen, so the height comes from the
/// column. Native and placeholder widgets count; controls and band items don't.
#[test]
fn with_no_layout_height_comes_from_the_column() {
    let fx = fx(&["store:read", "store:write"]);
    let h = |fx: &Fx, p: &str| frame_of(fx, p)["hello"]["height"].clone();
    let custom = |id: &str, col: u8| format!("[[widget]]\nid = \"{id}\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = {col}\n");
    let two_cols = "schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 50\nmin-width = 200\n[[layout.column]]\nsize = 50\nmin-width = 200\n";
    write(&fx, &format!("{COLUMN}{}", custom("b1", 0)));
    assert_eq!(h(&fx, "b1"), json!("fill"), "alone in its column");
    write(&fx, &format!("{COLUMN}{}{}", custom("b1", 0), custom("b2", 0)));
    assert_eq!(h(&fx, "b1"), json!("auto"), "another custom widget");
    write(&fx, &format!("{two_cols}{}{}", custom("b1", 0), custom("b2", 1)));
    assert_eq!(h(&fx, "b1"), json!("fill"), "the other column doesn't count");
    write(&fx, &format!("{COLUMN}{}[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n", custom("b1", 0)));
    assert_eq!(h(&fx, "b1"), json!("auto"), "a native widget counts");
    write(&fx, &format!("{COLUMN}{}[[widget]]\nid = \"r\"\nkind = \"nav-rail\"\ncolumn = 0\n", custom("b1", 0)));
    assert_eq!(h(&fx, "b1"), json!("auto"), "a placeholder widget counts");
    write(&fx, &format!("{COLUMN}{}[[widget]]\nid = \"r\"\nkind = \"nav-rail\"\nslot = \"top\"\n", custom("b1", 0)));
    assert_eq!(h(&fx, "b1"), json!("fill"), "a band item doesn't");
    write(&fx, &format!("{COLUMN}{}[[widget]]\nkind = \"garden-switcher\"\ncolumn = 0\n[[widget]]\nkind = \"zen-toggle\"\ncolumn = 0\n", custom("b1", 0)));
    assert_eq!(h(&fx, "b1"), json!("fill"), "controls don't");
}

// ── frameVersion ────────────────────────────────────────────────────

/// The rev 1 bug this fixes: a `config` edit moved the page's `version` but
/// not `hash`, so a phone kept its frame and its old `hello.config`.
#[test]
fn frame_version_moves_on_config_and_height_and_on_nothing_else_on_the_page() {
    let fx = fx(&["store:read", "store:write"]);
    let phone = |row: &str| format!("{TWO}[[phone.screen]]\nlabel = \"One\"\n[[phone.screen.widget]]\nid = \"b1\"\n{row}\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b2\"\n");
    write(&fx, &phone(""));
    let base = fv(&fx, "b1");
    let other = fv(&fx, "b2");
    let hash = fx.d.hash();
    assert_eq!(frame_of(&fx, "b1")["hash"], json!(hash));

    // Same again: no change.
    assert_eq!(fv(&fx, "b1"), base);

    // Moves: the phone's config, the height, the desktop's config it merges.
    write(&fx, &phone("[phone.screen.widget.config]\nzoom = 2"));
    let with_config = fv(&fx, "b1");
    assert_ne!(with_config, base, "a phone config edit");
    assert_eq!(frame_of(&fx, "b1")["hash"], json!(hash), "hash is code only: it can't carry this");
    write(&fx, &phone("height = 420"));
    assert_ne!(fv(&fx, "b1"), base, "a height edit");
    write(&fx, &phone("").replace("zoom = 1", "zoom = 3"));
    assert_ne!(fv(&fx, "b1"), base, "a desktop config edit it merges");
    assert_eq!(fv(&fx, "b2"), other, "another placement's edit isn't this one's");

    // Doesn't move: everything else on the page, which still moves `version`.
    write(&fx, &phone(""));
    let version = page_of(&fx)["version"].clone();
    assert_eq!(fv(&fx, "b1"), base, "back to the same config is the same version");
    write(&fx, &phone("").replace("label = \"One\"", "label = \"Renamed\""));
    assert_eq!(fv(&fx, "b1"), base, "a screen label");
    assert_ne!(page_of(&fx)["version"], version, "the page's version did move");
    write(&fx, &phone("").replace("min-width = 200", "min-width = 240"));
    assert_eq!(fv(&fx, "b1"), base, "a desktop layout edit");
    write(&fx, &format!("{}\n[colors.light]\naccent = \"#2a4d8f\"\n", phone("")));
    assert_eq!(fv(&fx, "b1"), base, "a theme edit");
    // A desktop key the phone overrides merges to the same config.
    write(&fx, &phone("[phone.screen.widget.config]\nzoom = 2"));
    write(&fx, &phone("[phone.screen.widget.config]\nzoom = 2").replace("zoom = 1", "zoom = 9"));
    assert_eq!(fv(&fx, "b1"), with_config, "the same merged config");

    // The code: `hash` moves, so frameVersion does.
    write(&fx, &phone(""));
    std::fs::write(fx.d.f.widget_dir("board").join("hello.js"), "// new code\n").expect("edit the code");
    assert_ne!(frame_of(&fx, "b1")["hash"], json!(hash), "the bundle changed");
    assert_ne!(fv(&fx, "b1"), base, "new code");
}

/// A row with no good version has none; `remote/frame` answers 409.
#[test]
fn a_placement_with_no_good_version_has_no_frame_version() {
    let fx = fx(&["store:read", "store:write"]);
    write(&fx, &format!("{COLUMN}[[widget]]\nid = \"gone\"\nkind = \"custom\"\nwidget = \"no-such-folder\"\ncolumn = 0\n"));
    let page = page_of(&fx);
    assert_eq!(page["remote"]["widgets"], json!([{ "id": "gone", "run": "frame" }]), "{page}");
    assert_eq!(refused(FRAME, &fx.phone, &[("garden", &fx.d.garden), ("placement", "gone")], J::Null), (409, "widget_broken".into()));
    // A folder that has never bundled answers the same.
    fx.d.f.new_widget("never-good", Some("hello")).expect("folder");
    std::fs::write(fx.d.f.widget_dir("never-good").join("manifest.json"), "not json").expect("break it");
    write(&fx, &format!("{COLUMN}[[widget]]\nid = \"bad\"\nkind = \"custom\"\nwidget = \"never-good\"\ncolumn = 0\n"));
    assert_eq!(page_of(&fx)["remote"]["widgets"], json!([{ "id": "bad", "run": "frame" }]));
    assert_eq!(refused(FRAME, &fx.phone, &[("garden", &fx.d.garden), ("placement", "bad")], J::Null), (409, "widget_broken".into()));
}

/// Only frames carry it.
#[test]
fn native_and_placeholder_rows_have_no_frame_version() {
    let fx = fx(&["store:read", "store:write"]);
    write(&fx, &format!("{COLUMN}[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n[[widget]]\nid = \"r\"\nkind = \"nav-rail\"\ncolumn = 0\n"));
    assert_eq!(
        page_of(&fx)["remote"]["widgets"],
        json!([{ "id": "a", "run": "native" }, { "id": "r", "run": "placeholder", "reason": "desktop_only" }])
    );
}

// ── a Garden with no mobile version ─────────────────────────────────

/// Old phones keep working: the page has no `phone` key, and a page's answer
/// differs from desktop's only by `remote`, as rev 1 says.
#[test]
fn a_garden_with_no_layout_sends_no_phone_key() {
    let fx = fx(&["store:read", "store:write"]);
    write(&fx, TWO);
    let page = page_of(&fx);
    assert!(page["page"].get("phone").is_none(), "{}", page["page"]);
    assert_eq!(frame_of(&fx, "b1")["hello"]["config"], json!({ "source": "fleet", "zoom": 1, "bubble": true }));
}

/// One Garden, two versions: a Garden with a layout, or with none, or whose
/// layout dropped away, is on the list all the same.
#[test]
fn every_garden_stays_on_the_list_whatever_its_layout() {
    let fx = fx(&["store:read", "store:write"]);
    let count = fx.d.f.gardens().len();
    assert!(count >= 2, "the fixture has several Gardens");
    let list = |fx: &Fx| ok_of(GARDENS, &fx.phone, &[], J::Null)["gardens"].as_array().expect("gardens").len();
    assert_eq!(list(&fx), count);
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n"));
    assert_eq!(list(&fx), count, "with a layout");
    let row = ok_of(GARDENS, &fx.phone, &[], J::Null);
    let keys: Vec<&str> = row["gardens"][0].as_object().expect("a row").keys().map(String::as_str).collect();
    assert_eq!(keys, ["id", "index", "locked", "name", "template", "theme", "version"], "no per-device field: {row}");
}

/// A phone edit changes the page's `version`, so the feed tells the phone.
#[test]
fn a_layout_edit_moves_the_feed() {
    let fx = fx(&["store:read", "store:write"]);
    write(&fx, TWO);
    let before = ok_of(GARDENS, &fx.phone, &[], J::Null);
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n"));
    let after = ok_of(GARDENS, &fx.phone, &[("since", before["cursor"].as_str().expect("cursor"))], J::Null);
    assert_eq!(after["full"], true, "{after}");
    assert_ne!(after["gardens"][0]["version"], before["gardens"][0]["version"]);
}

// ── sources a phone's config binds ──────────────────────────────────

#[test]
fn what_a_phone_binds_is_listed_apart_and_the_desktop_s_list_does_not_widen() {
    let fx = fx(&["sources:read"]);
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n[phone.screen.widget.config]\nlatest = \"phone-only\"\n"));
    let page = page_of(&fx);
    let b1 = &page["page"]["widgets"][0];
    assert_eq!(b1["sourceAddresses"], json!(["fleet"]), "the desktop frame's own list: {b1}");
    assert_eq!(b1["phoneSourceAddresses"], json!(["phone-only"]), "{b1}");
    let b2 = &page["page"]["widgets"][1];
    assert!(b2.get("phoneSourceAddresses").is_none(), "only when there are any: {b2}");
    let locked_on = k2_core::zen::ZenFiles::local();
    locked_on.set_garden_lock(&fx.d.garden, true).expect("lock");
    let names: Vec<String> = locked_on.locked_placement_sources_for(None).into_iter().collect();
    assert_eq!(names, ["fleet", "phone-only"], "a lock protects what the phone reads too");
    assert_eq!(fx.d.f.configured_source_addresses("board"), ["fleet", "phone-only"]);
    let show = fx.d.f.widget_show("board", &k2_core::zen::widget_access::WidgetPauses::empty()).expect("show");
    assert_eq!(show["placements"][0]["sources"], json!(["fleet", "phone-only"]), "{show}");
}

/// A phone `config` can override a key, and the override is what's bound there.
#[test]
fn an_override_binds_the_phone_s_address_and_both_stay_protected() {
    let fx = fx(&["sources:read"]);
    write(&fx, &format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n[phone.screen.widget.config]\nsource = \"small\"\n"));
    let page = page_of(&fx);
    let b1 = &page["page"]["widgets"][0];
    assert_eq!(b1["sourceAddresses"], json!(["fleet"]));
    assert_eq!(b1["phoneSourceAddresses"], json!(["small"]));
}

/// A bad address in a phone's `config` is the same finding as on the desktop.
#[test]
fn a_bad_address_in_a_phone_config_is_reported() {
    let fx = fx(&["sources:read"]);
    let text = format!("{TWO}[[phone.screen]]\n[[phone.screen.widget]]\nid = \"b1\"\n[phone.screen.widget.config]\nlatest = \"not an address\"\n");
    write(&fx, &text);
    let page = page_of(&fx);
    let errors = page["errors"].as_array().expect("errors");
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0]["message"].as_str().is_some_and(|m| m.starts_with("'not an address' isn't a source address")), "{errors:?}");
    let line = text.lines().position(|l| l.starts_with("latest")).expect("the line") + 1;
    assert_eq!(errors[0]["line"], json!(line), "{errors:?}");
}

// ── saves on a phone (§H) ───────────────────────────────────────────

fn phone_call(fx: &Fx, verb: &str, args: J) -> J {
    ok_of(CALL, &fx.phone, &[], call_body(fx, verb, args))["result"].clone()
}

#[test]
fn a_phone_s_save_never_touches_the_desktop_s() {
    let fx = fx(&["store:read", "store:write"]);
    fx.d.call("save.set", json!(["camera", { "zoom": 1 }]));
    // A read of a key the phone hasn't set falls back to the desktop's.
    assert_eq!(phone_call(&fx, "save.get", json!(["camera"])), json!({ "zoom": 1 }));
    // A phone write is the phone's: the desktop still reads its own.
    phone_call(&fx, "save.set", json!(["camera", { "zoom": 3 }]));
    assert_eq!(phone_call(&fx, "save.get", json!(["camera"])), json!({ "zoom": 3 }));
    assert_eq!(fx.d.call("save.get", json!(["camera"])), json!({ "zoom": 1 }), "panning on the phone didn't move the desktop");
    // A later desktop write doesn't move the phone's.
    fx.d.call("save.set", json!(["camera", { "zoom": 2 }]));
    assert_eq!(phone_call(&fx, "save.get", json!(["camera"])), json!({ "zoom": 3 }));
    // Remove takes only the phone's: the desktop's shows through again.
    phone_call(&fx, "save.remove", json!(["camera"]));
    assert_eq!(fx.d.call("save.get", json!(["camera"])), json!({ "zoom": 2 }), "a phone's remove never reaches the desktop");
    assert_eq!(phone_call(&fx, "save.get", json!(["camera"])), json!({ "zoom": 2 }));
    // Neither has it: null, on both.
    assert_eq!(phone_call(&fx, "save.get", json!(["nothing"])), J::Null);
}

#[test]
fn a_phone_s_save_is_the_desktop_s_null_too() {
    let fx = fx(&["store:read", "store:write"]);
    fx.d.call("save.set", json!(["k", 1]));
    phone_call(&fx, "save.set", json!(["k", null]));
    assert_eq!(phone_call(&fx, "save.get", json!(["k"])), J::Null, "a saved null is the phone's value, not a miss");
    assert_eq!(fx.d.call("save.get", json!(["k"])), json!(1));
}

#[test]
fn save_keys_lists_both_on_a_phone_and_only_its_own_on_the_desktop() {
    let fx = fx(&["store:read", "store:write"]);
    fx.d.call("save.set", json!(["both", 1]));
    fx.d.call("save.set", json!(["desk", 1]));
    phone_call(&fx, "save.set", json!(["both", 2]));
    phone_call(&fx, "save.set", json!(["phone", 2]));
    phone_call(&fx, "save.set", json!(["other/x", 2]));
    assert_eq!(phone_call(&fx, "save.keys", json!([])), json!(["both", "desk", "other/x", "phone"]), "sorted, once each");
    assert_eq!(fx.d.call("save.keys", json!([])), json!(["both", "desk"]), "the desktop is untouched");
    assert_eq!(phone_call(&fx, "save.keys", json!([{ "prefix": "other/" }])), json!(["other/x"]));
}

/// Every phone shares one set of saves: a second phone reads the first's.
#[test]
fn a_person_s_phones_share_their_saves() {
    let fx = fx(&["store:read", "store:write"]);
    let second = pair("rosphone", "ios-rosson-2");
    phone_call(&fx, "save.set", json!(["camera", 9]));
    let v = ok_of(CALL, &second, &[], call_body(&fx, "save.get", json!(["camera"])));
    assert_eq!(v["result"], json!(9));
}

/// `store.*` is the widget's data, one for every device.
#[test]
fn the_store_stays_shared() {
    let fx = fx(&["store:read", "store:write"]);
    phone_call(&fx, "store.put", json!(["card/1", { "t": "from a phone" }]));
    assert_eq!(fx.d.call("store.get", json!(["card/1"]))["value"], json!({ "t": "from a phone" }));
}

/// The desktop's door has no way to name a phone's space.
#[test]
fn no_widget_reaches_the_phone_s_saves_by_name() {
    let fx = fx(&["store:read", "store:write"]);
    phone_call(&fx, "save.set", json!(["secret-pan", 1]));
    assert_eq!(fx.d.call("save.get", json!(["secret-pan"])), J::Null);
    assert_eq!(fx.d.call("save.keys", json!([])), json!([]));
    assert!(k2_core::garden_data::Space::save("phone~board").is_err());
}

/// Bad keys are refused the same on either door.
#[test]
fn a_bad_key_refuses_on_a_phone_as_on_a_desktop() {
    let fx = fx(&["store:read", "store:write"]);
    let (s, code) = refused(CALL, &fx.phone, &[], call_body(&fx, "save.set", json!(["/bad", 1])));
    assert_eq!((s, code.as_str()), (400, fx.d.refuse("save.set", json!(["/bad", 1])).as_str()));
}

/// Deleting or moving a widget's data takes the phone's saves with the rest.
#[test]
fn a_phone_s_saves_go_with_the_widget_s_data() {
    use crate::garden_data::agent_routes::tests::ok as data;
    use crate::zen_data_routes::DataWho;
    let fx = fx(&["store:read", "store:write"]);
    phone_call(&fx, "save.set", json!(["camera", 9]));
    fx.d.call("save.set", json!(["camera", 1]));
    let listed = data(&DataWho::Owner, "GET", "/cli/zen/data/spaces", json!({}), &[]);
    let saves = listed["saves"].as_array().expect("saves");
    assert_eq!(saves.len(), 1, "one row a widget: {saves:?}");
    assert_eq!(saves[0]["widget"], "board", "{saves:?}");
    assert_eq!(saves[0]["keys"], json!(2), "both devices' keys: {saves:?}");
    data(&DataWho::Owner, "POST", "/cli/zen/data/space/delete", json!({ "widget": "board", "confirm": "board" }), &[]);
    assert_eq!(phone_call(&fx, "save.get", json!(["camera"])), J::Null, "gone with the widget's data");
}
