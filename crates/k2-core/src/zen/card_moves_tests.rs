//! Tests for the one-time move of the data-source cards to `@3` (0.46.4).
//! Every Zen folder is under the temp dir; never the real `~/.k2`.

use super::*;
use crate::zen::store::HISTORY_DIR;

struct Tmp(PathBuf);
impl Drop for Tmp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set_up(tag: &str) -> (Tmp, ZenFiles) {
    let d = std::env::temp_dir().join(format!("k2-zen-cardmoves-{tag}-{}", uuid::Uuid::new_v4().simple()));
    let f = ZenFiles::new(d.join(".k2").join("zen"));
    assert!(f.root().starts_with(std::env::temp_dir()), "tests never touch the real Zen folder");
    f.setup().expect("setup");
    (Tmp(d), f)
}

/// A new Garden named `name` whose file is `text` (checked clean).
fn garden(f: &ZenFiles, name: &str, text: &str) -> String {
    let id = f.new_garden(name, None, None, None).expect("new garden").id;
    let gf = ZenFile::Garden(id.clone());
    fs::write(f.path_of(&gf), text).expect("write garden");
    let c = f.check_src(&gf, text, &f.base_for(&gf));
    assert!(c.is_clean(), "{name}'s fixture must check clean: {:?}", c.errors);
    id
}

fn disk(f: &ZenFiles, id: &str) -> String {
    fs::read_to_string(f.path_of(&ZenFile::Garden(id.into()))).expect("read garden")
}

fn moved_dir(f: &ZenFiles, id: &str) -> PathBuf {
    f.root().join(HISTORY_DIR).join("gardens").join(format!("{id}.toml")).join(MOVED_DIR)
}

/// A person's page: comments (one names an old card), odd spacing, two old
/// cards (double and single quotes), a card already on @3, a user widget
/// and a theme table.
const OPS_PAGE: &str = "# My ops page. Old note: widget = \"k2:fleet-grid@2\" was the first card.\n\
schema = 1\n\
\n\
[layout]\n\
kind = \"columns\"\n\
[[layout.column]]\n\
size = 60\n\
min-width = 200\n\
[[layout.column]]\n\
size   = 40   # wide enough\n\
min-width = 200\n\
\n\
[[widget]]\n\
id = \"fleet\"\n\
kind = \"custom\"\n\
widget =   \"k2:fleet-grid@2\"   # the grid\n\
column = 0\n\
[widget.props.config]\n\
source = \"fleet\"\n\
\n\
[[widget]]\n\
id = \"money\"\n\
kind = \"custom\"\n\
widget = 'k2:money-alerts@1'\n\
column = 1\n\
\n\
[[widget]]\n\
id = \"spread\"\n\
kind = \"custom\"\n\
widget = \"k2:version-spread@3\"\n\
column = 1\n\
\n\
[colors.light]\n\
accent = \"#aa3355\"\n";

const PLAIN_PAGE: &str = "schema = 1\n\n[layout]\nkind = \"columns\"\n[[layout.column]]\nsize = 100\nmin-width = 200\n\n[[widget]]\nid = \"card\"\nkind = \"custom\"\nwidget = \"{w}\"\ncolumn = 0\n";

fn plain_page(w: &str) -> String {
    PLAIN_PAGE.replace("{w}", w)
}

#[test]
fn the_five_cards_are_the_source_cards() {
    let names: Vec<&str> = crate::zen::builtin_widgets::SOURCE_CARDS
        .iter()
        .map(|(id, _)| id.trim_start_matches(BUILTIN_PREFIX).split('@').next().expect("name"))
        .collect();
    assert_eq!(names, MOVED_CARDS);
    for c in MOVED_CARDS {
        for v in [1, 2, 3] {
            let id = format!("k2:{c}@{v}");
            assert!(
                crate::zen::builtin_widgets::builtin_widget(&parse_widget_ref(&id).expect("parses")).is_some(),
                "{id} ships"
            );
        }
        assert_eq!(moved_card(&format!("k2:{c}@1")).as_deref(), Some(format!("k2:{c}@3").as_str()));
        assert_eq!(moved_card(&format!("k2:{c}@2")).as_deref(), Some(format!("k2:{c}@3").as_str()));
        assert_eq!(moved_card(&format!("k2:{c}@3")), None);
        assert_eq!(moved_card(&format!("k2:{c}@12")), None);
    }
    assert_eq!(moved_card("k2:kanban@2"), None, "only the data-source cards move");
    assert_eq!(moved_card("k2:diary@1"), None);
    assert_eq!(moved_card("fleet-grid"), None, "a user widget folder never moves");
}

#[test]
fn refs_are_found_by_place_and_rewritten_in_place() {
    let hits = find_card_refs(OPS_PAGE).expect("parses");
    let got: Vec<(&str, &str, bool)> = hits.iter().map(|h| (h.from.as_str(), h.to.as_str(), h.plain)).collect();
    assert_eq!(got, [("k2:fleet-grid@2", "k2:fleet-grid@3", true), ("k2:money-alerts@1", "k2:money-alerts@3", true)]);
    assert_eq!(&OPS_PAGE[hits[0].range.clone()], "\"k2:fleet-grid@2\"");
    assert_eq!(&OPS_PAGE[hits[1].range.clone()], "'k2:money-alerts@1'");
    let want = OPS_PAGE
        .replace("widget =   \"k2:fleet-grid@2\"   # the grid", "widget =   \"k2:fleet-grid@3\"   # the grid")
        .replace("widget = 'k2:money-alerts@1'", "widget = 'k2:money-alerts@3'");
    assert_ne!(want, OPS_PAGE);
    assert_eq!(apply_card_moves(OPS_PAGE, &hits), want);
    assert!(want.contains("Old note: widget = \"k2:fleet-grid@2\" was"), "a comment is not a card");

    // An inline array of widgets.
    let inline = "schema = 1\nwidget = [ { id = \"d\", kind = \"custom\", widget = \"k2:deploys@2\", column = 0 } ]\n";
    let h = find_card_refs(inline).expect("parses");
    assert_eq!(h.len(), 1);
    assert_eq!(apply_card_moves(inline, &h), inline.replace("k2:deploys@2", "k2:deploys@3"));

    // A name spelled with an escape is found but never rewritten.
    let escaped = "schema = 1\n[[widget]]\nid = \"d\"\nkind = \"custom\"\nwidget = \"k2:relays\\u002ddns@1\"\ncolumn = 0\n";
    let h = find_card_refs(escaped).expect("parses");
    assert_eq!(h.len(), 1);
    assert!(!h[0].plain, "{h:?}");

    // Not a custom widget: not a card.
    let builtin_kind = "schema = 1\n[[widget]]\nid = \"t\"\nkind = \"conversation\"\nwidget = \"k2:fleet-grid@2\"\n";
    assert_eq!(find_card_refs(builtin_kind).expect("parses"), Vec::new());
    assert!(find_card_refs("schema = = 1").is_err(), "not TOML");
}

#[test]
fn a_garden_on_2_moves_to_3_with_every_other_byte_kept() {
    let (_t, f) = set_up("moves");
    let ops = garden(&f, "Ops", OPS_PAGE);
    let deploys = garden(&f, "Deploys", &plain_page("k2:deploys@2"));
    let gf = ZenFile::Garden(ops.clone());
    let snaps_before = f.snapshots(&gf).len();

    let r = f.move_source_cards_once().expect("move").expect("it ran");

    let want = OPS_PAGE
        .replace("widget =   \"k2:fleet-grid@2\"   # the grid", "widget =   \"k2:fleet-grid@3\"   # the grid")
        .replace("widget = 'k2:money-alerts@1'", "widget = 'k2:money-alerts@3'");
    assert_eq!(disk(&f, &ops), want, "only the two card versions changed, byte for byte");
    assert_eq!(disk(&f, &deploys), plain_page("k2:deploys@3"));
    assert!(f.check_src(&gf, &disk(&f, &ops), &f.base_for(&gf)).is_clean(), "the moved page checks clean");

    assert_eq!(r.moved.iter().map(|g| g.garden.as_str()).collect::<Vec<_>>(), [ops.as_str(), deploys.as_str()]);
    assert_eq!(r.locked, Vec::new());
    assert_eq!(r.left, Vec::new());
    let m = &r.moved[0];
    assert_eq!((m.name.as_str(), m.refs.len()), ("Ops", 2));
    assert_eq!(m.refs[0], CardRef { from: "k2:fleet-grid@2".into(), to: "k2:fleet-grid@3".into() });

    // Backups: the old text, as a snapshot and as a copy that is never pruned.
    assert_eq!(fs::read_to_string(f.root().join(&m.backup)).expect("backup"), OPS_PAGE);
    assert!(m.backup.starts_with(&format!(".history/gardens/{ops}.toml/moved/")), "{}", m.backup);
    assert_eq!(f.snapshot_text(&gf, &m.snapshot).as_deref(), Some(OPS_PAGE), "k2 zen reset --to {} restores it", m.snapshot);
    assert!(f.snapshots(&gf).len() > snaps_before, "a snapshot was kept");
    assert!(f.snapshots(&gf).iter().all(|s| !s.name.contains(CARD_MOVE)), "the moved/ copy is never a snapshot");

    // The marker holds the report.
    assert_eq!(f.card_move_report().as_ref(), Some(&r));

    // News: one notice, first, that names both Gardens.
    let news = f.news().expect("news");
    let item = &news["items"][0];
    assert_eq!((item["id"].as_str(), item["kind"].as_str()), (Some(CARD_MOVE_NOTICE), Some("notice")));
    assert_eq!(item["title"].as_str(), Some("Data cards updated"));
    let body = item["body"].as_str().expect("body");
    assert!(body.contains("in Ops and Deploys to their newest version"), "{body}");
    let detail: Vec<&str> = item["detail"].as_array().expect("detail").iter().map(|v| v.as_str().expect("line")).collect();
    assert!(detail[0].starts_with(&format!("Ops ({ops}): k2:fleet-grid@2 → k2:fleet-grid@3, k2:money-alerts@1 → k2:money-alerts@3")), "{detail:?}");
    f.news_seen(&[CARD_MOVE_NOTICE.to_string()], false).expect("seen");
    let news = f.news().expect("news");
    assert!(news["items"].as_array().expect("items").iter().all(|i| i["id"] != CARD_MOVE_NOTICE), "seen once, gone");
}

#[test]
fn a_locked_garden_is_skipped_and_reported() {
    let (_t, f) = set_up("locked");
    let page = plain_page("k2:relays-dns@2");
    let id = garden(&f, "Edge", &page);
    f.set_garden_lock(&id, true).expect("lock");
    let frozen_path = f.lock_dir(&id).join(crate::zen::lock::LOCKED_GARDEN_FILE);
    assert_eq!(fs::read_to_string(&frozen_path).expect("frozen"), page);

    let r = f.move_source_cards_once().expect("move").expect("it ran");

    assert_eq!(r.moved, Vec::new());
    assert_eq!(
        r.locked,
        vec![LeftGarden { garden: id.clone(), name: "Edge".into(), refs: vec!["k2:relays-dns@2".into()], reason: "locked".into() }]
    );
    assert_eq!(disk(&f, &id), page, "the file is untouched");
    assert_eq!(fs::read_to_string(&frozen_path).expect("frozen"), page, "the frozen copy is untouched");
    assert!(!moved_dir(&f, &id).exists(), "no backup: nothing changed");
    assert!(f.is_locked(&id), "still locked");

    let news = f.news().expect("news");
    let item = &news["items"][0];
    assert_eq!(item["id"].as_str(), Some(CARD_MOVE_NOTICE));
    assert_eq!(item["title"].as_str(), Some("Data cards to update by hand"));
    let body = item["body"].as_str().expect("body");
    assert!(body.contains("Edge is locked, so K2 left it as it is: unlock it and change each card's @1 or @2 to @3."), "{body}");
    assert_eq!(item["detail"][0].as_str(), Some(format!("Edge ({id}): locked, not moved: k2:relays-dns@2").as_str()));
}

#[test]
fn gardens_on_3_or_without_cards_are_untouched_and_there_is_no_news() {
    let (_t, f) = set_up("untouched");
    let on3 = plain_page("k2:fleet-grid@3");
    let a = garden(&f, "On three", &on3);
    let other = plain_page("my-widget");
    let other = other.replace("widget = \"my-widget\"", "widget = \"k2:kanban@2\"");
    let b = garden(&f, "Board", &other);
    let stubs: Vec<(String, String)> = f.gardens().into_iter().filter(|g| g.id != a && g.id != b).map(|g| (g.id.clone(), disk(&f, &g.id))).collect();
    let snaps: Vec<usize> = [&a, &b].iter().map(|id| f.snapshots(&ZenFile::Garden((*id).clone())).len()).collect();

    let r = f.move_source_cards_once().expect("move").expect("it ran");

    assert_eq!((r.moved.len(), r.locked.len(), r.left.len()), (0, 0, 0), "{r:?}");
    assert_eq!(disk(&f, &a), on3);
    assert_eq!(disk(&f, &b), other);
    for (id, text) in &stubs {
        assert_eq!(&disk(&f, id), text, "{id} untouched");
    }
    let after: Vec<usize> = [&a, &b].iter().map(|id| f.snapshots(&ZenFile::Garden((*id).clone())).len()).collect();
    assert_eq!(after, snaps, "no snapshot written");
    assert!(!moved_dir(&f, &a).exists() && !moved_dir(&f, &b).exists(), "no backup written");
    assert!(f.card_move_marker().is_file(), "the marker says it ran");
    assert!(f.read_news().notices.is_empty(), "nothing to say");
}

#[test]
fn a_second_run_does_nothing() {
    let (_t, f) = set_up("twice");
    let id = garden(&f, "Money", &plain_page("k2:money-alerts@2"));
    let first = f.move_source_cards_once().expect("move").expect("it ran");
    assert_eq!(first.moved.len(), 1);

    // A Garden that names @2 after the move (an agent wrote it) stays: the
    // move ran once.
    let later = garden(&f, "Later", &plain_page("k2:fleet-grid@2"));
    let marker = fs::read_to_string(f.card_move_marker()).expect("marker");
    let news = fs::read_to_string(f.news_path()).expect("news");
    assert_eq!(f.move_source_cards_once().expect("again"), None, "never runs twice");
    assert_eq!(disk(&f, &later), plain_page("k2:fleet-grid@2"));
    assert_eq!(disk(&f, &id), plain_page("k2:money-alerts@3"));
    assert_eq!(fs::read_to_string(f.card_move_marker()).expect("marker"), marker, "marker unchanged");
    assert_eq!(fs::read_to_string(f.news_path()).expect("news"), news, "no second notice");
    assert_eq!(fs::read_dir(moved_dir(&f, &id)).expect("moved dir").count(), 1, "one backup");
}

#[test]
fn a_garden_with_errors_or_an_odd_spelling_is_left_and_reported() {
    let (_t, f) = set_up("left");
    let ok = garden(&f, "Fine", &plain_page("k2:deploys@1"));
    // Valid TOML with a Garden error (an unknown key): the live page is the
    // last good snapshot, so K2 doesn't guess.
    let broken_id = f.new_garden("Broken", None, None, None).expect("new garden").id;
    let broken = plain_page("k2:fleet-grid@1").replace("column = 0\n", "column = 0\nbogus = 1\n");
    fs::write(f.path_of(&ZenFile::Garden(broken_id.clone())), &broken).expect("write");
    let not_toml_id = f.new_garden("Half saved", None, None, None).expect("new garden").id;
    let not_toml = "schema = 1\n[[widget]]\nwidget = \"k2:relays-dns@2\"\nkind = = \"custom\"\n";
    fs::write(f.path_of(&ZenFile::Garden(not_toml_id.clone())), not_toml).expect("write");
    let odd_id = f.new_garden("Odd", None, None, None).expect("new garden").id;
    let odd = plain_page("k2:version-spread@2").replace("\"k2:version-spread@2\"", "\"k2:version\\u002dspread@2\"");
    fs::write(f.path_of(&ZenFile::Garden(odd_id.clone())), &odd).expect("write");

    let r = f.move_source_cards_once().expect("move").expect("it ran");

    assert_eq!(r.moved.iter().map(|g| g.garden.clone()).collect::<Vec<_>>(), vec![ok.clone()]);
    let left: Vec<(&str, &str)> = r.left.iter().map(|g| (g.name.as_str(), g.reason.as_str())).collect();
    assert_eq!(left, [("Broken", "has_errors"), ("Half saved", "not_toml"), ("Odd", "unusual_spelling")]);
    assert_eq!(disk(&f, &broken_id), broken);
    assert_eq!(disk(&f, &not_toml_id), not_toml);
    assert_eq!(disk(&f, &odd_id), odd);
    let n = &f.read_news().notices[0];
    assert!(
        n.body.contains("K2 couldn't move the cards in Broken (its file has errors), Half saved (its file isn't valid TOML) and Odd (a card's name is written in an unusual way)"),
        "{}",
        n.body
    );
    assert!(n.detail.contains(&format!("Broken ({broken_id}): not moved, its file has errors: k2:fleet-grid@1")), "{:?}", n.detail);
}
