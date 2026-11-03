//! `[phone]`, a Garden's mobile layout (contract-gardens-phone rev 2 §A):
//! the wire shape, every §A.5 refusal with its exact text and position, the
//! warnings, the template rules, and that a Garden with no `[phone]` answers
//! as it always did. Every Zen folder is under the temp dir. Fail loudly.

use std::fs;
use std::path::PathBuf;

use serde_json::{json, Value as J};

use crate::zen::schema::{self, Checked, FileKind};
use crate::zen::{self, Defaults, ZenError, ZenFile, ZenFiles, BLANK_TEMPLATE_ID, TEMPLATE_ID};

/// A page of its own: Agents, a Conversation, a rail and two custom cards.
const OWN: &str = "schema = 1\n\
[layout]\nkind = \"columns\"\n\
[[layout.column]]\nsize = 40\nmin-width = 200\n\
[[layout.column]]\nsize = 60\nmin-width = 200\n\
[[widget]]\nid = \"agents\"\nkind = \"agents\"\ncolumn = 0\n\
[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 0\n\
[[widget]]\nid = \"rail\"\nkind = \"nav-rail\"\ncolumn = 0\n\
[[widget]]\nid = \"fleet\"\nkind = \"custom\"\nwidget = \"fleet-canvas\"\ncolumn = 1\n\
[widget.props.config]\nsource = \"a,b\"\nbubble = true\nzoom = 1\n\
[[widget]]\nid = \"board\"\nkind = \"custom\"\nwidget = \"board\"\ncolumn = 1\n";

fn check(text: &str) -> Checked {
    schema::check_garden("gardens/g-t.toml", text, zen::builtin_layer(), BLANK_TEMPLATE_ID)
}

fn own(phone: &str) -> String {
    format!("{OWN}{phone}")
}

/// The line, 1-based, of the last line of `text` that starts with `needle`
/// (the fixture's own widgets come first, `[phone]` last).
fn line_of(text: &str, needle: &str) -> usize {
    let lines: Vec<&str> = text.lines().collect();
    lines.iter().rposition(|l| l.trim_start().starts_with(needle)).unwrap_or_else(|| panic!("no {needle} in:\n{text}")) + 1
}

fn only_error(c: &Checked) -> String {
    assert_eq!(c.errors.len(), 1, "exactly one error: {:?}", c.errors);
    c.errors[0].message.clone()
}

/// The file refuses with exactly `message`, on the line starting `at`.
fn refused(phone: &str, at: &str, message: &str) {
    let text = own(phone);
    let c = check(&text);
    assert_eq!(only_error(&c), message, "{phone}");
    assert_eq!(c.errors[0].file, "gardens/g-t.toml", "{phone}");
    assert_eq!(c.errors[0].line, line_of(&text, at), "{phone}: {:?}", c.errors[0]);
    assert!(c.errors[0].col >= 1, "{:?}", c.errors[0]);
}

// ── the shape ───────────────────────────────────────────────────────

/// Contract §A.1 and §A.2: the TOML there gives the JSON there.
#[test]
fn the_contract_s_toml_gives_the_contract_s_wire_shape() {
    let text = own(
        "[[phone.screen]]\nlabel = \"Fleet\"\ncanvas = \"full\"\n\
[[phone.screen.widget]]\nid = \"fleet\"\nheight = \"fill\"\n[phone.screen.widget.config]\nbubble = false\n\
[[phone.screen]]\nlabel = \"Talk\"\n\
[[phone.screen.widget]]\nid = \"agents\"\nheight = \"auto\"\n\
[[phone.screen.widget]]\nid = \"talk\"\nheight = 420\n",
    );
    let c = check(&text);
    assert!(c.is_clean(), "{:?}", c.errors);
    assert!(c.authoring.is_empty(), "{:?}", c.authoring);
    let page = zen::garden_page(&c.layer, BLANK_TEMPLATE_ID);
    assert_eq!(
        page["phone"],
        json!({
            "screens": [
                { "label": "Fleet", "canvas": "full", "widgets": [{ "id": "fleet", "height": "fill", "config": { "bubble": false } }] },
                { "label": "Talk", "widgets": [{ "id": "agents", "height": "auto" }, { "id": "talk", "height": 420 }] },
            ]
        })
    );
}

/// `canvas = "framed"` is the default, so it is never sent; `height` and
/// `label` are only sent when set.
#[test]
fn only_what_the_file_set_is_sent() {
    let c = check(&own("[[phone.screen]]\ncanvas = \"framed\"\n[[phone.screen.widget]]\nid = \"board\"\n"));
    assert!(c.is_clean(), "{:?}", c.errors);
    assert_eq!(zen::garden_page(&c.layer, BLANK_TEMPLATE_ID)["phone"], json!({ "screens": [{ "widgets": [{ "id": "board" }] }] }));
}

/// A widget left out of `[phone]` stays on the page: only the layout skips it.
#[test]
fn a_widget_left_out_stays_in_page_widgets() {
    let c = check(&own("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\n"));
    let page = zen::garden_page(&c.layer, BLANK_TEMPLATE_ID);
    let ids: Vec<&str> = page["widgets"].as_array().expect("widgets").iter().filter_map(|w| w["id"].as_str()).collect();
    assert_eq!(ids, vec!["agents", "talk", "rail", "fleet", "board"]);
}

/// The twelve-screen limit is inclusive.
#[test]
fn twelve_screens_are_fine() {
    let mut phone = String::new();
    let mut widgets = String::new();
    // A page holds 12 content widgets, at most 10 of them custom.
    for i in 0..12 {
        let kind = if i < 10 { "kind = \"custom\"\nwidget = \"board\"" } else { "kind = \"agents\"" };
        widgets.push_str(&format!("[[widget]]\nid = \"c{i}\"\n{kind}\ncolumn = 0\n"));
        phone.push_str(&format!("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"c{i}\"\n"));
    }
    let c = check(&format!("schema = 1\n{widgets}{phone}"));
    assert!(c.is_clean(), "{:?}", c.errors);
    assert_eq!(zen::garden_page(&c.layer, BLANK_TEMPLATE_ID)["phone"]["screens"].as_array().map(Vec::len), Some(12));
}

// ── §A.5: every refusal, exact text and position ────────────────────

#[test]
fn no_screens_refuses() {
    refused("[phone]\n", "[phone]", "[phone] needs at least one [[phone.screen]]");
}

#[test]
fn over_twelve_screens_refuses() {
    let phone = "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\n".repeat(13);
    let text = own(&phone);
    let c = check(&text);
    assert_eq!(only_error(&c), "a phone layout has 1 to 12 screens; this one has 13");
}

#[test]
fn an_empty_screen_refuses() {
    refused(
        "[[phone.screen]]\nlabel = \"Empty\"\n",
        "label =",
        "phone screen 1 has no widgets; add [[phone.screen.widget]] or drop the screen",
    );
}

#[test]
fn unknown_keys_refuse_at_each_level() {
    refused("[phone]\nscreens = 1\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\n", "screens", "unknown key 'screens' in [phone]; did you mean 'screen'?");
    refused("[phone]\nzz = 1\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\n", "zz", "unknown key 'zz' in [phone]; allowed: screen");
    refused(
        "[[phone.screen]]\nzzzz = 1\n[[phone.screen.widget]]\nid = \"board\"\n",
        "zzzz",
        "unknown key 'zzzz' in [[phone.screen]]; allowed: label, canvas, widget",
    );
    refused(
        "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\nzzzz = 1\n",
        "zzzz",
        "unknown key 'zzzz' in [[phone.screen.widget]]; allowed: id, height, config",
    );
}

#[test]
fn a_bad_label_refuses() {
    let m = "screen label is text, 1 to 24 characters, with no control characters (it names the screen's tab)";
    let long = format!("[[phone.screen]]\nlabel = \"{}\"\n[[phone.screen.widget]]\nid = \"board\"\n", "x".repeat(25));
    refused(&long, "label", m);
    refused("[[phone.screen]]\nlabel = \"\"\n[[phone.screen.widget]]\nid = \"board\"\n", "label", m);
    refused("[[phone.screen]]\nlabel = \"  \"\n[[phone.screen.widget]]\nid = \"board\"\n", "label", m);
    refused("[[phone.screen]]\nlabel = \"a\\nb\"\n[[phone.screen.widget]]\nid = \"board\"\n", "label", m);
    refused("[[phone.screen]]\nlabel = 3\n[[phone.screen.widget]]\nid = \"board\"\n", "label", m);
    // 24 characters, in characters not bytes.
    let c = check(&own(&format!("[[phone.screen]]\nlabel = \"{}\"\n[[phone.screen.widget]]\nid = \"board\"\n", "é".repeat(24))));
    assert!(c.is_clean(), "{:?}", c.errors);
}

#[test]
fn a_bad_canvas_refuses() {
    refused(
        "[[phone.screen]]\ncanvas = \"wide\"\n[[phone.screen.widget]]\nid = \"board\"\n",
        "canvas",
        "screen canvas must be one of: framed, full",
    );
}

#[test]
fn a_full_canvas_with_several_widgets_refuses() {
    refused(
        "[[phone.screen]]\ncanvas = \"full\"\n[[phone.screen.widget]]\nid = \"board\"\n[[phone.screen.widget]]\nid = \"fleet\"\n",
        "canvas",
        "a full-canvas screen holds one widget; this one has 2",
    );
}

#[test]
fn no_id_refuses() {
    refused("[[phone.screen]]\n[[phone.screen.widget]]\nheight = \"auto\"\n", "height", "each [[phone.screen.widget]] needs an id");
    refused("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"\"\n", "id", "each [[phone.screen.widget]] needs an id");
    refused("[[phone.screen]]\n[[phone.screen.widget]]\nid = 3\n", "id", "each [[phone.screen.widget]] needs an id");
}

#[test]
fn an_id_not_on_the_page_refuses_against_the_file_s_own_widgets() {
    let phone = "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"nope\"\n";
    refused(phone, "id = \"nope\"", "phone screen 1 names widget 'nope', which is not on this page");
    assert!(check(&own(phone)).authoring.is_empty(), "it's a plain error here, not an authoring finding");
}

#[test]
fn a_widget_on_two_screens_refuses() {
    refused(
        "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"fleet\"\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\nheight = 300\n",
        "id = \"board\"",
        "widget 'board' is on phone screen 1 and screen 3; a widget sits on one screen",
    );
    let text = own("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\n[[phone.screen.widget]]\nid = \"board\"\n");
    assert_eq!(only_error(&check(&text)), "widget 'board' is twice on phone screen 1; a widget sits on one screen");
}

#[test]
fn a_control_refuses() {
    let text = format!(
        "{OWN}[[widget]]\nkind = \"garden-switcher\"\nslot = \"top\"\n[[widget]]\nkind = \"zen-toggle\"\nslot = \"top\"\n[[widget]]\nid = \"more\"\nkind = \"menu\"\nslot = \"top\"\n[[widget]]\nkind = \"theme-picker\"\nslot = \"menu\"\nmenu = \"more\"\n\
[[phone.screen]]\n[[phone.screen.widget]]\nid = \"more\"\n"
    );
    let c = check(&text);
    assert_eq!(only_error(&c), "'more' is a control, not a widget; the phone draws its own controls");
}

#[test]
fn a_bad_height_refuses() {
    let m = "widget height is \"fill\", \"auto\" or a number of px, 160 to 4000";
    for bad in ["159", "4001", "0", "-200", "\"tall\"", "true", "[1]", "nan"] {
        refused(&format!("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\nheight = {bad}\n"), "height", m);
    }
    for good in ["160", "4000", "420", "420.5", "\"auto\"", "\"fill\""] {
        let c = check(&own(&format!("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\nheight = {good}\n")));
        assert!(c.is_clean(), "{good}: {:?}", c.errors);
    }
}

#[test]
fn fill_in_a_stack_refuses() {
    refused(
        "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\nheight = \"fill\"\n[[phone.screen.widget]]\nid = \"fleet\"\n",
        "height",
        "height = \"fill\" needs the widget alone on its screen; use \"auto\" or a px height",
    );
}

#[test]
fn config_on_a_built_in_kind_refuses() {
    refused(
        "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"talk\"\n[phone.screen.widget.config]\nx = 1\n",
        "[phone.screen.widget.config]",
        "config is only for a custom widget; 'talk' is a conversation widget",
    );
}

#[test]
fn a_config_value_that_is_not_a_scalar_refuses() {
    let base = "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"fleet\"\n[phone.screen.widget.config]\n";
    refused(&format!("{base}x = [1]\n"), "x =", "config.x must be a string, a number or true/false (no lists or tables)");
    refused(&format!("{base}x = {{ y = 1 }}\n"), "x =", "config.x must be a string, a number or true/false (no lists or tables)");
    let c = check(&own("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"fleet\"\nconfig = 3\n"));
    assert_eq!(only_error(&c), "`phone.screen.widget.config` must be a table, like [phone.screen.widget.config]");
}

/// The limit counts the merged JSON, the desktop's `config` included.
#[test]
fn a_config_too_long_merged_refuses() {
    let phone = "[[phone.screen]]\n[[phone.screen.widget]]\nid = \"fleet\"\n[phone.screen.widget.config]\n";
    let c = check(&own(&format!("{phone}x = \"{}\"\n", "y".repeat(3970))));
    assert!(c.is_clean(), "{:?}", c.errors);
    let over = "y".repeat(4100);
    let c = check(&own(&format!("{phone}x = \"{over}\"\n")));
    let m = only_error(&c);
    let want = serde_json::from_str::<J>(&format!("{{\"source\":\"a,b\",\"bubble\":true,\"zoom\":1,\"x\":\"{over}\"}}")).expect("json").to_string().len();
    assert_eq!(m, format!("config is {want} bytes as JSON with the desktop config merged in; a custom widget's config is at most 4096 (4 KB)"));
}

#[test]
fn phone_belongs_only_in_a_garden_file() {
    let want = "[phone] belongs in a Garden file (~/.k2/zen/gardens/<id>.toml); list them with k2 zen garden list";
    let text = "schema = 1\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"x\"\n";
    let c = schema::check("zen.toml", text, FileKind::Zen, zen::builtin_layer());
    assert_eq!(only_error(&c), want);
    let c = schema::check("themes/x/theme.toml", text, FileKind::Theme, zen::builtin_layer());
    assert_eq!(only_error(&c), want);
}

// ── warnings ────────────────────────────────────────────────────────

#[test]
fn a_placeholder_widget_warns_and_stays_listed() {
    let c = check(&own("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"rail\"\n"));
    assert!(c.is_clean(), "{:?}", c.errors);
    assert!(c.authoring.is_empty(), "{:?}", c.authoring);
    assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
    assert_eq!(c.warnings[0].message, "'rail' is a nav-rail widget: the phone shows it as \"open this on your computer\"");
    assert_eq!(zen::garden_page(&c.layer, BLANK_TEMPLATE_ID)["phone"]["screens"][0]["widgets"][0]["id"], "rail");
}

#[test]
fn a_conversation_without_its_agents_widget_warns() {
    let c = check(&own("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"talk\"\n"));
    assert!(c.is_clean(), "{:?}", c.errors);
    assert_eq!(c.warnings.len(), 1, "{:?}", c.warnings);
    assert_eq!(c.warnings[0].message, "conversation 'talk' follows 'agents', which isn't in [phone]: the phone can't pick its agent. Add 'agents' to a screen");
    let c = check(&own("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"talk\"\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"agents\"\n"));
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
}

// ── against a template ──────────────────────────────────────────────

fn on_texting(phone: &str) -> Checked {
    schema::check_garden("gardens/g-t.toml", &format!("schema = 1\n{phone}"), zen::builtin_layer(), TEMPLATE_ID)
}

/// A file with only `[phone]` keeps the template's page and carries the layout.
#[test]
fn a_file_with_only_phone_keeps_the_template_s_page() {
    let c = on_texting("[[phone.screen]]\nlabel = \"Agents\"\n[[phone.screen.widget]]\nid = \"agents\"\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"conversation\"\n");
    assert!(c.is_clean(), "{:?}", c.errors);
    assert!(c.warnings.is_empty(), "{:?}", c.warnings);
    let page = zen::garden_page(&c.layer, TEMPLATE_ID);
    let mut bare = page.clone();
    bare.as_object_mut().expect("a page").remove("phone");
    assert_eq!(bare, zen::texting_page(), "everything but the layout is the template's");
    assert_eq!(page["phone"]["screens"][0]["label"], "Agents");
}

/// A K2 update may move a template under a good file: what no longer fits is
/// a warning and is left out, never an error. New text saying the same is
/// refused (`authoring`).
#[test]
fn against_a_template_a_dangling_id_warns_and_is_dropped() {
    let c = on_texting("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"agents\"\n[[phone.screen.widget]]\nid = \"gone\"\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"also-gone\"\n");
    assert!(c.is_clean(), "{:?}", c.errors);
    let want = vec![
        "phone screen 1 names widget 'gone', which is not on this page".to_string(),
        "phone screen 2 names widget 'also-gone', which is not on this page".to_string(),
    ];
    assert_eq!(c.warnings.iter().map(|d| d.message.clone()).collect::<Vec<_>>(), want);
    assert_eq!(c.authoring.iter().map(|d| d.message.clone()).collect::<Vec<_>>(), want);
    let page = zen::garden_page(&c.layer, TEMPLATE_ID);
    assert_eq!(page["phone"], json!({ "screens": [{ "widgets": [{ "id": "agents" }] }] }), "the id and its empty screen went");
}

#[test]
fn no_screens_left_drops_the_layout() {
    let c = on_texting("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"gone\"\n");
    assert!(c.is_clean(), "{:?}", c.errors);
    assert!(zen::garden_page(&c.layer, TEMPLATE_ID).get("phone").is_none(), "falls back to the default layout");
}

#[test]
fn against_a_template_a_control_or_a_built_in_s_config_warns_and_is_left_out() {
    let c = on_texting("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"zen-toggle\"\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"agents\"\n[phone.screen.widget.config]\nx = 1\n");
    assert!(c.is_clean(), "{:?}", c.errors);
    assert_eq!(
        c.authoring.iter().map(|d| d.message.clone()).collect::<Vec<_>>(),
        vec![
            "'zen-toggle' is a control, not a widget; the phone draws its own controls".to_string(),
            "config is only for a custom widget; 'agents' is an agents widget".to_string(),
        ]
    );
    let page = zen::garden_page(&c.layer, TEMPLATE_ID);
    assert_eq!(page["phone"], json!({ "screens": [{ "widgets": [{ "id": "agents" }] }] }));
}

// ── a template's own [phone] ────────────────────────────────────────

const TEMPLATE_WITH_PHONE: &str = "[template]\nid = \"k2.test@1\"\n\
[layout]\nkind = \"columns\"\n\
[[layout.column]]\nsize = 100\nmin-width = 200\n\
[[widget]]\nid = \"agents\"\nkind = \"agents\"\ncolumn = 0\n\
[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 0\n\
[[phone.screen]]\nlabel = \"Agents\"\ncanvas = \"framed\"\n[[phone.screen.widget]]\nid = \"agents\"\nheight = \"fill\"\n\
[[phone.screen]]\ncanvas = \"full\"\n[[phone.screen.widget]]\nid = \"talk\"\n";

#[test]
fn a_template_may_carry_phone() {
    let page = zen::build_template_page_in(Defaults::live(), "k2.test@1", TEMPLATE_WITH_PHONE);
    assert_eq!(
        page["phone"],
        json!({ "screens": [
            { "label": "Agents", "widgets": [{ "id": "agents", "height": "fill" }] },
            { "canvas": "full", "widgets": [{ "id": "talk" }] },
        ] })
    );
    assert!(zen::texting_page().get("phone").is_none());
    assert!(zen::template_page(BLANK_TEMPLATE_ID).expect("blank").get("phone").is_none());
}

/// The file's own widgets replace a template's, and with them its layout.
#[test]
fn a_template_s_layout_goes_with_its_widgets() {
    let mut page = zen::build_template_page_in(Defaults::live(), "k2.test@1", TEMPLATE_WITH_PHONE);
    zen::phone::settle(&mut page, None, false);
    assert!(page.get("phone").is_some(), "the template's own stands");
    zen::phone::settle(&mut page, None, true);
    assert!(page.get("phone").is_none(), "gone when the file replaced the widgets");
}

/// Any template K2 ships that carries `[phone]` must check clean as a file
/// with only that table (none does yet: fallback covers them, plan Q6).
#[test]
fn every_template_s_phone_checks_clean() {
    let mut ids: Vec<String> = zen::TEMPLATES.iter().map(|(id, _)| (*id).to_string()).collect();
    ids.extend(zen::garden_catalog::garden_catalog().iter().map(|e| e.template_id.clone()));
    assert!(ids.len() > 2, "the catalog is in the list");
    for id in ids {
        let src = Defaults::live().template_source(&id).unwrap_or_else(|| panic!("no source for {id}"));
        let v: toml::Value = toml::from_str(src).unwrap_or_else(|e| panic!("{id}: {e}"));
        let Some(phone) = v.get("phone") else { continue };
        let mut file = toml::map::Map::new();
        file.insert("schema".into(), toml::Value::Integer(schema::SCHEMA_VERSION));
        file.insert("phone".into(), phone.clone());
        let text = toml::to_string(&toml::Value::Table(file)).expect("toml");
        let c = schema::check_garden(&format!("builtin:{id}#phone"), &text, zen::builtin_layer(), &id);
        assert!(c.is_clean() && c.authoring.is_empty(), "{id}: {:?} {:?}", c.errors, c.authoring);
    }
}

// ── a Garden with no [phone] ────────────────────────────────────────

/// Nothing changes for a Garden with no mobile version: no key on the page,
/// and the layout key is the only difference one makes.
#[test]
fn a_garden_with_no_phone_answers_as_before() {
    let plain = check(OWN);
    assert!(plain.is_clean(), "{:?}", plain.errors);
    assert!(!plain.layer.contains_key("page.phone"));
    let page = zen::garden_page(&plain.layer, BLANK_TEMPLATE_ID);
    assert!(page.get("phone").is_none(), "{page}");
    let with = check(&own("[[phone.screen]]\n[[phone.screen.widget]]\nid = \"board\"\n"));
    let mut other = zen::garden_page(&with.layer, BLANK_TEMPLATE_ID);
    other.as_object_mut().expect("a page").remove("phone");
    assert_eq!(page, other, "[phone] changes nothing the desktop reads");
    let stub = check("schema = 1\n");
    assert!(zen::garden_page(&stub.layer, BLANK_TEMPLATE_ID).get("phone").is_none());
}

// ── through the folder: put, last good, own keys, resolve ───────────

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set_up() -> (Tmp, ZenFiles, String) {
    let d = std::env::temp_dir().join(format!("k2-zen-phone-{}", uuid::Uuid::new_v4().simple()));
    let f = ZenFiles::new(d.join(".k2").join("zen"));
    assert!(f.root().starts_with(std::env::temp_dir()), "tests never touch the real Zen folder");
    f.setup().expect("setup");
    let id = f.gardens()[0].id.clone();
    (Tmp(d), f, id)
}

fn bad_request(e: ZenError) -> String {
    match e {
        ZenError::BadRequest(m) => m,
        other => panic!("want BadRequest, got {other:?}"),
    }
}

const PHONE_AGENTS: &str = "schema = 1\n[[phone.screen]]\nlabel = \"People\"\n[[phone.screen.widget]]\nid = \"agents\"\n";

#[test]
fn put_writes_a_good_layout_and_the_page_answers_it() {
    let (_t, f, id) = set_up();
    let before = f.resolve(Some(&id)).expect("resolve")["version"].clone();
    let v = f.put_garden(&id, PHONE_AGENTS, false).expect("put");
    assert_eq!(v["changed"], true);
    let after = f.resolve(Some(&id)).expect("resolve");
    assert_eq!(after["page"]["phone"]["screens"][0]["label"], "People");
    assert_ne!(after["version"], before, "the page's version covers page.phone");
    assert_eq!(f.garden_own_keys(&id).expect("keys"), vec!["phone".to_string()], "Start with the default names it");
}

#[test]
fn put_refuses_a_bad_shape_and_writes_nothing() {
    let (_t, f, id) = set_up();
    let path = f.path_of(&ZenFile::Garden(id.clone()));
    let before = fs::read_to_string(&path).expect("stub");
    let bad = "schema = 1\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"agents\"\nheight = 3\n";
    let m = bad_request(f.put_garden(&id, bad, false).expect_err("a bad height"));
    assert!(m.contains("the page has errors, nothing was written") && m.contains("widget height is"), "{m}");
    assert_eq!(fs::read_to_string(&path).expect("file"), before, "nothing was written");
}

/// New text naming an id that isn't on today's template is refused; the same
/// text already on disk (a K2 update moved the template) only warns, and the
/// page still answers.
#[test]
fn a_dangling_id_is_refused_when_written_and_only_warns_when_live() {
    let (_t, f, id) = set_up();
    let text = "schema = 1\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"gone\"\n[[phone.screen]]\n[[phone.screen.widget]]\nid = \"agents\"\n";
    let m = bad_request(f.put_garden(&id, text, false).expect_err("a dangling id"));
    assert!(m.contains("phone screen 1 names widget 'gone', which is not on this page"), "{m}");
    fs::write(f.path_of(&ZenFile::Garden(id.clone())), text).expect("the file is already there");
    let v = f.resolve(Some(&id)).expect("resolve");
    assert_eq!(v["errors"], json!([]), "never an error: {}", v["errors"]);
    assert_eq!(v["warnings"][0]["message"], "phone screen 1 names widget 'gone', which is not on this page", "{}", v["warnings"]);
    assert_eq!(v["page"]["phone"], json!({ "screens": [{ "widgets": [{ "id": "agents" }] }] }));
    let checked = f.validate(Some(&ZenFile::Garden(id))).expect("validate");
    assert_eq!(checked["ok"], true, "validate shows it as a warning: {checked}");
    assert_eq!(checked["warnings"][0]["message"], "phone screen 1 names widget 'gone', which is not on this page");
}

/// A locked Garden refuses an agent's layout as it refuses any edit.
#[test]
fn a_locked_garden_refuses_a_layout() {
    let (_t, f, id) = set_up();
    f.set_garden_lock(&id, true).expect("lock");
    match f.put_garden(&id, PHONE_AGENTS, false) {
        Err(ZenError::GardenLocked { .. }) => {}
        other => panic!("want garden_locked, got {:?}", other.map(|_| "written")),
    }
}

/// A file a bad `[phone]` broke falls back to its last good page, as any
/// other bad file does: the desktop keeps working.
#[test]
fn a_bad_phone_table_on_disk_falls_back_to_the_last_good_page() {
    let (_t, f, id) = set_up();
    f.put_garden(&id, PHONE_AGENTS, false).expect("put");
    f.refresh().expect("snapshot the good one");
    fs::write(f.path_of(&ZenFile::Garden(id.clone())), "schema = 1\n[[phone.screen]]\n").expect("write");
    let v = f.resolve(Some(&id)).expect("resolve");
    assert_eq!(v["errors"][0]["message"], "phone screen 1 has no widgets; add [[phone.screen.widget]] or drop the screen");
    assert_eq!(v["page"]["phone"]["screens"][0]["label"], "People", "last good: {}", v["page"]);
}

// ── the docs say what the checker does ──────────────────────────────

#[test]
fn the_k2_zen_skill_has_the_section_and_its_toml_checks_clean() {
    let body = zen::skill::generate_k2_zen_skill();
    assert!(body.contains("## A Garden's mobile layout"), "the skill names it");
    assert!(body.contains("k2 zen guide phone"), "and points at the page");
    let section = zen::skill::phone_section();
    assert!(section.contains("1 to 12 screens") && section.contains("160 to 4000") && section.contains("1 to 24 characters"), "{section}");
    let toml = section.split("```toml\n").nth(1).and_then(|r| r.split("```").next()).expect("a TOML block");
    let widgets = "[[widget]]\nid = \"fleet\"\nkind = \"custom\"\nwidget = \"fleet-canvas\"\ncolumn = 0\n";
    let c = check(&format!("schema = 1\n{widgets}{toml}"));
    assert!(c.is_clean() && c.warnings.is_empty(), "the skill's example: {:?} {:?}", c.errors, c.warnings);
}
