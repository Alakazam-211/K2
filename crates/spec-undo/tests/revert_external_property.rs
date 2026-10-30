//! Outside saves against the oracle, by root (v0.9.7.2; the fault: a revert of an outside save
//! put back fresh text, `crates/base/tests/revert_external.rs`). Seeded scripts through a real
//! session on a file: agents insert and delete lines, undo and redo, an outside editor saves a
//! whole-text replacement through the kit's real ingest (trimming in play), the owner reverts a
//! change (an outside save among them, all or `--partial`) and undoes and redoes the revert,
//! restores a version, plants and purges a secret, and the session restarts. The same calls go
//! to `spec_undo::model` (an edit and a save with the hunks the kit reports). After every step:
//! the same text; each live character's root names the same change on both sides, is a
//! restoration on both or neither, and was put back by the same participant; blame's
//! `restored_by` of every line is that participant. `REVEXT_SCRIPTS` scripts (default 300) of
//! `REVEXT_STEPS` steps (default 40).
//!
//! Not the trace generator's outside-save step (a follow-up): this is its own small script.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

use k2_mpc_base::differential::{with_prov_mutant, ProvMutant};
use k2_mpc_base::session::{
    AuditEvent, ChangeEvent, Config, ConnId, Host, Participant, ParticipantKind, PurgeRequest,
    PurgeScope, Rights, Session, SessionManager, Stay, TextFilePack,
};
use k2_mpc_base::store::{DocKey, DocStore};
use rand::rngs::SmallRng;
use rand::{Rng, SeedableRng};
use spec_undo::model::{Bounds, Edit, Mode, Model, Request};

const ROOT: &str = TextFilePack::ROOT;
const PIDS: [&str; 3] = ["a0", "a1", "olive"];

struct H(AtomicU64);
impl Host for H {
    fn rights(&self, who: &Participant, _: &DocKey) -> Rights {
        let mut r = Rights::full();
        r.owner = who.pid == "olive";
        r
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn config(b: &Bounds) -> Config {
    let mut c = Config::default();
    c.edit_burst = 1_000_000;
    c.edit_rate_per_s = 1_000_000;
    c.undo_depth = b.depth;
    c.revert_keep_changes = b.keep_changes;
    c.revert_keep_ms = 0;
    c.history_tick_ms = 0;
    c
}

struct Kit {
    dir: std::path::PathBuf,
    file: std::path::PathBuf,
    mgr: Option<SessionManager>,
    s: Option<Arc<Session>>,
    conns: HashMap<&'static str, ConnId>,
    events: Option<Receiver<ChangeEvent>>,
    bounds: Bounds,
}

impl Kit {
    fn open(seed: u64, text: &str, bounds: Bounds) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "k2mpc-revext-{}-{seed}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("ws")).unwrap();
        let file = dir.join("ws/doc.md");
        std::fs::write(&file, text).unwrap();
        let mut k = Kit {
            dir,
            file,
            mgr: None,
            s: None,
            conns: HashMap::new(),
            events: None,
            bounds,
        };
        k.reopen();
        k
    }

    fn reopen(&mut self) {
        let store = Arc::new(DocStore::open(self.dir.join("docs.redb")).unwrap());
        let mgr = SessionManager::new(
            store,
            Arc::new(H(AtomicU64::new(1_000_000))),
            config(&self.bounds),
        );
        let key = DocKey {
            space: "ws".into(),
            path: "doc.md".into(),
        };
        let s = mgr
            .open_file(
                Arc::new(TextFilePack::new("markdown", 4 << 20)),
                &key,
                &self.file,
                &self.dir.join("ws"),
            )
            .unwrap();
        self.conns.clear();
        for p in PIDS {
            let c = s
                .join(
                    Participant::new(p, p, "#1", ParticipantKind::Agent, "cli"),
                    Stay::Held,
                )
                .unwrap()
                .conn;
            self.conns.insert(p, c);
        }
        self.events = Some(s.subscribe(self.conns["olive"]).unwrap());
        self.s = Some(s);
        self.mgr = Some(mgr);
    }

    fn restart(&mut self) {
        self.events = None;
        drop(self.s.take());
        let mgr = self.mgr.take().unwrap();
        mgr.release().unwrap();
        drop(mgr);
        self.reopen();
    }

    fn s(&self) -> &Session {
        self.s.as_ref().unwrap()
    }

    fn c(&self, p: &str) -> ConnId {
        self.conns[p]
    }

    fn text(&self) -> String {
        self.s().read(self.c("olive")).unwrap().text_by_root[0]
            .1
            .clone()
    }

    fn version(&self) -> u64 {
        self.s().read(self.c("olive")).unwrap().version
    }

    /// The hunks of the changes since the last call (one per change, as the channel reports).
    fn drain(&self) -> Vec<ChangeEvent> {
        self.events.as_ref().unwrap().try_iter().collect()
    }

    /// Each live character: (char, its root's change, a restoration, who put it back).
    fn chars(&self) -> Vec<(char, Option<u64>, bool, Option<String>)> {
        self.s()
            .char_blame(ROOT)
            .unwrap()
            .into_iter()
            .map(|c| {
                let restored = (c.client, c.clock) != (c.root_client, c.root_clock);
                (
                    c.ch,
                    c.root_change,
                    restored,
                    restored.then(|| c.pid.clone()).flatten(),
                )
            })
            .collect()
    }

    /// Blame's `restored_by` per line.
    fn restored_by(&self) -> Vec<Option<String>> {
        let b = self.s().blame(self.c("olive"), ROOT, None).unwrap();
        b.lines.iter().map(|l| l.restored_by.clone()).collect()
    }

    fn finish(mut self) {
        self.events = None;
        drop(self.s.take());
        if let Some(m) = self.mgr.take() {
            m.release().unwrap();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Blame's `restored_by` of each line, read off the oracle by blame's rule: the character with
/// the newest root change; on a tie a restored one, the most recently restored, then the first.
fn model_restored_by(m: &Model) -> Vec<Option<String>> {
    let mut out = Vec::new();
    let mut line: Vec<(u64, Option<u64>, Option<String>)> = Vec::new();
    let pick = |line: &[(u64, Option<u64>, Option<String>)]| -> Option<String> {
        let mut best: Option<&(u64, Option<u64>, Option<String>)> = None;
        for c in line {
            if best.map_or(true, |b| (c.0, c.1) > (b.0, b.1)) {
                best = Some(c);
            }
        }
        best.and_then(|b| b.2.clone())
    };
    for (ch, x) in m.texts[0].chars().zip(&m.elems[0]) {
        let restored = x.id != x.prov;
        line.push((
            x.prov.0,
            restored.then_some(x.id.0),
            restored.then(|| m.who[x.owner as usize].clone()),
        ));
        if ch == '\n' {
            out.push(pick(&line));
            line.clear();
        }
    }
    if !line.is_empty() {
        out.push(pick(&line));
    }
    out
}

fn model_chars(m: &Model) -> Vec<(char, u64, bool, Option<String>)> {
    m.texts[0]
        .chars()
        .zip(&m.elems[0])
        .map(|(ch, x)| {
            let restored = x.id != x.prov;
            (
                ch,
                x.prov.0,
                restored,
                restored.then(|| m.who[x.owner as usize].clone()),
            )
        })
        .collect()
}

fn lines_of(t: &str) -> Vec<String> {
    t.lines().map(str::to_owned).collect()
}

/// One script; `Err` names the first step where the kit and the oracle differ.
fn script(seed: u64, steps: usize) -> Result<(), String> {
    let mut rng = SmallRng::seed_from_u64(seed);
    let bounds = Bounds::default();
    let seed_text = "s0\ns1\ns2\n".to_owned();
    let mut kit = Kit::open(seed, &seed_text, bounds);
    let mut m = Model::new(vec![seed_text.clone()], kit.version(), bounds);
    let mut log: Vec<String> = Vec::new();
    let mut next = 0u32;
    let mut secrets: Vec<String> = Vec::new();
    // root-change names: the kit's change ids and the oracle's, matched on first sight
    let mut k2m: HashMap<Option<u64>, u64> = HashMap::new();
    let mut m2k: HashMap<u64, Option<u64>> = HashMap::new();
    let result = (|| -> Result<(), String> {
        for i in 0..steps {
            let p = PIDS[rng.gen_range(0..2)];
            let text = kit.text();
            let lines = lines_of(&text);
            let pick = rng.gen_range(0..100);
            let what: String;
            // an edit of the kit's goes to the oracle with the hunks the kit reports
            let feed = |kit: &Kit, m: &mut Model, pid: &str| {
                for ev in kit.drain() {
                    let es: Vec<Edit> = ev
                        .hunks
                        .iter()
                        .map(|h| Edit {
                            start: h.start,
                            end: h.end,
                            insert: h.insert.clone(),
                        })
                        .collect();
                    m.edit(pid, None, vec![(0, es)]);
                }
            };
            match pick {
                0..=24 => {
                    next += 1;
                    let at = rng.gen_range(1..=lines.len() + 1).min(lines.len().max(1));
                    let argv: Vec<String> = ["insert", "--line", &at.to_string()]
                        .iter()
                        .map(|a| (*a).to_owned())
                        .collect();
                    let t = format!("{p}:{next}\n");
                    let _ = kit.s().run_tool(kit.c(p), ROOT, &argv, t.as_bytes());
                    feed(&kit, &mut m, p);
                    what = format!("{p} inserts {t:?} at line {at}");
                }
                25..=36 if !lines.is_empty() => {
                    let n = rng.gen_range(1..=lines.len());
                    let argv: Vec<String> = ["delete", "--lines", &format!("{n},{n}")]
                        .iter()
                        .map(|a| (*a).to_owned())
                        .collect();
                    let _ = kit.s().run_tool(kit.c(p), ROOT, &argv, b"");
                    feed(&kit, &mut m, p);
                    what = format!("{p} deletes line {n}");
                }
                37..=46 => {
                    let r = kit.s().undo(kit.c(p), 1);
                    m.undo(p, None, &Request::new(1, Mode::All));
                    kit.drain();
                    what = format!("{p} undoes: {r:?}");
                }
                47..=52 => {
                    let r = kit.s().redo(kit.c(p), 1);
                    m.redo(p, None, &Request::new(1, Mode::All));
                    kit.drain();
                    what = format!("{p} redoes: {r:?}");
                }
                53..=66 => {
                    // a whole-text replacement from the file the kit wrote
                    kit.s().flush(kit.c("olive")).unwrap();
                    let mut ls = lines.clone();
                    for _ in 0..rng.gen_range(1..=2) {
                        if !ls.is_empty() && rng.gen_bool(0.6) {
                            ls.remove(rng.gen_range(0..ls.len()));
                        } else {
                            next += 1;
                            ls.insert(rng.gen_range(0..=ls.len()), format!("ed:{next}"));
                        }
                    }
                    let new: String = ls.iter().map(|l| format!("{l}\n")).collect();
                    std::fs::write(&kit.file, &new).unwrap();
                    let r = kit.s().file_changed().unwrap();
                    feed(&kit, &mut m, "kit:external");
                    what = format!("outside save {new:?}: {r:?}");
                }
                67..=78 => {
                    // the owner reverts a recent change, then sometimes undoes and redoes it
                    let head = kit.version();
                    let ch = rng.gen_range(head.saturating_sub(6).max(2)..=head.max(2));
                    let partial = rng.gen_bool(0.3);
                    let r = kit.s().revert(kit.c("olive"), ch, partial);
                    m.revert("olive", None, ch, partial);
                    kit.drain();
                    what = format!("olive reverts change {ch} (partial {partial}): {r:?}");
                    if rng.gen_bool(0.5) {
                        let u = kit.s().undo(kit.c("olive"), 1);
                        m.undo("olive", None, &Request::new(1, Mode::All));
                        kit.drain();
                        log.push(format!("{i}: {what}"));
                        compare(&kit, &m, &mut k2m, &mut m2k)
                            .map_err(|e| format!("step {i}, the revert: {e}"))?;
                        let rd = kit.s().redo(kit.c("olive"), 1);
                        m.redo("olive", None, &Request::new(1, Mode::All));
                        kit.drain();
                        log.push(format!("{i}: olive undoes it: {u:?}, redoes it: {rd:?}"));
                        compare(&kit, &m, &mut k2m, &mut m2k)
                            .map_err(|e| format!("step {i}, after undo and redo: {e}"))?;
                        continue;
                    }
                }
                79..=85 => {
                    let vs = kit.s().versions(kit.c("olive")).unwrap();
                    let v = vs[rng.gen_range(0..vs.len())].id;
                    let t = kit.s().show(kit.c("olive"), v).unwrap()[0].1.clone();
                    match kit.s().restore(kit.c("olive"), v) {
                        Ok(_) => {
                            m.restore("olive", None, std::slice::from_ref(&t));
                            what = format!("olive restores v{v}");
                        }
                        Err(e) => what = format!("olive restores v{v}: refused {e:?}"),
                    }
                    kit.drain();
                }
                86..=90 => {
                    next += 1;
                    let sec = format!("sec:{next:08x}");
                    let argv: Vec<String> = ["insert", "--line", "1"]
                        .iter()
                        .map(|a| (*a).to_owned())
                        .collect();
                    let _ = kit
                        .s()
                        .run_tool(kit.c(p), ROOT, &argv, format!("{sec}\n").as_bytes());
                    feed(&kit, &mut m, p);
                    secrets.push(sec.clone());
                    what = format!("{p} plants {sec}");
                }
                91..=94 if !secrets.is_empty() => {
                    let sec = secrets.swap_remove(rng.gen_range(0..secrets.len()));
                    let r = kit.s().purge(
                        kit.c("olive"),
                        PurgeRequest::new(PurgeScope::Text(format!("{sec}\n"))).and_remove(),
                    );
                    m.purge_text("olive", None, &format!("{sec}\n"));
                    kit.drain();
                    what = format!("olive purges {sec}: {}", r.is_ok());
                }
                95..=99 => {
                    kit.restart();
                    m.restart();
                    what = "restart".into();
                }
                _ => continue,
            }
            log.push(format!("{i}: {what}"));
            compare(&kit, &m, &mut k2m, &mut m2k).map_err(|e| format!("step {i}: {e}"))?;
        }
        Ok(())
    })();
    kit.finish();
    result.map_err(|e| {
        let tail: Vec<&String> = log.iter().rev().take(12).rev().collect();
        format!(
            "seed {seed}: {e}\n  {}",
            tail.iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
                .join("\n  ")
        )
    })
}

fn compare(
    kit: &Kit,
    m: &Model,
    k2m: &mut HashMap<Option<u64>, u64>,
    m2k: &mut HashMap<u64, Option<u64>>,
) -> Result<(), String> {
    let text = kit.text();
    if kit.version() != m.head {
        return Err(format!(
            "change numbering: kit v{}, oracle {} ({text:?})",
            kit.version(),
            m.head
        ));
    }
    if text != m.texts[0] {
        return Err(format!("text: kit {text:?}, oracle {:?}", m.texts[0]));
    }
    let kc = kit.chars();
    let mc = model_chars(m);
    for (k, ((ch, kroot, kres, kby), (_, mroot, mres, mby))) in kc.iter().zip(&mc).enumerate() {
        let a = *k2m.entry(*kroot).or_insert(*mroot);
        let b = *m2k.entry(*mroot).or_insert(*kroot);
        if a != *mroot || b != *kroot {
            return Err(format!(
                "char {k} {ch:?}: root's change: kit {kroot:?}, oracle {mroot} (U23)"
            ));
        }
        if kres != mres || kby != mby {
            return Err(format!(
                "char {k} {ch:?}: put back: kit {kres} by {kby:?}, oracle {mres} by {mby:?}"
            ));
        }
    }
    // blame's `restored_by`, by blame's rule (`model_restored_by`)
    let line_by = model_restored_by(m);
    let kb = kit.restored_by();
    if kb != line_by {
        return Err(format!(
            "blame's restored_by: kit {kb:?}, oracle {line_by:?}"
        ));
    }
    Ok(())
}

fn n(var: &str, default: u64) -> u64 {
    std::env::var(var)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[test]
fn outside_saves_agree_with_the_oracle_by_root() {
    let (scripts, steps) = (n("REVEXT_SCRIPTS", 300), n("REVEXT_STEPS", 40) as usize);
    let mut bad = Vec::new();
    for seed in 0..scripts {
        if let Err(e) = script(seed, steps) {
            bad.push(e);
        }
    }
    eprintln!(
        "[revext] {scripts} scripts of {steps} steps: {} disagree",
        bad.len()
    );
    for e in bad.iter().take(3) {
        eprintln!("[revext] {e}");
    }
    assert!(
        bad.is_empty(),
        "{} of {scripts} scripts disagree",
        bad.len()
    );
}

/// The planted fault (`ProvMutant::OutsideSaveKeepsNoRoots`): some script disagrees.
#[test]
fn the_property_catches_the_dropped_roots() {
    let caught = (0..n("REVEXT_SCRIPTS", 300)).find_map(|seed| {
        with_prov_mutant(ProvMutant::OutsideSaveKeepsNoRoots, || {
            script(seed, 40).err()
        })
    });
    let e = caught.expect("the planted fault went unseen");
    eprintln!("[revext] the mutant, caught: {e}");
}
