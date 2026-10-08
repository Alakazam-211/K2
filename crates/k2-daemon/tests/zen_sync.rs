//! Zen Garden sync with K2's defaults (prd-zen-garden-sync-defaults-v1
//! tests GS46–GS52).
//!
//! Drives `k2_core::zen` on temp folders (`ZenFiles::new(<temp>)` and
//! `ZenFiles::with_defaults(<temp>, <fake set>)`), plus the REAL
//! `k2-daemon` binary under a temp HOME (GS52) and the in-process
//! dispatcher for the passport refusal. Nothing here touches the real
//! `~/.k2/zen`; no test spawns an agent CLI.
//!
//! Fail loudly: every assertion names what it saw.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

use k2_core::zen::defaults::{canonical_json, Defaults, DefaultsSet};
use k2_core::zen::schema::{self, Layer};
use k2_core::zen::sync::{self, Mode, Part, PartState, Parts, View};
use k2_core::zen::{self as zen, ZenError, ZenFile, ZenFiles};
use serde_json::{json, Value as J};
use sha2::{Digest, Sha256};

// ── helpers ────────────────────────────────────────────────────────────

struct TempRoot(PathBuf);

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir(tag: &str) -> (TempRoot, PathBuf) {
    let dir = std::env::temp_dir().join(format!("k2-zen-sync-{tag}-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let zen = dir.join("zen");
    (TempRoot(dir), zen)
}

/// Zen set up the way 0.45.0 left it: Gardens 1 and 2, no sync state, no
/// archive, no news (the computer this release upgrades).
fn legacy_setup(f: &ZenFiles) {
    let out = f.setup().expect("setup");
    assert!(out.created_default, "first setup makes the first Gardens: {out:?}");
    for p in [f.sync_path(), f.news_path()] {
        std::fs::remove_file(&p).unwrap_or_else(|e| panic!("remove {}: {e}", p.display()));
    }
    std::fs::remove_dir_all(f.defaults_dir()).expect("remove .defaults");
    let mirrors = f.history_root().join("gardens");
    if mirrors.is_dir() {
        for e in std::fs::read_dir(&mirrors).expect("history/gardens").flatten() {
            let _ = std::fs::remove_file(e.path().join("sync.json"));
        }
    }
    f.forget_boot();
}

fn gid(f: &ZenFiles, i: usize) -> String {
    f.gardens().get(i).unwrap_or_else(|| panic!("no Garden at {i}: {:?}", f.gardens())).id.clone()
}

fn write_garden(f: &ZenFiles, id: &str, text: &[u8]) {
    let p = f.path_of(&ZenFile::Garden(id.to_string()));
    std::fs::write(&p, text).unwrap_or_else(|e| panic!("write {}: {e}", p.display()));
}

fn new_garden(f: &ZenFiles, name: &str, template: &str, text: Option<&str>) -> String {
    let g = f.new_garden(name, Some(template), None, None).unwrap_or_else(|e| panic!("new_garden {name}: {e}"));
    if let Some(t) = text {
        write_garden(f, &g.id, t.as_bytes());
    }
    g.id
}

/// The answer without what legitimately differs between two views of the
/// same look: the sync block, the version (it covers sync) and when the
/// files were last good.
fn look(mut v: J) -> J {
    assert!(v.is_object(), "resolve answer must be an object: {v}");
    let o = v.as_object_mut().expect("checked above");
    for k in ["sync", "version", "lastGoodAt"] {
        o.remove(k);
    }
    v
}

fn synced() -> PartState {
    PartState::synced()
}

fn copy_of(fp: &str) -> PartState {
    PartState { mode: Mode::Copy, defaults: Some(fp.to_string()), since: None, reason: None }
}

fn view(page: PartState, theme: PartState) -> View {
    View { preview: None, page: Some(page), theme: Some(theme), meta: false }
}

fn resolve_as(f: &ZenFiles, id: &str, v: &View) -> J {
    look(f.resolve_view(Some(id), v).unwrap_or_else(|e| panic!("resolve {id}: {e}")))
}

fn resolve_now(f: &ZenFiles, id: &str) -> J {
    look(f.resolve(Some(id)).unwrap_or_else(|e| panic!("resolve {id}: {e}")))
}

/// Every user-facing file in the folder (Garden pages, the list, the
/// active theme, theme bundles) with its bytes and mtime.
fn user_files(f: &ZenFiles) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut add = |p: PathBuf| {
        if p.is_file() {
            let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).unwrap_or_else(|e| panic!("mtime {}: {e}", p.display()));
            out.insert(p, (bytes, mtime));
        }
    };
    for e in std::fs::read_dir(f.gardens_dir()).expect("gardens dir").flatten() {
        add(e.path());
    }
    add(f.root().join("gardens.json"));
    add(f.root().join("active.json"));
    add(f.root().join("zen.toml"));
    if let Ok(rd) = std::fs::read_dir(f.themes_dir()) {
        for e in rd.flatten() {
            add(e.path().join("theme.toml"));
        }
    }
    out
}

/// Every file under the folder with bytes and mtime (the "writes nothing"
/// check).
fn all_files(dir: &Path) -> BTreeMap<PathBuf, (Vec<u8>, SystemTime)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("read_dir {}: {e}", d.display())).flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let bytes = std::fs::read(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
                let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).expect("mtime");
                out.insert(p, (bytes, mtime));
            }
        }
    }
    out
}

/// A fake set: this binary's, edited, with its fingerprint recomputed.
fn fake_set(edit: impl FnOnce(&mut DefaultsSet)) -> Defaults {
    let mut s = DefaultsSet::from_binary();
    edit(&mut s);
    s.fingerprint = s.compute_fingerprint().expect("fake set fingerprint");
    Defaults::from_set(s).unwrap_or_else(|e| panic!("fake set must load: {e}"))
}

fn replace_in(text: &mut String, from: &str, to: &str) {
    assert!(text.contains(from), "fixture edit: {from:?} not found");
    *text = text.replacen(from, to, 1);
}

/// "The next release": texting's columns move, `agents.preview` defaults
/// off, `basic`'s light accent changes, the frame's glass edge changes.
fn next_release() -> Defaults {
    fake_set(|s| {
        let t = s.templates.get_mut(zen::TEMPLATE_ID).expect("texting in the set");
        replace_in(t, "size = 34", "size = 30");
        replace_in(t, "size = 66", "size = 70");
        s.widget_props.insert("agents.preview".into(), json!(false));
        let basic = s.themes.get_mut("basic").expect("basic in the set");
        replace_in(basic, "accent = \"#2563eb\"", "accent = \"#1d4ed8\"");
        s.frame["glass"]["edge-border-mix"] = json!(60);
    })
}

/// A Diary catalog Garden for a fake set (the real one is B2's file).
fn diary_toml(version: u32) -> String {
    format!(
        "id = \"k2.diary@{version}\"\nschema = 1\n\n[catalog]\nshort = \"diary\"\nlabel = \"Diary\"\n\
         description = \"Write to one agent at a time; replies appear in handwriting.\"\norder = 10\n\n\
         [layout]\nkind = \"columns\"\n\n[[layout.column]]\nsize = 100\nmin-width = 360\n\n\
         [[widget]]\nid = \"agents\"\nkind = \"agents\"\ncolumn = 0\n\n\
         [[widget]]\nkind = \"garden-switcher\"\nslot = \"top\"\nalign = \"start\"\n\n\
         [[widget]]\nkind = \"zen-toggle\"\nslot = \"top\"\nalign = \"end\"\n"
    )
}

fn with_diary(s: &mut DefaultsSet, version: u32) {
    s.templates.insert(format!("k2.diary@{version}"), diary_toml(version));
}

fn short_hash(v: &J) -> String {
    let d = Sha256::digest(canonical_json(v).as_bytes());
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

const THEME_ONLY: &str = "schema = 1\ntemplate = \"k2.texting@1\"\n\n[colors.light]\naccent = \"#9a3412\"\n";
const LAYOUT_ONLY: &str = "schema = 1\ntemplate = \"k2.texting@1\"\n\n[layout]\nkind = \"columns\"\n\n\
[[layout.column]]\nsize = 45\nmin-width = 260\n\n[[layout.column]]\nsize = 55\nmin-width = 360\n";
/// Garden 2's shape: a blank Garden with its own agents and conversation.
const CONTENT_ONLY: &str = "schema = 1\ntemplate = \"k2.blank@1\"\n\n[layout]\nkind = \"columns\"\n\n\
[[layout.column]]\nsize = 40\nmin-width = 240\n\n[[layout.column]]\nsize = 60\nmin-width = 360\n\n\
[[widget]]\nid = \"agents\"\nkind = \"agents\"\ncolumn = 0\n\n[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 1\n";
const CHROME_ONLY: &str = "schema = 1\ntemplate = \"k2.texting@1\"\n\n\
[[widget]]\nkind = \"zen-toggle\"\nslot = \"top\"\nalign = \"start\"\n\n\
[[widget]]\nkind = \"garden-switcher\"\nslot = \"top\"\nalign = \"end\"\n";

/// Run the static `k2 zen guide <args>` with no daemon (as zen_core does).
fn guide(args: &[&str]) -> String {
    let cli = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/k2");
    let home = std::env::temp_dir().join(format!("k2-zen-sync-guide-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&home).expect("empty HOME");
    let out = Command::new("bash")
        .arg(&cli)
        .args(["zen", "guide"])
        .args(args)
        .env_clear()
        .env("HOME", &home)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .output()
        .expect("run cli/k2 zen guide");
    std::fs::remove_dir_all(&home).expect("cleanup");
    assert!(out.status.success(), "k2 zen guide {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).expect("utf-8")
}

// ── S0 / GS46: freeze guards ───────────────────────────────────────────

/// GS44/GS46: K2's built-in defaults are frozen through 0.45.1. Each
/// template, theme, the prop defaults and the frame hash to the pinned
/// value; a NEW catalog Garden is new data and may appear. To change a
/// default after the freeze (GS45): edit the data, update this table, add
/// one WHATS_NEW line ("Gardens that sync with K2's default get X").
const FROZEN: &[(&str, &str)] = &[
    ("frame", "5964f47ab2376b7f"),
    ("template:k2.blank@1", "3243f8b8edef13c3"),
    ("template:k2.texting@1", "c81d84543be1ef49"),
    ("theme:basic", "7fc5a1fb5062b263"),
    ("theme:midnight", "ff5f017a77ab1206"),
    ("theme:paper", "08fe35e01810ae29"),
    ("widget-props", "1f7a0792e682fa1e"),
];

fn frozen_hashes() -> BTreeMap<String, String> {
    let set = DefaultsSet::from_binary();
    let content = set.content().expect("built-in content parses");
    let mut out = BTreeMap::new();
    for (id, t) in content["templates"].as_object().expect("templates") {
        out.insert(format!("template:{id}"), short_hash(t));
    }
    for (name, t) in content["themes"].as_object().expect("themes") {
        out.insert(format!("theme:{name}"), short_hash(t));
    }
    out.insert("widget-props".into(), short_hash(&content["widgetProps"]));
    out.insert("frame".into(), short_hash(&content["frame"]));
    out
}

#[test]
fn gs46_builtin_defaults_are_frozen() {
    let now = frozen_hashes();
    let table: String = now.iter().map(|(k, v)| format!("    (\"{k}\", \"{v}\"),\n")).collect();
    for (k, want) in FROZEN {
        let got = now.get(*k).unwrap_or_else(|| panic!("frozen default {k} is gone from K2's defaults"));
        assert_eq!(
            got, want,
            "K2's default {k} changed during the freeze (prd-zen-garden-sync-defaults-v1 GS44). \
             If this is a deliberate default change after the freeze, follow GS45. Today's table:\n{table}"
        );
    }
    for k in now.keys() {
        let pinned = FROZEN.iter().any(|(f, _)| f == k);
        assert!(
            pinned || k.starts_with("template:k2.") && !k.contains("texting") && !k.contains("blank"),
            "{k} is a new default that isn't a catalog Garden; pin it. Today's table:\n{table}"
        );
    }
}

/// GS15/GS46: grammar is code, not data. These outputs are what a file
/// means; changing them is a schema change, not a default change.
const GRAMMAR_HASH: &str = "fca41bc99d299ee9";

const GRAMMAR_FIXTURE: &str = "schema = 1\ntemplate = \"k2.blank@1\"\n\n[layout]\nkind = \"columns\"\n\n\
[[layout.column]]\nsize = 40\nmin-width = 240\n\n[[layout.column]]\nsize = 60\nmin-width = 360\n\n\
[[widget]]\nid = \"agents\"\nkind = \"agents\"\ncolumn = 0\n\n\
[[widget]]\nid = \"talk\"\nkind = \"conversation\"\ncolumn = 1\n\n\
[[widget]]\nid = \"rail\"\nkind = \"nav-rail\"\nslot = \"top\"\nalign = \"center\"\n\n\
[[widget]]\nkind = \"garden-switcher\"\nslot = \"top\"\nalign = \"start\"\n\n\
[[widget]]\nkind = \"zen-toggle\"\nslot = \"top\"\nalign = \"end\"\n\n\
[[widget]]\nid = \"more\"\nkind = \"menu\"\nslot = \"bottom\"\nalign = \"end\"\n\n\
[[widget]]\nkind = \"theme-picker\"\nslot = \"menu\"\nmenu = \"more\"\n";

fn grammar_value() -> J {
    let rt = schema::resolve(&Layer::new(), &Layer::new());
    let fonts: Vec<J> = schema::FONT_FAMILIES
        .iter()
        .map(|(name, _, _)| schema::font_json(name, json!(14), json!(1.45)))
        .collect();
    let c = schema::check_garden("gardens/g-grammar.toml", GRAMMAR_FIXTURE, zen::builtin_layer(), zen::BLANK_TEMPLATE_ID);
    assert!(c.is_clean(), "grammar fixture must be clean: {:?}", c.errors);
    let page = zen::garden_page(&c.layer, zen::BLANK_TEMPLATE_ID);
    let widgets: Vec<J> = page["widgets"]
        .as_array()
        .expect("widgets")
        .iter()
        .map(|w| json!({ "id": w["id"], "kind": w["kind"], "slot": w["slot"], "agents": w["props"]["agents"], "orientation": w["props"]["orientation"] }))
        .collect();
    json!({
        "beziers": schema::BUILTIN_BEZIERS.iter().map(|(n, b)| json!([n, b])).collect::<Vec<_>>(),
        "animationTree": schema::ANIMATION_TREE.iter().map(|(n, p)| json!([n, p])).collect::<Vec<_>>(),
        "animationStyles": schema::ANIMATION_STYLES,
        "fallbacks": { "font": rt.font, "motion": rt.motion, "background": rt.background, "chrome": rt.chrome },
        "fonts": fonts,
        "placement": {
            "bands": page["bands"], "edges": page["edges"], "menus": page["menus"],
            "controls": page["controls"], "chromeFrom": page["chrome"]["from"], "widgets": widgets,
        },
    })
}

#[test]
fn gs46_grammar_is_frozen() {
    let got = short_hash(&grammar_value());
    assert_eq!(
        got, GRAMMAR_HASH,
        "Zen grammar changed (place_rows, link_conversations, normalize_slot_props, garden_controls, the resolve \
         fallbacks, BUILTIN_BEZIERS or the font stacks). That is a schema change, not a default change \
         (prd-zen-garden-sync-defaults-v1 GS15). Today's hash: {got}"
    );
}

#[test]
fn gs46_real_home_guard_panics_under_test() {
    let real = Path::new("/Users/someone");
    let hit = std::panic::catch_unwind(|| sync::guard_real_home_with(&real.join(".k2/zen"), true, Some(real)));
    assert!(hit.is_err(), "a test-mode ZenFiles on the real ~/.k2/zen must panic");
    let below = std::panic::catch_unwind(|| sync::guard_real_home_with(&real.join(".k2/zen/gardens"), true, Some(real)));
    assert!(below.is_err(), "anything under the real ~/.k2/zen must panic too");
    sync::guard_real_home_with(&std::env::temp_dir().join("x/.k2/zen"), true, Some(real));
    sync::guard_real_home_with(&real.join(".k2/zen"), false, Some(real));
    // This test binary itself never points at the real home.
    let (_t, z) = temp_dir("guard");
    sync::guard_real_home(&z);
}

// ── S1 / GS10–GS14, GS47: the archive equals live ──────────────────────

#[test]
fn gs10_gs11_fingerprint_ignores_comments_and_catalog_metadata() {
    let a = DefaultsSet::from_binary();
    let mut b = a.clone();
    let t = b.templates.get_mut(zen::TEMPLATE_ID).expect("texting");
    *t = format!("# a new comment\n{t}");
    assert_eq!(a.compute_fingerprint().expect("a"), b.compute_fingerprint().expect("b"), "a comment-only edit makes no new set");
    let mut c = a.clone();
    with_diary(&mut c, 1);
    let mut d = c.clone();
    let dt = d.templates.get_mut("k2.diary@1").expect("diary");
    *dt = dt.replace("label = \"Diary\"", "label = \"My Diary\"");
    assert_eq!(c.compute_fingerprint().expect("c"), d.compute_fingerprint().expect("d"), "[catalog] metadata isn't in the hash");
    assert_ne!(a.compute_fingerprint().expect("a"), c.compute_fingerprint().expect("c"), "a new catalog page is a new set");
    let fp = a.compute_fingerprint().expect("a");
    assert!(sync::valid_fingerprint(&fp), "{fp} is d- + 16 hex");
    assert_eq!(Defaults::live().fingerprint(), fp, "live() carries the binary's fingerprint");
}

#[test]
fn gs47_an_archived_copy_of_live_is_live() {
    let (_t, z) = temp_dir("archive");
    let f = ZenFiles::new(&z);
    f.setup().expect("setup");
    let live = Defaults::live();
    let path = f.defaults_dir().join(format!("{}.json", live.fingerprint()));
    assert!(path.is_file(), "setup archives the live set at {}", path.display());
    let text = std::fs::read_to_string(&path).expect("archive");
    let set: DefaultsSet = serde_json::from_str(&text).expect("archive parses");
    assert_eq!(set.k2_version, env!("CARGO_PKG_VERSION"));
    assert!(!set.archived_at.is_empty(), "archivedAt is stamped");
    let disk = Defaults::from_set(set).expect("the archive loads");
    for id in live.template_ids() {
        assert_eq!(zen::template_page_in(&disk, id), zen::template_page(id), "template {id} from the archive equals live");
    }
    for t in zen::BUILTIN_THEMES {
        assert_eq!(disk.theme_layer(t.name), live.theme_layer(t.name), "theme {} from the archive equals live", t.name);
    }
    assert_eq!(disk.frame(), live.frame());
    for p in schema::WIDGET_PROPS {
        assert_eq!(disk.widget_prop_default(p.kind, p.name), live.widget_prop_default(p.kind, p.name), "{}.{}", p.kind, p.name);
    }
}

/// GS47 + GS48: every fixture resolved as its own copy equals live, through
/// the archive on disk; when K2's defaults change, copies don't move,
/// synced Gardens do, and no user file is touched.
#[test]
fn gs47_gs48_copies_hold_their_look_and_synced_gardens_follow() {
    let (_t, z) = temp_dir("golden");
    let f = ZenFiles::new(&z);
    legacy_setup(&f);
    let g1 = gid(&f, 0);
    let g2 = gid(&f, 1);
    let mut fixtures: Vec<(String, String)> = vec![("texting untouched".into(), g1.clone()), ("blank untouched".into(), g2.clone())];
    for (name, tpl, text) in [
        ("theme only", "texting", THEME_ONLY),
        ("layout only", "texting", LAYOUT_ONLY),
        ("content only", "blank", CONTENT_ONLY),
        ("chrome only", "texting", CHROME_ONLY),
    ] {
        fixtures.push((name.into(), new_garden(&f, name, tpl, Some(text))));
    }
    for ex in guide(&["example", "--list"]).lines() {
        let src = guide(&["example", ex, "--toml"]);
        fixtures.push((format!("example {ex}"), new_garden(&f, &format!("ex {ex}"), "texting", Some(&src))));
    }
    // A broken file keeps its last good version.
    let broken = new_garden(&f, "broken", "texting", Some(THEME_ONLY));
    f.refresh().expect("refresh snapshots the clean file");
    write_garden(&f, &broken, b"schema = 1\ntemplate = \"k2.texting@1\"\n[colors.light]\nacent = \"#fff\"\n");
    fixtures.push(("broken (last good)".into(), broken.clone()));
    // A user theme over Paper.
    f.new_theme("hearth", Some("paper")).expect("theme new hearth from paper");
    let hearth = new_garden(&f, "hearth", "texting", None);
    f.set_theme(Some("hearth"), Some(&hearth)).expect("pick hearth");
    fixtures.push(("user theme over paper".into(), hearth.clone()));

    // Before any sync state: what 0.45.0 showed (live, synced).
    let before: BTreeMap<String, J> = fixtures.iter().map(|(_, id)| (id.clone(), resolve_as(&f, id, &view(synced(), synced())))).collect();
    // The upgrade pass, then every untouched Garden turned off too: all
    // are own copies of F.
    // The loader already ran (the first read after legacy setup: the
    // refresh above); every Garden made before it was decided by the pass.
    f.sync_boot();
    let fp = Defaults::live().fingerprint().to_string();
    for (name, id) in &fixtures {
        let st = f.sync_state().garden(id);
        if st.part(Part::Page).mode == Mode::Synced {
            assert!(name.contains("untouched") || name == "user theme over paper", "{name}: only untouched Gardens sync after the pass: {st:?}");
            f.set_garden_sync(id, Parts::Both, false).unwrap_or_else(|e| panic!("{name}: off: {e}"));
        }
        let st = f.sync_state().garden(id);
        for p in [Part::Page, Part::Theme] {
            assert_eq!(st.part(p).mode, Mode::Copy, "{name}: {p:?} is a copy");
            assert_eq!(st.part(p).defaults.as_deref(), Some(fp.as_str()), "{name}: copy of live F");
        }
        assert_eq!(resolve_now(&f, id), before[id], "GS47 {name}: copy of live equals live");
    }
    let files_before = user_files(&f);

    // The next release: live moves; F is now read from the archive.
    let next = next_release();
    let next_fp = next.fingerprint().to_string();
    let f = ZenFiles::with_defaults(&z, next);
    f.sync_boot();
    let sf = f.sync_state();
    assert_eq!(sf.live_defaults.as_deref(), Some(next_fp.as_str()), "live rotated");
    assert_eq!(sf.previous_defaults.as_deref(), Some(fp.as_str()), "previous is F");
    assert!(f.defaults_dir().join(format!("{next_fp}.json")).is_file(), "the new live set is archived");
    for (name, id) in &fixtures {
        assert_eq!(resolve_now(&f, id), before[id], "GS48 {name}: an own copy doesn't move");
        let moved = resolve_as(&f, id, &view(synced(), synced()));
        assert_ne!(moved, before[id], "GS48 {name}: synced, it shows the new defaults");
        assert_eq!(moved["frame"]["glass"]["edge-border-mix"], 60, "{name}: synced frame");
        assert_eq!(before[id]["frame"]["glass"]["edge-border-mix"], 55, "{name}: the copy's frame");
    }
    let texting_synced = resolve_as(&f, &g1, &view(synced(), synced()));
    assert_eq!(texting_synced["page"]["layout"]["split"], json!([30, 70]), "synced texting gets the new columns");
    assert_eq!(before[&g1]["page"]["layout"]["split"], json!([34, 66]));
    assert_eq!(texting_synced["theme"]["tokens"]["colors"]["light"]["accent"], "#1d4ed8", "synced basic accent");
    assert_eq!(before[&g1]["theme"]["tokens"]["colors"]["light"]["accent"], "#2563eb");
    let content = &fixtures.iter().find(|(n, _)| n == "content only").expect("content fixture").1;
    let content_synced = resolve_as(&f, content, &view(synced(), synced()));
    let agents = |v: &J| v["page"]["widgets"].as_array().expect("widgets").iter().find(|w| w["kind"] == "agents").cloned().expect("agents widget");
    assert_eq!(agents(&content_synced)["props"]["preview"], false, "synced: the new prop default");
    assert_eq!(agents(&before[content])["props"]["preview"], true, "copy: the old prop default");
    assert_eq!(user_files(&f), files_before, "GS48: no Garden file, list, active or theme file changed");
}

// ── S2 / GS35–GS39, GS49: the upgrade pass ─────────────────────────────

#[test]
fn gs49_upgrade_pass_states_writes_and_second_run() {
    let (_t, z) = temp_dir("upgrade");
    let f = ZenFiles::new(&z);
    legacy_setup(&f);
    let texting = gid(&f, 0);
    let blank = gid(&f, 1);
    f.rename_garden(&blank, "Renamed").expect("rename");
    let stub = |id: &str| zen::store::garden_stub(id, "x", zen::TEMPLATE_ID);
    let mut want: Vec<(String, String, bool)> = vec![
        ("texting".into(), texting.clone(), false),
        ("renamed blank".into(), blank.clone(), false),
    ];
    let crlf = new_garden(&f, "crlf", "texting", None);
    write_garden(&f, &crlf, stub(&crlf).replace('\n', "\r\n").as_bytes());
    want.push(("CRLF stub".into(), crlf, false));
    let comment = new_garden(&f, "comment", "texting", None);
    write_garden(&f, &comment, format!("# my own comment\n{}", stub(&comment)).as_bytes());
    want.push(("edited comment".into(), comment, false));
    let bom = new_garden(&f, "bom", "texting", None);
    let mut bom_bytes = vec![0xEF, 0xBB, 0xBF];
    bom_bytes.extend_from_slice(stub(&bom).as_bytes());
    write_garden(&f, &bom, &bom_bytes);
    let bom_parses = matches!(f.garden_own_keys(&bom), Ok(ref k) if k.is_empty());
    want.push(("BOM stub".into(), bom, !bom_parses));
    for (name, tpl, text) in [
        ("theme", "texting", THEME_ONLY),
        ("layout", "texting", LAYOUT_ONLY),
        ("content", "blank", CONTENT_ONLY),
        ("chrome", "texting", CHROME_ONLY),
    ] {
        want.push((format!("customized {name}"), new_garden(&f, name, tpl, Some(text)), true));
    }
    let latin1 = new_garden(&f, "latin1", "texting", None);
    write_garden(&f, &latin1, b"schema = 1\n# caf\xe9\n");
    want.push(("non-UTF-8".into(), latin1, true));
    let syntax = new_garden(&f, "syntax", "texting", None);
    write_garden(&f, &syntax, b"schema = 1\n[colors.light\n");
    want.push(("TOML syntax error".into(), syntax, true));
    let missing = new_garden(&f, "missing", "texting", None);
    std::fs::remove_file(f.path_of(&ZenFile::Garden(missing.clone()))).expect("remove page");
    want.push(("missing file".into(), missing, false));
    // Setup seeds the catalog's new-user Gardens (the Diary), untouched.
    let starts = [zen::TEMPLATE_ID, zen::BLANK_TEMPLATE_ID];
    for g in f.gardens().into_iter().filter(|g| !starts.contains(&g.template.as_str())) {
        if !want.iter().any(|(_, id, _)| *id == g.id) {
            want.push((format!("seeded {}", g.name), g.id, false));
        }
    }
    std::fs::write(f.gardens_dir().join("g-orphan1.toml"), THEME_ONLY).expect("orphan");
    want.push(("orphan with changes".into(), "g-orphan1".into(), true));
    std::fs::write(f.gardens_dir().join("g-orphan2.toml"), stub("g-orphan2")).expect("orphan stub");
    want.push(("orphan stub".into(), "g-orphan2".into(), false));

    let users = user_files(&f);
    let lines = f.sync_boot();
    let fp = Defaults::live().fingerprint().to_string();
    let sf = f.sync_state();
    assert!(!sf.migrated_at.is_empty(), "migratedAt is stamped");
    assert_eq!(sf.live_defaults.as_deref(), Some(fp.as_str()));
    for (name, id, copy) in &want {
        let e = sf.gardens.get(id).unwrap_or_else(|| panic!("{name}: no entry in sync.json: {sf:?}"));
        for p in [Part::Page, Part::Theme] {
            let st = e.part(p);
            assert_eq!(st.is_copy(), *copy, "{name} {p:?}: {st:?}");
            assert_eq!(st.reason.as_deref(), Some("upgrade"), "{name}");
            if *copy {
                assert_eq!(st.defaults.as_deref(), Some(fp.as_str()), "{name}: copy of F");
            }
        }
        let mirror = f.mirror_path(id);
        assert!(mirror.is_file(), "{name}: mirror {} written", mirror.display());
        assert_eq!(lines.iter().filter(|l| l.contains(id.as_str())).count(), 1, "{name}: one log line: {lines:?}");
    }
    assert_eq!(lines.len(), want.len(), "one log line per Garden: {lines:?}");
    assert_eq!(user_files(&f), users, "GS36: no Garden file, list, active or theme file changes");
    for (name, id, _) in &want {
        if f.find_garden(id).is_some() {
            assert_eq!(resolve_now(&f, id), resolve_as(&f, id, &view(synced(), synced())), "GS37 {name}: nothing moves");
        }
    }
    let news = f.read_news();
    assert!(news.catalog_seen.is_empty(), "a set-up computer starts with nothing seen");
    // A second run (a restart) writes nothing.
    let snap = all_files(&z);
    f.forget_boot();
    assert!(f.sync_boot().is_empty(), "the second run decides nothing");
    assert_eq!(all_files(&z), snap, "the second run writes nothing");
}

#[test]
fn gs49_upgrade_with_a_missing_or_unreadable_list() {
    for (tag, unreadable) in [("nolist", false), ("badlist", true)] {
        let (_t, z) = temp_dir(tag);
        let f = ZenFiles::new(&z);
        legacy_setup(&f);
        let a = gid(&f, 0);
        let b = gid(&f, 1);
        write_garden(&f, &b, CONTENT_ONLY.as_bytes());
        let list = f.root().join("gardens.json");
        if unreadable {
            std::fs::write(&list, "{ not json").expect("break list");
        } else {
            std::fs::remove_file(&list).expect("remove list");
        }
        let before = std::fs::read(&list).ok();
        let lines = f.sync_boot();
        let n = f.gardens().len();
        assert!(n >= 2, "{tag}: the list was rebuilt from the files");
        assert_eq!(lines.len(), n, "{tag}: every Garden decided: {lines:?}");
        let sf = f.sync_state();
        assert!(!sf.garden(&a).part(Part::Page).is_copy(), "{tag}: untouched syncs");
        assert!(sf.garden(&b).part(Part::Page).is_copy(), "{tag}: customized is a copy");
        assert_eq!(std::fs::read(&list).ok(), before, "{tag}: the pass never writes gardens.json");
    }
}

#[test]
fn gs38_fresh_setup_starts_synced_with_the_catalog_seen() {
    let (_t, z) = temp_dir("fresh");
    let live = fake_set(|s| with_diary(s, 1));
    let f = ZenFiles::with_defaults(&z, live);
    f.setup().expect("setup");
    let sf = f.sync_state();
    assert!(sf.gardens.is_empty(), "new Gardens get no entry (absent = synced): {sf:?}");
    assert_eq!(sf.live_defaults.as_deref(), Some(f.live_defaults().fingerprint()));
    let news = f.news().expect("news");
    assert_eq!(news["items"], json!([]), "a new person sees the catalog as the catalog, not as new: {news}");
    assert_eq!(f.read_news().catalog_seen.get("diary"), Some(&1));
    for g in f.gardens() {
        let v = f.resolve(Some(&g.id)).expect("resolve");
        assert_eq!(v["sync"]["page"]["mode"], "synced", "{v}");
        assert_eq!(v["sync"]["theme"]["mode"], "synced");
    }
}

// ── S2 / GS24–GS28, GS50: toggles, undo, delete, follow-K2, rebuild ────

#[test]
fn gs50_toggles_preview_undo_and_keep_previous() {
    let (_t, z) = temp_dir("toggles");
    let f = ZenFiles::new(&z);
    legacy_setup(&f);
    let g1 = gid(&f, 0);
    let g2 = gid(&f, 1);
    write_garden(&f, &g2, CONTENT_ONLY.as_bytes());
    f.sync_boot();
    let fp = Defaults::live().fingerprint().to_string();

    // Off: nothing on screen changes; the state before is the undo.
    let before = resolve_now(&f, &g1);
    let out = f.set_garden_sync(&g1, Parts::Page, false).expect("off");
    assert!(out.changed);
    assert_eq!(resolve_now(&f, &g1), before, "turning sync off changes nothing on screen");
    let st = f.sync_state().garden(&g1);
    assert_eq!(st.part(Part::Page), PartState { mode: Mode::Copy, defaults: Some(fp.clone()), since: st.part(Part::Page).since.clone(), reason: Some("turned-off".into()) });
    assert!(!st.part(Part::Theme).is_copy(), "only the page part turned off");
    assert_eq!(st.undo.as_ref().and_then(|u| u.page.clone()).map(|p| p.mode), Some(Mode::Synced));
    assert!(!f.set_garden_sync(&g1, Parts::Page, false).expect("off again").changed, "off twice changes nothing");
    let v = f.resolve(Some(&g1)).expect("get");
    assert_eq!(v["sync"]["page"]["mode"], "copy");
    assert_eq!(v["sync"]["page"]["newerDefault"], false, "a copy of live has no newer default");
    f.undo_garden_sync(&g1).expect("undo");
    assert!(!f.sync_state().garden(&g1).part(Part::Page).is_copy(), "undo restored synced");
    assert!(matches!(f.undo_garden_sync(&g1), Err(ZenError::BadRequest(_))), "the undo slot is used up");
    let mirror: J = serde_json::from_str(&std::fs::read_to_string(f.mirror_path(&g1)).expect("mirror")).expect("mirror json");
    assert_eq!(mirror["page"]["mode"], "synced", "the mirror follows: {mirror}");

    // The next release: Garden 2 (a copy) has a newer default; on = preview.
    let f = ZenFiles::with_defaults(&z, next_release());
    f.sync_boot();
    let g2_copy = resolve_now(&f, &g2);
    let v = f.resolve(Some(&g2)).expect("get g2");
    assert_eq!(v["sync"]["page"]["newerDefault"], true, "the copy has a newer default: {}", v["sync"]);
    assert_eq!(v["sync"]["theme"]["newerDefault"], true);
    let preview = f.resolve_view(Some(&g2), &View::preview(Parts::Page)).expect("preview");
    assert_eq!(preview["sync"]["preview"], "page");
    let preview = look(preview);
    assert_ne!(preview, g2_copy, "the preview shows K2's default");
    assert!(f.sync_state().garden(&g2).part(Part::Page).is_copy(), "preview writes nothing");
    f.set_garden_sync(&g2, Parts::Page, true).expect("on");
    assert_eq!(resolve_now(&f, &g2), preview, "on equals the preview");
    f.undo_garden_sync(&g2).expect("undo on");
    assert_eq!(resolve_now(&f, &g2), g2_copy, "undo puts the copy back");

    // Keep my previous look: Garden 1 synced, its look moved.
    let offered = f.keep_previous_offered(&g1).expect("offered");
    assert_eq!(offered, vec![Part::Page, Part::Theme], "both parts of Garden 1 moved");
    assert!(f.keep_previous(&g2, Parts::Both).is_err(), "not offered on a copy");
    let g1_new = resolve_now(&f, &g1);
    f.keep_previous(&g1, Parts::Both).expect("keep previous");
    let st = f.sync_state().garden(&g1);
    assert_eq!(st.part(Part::Page).defaults.as_deref(), Some(fp.as_str()), "a copy of previousDefaults");
    assert_eq!(st.part(Part::Theme).reason.as_deref(), Some("kept-previous"));
    assert_eq!(resolve_now(&f, &g1), before, "the previous look is back");
    assert_ne!(resolve_now(&f, &g1), g1_new);
}

#[test]
fn gs27_gs28_delete_restore_template_and_reset() {
    let (_t, z) = temp_dir("lifecycle");
    let f = ZenFiles::new(&z);
    legacy_setup(&f);
    let g1 = gid(&f, 0);
    let g2 = gid(&f, 1);
    write_garden(&f, &g2, CONTENT_ONLY.as_bytes());
    f.sync_boot();
    assert!(f.sync_state().garden(&g2).part(Part::Page).is_copy());

    // garden/template (Start with the default) follows K2, with undo.
    f.set_garden_template(&g2, "texting", true).expect("template");
    let st = f.sync_state().garden(&g2);
    assert!(!st.part(Part::Page).is_copy(), "Start with the default sets page sync on");
    assert!(st.part(Part::Theme).is_copy(), "the theme part is left alone");
    assert!(st.undo.as_ref().and_then(|u| u.page.as_ref()).is_some_and(|p| p.is_copy()), "undo holds the copy");

    // reset --to <snapshot> leaves the state; reset to the stub follows K2.
    f.set_garden_sync(&g2, Parts::Page, false).expect("off");
    write_garden(&f, &g2, CONTENT_ONLY.as_bytes());
    f.refresh().expect("refresh");
    let snap = f.snapshots(&ZenFile::Garden(g2.clone())).into_iter().next().expect("a snapshot").name;
    f.reset(&ZenFile::Garden(g2.clone()), Some(&snap)).expect("reset --to");
    assert!(f.sync_state().garden(&g2).part(Part::Page).is_copy(), "reset --to leaves sync alone");
    f.reset(&ZenFile::Garden(g2.clone()), None).expect("reset");
    assert!(!f.sync_state().garden(&g2).part(Part::Page).is_copy(), "reset to the stub follows K2");

    // Delete carries the state; a hand restore brings it back.
    f.set_garden_sync(&g2, Parts::Both, false).expect("off both");
    let state = f.sync_state().garden(&g2);
    let out = f.delete_garden(&g2).expect("delete");
    assert!(!f.sync_state().gardens.contains_key(&g2), "the entry leaves sync.json");
    let hist = f.history_root().join("gardens").join(format!("{g2}.toml"));
    let rec: J = serde_json::from_str(&std::fs::read_to_string(hist.join("deleted.json")).expect("deleted.json")).expect("json");
    assert_eq!(rec["sync"]["page"]["mode"], "copy", "deleted.json carries the state: {rec}");
    let snap = out.snapshot.expect("the page moved to history");
    std::fs::copy(hist.join(&snap), f.path_of(&ZenFile::Garden(g2.clone()))).expect("hand restore");
    std::fs::remove_file(f.root().join("gardens.json")).expect("rebuild the list");
    assert!(f.find_garden(&g2).is_some(), "the restored Garden is listed");
    let back = f.sync_state().garden(&g2);
    assert_eq!(back.part(Part::Page), state.part(Part::Page), "restored with its state");
    assert_eq!(back.part(Part::Theme), state.part(Part::Theme));
    assert!(f.find_garden(&g1).is_some());
}

#[test]
fn gs18_gs14_rebuild_damaged_sets_and_the_floor() {
    let (_t, z) = temp_dir("rebuild");
    let f = ZenFiles::new(&z);
    legacy_setup(&f);
    let g1 = gid(&f, 0);
    let g2 = gid(&f, 1);
    write_garden(&f, &g2, CONTENT_ONLY.as_bytes());
    f.sync_boot();
    f.set_garden_sync(&g1, Parts::Theme, false).expect("off");
    let want = f.sync_state();

    // A lost sync.json is rebuilt from the mirrors.
    std::fs::remove_file(f.sync_path()).expect("lose sync.json");
    f.forget_boot();
    f.sync_boot();
    let got = f.sync_state();
    for id in [&g1, &g2] {
        assert_eq!(got.garden(id).page, want.garden(id).page, "{id}: page rebuilt");
        assert_eq!(got.garden(id).theme, want.garden(id).theme, "{id}: theme rebuilt");
    }
    assert_eq!(got.rebuilt.as_deref(), Some("mirrors"));
    let doctor = f.doctor_checks();
    let sync_line = doctor.iter().find(|c| c["name"] == "sync state").expect("sync state line");
    assert_eq!(sync_line["ok"], false, "the doctor says it was rebuilt: {sync_line}");

    // A damaged live set is rewritten from the binary at the next boot; a
    // damaged older set resolves live with a warning.
    let live_path = f.defaults_dir().join(format!("{}.json", Defaults::live().fingerprint()));
    std::fs::write(&live_path, "{ damaged").expect("damage live");
    f.forget_boot();
    f.sync_boot();
    let set: DefaultsSet = serde_json::from_str(&std::fs::read_to_string(&live_path).expect("live set")).expect("rewritten");
    assert_eq!(set.fingerprint, Defaults::live().fingerprint(), "the damaged live set was rewritten");
    // Point Garden 2 at a damaged non-live set.
    let bad = "d-0123456789abcdef";
    std::fs::write(f.defaults_dir().join(format!("{bad}.json")), "{}").expect("bad set");
    let mut sf: J = serde_json::from_str(&std::fs::read_to_string(f.sync_path()).expect("sync.json")).expect("json");
    sf["gardens"][&g2]["page"]["defaults"] = json!(bad);
    std::fs::write(f.sync_path(), serde_json::to_string_pretty(&sf).expect("ser")).expect("write");
    let v = f.resolve(Some(&g2)).expect("resolve on a damaged set");
    let warned = v["warnings"].as_array().expect("warnings").iter().any(|w| w["file"] == format!(".defaults/{bad}.json"));
    assert!(warned, "a damaged set warns: {}", v["warnings"]);
    assert_eq!(look(v.clone())["page"], resolve_as(&f, &g2, &view(synced(), copy_of(Defaults::live().fingerprint())))["page"], "it resolves live");
    let doctor = f.doctor_checks();
    let arch = doctor.iter().find(|c| c["name"] == "defaults archive").expect("archive line");
    assert_eq!(arch["ok"], false, "{arch}");
    assert!(arch["detail"].as_str().expect("detail").contains(bad), "{arch}");

    // GF3: a set that asks for a looser control check is clamped.
    let (_t2, z2) = temp_dir("floor");
    let loose = fake_set(|s| {
        s.frame["controls"]["min-opacity"] = json!(0.1);
        s.frame["controls"]["target-px"] = json!(30);
    });
    let f2 = ZenFiles::with_defaults(&z2, loose);
    f2.setup().expect("setup");
    let v = f2.resolve(None).expect("resolve");
    assert_eq!(v["frame"]["controls"]["min-opacity"], json!(0.3), "clamped up to the floor: {}", v["frame"]);
    assert_eq!(v["frame"]["controls"]["target-px"], 30, "stricter stays");
    let floor = f2.doctor_checks().into_iter().find(|c| c["name"] == "frame floor").expect("frame floor line");
    assert_eq!(floor["ok"], false, "{floor}");
}

// ── S3 / GS51: news ────────────────────────────────────────────────────

#[test]
fn gs51_news_catalog_updates_and_seen() {
    let (_t, z) = temp_dir("news");
    // A computer set up before this release, on a K2 whose catalog has
    // the Diary.
    let f = ZenFiles::new(&z);
    legacy_setup(&f);
    let g1 = gid(&f, 0);
    let g2 = gid(&f, 1);
    write_garden(&f, &g2, CONTENT_ONLY.as_bytes());
    let f = ZenFiles::with_defaults(&z, fake_set(|s| with_diary(s, 1)));
    let news = f.news().expect("news");
    let ids: Vec<&str> = news["items"].as_array().expect("items").iter().filter_map(|i| i["id"].as_str()).collect();
    assert_eq!(ids, vec!["catalog:diary"], "the first run lists the Diary: {news}");
    assert_eq!(news["items"][0]["label"], "Diary");
    assert_eq!(news["items"][0]["template"], "k2.diary@1");
    assert_eq!(news["copiesWithNewerDefault"], 0, "{news}");

    // The next release: update items only for synced Gardens that moved.
    let f = ZenFiles::with_defaults(&z, fake_set(|s| {
        with_diary(s, 1);
        let t = s.templates.get_mut(zen::TEMPLATE_ID).expect("texting");
        replace_in(t, "size = 34", "size = 30");
        replace_in(t, "size = 66", "size = 70");
    }));
    let news = f.news().expect("news after the release");
    let items = news["items"].as_array().expect("items");
    let update = items.iter().find(|i| i["kind"] == "update").unwrap_or_else(|| panic!("an update item: {news}"));
    assert_eq!(update["garden"], g1.as_str(), "Garden 1 (synced texting) moved");
    assert_eq!(update["part"], "page");
    assert_eq!(update["keepPrevious"], true);
    assert!(!items.iter().any(|i| i["garden"] == g2.as_str()), "Garden 2 is a copy: no update item");
    assert_eq!(news["copiesWithNewerDefault"], 0, "Garden 2 doesn't lean on texting: {news}");
    assert_eq!(items[0]["kind"], "update", "updates first, then the catalog");

    // Seen hides them; creating from the catalog marks it seen.
    let uid = update["id"].as_str().expect("id").to_string();
    f.news_seen(&[uid], false).expect("seen");
    let left: Vec<String> = f.news().expect("news")["items"].as_array().expect("items").iter().filter_map(|i| i["id"].as_str().map(str::to_string)).collect();
    assert_eq!(left, vec!["catalog:diary".to_string()]);
    f.news_mark_template_seen("k2.diary@1").expect("made a Diary");
    assert_eq!(f.news().expect("news")["items"], json!([]), "creating from the catalog marks it seen");
    assert!(f.news_seen(&["nonsense".into()], false).is_err(), "unknown id shapes are refused");
    assert!(f.news_seen(&[], false).is_err(), "ids or all");

    // A newer Diary version isn't news by itself.
    let f = ZenFiles::with_defaults(&z, fake_set(|s| {
        with_diary(s, 1);
        with_diary(s, 2);
    }));
    let news = f.news().expect("news");
    assert!(!news["items"].as_array().expect("items").iter().any(|i| i["kind"] == "catalog"), "diary@2 isn't new: {news}");
}

// ── GS52: headless, the real binary under a temp HOME ──────────────────

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Daemon {
    child: std::process::Child,
    port: u16,
    owner: String,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    let t = std::fs::read_to_string(path).ok()?.trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn spawn_daemon(home: &Path) -> Daemon {
    let port_file = home.join(".k2").join("daemon.port");
    let child = Command::new(env!("CARGO_BIN_EXE_k2-daemon"))
        .env("HOME", home)
        .env("K2_TEST_AGENT_SHIM_DIR", home.join("agent-shim-empty"))
        .env("K2SO_WATCHDOG_DISABLED", "1")
        .env("K2_HEARTBEAT_NO_SELF_HEAL", "1")
        .env("K2_SUBSCRIPTION_PROBE", "deny")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn k2-daemon");
    let token_file = home.join(".k2").join("daemon.token");
    let deadline = Instant::now() + Duration::from_secs(20);
    let (port, owner) = loop {
        let port = read_trimmed(&port_file).and_then(|p| p.parse::<u16>().ok());
        if let (Some(port), Some(owner)) = (port, read_trimmed(&token_file)) {
            break (port, owner);
        }
        assert!(Instant::now() < deadline, "daemon never published daemon.port + daemon.token");
        std::thread::sleep(Duration::from_millis(50));
    };
    let d = Daemon { child, port, owner };
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(mut c) = Conn::try_open(d.port) {
            let (s, b) = c.request("GET", "/boot-status", None);
            if s == 200 && json(&b, "boot-status")["phase"] == "ready" {
                break;
            }
        }
        assert!(Instant::now() < deadline, "daemon never reached ready");
        std::thread::sleep(Duration::from_millis(100));
    }
    d
}

struct Conn {
    reader: BufReader<TcpStream>,
}

impl Conn {
    fn try_open(port: u16) -> std::io::Result<Conn> {
        let s = TcpStream::connect(("127.0.0.1", port))?;
        s.set_read_timeout(Some(Duration::from_secs(15)))?;
        Ok(Conn { reader: BufReader::new(s) })
    }

    fn request(&mut self, method: &str, path: &str, body: Option<&str>) -> (u16, String) {
        let body = body.unwrap_or("");
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        );
        self.reader.get_mut().write_all(head.as_bytes()).expect("write request");
        let mut status_line = String::new();
        self.reader.read_line(&mut status_line).expect("status line");
        let status: u16 = status_line.split_whitespace().nth(1).and_then(|s| s.parse().ok()).unwrap_or_else(|| panic!("{path}: {status_line:?}"));
        let mut len = None;
        loop {
            let mut line = String::new();
            self.reader.read_line(&mut line).expect("header");
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                if k.eq_ignore_ascii_case("content-length") {
                    len = Some(v.trim().parse::<usize>().expect("content-length"));
                }
            }
        }
        let mut buf = vec![0u8; len.unwrap_or_else(|| panic!("{path}: no Content-Length"))];
        self.reader.read_exact(&mut buf).expect("body");
        (status, String::from_utf8(buf).expect("utf-8"))
    }
}

fn json(body: &str, what: &str) -> J {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("{what}: not JSON ({e}): {body}"))
}

fn call(port: u16, method: &str, path: &str, body: Option<&str>) -> (u16, J) {
    let (s, b) = Conn::try_open(port).expect("connect").request(method, path, body);
    (s, json(&b, path))
}

#[test]
fn gs52_headless_sync_and_news_routes() {
    let home = Home(std::env::temp_dir().join(format!("k2-zen-sync-headless-{}-{}", std::process::id(), uuid::Uuid::new_v4())));
    std::fs::create_dir_all(home.0.join(".k2")).expect("HOME/.k2");
    std::fs::create_dir_all(home.0.join("agent-shim-empty")).expect("shim");
    let d = spawn_daemon(&home.0);
    let (port, tok) = (d.port, d.owner.clone());

    let (s, b) = call(port, "GET", "/boot-status", None);
    assert_eq!(s, 200);
    assert!(b["features"].as_array().expect("features").iter().any(|x| x == "zen-sync-v1"), "zen-sync-v1: {b}");

    // Before setup: 404 zen_not_set_up, never a crash.
    let (s, v) = call(port, "GET", &format!("/cli/zen/sync?token={tok}"), None);
    assert_eq!((s, v["error"].as_str()), (404, Some("zen_not_set_up")), "{v}");
    let (s, v) = call(port, "POST", &format!("/cli/zen/setup?token={tok}"), Some("{}"));
    assert_eq!(s, 200, "setup: {v}");
    let g1 = v["gardens"][0]["id"].as_str().expect("Garden 1").to_string();
    let zen_dir = home.0.join(".k2").join("zen");
    assert!(zen_dir.join("sync.json").is_file() && zen_dir.join("news.json").is_file(), "setup writes the sync state");
    assert_eq!(std::fs::read_dir(zen_dir.join(".defaults")).expect(".defaults").count(), 1, "the live set is archived");

    let (s, v) = call(port, "GET", &format!("/cli/zen/sync?token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    let setup_gardens = std::fs::read_dir(zen_dir.join("gardens")).expect("gardens").count();
    assert_eq!(v["gardens"].as_array().expect("rows").len(), setup_gardens, "one row per Garden setup made");
    assert_eq!(v["gardens"][0]["page"]["mode"], "synced");
    assert_eq!(v["gardens"][0]["themeBase"]["label"], "Basic");
    let (s, v) = call(port, "GET", &format!("/cli/zen/news?token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["items"], json!([]), "a fresh computer has no news: {v}");
    let (s, v) = call(port, "GET", &format!("/cli/zen/get?garden={g1}&preview=page&token={tok}"), None);
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["sync"]["preview"], "page");
    assert!(v["frame"]["glass"].is_object(), "frame in the answer: {v}");
    let (s, v) = call(port, "GET", &format!("/cli/zen/get?garden={g1}&preview=sideways&token={tok}"), None);
    assert_eq!(s, 400, "a bad preview is refused: {v}");

    // GET on the POST rows → 405.
    for p in ["/cli/zen/garden/sync", "/cli/zen/news/seen"] {
        let (s, b) = Conn::try_open(port).expect("connect").request("GET", &format!("{p}?token={tok}"), None);
        assert_eq!(s, 405, "GET {p}: {b}");
    }
    // Off, then undo, over HTTP.
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/sync?token={tok}"), Some(&format!(r#"{{"garden":"{g1}","part":"both","sync":false}}"#)));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["garden"]["page"]["mode"], "copy", "{v}");
    assert_eq!(v["garden"]["undo"]["parts"], json!(["page", "theme"]));
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/sync?token={tok}"), Some(&format!(r#"{{"garden":"{g1}","undo":true}}"#)));
    assert_eq!(s, 200, "{v}");
    assert_eq!(v["garden"]["page"]["mode"], "synced");
    let (s, v) = call(port, "POST", &format!("/cli/zen/garden/sync?token={tok}"), Some(&format!(r#"{{"garden":"{g1}","sync":true}}"#)));
    assert_eq!(s, 400, "sync needs a part: {v}");
    let (s, v) = call(port, "POST", &format!("/cli/zen/news/seen?token={tok}"), Some(r#"{"all":true}"#));
    assert_eq!(s, 200, "{v}");

    // A Connect login (any role) is refused.
    let (s, v) = call(port, "POST", &format!("/cli/users/add?token={tok}"), Some(r#"{"username":"zsync","password":"correct-horse-battery-9"}"#));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(port, "POST", &format!("/cli/users/set-role?token={tok}"), Some(r#"{"username":"zsync","role":"owner"}"#));
    assert_eq!(s, 200, "{v}");
    let (s, v) = call(port, "POST", "/cli/auth/login", Some(r#"{"username":"zsync","password":"correct-horse-battery-9"}"#));
    assert_eq!(s, 200, "{v}");
    let login = v["token"].as_str().expect("login token").to_string();
    for (m, p, body) in [
        ("GET", "/cli/zen/sync".to_string(), None),
        ("GET", "/cli/zen/news".to_string(), None),
        ("GET", format!("/cli/zen/get?garden={g1}&preview=page"), None),
        ("POST", "/cli/zen/garden/sync".to_string(), Some(format!(r#"{{"garden":"{g1}","part":"page","sync":false}}"#))),
        ("POST", "/cli/zen/news/seen".to_string(), Some(r#"{"all":true}"#.to_string())),
    ] {
        let sep = if p.contains('?') { '&' } else { '?' };
        let (s, v) = call(port, m, &format!("{p}{sep}token={login}"), body.as_deref());
        assert_eq!((s, v["error"].as_str()), (403, Some("zen_local_only")), "Connect owner {m} {p}: {v}");
    }
    assert_eq!(call(port, "GET", &format!("/cli/zen/sync?token={tok}"), None).1["gardens"][0]["page"]["mode"], "synced", "refused calls changed nothing");
}

/// GS32/GS52: an agent passport is refused on every sync and news route
/// (in-process dispatcher, where passports are minted).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn gs52_agent_passport_is_refused() {
    use k2_core::session::SessionId;
    use k2_daemon::session_token::{self, CredMode, HookPrincipal, Provider};
    let prev = std::env::var_os("HOME");
    let tmp = std::env::temp_dir().join(format!("k2-zen-sync-passport-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp).expect("temp HOME");
    std::env::set_var("HOME", &tmp);
    let _ = k2_core::db::init_for_tests();
    let d = tokio::task::block_in_place(|| tokio::runtime::Handle::current().block_on(k2_daemon::test_harness::start("owner-token-zen-sync")));
    let passport = session_token::mint_session_token(
        &SessionId::new(),
        "pane-zen-sync",
        HookPrincipal { workspace_uuid: "ws-zen-sync".into(), agent_address: "agent-zen-sync".into() },
        CredMode::ApiKey,
        Provider::Anthropic,
    );
    assert!(session_token::validate_hook(&passport).is_some(), "passport validates");
    for (m, p, body) in [
        ("GET", "/cli/zen/sync", None),
        ("GET", "/cli/zen/news", None),
        ("GET", "/cli/zen/get?preview=page", None),
        ("POST", "/cli/zen/garden/sync", Some(r#"{"garden":"g-x","part":"page","sync":true}"#)),
        ("POST", "/cli/zen/news/seen", Some(r#"{"all":true}"#)),
    ] {
        let sep = if p.contains('?') { '&' } else { '?' };
        let (s, v) = call(d.port, m, &format!("{p}{sep}token={passport}"), body);
        assert_eq!((s, v["error"].as_str()), (403, Some("zen_local_only")), "passport {m} {p}: {v}");
    }
    assert!(!tmp.join(".k2").join("zen").exists(), "refused calls wrote nothing");
    match prev {
        Some(p) => std::env::set_var("HOME", p),
        None => std::env::remove_var("HOME"),
    }
    std::fs::remove_dir_all(&tmp).expect("remove temp HOME");
}
