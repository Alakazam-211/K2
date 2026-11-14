//! Prints a file's tracks and sample tables as text, for the ffprobe
//! referee (scripts/referee-ffprobe.sh) and for looking at a file by hand:
//!   cargo run -p k2-isobmff --example dump_tables -- <file>
//! Lines: `movie …`, then per track `track <i> id=… handler=… codec=…
//! timescale=… samples=… presentation_offset=…` and one
//! `S <i> <pts> <dts> <size> <offset> <K|_>` per sample in decode order,
//! then `timecode <i> <hh:mm:ss:ff>` for tmcd tracks.

use k2_isobmff::{Codec, Limits, Mp4Reader};
use std::io::{BufReader, BufWriter, Write};

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_tables <file>");
    let f = std::fs::File::open(&path).expect("open the file");
    let limits = Limits { max_samples_per_track: 1 << 27, max_total_samples: 1 << 28, max_box_bytes: 1 << 30, max_table_bytes: 1 << 29, max_entries: 1 << 27, ..Limits::default() };
    let mut r = match Mp4Reader::with_limits(BufReader::new(f), limits) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };
    let out = std::io::stdout();
    let mut o = BufWriter::new(out.lock());
    let m = r.movie();
    writeln!(o, "movie timescale={} tracks={} fragmented={} fragments={} quicktime={}", m.timescale, m.tracks.len(), m.fragmented, m.fragment_count, m.quicktime).unwrap();
    let mut tmcd = Vec::new();
    for (i, t) in m.tracks.iter().enumerate() {
        let codec = match t.entries.first().map(|e| (&e.codec, e)) {
            Some((Codec::Avc(c), _)) => c.codec_string(),
            Some((Codec::Av1(c), _)) => c.codec_string(),
            Some((Codec::Hevc(h), _)) => format!("hevc.{}.L{}.{}bit", h.profile_idc, h.level_idc, h.bit_depth_luma),
            Some((Codec::ProRes(p), _)) => format!("prores.{p:?}"),
            Some((Codec::Opus(c), _)) => format!("opus.{}ch", c.output_channel_count),
            Some((Codec::Mp4Audio(a), _)) => format!("mp4a.{:x}.{}", a.object_type_indication, a.audio_object_type.unwrap_or(0)),
            Some((Codec::Pcm(p), _)) => format!("pcm.{}{}{}", if p.float { "f" } else if p.signed { "s" } else { "u" }, p.bits_per_sample, if p.big_endian { "be" } else { "le" }),
            Some((Codec::Timecode(_), _)) => {
                tmcd.push((i, t.id));
                "tmcd".into()
            }
            Some((_, e)) => format!("unknown.{}", e.fourcc),
            None => "none".into(),
        };
        writeln!(o, "track {i} id={} handler={} codec={} timescale={} samples={} presentation_offset={}", t.id, t.handler, codec, t.timescale, t.sample_count(), t.presentation_offset(m.timescale)).unwrap();
        for s in t.samples() {
            writeln!(o, "S {i} {} {} {} {} {}", s.pts, s.dts, s.size, s.offset, if s.keyframe { 'K' } else { '_' }).unwrap();
        }
    }
    for (i, id) in tmcd {
        match r.start_timecode(id) {
            Ok(Some(tc)) => writeln!(o, "timecode {i} {tc}").unwrap(),
            Ok(None) => writeln!(o, "timecode {i} none").unwrap(),
            Err(e) => writeln!(o, "timecode {i} error {e}").unwrap(),
        }
    }
}
