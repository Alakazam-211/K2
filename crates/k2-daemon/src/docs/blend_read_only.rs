//! `.blend` files are read-only in K2 for now (the 3D scene core), behind
//! one switch: [`BLEND_WRITER`].
//!
//! While [`BLEND_WRITER`] is false (0.46.15), K2 writes no `.blend` at all: it reads them (the live
//! scene, the mesh read the previews draw from, the index and the pack's read
//! verbs) and refuses everything that would write one, with one typed reason,
//! [`CODE`]. Three layers, each enough on its own:
//!
//! 1. **Rights** ([`cap`], used by `host::rights_for`, which the kit asks, and
//!    by the routes' own rights): no one has edit on a `.blend` while the
//!    blend core is on, so the kit refuses every edit, write, restore, undo
//!    and revert, live or tier 0, before it applies.
//! 2. **The routes** ([`refused`]): those calls, and the pack's edit verbs
//!    (`object place`, `object visibility`, `object rename`), are refused by
//!    K2 first with `409 blend_writer_disabled` (exit 3), so callers get the
//!    reason in words.
//! 3. **The packs** ([`SceneReadOnly`], [`BlendIndexOnly`]): the live pack
//!    renders only what the file already holds (a render that would change a
//!    byte is an error), never builds a file from scratch, and the tier-0
//!    pack never merges (an outside save that would need a merge keeps both,
//!    the kit's conflict copy) and lets no agent write.
//!
//! While the blend core is off a `.blend` is an any-file doc, as before the
//! core: no BLEND pack is involved and nothing here applies.
//!
//! **Turning the writer on** is [`BLEND_WRITER`] = `true`: then nobody's
//! rights are capped, nothing is refused, the live and tier-0 docs open with
//! the BLEND pack itself ([`live_pack`], [`blob_pack`]: its splice, merge
//! and agent writes), a scene's welcome keeps the edit right and lists
//! `claims`, and has no `read_only_reason`.

use std::io::Read;

use k2_core::docs_cores;
use k2_core::product_name;
use k2_mpc_base::doc::Neutral;
use k2_mpc_base::elem::{Applied, Key};
use k2_mpc_base::index::{IndexDraft, IndexLimits, IndexRefused, IndexSpec};
use k2_mpc_base::placement::{Placement, Space};
use k2_mpc_base::schema::Schema;
use k2_mpc_base::session::{
    AgentWrites, BlobMerge, BlobPack, Declined, Pack, PackContent, PackError, Pairing, Reconciled, SaveMerge, SpliceError, VerbState, View,
};
use k2_mpc_base::view_index::{ViewIndex, ViewSchema, ViewSource, ViewTouched};
use k2_mpc_blend::live::BlendLive;
use k2_mpc_blend::BlendPack;

use super::blob_packs::{kind_by_name, BlobKind};
use crate::cli_response::CliResponse;

/// The one switch for writing `.blend` files: off in 0.46.15. Everything
/// in this module follows [`writer_on`].
pub const BLEND_WRITER: bool = false;

/// Whether K2 writes `.blend` files: [`BLEND_WRITER`] (unit tests may turn it
/// on for their own process, [`set_writer_for_test`]).
pub fn writer_on() -> bool {
    #[cfg(test)]
    if let Some(on) = *test_writer().lock().unwrap_or_else(|p| p.into_inner()) {
        return on;
    }
    BLEND_WRITER
}

#[cfg(test)]
fn test_writer() -> &'static std::sync::Mutex<Option<bool>> {
    static W: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
    &W
}

/// Test-only: the writer on or off for this process (`None`: the switch).
#[cfg(test)]
pub fn set_writer_for_test(on: Option<bool>) {
    *test_writer().lock().unwrap_or_else(|p| p.into_inner()) = on;
}

/// The live pack a scene opens with: the BLEND pack when the writer is on,
/// else [`SceneReadOnly`].
pub fn live_pack() -> std::sync::Arc<dyn Pack> {
    if writer_on() {
        std::sync::Arc::new(BlendLive::default())
    } else {
        std::sync::Arc::new(SceneReadOnly::default())
    }
}

/// The tier-0 pack a `.blend` opens with: the BLEND pack when the writer is
/// on, else [`BlendIndexOnly`].
pub fn blob_pack() -> std::sync::Arc<dyn BlobPack> {
    if writer_on() {
        std::sync::Arc::new(BlendPack::default())
    } else {
        std::sync::Arc::new(BlendIndexOnly::default())
    }
}

/// What a scene's live welcome adds: `read_only_reason` while it is
/// read-only ([`writes_off`]), nothing once K2 writes it.
pub fn welcome_fields(rel: &str) -> Vec<(&'static str, &'static str)> {
    if writes_off(rel) {
        vec![(WELCOME_FIELD, WHY)]
    } else {
        Vec::new()
    }
}

/// The refusal's code.
pub const CODE: &str = "blend_writer_disabled";

/// The refusal's words, on every write path and as the live welcome's
/// `read_only_reason` ([`WELCOME_FIELD`]).
pub const WHY: &str = concat!(product_name!(), " can't save .blend edits yet; this file opens read-only.");

/// The live welcome's field that says why a scene is read-only (its
/// `rights` is `read`), so a viewer shows it from the start.
pub const WELCOME_FIELD: &str = "read_only_reason";

/// Whether `rel` is a `.blend` K2 must not write now: a `.blend` while the
/// blend core is on and the writer off ([`writer_on`]).
pub fn writes_off(rel: &str) -> bool {
    !writer_on() && kind_by_name(rel) == BlobKind::Blend && super::cores::is_on(docs_cores::BLEND)
}

/// `r` with no edit on a `.blend` K2 must not write ([`writes_off`]):
/// reading, history and comments (which never write the file) stay.
pub fn cap(rel: &str, r: super::host::DocRights) -> super::host::DocRights {
    if writes_off(rel) {
        super::host::DocRights { edit: false, comment: r.comment || r.edit, ..r }
    } else {
        r
    }
}

/// Whether `argv` is one of the live pack's edit verbs (by its own registry:
/// a verb that isn't a structured read).
pub fn is_edit_verb(argv: &[String]) -> bool {
    let two = argv.iter().take(2).map(String::as_str).collect::<Vec<_>>().join(" ");
    let one = argv.first().map(String::as_str).unwrap_or("");
    let verbs = k2_mpc_blend::live::verbs::VERBS;
    let found = verbs.iter().find(|v| v.name() == two).or_else(|| verbs.iter().find(|v| v.name() == one));
    found.is_some_and(|v| v.kind() == k2_mpc_base::tools::Kind::Edit)
}

/// The refusal: `409 blend_writer_disabled`, exit 3, its hint [`WHY`]
/// word for word, and the file as `path`.
pub fn refusal(rel: &str) -> CliResponse {
    let mut r = super::routes::with_exit(super::caller::refusal("409 Conflict", CODE, WHY), k2_mpc_base::tools::exit::REFUSED);
    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&r.body) {
        if let Some(e) = v.get_mut("error").and_then(serde_json::Value::as_object_mut) {
            e.insert("path".into(), serde_json::Value::from(rel));
            r.body = v.to_string();
        }
    }
    r
}

/// A call that would write `rel` while it is read-only: the edit routes
/// (`edit`, `write`, `undo`, `redo`, `restore`, `revert`, `use-theirs`) and
/// the live pack's edit verbs through `/cli/docs/tool` (`argv`).
pub fn refused(path: &str, rel: &str, writes: bool, argv: Option<&[String]>) -> Option<CliResponse> {
    if !writes_off(rel) {
        return None;
    }
    let tool_edits = path == super::routes::TOOL && argv.is_some_and(is_edit_verb);
    (writes || tool_edits).then(|| refusal(rel))
}

/// A scene socket's reply to a person's `edit`, `undo` or `redo` on a `.blend`
/// K2 must not write ([`writes_off`]): `refused {op}` with the kit's
/// `MPC-E113`, [`WHY`] as its words and [`CODE`] in `details.code` (with the
/// hint), so the person reads the same reason as on every write route.
/// `None` for any other frame or file: the keyed socket's own refusal
/// (`live_deck::refuse`) answers those. Either way nothing reaches the kit.
pub fn socket_refusal(f: &super::live_wire::ClientFrame, rel: &str) -> Option<k2_mpc_base::errors::Value> {
    use super::live_wire::{self as wire, ClientFrame};
    let op = match f {
        ClientFrame::Edit { op, .. } | ClientFrame::Undo { op } | ClientFrame::Redo { op } => *op,
        _ => return None,
    };
    if !writes_off(rel) {
        return None;
    }
    Some(wire::refused_frame(op, &wire::report("MPC-E113", WHY).with("code", CODE).with("hint", WHY)))
}

/// The live BLEND pack, read-only: every read is `BlendLive`'s; a render
/// that would change the file is an error, and a file is never built from
/// content alone.
#[derive(Debug, Default)]
pub struct SceneReadOnly(pub BlendLive);

fn writer_off() -> PackError {
    PackError(format!("{CODE}: {WHY}"))
}

impl Pack for SceneReadOnly {
    fn schema(&self) -> Schema {
        self.0.schema()
    }

    fn decode(&self, bytes: &[u8]) -> Result<PackContent, Declined> {
        self.0.decode(bytes)
    }

    /// Never: a file built from content alone is the pack's writer.
    fn encode(&self, _content: &[(String, Neutral)], _codec: &[u8]) -> Result<Vec<u8>, PackError> {
        Err(writer_off())
    }

    fn wants_carrier(&self) -> bool {
        self.0.wants_carrier()
    }

    fn wants_pairing(&self) -> bool {
        self.0.wants_pairing()
    }

    /// The carrier as it is, only: what the doc renders to when nothing has
    /// changed (every read's bytes). A render that would change a byte is
    /// refused, so no write-back ever reaches the file.
    fn encode_from_paired(&self, carrier: &[u8], pairing: &Pairing, content: &[(String, Neutral)], codec: &[u8]) -> Result<Vec<u8>, SpliceError> {
        let out = self.0.encode_from_paired(carrier, pairing, content, codec)?;
        if out != carrier {
            return Err(SpliceError::Pack(writer_off()));
        }
        Ok(out)
    }

    fn view_schema(&self) -> Option<ViewSchema> {
        self.0.view_schema()
    }

    fn view_derive(&self, source: ViewSource<'_>, index: &mut ViewIndex) {
        self.0.view_derive(source, index)
    }

    fn view_derive_change(&self, source: ViewSource<'_>, index: &mut ViewIndex, touched: &ViewTouched) -> bool {
        self.0.view_derive_change(source, index, touched)
    }

    fn view_placement(&self, source: ViewSource<'_>, root: &str, key: &Key) -> Option<Placement> {
        self.0.view_placement(source, root, key)
    }

    fn view_space(&self, source: ViewSource<'_>) -> Option<Space> {
        self.0.view_space(source)
    }

    fn address_of(&self, root: &str, key: &Key) -> String {
        self.0.address_of(root, key)
    }

    fn key_of(&self, root: &str, address: &str) -> Option<Key> {
        self.0.key_of(root, address)
    }

    fn reconcile(&self, synced: &View, bytes: &[u8], codec: &[u8]) -> Result<Reconciled, Declined> {
        self.0.reconcile(synced, bytes, codec)
    }

    fn verbs(&self) -> &'static [k2_mpc_base::tools::PackVerb] {
        self.0.verbs()
    }

    fn errors(&self) -> &'static [k2_mpc_base::errors::ErrorEntry] {
        self.0.errors()
    }

    fn verb_state_build(&self, view: &View) -> Option<Box<dyn VerbState>> {
        self.0.verb_state_build(view)
    }

    fn verb_state_change(&self, state: &mut (dyn VerbState + 'static), applied: &[Applied], view: &View) -> bool {
        self.0.verb_state_change(state, applied, view)
    }

    fn composite(&self) -> &'static [(&'static str, &'static str)] {
        self.0.composite()
    }
}

/// The tier-0 BLEND pack, index only: its index (and so its verbs) is
/// `BlendPack`'s; it never merges (a save that would need one keeps both)
/// and lets no agent write.
#[derive(Debug, Default)]
pub struct BlendIndexOnly(pub BlendPack);

impl BlobPack for BlendIndexOnly {
    fn can_merge(&self) -> bool {
        false
    }

    /// Keep both: a merged file is the pack's writer.
    fn merge3_bytes(&self, _base: &[u8], _ours: &[u8], _theirs: &[u8]) -> BlobMerge {
        BlobMerge::Conflict
    }

    fn merge3_known(&self, _m: &SaveMerge<'_>) -> BlobMerge {
        BlobMerge::Conflict
    }

    fn index_spec(&self) -> Option<IndexSpec> {
        self.0.index_spec()
    }

    fn index(&self, src: &mut dyn Read, size: u64, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
        self.0.index(src, size, limits)
    }

    fn agent_writes(&self) -> AgentWrites {
        AgentWrites::Refused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `content` with one object's viewport flag flipped (what `object
    /// visibility` does): the first object, in the pairing's order, whose
    /// flip the BLEND pack's own splice writes (a linked object has no file
    /// key to write). Panics when none is.
    fn first_hidden_flipped(bytes: &[u8], content: &[(String, Neutral)], pairing: &Pairing) -> Vec<(String, Neutral)> {
        let objs = pairing.keys(k2_mpc_blend::live::OBJS).expect("the objects' keys");
        let mut errs = Vec::new();
        for o in objs {
            let key = k2_mpc_blend::live::prop(o, k2_mpc_blend::live::f::HIDDEN);
            let mut out = content.to_vec();
            let props = out
                .iter_mut()
                .find(|(r, _)| r == k2_mpc_blend::live::OBJ_PROPS)
                .and_then(|(_, n)| match n {
                    Neutral::Map(m) => Some(m),
                    _ => None,
                })
                .expect("the objects' fields");
            let flipped = if props.get(&key) == Some(&k2_mpc_base::elem::Value::str("1")) { "0" } else { "1" };
            props.insert(key, k2_mpc_base::elem::Value::str(flipped));
            match BlendLive::default().encode_from_paired(bytes, pairing, &out, &[1]) {
                Ok(_) => return out,
                Err(e) => errs.push(format!("{o:?}: {e}")),
            }
        }
        panic!("the BLEND pack's splice wrote no object's visibility flip: {errs:?}");
    }

    use k2_mpc_base::session::{
        AuditEvent, Config, ConnId, DiskState, Host, Need, Participant, ParticipantKind, Rights, Session, SessionError, SessionManager, Stay,
    };
    use k2_mpc_base::store::{DocKey, DocStore};
    use k2_mpc_base::tools::{ToolOptions, ToolOutput, ToolRunError};
    use std::sync::Arc;

    /// The kit's host for a live scene: K2's own rule (`K2Host::rights` is
    /// `host::rights_for`, layer 1), or (`false`) one that lets everyone
    /// edit: what any path that skipped layer 1 would see.
    struct LayerOne(bool);

    impl Host for LayerOne {
        fn rights(&self, who: &Participant, key: &DocKey) -> Rights {
            if self.0 {
                super::super::host::rights_for(&who.pid, key, None).kit()
            } else {
                Rights::editor()
            }
        }
        fn now_ms(&self) -> u64 {
            super::super::host::now_ms()
        }
        fn on_event(&self, _: &AuditEvent) {}
    }

    /// The workspace the scene is in; `agent:<it>` is the agent of that
    /// workspace (full rights there before the cap).
    const SPACE: &str = "ws-blend";

    /// A `.blend` on disk, opened live with the pack K2 opens a scene with
    /// ([`live_pack`]) through the kit's manager, and a person (the owner)
    /// and the workspace's own agent joined.
    struct Scene {
        dir: std::path::PathBuf,
        mgr: SessionManager,
        s: Arc<Session>,
        file: std::path::PathBuf,
        bytes: Vec<u8>,
        person: ConnId,
        agent: ConnId,
    }

    impl Drop for Scene {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    impl Scene {
        fn open(layer_one: bool) -> Scene {
            let dir = std::env::temp_dir().join(format!("k2-blend-ro-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).expect("dir");
            let dir = dir.canonicalize().expect("canonical dir");
            let bytes = k2_mpc_blend::testgen::scenes::living_room();
            let file = dir.join("room.blend");
            std::fs::write(&file, &bytes).expect("the scene on disk");
            let mut config = Config::default();
            config.edit_rate_per_s = 1_000_000;
            config.edit_burst = 1_000_000;
            let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
            let mgr = SessionManager::new(store, Arc::new(LayerOne(layer_one)), config);
            let s = mgr.open_file(live_pack(), &DocKey { space: SPACE.into(), path: "room.blend".into() }, &file, &dir).expect("the scene opens live");
            let person = s.join(Participant::new("person:owner", "Owner", "#7c3aed", ParticipantKind::Person, "tab"), Stay::Held).expect("the owner joins").conn;
            let agent = s.join(Participant::new(format!("agent:{SPACE}"), "Agent", "#16a34a", ParticipantKind::Agent, "cli"), Stay::Held).expect("the agent joins").conn;
            Scene { dir, mgr, s, file, bytes, person, agent }
        }

        fn tool(&self, conn: ConnId, argv: &[&str]) -> Result<ToolOutput, ToolRunError> {
            let argv: Vec<String> = argv.iter().map(|a| a.to_string()).collect();
            self.s.run_tool_with(conn, k2_mpc_markdown::ROOT, &argv, b"", &ToolOptions::new())
        }

        /// A read verb's stdout; panics when it fails.
        fn shown(&self, conn: ConnId, argv: &[&str]) -> String {
            let out = self.tool(conn, argv).unwrap_or_else(|e| panic!("{argv:?} must read: {e}"));
            assert_eq!(out.exit, 0, "{argv:?}: {}", String::from_utf8_lossy(&out.stdout));
            String::from_utf8(out.stdout).expect("UTF-8")
        }

        /// The objects' addresses: each collection `scene list` names, then
        /// the objects `collection show` lists in it.
        fn objects(&self) -> Vec<String> {
            fn words<'a>(text: &'a str, prefix: &str) -> Vec<&'a str> {
                let mut out: Vec<&str> = Vec::new();
                for w in text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '@' || c == ':')) {
                    if w.starts_with(prefix) && !out.contains(&w) {
                        out.push(w);
                    }
                }
                out
            }
            let list = self.shown(self.person, &["scene", "list"]);
            let mut out: Vec<String> = Vec::new();
            for coll in words(&list, "@coll:") {
                let shown = self.shown(self.person, &["collection", "show", coll]);
                for o in words(&shown, "@obj:") {
                    if !out.iter().any(|x| x == o) {
                        out.push(o.to_string());
                    }
                }
            }
            assert!(!out.is_empty(), "no collection lists an object: {list}");
            out
        }
    }

    /// The first `object visibility` call the BLEND pack applies as a change
    /// to a fresh copy of the scene (layer 1 skipped): the edit a person's
    /// client would send. Panics when none applies.
    fn a_changing_edit() -> Vec<String> {
        let sc = Scene::open(false);
        for obj in sc.objects() {
            for v in ["off", "on"] {
                let argv = ["object", "visibility", obj.as_str(), "--viewport", v];
                if let Ok(out) = sc.tool(sc.person, &argv) {
                    if out.exit == 0 && out.changed.is_some() {
                        return argv.iter().map(|a| a.to_string()).collect();
                    }
                }
            }
        }
        panic!("no object's visibility flip applied on the living room scene");
    }

    /// The stuck read's shape, shown with layer 1 skipped (a host that gave
    /// a person edit on a read-only `.blend`): the change goes into the doc,
    /// the read-only pack refuses to render it, and from then on every read
    /// that renders the file (a whole read, for anyone) fails with that
    /// refusal, the change waiting for a write-back. This is why
    /// layer 1 must hold on every path; the test below pins that it does.
    /// When the kit stops rendering on reads (K2 Cores' `Pack::check_edit`),
    /// the reads here answer and this control can go.
    #[test]
    fn without_layer_one_a_refused_render_sticks_every_read() {
        let _m = super::super::cores::tests::mirror_lock();
        set_writer_for_test(None);
        let edit = a_changing_edit();
        let sc = Scene::open(false);
        let obj = edit[2].clone();
        sc.shown(sc.person, &["object", "show", &obj]);
        sc.s.read(sc.person).expect("a whole read before the change");
        let argv: Vec<&str> = edit.iter().map(String::as_str).collect();
        let out = sc.tool(sc.person, &argv).unwrap_or_else(|e| panic!("with edit, the change applies: {e}"));
        assert!(out.changed.is_some(), "the change is in the doc");
        for (who, conn) in [("the person", sc.person), ("the agent", sc.agent), ("the person again", sc.person)] {
            let read = sc.s.read(conn);
            assert!(matches!(&read, Err(e) if e.to_string().contains(CODE)), "{who}: a whole read fails with the render's refusal: {:?}", read.map(|r| r.version));
        }
        assert!(matches!(sc.s.status().expect("status").disk, Some(DiskState::Dirty { .. })), "the change waits for a write-back that can't happen");
        // A structured verb read (a View, no render) still answers: only
        // reads that render the file (whole reads, previews, exports) stick.
        sc.shown(sc.person, &["object", "show", &obj]);
    }

    /// Layer 1 on every principal: with the blend core on, a person (the
    /// owner, the most rights there are) and the workspace's own agent both
    /// have read and no edit on a `.blend`, so the kit refuses their edit
    /// verbs and undo (`Forbidden {need: Edit}`) before applying anything:
    /// the version, the reads (whole, `object show`, `scene list`), the
    /// disk state and the file all stay as they were, and the kit's disk
    /// tick reports no error for it. The edit is one the pack itself would
    /// apply (shown by the control above).
    #[test]
    fn a_person_or_agent_edit_never_reaches_a_live_blend_and_reads_keep_working() {
        let _m = super::super::cores::tests::mirror_lock();
        super::super::cores::tests::all_off();
        set_writer_for_test(None);
        let edit = a_changing_edit();
        super::super::cores::set_for_test("blend", true);
        let key = DocKey { space: SPACE.into(), path: "room.blend".into() };
        for pid in ["person:owner".to_string(), format!("agent:{SPACE}")] {
            let r = super::super::host::rights_for(&pid, &key, None);
            assert!(r.read && !r.edit, "{pid}: {r:?}");
        }
        let sc = Scene::open(true);
        let obj = edit[2].clone();
        let v0 = sc.s.status().expect("status").version;
        let read0 = sc.s.read(sc.person).expect("a whole read");
        assert_eq!(read0.bytes, sc.bytes, "the read is the file");
        let shown0 = sc.shown(sc.person, &["object", "show", &obj]);
        let list0 = sc.shown(sc.person, &["scene", "list"]);
        let argv: Vec<&str> = edit.iter().map(String::as_str).collect();
        let place = ["object", "place", obj.as_str(), "--loc", "0,0,1"];
        let rename = ["object", "rename", obj.as_str(), "--name", "Moved", "--break-links"];
        for (who, conn) in [("the owner", sc.person), ("the agent", sc.agent)] {
            for a in [&argv[..], &place[..], &rename[..]] {
                match sc.tool(conn, a) {
                    Err(ToolRunError::Session(SessionError::Forbidden { need: Need::Edit })) => {}
                    other => panic!("{who}'s {a:?} must be refused for the edit right: {:?}", other.map(|o| (o.exit, o.changed.is_some()))),
                }
            }
            match sc.s.undo(conn, 1) {
                Err(SessionError::Forbidden { need: Need::Edit }) => {}
                other => panic!("{who}'s undo must be refused for the edit right: {:?}", other.map(|_| ())),
            }
        }
        let st = sc.s.status().expect("status");
        assert_eq!(st.version, v0, "nothing was applied");
        assert!(matches!(st.disk, Some(DiskState::Clean)), "nothing waits to be written: {:?}", st.disk);
        let tick = sc.mgr.tick().expect("the kit's tick");
        assert!(tick.errors.iter().all(|(k, _)| *k != key), "no write-back error on the tick: {:?}", tick.errors);
        for conn in [sc.person, sc.agent] {
            let read = sc.s.read(conn).unwrap_or_else(|e| panic!("a whole read after the refusals: {e}"));
            assert_eq!((read.version, &read.bytes), (v0, &sc.bytes), "the original scene");
            assert_eq!(sc.shown(conn, &["object", "show", &obj]), shown0, "object show is as before");
            assert_eq!(sc.shown(conn, &["scene", "list"]), list0, "scene list is as before");
        }
        assert_eq!(std::fs::read(&sc.file).expect("the file"), sc.bytes, "the file is untouched");
        super::super::cores::tests::all_off();
    }

    /// A person's edit, undo or redo on a live `.blend`'s socket is refused
    /// with the write routes' reason; other frames, or another file, are
    /// left to the keyed socket's own answer.
    #[test]
    fn a_scene_socket_refuses_a_persons_edit_with_the_typed_reason() {
        use super::super::live_wire::ClientFrame;
        let _m = super::super::cores::tests::mirror_lock();
        super::super::cores::tests::all_off();
        set_writer_for_test(None);
        let edit = ClientFrame::Edit { op: 7, root: "colls".into(), base: 1, hunks: vec![], group: None };
        assert!(socket_refusal(&edit, "room.blend").is_none(), "the core off: not a scene");
        super::super::cores::set_for_test("blend", true);
        for (f, op) in [(edit, 7), (ClientFrame::Undo { op: 8 }, 8), (ClientFrame::Redo { op: 9 }, 9)] {
            let v: serde_json::Value = serde_json::from_str(&socket_refusal(&f, "a/Room.blend").expect("refused").to_json()).expect("JSON");
            assert_eq!((&v["type"], &v["op"], &v["error"]["id"]), (&serde_json::json!("refused"), &serde_json::json!(op), &serde_json::json!("MPC-E113")), "{v}");
            assert_eq!((&v["error"]["details"]["code"], &v["error"]["details"]["hint"]), (&serde_json::json!(CODE), &serde_json::json!(WHY)), "{v}");
            assert!(v["error"]["message"].as_str().is_some_and(|m| m.contains(WHY)), "{v}");
        }
        assert!(socket_refusal(&ClientFrame::Watch, "room.blend").is_none());
        assert!(socket_refusal(&ClientFrame::Undo { op: 1 }, "deck.pptx").is_none());
        set_writer_for_test(Some(true));
        assert!(socket_refusal(&ClientFrame::Undo { op: 1 }, "room.blend").is_none(), "the writer on");
        set_writer_for_test(None);
        super::super::cores::tests::all_off();
    }

    /// The live pack's edit verbs are refused by name; its reads aren't.
    #[test]
    fn the_live_packs_edit_verbs_are_known_by_its_registry() {
        let v = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for edit in [&["object", "place", "@obj:1", "--loc", "1,2,3"][..], &["object", "visibility", "@obj:1"], &["object", "rename", "@obj:1", "--name", "Lamp"]] {
            assert!(is_edit_verb(&v(edit)), "{edit:?}");
        }
        for read in [&["scene", "list"][..], &["object", "show", "@obj:1"], &["collection", "show", "@coll:1"], &["timeline", "info"], &[]] {
            assert!(!is_edit_verb(&v(read)), "{read:?}");
        }
    }

    /// While the blend core is on, a `.blend` has no edit right for anyone
    /// (comments stay), and the writing calls are refused with the typed
    /// reason; off, or another file, nothing changes.
    #[test]
    fn a_blend_is_read_only_only_while_its_core_is_on() {
        let _m = super::super::cores::tests::mirror_lock();
        super::super::cores::tests::all_off();
        assert!(!BLEND_WRITER, "0.46.15 ships with the writer off");
        set_writer_for_test(None);
        let full = super::super::host::DocRights { read: true, edit: true, history: true, owner: true, comment: false };
        assert_eq!(cap("room.blend", full), full, "the core off: an any-file doc as before");
        assert!(refused(super::super::routes::TOOL, "room.blend", true, None).is_none());
        super::super::cores::set_for_test("blend", true);
        let capped = cap("a/Room.BLEND", full);
        assert!(capped.read && capped.history && capped.owner && !capped.edit && capped.comment, "{capped:?}");
        assert_eq!(cap("notes.md", full), full);
        let r = refused("/cli/docs/write", "room.blend", true, None).expect("a write is refused");
        assert_eq!(r.status, "409 Conflict");
        let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
        assert_eq!((v["error"]["code"].as_str(), v["error"]["exit"].as_u64()), (Some(CODE), Some(3)), "{v}");
        assert_eq!(v["error"]["hint"], WHY, "{v}");
        assert_eq!(v["error"]["path"], "room.blend", "{v}");
        assert_eq!(WHY, "K2 can't save .blend edits yet; this file opens read-only.");
        let place = vec!["object".to_string(), "place".into(), "@obj:1".into(), "--loc".into(), "0,0,1".into()];
        assert!(refused(super::super::routes::TOOL, "room.blend", false, Some(&place)).is_some(), "an edit verb");
        let list = vec!["scene".to_string(), "list".into()];
        assert!(refused(super::super::routes::TOOL, "room.blend", false, Some(&list)).is_none(), "a read verb");
        assert!(refused("/cli/docs/write", "notes.md", true, None).is_none(), "another file");
        assert_eq!(welcome_fields("room.blend"), vec![("read_only_reason", "K2 can't save .blend edits yet; this file opens read-only.")]);
        super::super::cores::tests::all_off();
    }

    /// The switch on: no right capped, nothing refused, the BLEND pack
    /// itself (its merge, its agent writes, its splice), no read-only
    /// reason; off again, read-only again. One switch decides it all.
    #[test]
    fn the_writer_switch_on_lifts_every_layer() {
        let _m = super::super::cores::tests::mirror_lock();
        super::super::cores::tests::all_off();
        super::super::cores::set_for_test("blend", true);
        let full = super::super::host::DocRights { read: true, edit: true, history: true, owner: true, comment: false };
        let place = vec!["object".to_string(), "place".into(), "@obj:1".into(), "--loc".into(), "0,0,1".into()];
        set_writer_for_test(Some(true));
        assert!(writer_on() && !writes_off("room.blend"));
        assert_eq!(cap("room.blend", full), full, "the edit right stays");
        assert!(refused("/cli/docs/write", "room.blend", true, None).is_none());
        assert!(refused(super::super::routes::TOOL, "room.blend", false, Some(&place)).is_none());
        assert!(welcome_fields("room.blend").is_empty(), "no read_only_reason");
        assert!(matches!(blob_pack().agent_writes(), AgentWrites::Allowed), "the BLEND pack's own tier 0");
        let bytes = k2_mpc_blend::testgen::scenes::living_room();
        let content = live_pack().decode(&bytes).expect("reads").roots;
        let pairing = k2_mpc_base::session::list_pairing(&content);
        let changed = first_hidden_flipped(&bytes, &content, &pairing);
        let out = live_pack().encode_from_paired(&bytes, &pairing, &changed, &[1]).unwrap_or_else(|e| panic!("the pack's own splice: {e:?}"));
        assert_ne!(out, bytes, "the change is written");
        set_writer_for_test(Some(false));
        assert!(writes_off("room.blend"));
        assert_eq!(cap("room.blend", full).edit, false);
        assert!(refused("/cli/docs/write", "room.blend", true, None).is_some());
        assert!(matches!(blob_pack().agent_writes(), AgentWrites::Refused));
        assert!(live_pack().encode(&content, &[1]).is_err(), "never a file from content alone");
        assert!(live_pack().encode_from_paired(&bytes, &pairing, &changed, &[1]).is_err(), "the change is refused");
        assert_eq!(welcome_fields("room.blend").len(), 1);
        set_writer_for_test(None);
        super::super::cores::tests::all_off();
    }

    /// The live pack renders the file as it is (every read) and refuses to
    /// build or change one; the tier-0 pack keeps both instead of merging
    /// and lets no agent write.
    #[test]
    fn the_packs_render_only_what_the_file_holds() {
        let bytes = k2_mpc_blend::testgen::scenes::living_room();
        let p = SceneReadOnly::default();
        let content = p.decode(&bytes).expect("it reads").roots;
        let pairing = k2_mpc_base::session::list_pairing(&content);
        assert_eq!(p.encode_from_paired(&bytes, &pairing, &content, &[1]).expect("the carrier as it is"), bytes);
        assert!(p.encode(&content, &[1]).is_err(), "never a file from content alone");
        // The first object hidden (as `object visibility` would): the pack
        // splices it into the file, a write that changes bytes; the
        // read-only pack refuses it.
        let changed = first_hidden_flipped(&bytes, &content, &pairing);
        let written = BlendLive::default().encode_from_paired(&bytes, &pairing, &changed, &[1]).unwrap_or_else(|e| panic!("the pack itself splices it: {e:?}"));
        assert_ne!(written, bytes, "the pack's own splice changes the file");
        match p.encode_from_paired(&bytes, &pairing, &changed, &[1]) {
            Err(SpliceError::Pack(e)) => assert!(e.0.contains(CODE), "{e:?}"),
            other => panic!("a changed render must be refused: {:?}", other.map(|b| b.len())),
        }
        let t0 = BlendIndexOnly::default();
        assert!(matches!(t0.merge3_bytes(&bytes, &bytes, b"theirs"), BlobMerge::Conflict));
        assert!(matches!(t0.agent_writes(), AgentWrites::Refused));
        assert_eq!(t0.index_spec().map(|s| s.pack), Some(k2_mpc_blend::pack::PACK), "the same index");
    }
}
