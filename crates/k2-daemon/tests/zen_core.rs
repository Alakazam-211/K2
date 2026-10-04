//! Zen Mode v1 core (prd-zen-mode-v1 S1/S3, tests T1.5, T1.6, T1.2, T3.2).
//!
//! Drives `k2_core::zen` directly on temp folders: the schema table, last
//! good, `.history/` and reset, per-Home pages, `homes.json` sync, the
//! agent-can't-grant rule at the file layer, and the `k2-zen` skill.
//! k2-core tests run through this daemon test binary (never `cargo test -p
//! k2-core`). Nothing here touches the real `~/.k2/zen`.
//!
//! Fail loudly: every assertion names what it saw.

use std::path::{Path, PathBuf};

use k2_core::zen::schema::{self, FileKind, Layer};
use k2_core::zen::store::{HISTORY_KEEP, HOMES_FILE};
use k2_core::zen::{self as zen, HomeEntry, ZenError, ZenFile, ZenFiles};
use serde_json::Value as J;

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

fn set_up(tag: &str) -> (TempRoot, ZenFiles) {
    let (t, f) = temp_root(tag);
    let out = f.ensure_page("home-1", "Work").expect("ensure_page");
    assert!(out.created_folder && out.created_zen && out.created_page, "first ensure creates all: {out:?}");
    (t, f)
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

fn check_page(src: &str) -> schema::Checked {
    schema::check("pages/home-1.toml", src, FileKind::Page, zen::builtin_layer())
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

// ── builtin + template ───────────────────────────────────────────────

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
    assert!(rt.background.is_none(), "the default theme has no background");
    assert_eq!(rt.tokens["scheme"], "auto");
    assert_eq!(rt.chrome["corners"], "system");
    assert_eq!(rt.chrome["stoplights"], "round");
    assert_eq!(rt.chrome["stoplight-offset"], serde_json::json!([0, 0]));
    let anims = rt.motion["animations"].as_object().expect("animations object");
    assert_eq!(anims.len(), schema::ANIMATION_TREE.len(), "one resolved line per tree node: {anims:?}");
    assert_eq!(rt.motion["reducedMotion"], "instant");
}

#[test]
fn template_page_is_data_with_required_controls_and_known_caps() {
    let page = zen::texting_page();
    assert_eq!(page["template"], "k2.texting@1");
    assert_eq!(page["layout"]["kind"], "columns");
    // The S4 renderer contract (docs/zen-contract.md).
    assert_eq!(page["layout"]["split"], serde_json::json!([34, 66]), "{page}");
    assert_eq!(page["layout"]["minWidths"], serde_json::json!([240, 360]), "{page}");
    let cols = page["layout"]["columns"].as_array().expect("columns");
    assert_eq!(cols.len(), 2, "{page}");
    assert_eq!(cols[0]["widget"], "agents");
    assert_eq!(cols[1]["widget"], "conversation");
    let widgets = page["widgets"].as_array().expect("widgets");
    let kinds: Vec<&str> = widgets.iter().filter_map(|w| w["kind"].as_str()).collect();
    assert_eq!(kinds, vec!["agents", "conversation"], "{page}");
    assert_eq!(widgets[0]["column"], 0, "{page}");
    assert_eq!(widgets[1]["column"], 1, "{page}");
    for w in widgets {
        assert_eq!(w["source"], "builtin", "built-ins are granted by K2: {w}");
        for cap in w["caps"].as_array().expect("caps") {
            let cap = cap.as_str().expect("cap string");
            assert!(zen::BRIDGE_CAPS.contains(&cap), "unknown cap {cap} in {w}");
        }
    }
    // Decision 9: no unread tracking; the Agents widget shows live status.
    let status = &widgets[0]["props"]["status"];
    assert_eq!(status, &serde_json::json!(["working", "idle", "needs-you"]), "{page}");
    // Decision 6: compose reuses the attachment path.
    assert_eq!(widgets[1]["props"]["attachments"], true, "{page}");
    // The Agents widget may open K2's Add agent picker (`agents.add`).
    let agent_caps: Vec<&str> = widgets[0]["caps"].as_array().expect("agents caps").iter().filter_map(|c| c.as_str()).collect();
    assert_eq!(agent_caps, vec!["agents:read", "agents:add", "presence:read"], "{page}");
    assert!(zen::BRIDGE_CAPS.contains(&"agents:add"), "agents:add must be a known bridge cap");
    let controls: Vec<&str> =
        page["controls"].as_array().expect("controls").iter().filter_map(|c| c["kind"].as_str()).collect();
    for req in zen::REQUIRED_CONTROLS {
        assert!(controls.contains(req), "template must declare control {req}: {controls:?}");
    }
    // Rosson 2026-10-04: Home switcher top left; the Zen toggle (the way out)
    // and Add agent bottom left under the Agents column (column 0). Matches
    // the renderer's `k2.texting@1` controls.
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
        ("home-switcher", "top-left", None),
        ("drag-region", "top", None),
        ("zen-toggle", "bottom-left", Some(0)),
        ("add-agent", "bottom-left", Some(0)),
    ]
    .into_iter()
    .map(|(k, p, c)| (k.to_string(), p.to_string(), c))
    .collect();
    assert_eq!(placed, want, "{page}");
    assert_eq!(cols[0]["widget"], "agents", "column 0 must be the Agents column the footer sits under");
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
fn t1_5_v2_tables_warn_schema_2_errors_and_more() {
    let c = check_page("schema = 1\ntemplate = \"k2.texting@1\"\n\n[layout]\nkind = \"grid\"\n\n[[widget]]\nid = \"x\"\n");
    assert!(c.is_clean(), "[layout]/[[widget]] are warnings, not errors:\n{}", diag_list(&c));
    assert_eq!(c.warnings.len(), 2, "{}", diag_list(&c));
    assert_eq!(c.warnings[0].line, 4);
    assert_eq!(c.warnings[0].message, schema::V2_WARNING);
    assert_eq!(c.warnings[1].line, 7);

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
    assert_one_error(&c, 2, "belongs in pages/", "template in zen.toml");
    let c = check_page("schema = 1\ntemplate = \"k2.dashboard@1\"\n");
    assert_one_error(&c, 2, "unknown template", "page template");
    let c = check_zen("schema = 1\n[colors.light\ncanvas = \"#fff\"\n");
    assert_eq!(c.errors.len(), 1, "{}", diag_list(&c));
    assert!(c.errors[0].message.starts_with("invalid TOML"), "{}", c.errors[0].render());
    assert_eq!(c.errors[0].line, 2, "syntax error line: {}", c.errors[0].render());
    let big = format!("schema = 1\n#{}\n", "x".repeat(schema::MAX_FILE_BYTES));
    assert_one_error(&check_zen(&big), 1, "under 64 KB", "size cap");
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

// ── store: last good, history, reset, pages, homes ─────────────────────

#[test]
fn t1_2_broken_file_keeps_last_good_across_a_restart() {
    let (_t, f) = set_up("lastgood");
    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\nradius = 6\n");
    f.refresh().expect("refresh");
    let good = f.resolve(Some("home-1")).expect("resolve good");
    assert_eq!(good["theme"]["tokens"]["shape"]["radius"], 6, "{good}");
    assert_eq!(good["errors"], serde_json::json!([]));

    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\nradius = 6\nacent = 1\n");
    f.refresh().expect("refresh broken");
    // A fresh handle reads only the disk: the restart case.
    let after = ZenFiles::new(f.root().to_path_buf()).resolve(Some("home-1")).expect("resolve broken");
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
    let bare = f.resolve(Some("home-1")).expect("resolve no history");
    assert_eq!(bare["sources"]["zen.toml"], "default", "{bare}");
    assert_eq!(bare["theme"]["tokens"]["shape"]["radius"], 14);
    assert_eq!(bare["errors"].as_array().map(Vec::len), Some(1));
}

#[test]
fn t1_6_history_keeps_20_and_reset_snapshots_first_and_spares_homes_json() {
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

    let homes_before = std::fs::read(f.root().join(HOMES_FILE)).expect("homes.json");
    let oldest = snaps.last().expect("oldest").name.clone();
    let oldest_text = std::fs::read_to_string(f.history_root().join("zen.toml").join(&oldest)).expect("oldest text");
    let current_before = std::fs::read_to_string(f.path_of(&ZenFile::Zen)).expect("current");
    let out = f.reset(&ZenFile::Zen, Some(&oldest)).expect("reset to oldest");
    assert_eq!(out.restored, oldest);
    let kept = out.snapshot.expect("the pre-reset file is kept");
    let kept_text = std::fs::read_to_string(f.history_root().join("zen.toml").join(&kept)).expect("kept text");
    assert_eq!(kept_text, current_before, "reset must snapshot the current file first");
    assert_eq!(std::fs::read_to_string(f.path_of(&ZenFile::Zen)).expect("restored"), oldest_text);
    assert_eq!(std::fs::read(f.root().join(HOMES_FILE)).expect("homes.json"), homes_before, "reset never touches homes.json");

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

    // A page resets to its stub, named from homes.json.
    let page = ZenFile::Page("home-1".into());
    write(&f, &page, "schema = 1\ntemplate = \"k2.texting@1\"\n[shape]\ngap = 20\n");
    let out = f.reset(&page, None).expect("reset page");
    assert_eq!(out.file, "pages/home-1.toml");
    let stub = std::fs::read_to_string(f.path_of(&page)).expect("page stub");
    assert!(stub.contains("\"Work\"") && stub.contains("template = \"k2.texting@1\""), "{stub}");
}

#[test]
fn per_home_pages_override_zen_toml_and_missing_pages_are_the_template() {
    let (_t, f) = set_up("pages");
    f.ensure_page("home-2", "Play").expect("second home");
    write(&f, &ZenFile::Page("home-2".into()), "schema = 1\ntemplate = \"k2.texting@1\"\n[shape]\nradius = 2\n[colors.dark]\naccent = \"#ffcc00\"\n");
    let one = f.resolve(Some("home-1")).expect("home-1");
    let two = f.resolve(Some("home-2")).expect("home-2");
    let none = f.resolve(Some("never-made")).expect("missing page");
    assert_eq!(one["theme"]["tokens"]["shape"]["radius"], 14);
    assert_eq!(two["theme"]["tokens"]["shape"]["radius"], 2);
    assert_eq!(two["theme"]["tokens"]["colors"]["dark"]["accent"], "#ffcc00");
    assert_eq!(two["page"], one["page"], "v1 pages are all the template");
    assert_ne!(one["version"], two["version"], "a page override changes the version");
    assert_eq!(none["version"], one["version"], "a missing page resolves to the template + zen.toml");
    assert_eq!(none["errors"], serde_json::json!([]));
    match f.resolve(Some("../etc")) {
        Err(ZenError::BadRequest(_)) => {}
        other => panic!("a bad home id must be refused, got {other:?}"),
    }
    // ensure never overwrites.
    let again = f.ensure_page("home-2", "Play renamed").expect("ensure again");
    assert!(!again.created_page && !again.created_zen && !again.created_folder, "{again:?}");
    assert!(std::fs::read_to_string(f.path_of(&ZenFile::Page("home-2".into()))).expect("page").contains("radius = 2"));
    assert_eq!(f.find_home("Play renamed").as_deref(), Some("home-2"), "ensure renames in homes.json");
}

#[test]
fn homes_sync_archives_pages_of_deleted_homes() {
    let (_t, f) = set_up("sync");
    f.ensure_page("home-2", "Play").expect("second");
    let (written, archived) = f
        .sync_homes(vec![HomeEntry { id: "home-1".into(), name: "Work".into() }])
        .expect("sync");
    assert!(written);
    assert_eq!(archived, vec!["home-2".to_string()]);
    assert!(!f.path_of(&ZenFile::Page("home-2".into())).exists(), "page moved out");
    assert_eq!(f.snapshots(&ZenFile::Page("home-2".into())).len(), 1, "page kept in .history/");
    let h = f.history(None).expect("history");
    assert!(h["files"].as_array().expect("files").iter().any(|x| x["file"] == "pages/home-2.toml"), "{h}");
    // An empty list is a bug upstream: archive nothing.
    let (_, archived) = f.sync_homes(vec![]).expect("empty sync");
    assert!(archived.is_empty());
    assert!(f.path_of(&ZenFile::Page("home-1".into())).exists());
    assert!(f.sync_homes(vec![HomeEntry { id: "../x".into(), name: "x".into() }]).is_err());
    // Not set up: nothing written, no folder made.
    let (_t2, g) = temp_root("sync-none");
    assert_eq!(g.sync_homes(vec![HomeEntry { id: "a".into(), name: "A".into() }]).expect("sync"), (false, vec![]));
    assert!(!g.exists(), "homes/sync must never create the folder");
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
    assert_ne!(f.refresh().expect("c"), b, "new errors move the fingerprint");
}

// ── agent can't grant (file layer) ──────────────────────────────────

#[test]
fn no_route_level_name_reaches_daemon_owned_files_and_grants_are_ignored() {
    for bad in ["grants.json", "homes.json", ".history/zen.toml", "../zen.toml", "pages/../grants.json", "pages/a b.toml", "/etc/passwd"] {
        assert!(ZenFile::parse(bad).is_err(), "{bad} must not be a nameable Zen file");
    }
    assert_eq!(ZenFile::parse("zen.toml").expect("zen"), ZenFile::Zen);
    assert_eq!(ZenFile::parse("pages/home-1.toml").expect("page"), ZenFile::Page("home-1".into()));
    let (_t, f) = temp_root("grants");
    assert!(f.ensure_page("../grants", "x").is_err(), "a home id can't escape pages/");
    assert!(f.ensure_page("grants.json", "x").is_err());

    let (_t, f) = set_up("grants2");
    let before = f.resolve(Some("home-1")).expect("before");
    let grants = f.root().join("grants.json");
    let planted = r#"{"widgets":{"evil":{"caps":["thread:post","net:*"]}}}"#;
    std::fs::write(&grants, planted).expect("plant grants.json");
    f.refresh().expect("refresh");
    let after = f.resolve(Some("home-1")).expect("after");
    assert_eq!(after["version"], before["version"], "v1 ignores grants.json entirely");
    assert_eq!(after["page"], before["page"]);
    let _ = f.reset(&ZenFile::Zen, None).expect("reset");
    let _ = f.sync_homes(vec![HomeEntry { id: "home-1".into(), name: "Work".into() }]).expect("sync");
    assert_eq!(std::fs::read_to_string(&grants).expect("grants"), planted, "no Zen call writes grants.json");
    let (_t, g) = set_up("grants3");
    let _ = g.refresh();
    let _ = g.reset(&ZenFile::Zen, None);
    assert!(!g.root().join("grants.json").exists(), "Zen v1 never creates grants.json");
}

// ── T3.2 the k2-zen skill ─────────────────────────────────────────────

#[test]
fn t3_2_skill_documents_every_schema_token_and_the_grant_rule() {
    let body = zen::skill::generate_k2_zen_skill();
    let lower = body.to_lowercase();
    for must in ["k2 zen validate", "k2 zen reset", "k2 zen history", "k2 zen doctor", "~/.k2/zen"] {
        assert!(body.contains(must), "skill must mention {must}");
    }
    assert!(lower.contains("never write grants.json"), "skill must say never write grants.json");
    assert!(lower.contains("never write homes.json") && lower.contains("never write .history/"), "skill must fence the daemon-owned files");
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
    want.extend(["`fade`", "`slide`", "`slidefade`", "`popin <n>%`", "`stoplight-offset"].map(String::from));
    want.extend(["agents.list()", "agents.add(", "presence.get(", "thread.read(", "thread.post(", "thread.answer(", "thread.void(", "homes.select(", "zen.exit()", "controls.bind(", "theme.get()"].map(String::from));
    let missing: Vec<&String> = want.iter().filter(|w| !body.contains(w.as_str())).collect();
    assert!(missing.is_empty(), "skill is missing schema names: {missing:?}");
    for (name, b) in schema::BUILTIN_BEZIERS {
        let shown = b.iter().map(|v| if v.fract() == 0.0 { format!("{}", *v as i64) } else { format!("{v}") }).collect::<Vec<_>>().join(", ");
        assert!(body.contains(&format!("`{name}` [{shown}]")), "skill must show {name}'s points [{shown}]");
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

// ── Omarchy additions: built-in themes, layering, cycling, images ─────

fn png(extra: usize) -> Vec<u8> {
    let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
    b.resize(8 + extra, 7);
    b
}

#[test]
fn every_builtin_theme_is_clean_complete_and_distinct() {
    assert_eq!(zen::BUILTIN_THEMES[0].name, zen::DEFAULT_THEME, "default comes first");
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
fn layering_builtin_then_override_then_zen_toml_then_page_and_reset_clears_the_override() {
    let (_t, f) = set_up("layering");
    // zen.toml starts with no tables: the defaults live in the built-ins.
    let stub = std::fs::read_to_string(f.path_of(&ZenFile::Zen)).expect("zen.toml");
    assert_eq!(stub, zen::DEFAULT_ZEN_TOML);
    assert!(check_zen(&stub).layer.is_empty(), "the zen.toml stub sets nothing");
    let base = f.resolve(Some("home-1")).expect("base");
    assert_eq!(base["theme"]["name"], "default");
    assert_eq!(base["theme"]["builtin"], true);
    assert_eq!(base["theme"]["user"], false);
    assert_eq!(base["theme"]["scope"], "global");
    assert_eq!(base["theme"]["tokens"]["colors"]["light"]["accent"], "#2563eb");

    // Override the built-in: the user's value wins, the rest stays K2's.
    let out = f.new_theme("default", None).expect("override default");
    assert_eq!(out.from, "default");
    let ov = ZenFile::Theme("default".into());
    let text = std::fs::read_to_string(f.path_of(&ov)).expect("override text");
    assert!(text.contains("started from K2's \"default\""), "{text}");
    assert!(check_theme(&text).is_clean(), "the starter validates:\n{}", diag_list(&check_theme(&text)));
    let same = f.resolve(Some("home-1")).expect("same");
    assert_eq!(same["theme"]["tokens"], base["theme"]["tokens"], "a fresh copy looks the same");
    assert_eq!(same["theme"]["user"], true);
    write(&f, &ov, "schema = 1\n[colors.light]\naccent = \"#1d4ed8\"\n");
    let o = f.resolve(Some("home-1")).expect("override");
    assert_eq!(o["theme"]["tokens"]["colors"]["light"]["accent"], "#1d4ed8", "override beats the built-in");
    assert_eq!(o["theme"]["tokens"]["colors"]["light"]["canvas"], "#f7f8fa", "unset keys come from the built-in");

    // zen.toml over the theme, the page over zen.toml.
    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\nradius = 5\n[colors.light]\naccent = \"#7c2d12\"\n");
    let z = f.resolve(Some("home-1")).expect("zen");
    assert_eq!(z["theme"]["tokens"]["shape"]["radius"], 5);
    assert_eq!(z["theme"]["tokens"]["colors"]["light"]["accent"], "#7c2d12", "zen.toml beats the theme");
    write(&f, &ZenFile::Page("home-1".into()), "schema = 1\ntemplate = \"k2.texting@1\"\n[colors.light]\naccent = \"#065f46\"\n");
    let pg = f.resolve(Some("home-1")).expect("page");
    assert_eq!(pg["theme"]["tokens"]["colors"]["light"]["accent"], "#065f46", "the page beats zen.toml");
    assert_eq!(pg["theme"]["tokens"]["shape"]["radius"], 5);

    // Another theme: zen.toml still applies; the default override doesn't.
    write(&f, &ZenFile::Page("home-1".into()), &zen::store::page_stub("home-1", "Work"));
    write(&f, &ZenFile::Zen, "schema = 1\n[shape]\nradius = 5\n");
    f.set_theme(Some("paper"), None).expect("paper");
    let p = f.resolve(Some("home-1")).expect("paper resolve");
    assert_eq!(p["theme"]["name"], "paper");
    assert_eq!(p["theme"]["tokens"]["colors"]["light"]["accent"], "#2f5d8a");
    assert_eq!(p["theme"]["tokens"]["shape"]["radius"], 5, "zen.toml applies on every theme");
    assert_eq!(p["theme"]["font"]["family"], "serif");
    assert_eq!(p["theme"]["tokens"]["colors"]["dark"]["canvas"], "#121316", "paper's unset keys come from default");

    // reset --theme default: snapshot, then the override is gone.
    f.set_theme(Some("default"), None).expect("back");
    let r = f.reset(&ov, None).expect("reset override");
    assert_eq!(r.restored, "builtin");
    let kept = r.snapshot.expect("the override is kept as a snapshot");
    assert!(std::fs::read_to_string(f.history_root().join("themes/default/theme.toml").join(&kept)).expect("snap").contains("#1d4ed8"));
    assert!(!f.path_of(&ov).exists(), "reset removes the override");
    let back = f.resolve(Some("home-1")).expect("back");
    assert_eq!(back["theme"]["tokens"]["colors"]["light"]["accent"], "#2563eb", "the built-in shows again");
    assert_eq!(back["theme"]["user"], false);
    // Undo the reset from history.
    f.reset(&ov, Some(&kept)).expect("undo");
    assert_eq!(f.resolve(Some("home-1")).expect("undo")["theme"]["tokens"]["colors"]["light"]["accent"], "#1d4ed8");

    // A user-only theme resets to a copy of default; reset never touches active.json.
    f.new_theme("mine", Some("midnight")).expect("mine");
    let active_before = std::fs::read(f.root().join(zen::store::ACTIVE_FILE)).expect("active.json");
    write(&f, &ZenFile::Theme("mine".into()), "schema = 1\n[shape]\ngap = 4\n");
    let r = f.reset(&ZenFile::Theme("mine".into()), None).expect("reset mine");
    assert_eq!(r.restored, "default");
    let t = std::fs::read_to_string(f.path_of(&ZenFile::Theme("mine".into()))).expect("mine");
    assert!(t.contains("started from K2's \"default\""), "{t}");
    assert_eq!(std::fs::read(f.root().join(zen::store::ACTIVE_FILE)).expect("active"), active_before);
}

#[test]
fn cycling_order_wraps_and_a_home_pick_beats_the_global_one() {
    let (_t, f) = set_up("cycle");
    f.new_theme("zeta", None).expect("zeta");
    f.new_theme("alpha", None).expect("alpha");
    let names: Vec<String> = f.themes().into_iter().map(|t| t.name).collect();
    assert_eq!(names, vec!["default", "paper", "midnight", "alpha", "zeta"], "built-ins first, then the user's by name");
    let mut seen = Vec::new();
    for _ in 0..5 {
        seen.push(f.cycle_theme(1, None).expect("next").theme);
    }
    assert_eq!(seen, vec!["paper", "midnight", "alpha", "zeta", "default"], "next walks the list and wraps");
    assert_eq!(f.cycle_theme(-1, None).expect("prev").theme, "zeta", "prev wraps backwards");
    assert_eq!(f.cycle_theme(-1, None).expect("prev").theme, "alpha");

    // Per Home: cycling for Work starts from what Work shows (the global).
    let w = f.cycle_theme(1, Some("Work")).expect("home next");
    assert_eq!((w.theme.as_str(), w.scope.as_str(), w.home.as_deref()), ("zeta", "home", Some("home-1")));
    assert_eq!(f.active_theme(None).name, "alpha", "the global pick is untouched");
    assert_eq!(f.resolve(Some("home-1")).expect("w")["theme"]["name"], "zeta");
    assert_eq!(f.resolve(Some("home-2")).expect("other")["theme"]["name"], "alpha", "other Homes follow the global");
    let cleared = f.set_theme(None, Some("home-1")).expect("clear");
    assert_eq!((cleared.theme.as_str(), cleared.scope.as_str()), ("alpha", "global"));

    // A deleted Home's pick goes with it.
    f.ensure_page("home-2", "Play").expect("home-2");
    f.set_theme(Some("paper"), Some("Play")).expect("pick");
    f.sync_homes(vec![HomeEntry { id: "home-1".into(), name: "Work".into() }]).expect("sync");
    assert!(f.read_active().homes.is_empty(), "{:?}", f.read_active());
}

#[test]
fn unknown_theme_names_are_refused_and_change_nothing() {
    let (_t, f) = set_up("unknown");
    match f.set_theme(Some("neon"), None) {
        Err(ZenError::UnknownTheme { name, known }) => {
            assert_eq!(name, "neon");
            assert_eq!(known, vec!["default", "paper", "midnight"]);
        }
        other => panic!("an unknown theme must be UnknownTheme, got {other:?}"),
    }
    let msg = f.set_theme(Some("neon"), None).expect_err("unknown").to_string();
    assert!(msg.contains("no theme 'neon'") && msg.contains("k2 zen theme new neon"), "{msg}");
    assert!(!f.root().join(zen::store::ACTIVE_FILE).exists(), "a refused switch writes nothing");
    assert!(matches!(f.set_theme(Some("paper"), Some("Nowhere")), Err(ZenError::NotFound(_))));
    assert!(matches!(f.reset(&ZenFile::Theme("neon".into()), None), Err(ZenError::UnknownTheme { .. })));
    assert!(matches!(f.new_theme("Bad Name", None), Err(ZenError::BadRequest(_))));
    assert!(matches!(f.new_theme("x", Some("neon")), Err(ZenError::UnknownTheme { .. })));
    f.new_theme("mine", None).expect("mine");
    assert!(matches!(f.new_theme("mine", None), Err(ZenError::Conflict(_))), "new never overwrites");
    for bad in ["themes/../grants.json", "themes/Mine/theme.toml", "themes/a/b/theme.toml", "active.json"] {
        assert!(ZenFile::parse(bad).is_err(), "{bad} must not be nameable");
    }
    assert_eq!(ZenFile::parse("themes/mine/theme.toml").expect("theme"), ZenFile::Theme("mine".into()));

    // The active theme's folder is deleted by hand: default, with a warning.
    f.set_theme(Some("mine"), None).expect("mine");
    std::fs::remove_dir_all(f.theme_dir("mine")).expect("rm mine");
    let g = f.resolve(Some("home-1")).expect("resolve");
    assert_eq!(g["theme"]["name"], "default", "{g}");
    let w = g["warnings"].as_array().expect("warnings");
    assert!(w.iter().any(|d| d["file"] == "active.json" && d["message"].as_str().is_some_and(|m| m.contains("theme 'mine' doesn't exist"))), "{g}");
    assert_eq!(g["errors"], serde_json::json!([]));
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
    assert_one_error(&c, 2, "template line belongs in pages/", "template in a theme");
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
