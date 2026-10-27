//! Tests for S0 (prd-widget-size-and-shared-libs-v1 vs-live L1): the
//! manifest is part of a widget's version, frozen copies are re-checked on
//! read, and the one-time carry moves old signatures to the new hashes.
//! Every Zen folder is under the temp dir; the database is in memory.

use std::path::{Path, PathBuf};

use rusqlite::params;

use super::*;
use crate::garden_data::authors::{CallerKind, StampSource};
use crate::garden_data::signer_gate;
use crate::zen::bundle::Bundle;
use crate::zen::store::ZenFile;
use crate::zen::widget_store::{read_saved_bundle, LAST_GOOD_HTML, LAST_GOOD_JSON, PRE_CARRY_JSON};
use crate::zen::garden_access::{self as ga, Actor, Change, Via};

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set_up(tag: &str) -> (Tmp, ZenFiles) {
    let d = std::env::temp_dir().join(format!("k2-zen-carry-{tag}-{}", uuid::Uuid::new_v4().simple()));
    let f = ZenFiles::new(d.join(".k2").join("zen"));
    assert!(f.root().starts_with(std::env::temp_dir()), "tests never touch the real Zen folder");
    f.setup().expect("setup");
    (Tmp(d), f)
}

fn db() -> Connection {
    let conn = Connection::open_in_memory().expect("memory db");
    crate::db::run_migrations(&conn).expect("migrations");
    for id in ["w-maker", "w-other"] {
        conn.execute("INSERT INTO projects (id, name, path) VALUES (?1, ?1, ?2)", params![id, format!("/w/{id}")])
            .expect("project row");
    }
    conn
}

fn gid(f: &ZenFiles, name: &str) -> String {
    f.find_garden(name).unwrap_or_else(|| panic!("no Garden {name}")).1.id
}

fn place(f: &ZenFiles, garden: &str, widget: &str) {
    let page = format!(
        "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"card\"\nkind = \"custom\"\nwidget = \"{widget}\"\ncolumn = 0\n"
    );
    fs::write(f.path_of(&ZenFile::Garden(garden.into())), page).expect("write garden");
    f.refresh().expect("refresh");
}

fn manifest_path(f: &ZenFiles, w: &str) -> PathBuf {
    f.widget_dir(w).join(widgets::MANIFEST_FILE)
}

fn edit_manifest(f: &ZenFiles, w: &str, edit: impl Fn(&mut J)) {
    let p = manifest_path(f, w);
    let mut v: J = serde_json::from_str(&fs::read_to_string(&p).expect("read manifest")).expect("manifest json");
    edit(&mut v);
    fs::write(&p, serde_json::to_string_pretty(&v).expect("json")).expect("write manifest");
}

fn live(f: &ZenFiles, w: &str) -> (Manifest, Bundle) {
    let st = f.widget_status(w);
    assert_eq!(st.state, WidgetState::Ok, "{w}: {:?}", st.errors);
    (st.manifest.expect("manifest"), st.bundle.expect("bundle"))
}

/// The shape K2 0.46.4 wrote: `hash` = the document's sha256, no scheme.
fn write_legacy(dir: &Path, m: &Manifest, b: &Bundle) {
    fs::create_dir_all(dir).expect("mkdir");
    fs::write(dir.join(LAST_GOOD_HTML), b.template()).expect("html");
    let meta = json!({
        "hash": b.template_hash(),
        "at": "2026-10-20T08:00:00.000Z",
        "nonceAt": b.nonce_offsets(),
        "codeBytes": b.code_bytes,
        "manifest": widgets::manifest_json(m),
    });
    fs::write(dir.join(LAST_GOOD_JSON), serde_json::to_string_pretty(&meta).expect("json")).expect("meta");
}

fn agent(p: &str) -> MadeBy {
    MadeBy::Agent { project_id: p.into() }
}

/// L1: a manifest-only edit on disk is a new version. The old stamp no
/// longer covers it, the gate keeps Always only, and the hash (the frame's
/// key) changes, so the frame reloads. A wording edit keeps the version.
#[test]
fn a_manifest_only_disk_edit_is_a_new_unsigned_version() {
    let (_t, f) = set_up("disk");
    let conn = db();
    f.new_widget("board", Some("hello")).expect("new");
    let g = gid(&f, "Garden 1");
    place(&f, &g, "board");
    ga::add(&conn, &g, "w-maker", &Change::new(Via::Settings, &Actor::Owner, 1)).expect("list");
    let (_, b0) = live(&f, "board");
    authors::stamp(&conn, "board", &b0.hash, &agent("w-maker"), StampSource::Sign, CallerKind::Agent, 2).expect("stamp");
    assert!(signer_gate::passes(&conn, &g, "board", &b0.hash).expect("gate"), "the signed version passes");

    // Wording only: same version, still signed.
    edit_manifest(&f, "board", |v| v["description"] = json!("Says hello, in other words."));
    let (_, b1) = live(&f, "board");
    assert_eq!(b1.hash, b0.hash, "a description edit is not a new version");

    // The "why" next to a cap is what the person decides on (review
    // finding 7): rewriting it is a new, unsigned version.
    edit_manifest(&f, "board", |v| v["reasons"] = json!({"agents:read": "Only to show who is around."}));
    let (mr, br) = live(&f, "board");
    assert_eq!(mr.reasons.get("agents:read").map(String::as_str), Some("Only to show who is around."));
    assert_eq!(br.template(), b0.template(), "the code didn't change");
    assert_ne!(br.hash, b0.hash, "reasons are part of the version");
    assert!(!signer_gate::passes(&conn, &g, "board", &br.hash).expect("gate"), "rewritten reasons are unsigned");
    edit_manifest(&f, "board", |v| v["reasons"] = json!({"agents:read": "to say hello to the first agent in this Home"}));
    assert_eq!(live(&f, "board").1.hash, b0.hash, "the starter's own reason back: the signed version again");

    // A cap added by hand: same document, new version, unsigned.
    edit_manifest(&f, "board", |v| v["caps"] = json!(["agents:read", "thread:post"]));
    let (m2, b2) = live(&f, "board");
    assert_eq!(m2.caps, vec!["agents:read".to_string(), "thread:post".to_string()]);
    assert_eq!(b2.template(), b0.template(), "the code didn't change");
    assert_ne!(b2.hash, b0.hash, "the manifest is part of the version");
    assert!(!signer_gate::passes(&conn, &g, "board", &b2.hash).expect("gate"), "a raw manifest edit fails the gate");
    let caps: Vec<String> = m2.caps.clone();
    assert_eq!(signer_gate::always_only(&caps), vec!["agents:read".to_string()], "unsigned: Always only");

    // A library (a CDN file) added by hand: also a new, unsigned version.
    let sri = format!("sha384-{}", "A".repeat(64));
    edit_manifest(&f, "board", |v| {
        v["caps"] = json!(["agents:read"]);
        v["requires"] = json!({"libs": [{"url": "https://cdn.jsdelivr.net/npm/left-pad@1.3.0/index.js", "integrity": sri}]});
    });
    let (m3, b3) = live(&f, "board");
    assert_eq!(m3.libs.len(), 1);
    assert_eq!(b3.template(), b0.template());
    assert_ne!(b3.hash, b0.hash, "a CDN file is part of the version");
    assert!(!signer_gate::passes(&conn, &g, "board", &b3.hash).expect("gate"), "a CDN file added on disk is unsigned");
    // Another integrity for the same URL: another version again.
    let sri2 = format!("sha384-{}", "B".repeat(64));
    edit_manifest(&f, "board", |v| v["requires"]["libs"][0]["integrity"] = json!(sri2));
    assert_ne!(live(&f, "board").1.hash, b3.hash, "the pin is part of the version");
    // A shipped library line: same rule, checked without the CDN pin rules.
    edit_manifest(&f, "board", |v| {
        v["requires"] = json!({"libs": ["marked@18"]});
    });
    let (m4, b4) = live(&f, "board");
    assert_eq!(m4.libs, vec![json!("marked@18")]);
    assert_ne!(b4.hash, b0.hash);
    assert!(!signer_gate::passes(&conn, &g, "board", &b4.hash).expect("gate"), "a library added on disk is unsigned");

    // Putting it back as it was is the signed version again.
    edit_manifest(&f, "board", |v| {
        v.as_object_mut().expect("object").remove("requires");
    });
    assert_eq!(live(&f, "board").1.hash, b0.hash, "the same document and run fields are the same version");
}

/// L1 / L3: libraries go into the version as written, never resolved, so
/// a K2 patch update inside a line can't unsign a widget.
#[test]
fn libraries_are_hashed_as_written() {
    let base = |libs: J| {
        let src = json!({"schema": 1, "name": "T", "caps": ["agents:read"], "requires": {"libs": libs}}).to_string();
        widgets::parse_manifest("widgets/t/manifest.json", &src, "t").0.expect("manifest")
    };
    let line = base(json!(["uplot@1"]));
    let d = widgets::version_digest(&line);
    assert!(d.contains("\"uplot@1\""), "{d}");
    let shipped = crate::zen::stdlib::zen_lib_manifest().libs.iter().find(|l| l.id == "uplot").expect("uplot row").version.clone();
    assert!(!d.contains(&shipped), "the resolved version {shipped} is not in the digest: {d}");
    assert_ne!(widgets::version_digest(&base(json!(["uplot@1", "marked@18"]))), widgets::version_digest(&base(json!(["marked@18", "uplot@1"]))), "load order is part of it");
    // Caps and reads are sets: their order is not.
    let a = widgets::parse_manifest("w/m", &json!({"schema": 1, "name": "T", "caps": ["agents:read", "thread:read"]}).to_string(), "t").0.expect("a");
    let b = widgets::parse_manifest("w/m", &json!({"schema": 1, "name": "U", "description": "x", "caps": ["thread:read", "agents:read"]}).to_string(), "t").0.expect("b");
    assert_eq!(widgets::version_digest(&a), widgets::version_digest(&b));
}

/// L1: a locked Garden's frozen copy is re-checked on every read. A hand
/// edit to its saved manifest refuses the copy (broken; never the live
/// folder instead).
#[test]
fn a_tampered_frozen_manifest_is_refused() {
    let (_t, f) = set_up("frozen");
    f.new_widget("board", Some("hello")).expect("new");
    let g = gid(&f, "Garden 1");
    place(&f, &g, "board");
    f.set_garden_lock(&g, true).expect("lock");
    let frozen = f.frozen_widget(&g, "board").expect("frozen copy");
    let h0 = frozen.hash().to_string();
    assert_eq!(h0, live(&f, "board").1.hash);
    let meta_path = f.lock_dir(&g).join(crate::zen::lock::LOCKED_WIDGETS_DIR).join("board").join(LAST_GOOD_JSON);
    let mut meta: J = serde_json::from_str(&fs::read_to_string(&meta_path).expect("meta")).expect("json");
    assert_eq!(meta["hashScheme"], json!(crate::zen::widget_store::HASH_SCHEME));
    meta["manifest"]["caps"] = json!(["agents:read", "thread:post", "files:read"]);
    meta["manifest"]["requires"] = json!(null);
    meta["manifest"]["libs"] = json!([{"url": "https://cdn.jsdelivr.net/npm/x@1.0.0/x.js", "integrity": "sha384-x"}]);
    fs::write(&meta_path, serde_json::to_string_pretty(&meta).expect("json")).expect("tamper");
    let st = f.garden_widget_status(&g, "board");
    assert_eq!(st.state, WidgetState::Broken, "{st:?}");
    assert!(st.bundle.is_none(), "a tampered copy never runs");
    assert!(st.errors[0].message.contains("changed on disk"), "{:?}", st.errors);
    assert!(f.widget_bundle_in(Some(&g), "board").expect("route").bundle.is_none(), "the bundle route has nothing to serve");
    // Stripping the scheme (the old shape) is refused too.
    meta.as_object_mut().expect("object").remove("hashScheme");
    fs::write(&meta_path, serde_json::to_string_pretty(&meta).expect("json")).expect("old shape");
    assert_eq!(f.garden_widget_status(&g, "board").state, WidgetState::Broken);
    // Unlock and lock again: the folder's version is frozen anew.
    f.set_garden_lock(&g, false).expect("unlock");
    f.set_garden_lock(&g, true).expect("lock");
    assert_eq!(f.garden_widget_status(&g, "board").hash(), h0);
}

/// The one-time carry: each stamp moves from the old hash to the new one
/// (live, last good, frozen), even when the live manifest differs from the
/// signed one (logged); old-shape copies are rewritten; a second run does
/// nothing; an old-shape copy planted afterwards is refused.
#[test]
fn the_carry_moves_signatures_once_and_logs_manifest_differences() {
    let (_t, f) = set_up("carry");
    let conn = db();
    f.new_widget("board", Some("hello")).expect("new board");
    f.new_widget("notes", Some("hello")).expect("new notes");
    f.new_widget("loose", Some("hello")).expect("new loose");
    edit_manifest(&f, "notes", |v| v["name"] = json!("Notes"));
    edit_manifest(&f, "loose", |v| v["description"] = json!("unsigned"));
    let g = gid(&f, "Garden 1");
    place(&f, &g, "board");
    f.set_garden_lock(&g, true).expect("lock");

    // 0.46.4's world: last-good and the frozen copy in the old shape,
    // stamps on the document hash.
    let (bm, bb) = live(&f, "board");
    let (nm, nb) = live(&f, "notes");
    write_legacy(&f.widget_history_dir("board"), &bm, &bb);
    write_legacy(&f.widget_history_dir("notes"), &nm, &nb);
    let frozen_dir = f.lock_dir(&g).join(crate::zen::lock::LOCKED_WIDGETS_DIR).join("board");
    write_legacy(&frozen_dir, &bm, &bb);
    let old_board = bb.template_hash();
    let old_notes = nb.template_hash();
    authors::stamp(&conn, "board", &old_board, &agent("w-maker"), StampSource::Sign, CallerKind::Agent, 100).expect("stamp board");
    authors::stamp(&conn, "notes", &old_notes, &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 101).expect("stamp notes");
    assert!(f.frozen_widget(&g, "board").is_some_and(|s| s.bundle.is_none()), "before the carry an old-shape copy isn't run");

    // Notes' manifest changed after it was signed (same code): a cap by hand.
    edit_manifest(&f, "notes", |v| v["caps"] = json!(["agents:read", "thread:read"]));
    let (_, nb2) = live(&f, "notes");
    assert_eq!(nb2.template_hash(), old_notes, "same document");

    let r = carry_once(&conn, &f, 200).expect("carry").expect("first run");
    // Board: live, last good and frozen all name one new hash, signed by its maker.
    assert_eq!(authors::maker_of(&conn, "board", &bb.hash).expect("maker"), agent("w-maker"));
    let board_rows: Vec<&Carried> = r.versions.iter().filter(|v| v.widget == "board").collect();
    assert_eq!(board_rows.len(), 3, "{board_rows:?}");
    assert!(board_rows.iter().all(|v| v.new_hash == bb.hash && v.old_hash == old_board));
    assert_eq!(board_rows.iter().find(|v| v.place == Place::Frozen).and_then(|v| v.garden.clone()), Some(g.clone()));
    let (signed_at, source): (i64, String) = conn
        .query_row("SELECT signed_at, source FROM zen_widget_authors WHERE widget = 'board' AND hash = ?1", params![bb.hash], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("carried row");
    assert_eq!((signed_at, source.as_str()), (100, "import"), "same signing time, source import");
    // Notes: carried although its live manifest differs, and the difference is logged.
    assert_eq!(authors::maker_of(&conn, "notes", &nb2.hash).expect("maker"), MadeBy::Owner, "carried anyway (Rosson's default)");
    let differs = r.differing();
    assert_eq!(differs.len(), 1, "{:?}", r.versions);
    assert_eq!(differs[0].widget, "notes");
    assert_eq!(differs[0].differs.as_deref(), Some("now asks for thread:read"), "{differs:?}");
    let logged: String = conn
        .query_row("SELECT differs FROM zen_version_carry WHERE widget = 'notes' AND place = 'live'", [], |r| r.get(0))
        .expect("logged difference");
    assert!(logged.contains("thread:read"), "{logged}");
    // Notes' last good keeps the manifest it was saved with: its own new hash, signed too.
    let notes_last = r.versions.iter().find(|v| v.widget == "notes" && v.place == Place::LastGood).expect("notes last good");
    assert_eq!(notes_last.new_hash, nb.hash);
    assert_eq!(authors::maker_of(&conn, "notes", &nb.hash).expect("maker"), MadeBy::Owner);
    // Loose was never signed: still unsigned.
    let (_, lb) = live(&f, "loose");
    assert_eq!(authors::maker_of(&conn, "loose", &lb.hash).expect("maker"), MadeBy::Unclaimed);
    assert!(r.versions.iter().any(|v| v.widget == "loose" && v.maker.is_none()));
    // The old-shape copies are rewritten and read again, `at` kept.
    assert_eq!(r.rewritten, 3, "{r:?}");
    assert!(r.failed.is_empty());
    let (_, fb, at) = read_saved_bundle(&frozen_dir).expect("frozen copy reads after the carry");
    assert_eq!((fb.hash.as_str(), at.as_str()), (bb.hash.as_str(), "2026-10-20T08:00:00.000Z"));
    assert_eq!(f.garden_widget_status(&g, "board").hash(), bb.hash, "the locked Garden runs its frozen copy again");
    assert_eq!(f.widget_status("notes").last_good_at, None, "notes' live version isn't its last good one");
    // The news line names the difference.
    let n = notice(&r).expect("a notice");
    assert!(
        n.detail.iter().any(|d| d.starts_with("notes: signed by owner; since then it now asks for thread:read.") && d.contains("k2 zen widget sign notes --ask")),
        "{:?}",
        n.detail
    );
    // The difference waits for the person until they sign that version.
    assert_eq!(unreviewed_difference(&conn, "notes", &nb2.hash).expect("read").as_deref(), Some("now asks for thread:read"));
    assert_eq!(unreviewed(&conn).expect("list").len(), 1);
    assert_eq!(
        version_changes(&f, &conn, "notes").expect("changes").as_deref(),
        Some("This version of notes asks for agents:read, thread:read; reads no other widget's data; loads no libraries; reads no data sources. Since it was signed, it now asks for thread:read.")
    );
    assert_eq!(mark_reviewed(&conn, "notes", &nb2.hash, 400).expect("review"), 1);
    assert!(unreviewed_difference(&conn, "notes", &nb2.hash).expect("read").is_none());

    // Once only.
    assert!(ran(&conn).expect("ran"));
    assert!(carry_once(&conn, &f, 300).expect("second").is_none(), "a second boot does nothing");
    // An old-shape copy put on disk afterwards, with a wider manifest, is refused.
    let mut wide = bm.clone();
    wide.caps.push("thread:post".into());
    write_legacy(&frozen_dir, &wide, &bb);
    assert!(f.garden_widget_status(&g, "board").bundle.is_none(), "an old-shape file after the carry never runs");
}

/// A computer that never set Zen up: the run is recorded with nothing to do.
#[test]
fn the_carry_records_a_run_with_no_zen_folder() {
    let d = std::env::temp_dir().join(format!("k2-zen-carry-none-{}", uuid::Uuid::new_v4().simple()));
    let _t = Tmp(d.clone());
    let f = ZenFiles::new(d.join(".k2").join("zen"));
    let conn = db();
    let r = carry_once(&conn, &f, 1).expect("carry").expect("ran");
    assert!(r.versions.is_empty());
    assert!(ran(&conn).expect("ran"));
    assert!(notice(&r).is_none());
}

#[test]
fn manifest_diff_names_each_change() {
    let m = |v: J| widgets::parse_manifest("w/m", &v.to_string(), "t").0.expect("manifest");
    let a = m(json!({"schema": 1, "name": "A", "caps": ["agents:read", "store:read"], "reads": ["other"]}));
    let b = m(json!({"schema": 1, "name": "B", "caps": ["agents:read", "thread:post"], "requires": {"libs": ["marked@18"]}}));
    let d = manifest_diff(&a, &b).expect("differs");
    assert_eq!(
        d,
        "now asks for thread:post; no longer asks for store:read; no longer reads the data of other; now loads the library marked@18"
    );
    assert_eq!(manifest_diff(&a, &a), None);
    let sri = format!("sha384-{}", "A".repeat(64));
    let c = m(json!({"schema": 1, "name": "C", "caps": ["agents:read", "store:read"], "reads": ["other"],
        "requires": {"libs": [{"url": "https://cdn.jsdelivr.net/npm/x@1.0.0/x.js", "integrity": sri}]}}));
    assert_eq!(manifest_diff(&a, &c).as_deref(), Some("now loads the CDN file https://cdn.jsdelivr.net/npm/x@1.0.0/x.js"));
    assert_eq!(
        manifest_summary(&c),
        "asks for agents:read, store:read; reads the data of other; loads the CDN file https://cdn.jsdelivr.net/npm/x@1.0.0/x.js; reads no data sources"
    );
}

/// The zen.sign card's words: the difference from the newest signed
/// version in the code history, or what a never-signed widget asks for.
#[test]
fn version_changes_reads_the_last_signed_version_from_history() {
    let (_t, f) = set_up("changes");
    let conn = db();
    f.new_widget("board", Some("hello")).expect("new");
    f.refresh().expect("refresh");
    assert_eq!(
        version_changes(&f, &conn, "board").expect("changes").as_deref(),
        Some("This version of board asks for agents:read; reads no other widget's data; loads no libraries; reads no data sources. No earlier version was signed by you or an agent on its Gardens' lists.")
    );
    let (_, b0) = live(&f, "board");
    authors::stamp(&conn, "board", &b0.hash, &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 1).expect("stamp");
    edit_manifest(&f, "board", |v| v["requires"] = json!({"libs": ["uplot@1"]}));
    f.refresh().expect("refresh");
    assert_eq!(
        version_changes(&f, &conn, "board").expect("changes").as_deref(),
        Some("This version of board asks for agents:read; reads no other widget's data; loads uplot@1; reads no data sources. Since the last version signed by you or an agent on its Gardens' lists, it now loads the library uplot@1.")
    );
    assert_eq!(version_changes(&f, &conn, "nope").expect("changes"), None);
}


/// Security review finding 4: K2 0.46.5 served before the carry ran (a
/// failed first carry) and rewrote the old saved copies. The copy saved
/// before 0.46.5 is kept beside it, so the late carry still compares: an
/// unchanged widget keeps its signature; one whose manifest changed in
/// between is NOT carried (a row, the log, the audit, the news).
#[test]
fn a_late_carry_refuses_a_manifest_changed_since_0_46_5_first_ran() {
    let (_t, f) = set_up("late");
    let conn = db();
    f.new_widget("board", Some("hello")).expect("new board");
    f.new_widget("notes", Some("hello")).expect("new notes");
    f.refresh().expect("refresh");
    let (bm, bb) = live(&f, "board");
    let (nm, nb) = live(&f, "notes");
    write_legacy(&f.widget_history_dir("board"), &bm, &bb);
    write_legacy(&f.widget_history_dir("notes"), &nm, &nb);
    authors::stamp(&conn, "board", &bb.template_hash(), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 100).expect("stamp board");
    authors::stamp(&conn, "notes", &nb.template_hash(), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 101).expect("stamp notes");

    // 0.46.5 serves without the carry: its first refresh rewrites both
    // saved copies (keeping each old one beside), and notes is widened.
    f.refresh().expect("0.46.5 refresh");
    for w in ["board", "notes"] {
        let h = f.widget_history_dir(w);
        assert!(read_legacy_saved_bundle(&h).is_none(), "{w}: rewritten in the new shape");
        assert!(h.join(PRE_CARRY_JSON).exists(), "{w}: the copy saved before 0.46.5 is kept");
    }
    edit_manifest(&f, "notes", |v| v["caps"] = json!(["agents:read", "thread:post"]));
    f.refresh().expect("refresh again");
    let kept = read_pre_carry_saved_bundle(&f.widget_history_dir("notes")).expect("the first old copy stays");
    assert_eq!(kept.0.caps, nm.caps, "the kept copy is the one saved before 0.46.5");
    let (_, nb2) = live(&f, "notes");
    assert_eq!(nb2.template_hash(), nb.template_hash(), "same code, wider manifest");

    let r = carry_once(&conn, &f, 200).expect("carry").expect("first run");
    // Board didn't change: carried.
    assert_eq!(authors::maker_of(&conn, "board", &bb.hash).expect("maker"), MadeBy::Owner);
    // Notes changed in between: its live version is NOT carried.
    assert_eq!(authors::maker_of(&conn, "notes", &nb2.hash).expect("maker"), MadeBy::Unclaimed, "never carried onto a manifest changed since");
    let refused = r.refused();
    assert_eq!(refused.len(), 1, "{:?}", r.versions);
    assert_eq!((refused[0].widget.as_str(), refused[0].place), ("notes", Place::Live));
    assert_eq!(refused[0].refused.as_deref(), Some("K2 0.46.5 ran before this carry and since then it now asks for thread:post"));
    assert!(refused[0].maker.is_none() && refused[0].differs.is_none(), "{:?}", refused[0]);
    let (carried, why): (i64, String) = conn
        .query_row("SELECT carried, refused FROM zen_version_carry WHERE widget = 'notes' AND place = 'live'", [], |r| Ok((r.get(0)?, r.get(1)?)))
        .expect("the refused row");
    assert_eq!(carried, 0);
    assert!(why.contains("thread:post"), "{why}");
    // What was signed (the kept copy's own version) still carries.
    assert_eq!(authors::maker_of(&conn, "notes", &nb.hash).expect("maker"), MadeBy::Owner);
    // The newer saved copies stay; the kept old ones are gone.
    for w in ["board", "notes"] {
        let h = f.widget_history_dir(w);
        assert!(!h.join(PRE_CARRY_JSON).exists(), "{w}: the kept copy is removed after the carry");
        assert!(read_saved_bundle(&h).is_some(), "{w}: its saved copy reads");
    }
    assert_eq!(read_saved_bundle(&f.widget_history_dir("notes")).expect("notes").1.hash, nb2.hash, "the late carry left the newer copy");
    // The news and the review list.
    let n = notice(&r).expect("a notice");
    assert!(n.detail.iter().any(|d| d.starts_with("notes: K2 didn't keep its signature: K2 0.46.5 ran before this carry")), "{:?}", n.detail);
    assert!(unreviewed(&conn).expect("list").is_empty(), "a refused carry has no signature to review");
}

/// A late carry with no copy saved before 0.46.5 (nothing kept) can't tell
/// what was signed: a signed live version is not carried.
#[test]
fn a_late_carry_without_the_old_copy_carries_nothing_live() {
    let (_t, f) = set_up("late-none");
    let conn = db();
    f.new_widget("board", Some("hello")).expect("new board");
    f.refresh().expect("refresh: a new-shape saved copy, no old one");
    let (_, bb) = live(&f, "board");
    authors::stamp(&conn, "board", &bb.template_hash(), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 100).expect("stamp");
    let r = carry_once(&conn, &f, 200).expect("carry").expect("first run");
    assert_eq!(authors::maker_of(&conn, "board", &bb.hash).expect("maker"), MadeBy::Unclaimed);
    assert_eq!(r.refused().len(), 1, "{:?}", r.versions);
    assert!(r.refused()[0].refused.as_deref().is_some_and(|w| w.contains("can't tell what was signed")), "{:?}", r.refused());
}

/// Security review finding 6: a frozen copy whose saved manifest differs
/// from the one in the code history with the same document is logged at
/// the carry (carried, as Rosson's default, but never silently).
#[test]
fn the_carry_logs_a_frozen_copy_whose_manifest_differs_from_its_history() {
    let (_t, f) = set_up("frozen-diff");
    let conn = db();
    f.new_widget("board", Some("hello")).expect("new");
    let g = gid(&f, "Garden 1");
    place(&f, &g, "board");
    f.set_garden_lock(&g, true).expect("lock");
    let (bm, bb) = live(&f, "board");
    write_legacy(&f.widget_history_dir("board"), &bm, &bb);
    // Before the upgrade the frozen copy's manifest was widened on disk
    // (0.46.4 never hashed it).
    let mut wide = bm.clone();
    wide.caps.push("thread:post".into());
    let wb = Bundle::from_template(&bb.template(), &bb.nonce_offsets(), bb.code_bytes, widgets::version_digest(&wide)).expect("bundle");
    let frozen_dir = f.lock_dir(&g).join(crate::zen::lock::LOCKED_WIDGETS_DIR).join("board");
    write_legacy(&frozen_dir, &wide, &wb);
    authors::stamp(&conn, "board", &bb.template_hash(), &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 100).expect("stamp");

    let r = carry_once(&conn, &f, 200).expect("carry").expect("first run");
    let frozen = r.versions.iter().find(|v| v.place == Place::Frozen).expect("frozen row");
    assert_eq!(frozen.differs.as_deref(), Some("now asks for thread:post"), "{frozen:?}");
    assert_eq!(frozen.maker.as_deref(), Some("owner"), "carried, and logged");
    let last = r.versions.iter().find(|v| v.place == Place::LastGood).expect("last good row");
    assert_eq!(last.differs, None, "the last good copy matches its code history");
    let logged: String = conn
        .query_row("SELECT differs FROM zen_version_carry WHERE widget = 'board' AND place = 'frozen'", [], |r| r.get(0))
        .expect("logged");
    assert!(logged.contains("thread:post"), "{logged}");
    assert_eq!(unreviewed_difference(&conn, "board", &wb.hash).expect("read").as_deref(), Some("now asks for thread:post"));
    let n = notice(&r).expect("a notice");
    assert!(n.detail.iter().any(|d| d.starts_with("board: signed by owner; since then it now asks for thread:post.")), "{:?}", n.detail);
}

/// Security review finding 3, in core: a planted snapshot (a signed hash
/// in its meta, other files) is never a baseline, and an unlisted agent's
/// stamp isn't "signed".
#[test]
fn version_changes_ignores_planted_snapshots_and_unlisted_signers() {
    let (_t, f) = set_up("planted");
    let conn = db();
    f.new_widget("board", Some("hello")).expect("new");
    let g = gid(&f, "Garden 1");
    place(&f, &g, "board");
    let (_, b0) = live(&f, "board");
    authors::stamp(&conn, "board", &b0.hash, &MadeBy::Owner, StampSource::Sign, CallerKind::Owner, 1).expect("person signs v0");
    edit_manifest(&f, "board", |v| v["caps"] = json!(["agents:read", "thread:post"]));
    f.refresh().expect("refresh");
    let (_, b1) = live(&f, "board");
    // w-other isn't on Garden 1's list: its stamp on v1 is no baseline.
    authors::stamp(&conn, "board", &b1.hash, &agent("w-other"), StampSource::Sign, CallerKind::Agent, 2).expect("unlisted stamp");
    edit_manifest(&f, "board", |v| v["caps"] = json!(["agents:read", "thread:post", "presence:read"]));
    f.refresh().expect("refresh");
    let (_, b2) = live(&f, "board");
    // Planted: v0's hash in the meta, v2's files.
    let snap = f.widget_history_dir("board").join("29991231T235959999Z-000");
    fs::create_dir_all(&snap).expect("dir");
    for e in fs::read_dir(f.widget_dir("board")).expect("folder").flatten() {
        fs::copy(e.path(), snap.join(e.file_name())).expect("copy");
    }
    fs::write(snap.join("meta.json"), json!({"at": "2999-12-31T23:59:59Z", "clean": true, "hash": b0.hash}).to_string()).expect("meta");
    assert_eq!(f.widget_snapshots("board")[0].name, "29991231T235959999Z-000");
    let words = version_changes(&f, &conn, "board").expect("changes").expect("live");
    assert_eq!(
        words,
        format!(
            "This version of board asks for agents:read, thread:post, presence:read; reads no other widget's data; loads no libraries; reads no data sources. Since the last version signed by you or an agent on its Gardens' lists, it now asks for thread:post, presence:read."
        ),
        "against the person's v0 (b2 = {})",
        b2.hash
    );
    // With w-other listed, its v1 is the baseline.
    ga::add(&conn, &g, "w-other", &Change::new(Via::Settings, &Actor::Owner, 3)).expect("list");
    let words = version_changes(&f, &conn, "board").expect("changes").expect("live");
    assert!(words.ends_with(&format!("Since the last version signed by you or an agent on its Gardens' lists, it now asks for presence:read.")), "{words}");
}
