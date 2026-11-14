//! The reader on untrusted bytes, as the worker's `probe` runs it: open
//! (every box, table and fragment checked against small limits), then walk
//! every sample of every track, read the first ones' bytes, the start
//! timecode, the presentation offset, the codec strings and the AV1
//! sequence header, and convert the first H.264 samples to Annex B. A typed
//! error is fine; a panic, a fault, a hang or an allocation past libFuzzer's
//! limit is a finding.
#![no_main]

use k2_isobmff::{avcc_to_annexb, Codec, Limits, Mp4Reader, Sample};
use libfuzzer_sys::fuzz_target;
use std::io::Cursor;

fuzz_target!(|data: &[u8]| {
    let limits = Limits {
        max_boxes: 20_000,
        max_box_bytes: 1 << 22,
        max_table_bytes: 1 << 21,
        max_entries: 1 << 18,
        max_samples_per_track: 1 << 18,
        max_total_samples: 1 << 19,
        max_fragments: 4096,
        max_sample_bytes: 1 << 20,
        ..Limits::default()
    };
    let Ok(mut r) = Mp4Reader::with_limits(Cursor::new(data), limits) else { return };
    let file_len = r.file_len();
    let movie_ts = r.movie().timescale;
    let mut first: Vec<(u32, Option<k2_isobmff::AvcConfig>, Vec<Sample>)> = Vec::new();
    for t in &r.movie().tracks {
        let mut n = 0u64;
        for s in t.samples() {
            // Validated at open: every sample lies inside the file. (Decode
            // times may step back at a fragment's tfdt: not checked here.)
            assert!(s.offset + s.size as u64 <= file_len);
            n += 1;
        }
        assert_eq!(n, t.sample_count());
        let _ = t.presentation_offset(movie_ts);
        let mut avc = None;
        for e in &t.entries {
            match &e.codec {
                Codec::Avc(c) => {
                    let _ = c.codec_string();
                    let _ = c.to_bytes();
                    avc = Some(c.clone());
                }
                Codec::Av1(c) => {
                    let _ = c.codec_string();
                    let _ = c.sequence_header();
                }
                _ => {}
            }
        }
        first.push((t.id, avc, t.samples().take(16).collect()));
    }
    let mut buf = Vec::new();
    let mut annexb = Vec::new();
    for (id, avc, samples) in first {
        let _ = r.start_timecode(id);
        for s in &samples {
            if r.read_sample(s, &mut buf).is_ok() {
                if let Some(c) = &avc {
                    let _ = avcc_to_annexb(&buf, c, &mut annexb);
                }
            }
        }
    }
});
