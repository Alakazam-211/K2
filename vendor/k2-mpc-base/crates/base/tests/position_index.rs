//! The engine's position index check reaches the host (v0.9.7): with a bug planted in the
//! index (the test-only `planted-position-bugs` feature: a removal's markers off by one, or
//! markers kept when another client's items are integrated) and the check on every marker
//! lookup, the shadow walk's disagreements are reported as
//! [`AuditEvent::PositionIndexMismatch`] (indices only), and the doc's text is still exactly
//! what the edits made (the walk's position was used). Without a planted bug, nothing is
//! reported.
//!
//! Where items are integrated (v0.9.7, after v0.9.5): a session's live doc integrates other
//! clients' items only from an editor's update, which the host applies through
//! [`Doc::apply_client_update`]. Since v0.9.5 an outside save is merged as a local edit on the
//! live doc (`edit_as`), not integrated, so the integrate bug is driven at the doc, by a raw
//! Yrs client's updates (built through Yrs's normal API), and the session's workload checks
//! that it is never served there either.
//!
//! ```sh
//! cargo test -p k2-mpc-base --features planted-position-bugs --test position_index
//! ```

#![cfg(feature = "planted-position-bugs")]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use k2_mpc_base::differential::PlantedPositionBug;
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Host, Participant, ParticipantKind, Rights, Session,
    SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};

const ROOT: &str = TextFilePack::ROOT;

#[derive(Default)]
struct Rec {
    found: Mutex<Vec<(Option<String>, u32, u32)>>,
}

impl Host for Rec {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full()
    }
    fn now_ms(&self) -> u64 {
        1_000
    }
    fn on_event(&self, ev: &AuditEvent) {
        if let AuditEvent::PositionIndexMismatch {
            root,
            index,
            marker,
            ..
        } = ev
        {
            self.found
                .lock()
                .unwrap()
                .push((root.clone(), *index, *marker));
        }
    }
}

fn open(tag: &str) -> (Arc<Rec>, SessionManager, Arc<Session>, ConnId, PathBuf) {
    let dir = std::env::temp_dir().join(format!("k2mb-posidx-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("ws")).unwrap();
    let file = dir.join("ws").join("a.md");
    let text: String = (0..40).map(|i| format!("line {i} of the doc\n")).collect();
    std::fs::write(&file, text).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let host = Arc::new(Rec::default());
    let mut config = Config::default();
    config.position_check_every = 1;
    config.edit_rate_per_s = 1_000_000;
    config.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store, host.clone(), config);
    let key = DocKey {
        space: "ws".into(),
        path: "a.md".into(),
    };
    let s = mgr
        .open_file(
            Arc::new(TextFilePack::new("markdown", 1 << 20)),
            &key,
            &file,
            &dir.join("ws"),
        )
        .unwrap();
    let p = Participant::new("ann", "Ann", "#336699", ParticipantKind::Agent, "cli");
    let conn = s.join(p, Stay::Held).unwrap().conn;
    (host, mgr, s, conn, file)
}

/// Edits that delete and type at positions after earlier ones, in several places, with an
/// outside edit to the file between rounds (merged into the doc: its items are integrated
/// there, not typed); checks the text after each against a plain string model.
fn workload(s: &Session, conn: ConnId, file: &Path) {
    let text = |s: &Session| s.read(conn).unwrap().text_by_root[0].1.clone();
    let mut model: Vec<String> = (0..40).map(|i| format!("line {i} of the doc\n")).collect();
    for round in 0..12usize {
        let k = (round * 7) % model.len();
        // a delete, then text typed further on (the lookups go through markers)
        model.remove(k);
        let at = (k + 5).min(model.len());
        model.insert(at, format!("typed {round}\n"));
        s.replace_text(conn, ROOT, &model.concat()).unwrap();
        assert_eq!(text(s), model.concat());
        if round % 3 == 2 {
            // written back, then edited outside the kit, and merged in
            s.flush(conn).unwrap();
            let at = (k + 11) % model.len();
            // inserts only: a delete drops the text's markers anyway
            model.insert(at, format!("outside {round}\n"));
            std::fs::write(file, model.concat()).unwrap();
            s.check_disk().unwrap();
            assert_eq!(text(s), model.concat());
        }
    }
}

#[test]
fn a_planted_index_bug_is_reported_and_never_served() {
    // the integrate bug: the session integrates nothing after a load (an outside save is a
    // local edit since v0.9.5), so it is never served there; reported at the doc (below)
    {
        let (_host, _mgr, s, conn, file) = open("integrate-session");
        s.plant_position_bug_for_tests(PlantedPositionBug::KeepMarkersOnIntegrate)
            .unwrap();
        workload(&s, conn, &file);
    }
    {
        let bug = PlantedPositionBug::RemoveOffByOne;
        let (host, _mgr, s, conn, file) = open(&format!("{bug:?}"));
        s.plant_position_bug_for_tests(bug).unwrap();
        workload(&s, conn, &file);
        let found = host.found.lock().unwrap().clone();
        eprintln!(
            "[position index] planted {bug:?}, check 1-in-1: {} mismatches reported; the text equals the model after every edit",
            found.len()
        );
        assert!(
            !found.is_empty(),
            "{bug:?}: the shadow check reported nothing"
        );
        assert!(
            found.iter().all(|(r, _, _)| r.as_deref() == Some(ROOT)),
            "{found:?}"
        );
    }
}

#[test]
fn an_unplanted_index_reports_nothing() {
    let (host, _mgr, s, conn, file) = open("clean");
    workload(&s, conn, &file);
    assert!(host.found.lock().unwrap().is_empty());
}

/// The integrate bug where items are integrated: an editor's updates applied to the doc
/// (`Doc::apply_client_update`) between the doc's own edits, whose lookups then go through
/// the markers the integration should have moved. Every sampled lookup is checked: the
/// mismatches are counted (what the session reports as `PositionIndexMismatch`) and the text
/// is the model's after every step (the walk's position was used).
#[test]
fn a_planted_integrate_bug_is_caught_where_items_are_integrated() {
    use k2_mpc_base::{Doc, Kind, Schema};
    use yrs::updates::decoder::Decode;
    use yrs::updates::encoder::Encode;
    use yrs::{OffsetKind, Options, ReadTxn, Text, Transact, Update};

    const S: Schema = Schema {
        name: "test",
        roots: &[("source", Kind::Text)],
    };
    for planted in [false, true] {
        let mut model: Vec<String> = (0..40).map(|i| format!("line {i} of the doc\n")).collect();
        let mut d = Doc::new(S).unwrap();
        d.replace_text_as(7, "source", &model.concat()).unwrap();
        d.set_position_check_every(1);
        if planted {
            d.plant_position_bug(PlantedPositionBug::KeepMarkersOnIntegrate);
        }
        // a raw Yrs client (a Yjs stand-in) holding what the doc holds
        let c = yrs::Doc::with_options(Options {
            client_id: yrs::block::ClientID::new(100),
            offset_kind: OffsetKind::Utf16,
            ..Options::default()
        });
        let ct = c.get_or_insert_text("source");
        let sync = |c: &yrs::Doc, d: &Doc| {
            let u = d
                .encode_diff(&c.transact().state_vector().encode_v1())
                .unwrap();
            c.transact_mut()
                .apply_update(Update::decode_v1(&u).unwrap())
                .unwrap();
        };
        sync(&c, &d);
        for round in 0..12usize {
            // the doc's own edit further on (its lookups set markers there)
            let k = (round * 7) % model.len();
            model.remove(k);
            let at = (k + 5).min(model.len());
            model.insert(at, format!("typed {round}\n"));
            d.replace_text_as(7, "source", &model.concat()).unwrap();
            assert_eq!(d.text("source").unwrap(), model.concat());
            // the editor types a line near the start, integrated into the doc
            sync(&c, &d);
            let line = (round * 3) % 5;
            let off: usize = model[..line].iter().map(|l| l.encode_utf16().count()).sum();
            let sv = c.transact().state_vector();
            ct.insert(
                &mut c.transact_mut(),
                off as u32,
                &format!("editor {round}\n"),
            );
            let u = c.transact().encode_state_as_update_v1(&sv);
            d.apply_client_update(&u, 100).unwrap();
            model.insert(line, format!("editor {round}\n"));
            assert_eq!(d.text("source").unwrap(), model.concat());
        }
        let stats = d.position_index_stats();
        eprintln!("[position index] doc, planted {planted}: {stats:?}; the text equals the model after every step");
        if planted {
            assert!(
                stats.mismatches > 0,
                "KeepMarkersOnIntegrate: the check found nothing where items are integrated"
            );
        } else {
            assert_eq!(stats.mismatches, 0, "{stats:?}");
        }
    }
}
