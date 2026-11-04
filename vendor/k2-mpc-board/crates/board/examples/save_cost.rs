//! What a whole-file save costs, by board size and image bytes, on the disk it runs on.
//!
//! Each sample does what a guarded save does: apply one group, check the guard on the file, write a
//! temporary file, flush it to disk, and rename it over; `open` is timed too, since a host opens the
//! file to save it. Median and max of the samples are printed. Run with a release build.
//!
//! The generated board, from a fixed seed, is meant to compress as real text does: words of 2 to 10
//! letters from a vocabulary of 2,000, in this mix of objects:
//!   70% stickies of 3 to 40 words, one in 20 of them long (200 to 600 words);
//!   10% cards with a title, some text and 3 check items;
//!    5% connectors between stickies;
//!    5% pen strokes of 50 to 300 points;
//!    5% comments with 0 to 3 replies;
//!    5% tables, 8 rows by 6 columns of short cells.
//! Images are bytes that don't compress, each at most the part limit.
//!
//!   save_cost <dir> [--big]

use k2_mpc_board::json::{self, Fields};
use k2_mpc_board::*;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::time::Instant;

const MIB: u64 = 1024 * 1024;
const PERSON: &str = "p-0123456789abcdef";

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
    fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high - low + 1)
    }
}

fn vocabulary(rng: &mut Rng) -> Vec<String> {
    (0..2000)
        .map(|_| {
            let len = rng.range(2, 10);
            (0..len)
                .map(|_| (b'a' + rng.below(26) as u8) as char)
                .collect()
        })
        .collect()
}

fn words(rng: &mut Rng, vocab: &[String], count: u64) -> String {
    let mut out = String::new();
    for i in 0..count {
        if i > 0 {
            out.push(' ');
        }
        // Common words come up more often, as in real text.
        let pick =
            rng.below(vocab.len() as u64) * rng.below(vocab.len() as u64) / vocab.len() as u64;
        out.push_str(&vocab[pick as usize]);
    }
    out
}

fn object(i: usize, rng: &mut Rng, vocab: &[String]) -> String {
    let id = format!("o-{i:010x}");
    let (x, y) = (rng.below(50_000), rng.below(50_000));
    let roll = rng.below(100);
    match roll {
        0..=69 => {
            let n = if rng.below(20) == 0 {
                rng.range(200, 600)
            } else {
                rng.range(3, 40)
            };
            let text = words(rng, vocab, n);
            format!("{{\"color\":\"yellow\",\"id\":\"{id}\",\"text\":\"{text}\",\"type\":\"sticky\",\"x\":{x},\"y\":{y}.5}}")
        }
        70..=79 => {
            let (title, text) = (words(rng, vocab, 4), words(rng, vocab, 25));
            let checks: Vec<String> = (0..3)
                .map(|c| {
                    format!(
                        "{{\"done\":false,\"id\":\"c{c}\",\"text\":\"{}\"}}",
                        words(rng, vocab, 5)
                    )
                })
                .collect();
            format!("{{\"checks\":[{}],\"id\":\"{id}\",\"text\":\"{text}\",\"title\":\"{title}\",\"type\":\"card\",\"x\":{x},\"y\":{y}}}", checks.join(","))
        }
        80..=84 => {
            let (a, b) = (rng.below(i as u64 + 1), rng.below(i as u64 + 1));
            format!("{{\"from\":{{\"anchor\":\"auto\",\"id\":\"o-{a:010x}\"}},\"id\":\"{id}\",\"to\":{{\"anchor\":\"auto\",\"id\":\"o-{b:010x}\"}},\"type\":\"connector\"}}")
        }
        85..=89 => {
            let n = rng.range(50, 300);
            let mut points = String::new();
            let (mut px, mut py) = (0i64, 0i64);
            for p in 0..n {
                px += rng.below(21) as i64 - 10;
                py += rng.below(21) as i64 - 10;
                if p > 0 {
                    points.push(',');
                }
                let _ = write!(points, "[{px},{py}]");
            }
            format!(
                "{{\"id\":\"{id}\",\"points\":[{points}],\"type\":\"pen\",\"x\":{x},\"y\":{y}}}"
            )
        }
        90..=94 => {
            let text = words(rng, vocab, 20);
            let replies: Vec<String> = (0..rng.below(4))
                .map(|r| {
                    format!("{{\"at\":\"2026-11-03T10:00:00Z\",\"by\":\"{PERSON}\",\"id\":\"r{r}\",\"text\":\"{}\"}}", words(rng, vocab, 15))
                })
                .collect();
            format!(
                "{{\"id\":\"{id}\",\"replies\":[{}],\"text\":\"{text}\",\"type\":\"comment\"}}",
                replies.join(",")
            )
        }
        _ => {
            let columns: Vec<String> = (0..6).map(|c| format!("{{\"id\":\"c{c}\"}}")).collect();
            let rows: Vec<String> = (0..8)
                .map(|r| {
                    let cells: Vec<String> = (0..6)
                        .map(|c| format!("\"c{c}\":\"{}\"", words(rng, vocab, 3)))
                        .collect();
                    format!("{{\"cells\":{{{}}},\"id\":\"r{r}\"}}", cells.join(","))
                })
                .collect();
            format!(
                "{{\"columns\":[{}],\"id\":\"{id}\",\"rows\":[{}],\"type\":\"table\"}}",
                columns.join(","),
                rows.join(",")
            )
        }
    }
}

/// Bytes that don't compress, from a fast generator.
fn noise(len: u64, seed: u64) -> Vec<u8> {
    let mut rng = Rng(seed | 1);
    let mut out = Vec::with_capacity(len as usize + 8);
    while (out.len() as u64) < len {
        out.extend_from_slice(&rng.next().to_le_bytes());
    }
    out.truncate(len as usize);
    out
}

fn add_images(parts: &mut MemParts, image_mib: u64) -> Vec<Sha256> {
    let mut hashes = Vec::new();
    let mut left = image_mib * MIB;
    let mut seed = 0x1234_5678_9ABC_DEF0u64;
    while left > 0 {
        let size = left.min(Limits::V1.part_bytes);
        hashes.push(parts.add_blob("image/png", noise(size, seed)));
        seed = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
        left -= size;
    }
    hashes
}

/// Exactly `objects` objects, the images among them.
fn board_text(objects: usize, images: &[Sha256]) -> String {
    let mut rng = Rng(0x0F1E_2D3C_4B5A_6978);
    let vocab = vocabulary(&mut rng);
    let plain = objects.saturating_sub(images.len());
    let mut items: Vec<String> = (0..plain).map(|i| object(i, &mut rng, &vocab)).collect();
    for (i, h) in images.iter().enumerate() {
        items.push(format!(
            "{{\"id\":\"i-{i:010x}\",\"src\":{{\"blob\":\"{h}\"}},\"type\":\"image\"}}"
        ));
    }
    format!(
        "{{\"frames\":[],\"objects\":[{}],\"people\":{{\"{PERSON}\":{{\"name\":\"Sample\"}}}},\"settings\":{{}}}}",
        items.join(",")
    )
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn stats(mut v: Vec<f64>) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[v.len() / 2], v[v.len() - 1])
}

fn row(
    dir: &Path,
    objects: usize,
    image_mib: u64,
    samples: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    let l = Limits::V1;
    let path = dir.join("save.k5board");
    let temp = dir.join("save.k5board.tmp");
    let (raw_kib, images_bytes);
    {
        let mut parts = MemParts::new();
        let images = add_images(&mut parts, image_mib);
        let board = Board::from_json(board_text(objects, &images).as_bytes(), &l)?;
        raw_kib = board.to_canonical_json()?.len() as f64 / 1024.0;
        images_bytes = image_mib * MIB;
        let mut out = BufWriter::new(File::create(&path)?);
        write(
            &mut out,
            &WriteMeta::new(Lineage::from_bytes([1; 16])),
            &board,
            &mut parts,
            &l,
        )?;
        out.flush()?;
    }
    let file_bytes = fs::metadata(&path)?.len();
    let packed_kib = file_bytes.saturating_sub(images_bytes) as f64 / 1024.0;

    let (mut open, mut group, mut guard, mut wrote, mut sync, mut save) = (
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    for sample in 0..samples {
        let t = Instant::now();
        let mut pkg = Package::open(File::open(&path)?, &l)?;
        open.push(ms(t));
        let read_guard = pkg.package_guard();
        let mut board = pkg.board().clone();
        let target = board.objects[board.objects.len() / 2].id.clone();
        let rev = board.objects[board.objects.len() / 2].rev()?;
        let mut set = Fields::new();
        set.insert("sampled".to_string(), json::Value::Num(sample as f64));
        let edit = Group {
            id: "edit".to_string(),
            by: None,
            ops: vec![Op::Set {
                id: target,
                rev,
                set,
                unset: Vec::new(),
            }],
        };

        let t = Instant::now();
        apply_group(&mut board, &edit).map_err(|c| format!("{c:?}"))?;
        let group_ms = ms(t);

        let t = Instant::now();
        let on_disk = package_guard_of(File::open(&path)?, &l)?;
        if on_disk != read_guard {
            return Err("the guard moved".into());
        }
        let guard_ms = ms(t);

        let meta = pkg.manifest().write_meta();
        let t = Instant::now();
        let mut out = BufWriter::new(File::create(&temp)?);
        write(&mut out, &meta, &board, &mut pkg, &l)?;
        let inner = out.into_inner().map_err(|e| e.into_error())?;
        let write_ms = ms(t);

        let t = Instant::now();
        inner.sync_all()?;
        fs::rename(&temp, &path)?;
        let sync_ms = ms(t);

        group.push(group_ms);
        guard.push(guard_ms);
        wrote.push(write_ms);
        sync.push(sync_ms);
        save.push(group_ms + guard_ms + write_ms + sync_ms);
    }
    let (o, g, c, w, y, s) = (
        stats(open),
        stats(group),
        stats(guard),
        stats(wrote),
        stats(sync),
        stats(save),
    );
    println!(
        "save-cost objects={objects} image_mib={image_mib} samples={samples} file_mib={:.1} board_raw_kib={raw_kib:.0} board_packed_kib={packed_kib:.0} \
open_ms={:.1}/{:.1} group_ms={:.2}/{:.2} guard_ms={:.2}/{:.2} write_ms={:.1}/{:.1} sync_rename_ms={:.1}/{:.1} save_ms={:.1}/{:.1}",
        file_bytes as f64 / MIB as f64, o.0, o.1, g.0, g.1, c.0, c.1, w.0, w.1, y.0, y.1, s.0, s.1
    );
    fs::remove_file(&path)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().ok_or("usage: save_cost <dir> [--big]")?;
    let big = args.iter().any(|a| a == "--big");
    fs::create_dir_all(dir)?;
    let dir = Path::new(dir);
    println!("each timing is median/max of the samples; save_ms is group + guard + write + sync and rename, without open");
    for (objects, image_mib, samples) in [
        (100, 0, 41),
        (1000, 0, 41),
        (10_000, 0, 21),
        (100_000, 0, 7),
        (1000, 10, 11),
        (1000, 100, 7),
        (10_000, 100, 5),
        (1000, 500, 5),
    ] {
        row(dir, objects, image_mib, samples)?;
    }
    if big {
        row(dir, 1000, 1024, 3)?;
        row(dir, 1000, 2000, 3)?;
        row(dir, 100_000, 2000, 3)?;
    }
    Ok(())
}
