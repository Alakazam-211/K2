//! `k5-cli msg <ws> --inbox-silent|--inbox-wake …` (tray packages) against
//! the shell CLI's, on the parity rig ([`common::parity`]).
//!
//! - Files on this machine: one (`path=`) or several (`paths=`), the
//!   brief from the leftover words, the wake note.
//! - The daemon can't read the path: the upload (`/cli/fs/upload-binary`,
//!   base64), the deliver from the staged path, the delete; a refused
//!   upload's teaching, exit 3. Several files: one tar.gz, compared by
//!   what is in it (names, types, modes, bytes), the upload's archive
//!   bytes masked.
//! - No files: compose, silent and wake (the inbox pointer's live send,
//!   the merged line), gated, to another server.
//! - The words: wire form, an unknown flag, no title, a missing file, a
//!   value flag as the last word (D6).
//! - Help never acts.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;
use std::io::Read;

const VERB: &str = "msg";
const KEY: &str = "parity-key-0001";
const ON: &str = r#"{"version":"0","features":["send-once"]}"#;
const LANDED: &str = r#"{"ok":true,"id":"i1","filename":"note.md"}"#;
const NOT_READABLE: &str =
    r#"{"error":{"code":"bad_request","hint":"source path is not a readable file"}}"#;

fn files(rig: &Rig) {
    owner_only(rig);
    std::fs::write(rig.proj.join("note.md"), "a note\n").unwrap();
    std::fs::write(rig.proj.join("a.txt"), "alpha\n").unwrap();
    std::fs::create_dir_all(rig.proj.join("sub")).unwrap();
    std::fs::write(rig.proj.join("sub/a.txt"), "second alpha\n").unwrap();
    let bin: Vec<u8> = (0..=255u8).cycle().take(70_000).collect();
    std::fs::write(rig.proj.join("b.bin"), bin).unwrap();
    let link = rig.proj.join("ln.txt");
    if std::fs::symlink_metadata(&link).is_err() {
        std::os::unix::fs::symlink("a.txt", &link).unwrap();
    }
}

fn plain(
    name: &'static str,
    args: &'static [&'static str],
    replies: Vec<Reply>,
    code: i32,
    requests: usize,
) -> Case {
    Case {
        env: vec![("K2_PORT", PORT.to_string())],
        ..case(name, args, &[], replies, files, code, requests)
    }
}

#[test]
fn tray_packages_as_the_shell_sends_them() {
    let rig = Rig::new(VERB, "tray");
    let v = vec![
        // covers: msg
        plain(
            "one file, silent",
            &["ws", "--inbox-silent", "note.md", "--idempotency-key", KEY],
            vec![ok(ON), ok(LANDED)],
            0,
            2,
        ),
        // covers: msg
        plain(
            "two files, wake, a brief and a failed wake",
            &[
                "ws",
                "--inbox-wake",
                "a.txt",
                "b.bin",
                "read",
                "these",
                "--body",
                "B",
                "--from",
                "me",
                "--summary",
                "S",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                ok(
                    r#"{"ok":true,"id":"i2","wake":{"success":false,"reason":"asleep","hint":"later"},"addressNote":"ws is now ws2"}"#,
                ),
            ],
            0,
            2,
        ),
        plain(
            "--path and a gate",
            &[
                "ws",
                "--inbox-silent",
                "--path",
                "./a.txt",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"gated","hint":"turn it on"}}"#.into(),
                ),
            ],
            3,
            2,
        ),
        plain(
            "an answer that is not JSON",
            &["ws", "--inbox-silent", "note.md", "--idempotency-key", KEY],
            vec![ok(ON), ok("plain words")],
            0,
            2,
        ),
        plain(
            "a refusal's closing line",
            &["ws", "--inbox-silent", "note.md", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Answer(403, PERMISSION_ASK.into())],
            1,
            2,
        ),
        plain(
            "not readable there: upload, deliver, delete",
            &["ws", "--inbox-wake", "note.md", "--idempotency-key", KEY],
            vec![
                ok(ON),
                Reply::Answer(400, NOT_READABLE.into()),
                ok(r#"{"path":"/srv/ws"}"#),
                ok(r#"{"path":"/srv/ws/.k2/tmp/msg-inbox/note.md"}"#),
                ok(LANDED),
                ok("{}"),
            ],
            0,
            6,
        ),
        plain(
            "a binary file uploaded",
            &["ws", "--inbox-silent", "b.bin", "--idempotency-key", KEY],
            vec![
                ok(ON),
                Reply::Answer(400, NOT_READABLE.into()),
                ok(r#"{"path":"/srv/ws"}"#),
                ok(r#"{"path":"/srv/ws/.k2/tmp/msg-inbox/b.bin"}"#),
                ok(LANDED),
                ok("{}"),
            ],
            0,
            6,
        ),
        plain(
            "the upload refused: the teaching, exit 3",
            &["ws", "--inbox-silent", "note.md", "--idempotency-key", KEY],
            vec![
                ok(ON),
                Reply::Answer(400, NOT_READABLE.into()),
                ok(r#"{"path":"/srv/ws"}"#),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"forbidden","hint":"owner only"}}"#.into(),
                ),
            ],
            3,
            4,
        ),
        plain(
            "the upload with no path back",
            &["ws", "--inbox-silent", "note.md", "--idempotency-key", KEY],
            vec![
                ok(ON),
                Reply::Answer(400, NOT_READABLE.into()),
                ok(r#"{"path":"/srv/ws"}"#),
                ok(r#"{"saved":true}"#),
            ],
            1,
            4,
        ),
        plain(
            "no folder for the workspace",
            &["ws", "--inbox-silent", "note.md", "--idempotency-key", KEY],
            vec![ok(ON), Reply::Answer(400, NOT_READABLE.into()), ok("{}")],
            1,
            3,
        ),
        plain(
            "compose, silent",
            &[
                "ws",
                "--inbox-silent",
                "--title",
                "T",
                "--body",
                "B",
                "--priority",
                "high",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON), ok(r#"{"ok":true,"id":"c1","title":"T"}"#)],
            0,
            2,
        ),
        plain(
            "compose, wake: the pointer and the merged line",
            &[
                "ws",
                "--inbox-wake",
                "--title",
                "T",
                "--from",
                "me",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                ok(r#"{"ok":true,"id":"c1","title":"Tee"}"#),
                ok(
                    r#"{"success":true,"target_session_id":"s1","woke":true,"wake_ms":12,"attempts":1}"#,
                ),
            ],
            0,
            3,
        ),
        plain(
            "compose, wake, the wake not delivered",
            &[
                "ws",
                "--inbox-wake",
                "--title",
                "T",
                "--idempotency-key",
                KEY,
            ],
            vec![
                ok(ON),
                ok(r#"{"ok":true,"id":"c1","wake":1}"#),
                ok(r#"{"success":false,"reason":"no_agent","hint":"nobody home"}"#),
            ],
            0,
            3,
        ),
        plain(
            "compose, wake, gated",
            &[
                "ws",
                "--inbox-wake",
                "--title",
                "T",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON), ok(r#"{"error":{"code":"gated","hint":"é"}}"#)],
            3,
            2,
        ),
        plain(
            "compose to another server",
            &[
                "ada::box.example",
                "--inbox-wake",
                "--title",
                "T",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON), ok(r#"{"ok":true,"id":"c9"}"#)],
            0,
            2,
        ),
        plain(
            "no title and no file",
            &["ws", "--inbox-wake", "just words", "--idempotency-key", KEY],
            vec![ok(ON)],
            2,
            1,
        ),
        plain(
            "a file that isn't there",
            &[
                "ws",
                "--inbox-silent",
                "missing.md",
                "--idempotency-key",
                KEY,
            ],
            vec![ok(ON)],
            2,
            1,
        ),
        plain(
            "federation's wire form",
            &["fp::ws::ada", "--inbox-wake", "note.md"],
            vec![],
            2,
            0,
        ),
        plain(
            "an unknown flag",
            &["ws", "--inbox-wake", "note.md", "--zap"],
            vec![],
            2,
            0,
        ),
        plain(
            "a bad key",
            &[
                "ws",
                "--inbox-wake",
                "note.md",
                "--idempotency-key",
                "short",
            ],
            vec![],
            2,
            0,
        ),
    ];
    for c in &v {
        rig.check(c);
    }
}

/// The tray forms' loop reads `-h` and `--help` as unknown flags: the
/// shell's line, exit 2, nothing sent (no help either).
#[test]
fn tray_help_never_acts() {
    let rig = Rig::new(VERB, "trayhelp");
    for (name, args) in [
        (
            "-h after a file",
            &["ws", "--inbox-wake", "note.md", "-h"][..],
        ),
        (
            "--help with a title",
            &["ws", "--inbox-silent", "--title", "T", "--help"],
        ),
    ] {
        rig.check(&Case {
            compare: Compare::Shell,
            ..plain(name, args, vec![], 2, 0)
        });
    }
}

/// What an archive holds: each entry's name, type, mode and bytes.
fn contents(gz: &[u8]) -> Vec<(String, u8, u32, Vec<u8>)> {
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .read_to_end(&mut raw)
        .expect("a gzip archive");
    let mut out = Vec::new();
    let mut at = 0;
    let mut pax_path: Option<String> = None;
    let octal = |b: &[u8]| -> u64 {
        let s = String::from_utf8_lossy(b);
        let s = s.trim_matches(|c: char| c == '\0' || c == ' ');
        u64::from_str_radix(s, 8).unwrap_or_else(|e| panic!("octal {s:?}: {e}"))
    };
    while at + 512 <= raw.len() {
        let h = &raw[at..at + 512];
        if h.iter().all(|b| *b == 0) {
            break;
        }
        let name_end = h[..100].iter().position(|b| *b == 0).unwrap_or(100);
        let name = String::from_utf8_lossy(&h[..name_end]).into_owned();
        let size = octal(&h[124..136]) as usize;
        let kind = h[156];
        let mode = octal(&h[100..108]) as u32;
        let data = raw[at + 512..at + 512 + size].to_vec();
        at += 512 + size.div_ceil(512) * 512;
        match kind {
            b'x' => {
                let text = String::from_utf8_lossy(&data).into_owned();
                for rec in text.split('\n').filter(|r| !r.is_empty()) {
                    let (_, kv) = rec.split_once(' ').expect("a pax record");
                    if let Some(p) = kv.strip_prefix("path=") {
                        pax_path = Some(p.to_string());
                    }
                }
            }
            b'g' => {}
            _ => {
                let name = pax_path.take().unwrap_or(name);
                let link_end = h[157..257].iter().position(|b| *b == 0).unwrap_or(100);
                let body = if kind == b'2' {
                    h[157..157 + link_end].to_vec()
                } else {
                    data
                };
                out.push((name, if kind == 0 { b'0' } else { kind }, mode, body));
            }
        }
    }
    out
}

fn b64decode(s: &str) -> Vec<u8> {
    let val = |c: u8| -> u32 {
        match c {
            b'A'..=b'Z' => u32::from(c - b'A'),
            b'a'..=b'z' => u32::from(c - b'a') + 26,
            b'0'..=b'9' => u32::from(c - b'0') + 52,
            b'+' => 62,
            b'/' => 63,
            _ => panic!("not base64: {c}"),
        }
    };
    let mut out = Vec::new();
    for chunk in s.as_bytes().chunks(4) {
        let pad = chunk.iter().filter(|c| **c == b'=').count();
        let mut n = 0u32;
        for (i, c) in chunk.iter().enumerate() {
            let v = if *c == b'=' { 0 } else { val(*c) };
            n |= v << (18 - 6 * i);
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    out
}

/// A request with the upload's archive (its base64, its random file name
/// and its length) masked; the archive's contents beside it.
fn mask_archive(r: &str) -> (String, Option<Vec<(String, u8, u32, Vec<u8>)>>) {
    // A normalised request starts with its channel's line (`[tcp]`).
    if !r.contains("POST /cli/fs/upload-binary ") {
        return (r.to_string(), None);
    }
    let start = r.find(r#""base64":""#).expect("an upload's base64") + r#""base64":""#.len();
    let end = start + r[start..].find('"').expect("the base64's end");
    let archive = contents(&b64decode(&r[start..end]));
    let mut t = format!("{}<archive>{}", &r[..start], &r[end..]);
    let f = t.find(r#""filename":""#).expect("a file name") + r#""filename":""#.len();
    let fe = f + t[f..].find('"').expect("the name's end");
    t.replace_range(f..fe, "<bundle>");
    let t = t
        .lines()
        .map(|l| {
            if l.starts_with("Content-Length: ") {
                "Content-Length: <n>".to_string()
            } else {
                l.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (t, Some(archive))
}

/// Several files to a daemon that can't read them: one archive, compared
/// by what is in it.
#[test]
fn tray_bundle_by_contents() {
    let rig = Rig::new(VERB, "bundle");
    let c = plain(
        "several files staged as one archive",
        &[
            "ws",
            "--inbox-wake",
            "a.txt",
            "sub/a.txt",
            "b.bin",
            "ln.txt",
            "--idempotency-key",
            KEY,
        ],
        vec![
            ok(ON),
            Reply::Answer(400, NOT_READABLE.into()),
            ok(r#"{"path":"/srv/ws"}"#),
            ok(r#"{"path":"/srv/ws/.k2/tmp/msg-inbox/bundle.tar.gz"}"#),
            ok(LANDED),
            ok("{}"),
        ],
        0,
        6,
    );
    let name = c.name;
    rig.prepare(Side::Bash, &c);
    let b = rig.run(true, &c);
    assert_eq!(b.code, Some(0), "{name}: bash: {b:?}");
    let port = rig.stub_of(Side::Bash).port;
    let bl: Vec<String> = rig
        .read(Side::Bash, false, name)
        .iter()
        .map(|r| normalise(r, false, VERB, name).replace(&port.to_string(), "{port}"))
        .collect();
    assert_eq!(bl.len(), 6, "{name}: bash's requests: {bl:#?}");
    let (bl, barch): (Vec<String>, Vec<_>) = bl.iter().map(|r| mask_archive(r)).unzip();
    let barch: Vec<_> = barch.into_iter().flatten().collect();
    assert_eq!(barch.len(), 1, "{name}: bash uploaded one archive");
    let names: Vec<&str> = barch[0].iter().map(|e| e.0.as_str()).collect();
    assert_eq!(
        names,
        ["a.txt", "a-2.txt", "b.bin", "ln.txt"],
        "{name}: bash's archive"
    );
    for without in [false, true] {
        let side = if without { Side::NoPython3 } else { Side::Rust };
        let (r, rl) = rig.binary(&c, without);
        let port = rig.stub_of(side).port;
        let rl: Vec<String> = rl
            .iter()
            .map(|l| l.replace(&port.to_string(), "{port}"))
            .collect();
        let (rl, rarch): (Vec<String>, Vec<_>) = rl.iter().map(|r| mask_archive(r)).unzip();
        let rarch: Vec<_> = rarch.into_iter().flatten().collect();
        assert_eq!(rl, bl, "{name} ({side:?}): the requests");
        assert_eq!(rarch, barch, "{name} ({side:?}): the archive's contents");
        assert_eq!(
            (r.code, r.signal),
            (b.code, b.signal),
            "{name} ({side:?}): {r:?}"
        );
        assert_eq!(r.stdout, b.stdout, "{name} ({side:?}): stdout");
        assert_eq!(r.stderr, b.stderr, "{name} ({side:?}): stderr");
    }
}

/// A value flag as the last word of a tray form: bash's unbound-variable
/// line and exit 1; the binary's D6 line and exit 2 (`edge-usage-exit-2`).
#[test]
fn tray_value_flag_last_is_the_d6_line() {
    let rig = Rig::new(VERB, "trayd6");
    let cli = k2_client::brand::CLI_NAME;
    for (args, flag) in [
        (&["ws", "--inbox-silent", "--title"][..], "--title"),
        (&["ws", "--inbox-wake", "note.md", "--path"], "--path"),
        (&["ws", "--inbox-wake", "--body"], "--body"),
    ] {
        let c = plain("a value flag last", args, vec![], 1, 0);
        rig.prepare(Side::Bash, &c);
        let b = rig.run(true, &c);
        assert_eq!(b.code, Some(1), "{flag}: bash: {b:?}");
        assert!(
            b.stderr.ends_with(": $2: unbound variable\n"),
            "{flag}: bash: {b:?}"
        );
        assert!(
            rig.read(Side::Bash, false, flag).is_empty(),
            "{flag}: bash sent"
        );
        let (r, sent) = rig.binary(&c, false);
        assert!(sent.is_empty(), "{flag}: the binary sent {sent:?}");
        assert_eq!((r.code, r.signal), (Some(2), None), "{flag}: {r:?}");
        assert_eq!(
            r.stderr,
            format!("Error: {flag} needs a value (try '{cli} msg --help')\n"),
        );
    }
}
