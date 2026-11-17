//! The test pack and harness of the edit check rows (`tests/pack_check_edit.rs`) and the render
//! net rows (`tests/render_net.rs`): `net-units`, one line per unit (its key, then
//! `\t<field>=<value>` per field, in field order), a list of units and a map of their fields
//! (`u1:title`).
//!
//! - It keeps a carrier (`Pack::wants_carrier`) when asked to, and its `encode` and
//!   `encode_from` fail while `failing` is set (a file the pack can't render).
//! - It opts in to the kit's check of every live change (`Pack::checks_edits`) while `checks`
//!   is set, and `check_edit` refuses any change to the unit `refuse` names while that unit is
//!   in the doc (`NET-E001`), as a pack whose write-back can't write it would.
//! - It pairs its carrier (`Pack::wants_pairing`, the default pairing) while `pairs` is set.
//! - Its verbs: `unit list` (a structured read: each unit's key) and `unit set <unit> <field>
//!   <value>` (a structured edit).
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use k2_mpc_base::blob::{BlobConfig, BlobStore};
use k2_mpc_base::doc::{Edit, Neutral};
use k2_mpc_base::elem::{Key, MapOp, Value};
use k2_mpc_base::parity::PersonAction;
use k2_mpc_base::schema::{Kind, Schema};
use k2_mpc_base::session::{
    AuditEvent, Config, ConnId, Declined, EditOptions, Host, Pack, PackContent, PackError,
    PackRefusal, Participant, ParticipantKind, Rights, Session, SessionManager, Stay, View,
};
use k2_mpc_base::store::{DocKey, DocStore};
use k2_mpc_base::tools::{PackVerb, StructEdit, StructOutput, ToolError};

pub const UNITS: &str = "units";
pub const FIELDS: &str = "fields";

pub const SCHEMA: Schema = Schema {
    name: "net-units",
    roots: &[(UNITS, Kind::MovableList), (FIELDS, Kind::Map)],
};

/// The pack's own code for a change it refuses.
pub const REFUSED: &str = "NET-E001";

pub struct Units {
    carrier: bool,
    /// While set, `encode` and `encode_from` fail.
    pub failing: AtomicBool,
    /// `Pack::checks_edits`' answer.
    pub checks: AtomicBool,
    /// `Pack::wants_pairing`'s answer (off unless a row sets it before the open).
    pub pairs: AtomicBool,
    /// The unit whose changes `check_edit` refuses.
    pub refuse: Mutex<Option<String>>,
    /// Every `check_edit` call (a test-only count).
    pub checked: AtomicU64,
    /// Every `encode` and `encode_from` call (a test-only count).
    pub encodes: AtomicU64,
}

impl Units {
    pub fn new(carrier: bool) -> Arc<Self> {
        Arc::new(Self {
            carrier,
            failing: AtomicBool::new(false),
            checks: AtomicBool::new(true),
            pairs: AtomicBool::new(false),
            refuse: Mutex::new(None),
            checked: AtomicU64::new(0),
            encodes: AtomicU64::new(0),
        })
    }
    pub fn fail(&self, on: bool) {
        self.failing.store(on, Ordering::SeqCst);
    }
    pub fn refuse(&self, unit: Option<&str>) {
        *self.refuse.lock().unwrap() = unit.map(str::to_owned);
    }
    pub fn checked(&self) -> u64 {
        self.checked.load(Ordering::SeqCst)
    }
    pub fn encodes(&self) -> u64 {
        self.encodes.load(Ordering::SeqCst)
    }
}

fn bad(message: String) -> Declined {
    Declined {
        code: "malformed",
        message,
    }
}

/// Each unit's key and fields, in order.
type UnitList = Vec<(Key, BTreeMap<String, String>)>;

/// The units of the doc's content, each with its fields.
fn units_of(c: &[(String, Neutral)]) -> Result<UnitList, PackError> {
    let mut units: &[(Key, Value)] = &[];
    let empty = BTreeMap::new();
    let mut fields = &empty;
    for (r, n) in c {
        match n {
            Neutral::List(l) if r == UNITS => units = l,
            Neutral::Map(m) if r == FIELDS => fields = m,
            _ => return Err(PackError(format!("unexpected root {r}"))),
        }
    }
    Ok(units
        .iter()
        .map(|(k, _)| {
            let prefix = format!("{}:", k.as_str());
            let mine = fields
                .range(prefix.clone()..)
                .map_while(|(fk, v)| {
                    let f = fk.strip_prefix(&prefix)?;
                    Some((f.to_owned(), v.as_str().unwrap_or("").to_owned()))
                })
                .collect();
            (k.clone(), mine)
        })
        .collect())
}

impl Pack for Units {
    fn schema(&self) -> Schema {
        SCHEMA
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        let s = std::str::from_utf8(b).map_err(|e| bad(e.to_string()))?;
        let mut list = Vec::new();
        let mut fields = BTreeMap::new();
        for line in s.lines() {
            let mut parts = line.split('\t');
            let key = Key::new(parts.next().unwrap_or("")).map_err(|e| bad(e.to_string()))?;
            for p in parts {
                let (f, v) = p
                    .split_once('=')
                    .ok_or_else(|| bad(format!("no = in {p:?}")))?;
                fields.insert(format!("{}:{f}", key.as_str()), Value::str(v));
            }
            list.push((key, Value::Null));
        }
        Ok(PackContent {
            roots: vec![
                (UNITS.into(), Neutral::List(list)),
                (FIELDS.into(), Neutral::Map(fields)),
            ],
            codec: vec![1],
        })
    }
    fn encode(&self, c: &[(String, Neutral)], _: &[u8]) -> Result<Vec<u8>, PackError> {
        self.encodes.fetch_add(1, Ordering::SeqCst);
        if self.failing.load(Ordering::SeqCst) {
            return Err(PackError("a planted render failure".into()));
        }
        let mut out = String::new();
        for (k, fields) in units_of(c)? {
            out.push_str(k.as_str());
            for (f, v) in fields {
                out.push_str(&format!("\t{f}={v}"));
            }
            out.push('\n');
        }
        Ok(out.into_bytes())
    }
    fn wants_carrier(&self) -> bool {
        self.carrier
    }
    fn encode_from(
        &self,
        _carrier: &[u8],
        c: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, PackError> {
        self.encode(c, codec)
    }
    fn wants_pairing(&self) -> bool {
        self.pairs.load(Ordering::SeqCst)
    }
    fn checks_edits(&self) -> bool {
        self.checks.load(Ordering::SeqCst)
    }
    fn check_edit(&self, view: &View, edits: &[Edit]) -> Result<(), PackRefusal> {
        self.checked.fetch_add(1, Ordering::SeqCst);
        let Some(unit) = self.refuse.lock().unwrap().clone() else {
            return Ok(());
        };
        let held = view
            .list(UNITS)
            .is_some_and(|l| l.iter().any(|(k, _)| k.as_str() == unit));
        let prefix = format!("{unit}:");
        let touches = edits.iter().any(|e| match e {
            Edit::List { ops, .. } => ops.iter().any(|o| o.key().as_str() == unit),
            Edit::Map { ops, .. } => ops.iter().any(|o| o.key().starts_with(&prefix)),
            _ => false,
        });
        match held && touches {
            true => Err(PackRefusal {
                code: REFUSED,
                message: format!("{unit} is driven: the file can't hold a change to it"),
            }),
            false => Ok(()),
        }
    }
    fn verbs(&self) -> &'static [PackVerb] {
        VERBS
    }
}

fn usage(u: &str) -> ToolError {
    ToolError::usage(format!("usage: {u}"))
}

/// `unit list`: each unit's key, in order (a structured read: the keyed state only).
fn unit_list(_: &[String]) -> Result<StructEdit, ToolError> {
    Ok(Box::new(|ctx| {
        let mut out = String::new();
        for (k, _) in ctx.view().list(UNITS).unwrap_or(&[]) {
            out.push_str(k.as_str());
            out.push('\n');
        }
        Ok(StructOutput::new(Vec::new(), out.into_bytes(), 0))
    }))
}

/// `unit set <unit> <field> <value>`: one field.
fn unit_set(args: &[String]) -> Result<StructEdit, ToolError> {
    let [unit, field, value] = args else {
        return Err(usage("unit set <unit> <field> <value>"));
    };
    let (key, value) = (format!("{unit}:{field}"), value.clone());
    Ok(Box::new(move |_| {
        Ok(StructOutput::new(
            vec![Edit::Map {
                root: FIELDS.into(),
                ops: vec![MapOp::Set {
                    key,
                    value: Value::str(&value),
                }],
            }],
            Vec::new(),
            0,
        ))
    }))
}

const VERBS: &[PackVerb] = &[
    PackVerb::structured_read(
        "unit list",
        "unit list",
        &[PersonAction::ReadAll],
        &[0],
        unit_list,
    ),
    PackVerb::structured_edit(
        "unit set",
        "unit set <unit> <field> <value>",
        &[PersonAction::TypeOver],
        &[0, 2],
        unit_set,
    ),
];

// ---------- harness ----------

pub struct TestHost {
    pub now: AtomicU64,
}

impl Host for TestHost {
    fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
        Rights::full().with_owner(true)
    }
    fn now_ms(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
    fn on_event(&self, _: &AuditEvent) {}
}

pub struct Fx {
    pub dir: PathBuf,
    pub ws: PathBuf,
    pub host: Arc<TestHost>,
    pub mgr: SessionManager,
    pub pack: Arc<Units>,
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

pub const NAME: &str = "scene.units";

/// Three units, as the file holds them.
pub const F0: &str = "u1\ttitle=A\nu2\ttitle=B\nu3\ttitle=C\n";

pub fn key() -> DocKey {
    DocKey {
        space: "ws".into(),
        path: NAME.into(),
    }
}

/// A fixture whose file holds `F0`, opened by a pack that keeps a carrier or not.
pub fn fx(tag: &str, carrier: bool) -> Fx {
    static N: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "k2mb-net-{tag}-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = fs::remove_dir_all(&dir);
    let ws = dir.join("ws");
    fs::create_dir_all(&ws).unwrap();
    let ws = fs::canonicalize(ws).unwrap();
    fs::write(ws.join(NAME), F0).unwrap();
    let host = Arc::new(TestHost {
        now: AtomicU64::new(1_793_111_400_000),
    });
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
    let mgr = SessionManager::new(store, host.clone(), c).with_blob_store(blobs);
    Fx {
        dir,
        ws,
        host,
        mgr,
        pack: Units::new(carrier),
    }
}

impl Fx {
    pub fn path(&self) -> PathBuf {
        self.ws.join(NAME)
    }
    pub fn bytes(&self) -> String {
        fs::read_to_string(self.path()).unwrap()
    }
    pub fn open(&self) -> Arc<Session> {
        self.mgr
            .open_file(self.pack.clone(), &key(), &self.path(), &self.ws)
            .unwrap()
    }
    pub fn join(&self, s: &Session, pid: &str, kind: ParticipantKind) -> ConnId {
        s.join(
            Participant::new(pid, pid, "#123456", kind, "cli"),
            Stay::Held,
        )
        .unwrap()
        .conn
    }
    pub fn person(&self, s: &Session) -> ConnId {
        self.join(s, "ann", ParticipantKind::Person)
    }
    pub fn agent(&self, s: &Session) -> ConnId {
        self.join(s, "bot", ParticipantKind::Agent)
    }
    pub fn advance(&self, ms: u64) {
        self.host.now.fetch_add(ms, Ordering::SeqCst);
    }
}

/// One field set, as a person's edit sends it (by key, no base).
pub fn set(unit: &str, field: &str, value: &str) -> Vec<Edit> {
    vec![Edit::Map {
        root: FIELDS.into(),
        ops: vec![MapOp::Set {
            key: format!("{unit}:{field}"),
            value: Value::str(value),
        }],
    }]
}

/// A person's direct edit of one field.
pub fn person_set(
    s: &Session,
    c: ConnId,
    unit: &str,
    field: &str,
    value: &str,
) -> Result<u64, k2_mpc_base::session::SessionError> {
    s.edit_ops(c, set(unit, field, value), &EditOptions::new())
        .map(|r| r.version)
}

pub fn argv(s: &str) -> Vec<String> {
    s.split(' ').map(str::to_owned).collect()
}

pub fn need(ok: bool, row: &str, what: impl FnOnce() -> String) -> Result<(), String> {
    match ok {
        true => Ok(()),
        false => Err(format!("{row}: {}", what())),
    }
}
