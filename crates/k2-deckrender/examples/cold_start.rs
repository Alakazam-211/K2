//! Cold-start timing: one `k2-deckrender` process per render (spawn, font set-up, parse, draw,
//! encode), as the host runs it. `cargo run --release -p k2-deckrender --example cold_start --
//! <worker binary> <runs> <width> <deck.pptx>...` prints one JSON line per deck and a summary:
//! wall-clock median and p95, and the worker's own timings.
use std::path::Path;
use std::time::{Duration, Instant};

use k2_deckrender::host::{self, Limits, Request};
use k2_deckrender::proto::SlideRef;

fn pct(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return 0.0;
    }
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [bin, runs, width, decks @ ..] = args.as_slice() else {
        eprintln!("usage: cold_start <worker> <runs> <width> <deck.pptx>...");
        std::process::exit(2);
    };
    let (runs, width): (usize, u32) = (runs.parse().expect("runs"), width.parse().expect("width"));
    let limits = Limits { wall: Duration::from_secs(60), ..Limits::default() };
    let (mut wall, mut fonts, mut parse, mut render, mut encode) = (vec![], vec![], vec![], vec![], vec![]);
    for deck in decks {
        let bytes = std::fs::read(deck).expect("deck");
        let first = host::render(Path::new(bin), &Request::new(&bytes, SlideRef::Index(0), width), &limits);
        let count = first.as_ref().map(|r| r.meta.slide_count).unwrap_or(0);
        let mut deck_wall = vec![];
        let mut failures = 0;
        for slide in 0..count {
            for _ in 0..runs {
                let t = Instant::now();
                match host::render(Path::new(bin), &Request::new(&bytes, SlideRef::Index(slide), width), &limits) {
                    Ok(r) => {
                        let ms = t.elapsed().as_secs_f64() * 1e3;
                        deck_wall.push(ms);
                        wall.push(ms);
                        fonts.push(r.meta.timings.fonts_ms);
                        parse.push(r.meta.timings.parse_ms);
                        render.push(r.meta.timings.render_ms);
                        encode.push(r.meta.timings.encode_ms);
                    }
                    Err(_) => failures += 1,
                }
            }
        }
        let name = Path::new(deck).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        println!(
            "{{\"deck\":{name:?},\"bytes\":{},\"slides\":{count},\"renders\":{},\"failures\":{failures},\"wall_ms_median\":{:.1},\"wall_ms_p95\":{:.1}}}",
            bytes.len(),
            deck_wall.len(),
            pct(&mut deck_wall.clone(), 0.5),
            pct(&mut deck_wall, 0.95)
        );
    }
    println!(
        "{{\"summary\":true,\"width\":{width},\"renders\":{},\"wall_ms_median\":{:.1},\"wall_ms_p95\":{:.1},\"fonts_ms_median\":{:.2},\"fonts_ms_p95\":{:.2},\"parse_ms_median\":{:.1},\"parse_ms_p95\":{:.1},\"render_ms_median\":{:.1},\"render_ms_p95\":{:.1},\"encode_ms_median\":{:.1},\"encode_ms_p95\":{:.1}}}",
        wall.len(),
        pct(&mut wall.clone(), 0.5),
        pct(&mut wall, 0.95),
        pct(&mut fonts.clone(), 0.5),
        pct(&mut fonts, 0.95),
        pct(&mut parse.clone(), 0.5),
        pct(&mut parse, 0.95),
        pct(&mut render.clone(), 0.5),
        pct(&mut render, 0.95),
        pct(&mut encode.clone(), 0.5),
        pct(&mut encode, 0.95)
    );
}
