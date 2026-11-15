//! A digest of the whole pipeline's output with no capture stage: playback,
//! the encoded and re-decoded capture, and the echo canceller's statistics,
//! for three processing configs. The numbers depend on the platform (SIMD
//! paths), so this reports rather than asserts; the same test on the same
//! machine before and after a change says whether the no-stage path is
//! bit for bit what it was.

use super::*;
use crate::testkit::{add_into, delay, floor_noise, scale, talker_track, TALKERS};

fn fnv(h: &mut u64, bytes: &[u8]) {
    for b in bytes {
        *h ^= *b as u64;
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

#[test]
fn report_no_stage_digest() {
    let n = 48_000 * 10;
    let mut rng = Rng::new(4242);
    let far = talker_track(&TALKERS[0], &mut rng, n);
    let mut near = talker_track(&TALKERS[2], &mut rng, n);
    scale(&mut near, 0.5);
    let floor = floor_noise(&mut rng, n, -72.0);
    let packets = encode_remote(&far, EncoderCfg::default());
    let sched = schedule(
        &packets,
        Net {
            base_ms: 30.0,
            ..Net::default()
        },
    );
    for (name, p) in [
        ("default", ProcessingConfig::default()),
        ("aec", aec_only()),
        ("off", ProcessingConfig::off()),
    ] {
        let cfg = core_cfg(p);
        let played = run_core(&cfg, &sched, n, None);
        let mut echo = delay(&played.rendered, 60 * 48);
        scale(&mut echo, 0.3);
        let mut mic = floor.clone();
        add_into(&mut mic, &echo);
        add_into(&mut mic, &near);
        let run = run_core(&cfg, &sched, n, Some(&mic));
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for v in run.rendered.iter().chain(run.out.iter()) {
            fnv(&mut h, &v.to_bits().to_le_bytes());
        }
        fnv(&mut h, &run.stats.aec_delay_ms.unwrap_or(-1).to_le_bytes());
        fnv(
            &mut h,
            &run.stats
                .aec_erle_db
                .unwrap_or(-1.0)
                .to_bits()
                .to_le_bytes(),
        );
        eprintln!("[digest] {name} {h:016x}");
    }
}
