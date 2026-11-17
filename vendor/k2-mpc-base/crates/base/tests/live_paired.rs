//! The carrier's pairing (`docs/SESSIONS.md`, "The carrier's pairing";
//! `docs/design/carrier-synced-content-r0.md`): for a pack that keeps a carrier and **mints** its
//! keys, the kit keeps the carrier's keys (a `Pairing`, in the order the pack's decode yields the
//! units) with every carrier, live and per version, and hands it to `Pack::encode_from_paired`.
//!
//! - The pack writes back exactly after a live insert, an app's save, a restart and a purge
//!   followed by a verb (and a purge with the file missing, then the file back).
//! - A version cut between two syncs is spliced with its own pairing: `file_at` is exact.
//! - A pairing that doesn't fit is refused typed (`PairingMismatch`, `MPC-E521`), the file as it
//!   was; never a splice by guess.
//! - N reads make no pairing (the stored one is used after a restart).
//! - Erasure: a byte scan of the store and the blob folder finds nothing after a purge.
//! - Whether the doc holds an outside save is decided with no rendering.
//! - An outside save on unwritten changes keeps both, and the save is the live doc exactly.
//! - A save the pack re-keys (its keys follow the file, as a workbook's rows do) is the carrier,
//!   with its own pairing, before anything renders the doc that took it: no `MPC-E521` on the
//!   reads after it, the doc clean, writes and `file_at` byte for byte; with unwritten changes
//!   too (both kept).
//!
//! Plants (`KeyedMutant`): `SyncedContentStale`, `SpliceWithoutMatch`, `PairingMissingOnVersion`,
//! `HoldsFileCallsPack`, `DirtySaveMergedByKey`, `SettleBeforeCarrier`, each caught by its row,
//! by name.
//!
//! The test pack, `rows-minted`: one line per row, **with no id**; a line starting with `#` is
//! opaque (kept after the row it follows); a row's value is its text trimmed, so the padding
//! around it is bytes only the carrier holds. `decode` mints `r1..rn` in file order; `encode`
//! writes each row bare. `encode_from` (carrier only) pairs by the keys `decode` mints, which is
//! wrong once a live insert or an app's save moved the rows: the scaffold's kit and the plant
//! `SpliceWithoutMatch` go through it. `encode_from_paired` puts the pairing's keys on the
//! carrier's rows in order and splices: an untouched row keeps its line and the opaque lines
//! after it; a changed or new one is written bare; a deleted one goes with its opaque lines.
//!
//! With `rekeys` set, the pack re-keys on an app's save that changes the rows (as a pack whose
//! keys follow the file's positions): every row of the save gets a key of that save, `s<g>.<n>`
//! (the open's are `r<n>`, save 0), and `encode_from_paired` refuses a pairing from an earlier
//! save than a key the doc holds (`PairingMismatch`): the carrier's keys are gone from the doc.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::differential::{self, KeyedMutant};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Anchor, Key, ListOp, Value};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, DiskCheck, DiskState, EditOptions, Host, Pack,
    PackContent, PackError, Pairing, Participant, ParticipantKind, PurgeRequest, PurgeScope,
    Reconciled, Rights, Session, SessionError, SessionManager, SpliceError, Stay, View,
};
use k2_mpc_base::store::{scan_file, DocKey, DocStore, VersionId, VersionReason};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError};

// ---------- the test pack ----------

const ROWS: &str = "rows";

const SCHEMA: Schema = Schema {
    name: "rows-minted",
    roots: &[(ROWS, Kind::MovableList)],
};

#[derive(Default)]
struct RowsPack {
    /// `Pack::pairing` calls.
    pairings: AtomicU64,
    /// `encode_from_paired` calls.
    paired: AtomicU64,
    /// `encode_from` (carrier-only) calls.
    blind: AtomicU64,
    /// While set, `pairing` leaves out the last key (a pack bug: a pairing that doesn't fit).
    bad_pairing: AtomicBool,
    /// While set, an app's save that changes the rows is re-keyed whole (`s<g>.<n>`), and a
    /// pairing older than the doc's keys is refused.
    rekeys: AtomicBool,
    /// The same file type as an older kit's pack has it: it keeps a carrier but no pairing, and
    /// writes the file with its own `encode`.
    older: bool,
}

impl RowsPack {
    fn pairings(&self) -> u64 {
        self.pairings.load(Ordering::SeqCst)
    }
    fn paired(&self) -> u64 {
        self.paired.load(Ordering::SeqCst)
    }
}

/// A row of the file: its value (trimmed), its line, the opaque lines after it.
struct Unit<'a> {
    value: &'a str,
    raw: &'a str,
    after: Vec<&'a str>,
}

/// The opaque lines before the first row, then the rows.
fn parse(b: &[u8]) -> Result<(Vec<&str>, Vec<Unit<'_>>), String> {
    let s = std::str::from_utf8(b).map_err(|e| e.to_string())?;
    let mut lead = Vec::new();
    let mut units: Vec<Unit<'_>> = Vec::new();
    for raw in s.split_inclusive('\n') {
        if raw.starts_with('#') {
            match units.last_mut() {
                Some(u) => u.after.push(raw),
                None => lead.push(raw),
            }
            continue;
        }
        let value = raw.strip_suffix('\n').unwrap_or(raw).trim();
        if value.is_empty() {
            return Err("an empty row".into());
        }
        units.push(Unit {
            value,
            raw,
            after: Vec::new(),
        });
    }
    Ok((lead, units))
}

/// The save a pack-minted key comes from: `r<n>` the open's (0), `s<g>.<n>` the g-th re-keying
/// save's; a key minted live (any other) none.
fn save_of(k: &Key) -> Option<u64> {
    let s = k.as_str();
    if s.strip_prefix('r')
        .is_some_and(|n| n.parse::<u64>().is_ok())
    {
        return Some(0);
    }
    s.strip_prefix('s')?.split_once('.')?.0.parse().ok()
}

fn rows_of(c: &[(String, Neutral)]) -> Result<&[(Key, Value)], PackError> {
    match c {
        [(r, Neutral::List(l))] if r == ROWS => Ok(l),
        _ => Err(PackError("expects one list root `rows`".into())),
    }
}

fn val(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}

fn bare(v: &str) -> String {
    format!("{v}\n")
}

/// Splice `content` into `lead` and the carrier's `units`, each under the key `keys` gives it.
fn splice(lead: &[&str], units: &[Unit<'_>], keys: &[Key], content: &[(Key, Value)]) -> Vec<u8> {
    let at: HashMap<&Key, usize> = keys.iter().enumerate().map(|(i, k)| (k, i)).collect();
    let mut out: String = lead.concat();
    for (k, v) in content {
        match at.get(k).map(|i| &units[*i]) {
            Some(u) => {
                if u.value == val(v) {
                    out.push_str(u.raw);
                } else {
                    out.push_str(&bare(val(v)));
                }
                out.push_str(&u.after.concat());
            }
            None => out.push_str(&bare(val(v))),
        }
    }
    out.into_bytes()
}

impl Pack for RowsPack {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let (_, units) = parse(b).map_err(|m| Declined {
            code: "malformed",
            message: m,
        })?;
        let list = units
            .iter()
            .enumerate()
            .map(|(i, u)| {
                (
                    Key::new(format!("r{}", i + 1)).unwrap(),
                    Value::str(u.value),
                )
            })
            .collect();
        Ok(PackContent {
            roots: vec![(ROWS.into(), Neutral::List(list))],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        Ok(rows_of(c)?
            .iter()
            .map(|(_, v)| bare(val(v)))
            .collect::<String>()
            .into_bytes())
    }
    fn wants_carrier(&self) -> bool {
        true
    }
    /// Carrier only: pairs by the keys `decode` mints (r1, r2, ... in file order).
    fn encode_from(
        &self,
        carrier: &[u8],
        c: &[(String, Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        self.blind.fetch_add(1, Ordering::SeqCst);
        if self.older {
            return self.encode(c, &[1]);
        }
        let (lead, units) = parse(carrier).map_err(PackError)?;
        let keys: Vec<Key> = (1..=units.len())
            .map(|i| Key::new(format!("r{i}")).unwrap())
            .collect();
        Ok(splice(&lead, &units, &keys, rows_of(c)?))
    }
    fn wants_pairing(&self) -> bool {
        !self.older
    }
    fn pairing(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Pairing, PackError> {
        self.pairings.fetch_add(1, Ordering::SeqCst);
        let mut keys: Vec<Key> = rows_of(c)?.iter().map(|(k, _)| k.clone()).collect();
        if self.bad_pairing.load(Ordering::SeqCst) {
            keys.pop();
        }
        Ok(Pairing {
            roots: vec![(ROWS.into(), keys)],
        })
    }
    fn encode_from_paired(
        &self,
        carrier: &[u8],
        pairing: &Pairing,
        c: &[(String, Neutral)],
        _: &[u8],
    ) -> Result<Vec<u8>, SpliceError> {
        self.paired.fetch_add(1, Ordering::SeqCst);
        let (lead, units) = parse(carrier).map_err(PackError)?;
        let keys = pairing
            .keys(ROWS)
            .ok_or_else(|| SpliceError::PairingMismatch("no keys for `rows`".into()))?;
        if keys.len() != units.len() {
            return Err(SpliceError::PairingMismatch(format!(
                "{} keys for {} rows",
                keys.len(),
                units.len()
            )));
        }
        let rows = rows_of(c)?;
        if self.rekeys.load(Ordering::SeqCst) {
            // the carrier's keys are of one save: a key the doc holds from a later one means the
            // pairing names keys the doc doesn't hold any more
            let paired = keys.iter().filter_map(save_of).max().unwrap_or(0);
            let later = rows
                .iter()
                .find_map(|(k, _)| save_of(k).filter(|g| *g > paired).map(|g| (k, g)));
            if let Some((k, g)) = later {
                return Err(SpliceError::PairingMismatch(format!(
                    "row key {} is from save {g}, the pairing from save {paired}",
                    k.as_str()
                )));
            }
        }
        Ok(splice(&lead, &units, keys, rows))
    }
    /// Pairs the save's rows with the synced ones by value (a longest common run); a row only
    /// the save has is new, under a fresh key `o<n>`.
    fn reconcile(&self, synced: &View, bytes: &[u8], _: &[u8]) -> Result<Reconciled, Declined> {
        let new = self.decode(bytes)?.roots;
        let Neutral::List(new) = &new[0].1 else {
            unreachable!("decode's root")
        };
        let old: &[(Key, Value)] = synced.list(ROWS).unwrap_or(&[]);
        if self.rekeys.load(Ordering::SeqCst) {
            return Ok(rekeyed(synced, old, new));
        }
        let mut next = 1 + old
            .iter()
            .map(|(k, _)| k)
            .chain(synced.hidden(ROWS).into_iter().flat_map(|h| h.keys()))
            .filter_map(|k| k.as_str().strip_prefix('o')?.parse::<u64>().ok())
            .max()
            .unwrap_or(0);
        // the longest common subsequence of the values
        let (n, m) = (old.len(), new.len());
        let mut t = vec![vec![0u32; m + 1]; n + 1];
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i][j] = if old[i].1 == new[j].1 {
                    t[i + 1][j + 1] + 1
                } else {
                    t[i + 1][j].max(t[i][j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        let mut kept: Vec<Option<Key>> = vec![None; m];
        while i < n && j < m {
            if old[i].1 == new[j].1 {
                kept[j] = Some(old[i].0.clone());
                i += 1;
                j += 1;
            } else if t[i + 1][j] >= t[i][j + 1] {
                i += 1;
            } else {
                j += 1;
            }
        }
        let keep: HashSet<&Key> = kept.iter().flatten().collect();
        let mut ops: Vec<ListOp> = old
            .iter()
            .filter(|(k, _)| !keep.contains(k))
            .map(|(k, _)| ListOp::Delete { key: k.clone() })
            .collect();
        let mut prev: Option<Key> = None;
        for (j, (_, v)) in new.iter().enumerate() {
            let k = match &kept[j] {
                Some(k) => k.clone(),
                None => {
                    let k = Key::new(format!("o{next}")).unwrap();
                    next += 1;
                    ops.push(ListOp::Insert {
                        key: k.clone(),
                        at: prev.clone().map_or(Anchor::Start, Anchor::After),
                        value: v.clone(),
                    });
                    k
                }
            };
            prev = Some(k);
        }
        let edits = match ops.is_empty() {
            true => Vec::new(),
            false => vec![Edit::List {
                root: ROWS.into(),
                ops,
            }],
        };
        Ok(Reconciled {
            edits,
            codec: vec![1],
            ..Reconciled::default()
        })
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
}

/// A re-keying pack's reconcile: no edit when the save's rows are the synced ones; else every
/// synced row goes and every row of the save comes under a key of a new save, `s<g>.<n>`.
fn rekeyed(synced: &View, old: &[(Key, Value)], new: &[(Key, Value)]) -> Reconciled {
    let same = old.len() == new.len() && old.iter().zip(new).all(|(a, b)| a.1 == b.1);
    if same {
        return Reconciled {
            codec: vec![1],
            ..Reconciled::default()
        };
    }
    let g = 1 + old
        .iter()
        .map(|(k, _)| k)
        .chain(synced.hidden(ROWS).into_iter().flat_map(|h| h.keys()))
        .filter_map(save_of)
        .max()
        .unwrap_or(0);
    let mut ops: Vec<ListOp> = old
        .iter()
        .map(|(k, _)| ListOp::Delete { key: k.clone() })
        .collect();
    let mut prev: Option<Key> = None;
    for (j, (_, v)) in new.iter().enumerate() {
        let k = Key::new(format!("s{g}.{}", j + 1)).unwrap();
        ops.push(ListOp::Insert {
            key: k.clone(),
            at: prev.take().map_or(Anchor::Start, Anchor::After),
            value: v.clone(),
        });
        prev = Some(k);
    }
    Reconciled {
        edits: vec![Edit::List {
            root: ROWS.into(),
            ops,
        }],
        codec: vec![1],
        ..Reconciled::default()
    }
}

/// `row list`: a structured read verb (the keyed state): each row's key and value.
fn row_list(_: &[String]) -> Result<StructEdit, ToolError> {
    Ok(Box::new(|ctx| {
        let mut out = String::new();
        for (k, v) in ctx.view().list(ROWS).unwrap_or(&[]) {
            out.push_str(&format!("{}={}\n", k.as_str(), val(v)));
        }
        Ok(StructOutput::new(Vec::new(), out.into_bytes(), 0))
    }))
}

const VERBS: &[PackVerb] = &[PackVerb::structured_read(
    "row list",
    "row list",
    &[PersonAction::ReadAll],
    &[0],
    row_list,
)];

// ---------- harness ----------

struct TestHost {
    now: AtomicU64,
    rights: Mutex<HashMap<String, Rights>>,
    events: Mutex<Vec<AuditEvent>>,
}

impl Host for TestHost {
    fn rights(&self, who: &Participant, _key: &DocKey) -> Rights {
        self.rights
            .lock()
            .unwrap()
            .get(&who.pid)
            .copied()
            .unwrap_or(Rights::none())
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, e: &AuditEvent) {
        self.events.lock().unwrap().push(e.clone());
    }
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
    store: Arc<DocStore>,
    blobs: Arc<BlobStore>,
    mgr: SessionManager,
    pack: Arc<RowsPack>,
}

const NAME: &str = "list.rows";

fn fx(tag: &str) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "k2mb-live-paired-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&p);
    let ws = p.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    let host = Arc::new(TestHost {
        // 2026-10-27 14:30:00 UTC
        now: AtomicU64::new(1_793_111_400_000),
        rights: Mutex::new(HashMap::new()),
        events: Mutex::new(Vec::new()),
    });
    let store = Arc::new(DocStore::open(p.join("docs.redb")).unwrap());
    let h = host.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            p.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || h.now_ms()),
        )
        .unwrap(),
    );
    let mut c = Config::default();
    c.edit_rate_per_s = 1_000_000;
    c.edit_burst = 1_000_000;
    let mgr = SessionManager::new(store.clone(), host.clone(), c).with_blob_store(blobs.clone());
    Fx {
        dir: TmpDir(p),
        ws,
        host,
        store,
        blobs,
        mgr,
        pack: Arc::new(RowsPack::default()),
    }
}

fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: NAME.into(),
    }
}

impl Fx {
    fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    fn write(&self, bytes: &str) {
        fs::write(self.path(), bytes).unwrap();
    }
    fn bytes(&self) -> String {
        fs::read_to_string(self.path()).unwrap()
    }
    fn open(&self) -> Arc<Session> {
        self.open_with(self.pack.clone())
    }
    fn open_with(&self, pack: Arc<RowsPack>) -> Arc<Session> {
        self.mgr
            .open_file(pack, &key(), &self.path(), &self.ws)
            .unwrap()
    }
    fn join(&self, s: &Session, pid: &str, kind: ParticipantKind, owner: bool) -> ConnId {
        self.host
            .rights
            .lock()
            .unwrap()
            .insert(pid.into(), Rights::full().with_owner(owner));
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    fn agent(&self, s: &Session) -> ConnId {
        self.join(s, "bot", ParticipantKind::Agent, false)
    }
    fn owner(&self, s: &Session) -> ConnId {
        self.join(s, "own", ParticipantKind::Person, true)
    }
    fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
    /// A stopped process: a new manager on the same stores, the old one dropped without a
    /// write-back.
    fn restart(&mut self) {
        self.mgr = SessionManager::new(
            self.store.clone(),
            self.host.clone(),
            self.mgr.config().clone(),
        )
        .with_blob_store(self.blobs.clone());
    }
    fn events(&self) -> Vec<AuditEvent> {
        self.host.events.lock().unwrap().clone()
    }
    fn blob_findings(&self, needle: &str) -> usize {
        files_under(&self.dir.0.join("blobs"))
            .iter()
            .map(|p| scan_file(p, &[needle.as_bytes()]).unwrap().len())
            .sum()
    }
    fn store_findings(&self, needle: &str) -> usize {
        self.store.scan_for(&[needle.as_bytes()]).unwrap().len()
    }
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir(dir) else {
        return out;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(files_under(&p));
        } else {
            out.push(p);
        }
    }
    out
}

/// An app's save: written beside the file, then renamed over it.
fn app_save(path: &Path, bytes: &str) {
    let tmp = path.with_file_name(".app-save.tmp");
    fs::write(&tmp, bytes).unwrap();
    fs::rename(&tmp, path).unwrap();
}

fn k(s: &str) -> Key {
    Key::new(s).unwrap()
}

/// One list edit as `c`, at the version the status gives (no read of the file's bytes).
fn edit(s: &Session, c: ConnId, ops: Vec<ListOp>) -> Result<u64, SessionError> {
    let mut opts = EditOptions::new();
    opts.base = Some(s.status()?.version);
    Ok(s.edit_ops(
        c,
        vec![Edit::List {
            root: ROWS.into(),
            ops,
        }],
        &opts,
    )?
    .version)
}

fn insert_after(s: &Session, c: ConnId, at: Option<&str>, key: &str, v: &str) -> u64 {
    edit(
        s,
        c,
        vec![ListOp::Insert {
            key: k(key),
            at: at.map_or(Anchor::Start, |a| Anchor::After(k(a))),
            value: Value::str(v),
        }],
    )
    .unwrap()
}

fn set(s: &Session, c: ConnId, key: &str, v: &str) -> u64 {
    edit(
        s,
        c,
        vec![ListOp::Set {
            key: k(key),
            value: Value::str(v),
        }],
    )
    .unwrap()
}

/// The doc's rows as `row list` prints them (the keyed state; renders nothing).
fn rows(s: &Session, c: ConnId) -> String {
    let o = s
        .run_tool(c, ROWS, &["row".into(), "list".into()], b"")
        .unwrap();
    String::from_utf8(o.stdout).unwrap()
}

fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {}", what()))
    }
}

/// Write back as `c`; the write must happen.
fn flush(row: &str, s: &Session, c: ConnId, what: &str) -> Result<(), String> {
    let out = s
        .flush(c)
        .map_err(|e| format!("{row}: {what}: the write-back failed: {e}"))?;
    need(out.written, row, || format!("{what}: nothing was written"))
}

/// The file: an opaque header, a padded row, a row with an opaque note after it, a padded row
/// and an opaque trailer. `decode` mints r1 (apple), r2 (banana), r3 (cherry).
const F0: &str = "# rows v1\n  apple  \nbanana\n# about banana\n cherry\n# end\n";

// ---------- a live insert, then an edit ----------

/// A live insert (fig after apple), written; then cherry edited, written. Both writes are
/// splices: padding and opaque lines stay with their rows. Red without the pairing: after the
/// first write the carrier holds four rows, `decode` mints r1..r4 for them, and the live keys
/// (r1, n1, r2, r3) pair with the wrong lines.
fn row_live_insert_then_edit_writes_both_exactly() -> Result<(), String> {
    const ROW: &str = "row_live_insert_then_edit_writes_both_exactly";
    let f = fx("insert");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    insert_after(&s, bot, Some("r1"), "n1", "fig");
    flush(ROW, &s, bot, "the first write")?;
    const F1: &str = "# rows v1\n  apple  \nfig\nbanana\n# about banana\n cherry\n# end\n";
    need(f.bytes() == F1, ROW, || {
        format!("the first write: {:?}", f.bytes())
    })?;
    set(&s, bot, "r3", "cherry pie");
    flush(ROW, &s, bot, "the second write")?;
    const F2: &str = "# rows v1\n  apple  \nfig\nbanana\n# about banana\ncherry pie\n# end\n";
    need(f.bytes() == F2, ROW, || {
        format!(
            "the second write splices cherry's line only: {:?}",
            f.bytes()
        )
    })?;
    need(f.pack.paired() > 0, ROW, || {
        "the kit called the paired hook".into()
    })
}

#[test]
fn live_insert_then_edit_writes_both_exactly() {
    row_live_insert_then_edit_writes_both_exactly().unwrap();
}

/// The planted fault "synced content stale": the second write goes red, by name.
#[test]
fn the_planted_synced_content_stale_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::SyncedContentStale, || {
        let name = "row_live_insert_then_edit_writes_both_exactly";
        let e = row_live_insert_then_edit_writes_both_exactly()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("the second write"), "{e}");
    });
}

// ---------- an app's save, then a live edit ----------

/// An app's save that inserts a row above the others (no kit write since the import: it applies
/// by key); then a live edit of banana is spliced into banana's line of the save, and apple's
/// padding stays. Red without the pairing: `decode` of the save mints r1 for the new row.
fn row_app_save_above_then_live_edit_splices_the_right_row() -> Result<(), String> {
    const ROW: &str = "row_app_save_above_then_live_edit_splices_the_right_row";
    let f = fx("save");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    f.advance(1_000);
    const SAVE: &str = "# rows v1\ndate\n  apple  \nbanana\n# about banana\n cherry\n# end\n";
    app_save(&f.path(), SAVE);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(matches!(out, DiskCheck::Ingested { .. }), ROW, || {
        format!("control: the save is taken in: {out:?}")
    })?;
    need(
        rows(&s, bot) == "o1=date\nr1=apple\nr2=banana\nr3=cherry\n",
        ROW,
        || format!("control: the save's row is new: {:?}", rows(&s, bot)),
    )?;
    set(&s, bot, "r2", "blueberry");
    flush(ROW, &s, bot, "the write after the save")?;
    const W: &str = "# rows v1\ndate\n  apple  \nblueberry\n# about banana\n cherry\n# end\n";
    need(f.bytes() == W, ROW, || {
        format!("the edit is spliced into banana's line: {:?}", f.bytes())
    })
}

#[test]
fn app_save_above_then_live_edit_splices_the_right_row() {
    row_app_save_above_then_live_edit_splices_the_right_row().unwrap();
}

// ---------- a restart, then a verb; reads make nothing ----------

/// After a live insert written and a restart, N reads (the file's bytes and a structured read)
/// make no pairing (the stored one is used); a verb is then spliced exactly.
fn row_restart_then_verb_and_reads_make_nothing() -> Result<(), String> {
    const ROW: &str = "row_restart_then_verb_and_reads_make_nothing";
    let mut f = fx("restart");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    insert_after(&s, bot, Some("r1"), "n1", "fig");
    flush(ROW, &s, bot, "the write before the restart")?;
    drop(s);
    f.restart();
    let s = f.open();
    let bot = f.agent(&s);
    differential::take_pairings_made();
    let made = f.pack.pairings();
    const F1: &str = "# rows v1\n  apple  \nfig\nbanana\n# about banana\n cherry\n# end\n";
    for i in 0..20 {
        let got =
            String::from_utf8(s.read(bot).map_err(|e| format!("{ROW}: read: {e}"))?.bytes).unwrap();
        need(got == F1, ROW, || {
            format!("read {i} renders the file: {got:?}")
        })?;
        need(
            rows(&s, bot) == "r1=apple\nn1=fig\nr2=banana\nr3=cherry\n",
            ROW,
            || format!("control: row list {i}"),
        )?;
    }
    let kit = differential::take_pairings_made();
    need(kit == 0 && f.pack.pairings() == made, ROW, || {
        format!(
            "20 reads after a restart made pairings: kit {kit}, pack {}",
            f.pack.pairings() - made
        )
    })?;
    set(&s, bot, "r2", "blueberry");
    flush(ROW, &s, bot, "the write after the restart")?;
    const W: &str = "# rows v1\n  apple  \nfig\nblueberry\n# about banana\n cherry\n# end\n";
    need(f.bytes() == W, ROW, || {
        format!("the verb is spliced after the restart: {:?}", f.bytes())
    })
}

#[test]
fn restart_then_verb_and_reads_make_nothing() {
    row_restart_then_verb_and_reads_make_nothing().unwrap();
}

// ---------- purge and erasure, then a verb ----------

const SECRET: &str = "Qz7Kx9Wm3pV8rT2nJ4hB6yL1";

/// The agent inserts fig and writes a secret into banana; the kit writes the file. The owner
/// overwrites the secret (unwritten) and purges with `scope`: the file holds the purged doc,
/// spliced exactly; a byte scan of the store file and the blob folder finds no secret; a verb
/// after the purge is spliced exactly.
fn purge_then_verb(row: &str, scope: PurgeScope) -> Result<(), String> {
    let f = fx("purge");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let own = f.owner(&s);
    insert_after(&s, bot, Some("r1"), "n1", "fig");
    set(&s, bot, "r2", SECRET);
    flush(row, &s, bot, "control: the secret's write")?;
    need(f.blob_findings(SECRET) > 0, row, || {
        "control: the carrier holds the secret".into()
    })?;
    set(&s, own, "r2", "clean");
    s.purge(own, PurgeRequest::new(scope))
        .map_err(|e| format!("{row}: the purge: {e}"))?;
    const P: &str = "# rows v1\n  apple  \nfig\nclean\n# about banana\n cherry\n# end\n";
    need(f.bytes() == P, row, || {
        format!("the purge's write-back is a splice: {:?}", f.bytes())
    })?;
    let blobs = f.blob_findings(SECRET);
    need(blobs == 0, row, || {
        format!("the blob folder's byte scan finds the secret: {blobs}")
    })?;
    let store = f.store_findings(SECRET);
    need(store == 0, row, || {
        format!("the store file's byte scan finds the secret: {store}")
    })?;
    // the owner's verb (a participant erasure removed the agent's connections)
    set(&s, own, "r3", "cherry pie");
    flush(row, &s, own, "the verb after the purge")?;
    const W: &str = "# rows v1\n  apple  \nfig\nclean\n# about banana\ncherry pie\n# end\n";
    need(f.bytes() == W, row, || {
        format!("the verb after the purge is a splice: {:?}", f.bytes())
    })
}

fn row_history_purge_then_verb() -> Result<(), String> {
    purge_then_verb("row_history_purge_then_verb", PurgeScope::History)
}

fn row_participant_erasure_then_verb() -> Result<(), String> {
    purge_then_verb(
        "row_participant_erasure_then_verb",
        PurgeScope::Participant {
            pid: "bot".into(),
            pseudonym: "someone".into(),
        },
    )
}

#[test]
fn history_purge_then_verb() {
    row_history_purge_then_verb().unwrap();
}

#[test]
fn participant_erasure_then_verb() {
    row_participant_erasure_then_verb().unwrap();
}

/// A purge while the file is missing (it can't be written back: the carrier stays, reported),
/// then the file comes back as the kit last wrote it, then a verb: spliced exactly. The pairing
/// holds no values, so the purge can't make it disagree with the carrier.
fn row_purge_with_the_file_missing_then_back_then_verb() -> Result<(), String> {
    const ROW: &str = "row_purge_with_the_file_missing_then_back_then_verb";
    let f = fx("purge-missing");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    let own = f.owner(&s);
    insert_after(&s, bot, Some("r1"), "n1", "fig");
    flush(ROW, &s, bot, "control: the write")?;
    const F1: &str = "# rows v1\n  apple  \nfig\nbanana\n# about banana\n cherry\n# end\n";
    fs::remove_file(f.path()).unwrap();
    s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    s.purge(own, PurgeRequest::new(PurgeScope::History))
        .map_err(|e| format!("{ROW}: the purge: {e}"))?;
    f.advance(1_000);
    f.write(F1);
    s.file_changed()
        .map_err(|e| format!("{ROW}: the file back: {e}"))?;
    set(&s, bot, "r3", "cherry pie");
    flush(ROW, &s, bot, "the verb once the file is back")?;
    const W: &str = "# rows v1\n  apple  \nfig\nbanana\n# about banana\ncherry pie\n# end\n";
    need(f.bytes() == W, ROW, || {
        format!("the verb is spliced: {:?}", f.bytes())
    })
}

#[test]
fn purge_with_the_file_missing_then_back_then_verb() {
    row_purge_with_the_file_missing_then_back_then_verb().unwrap();
}

// ---------- a version cut between two syncs ----------

/// fig is inserted and written (W1); cherry is edited and a version is named before the next
/// write (its carrier is W1); apricot is inserted at the top and written. The version's file is
/// W1 with cherry's line spliced, by the version's own pairing.
fn row_version_between_syncs_file_at_is_exact() -> Result<(), String> {
    const ROW: &str = "row_version_between_syncs_file_at_is_exact";
    let f = fx("version");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    insert_after(&s, bot, Some("r1"), "n1", "fig");
    flush(ROW, &s, bot, "control: the first write")?;
    set(&s, bot, "r3", "cherry pie");
    let v = s
        .name_version(bot, "between", None)
        .map_err(|e| format!("{ROW}: naming the version: {e}"))?;
    insert_after(&s, bot, None, "n2", "apricot");
    flush(ROW, &s, bot, "control: the second write")?;
    const W2: &str =
        "# rows v1\napricot\n  apple  \nfig\nbanana\n# about banana\ncherry pie\n# end\n";
    need(f.bytes() == W2, ROW, || {
        format!("control: the second write: {:?}", f.bytes())
    })?;
    let got = s
        .file_at(bot, v)
        .map_err(|e| format!("{ROW}: file_at of the version: {e}"))?;
    const V: &str = "# rows v1\n  apple  \nfig\nbanana\n# about banana\ncherry pie\n# end\n";
    need(got.bytes == V.as_bytes(), ROW, || {
        format!(
            "file_at of the version: {:?}",
            String::from_utf8_lossy(&got.bytes)
        )
    })
}

#[test]
fn version_between_syncs_file_at_is_exact() {
    row_version_between_syncs_file_at_is_exact().unwrap();
}

/// The planted fault "pairing missing on version": the version row goes red, by name.
#[test]
fn the_planted_pairing_missing_on_version_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::PairingMissingOnVersion, || {
        let name = "row_version_between_syncs_file_at_is_exact";
        let e = row_version_between_syncs_file_at_is_exact()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("file_at of the version"), "{e}");
    });
}

// ---------- a pairing that doesn't fit ----------

/// The pack makes a pairing that leaves out a key (a pack bug) at a kit write; the next
/// rendering is refused typed (`PairingMismatch`): the write-back fails, the file stays as the
/// kit last wrote it, and the doc keeps the edit.
fn row_a_pairing_that_does_not_fit_is_refused_typed() -> Result<(), String> {
    const ROW: &str = "row_a_pairing_that_does_not_fit_is_refused_typed";
    let f = fx("mismatch");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    f.pack.bad_pairing.store(true, Ordering::SeqCst);
    insert_after(&s, bot, Some("r1"), "n1", "fig");
    flush(ROW, &s, bot, "control: the write that keeps a bad pairing")?;
    const F1: &str = "# rows v1\n  apple  \nfig\nbanana\n# about banana\n cherry\n# end\n";
    need(f.bytes() == F1, ROW, || {
        format!("control: the write: {:?}", f.bytes())
    })?;
    let v = set(&s, bot, "r3", "cherry pie");
    let blind = f.pack.blind.load(Ordering::SeqCst);
    let out = s.flush(bot);
    need(
        matches!(out, Err(SessionError::PairingMismatch { .. })),
        ROW,
        || format!("the write-back is refused typed: {out:?}"),
    )?;
    need(f.pack.blind.load(Ordering::SeqCst) == blind, ROW, || {
        "no carrier-only splice was made".into()
    })?;
    let now = s.file_now(bot).map(|b| b.bytes);
    need(
        matches!(now, Err(SessionError::PairingMismatch { .. })),
        ROW,
        || "file_now is refused typed".into(),
    )?;
    need(f.bytes() == F1, ROW, || {
        format!("the file is as it was: {:?}", f.bytes())
    })?;
    need(s.status().map(|x| x.version) == Ok(v), ROW, || {
        "the doc keeps the edit".into()
    })?;
    // a structured read gives its result, told by a warning naming the refusal (before the
    // render net, it returned the refusal instead of its result)
    let read = s.run_tool(bot, ROWS, &["row".into(), "list".into()], b"");
    need(
        matches!(&read, Ok(o) if o.warnings.lines().len() == 1
            && o.warnings.lines()[0].contains("MPC-E521")),
        ROW,
        || format!("a structured read is told, by a warning: {read:?}"),
    )?;
    // a whole read gives the doc with the file as last written, and the refusal, typed
    let whole = s.read(bot);
    need(
        matches!(&whole, Ok(r) if r.bytes == F1.as_bytes()
            && matches!(r.render_failed, Some(SessionError::PairingMismatch { .. }))),
        ROW,
        || format!("a whole read is told, typed: {whole:?}"),
    )?;
    let e = SessionError::PairingMismatch { reason: "x".into() };
    need(
        k2_mpc_base::tools::session_exit(&e) == 1 && e.to_string().contains("pairing_mismatch"),
        ROW,
        || "the refusal's exit and name".into(),
    )
}

#[test]
fn a_pairing_that_does_not_fit_is_refused_typed() {
    row_a_pairing_that_does_not_fit_is_refused_typed().unwrap();
}

/// The planted fault "splice without match": the refusal row goes red, by name.
#[test]
fn the_planted_splice_without_match_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::SpliceWithoutMatch, || {
        let name = "row_a_pairing_that_does_not_fit_is_refused_typed";
        let e = row_a_pairing_that_does_not_fit_is_refused_typed()
            .expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("refused typed"), "{e}");
    });
}

// ---------- the held check renders nothing ----------

/// An app's save is taken in, and whether the doc holds it is decided with no rendering: the
/// kit compares keyed content.
fn row_the_held_check_renders_nothing() -> Result<(), String> {
    const ROW: &str = "row_the_held_check_renders_nothing";
    let f = fx("held");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    f.advance(1_000);
    const SAVE: &str = "# rows v1\n  apple  \nbanana\n# about banana\n cherry\nelder\n# end\n";
    app_save(&f.path(), SAVE);
    differential::take_held_renders();
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    let held = differential::take_held_renders();
    need(matches!(out, DiskCheck::Ingested { .. }), ROW, || {
        format!("control: the save is taken in: {out:?}")
    })?;
    need(held == 0, ROW, || {
        format!("the held check rendered {held} times")
    })?;
    let got = String::from_utf8(s.read(bot).unwrap().bytes).unwrap();
    need(got == SAVE, ROW, || {
        format!("the doc holds the save: {got:?}")
    })?;
    need(matches!(s.flush(bot), Ok(o) if !o.written), ROW, || {
        "nothing to write: the doc holds the file".into()
    })
}

#[test]
fn the_held_check_renders_nothing() {
    row_the_held_check_renders_nothing().unwrap();
}

/// The planted fault "holds file calls pack": the held-check row goes red, by name.
#[test]
fn the_planted_holds_file_calls_pack_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::HoldsFileCallsPack, || {
        let name = "row_the_held_check_renders_nothing";
        let e =
            row_the_held_check_renders_nothing().expect_err(&format!("planted, {name} must fail"));
        assert!(e.starts_with(&format!("{name}: ")), "{e}");
        assert!(e.contains("the held check rendered"), "{e}");
    });
}

// ---------- an outside save on unwritten changes keeps both ----------

/// The agent edits apple (not written yet) and an app saves, adding elder: the save keeps both.
/// The save is the live doc exactly (the agent's edit isn't merged into it); the kit's version
/// is a version and a conflict copy, byte for byte; one `BlobConflict`. A verb afterwards is
/// spliced into the save.
fn row_a_save_on_unwritten_changes_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_a_save_on_unwritten_changes_keeps_both";
    let f = fx("dirty");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    set(&s, bot, "r1", "Agent");
    f.advance(1_000);
    const SAVE: &str = "# rows v1\n  apple  \nbanana\n# about banana\n cherry\nelder\n# end\n";
    app_save(&f.path(), SAVE);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    let DiskCheck::Conflict {
        kept,
        copy: Some(copy),
        ..
    } = out
    else {
        return Err(format!("{ROW}: the save keeps both: {out:?}"));
    };
    const KITS: &str = "# rows v1\nAgent\nbanana\n# about banana\n cherry\n# end\n";
    let held = fs::read_to_string(&copy).unwrap();
    need(held == KITS, ROW, || {
        format!("the copy is the kit's version: {held:?}")
    })?;
    let kept_bytes = s.file_at(bot, kept).map(|b| b.bytes);
    need(kept_bytes.as_deref() == Ok(KITS.as_bytes()), ROW, || {
        format!("the kept version's file is the kit's version: {kept_bytes:?}")
    })?;
    let got = String::from_utf8(s.read(bot).unwrap().bytes).unwrap();
    need(got == SAVE, ROW, || {
        format!("the save is the live doc exactly: {got:?}")
    })?;
    let ev = f
        .events()
        .iter()
        .filter(|e| matches!(e, AuditEvent::BlobConflict { .. }))
        .count();
    need(ev == 1, ROW, || format!("one BlobConflict: {ev}"))?;
    need(f.bytes() == SAVE, ROW, || "the save stays on disk".into())?;
    let elder = rows(&s, bot)
        .lines()
        .find_map(|l| l.strip_suffix("=elder").map(str::to_owned))
        .ok_or_else(|| format!("{ROW}: elder is a row"))?;
    set(&s, bot, &elder, "elderberry");
    flush(ROW, &s, bot, "the verb after the save")?;
    const W: &str = "# rows v1\n  apple  \nbanana\n# about banana\n cherry\nelderberry\n# end\n";
    need(f.bytes() == W, ROW, || {
        format!("the verb is spliced into the save: {:?}", f.bytes())
    })
}

#[test]
fn a_save_on_unwritten_changes_keeps_both() {
    row_a_save_on_unwritten_changes_keeps_both().unwrap();
}

/// The same across a restart: the agent's edit is in the store, the kit never wrote it (the
/// process stopped before the write-back), and the app saves while the doc is loaded again.
fn row_a_save_on_unwritten_changes_keeps_both_after_a_restart() -> Result<(), String> {
    const ROW: &str = "row_a_save_on_unwritten_changes_keeps_both_after_a_restart";
    let mut f = fx("dirty-restart");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    set(&s, bot, "r1", "Agent");
    drop(s);
    f.restart();
    let s = f.open();
    let bot = f.agent(&s);
    f.advance(1_000);
    const SAVE: &str = "# rows v1\n  apple  \nbanana\n# about banana\n cherry\nelder\n# end\n";
    app_save(&f.path(), SAVE);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    need(matches!(out, DiskCheck::Conflict { .. }), ROW, || {
        format!("the save keeps both after a restart: {out:?}")
    })?;
    let got = String::from_utf8(s.read(bot).unwrap().bytes).unwrap();
    need(got == SAVE, ROW, || {
        format!("the save is the live doc exactly: {got:?}")
    })
}

#[test]
fn a_save_on_unwritten_changes_keeps_both_after_a_restart() {
    row_a_save_on_unwritten_changes_keeps_both_after_a_restart().unwrap();
}

/// The planted fault "dirty save merged by key": both rows go red, by name.
#[test]
fn the_planted_dirty_save_merged_by_key_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::DirtySaveMergedByKey, || {
        for (name, row) in [
            (
                "row_a_save_on_unwritten_changes_keeps_both",
                row_a_save_on_unwritten_changes_keeps_both as fn() -> Result<(), String>,
            ),
            (
                "row_a_save_on_unwritten_changes_keeps_both_after_a_restart",
                row_a_save_on_unwritten_changes_keeps_both_after_a_restart,
            ),
        ] {
            let e = row().expect_err(&format!("planted, {name} must fail"));
            assert!(e.starts_with(&format!("{name}: ")), "{e}");
            assert!(e.contains("keeps both"), "{e}");
        }
    });
}

// ---------- a save the pack re-keys: its carrier and pairing before anything renders ----------

/// `row list` as `c`, the error as text (the row reports it instead of unwinding); a warning
/// (the file can't be rendered at the doc's version) is an error here too.
fn row_list_of(s: &Session, c: ConnId) -> Result<String, String> {
    let o = s
        .run_tool(c, ROWS, &["row".into(), "list".into()], b"")
        .map_err(|e| e.to_string())?;
    match o.warnings.is_empty() {
        true => Ok(String::from_utf8(o.stdout).unwrap()),
        false => Err(format!("warnings: {:?}", o.warnings.lines())),
    }
}

/// The file's bytes a whole read as `c` gives; a read given the synced bytes because the file
/// can't be rendered (`render_failed`) is an error here.
fn file_read(s: &Session, c: ConnId) -> Result<Vec<u8>, String> {
    let r = s.read(c).map_err(|e| e.to_string())?;
    match r.render_failed {
        None => Ok(r.bytes),
        Some(e) => Err(format!("the file can't be rendered: {e}")),
    }
}

/// The id of the version an outside save cut at `seq`.
fn save_version(s: &Session, c: ConnId, seq: u64) -> Result<VersionId, String> {
    s.versions(c)
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|v| v.seq == seq && v.reason == VersionReason::External)
        .map(|v| v.id)
        .max()
        .ok_or_else(|| format!("no outside save's version at {seq}"))
}

/// An app's save that changes the rows (date above, banana gone, padding and an opaque line of
/// its own), on a pack that re-keys it whole: the doc's keys are all the save's. The save is the
/// carrier, with its pairing, before the kit renders the doc that took it: a structured read
/// after it is no `PairingMismatch` (`MPC-E521`, the old carrier's keys are gone from the doc),
/// the doc is clean, the file read is the save, nothing is written; then a live edit is spliced
/// into the save, and the save's version's file is the save, byte for byte.
fn row_a_rekeyed_save_is_the_carrier_before_any_rendering() -> Result<(), String> {
    const ROW: &str = "row_a_rekeyed_save_is_the_carrier_before_any_rendering";
    let f = fx("rekeyed");
    f.pack.rekeys.store(true, Ordering::SeqCst);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    f.advance(1_000);
    const SAVE: &str = "# rows v2\n date \n  apple  \n# about apple\n cherry\n# end\n";
    app_save(&f.path(), SAVE);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    let DiskCheck::Ingested { version } = out else {
        return Err(format!("{ROW}: control: the save is taken in: {out:?}"));
    };
    let read = row_list_of(&s, bot);
    need(
        read.as_deref() == Ok("s1.1=date\ns1.2=apple\ns1.3=cherry\n"),
        ROW,
        || format!("the read after the save: {read:?}"),
    )?;
    let disk = s.status().map(|x| x.disk);
    need(disk == Ok(Some(DiskState::Clean)), ROW, || {
        format!("the doc holds the save, clean: {disk:?}")
    })?;
    let got = file_read(&s, bot);
    need(got.as_deref() == Ok(SAVE.as_bytes()), ROW, || {
        format!("the file read after the save is the save: {got:?}")
    })?;
    need(matches!(s.flush(bot), Ok(o) if !o.written), ROW, || {
        "nothing to write after the save".into()
    })?;
    set(&s, bot, "s1.3", "cherry pie");
    flush(ROW, &s, bot, "the write after the save")?;
    const W: &str = "# rows v2\n date \n  apple  \n# about apple\ncherry pie\n# end\n";
    need(f.bytes() == W, ROW, || {
        format!("the edit is spliced into the save: {:?}", f.bytes())
    })?;
    let v = save_version(&s, bot, version).map_err(|e| format!("{ROW}: {e}"))?;
    let at = s.file_at(bot, v).map(|b| b.bytes);
    need(at.as_deref() == Ok(SAVE.as_bytes()), ROW, || {
        format!("file_at of the save's version is the save: {at:?}")
    })
}

#[test]
fn a_rekeyed_save_is_the_carrier_before_any_rendering() {
    row_a_rekeyed_save_is_the_carrier_before_any_rendering().unwrap();
}

/// The same pack, the agent's edit of apple not written yet: the app's save (date above) keeps
/// both. The kit's version (the copy and the kept version) is rendered from the carrier before
/// the save, byte for byte; the save, re-keyed, is the live doc exactly, and is the carrier
/// with its pairing before anything renders it: the read after it is no `PairingMismatch`, the
/// doc is clean, and a verb is spliced into the save.
fn row_a_rekeyed_save_on_unwritten_changes_keeps_both() -> Result<(), String> {
    const ROW: &str = "row_a_rekeyed_save_on_unwritten_changes_keeps_both";
    let f = fx("rekeyed-dirty");
    f.pack.rekeys.store(true, Ordering::SeqCst);
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    set(&s, bot, "r1", "Agent");
    f.advance(1_000);
    const SAVE: &str = "# rows v2\n date \n  apple  \nbanana\n# about banana\n cherry\n# end\n";
    app_save(&f.path(), SAVE);
    let out = s.file_changed().map_err(|e| format!("{ROW}: {e}"))?;
    let DiskCheck::Conflict {
        kept,
        copy: Some(copy),
        ..
    } = out
    else {
        return Err(format!("{ROW}: control: the save keeps both: {out:?}"));
    };
    let read = row_list_of(&s, bot);
    need(
        read.as_deref() == Ok("s1.1=date\ns1.2=apple\ns1.3=banana\ns1.4=cherry\n"),
        ROW,
        || format!("the read after the save: {read:?}"),
    )?;
    let disk = s.status().map(|x| x.disk);
    need(disk == Ok(Some(DiskState::Clean)), ROW, || {
        format!("the doc is the save, clean: {disk:?}")
    })?;
    const KITS: &str = "# rows v1\nAgent\nbanana\n# about banana\n cherry\n# end\n";
    let held = fs::read_to_string(&copy).unwrap();
    need(held == KITS, ROW, || {
        format!("the copy is the kit's version: {held:?}")
    })?;
    let kept_bytes = s.file_at(bot, kept).map(|b| b.bytes);
    need(kept_bytes.as_deref() == Ok(KITS.as_bytes()), ROW, || {
        format!("the kept version's file is the kit's version: {kept_bytes:?}")
    })?;
    let got = file_read(&s, bot);
    need(got.as_deref() == Ok(SAVE.as_bytes()), ROW, || {
        format!("the save is the live doc exactly: {got:?}")
    })?;
    let ev = f
        .events()
        .iter()
        .filter(|e| matches!(e, AuditEvent::BlobConflict { .. }))
        .count();
    need(ev == 1, ROW, || format!("one BlobConflict: {ev}"))?;
    set(&s, bot, "s1.3", "blueberry");
    flush(ROW, &s, bot, "the verb after the save")?;
    const W: &str = "# rows v2\n date \n  apple  \nblueberry\n# about banana\n cherry\n# end\n";
    need(f.bytes() == W, ROW, || {
        format!("the verb is spliced into the save: {:?}", f.bytes())
    })
}

#[test]
fn a_rekeyed_save_on_unwritten_changes_keeps_both() {
    row_a_rekeyed_save_on_unwritten_changes_keeps_both().unwrap();
}

/// The planted fault "settle before carrier" (the old order: the settle renders the doc that
/// took the save with the carrier and pairing from before it): the splice is refused and the doc
/// marked dirty, though it holds the save. The refusal doesn't outlive the switch to the save's
/// carrier (the read after it passes), so both rows go red at the clean check, by name.
#[test]
fn the_planted_settle_before_carrier_is_caught_by_name() {
    differential::with_keyed_mutant(KeyedMutant::SettleBeforeCarrier, || {
        for (name, row) in [
            (
                "row_a_rekeyed_save_is_the_carrier_before_any_rendering",
                row_a_rekeyed_save_is_the_carrier_before_any_rendering
                    as fn() -> Result<(), String>,
            ),
            (
                "row_a_rekeyed_save_on_unwritten_changes_keeps_both",
                row_a_rekeyed_save_on_unwritten_changes_keeps_both,
            ),
        ] {
            let e = row().expect_err(&format!("planted, {name} must fail"));
            assert!(e.starts_with(&format!("{name}: ")), "{e}");
            assert!(e.contains(", clean: Ok(Some(Dirty"), "{e}");
        }
    });
}

// ---------- an older kit's write (N-1, in process) ----------

/// An older kit (the same file type keeping a carrier and no pairing) writes the file after this
/// kit did: it inserts a row, so the file holds five rows while the stored pairing, naming this
/// kit's last carrier, holds four keys. Opened here again, the stale row is not used: the
/// pairing is made again from the stored synced state (one pairing made at the open), and a verb
/// is spliced exactly into the older kit's bytes.
fn row_an_older_kits_write_makes_the_pairing_again() -> Result<(), String> {
    const ROW: &str = "row_an_older_kits_write_makes_the_pairing_again";
    let mut f = fx("older");
    f.write(F0);
    let s = f.open();
    let bot = f.agent(&s);
    insert_after(&s, bot, Some("r1"), "n1", "fig");
    flush(ROW, &s, bot, "control: this kit's write")?;
    drop(s);
    f.restart();
    let older = Arc::new(RowsPack {
        older: true,
        ..RowsPack::default()
    });
    let s = f.open_with(older);
    let bot = f.agent(&s);
    insert_after(&s, bot, None, "m1", "date");
    flush(ROW, &s, bot, "control: the older kit's write")?;
    const OLD: &str = "date\napple\nfig\nbanana\ncherry\n";
    need(f.bytes() == OLD, ROW, || {
        format!("control: the older kit's own encode: {:?}", f.bytes())
    })?;
    drop(s);
    f.restart();
    differential::take_pairings_made();
    let s = f.open();
    let bot = f.agent(&s);
    let made = differential::take_pairings_made();
    need(made == 1, ROW, || {
        format!("the open made the pairing again once: {made}")
    })?;
    set(&s, bot, "r2", "blueberry");
    flush(ROW, &s, bot, "this kit's write after the older kit's")?;
    const W: &str = "date\napple\nfig\nblueberry\ncherry\n";
    need(f.bytes() == W, ROW, || {
        format!(
            "the verb is spliced into the older kit's bytes: {:?}",
            f.bytes()
        )
    })
}

#[test]
fn an_older_kits_write_makes_the_pairing_again() {
    row_an_older_kits_write_makes_the_pairing_again().unwrap();
}

// ---------- the byte-exact row and the hook's default ----------

/// The kit's byte-exact row (`carrier_judge`) holds for a pairing pack, and a pack that keeps
/// the defaults pairs nothing (its `encode_from_paired` is `encode_from`).
#[test]
fn the_byte_exact_row_holds_and_the_default_pairs_nothing() {
    use k2_mpc_base::session::test_support::{carrier_judge, CarrierVerdict};
    let pack: Arc<dyn Pack> = Arc::new(RowsPack::default());
    let got = carrier_judge(pack, &[("a.rows", F0.as_bytes())]);
    assert_eq!(got, Ok(CarrierVerdict::ByteExact { files: 1 }));
    struct Plain;
    impl Pack for Plain {
        fn schema(&self) -> Schema {
            SCHEMA
        }
        fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
            RowsPack::default().decode(b)
        }
        fn encode(&self, c: &[(String, Neutral)], x: &[u8]) -> Result<Vec<u8>, PackError> {
            RowsPack::default().encode(c, x)
        }
    }
    let c = Plain.decode(F0.as_bytes()).unwrap().roots;
    assert!(!Plain.wants_pairing());
    assert_eq!(
        Plain
            .pairing(&c, &[1])
            .unwrap()
            .keys(ROWS)
            .map(<[Key]>::len),
        Some(3)
    );
    assert_eq!(
        Plain.encode_from_paired(F0.as_bytes(), &Pairing::default(), &c, &[1]),
        Ok(b"apple\nbanana\ncherry\n".to_vec())
    );
}

/// The stored form round-trips; another version or trailing bytes read as none.
#[test]
fn a_pairing_round_trips() {
    let p = Pairing {
        roots: vec![
            ("rows".into(), vec![k("r1"), k("n1.ab")]),
            ("x".into(), vec![]),
        ],
    };
    let b = p.to_bytes();
    assert_eq!(Pairing::from_bytes(&b), Some(p.clone()));
    let mut other = b.clone();
    other[0] = 2;
    assert_eq!(Pairing::from_bytes(&other), None);
    let mut long = b;
    long.push(0);
    assert_eq!(Pairing::from_bytes(&long), None);
    assert_eq!(p.len(), 2);
}

// ---------- N-1 of the pairing (tools/n1/n1.sh layout-pairing / node-pairing) ----------

const N1_T0: u64 = 1_000_000;
const HOUR: u64 = 60 * 60_000;

/// `c.rows`' inserted key: in its pairing row and its doc only, so a byte scan after this
/// build's rewrite says whether the rows a previous release's delete left were swept.
const N1_MARK: &str = "q9z7c";

struct N1Clock(Arc<AtomicU64>);
impl Host for N1Clock {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

fn n1_dir() -> PathBuf {
    PathBuf::from(std::env::var("K2MB_N1_OUT").expect("K2MB_N1_OUT"))
}

fn n1_key(path: &str) -> DocKey {
    DocKey {
        space: "ws".into(),
        path: path.into(),
    }
}

/// The store, blob store and manager in `dir`, and its folder `dir/ws`.
fn n1_open(dir: &Path, now: u64) -> (Arc<DocStore>, SessionManager, PathBuf) {
    fs::create_dir_all(dir.join("ws")).unwrap();
    let store = Arc::new(DocStore::open(dir.join("docs.redb")).unwrap());
    let now = Arc::new(AtomicU64::new(now));
    let clock = now.clone();
    let blobs = Arc::new(
        BlobStore::open(
            store.clone(),
            dir.join("blobs"),
            BlobConfig::default(),
            Arc::new(move || clock.load(Ordering::SeqCst)),
        )
        .unwrap(),
    );
    let mgr = SessionManager::new(store.clone(), Arc::new(N1Clock(now)), Config::default())
        .with_blob_store(blobs);
    (store, mgr, fs::canonicalize(dir.join("ws")).unwrap())
}

fn n1_person(s: &Session) -> ConnId {
    s.join(
        Participant::new("p", "P", "#111111", ParticipantKind::Person, "cli"),
        Stay::Held,
    )
    .unwrap()
    .conn
}

/// Open `name` with the pairing pack: the pairings the open made, the session and a person.
fn n1_doc(mgr: &SessionManager, ws: &Path, name: &str) -> (u64, Arc<Session>, ConnId) {
    differential::take_pairings_made();
    let s = mgr
        .open_file(
            Arc::new(RowsPack::default()),
            &n1_key(name),
            &ws.join(name),
            ws,
        )
        .unwrap();
    let made = differential::take_pairings_made();
    let p = n1_person(&s);
    (made, s, p)
}

/// Three live docs of the pairing pack, each with a carrier and a pairing (`carrier_pairings`),
/// for the previous release's consumer (tools/n1/consumer-pairing): `a.rows` it writes, `b.rows`
/// it leaves, `c.rows` it deletes.
#[test]
#[ignore = "tools/n1/n1.sh: writes the store for the previous release's consumer"]
fn n1_pairing_producer() {
    let dir = n1_dir();
    let (store, mgr, ws) = n1_open(&dir, N1_T0);
    const F1: &str = "# rows v1\n  apple  \nfig\nbanana\n# about banana\n cherry\n# end\n";
    for (name, key) in [("a.rows", "n1"), ("b.rows", "n1"), ("c.rows", N1_MARK)] {
        fs::write(ws.join(name), F0).unwrap();
        let (_, s, p) = n1_doc(&mgr, &ws, name);
        insert_after(&s, p, Some("r1"), key, "fig");
        assert!(s.flush(p).unwrap().written);
        assert_eq!(fs::read_to_string(ws.join(name)).unwrap(), F1);
    }
    mgr.release().unwrap();
    store
        .check_rewritable()
        .expect("this build knows carrier_pairings");
    println!("this build: three docs with a carrier and a pairing each (carrier_pairings)");
}

/// After the previous release's run. `b.rows`, which it left, opens on its stored pairing (no
/// pairing made) and a verb splices exactly. `a.rows`, which it wrote with its own encode (a row
/// inserted, the stored pairing left naming this build's carrier), opens with the pairing made
/// again from the stored synced state, and a verb splices exactly into its bytes. `c.rows`, which
/// it deleted, leaving its pairing rows: swept at this build's open, so after this build's
/// rewrite a byte scan of the store finds none. Its own store (`old/`, a carrier and no pairing)
/// opens with the pairing made from the synced state, and a verb splices exactly.
#[test]
#[ignore = "tools/n1/n1.sh: reads the store the previous release's consumer left"]
fn n1_pairing_after() {
    let dir = n1_dir();
    let (store, mgr, ws) = n1_open(&dir, N1_T0 + 6 * HOUR);
    let (made, s, p) = n1_doc(&mgr, &ws, "b.rows");
    assert_eq!(made, 0, "b.rows opens on its stored pairing");
    set(&s, p, "r2", "blueberry");
    assert!(s.flush(p).unwrap().written);
    assert_eq!(
        fs::read_to_string(ws.join("b.rows")).unwrap(),
        "# rows v1\n  apple  \nfig\nblueberry\n# about banana\n cherry\n# end\n"
    );
    println!("b.rows: the stored pairing, untouched by the older kit; a verb spliced");
    let older = fs::read_to_string(ws.join("a.rows")).unwrap();
    assert_eq!(
        older, "date\napple\nfig\nbanana\ncherry\n",
        "the older kit's write"
    );
    let (made, s, p) = n1_doc(&mgr, &ws, "a.rows");
    assert_eq!(
        made, 1,
        "a.rows: the stale pairing is made again at the open"
    );
    set(&s, p, "r2", "blueberry");
    assert!(s.flush(p).unwrap().written);
    assert_eq!(
        fs::read_to_string(ws.join("a.rows")).unwrap(),
        "date\napple\nfig\nblueberry\ncherry\n"
    );
    println!("a.rows: the older kit's write; the pairing made again; a verb spliced");
    mgr.release().unwrap();
    drop(mgr);
    store
        .rewrite_file()
        .expect("this build's rewrite takes the store");
    let left = store.scan_for(&[N1_MARK.as_bytes()]).unwrap().len();
    assert_eq!(left, 0, "c.rows' pairing rows were swept");
    println!("c.rows: deleted by the older kit; its pairing rows swept here");
    drop(store);
    let (_, mgr, ws) = n1_open(&dir.join("old"), N1_T0 + 6 * HOUR);
    let (made, s, p) = n1_doc(&mgr, &ws, "d.rows");
    assert_eq!(
        made, 1,
        "the older kit's own store: the pairing made at the open"
    );
    set(&s, p, "r2", "blueberry");
    assert!(s.flush(p).unwrap().written);
    assert_eq!(
        fs::read_to_string(ws.join("d.rows")).unwrap(),
        "date\napple\nblueberry\ncherry\n"
    );
    mgr.release().unwrap();
    println!("old/d.rows: a carrier and no pairing; the pairing made; a verb spliced");
    println!("N-1 PAIRING (this build) OK");
}
