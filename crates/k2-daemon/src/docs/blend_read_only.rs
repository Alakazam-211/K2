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
pub const WHY: &str = "K2 can't save .blend edits yet; this file opens read-only.";

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
        let first = pairing.keys(k2_mpc_blend::live::OBJS).and_then(|k| k.first()).expect("an object").clone();
        let mut renamed = content.clone();
        for (r, n) in renamed.iter_mut() {
            if let (true, Neutral::Map(m)) = (r == k2_mpc_blend::live::OBJ_PROPS, n) {
                m.insert(k2_mpc_blend::live::prop(&first, k2_mpc_blend::live::f::NAME), k2_mpc_base::elem::Value::str("X"));
            }
        }
        let out = live_pack().encode_from_paired(&bytes, &pairing, &renamed, &[1]).unwrap_or_else(|e| panic!("the pack's own splice: {e:?}"));
        assert_ne!(out, bytes, "a rename is written");
        set_writer_for_test(Some(false));
        assert!(writes_off("room.blend"));
        assert_eq!(cap("room.blend", full).edit, false);
        assert!(refused("/cli/docs/write", "room.blend", true, None).is_some());
        assert!(matches!(blob_pack().agent_writes(), AgentWrites::Refused));
        assert!(live_pack().encode(&content, &[1]).is_err(), "never a file from content alone");
        assert!(live_pack().encode_from_paired(&bytes, &pairing, &renamed, &[1]).is_err(), "the rename is refused");
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
        // The first object renamed (a shorter name, as the rename verb
        // would): the pack splices it into the file, a write that changes
        // bytes; the read-only pack refuses it.
        let mut renamed = content.clone();
        let first = pairing.keys(k2_mpc_blend::live::OBJS).and_then(|k| k.first()).expect("an object").clone();
        let name_key = k2_mpc_blend::live::prop(&first, k2_mpc_blend::live::f::NAME);
        let props = renamed
            .iter_mut()
            .find(|(r, _)| r == k2_mpc_blend::live::OBJ_PROPS)
            .and_then(|(_, n)| match n {
                Neutral::Map(m) => Some(m),
                _ => None,
            })
            .expect("the objects' fields");
        let old = props.get(&name_key).cloned().unwrap_or_else(|| panic!("no {name_key:?} in the objects' fields"));
        props.insert(name_key, k2_mpc_base::elem::Value::str("X"));
        assert_ne!(old, k2_mpc_base::elem::Value::str("X"));
        let written = BlendLive::default().encode_from_paired(&bytes, &pairing, &renamed, &[1]).unwrap_or_else(|e| panic!("the pack itself splices a rename: {e:?}"));
        assert_ne!(written, bytes, "the pack's own splice changes the file");
        match p.encode_from_paired(&bytes, &pairing, &renamed, &[1]) {
            Err(SpliceError::Pack(e)) => assert!(e.0.contains(CODE), "{e:?}"),
            other => panic!("a changed render must be refused: {:?}", other.map(|b| b.len())),
        }
        let t0 = BlendIndexOnly::default();
        assert!(matches!(t0.merge3_bytes(&bytes, &bytes, b"theirs"), BlobMerge::Conflict));
        assert!(matches!(t0.agent_writes(), AgentWrites::Refused));
        assert_eq!(t0.index_spec().map(|s| s.pack), Some(k2_mpc_blend::pack::PACK), "the same index");
    }
}
