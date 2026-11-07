//! `undo_enum dfs --checkpoint` on a resumed run: `seeds=K/N` counts the seeds taken from the
//! checkpoint as finished, so the SUMMARY and the RUNREC also say how many this run explored
//! (`explored=`) and how many it resumed (`resumed=`). A run that pre-marks some seeds in its
//! checkpoint must report exactly those as resumed and the rest as explored.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The small bound: 2 pids x 1 event + 1 undo (seed `abb`, `MAXLEN` 4), seeds at depth 2.
const BOUND: [&str; 14] = [
    "--pids",
    "2",
    "--events",
    "1",
    "--undos",
    "1",
    "--reverts",
    "0",
    "--maxlen",
    "4",
    "--init",
    "abb",
    "--seed-depth",
    "2",
];

fn run_dfs(checkpoint: &Path) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_undo_enum"))
        .arg("dfs")
        .args(BOUND)
        .args(["--sym", "--threads", "2", "--run", "dfs_resume_test"])
        .arg("--checkpoint")
        .arg(checkpoint)
        .output()
        .expect("undo_enum runs");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    assert!(out.status.success(), "undo_enum dfs failed:\n{stdout}");
    stdout
}

fn line<'a>(stdout: &'a str, prefix: &str) -> &'a str {
    stdout
        .lines()
        .find(|l| l.starts_with(prefix))
        .unwrap_or_else(|| panic!("no {prefix} line:\n{stdout}"))
}

/// `key=value` from a SUMMARY line, or `"key":value` from a RUNREC line.
fn field(l: &str, key: &str) -> String {
    let (pat, end): (String, &[char]) = if l.starts_with("RUNREC ") {
        (format!("\"{key}\":"), &[',', '}'])
    } else {
        (format!(" {key}="), &[' '])
    };
    l.split(pat.as_str())
        .nth(1)
        .and_then(|r| r.split(end).next())
        .unwrap_or_else(|| panic!("{key} not in: {l}"))
        .to_owned()
}

fn scratch(name: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("undo-enum-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

#[test]
fn a_resumed_dfs_reports_explored_and_resumed_seeds() {
    let dir = scratch("resume");
    // 1. a fresh run: every seed explored, none resumed; its checkpoint holds every seed
    let full_ck = dir.join("full.txt");
    let full = run_dfs(&full_ck);
    let s_full = line(&full, "SUMMARY: mode=dfs");
    let (k, n) = {
        let s = field(s_full, "seeds");
        let (k, n) = s.split_once('/').expect("seeds=K/N");
        (
            k.parse::<usize>().expect("K"),
            n.parse::<usize>().expect("N"),
        )
    };
    assert!(n >= 4, "too few seeds to split: {s_full}");
    assert_eq!(k, n, "a fresh run finishes every seed: {s_full}");
    assert_eq!(field(s_full, "explored"), n.to_string(), "{s_full}");
    assert_eq!(field(s_full, "resumed"), "0", "{s_full}");

    // 2. a checkpoint that pre-marks the first m seeds (their own lines from run 1)
    let text = std::fs::read_to_string(&full_ck).expect("the checkpoint");
    let mut lines = text.lines();
    let header = lines.next().expect("a header");
    let mut seed_lines: Vec<(usize, &str)> = lines
        .filter(|l| l.starts_with("seed "))
        .map(|l| (l.split_whitespace().nth(1).unwrap().parse().unwrap(), l))
        .collect();
    seed_lines.sort();
    assert_eq!(
        seed_lines.len(),
        n,
        "one line per seed in {}",
        full_ck.display()
    );
    let m = n / 3;
    let part_ck = dir.join("part.txt");
    let mut part = format!("{header}\n");
    for (_, l) in &seed_lines[..m] {
        part.push_str(l);
        part.push('\n');
    }
    std::fs::write(&part_ck, part).expect("write the checkpoint");

    // 3. the resumed run: m resumed, n - m explored, and the same totals as run 1
    let resumed = run_dfs(&part_ck);
    assert!(
        resumed.contains(&format!("resuming: {m} seeds done")),
        "{resumed}"
    );
    let s = line(&resumed, "SUMMARY: mode=dfs");
    assert_eq!(field(s, "seeds"), format!("{n}/{n}"), "{s}");
    assert_eq!(field(s, "explored"), (n - m).to_string(), "explored: {s}");
    assert_eq!(field(s, "resumed"), m.to_string(), "resumed: {s}");
    assert_eq!(field(s, "states"), field(s_full, "states"), "{s}");
    let r = line(&resumed, "RUNREC ");
    assert_eq!(field(r, "seeds"), n.to_string(), "{r}");
    assert_eq!(field(r, "seeds_all"), n.to_string(), "{r}");
    assert_eq!(field(r, "explored"), (n - m).to_string(), "explored: {r}");
    assert_eq!(field(r, "resumed"), m.to_string(), "resumed: {r}");
    let _ = std::fs::remove_dir_all(&dir);
}
