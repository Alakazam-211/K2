//! `.dxf` and `.dwg` drawings are read-only in K2 for now (the CAD core).
//!
//! While the CAD core is on, K2 reads drawings (the DXF index and its verbs,
//! the drawing-model read the viewer draws from, the pack's read verbs, the
//! AV reading of a set: [`super::cad`]) and refuses everything that would
//! write one, with one typed reason, [`CODE`]. Three layers, each enough on
//! its own (as `.blend` files, [`super::blend_read_only`]):
//!
//! 1. **Rights** ([`cap`], used by `host::rights_for`, which the kit asks,
//!    and by the routes' own rights): no one has edit on a drawing, so the
//!    kit refuses every write, restore, undo and revert before it applies.
//! 2. **The routes** ([`refused`]): those calls, and the pack's edit verbs
//!    ([`EDIT_VERBS`]), are refused by K2 first with `409
//!    cad_writer_disabled` (exit 3), so callers get the reason in words. The
//!    docs socket declines a drawing with the same code ([`socket_decline`]).
//! 3. **The packs** ([`DxfIndexOnly`], [`DwgReadOnly`]): the tier-0 packs
//!    never merge (an outside save that would need a merge keeps both, the
//!    kit's conflict copy) and let no agent write.
//!
//! While the CAD core is off a drawing is an any-file doc, as before the
//! core: no CAD pack is involved and nothing here applies.
//!
//! **Editing, next.** The CAD pack's edit verbs for DXF ([`EDIT_VERBS`]:
//! `attrib set`, `entity set`, `entity move`, `entity add`, `entity
//! delete`) come with kit v0.22.0. Then people (in the viewer) and agents
//! edit a `.dxf` through those verbs only, on the kit's verb path
//! (`/cli/docs/tool`, `BlobSession::run_tool`, a `--dry-run` and then the
//! real call, each with `--base`), with their own rights: [`EDIT_VERBS_ON`]
//! keeps the edit right on a `.dxf` (layer 1), lets the verbs through
//! (layer 2) and gives [`DxfIndexOnly`] the pack's merge (layer 3: an app's
//! save after a verb merges by field once the host notes the version the
//! app opened; until then both files are kept). Whole-file writes stay
//! refused for everyone, and a `.dwg` stays read-only (its pack has no
//! writer). With v0.22.0, [`DxfIndexOnly`] also answers the kit's
//! `AgentWrites::VerbsOnly` in place of `Refused`, which would refuse an
//! agent's verbs too.

use std::io::Read;

use k2_core::{docs_cores, product_name};
use k2_mpc_base::index::{IndexDraft, IndexLimits, IndexRefused, IndexSpec};
use k2_mpc_base::session::{AgentWrites, BlobMerge, BlobPack, SaveMerge};
use k2_mpc_dxf::DxfPack;

use super::blob_packs::{kind_by_name, BlobKind};
use crate::cli_response::CliResponse;

/// People and agents edit a `.dxf` through the CAD pack's edit verbs (kit
/// v0.22.0, CAD pack 485cd80): on. Module docs for what it changes.
pub const EDIT_VERBS_ON: bool = true;

/// Whether the pack's edit verbs run on a `.dxf`: [`EDIT_VERBS_ON`] (unit
/// tests may turn it on for their own process).
pub fn edit_verbs_on() -> bool {
    #[cfg(test)]
    if let Some(on) = *test_verbs().lock().unwrap_or_else(|p| p.into_inner()) {
        return on;
    }
    EDIT_VERBS_ON
}

#[cfg(test)]
fn test_verbs() -> &'static std::sync::Mutex<Option<bool>> {
    static V: std::sync::Mutex<Option<bool>> = std::sync::Mutex::new(None);
    &V
}

/// Test-only: the edit verbs on or off for this process (`None`: the
/// switch).
#[cfg(test)]
pub fn set_edit_verbs_for_test(on: Option<bool>) {
    *test_verbs().lock().unwrap_or_else(|p| p.into_inner()) = on;
}

/// The refusal's code.
pub const CODE: &str = "cad_writer_disabled";

/// The refusal's words, on every write path and at the docs socket.
pub const WHY: &str = concat!(product_name!(), " can't save drawing edits yet; this file opens read-only.");

/// The refusal's words for a whole-file write of a drawing that takes the
/// edit verbs (a `.dxf` while they are on), for people and agents alike.
pub const WHY_WHOLE: &str = "Drawings change through edits; whole-file saves aren't supported yet.";

/// The CAD pack's edit verbs on a `.dxf` (kit v0.22.0), by their first two
/// words. Its index reads (`entity list|show|diff|blame`) aren't edits.
pub const EDIT_VERBS: [&str; 5] = ["attrib set", "entity set", "entity move", "entity add", "entity delete"];

/// Whether `argv` is one of [`EDIT_VERBS`].
pub fn is_edit_verb(argv: &[String]) -> bool {
    let two = argv.iter().take(2).map(String::as_str).collect::<Vec<_>>().join(" ");
    EDIT_VERBS.contains(&two.as_str())
}

/// Whether `rel` is a drawing by its name (`.dxf`, `.dwg`), on or off.
pub fn is_drawing(rel: &str) -> bool {
    matches!(kind_by_name(rel), BlobKind::Dxf | BlobKind::Dwg)
}

/// Whether `rel` is a drawing K2 must not write now: a drawing while the CAD
/// core is on.
pub fn writes_off(rel: &str) -> bool {
    is_drawing(rel) && super::cores::is_on(docs_cores::CAD)
}

/// Whether `rel` takes the pack's edit verbs now: a `.dxf` while the CAD
/// core and [`edit_verbs_on`] are.
fn verbs_on(rel: &str) -> bool {
    edit_verbs_on() && kind_by_name(rel) == BlobKind::Dxf && writes_off(rel)
}

/// Whether K2 writes this drawing now, the viewer's `writer`: a `.dxf`
/// through the pack's edit verbs while they are on; never a `.dwg`.
pub fn writer(rel: &str) -> bool {
    verbs_on(rel)
}

/// Why a drawing opens read-only now, for the viewer up front (as a
/// scene's welcome says it, `blend_read_only::WELCOME_FIELD`): [`WHY`]
/// while the CAD core is on and K2 doesn't write it ([`writer`]); `None`
/// once it does, or while the core is off (an any-file doc).
pub fn read_only_reason(rel: &str) -> Option<&'static str> {
    (writes_off(rel) && !writer(rel)).then_some(WHY)
}

/// The field that carries [`read_only_reason`] (the BLEND welcome's name).
pub const READ_ONLY_FIELD: &str = super::blend_read_only::WELCOME_FIELD;

/// `r` with no edit on a drawing K2 must not write ([`writes_off`]):
/// reading, history and comments (which never write the file) stay. With
/// the edit verbs on, a `.dxf` keeps the caller's own edit right (the
/// verbs need it; whole-file writes are refused by [`refused`]).
pub fn cap(rel: &str, r: super::host::DocRights) -> super::host::DocRights {
    if !writes_off(rel) || verbs_on(rel) {
        return r;
    }
    super::host::DocRights { edit: false, comment: r.comment || r.edit, ..r }
}

/// The refusal: `409 cad_writer_disabled`, exit 3, its hint [`WHY`] word for
/// word, and the file as `path`.
pub fn refusal(rel: &str) -> CliResponse {
    // A drawing that takes the edit verbs changes through them; a whole-file
    // save of it isn't supported (people included: the Steward's rule).
    let why = if verbs_on(rel) { WHY_WHOLE } else { WHY };
    let mut r = super::routes::with_exit(super::caller::refusal("409 Conflict", CODE, why), k2_mpc_base::tools::exit::REFUSED);
    if let Ok(mut v) = serde_json::from_str::<serde_json::Value>(&r.body) {
        if let Some(e) = v.get_mut("error").and_then(serde_json::Value::as_object_mut) {
            e.insert("path".into(), serde_json::Value::from(rel));
            r.body = v.to_string();
        }
    }
    r
}

/// A call that would write `rel` while it is read-only: the whole-file
/// write routes (`edit`, `write`, `undo`, `redo`, `restore`, `revert`,
/// `use-theirs`; `writes`), always; the pack's edit verbs through
/// `/cli/docs/tool` (`argv`) unless they are on for this file. Never a read
/// or a comment.
pub fn refused(path: &str, rel: &str, writes: bool, argv: Option<&[String]>) -> Option<CliResponse> {
    if !writes_off(rel) {
        return None;
    }
    let verb = path == super::routes::TOOL && argv.is_some_and(is_edit_verb);
    (writes || (verb && !verbs_on(rel))).then(|| refusal(rel))
}

/// The docs socket's decline for a drawing K2 must not write: `(code,
/// message)`. A drawing has no live view (the socket edits text); it is
/// declined with the write routes' reason, so a client reads one answer.
pub fn socket_decline(rel: &str) -> Option<(&'static str, &'static str)> {
    writes_off(rel).then_some((CODE, if verbs_on(rel) { WHY_WHOLE } else { WHY }))
}

/// The tier-0 DXF pack: its index (and so its verbs) is `DxfPack`'s; it
/// lets no agent write the whole file and, while the edit verbs are off,
/// never merges (a save that would need one keeps both).
#[derive(Debug, Default, Clone, Copy)]
pub struct DxfIndexOnly(pub DxfPack);

impl BlobPack for DxfIndexOnly {
    /// Never while nobody edits drawings (the doc never has a version its
    /// file lacks); with the edit verbs on, the pack's merge by field.
    fn can_merge(&self) -> bool {
        edit_verbs_on() && self.0.can_merge()
    }

    /// Keep both: a merged file is the pack's writer (the pack's own merge
    /// with the edit verbs on).
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        if edit_verbs_on() {
            return self.0.merge3_bytes(base, ours, theirs);
        }
        BlobMerge::Conflict
    }

    fn merge3_known(&self, m: &SaveMerge<'_>) -> BlobMerge {
        if edit_verbs_on() {
            return self.0.merge3_known(m);
        }
        BlobMerge::Conflict
    }

    fn index_spec(&self) -> Option<IndexSpec> {
        self.0.index_spec()
    }

    fn index(&self, src: &mut dyn Read, size: u64, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
        self.0.index(src, size, limits)
    }

    /// Never an agent's whole-file write (`MPC-E715`); with the edit verbs
    /// on, the kit's verbs-only setting lets their verbs through.
    fn agent_writes(&self) -> AgentWrites {
        if edit_verbs_on() {
            AgentWrites::VerbsOnly
        } else {
            AgentWrites::Refused
        }
    }

    /// The pack's verbs, reads and edits (the kit's registry for the kind
    /// is built once from them); with the switch off, layers 1 and 2 refuse
    /// the edits.
    fn verbs(&self) -> &'static [k2_mpc_base::session::BlobVerb] {
        self.0.verbs()
    }

    fn errors(&self) -> &'static [k2_mpc_base::errors::ErrorEntry] {
        self.0.errors()
    }

    fn hold_reach(&self, kind: &str) -> k2_mpc_base::session::HoldReach {
        self.0.hold_reach(kind)
    }

    fn save_pairing(&self, facts: &k2_mpc_base::session::SaveFacts<'_>) -> k2_mpc_base::session::SavePairing {
        self.0.save_pairing(facts)
    }
}

/// The tier-0 DWG pack: no index yet (the DWG reader has no blob pack of
/// its own), never a merge and no agent write. Its reads are the drawing
/// model's ([`super::cad`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct DwgReadOnly;

impl BlobPack for DwgReadOnly {
    fn can_merge(&self) -> bool {
        false
    }

    fn merge3_bytes(&self, _base: &[u8], _ours: &[u8], _theirs: &[u8]) -> BlobMerge {
        BlobMerge::Conflict
    }

    fn merge3_known(&self, _m: &SaveMerge<'_>) -> BlobMerge {
        BlobMerge::Conflict
    }

    fn agent_writes(&self) -> AgentWrites {
        AgentWrites::Refused
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> super::super::host::DocRights {
        super::super::host::DocRights { read: true, edit: true, history: true, owner: true, comment: false }
    }

    fn argv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    /// While the CAD core is on, a drawing has no edit right for anyone
    /// (comments stay), and the writing calls and the pack's edit verbs are
    /// refused with the typed reason; off, or another file, nothing changes.
    #[test]
    fn a_drawing_is_read_only_only_while_its_core_is_on() {
        let _m = super::super::cores::tests::mirror_lock();
        super::super::cores::tests::all_off();
        set_edit_verbs_for_test(Some(false)); // the read path: no drawing edits
        let tool = super::super::routes::TOOL;
        let mv = argv(&["entity", "move", "@h:2B1", "--by", "10,0", "--unit", "mm"]);
        for rel in ["plan.dxf", "a/Site.DWG"] {
            assert_eq!(cap(rel, full()), full(), "{rel}: the core off: an any-file doc as before");
            assert_eq!(read_only_reason(rel), None, "{rel}: the core off: no CAD reason");
            assert!(refused("/cli/docs/write", rel, true, None).is_none(), "{rel}");
            assert!(refused(tool, rel, false, Some(&mv)).is_none(), "{rel}");
            assert!(socket_decline(rel).is_none(), "{rel}");
        }
        super::super::cores::set_for_test("cad", true);
        for rel in ["plan.dxf", "a/Site.DWG", "x/Y.Dxf"] {
            let c = cap(rel, full());
            assert!(c.read && c.history && c.owner && !c.edit && c.comment, "{rel}: {c:?}");
            for r in [refused("/cli/docs/write", rel, true, None), refused(tool, rel, false, Some(&mv))] {
                let r = r.unwrap_or_else(|| panic!("{rel}: refused"));
                assert_eq!(r.status, "409 Conflict");
                let v: serde_json::Value = serde_json::from_str(&r.body).expect("json");
                assert_eq!((v["error"]["code"].as_str(), v["error"]["exit"].as_u64()), (Some(CODE), Some(3)), "{v}");
                assert_eq!((&v["error"]["hint"], &v["error"]["path"]), (&serde_json::json!(WHY), &serde_json::json!(rel)), "{v}");
            }
            assert!(refused("/cli/docs/read", rel, false, None).is_none(), "{rel}: a read");
            assert!(refused(tool, rel, false, Some(&argv(&["entity", "list"]))).is_none(), "{rel}: an index read");
            assert_eq!(socket_decline(rel), Some((CODE, WHY)));
        }
        assert_eq!(WHY, "K2 can't save drawing edits yet; this file opens read-only.");
        for rel in ["plan.dxf", "a/Site.DWG"] {
            assert_eq!((writer(rel), read_only_reason(rel)), (false, Some(WHY)), "{rel}: read only, said up front");
        }
        assert_eq!(READ_ONLY_FIELD, "read_only_reason");
        for other in ["notes.md", "room.blend", "plan.dxf.txt", "dxf", ".dwg", "part.dgn"] {
            assert_eq!(read_only_reason(other), None, "{other}");
            assert_eq!(cap(other, full()), full(), "{other}");
            assert!(refused("/cli/docs/write", other, true, None).is_none(), "{other}");
        }
        set_edit_verbs_for_test(None);
        super::super::cores::tests::all_off();
    }

    /// The switch for the pack's edit verbs (kit v0.22.0): on, a `.dxf` keeps
    /// the caller's edit right and its verbs go through, for people and
    /// agents alike; a `.dwg` stays read-only, whole-file writes stay
    /// refused, and the DXF pack merges an app's save by field.
    #[test]
    fn edit_verbs_on_open_a_dxf_to_its_verbs_only() {
        let _m = super::super::cores::tests::mirror_lock();
        super::super::cores::tests::all_off();
        super::super::cores::set_for_test("cad", true);
        set_edit_verbs_for_test(Some(true));
        let tool = super::super::routes::TOOL;
        let mv = argv(&["entity", "move", "@h:2B1", "--by", "10,0", "--unit", "mm"]);
        assert!(cap("plan.dxf", full()).edit, "a .dxf keeps the edit right");
        assert!(!cap("site.dwg", full()).edit, "a .dwg stays read-only");
        assert!(refused(tool, "plan.dxf", false, Some(&mv)).is_none(), "an edit verb on a .dxf");
        assert!(refused(tool, "site.dwg", false, Some(&mv)).is_some(), "an edit verb on a .dwg");
        for route in ["/cli/docs/write", "/cli/docs/restore", "/cli/docs/undo"] {
            assert!(refused(route, "plan.dxf", true, None).is_some(), "{route}: whole-file writes stay refused");
        }
        assert!(DxfIndexOnly::default().can_merge(), "an app's save after a verb merges by field");
        assert_eq!((writer("plan.dxf"), read_only_reason("plan.dxf")), (true, None), "a .dxf K2 writes: no read-only reason");
        assert_eq!((writer("site.dwg"), read_only_reason("site.dwg")), (false, Some(WHY)), "a .dwg never");
        let whole: serde_json::Value = serde_json::from_str(&refused("/cli/docs/write", "plan.dxf", true, None).expect("refused").body).expect("json");
        assert_eq!((&whole["error"]["code"], &whole["error"]["hint"]), (&serde_json::json!(CODE), &serde_json::json!(WHY_WHOLE)), "{whole}");
        assert!(matches!(DxfIndexOnly::default().agent_writes(), AgentWrites::VerbsOnly), "agents' verbs pass, their whole-file writes don't (MPC-E715)");
        assert!(EDIT_VERBS_ON, "0.46.15 ships the edit verbs on");
        set_edit_verbs_for_test(Some(false));
        assert!(!cap("plan.dxf", full()).edit, "the switch off again");
        assert!(refused(tool, "plan.dxf", false, Some(&mv)).is_some());
        set_edit_verbs_for_test(None);
        super::super::cores::tests::all_off();
    }

    /// Through the kit with the pack K2 opens a `.dxf` with: a person's and
    /// an agent's verb edit both land after a dry run (the real call holds
    /// the dry run's digest), each one version; an agent's whole-file write
    /// is the kit's `MPC-E715` (verbs only) and changes nothing.
    #[test]
    fn people_and_agents_edit_a_dxf_through_its_verbs_only() {
        use k2_mpc_base::blob::{BlobConfig, BlobStore};
        use k2_mpc_base::session::{AuditEvent, Config, Host, Participant, ParticipantKind, Rights, SessionError, SessionManager, Stay};
        use k2_mpc_base::store::{DocKey, DocStore};
        use k2_mpc_base::tools::ToolOptions;
        use std::sync::Arc;

        struct Editors;
        impl Host for Editors {
            fn rights(&self, _: &Participant, _: &DocKey) -> Rights {
                Rights::editor()
            }
            fn now_ms(&self) -> u64 {
                super::super::host::now_ms()
            }
            fn on_event(&self, _: &AuditEvent) {}
        }
        let _m = super::super::cores::tests::mirror_lock();
        set_edit_verbs_for_test(Some(true));
        let dir = std::env::temp_dir().join(format!("k2-cad-verbs-{}-{}", std::process::id(), uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("dir");
        let store = Arc::new(DocStore::open(dir.join("docs.redb")).expect("store"));
        let blobs = Arc::new(BlobStore::open(store.clone(), dir.join("blobs"), BlobConfig::default(), Arc::new(|| 1_000_000)).expect("blobs"));
        let mut c = Config::default();
        c.edit_rate_per_s = 1_000_000;
        c.edit_burst = 1_000_000;
        let mgr = SessionManager::new(store, Arc::new(Editors), c).with_blob_store(blobs);
        let sampler = k2_mpc_dxf::testgen::draw_sampler(k2_mpc_dxf::testgen::Ver::R2018).build().bytes;
        let key = DocKey { space: "ws-cad".into(), path: "plan.dxf".into() };
        let s = mgr.open_blob(Arc::new(DxfIndexOnly::default()), &key, Some(&mut &sampler[..])).expect("the drawing opens");
        let line = {
            let d = k2_mpc_dxf::verbs::run(&sampler, "draw page", &["--version", "1", "--space", "model"]).expect("a page");
            let v: serde_json::Value = serde_json::from_str(&d.data.to_json()).expect("json");
            let h = v["entities"].as_array().and_then(|es| es.iter().find(|e| e["kind"] == "LINE")).and_then(|e| e["h"].as_str()).expect("a LINE").to_string();
            if h.starts_with("@h:") { h } else { format!("@h:{h}") }
        };
        let opts = ToolOptions::new().json(true).result_json(true);
        for (who, kind) in [("person:owner", ParticipantKind::Person), ("agent:ws-cad", ParticipantKind::Agent)] {
            let conn = s.join(Participant::new(who, who, "#123456", kind, "cli"), Stay::Held).expect("join").conn;
            let base = s.status().expect("status").version.to_string();
            let mv = |extra: &[&str]| -> Vec<String> {
                ["entity", "move", line.as_str(), "--by", "10,0", "--unit", "mm", "--base", base.as_str()].iter().chain(extra).map(|a| a.to_string()).collect()
            };
            let dry = s.run_tool(conn, &mv(&["--dry-run"]), &opts).unwrap_or_else(|e| panic!("{who}'s dry run: {e:?}"));
            assert_eq!((dry.exit, dry.version_made()), (0, None), "{who}: a dry run records nothing");
            let r: serde_json::Value = serde_json::from_str(&dry.result().expect("the result as data").to_json()).expect("json");
            let digest = r["changed"]["bytes"]["after_digest"].as_str().unwrap_or_else(|| panic!("{who}: the digest: {r}")).to_string();
            let real = s.run_tool(conn, &mv(&["--expect", digest.as_str()]), &opts).unwrap_or_else(|e| panic!("{who}'s edit: {e:?}"));
            assert_eq!(real.exit, 0, "{who}: {}", String::from_utf8_lossy(&real.stdout));
            let made = real.version_made().unwrap_or_else(|| panic!("{who}: the edit made a version"));
            assert_eq!(made, s.status().expect("status").version, "{who}");
            if kind == ParticipantKind::Agent {
                let head = s.status().expect("status").version;
                match s.write_bytes(conn, &sampler, head) {
                    Err(SessionError::AgentWriteRefused) => {}
                    other => panic!("an agent's whole-file write must be MPC-E715: {:?}", other.map(|w| w.version)),
                }
                assert_eq!(s.status().expect("status").version, head, "nothing written");
            }
        }
        set_edit_verbs_for_test(None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The pack's edit verbs by their two words; its index reads aren't.
    #[test]
    fn the_edit_verbs_are_known_by_their_words() {
        for e in [&["attrib", "set", "@h:1", "--tag", "T", "--value", "V"][..], &["entity", "set", "@h:1", "--layer", "0"], &["entity", "add", "line"], &["entity", "delete", "@h:1"], &["entity", "move", "@h:1"]] {
            assert!(is_edit_verb(&argv(e)), "{e:?}");
        }
        for r in [&["entity", "list"][..], &["entity", "show", "@h:1"], &["entity", "diff", "1"], &["entity", "blame"], &["comment", "--doc"], &[]] {
            assert!(!is_edit_verb(&argv(r)), "{r:?}");
        }
    }

    /// The tier-0 packs keep both instead of merging and let no agent write;
    /// the DXF one indexes as the DXF pack does, the DWG one doesn't index.
    #[test]
    fn the_packs_never_merge_or_let_an_agent_write() {
        let _m = super::super::cores::tests::mirror_lock();
        set_edit_verbs_for_test(Some(false));
        let dxf = DxfIndexOnly::default();
        assert!(!dxf.can_merge());
        assert!(matches!(dxf.merge3_bytes(b"0\nEOF\n", b"0\nEOF\n", b"theirs"), BlobMerge::Conflict));
        assert!(matches!(dxf.agent_writes(), AgentWrites::Refused));
        assert_eq!(dxf.index_spec().map(|s| s.pack), DxfPack.index_spec().map(|s| s.pack), "the same index");
        assert_eq!(dxf.index_spec().map(|s| s.pack), Some("dxf"));
        let dwg = DwgReadOnly;
        assert!(!dwg.can_merge() && dwg.index_spec().is_none());
        assert!(matches!(dwg.merge3_bytes(b"a", b"a", b"b"), BlobMerge::Conflict));
        assert!(matches!(dwg.agent_writes(), AgentWrites::Refused));
        set_edit_verbs_for_test(None);
    }
}
