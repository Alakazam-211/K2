//! What `merge3` costs, by board size and shape. A host holds its writer lock throughout, and a file
//! can be built to be slow, so the shapes include the worst one we can construct.
//!
//!   merge_cost [--shape NAME] [size ...]     (default: every shape, at 1000 10000 100000)
//!
//! With `--shape`, only that shape is built and run, so a job's peak memory is that shape's. Each
//! line ends with the process's peak resident memory so far, on Linux.
//!
//! The shapes:
//!   typical    stickies; each side changes 100 objects and moves 10.
//!   reversed   stickies; ours reverses the whole order; theirs is the base.
//!   worst      see `worst` below.
//!   rotated    `worst`, with theirs also rotated by half the board.
//!   full-size  the typical edits on a board as large as a file allows, at 2.5 KiB an object.
//!   full-worst `worst` with theirs rotated, on that board: every step at once.
//!   conflicts  that board with every object changed on both sides: the error keeps the first
//!              `MERGE_CONFLICTS_MAX` with their sides, and counts the rest.
//!
//! Run with a release build.

use k2_mpc_board::json::{Fields, Value};
use k2_mpc_board::*;
use std::time::Instant;

fn object(id: String, kind: ObjectType, text: &str) -> Object {
    let mut fields = Fields::new();
    if !text.is_empty() {
        fields.insert("text".to_string(), Value::Str(text.to_string()));
    }
    Object {
        id: Id::new(&id).expect("an id"),
        kind,
        fields,
    }
}

fn board(objects: Vec<Object>) -> Board {
    let frames = objects
        .iter()
        .filter(|o| o.kind == ObjectType::Frame)
        .map(|o| o.id.clone())
        .collect();
    Board {
        objects,
        frames,
        ..Board::empty()
    }
}

fn stickies(n: usize, text: &str) -> Board {
    board(
        (0..n)
            .map(|i| object(format!("s-{i}"), ObjectType::Sticky, text))
            .collect(),
    )
}

/// Each side changes 100 objects and moves 10, spread over the board.
fn typical(base: &Board) -> (Board, Board) {
    let n = base.objects.len();
    let (mut ours, mut theirs) = (base.clone(), base.clone());
    for k in 0..100 {
        let at = |offset: usize| (k * n / 100 + offset) % n;
        ours.objects[at(0)]
            .fields
            .insert("x".to_string(), Value::Num(1.0));
        theirs.objects[at(1)]
            .fields
            .insert("y".to_string(), Value::Num(2.0));
    }
    for k in 0..10 {
        let from = (k * n / 10 + 2) % n;
        let moved = ours.objects.remove(from);
        ours.objects.push(moved);
        let from = (k * n / 10 + 3) % n;
        let moved = theirs.objects.remove(from);
        theirs.objects.insert(0, moved);
    }
    (ours, theirs)
}

fn reversed(base: &Board) -> Board {
    let mut ours = base.clone();
    ours.objects.reverse();
    ours.frames.reverse();
    ours
}

/// The worst shape we can construct for the order, and why we believe it is.
///
/// Placing a moved object is constant work against the list, so the cost is the moved objects, the
/// id compares in the sets, and the bytes copied. Every object moved is the most there can be:
/// against a full reverse, the longest run that kept its order is one, so all but one count as moved.
/// Each list has its own merge, so every object is a frame and both lists run in full. An id compare
/// is the unit of work in the sets, so ids take the full 24 bytes and differ only at the end: a
/// compare reads them all. For the first version, which scanned for each moved object, this was also
/// the worst for the scans: with theirs as the base, the object placed next was at the far end of
/// what was left. Bytes copied are at most the board itself, so `full-worst` adds those.
fn worst(n: usize, text: &str) -> (Board, Board, Board) {
    let base = board(
        (0..n)
            .map(|i| object(format!("aaaaaaaaaaaaaaaaaa{i:06}"), ObjectType::Frame, text))
            .collect(),
    );
    let ours = reversed(&base);
    let theirs = base.clone();
    (base, ours, theirs)
}

/// The process's peak resident memory so far, from Linux's `/proc`; elsewhere `-`.
fn peak() -> String {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("VmHWM:"))
                .map(|l| l["VmHWM:".len()..].trim().replace(' ', ""))
        })
        .unwrap_or_else(|| "-".to_string())
}

fn run(shape: &str, base: &Board, ours: &Board, theirs: &Board) {
    let start = Instant::now();
    let merged = merge3(base, ours, theirs);
    let ms = start.elapsed().as_secs_f64() * 1000.0;
    match merged {
        Ok(m) => println!(
            "merge_cost: shape={shape} objects={} frames={} moved={} from_ours={} ms={ms:.1} peak={}",
            m.board.objects.len(),
            m.board.frames.len(),
            m.moved.len(),
            m.from_ours.len(),
            peak(),
        ),
        Err(MergeError::Conflicts(c)) => println!(
            "merge_cost: shape={shape} objects={} conflicts={} kept={} truncated={} ms={ms:.1} peak={}",
            base.objects.len(),
            c.totals.all(),
            c.kept(),
            c.truncated,
            peak(),
        ),
        Err(e) => panic!("{shape}: {e:?}"),
    }
}

fn main() {
    let mut only: Option<String> = None;
    let mut args: Vec<usize> = Vec::new();
    let mut given = std::env::args().skip(1);
    while let Some(a) = given.next() {
        if a == "--shape" {
            only = Some(given.next().expect("a shape after --shape"));
        } else {
            args.push(a.parse().expect("a size"));
        }
    }
    const SHAPES: [&str; 7] = [
        "typical",
        "reversed",
        "worst",
        "rotated",
        "full-size",
        "conflicts",
        "full-worst",
    ];
    if let Some(s) = &only {
        assert!(SHAPES.contains(&s.as_str()), "no shape {s}");
    }
    let wanted = |s: &str| only.as_deref().map_or(true, |o| o == s);
    let sizes = if args.is_empty() {
        vec![1_000, 10_000, 100_000]
    } else {
        args
    };
    for n in sizes {
        if wanted("typical") || wanted("reversed") {
            let base = stickies(n, "");
            if wanted("typical") {
                let (ours, theirs) = typical(&base);
                run("typical", &base, &ours, &theirs);
            }
            if wanted("reversed") {
                run("reversed", &base, &reversed(&base), &base);
            }
        }
        if wanted("worst") || wanted("rotated") {
            let (base, ours, theirs) = worst(n, "");
            if wanted("worst") {
                run("worst", &base, &ours, &theirs);
            }
            if wanted("rotated") {
                let mut rotated = theirs.clone();
                rotated.objects.rotate_left(n / 2);
                rotated.frames.rotate_left(n / 2);
                run("rotated", &base, &ours, &rotated);
            }
        }
        let text = "x".repeat(2500);
        if wanted("full-size") || wanted("conflicts") {
            let base = stickies(n, &text);
            if wanted("full-size") {
                let (ours, theirs) = typical(&base);
                run("full-size", &base, &ours, &theirs);
            }
            if wanted("conflicts") {
                let (mut ours, mut theirs) = (base.clone(), base.clone());
                for o in &mut ours.objects {
                    o.fields.insert("x".to_string(), Value::Num(1.0));
                }
                for o in &mut theirs.objects {
                    o.fields.insert("x".to_string(), Value::Num(2.0));
                }
                run("conflicts", &base, &ours, &theirs);
            }
        }
        if wanted("full-worst") {
            let (base, ours, mut theirs) = worst(n, &text);
            theirs.objects.rotate_left(n / 2);
            theirs.frames.rotate_left(n / 2);
            run("full-worst", &base, &ours, &theirs);
        }
    }
}
