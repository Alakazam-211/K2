//! The reuse rows: a live state's K2SCENE1 written over the carrier's mesh read
//! ([`k2_mpc_blend::live::reuse`]), byte for byte the whole mesh read of a version that holds
//! the state. Two routes: the kit's, through a live session whose pack calls
//! [`live_k2scene`] from its view hook on the state and carrier the kit hands it ([`Hooked`]),
//! the oracle the whole read of the version the kit's write-back writes; and the direct one
//! ([`live_k2scene_of`]) on a store edited by hand, for the states a write-back doesn't write,
//! the oracle the whole read of a generated file that holds the state. Each row returns
//! `Err("<row>: <step>: ...")`, a `control` step first that its plant leaves passing.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Applied, Key, KeyedRoot, KeyedState, Value};
use k2_mpc_base::errors::ErrorEntry;
use k2_mpc_base::index::Digest;
use k2_mpc_base::placement::{Placement, Space};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    ConnId, Declined, DiskCheck, Pack, PackContent, PackError, Pairing, Reconciled, Session,
    SpliceError, VerbState, View,
};
use k2_mpc_base::tools::PackVerb;
use k2_mpc_base::view_index::{ViewIndex, ViewSchema, ViewSource, ViewTouched};
use k2_mpc_blend::live::place::CarrierView;
use k2_mpc_blend::live::reuse::{live_k2scene, live_k2scene_of};
use k2_mpc_blend::live::{
    BlendLive, LiveK2Scene, LiveLimits, ReuseDeclined, COLLS, COLL_PROPS, OBJS, OBJ_PROPS,
};
use k2_mpc_blend::meshread::{mesh_read_paired, MeshReadError, MeshReadLimits};
use k2_mpc_blend::reader::take_reads;
use k2_mpc_blend::testgen::scenes::{box_mesh, living_room, living_room_edited, RoomEdits};
use k2_mpc_blend::testgen::{CameraSpec, MeshLayouts, MeshSpec, ObjectSpec, SceneBuilder};

use crate::live_support::{app_save, fx, key, run_based, stdout, Fx};

fn step(ok: bool, row: &str, name: &str, why: impl FnOnce() -> String) -> Result<(), String> {
    if ok {
        Ok(())
    } else {
        Err(format!("{row}: {name}: {}", why()))
    }
}

// ---------- the kit's route: the pack's view hook calls the reuse ----------

/// One call the hook made: what it gave, and the files read during the call.
#[derive(Debug, Clone)]
pub struct Got {
    pub out: Result<LiveK2Scene, ReuseDeclined>,
    pub reads: u64,
}

/// The live pack, its view hook calling [`live_k2scene`] after its own derive (so the
/// carrier's one decode is the view's, never the call's), each call kept with the files the
/// reader began to read during it.
#[derive(Default)]
pub struct Hooked {
    pub pack: BlendLive,
    pub limits: Mutex<MeshReadLimits>,
    pub got: Mutex<Vec<Got>>,
}

impl Hooked {
    pub fn new() -> Hooked {
        Hooked::default()
    }

    fn call(&self, source: ViewSource<'_>) {
        let ViewSource::Live(state, Some(carrier)) = source else {
            return;
        };
        let limits = *self.limits.lock().unwrap();
        let _ = take_reads();
        let out = live_k2scene(state, carrier, &limits);
        let reads = take_reads();
        self.got.lock().unwrap().push(Got { out, reads });
    }

    /// The last call's, and the calls kept emptied.
    pub fn last(&self) -> Option<Got> {
        let mut g = self.got.lock().unwrap();
        let last = g.last().cloned();
        g.clear();
        last
    }
}

impl Pack for Hooked {
    fn schema(&self) -> Schema {
        self.pack.schema()
    }
    fn decode(&self, b: &[u8]) -> Result<PackContent, Declined> {
        self.pack.decode(b)
    }
    fn encode(&self, c: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, PackError> {
        self.pack.encode(c, codec)
    }
    fn wants_carrier(&self) -> bool {
        self.pack.wants_carrier()
    }
    fn wants_pairing(&self) -> bool {
        self.pack.wants_pairing()
    }
    fn encode_from_paired(
        &self,
        carrier: &[u8],
        pairing: &Pairing,
        c: &[(String, Neutral)],
        codec: &[u8],
    ) -> Result<Vec<u8>, SpliceError> {
        self.pack.encode_from_paired(carrier, pairing, c, codec)
    }
    fn view_schema(&self) -> Option<ViewSchema> {
        self.pack.view_schema()
    }
    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        self.pack.view_derive(source, index);
        self.call(source);
    }
    fn view_derive_change(
        &self,
        source: ViewSource<'_>,
        index: &mut ViewIndex,
        touched: &ViewTouched,
    ) -> bool {
        let kept = self.pack.view_derive_change(source, index, touched);
        self.call(source);
        kept
    }
    fn view_placement(&self, source: ViewSource<'_>, root: &str, key: &Key) -> Option<Placement> {
        self.pack.view_placement(source, root, key)
    }
    fn view_space(&self, source: ViewSource<'_>) -> Option<Space> {
        self.pack.view_space(source)
    }
    fn address_of(&self, root: &str, key: &Key) -> String {
        self.pack.address_of(root, key)
    }
    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        self.pack.key_of(root, address)
    }
    fn reconcile(&self, synced: &View, bytes: &[u8], codec: &[u8]) -> Result<Reconciled, Declined> {
        self.pack.reconcile(synced, bytes, codec)
    }
    fn verbs(&self) -> &'static [PackVerb] {
        self.pack.verbs()
    }
    fn errors(&self) -> &'static [ErrorEntry] {
        self.pack.errors()
    }
    fn verb_state_build(&self, view: &View) -> Option<Box<dyn VerbState>> {
        self.pack.verb_state_build(view)
    }
    fn verb_state_change(
        &self,
        state: &mut (dyn VerbState + 'static),
        applied: &[Applied],
        view: &View,
    ) -> bool {
        self.pack.verb_state_change(state, applied, view)
    }
    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        self.pack.composite()
    }
}

/// `bytes` opened live with the hooked pack.
pub fn open_hooked(tag: &str, bytes: &[u8]) -> (Fx, Arc<Session>, Arc<Hooked>) {
    let f = fx(tag, |_| {});
    f.write(bytes);
    let h = Arc::new(Hooked::new());
    let s = f
        .mgr
        .open_file(h.clone(), &key(), &f.path(), &f.ws)
        .expect("the scene opens live");
    (f, s, h)
}

/// The view read as `c`, and the hook's last call.
fn call(row: &str, what: &str, s: &Session, c: ConnId, h: &Hooked) -> Result<Got, String> {
    s.view_index(c)
        .map_err(|e| format!("{row}: {what}: the view: {e}"))?
        .ok_or_else(|| format!("{row}: {what}: no view index"))?;
    h.last()
        .ok_or_else(|| format!("{row}: {what}: the hook made no call"))
}

/// The call's output, or the step's failure.
fn ok(row: &str, what: &str, g: Got) -> Result<LiveK2Scene, String> {
    g.out
        .map_err(|e| format!("{row}: {what}: the call declined ({}): {e}", e.code()))
}

/// The live doc's roots, as `c` reads them.
pub fn roots(s: &Session, c: ConnId) -> Vec<(String, Neutral)> {
    s.read(c).expect("the doc reads").content
}

/// Write back as `c`; the write must happen.
fn flush(row: &str, what: &str, s: &Session, c: ConnId) -> Result<(), String> {
    let out = s
        .flush(c)
        .map_err(|e| format!("{row}: {what}: the write-back failed: {e}"))?;
    step(out.written, row, what, || "nothing was written".into())
}

/// An agent's edit verb on the object named `name`, with a fresh base; exit 0 required.
fn verb(row: &str, s: &Session, c: ConnId, name: &str, a: &[&str]) -> Result<(), String> {
    let k = obj_key(&roots(s, c), name).ok_or_else(|| format!("{row}: no object {name}"))?;
    let addr = format!("@obj:{}", &k[1..]);
    let mut all: Vec<&str> = a[..2].to_vec();
    all.push(&addr);
    all.extend_from_slice(&a[2..]);
    let o = run_based(s, c, &all).map_err(|e| format!("{row}: {all:?}: {e}"))?;
    step(o.exit == 0, row, &format!("{all:?}"), || stdout(&o))
}

// ---------- the store, by hand ----------

/// A list root's keys, in order.
pub fn keys(c: &[(String, Neutral)], root: &str) -> Vec<Key> {
    c.iter()
        .find_map(|(r, n)| match n {
            Neutral::List(l) if r == root => Some(l.iter().map(|(k, _)| k.clone()).collect()),
            _ => None,
        })
        .unwrap_or_default()
}

/// The pairing the kit makes of roots `c`: each list root's keys, in order.
pub fn pairing_of(c: &[(String, Neutral)]) -> Pairing {
    Pairing {
        roots: vec![
            (COLLS.to_owned(), keys(c, COLLS)),
            (OBJS.to_owned(), keys(c, OBJS)),
        ],
    }
}

/// A composite field.
pub fn get(c: &[(String, Neutral)], root: &str, elem: &str, fld: &str) -> Option<String> {
    let k = format!("{elem}\u{1f}{fld}");
    c.iter().find_map(|(r, n)| match n {
        Neutral::Map(m) if r == root => m.get(&k).and_then(Value::as_str).map(str::to_owned),
        _ => None,
    })
}

/// Sets a composite field.
pub fn set(c: &mut [(String, Neutral)], root: &str, elem: &str, fld: &str, v: &str) {
    for (r, n) in c.iter_mut() {
        if let Neutral::Map(m) = n {
            if r == root {
                m.insert(format!("{elem}\u{1f}{fld}"), Value::str(v));
            }
        }
    }
}

/// Removes a list element (its fields stay, as a deleted element's do).
pub fn delete(c: &mut [(String, Neutral)], root: &str, elem: &str) {
    for (r, n) in c.iter_mut() {
        if let Neutral::List(l) = n {
            if r == root {
                l.retain(|(k, _)| k.as_str() != elem);
            }
        }
    }
}

/// Adds a list element at the end.
pub fn insert(c: &mut [(String, Neutral)], root: &str, elem: &str, value: &str) {
    for (r, n) in c.iter_mut() {
        if let Neutral::List(l) = n {
            if r == root {
                l.push((Key::new(elem).expect("a key"), Value::str(value)));
            }
        }
    }
}

/// The key of the object named `name`.
pub fn obj_key(c: &[(String, Neutral)], name: &str) -> Option<String> {
    keys(c, OBJS)
        .into_iter()
        .map(|k| k.as_str().to_owned())
        .find(|k| get(c, OBJ_PROPS, k, "name").as_deref() == Some(name))
}

/// The key of the collection named `name`.
pub fn coll_key(c: &[(String, Neutral)], name: &str) -> Option<String> {
    keys(c, COLLS)
        .into_iter()
        .map(|k| k.as_str().to_owned())
        .find(|k| get(c, COLL_PROPS, k, "name").as_deref() == Some(name))
}

/// The keyed state of roots `c`.
pub fn state_of(c: &[(String, Neutral)]) -> KeyedState {
    KeyedState {
        roots: c
            .iter()
            .filter_map(|(r, n)| {
                let root = match n {
                    Neutral::List(l) => KeyedRoot::List {
                        live: l.clone(),
                        hidden: BTreeMap::new(),
                        places: Vec::new(),
                    },
                    Neutral::Map(m) => KeyedRoot::Map(m.clone()),
                    _ => return None,
                };
                Some((r.clone(), root))
            })
            .collect(),
    }
}

/// A carrier read as the view reads it, with the roots and the pairing an import of it gives.
pub fn carrier(bytes: &[u8]) -> (CarrierView, Vec<(String, Neutral)>, Pairing) {
    let cv = CarrierView::read(bytes, &LiveLimits::default()).expect("the carrier reads");
    let c = BlendLive::default()
        .decode(bytes)
        .expect("the carrier decodes")
        .roots;
    let p = pairing_of(&c);
    (cv, c, p)
}

/// The direct call on roots `c`.
pub fn direct(
    cv: &CarrierView,
    c: &[(String, Neutral)],
    p: &Pairing,
    limits: &MeshReadLimits,
) -> Result<LiveK2Scene, ReuseDeclined> {
    live_k2scene_of(&state_of(c), cv, p, limits)
}

// ---------- the oracle ----------

/// The whole mesh read of `bytes`, keyed by `p`.
pub fn whole(row: &str, what: &str, bytes: &[u8], p: &Pairing) -> Result<Vec<u8>, String> {
    mesh_read_paired(bytes, Some(p), &MeshReadLimits::default())
        .map(|r| r.k2scene)
        .map_err(|e| format!("{row}: {what}: the whole read refused: {e}"))
}

/// `got` is byte for byte `want`.
fn same(row: &str, what: &str, got: &LiveK2Scene, want: &[u8]) -> Result<(), String> {
    let all = got.to_vec();
    step(all == want, row, what, || {
        let at = all.iter().zip(want).position(|(a, b)| a != b);
        let at = at.unwrap_or(all.len().min(want.len()));
        let show = |b: &[u8]| {
            String::from_utf8_lossy(&b[at.saturating_sub(40)..(at + 40).min(b.len())]).into_owned()
        };
        format!(
            "{} bytes, the whole read's {}; first difference at {at}: {:?} / {:?}",
            all.len(),
            want.len(),
            show(&all),
            show(want)
        )
    })
}

/// The header's length field and its pad.
fn pad_of(k: &LiveK2Scene) -> Option<usize> {
    let h = &k.header;
    let hlen = u32::from_le_bytes(h.get(8..12)?.try_into().ok()?) as usize;
    h.len().checked_sub(12 + hlen)
}

// ---------- the rows ----------

/// From the living room, a sequence of live edits through the kit (a place, a turn, a hide, a
/// rename, a parent moved with its children, a camera moved): after each, the call's
/// `header ++ data` is byte for byte the whole read of the version the write-back writes. Then,
/// on the store by hand, what a write-back doesn't write (a collection hidden, an object moved
/// into another collection, one deleted): each the whole read of a generated file holding it.
pub fn row_reuse_parity() -> Result<(), String> {
    const ROW: &str = "row_reuse_parity";
    let (f, s, h) = open_hooked("reuse-parity", &living_room());
    let a = f.agent(&s);
    let got = ok(ROW, "control", call(ROW, "control", &s, a, &h)?)?;
    let want = whole(ROW, "control", &f.bytes(), &pairing_of(&roots(&s, a)))?;
    same(ROW, "control", &got, &want)?;
    let edits: &[(&str, &str, &[&str])] = &[
        (
            "place",
            "Sofa",
            &["object", "place", "--loc", "-1.5,-1,0.1"],
        ),
        (
            "turn",
            "Chair.002",
            &["object", "place", "--rot", "0,0,2.5"],
        ),
        (
            "hide",
            "Rug",
            &["object", "visibility", "--viewport", "off"],
        ),
        (
            "rename",
            "Table",
            &[
                "object",
                "rename",
                "--name",
                "Dining table",
                "--break-links",
            ],
        ),
        (
            "parent moved",
            "Shelf",
            &["object", "place", "--loc", "2,2,0.2"],
        ),
        (
            "camera moved",
            "Camera",
            &["object", "place", "--loc", "-3,-3,1.5"],
        ),
    ];
    let mut checked = 0;
    for (what, name, a_) in edits {
        verb(ROW, &s, a, name, a_)?;
        let got = ok(ROW, what, call(ROW, what, &s, a, &h)?)?;
        flush(ROW, what, &s, a)?;
        let want = whole(ROW, what, &f.bytes(), &pairing_of(&roots(&s, a)))?;
        same(ROW, what, &got, &want)?;
        checked += 1;
    }
    // what a write-back doesn't write, on the store by hand
    let (cv, mut c, p) = carrier(&living_room());
    let props = coll_key(&c, "Props").ok_or(format!("{ROW}: no Props"))?;
    let room = coll_key(&c, "Room").ok_or(format!("{ROW}: no Room"))?;
    let mug = obj_key(&c, "Mug").ok_or(format!("{ROW}: no Mug"))?;
    let chair = obj_key(&c, "Chair.004").ok_or(format!("{ROW}: no Chair.004"))?;
    let l = MeshReadLimits::default();
    set(&mut c, COLL_PROPS, &props, "hidden", "1");
    let mut e = RoomEdits {
        props_hidden: true,
        ..RoomEdits::default()
    };
    let got = direct(&cv, &c, &p, &l).map_err(|d| format!("{ROW}: collection hidden: {d}"))?;
    same(
        ROW,
        "collection hidden",
        &got,
        &whole(ROW, "collection hidden", &living_room_edited(&e), &p)?,
    )?;
    set(&mut c, OBJ_PROPS, &mug, "coll", &room);
    e.to_room = Some("Mug");
    let got = direct(&cv, &c, &p, &l).map_err(|d| format!("{ROW}: moved: {d}"))?;
    same(
        ROW,
        "moved",
        &got,
        &whole(ROW, "moved", &living_room_edited(&e), &p)?,
    )?;
    delete(&mut c, OBJS, &chair);
    e.without = Some("Chair.004");
    let got = direct(&cv, &c, &p, &l).map_err(|d| format!("{ROW}: deleted: {d}"))?;
    let left = pairing_of(&c);
    same(
        ROW,
        "deleted",
        &got,
        &whole(ROW, "deleted", &living_room_edited(&e), &left)?,
    )?;
    checked += 3;
    println!("{ROW}: {checked} states byte for byte the whole read");
    Ok(())
}

/// Renames that grow the header a byte at a time, so its pad takes each of 0, 1, 2 and 3:
/// after each, byte for byte the whole read of the written-back version.
pub fn row_reuse_header_lengths() -> Result<(), String> {
    const ROW: &str = "row_reuse_header_lengths";
    let (f, s, h) = open_hooked("reuse-lengths", &living_room());
    let a = f.agent(&s);
    let got = ok(ROW, "control", call(ROW, "control", &s, a, &h)?)?;
    let mut pads = vec![pad_of(&got)];
    let mut name = "Sofa".to_owned();
    for next in ["SofaA", "SofaAB", "SofaABC", "SofaABCD"] {
        let what = format!("rename to {next}");
        verb(
            ROW,
            &s,
            a,
            &name,
            &["object", "rename", "--name", next, "--break-links"],
        )?;
        name = next.to_owned();
        let got = ok(ROW, &what, call(ROW, &what, &s, a, &h)?)?;
        flush(ROW, &what, &s, a)?;
        let want = whole(ROW, &what, &f.bytes(), &pairing_of(&roots(&s, a)))?;
        same(ROW, &what, &got, &want)?;
        step(got.header.len() % 4 == 0, ROW, &what, || {
            format!("the header part is {} bytes", got.header.len())
        })?;
        pads.push(pad_of(&got));
    }
    let mut seen: Vec<usize> = pads.iter().flatten().copied().collect();
    seen.sort_unstable();
    seen.dedup();
    step(seen == [0, 1, 2, 3], ROW, "every pad", || {
        format!("pads {pads:?}")
    })?;
    println!("{ROW}: pads {pads:?}");
    Ok(())
}

/// SHA-256 of `data`, lowercase hex, worked out here.
fn sha_hex(data: &[u8]) -> String {
    Digest::of(data)
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// The carrier id: the same on one carrier, and on a written-back carrier whose meshes are the
/// same; another after an outside save whose mesh differs (the same counts, other positions),
/// taken in on a session of its own (the kit keeps both when a save follows its own write) as
/// the carrier (with no store edit, the kit says Unchanged);
/// always SHA-256 of the data section.
pub fn row_reuse_carrier_id() -> Result<(), String> {
    const ROW: &str = "row_reuse_carrier_id";
    let (f, s, h) = open_hooked("reuse-id", &living_room());
    let a = f.agent(&s);
    let r0 = ok(ROW, "control", call(ROW, "control", &s, a, &h)?)?;
    step(
        r0.carrier_id.len() == 64 && r0.carrier_id.bytes().all(|b| b.is_ascii_hexdigit()),
        ROW,
        "control",
        || format!("id {:?}", r0.carrier_id),
    )?;
    verb(ROW, &s, a, "Sofa", &["object", "place", "--loc", "1,1,0"])?;
    let r1 = ok(ROW, "one carrier", call(ROW, "one carrier", &s, a, &h)?)?;
    step(r1.carrier_id == r0.carrier_id, ROW, "one carrier", || {
        format!("{} then {}", r0.carrier_id, r1.carrier_id)
    })?;
    flush(ROW, "write-back", &s, a)?;
    let r2 = ok(ROW, "write-back", call(ROW, "write-back", &s, a, &h)?)?;
    step(
        r2.carrier_id == r0.carrier_id && r2.data[..] == r0.data[..],
        ROW,
        "write-back",
        || {
            format!(
                "{} after the write-back, {} before",
                r2.carrier_id, r0.carrier_id
            )
        },
    )?;
    // an outside save onto a doc the kit hasn't written: taken in as the new carrier
    let (f, s, h) = open_hooked("reuse-id-save", &living_room());
    let a = f.agent(&s);
    let r = ok(
        ROW,
        "second session",
        call(ROW, "second session", &s, a, &h)?,
    )?;
    step(r.carrier_id == r0.carrier_id, ROW, "second session", || {
        format!(
            "{} on the same bytes, {} before",
            r.carrier_id, r0.carrier_id
        )
    })?;
    let save = living_room_edited(&RoomEdits {
        grown: Some("Rug"),
        ..RoomEdits::default()
    });
    app_save(&f.path(), &save);
    let taken = s
        .file_changed()
        .map_err(|e| format!("{ROW}: outside save: {e}"))?;
    // a save whose store content is the doc's (only a mesh differs) has no edit to ingest: the
    // kit takes its bytes as the carrier and says Unchanged; the steps below show it was taken
    step(
        matches!(taken, DiskCheck::Ingested { .. } | DiskCheck::Unchanged),
        ROW,
        "outside save",
        || format!("the save isn't taken in: {taken:?}"),
    )?;
    let r3 = ok(ROW, "new meshes", call(ROW, "new meshes", &s, a, &h)?)?;
    step(
        r3.data[..] != r0.data[..] && r3.data.len() == r0.data.len(),
        ROW,
        "setup",
        || "the save's data section isn't another of the same length".into(),
    )?;
    step(r3.carrier_id != r0.carrier_id, ROW, "new meshes", || {
        format!("the id stayed {}", r0.carrier_id)
    })?;
    let want = whole(ROW, "new meshes", &f.bytes(), &pairing_of(&roots(&s, a)))?;
    same(ROW, "new meshes", &r3, &want)?;
    for (what, r) in [("control", &r0), ("new meshes", &r3)] {
        step(r.carrier_id == sha_hex(&r.data), ROW, "sha-256", || {
            format!("{what}: {} isn't SHA-256 of the data", r.carrier_id)
        })?;
    }
    println!("{ROW}: ids {} and {}", r0.carrier_id, r3.carrier_id);
    Ok(())
}

/// The calls over one carrier share one data section (the same allocation), and it is the
/// whole read's.
pub fn row_reuse_shared_data() -> Result<(), String> {
    const ROW: &str = "row_reuse_shared_data";
    let (f, s, h) = open_hooked("reuse-shared", &living_room());
    let a = f.agent(&s);
    let r0 = ok(ROW, "control", call(ROW, "control", &s, a, &h)?)?;
    let want = whole(ROW, "control", &f.bytes(), &pairing_of(&roots(&s, a)))?;
    let at = want.len() - r0.data.len();
    step(r0.data[..] == want[at..], ROW, "control", || {
        "the data isn't the whole read's".into()
    })?;
    let mut all = vec![r0];
    for (name, loc) in [("Sofa", "1,1,0"), ("Chair.001", "0.5,0,0")] {
        verb(ROW, &s, a, name, &["object", "place", "--loc", loc])?;
        all.push(ok(ROW, name, call(ROW, name, &s, a, &h)?)?);
    }
    step(
        all.windows(2).all(|w| Arc::ptr_eq(&w[0].data, &w[1].data)),
        ROW,
        "shared",
        || format!("{} calls on one carrier don't share their data", all.len()),
    )?;
    println!(
        "{ROW}: {} calls share one data section of {} bytes",
        all.len(),
        all[0].data.len()
    );
    Ok(())
}

/// No file is decoded during a call: the reader's count stays 0 across a sequence of edits.
pub fn row_reuse_no_decode() -> Result<(), String> {
    const ROW: &str = "row_reuse_no_decode";
    let (f, s, h) = open_hooked("reuse-decode", &living_room());
    let a = f.agent(&s);
    let first = call(ROW, "control", &s, a, &h)?;
    let mut reads = vec![first.reads];
    ok(ROW, "control", first)?;
    let edits: &[(&str, &[&str])] = &[
        ("Sofa", &["object", "place", "--loc", "1,1,0"]),
        ("Rug", &["object", "visibility", "--viewport", "off"]),
        (
            "Vase",
            &["object", "rename", "--name", "Urn", "--break-links"],
        ),
    ];
    for (name, a_) in edits {
        verb(ROW, &s, a, name, a_)?;
        let g = call(ROW, name, &s, a, &h)?;
        reads.push(g.reads);
        ok(ROW, name, g)?;
    }
    step(reads.iter().all(|&r| r == 0), ROW, "no decode", || {
        format!("files read per call: {reads:?}")
    })?;
    println!(
        "{ROW}: {} calls, files read per call {reads:?}",
        reads.len()
    );
    Ok(())
}

/// A scene for the lists: `S` holds `A` and `B` (sharing `Cube`) and its camera `Cam`; `C`
/// (a `Cube` too) is in no collection.
pub fn lists_scene() -> Vec<u8> {
    let mut b = SceneBuilder::meshes(402, MeshLayouts::LAYERS);
    b.file_global(Some("S"));
    b.scene("S", &["A", "B", "Cam"], &[], Some("Cam"));
    let mesh = |x: f32| ObjectSpec {
        data: Some("ME:Cube".into()),
        ty: 1,
        rotmode: 1,
        loc: [x, 0.0, 0.0],
        scale: [1.0; 3],
        ..ObjectSpec::default()
    };
    b.object("A", &mesh(0.0));
    b.object("B", &mesh(2.0));
    b.object("C", &mesh(4.0));
    b.object(
        "Cam",
        &ObjectSpec {
            data: Some("CA:Cam".into()),
            ty: 11,
            rotmode: 1,
            loc: [0.0, -8.0, 2.0],
            rot: [1.4, 0.0, 0.0],
            scale: [1.0; 3],
            ..ObjectSpec::default()
        },
    );
    b.camera_with("Cam", &CameraSpec::default());
    b.mesh_with("Cube", &box_mesh([-1.0; 3], [1.0; 3]), MeshLayouts::LAYERS);
    b.bytes()
}

/// The lists, against the carrier's own read: none on the carrier's state; an object moved, its
/// camera moved (changed); an object brought into the scene drawn (added); one deleted.
pub fn row_reuse_key_lists() -> Result<(), String> {
    const ROW: &str = "row_reuse_key_lists";
    let (cv, mut c, p) = carrier(&lists_scene());
    let l = MeshReadLimits::default();
    let addr = |k: &str| format!("@obj:{}", &k[1..]);
    let lists = |r: &LiveK2Scene| (r.changed.clone(), r.added.clone(), r.deleted.clone());
    let r0 = direct(&cv, &c, &p, &l).map_err(|d| format!("{ROW}: control: {d}"))?;
    let want = whole(ROW, "control", &lists_scene(), &p)?;
    same(ROW, "control", &r0, &want)?;
    step(
        lists(&r0) == (vec![], vec![], vec![]),
        ROW,
        "unchanged",
        || format!("{:?}", lists(&r0)),
    )?;
    let (a, b, cc, cam) = (
        obj_key(&c, "A").ok_or("no A")?,
        obj_key(&c, "B").ok_or("no B")?,
        obj_key(&c, "C").ok_or("no C")?,
        obj_key(&c, "Cam").ok_or("no Cam")?,
    );
    let master = coll_key(&c, "S").ok_or("no S")?;
    let cam_addr = format!("@cam:{}", &cam[1..]);
    set(&mut c, OBJ_PROPS, &a, "xform", "0,1,0,0,0,0,1,1,1");
    let r = direct(&cv, &c, &p, &l).map_err(|d| format!("{ROW}: moved: {d}"))?;
    step(
        lists(&r) == (vec![addr(&a)], vec![], vec![]),
        ROW,
        "moved",
        || format!("{:?}", lists(&r)),
    )?;
    set(&mut c, OBJ_PROPS, &cam, "xform", "0,-9,2,1.4,0,0,1,1,1");
    let r = direct(&cv, &c, &p, &l).map_err(|d| format!("{ROW}: camera: {d}"))?;
    step(
        lists(&r) == (vec![addr(&a), cam_addr.clone()], vec![], vec![]),
        ROW,
        "camera",
        || format!("{:?}", lists(&r)),
    )?;
    set(&mut c, OBJ_PROPS, &cc, "coll", &master);
    let r = direct(&cv, &c, &p, &l).map_err(|d| format!("{ROW}: added: {d}"))?;
    step(
        lists(&r) == (vec![addr(&a), cam_addr.clone()], vec![addr(&cc)], vec![]),
        ROW,
        "added",
        || format!("{:?}", lists(&r)),
    )?;
    delete(&mut c, OBJS, &b);
    let r = direct(&cv, &c, &p, &l).map_err(|d| format!("{ROW}: deleted: {d}"))?;
    step(
        lists(&r) == (vec![addr(&a), cam_addr], vec![addr(&cc)], vec![addr(&b)]),
        ROW,
        "deleted",
        || format!("{:?}", lists(&r)),
    )?;
    println!(
        "{ROW}: changed {:?}, added {:?}, deleted {:?}",
        r.changed, r.added, r.deleted
    );
    Ok(())
}

/// A grid of `n` by `n` vertices, `(n - 1)^2` quads.
pub fn grid(n: u32) -> MeshSpec {
    let mut positions = Vec::with_capacity((n * n) as usize);
    for j in 0..n {
        for i in 0..n {
            positions.push([i as f32, j as f32, 0.0]);
        }
    }
    let mut faces = Vec::with_capacity(((n - 1) * (n - 1)) as usize);
    for j in 0..n - 1 {
        for i in 0..n - 1 {
            let a = j * n + i;
            faces.push(vec![a, a + 1, a + n + 1, a + n]);
        }
    }
    MeshSpec {
        positions,
        faces,
        ..MeshSpec::default()
    }
}

/// The grid's side: 100,489 vertices, 199,712 triangles.
pub const GRID: u32 = 317;

/// `objects` objects sharing one grid mesh of [`GRID`] by [`GRID`] vertices.
pub fn grid_scene(objects: usize) -> Vec<u8> {
    let mut b = SceneBuilder::meshes(402, MeshLayouts::LAYERS);
    b.file_global(Some("S"));
    let names: Vec<String> = (1..=objects).map(|i| format!("G{i}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    b.scene("S", &refs, &[], None);
    for (i, n) in names.iter().enumerate() {
        b.object(
            n,
            &ObjectSpec {
                data: Some("ME:Grid".into()),
                ty: 1,
                rotmode: 1,
                loc: [0.0, 0.0, i as f32],
                scale: [1.0; 3],
                ..ObjectSpec::default()
            },
        );
    }
    b.mesh_with("Grid", &grid(GRID), MeshLayouts::LAYERS);
    b.bytes()
}

/// `got` is declined as `want` says, with the fallback in its text.
fn declined(
    row: &str,
    what: &str,
    got: Result<LiveK2Scene, ReuseDeclined>,
    code: &str,
    want: impl FnOnce(&ReuseDeclined) -> bool,
) -> Result<(), String> {
    let d = match got {
        Ok(k) => return Err(format!("{row}: {what}: not declined: {k:?}")),
        Err(d) => d,
    };
    step(
        d.code() == code && want(&d) && d.to_string().contains("read the written-back version"),
        row,
        what,
        || format!("declined {} ({d}), want {code}", d.code()),
    )
}

/// Each decline, typed, its fallback named: no carrier read, a pairing that doesn't fit, a mesh
/// the carrier's read doesn't hold, an object the carrier has no facts for, the scene drawn's
/// master collection deleted, a collection's parent changed, a mesh of the carrier's read no object uses, a
/// change the header can't take (a parent deleted, a rotation mode, the scene's camera
/// deleted), a limit crossed.
pub fn row_reuse_declines() -> Result<(), String> {
    const ROW: &str = "row_reuse_declines";
    let room = living_room();
    let (cv, c0, p) = carrier(&room);
    let l = MeshReadLimits::default();
    let got = direct(&cv, &c0, &p, &l);
    step(got.is_ok(), ROW, "control", || format!("{got:?}"))?;
    let k = |n: &str| obj_key(&c0, n).unwrap_or_default();
    let addr = |n: &str| format!("@obj:{}", &k(n)[1..]);
    let coll = |n: &str| coll_key(&c0, n).unwrap_or_default();
    let chair_data = get(&c0, OBJ_PROPS, &k("Chair.001"), "data").unwrap_or_default();

    // the pairing: one object key short
    let mut short = p.clone();
    for (r, keys) in &mut short.roots {
        if r == OBJS {
            keys.pop();
        }
    }
    declined(
        ROW,
        "pairing",
        direct(&cv, &c0, &short, &l),
        "reuse_pairing",
        |d| matches!(d, ReuseDeclined::Pairing { .. }),
    )?;
    // an object added in Room: a mesh the carrier's read doesn't hold, then one it holds
    let added = |data: &str, n: u64| {
        let mut c = c0.clone();
        let key = format!("o{n}");
        insert(&mut c, OBJS, &key, "mesh");
        for (fld, v) in [
            ("coll", coll("Room").as_str()),
            ("data", data),
            ("name", "New"),
            ("xform", "0,0,0,0,0,0,1,1,1"),
            ("rotmode", "xyz"),
            ("hidden", "0"),
        ] {
            set(&mut c, OBJ_PROPS, &key, fld, v);
        }
        c
    };
    declined(
        ROW,
        "mesh not held",
        direct(&cv, &added("mesh @me:Nope", 900), &p, &l),
        "reuse_mesh_not_held",
        |d| matches!(d, ReuseDeclined::MeshNotHeld { key } if key == "@obj:900"),
    )?;
    declined(
        ROW,
        "no facts: added",
        direct(&cv, &added(&chair_data, 901), &p, &l),
        "reuse_no_facts",
        |d| matches!(d, ReuseDeclined::NoFacts { key } if key == "@obj:901"),
    )?;
    let mut c = c0.clone();
    set(&mut c, OBJ_PROPS, &k("Sofa"), "data", &chair_data);
    declined(
        ROW,
        "no facts: data",
        direct(&cv, &c, &p, &l),
        "reuse_no_facts",
        |d| matches!(d, ReuseDeclined::NoFacts { key } if *key == addr("Sofa")),
    )?;
    let mut c = c0.clone();
    delete(&mut c, COLLS, &coll("Scene"));
    declined(
        ROW,
        "drawn scene",
        direct(&cv, &c, &p, &l),
        "reuse_drawn_scene",
        |d| matches!(d, ReuseDeclined::DrawnScene { .. }),
    )?;
    let mut c = c0.clone();
    set(&mut c, COLL_PROPS, &coll("Props"), "parent", &coll("Room"));
    declined(
        ROW,
        "collections",
        direct(&cv, &c, &p, &l),
        "reuse_collections",
        |d| matches!(d, ReuseDeclined::Collections { .. }),
    )?;
    let mut c = c0.clone();
    delete(&mut c, OBJS, &k("Rug"));
    declined(
        ROW,
        "meshes changed",
        direct(&cv, &c, &p, &l),
        "reuse_meshes_changed",
        |d| matches!(d, ReuseDeclined::MeshesChanged),
    )?;
    let mut c = c0.clone();
    delete(&mut c, OBJS, &k("Shelf"));
    declined(
        ROW,
        "parent deleted",
        direct(&cv, &c, &p, &l),
        "reuse_unsupported",
        |d| matches!(d, ReuseDeclined::Unsupported { key, .. } if *key == addr("Book.001")),
    )?;
    let mut c = c0.clone();
    set(&mut c, OBJ_PROPS, &k("Sofa"), "rotmode", "quaternion");
    declined(
        ROW,
        "rotation mode",
        direct(&cv, &c, &p, &l),
        "reuse_unsupported",
        |d| matches!(d, ReuseDeclined::Unsupported { key, .. } if *key == addr("Sofa")),
    )?;
    let mut c = c0.clone();
    delete(&mut c, OBJS, &k("Camera"));
    declined(
        ROW,
        "scene camera",
        direct(&cv, &c, &p, &l),
        "reuse_unsupported",
        |d| matches!(d, ReuseDeclined::Unsupported { key, .. } if *key == addr("Camera")),
    )?;
    let mut tight = MeshReadLimits::default();
    tight.max_objects = 3;
    declined(
        ROW,
        "limit",
        direct(&cv, &c0, &p, &tight),
        "too_large",
        |d| {
            matches!(
                d,
                ReuseDeclined::Refused(MeshReadError::TooLarge {
                    what: "objects",
                    ..
                })
            )
        },
    )?;
    // a carrier whose own read refuses: more drawn vertices than the read writes
    let (big, bc, bp) = carrier(&grid_scene(81));
    declined(
        ROW,
        "no carrier read",
        direct(&big, &bc, &bp, &l),
        "reuse_no_carrier_read",
        |d| matches!(d, ReuseDeclined::NoCarrierRead),
    )?;
    println!("{ROW}: 12 declines, each typed");
    Ok(())
}
