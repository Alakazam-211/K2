// Added by Alakazam Labs: this file is not part of the original PhotoCraft crate (see VENDORED.md).
//! A records-only walker for the layer records of a PSD or PSB file, from a stream.
//!
//! [`crate::PsdFile::from_bytes`] needs the whole file in one slice and copies every channel's
//! encoded bytes into its model. [`walk_records`] reads forward from any [`Read`], holds the
//! layer records and nothing else, and hands the channel bytes to a [`WalkSink`] as they pass, in
//! pieces from a fixed 64 KiB buffer: a multi-GB file never needs more than that plus its
//! records, and the pixels are never copied or decoded.
//!
//! Every other global tagged block passes to the sink the same way, so a reader can pick out the
//! few it wants (the linked-file items of `lnk2` and its kin) without holding the rest.
//!
//! It reads the same structure as the slice parser: this module reuses [`LayerRecord`]'s own
//! reader, and its section and padding rules follow `PsdFile::from_bytes` and `LayerInfo::read_body`
//! step for step (the differential tests and fuzz targets hold the two together). It stops at the
//! end of the layer and mask section and does not read the merged image, so a caller that needs
//! a verified end of the stream drains the reader itself.
//!
//! Limits are decided before anything is held: see [`WalkLimits`]. The walker never allocates a
//! buffer from a length field: each length is first checked against the section around it, and
//! bytes are held only as they arrive.

use std::io::{self, Read};

use crate::compression::Compression;
use crate::error::PsdError;
use crate::header::{Header, Version};
use crate::io::Reader;
use crate::layer::{LayerRecord, MAX_LAYER_CHANNELS};
use crate::resources::ImageResource;
use crate::tagged::{is_sig, uses_long_length};

/// Size of the fixed buffer the bytes pass through.
const BUF: usize = 64 * 1024;

/// What a walk may hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkLimits {
    /// Most bytes one layer record may hold (fixed part, mask, ranges, name and blocks).
    pub max_record_bytes: u64,
    /// Most bytes all layer records may hold together.
    pub max_total_record_bytes: u64,
    /// Most layers accepted.
    pub max_layers: u32,
    /// Largest image resource kept on request ([`WalkOptions::keep_resources`]).
    pub max_kept_resource_bytes: u32,
}

impl Default for WalkLimits {
    fn default() -> Self {
        WalkLimits { max_record_bytes: 64 * 1024 * 1024, max_total_record_bytes: 256 * 1024 * 1024, max_layers: 32_768, max_kept_resource_bytes: 1024 * 1024 }
    }
}

/// Options of a walk.
#[derive(Debug, Clone, Default)]
pub struct WalkOptions<'a> {
    /// The limits.
    pub limits: WalkLimits,
    /// Image resource ids to keep (every other resource is skipped). Empty keeps none.
    pub keep_resources: &'a [u16],
}

/// Receives what a walk reads, as it passes. Every method has a default that ignores it.
///
/// A walk may discard layers it has already passed to the sink when a later layer block replaces
/// them, or turns out to be unreadable. [`WalkSink::layers_reset`] says so: what was received
/// before it no longer applies. The layers a successful walk returns are always the last ones
/// received since the last reset, in order.
pub trait WalkSink {
    /// The layers received so far no longer apply: forget them.
    fn layers_reset(&mut self) {}
    /// The whole stored record of layer `layer` (without channel data), as read.
    fn record(&mut self, layer: usize, bytes: &[u8]) {
        let _ = (layer, bytes);
    }
    /// Channel `channel` of layer `layer` starts: `len` bytes of encoded data follow.
    fn channel_begin(&mut self, layer: usize, channel: usize, id: i16, compression: Compression, len: u64) {
        let _ = (layer, channel, id, compression, len);
    }
    /// A piece of the current channel's encoded data, in file order.
    fn channel_bytes(&mut self, bytes: &[u8]) {
        let _ = bytes;
    }
    /// The current channel ended.
    fn channel_end(&mut self) {}
    /// A global tagged block that does not hold the layers starts: `len` data bytes follow (the
    /// signature, key and length field are already read).
    fn global_block_begin(&mut self, key: [u8; 4], len: u64) {
        let _ = (key, len);
    }
    /// A piece of the current global block's data, in file order.
    fn global_block_bytes(&mut self, bytes: &[u8]) {
        let _ = bytes;
    }
    /// The current global block ended.
    fn global_block_end(&mut self) {}
}

impl WalkSink for () {}

/// Why a walk failed.
#[derive(Debug, thiserror::Error)]
pub enum WalkError {
    /// The reader itself failed: a damaged source, not a malformed file.
    #[error("reading failed: {0}")]
    Read(#[from] io::Error),
    /// The file ended before a section it declared was complete.
    #[error("the file ended early at offset {offset}")]
    Truncated {
        /// Offset where the stream ended.
        offset: u64,
    },
    /// The file breaks the format.
    #[error("malformed file: {0}")]
    Format(#[from] PsdError),
    /// The file is over a limit of [`WalkLimits`].
    #[error("over a limit: {0}")]
    Limit(&'static str),
}

/// Where the layers of a walked file were stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerSource {
    /// The file has no layer info.
    None,
    /// In the layer info field of the layer and mask section.
    Section,
    /// In a global tagged block.
    GlobalBlock {
        /// Position among the global blocks, not counting the layer block itself.
        index: usize,
        /// Block key (`Lr16`, `Lr32` or `Layr`).
        key: [u8; 4],
    },
}

/// One channel of a walked layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedChannel {
    /// Channel id.
    pub id: i16,
    /// Compression; `None` when the stored length was 0 (no data at all).
    pub compression: Option<Compression>,
    /// The stored length field: compression field plus encoded bytes.
    pub stored_len: u64,
    /// Offset in the file where the encoded bytes (after the compression field) begin. Meaningless
    /// when `compression` is `None`.
    pub data_offset: u64,
}

/// One walked layer.
#[derive(Debug, Clone, PartialEq)]
pub struct WalkedLayer {
    /// The record, with the same fields as [`crate::PsdFile`] reads. Its `channels` hold the ids
    /// only: their `compression` is `None` and `data` is empty, so a decode fails loudly. Use
    /// [`WalkedLayer::channels`] for their sizes.
    pub record: LayerRecord,
    /// The channels in stored order.
    pub channels: Vec<WalkedChannel>,
}

/// The outcome of a walk.
#[derive(Debug, Clone, PartialEq)]
pub struct Walked {
    /// The file header.
    pub header: Header,
    /// The image resources asked for, in file order.
    pub resources: Vec<ImageResource>,
    /// The layer count was stored negative.
    pub merged_alpha: bool,
    /// Layers in file order (bottom-most first).
    pub layers: Vec<WalkedLayer>,
    /// Where they came from.
    pub source: LayerSource,
    /// A global layer block that should have held the layers but could not be parsed: the file
    /// reads as flattened, and this says its layers were lost.
    pub unreadable: Option<PsdError>,
    /// Offset where the walk stopped: the end of the layer and mask section.
    pub consumed: u64,
}

/// Forward cursor over a reader, through one fixed buffer.
struct Cursor<'a, R: ?Sized> {
    src: &'a mut R,
    buf: Vec<u8>,
    start: usize,
    stop: usize,
    pos: u64,
}

const NO_END: u64 = u64::MAX;

impl<'a, R: Read + ?Sized> Cursor<'a, R> {
    fn new(src: &'a mut R) -> Self {
        Cursor { src, buf: vec![0; BUF], start: 0, stop: 0, pos: 0 }
    }

    fn remaining(&self, end: u64) -> u64 {
        end.saturating_sub(self.pos)
    }

    /// Errors unless `n` bytes fit before `end`. A format error: the file says so itself.
    fn bound(&self, n: u64, end: u64) -> Result<(), WalkError> {
        let left = self.remaining(end);
        if n > left {
            return Err(WalkError::Format(PsdError::UnexpectedEof {
                offset: usize::try_from(self.pos).unwrap_or(usize::MAX),
                needed: usize::try_from(n - left).unwrap_or(usize::MAX),
            }));
        }
        Ok(())
    }

    /// Fills the buffer to at least `need` (≤ [`BUF`]) bytes, or to the end of the stream.
    fn fill(&mut self, need: usize) -> Result<usize, WalkError> {
        while self.stop - self.start < need {
            if self.start > 0 {
                self.buf.copy_within(self.start..self.stop, 0);
                self.stop -= self.start;
                self.start = 0;
            }
            let n = loop {
                match self.src.read(&mut self.buf[self.stop..]) {
                    Ok(n) => break n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(WalkError::Read(e)),
                }
            };
            if n == 0 {
                break;
            }
            self.stop += n;
        }
        Ok(self.stop - self.start)
    }

    /// The next `n` (≤ [`BUF`]) bytes, without consuming them. Errors if the stream ends first.
    fn peek(&mut self, n: usize) -> Result<&[u8], WalkError> {
        let avail = self.fill(n)?;
        if avail < n {
            return Err(WalkError::Truncated { offset: self.pos + avail as u64 });
        }
        Ok(&self.buf[self.start..self.start + n])
    }

    /// The next `n` (≤ [`BUF`]) bytes, consumed; `n` must fit before `end`.
    fn take(&mut self, n: usize, end: u64) -> Result<&[u8], WalkError> {
        self.bound(n as u64, end)?;
        let avail = self.fill(n)?;
        if avail < n {
            return Err(WalkError::Truncated { offset: self.pos + avail as u64 });
        }
        let at = self.start;
        self.start += n;
        self.pos += n as u64;
        Ok(&self.buf[at..at + n])
    }

    /// Moves past `n` bytes, which must fit before `end`, handing each piece to `each`.
    fn skip_with(&mut self, n: u64, end: u64, mut each: impl FnMut(&[u8])) -> Result<(), WalkError> {
        self.bound(n, end)?;
        let mut left = n;
        while left > 0 {
            let avail = self.fill(1)?;
            if avail == 0 {
                return Err(WalkError::Truncated { offset: self.pos });
            }
            let piece = left.min(avail as u64) as usize;
            each(&self.buf[self.start..self.start + piece]);
            self.start += piece;
            self.pos += piece as u64;
            left -= piece as u64;
        }
        Ok(())
    }

    fn skip(&mut self, n: u64, end: u64) -> Result<(), WalkError> {
        self.skip_with(n, end, |_| {})
    }

    fn skip_to(&mut self, target: u64) -> Result<(), WalkError> {
        let n = target.saturating_sub(self.pos);
        self.skip(n, target)
    }

    fn u16(&mut self, end: u64) -> Result<u16, WalkError> {
        let b = self.take(2, end)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn i16(&mut self, end: u64) -> Result<i16, WalkError> {
        let b = self.take(2, end)?;
        Ok(i16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self, end: u64) -> Result<u32, WalkError> {
        let b = self.take(4, end)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u64(&mut self, end: u64) -> Result<u64, WalkError> {
        let b = self.take(8, end)?;
        Ok(u64::from_be_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]))
    }

    fn len_field(&mut self, long: bool, end: u64) -> Result<u64, WalkError> {
        if long { self.u64(end) } else { Ok(u64::from(self.u32(end)?)) }
    }

    /// Appends the next `n` bytes, which must fit before `end`, to `out`.
    fn append(&mut self, out: &mut Vec<u8>, n: u64, end: u64) -> Result<(), WalkError> {
        self.skip_with(n, end, |piece| out.extend_from_slice(piece))
    }
}

/// Walks the layer records of a PSD or PSB file from `src`. See the module docs.
pub fn walk_records<R: Read + ?Sized, S: WalkSink + ?Sized>(src: &mut R, opts: &WalkOptions<'_>, sink: &mut S) -> Result<Walked, WalkError> {
    let mut w = Walker { cur: Cursor::new(src), limits: &opts.limits, keep: opts.keep_resources, sink, held: 0 };
    w.file()
}

struct Walker<'a, 'b, R: ?Sized, S: ?Sized> {
    cur: Cursor<'a, R>,
    limits: &'b WalkLimits,
    keep: &'b [u16],
    sink: &'a mut S,
    held: u64,
}

impl<R: Read + ?Sized, S: WalkSink + ?Sized> Walker<'_, '_, R, S> {
    fn file(&mut self) -> Result<Walked, WalkError> {
        let header = {
            let b = self.cur.take(26, NO_END)?;
            Header::read(&mut Reader::new(b))?
        };
        let psb = header.version.is_psb();
        let cmd_len = self.cur.u32(NO_END)?;
        self.cur.skip(u64::from(cmd_len), NO_END)?;
        let resources = self.resources()?;

        let mut merged_alpha = false;
        let mut layers = Vec::new();
        let mut source = LayerSource::None;
        let mut unreadable = None;

        let lm_len = self.cur.len_field(psb, NO_END)?;
        let lm_end = self.cur.pos.saturating_add(lm_len);
        if lm_len > 0 {
            let li_len = self.cur.len_field(psb, lm_end)?;
            self.cur.bound(li_len, lm_end)?;
            if li_len > 0 {
                let li_end = self.cur.pos + li_len;
                let (ma, ls) = self.layer_info(li_end, header.version)?;
                merged_alpha = ma;
                layers = ls;
                source = LayerSource::Section;
            }
            if self.cur.remaining(lm_end) >= 4 {
                let mask_len = self.cur.u32(lm_end)?;
                self.cur.skip(u64::from(mask_len), lm_end)?;
                self.global_blocks(&header, lm_end, &mut merged_alpha, &mut layers, &mut source, &mut unreadable)?;
            } else {
                self.cur.skip_to(lm_end)?;
            }
        }
        Ok(Walked { header, resources, merged_alpha, layers, source, unreadable, consumed: self.cur.pos })
    }

    /// The image resources section: skipped, except the ids asked for.
    fn resources(&mut self) -> Result<Vec<ImageResource>, WalkError> {
        let len = self.cur.u32(NO_END)?;
        let end = self.cur.pos.saturating_add(u64::from(len));
        let mut kept = Vec::new();
        while self.cur.pos < end {
            let signature: [u8; 4] = {
                let b = self.cur.take(4, end)?;
                [b[0], b[1], b[2], b[3]]
            };
            if !matches!(&signature, b"8BIM" | b"MeSa" | b"AgHg" | b"PHUT" | b"DCSR") {
                return Err(PsdError::InvalidSignature { expected: "8BIM resource", found: signature }.into());
            }
            let id = self.cur.u16(end)?;
            // A Pascal name padded to even (the length byte counted).
            let name_len = u64::from(self.cur.take(1, end)?[0]);
            let keep = self.keep.contains(&id);
            let mut name = Vec::new();
            if keep {
                self.cur.append(&mut name, name_len, end)?;
            } else {
                self.cur.skip(name_len, end)?;
            }
            if !(1 + name_len).is_multiple_of(2) {
                self.cur.skip(1, end)?;
            }
            let data_len = self.cur.u32(end)?;
            self.cur.bound(u64::from(data_len), end)?;
            if keep {
                if data_len > self.limits.max_kept_resource_bytes {
                    return Err(WalkError::Limit("a kept image resource is too large"));
                }
                let mut data = Vec::new();
                self.cur.append(&mut data, u64::from(data_len), end)?;
                kept.push(ImageResource { signature, id, name, data });
            } else {
                self.cur.skip(u64::from(data_len), end)?;
            }
            if data_len % 2 == 1 {
                self.cur.skip(1, end)?;
            }
        }
        Ok(kept)
    }

    /// The global tagged blocks: skipped, except the block that may hold the layers.
    fn global_blocks(
        &mut self,
        header: &Header,
        lm_end: u64,
        merged_alpha: &mut bool,
        layers: &mut Vec<WalkedLayer>,
        source: &mut LayerSource,
        unreadable: &mut Option<PsdError>,
    ) -> Result<(), WalkError> {
        let target: Option<&[u8; 4]> = match header.depth {
            16 => Some(b"Lr16"),
            32 => Some(b"Lr32"),
            _ => None,
        };
        let mut candidate_seen = match target {
            Some(_) => false,
            None => !layers.is_empty(),
        };
        // Blocks before this one: a layer block is only ever consumed once, so it never counts.
        let mut kept = 0usize;
        loop {
            let rest = self.cur.remaining(lm_end);
            let look = self.cur.peek(rest.min(12) as usize)?;
            if rest < 12 || !is_sig(look) {
                self.cur.skip_to(lm_end)?;
                return Ok(());
            }
            let key: [u8; 4] = {
                let b = self.cur.take(8, lm_end)?;
                [b[4], b[5], b[6], b[7]]
            };
            let len = self.cur.len_field(uses_long_length(header.version, &key), lm_end)?;
            self.cur.bound(len, lm_end)?;
            let data_end = self.cur.pos + len;
            let is_candidate = match target {
                Some(k) => &key == k,
                None => matches!(&key, b"Layr" | b"Lr16" | b"Lr32"),
            };
            if !candidate_seen && is_candidate {
                candidate_seen = true;
                let had_info = *source != LayerSource::None;
                self.sink.layers_reset();
                match self.layer_info(data_end, header.version) {
                    Ok((ma, ls)) => {
                        *merged_alpha = ma;
                        *layers = ls;
                        *source = LayerSource::GlobalBlock { index: kept, key };
                    }
                    Err(WalkError::Format(e)) => {
                        if target.is_some() {
                            *merged_alpha = false;
                            layers.clear();
                            *source = LayerSource::None;
                        }
                        if target.is_some() || !had_info {
                            *unreadable = Some(e);
                        }
                        self.sink.layers_reset();
                    }
                    Err(other) => return Err(other),
                }
            } else {
                self.sink.global_block_begin(key, len);
                let sink = &mut *self.sink;
                self.cur.skip_with(len, data_end, |piece| sink.global_block_bytes(piece))?;
                self.sink.global_block_end();
            }
            self.cur.skip_to(data_end)?;
            kept += 1;
            let pad = self.block_padding(lm_end)?;
            self.cur.skip(pad, lm_end)?;
        }
    }

    /// The padding after a block's data, as far as it matters here: the gap (at most three bytes)
    /// to the next block's signature. Without a signature that near, `read_blocks_with` picks a
    /// padding for its round trip, but the rest of the section is trailing bytes either way.
    fn block_padding(&mut self, lm_end: u64) -> Result<u64, WalkError> {
        let rest = self.cur.remaining(lm_end);
        let look = self.cur.peek(rest.min(7) as usize)?;
        for k in 0..=3u64 {
            if k < rest && is_sig(&look[k as usize..]) {
                return Ok(k);
            }
        }
        Ok(0)
    }

    /// A layer info body ending at `end`, read in place: records, then each channel's bytes.
    fn layer_info(&mut self, end: u64, version: Version) -> Result<(bool, Vec<WalkedLayer>), WalkError> {
        let psb = version.is_psb();
        let count = self.cur.i16(end)?;
        let merged_alpha = count < 0;
        let n = count.unsigned_abs();
        // Minimum record size: 16 rect + 2 + 4 sig + 4 key + 4 + 4 extra len.
        self.cur.bound(u64::from(n) * 34, end)?;
        if u32::from(n) > self.limits.max_layers {
            return Err(WalkError::Limit("too many layers"));
        }
        let mut pending: Vec<(LayerRecord, Vec<u64>)> = Vec::with_capacity(usize::from(n));
        for layer in 0..usize::from(n) {
            let mut raw = Vec::new();
            self.cur.append(&mut raw, 18, end)?;
            let nch = u16::from_be_bytes([raw[16], raw[17]]);
            if nch > MAX_LAYER_CHANNELS {
                return Err(PsdError::LimitExceeded("too many channels in layer").into());
            }
            let entry = if psb { 10u64 } else { 6u64 };
            self.cur.append(&mut raw, u64::from(nch) * entry + 16, end)?;
            let at = raw.len() - 4;
            let extra_len = u64::from(u32::from_be_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]));
            // A length past the section is the file's own error, as in the slice parser; only a
            // record that is really there can be over a limit.
            self.cur.bound(extra_len, end)?;
            let record_len = raw.len() as u64 + extra_len;
            if record_len > self.limits.max_record_bytes {
                return Err(WalkError::Limit("a layer record is too large"));
            }
            self.held = self.held.saturating_add(record_len);
            if self.held > self.limits.max_total_record_bytes {
                return Err(WalkError::Limit("the layer records are too large"));
            }
            self.cur.append(&mut raw, extra_len, end)?;
            let (record, lens) = LayerRecord::read(&mut Reader::new(&raw), version)?;
            self.sink.record(layer, &raw);
            pending.push((record, lens));
        }
        let mut layers = Vec::with_capacity(pending.len());
        for (layer, (record, lens)) in pending.into_iter().enumerate() {
            let mut channels = Vec::with_capacity(lens.len());
            for (channel, (ch, len)) in record.channels.iter().zip(lens).enumerate() {
                let id = ch.id;
                if len == 0 {
                    channels.push(WalkedChannel { id, compression: None, stored_len: 0, data_offset: self.cur.pos });
                    continue;
                }
                if len == 1 {
                    return Err(PsdError::invalid("channel data length 1").into());
                }
                let compression = Compression::from_u16(self.cur.u16(end)?);
                let data_len = len - 2;
                self.cur.bound(data_len, end)?;
                let data_offset = self.cur.pos;
                self.sink.channel_begin(layer, channel, id, compression, data_len);
                let sink = &mut *self.sink;
                self.cur.skip_with(data_len, end, |piece| sink.channel_bytes(piece))?;
                self.sink.channel_end();
                channels.push(WalkedChannel { id, compression: Some(compression), stored_len: len, data_offset });
            }
            layers.push(WalkedLayer { record, channels });
        }
        self.cur.skip_to(end)?;
        Ok((merged_alpha, layers))
    }
}
