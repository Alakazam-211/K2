//! The pack: one version of a BLEND file into the kit's structure index ([`IndexDraft`]), from
//! the scene model ([`crate::scene`]).
//!
//! **What a node is.** Every data-block but the program's own interface (window managers,
//! workspaces, screens). Depth first, parent before children:
//!
//! | Node | Children |
//! |---|---|
//! | a scene | its master collection's child collections, then its objects |
//! | a collection | its child collections, then its objects |
//! | an object, and anything else | none |
//!
//! A collection or object sits under the first scene or collection that holds it, scenes in
//! file order; the others that hold it are in its `collections` attribute. What no scene holds
//! is a root, after the scenes, in file order: a collection no scene holds first takes the
//! objects it holds that nothing placed yet.
//!
//! **Keys** are the model's (`@ob:Cube`, `@li:lib.blend/ma:Steel`): unique per type and library
//! in one file, and as stable as the name is. A key over the kit's limit is left out and the kit
//! mints one.
//!
//! **What a node holds.** `attrs`: the struct (`struct`), the file's own fields of the kinds the
//! model reads, named as the file names them (`loc`, `rot`, `quat`, `size`, `restrictflag`,
//! `lens`, `energy`...), and what the model resolved, by key: `data`, `data_type`, `parent`,
//! `materials` (slots in order, `-` for an empty one), `collections` (what holds it), `camera`,
//! `world`, `library`, a library's `file` name; and the model's named values
//! ([`crate::scene::IdBlock::named`]: `hide_viewport`, `hide_render`, `hide_select`,
//! `object_type`, `rotation_mode`, `light_type`, `camera_type`, `other_bits`). `visible` is
//! false for an object or collection disabled in viewports (so the kit hides what a hidden
//! collection holds); it is true for everything else, and for every node of a file outside
//! [`crate::scene::NAMES_FROM`] to [`crate::scene::NAMES_TO`]. The record digest covers
//! the node's kind, name and attributes; the content digest is the model's (pointers replaced by
//! what they name). Weak IDs for anchors: an object's `data`, so a renamed object that keeps its
//! data is offered as a candidate. Flags: `linked` (from a library), `content_unstable`,
//! `name_not_utf8`, `name_truncated`.
//!
//! **Limits.** The index reads a version as a stream, digesting large blocks as they pass and
//! never holding the file, under [`BlendPack`]'s `index_limits`: by default 4 GiB after
//! decompression ([`INDEX_MAX_RAW_BYTES`]), so a small compressed file can't cost it more
//! decoding than that. What is decoded whole, in memory (the merges, and the file verbs a host
//! runs with them), is under its `limits`: the live tier's, 512 MiB.
//!
//! **Merging, built and held.** [`merge_known_base`] merges a person's app save by
//! object and field ([`crate::merge`]): the person's save is the file and the agent's field
//! changes are replayed onto it. **A clash keeps both, whole-file:** where both changed a field
//! (or the agent's change can't be placed), the merge reports it
//! ([`crate::merge::FieldConflict`]), and until the kit keeps held conflicts durably the pack
//! doesn't hold the agent's side in its own memory, where a restart would lose it: the save is a
//! conflict ([`BlobMerge::Conflict`]), so the kit keeps the save as the file and the agent's
//! version beside it, a conflict copy it records in its store, as the xlsx pack does.
//!
//! **Why a session doesn't merge yet.** That merge is sound only on the version the app
//! had open, and the kit hands the pack what the file last held. The two are the same only when
//! the kit hasn't written the file since the last outside save; once it has, an app that never
//! read the kit's write saves on an older version, and merged against the file's, its save
//! reads as the person undoing every kit write it predates: the earlier agent change is lost,
//! with no conflict. The kit knows which case it is in (it keeps its writes since the last
//! outside save, to tell an echo from a save) but doesn't tell the pack, and doesn't hand over
//! the version the app opened. The pack can't rule the second case out, so it takes it, and the
//! base isn't known exactly: every save the kit would ask it to merge keeps both
//! ([`BlendPack`]'s `can_merge` is `false`, and `merge3_bytes` answers a conflict anyway). When
//! the kit passes the case, or the app's version, `merge3_bytes` calls [`merge_known_base`] on
//! it.
//!
//! **Agents write through the verbs** ([`crate::verbs`]): fixed-size fields, each with the
//! version the agent read. The kit can't tell a verb's write from any other, so agents may
//! write ([`AgentWrites::Allowed`]); a write that is more than field changes is the host's to
//! forbid, and a person's save merged onto one is a whole-file conflict.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;

use k2_mpc_base::anchor::AnchorRules;
use k2_mpc_base::index::{
    cut_text, Digest, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexSpec, IndexVerbs,
    NodeKey, INDEX_ATTR_KEY_MAX, INDEX_ATTR_VALUE_MAX, INDEX_NAME_MAX,
};
use k2_mpc_base::session::{AgentWrites, BlobMerge, BlobPack};

use crate::error::{ReadError, ReadLimits};
use crate::header::HeaderForm;
use crate::live::LiveLimits;
use crate::merge::{merge3, Outcome};
use crate::mutant;
use crate::reader::{read_blend, BlendFile};
use crate::scene::{read_scene, Detail, SceneModel};

/// The pack's name in the index's key.
pub const PACK: &str = "blend";
/// The reader's output format: bump when what a file indexes to changes, and the kit
/// recomputes.
pub const FORMAT: u32 = 2;
/// The index verbs' names. `export` stays `None`: a scene has no per-node export yet.
pub const VERBS: IndexVerbs = IndexVerbs::new(
    "object list",
    "object show",
    "object diff",
    "object blame",
    None,
);

/// The most bytes after decompression the streaming index decodes by default (4 GiB): the
/// reader's own default ([`crate::error::DEFAULT_MAX_RAW_BYTES`]). The index never holds the
/// file, so it may read past the whole-file cap, and the cap bounds the time a small compressed
/// file can cost it.
pub const INDEX_MAX_RAW_BYTES: u64 = crate::error::DEFAULT_MAX_RAW_BYTES;

/// The reader's default limits for the streaming index: the reader's own
/// ([`ReadLimits::default`], with the raw cap at [`INDEX_MAX_RAW_BYTES`]).
pub fn index_read_limits() -> ReadLimits {
    ReadLimits::default()
}

/// The reader's default limits with the raw cap at 64 GiB: what the planted bugs below fall back
/// to.
fn planted_unbounded() -> ReadLimits {
    let d = ReadLimits::default();
    ReadLimits::new(
        64 << 30,
        d.max_blocks,
        d.max_retained_bytes,
        d.retain_block_max,
        d.max_sdna_bytes,
        d.max_window_bytes,
    )
}

/// The pack for BLEND files: an index, and the merge held until the kit says what the app had
/// open (the module's doc). It keeps no state between calls: a clash is a whole-file
/// conflict the kit records, never a value held in the pack's memory.
#[derive(Debug)]
pub struct BlendPack {
    /// The reader's limits for what is decoded whole, in memory: the merges, and the file verbs
    /// a host runs with them ([`crate::verbs`]). The default is the live tier's
    /// ([`LiveLimits`]: 512 MiB after decompression), so a small compressed file can't make a
    /// whole-file decode hold more than that.
    pub limits: ReadLimits,
    /// The reader's limits for the streaming index ([`BlobPack::index`]), which digests large
    /// blocks as they pass and never holds the file: its raw cap bounds the bytes decoded. The
    /// default is [`index_read_limits`] (4 GiB after decompression); a host sets its own here.
    pub index_limits: ReadLimits,
}

impl Default for BlendPack {
    fn default() -> Self {
        let limits = if mutant::active("WholeFileDefaultUnbounded") {
            planted_unbounded()
        } else {
            LiveLimits::default().read
        };
        Self {
            limits,
            index_limits: index_read_limits(),
        }
    }
}

impl BlobPack for BlendPack {
    /// `false`: [`BlendPack::merge3_bytes`] keeps both on every save the kit would hand it, so
    /// the kit needn't read three versions into memory to learn that (the module's doc).
    fn can_merge(&self) -> bool {
        false
    }

    /// **The base.** The kit's `base` is what the file last held. If the kit wrote the file
    /// since the last outside save, the app's save may be made on an older version, and merged
    /// against `base` it would read as the person undoing the kit's writes: with a lease held,
    /// `base == ours`; with an earlier write on the file and the next one waiting to be written
    /// back, `base` is that earlier write. The pack can't tell that case from the safe one (the
    /// kit wrote nothing since, and `base` is the app's own last save), and the kit hands over
    /// neither the case nor the version the app opened, so every save that isn't one side's
    /// bytes again is a conflict: the kit keeps both. Never a merge on a base that isn't known.
    fn merge3_bytes(&self, base: &[u8], ours: &[u8], theirs: &[u8]) -> BlobMerge {
        if ours != theirs && base != theirs && !mutant::active("pack_no_stale_guard") {
            return BlobMerge::Conflict;
        }
        merge_known_base(base, ours, theirs, &self.limits)
    }

    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new(PACK, FORMAT, &VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        let unbounded = planted_unbounded();
        let read = if mutant::active("IndexDecodeUnbounded") {
            &unbounded
        } else {
            &self.index_limits
        };
        index_blend_with(src, limits, read)
    }

    /// Agents write, through the verbs ([`crate::verbs::run`]).
    fn agent_writes(&self) -> AgentWrites {
        AgentWrites::Allowed
    }
}

/// The merge of a person's app save `theirs` with the doc's version `ours`, when `base` is known
/// to be the version the app had open: the kit wrote nothing to the file since the app's last
/// save, or the kit's outside-save bridge handed that version over. [`merge3`], with every
/// clash kept whole: a merge that reports any [`crate::merge::FieldConflict`] is a conflict, so
/// the kit keeps the save as the file and the agent's version beside it, until it keeps held
/// conflicts durably. **Never call it with a base that is only what the file last held**
/// ([`BlendPack::merge3_bytes`]).
pub fn merge_known_base(base: &[u8], ours: &[u8], theirs: &[u8], limits: &ReadLimits) -> BlobMerge {
    let m = match merge3(base, ours, theirs, limits) {
        Outcome::Merged(m) => m,
        Outcome::Conflict(_) => return BlobMerge::Conflict,
        Outcome::Unsupported(_) => return BlobMerge::Unsupported,
    };
    if !m.conflicts.is_empty() && !mutant::active("pack_merges_clash") {
        return BlobMerge::Conflict;
    }
    BlobMerge::Merged(m.bytes)
}

/// The same anchor rules for the kit's `anchor::resolve` outside a session: the kit's own
/// (another kind is a kind change; renamed, moved and changed at once is in doubt; a candidate
/// has the same kind and an object's data, or the name, in common).
impl AnchorRules for BlendPack {}

/// Reads the file `src` holds into a draft, under `limits` and the index's default reader
/// limits ([`index_read_limits`]: 4 GiB after decompression): [`BlendPack`]'s default index.
pub fn index_blend(src: &mut dyn Read, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    index_blend_with(src, limits, &index_read_limits())
}

/// [`index_blend`] under the reader limits given.
pub fn index_blend_with(
    src: &mut dyn Read,
    limits: &IndexLimits,
    read_limits: &ReadLimits,
) -> Result<IndexDraft, IndexRefused> {
    let file = read_blend(src, read_limits).map_err(refusal)?;
    let model = read_scene(&file).map_err(refusal)?;
    index_model(&file, &model, limits)
}

/// A reader's refusal as the kit's: not a BLEND file, or a form this reader doesn't know, is a
/// file this pack doesn't index; anything else is unreadable, with fixed text.
pub fn refusal(e: ReadError) -> IndexRefused {
    match e {
        ReadError::Io(e) => IndexRefused::Read(e),
        ReadError::NotBlend | ReadError::Unknown { .. } => IndexRefused::Unsupported,
        e => IndexRefused::Unreadable {
            why: e.reason().to_owned(),
        },
    }
}

/// An attribute name the kit takes: lower-case ASCII, digits, `_`, `-` or `.`.
fn attr_name_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= INDEX_ATTR_KEY_MAX
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
        })
}

/// A placed data-block: its index, its parent's index in the draft, its depth.
type Placed = (usize, Option<u32>, u32);

fn cut(s: &str, max: usize) -> (String, bool) {
    let (kept, was_cut) = cut_text(s, u32::try_from(max).unwrap_or(u32::MAX));
    (kept.to_owned(), was_cut)
}

/// Indexes a model of `file` into a draft under `limits`.
pub fn index_model(
    file: &BlendFile,
    model: &SceneModel,
    limits: &IndexLimits,
) -> Result<IndexDraft, IndexRefused> {
    let ids = &model.ids;
    let shown: Vec<usize> = (0..ids.len()).filter(|&i| !ids[i].ui).collect();
    if shown.len() as u64 > u64::from(limits.max_nodes) {
        return Err(IndexRefused::TooManyNodes {
            count: shown.len() as u64,
            limit: limits.max_nodes,
        });
    }
    let key_of = |i: usize| ids[i].key.clone();
    let key_or_dash = |i: Option<usize>| i.and_then(key_of).unwrap_or_else(|| "-".to_owned());

    // what holds each data-block (scenes' master collections, collections), in file order
    let mut holders: HashMap<usize, Vec<usize>> = HashMap::new();
    let children_of = |i: usize| -> Vec<usize> {
        match &ids[i].detail {
            Detail::Scene {
                objects,
                collections,
                ..
            }
            | Detail::Collection {
                objects,
                collections,
            } => collections.iter().chain(objects).copied().collect(),
            _ => Vec::new(),
        }
    };
    for &i in &shown {
        for c in children_of(i) {
            let h = holders.entry(c).or_default();
            if !h.contains(&i) {
                h.push(i);
            }
        }
    }

    // placement: depth first from each scene, then each collection no scene placed, then the
    // rest; each data-block once
    let mut order: Vec<Placed> = Vec::with_capacity(shown.len());
    let mut placed = vec![false; ids.len()];
    let place_from =
        |root: usize, order: &mut Vec<Placed>, placed: &mut [bool]| -> Result<(), IndexRefused> {
            let mut stack: Vec<Placed> = vec![(root, None, 0)];
            placed[root] = true;
            while let Some((i, parent, depth)) = stack.pop() {
                if depth > u32::from(limits.max_depth) {
                    return Err(IndexRefused::TooDeep {
                        depth,
                        limit: limits.max_depth,
                    });
                }
                let me = order.len() as u32;
                order.push((i, parent, depth));
                let kids: Vec<usize> = children_of(i)
                    .into_iter()
                    .filter(|&c| !ids[c].ui && !placed[c])
                    .collect();
                // a child reached twice in one list is placed once
                let mut seen = BTreeSet::new();
                let kids: Vec<usize> = kids.into_iter().filter(|c| seen.insert(*c)).collect();
                for &c in &kids {
                    placed[c] = true;
                }
                for &c in kids.iter().rev() {
                    stack.push((c, Some(me), depth + 1));
                }
            }
            Ok(())
        };
    for &i in &shown {
        if ids[i].kind == "scene" && !placed[i] {
            place_from(i, &mut order, &mut placed[..])?;
        }
    }
    for &i in &shown {
        if matches!(ids[i].detail, Detail::Collection { .. }) && !placed[i] {
            place_from(i, &mut order, &mut placed[..])?;
        }
    }
    for &i in &shown {
        if !placed[i] {
            place_from(i, &mut order, &mut placed[..])?;
        }
    }

    let h = &file.header;
    let mut draft = IndexDraft::new(format!("blend-{}", h.version_text()));
    let doc = [
        ("version", h.version_text()),
        (
            "header",
            match h.form {
                HeaderForm::Classic => "classic",
                _ => "large",
            }
            .to_owned(),
        ),
        ("pointer_size", h.pointer_size.to_string()),
        (
            "endian",
            if h.big_endian { "big" } else { "little" }.to_owned(),
        ),
        ("compression", file.compression.as_str().to_owned()),
        ("blocks", file.blocks.len().to_string()),
        ("data_blocks", ids.len().to_string()),
        (
            "interface_data_blocks",
            (ids.len() - shown.len()).to_string(),
        ),
        ("unresolved_pointers", model.unresolved.to_string()),
    ];
    for (k, v) in doc {
        draft.attrs.insert(k.to_owned(), v);
    }

    for (i, parent, _) in order {
        let id = &ids[i];
        let mut attrs: BTreeMap<String, String> = BTreeMap::new();
        let mut flags = BTreeSet::new();
        let put = |attrs: &mut BTreeMap<String, String>, k: &str, v: String| {
            if attr_name_ok(k) {
                attrs.insert(k.to_owned(), cut(&v, INDEX_ATTR_VALUE_MAX).0);
            }
        };
        put(&mut attrs, "struct", id.struct_name.clone());
        for (k, v) in id.fields.iter().chain(&id.named) {
            put(&mut attrs, k, v.clone());
        }
        if let Some(l) = id.library {
            put(&mut attrs, "library", key_or_dash(Some(l)));
            flags.insert("linked".to_owned());
        }
        if !id.materials.is_empty() {
            let slots: Vec<String> = id.materials.iter().map(|m| key_or_dash(*m)).collect();
            put(&mut attrs, "materials", slots.join(","));
        }
        if let Some(hs) = holders.get(&i) {
            let hs: Vec<String> = hs.iter().map(|&x| key_or_dash(Some(x))).collect();
            put(&mut attrs, "collections", hs.join(","));
        }
        let mut weak = BTreeMap::new();
        match &id.detail {
            Detail::Object { data, parent } => {
                put(&mut attrs, "data", key_or_dash(*data));
                put(
                    &mut attrs,
                    "data_type",
                    data.map_or_else(|| "empty".to_owned(), |d| ids[d].kind.clone()),
                );
                if parent.is_some() {
                    put(&mut attrs, "parent", key_or_dash(*parent));
                }
                if let Some(k) = data.and_then(key_of) {
                    weak.insert("data".to_owned(), cut(&k, INDEX_ATTR_VALUE_MAX).0);
                }
            }
            Detail::Scene {
                objects,
                collections,
                camera,
                world,
            } => {
                put(&mut attrs, "objects", objects.len().to_string());
                put(&mut attrs, "children", collections.len().to_string());
                if camera.is_some() {
                    put(&mut attrs, "camera", key_or_dash(*camera));
                }
                if world.is_some() {
                    put(&mut attrs, "world", key_or_dash(*world));
                }
            }
            Detail::Collection {
                objects,
                collections,
            } => {
                put(&mut attrs, "objects", objects.len().to_string());
                put(&mut attrs, "children", collections.len().to_string());
            }
            Detail::Library { file: Some(f) } => put(&mut attrs, "file", f.clone()),
            _ => {}
        }
        if !id.content_stable {
            flags.insert("content_unstable".to_owned());
        }
        if std::str::from_utf8(&id.name_bytes).is_err() {
            flags.insert("name_not_utf8".to_owned());
        }
        let (name, name_cut) = cut(&id.name, INDEX_NAME_MAX);
        if name_cut {
            flags.insert("name_truncated".to_owned());
        }
        let record = record_digest(&id.kind, &id.name_bytes, &attrs, &flags);
        let mut node = DraftNode::new(id.kind.clone(), name, record);
        node.key = id
            .key
            .as_deref()
            .filter(|k| NodeKey::parse(k).is_some())
            .map(str::to_owned);
        node.parent = parent;
        node.attrs = attrs;
        node.flags = flags;
        node.weak = weak;
        node.visible = !id.hidden.unwrap_or(false);
        node.content = Some(id.content);
        draft.nodes.push(node);
    }
    Ok(draft)
}

/// The record digest: the node's kind, name bytes, attributes and flags.
fn record_digest(
    kind: &str,
    name: &[u8],
    attrs: &BTreeMap<String, String>,
    flags: &BTreeSet<String>,
) -> Digest {
    let mut d = Digest::stream();
    let mut field = |b: &[u8]| {
        d.update(&(b.len() as u64).to_le_bytes());
        d.update(b);
    };
    field(b"k2-mpc-blend/record/v1");
    field(kind.as_bytes());
    field(name);
    for (k, v) in attrs {
        field(k.as_bytes());
        field(v.as_bytes());
    }
    for f in flags {
        field(f.as_bytes());
    }
    d.finish()
}
