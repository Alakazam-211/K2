//! Zen core (prd-zen-mode-v1 S1/S3, tests T1.5, T1.6, T1.2, T3.2;
//! prd-zen-gardens-v1 S1/S2/S6, tests TG1.5–TG1.6, TG2.2, TG6.1).
//!
//! Drives `k2_core::zen` directly on temp folders: the schema table, last
//! good, `.history/` and reset, the Garden list (create, rename, reorder,
//! delete to history), per-Garden pages and theme picks, templates as data,
//! the G38 layout of built-in widgets, the agent-can't-grant rule at the
//! file layer, and the `k2-zen` skill. k2-core tests run through this daemon
//! test binary (never `cargo test -p k2-core`). Nothing here touches the
//! real `~/.k2/zen`.
//!
//! Fail loudly: every assertion names what it saw.

use std::path::{Path, PathBuf};

use k2_core::zen::schema::{self, FileKind, Layer};
use k2_core::zen::store::{ListSource, ACTIVE_FILE, GARDENS_FILE, HISTORY_KEEP, LAST_GARDEN};
use k2_core::zen::{self as zen, ZenError, ZenFile, ZenFiles};
use serde_json::{json, Value as J};

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_root(tag: &str) -> (TempRoot, ZenFiles) {
    let dir = std::env::temp_dir().join(format!(
        "k2-zen-core-{tag}-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let zen_dir = dir.join("zen");
    (TempRoot(dir), ZenFiles::new(zen_dir))
}

/// Zen set up the way the app does it: `setup` makes Garden 1 (texting)
/// and Garden 2 (empty).
fn set_up(tag: &str) -> (TempRoot, ZenFiles) {
    let (t, f) = temp_root(tag);
    let out = f.setup().expect("setup");
    assert!(out.created_folder && out.created_zen && out.created_default, "first setup creates all: {out:?}");
    let names: Vec<&str> = out.gardens.iter().map(|g| g.name.as_str()).collect();
    assert_eq!(names, vec!["Garden 1", "Garden 2"], "{out:?}");
    (t, f)
}

/// The id of the Garden at 0-based position `i`.
fn gid(f: &ZenFiles, i: usize) -> String {
    f.gardens().get(i).unwrap_or_else(|| panic!("no Garden at {i}: {:?}", f.gardens())).id.clone()
}

fn garden(id: &str) -> ZenFile {
    ZenFile::Garden(id.to_string())
}

fn write(f: &ZenFiles, file: &ZenFile, text: &str) {
    std::fs::write(f.path_of(file), text).unwrap_or_else(|e| panic!("write {}: {e}", file.label()));
}

fn check_zen(src: &str) -> schema::Checked {
    schema::check("zen.toml", src, FileKind::Zen, zen::builtin_layer())
}

fn check_theme(src: &str) -> schema::Checked {
    schema::check("themes/t/theme.toml", src, FileKind::Theme, zen::builtin_layer())
}

/// A Garden page made with the texting template.
fn check_page(src: &str) -> schema::Checked {
    schema::check_garden("gardens/g-1.toml", src, zen::builtin_layer(), zen::TEMPLATE_ID)
}

/// A Garden page made with the blank template.
fn check_blank(src: &str) -> schema::Checked {
    schema::check_garden("gardens/g-1.toml", src, zen::builtin_layer(), zen::BLANK_TEMPLATE_ID)
}

fn diag_list(c: &schema::Checked) -> String {
    c.errors.iter().chain(c.warnings.iter()).map(|d| d.render()).collect::<Vec<_>>().join("\n")
}

/// Assert exactly one error at `line` whose message contains `needle`.
fn assert_one_error(c: &schema::Checked, line: usize, needle: &str, what: &str) {
    assert_eq!(c.errors.len(), 1, "{what}: want exactly one error, got:\n{}", diag_list(c));
    let e = &c.errors[0];
    assert_eq!(e.line, line, "{what}: wrong line: {}", e.render());
    assert!(e.message.contains(needle), "{what}: message {:?} must contain {needle:?}", e.message);
}

fn kinds_of(page: &J, key: &str) -> Vec<String> {
    page[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} must be an array: {page}"))
        .iter()
        .map(|c| c["kind"].as_str().unwrap_or_else(|| panic!("{key} entry without kind: {c}")).to_string())
        .collect()
}

// ── builtin + templates ──────────────────────────────────────────────

#[test]
fn builtin_theme_is_clean_and_resolves_every_token() {
    let c = zen::check_builtin();
    assert!(c.is_clean(), "the default zen.toml must validate clean:\n{}", diag_list(&c));
    assert!(c.warnings.is_empty(), "the default zen.toml must have no warnings:\n{}", diag_list(&c));
    let rt = schema::resolve(zen::builtin_layer(), &Layer::new());
    for scheme in ["light", "dark"] {
        for tok in schema::COLOR_TOKENS {
            let v = &rt.tokens["colors"][scheme][*tok];
            assert!(v.is_string(), "builtin colors.{scheme}.{tok} must be set, got {v}");
        }
        for tok in schema::TERMINAL_TOKENS {
            let v = &rt.terminal["palette"][scheme][*tok];
            assert!(v.is_string(), "builtin terminal.{scheme}.{tok} must be set, got {v}");
        }
    }
    for n in schema::NUM_TOKENS.iter().filter(|n| n.table == "shape") {
        let v = &rt.tokens["shape"][n.key];
        assert!(v.is_number(), "builtin shape.{} must be set, got {v}", n.key);
    }
    assert_eq!(rt.font["family"], "system");
    assert_eq!(rt.font["size"], 14);
    assert_eq!(rt.font["lineHeight"], 1.45);
    assert_eq!(rt.font["terminal"]["family"], "meslo", "a proportional family pairs with meslo in terminals");
    assert!(rt.background.is_none(), "the basic theme has no background");
    assert_eq!(rt.tokens["scheme"], "auto");
    assert_eq!(rt.chrome["corners"], "system");
    assert_eq!(rt.chrome["stoplights"], "round");
    assert_eq!(rt.chrome["stoplight-offset"], json!([0, 0]));
    let anims = rt.motion["animations"].as_object().expect("animations object");
    assert_eq!(anims.len(), schema::ANIMATION_TREE.len(), "one resolved line per tree node: {anims:?}");
    assert_eq!(rt.motion["reducedMotion"], "instant");
}

/// TG1.6: every template is data, declares the two required controls
/// (G24: Garden switcher, not Home switcher) and only known caps.
#[test]
fn template_page_is_data_with_required_controls_and_known_caps() {
    // Rosson 2026-10-04: exactly two required controls.
    assert_eq!(zen::REQUIRED_CONTROLS, &["zen-toggle", "garden-switcher"]);
    assert!(zen::BRIDGE_CAPS.contains(&"gardens:manage"), "G29: gardens:manage is a bridge cap");
    let ids: Vec<&str> = zen::TEMPLATES.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec!["k2.texting@1", "k2.blank@1"]);
    assert_eq!(schema::TEMPLATE_IDS, ids.as_slice(), "the schema accepts exactly the shipped templates");
    for id in &ids {
        let page = zen::template_page(id).unwrap_or_else(|| panic!("template {id}"));
        assert_eq!(page["template"], *id);
        assert_eq!(page["layout"]["kind"], "columns", "{page}");
        let controls = kinds_of(&page, "controls");
        for req in zen::REQUIRED_CONTROLS {
            assert!(controls.iter().any(|c| c == req), "{id} must declare control {req}: {controls:?}");
        }
        assert!(!controls.iter().any(|c| c == "home-switcher"), "{id}: the Home switcher is gone from Zen: {controls:?}");
        for w in page["widgets"].as_array().expect("widgets") {
            assert_eq!(w["source"], "builtin", "built-ins are granted by K2: {w}");
            let kind = w["kind"].as_str().expect("kind");
            let want: Vec<&str> = zen::widget_caps(kind).unwrap_or_else(|| panic!("caps for {kind}")).to_vec();
            let caps: Vec<&str> = w["caps"].as_array().expect("caps").iter().filter_map(J::as_str).collect();
            assert_eq!(caps, want, "{id}: a template widget gets exactly K2's caps for its kind: {w}");
            for cap in &caps {
                assert!(zen::BRIDGE_CAPS.contains(cap), "unknown cap {cap} in {w}");
                assert_ne!(*cap, "gardens:manage", "G29: gardens:manage is never a widget cap: {w}");
            }
        }
        let split: f64 = page["layout"]["split"].as_array().expect("split").iter().filter_map(J::as_f64).sum();
        assert_eq!(split, 100.0, "{id}: columns add up to 100: {page}");
    }
    // The texting template: Agents (with its Home picker) beside Conversation.
    let page = zen::texting_page();
    assert_eq!(page["layout"]["split"], json!([34, 66]), "{page}");
    assert_eq!(page["layout"]["minWidths"], json!([240, 360]), "{page}");
    let cols = page["layout"]["columns"].as_array().expect("columns");
    assert_eq!(cols[0]["widget"], "agents");
    assert_eq!(cols[1]["widget"], "conversation");
    assert_eq!(kinds_of(&page, "widgets"), vec!["agents", "conversation", "nav-rail"], "{page}");
    // Rosson 2026-10-04: Garden 1's thin left rail, in column 0 beside the
    // Agents box, with the one cap that switches the Garden's rail view.
    let nav = &page["widgets"][2];
    assert_eq!((nav["id"].as_str(), nav["column"].as_u64()), (Some("nav"), Some(0)), "{nav}");
    assert_eq!(nav["caps"], json!(["app:navigate"]), "{nav}");
    assert!(zen::BRIDGE_CAPS.contains(&"app:navigate"), "app:navigate is a bridge cap");
    let blank = zen::template_page(zen::BLANK_TEMPLATE_ID).expect("blank");
    assert!(!kinds_of(&blank, "widgets").contains(&"nav-rail".to_string()), "blank Gardens get no rail: {blank}");
    let agents = &page["widgets"][0];
    assert_eq!(agents["column"], 0);
    assert_eq!(agents["props"]["home-picker"], true, "G11: the texting Agents widget has a Home picker: {agents}");
    assert_eq!(agents["props"]["mode"], "home", "the whole Home by default: {agents}");
    assert_eq!(agents["props"]["status"], json!(["working", "idle", "needs-you"]), "{agents}");
    let conv = &page["widgets"][1];
    assert_eq!(conv["column"], 1);
    assert_eq!(conv["props"]["attachments"], true, "{conv}");
    assert_eq!(conv["props"]["agents"], "agents", "the Conversation follows the Agents widget: {conv}");
    let placed: Vec<(String, String, Option<i64>)> = page["controls"]
        .as_array()
        .expect("controls")
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().expect("control kind").to_string(),
                c["placement"].as_str().expect("control placement").to_string(),
                c["column"].as_i64(),
            )
        })
        .collect();
    let want: Vec<(String, String, Option<i64>)> = [
        ("garden-switcher", "top-left", None),
        ("drag-region", "top", None),
        // Rosson 2026-10-04: the Zen toggle top right; Add agent is the
        // Agents widget's last row.
        ("zen-toggle", "top-right", None),
        ("add-agent", "widget-bottom-left", None),
    ]
    .into_iter()
    .map(|(k, p, c)| (k.to_string(), p.to_string(), c))
    .collect();
    assert_eq!(placed, want, "{page}");
    let add = page["controls"].as_array().expect("controls").iter().find(|c| c["kind"] == "add-agent").expect("add-agent");
    assert_eq!(add["widget"], "agents", "Add agent belongs to the Agents widget: {add}");
    // The blank template: one column holding the empty-Garden widget, no Add agent.
    let blank = zen::template_page(zen::BLANK_TEMPLATE_ID).expect("blank");
    assert_eq!(blank["layout"]["split"], json!([100]), "{blank}");
    assert_eq!(kinds_of(&blank, "widgets"), vec!["garden-empty"], "{blank}");
    // Rosson 2026-10-04: Start with the default turns THIS Garden into the
    // texting page (`gardens.useTemplate`), the only write a widget gets.
    assert_eq!(blank["widgets"][0]["caps"], json!(["agents:read", "thread:read", "thread:post", "gardens:template"]), "{blank}");
    assert!(zen::BRIDGE_CAPS.contains(&"gardens:template"), "gardens:template is a bridge cap");
    let with_template: Vec<&str> = zen::BUILTIN_WIDGET_CAPS.iter().filter(|(_, c)| c.contains(&"gardens:template")).map(|(k, _)| *k).collect();
    assert_eq!(with_template, vec!["garden-empty"], "only the empty-Garden widget may switch its Garden's template");
    assert_eq!(kinds_of(&blank, "controls"), vec!["garden-switcher", "drag-region", "zen-toggle"], "{blank}");
    assert!(zen::template_page("k2.unknown@1").is_none());
}

// ── T1.5 validation table ────────────────────────────────────────────

#[test]
fn t1_5_unknown_key_reports_file_line_col() {
    let src = "schema = 1\n\n[colors.light]\ncanvas = \"#ffffff\"\n# a comment\n\n  acent = \"#fff\"\n";
    let c = check_zen(src);
    assert_one_error(&c, 7, "unknown key 'acent'", "unknown key");
    assert_eq!(c.errors[0].col, 3, "column of the indented key: {}", c.errors[0].render());
    assert!(c.errors[0].message.contains("did you mean 'accent'"), "{}", c.errors[0].message);
    assert_eq!(c.errors[0].render(), format!("zen.toml:7:3: {}", c.errors[0].message));
}

#[test]
fn t1_5_every_number_token_out_of_range_is_an_error_at_its_line() {
    // `[background]` lives only in theme bundles; everything else anywhere.
    let check = |src: &str, table: &str| {
        if table == "background" { check_theme(src) } else { check_zen(src) }
    };
    for n in schema::NUM_TOKENS {
        for bad in [n.min - 1.0, n.max + 1.0] {
            let src = format!("schema = 1\n[{}]\n{} = {bad}\n", n.table, n.key);
            let c = check(&src, n.table);
            assert_one_error(&c, 3, "out of range", &format!("{}.{} = {bad}", n.table, n.key));
        }
        let ok = format!("schema = 1\n[{}]\n{} = {}\n", n.table, n.key, n.max);
        assert!(check(&ok, n.table).is_clean(), "{}.{} = max must pass:\n{}", n.table, n.key, diag_list(&check(&ok, n.table)));
    }
    let c = check_zen("schema = 1\n[font]\nsize = \"big\"\n");
    assert_one_error(&c, 3, "must be a number", "size as string");
}

#[test]
fn t1_5_bad_color_and_low_contrast() {
    let c = check_zen("schema = 1\n[colors.dark]\ntext = \"#12\"\n");
    assert_one_error(&c, 3, "is not a colour", "bad hex");
    let c = check_zen("schema = 1\n[colors.dark]\ntext = \"rgb(300, 0, 0)\"\n");
    assert_one_error(&c, 3, "is not a colour", "rgb out of range");
    for ok in ["#abc", "#abcd", "#aabbcc", "#aabbccdd", "rgb(10, 20, 30)", "rgba(10, 20, 30, 0.5)"] {
        let c = check_zen(&format!("schema = 1\n[colors.light]\nborder = \"{ok}\"\n"));
        assert!(c.is_clean(), "{ok} must parse:\n{}", diag_list(&c));
    }
    // Low contrast: dark text on the dark default canvas.
    let c = check_zen("schema = 1\n[colors.dark]\n\ntext = \"#222222\"\n");
    assert_one_error(&c, 4, "needs at least 3:1", "low contrast text");
    // Changing only the canvas so the default accent fails is caught at canvas.
    let c = check_zen("schema = 1\n[colors.light]\ncanvas = \"#b4532a\"\n");
    assert!(!c.is_clean(), "accent on an accent-coloured canvas must fail");
    assert!(c.errors.iter().all(|e| e.line == 3), "contrast errors sit on the canvas line:\n{}", diag_list(&c));
}

#[test]
fn t1_5_page_tables_schema_2_errors_and_more() {
    // [[control]] in a Garden is a warning (controls come from the template);
    // layout and widgets anywhere but a Garden are errors.
    let c = check_page("schema = 1\ntemplate = \"k2.texting@1\"\n\n[[control]]\nkind = \"zen-toggle\"\n");
    assert!(c.is_clean(), "[[control]] is a warning, not an error:\n{}", diag_list(&c));
    assert_eq!(c.warnings.len(), 1, "{}", diag_list(&c));
    assert_eq!(c.warnings[0].line, 4);
    assert_eq!(c.warnings[0].message, schema::CONTROL_WARNING);
    let c = check_zen("schema = 1\n[layout]\nkind = \"columns\"\n");
    assert_one_error(&c, 2, "belongs in a Garden file", "layout in zen.toml");
    let c = check_theme("schema = 1\n[[widget]]\nid = \"x\"\n");
    assert_one_error(&c, 2, "belongs in a Garden file", "widget in a theme");

    let c = check_zen("schema = 2\n");
    assert_one_error(&c, 1, "this K2 reads Zen schema 1", "schema 2");
    let c = check_zen("[theme]\nscheme = \"dark\"\n");
    assert_one_error(&c, 1, "missing `schema = 1`", "missing schema");
    let c = check_zen("schema = 1\n[theme]\nscheme = \"sepia\"\n");
    assert_one_error(&c, 3, "is not one of", "bad scheme");
    let c = check_zen("schema = 1\n[chrome]\nstoplights = \"hidden\"\n");
    assert_one_error(&c, 3, "doesn't let K2 hide", "hidden stoplights");
    let c = check_zen("schema = 1\n[chrome]\ncorners = \"rounded\"\n");
    assert_one_error(&c, 3, "is not one of", "corners");
    let c = check_zen("schema = 1\n[chrome]\nstoplight-offset = [4, 30]\n");
    assert_one_error(&c, 3, "0 to 24", "offset");
    let c = check_zen("schema = 1\n[animation]\nzenIn = [1, 4, \"wobble\"]\n");
    assert_one_error(&c, 3, "unknown curve 'wobble'", "unknown curve");
    let c = check_zen("schema = 1\n[bezier]\nwobble = [0.2, 1.4, 0.3, 1]\n[animation]\nzenIn = [1, 4, \"wobble\", \"popin 92%\"]\n");
    assert!(c.is_clean(), "a curve defined in [bezier] may overshoot:\n{}", diag_list(&c));
    let c = check_zen("schema = 1\n[animation]\nzenIn = [1, 4, \"glide\", \"popin 120%\"]\n");
    assert_one_error(&c, 3, "0 to 100", "popin range");
    let c = check_zen("schema = 1\n[animation]\nzenIn = [1, 4, \"glide\", \"spin\"]\n");
    assert_one_error(&c, 3, "unknown style 'spin'", "style");
    let c = check_zen("schema = 1\n[animation]\nzenSpin = [1, 4, \"glide\"]\n");
    assert_one_error(&c, 3, "unknown key 'zenSpin'", "animation name");
    let c = check_zen("schema = 1\n[bezier]\nbad = [1.5, 0, 0.5, 1]\n");
    assert_one_error(&c, 3, "x1 and x2 must be 0 to 1", "bezier x");
    let c = check_zen("schema = 1\n[colors]\nsepia = { text = \"#000\" }\n");
    assert_one_error(&c, 3, "unknown key 'sepia'", "colors scheme");
    let c = check_zen("schema = 1\ntemplate = \"k2.texting@1\"\n");
    assert_one_error(&c, 2, "belongs in gardens/", "template in zen.toml");
    // TG1.6: the schema accepts k2.blank@1 and refuses an unknown template at its line.
    assert!(check_page("schema = 1\ntemplate = \"k2.blank@1\"\n").is_clean());
    let c = check_page("schema = 1\n\ntemplate = \"k2.unknown@1\"\n");
    assert_one_error(&c, 3, "unknown template 'k2.unknown@1'", "page template");
    let c = check_zen("schema = 1\n[colors.light\ncanvas = \"#fff\"\n");
    assert_eq!(c.errors.len(), 1, "{}", diag_list(&c));
    assert!(c.errors[0].message.starts_with("invalid TOML"), "{}", c.errors[0].render());
    assert_eq!(c.errors[0].line, 2, "syntax error line: {}", c.errors[0].render());
    let big = format!("schema = 1\n#{}\n", "x".repeat(schema::MAX_FILE_BYTES));
    assert_one_error(&check_zen(&big), 1, "under 64 KB", "size cap");
}

/// TG6.1 (G38, Rosson's answers 1 and 5): a Garden lays out K2's built-in
/// widgets; a widget shows a whole Home or one agent filtered from it; a
/// page never names caps; controls stay the template's.
#[test]
fn tg6_1_garden_layout_of_builtin_widgets() {
    let two = "schema = 1\n\
template = \"k2.blank@1\"\n\
[layout]\n\
kind = \"columns\"\n\
[[layout.column]]\n\
size = 40\n\
min-width = 240\n\
[[layout.column]]\n\
size = 60\n\
\n\
[[widget]]\n\
id = \"agents\"\n\
kind = \"agents\"\n\
column = 0\n\
[widget.props]\n\
home = \"Work\"\n\
\n\
[[widget]]\n\
id = \"talk\"\n\
kind = \"conversation\"\n\
column = 1\n\
[widget.props]\n\
agents = \"agents\"\n";
    let c = check_blank(two);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(page["template"], "k2.blank@1");
    assert_eq!(page["layout"]["split"], json!([40, 60]), "{page}");
    assert_eq!(page["layout"]["minWidths"], json!([240, 0]), "an unset min-width is 0: {page}");
    assert_eq!(page["layout"]["columns"][0]["widget"], "agents", "{page}");
    assert_eq!(page["layout"]["columns"][1]["widget"], "talk", "{page}");
    assert_eq!(kinds_of(&page, "widgets"), vec!["agents", "conversation"], "the file's widgets replace garden-empty: {page}");
    let a = &page["widgets"][0];
    assert_eq!(a["props"]["home"], "Work");
    assert_eq!(a["props"]["mode"], "home", "a Home without an agent is the whole Home: {a}");
    assert_eq!(a["props"]["home-picker"], false, "defaults filled: {a}");
    assert_eq!(a["props"]["preview"], true, "defaults filled: {a}");
    assert_eq!(a["caps"], json!(["agents:read", "agents:add", "presence:read"]), "K2's caps: {a}");
    assert_eq!(a["source"], "builtin");
    assert_eq!(page["widgets"][1]["caps"], json!(["agents:read", "presence:read", "thread:read", "thread:post"]));
    assert_eq!(kinds_of(&page, "controls"), vec!["garden-switcher", "drag-region", "zen-toggle"], "controls stay the template's");

    // One agent filtered from a Home: `agent` makes the widget single-agent.
    let one = "schema = 1\n[[widget]]\nid = \"cortana\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nhome = \"Work\"\nagent = \"cortana\"\n";
    let c = check_blank(one);
    assert!(c.is_clean(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(page["widgets"][0]["props"]["mode"], "agent", "{page}");
    assert_eq!(page["widgets"][0]["props"]["agent"], "cortana");
    assert_eq!(page["layout"]["split"], json!([100]), "the blank template's one column stays: {page}");
    // A conversation pinned to one agent needs no Agents widget.
    let pinned = "schema = 1\n[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 0\n[widget.props]\nagent = \"sales\"\nhome = \"Play\"\n";
    let c = check_blank(pinned);
    assert!(c.is_clean(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert!(page["widgets"][0]["props"].get("agents").is_none(), "a pinned conversation follows nothing: {page}");
    // On the texting template, a layout alone re-sizes the template's widgets.
    let resized = "schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 50\nmin-width = 200\n[[layout.column]]\nsize = 50\nmin-width = 200\n";
    let c = check_page(resized);
    assert!(c.is_clean(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::TEMPLATE_ID);
    assert_eq!(page["layout"]["split"], json!([50, 50]));
    assert_eq!(kinds_of(&page, "widgets"), vec!["agents", "conversation", "nav-rail"], "{page}");
    // A Garden file may place the rail like any built-in widget.
    let railed = "schema = 1\n[[widget]]\nid = \"rail\"\nkind = \"nav-rail\"\ncolumn = 0\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n";
    let c = check_blank(railed);
    assert!(c.is_clean(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(kinds_of(&page, "widgets"), vec!["nav-rail", "agents"], "{page}");
    assert_eq!(page["widgets"][0]["caps"], json!(["app:navigate"]), "{page}");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"rail\"\nkind = \"nav-rail\"\ncolumn = 0\n[widget.props]\nhome = \"Work\"\n");
    assert_one_error(&c, 7, "unknown key 'home'", "the rail has no props");

    // Errors, each at its line.
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\ncaps = [\"thread:post\"]\n");
    assert_one_error(&c, 6, "built-in widgets get K2's caps", "caps in a widget");
    let c = check_blank("schema = 1\ncaps = [\"net:*\"]\n");
    assert_one_error(&c, 2, "built-in widgets get K2's caps", "caps at the top");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\n\nkind = \"tickets\"\ncolumn = 0\n");
    assert_one_error(&c, 5, "unknown widget kind 'tickets'", "unknown kind");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"garden-empty\"\ncolumn = 0\n");
    assert_one_error(&c, 4, "comes from the blank template", "garden-empty from a file");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 1\n");
    assert_one_error(&c, 5, "this page has 1 column", "column out of the blank layout");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nhom = \"Work\"\n");
    assert_one_error(&c, 7, "did you mean 'home'", "unknown prop");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nmode = \"agent\"\n");
    assert_one_error(&c, 7, "needs agent =", "agent mode without an agent");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nmode = \"home\"\nagent = \"x\"\n");
    assert_one_error(&c, 8, "filters the widget to one agent", "home mode with an agent");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nstatus = [\"working\", \"asleep\"]\n");
    assert_one_error(&c, 7, "status must be a list", "bad status");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"t\"\nkind = \"conversation\"\ncolumn = 0\n");
    assert_one_error(&c, 5, "has no agent to show", "conversation alone");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"t\"\nkind = \"conversation\"\ncolumn = 0\n[widget.props]\nagents = \"nope\"\n");
    assert_one_error(&c, 7, "follows 'nope', which is not an Agents widget", "dangling agents link");
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n[[widget]]\nid = \"a\"\nkind = \"agents\"\ncolumn = 0\n");
    assert_one_error(&c, 7, "used twice", "duplicate id");
    let c = check_blank("schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 40\n[[layout.column]]\nsize = 40\n");
    assert_one_error(&c, 4, "add up to 80", "sizes not 100");
    let c = check_blank("schema = 1\n[layout]\nkind = \"grid\"\n[[layout.column]]\nsize = 100\n");
    assert_one_error(&c, 3, "layout kind 'grid'", "layout kind");
    let c = check_blank("schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 900\n");
    assert_one_error(&c, 6, "min-width is px, 0 to 800", "min-width");
    let four = "schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 25\n[[layout.column]]\nsize = 25\n[[layout.column]]\nsize = 25\n[[layout.column]]\nsize = 25\n";
    assert_one_error(&check_blank(four), 4, "1 to 3 columns", "four columns");
    // The texting template's Conversation sits in column 1: a one-column
    // layout without its own widgets can't hold it.
    let c = check_page("schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\n");
    assert_one_error(&c, 2, "the template's widget 'conversation' sits in column 1", "layout too narrow for the template");
    let c = check_blank("schema = 1\nwidgets = []\n");
    assert_one_error(&c, 2, "one [[widget]] block per widget", "widgets plural");
}

/// Rosson 2026-10-06: a Garden file places a widget in the top band (right
/// of the Garden switcher) with `slot = "top"`, the way it places one in a
/// column. Only `nav-rail` fits; it draws as a row there; a page with the
/// rail up top draws no second rail; the controls stay the template's.
#[test]
fn tg6_2_top_band_slot_for_the_nav_rail() {
    // The docs' example: no id, no column.
    let c = check_blank("schema = 1\n[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\n");
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    let rail = &page["widgets"][0];
    assert_eq!(rail["kind"], "nav-rail", "{page}");
    assert_eq!(rail["id"], "nav-rail", "a top widget's id defaults to its kind: {rail}");
    assert_eq!(rail["slot"], "top", "{rail}");
    assert!(rail.get("column").is_none(), "a top widget has no column: {rail}");
    assert_eq!(rail["props"]["orientation"], "row", "row by default in the top band: {rail}");
    assert_eq!(rail["caps"], json!(["app:navigate"]), "{rail}");
    assert_eq!(kinds_of(&page, "controls"), vec!["garden-switcher", "drag-region", "zen-toggle"], "controls stay the template's");
    for required in zen::REQUIRED_CONTROLS {
        assert!(kinds_of(&page, "controls").iter().any(|k| k == required), "required control {required} still declared: {page}");
    }
    assert!(page["layout"]["columns"][0].get("widget").is_none(), "a top widget names no column: {page}");

    // The texting page with the rail moved up: agents + conversation in the
    // columns, the rail in the band, explicit id and orientation.
    let texting = "schema = 1\n\
template = \"k2.texting@1\"\n\
[[widget]]\nid = \"agents\"\nkind = \"agents\"\ncolumn = 0\n\
[[widget]]\nid = \"conversation\"\nkind = \"conversation\"\ncolumn = 1\n\
[[widget]]\nid = \"nav\"\nkind = \"nav-rail\"\nslot = \"top\"\n[widget.props]\norientation = \"row\"\n";
    let c = check_page(texting);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::TEMPLATE_ID);
    assert_eq!(kinds_of(&page, "widgets"), vec!["agents", "conversation", "nav-rail"], "{page}");
    let slots: Vec<&str> = page["widgets"].as_array().unwrap().iter().map(|w| w["slot"].as_str().unwrap()).collect();
    assert_eq!(slots, vec!["column", "column", "top"], "every widget says its slot: {page}");
    assert_eq!(page["layout"]["columns"][0]["widget"], "agents", "{page}");
    assert_eq!(page["widgets"][1]["props"]["agents"], "agents", "the conversation still links: {page}");

    // Default stays: the texting template's rail is in column 0, a column strip.
    let tpl = zen::texting_page();
    let nav = tpl["widgets"].as_array().unwrap().iter().find(|w| w["kind"] == "nav-rail").cloned().unwrap();
    assert_eq!((nav["slot"].clone(), nav["column"].clone(), nav["props"]["orientation"].clone()), (json!("column"), json!(0), json!("column")), "{nav}");
    // A rail in a column may ask for a row.
    let c = check_blank("schema = 1\n[[widget]]\nid = \"r\"\nkind = \"nav-rail\"\ncolumn = 0\n[widget.props]\norientation = \"row\"\n");
    assert!(c.is_clean(), "{}", diag_list(&c));
    assert_eq!(zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID)["widgets"][0]["props"]["orientation"], "row");
    // `slot = "column"` is the default, said out loud.
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\nslot = \"column\"\ncolumn = 0\n");
    assert!(c.is_clean(), "{}", diag_list(&c));

    // Errors, each at its line.
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\nslot = \"top\"\n");
    // FC41: the allowed list grows with the chrome kinds; the start stays.
    assert_one_error(
        &c,
        5,
        "'agents' can't go in the top band; slot = \"top\" takes: nav-rail, garden-switcher, zen-toggle, usage, theme-picker, menu. Place 'agents' in a column",
        "a kind that doesn't fit the band",
    );
    let c = check_blank("schema = 1\n[[widget]]\nid = \"t\"\nkind = \"conversation\"\nslot = \"top\"\n[widget.props]\nagent = \"sales\"\n");
    assert_one_error(&c, 5, "takes: nav-rail", "conversation in the band");
    // `bottom` is a real slot now (FC7); an unknown one is still an error.
    let c = check_blank("schema = 1\n[[widget]]\nkind = \"nav-rail\"\nslot = \"side\"\n");
    assert_eq!(c.errors.len(), 2, "bad slot, then no id: {}", diag_list(&c));
    assert_eq!(c.errors[0].line, 4, "{}", diag_list(&c));
    assert!(c.errors[0].message.contains("widget slot must be one of: column, top, bottom, menu"), "{}", diag_list(&c));
    let c = check_blank("schema = 1\n[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\ncolumn = 0\n");
    assert_one_error(&c, 5, "has no column", "a column on a top widget");
    let c = check_blank("schema = 1\n[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\n[widget.props]\norientation = \"column\"\n");
    assert_one_error(&c, 6, "the top band is one row high", "a column rail in the band");
    let c = check_blank("schema = 1\n[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\n[widget.props]\norientation = \"diagonal\"\n");
    assert_one_error(&c, 6, "orientation must be one of: row, column", "a bad orientation");
    let c = check_blank("schema = 1\n[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\nplace = \"left\"\n");
    assert_one_error(&c, 5, "unknown key 'place'", "unknown key");
    assert!(c.errors[0].message.contains("slot"), "the unknown-key error lists slot: {}", c.errors[0].message);
    // The duplicate rule: the rail up top, and another in a column.
    let c = check_blank("schema = 1\n[[widget]]\nid = \"top\"\nkind = \"nav-rail\"\nslot = \"top\"\n[[widget]]\nid = \"side\"\nkind = \"nav-rail\"\ncolumn = 0\n");
    assert_one_error(&c, 7, "the nav rail is already in the top band ('top'); a page draws one nav rail", "rail in band and column");
    // Order doesn't matter: the column one is the extra.
    let c = check_blank("schema = 1\n[[widget]]\nid = \"side\"\nkind = \"nav-rail\"\ncolumn = 0\n[[widget]]\nid = \"top\"\nkind = \"nav-rail\"\nslot = \"top\"\n");
    assert_one_error(&c, 3, "remove 'side'", "column rail declared first");
    // Two rails up top: one too many, and a default id used twice.
    let c = check_blank("schema = 1\n[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\n[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\n");
    assert!(c.errors.iter().any(|e| e.line == 6 && e.message.contains("default id) is used twice")), "{}", diag_list(&c));
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"nav-rail\"\nslot = \"top\"\n[[widget]]\nid = \"b\"\nkind = \"nav-rail\"\nslot = \"top\"\n[[widget]]\nid = \"c\"\nkind = \"nav-rail\"\nslot = \"top\"\n");
    assert!(c.errors.iter().any(|e| e.message.contains("the top band holds at most 2 widgets; this page puts 3 there")), "{}", diag_list(&c));
    assert!(c.errors.iter().any(|e| e.line == 7 && e.message.contains("remove 'b'")), "{}", diag_list(&c));
    // A column widget still needs a column; the message names the band.
    let c = check_blank("schema = 1\n[[widget]]\nid = \"a\"\nkind = \"agents\"\n");
    assert_one_error(&c, 3, "or slot = \"top\" for the top band", "no column, no slot");
    // `slot` is for Garden files only.
    let c = check_zen("schema = 1\n[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\n");
    assert!(c.errors.iter().any(|e| e.message.contains("belongs in a Garden file")), "{}", diag_list(&c));
}

// ── prd-zen-freeform-chrome S1: Zen controls are widgets ──────────────

/// A Garden file from lines (line 1 is the first entry), so every
/// expected line number below can be read off the list.
fn lines(ls: &[&str]) -> String {
    let mut s = ls.join("\n");
    s.push('\n');
    s
}

/// Rosson's ask, verbatim from the PRD (§1): switcher bottom left, theme
/// and toggle bottom right, usage not placed, no top band.
const BOTTOM_BAR: &str = "schema = 1\n\
\n\
[[widget]]\n\
kind = \"garden-switcher\"\n\
slot = \"bottom\"\n\
\n\
[[widget]]\n\
kind = \"theme-picker\"\n\
slot = \"bottom\"\n\
align = \"end\"\n\
\n\
[[widget]]\n\
kind = \"zen-toggle\"\n\
slot = \"bottom\"\n\
align = \"end\"\n";

/// Both required controls and the theme in one menu, top right (PRD §1).
const MENU_BOTH: &str = "schema = 1\n\
\n\
[[widget]]\n\
id = \"more\"\n\
kind = \"menu\"\n\
slot = \"top\"\n\
align = \"end\"\n\
[widget.props]\n\
icon = \"dots\"\n\
label = \"More\"\n\
\n\
[[widget]]\n\
kind = \"garden-switcher\"\n\
slot = \"menu\"\n\
menu = \"more\"\n\
\n\
[[widget]]\n\
kind = \"theme-picker\"\n\
slot = \"menu\"\n\
menu = \"more\"\n\
\n\
[[widget]]\n\
kind = \"zen-toggle\"\n\
slot = \"menu\"\n\
menu = \"more\"\n";

fn ids(v: &J) -> Vec<String> {
    v.as_array()
        .unwrap_or_else(|| panic!("an id list: {v}"))
        .iter()
        .map(|x| x.as_str().unwrap_or_else(|| panic!("an id: {x}")).to_string())
        .collect()
}

fn group(start: &[&str], center: &[&str], end: &[&str]) -> J {
    json!({ "start": start, "center": center, "end": end })
}

fn placements(page: &J) -> Vec<(String, String)> {
    page["controls"]
        .as_array()
        .unwrap_or_else(|| panic!("controls: {page}"))
        .iter()
        .map(|c| {
            (
                c["kind"].as_str().unwrap_or_else(|| panic!("control kind: {c}")).to_string(),
                c["placement"].as_str().unwrap_or_else(|| panic!("control placement: {c}")).to_string(),
            )
        })
        .collect()
}

/// The older-app safety property (FC28, FC58) and the answer's internal
/// consistency: `widgets` never holds a chrome kind; every chrome item is a
/// chrome kind; every id in `bands`, `edges` and `menus` is a widget or a
/// chrome item; `controls` always declares both required kinds.
fn assert_page_safe(page: &J, what: &str) {
    let widgets = page["widgets"].as_array().unwrap_or_else(|| panic!("{what}: widgets: {page}"));
    for w in widgets {
        let k = w["kind"].as_str().unwrap_or_else(|| panic!("{what}: widget kind: {w}"));
        assert!(!schema::is_chrome_kind(k), "{what}: chrome kind '{k}' leaked into page.widgets: {page}");
    }
    let items = page["chrome"]["items"].as_array().unwrap_or_else(|| panic!("{what}: chrome.items: {page}"));
    for c in items {
        let k = c["kind"].as_str().unwrap_or_else(|| panic!("{what}: chrome kind: {c}"));
        assert!(schema::is_chrome_kind(k), "{what}: '{k}' in chrome.items is not a chrome kind: {page}");
    }
    let from = page["chrome"]["from"].as_str().unwrap_or_else(|| panic!("{what}: chrome.from: {page}"));
    assert!(from == "template" || from == "garden", "{what}: chrome.from {from}");
    let known: Vec<String> =
        widgets.iter().chain(items.iter()).map(|w| w["id"].as_str().unwrap_or_else(|| panic!("{what}: id: {w}")).to_string()).collect();
    let mut placed: Vec<String> = Vec::new();
    for b in ["top", "bottom"] {
        let band = &page["bands"][b];
        if band.is_null() {
            continue;
        }
        for a in ["start", "center", "end"] {
            placed.extend(ids(&band[a]));
        }
    }
    for e in page["edges"].as_array().unwrap_or_else(|| panic!("{what}: edges: {page}")) {
        for a in ["start", "center", "end"] {
            placed.extend(ids(&e[a]));
        }
    }
    for (_, list) in page["menus"].as_object().unwrap_or_else(|| panic!("{what}: menus: {page}")) {
        placed.extend(ids(list));
    }
    for id in &placed {
        assert!(known.contains(id), "{what}: '{id}' is placed but is neither a widget nor a chrome item: {page}");
    }
    // Every chrome item is drawn somewhere (a band, an edge or a menu).
    for c in items {
        let id = c["id"].as_str().unwrap_or_default().to_string();
        assert!(placed.contains(&id), "{what}: chrome '{id}' is in no band, edge or menu: {page}");
    }
    let kinds = kinds_of(page, "controls");
    for req in zen::REQUIRED_CONTROLS {
        assert!(kinds.iter().any(|k| k == req), "{what}: controls must declare {req}: {kinds:?}");
    }
}

/// FC-T3 (FC30, FC42): both templates list their chrome as data. It passes
/// the Garden checks, agrees with `[[control]]`, and resolves to today's
/// band: the switcher at the start; usage, theme and the toggle at the end.
#[test]
fn fc_t3_templates_list_their_chrome_as_data() {
    for id in schema::TEMPLATE_IDS {
        let c = zen::check_template_chrome(id).unwrap_or_else(|| panic!("template {id}"));
        assert!(c.is_clean() && c.warnings.is_empty(), "{id}: template chrome must pass the Garden checks:\n{}", diag_list(&c));
        let page = zen::template_page(id).unwrap_or_else(|| panic!("template {id}"));
        assert_eq!(page["chrome"]["from"], "template", "{id}: {page}");
        let items = page["chrome"]["items"].as_array().expect("chrome items");
        let kinds: Vec<&str> = items.iter().map(|c| c["kind"].as_str().expect("kind")).collect();
        assert_eq!(kinds, vec!["garden-switcher", "usage", "theme-picker", "zen-toggle"], "{id}: {page}");
        assert_eq!(
            page["chrome"]["items"][0],
            json!({"id": "garden-switcher", "kind": "garden-switcher", "slot": "top", "align": "start", "props": {}, "caps": ["gardens:manage"]}),
            "{id}: FC31: only the switcher gets a cap"
        );
        for c in &items[1..] {
            assert_eq!(c["caps"], json!([]), "{id}: {c}");
            assert_eq!((c["slot"].as_str(), c["align"].as_str()), (Some("top"), Some("end")), "{id}: {c}");
        }
        assert_eq!(page["bands"]["top"], group(&["garden-switcher"], &[], &["usage", "theme-picker", "zen-toggle"]), "{id}: {page}");
        assert!(page["bands"]["bottom"].is_null(), "{id}: no bottom band: {page}");
        assert_eq!(page["edges"], json!([]), "{id}: {page}");
        assert_eq!(page["menus"], json!({}), "{id}: {page}");
        // The [[control]] list agrees: start ↔ top-left, end ↔ top-right.
        let p = placements(&page);
        let at = |k: &str| p.iter().find(|(kind, _)| kind == k).map(|(_, pl)| pl.clone());
        assert_eq!(at("garden-switcher").as_deref(), Some("top-left"), "{id}: {p:?}");
        assert_eq!(at("zen-toggle").as_deref(), Some("top-right"), "{id}: {p:?}");
        assert_eq!(at("drag-region").as_deref(), Some("top"), "{id}: {p:?}");
        assert_page_safe(&page, id);
    }
    // Chrome never reaches the template's content widgets.
    assert_eq!(kinds_of(&zen::texting_page(), "widgets"), vec!["agents", "conversation", "nav-rail"]);
    assert_eq!(kinds_of(&zen::template_page(zen::BLANK_TEMPLATE_ID).expect("blank"), "widgets"), vec!["garden-empty"]);
}

/// FC-T2 (FC3): content and chrome are two groups with their own replace
/// rules.
#[test]
fn fc_t2_content_and_chrome_replace_rules() {
    let tpl = zen::texting_page();
    // Content only: the template's chrome stays, `controls` byte-identical.
    let content_only = "schema = 1\n\
[[widget]]\nid = \"agents\"\nkind = \"agents\"\ncolumn = 0\n\
[[widget]]\nid = \"conversation\"\nkind = \"conversation\"\ncolumn = 1\n\
[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\n";
    let c = check_page(content_only);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::TEMPLATE_ID);
    assert_eq!(page["chrome"]["from"], "template", "{page}");
    assert_eq!(page["chrome"]["items"], tpl["chrome"]["items"], "the template's chrome: {page}");
    assert_eq!(page["controls"].to_string(), tpl["controls"].to_string(), "controls byte-identical to today");
    // The file's band widget sits between the switcher and the end group (live).
    assert_eq!(page["bands"]["top"], group(&["garden-switcher", "nav-rail"], &[], &["usage", "theme-picker", "zen-toggle"]), "{page}");
    assert_page_safe(&page, "content only");
    // Template chrome keeps its corners: a file rail at the end of the top
    // band sits left of the template's end group, so the toggle stays in
    // the corner.
    let c = check_page(&content_only.replace("slot = \"top\"\n", "slot = \"top\"\nalign = \"end\"\n"));
    assert!(c.is_clean(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::TEMPLATE_ID);
    assert_eq!(page["bands"]["top"], group(&["garden-switcher"], &[], &["nav-rail", "usage", "theme-picker", "zen-toggle"]), "{page}");
    assert_eq!(page["widgets"][2]["align"], "end", "a content widget gains align when set (FC28): {page}");

    // Chrome only: the template's content stays; ALL of its chrome is gone.
    let c = check_page(BOTTOM_BAR);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::TEMPLATE_ID);
    assert_eq!(page["widgets"], tpl["widgets"], "the template's content widgets, untouched: {page}");
    assert_eq!(page["layout"], tpl["layout"], "{page}");
    assert_eq!(page["chrome"]["from"], "garden", "{page}");
    let kinds: Vec<&str> = page["chrome"]["items"].as_array().expect("items").iter().map(|c| c["kind"].as_str().expect("kind")).collect();
    assert_eq!(kinds, vec!["garden-switcher", "theme-picker", "zen-toggle"], "usage is not placed, so not shown: {page}");
    assert!(page["bands"]["top"].is_null(), "no top band: {page}");
    assert_eq!(page["bands"]["bottom"], group(&["garden-switcher"], &[], &["theme-picker", "zen-toggle"]), "{page}");
    assert_eq!(
        placements(&page),
        vec![
            ("garden-switcher".to_string(), "bottom-start".to_string()),
            ("drag-region".to_string(), "bands".to_string()),
            ("zen-toggle".to_string(), "bottom-end".to_string()),
            ("add-agent".to_string(), "widget-bottom-left".to_string()),
        ],
        "{page}"
    );
    assert_eq!(page["controls"][3]["widget"], "agents", "Add agent names the page's Agents widget: {page}");
    assert_page_safe(&page, "bottom-bar");
    // On the blank template there is no Agents widget, so no Add agent.
    let c = check_blank(BOTTOM_BAR);
    assert!(c.is_clean(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(kinds_of(&page, "controls"), vec!["garden-switcher", "drag-region", "zen-toggle"], "{page}");
    assert_eq!(kinds_of(&page, "widgets"), vec!["garden-empty"], "{page}");

    // Both: both replaced, and the bands keep FILE order across the groups.
    let both = lines(&[
        "schema = 1",
        "[[widget]]",
        "id = \"a\"",
        "kind = \"agents\"",
        "column = 0",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"bottom\"",
        "align = \"end\"",
        "[[widget]]",
        "kind = \"nav-rail\"",
        "slot = \"bottom\"",
        "align = \"end\"",
        "[[widget]]",
        "kind = \"garden-switcher\"",
        "slot = \"bottom\"",
    ]);
    let c = check_blank(&both);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(kinds_of(&page, "widgets"), vec!["agents", "nav-rail"], "content replaced, chrome kept out: {page}");
    assert_eq!(page["widgets"][1]["slot"], "bottom", "{page}");
    assert_eq!(page["widgets"][1]["props"]["orientation"], "row", "a rail in the bottom band is a row: {page}");
    assert_eq!(page["chrome"]["from"], "garden", "{page}");
    assert_eq!(page["bands"]["bottom"], group(&["garden-switcher"], &[], &["zen-toggle", "nav-rail"]), "file order: the LAST end item is in the corner: {page}");
    assert!(page["bands"]["top"].is_null(), "{page}");
    assert_eq!(page["controls"][3], json!({"kind": "add-agent", "placement": "widget-bottom-left", "widget": "a"}), "{page}");
    assert_page_safe(&page, "both");
}

/// FC-T1 clean shapes: every slot, `align`, `edge` and menu shape; file
/// order in `bands`; a missing id defaults to the kind.
#[test]
fn fc_t1_chrome_grammar_clean_shapes() {
    // Rosson's two examples.
    let c = check_blank(MENU_BOTH);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(page["bands"]["top"], group(&[], &[], &["more"]), "only the menu button in the top band: {page}");
    assert_eq!(page["menus"], json!({"more": ["garden-switcher", "theme-picker", "zen-toggle"]}), "{page}");
    let menu = page["chrome"]["items"].as_array().expect("items").iter().find(|c| c["kind"] == "menu").cloned().expect("menu");
    assert_eq!(
        menu,
        json!({"id": "more", "kind": "menu", "slot": "top", "align": "end", "props": {"icon": "dots", "label": "More"}, "caps": []}),
        "{page}"
    );
    let toggle = page["chrome"]["items"].as_array().expect("items").iter().find(|c| c["kind"] == "zen-toggle").cloned().expect("toggle");
    assert_eq!(toggle, json!({"id": "zen-toggle", "kind": "zen-toggle", "slot": "menu", "menu": "more", "props": {}, "caps": []}), "{page}");
    assert_eq!(
        placements(&page),
        vec![
            ("garden-switcher".to_string(), "menu:more".to_string()),
            ("drag-region".to_string(), "bands".to_string()),
            ("zen-toggle".to_string(), "menu:more".to_string()),
        ],
        "{page}"
    );
    assert_page_safe(&page, "menu-both");

    // A menu with no props gets the dots icon and no label (the name is
    // "More" in the renderer); a menu in the bottom-right corner.
    let corner = lines(&[
        "schema = 1",
        "[[widget]]",
        "kind = \"menu\"",
        "slot = \"bottom\"",
        "align = \"end\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"menu\"",
        "menu = \"menu\"",
        "[[widget]]",
        "kind = \"garden-switcher\"",
        "slot = \"menu\"",
        "menu = \"menu\"",
        "[[widget]]",
        "kind = \"usage\"",
        "slot = \"menu\"",
        "menu = \"menu\"",
    ]);
    let c = check_blank(&corner);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(page["bands"]["bottom"], group(&[], &[], &["menu"]), "{page}");
    assert_eq!(page["menus"], json!({"menu": ["zen-toggle", "garden-switcher", "usage"]}), "menu items in file order: {page}");
    assert_eq!(page["chrome"]["items"][0]["props"], json!({"icon": "dots"}), "{page}");
    assert_page_safe(&page, "menu-bottom-right");

    // Column edges: the toggle at the bottom-right of the conversation
    // column, the switcher at the center of the top band, a row rail at
    // column 0's bottom edge (no id: it defaults to its kind), the theme
    // at column 0's top edge (no edge: top by default).
    let edges = lines(&[
        "schema = 1",
        "[[widget]]",
        "id = \"agents\"",
        "kind = \"agents\"",
        "column = 0",
        "[[widget]]",
        "id = \"conversation\"",
        "kind = \"conversation\"",
        "column = 1",
        "[[widget]]",
        "kind = \"nav-rail\"",
        "column = 0",
        "edge = \"bottom\"",
        "[[widget]]",
        "kind = \"garden-switcher\"",
        "slot = \"top\"",
        "align = \"center\"",
        "[[widget]]",
        "kind = \"theme-picker\"",
        "column = 0",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "column = 1",
        "edge = \"bottom\"",
        "align = \"end\"",
    ]);
    let c = check_page(&edges);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::TEMPLATE_ID);
    assert_eq!(page["bands"]["top"], group(&[], &["garden-switcher"], &[]), "{page}");
    assert!(page["bands"]["bottom"].is_null(), "{page}");
    assert_eq!(
        page["edges"],
        json!([
            {"column": 0, "edge": "top", "start": ["theme-picker"], "center": [], "end": []},
            {"column": 0, "edge": "bottom", "start": ["nav-rail"], "center": [], "end": []},
            {"column": 1, "edge": "bottom", "start": [], "center": [], "end": ["zen-toggle"]},
        ]),
        "edges sorted by column, top before bottom: {page}"
    );
    let rail = &page["widgets"][2];
    assert_eq!((rail["id"].as_str(), rail["edge"].as_str(), rail["props"]["orientation"].as_str()), (Some("nav-rail"), Some("bottom"), Some("row")), "{rail}");
    assert!(rail.get("align").is_none(), "a content widget gains align only when set: {rail}");
    let theme = page["chrome"]["items"].as_array().expect("items").iter().find(|c| c["kind"] == "theme-picker").cloned().expect("theme");
    assert_eq!(theme, json!({"id": "theme-picker", "kind": "theme-picker", "slot": "column", "column": 0, "edge": "top", "align": "start", "props": {}, "caps": []}), "{page}");
    let p = placements(&page);
    assert_eq!(p[0], ("garden-switcher".to_string(), "top-center".to_string()), "{p:?}");
    assert_eq!(p[2], ("zen-toggle".to_string(), "column-1-bottom-end".to_string()), "{p:?}");
    assert_page_safe(&page, "column-corner");

    // Explicit ids and `slot = "column"` said out loud; a menu item may
    // name a menu declared after it.
    let late_menu = lines(&[
        "schema = 1",
        "[[widget]]",
        "id = \"switch\"",
        "kind = \"garden-switcher\"",
        "slot = \"menu\"",
        "menu = \"m\"",
        "[[widget]]",
        "id = \"out\"",
        "kind = \"zen-toggle\"",
        "slot = \"column\"",
        "column = 0",
        "edge = \"top\"",
        "align = \"end\"",
        "[[widget]]",
        "id = \"m\"",
        "kind = \"menu\"",
        "slot = \"top\"",
        "[widget.props]",
        "icon = \"zen\"",
    ]);
    let c = check_blank(&late_menu);
    assert!(c.is_clean() && c.warnings.is_empty(), "{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(page["menus"], json!({"m": ["switch"]}), "{page}");
    assert_eq!(page["bands"]["top"], group(&["m"], &[], &[]), "{page}");
    assert_eq!(placements(&page)[2], ("zen-toggle".to_string(), "column-0-top-end".to_string()), "{page}");
    assert_page_safe(&page, "late menu");

    // FC47: limits are counted per group, so 12 content widgets and the two
    // required controls fit on one page.
    let mut many = String::from("schema = 1\n");
    for n in 0..schema::MAX_WIDGETS {
        many.push_str(&format!("[[widget]]\nid = \"a{n}\"\nkind = \"agents\"\ncolumn = 0\n"));
    }
    many.push_str("[[widget]]\nkind = \"garden-switcher\"\nslot = \"top\"\n[[widget]]\nkind = \"zen-toggle\"\nslot = \"top\"\nalign = \"end\"\n");
    let c = check_blank(&many);
    assert!(c.is_clean(), "12 content + 2 chrome is within both limits:\n{}", diag_list(&c));
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    assert_eq!(page["widgets"].as_array().map(Vec::len), Some(schema::MAX_WIDGETS), "{page}");
    assert_page_safe(&page, "12 + 2");
}

/// FC-T1 rejections (FC24, FC47, FC5): each is an error at its exact line,
/// with its message.
#[test]
fn fc_t1_chrome_grammar_rejections() {
    // 1. Chrome without the toggle, and without the switcher [FC49]: at the
    //    first chrome widget's line, even when content comes first.
    let c = check_blank(&lines(&["schema = 1", "[[widget]]", "kind = \"garden-switcher\"", "slot = \"bottom\""]));
    assert_one_error(&c, 3, &schema::required_chrome_error("zen-toggle"), "no toggle");
    assert!(c.errors[0].message.starts_with(
        "This file places Zen controls, so it replaces the template's. It must also place a zen-toggle (the way out): add [[widget]] kind = \"zen-toggle\" slot = \"top\" align = \"end\", or put it in a menu. See k2 zen guide required."
    ), "the FC24 sentence: {}", c.errors[0].message);
    let c = check_page(&lines(&[
        "schema = 1",
        "[[widget]]",
        "id = \"agents\"",
        "kind = \"agents\"",
        "column = 0",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"top\"",
    ]));
    assert_one_error(&c, 7, "It must also place a garden-switcher", "no switcher");
    let c = check_blank(&lines(&["schema = 1", "[[widget]]", "id = \"x\"", "kind = \"menu\"", "slot = \"top\"", "[[widget]]", "kind = \"usage\"", "slot = \"menu\"", "menu = \"x\""]));
    assert_eq!(c.errors.len(), 2, "both required controls missing: {}", diag_list(&c));
    assert!(c.errors.iter().all(|e| e.line == 3), "{}", diag_list(&c));
    // 2. A chrome kind placed twice (menu may repeat).
    let c = check_blank(&lines(&[
        "schema = 1",
        "[[widget]]",
        "kind = \"garden-switcher\"",
        "slot = \"top\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"top\"",
        "[[widget]]",
        "id = \"again\"",
        "kind = \"zen-toggle\"",
        "slot = \"bottom\"",
    ]));
    assert_one_error(&c, 9, "'zen-toggle' is placed twice", "toggle twice");
    // 3. drag-region (FC5): at the kind value, one error.
    let c = check_blank(&lines(&["schema = 1", "[[widget]]", "kind = \"drag-region\"", "slot = \"top\""]));
    assert_one_error(&c, 3, schema::DRAG_REGION_ERROR, "drag-region");
    // 4. slot = "menu" with no menu; a menu that names no menu; menu on another slot.
    let base = ["[[widget]]", "kind = \"garden-switcher\"", "slot = \"top\""];
    let with = |extra: &[&str]| -> String {
        let mut v: Vec<&str> = vec!["schema = 1"];
        v.extend_from_slice(&base);
        v.extend_from_slice(extra);
        lines(&v)
    };
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"menu\""]));
    assert_one_error(&c, 6, "needs menu = \"<menu id>\"", "menu item without menu");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"menu\"", "menu = \"nope\""]));
    assert_one_error(&c, 8, "menu = \"nope\" names no menu on this page", "dangling menu");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "menu = \"x\""]));
    assert_one_error(&c, 8, "goes only with slot = \"menu\"", "menu key on a band item");
    // 5. A menu inside a menu; a content kind in a menu; an empty menu; too many items.
    let c = check_blank(&with(&[
        "[[widget]]",
        "id = \"outer\"",
        "kind = \"menu\"",
        "slot = \"top\"",
        "[[widget]]",
        "id = \"inner\"",
        "kind = \"menu\"",
        "slot = \"menu\"",
        "menu = \"outer\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"menu\"",
        "menu = \"outer\"",
    ]));
    assert_one_error(&c, 12, "a menu can't hold another menu", "nested menu");
    let c = check_blank(&with(&[
        "[[widget]]",
        "kind = \"menu\"",
        "slot = \"top\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"menu\"",
        "menu = \"menu\"",
        "[[widget]]",
        "id = \"a\"",
        "kind = \"agents\"",
        "slot = \"menu\"",
        "menu = \"menu\"",
    ]));
    assert_one_error(&c, 15, "'agents' can't go in a menu; a menu holds only: zen-toggle, garden-switcher, theme-picker, usage", "content in a menu");
    let c = check_blank(&with(&["[[widget]]", "kind = \"menu\"", "slot = \"top\"", "[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\""]));
    assert_one_error(&c, 6, "menu 'menu' holds nothing", "empty menu");
    let mut crowded = vec!["schema = 1", "[[widget]]", "kind = \"menu\"", "slot = \"top\""];
    let item = ["[[widget]]", "kind = \"zen-toggle\"", "slot = \"menu\"", "menu = \"menu\""];
    let ids7: Vec<String> = (0..7).map(|n| format!("id = \"t{n}\"")).collect();
    for idl in &ids7 {
        crowded.extend_from_slice(&item[..1]);
        crowded.push(idl.as_str());
        crowded.extend_from_slice(&item[1..]);
    }
    let c = check_blank(&lines(&crowded));
    assert!(c.errors.iter().any(|e| e.line == 3 && e.message.contains("menu 'menu' holds at most 6 items; it has 7")), "{}", diag_list(&c));
    // 6. align and edge where they have no meaning; values off the list.
    let c = check_blank(&with(&["[[widget]]", "kind = \"menu\"", "slot = \"top\"", "[[widget]]", "kind = \"zen-toggle\"", "slot = \"menu\"", "menu = \"menu\"", "align = \"end\""]));
    assert_one_error(&c, 12, "a menu lists its items in file order", "align in a menu");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"bottom\"", "edge = \"top\""]));
    assert_one_error(&c, 8, "edge picks the top or bottom of a column", "edge in a band");
    let c = check_page(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "[[widget]]", "id = \"a\"", "kind = \"agents\"", "column = 0", "align = \"end\""]));
    assert_one_error(&c, 12, "'agents' fills its column and takes no edge or align", "align on agents");
    let c = check_page(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "[[widget]]", "id = \"t\"", "kind = \"conversation\"", "column = 0", "edge = \"bottom\"", "[widget.props]", "agent = \"x\""]));
    assert_one_error(&c, 12, "'conversation' fills its column", "edge on conversation");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "align = \"right\""]));
    assert_one_error(&c, 8, "align must be one of: start, center, end", "bad align");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "column = 0", "edge = \"left\""]));
    assert_one_error(&c, 8, "edge must be one of: top, bottom", "bad edge");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "[[widget]]", "id = \"r\"", "kind = \"nav-rail\"", "column = 0", "align = \"end\""]));
    assert_one_error(&c, 9, "align places a row item", "align on a column rail strip");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "[[widget]]", "kind = \"nav-rail\"", "column = 0", "edge = \"top\"", "[widget.props]", "orientation = \"column\""]));
    assert_one_error(&c, 13, "a column edge is one row high", "a column rail at an edge");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"bottom\"", "column = 0"]));
    assert_one_error(&c, 8, "a bottom-band widget (slot = \"bottom\") has no column", "column on a band item");
    let c = check_blank(&with(&["[[widget]]", "kind = \"menu\"", "slot = \"top\"", "[[widget]]", "kind = \"zen-toggle\"", "slot = \"menu\"", "menu = \"menu\"", "column = 0"]));
    assert_one_error(&c, 12, "a menu item (slot = \"menu\") has no column", "column on a menu item");
    let menu_with = |prop: &str| {
        with(&["[[widget]]", "kind = \"menu\"", "slot = \"top\"", "[widget.props]", prop, "[[widget]]", "kind = \"zen-toggle\"", "slot = \"menu\"", "menu = \"menu\""])
    };
    let c = check_blank(&menu_with("label = \"A label far longer than the limit\""));
    assert_one_error(&c, 9, "menu.label is at most 24 characters", "long label");
    let c = check_blank(&menu_with("icon = \"star\""));
    assert_one_error(&c, 9, "menu.icon must be one of: dots, bars, zen", "bad icon");
    // 7. Limits.
    let mut bands9 = vec!["schema = 1"];
    let tid: Vec<String> = (0..9).map(|n| format!("id = \"z{n}\"")).collect();
    for idl in &tid {
        bands9.extend_from_slice(&["[[widget]]", idl.as_str(), "kind = \"zen-toggle\"", "slot = \"top\""]);
    }
    let c = check_blank(&lines(&bands9));
    assert!(c.errors.iter().any(|e| e.message.contains("the top band holds at most 8 items; this page puts 9 there")), "{}", diag_list(&c));
    let mut edge5 = vec!["schema = 1"];
    for idl in &tid[..5] {
        edge5.extend_from_slice(&["[[widget]]", idl.as_str(), "kind = \"usage\"", "column = 0", "edge = \"bottom\""]);
    }
    let c = check_blank(&lines(&edge5));
    assert!(c.errors.iter().any(|e| e.message.contains("column 0's bottom edge holds at most 4 items; this page puts 5 there")), "{}", diag_list(&c));
    let mut menus4 = vec!["schema = 1"];
    for idl in &tid[..4] {
        menus4.extend_from_slice(&["[[widget]]", idl.as_str(), "kind = \"menu\"", "slot = \"top\""]);
    }
    let c = check_blank(&lines(&menus4));
    assert!(c.errors.iter().any(|e| e.message.contains("a page has at most 3 menus; this one has 4")), "{}", diag_list(&c));
    let mut chrome11 = vec!["schema = 1"];
    let cid: Vec<String> = (0..11).map(|n| format!("id = \"c{n}\"")).collect();
    for idl in &cid {
        chrome11.extend_from_slice(&["[[widget]]", idl.as_str(), "kind = \"usage\"", "column = 0"]);
    }
    let c = check_blank(&lines(&chrome11));
    assert!(c.errors.iter().any(|e| e.message.contains("places at most 10 Zen controls (chrome widgets); this one has 11")), "{}", diag_list(&c));
    let mut content13 = String::from("schema = 1\n");
    for n in 0..13 {
        content13.push_str(&format!("[[widget]]\nid = \"a{n}\"\nkind = \"agents\"\ncolumn = 0\n"));
    }
    let c = check_blank(&content13);
    assert_eq!(c.errors.len(), 1, "{}", diag_list(&c));
    assert!(c.errors[0].message.contains("a Garden page holds at most 12 widgets; this one has 13"), "{}", diag_list(&c));
    // 8. A kind that doesn't fit its slot names every allowed kind.
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "[[widget]]", "id = \"a\"", "kind = \"agents\"", "slot = \"bottom\""]));
    assert_one_error(
        &c,
        11,
        "'agents' can't go in the bottom band; slot = \"bottom\" takes: nav-rail, garden-switcher, zen-toggle, usage, theme-picker, menu",
        "agents in the bottom band",
    );
    // 10. A chrome widget in a column the layout doesn't have.
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "column = 1", "edge = \"bottom\""]));
    assert_one_error(&c, 7, "widget 'zen-toggle' is in column 1, but this page has 1 column", "chrome off the layout");
    // 11. caps on chrome.
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "caps = [\"zen:exit\"]"]));
    assert_one_error(&c, 8, "built-in widgets get K2's caps", "caps on chrome");
    // Unknown kind lists both groups; the unknown-key list names the new keys.
    let c = check_blank(&with(&["[[widget]]", "kind = \"clock\"", "slot = \"top\""]));
    assert_one_error(&c, 6, "unknown widget kind 'clock'; built-in widgets are: agents, conversation, nav-rail; Zen controls are: garden-switcher, zen-toggle, usage, theme-picker, menu", "unknown kind");
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "place = \"left\""]));
    assert_one_error(&c, 8, "unknown key 'place' in [[widget]]; allowed: id, kind, slot, column, edge, align, menu, props", "unknown key");
    // Two default ids collide: two menus without ids.
    let c = check_blank(&with(&[
        "[[widget]]",
        "kind = \"menu\"",
        "slot = \"top\"",
        "[[widget]]",
        "kind = \"menu\"",
        "slot = \"bottom\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"menu\"",
        "menu = \"menu\"",
    ]));
    assert_one_error(&c, 9, "widget id 'menu' (this widget's default id) is used twice", "two default menu ids");
    // A content widget in a column's body still needs its id and column.
    let c = check_blank(&with(&["[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\"", "[[widget]]", "kind = \"agents\"", "column = 0"]));
    assert_one_error(&c, 9, "needs an id", "agents without an id");
    // The rail in the bottom band counts for the one-rail rule.
    let c = check_blank(&lines(&["schema = 1", "[[widget]]", "id = \"low\"", "kind = \"nav-rail\"", "slot = \"bottom\"", "[[widget]]", "id = \"side\"", "kind = \"nav-rail\"", "column = 0"]));
    assert_one_error(&c, 7, "the nav rail is already in the bottom band ('low')", "a second rail beside a bottom rail");
    // [[control]] stays a warning, with the FC52 text.
    let c = check_blank(&lines(&["schema = 1", "[[control]]", "kind = \"zen-toggle\""]));
    assert!(c.is_clean(), "{}", diag_list(&c));
    assert_eq!(c.warnings.len(), 1, "{}", diag_list(&c));
    assert_eq!(
        c.warnings[0].message,
        "[[control]] is ignored: place controls with [[widget]] kind = \"zen-toggle\" (see k2 zen guide bands)"
    );
}

#[test]
fn animation_tree_user_global_beats_builtin_leaves_and_styles_inherit() {
    let user = check_zen("schema = 1\n[animation]\nglobal = [1, 8, \"soft\"]\n").layer;
    let rt = schema::resolve(zen::builtin_layer(), &user);
    let a = &rt.motion["animations"];
    assert_eq!(a["zenIn"]["speed"], 8, "user global must beat the builtin zenIn: {}", a["zenIn"]);
    assert_eq!(a["zenIn"]["durationMs"], 800);
    assert_eq!(a["zenIn"]["from"], "global");
    assert_eq!(a["zenIn"]["style"], "fade", "style falls back to the builtin chain: {}", a["zenIn"]);
    assert_eq!(a["zenIn"]["ease"], "cubic-bezier(0.45, 0, 0.55, 1)");
    let user = check_zen("schema = 1\n[bezier]\nbouncy = [0.3, 1.8, 0.6, -0.4]\n[animation]\nrows = [1, 5, \"bouncy\", \"slide\"]\nrowIn = [0]\n").layer;
    let rt = schema::resolve(zen::builtin_layer(), &user);
    let a = &rt.motion["animations"];
    assert_eq!(a["rowMove"]["from"], "rows", "a child takes its parent's line: {}", a["rowMove"]);
    assert_eq!(a["rowMove"]["style"], "slide");
    assert_eq!(a["rowMove"]["ease"], "cubic-bezier(0.3, 1.8, 0.6, -0.4)");
    assert_eq!(a["rowIn"]["on"], false);
    assert_eq!(a["rowIn"]["durationMs"], 0);
}

// ── store: last good, history, reset, Gardens ─────────────────────────

#[test]
fn t1_2_broken_file_keeps_last_good_across_a_restart() {
    let (_t, f) = set_up("lastgood");
    let g = gid(&f, 0);
    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\nradius = 6\n");
    f.refresh().expect("refresh");
    let good = f.resolve(Some(&g)).expect("resolve good");
    assert_eq!(good["theme"]["tokens"]["shape"]["radius"], 6, "{good}");
    assert_eq!(good["errors"], json!([]));

    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\nradius = 6\nacent = 1\n");
    f.refresh().expect("refresh broken");
    // A fresh handle reads only the disk: the restart case.
    let after = ZenFiles::new(f.root().to_path_buf()).resolve(Some(&g)).expect("resolve broken");
    assert_eq!(after["version"], good["version"], "a broken file must keep the last good version");
    assert_eq!(after["theme"]["tokens"]["shape"]["radius"], 6);
    let errs = after["errors"].as_array().expect("errors");
    assert_eq!(errs.len(), 1, "{after}");
    assert_eq!(errs[0]["file"], "zen.toml");
    assert_eq!(errs[0]["line"], 4);
    assert!(after["sources"]["zen.toml"].as_str().is_some_and(|s| s.starts_with("snapshot:")), "{after}");
    assert!(after["lastGoodAt"].is_string(), "{after}");

    // validate never changes the live version or writes a snapshot.
    let before = f.snapshots(&ZenFile::Zen).len();
    let v = f.validate(None).expect("validate");
    assert_eq!(v["ok"], false, "{v}");
    assert_eq!(f.snapshots(&ZenFile::Zen).len(), before, "validate must not snapshot");

    // No clean history at all: the default theme, still with the error.
    std::fs::remove_dir_all(f.history_root()).expect("rm history");
    let bare = f.resolve(Some(&g)).expect("resolve no history");
    assert_eq!(bare["sources"]["zen.toml"], "default", "{bare}");
    assert_eq!(bare["theme"]["tokens"]["shape"]["radius"], 14);
    assert_eq!(bare["errors"].as_array().map(Vec::len), Some(1));
}

#[test]
fn t1_6_history_keeps_20_and_reset_snapshots_first_and_spares_gardens_json() {
    let (_t, f) = set_up("history");
    for i in 0..25 {
        let size = 12 + (i % 8);
        write(&f, &ZenFile::Zen, &format!("schema = 1\n# save {i}\n[font]\nsize = {size}\n"));
        f.refresh().expect("refresh");
    }
    let snaps = f.snapshots(&ZenFile::Zen);
    assert_eq!(snaps.len(), HISTORY_KEEP, "25 clean saves keep 20: {:?}", snaps.iter().map(|s| &s.name).collect::<Vec<_>>());
    let mut names: Vec<&String> = snaps.iter().map(|s| &s.name).collect();
    names.sort();
    names.reverse();
    assert_eq!(names, snaps.iter().map(|s| &s.name).collect::<Vec<_>>(), "newest first, names sort by time");
    assert!(snaps.iter().all(|s| s.at.ends_with('Z') && s.at.len() == 24), "snapshot times are RFC 3339 ms: {:?}", snaps[0].at);

    let list_before = std::fs::read(f.root().join(GARDENS_FILE)).expect("gardens.json");
    let oldest = snaps.last().expect("oldest").name.clone();
    let oldest_text = std::fs::read_to_string(f.history_root().join("zen.toml").join(&oldest)).expect("oldest text");
    let current_before = std::fs::read_to_string(f.path_of(&ZenFile::Zen)).expect("current");
    let out = f.reset(&ZenFile::Zen, Some(&oldest)).expect("reset to oldest");
    assert_eq!(out.restored, oldest);
    let kept = out.snapshot.expect("the pre-reset file is kept");
    let kept_text = std::fs::read_to_string(f.history_root().join("zen.toml").join(&kept)).expect("kept text");
    assert_eq!(kept_text, current_before, "reset must snapshot the current file first");
    assert_eq!(std::fs::read_to_string(f.path_of(&ZenFile::Zen)).expect("restored"), oldest_text);
    assert_eq!(std::fs::read(f.root().join(GARDENS_FILE)).expect("gardens.json"), list_before, "reset never touches gardens.json");

    // A broken current file is kept too, so the user's edit isn't lost.
    write(&f, &ZenFile::Zen, "schema = 1\nnope = 1\n");
    let out = f.reset(&ZenFile::Zen, None).expect("reset to default");
    assert_eq!(out.restored, "default");
    let kept = out.snapshot.expect("broken file kept");
    assert_eq!(
        std::fs::read_to_string(f.history_root().join("zen.toml").join(&kept)).expect("kept broken"),
        "schema = 1\nnope = 1\n"
    );
    assert_eq!(std::fs::read_to_string(f.path_of(&ZenFile::Zen)).expect("default"), zen::DEFAULT_ZEN_TOML);
    assert!(f.snapshots(&ZenFile::Zen).len() <= HISTORY_KEEP, "reset respects the cap");

    match f.reset(&ZenFile::Zen, Some("19990101T000000000Z-000")) {
        Err(ZenError::NotFound(m)) => assert!(m.contains("no snapshot"), "{m}"),
        other => panic!("unknown snapshot must be NotFound, got {other:?}"),
    }

    // A Garden resets to its template's stub, named from gardens.json (G17).
    let g = gid(&f, 0);
    let page = garden(&g);
    write(&f, &page, "schema = 1\ntemplate = \"k2.texting@1\"\n[shape]\ngap = 20\n");
    let out = f.reset(&page, None).expect("reset page");
    assert_eq!(out.file, format!("gardens/{g}.toml"));
    let stub = std::fs::read_to_string(f.path_of(&page)).expect("page stub");
    assert!(stub.contains("\"Garden 1\"") && stub.contains("template = \"k2.texting@1\""), "{stub}");
    let n = f.new_garden("Notes", None, None, None).expect("notes");
    write(&f, &garden(&n.id), "schema = 1\n[shape]\ngap = 20\n");
    f.reset(&garden(&n.id), None).expect("reset notes");
    let stub = std::fs::read_to_string(f.path_of(&garden(&n.id))).expect("notes stub");
    assert!(stub.contains("\"Notes\"") && stub.contains("template = \"k2.blank@1\""), "a blank Garden resets to the blank stub: {stub}");
}

/// TG1.1 (core half): setup makes Garden 1 on the texting template and
/// Garden 2 on the empty one (Rosson 2026-10-04), once; a new
/// Garden is `g-` + 8 hex on the blank template with the empty widget and a
/// stub on disk; the daemon honours each page's template (G40).
#[test]
fn setup_default_garden_new_garden_is_empty_and_template_is_honoured() {
    let (_t, f) = temp_root("setup");
    assert!(!f.is_set_up(), "no folder: not set up");
    assert!(matches!(f.resolve(None), Err(ZenError::NotSetUp)));
    assert!(matches!(f.new_garden("Notes", None, None, None), Err(ZenError::NotSetUp)), "agents can't set Zen up");
    assert!(!f.exists(), "a refused garden/new never creates the folder");

    let out = f.setup().expect("setup");
    assert!(out.created_folder && out.created_default);
    let list = f.gardens();
    assert_eq!(list.len(), 2, "{list:?}");
    let d = &list[0];
    assert_eq!(d.name, "Garden 1");
    assert_eq!(d.template, "k2.texting@1");
    let two = list[1].clone();
    assert_eq!((two.name.as_str(), two.template.as_str()), ("Garden 2", "k2.blank@1"), "{two:?}");
    assert_ne!(two.id, d.id);
    assert!(f.path_of(&garden(&two.id)).is_file(), "Garden 2 has its stub");
    let g2 = f.resolve(Some(&two.id)).expect("Garden 2");
    assert_eq!(kinds_of(&g2["page"], "widgets"), vec!["garden-empty"], "Garden 2 holds only the empty-Garden widget: {g2}");
    assert_eq!(g2["garden"]["index"], 2, "{g2}");
    assert_eq!(g2["errors"], json!([]), "{g2}");
    assert!(d.id.starts_with("g-") && d.id.len() == 10 && d.id[2..].chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()), "id shape: {}", d.id);
    assert!(f.path_of(&garden(&d.id)).is_file(), "Garden 1 has its stub");
    let g = f.resolve(None).expect("first Garden");
    assert_eq!(g["garden"], json!({ "id": d.id, "name": "Garden 1", "index": 1 }), "{g}");
    assert!(g.get("home").is_none(), "G12: no `home` in the answer: {g}");
    assert_eq!(g["page"]["template"], "k2.texting@1");
    assert!(kinds_of(&g["page"], "controls").contains(&"garden-switcher".to_string()), "{g}");
    // Setup is idempotent.
    let again = f.setup().expect("setup again");
    assert!(!again.created_folder && !again.created_default, "{again:?}");
    assert_eq!(f.gardens().len(), 2, "setup again makes nothing new");

    let n = f.new_garden("  Notes\u{7}  ", None, None, None).expect("new");
    assert_eq!(n.name, "Notes", "trimmed, control characters dropped");
    assert_eq!(n.template, "k2.blank@1", "a new Garden is empty by default");
    assert!(n.id.starts_with("g-") && n.id.len() == 10, "{}", n.id);
    let stub = std::fs::read_to_string(f.path_of(&garden(&n.id))).expect("stub on disk");
    assert!(stub.contains("Zen Garden \"Notes\"") && stub.contains("template = \"k2.blank@1\""), "{stub}");
    let e = f.resolve(Some(&n.id)).expect("empty Garden");
    assert_eq!(e["page"]["template"], "k2.blank@1", "{e}");
    assert_eq!(kinds_of(&e["page"], "widgets"), vec!["garden-empty"], "{e}");
    assert_eq!(e["garden"]["index"], 3);
    assert_eq!(e["errors"], json!([]), "the stub validates: {e}");
    assert_eq!(f.resolve(Some("notes")).expect("by name")["garden"]["id"], n.id, "a name selects, case aside");

    // The template on the page wins (G40): the blank Garden turns into texting.
    write(&f, &garden(&n.id), "schema = 1\ntemplate = \"k2.texting@1\"\n");
    let t = f.resolve(Some(&n.id)).expect("texting");
    assert_eq!(t["page"]["template"], "k2.texting@1", "{t}");
    assert_eq!(kinds_of(&t["page"], "widgets"), vec!["agents", "conversation", "nav-rail"]);
    // A missing page file resolves to the template the Garden was made with.
    std::fs::remove_file(f.path_of(&garden(&n.id))).expect("rm page");
    let m = f.resolve(Some(&n.id)).expect("missing page");
    assert_eq!(m["page"]["template"], "k2.blank@1", "{m}");
    assert_eq!(m["errors"], json!([]));
    std::fs::remove_file(f.path_of(&garden(&d.id))).expect("rm default page");
    assert_eq!(f.resolve(Some(&d.id)).expect("default missing")["page"]["template"], "k2.texting@1");
    // A Garden with layout honours it in `get`.
    write(
        &f,
        &garden(&n.id),
        "schema = 1\n[[widget]]\nid = \"cortana\"\nkind = \"agents\"\ncolumn = 0\n[widget.props]\nagent = \"cortana\"\nhome = \"Work\"\n",
    );
    let w = f.resolve(Some(&n.id)).expect("widgets");
    assert_eq!(w["page"]["widgets"][0]["props"]["mode"], "agent", "{w}");
    assert_eq!(w["errors"], json!([]), "{w}");
    match f.resolve(Some("nowhere")) {
        Err(ZenError::UnknownGarden { garden, known }) => {
            assert_eq!(garden, "nowhere");
            assert_eq!(known, vec![d.id.clone(), two.id.clone(), n.id.clone()]);
        }
        other => panic!("an unknown Garden must be UnknownGarden, got {other:?}"),
    }

    // A deleted Garden 2 never comes back: setup seeds only an empty list.
    f.delete_garden(&two.id).expect("delete Garden 2");
    let again = f.setup().expect("setup after deleting Garden 2");
    assert!(!again.created_default, "{again:?}");
    let names: Vec<String> = f.gardens().into_iter().map(|g| g.name).collect();
    assert_eq!(names, vec!["Garden 1".to_string(), "Notes".to_string()], "Garden 2 stays deleted");
    // "Garden 2" is a free name again.
    f.new_garden("Garden 2", None, None, None).expect("a new Garden 2");
}

/// TG1.6: the Garden list's rules (names, positions, seedHome, deletes)
/// and that the fingerprint moves on every list change.
#[test]
fn garden_list_crud_rules_and_fingerprint() {
    let (_t, f) = set_up("crud");
    let d = gid(&f, 0);
    let fp0 = f.refresh().expect("fp0");
    assert_eq!(f.refresh().expect("same"), fp0, "nothing changed");

    for (bad, needle) in [("", "needs a name"), ("   ", "needs a name"), ("\u{1}\u{2}", "needs a name")] {
        match f.new_garden(bad, None, None, None) {
            Err(ZenError::BadRequest(m)) => assert!(m.contains(needle), "{bad:?}: {m}"),
            other => panic!("{bad:?} must be refused, got {other:?}"),
        }
    }
    let long = "x".repeat(61);
    assert!(matches!(f.new_garden(&long, None, None, None), Err(ZenError::BadRequest(_))), "61 characters is too long");
    f.new_garden(&"y".repeat(60), None, None, None).expect("60 characters is fine");
    match f.new_garden("GARDEN 1", None, None, None) {
        Err(ZenError::GardenExists(m)) => assert!(m.contains("You already have a Garden called"), "{m}"),
        other => panic!("a case clash must be GardenExists, got {other:?}"),
    }
    assert!(matches!(f.new_garden("x", Some("dashboard"), None, None), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.new_garden("x", None, Some("../evil"), None), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.new_garden("x", None, None, Some(0)), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.new_garden("x", None, None, Some(5)), Err(ZenError::BadRequest(_))), "n+1 is the last position");

    let fp1 = f.refresh().expect("fp1");
    assert_ne!(fp1, fp0, "a create moves the fingerprint");
    let m = f.new_garden("Mornings", Some("texting"), Some("home-2"), Some(1)).expect("mornings first");
    assert_eq!(m.template, "k2.texting@1");
    assert_eq!(m.seed_home.as_deref(), Some("home-2"), "seedHome round trip");
    assert_eq!(f.find_garden(&m.id).expect("find").1.seed_home.as_deref(), Some("home-2"), "seedHome is on disk");
    let ids: Vec<String> = f.gardens().into_iter().map(|g| g.id).collect();
    assert_eq!(ids[0], m.id, "at = 1 puts it first: {ids:?}");
    assert_eq!(ids[1], d);
    let fp2 = f.refresh().expect("fp2");

    // Rename: same name is a no-op; a case change is a change; clash refused.
    let (_, changed) = f.rename_garden(&m.id, "Mornings").expect("same name");
    assert!(!changed, "the same name changes nothing");
    assert_eq!(f.refresh().expect("same fp"), fp2, "a no-op rename keeps the fingerprint");
    assert!(matches!(f.rename_garden(&m.id, "garden 1"), Err(ZenError::GardenExists(_))));
    let (g, changed) = f.rename_garden("mornings", "Dawn").expect("rename by name");
    assert!(changed && g.name == "Dawn" && g.id == m.id, "ids are stable: {g:?}");
    let fp3 = f.refresh().expect("fp3");
    assert_ne!(fp3, fp2, "a rename moves the fingerprint");
    assert!(f.history_root().join(format!("gardens/{}.toml", m.id)).is_dir(), "the stub was snapshotted on refresh");
    let snaps_before = f.snapshots(&garden(&m.id)).len();

    // Reorder: 1-based, out of range refused, same place a no-op.
    assert!(matches!(f.reorder_garden(&m.id, 0), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.reorder_garden(&m.id, 5), Err(ZenError::BadRequest(_))));
    let (_, moved) = f.reorder_garden(&m.id, 1).expect("same place");
    assert!(!moved);
    let (list, moved) = f.reorder_garden("Dawn", 4).expect("to the end");
    assert!(moved);
    assert_eq!(list[3].id, m.id, "{list:?}");
    assert_ne!(f.refresh().expect("fp4"), fp3, "a reorder moves the fingerprint");
    assert_eq!(f.snapshots(&garden(&m.id)).len(), snaps_before, "rename and reorder never touch history");
    assert!(matches!(f.rename_garden("nowhere", "x"), Err(ZenError::UnknownGarden { .. })));
}

/// G17: delete moves the page into `.history/` (never `rm`), drops the
/// list entry and the theme pick; the last Garden stays.
#[test]
fn delete_moves_the_page_to_history_and_the_last_garden_stays() {
    let (_t, f) = set_up("delete");
    let d = gid(&f, 0);
    let n = f.new_garden("Notes", None, None, None).expect("notes");
    let page = garden(&n.id);
    let text = "schema = 1\n[shape]\ngap = 6\n";
    write(&f, &page, text);
    f.set_theme(Some("paper"), Some("Notes")).expect("own theme");
    let fp = f.refresh().expect("fp");

    let out = f.delete_garden("notes").expect("delete");
    assert_eq!(out.deleted.id, n.id);
    let snap = out.snapshot.expect("the page moved into history");
    let moved = f.history_root().join(format!("gardens/{}.toml", n.id)).join(&snap);
    assert_eq!(std::fs::read_to_string(&moved).expect("snapshot"), text, "history holds the page byte for byte");
    assert!(!f.path_of(&page).exists(), "the page left gardens/");
    let rec: J = serde_json::from_str(
        &std::fs::read_to_string(f.history_root().join(format!("gardens/{}.toml/deleted.json", n.id))).expect("deleted.json"),
    )
    .expect("record JSON");
    assert_eq!(rec["name"], "Notes", "{rec}");
    assert!(rec["deletedAt"].is_string(), "{rec}");
    assert_eq!(f.gardens().len(), 2, "Garden 1 and Garden 2 are left");
    assert!(!f.read_active().gardens.contains_key(&n.id), "its theme pick goes with it");
    assert_ne!(f.refresh().expect("after"), fp, "a delete moves the fingerprint");
    match f.resolve(Some(&n.id)) {
        Err(ZenError::UnknownGarden { .. }) => {}
        other => panic!("a deleted Garden is unknown, got {other:?}"),
    }
    // History still lists it by id, as a deleted Garden.
    let h = f.history(Some(&f.garden_file(&n.id, true).expect("deleted id"))).expect("history");
    let entry = &h["files"][0];
    assert_eq!(entry["file"], format!("gardens/{}.toml", n.id));
    assert_eq!(entry["deleted"], true, "{h}");
    assert_eq!(entry["snapshots"].as_array().map(Vec::len).unwrap_or(0) >= 1, true, "{h}");
    let all = f.history(None).expect("all history");
    assert!(all["files"].as_array().expect("files").iter().any(|x| x["garden"] == n.id), "{all}");
    assert!(f.garden_file(&n.id, false).is_err(), "only history may name a deleted Garden");

    f.delete_garden("Garden 2").expect("delete Garden 2");
    match f.delete_garden(&d) {
        Err(ZenError::LastGarden) => {}
        other => panic!("the last Garden must stay, got {other:?}"),
    }
    assert_eq!(ZenError::LastGarden.to_string(), LAST_GARDEN);
    assert_eq!(LAST_GARDEN, "That's your last Garden.");
    // A Garden with no page file deletes too (no snapshot, still a record).
    let e = f.new_garden("Empty", None, None, None).expect("empty");
    std::fs::remove_file(f.path_of(&garden(&e.id))).expect("rm page");
    let out = f.delete_garden(&e.id).expect("delete fileless");
    assert!(out.snapshot.is_none());
    assert!(f.history_root().join(format!("gardens/{}.toml/deleted.json", e.id)).is_file());
}

/// G9: a missing or unreadable `gardens.json` is rebuilt from the files;
/// nothing is deleted, and an unreadable list is kept in history.
#[test]
fn list_is_rebuilt_from_files_and_an_unreadable_list_is_kept() {
    let (_t, f) = set_up("rebuild");
    let n = f.new_garden("Notes", None, None, None).expect("notes");
    std::fs::remove_file(f.root().join(GARDENS_FILE)).expect("rm gardens.json");
    let (list, source) = f.read_list();
    assert_eq!(source, ListSource::Rebuilt);
    let mut ids: Vec<String> = list.iter().map(|g| g.id.clone()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted, "id order");
    assert!(list.iter().all(|g| g.name == g.id), "name = id: {list:?}");
    assert_eq!(list.iter().find(|g| g.id == n.id).expect("notes").template, "k2.blank@1", "template from the file");
    assert!(f.is_set_up(), "files alone keep Zen set up");
    let doctor = f.doctor_checks();
    let c = doctor.iter().find(|c| c["name"] == "gardens.json").expect("gardens.json check");
    assert_eq!(c["ok"], false, "a rebuilt list is a doctor warning: {c}");
    // Any change writes it again.
    f.new_garden("Later", None, None, None).expect("later");
    assert_eq!(f.read_list().1, ListSource::File);

    std::fs::write(f.root().join(GARDENS_FILE), "{ not json").expect("break list");
    let (list, source) = f.read_list();
    assert!(matches!(source, ListSource::Unreadable(_)), "{source:?}");
    ids = list.iter().map(|g| g.id.clone()).collect();
    assert_eq!(ids.len(), 4, "rebuilt from the four files: {ids:?}");
    f.rename_garden(&n.id, "Notes again").expect("rename writes the list");
    let kept: Vec<_> = std::fs::read_dir(f.history_root().join(GARDENS_FILE)).expect("kept dir").flatten().collect();
    assert_eq!(kept.len(), 1, "the unreadable list is kept, not overwritten");
    assert_eq!(std::fs::read_to_string(kept[0].path()).expect("kept"), "{ not json");
    assert_eq!(f.read_list().1, ListSource::File);
}

#[test]
fn per_garden_pages_override_zen_toml() {
    let (_t, f) = set_up("pages");
    let one = gid(&f, 0);
    let two = f.new_garden("Play", Some("texting"), None, None).expect("second").id;
    write(&f, &garden(&two), "schema = 1\ntemplate = \"k2.texting@1\"\n[shape]\nradius = 2\n[colors.dark]\naccent = \"#ffcc00\"\n");
    let a = f.resolve(Some(&one)).expect("one");
    let b = f.resolve(Some(&two)).expect("two");
    assert_eq!(a["theme"]["tokens"]["shape"]["radius"], 14);
    assert_eq!(b["theme"]["tokens"]["shape"]["radius"], 2);
    assert_eq!(b["theme"]["tokens"]["colors"]["dark"]["accent"], "#ffcc00");
    assert_eq!(b["page"], a["page"], "both are the texting template");
    assert_ne!(a["version"], b["version"], "a page override changes the version");
    match f.resolve(Some("../etc")) {
        Err(ZenError::UnknownGarden { .. }) => {}
        other => panic!("a bad Garden must be refused, got {other:?}"),
    }
}

#[test]
fn refresh_fingerprint_moves_only_on_effective_change() {
    let (_t, f) = set_up("fingerprint");
    let a = f.refresh().expect("a");
    assert_eq!(f.refresh().expect("again"), a, "nothing changed");
    let before = f.snapshots(&ZenFile::Zen).len();
    let mut text = std::fs::read_to_string(f.path_of(&ZenFile::Zen)).expect("zen");
    text.push_str("\n# just a comment\n");
    write(&f, &ZenFile::Zen, &text);
    assert_eq!(f.refresh().expect("comment"), a, "a comment-only save resolves the same");
    assert_eq!(f.snapshots(&ZenFile::Zen).len(), before + 1, "but it is new content: snapshot it");
    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\ngap = 20\n");
    let b = f.refresh().expect("b");
    assert_ne!(a, b, "a real change moves the fingerprint");
    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\ngap = 20\nbogus = 1\n");
    let c = f.refresh().expect("c");
    assert_ne!(c, b, "new errors move the fingerprint");
    // A Garden page's layout is in the fingerprint too.
    let g = gid(&f, 0);
    write(&f, &garden(&g), "schema = 1\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 50\n[[layout.column]]\nsize = 50\n");
    assert_ne!(f.refresh().expect("d"), c, "a layout change moves the fingerprint");
}

// ── agent can't grant (file layer) ──────────────────────────────────

#[test]
fn no_route_level_name_reaches_daemon_owned_files_and_grants_are_ignored() {
    for bad in [
        "grants.json",
        "gardens.json",
        "homes.json",
        ".history/zen.toml",
        "../zen.toml",
        "gardens/../grants.json",
        "gardens/a b.toml",
        "pages/home-1.toml",
        "/etc/passwd",
    ] {
        assert!(ZenFile::parse(bad).is_err(), "{bad} must not be a nameable Zen file");
    }
    match ZenFile::parse("pages/home-1.toml") {
        Err(ZenError::BadRequest(m)) => assert!(m.contains("Zen pages are Gardens now"), "{m}"),
        other => panic!("pages/ must point at gardens/, got {other:?}"),
    }
    assert_eq!(ZenFile::parse("zen.toml").expect("zen"), ZenFile::Zen);
    assert_eq!(ZenFile::parse("gardens/g-3f9a12c0.toml").expect("garden"), garden("g-3f9a12c0"));

    let (_t, f) = set_up("grants2");
    let g = gid(&f, 0);
    let before = f.resolve(Some(&g)).expect("before");
    let grants = f.root().join("grants.json");
    let planted = r#"{"widgets":{"evil":{"caps":["thread:post","net:*"]}}}"#;
    std::fs::write(&grants, planted).expect("plant grants.json");
    f.refresh().expect("refresh");
    let after = f.resolve(Some(&g)).expect("after");
    assert_eq!(after["version"], before["version"], "Zen ignores grants.json entirely");
    assert_eq!(after["page"], before["page"]);
    let _ = f.reset(&ZenFile::Zen, None).expect("reset");
    let n = f.new_garden("Notes", None, None, None).expect("new");
    f.delete_garden(&n.id).expect("delete");
    assert_eq!(std::fs::read_to_string(&grants).expect("grants"), planted, "no Zen call writes grants.json");
    let (_t, h) = set_up("grants3");
    let _ = h.refresh();
    let _ = h.reset(&ZenFile::Zen, None);
    assert!(!h.root().join("grants.json").exists(), "Zen never creates grants.json");
}

/// A folder left from before Gardens (no list) is not set up, and setup
/// adds Garden 1 and Garden 2 without touching the old files (no
/// migration, Rosson).
#[test]
fn a_leftover_folder_without_gardens_is_not_set_up_and_setup_ignores_old_files() {
    let (_t, f) = temp_root("leftover");
    std::fs::create_dir_all(f.root().join("pages")).expect("mkdir pages");
    std::fs::write(f.root().join("pages/home-1.toml"), "schema = 1\ntemplate = \"k2.texting@1\"\n").expect("old page");
    std::fs::write(f.root().join("homes.json"), r#"{"version":1,"homes":[{"id":"home-1","name":"Work"}]}"#).expect("homes.json");
    std::fs::write(f.root().join(ACTIVE_FILE), r#"{"version":1,"theme":"paper","homes":{"home-1":"midnight"}}"#).expect("active v1");
    assert!(f.exists() && !f.is_set_up(), "a folder without Gardens is not set up");
    assert!(matches!(f.refresh(), Err(ZenError::NotSetUp)));
    let out = f.setup().expect("setup");
    assert!(!out.created_folder && out.created_default, "{out:?}");
    let names: Vec<String> = f.gardens().into_iter().map(|g| g.name).collect();
    assert_eq!(names, vec!["Garden 1".to_string(), "Garden 2".to_string()]);
    assert!(f.root().join("pages/home-1.toml").is_file() && f.root().join("homes.json").is_file(), "old files are ignored, not touched");
    let g = f.resolve(None).expect("resolve");
    assert_eq!(g["theme"]["name"], "paper", "the global pick survives: {g}");
    assert_eq!(g["theme"]["scope"], "global", "an old per-Home pick is ignored: {g}");
}

// ── T3.2 / TG2.2 the k2-zen skill ─────────────────────────────────────

#[test]
fn t3_2_skill_documents_every_schema_token_and_the_grant_rule() {
    let body = zen::skill::generate_k2_zen_skill();
    let lower = body.to_lowercase();
    for must in ["k2 zen validate", "k2 zen reset", "k2 zen history", "k2 zen doctor", "~/.k2/zen"] {
        assert!(body.contains(must), "skill must mention {must}");
    }
    // TG2.2: Gardens.
    for must in [
        "k2 zen garden list",
        "k2 zen garden new <name>",
        "k2 zen garden rename",
        "k2 zen garden delete",
        "k2 zen garden reorder",
        "--garden",
        "k2 zen validate --garden <id>",
        "gardens/<id>.toml",
        "In my Zen Garden",
        "[layout]",
        "[[widget]]",
        "[[layout.column]]",
    ] {
        assert!(body.contains(must), "skill must mention {must}");
    }
    assert!(!body.contains("--home"), "the skill must not teach --home any more");
    assert!(!body.contains("pages/<home-id>"), "the skill must not teach per-Home pages");
    assert!(lower.contains("never write grants.json"), "skill must say never write grants.json");
    assert!(lower.contains("never write gardens.json") && lower.contains("never write .history/"), "skill must fence the daemon-owned files");
    assert!(lower.contains("never write active.json"), "skill must fence active.json");
    for must in ["k2 zen theme list", "k2 zen theme next", "k2 zen theme prev", "k2 zen theme set <name>", "k2 zen theme new <name>", "k2 zen reset --theme", "themes/<name>/theme.toml"] {
        assert!(body.contains(must), "skill must mention {must}");
    }
    assert!(lower.contains("request") && lower.contains("never grant"), "agents request, never grant");
    let mut want: Vec<String> = Vec::new();
    want.extend(schema::COLOR_TOKENS.iter().map(|t| format!("`{t}`")));
    want.extend(schema::TERMINAL_TOKENS.iter().map(|t| format!("`{t}`")));
    want.extend(schema::BACKGROUND_FITS.iter().map(|t| format!("`{t}`")));
    want.extend(schema::BACKGROUND_TYPES.iter().map(|(e, _)| format!("`.{e}`")));
    want.extend(schema::BUNDLE_TABLES.iter().map(|t| t.to_string()));
    want.extend(zen::BUILTIN_THEMES.iter().map(|t| format!("`{}`", t.name)));
    want.extend(schema::NUM_TOKENS.iter().map(|n| format!("`{}`", n.key)));
    want.extend(schema::ANIMATION_TREE.iter().map(|(n, _)| format!("`{n}`")));
    want.extend(schema::BUILTIN_BEZIERS.iter().map(|(n, _)| format!("`{n}`")));
    want.extend(schema::SCHEMES.iter().chain(schema::FAMILIES).chain(schema::CORNERS).chain(schema::STOPLIGHTS).map(|t| format!("`{t}`")));
    want.extend(schema::THEME_TABLES.iter().map(|t| t.to_string()));
    want.extend(zen::BRIDGE_CAPS.iter().map(|c| format!("`{c}`")));
    want.extend(zen::REQUIRED_CONTROLS.iter().map(|c| format!("`{c}`")));
    want.extend(schema::TEMPLATE_IDS.iter().map(|t| format!("`{t}`")));
    want.extend(schema::LAYOUT_KINDS.iter().map(|t| format!("`{t}`")));
    // TG2.2: every built-in widget kind and prop (generated).
    want.extend(schema::WIDGET_KINDS.iter().map(|(k, _)| format!("**`{k}`**")));
    want.extend(schema::TEMPLATE_WIDGET_KINDS.iter().map(|(k, _)| format!("`{k}`")));
    want.extend(schema::WIDGET_PROPS.iter().map(|p| format!("- `{}` (", p.name)));
    want.extend(schema::AGENTS_MODES.iter().chain(schema::AGENT_STATUSES).map(|m| format!("`{m}`")));
    want.extend(["`fade`", "`slide`", "`slidefade`", "`popin <n>%`", "`stoplight-offset"].map(String::from));
    want.extend(
        [
            "agents.list()",
            "agents.add(",
            "agents.home()",
            "agents.setHome(",
            "agents.local()",
            "homes.list()",
            "compose.draft(",
            "gardens.list()",
            "gardens.current()",
            "gardens.switch(",
            "gardens.create(",
            "gardens.rename(",
            "gardens.delete(",
            "gardens.useTemplate(",
            "presence.get(",
            "thread.read(",
            "thread.post(",
            "thread.answer(",
            "thread.void(",
            "zen.exit()",
            "controls.bind(",
            "theme.get()",
        ]
        .map(String::from),
    );
    let missing: Vec<&String> = want.iter().filter(|w| !body.contains(w.as_str())).collect();
    assert!(missing.is_empty(), "skill is missing schema names: {missing:?}");
    assert!(!body.contains("homes.select("), "G29: homes.select is gone");
    for (name, b) in schema::BUILTIN_BEZIERS {
        let shown = b.iter().map(|v| if v.fract() == 0.0 { format!("{}", *v as i64) } else { format!("{v}") }).collect::<Vec<_>>().join(", ");
        assert!(body.contains(&format!("`{name}` [{shown}]")), "skill must show {name}'s points [{shown}]");
    }
    assert!(!body.contains("leaves Zen in this window"), "v6: the rail no longer leaves Zen");
    assert!(body.contains("`app.current()`"), "v6: app.current is documented");
    for must in ["k2 zen garden template <garden> texting|blank [--force]", "gardens.useTemplate(", "Start with the default", "`basic`"] {
        assert!(body.contains(must), "skill must mention {must}");
    }
    assert!(!body.contains("`default` theme") && !body.contains("K2's `default`"), "the skill must not name a `default` theme");
    assert_eq!(
        k2_core::skills::version::SKILL_VERSION_ZEN,
        9,
        "k2-zen skill v9 (freeform chrome: Zen controls are widgets; v8 was `slot = \"top\"` for the nav rail)"
    );
    // v8: the top band slot, with the example people copy (kept in v9).
    for must in [
        "[[widget]]\nkind = \"nav-rail\"\nslot = \"top\"\n",
        "`slot`: `column`, `top`, `bottom`, `menu`",
        "Content kinds that fit a band: `nav-rail`",
        "- `orientation` (",
    ] {
        assert!(body.contains(must), "skill must mention {must:?}");
    }
}

/// FC-T5 (prd-zen-freeform-chrome FC38): skill v9 documents every value of
/// the chrome tables, the two examples, and the guide pointer; the live
/// "the band's controls stay where they are" text is gone.
#[test]
fn fc_t5_skill_v9_documents_the_chrome_tables() {
    let body = k2_core::zen::skill::generate_k2_zen_skill();
    let mut want: Vec<String> = Vec::new();
    want.extend(schema::WIDGET_SLOTS.iter().map(|s| format!("`{s}`")));
    want.extend(schema::CHROME_KINDS.iter().map(|(k, _)| format!("**`{k}`**")));
    want.extend(schema::MENU_ITEM_KINDS.iter().map(|k| format!("`{k}`")));
    want.extend(schema::ALIGNS.iter().map(|a| format!("`{a}`")));
    want.extend(schema::EDGES.iter().map(|e| format!("`{e}`")));
    want.extend(schema::MENU_ICONS.iter().map(|i| format!("`{i}`")));
    want.extend(schema::FILL_WIDGET_KINDS.iter().map(|k| format!("`{k}`")));
    want.extend(
        [
            format!("{} items per band", schema::MAX_BAND_ITEMS),
            format!("{} per column edge", schema::MAX_EDGE_ITEMS),
            format!("at most {} menus", schema::MAX_MENUS),
            format!("A menu holds 1 to {} items", schema::MAX_MENU_ITEMS),
            format!("{} Zen controls", schema::MAX_CHROME_WIDGETS),
            schema::DRAG_REGION_ERROR.to_string(),
            "Before moving controls, read `k2 zen guide bands` and `k2 zen guide required`".to_string(),
            "`bottom-bar`".to_string(),
            "`menu-both`".to_string(),
            "LAST item in the file sits in the corner".to_string(),
            "K2 grants it `gardens:manage`".to_string(),
            "- `icon` (".to_string(),
            "- `label` (".to_string(),
        ],
    );
    let missing: Vec<&String> = want.iter().filter(|w| !body.contains(w.as_str())).collect();
    assert!(missing.is_empty(), "skill v9 is missing chrome names: {missing:?}");
    assert!(!body.contains("stay where they are"), "the live v8 sentence about fixed controls must go");
    assert!(!body.contains("can't drop or move them"), "controls can move now");
    // The two examples in the skill are clean Garden files.
    for (name, start) in [("bottom-bar", "`bottom-bar`"), ("menu-both", "`menu-both`")] {
        let at = body.find(start).unwrap_or_else(|| panic!("{name} example"));
        let rest = &body[at..];
        let open = rest.find("```toml\n").unwrap_or_else(|| panic!("{name}: no toml fence")) + "```toml\n".len();
        let close = rest[open..].find("```").unwrap_or_else(|| panic!("{name}: no closing fence"));
        let src = &rest[open..open + close];
        let c = check_blank(src);
        assert!(c.is_clean() && c.warnings.is_empty(), "skill example {name} must validate clean:\n{}\n{src}", diag_list(&c));
    }
}

fn temp_dot(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("k2-zen-dot-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("mkdir dot");
    dir
}

fn skill_path(dot: &Path) -> PathBuf {
    dot.join("skills/k2-zen/SKILL.md")
}

#[test]
fn k2_zen_skill_is_seeded_only_where_zen_is_set_up() {
    let dot = temp_dot("off");
    assert!(!k2_core::workspace::skill_regen::ensure_zen_skill(&dot, false), "no Zen folder: no skill");
    assert!(!skill_path(&dot).exists(), "headless servers never see k2-zen");
    assert!(k2_core::workspace::skill_regen::ensure_zen_skill(&dot, true), "Zen set up: skill written");
    let text = std::fs::read_to_string(skill_path(&dot)).expect("skill");
    assert!(text.contains("name: k2-zen"), "frontmatter: {}", &text[..text.len().min(300)]);
    assert!(text.contains("k2 zen validate"));
    let _ = std::fs::remove_dir_all(&dot);
}

#[test]
fn diagnostics_serialize_with_file_line_col_message() {
    let c = check_zen("schema = 1\nacent = 1\n");
    let v: J = serde_json::to_value(&c.errors[0]).expect("serialize");
    assert_eq!(v["file"], "zen.toml");
    assert_eq!(v["line"], 2);
    assert_eq!(v["col"], 1);
    assert!(v["message"].as_str().is_some_and(|m| m.starts_with("unknown key 'acent'")), "{v}");
}

fn png(extra: usize) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.resize(8 + extra, 7);
    b
}

#[test]
fn every_builtin_theme_is_clean_complete_and_distinct() {
    // Rosson 2026-10-04: K2's own theme is `basic` (it was `default`).
    assert_eq!(zen::DEFAULT_THEME, "basic");
    assert_eq!(zen::BUILTIN_THEMES[0].name, zen::DEFAULT_THEME, "basic comes first");
    let names: Vec<&str> = zen::BUILTIN_THEMES.iter().map(|t| t.name).collect();
    assert_eq!(names, vec!["basic", "paper", "midnight"], "no built-in is called default any more");
    // Rosson 2026-10-04: ids stay lower case; people see capitalized names.
    let labels: Vec<&str> = zen::BUILTIN_THEMES.iter().map(|t| t.label).collect();
    assert_eq!(labels, vec!["Basic", "Paper", "Midnight"]);
    assert_eq!(zen::theme_label("basic"), "Basic");
    assert_eq!(zen::theme_label("sunset"), "Sunset", "a user theme shows its id with a capital");
    assert_eq!(zen::theme_label("k2-light"), "K2-light");
    assert!(zen::BUILTIN_THEMES[0].toml.starts_with("# Zen theme \"basic\""), "themes/basic.toml is embedded");
    let mut versions = std::collections::BTreeSet::new();
    for t in zen::BUILTIN_THEMES {
        let c = zen::check_builtin_theme(t.name).expect("built in");
        assert!(c.is_clean() && c.warnings.is_empty(), "built-in {} must be clean:\n{}", t.name, diag_list(&c));
        let layer = zen::builtin_theme_layer(t.name).expect("layer");
        let rt = schema::resolve(layer, &Layer::new());
        for scheme in ["light", "dark"] {
            for tok in schema::COLOR_TOKENS {
                assert!(rt.tokens["colors"][scheme][*tok].is_string(), "{} colors.{scheme}.{tok}", t.name);
            }
            for tok in schema::TERMINAL_TOKENS {
                assert!(rt.terminal["palette"][scheme][*tok].is_string(), "{} terminal.{scheme}.{tok}", t.name);
            }
        }
        assert!(rt.font["stack"].is_string() && rt.font["terminal"]["monospace"] == true, "{}: {}", t.name, rt.font);
        assert!(zen::valid_theme_name(t.name), "{} is a valid name", t.name);
        versions.insert(format!("{:?}", (rt.tokens, rt.font)));
    }
    assert_eq!(versions.len(), zen::BUILTIN_THEMES.len(), "every built-in looks different");
    let mid = schema::resolve(zen::builtin_theme_layer("midnight").expect("midnight"), &Layer::new());
    assert_eq!(mid.font["family"], "jetbrains-mono");
    assert_eq!(mid.font["terminal"]["family"], "jetbrains-mono", "a monospace family is used in terminals as is");
    assert_eq!(mid.tokens["scheme"], "dark");
}

#[test]
fn layering_builtin_then_override_then_zen_toml_then_garden_and_reset_clears_the_override() {
    let (_t, f) = set_up("layering");
    let g = gid(&f, 0);
    // zen.toml starts with no tables: the defaults live in the built-ins.
    let stub = std::fs::read_to_string(f.path_of(&ZenFile::Zen)).expect("zen.toml");
    assert_eq!(stub, zen::DEFAULT_ZEN_TOML);
    assert!(check_zen(&stub).layer.is_empty(), "the zen.toml stub sets nothing");
    let base = f.resolve(Some(&g)).expect("base");
    assert_eq!(base["theme"]["name"], "basic");
    assert_eq!(base["theme"]["builtin"], true);
    assert_eq!(base["theme"]["user"], false);
    assert_eq!(base["theme"]["scope"], "global");
    assert_eq!(base["theme"]["tokens"]["colors"]["light"]["accent"], "#2563eb");

    // Override the built-in: the user's value wins, the rest stays K2's.
    let out = f.new_theme("basic", None).expect("override basic");
    assert_eq!(out.from, "basic");
    let ov = ZenFile::Theme("basic".into());
    let text = std::fs::read_to_string(f.path_of(&ov)).expect("override text");
    assert!(text.contains("started from K2's \"basic\""), "{text}");
    assert!(check_theme(&text).is_clean(), "the starter validates:\n{}", diag_list(&check_theme(&text)));
    let same = f.resolve(Some(&g)).expect("same");
    assert_eq!(same["theme"]["tokens"], base["theme"]["tokens"], "a fresh copy looks the same");
    assert_eq!(same["theme"]["user"], true);
    write(&f, &ov, "schema = 1\n[colors.light]\naccent = \"#1d4ed8\"\n");
    let o = f.resolve(Some(&g)).expect("override");
    assert_eq!(o["theme"]["tokens"]["colors"]["light"]["accent"], "#1d4ed8", "override beats the built-in");
    assert_eq!(o["theme"]["tokens"]["colors"]["light"]["canvas"], "#f7f8fa", "unset keys come from the built-in");

    // zen.toml over the theme, the Garden over zen.toml.
    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\nradius = 5\n[colors.light]\naccent = \"#7c2d12\"\n");
    let z = f.resolve(Some(&g)).expect("zen");
    assert_eq!(z["theme"]["tokens"]["shape"]["radius"], 5);
    assert_eq!(z["theme"]["tokens"]["colors"]["light"]["accent"], "#7c2d12", "zen.toml beats the theme");
    write(&f, &garden(&g), "schema = 1\ntemplate = \"k2.texting@1\"\n[colors.light]\naccent = \"#065f46\"\n");
    let pg = f.resolve(Some(&g)).expect("page");
    assert_eq!(pg["theme"]["tokens"]["colors"]["light"]["accent"], "#065f46", "the Garden beats zen.toml");
    assert_eq!(pg["theme"]["tokens"]["shape"]["radius"], 5);

    // Another theme: zen.toml still applies; the basic override doesn't.
    write(&f, &garden(&g), &zen::store::garden_stub(&g, "Garden 1", zen::TEMPLATE_ID));
    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\nradius = 5\n");
    f.set_theme(Some("paper"), None).expect("paper");
    let p = f.resolve(Some(&g)).expect("paper resolve");
    assert_eq!(p["theme"]["name"], "paper");
    assert_eq!(p["theme"]["tokens"]["colors"]["light"]["accent"], "#2f5d8a");
    assert_eq!(p["theme"]["tokens"]["shape"]["radius"], 5, "zen.toml applies on every theme");
    assert_eq!(p["theme"]["font"]["family"], "serif");
    assert_eq!(p["theme"]["tokens"]["colors"]["dark"]["canvas"], "#121316", "paper's unset keys come from basic");

    // reset --theme basic: snapshot, then the override is gone.
    f.set_theme(Some("basic"), None).expect("back");
    let r = f.reset(&ov, None).expect("reset override");
    assert_eq!(r.restored, "builtin");
    let kept = r.snapshot.expect("the override is kept as a snapshot");
    assert!(std::fs::read_to_string(f.history_root().join("themes/basic/theme.toml").join(&kept)).expect("snap").contains("#1d4ed8"));
    assert!(!f.path_of(&ov).exists(), "reset removes the override");
    let back = f.resolve(Some(&g)).expect("back");
    assert_eq!(back["theme"]["tokens"]["colors"]["light"]["accent"], "#2563eb", "the built-in shows again");
    assert_eq!(back["theme"]["user"], false);
    // Undo the reset from history.
    f.reset(&ov, Some(&kept)).expect("undo");
    assert_eq!(f.resolve(Some(&g)).expect("undo")["theme"]["tokens"]["colors"]["light"]["accent"], "#1d4ed8");

    // A user-only theme resets to a copy of basic; reset never touches active.json.
    f.new_theme("mine", Some("midnight")).expect("mine");
    let active_before = std::fs::read(f.root().join(ACTIVE_FILE)).expect("active.json");
    write(&f, &ZenFile::Theme("mine".into()), "schema = 1\n[shape]\ngap = 4\n");
    let r = f.reset(&ZenFile::Theme("mine".into()), None).expect("reset mine");
    assert_eq!(r.restored, "default");
    let t = std::fs::read_to_string(f.path_of(&ZenFile::Theme("mine".into()))).expect("mine");
    assert!(t.contains("started from K2's \"basic\""), "{t}");
    assert_eq!(std::fs::read(f.root().join(ACTIVE_FILE)).expect("active"), active_before);
}

/// G16: one global theme plus an optional pick per Garden, in
/// `active.json` v2; a deleted Garden's pick goes with it.
#[test]
fn cycling_order_wraps_and_a_garden_pick_beats_the_global_one() {
    let (_t, f) = set_up("cycle");
    let one = gid(&f, 0);
    let two = f.new_garden("Play", None, None, None).expect("play").id;
    f.new_theme("zeta", None).expect("zeta");
    f.new_theme("alpha", None).expect("alpha");
    let names: Vec<String> = f.themes().into_iter().map(|t| t.name).collect();
    assert_eq!(names, vec!["basic", "paper", "midnight", "alpha", "zeta"], "built-ins first, then the user's by name");
    let labels: Vec<String> = f.themes().into_iter().map(|t| t.label).collect();
    assert_eq!(labels, vec!["Basic", "Paper", "Midnight", "Alpha", "Zeta"], "lists show capitalized names");
    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(f.cycle_theme(1, None).expect("next").theme);
    }
    assert_eq!(seen, vec!["paper", "midnight", "alpha", "zeta", "basic"], "next walks the list and wraps");
    assert_eq!(f.cycle_theme(-1, None).expect("prev").theme, "zeta", "prev wraps backwards");
    assert_eq!(f.cycle_theme(-1, None).expect("prev").theme, "alpha");

    // Per Garden: cycling for Garden 1 starts from what it shows (the global).
    let w = f.cycle_theme(1, Some("Garden 1")).expect("garden next");
    assert_eq!((w.theme.as_str(), w.scope.as_str(), w.garden.as_deref()), ("zeta", "garden", Some(one.as_str())));
    assert_eq!(f.active_theme(None).name, "alpha", "the global pick is untouched");
    let r = f.resolve(Some(&one)).expect("w");
    assert_eq!(r["theme"]["name"], "zeta");
    assert_eq!(r["theme"]["scope"], "garden", "{r}");
    assert_eq!(f.resolve(Some(&two)).expect("other")["theme"]["name"], "alpha", "other Gardens follow the global");
    let active: J = serde_json::from_str(&std::fs::read_to_string(f.root().join(ACTIVE_FILE)).expect("active.json")).expect("JSON");
    assert_eq!(active["version"], 2, "{active}");
    assert_eq!(active["gardens"][&one], "zeta", "{active}");
    let list = f.theme_list(Some(&one)).expect("list");
    assert_eq!(list["garden"], one.as_str());
    assert_eq!(list["gardenTheme"], "zeta");
    let gj = f.gardens_json();
    assert_eq!(gj[0]["theme"], "zeta", "the list shows a Garden's own theme: {gj:?}");
    assert!(gj[1]["theme"].is_null(), "{gj:?}");
    let cleared = f.set_theme(None, Some(&one)).expect("clear");
    assert_eq!((cleared.theme.as_str(), cleared.scope.as_str()), ("alpha", "global"));

    // A deleted Garden's pick goes with it.
    f.set_theme(Some("paper"), Some("Play")).expect("pick");
    assert!(f.read_active().gardens.contains_key(&two));
    f.delete_garden("Play").expect("delete");
    assert!(f.read_active().gardens.is_empty(), "{:?}", f.read_active());
}

#[test]
fn unknown_theme_names_are_refused_and_change_nothing() {
    let (_t, f) = set_up("unknown");
    let g = gid(&f, 0);
    match f.set_theme(Some("neon"), None) {
        Err(ZenError::UnknownTheme { name, known }) => {
            assert_eq!(name, "neon");
            assert_eq!(known, vec!["basic", "paper", "midnight"]);
        }
        other => panic!("an unknown theme must be UnknownTheme, got {other:?}"),
    }
    let msg = f.set_theme(Some("neon"), None).expect_err("unknown").to_string();
    assert!(msg.contains("no theme 'neon'") && msg.contains("k2 zen theme new neon"), "{msg}");
    assert!(!f.root().join(ACTIVE_FILE).exists(), "a refused switch writes nothing");
    assert!(matches!(f.set_theme(Some("paper"), Some("Nowhere")), Err(ZenError::UnknownGarden { .. })));
    assert!(matches!(f.reset(&ZenFile::Theme("neon".into()), None), Err(ZenError::UnknownTheme { .. })));
    assert!(matches!(f.new_theme("Bad Name", None), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.new_theme("x", Some("neon")), Err(ZenError::UnknownTheme { .. })));
    f.new_theme("mine", None).expect("mine");
    assert!(matches!(f.new_theme("mine", None), Err(ZenError::Conflict(_))), "new never overwrites");
    for bad in ["themes/../grants.json", "themes/Mine/theme.toml", "themes/a/b/theme.toml", "active.json"] {
        assert!(ZenFile::parse(bad).is_err(), "{bad} must not be nameable");
    }
    assert_eq!(ZenFile::parse("themes/mine/theme.toml").expect("theme"), ZenFile::Theme("mine".into()));

    // The active theme's folder is deleted by hand: basic, with a warning.
    f.set_theme(Some("mine"), None).expect("mine");
    std::fs::remove_dir_all(f.theme_dir("mine")).expect("rm mine");
    let r = f.resolve(Some(&g)).expect("resolve");
    assert_eq!(r["theme"]["name"], "basic", "{r}");
    let w = r["warnings"].as_array().expect("warnings");
    assert!(w.iter().any(|d| d["file"] == "active.json" && d["message"].as_str().is_some_and(|m| m.contains("theme 'mine' doesn't exist"))), "{r}");
    assert_eq!(r["errors"], json!([]));
}

/// Rosson 2026-10-04: `default` is `basic` now. Zen never shipped, so
/// nothing is migrated, except that a saved pick of `default` in an
/// existing `active.json` reads as `basic`, quietly (no warning), unless
/// the person has a theme of their own called `default`.
#[test]
fn a_saved_default_pick_reads_as_basic_quietly() {
    let (_t, f) = set_up("alias");
    let one = gid(&f, 0);
    let two = gid(&f, 1);
    std::fs::write(
        f.root().join(ACTIVE_FILE),
        format!(r#"{{"version":2,"theme":"default","gardens":{{"{two}":"default"}}}}"#),
    )
    .expect("write active.json");
    let a = f.read_active();
    assert_eq!(a.theme.as_deref(), Some("basic"), "{a:?}");
    assert_eq!(a.gardens.get(&two).map(String::as_str), Some("basic"), "{a:?}");
    let t = f.active_theme(Some(&one));
    assert_eq!((t.name.as_str(), t.scope, t.missing.as_deref()), ("basic", "global", None));
    let t2 = f.active_theme(Some(&two));
    assert_eq!((t2.name.as_str(), t2.scope, t2.missing.as_deref()), ("basic", "garden", None));
    for g in [&one, &two] {
        let r = f.resolve(Some(g)).expect("resolve");
        assert_eq!(r["theme"]["name"], "basic", "{r}");
        assert_eq!(r["warnings"], json!([]), "the alias is quiet: {r}");
        assert_eq!(r["errors"], json!([]), "{r}");
    }
    let list = f.theme_list(None).expect("list");
    assert_eq!(list["active"], "basic", "{list}");
    assert_eq!(list["global"], "basic", "{list}");
    assert!(list["missing"].is_null(), "{list}");
    let doctor = f.doctor_checks();
    let themes = doctor.iter().find(|c| c["name"] == "themes").expect("themes check");
    assert_eq!(themes["ok"], true, "the doctor doesn't flag the alias: {themes}");
    // A switch writes the new name; `default` is never written back.
    f.cycle_theme(1, None).expect("next");
    let raw = std::fs::read_to_string(f.root().join(ACTIVE_FILE)).expect("active.json");
    assert!(raw.contains("\"paper\"") && !raw.contains("\"default\""), "{raw}");

    // `default` is not a built-in name any more: set refuses it.
    assert!(matches!(f.set_theme(Some("default"), None), Err(ZenError::UnknownTheme { .. })));
    // The person's own theme called `default` is theirs: no alias.
    f.new_theme("default", None).expect("a user theme called default");
    f.set_theme(Some("default"), None).expect("pick it");
    assert_eq!(f.read_active().theme.as_deref(), Some("default"));
    let r = f.resolve(Some(&one)).expect("resolve");
    assert_eq!((r["theme"]["name"].as_str(), r["theme"]["builtin"].as_bool()), (Some("default"), Some(false)), "{r}");
}

/// Rosson 2026-10-04, "Start with the default": `garden/template` turns a
/// Garden into Garden 1's texting page (or back to empty), keeping its id,
/// name and place; the old file goes to history first; idempotent; a file
/// with its own layout, widgets or theme tables needs `force`.
#[test]
fn garden_template_starts_a_garden_over_with_the_default() {
    let (t, f) = temp_root("template-unset");
    assert!(matches!(f.set_garden_template("Garden 2", "texting", false), Err(ZenError::NotSetUp)));
    drop(t);
    let (_t, f) = set_up("template");
    let two = gid(&f, 1);
    let before = f.gardens();
    assert_eq!(before[1].template, zen::BLANK_TEMPLATE_ID);
    let fp0 = f.refresh().expect("fp0");
    let stub = std::fs::read_to_string(f.path_of(&garden(&two))).expect("Garden 2 stub");

    let out = f.set_garden_template("Garden 2", "texting", false).expect("texting");
    assert!(out.changed, "{out:?}");
    assert_eq!(out.template, zen::TEMPLATE_ID);
    assert!(out.replaced.is_empty(), "an empty Garden loses nothing: {out:?}");
    assert_eq!((out.garden.id.as_str(), out.garden.name.as_str()), (two.as_str(), "Garden 2"));
    let after = f.gardens();
    assert_eq!(after.iter().map(|g| (&g.id, &g.name)).collect::<Vec<_>>(), before.iter().map(|g| (&g.id, &g.name)).collect::<Vec<_>>(), "same ids, names, order");
    assert_eq!(after[1].template, zen::TEMPLATE_ID, "the list names the new template");
    assert_eq!(after[1].created_at, before[1].created_at, "it is the same Garden");
    let text = std::fs::read_to_string(f.path_of(&garden(&two))).expect("new file");
    assert_eq!(text, zen::store::garden_stub(&two, "Garden 2", zen::TEMPLATE_ID));
    let snap = out.snapshot.clone().expect("the old file is kept");
    assert_eq!(
        std::fs::read_to_string(f.history_root().join(format!("gardens/{two}.toml")).join(&snap)).expect("snapshot"),
        stub,
        "history holds the empty stub"
    );
    let page = f.resolve(Some(&two)).expect("resolve");
    assert_eq!(page["page"]["template"], zen::TEMPLATE_ID, "{page}");
    assert_eq!(kinds_of(&page["page"], "widgets"), vec!["agents", "conversation", "nav-rail"], "Garden 1's page: {page}");
    assert_eq!(page["page"], zen::texting_page(), "exactly the default layout");
    assert_eq!(page["garden"], json!({"id": two, "name": "Garden 2", "index": 2}), "{page}");
    assert_eq!(page["errors"], json!([]), "{page}");
    let fp1 = f.refresh().expect("fp1");
    assert_ne!(fp0, fp1, "the switch moves the fingerprint (one zen_changed)");

    // Idempotent: nothing written, nothing moved.
    let n_snaps = f.snapshots(&garden(&two)).len();
    let again = f.set_garden_template(&two, "texting", false).expect("again");
    assert!(!again.changed && again.snapshot.is_none(), "{again:?}");
    assert_eq!(std::fs::read_to_string(f.path_of(&garden(&two))).expect("file"), text);
    assert_eq!(f.snapshots(&garden(&two)).len(), n_snaps);
    assert_eq!(f.refresh().expect("fp2"), fp1, "a no-op moves nothing");
    // And back to empty, the same way.
    let blank = f.set_garden_template("garden 2", "blank", false).expect("blank");
    assert!(blank.changed);
    assert_eq!(f.gardens()[1].template, zen::BLANK_TEMPLATE_ID);
    assert_eq!(f.resolve(Some(&two)).expect("blank")["page"]["template"], zen::BLANK_TEMPLATE_ID);

    // A Garden with its own widgets and colors: refused without force,
    // nothing written; force replaces them and keeps them in history.
    let mine = "schema = 1\ntemplate = \"k2.blank@1\"\n[[widget]]\nid = \"work\"\nkind = \"agents\"\ncolumn = 0\n[colors.light]\naccent = \"#065f46\"\n";
    write(&f, &garden(&two), mine);
    assert_eq!(f.garden_own_keys(&two).expect("keys"), vec!["colors".to_string(), "widget".to_string()]);
    match f.set_garden_template(&two, "texting", false) {
        Err(ZenError::GardenHasChanges { garden, keys }) => {
            assert_eq!(garden, two);
            assert_eq!(keys, vec!["colors", "widget"]);
        }
        other => panic!("own changes need force, got {other:?}"),
    }
    assert_eq!(std::fs::read_to_string(f.path_of(&garden(&two))).expect("file"), mine, "a refusal writes nothing");
    assert_eq!(f.gardens()[1].template, zen::BLANK_TEMPLATE_ID);
    let forced = f.set_garden_template(&two, "texting", true).expect("force");
    assert_eq!(forced.replaced, vec!["colors", "widget"]);
    let kept = forced.snapshot.expect("kept");
    assert_eq!(
        std::fs::read_to_string(f.history_root().join(format!("gardens/{two}.toml")).join(&kept)).expect("snapshot"),
        mine,
        "the replaced file is in history"
    );
    // A file that doesn't parse counts as changes too.
    write(&f, &garden(&two), "schema = 1\n[[widget\n");
    assert!(matches!(f.set_garden_template(&two, "blank", false), Err(ZenError::GardenHasChanges { .. })));

    // Bad input.
    assert!(matches!(f.set_garden_template("Nowhere", "texting", false), Err(ZenError::UnknownGarden { .. })));
    assert!(matches!(f.set_garden_template(&two, "dashboard", false), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.set_garden_template(&two, " ", false), Err(ZenError::BadRequest(_))));
    assert!(!f.root().join("grants.json").exists(), "garden/template never writes grants.json");
}

#[test]
fn only_known_theme_tokens_are_accepted() {
    let c = check_zen("schema = 1\n[terminal.dark]\nbluee = \"#000\"\n");
    assert_one_error(&c, 3, "did you mean 'blue'", "terminal token");
    let c = check_zen("schema = 1\n[terminal.dim]\nblue = \"#000\"\n");
    assert_one_error(&c, 2, "unknown key 'dim'", "terminal scheme");
    let c = check_zen("schema = 1\n[terminal.light]\nred = \"crimson\"\n");
    assert_one_error(&c, 3, "is not a colour", "terminal colour");
    let c = check_zen("schema = 1\n[font]\nfamily = \"comic-sans\"\n");
    assert_one_error(&c, 3, "is not one of", "font family");
    let c = check_zen("schema = 1\n[font]\nweight = 700\n");
    assert_one_error(&c, 3, "unknown key 'weight'", "font key");
    let c = check_zen("schema = 1\n\n[type]\nsize = 14\n");
    assert_one_error(&c, 3, "[type] is now [font]", "old [type]");
    let c = check_zen("schema = 1\n[background]\nimage = \"a.png\"\n");
    assert_one_error(&c, 2, "belongs in a theme bundle", "background outside a bundle");
    let c = check_page("schema = 1\ntemplate = \"k2.texting@1\"\n[background]\nimage = \"a.png\"\n");
    assert_one_error(&c, 3, "belongs in a theme bundle", "background on a page");
    for (bad, needle) in [
        ("../x.png", "plain file name"),
        ("sub/x.png", "plain file name"),
        (".hidden.png", "plain file name"),
        ("wall.svg", "no SVG"),
        ("wall.bmp", "must be one of"),
    ] {
        let c = check_theme(&format!("schema = 1\n[background]\nimage = \"{bad}\"\n"));
        assert_one_error(&c, 3, needle, bad);
    }
    let c = check_theme("schema = 1\n[background]\nfit = \"stretch\"\n");
    assert_one_error(&c, 3, "is not one of", "fit");
    let c = check_theme("schema = 1\ntemplate = \"k2.texting@1\"\n");
    assert_one_error(&c, 2, "template line belongs in gardens/", "template in a theme");
    let c = check_theme("schema = 1\n[background]\nimage = \"wall.JPG\"\nfit = \"cover\"\nopacity = 0.4\n[terminal.dark]\nblue = \"#00f\"\n[font]\nfamily = \"lilex\"\n");
    assert!(c.is_clean(), "{}", diag_list(&c));
}

#[test]
fn theme_file_errors_keep_the_last_good_theme() {
    let (_t, f) = set_up("theme-lastgood");
    f.new_theme("sunset", None).expect("sunset");
    f.set_theme(Some("sunset"), None).expect("use");
    let file = ZenFile::Theme("sunset".into());
    write(&f, &file, "schema = 1\n[colors.dark]\naccent = \"#ff9e64\"\n");
    f.refresh().expect("refresh good");
    let good = f.resolve(None).expect("good");
    write(&f, &file, "schema = 1\n[colors.dark]\naccent = \"#ff9e64\"\n\n  acent = \"#000\"\n");
    f.refresh().expect("refresh bad");
    let bad = ZenFiles::new(f.root().to_path_buf()).resolve(None).expect("bad");
    assert_eq!(bad["theme"], good["theme"], "the last good theme stays");
    assert_eq!(bad["version"], good["version"]);
    let e = &bad["errors"][0];
    assert_eq!((e["file"].as_str(), e["line"].as_i64(), e["col"].as_i64()), (Some("themes/sunset/theme.toml"), Some(5), Some(3)), "{bad}");
    assert!(bad["sources"]["themes/sunset/theme.toml"].as_str().is_some_and(|s| s.starts_with("snapshot:")), "{bad}");
    let v = f.validate(Some(&file)).expect("validate");
    assert_eq!(v["ok"], false, "{v}");
}

#[test]
fn background_image_is_capped_typed_and_keeps_its_last_good_copy() {
    let (_t, f) = set_up("background");
    f.new_theme("sunset", None).expect("sunset");
    f.set_theme(Some("sunset"), None).expect("use");
    let file = ZenFile::Theme("sunset".into());
    let dir = f.theme_dir("sunset");
    write(&f, &file, "schema = 1\n[background]\n\nimage = \"wall.png\"\nfit = \"contain\"\n");

    // Missing image: error at the image line, no background yet.
    let g = f.resolve(None).expect("missing");
    assert!(g["theme"].get("background").is_none(), "{g}");
    assert_eq!(g["errors"][0]["line"], 4, "{g}");
    assert!(g["errors"][0]["message"].as_str().is_some_and(|m| m.contains("isn't in this theme's folder")), "{g}");

    // At the cap exactly: served as a data URL; refresh keeps a copy.
    let ok = png(schema::MAX_BACKGROUND_BYTES as usize - 8);
    std::fs::write(dir.join("wall.png"), &ok).expect("png");
    f.refresh().expect("refresh");
    let g = f.resolve(None).expect("ok");
    assert_eq!(g["errors"], serde_json::json!([]), "{}", g["errors"]);
    let bg = &g["theme"]["background"];
    use base64::Engine as _;
    let want = format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&ok));
    assert_eq!(bg["dataUrl"].as_str(), Some(want.as_str()), "the image is the data URL");
    assert_eq!(bg["bytes"], ok.len());
    assert_eq!(bg["fit"], "contain");
    assert_eq!(bg["opacity"], 1, "opacity defaults to 1");
    assert_eq!(bg["lastGood"], false);
    let good_version = g["version"].clone();

    // One byte over the cap: refused with file:line, the last good stays.
    std::fs::write(dir.join("wall.png"), png(schema::MAX_BACKGROUND_BYTES as usize - 7)).expect("big");
    f.refresh().expect("refresh big");
    let g = f.resolve(None).expect("big");
    let e = &g["errors"][0];
    assert_eq!((e["file"].as_str(), e["line"].as_i64()), (Some("themes/sunset/theme.toml"), Some(4)), "{e}");
    assert!(e["message"].as_str().is_some_and(|m| m.contains("is 2097153 bytes; the limit is 2097152 bytes")), "{e}");
    assert_eq!(g["theme"]["background"]["dataUrl"].as_str(), Some(want.as_str()), "the last good image stays");
    assert_eq!(g["theme"]["background"]["lastGood"], true);
    assert_ne!(g["version"], good_version, "lastGood moves the version");
    let v = f.validate(None).expect("validate");
    assert_eq!(v["ok"], false, "validate reports the image: {v}");

    // Wrong bytes for the extension, empty, and a link are refused too.
    std::fs::write(dir.join("wall.png"), [0xFFu8, 0xD8, 0xFF, 0xE0, 0, 0]).expect("jpeg bytes");
    let g = f.resolve(None).expect("jpeg");
    assert!(g["errors"][0]["message"].as_str().is_some_and(|m| m.contains("is really image/jpeg")), "{g}");
    std::fs::write(dir.join("wall.png"), b"").expect("empty");
    let g = f.resolve(None).expect("empty");
    assert!(g["errors"][0]["message"].as_str().is_some_and(|m| m.contains("is empty")), "{g}");
    std::fs::remove_file(dir.join("wall.png")).expect("rm");
    let outside = f.root().join("outside.png");
    std::fs::write(&outside, &ok).expect("outside");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, dir.join("wall.png")).expect("symlink");
        let g = f.resolve(None).expect("link");
        assert!(g["errors"][0]["message"].as_str().is_some_and(|m| m.contains("is a link")), "{g}");
        assert_eq!(g["theme"]["background"]["lastGood"], true, "a link never replaces the last good image");
    }

    // theme new --from a user theme copies its image.
    std::fs::remove_file(dir.join("wall.png")).expect("rm link");
    std::fs::write(dir.join("wall.png"), &ok).expect("png again");
    let out = f.new_theme("dusk", Some("sunset")).expect("dusk");
    assert_eq!(out.copied_image.as_deref(), Some("wall.png"));
    assert_eq!(std::fs::read(f.theme_dir("dusk").join("wall.png")).expect("copied"), ok);
}
