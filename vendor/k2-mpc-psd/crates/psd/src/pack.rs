//! The pack: reads one version of a PSD or PSB file into the kit's structure index
//! ([`IndexDraft`]), from a stream, holding the layer records and never the pixels.
//!
//! **What a node is.** Every layer record except a group's closing marker (the bounding section
//! divider) is one node. A group's children are the records between its marker and its own
//! record, as the file nests them; the kit gets them parent first, siblings top to bottom (the
//! file stores them bottom to top). Nesting the file never closes is undone the way the full
//! reader's [`photocraft_psd::PsdFile::layer_tree`] does it: a folder without a marker is an
//! empty group, and an unclosed marker's layers move up to the level around it.
//!
//! **Keys.** A node's key is its layer ID (`@lyid:N`, from the record's `lyid` block), exactly as
//! the file has it, or `None` when the record has none. The pack never invents an ID and never
//! drops a repeated one: the kit mints a key for every node whose ID is missing or held by
//! another node in the same version, and carries minted keys across versions itself.
//!
//! **Kinds** (the first rule that fits): `group`, or `artboard` for a group with artboard data;
//! `other` for a record with a section type the format doesn't define; `text` (type tool data);
//! `smart` (placed or smart object data); `adjustment` (an adjustment layer's block); `fill` (a
//! solid, gradient or pattern fill); then `shape` for a fill, or a plain layer, whose pixel data
//! the file marks irrelevant and which has vector data; else `raster`.
//!
//! **Hashes.** Raw digests through the kit's [`Digest`], which the kit keys before storing. The
//! record digest covers the stored record with its channel lengths zeroed (a pixel change that
//! only changes a compressed length is a content change, not a record change); the content
//! digest covers every channel's id, compression, length and encoded bytes as stored, fed
//! piece by piece as the walker passes them, so no channel is ever held.
//!
//! Nothing a node holds is styling: no effect, adjustment or text style parameters, only the
//! plain text of a text layer (cut to the kit's limit).
//!
//! **Anchors.** The pack's half of an anchor's state is in [`crate::anchor`]: kind families
//! (group and artboard, fill and shape), any other kind change is `KindChanged`, the kit's
//! identity rule within a family, and `placed_id` as a weak ID that only ever suggests a
//! candidate.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Read};

use k2_mpc_base::anchor::{AnchorJudgment, AnchorRules, CandidateWhy};
use k2_mpc_base::index::{
    cut_text, Digest, DigestStream, DraftNode, IndexDraft, IndexLimits, IndexRefused, IndexSpec,
    IndexVerbs, NodeRect, INDEX_ATTR_VALUE_MAX, INDEX_NAME_MAX, INDEX_TEXT_MAX,
};
use k2_mpc_base::index::{Fingerprint, IndexNode};
use k2_mpc_base::session::{AgentWrites, BlobPack};
use photocraft_psd::compression::Compression;
use photocraft_psd::descriptor::{Value, VersionedDescriptor};
use photocraft_psd::layer::{LayerRecord, MaskData};
use photocraft_psd::records::{walk_records, WalkError, WalkOptions, WalkSink};
use photocraft_psd::tagged::{BlockData, SectionType, TaggedBlock};

/// The pack's name in the index's key.
pub const PACK: &str = "psd";
/// The reader's output format: bump when what a file reads to changes, and the kit recomputes.
pub const FORMAT: u32 = 1;
/// The index verbs' names. `export` stays `None` until the raster exporter exists.
pub const VERBS: IndexVerbs = IndexVerbs::new(
    "layer list",
    "layer show",
    "layer diff",
    "layer blame",
    None,
);

/// The image resource that holds the document's layer ID seed.
const RESOURCE_LAYER_ID_SEED: u16 = 1044;
/// The resources a walk keeps.
const KEEP: &[u16] = &[RESOURCE_LAYER_ID_SEED];
/// Most bytes of one linked-file item's head kept to read its type, ID and file name.
const LINK_HEAD_MAX: usize = 16 * 1024;
/// Most linked-file items remembered.
const LINK_ITEMS_MAX: usize = 4096;

/// Adjustment layers' blocks (the public format specification, "Adjustment layer").
const ADJUSTMENT_KEYS: &[&[u8; 4]] = &[
    b"brit", b"levl", b"curv", b"expA", b"vibA", b"hue ", b"hue2", b"blnc", b"blwh", b"phfl",
    b"mixr", b"clrL", b"nvrt", b"post", b"thrs", b"grdm", b"selc",
];
/// Fill layers' blocks: solid colour, gradient, pattern.
const FILL_KEYS: &[&[u8; 4]] = &[b"SoCo", b"GdFl", b"PtFl"];
/// Vector data that makes a fill or a plain layer whose pixels are irrelevant a shape.
const VECTOR_KEYS: &[&[u8; 4]] = &[b"vogk", b"vmsk", b"vsms"];
/// Placed and smart object data.
const SMART_KEYS: &[&[u8; 4]] = &[b"SoLd", b"SoLE", b"PlLd", b"plLd"];
/// Type tool data (`TySh`, and the older `tySh`).
const TEXT_KEYS: &[&[u8; 4]] = &[b"TySh", b"tySh"];
/// Artboard data on a group's record.
const ARTBOARD_KEYS: &[&[u8; 4]] = &[b"artb", b"artd", b"abdd"];
/// Layer styles (effects).
const STYLE_KEYS: &[&[u8; 4]] = &[b"lfx2", b"lmfx", b"lrFX"];
/// Global blocks that list linked and embedded files.
const LINK_KEYS: &[&[u8; 4]] = &[b"lnk2", b"lnk3", b"lnkD", b"lnkE"];

/// The pack for PSD and PSB files: an index, and no agent writes.
#[derive(Debug, Clone, Copy, Default)]
pub struct PsdPack;

impl BlobPack for PsdPack {
    fn index_spec(&self) -> Option<IndexSpec> {
        Some(IndexSpec::new(PACK, FORMAT, &VERBS))
    }

    fn index(
        &self,
        src: &mut dyn Read,
        _size: u64,
        limits: &IndexLimits,
    ) -> Result<IndexDraft, IndexRefused> {
        index_psd(src, limits)
    }

    /// [`crate::anchor::judge`]: kind families, then the kit's identity rule.
    fn judge_anchor(
        &self,
        was: &Fingerprint,
        hint: Option<&str>,
        now: &IndexNode,
    ) -> AnchorJudgment {
        crate::anchor::judge(was, hint, now)
    }

    /// [`crate::anchor::candidate`]: the same family, with `placed_id` or the name in common.
    fn anchor_candidate(
        &self,
        was: &Fingerprint,
        hint: Option<&str>,
        node: &IndexNode,
    ) -> Option<CandidateWhy> {
        crate::anchor::candidate(was, hint, node)
    }

    /// Agents never write a PSD file: they propose a new file and a comment.
    fn agent_writes(&self) -> AgentWrites {
        AgentWrites::Refused
    }
}

/// The same rules for the kit's `anchor::resolve`, outside a session.
impl AnchorRules for PsdPack {
    fn judge(&self, was: &Fingerprint, hint: Option<&str>, now: &IndexNode) -> AnchorJudgment {
        crate::anchor::judge(was, hint, now)
    }

    fn candidate(
        &self,
        was: &Fingerprint,
        hint: Option<&str>,
        node: &IndexNode,
    ) -> Option<CandidateWhy> {
        crate::anchor::candidate(was, hint, node)
    }
}

/// Reads the file `src` holds into a draft, under `limits`: [`PsdPack`]'s index. Forward only;
/// it stops at the end of the layer and mask section.
pub fn index_psd(src: &mut dyn Read, limits: &IndexLimits) -> Result<IndexDraft, IndexRefused> {
    // The header first, to know the record layout (PSB's channel lengths are 8 bytes), then the
    // same bytes again for the walker.
    let mut head = [0u8; 26];
    let got = read_up_to(src, &mut head).map_err(IndexRefused::Read)?;
    if got < 4 || &head[..4] != b"8BPS" {
        return Err(IndexRefused::Unsupported);
    }
    let psb = got >= 6 && head[4..6] == [0, 2];
    let mut chained = io::Cursor::new(&head[..got]).chain(src);
    let mut sink = Sink::new(psb);
    let opts = WalkOptions {
        keep_resources: KEEP,
        ..WalkOptions::default()
    };
    let walked = walk_records(&mut chained, &opts, &mut sink).map_err(refusal)?;
    if walked.unreadable.is_some() {
        return Err(unreadable("the layer block can't be read"));
    }
    if sink.records.len() != walked.layers.len() {
        return Err(unreadable("the layer records were not all read"));
    }

    let mut draft = IndexDraft::new(if psb { "psb-2" } else { "psd-1" });
    let h = &walked.header;
    draft.attrs.insert("width".into(), h.width.to_string());
    draft.attrs.insert("height".into(), h.height.to_string());
    draft.attrs.insert("depth".into(), h.depth.to_string());
    draft
        .attrs
        .insert("color_mode".into(), h.color_mode.as_u16().to_string());
    if let Some(seed) = walked
        .resources
        .iter()
        .find(|r| r.id == RESOURCE_LAYER_ID_SEED)
        .and_then(|r| r.data.get(..4))
    {
        let seed = u32::from_be_bytes([seed[0], seed[1], seed[2], seed[3]]);
        draft.attrs.insert("layer_id_seed".into(), seed.to_string());
    }

    let records: Vec<&LayerRecord> = walked.layers.iter().map(|l| &l.record).collect();
    let nodes = records
        .iter()
        .filter(|r| r.section_type() != SectionType::BoundingDivider)
        .count();
    if nodes as u64 > u64::from(limits.max_nodes) {
        return Err(IndexRefused::TooManyNodes {
            count: nodes as u64,
            limit: limits.max_nodes,
        });
    }
    let tree = Tree::of(&records);
    let text_max = limits.max_text_bytes.min(INDEX_TEXT_MAX);
    let links = sink.links.items;
    let mut contents = sink.content;
    // Depth-first, top to bottom: (layer, depth, parent's draft index).
    let mut stack: Vec<(usize, u32, Option<u32>)> =
        tree.roots.iter().map(|&i| (i, 0, None)).collect();
    while let Some((i, depth, parent)) = stack.pop() {
        if depth > u32::from(limits.max_depth) {
            return Err(IndexRefused::TooDeep {
                depth,
                limit: limits.max_depth,
            });
        }
        let record = records[i];
        let digest = sink.records[i].ok_or_else(|| unreadable("a layer record was not read"))?;
        let mut node = node_of(record, digest, text_max, &links);
        node.parent = parent;
        if !matches!(node.kind.as_str(), "group" | "artboard") {
            node.content = contents
                .get_mut(i)
                .and_then(Option::take)
                .map(DigestStream::finish);
        }
        let me = draft.nodes.len() as u32;
        draft.nodes.push(node);
        for &c in &tree.children[i] {
            stack.push((c, depth + 1, Some(me)));
        }
    }
    Ok(draft)
}

fn read_up_to(src: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match src.read(&mut buf[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(got)
}

fn unreadable(why: &str) -> IndexRefused {
    IndexRefused::Unreadable {
        why: why.to_owned(),
    }
}

/// A walk's failure as the kit's refusal. The reasons are fixed text: nothing from the file.
fn refusal(e: WalkError) -> IndexRefused {
    match e {
        WalkError::Read(e) => IndexRefused::Read(e),
        WalkError::Truncated { .. } => unreadable("the file ends before its layer section does"),
        WalkError::Format(_) => unreadable("the file's layer section is malformed"),
        WalkError::Limit(what) => IndexRefused::Unreadable {
            why: format!("over a reader limit: {what}"),
        },
    }
}

/// Where each record sits: children per record (bottom to top, as stored) and the roots.
struct Tree {
    children: Vec<Vec<usize>>,
    roots: Vec<usize>,
}

impl Tree {
    /// The nesting of `records` (bottom to top), built flat: no recursion, however deep.
    fn of(records: &[&LayerRecord]) -> Self {
        let mut children = vec![Vec::new(); records.len()];
        let mut frames: Vec<Vec<usize>> = vec![Vec::new()];
        for (i, r) in records.iter().enumerate() {
            match r.section_type() {
                SectionType::BoundingDivider => frames.push(Vec::new()),
                SectionType::OpenFolder | SectionType::ClosedFolder => {
                    if frames.len() > 1 {
                        children[i] = frames.pop().unwrap_or_default();
                    }
                    if let Some(top) = frames.last_mut() {
                        top.push(i);
                    }
                }
                _ => {
                    if let Some(top) = frames.last_mut() {
                        top.push(i);
                    }
                }
            }
        }
        while frames.len() > 1 {
            let hoisted = frames.pop().unwrap_or_default();
            if let Some(top) = frames.last_mut() {
                top.extend(hoisted);
            }
        }
        Tree {
            children,
            roots: frames.pop().unwrap_or_default(),
        }
    }
}

fn has(record: &LayerRecord, keys: &[&[u8; 4]]) -> bool {
    record.blocks.iter().any(|b| keys.contains(&&b.key))
}

fn first<'a>(record: &'a LayerRecord, keys: &[&[u8; 4]]) -> Option<&'a TaggedBlock> {
    keys.iter().find_map(|k| record.block(k))
}

/// A four-character code as text: printable ASCII with trailing spaces dropped (`hue ` is
/// `hue`), anything else as `x` and its hex.
fn code_text(code: &[u8; 4]) -> String {
    if code.iter().all(|b| (0x20..0x7f).contains(b)) {
        String::from_utf8_lossy(code).trim_end().to_owned()
    } else {
        let mut s = String::from("x");
        for b in code {
            s.push_str(&format!("{b:02x}"));
        }
        s
    }
}

/// `s`, cut to an attribute value's limit.
fn attr_value(s: &str) -> String {
    cut_text(s, INDEX_ATTR_VALUE_MAX as u32).0.to_owned()
}

/// A record's typed blocks, read through one place: `bad` is set when a block this pack reads
/// is malformed.
struct Blocks<'a> {
    record: &'a LayerRecord,
    bad: bool,
}

impl<'a> Blocks<'a> {
    fn parsed(&mut self, key: &[u8; 4]) -> Option<BlockData<'a>> {
        match self.record.block(key)?.parsed()? {
            Ok(d) => Some(d),
            Err(_) => {
                self.bad = true;
                None
            }
        }
    }
}

/// The plain text of a `TySh` block: version, transform, text version, then the text
/// descriptor, whose `Txt ` item is the text.
fn type_text(data: &[u8]) -> Option<String> {
    let body = data.get(52..)?;
    let (d, _) = VersionedDescriptor::parse_prefix(body).ok()?;
    match d.descriptor.get("Txt ")? {
        Value::Text(t) => Some(t.to_string_lossy()),
        _ => None,
    }
}

/// The placed item's unique ID: `SoLd`/`SoLE` (`soLD`, version, descriptor with `Idnt`) or
/// `PlLd`/`plLd` (`plcL`, version, Pascal ID). `Err` when the block is malformed.
fn placed_id(record: &LayerRecord) -> Result<Option<String>, ()> {
    if let Some(b) = first(record, &[b"SoLd", b"SoLE"]) {
        let body = b
            .data
            .get(8..)
            .filter(|_| b.data.starts_with(b"soLD"))
            .ok_or(())?;
        let (d, _) = VersionedDescriptor::parse_prefix(body).map_err(|_| ())?;
        return match d.descriptor.get("Idnt") {
            Some(Value::Text(t)) => Ok(Some(t.to_string_lossy())),
            _ => Err(()),
        };
    }
    if let Some(b) = first(record, &[b"PlLd", b"plLd"]) {
        if !b.data.starts_with(b"plcL") {
            return Err(());
        }
        let n = usize::from(*b.data.get(8).ok_or(())?);
        let id = b.data.get(9..9 + n).ok_or(())?;
        return Ok(Some(String::from_utf8_lossy(id).into_owned()));
    }
    Ok(None)
}

/// The lock names of an `lspf` value, in a fixed order; `none` for none.
fn locks(bits: u32) -> String {
    let names: Vec<&str> = [
        (1u32, "transparent"),
        (1 << 1, "pixels"),
        (1 << 2, "position"),
        (1 << 31, "all"),
    ]
    .iter()
    .filter(|(b, _)| bits & b != 0)
    .map(|(_, n)| *n)
    .collect();
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.join(",")
    }
}

/// The node for one record (no parent, no content yet).
fn node_of(
    record: &LayerRecord,
    digest: Digest,
    text_max: u32,
    links: &HashMap<String, Link>,
) -> DraftNode {
    let mut rd = Blocks { record, bad: false };
    // the Unicode name, else the legacy one (as the full reader names it); a malformed `luni`
    // marks the node
    let _ = rd.parsed(b"luni");
    let name = record.name();
    let section = match rd.parsed(b"lsct") {
        Some(BlockData::SectionDivider(s)) => Some(s),
        _ => match rd.parsed(b"lsdk") {
            Some(BlockData::SectionDivider(s)) => Some(s),
            _ => None,
        },
    };
    let pixels_irrelevant = record.flags.bit4_useful() && record.flags.pixel_data_irrelevant();
    let vector = has(record, VECTOR_KEYS);
    let mut attrs = BTreeMap::new();
    let mut text = None;
    let mut text_cut = false;
    let kind = match section.as_ref().map(|s| s.kind) {
        Some(SectionType::OpenFolder | SectionType::ClosedFolder) => {
            if has(record, ARTBOARD_KEYS) {
                "artboard"
            } else {
                "group"
            }
        }
        Some(SectionType::Unknown(_)) => "other",
        _ if has(record, TEXT_KEYS) => {
            match record.block(b"TySh").map(|b| type_text(&b.data)) {
                Some(Some(t)) => {
                    let (kept, cut) = cut_text(&t, text_max);
                    text = Some(kept.to_owned());
                    text_cut = cut;
                }
                Some(None) => rd.bad = true,
                None => {}
            }
            "text"
        }
        _ if has(record, SMART_KEYS) => "smart",
        _ => {
            if let Some(b) = first(record, ADJUSTMENT_KEYS) {
                attrs.insert("adjustment_key".to_owned(), code_text(&b.key));
                "adjustment"
            } else if let Some(b) = first(record, FILL_KEYS) {
                attrs.insert("fill_key".to_owned(), code_text(&b.key));
                if pixels_irrelevant && vector {
                    "shape"
                } else {
                    "fill"
                }
            } else if pixels_irrelevant && vector {
                "shape"
            } else {
                "raster"
            }
        }
    };
    let group = matches!(kind, "group" | "artboard");

    let (name_kept, name_cut) = cut_text(&name, INDEX_NAME_MAX as u32);
    let mut node = DraftNode::new(kind, name_kept, digest);
    node.key = match rd.parsed(b"lyid") {
        Some(BlockData::LayerId(id)) => Some(format!("@lyid:{id}")),
        _ => None,
    };
    node.visible = record.is_visible();
    node.text = text;
    if !group {
        let r = &record.rect;
        if r.right > r.left && r.bottom > r.top {
            node.bounds = Some(NodeRect {
                left: i64::from(r.left),
                top: i64::from(r.top),
                right: i64::from(r.right),
                bottom: i64::from(r.bottom),
            });
        }
    }

    let blend = match (&section, group) {
        (Some(s), true) => s.blend_mode.unwrap_or(record.blend_mode),
        _ => record.blend_mode,
    };
    attrs.insert("blend".to_owned(), code_text(&blend.key()));
    attrs.insert("opacity".to_owned(), record.opacity.to_string());
    let fill_opacity = match rd.parsed(b"iOpa") {
        Some(BlockData::FillOpacity(v)) => v,
        _ => 255,
    };
    attrs.insert("fill_opacity".to_owned(), fill_opacity.to_string());
    attrs.insert(
        "clipped".to_owned(),
        if record.clipping != 0 {
            "true"
        } else {
            "false"
        }
        .to_owned(),
    );
    let lock_bits = match rd.parsed(b"lspf") {
        Some(BlockData::Protection(v)) => v,
        _ => 0,
    };
    attrs.insert("locks".to_owned(), locks(lock_bits));

    if kind == "smart" {
        match placed_id(record) {
            Ok(Some(id)) => {
                let id = attr_value(&id);
                if let Some(link) = links.get(&id) {
                    let source = match link {
                        Link::Embedded => "embedded".to_owned(),
                        Link::Linked(name) => attr_value(&format!("linked:{name}")),
                    };
                    attrs.insert("smart_source".to_owned(), source);
                }
                node.weak.insert("placed_id".to_owned(), id.clone());
                attrs.insert("placed_id".to_owned(), id);
            }
            Ok(None) => {}
            Err(()) => rd.bad = true,
        }
    }

    if matches!(record.mask, MaskData::Mask(_)) {
        node.flags.insert("raster_mask".to_owned());
    }
    if has(record, &[b"vmsk", b"vsms"]) {
        node.flags.insert("vector_mask".to_owned());
    }
    if has(record, STYLE_KEYS) {
        node.flags.insert("styles".to_owned());
    }
    if name_cut {
        node.flags.insert("name_truncated".to_owned());
    }
    if text_cut {
        node.flags.insert("text_truncated".to_owned());
    }
    if rd.bad {
        node.flags.insert("unreadable".to_owned());
    }
    node.attrs = attrs;
    node
}

/// Where a placed item's file is, from the global linked-file blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Link {
    /// The file is inside the document (`liFD`).
    Embedded,
    /// The file is outside it (`liFE`, `liFA`): its file name, never a path.
    Linked(String),
}

/// What the walk hands the pack: each record's digest and each layer's content digest, and the
/// linked-file items of the global blocks.
struct Sink {
    psb: bool,
    records: Vec<Option<Digest>>,
    content: Vec<Option<DigestStream>>,
    channel: Option<usize>,
    links: LinkScan,
}

impl Sink {
    fn new(psb: bool) -> Self {
        Sink {
            psb,
            records: Vec::new(),
            content: Vec::new(),
            channel: None,
            links: LinkScan::default(),
        }
    }
}

/// The record digest: the stored record with each channel's length zeroed (the ids stay).
fn record_digest(raw: &[u8], psb: bool) -> Digest {
    let mut d = Digest::stream();
    d.update(b"k2-mpc-psd/record/v1\0");
    let nch = raw
        .get(16..18)
        .map_or(0, |b| usize::from(u16::from_be_bytes([b[0], b[1]])));
    let entry = if psb { 10 } else { 6 };
    let table_end = (18 + nch * entry).min(raw.len());
    d.update(&raw[..18.min(raw.len())]);
    let zeros = [0u8; 8];
    let mut at = 18;
    while at + entry <= table_end {
        d.update(&raw[at..at + 2]);
        d.update(&zeros[..entry - 2]);
        at += entry;
    }
    d.update(&raw[at.min(raw.len())..]);
    d.finish()
}

impl WalkSink for Sink {
    fn layers_reset(&mut self) {
        self.records.clear();
        self.content.clear();
        self.channel = None;
    }

    fn record(&mut self, layer: usize, bytes: &[u8]) {
        if self.records.len() <= layer {
            self.records.resize(layer + 1, None);
            self.content.resize(layer + 1, None);
        }
        self.records[layer] = Some(record_digest(bytes, self.psb));
    }

    fn channel_begin(
        &mut self,
        layer: usize,
        _channel: usize,
        id: i16,
        compression: Compression,
        len: u64,
    ) {
        if self.content.len() <= layer {
            self.channel = None;
            return;
        }
        let d = self.content[layer].get_or_insert_with(|| {
            let mut d = Digest::stream();
            d.update(b"k2-mpc-psd/content/v1\0");
            d
        });
        d.update(&id.to_be_bytes());
        d.update(&compression.as_u16().to_be_bytes());
        d.update(&len.to_be_bytes());
        self.channel = Some(layer);
    }

    fn channel_bytes(&mut self, bytes: &[u8]) {
        if let Some(d) = self
            .channel
            .and_then(|l| self.content.get_mut(l))
            .and_then(Option::as_mut)
        {
            d.update(bytes);
        }
    }

    fn channel_end(&mut self) {
        self.channel = None;
    }

    fn global_block_begin(&mut self, key: [u8; 4], _len: u64) {
        self.links.begin(LINK_KEYS.contains(&&key));
    }

    fn global_block_bytes(&mut self, bytes: &[u8]) {
        self.links.bytes(bytes);
    }

    fn global_block_end(&mut self) {
        self.links.end();
    }
}

/// Reads linked-file items as their block's bytes pass: each item is a 64-bit length, the item,
/// and padding to four. Only an item's head is kept ([`LINK_HEAD_MAX`]), to read its type,
/// unique ID and file name; the file data is skipped.
#[derive(Default)]
struct LinkScan {
    state: Scan,
    items: HashMap<String, Link>,
}

#[derive(Default)]
enum Scan {
    /// Not in a linked-file block, or the block went wrong.
    #[default]
    Off,
    /// Reading an item's length.
    Len { buf: [u8; 8], have: usize },
    /// Inside an item: `left` bytes to go, the head so far, the padding after it.
    Item { left: u64, head: Vec<u8>, pad: u64 },
    /// Padding after an item.
    Pad { left: u64 },
}

impl LinkScan {
    fn begin(&mut self, linked_files: bool) {
        self.state = if linked_files {
            Scan::Len {
                buf: [0; 8],
                have: 0,
            }
        } else {
            Scan::Off
        };
    }

    fn end(&mut self) {
        self.state = Scan::Off;
    }

    fn bytes(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            match &mut self.state {
                Scan::Off => return,
                Scan::Len { buf, have } => {
                    let n = (8 - *have).min(bytes.len());
                    buf[*have..*have + n].copy_from_slice(&bytes[..n]);
                    *have += n;
                    bytes = &bytes[n..];
                    if *have == 8 {
                        let len = u64::from_be_bytes(*buf);
                        self.state = Scan::Item {
                            left: len,
                            head: Vec::new(),
                            pad: (4 - len % 4) % 4,
                        };
                        self.item_done_if_empty();
                    }
                }
                Scan::Item { left, head, .. } => {
                    let n = (*left).min(bytes.len() as u64) as usize;
                    let keep = (LINK_HEAD_MAX - head.len()).min(n);
                    head.extend_from_slice(&bytes[..keep]);
                    *left -= n as u64;
                    bytes = &bytes[n..];
                    self.item_done_if_empty();
                }
                Scan::Pad { left } => {
                    let n = (*left).min(bytes.len() as u64) as usize;
                    *left -= n as u64;
                    bytes = &bytes[n..];
                    if *left == 0 {
                        self.state = Scan::Len {
                            buf: [0; 8],
                            have: 0,
                        };
                    }
                }
            }
        }
    }

    /// An item whose bytes are all read: note it, then its padding.
    fn item_done_if_empty(&mut self) {
        let Scan::Item { left: 0, head, pad } = &mut self.state else {
            return;
        };
        let head = std::mem::take(head);
        let pad = *pad;
        if let Some((id, link)) = link_of(&head) {
            if self.items.len() < LINK_ITEMS_MAX {
                self.items.entry(id).or_insert(link);
            }
        }
        self.state = if pad == 0 {
            Scan::Len {
                buf: [0; 8],
                have: 0,
            }
        } else {
            Scan::Pad { left: pad }
        };
    }
}

/// An item's head: type, version, Pascal unique ID, Unicode file name (a count of UTF-16 units,
/// then the units).
fn link_of(head: &[u8]) -> Option<(String, Link)> {
    let kind: &[u8] = head.get(..4)?;
    let n = usize::from(*head.get(8)?);
    let id = String::from_utf8_lossy(head.get(9..9 + n)?).into_owned();
    let at = 9 + n;
    let count = u32::from_be_bytes(head.get(at..at + 4)?.try_into().ok()?) as usize;
    let units: Vec<u16> = head
        .get(at + 4..at.checked_add(4)?.checked_add(count.checked_mul(2)?)?)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    let mut name = String::from_utf16_lossy(&units);
    while name.ends_with('\0') {
        name.pop();
    }
    // a file name only: whatever precedes the last separator is a path and stays out
    let name = name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .to_owned();
    let id = cut_text(&id, INDEX_ATTR_VALUE_MAX as u32).0.to_owned();
    match kind {
        b"liFD" => Some((id, Link::Embedded)),
        b"liFE" | b"liFA" => Some((id, Link::Linked(name))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_link_item_head_gives_its_id_and_file_name_only() {
        let mut h = b"liFE".to_vec();
        h.extend(7u32.to_be_bytes());
        h.push(3);
        h.extend_from_slice(b"abc");
        let name: Vec<u16> = "dir/sub\\file.png".encode_utf16().collect();
        h.extend((name.len() as u32).to_be_bytes());
        for u in name {
            h.extend(u.to_be_bytes());
        }
        assert_eq!(
            link_of(&h),
            Some(("abc".to_owned(), Link::Linked("file.png".to_owned())))
        );
        h[..4].copy_from_slice(b"liFD");
        assert_eq!(link_of(&h), Some(("abc".to_owned(), Link::Embedded)));
        h[..4].copy_from_slice(b"zzzz");
        assert_eq!(link_of(&h), None);
        assert_eq!(link_of(&h[..10]), None, "a cut head reads as nothing");
    }

    #[test]
    fn the_link_scan_reads_items_split_anywhere() {
        let item = |kind: &[u8; 4], id: &str, data: usize| {
            let mut b = kind.to_vec();
            b.extend(7u32.to_be_bytes());
            b.push(id.len() as u8);
            b.extend_from_slice(id.as_bytes());
            b.extend(1u32.to_be_bytes());
            b.extend(u16::from(b'f').to_be_bytes());
            b.extend(vec![9u8; data]);
            let mut out = (b.len() as u64).to_be_bytes().to_vec();
            out.extend_from_slice(&b);
            while !out.len().is_multiple_of(4) {
                out.push(0);
            }
            out
        };
        let mut block = item(b"liFD", "one", 3);
        block.extend(item(b"liFE", "two", LINK_HEAD_MAX + 5));
        for step in [1, 3, 7, 4096, usize::MAX] {
            let mut s = LinkScan::default();
            s.begin(true);
            for piece in block.chunks(step.min(block.len())) {
                s.bytes(piece);
            }
            s.end();
            assert_eq!(s.items.len(), 2, "step {step}");
            assert_eq!(s.items["one"], Link::Embedded);
            assert_eq!(s.items["two"], Link::Linked("f".to_owned()));
        }
    }

    #[test]
    fn the_record_digest_ignores_channel_lengths_and_nothing_else() {
        let rec = |len: u32, name: u8| {
            let mut r = vec![0u8; 16];
            r.extend(1u16.to_be_bytes());
            r.extend(0i16.to_be_bytes());
            r.extend(len.to_be_bytes());
            r.extend_from_slice(b"8BIMnorm");
            r.push(name);
            r
        };
        assert_eq!(
            record_digest(&rec(10, 1), false),
            record_digest(&rec(99, 1), false)
        );
        assert_ne!(
            record_digest(&rec(10, 1), false),
            record_digest(&rec(10, 2), false)
        );
        let mut other_id = rec(10, 1);
        other_id[19] = 1;
        assert_ne!(
            record_digest(&rec(10, 1), false),
            record_digest(&other_id, false)
        );
    }

    #[test]
    fn locks_are_named_in_a_fixed_order() {
        assert_eq!(locks(0), "none");
        assert_eq!(locks(0b101), "transparent,position");
        assert_eq!(locks(1 << 31 | 2), "pixels,all");
    }

    #[test]
    fn codes_read_as_text_or_hex() {
        assert_eq!(code_text(b"norm"), "norm");
        assert_eq!(code_text(b"hue "), "hue");
        assert_eq!(code_text(&[0, 1, 0xff, b'a']), "x0001ff61");
    }
}
