//! prd-garden-column-collapse-and-agent-list-v1 A1 (Lane A): collapsible
//! Garden columns, the validator and the page answer.
//!
//! - `collapsible`, `collapsed` and `label` on `[[layout.column]]` reach the
//!   page answer only when the file sets them (CA5): a page without them is
//!   byte for byte what it was.
//! - Refused: `collapsed` without `collapsible`, every column collapsed, a
//!   collapsible one-column page, a bad `label` (CA15), a non-boolean flag.
//! - CA9: a collapsible column whose effective chrome holds `zen-toggle` or
//!   `garden-switcher` (directly at its edge, through a menu at its edge, or
//!   from the template's chrome when the file places none), or that holds a
//!   `nav-rail`.
//!
//! Pure validator calls: no daemon, no `~/.k2`. Fixtures are made up.

use k2_core::zen::defaults::{Defaults, DefaultsSet};
use k2_core::zen::schema;
use k2_core::zen::{self as zen};
use serde_json::{json, Value as J};

fn lines(ls: &[&str]) -> String {
    let mut s = ls.join("\n");
    s.push('\n');
    s
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

fn assert_clean(c: &schema::Checked, what: &str) {
    assert!(c.errors.is_empty() && c.warnings.is_empty(), "{what}: want no findings, got:\n{}", diag_list(c));
}

/// Exactly one error at `line` whose message contains `needle`.
fn assert_one_error(c: &schema::Checked, line: usize, needle: &str, what: &str) {
    assert_eq!(c.errors.len(), 1, "{what}: want exactly one error, got:\n{}", diag_list(c));
    let e = &c.errors[0];
    assert_eq!(e.line, line, "{what}: wrong line: {}", e.render());
    assert!(e.message.contains(needle), "{what}: message {:?} must contain {needle:?}", e.message);
}

/// The resolved page's `layout` for a clean file on the texting template.
fn layout_of(src: &str) -> J {
    let c = check_page(src);
    assert_clean(&c, "layout_of");
    zen::garden_page(&c.layer, zen::TEMPLATE_ID)["layout"].clone()
}

/// Two content columns (agents | conversation) and the required chrome in
/// the top band, with `col0` / `col1` extra lines on each column.
fn two_columns(col0: &[&str], col1: &[&str]) -> String {
    let mut v: Vec<&str> = vec!["schema = 1", "[layout]", "kind = \"columns\"", "[[layout.column]]", "size = 40"];
    v.extend_from_slice(col0);
    v.extend_from_slice(&["[[layout.column]]", "size = 60"]);
    v.extend_from_slice(col1);
    v.extend_from_slice(&[
        "[[widget]]",
        "id = \"agents\"",
        "kind = \"agents\"",
        "column = 0",
        "[[widget]]",
        "id = \"chat\"",
        "kind = \"conversation\"",
        "column = 1",
    ]);
    lines(&v)
}

#[test]
fn a1_keys_reach_the_layout_only_when_set() {
    // Without the keys: the columns are exactly what they were (CA5).
    let plain = layout_of(&two_columns(&[], &[]));
    for c in plain["columns"].as_array().expect("columns") {
        let keys: Vec<&String> = c.as_object().expect("column object").keys().collect();
        assert_eq!(keys, vec!["min-width", "size", "widget"], "no fold keys unless set: {plain}");
    }
    assert_eq!(plain["split"], json!([40, 60]), "{plain}");

    // With them: on the one column that set them, as set.
    let folded = layout_of(&two_columns(&[], &["collapsible = true", "collapsed = true", "label = \"Chat\""]));
    assert_eq!(folded["columns"][1]["collapsible"], json!(true), "{folded}");
    assert_eq!(folded["columns"][1]["collapsed"], json!(true), "{folded}");
    assert_eq!(folded["columns"][1]["label"], json!("Chat"), "{folded}");
    assert!(folded["columns"][0].get("collapsible").is_none(), "column 0 set nothing: {folded}");
    assert!(folded["columns"][0].get("label").is_none(), "{folded}");
    // `collapsible = false` is sent as set; a label alone is kept too.
    let off = layout_of(&two_columns(&["label = \"People\""], &["collapsible = false"]));
    assert_eq!(off["columns"][1]["collapsible"], json!(false), "{off}");
    assert!(off["columns"][1].get("collapsed").is_none(), "{off}");
    assert_eq!(off["columns"][0]["label"], json!("People"), "{off}");
    // Every column collapsible is fine, as long as one starts open.
    let both = layout_of(&two_columns(&["collapsible = true"], &["collapsible = true", "collapsed = true"]));
    assert_eq!(both["columns"][0]["collapsible"], json!(true), "{both}");
    assert!(both["columns"][0].get("collapsed").is_none(), "{both}");
}

/// 0.46.4 (Rosson 2026-10-23): stacked widgets are separate tiles; a column
/// that wants one panel says `joined = true`. Sent only when set, a boolean.
#[test]
fn joined_reaches_the_layout_only_when_set() {
    let plain = layout_of(&two_columns(&[], &[]));
    for c in plain["columns"].as_array().expect("columns") {
        assert!(c.get("joined").is_none(), "no joined unless set: {plain}");
    }
    let joined = layout_of(&two_columns(&[], &["joined = true"]));
    assert_eq!(joined["columns"][1]["joined"], json!(true), "{joined}");
    assert!(joined["columns"][0].get("joined").is_none(), "{joined}");
    let off = layout_of(&two_columns(&["joined = false"], &[]));
    assert_eq!(off["columns"][0]["joined"], json!(false), "{off}");
    // It works beside the fold keys.
    let both = layout_of(&two_columns(&[], &["collapsible = true", "joined = true"]));
    assert_eq!((both["columns"][1]["collapsible"].clone(), both["columns"][1]["joined"].clone()), (json!(true), json!(true)), "{both}");
    let c = check_page(&two_columns(&[], &["joined = \"yes\""]));
    assert_one_error(&c, 8, "column joined is true or false", "string flag");
}

#[test]
fn a1_layout_json_drops_collapsed_on_a_fixed_column() {
    let cols = vec![
        json!({"size": 50, "min-width": 0, "collapsed": true}),
        json!({"size": 50, "min-width": 0, "collapsible": true, "collapsed": true, "label": "B"}),
    ];
    let l = zen::layout_json(&json!("columns"), &J::Null, &cols);
    assert!(l["columns"][0].get("collapsed").is_none(), "{l}");
    assert_eq!(l["columns"][1]["collapsed"], json!(true), "{l}");
    assert_eq!(l["columns"][1]["label"], json!("B"), "{l}");
}

#[test]
fn a1_refusals_in_the_layout() {
    // `collapsed` without `collapsible`: at the collapsed value (line 9).
    let c = check_page(&two_columns(&[], &["collapsed = true"]));
    assert_one_error(&c, 8, "set collapsible = true too", "collapsed alone");
    let c = check_page(&two_columns(&[], &["collapsible = false", "collapsed = true"]));
    assert_one_error(&c, 9, "set collapsible = true too", "collapsed on a fixed column");

    // Every column collapsed: at the last column's collapsed.
    let c = check_page(&two_columns(&["collapsible = true", "collapsed = true"], &["collapsible = true", "collapsed = true"]));
    assert_one_error(&c, 11, "every column starts collapsed", "all collapsed");

    // A one-column page.
    let c = check_blank(&lines(&[
        "schema = 1",
        "[layout]",
        "kind = \"columns\"",
        "[[layout.column]]",
        "size = 100",
        "collapsible = true",
    ]));
    assert_one_error(&c, 6, "a one-column page can't fold its column", "one column");

    // Flags are booleans.
    let c = check_page(&two_columns(&[], &["collapsible = \"yes\""]));
    assert_one_error(&c, 8, "column collapsible is true or false", "string flag");
    let c = check_page(&two_columns(&[], &["collapsible = true", "collapsed = 1"]));
    assert_one_error(&c, 9, "column collapsed is true or false", "number flag");

    // Labels: 1–24 characters, not blank, no control characters, text.
    for (bad, what) in [
        ("label = \"\"", "empty"),
        ("label = \"   \"", "blank"),
        ("label = \"This label is far too long!\"", "25 characters"),
        ("label = \"Chat\\u0007\"", "a control character"),
        ("label = \"Two\\nlines\"", "a newline"),
        ("label = 7", "a number"),
    ] {
        let c = check_page(&two_columns(&[], &["collapsible = true", bad]));
        assert_one_error(&c, 9, "column label is text, 1 to 24 characters", what);
    }
    assert_clean(&check_page(&two_columns(&[], &["collapsible = true", "label = \"Twenty-four characters!!\""])), "24 is fine");

    // An unknown key names every column key.
    let c = check_page(&two_columns(&[], &["folded = true"]));
    assert_one_error(&c, 8, "collapsible", "unknown key lists the new keys");
}

#[test]
fn ca9_required_controls_and_the_rail_stay_out_of_a_collapsible_column() {
    // The file's own chrome, right at column 1's edge.
    let mut src = two_columns(&[], &["collapsible = true"]);
    src.push_str(&lines(&[
        "[[widget]]",
        "kind = \"garden-switcher\"",
        "slot = \"top\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "column = 1",
        "edge = \"bottom\"",
    ]));
    let c = check_page(&src);
    assert_one_error(&c, 8, "column 1 is collapsible, but its edge holds the zen-toggle", "toggle at the edge");

    // Through a menu at column 1's edge.
    let mut src = two_columns(&[], &["collapsible = true"]);
    src.push_str(&lines(&[
        "[[widget]]",
        "id = \"more\"",
        "kind = \"menu\"",
        "column = 1",
        "[[widget]]",
        "kind = \"garden-switcher\"",
        "slot = \"menu\"",
        "menu = \"more\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"top\"",
    ]));
    let c = check_page(&src);
    assert_one_error(&c, 8, "the garden-switcher (in menu 'more')", "switcher in a menu at the edge");

    // A menu at the edge holding only optional controls is fine, and so are
    // optional controls right at the edge.
    let mut src = two_columns(&[], &["collapsible = true"]);
    src.push_str(&lines(&[
        "[[widget]]",
        "id = \"more\"",
        "kind = \"menu\"",
        "column = 1",
        "[[widget]]",
        "kind = \"theme-picker\"",
        "slot = \"menu\"",
        "menu = \"more\"",
        "[[widget]]",
        "kind = \"usage\"",
        "column = 1",
        "edge = \"bottom\"",
        "[[widget]]",
        "kind = \"garden-switcher\"",
        "slot = \"top\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"top\"",
    ]));
    assert_clean(&check_page(&src), "optional controls at a collapsible edge");

    // A nav rail in the column (the file's widgets).
    let mut src = two_columns(&["collapsible = true"], &[]);
    src.push_str(&lines(&["[[widget]]", "id = \"nav\"", "kind = \"nav-rail\"", "column = 0"]));
    let c = check_page(&src);
    assert_one_error(&c, 6, "it holds the nav rail 'nav'", "rail strip");
    // A row rail at its edge too.
    let mut src = two_columns(&["collapsible = true"], &[]);
    src.push_str(&lines(&["[[widget]]", "kind = \"nav-rail\"", "column = 0", "edge = \"top\""]));
    let c = check_page(&src);
    assert_one_error(&c, 6, "it holds the nav rail", "row rail at the edge");
    // The template's rail (texting puts it in column 0) when the file
    // places no widgets of its own.
    let c = check_page(&lines(&[
        "schema = 1",
        "[layout]",
        "kind = \"columns\"",
        "[[layout.column]]",
        "size = 34",
        "collapsible = true",
        "[[layout.column]]",
        "size = 66",
    ]));
    assert_one_error(&c, 6, "it holds the nav rail 'nav'", "the template's rail");
    // The other column of that template is fine.
    assert_clean(
        &check_page(&lines(&[
            "schema = 1",
            "[layout]",
            "kind = \"columns\"",
            "[[layout.column]]",
            "size = 34",
            "[[layout.column]]",
            "size = 66",
            "collapsible = true",
            "label = \"Chat\"",
        ])),
        "texting's conversation column",
    );
}

/// The template's chrome counts when the file places none (FC24 rule 1).
/// No built-in template puts a control at a column edge, so this builds a
/// set with one that does.
#[test]
fn ca9_template_chrome_at_a_column_edge_counts() {
    const TID: &str = "test.edge@1";
    let tpl = lines(&[
        "id = \"test.edge@1\"",
        "schema = 1",
        "[layout]",
        "kind = \"columns\"",
        "[[layout.column]]",
        "size = 50",
        "[[layout.column]]",
        "size = 50",
        "[[widget]]",
        "id = \"agents\"",
        "kind = \"agents\"",
        "column = 0",
        "[[widget]]",
        "id = \"chat\"",
        "kind = \"conversation\"",
        "column = 1",
        "[[widget]]",
        "kind = \"garden-switcher\"",
        "slot = \"top\"",
        "[[widget]]",
        "id = \"more\"",
        "kind = \"menu\"",
        "column = 1",
        "edge = \"top\"",
        "[[widget]]",
        "kind = \"zen-toggle\"",
        "slot = \"menu\"",
        "menu = \"more\"",
    ]);
    let mut set = DefaultsSet::from_binary();
    set.templates.insert(TID.to_string(), tpl);
    set.fingerprint = set.compute_fingerprint().expect("fingerprint");
    let d = Defaults::from_set(set).expect("a set with the test template");
    let src = lines(&[
        "schema = 1",
        "[layout]",
        "kind = \"columns\"",
        "[[layout.column]]",
        "size = 50",
        "[[layout.column]]",
        "size = 50",
        "collapsible = true",
    ]);
    let c = schema::check_garden_in(&d, "gardens/g-1.toml", &src, zen::builtin_layer(), TID);
    assert_one_error(&c, 8, "the zen-toggle (in menu 'more')", "template chrome in a menu at the edge");
    // Column 0 holds no control there.
    let src0 = src.replace("size = 50\n[[layout.column]]\nsize = 50\ncollapsible = true\n", "size = 50\ncollapsible = true\n[[layout.column]]\nsize = 50\n");
    assert_ne!(src0, src, "the swap must change the file");
    let c = schema::check_garden_in(&d, "gardens/g-1.toml", &src0, zen::builtin_layer(), TID);
    assert_clean(&c, "column 0 of the edge template");
    // The file's own chrome replaces the template's: then the edge is empty.
    let mut own = src.clone();
    own.push_str(&lines(&["[[widget]]", "kind = \"garden-switcher\"", "slot = \"top\"", "[[widget]]", "kind = \"zen-toggle\"", "slot = \"top\""]));
    let c = schema::check_garden_in(&d, "gardens/g-1.toml", &own, zen::builtin_layer(), TID);
    assert_clean(&c, "the file's chrome replaces the template's");
}
