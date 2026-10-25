//! Exact-value redaction (VS6, KV33; compute secret-env SE23/SE24b). A
//! backstop, not the control: sealed injection (VS3), host binding and the
//! compute seal are what keep values where they belong.
//!
//! Shared by `k2-vault` (the daemon's surfaces) and `k2-node` (a compute
//! job's output before the journal), so both redact byte-for-byte alike.
//!
//! Each value is matched raw and in the encodings it most often turns up
//! in: base64 (std and url-safe, all three byte alignments, the partial
//! characters at both edges and any padding included), lower/upper hex,
//! percent-encoding, and JSON string escaping.
//! Matches become `[k2v:NAME]`.

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};

/// Values shorter than this aren't matched in K2's own surfaces (too many
/// false hits). Fetch/proxy redaction of the injected value has no floor.
pub const SURFACE_MIN_LEN: usize = 8;

/// Lowercase hex (the same bytes as `k2_vault::crypto::hex`).
pub fn hex(b: &[u8]) -> String {
    const H: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(H[(x >> 4) as usize] as char);
        s.push(H[(x & 15) as usize] as char);
    }
    s
}

/// Overwrites `buf` with zeros in a way the optimizer can't drop (SE19).
/// Best effort: copies the allocator or the automaton made are not
/// reachable from here.
pub fn wipe(buf: &mut [u8]) {
    for b in buf.iter_mut() {
        // SAFETY: `b` is a valid, aligned, exclusive reference.
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

pub struct Redactor {
    ac: Option<AhoCorasick>,
    replacements: Vec<Vec<u8>>,
    max_len: usize,
    /// The patterns themselves, for a stream's held-back tail.
    pats: Vec<Vec<u8>>,
    /// Bytes some pattern starts with.
    first: [bool; 256],
}

const B64_STD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

/// Bit `i` of `v`, most significant bit of `v[0]` first.
fn bit(v: &[u8], i: usize) -> u8 {
    (v[i / 8] >> (7 - (i % 8))) & 1
}

/// The value of `n` bits of `v` from bit `from`.
fn bits(v: &[u8], from: usize, n: usize) -> u8 {
    (0..n).fold(0u8, |acc, k| (acc << 1) | bit(v, from + k))
}

/// Every base64 form of `v` at each of the three byte alignments, in both
/// alphabets. Beyond the characters `v` alone determines, the partial
/// characters at either edge are matched too, in every form consistent with
/// `v`'s own bits there (the other bits come from the neighbouring bytes,
/// so each edge has 4 or 16 forms), and a final partial character is also
/// matched with its `=`/`==` padding. So a redacted value leaves no
/// character that carries any of its bits (VM root test, round 2: a
/// trailing `g==` survived and leaked the last byte's top bits).
fn base64_variants(v: &[u8], out: &mut Vec<Vec<u8>>) {
    if v.len() < 3 {
        return;
    }
    let nbits = v.len() * 8;
    for shift in 0..3usize {
        // `v` sits at bits [8·shift, 8·shift + nbits) of the encoded data.
        let s0 = 8 * shift;
        let s1 = s0 + nbits;
        let first = s0.div_ceil(6); // first char made of v's bits only
        let end = s1 / 6; // one past the last such char
        for alpha in [B64_STD, B64_URL] {
            let full: Vec<u8> = (first..end).map(|c| alpha[bits(v, 6 * c - s0, 6) as usize]).collect();
            // Leading partial char: its low k bits are v's first k bits.
            let lead: Vec<Option<u8>> = if s0 % 6 == 0 {
                vec![None]
            } else {
                let k = 6 * first - s0;
                let known = bits(v, 0, k);
                (0..(1u8 << (6 - k))).map(|u| Some(alpha[((u << k) | known) as usize])).collect()
            };
            // Trailing partial char: its high k bits are v's last k bits.
            let (trail, padded): (Vec<Option<u8>>, Option<(u8, usize)>) = if s1 % 6 == 0 {
                (vec![None], None)
            } else {
                let k = s1 - 6 * end;
                let known = bits(v, nbits - k, k);
                let forms = (0..(1u8 << (6 - k))).map(|u| Some(alpha[((known << (6 - k)) | u) as usize])).collect();
                (forms, Some((alpha[(known << (6 - k)) as usize], (6 - k) / 2)))
            };
            for l in &lead {
                let mut p: Vec<u8> = l.iter().copied().collect();
                p.extend_from_slice(&full);
                for t in &trail {
                    let mut q = p.clone();
                    q.extend(t.iter().copied());
                    if q.len() >= 4 {
                        out.push(q);
                    }
                }
                if let Some((c, pads)) = padded {
                    let mut q = p.clone();
                    q.push(c);
                    q.extend(std::iter::repeat(b'=').take(pads));
                    if q.len() >= 4 {
                        out.push(q);
                    }
                }
            }
        }
    }
}

fn percent(v: &[u8], upper: bool) -> Vec<u8> {
    let mut o = Vec::with_capacity(v.len() * 3);
    for &b in v {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            o.push(b);
        } else if upper {
            o.extend_from_slice(format!("%{b:02X}").as_bytes());
        } else {
            o.extend_from_slice(format!("%{b:02x}").as_bytes());
        }
    }
    o
}

/// Every byte pattern that should be redacted for one value.
pub fn variants(v: &[u8]) -> Vec<Vec<u8>> {
    let mut out = vec![v.to_vec()];
    base64_variants(v, &mut out);
    let hex = hex(v);
    out.push(hex.as_bytes().to_vec());
    out.push(hex.to_ascii_uppercase().into_bytes());
    out.push(percent(v, true));
    out.push(percent(v, false));
    if let Ok(s) = std::str::from_utf8(v) {
        if let Ok(j) = serde_json::to_string(s) {
            let inner = j[1..j.len() - 1].as_bytes().to_vec();
            out.push(inner.clone());
            out.push(String::from_utf8_lossy(&inner).replace('/', "\\/").into_bytes());
        }
    }
    out.sort();
    out.dedup();
    out
}

impl Redactor {
    /// `entries`: (name, value).
    pub fn new<'a>(entries: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> Redactor {
        let mut pats: Vec<Vec<u8>> = Vec::new();
        let mut reps: Vec<Vec<u8>> = Vec::new();
        for (name, value) in entries {
            if value.is_empty() {
                continue;
            }
            let rep = format!("[k2v:{name}]").into_bytes();
            for p in variants(value) {
                pats.push(p);
                reps.push(rep.clone());
            }
        }
        let max_len = pats.iter().map(Vec::len).max().unwrap_or(0);
        let ac = if pats.is_empty() {
            None
        } else {
            Some(
                AhoCorasickBuilder::new()
                    .match_kind(MatchKind::LeftmostLongest)
                    .build(&pats)
                    .expect("redactor patterns build"),
            )
        };
        let mut first = [false; 256];
        for p in &pats {
            first[p[0] as usize] = true;
        }
        Redactor { ac, replacements: reps, max_len, pats, first }
    }

    pub fn empty() -> Redactor {
        Redactor { ac: None, replacements: vec![], max_len: 0, pats: vec![], first: [false; 256] }
    }

    /// Length of the longest tail of `buf` that is the start of some
    /// pattern (but not a whole one): only that much has to wait for the
    /// next chunk. Text that can't begin a match goes out at once, so a
    /// stream (SSE) isn't held back by a whole pattern length.
    fn tail_hold(&self, buf: &[u8]) -> usize {
        let max = self.max_len.saturating_sub(1).min(buf.len());
        for k in (1..=max).rev() {
            let tail = &buf[buf.len() - k..];
            if !self.first[tail[0] as usize] {
                continue;
            }
            if self.pats.iter().any(|p| p.len() > k && p.starts_with(tail)) {
                return k;
            }
        }
        0
    }

    pub fn is_empty(&self) -> bool {
        self.ac.is_none()
    }

    pub fn redact(&self, input: &[u8]) -> Vec<u8> {
        match &self.ac {
            None => input.to_vec(),
            Some(ac) => {
                let mut out = Vec::with_capacity(input.len());
                let mut pos = 0;
                for m in ac.find_iter(input) {
                    out.extend_from_slice(&input[pos..m.start()]);
                    out.extend_from_slice(&self.replacements[m.pattern().as_usize()]);
                    pos = m.end();
                }
                out.extend_from_slice(&input[pos..]);
                out
            }
        }
    }

    pub fn redact_str(&self, input: &str) -> String {
        String::from_utf8_lossy(&self.redact(input.as_bytes())).into_owned()
    }

    /// True when `input` contains any pattern.
    pub fn hits(&self, input: &[u8]) -> bool {
        self.ac.as_ref().is_some_and(|ac| ac.is_match(input))
    }

    pub fn stream(&self) -> StreamRedactor<'_> {
        StreamRedactor { r: self, carry: Vec::new() }
    }
}

impl Drop for Redactor {
    /// Zeros the pattern copies this redactor holds (SE19: the node keeps a
    /// redactor for a secret job until it ends). The automaton's own tables
    /// can't be reached; see `wipe`.
    fn drop(&mut self) {
        for p in &mut self.pats {
            wipe(p);
        }
    }
}

/// Redacts a byte stream split into arbitrary chunks: holds back only a
/// tail that could be the start of a pattern (at most
/// `max_pattern_len − 1` bytes), so a value split across chunks is still
/// caught and everything else flows on.
pub struct StreamRedactor<'a> {
    r: &'a Redactor,
    carry: Vec<u8>,
}

impl StreamRedactor<'_> {
    pub fn push(&mut self, chunk: &[u8]) -> Vec<u8> {
        let ac = match &self.r.ac {
            None => return chunk.to_vec(),
            Some(ac) => ac,
        };
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);
        let hold = self.r.tail_hold(&buf);
        let safe_end = buf.len().saturating_sub(hold);
        let mut out = Vec::with_capacity(buf.len());
        let mut pos = 0;
        for m in ac.find_iter(&buf) {
            if m.start() >= safe_end {
                break;
            }
            out.extend_from_slice(&buf[pos..m.start()]);
            out.extend_from_slice(&self.r.replacements[m.pattern().as_usize()]);
            pos = m.end();
        }
        let cut = pos.max(safe_end);
        out.extend_from_slice(&buf[pos..cut]);
        self.carry = buf[cut..].to_vec();
        wipe(&mut buf);
        out
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let mut rest = std::mem::take(&mut self.carry);
        let out = self.r.redact(&rest);
        wipe(&mut rest);
        out
    }
}

impl Drop for StreamRedactor<'_> {
    /// A held-back tail may be the start of a value.
    fn drop(&mut self) {
        wipe(&mut self.carry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    use base64::Engine as _;

    const KEY: &[u8] = b"k2test_fake_key_4eC39HqLyjWDarjt";

    /// Round 2 (VM root test): no base64 character that carries any bit of
    /// the value survives, at either edge, with or without padding, in both
    /// alphabets, at every alignment, for 31-, 32- and 33-byte values (all
    /// three `len % 3`). Strict: the output is exactly the characters made
    /// of the neighbours' bits only, around the marker.
    #[test]
    fn base64_edges_and_padding_leave_nothing_of_the_value() {
        for len in [31usize, 32, 33] {
            let v: Vec<u8> = (0..len).map(|i| b"k2test-secret-EDGE-q8Zr3Lx0PwMnBvC1"[i % 34]).collect();
            let r = Redactor::new([("K", v.as_slice())]);
            for prefix in [&b""[..], b"x", b"\xffy"] {
                for suffix in [&b""[..], b"tail!", b"\x00"] {
                    for (name, engine) in [("std", &STANDARD), ("url", &URL_SAFE), ("std-nopad", &STANDARD_NO_PAD), ("url-nopad", &URL_SAFE_NO_PAD)] {
                        let data = [prefix, v.as_slice(), suffix].concat();
                        let enc = engine.encode(&data);
                        let head = &enc[..(8 * prefix.len()) / 6];
                        let tail = if suffix.is_empty() { "" } else { &enc[(8 * (prefix.len() + len)).div_ceil(6)..] };
                        let want = format!("{head}[k2v:K]{tail}");
                        let got = r.redact_str(&enc);
                        assert_eq!(got, want, "len {len} prefix {prefix:?} suffix {suffix:?} {name}");
                        // A stream split anywhere gives the same.
                        let mut s = r.stream();
                        let mut out = Vec::new();
                        for b in enc.as_bytes() {
                            out.extend(s.push(std::slice::from_ref(b)));
                        }
                        out.extend(s.finish());
                        assert_eq!(String::from_utf8(out).unwrap(), want, "stream: len {len} prefix {prefix:?} suffix {suffix:?} {name}");
                    }
                }
            }
        }
    }

    fn r() -> Redactor {
        Redactor::new([("STRIPE_KEY", KEY)])
    }

    #[test]
    fn raw_hex_percent_json() {
        let r = r();
        let s = r.redact(b"key=k2test_fake_key_4eC39HqLyjWDarjt&x=1");
        assert_eq!(s, b"key=[k2v:STRIPE_KEY]&x=1");
        let hex = hex(KEY);
        assert!(!String::from_utf8_lossy(&r.redact(hex.as_bytes())).contains(&hex));
        assert!(!String::from_utf8_lossy(&r.redact(hex.to_uppercase().as_bytes())).contains(&hex.to_uppercase()));
        let weird = Redactor::new([("P", &b"a/b c+d"[..])]);
        assert_eq!(weird.redact(b"x=a%2Fb%20c%2Bd"), b"x=[k2v:P]");
        assert_eq!(weird.redact(br#"{"v":"a\/b c+d"}"#), br#"{"v":"[k2v:P]"}"#);
    }

    #[test]
    fn base64_at_every_alignment() {
        let r = r();
        for prefix_len in 0..3 {
            for engine in [&STANDARD_NO_PAD, &URL_SAFE_NO_PAD] {
                let mut buf = b"xyz"[..prefix_len].to_vec();
                buf.extend_from_slice(KEY);
                buf.extend_from_slice(b"tail");
                let enc = engine.encode(&buf);
                let out = String::from_utf8_lossy(&r.redact(enc.as_bytes())).into_owned();
                assert!(out.contains("[k2v:STRIPE_KEY]"), "alignment {prefix_len}: {out}");
                // Nothing long of the encoded key remains.
                let full = engine.encode(KEY);
                assert!(!out.contains(&full[4..full.len() - 4]), "alignment {prefix_len}");
            }
        }
    }

    #[test]
    fn stream_catches_a_value_split_across_chunks() {
        let r = r();
        let msg = [b"hello ".as_slice(), KEY, b" world"].concat();
        for split in 1..msg.len() {
            let mut s = r.stream();
            let mut out = s.push(&msg[..split]);
            out.extend(s.push(&msg[split..]));
            out.extend(s.finish());
            assert_eq!(out, b"hello [k2v:STRIPE_KEY] world", "split at {split}");
        }
        // Byte-at-a-time.
        let mut s = r.stream();
        let mut out = Vec::new();
        for b in &msg {
            out.extend(s.push(std::slice::from_ref(b)));
        }
        out.extend(s.finish());
        assert_eq!(out, b"hello [k2v:STRIPE_KEY] world");
    }

    #[test]
    fn stream_holds_back_only_a_possible_start_of_a_value() {
        let r = r();
        let mut s = r.stream();
        // Nothing here can begin the key: all of it goes out at once.
        assert_eq!(s.push(b"data: {\"text\":\"hi\"}\n\n"), b"data: {\"text\":\"hi\"}\n\n");
        // The first bytes of the key wait for the next chunk.
        assert_eq!(s.push(b"event: x\ndata: k2tes"), b"event: x\ndata: ");
        let mut rest = s.push(b"t_fake_key_4eC39HqLyjWDarjt tail!\n\n");
        rest.extend(s.finish());
        assert_eq!(rest, b"[k2v:STRIPE_KEY] tail!\n\n");
        // A tail that only looked like a start goes out once it can't be one.
        let mut s = r.stream();
        assert_eq!(s.push(b"k2t"), b"");
        assert_eq!(s.push(b"other!"), b"k2tother!");
        assert!(s.finish().is_empty());
    }

    #[test]
    fn empty_redactor_passes_through() {
        let e = Redactor::empty();
        assert_eq!(e.redact(b"abc"), b"abc");
        let mut s = e.stream();
        assert_eq!(s.push(b"abc"), b"abc");
        assert!(s.finish().is_empty());
    }
}
