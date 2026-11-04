//! What a host's listing costs, per file: the header, the preview and the summary, beside a full
//! open. Each call runs in its own process, so the peak memory a line gives is that call's. Run
//! with a release build, on Linux for the peak.
//!
//!   header_cost make <file> [objects]           writes a board with a 1 MiB preview
//!   header_cost header|preview|summary|open <file>
//!
//! The board, from a fixed seed: stickies of 3 to 40 words, cards with three check items, and
//! connectors, in words of 2 to 10 letters from a vocabulary of 2,000, with its title last, as
//! canonical order puts it.

use k2_mpc_board::*;
use std::fs::{self, File};
use std::time::Instant;

const SAMPLES: usize = 7;
const PREVIEW_BOUND: u64 = 2 * 1024 * 1024;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

fn words(rng: &mut Rng, vocab: &[String], count: u64) -> String {
    let picks: Vec<&str> = (0..count)
        .map(|_| vocab[rng.below(vocab.len() as u64) as usize].as_str())
        .collect();
    picks.join(" ")
}

fn board_text(objects: usize) -> String {
    let mut rng = Rng(0x4EAD_E2C0_5700_0001);
    let vocab: Vec<String> = (0..2000)
        .map(|_| {
            let len = 2 + rng.below(9);
            (0..len)
                .map(|_| (b'a' + rng.below(26) as u8) as char)
                .collect()
        })
        .collect();
    let mut list = Vec::with_capacity(objects);
    for i in 0..objects {
        let id = format!("o-{i:010x}");
        let (x, y) = (rng.below(50_000), rng.below(50_000));
        let o = match rng.below(10) {
            0..=6 => {
                let n = 3 + rng.below(38);
                format!(
                    "{{\"id\":\"{id}\",\"text\":\"{}\",\"type\":\"sticky\",\"x\":{x},\"y\":{y}}}",
                    words(&mut rng, &vocab, n)
                )
            }
            7..=8 => {
                let checks: Vec<String> = (0..3)
                    .map(|c| {
                        format!(
                            "{{\"done\":false,\"id\":\"c{c}\",\"text\":\"{}\"}}",
                            words(&mut rng, &vocab, 5)
                        )
                    })
                    .collect();
                format!(
                    "{{\"checks\":[{}],\"id\":\"{id}\",\"title\":\"{}\",\"type\":\"card\",\"x\":{x},\"y\":{y}}}",
                    checks.join(","),
                    words(&mut rng, &vocab, 4)
                )
            }
            _ => {
                let (a, b) = (rng.below(i as u64 + 1), rng.below(i as u64 + 1));
                format!("{{\"from\":{{\"id\":\"o-{a:010x}\"}},\"id\":\"{id}\",\"to\":{{\"id\":\"o-{b:010x}\"}},\"type\":\"connector\"}}")
            }
        };
        list.push(o);
    }
    format!(
        "{{\"frames\":[],\"objects\":[{}],\"people\":{{}},\"settings\":{{\"title\":\"A board to list\"}}}}",
        list.join(",")
    )
}

fn make(path: &str, objects: usize) -> Result<(), Box<dyn std::error::Error>> {
    let board = Board::from_json(board_text(objects).as_bytes(), &Limits::V1)?;
    let mut parts = MemParts::new();
    let mut rng = Rng(5);
    parts.set_preview((0..1024 * 1024).map(|_| rng.next() as u8).collect());
    let mut out = Vec::new();
    write(
        &mut out,
        &WriteMeta::new(Lineage::from_bytes([7; 16])),
        &board,
        &mut parts,
        &Limits::V1,
    )?;
    fs::write(path, &out)?;
    let json = board.to_canonical_json()?.len();
    println!(
        "header_cost: made objects={objects} file_bytes={} board_json_bytes={json}",
        out.len()
    );
    Ok(())
}

/// The process's peak resident memory so far, from Linux's `/proc`; elsewhere `-`.
fn peak() -> String {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l["VmHWM:".len()..].trim().replace(' ', ""))
        })
        .unwrap_or_else(|| "-".to_string())
}

/// One call on the file, from opening it to the answer, and what it found.
fn once(call: &str, path: &str) -> Result<String, Box<dyn std::error::Error>> {
    let f = File::open(path)?;
    let l = Limits::V1;
    Ok(match call {
        "header" => {
            let h = read_header(f, &l)?;
            format!("guard={} parts={}", h.guard, h.parts.len())
        }
        "preview" => {
            let p = read_preview(f, &l, PREVIEW_BOUND)?;
            format!("preview_bytes={}", p.map_or(0, |p| p.bytes.len()))
        }
        "summary" => {
            let s = read_summary(f, &l)?;
            format!(
                "objects={} title={:?}",
                s.objects,
                s.title.unwrap_or_default()
            )
        }
        "open" => {
            let p = Package::open(f, &l)?;
            format!("objects={}", p.board().objects.len())
        }
        other => return Err(format!("no call {other}").into()),
    })
}

fn measure(call: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut ms = Vec::with_capacity(SAMPLES);
    let mut found = String::new();
    for _ in 0..SAMPLES {
        let start = Instant::now();
        found = once(call, path)?;
        ms.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    ms.sort_by(f64::total_cmp);
    println!(
        "header_cost: call={call} samples={SAMPLES} median_ms={:.2} max_ms={:.2} peak={} {found}",
        ms[SAMPLES / 2],
        ms[SAMPLES - 1],
        peak()
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let usage =
        "usage: header_cost make <file> [objects] | header_cost header|preview|summary|open <file>";
    let (call, path) = match (args.first(), args.get(1)) {
        (Some(c), Some(p)) => (c.as_str(), p.as_str()),
        _ => return Err(usage.into()),
    };
    if call == "make" {
        let objects = match args.get(2) {
            Some(n) => n.parse()?,
            None => 100_000,
        };
        return make(path, objects);
    }
    measure(call, path)
}
