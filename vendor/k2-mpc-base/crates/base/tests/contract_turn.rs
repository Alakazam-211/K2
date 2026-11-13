//! The host-given turn: an optional turn id on
//! every write entry, live and tier 0, stored on the change record, carried on change events
//! and in history, never invented by the kit, and cleared by a participant's erasure. Public
//! API only.
//!
//! The planted faults (`k2_mpc_base::differential::ContractMutant`) are caught by name:
//! `plant_turn_not_cleared_on_erase_is_caught` runs the erasure check under
//! `TurnNotClearedOnErase`, and `plant_revert_drops_the_turn_is_caught` the revert and restore
//! check under `RevertDropsTurn`, and each requires its check to fail where it says.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::{with_contract_mutant, ContractMutant};
use k2_mpc_base::elem::ListOp;
use k2_mpc_base::session::{
    AnyFile, AuditEvent, BlobSession, BlobWriteOptions, ChangeEvent, Config, ConnId, EditOptions,
    HistoryOptions, Host, LogItem, Participant, ParticipantKind, PurgeRequest, PurgeScope, Rights,
    Session, SessionManager, Stay, TextFilePack, UndoOutcome, UndoRequest, WriteOptions,
    WriteOutcome,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::ToolOptions;
use k2_mpc_base::{Edit, Key, Turn};

#[path = "keyed_pack/mod.rs"]
mod keyed_pack;

use keyed_pack::{Keyed, ITEMS};

const MD: TextFilePack = TextFilePack::new("markdown", 4 << 20);
const ROOT: &str = TextFilePack::ROOT;

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::full())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _ev: &AuditEvent) {}
}

struct TmpDir(PathBuf);
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Fx {
    dir: TmpDir,
    ws: PathBuf,
    host: Arc<TestHost>,
    mgr: SessionManager,
}

fn store_and_mgr(dir: &std::path::Path, host: &Arc<TestHost>) -> SessionManager {
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    SessionManager::new(store, host.clone(), c).with_blob_store(blobs)
}

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-turn-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
        rights: Mutex::new(HashMap::new()),
    });
    let mgr = store_and_mgr(&p, &host);
    let f = Fx {
        dir: TmpDir(p),
        ws,
        host,
        mgr,
    };
    f.host
        .rights
        .lock()
        .unwrap()
        .insert("own".into(), Rights::full().with_owner(true));
    f
}

fn key(p: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: p.into(),
    }
}

fn person(pid: &str) -> Participant {
    Participant::new(pid, pid, "#2f9e8f", ParticipantKind::Person, "tab")
}

fn agent(pid: &str) -> Participant {
    Participant::new(pid, pid, "#6d5bd0", ParticipantKind::Agent, "cli")
}

fn t(s: &str) -> Turn {
    Turn::new(s).unwrap()
}

impl Fx {
    fn text(&self, name: &str, body: &str) -> Arc<Session> {
        let p = self.ws.join(name);
        if !p.exists() {
            fs::write(&p, body).unwrap();
        }
        self.mgr
            .open_file(Arc::new(MD), &key(name), &p, &self.ws)
            .unwrap()
    }
    fn keyed(&self) -> Arc<Session> {
        self.mgr
            .open(Arc::new(Keyed), &key("doc.keyed"), Some(b"i1\ni2\ni3\n"))
            .unwrap()
    }
    fn blob(&self, name: &str, body: &[u8]) -> Arc<BlobSession> {
        let p = self.ws.join(name);
        if !p.exists() {
            fs::write(&p, body).unwrap();
        }
        self.mgr
            .open_blob_file(Arc::new(AnyFile), &key(name), &p, &self.ws)
            .unwrap()
    }
}

fn join(s: &Session, p: Participant) -> ConnId {
    s.join(p, Stay::Held).unwrap().conn
}

fn drain(rx: &Receiver<ChangeEvent>) -> Vec<ChangeEvent> {
    let mut v = Vec::new();
    while let Ok(e) = rx.try_recv() {
        v.push(e);
    }
    v
}

/// The turn of every change in the log, oldest first, as `(seq, turn)`.
fn log_turns(s: &Session, c: ConnId) -> Vec<(u64, Option<String>)> {
    let mut v: Vec<(u64, Option<String>)> = s
        .log(c, &Default::default())
        .unwrap()
        .into_iter()
        .filter_map(|i| match i {
            LogItem::Change { info, .. } => Some((info.seq, info.turn.map(|t| t.to_string()))),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

fn argv(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

/// Each live write entry K2 calls (KH §A.2 item 5) carries its own call's turn onto its change,
/// its change event, the live window and the log; a person's write and an agent's write with no
/// turn carry none; the turns survive a reload from the store.
#[test]
fn every_live_text_write_entry_carries_its_turn() {
    let f = fx("text");
    let s = f.text("a.md", "one\n");
    let own = join(&s, person("own"));
    let bot = join(&s, agent("atlas"));
    let rx = s.subscribe(own).unwrap();
    let v0 = s.read(own).unwrap().version;
    // run_tool_with
    s.run_tool_with(
        bot,
        ROOT,
        &argv(&["append"]),
        b"two\n",
        &ToolOptions::new().turn(t("t:9c41e07ab2d35f68.11")),
    )
    .unwrap();
    // edit_with_options
    s.edit_with_options(
        bot,
        ROOT,
        &EditOptions::new().turn(t("t:9c41e07ab2d35f68.12")),
        |live| Ok::<_, std::convert::Infallible>(format!("{live}three\n")),
    )
    .unwrap()
    .unwrap();
    // replace_text_with_options
    s.replace_text_with_options(
        bot,
        ROOT,
        "one\ntwo\nthree\nfour\n",
        &WriteOptions::new().turn(t("t:9c41e07ab2d35f68.13")),
    )
    .unwrap();
    // write_base_with_options
    let base = s.read(bot).unwrap().version;
    let out = s
        .write_base_with_options(
            bot,
            ROOT,
            base,
            "one\ntwo\nthree\nfour\nfive\n",
            &WriteOptions::new().turn(t("t:9c41e07ab2d35f68.14")),
        )
        .unwrap();
    assert!(matches!(out.outcome, WriteOutcome::Applied(_)));
    // undo_with and redo_with
    s.undo_with(bot, &UndoRequest::steps(1).turn(t("t:9c41e07ab2d35f68.15")))
        .unwrap();
    s.redo_with(bot, &UndoRequest::steps(1).turn(t("t:9c41e07ab2d35f68.16")))
        .unwrap();
    // an agent's call with no turn, and a person's
    s.replace_text(bot, ROOT, "one\ntwo\nthree\nfour\nfive\nsix\n")
        .unwrap();
    s.replace_text(own, ROOT, "one\ntwo\nthree\nfour\nfive\nsix\nseven\n")
        .unwrap();
    let want: Vec<Option<String>> = [
        Some("t:9c41e07ab2d35f68.11"),
        Some("t:9c41e07ab2d35f68.12"),
        Some("t:9c41e07ab2d35f68.13"),
        Some("t:9c41e07ab2d35f68.14"),
        Some("t:9c41e07ab2d35f68.15"),
        Some("t:9c41e07ab2d35f68.16"),
        None,
        None,
    ]
    .iter()
    .map(|x| x.map(str::to_owned))
    .collect();
    let events = drain(&rx);
    let got: Vec<Option<String>> = events
        .iter()
        .map(|e| e.turn.map(|t| t.to_string()))
        .collect();
    assert_eq!(got, want, "change events, one per change");
    let window: Vec<Option<String>> = s
        .events_since(own, v0)
        .unwrap()
        .iter()
        .map(|e| e.turn.map(|t| t.to_string()))
        .collect();
    assert_eq!(window, want, "the live window (events_since)");
    let logged: Vec<Option<String>> = log_turns(&s, own).into_iter().map(|x| x.1).collect();
    assert_eq!(logged[logged.len() - want.len()..], want[..], "the log");
    // the store keeps them: a reload reads the same
    drop(s);
    f.mgr.close(&key("a.md"), "test").unwrap();
    let s = f.text("a.md", "");
    let own = join(&s, person("own"));
    let logged: Vec<Option<String>> = log_turns(&s, own).into_iter().map(|x| x.1).collect();
    assert_eq!(
        logged[logged.len() - want.len()..],
        want[..],
        "the log after a reload"
    );
}

/// A keyed doc's writes (ops by key, and their structured undo) carry the turn too.
#[test]
fn keyed_writes_and_their_undo_carry_the_turn() {
    let f = fx("keyed");
    let s = f.keyed();
    let own = join(&s, person("own"));
    let bot = join(&s, agent("atlas"));
    let rx = s.subscribe(own).unwrap();
    let base = s.read(bot).unwrap().version;
    s.edit_ops(
        bot,
        vec![Edit::List {
            root: ITEMS.into(),
            ops: vec![ListOp::Delete {
                key: Key::new("i2").unwrap(),
            }],
        }],
        &EditOptions::new()
            .base(base)
            .turn(t("t:9c41e07ab2d35f68.21")),
    )
    .unwrap();
    s.undo_with(bot, &UndoRequest::steps(1).turn(t("t:9c41e07ab2d35f68.22")))
        .unwrap();
    let got: Vec<Option<String>> = drain(&rx)
        .iter()
        .map(|e| e.turn.map(|t| t.to_string()))
        .collect();
    assert!(!got.is_empty());
    assert!(
        got.iter()
            .all(|x| x.as_deref() == Some("t:9c41e07ab2d35f68.21")
                || x.as_deref() == Some("t:9c41e07ab2d35f68.22")),
        "every event of the two changes carries its call's turn: {got:?}"
    );
    let logged: Vec<Option<String>> = log_turns(&s, own).into_iter().map(|x| x.1).collect();
    assert_eq!(
        logged[logged.len() - 2..],
        [
            Some("t:9c41e07ab2d35f68.21".to_owned()),
            Some("t:9c41e07ab2d35f68.22".to_owned())
        ]
    );
}

/// The check behind the erasure rows: atlas wrote with a turn, nova wrote with one on the
/// owner's behalf... Erasing atlas clears atlas's turns and keeps nova's; erasing the person
/// nova acted for clears nova's. `Err` names the first turn left where it shouldn't be.
fn live_erasure_clears_turns() -> Result<(), String> {
    let f = fx("erase");
    let s = f.text("e.md", "a\n");
    let own = join(&s, person("own"));
    let atlas = join(&s, agent("atlas"));
    let nova = join(
        &s,
        Participant::new("nova", "Nova", "#b0577a", ParticipantKind::Agent, "cli")
            .on_behalf_of("ada"),
    );
    s.replace_text_with_options(atlas, ROOT, "a\nb\n", &WriteOptions::new().turn(t("t:a.1")))
        .unwrap();
    s.replace_text_with_options(
        nova,
        ROOT,
        "a\nb\nc\n",
        &WriteOptions::new().turn(t("t:n.1")),
    )
    .unwrap();
    let before = log_turns(&s, own);
    let with: Vec<&str> = before.iter().filter_map(|x| x.1.as_deref()).collect();
    assert_eq!(with, ["t:a.1", "t:n.1"], "both turns are logged before");
    let erase = |pid: &str, ps: &str| {
        s.purge(
            own,
            PurgeRequest::new(PurgeScope::Participant {
                pid: pid.into(),
                pseudonym: ps.into(),
            }),
        )
        .unwrap();
    };
    erase("atlas", "erased-1");
    let after = log_turns(&s, own);
    let left: Vec<&str> = after.iter().filter_map(|x| x.1.as_deref()).collect();
    if left != ["t:n.1"] {
        return Err(format!(
            "after erasing atlas: turns {left:?}, want [\"t:n.1\"]"
        ));
    }
    erase("ada", "erased-2");
    let after = log_turns(&s, own);
    let left: Vec<&str> = after.iter().filter_map(|x| x.1.as_deref()).collect();
    if !left.is_empty() {
        return Err(format!(
            "after erasing ada (nova wrote on her behalf): turns {left:?}, want none"
        ));
    }
    Ok(())
}

#[test]
fn erasure_clears_the_turns_of_the_erased_participants_changes() {
    live_erasure_clears_turns().unwrap();
    blob_erasure_clears_turns().unwrap();
}

/// The planted fault `TurnNotClearedOnErase`: both checks fail, at the erasure.
#[test]
fn plant_turn_not_cleared_on_erase_is_caught() {
    let live = with_contract_mutant(
        ContractMutant::TurnNotClearedOnErase,
        live_erasure_clears_turns,
    )
    .expect_err("the live check passes with the turn left after erasure");
    assert!(live.starts_with("after erasing atlas"), "{live}");
    let blob = with_contract_mutant(
        ContractMutant::TurnNotClearedOnErase,
        blob_erasure_clears_turns,
    )
    .expect_err("the tier-0 check passes with the turn left after erasure");
    assert!(blob.starts_with("after erasing atlas"), "{blob}");
}

/// Tier 0: `write_bytes_with` and `restore_with` record the turn with the version (`versions`,
/// `log`); a write with none records none; erasing the writer clears it.
fn blob_erasure_clears_turns() -> Result<(), String> {
    let f = fx("blob");
    let s = f.blob("deck.bin", b"v1");
    let own = s.join(person("own"), Stay::Held).unwrap().conn;
    let atlas = s.join(agent("atlas"), Stay::Held).unwrap().conn;
    let v1 = s.status().unwrap().version;
    let v2 = s
        .write_bytes_with(atlas, b"v2", v1, &BlobWriteOptions::new().turn(t("t:a.7")))
        .unwrap()
        .version;
    let v3 = s
        .restore_with(atlas, v1, &BlobWriteOptions::new().turn(t("t:a.8")))
        .unwrap()
        .version;
    let v4 = s.write_bytes(own, b"v4", v3).unwrap().version;
    let turns = |s: &BlobSession| -> HashMap<u64, Option<String>> {
        s.versions(own)
            .unwrap()
            .into_iter()
            .map(|(v, b)| (v, b.turn.map(|t| t.to_string())))
            .collect()
    };
    let got = turns(&s);
    assert_eq!(got[&v2].as_deref(), Some("t:a.7"));
    assert_eq!(got[&v3].as_deref(), Some("t:a.8"));
    assert_eq!(got[&v4], None, "a person's write has no turn");
    assert_eq!(got[&v1], None, "the import has no turn");
    let logged: HashMap<u64, Option<String>> = s
        .log(own)
        .unwrap()
        .into_iter()
        .map(|e| (e.id, e.turn.map(|t| t.to_string())))
        .collect();
    assert_eq!(logged, got, "log says what versions says");
    s.erase_participant(own, "atlas", "erased-1").unwrap();
    let left: Vec<String> = turns(&s).into_values().flatten().collect();
    if !left.is_empty() {
        return Err(format!("after erasing atlas: turns {left:?}, want none"));
    }
    Ok(())
}

/// A turn the host gives is checked when it is made; the kit never makes one up, and a call
/// with none leaves none (the default of every option).
#[test]
fn a_turn_is_the_hosts_and_none_is_the_default() {
    assert!(Turn::new("").is_err());
    assert!(Turn::new(&"x".repeat(65)).is_err());
    assert!(Turn::new("has space").is_err());
    assert_eq!(ToolOptions::new().turn, None);
    assert_eq!(EditOptions::new().turn, None);
    assert_eq!(WriteOptions::new().turn, None);
    assert_eq!(UndoRequest::steps(1).turn, None);
    assert_eq!(BlobWriteOptions::new().turn, None);
    let _ = &fx("none").dir;
}

// ---------- live revert and restore take the host's turn (K2's ask) ----------

/// The turn of change `seq` on `events` and in the log of `s`, as `(event, log)`; `Err` when
/// either has no such change.
fn turn_of(
    s: &Session,
    c: ConnId,
    events: &[ChangeEvent],
    seq: u64,
) -> Result<(Option<String>, Option<String>), String> {
    let e = events
        .iter()
        .find(|e| e.version == seq)
        .ok_or_else(|| format!("no event for change {seq}"))?;
    let l = log_turns(s, c)
        .into_iter()
        .find(|(x, _)| *x == seq)
        .ok_or_else(|| format!("change {seq} isn't in the log"))?;
    Ok((e.turn.map(|t| t.to_string()), l.1))
}

/// `what` made change `seq` with `want` as its turn on its event and in the log, or `Err` saying
/// where it doesn't.
fn check_turn(
    s: &Session,
    c: ConnId,
    events: &[ChangeEvent],
    seq: u64,
    what: &str,
    want: Option<&str>,
) -> Result<(), String> {
    let (event, log) = turn_of(s, c, events, seq)?;
    let want = want.map(str::to_owned);
    if event != want || log != want {
        return Err(format!(
            "{what} (change {seq}): turn {event:?} on its event and {log:?} in the log, want {want:?}"
        ));
    }
    Ok(())
}

/// The check behind the revert and restore rows: `revert_with` and `restore_with` carry the
/// host's turn onto the change they make, on a text doc and on a keyed doc (each kind's own
/// path); `revert` and `restore` carry none. `Err` names the first one whose turn is wrong.
fn reverts_and_restores_carry_the_turn() -> Result<(), String> {
    let f = fx("revert");
    // a text doc
    let s = f.text("r.md", "a\n");
    let own = join(&s, person("own"));
    let bot = join(&s, agent("atlas"));
    let rx = s.subscribe(own).unwrap();
    let vid = s.name_version(own, "start", None).unwrap();
    s.replace_text(bot, ROOT, "a\nb\n").unwrap();
    let wrote = s.status().unwrap().version;
    let out = s
        .revert_with(bot, wrote, false, &HistoryOptions::new().turn(t("t:r.1")))
        .unwrap();
    assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
    let reverted = s.status().unwrap().version;
    s.replace_text(bot, ROOT, "a\nc\n").unwrap();
    s.restore_with(bot, vid, &HistoryOptions::new().turn(t("t:r.2")))
        .unwrap();
    let restored = s.status().unwrap().version;
    s.replace_text(bot, ROOT, "a\nd\n").unwrap();
    let again = s.status().unwrap().version;
    s.revert(bot, again, false).unwrap();
    let plain_revert = s.status().unwrap().version;
    s.replace_text(bot, ROOT, "a\ne\n").unwrap();
    s.restore(bot, vid).unwrap();
    let plain_restore = s.status().unwrap().version;
    let events = drain(&rx);
    check_turn(&s, own, &events, reverted, "the revert", Some("t:r.1"))?;
    check_turn(&s, own, &events, restored, "the restore", Some("t:r.2"))?;
    check_turn(
        &s,
        own,
        &events,
        plain_revert,
        "a revert with no turn",
        None,
    )?;
    check_turn(
        &s,
        own,
        &events,
        plain_restore,
        "a restore with no turn",
        None,
    )?;
    // a keyed doc: the structured revert and restore
    let s = f.keyed();
    let own = join(&s, person("own"));
    let bot = join(&s, agent("atlas"));
    let rx = s.subscribe(own).unwrap();
    let vid = s.name_version(own, "start", None).unwrap();
    let base = s.read(bot).unwrap().version;
    let deleted = s
        .edit_ops(
            bot,
            vec![Edit::List {
                root: ITEMS.into(),
                ops: vec![ListOp::Delete {
                    key: Key::new("i2").unwrap(),
                }],
            }],
            &EditOptions::new().base(base),
        )
        .unwrap()
        .version;
    let out = s
        .revert_with(bot, deleted, false, &HistoryOptions::new().turn(t("t:r.3")))
        .unwrap();
    assert!(matches!(out, UndoOutcome::Applied { .. }), "{out:?}");
    let reverted = s.status().unwrap().version;
    let base = s.read(bot).unwrap().version;
    s.edit_ops(
        bot,
        vec![Edit::List {
            root: ITEMS.into(),
            ops: vec![ListOp::Delete {
                key: Key::new("i3").unwrap(),
            }],
        }],
        &EditOptions::new().base(base),
    )
    .unwrap();
    s.restore_with(bot, vid, &HistoryOptions::new().turn(t("t:r.4")))
        .unwrap();
    let restored = s.status().unwrap().version;
    let events = drain(&rx);
    check_turn(
        &s,
        own,
        &events,
        reverted,
        "the revert of a keyed change",
        Some("t:r.3"),
    )?;
    check_turn(
        &s,
        own,
        &events,
        restored,
        "the restore of a keyed doc",
        Some("t:r.4"),
    )?;
    Ok(())
}

#[test]
fn live_revert_and_restore_take_the_hosts_turn() {
    reverts_and_restores_carry_the_turn().unwrap();
    assert_eq!(HistoryOptions::new().turn, None);
}

/// The planted fault `RevertDropsTurn`: the revert's change has no turn.
#[test]
fn plant_revert_drops_the_turn_is_caught() {
    let e = with_contract_mutant(
        ContractMutant::RevertDropsTurn,
        reverts_and_restores_carry_the_turn,
    )
    .expect_err("the check passes with the revert's turn dropped");
    assert!(e.starts_with("the revert (change"), "{e}");
}
