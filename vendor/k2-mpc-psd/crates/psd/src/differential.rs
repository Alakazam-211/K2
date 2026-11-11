//! Test-only: compares a records-only walk of a file with the full reader's view of the same
//! file. Compiled only with the `differential` feature, for this repo's tests (and its fuzz targets,
//! on the main branch).

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::Read;

use photocraft_psd::compression::Compression;
use photocraft_psd::layer::LayerRecord;
use photocraft_psd::records::{
    walk_records, LayerSource, WalkError, WalkOptions, WalkSink, Walked,
};
use photocraft_psd::{LayerInfoPlacement, PsdFile};

/// Resource ids the comparison asks a walk to keep: common ones, so a test sees the
/// resource walk select and skip.
pub const KEEP_RESOURCES: &[u16] = &[1005, 1036, 1039, 1044, 1057, 1060];

/// A channel's bytes as the sink received them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecordedChannel {
    /// The channel id passed to `channel_begin`.
    pub id: i16,
    /// The compression passed to `channel_begin`.
    pub compression: Option<Compression>,
    /// The length passed to `channel_begin`.
    pub len: u64,
    /// The bytes received, in order.
    pub data: Vec<u8>,
    /// Whether the channel was ended.
    pub ended: bool,
}

/// A sink that records everything it is given, per (layer, channel) position.
#[derive(Debug, Clone, Default)]
pub struct Recording {
    /// The channels, by (layer, channel) position.
    pub channels: BTreeMap<(usize, usize), RecordedChannel>,
    /// The raw records, by layer position.
    pub records: BTreeMap<usize, Vec<u8>>,
    /// How many times the layers were discarded.
    pub resets: u32,
    open: Option<(usize, usize)>,
}

impl WalkSink for Recording {
    fn layers_reset(&mut self) {
        self.channels.clear();
        self.records.clear();
        self.open = None;
        self.resets += 1;
    }

    fn record(&mut self, layer: usize, bytes: &[u8]) {
        assert!(
            self.records.insert(layer, bytes.to_vec()).is_none(),
            "record given twice"
        );
    }

    fn channel_begin(
        &mut self,
        layer: usize,
        channel: usize,
        id: i16,
        compression: Compression,
        len: u64,
    ) {
        assert!(self.open.is_none(), "channel_begin while a channel is open");
        let fresh = RecordedChannel {
            id,
            compression: Some(compression),
            len,
            data: Vec::new(),
            ended: false,
        };
        assert!(
            self.channels.insert((layer, channel), fresh).is_none(),
            "channel begun twice"
        );
        self.open = Some((layer, channel));
    }

    fn channel_bytes(&mut self, bytes: &[u8]) {
        let open = self.open.expect("channel_bytes outside a channel");
        self.channels
            .get_mut(&open)
            .expect("the open channel")
            .data
            .extend_from_slice(bytes);
    }

    fn channel_end(&mut self) {
        let open = self.open.take().expect("channel_end outside a channel");
        self.channels
            .get_mut(&open)
            .expect("the open channel")
            .ended = true;
    }
}

/// `file`'s channels, emptied the way a walk returns them.
fn walked_shape(mut record: LayerRecord) -> LayerRecord {
    for ch in &mut record.channels {
        ch.compression = None;
        ch.data = Vec::new();
    }
    record
}

/// Walks `data`, reading in pieces of `step` bytes, and returns the outcome with what the sink saw.
pub fn walk_stepped(
    data: &[u8],
    step: usize,
    opts: &WalkOptions<'_>,
) -> (Result<Walked, WalkError>, Recording) {
    let mut sink = Recording::default();
    let mut src = Stepped {
        data,
        step: step.max(1),
    };
    let out = walk_records(&mut src, opts, &mut sink);
    (out, sink)
}

/// A reader that returns at most `step` bytes per call.
#[derive(Debug, Clone)]
pub struct Stepped<'a> {
    /// The bytes left.
    pub data: &'a [u8],
    /// The largest read.
    pub step: usize,
}

impl Read for Stepped<'_> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let n = self.step.min(out.len()).min(self.data.len());
        out[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        Ok(n)
    }
}

/// Compares a walk of `data` with the full reader. `Ok(())` when the full reader refuses the file
/// (a walk may accept more: it stops before the merged image). Otherwise the walk must succeed
/// and agree on everything it returns. The `Err` says what differed, in the structure's own
/// terms: a test failure message, not for a log.
pub fn compare_with_full(data: &[u8]) -> Result<(), String> {
    let Ok(full) = PsdFile::from_bytes(data) else {
        return Ok(());
    };
    let opts = WalkOptions {
        keep_resources: KEEP_RESOURCES,
        ..Default::default()
    };
    let (out, sink) = walk_stepped(data, usize::MAX, &opts);
    let walked = out.map_err(|e| format!("the walk failed where the full reader did not: {e}"))?;
    check_agrees(&full, &walked, &sink, data.len() as u64)
}

/// Checks a successful walk against the full reader's model of the same file.
pub fn check_agrees(
    full: &PsdFile,
    w: &Walked,
    sink: &Recording,
    file_len: u64,
) -> Result<(), String> {
    if w.header != full.header {
        return Err("the headers differ".into());
    }
    if w.consumed > file_len {
        return Err(format!(
            "consumed {} of a {}-byte file",
            w.consumed, file_len
        ));
    }
    let want_resources: Vec<_> = full
        .resources
        .iter()
        .filter(|r| KEEP_RESOURCES.contains(&r.id))
        .cloned()
        .collect();
    if w.resources != want_resources {
        return Err(format!(
            "resources differ: kept {} of the {} asked for",
            w.resources.len(),
            want_resources.len()
        ));
    }
    let (want_merged, want_layers) = match &full.layer_info {
        Some(info) => (info.merged_alpha, &info.layers[..]),
        None => (false, &[][..]),
    };
    let want_source = match (&full.layer_info, &full.layer_info_placement) {
        (None, _) => LayerSource::None,
        (Some(_), LayerInfoPlacement::Section) => LayerSource::Section,
        (Some(_), LayerInfoPlacement::GlobalBlock { index, key, .. }) => LayerSource::GlobalBlock {
            index: *index,
            key: *key,
        },
    };
    if w.source != want_source {
        return Err(format!(
            "layer source differs: {:?} vs {:?}",
            w.source, want_source
        ));
    }
    if w.merged_alpha != want_merged {
        return Err("merged alpha differs".into());
    }
    if w.unreadable.is_some() && full.layer_info.is_some() {
        return Err(
            "the walk reports an unreadable layer block but the full reader has layers".into(),
        );
    }
    if w.layers.len() != want_layers.len() {
        return Err(format!(
            "layer count differs: {} vs {}",
            w.layers.len(),
            want_layers.len()
        ));
    }
    let mut diff = String::new();
    for (i, (got, want)) in w.layers.iter().zip(want_layers).enumerate() {
        // Debug, not ==: a mask's feather can be NaN, which is never equal to itself.
        if format!("{:?}", got.record) != format!("{:?}", walked_shape(want.clone())) {
            let _ = writeln!(diff, "layer {i}: the records differ");
        }
        if got.channels.len() != want.channels.len() {
            let _ = writeln!(
                diff,
                "layer {i}: channel count {} vs {}",
                got.channels.len(),
                want.channels.len()
            );
            continue;
        }
        for (c, (gch, wch)) in got.channels.iter().zip(&want.channels).enumerate() {
            let label = format!("layer {i} channel {c}");
            if gch.id != wch.id {
                let _ = writeln!(diff, "{label}: id");
            }
            if gch.compression != wch.compression {
                let _ = writeln!(diff, "{label}: compression");
            }
            let want_len = if wch.compression.is_some() {
                2 + wch.data.len() as u64
            } else {
                0
            };
            if gch.stored_len != want_len {
                let _ = writeln!(
                    diff,
                    "{label}: stored length {} vs {}",
                    gch.stored_len, want_len
                );
            }
            if wch.compression.is_some() && gch.data_offset + gch.stored_len - 2 > w.consumed {
                let _ = writeln!(diff, "{label}: data past the end of the section");
            }
            match sink.channels.get(&(i, c)) {
                Some(seen) if wch.compression.is_some() => {
                    if seen.data != wch.data
                        || seen.len != wch.data.len() as u64
                        || !seen.ended
                        || seen.compression != wch.compression
                    {
                        let _ = writeln!(diff, "{label}: the sink's bytes differ");
                    }
                }
                None if wch.compression.is_none() => {}
                _ => {
                    let _ = writeln!(
                        diff,
                        "{label}: seen by the sink when it should not be, or not seen"
                    );
                }
            }
        }
    }
    if diff.is_empty() {
        Ok(())
    } else {
        Err(diff)
    }
}

// ----- the index against the full reader -----

/// The limits [`compare_index_with_full`] indexes under: no node cap, the kit's text cap, and a
/// depth the full reader's tree is built to in full.
pub const COMPARE_DEPTH: u16 = 200;

/// One node as the full reader's model says it is: what the index of the same file must hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullNode {
    /// The record's name.
    pub name: String,
    /// Its `lyid`.
    pub layer_id: Option<u32>,
    /// The parent's position in the list.
    pub parent: Option<u32>,
    /// A group (or artboard) in the full reader's tree.
    pub group: bool,
    /// The record's own flag.
    pub visible: bool,
    /// `left, top, right, bottom`, when not a group and not empty.
    pub bounds: Option<[i64; 4]>,
    /// The content digest of its channels as stored, computed from the whole model.
    pub content: Option<k2_mpc_base::index::Digest>,
}

/// The full reader's nodes for `full`, in the index's order (parent first, siblings top to
/// bottom), from its own tree ([`PsdFile::layer_tree_max_depth`]). `None` when the tree is deeper
/// than [`COMPARE_DEPTH`].
pub fn full_nodes(full: &PsdFile) -> Option<Vec<FullNode>> {
    use photocraft_psd::tree::LayerNode;
    let roots = full.layer_tree_max_depth(usize::from(COMPARE_DEPTH) + 1);
    let layers = full.layers();
    let mut out = Vec::new();
    // (node, depth, parent): a root's children come off the stack top first
    let mut stack: Vec<(&LayerNode, u16, Option<u32>)> =
        roots.iter().map(|n| (n, 0, None)).collect();
    while let Some((n, depth, parent)) = stack.pop() {
        if depth > COMPARE_DEPTH {
            return None;
        }
        let r = &layers[n.index()];
        let group = n.is_group();
        let rect = r.rect;
        let bounds = (!group && rect.right > rect.left && rect.bottom > rect.top).then(|| {
            [
                i64::from(rect.left),
                i64::from(rect.top),
                i64::from(rect.right),
                i64::from(rect.bottom),
            ]
        });
        let content = if group { None } else { content_of(r) };
        let me = out.len() as u32;
        out.push(FullNode {
            name: r.name(),
            layer_id: r.layer_id(),
            parent,
            group,
            visible: r.is_visible(),
            bounds,
            content,
        });
        for c in n.children() {
            stack.push((c, depth + 1, Some(me)));
        }
    }
    Some(out)
}

/// The content digest of a record's channels as the full reader holds them: each channel with
/// data, its id, compression, length and bytes.
fn content_of(r: &LayerRecord) -> Option<k2_mpc_base::index::Digest> {
    let mut d: Option<k2_mpc_base::index::DigestStream> = None;
    for ch in &r.channels {
        let Some(c) = ch.compression else { continue };
        let d = d.get_or_insert_with(|| {
            let mut d = k2_mpc_base::index::Digest::stream();
            d.update(b"k2-mpc-psd/content/v1\0");
            d
        });
        d.update(&ch.id.to_be_bytes());
        d.update(&c.as_u16().to_be_bytes());
        d.update(&(ch.data.len() as u64).to_be_bytes());
        d.update(&ch.data);
    }
    d.map(k2_mpc_base::index::DigestStream::finish)
}

/// Compares the pack's index of `data` with the full reader's tree. `Ok(())` when the full reader
/// refuses the file, or its tree is deeper than [`COMPARE_DEPTH`]. Otherwise the index must read
/// the file and agree, node for node, on name, layer ID, parent, group-ness, visibility, bounds
/// and content digest. The `Err` names what differed, never a name or a byte of the file.
pub fn compare_index_with_full(data: &[u8]) -> Result<(), String> {
    use k2_mpc_base::index::{IndexLimits, IndexRefused, INDEX_TEXT_MAX};
    let Ok(full) = PsdFile::from_bytes(data) else {
        return Ok(());
    };
    let limits = IndexLimits::new(u32::MAX, COMPARE_DEPTH, u64::MAX, INDEX_TEXT_MAX);
    let mut src = data;
    let out = crate::index_psd(&mut src, &limits);
    if full.layer_info.is_none() && full.unreadable_layer_block().is_some() {
        return match out {
            Err(IndexRefused::Unreadable { .. }) => Ok(()),
            _ => Err("the full reader lost the layers and the index did not refuse".into()),
        };
    }
    let Some(want) = full_nodes(&full) else {
        return Ok(());
    };
    let draft = match out {
        Ok(d) => d,
        Err(IndexRefused::TooDeep { .. }) => return Ok(()),
        Err(e) => {
            return Err(format!(
                "the index refused a file the full reader reads: {e}"
            ))
        }
    };
    if draft.nodes.len() != want.len() {
        return Err(format!(
            "node count {} vs the full reader's {}",
            draft.nodes.len(),
            want.len()
        ));
    }
    let mut diff = String::new();
    for (i, (got, want)) in draft.nodes.iter().zip(&want).enumerate() {
        let name =
            k2_mpc_base::index::cut_text(&want.name, k2_mpc_base::index::INDEX_NAME_MAX as u32).0;
        if got.name != name {
            let _ = writeln!(diff, "node {i}: name");
        }
        if got.key != want.layer_id.map(|id| format!("@lyid:{id}")) {
            let _ = writeln!(diff, "node {i}: layer ID");
        }
        if got.parent != want.parent {
            let _ = writeln!(
                diff,
                "node {i}: parent {:?} vs {:?}",
                got.parent, want.parent
            );
        }
        if matches!(got.kind.as_str(), "group" | "artboard") != want.group {
            let _ = writeln!(diff, "node {i}: group or not");
        }
        if got.visible != want.visible {
            let _ = writeln!(diff, "node {i}: visible");
        }
        let bounds = got.bounds.map(|b| [b.left, b.top, b.right, b.bottom]);
        if bounds != want.bounds {
            let _ = writeln!(diff, "node {i}: bounds");
        }
        if got.content != want.content {
            let _ = writeln!(diff, "node {i}: content digest");
        }
    }
    if diff.is_empty() {
        Ok(())
    } else {
        Err(diff)
    }
}
