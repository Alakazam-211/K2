//! The file as stored: plain, gzip (older saves) or zstd (newer saves), told apart by the first
//! bytes. Each is read forward only, as a stream: nothing is held but the decoder's own window.
//!
//! **zstd** (RFC 8878): a run of frames, each decoded with `ruzstd`, and skippable frames (which
//! the saving program uses for a seek table) skipped. Before a frame is handed to the decoder,
//! its header is read here so its window is checked against
//! [`ReadLimits::max_window_bytes`], which is the decoder's window bound too; a frame that needs
//! a dictionary is refused. The decoder holds one block at a time, and a block's output is
//! bounded by the format (128 KiB, or the window when that is smaller): a block that asks for
//! more is refused as undecodable before any of it is read, so a small stream can't make one
//! block decode past the reader's limits. A checksum, when a frame has one, is checked. Bytes
//! after the last frame that are no frame are refused.
//!
//! **gzip**: every member, through `flate2`'s pure-Rust backend.
//!
//! A failure of the source itself stays an I/O error ([`ReadError::Io`]); a stream that doesn't
//! decode is [`ReadError::Malformed`].

use std::io::{self, Read};

use flate2::read::MultiGzDecoder;
use ruzstd::decoding::{FrameDecoder, StreamingDecoder};

use crate::error::{ReadError, ReadLimits};
use crate::mutant;

/// How a file is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Compression {
    /// Stored as is.
    None,
    /// gzip (older saves).
    Gzip,
    /// zstd (newer saves).
    Zstd,
}

impl Compression {
    /// `none`, `gzip` or `zstd`.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Gzip => "gzip",
            Self::Zstd => "zstd",
        }
    }
}

const ZSTD_MAGIC: u32 = 0xFD2F_B528;
const SKIPPABLE_MASK: u32 = 0xFFFF_FFF0;
const SKIPPABLE: u32 = 0x184D_2A50;

/// The marker a decoding failure carries inside an `io::Error`, so the caller can tell it from a
/// failure of the source.
#[derive(Debug)]
pub(crate) struct Undecodable(pub(crate) &'static str);

impl std::fmt::Display for Undecodable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Undecodable {}

fn undecodable(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, Undecodable(what))
}

/// An `io::Error` from the stream as the reader's refusal.
pub(crate) fn read_error(e: io::Error) -> ReadError {
    if let Some(u) = e.get_ref().and_then(|r| r.downcast_ref::<Undecodable>()) {
        return ReadError::malformed(u.0);
    }
    if let Some(l) = e.get_ref().and_then(|r| r.downcast_ref::<OverLimit>()) {
        return ReadError::Limit {
            what: l.0,
            limit: l.1,
        };
    }
    ReadError::Io(e)
}

#[derive(Debug)]
pub(crate) struct OverLimit(pub(crate) &'static str, pub(crate) u64);

impl std::fmt::Display for OverLimit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "over a reader limit: {} (limit {})", self.0, self.1)
    }
}

impl std::error::Error for OverLimit {}

/// The source, with a few bytes put back in front of it, remembering whether the source itself
/// failed (a decoder wraps every error it sees).
pub(crate) struct Source<'a> {
    head: [u8; 32],
    at: usize,
    end: usize,
    inner: &'a mut dyn Read,
    failed: Option<io::ErrorKind>,
}

impl<'a> Source<'a> {
    fn new(inner: &'a mut dyn Read) -> Self {
        Self {
            head: [0; 32],
            at: 0,
            end: 0,
            inner,
            failed: None,
        }
    }

    /// Puts `bytes` back in front (only when nothing put back is left).
    fn unread(&mut self, bytes: &[u8]) {
        debug_assert!(self.at == self.end && bytes.len() <= self.head.len());
        self.head[..bytes.len()].copy_from_slice(bytes);
        self.at = 0;
        self.end = bytes.len();
    }

    /// Fills `buf` as far as the stream goes; the count read.
    fn read_up_to(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let mut got = 0;
        while got < buf.len() {
            match self.read(&mut buf[got..]) {
                Ok(0) => break,
                Ok(n) => got += n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(got)
    }

    /// The source's own error, if it failed; else `e` as a decoding failure.
    fn sort(&self, e: io::Error, what: &'static str) -> io::Error {
        match self.failed {
            Some(kind) => io::Error::new(kind, "the source failed"),
            None if e
                .get_ref()
                .is_some_and(|r| r.downcast_ref::<Undecodable>().is_some()) =>
            {
                e
            }
            None => undecodable(what),
        }
    }
}

impl Read for Source<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.at < self.end {
            let n = buf.len().min(self.end - self.at);
            buf[..n].copy_from_slice(&self.head[self.at..self.at + n]);
            self.at += n;
            return Ok(n);
        }
        match self.inner.read(buf) {
            Ok(n) => Ok(n),
            Err(e) => {
                if e.kind() != io::ErrorKind::Interrupted {
                    self.failed = Some(e.kind());
                }
                Err(e)
            }
        }
    }
}

/// The decompressed file, as a stream, and how it was stored.
pub(crate) struct Opened<'a> {
    pub compression: Compression,
    pub stream: Box<dyn Read + 'a>,
}

/// Looks at the first bytes of `src` and opens it for reading.
pub(crate) fn open<'a>(
    src: &'a mut dyn Read,
    limits: &ReadLimits,
) -> Result<Opened<'a>, ReadError> {
    let mut source = Source::new(src);
    let mut first = [0u8; 4];
    let got = source.read_up_to(&mut first).map_err(ReadError::Io)?;
    source.unread(&first[..got]);
    if got >= 2 && first[..2] == [0x1f, 0x8b] {
        return Ok(Opened {
            compression: Compression::Gzip,
            stream: Box::new(Gzip(MultiGzDecoder::new(source))),
        });
    }
    if got == 4 {
        let m = u32::from_le_bytes(first);
        if m == ZSTD_MAGIC || m & SKIPPABLE_MASK == SKIPPABLE {
            return Ok(Opened {
                compression: Compression::Zstd,
                stream: Box::new(Zstd {
                    state: State::Between(source),
                    max_window: limits.max_window_bytes,
                    frames: 0,
                }),
            });
        }
    }
    Ok(Opened {
        compression: Compression::None,
        stream: Box::new(source),
    })
}

struct Gzip<'a>(MultiGzDecoder<Source<'a>>);

impl Read for Gzip<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.0
            .read(buf)
            .map_err(|e| self.0.get_ref().sort(e, "the gzip stream doesn't decode"))
    }
}

enum State<'a> {
    /// Between frames: the next bytes are a frame's magic, or the end.
    Between(Source<'a>),
    Frame(Box<StreamingDecoder<Source<'a>, FrameDecoder>>),
    Done,
    /// A read failed: every later read fails too.
    Failed,
}

struct Zstd<'a> {
    state: State<'a>,
    max_window: u64,
    frames: u64,
}

/// What a frame header says about the frame's needs (RFC 8878, 3.1.1.1).
struct FrameNeeds {
    window: u64,
    dictionary: bool,
    /// The header's bytes after the magic, to hand back to the decoder.
    bytes: [u8; 14],
    len: usize,
}

fn read_frame_needs(src: &mut Source<'_>) -> io::Result<FrameNeeds> {
    let mut b = [0u8; 14];
    if src.read_up_to(&mut b[..1])? != 1 {
        return Err(undecodable("a zstd frame header is cut short"));
    }
    let fhd = b[0];
    if fhd & 0x08 != 0 {
        return Err(undecodable("a zstd frame header sets its reserved bit"));
    }
    let single = fhd & 0x20 != 0;
    let did_len = [0usize, 1, 2, 4][usize::from(fhd & 3)];
    let fcs_len = match fhd >> 6 {
        0 => usize::from(single),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    let wd_len = usize::from(!single);
    let len = 1 + wd_len + did_len + fcs_len;
    if src.read_up_to(&mut b[1..len])? != len - 1 {
        return Err(undecodable("a zstd frame header is cut short"));
    }
    let mut at = 1;
    let mut window = 0u64;
    if !single {
        let wd = b[1];
        let base = 1u64 << (10 + u32::from(wd >> 3));
        window = base + (base / 8) * u64::from(wd & 7);
        at += 1;
    }
    let dictionary = b[at..at + did_len].iter().any(|&x| x != 0);
    at += did_len;
    if single {
        let mut fcs = [0u8; 8];
        fcs[..fcs_len].copy_from_slice(&b[at..at + fcs_len]);
        window = u64::from_le_bytes(fcs) + if fcs_len == 2 { 256 } else { 0 };
    }
    Ok(FrameNeeds {
        window,
        dictionary,
        bytes: b,
        len,
    })
}

impl Read for Zstd<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        loop {
            match std::mem::replace(&mut self.state, State::Failed) {
                State::Done => {
                    self.state = State::Done;
                    return Ok(0);
                }
                State::Failed => return Err(undecodable("the zstd stream failed before")),
                State::Frame(mut dec) => match dec.read(buf) {
                    Ok(0) => {
                        let d = &dec.decoder;
                        if let (Some(want), Some(got)) =
                            (d.get_checksum_from_data(), d.get_calculated_checksum())
                        {
                            if want != got {
                                return Err(undecodable("a zstd frame's checksum is wrong"));
                            }
                        }
                        let (src, _decoder) = (*dec).into_parts();
                        self.state = State::Between(src);
                    }
                    Ok(n) => {
                        self.state = State::Frame(dec);
                        return Ok(n);
                    }
                    Err(e) => return Err(dec.get_ref().sort(e, "a zstd frame doesn't decode")),
                },
                State::Between(mut src) => {
                    let mut magic = [0u8; 4];
                    let got = src.read_up_to(&mut magic)?;
                    if got == 0 {
                        if self.frames == 0 {
                            return Err(undecodable("the zstd stream has no frame"));
                        }
                        self.state = State::Done;
                        return Ok(0);
                    }
                    if got < 4 {
                        return Err(undecodable("bytes after the last zstd frame"));
                    }
                    let m = u32::from_le_bytes(magic);
                    if m & SKIPPABLE_MASK == SKIPPABLE {
                        let mut len = [0u8; 4];
                        if src.read_up_to(&mut len)? != 4 {
                            return Err(undecodable("a skippable zstd frame is cut short"));
                        }
                        let mut left = u64::from(u32::from_le_bytes(len));
                        let mut sink = [0u8; 4096];
                        while left > 0 {
                            let n = left.min(sink.len() as u64) as usize;
                            if src.read_up_to(&mut sink[..n])? != n {
                                return Err(undecodable("a skippable zstd frame is cut short"));
                            }
                            left -= n as u64;
                        }
                        self.state = State::Between(src);
                        continue;
                    }
                    if m != ZSTD_MAGIC {
                        return Err(undecodable("bytes after the last zstd frame"));
                    }
                    let needs = read_frame_needs(&mut src)?;
                    if needs.dictionary {
                        return Err(undecodable("a zstd frame needs a dictionary"));
                    }
                    if needs.window > self.max_window {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            OverLimit("a zstd frame's window", self.max_window),
                        ));
                    }
                    let mut head = [0u8; 18];
                    head[..4].copy_from_slice(&magic);
                    head[4..4 + needs.len].copy_from_slice(&needs.bytes[..needs.len]);
                    src.unread(&head[..4 + needs.len]);
                    // the reader's window limit, checked above, bounds the decoder too: its own
                    // default bound would refuse a frame within the reader's limit
                    let mut decoder = FrameDecoder::new();
                    if !mutant::active("ZstdWindowAtDecoderDefault") {
                        decoder.set_max_window_size(self.max_window);
                    }
                    let dec = match StreamingDecoder::new_with_decoder(src, decoder) {
                        Ok(d) => d,
                        Err(_) => return Err(undecodable("a zstd frame header doesn't decode")),
                    };
                    self.frames += 1;
                    self.state = State::Frame(Box::new(dec));
                }
            }
        }
    }
}
