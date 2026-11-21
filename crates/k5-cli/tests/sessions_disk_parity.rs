//! `k5-cli sessions archives` and `sessions compact` against the shell
//! CLI's `cmd_sessions_list` and `cmd_sessions_compact`, on the parity rig
//! ([`common::parity`]): the same stdout, stderr and exit code, and
//! nothing sent (the folders are the source).
//!
//! - `archives`: the table and `--json`, with the times in the zone `TZ`
//!   names (and none set), the segments counted and summed, a folder
//!   with none, a name that is not ASCII, no sessions folder, one with no
//!   folders in it, a large list (above 256 KiB); the gates.
//! - `compact`: the usage, no such folder, nothing to compact, and the
//!   segments compressed by the system's gzip. Where `stat -f '%z'` does
//!   not print a size (GNU's `stat`), the shell's arithmetic on its lines
//!   is the shell's: the binary hands that line on.
//!
//! Each native row is the binary's own answer ([`answered_here`]).

#![cfg(unix)]

mod common;

use common::parity::*;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// `c` on the rig, then the binary alone once more: it answered the line
/// itself, so it never wrote the shell CLI out to hand it over.
fn answered_here(rig: &Rig, c: &Case) {
    rig.check(c);
    rig.binary(c, false);
    assert!(
        !k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
        "{}: the binary handed the line to the shell",
        c.name
    );
}

fn file(dir: &Path, name: &str, bytes: usize, secs: u64) {
    std::fs::create_dir_all(dir).unwrap();
    let p = dir.join(name);
    let body: Vec<u8> = (0..bytes).map(|i| b"session line\n"[i % 13]).collect();
    std::fs::write(&p, body).unwrap();
    let f = std::fs::File::options().write(true).open(&p).unwrap();
    f.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
        .unwrap();
}

fn sessions(r: &Rig) -> std::path::PathBuf {
    let d = r.proj.join(".k2").join("sessions");
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Three sessions, a stray file, an empty folder.
fn some(r: &Rig) {
    owner_and_port(r);
    let d = sessions(r);
    let b = d.join("sess-b");
    file(&b, "archive.ndjson", 1500, 1_700_000_000);
    file(&b, "archive.001.ndjson.gz", 300, 1_700_000_500);
    file(&b, "archive.002.ndjson", 2_400_000, 1_699_000_000);
    file(&b, "notes.txt", 99, 1_800_000_000);
    file(&d.join("sess-a"), "archive.ndjson", 10, 1_600_000_000);
    file(
        &d.join("caf\u{e9}-1"),
        "archive.ndjson",
        1024 * 1152,
        1_650_000_000,
    );
    std::fs::create_dir_all(d.join("empty")).unwrap();
    std::fs::write(d.join("stray"), "x").unwrap();
}

fn no_folders(r: &Rig) {
    owner_and_port(r);
    let d = sessions(r);
    std::fs::write(d.join("stray"), "x").unwrap();
}

fn no_sessions(r: &Rig) {
    owner_and_port(r);
    let d = r.proj.join(".k2").join("sessions");
    if d.exists() {
        std::fs::remove_dir_all(&d).unwrap();
    }
}

/// Enough sessions that the JSON is past 256 KiB.
fn many(r: &Rig) {
    owner_and_port(r);
    let d = sessions(r);
    for i in 0..2400 {
        file(
            &d.join(format!("session-{i:05}-0123456789abcdef")),
            "archive.ndjson",
            i,
            1_700_000_000 + i as u64,
        );
    }
}

#[test]
fn sessions_archives_as_the_shell_does() {
    let rig = Rig::new("sessions", "archives");
    let row = |name, args, setup, code| case(name, args, &[], vec![], setup, code, 0);
    for c in [
        // native: sessions archives
        row("the table", &["archives"], some as fn(&Rig), 0),
        // native: sessions archives --json
        row("as JSON", &["archives", "--json"], some, 0),
        row("other words skipped", &["archives", "x", "--json"], some, 0),
        row("-- ends the words", &["archives", "--", "--json"], some, 0),
        Case {
            env: vec![("TZ", "America/Denver".to_string())],
            ..row("in a zone TZ names", &["archives"], some, 0)
        },
        Case {
            env: vec![("TZ", "UTC0".to_string())],
            ..row("in UTC", &["archives"], some, 0)
        },
        row("no folders", &["archives"], no_folders, 0),
        row("no folders, JSON", &["archives", "--json"], no_folders, 0),
        row("no sessions folder", &["archives"], no_sessions, 0),
        row(
            "no sessions folder, JSON",
            &["archives", "--json"],
            no_sessions,
            0,
        ),
        row("a large list", &["archives", "--json"], many, 0),
        row("the connection gate", &["archives"], nothing, 1),
        case(
            "a session with no passport: the tool's refusal",
            &["archives"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            some,
            3,
            0,
        ),
    ] {
        answered_here(&rig, &c);
    }
}

/// A session with rotated segments, one already compressed, and the
/// active one.
fn rotated(r: &Rig) {
    owner_and_port(r);
    let s = sessions(r).join("s1");
    file(&s, "archive.001.ndjson", 40_000, 1_700_000_000);
    file(&s, "archive.002.ndjson", 7, 1_700_000_100);
    file(&s, "archive.003.ndjson.gz", 50, 1_700_000_200);
    file(&s, "archive.ndjson", 500, 1_700_000_300);
    file(&s, "archive.1.ndjson", 500, 1_700_000_300);
}

/// Nothing left to compress.
fn compacted(r: &Rig) {
    owner_and_port(r);
    let s = sessions(r).join("s1");
    file(&s, "archive.001.ndjson.gz", 50, 1_700_000_200);
    file(&s, "archive.ndjson", 500, 1_700_000_300);
}

/// Whether this system's `stat -f '%z' FILE` prints the file's size.
fn bsd_stat() -> bool {
    let out = std::process::Command::new("stat")
        .args(["-f", "%z", "/dev/null"])
        .output()
        .expect("stat runs");
    out.status.success() && String::from_utf8_lossy(&out.stdout).trim_end_matches('\n') == "0"
}

#[test]
fn sessions_compact_as_the_shell_does() {
    let rig = Rig::new("sessions", "compact");
    let row = |name, args, setup, code| case(name, args, &[], vec![], setup, code, 0);
    for c in [
        // native: sessions compact
        row("no id", &["compact"], owner_and_port as fn(&Rig), 1),
        row("an empty id", &["compact", ""], owner_and_port, 1),
        row("no such folder", &["compact", "nope"], compacted, 1),
        row("nothing to compact", &["compact", "s1"], compacted, 0),
        row("the connection gate", &["compact", "s1"], nothing, 1),
    ] {
        answered_here(&rig, &c);
    }
    let c = row("the segments", &["compact", "s1"], rotated, 0);
    if bsd_stat() {
        answered_here(&rig, &c);
    } else {
        // GNU's stat: the shell's own arithmetic (see the module's notes),
        // so the binary hands the line on, before anything is changed.
        rig.binary(&c, false);
        assert!(
            k5_cli::fallthrough::cache_dir(&rig.k2home).exists(),
            "{}: the binary answered where GNU's stat stops the shell",
            c.name
        );
    }
}
