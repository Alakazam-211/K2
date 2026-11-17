//! zstd frames built here block by block (RFC 8878), for the rows on what a crafted stream can
//! make the decoder do: raw and RLE blocks, and a compressed block whose sequences ask for far
//! more output than one block may hold. Every stream is made by this code when a row runs.

use k2_mpc_blend::{read_blend, Compression, ReadLimits};

/// The longest match one sequence can give: match length code 52, its 16 extra bits all set.
pub const MATCH_MAX: u64 = 65_539 + 0xFFFF;

/// The most bytes one block may hold after decoding (Block_Maximum_Size, 128 KiB).
pub const BLOCK_MAX: u32 = 128 << 10;

/// One block of a frame.
pub enum Block<'a> {
    /// Stored as is (at most [`BLOCK_MAX`] bytes).
    Raw(&'a [u8]),
    /// `byte`, `n` times.
    Rle(u8, u32),
    /// A compressed block of `sequences` sequences, each one literal (`byte`) then a match of
    /// [`MATCH_MAX`] bytes at offset 1: about 128 KiB of output per sequence, from two bytes of
    /// input, all of it in this one block.
    Repeats { byte: u8, sequences: u32 },
}

fn block_header(out: &mut Vec<u8>, last: bool, kind: u32, size: u32) {
    let v = u32::from(last) | (kind << 1) | (size << 3);
    out.extend_from_slice(&v.to_le_bytes()[..3]);
}

/// A frame of `blocks` with a window of 2^`window_log` bytes plus `mantissa` eighths of that
/// (`window_log` 10 to 41, `mantissa` 0 to 7), no checksum, no content size, no dictionary.
pub fn frame_with_window(window_log: u8, mantissa: u8, blocks: &[Block<'_>]) -> Vec<u8> {
    assert!((10..=41).contains(&window_log) && mantissa < 8);
    let mut out = vec![
        0x28,
        0xB5,
        0x2F,
        0xFD,
        0x00,
        ((window_log - 10) << 3) | mantissa,
    ];
    for (i, b) in blocks.iter().enumerate() {
        let last = i + 1 == blocks.len();
        match b {
            Block::Raw(bytes) => {
                let n = u32::try_from(bytes.len()).expect("a raw block's length");
                assert!(n <= BLOCK_MAX, "a raw block holds at most 128 KiB");
                block_header(&mut out, last, 0, n);
                out.extend_from_slice(bytes);
            }
            Block::Rle(byte, n) => {
                block_header(&mut out, last, 1, *n);
                out.push(*byte);
            }
            Block::Repeats { byte, sequences } => {
                let n = *sequences;
                assert!(
                    (128..0x7F00).contains(&n),
                    "the sequence count's two-byte form"
                );
                let mut c = vec![
                    // literals: one RLE byte, `n` of it (size format 3: a 20-bit size)
                    0b01 | (3 << 2) | (((n & 0xF) as u8) << 4),
                    ((n >> 4) & 0xFF) as u8,
                    ((n >> 12) & 0xFF) as u8,
                    *byte,
                    // sequences: the count, then every code table in RLE mode: literal length
                    // 1, offset code 0 (offset value 1: the last offset, 1 at a frame's start),
                    // match length 52
                    (0x80 + (n >> 8)) as u8,
                    (n & 0xFF) as u8,
                    0b0101_0100,
                    1,
                    0,
                    52,
                ];
                // the bit stream, read from its end: a start bit, then 16 extra bits of match
                // length per sequence, all set
                c.extend(std::iter::repeat_n(0xFF, 2 * n as usize));
                c.push(0x01);
                block_header(&mut out, last, 2, u32::try_from(c.len()).expect("small"));
                out.extend_from_slice(&c);
            }
        }
    }
    out
}

/// [`frame_with_window`] with a window of exactly 2^`window_log` bytes.
pub fn frame(window_log: u8, blocks: &[Block<'_>]) -> Vec<u8> {
    frame_with_window(window_log, 0, blocks)
}

/// The decompressed file's first bytes: a classic header (8-byte pointers, little-endian) and
/// one block header whose body is 2 GiB less 64 KiB long, so a reader reads on through whatever
/// follows until a limit stops it.
pub fn long_block_head() -> Vec<u8> {
    let mut h = b"BLENDER-v300".to_vec();
    h.extend_from_slice(b"TEST");
    h.extend_from_slice(&0x7FFF_0000i32.to_le_bytes());
    h.extend_from_slice(&0x1000u64.to_le_bytes());
    h.extend_from_slice(&0i32.to_le_bytes());
    h.extend_from_slice(&1i32.to_le_bytes());
    h
}

/// `raw` as one zstd frame of raw blocks with the window given.
pub fn stored(raw: &[u8], window_log: u8, mantissa: u8) -> Vec<u8> {
    let blocks: Vec<Block<'_>> = raw.chunks(BLOCK_MAX as usize).map(Block::Raw).collect();
    frame_with_window(window_log, mantissa, &blocks)
}

/// A frame whose window (144 MiB) is over the decoder's own default bound (128 MiB) and within
/// the reader's limit given (160 MiB) reads: the reader's limit is the one that acts, in both
/// directions (over it, the frame is refused by it). Each step's failure names the step.
pub fn row_window_over_decoder_default() -> Result<(), String> {
    let raw = super::studio(&super::Studio::default());
    let bytes = stored(&raw, 27, 1);
    let d = ReadLimits::default();
    let at = |window: u64| {
        ReadLimits::new(
            d.max_raw_bytes,
            d.max_blocks,
            d.max_retained_bytes,
            d.retain_block_max,
            d.max_sdna_bytes,
            window,
        )
    };
    let file = read_blend(&mut &bytes[..], &at(160 << 20))
        .map_err(|e| format!("window, step reads under a 160 MiB limit: {e}"))?;
    if file.compression != Compression::Zstd || file.raw_len != raw.len() as u64 {
        return Err(format!(
            "window, step reads whole: {:?}, {} of {} bytes",
            file.compression,
            file.raw_len,
            raw.len()
        ));
    }
    match read_blend(&mut &bytes[..], &at(128 << 20)) {
        Err(k2_mpc_blend::ReadError::Limit { what, limit }) if limit == 128 << 20 => {
            if what != "a zstd frame's window" {
                return Err(format!("window, step refused over the limit: by {what:?}"));
            }
        }
        other => {
            return Err(format!(
                "window, step refused over the limit: {:?}",
                other.map(|f| f.raw_len)
            ))
        }
    }
    Ok(())
}

/// The streaming index, through the pack ([`BlobPack::index`], what a host's index job calls),
/// with the decoded-bytes cap set to 1 GiB: the 2 GiB multi-block stream of
/// [`many_block_bomb`] is refused, typed (`Unreadable`, the reader's raw cap), by the cap. The
/// heap is the caller's to measure. Each step's failure names the step.
pub fn row_index_cap() -> Result<(), String> {
    use k2_mpc_base::index::IndexRefused;
    use k2_mpc_base::session::BlobPack;
    let bytes = many_block_bomb();
    let mut pack = k2_mpc_blend::BlendPack::default();
    let d = ReadLimits::default();
    pack.index_limits = ReadLimits::new(
        1 << 30,
        d.max_blocks,
        d.max_retained_bytes,
        d.retain_block_max,
        d.max_sdna_bytes,
        d.max_window_bytes,
    );
    match pack.index(&mut &bytes[..], bytes.len() as u64, &super::limits()) {
        Err(IndexRefused::Unreadable { why }) if why == "bytes after decompression" => Ok(()),
        Err(e) => Err(format!("index cap, step refused by the 1 GiB cap: {e}")),
        Ok(d) => Err(format!(
            "index cap, step refused by the 1 GiB cap: indexed {} nodes",
            d.nodes.len()
        )),
    }
}

/// 16,384 RLE blocks of 128 KiB behind [`long_block_head`]: 2 GiB after decoding, from a stream
/// of 64 KiB.
pub fn many_block_bomb() -> Vec<u8> {
    let head = long_block_head();
    let mut blocks = vec![Block::Raw(&head)];
    blocks.extend((0..16_384).map(|_| Block::Rle(0x55, BLOCK_MAX)));
    frame(17, &blocks)
}

/// One block header of the classic form (8-byte pointers, little-endian) whose body is
/// [`LONG_BODY`] bytes: the code `TEST`, the length, an address, schema index 0, count 1.
fn long_block_header() -> Vec<u8> {
    let mut h = b"TEST".to_vec();
    h.extend_from_slice(&(LONG_BODY as i32).to_le_bytes());
    h.extend_from_slice(&0x1000u64.to_le_bytes());
    h.extend_from_slice(&0i32.to_le_bytes());
    h.extend_from_slice(&1i32.to_le_bytes());
    h
}

/// The body length of each block of [`past_default_bomb`]: 2 GiB less 64 KiB.
pub const LONG_BODY: u64 = 0x7FFF_0000;

/// A stream that decodes to just past 4 GiB: a classic header and three long blocks
/// ([`long_block_header`]), the first two with their whole [`LONG_BODY`], the third with 256 KiB
/// of its body, every body run-length encoded; about 128 KiB in. Its decoded length is returned
/// beside it.
pub fn past_default_bomb() -> (Vec<u8>, u64) {
    let mut head = b"BLENDER-v300".to_vec();
    head.extend_from_slice(&long_block_header());
    let next = long_block_header();
    let full = (LONG_BODY / u64::from(BLOCK_MAX)) as usize;
    let rest = (LONG_BODY % u64::from(BLOCK_MAX)) as u32;
    let body = |blocks: &mut Vec<Block<'_>>| {
        blocks.extend((0..full).map(|_| Block::Rle(0x55, BLOCK_MAX)));
        if rest > 0 {
            blocks.push(Block::Rle(0x55, rest));
        }
    };
    let mut blocks = vec![Block::Raw(&head)];
    body(&mut blocks);
    blocks.push(Block::Raw(&next));
    body(&mut blocks);
    blocks.push(Block::Raw(&next));
    blocks.extend((0..2).map(|_| Block::Rle(0x55, BLOCK_MAX)));
    let total =
        head.len() as u64 + 2 * next.len() as u64 + 2 * LONG_BODY + 2 * u64::from(BLOCK_MAX);
    (frame(17, &blocks), total)
}

/// The reader's own default limits refuse a stream that decodes to just past 4 GiB, typed, by
/// the raw cap ([`ReadLimits::max_raw_bytes`], 4 GiB). The heap is the caller's to measure. Each
/// step's failure names the step.
pub fn row_read_default_cap() -> Result<(), String> {
    let (bytes, total) = past_default_bomb();
    if total <= 4 << 30 {
        return Err(format!(
            "read default, step the stream decodes past 4 GiB: {total}"
        ));
    }
    match read_blend(&mut &bytes[..], &ReadLimits::default()) {
        Err(k2_mpc_blend::ReadError::Limit { what, limit })
            if what == "bytes after decompression" && limit == 4 << 30 =>
        {
            Ok(())
        }
        other => Err(format!(
            "read default, step refused by the 4 GiB cap: {:?}",
            other.map(|f| f.raw_len)
        )),
    }
}
