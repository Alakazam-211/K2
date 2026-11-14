//! H.264 length-prefixed to Annex B on untrusted bytes. The first byte
//! picks the NAL length size (1, 2 or 4) for a fixed configuration; the
//! rest is the sample. The same bytes are also tried as an `avcC` record
//! and, when it parses, converted with its own length size. A typed error
//! is fine; a panic is a finding.
#![no_main]

use k2_isobmff::{avcc_to_annexb, AvcConfig};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Some((&sel, sample)) = data.split_first() else { return };
    let cfg = AvcConfig {
        profile: 0x64,
        profile_compatibility: 0,
        level: 0x1f,
        length_size: [1, 2, 4][sel as usize % 3],
        sps: vec![vec![0x67, 0x64, 0x00, 0x1f, 0xac]],
        pps: vec![vec![0x68, 0xeb, 0xe3]],
        chroma_format: None,
        bit_depth_luma: None,
        bit_depth_chroma: None,
        sps_ext: Vec::new(),
    };
    let mut out = Vec::new();
    if avcc_to_annexb(sample, &cfg, &mut out).is_ok() {
        // Every NAL got a 4-byte start code; at most the parameter sets added.
        assert!(out.is_empty() || out.starts_with(&[0, 0, 0, 1]));
        assert!(out.len() <= sample.len() * 4 + 64);
    }
    if let Ok(c) = AvcConfig::parse(sample) {
        if let Ok(bytes) = c.to_bytes() {
            assert_eq!(AvcConfig::parse(&bytes).as_ref(), Ok(&c), "avcC round trip");
        }
        let _ = avcc_to_annexb(sample, &c, &mut out);
    }
});
