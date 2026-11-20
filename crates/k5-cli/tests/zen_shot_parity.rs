//! `k5-cli zen widget preview --shot` against the shell CLI's, on the
//! parity rig ([`common::parity`]): the same request (the folder with
//! `--data`'s fixture and its `blobs/` folder, each file's type from the
//! system's `mime.types` as Python 3.12's `mimetypes` reads it), the same
//! run of the shot runner, and the same stdout, stderr and exit code.
//!
//! The binary is copied into a folder with a stand-in runner beside it
//! (where an install keeps `k2-widget-shot.mjs`), and the shell is told
//! that folder in `K5_CLI_SCRIPT_DIR`, as the front tells it. A stand-in
//! `node` first on `PATH` logs its words (the page's temp file by its
//! name's shape and folder), the page's mode and bytes, and whether the
//! front's variables reached it; then prints `SHOT_OUT`, says `SHOT_ERR`
//! on stderr and exits `SHOT_RC`. Each side's log must be the other's.

#![cfg(unix)]

mod common;

use common::parity::*;
use std::os::unix::fs::PermissionsExt;

const VERB: &str = "zen";

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn words(v: Vec<&'static str>) -> &'static [&'static str] {
    Box::leak(v.into_boxed_slice())
}

fn write_exe(path: &std::path::Path, text: &str) {
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The rig: the binary beside a stand-in runner, a stand-in `node`.
fn shot_rig() -> Rig {
    let mut rig = Rig::new(VERB, "shot");
    let inst = rig.dir.join("inst");
    std::fs::create_dir_all(&inst).unwrap();
    let exe = inst.join("k5-cli");
    std::fs::copy(BIN, &exe).unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(inst.join("k2-widget-shot.mjs"), "// stand-in\n").unwrap();
    rig.bin = leak(exe.to_string_lossy().into_owned());
    let shim = rig.dir.join("shim");
    std::fs::create_dir_all(&shim).unwrap();
    let log = rig.dir.join("node.log");
    write_exe(
        &shim.join("node"),
        &format!(
            r#"#!/bin/sh
log='{log}'
printf 'node' >> "$log"
prev=""
page=""
for a in "$@"; do
    if [ "$prev" = "--page" ]; then
        page="$a"
        where=elsewhere
        [ "$(dirname "$a")" = "${{TMPDIR%/}}" ] && where=tmpdir
        printf ' [page %s in %s]' "$(basename "$a" | sed 's/\.[^.]*$/.XXXXXX/')" "$where" >> "$log"
    else
        printf ' [%s]' "$a" >> "$log"
    fi
    prev="$a"
done
echo >> "$log"
printf 'front: %s %s %s\n' "${{K5_CLI_SCRIPT_DIR-unset}}" "${{K5_CLI_VIA-unset}}" "${{K5_CLI_WOULD_REFUSE-unset}}" >> "$log"
printf 'mode: %s\n' "$(ls -l "$page" | cut -c1-10)" >> "$log"
cat "$page" >> "$log"
echo '<end of page>' >> "$log"
[ -z "${{SHOT_ERR:-}}" ] || printf '%s\n' "$SHOT_ERR" >&2
[ -z "${{SHOT_OUT:-}}" ] || printf '%s\n\n' "$SHOT_OUT"
exit "${{SHOT_RC:-0}}"
"#,
            log = log.display()
        ),
    );
    rig.path_first = Some(shim);
    rig.tool_log = Some(log);
    rig
}

/// A staged widget folder and a `--data` fixture with its `blobs/`
/// folder: names Python's `mimetypes` reads by the built-in map, by the
/// system's `mime.types`, by neither, a compressed one, a link, a hidden
/// file, a folder, and two files with the same bytes (one ref, the later
/// name's).
fn fixture(r: &Rig) {
    owner_and_port(r);
    let d = r.proj.join("staged-w");
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(
        d.join("manifest.json"),
        r#"{"id":"wid","entry":"index.html"}"#,
    )
    .unwrap();
    std::fs::write(d.join("index.html"), "<p>é</p>\n").unwrap();
    std::fs::write(d.join("notes.txt"), "skipped with a note").unwrap();
    let f = r.proj.join("fixture");
    let _ = std::fs::remove_dir_all(&f);
    let b = f.join("blobs");
    std::fs::create_dir_all(b.join("sub")).unwrap();
    std::fs::write(
        f.join("data.json"),
        "{\"agent.send ws-1\": {\"ok\": true}, \"blobs\": 0,\r\n \"store\": {\"k\": 1e3, \"big\": 123456789012345678901234567890, \"\u{e9}\": \"\\u00e9\\ud83d\"}}\n",
    )
    .unwrap();
    let png: &[u8] = &[0, 1, 2, 250, 255];
    // Python 3.13 and later name more types in the built-in map (.webp
    // from 3.13, .woff2 from 3.14): the binary reads 3.12's, the named
    // difference `python-version-mime-map`. Those names go in only where
    // the shell's python3 is 3.12; every other name here types alike in
    // 3.9 to 3.14, and `.apk` is in no built-in map, only `mime.types`.
    if python3_version() == (3, 12) {
        std::fs::write(b.join("anim.webp"), b"webp").unwrap();
        std::fs::write(b.join("font.woff2"), b"woff2").unwrap();
    }
    for (name, bytes) in [
        ("logo.png", png),
        ("same.bin", png),
        ("photo.JPG", b"jpg"),
        ("store.apk", b"apk"),
        ("arch.tar.gz", b"tgz"),
        ("icon.svgz", b"svgz"),
        ("clip.webm", b"webm"),
        ("page.html", b"<p>"),
        ("notes.txt", b"n2"),
        ("x.json", b"{}"),
        ("a:b.png", b"colon"),
        ("noext", b"n"),
        (".hidden", b"h"),
        ("sub/inner.png", b"inner"),
    ] {
        std::fs::write(b.join(name), bytes).unwrap();
    }
    std::os::unix::fs::symlink(b.join("page.html"), b.join("link.htm")).unwrap();
    std::fs::write(f.join("list.json"), "[1, 2]").unwrap();
    std::fs::write(f.join("bad.json"), "{\"a\": 1,}").unwrap();
}

const PAGE: &str =
    "{\"ok\":true,\"host\":\"<!doctype html><p>\u{e9}</p>\",\"frame\":{\"w\":1}}\n\n";
const SHOT: &str = r#"{"ok":true,"png":"out.png","scheme":"dark","width":800,"height":600,"ms":412,"ready":true,"errors":[],"refusals":[]}"#;
const SHOT_SAD: &str = r#"{"ok":false,"png":"out.png","scheme":"light","width":1,"height":2.5,"ms":9,"ready":false,"errors":["TypeError: x is undefined",{"at":"a.js"}],"refusals":["store.get","agent.send"]}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    let mut c = c;
    c.env.extend(env);
    c
}

#[test]
fn zen_widget_shot_answers_as_the_shell_does() {
    let rig = shot_rig();
    let inst = rig.dir.join("inst").to_string_lossy().into_owned();
    let proj = rig.proj.to_string_lossy().into_owned();
    let folder = leak(format!("{proj}/staged-w"));
    let data = leak(format!("{proj}/fixture/data.json"));
    let script_dir = || vec![("K5_CLI_SCRIPT_DIR", inst.clone())];
    let run = |name: &'static str,
               args: Vec<&'static str>,
               env: Vec<(&'static str, String)>,
               replies: Vec<common::stub::Reply>,
               code: i32,
               requests: usize| {
        let mut e = script_dir();
        e.extend(env);
        with(
            e,
            case(name, words(args), &[], replies, fixture, code, requests),
        )
    };
    let full = vec![
        "widget",
        "preview",
        "--dir",
        folder,
        "w3",
        "--shot",
        "out.png",
        "--theme",
        "dark",
        "--zen-theme",
        "nord",
        "--width",
        "0800",
        "--height",
        "600",
        "--data",
        data,
        "--libs",
        "/opt/zen-lib",
        "--wait-ms",
        "250",
        "--config",
        " k = v",
        "--config=a=b",
        "--config",
        "k=again",
    ];
    let cases = vec![
        // covers: zen widget preview
        run(
            "preview --shot, every flag and the fixture's blobs",
            full.clone(),
            vec![("SHOT_OUT", SHOT.into())],
            vec![ok(PAGE)],
            0,
            1,
        ),
        // covers: zen widget preview --json
        run(
            "preview --shot --json: the runner's report as it is",
            vec![
                "widget", "preview", "--json", "--dir", folder, "--shot", "o.png",
            ],
            vec![("SHOT_OUT", SHOT_SAD.into()), ("SHOT_RC", "1".into())],
            vec![ok(PAGE)],
            1,
            1,
        ),
        run(
            "preview --shot: not ready, errors, refusals, the runner's own exit",
            vec![
                "widget", "preview", "--dir", folder, "--shot", "o.png", "--theme", "light",
            ],
            vec![
                ("SHOT_OUT", SHOT_SAD.into()),
                ("SHOT_RC", "1".into()),
                ("SHOT_ERR", "webkit: a warning".into()),
            ],
            vec![ok(PAGE)],
            1,
            1,
        ),
        run(
            "preview --shot: the runner prints nothing and exits 0",
            vec!["widget", "preview", "--dir", folder, "--shot", "o.png"],
            vec![],
            vec![ok(PAGE)],
            0,
            1,
        ),
        run(
            "preview --shot: the runner prints nothing and fails",
            vec!["widget", "preview", "--dir", folder, "--shot", "o.png"],
            vec![("SHOT_RC", "7".into()), ("SHOT_ERR", "no webkit".into())],
            vec![ok(PAGE)],
            7,
            1,
        ),
        Case {
            compare: Compare::Traceback,
            ..run(
                "preview --shot: a report with no scheme",
                vec!["widget", "preview", "--dir", folder, "--shot", "o.png"],
                vec![("SHOT_OUT", r#"{"png":"o.png"}"#.into())],
                vec![ok(PAGE)],
                1,
                1,
            )
        },
        Case {
            compare: Compare::Traceback,
            ..run(
                "preview --shot: a report that is not JSON",
                vec!["widget", "preview", "--dir", folder, "--shot", "o.png"],
                vec![("SHOT_OUT", "Error: playwright is missing".into())],
                vec![ok(PAGE)],
                1,
                1,
            )
        },
        run(
            "preview --shot: refused, nothing run",
            vec!["widget", "preview", "--dir", folder, "--shot", "o.png"],
            vec![],
            vec![ok(r#"{"error":"person_only","message":"people only"}"#)],
            3,
            1,
        ),
        run(
            "preview --shot --json: an error answer",
            vec![
                "widget", "preview", "--dir", folder, "--shot", "o.png", "--json",
            ],
            vec![],
            vec![ok(r#"{"error":"zen_off","message":"Zen is off"}"#)],
            1,
            1,
        ),
        run(
            "preview --shot: the folder has errors",
            vec!["widget", "preview", "--dir", folder, "--shot", "o.png"],
            vec![],
            vec![ok(
                r#"{"ok":false,"errors":[{"file":"index.html","line":2,"col":5,"message":"bad tag"},{"file":"a.js"}]}"#,
            )],
            1,
            1,
        ),
        run(
            "preview --shot: an empty answer",
            vec!["widget", "preview", "--dir", folder, "--shot", "o.png"],
            vec![],
            vec![ok("")],
            1,
            1,
        ),
        run(
            "preview --shot: an answer that is not JSON",
            vec!["widget", "preview", "--dir", folder, "--shot", "o.png"],
            vec![],
            vec![ok("<html>\r\nbusy\n")],
            1,
            1,
        ),
        run(
            "preview --shot, a --data fixture that is a list",
            vec![
                "widget",
                "preview",
                "--dir",
                folder,
                "--shot",
                "o.png",
                "--data",
                leak(format!("{proj}/fixture/list.json")),
            ],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot, a --data fixture that is not JSON",
            vec![
                "widget",
                "preview",
                "--dir",
                folder,
                "--shot",
                "o.png",
                "--data",
                "fixture/bad.json",
            ],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot, no --data file",
            vec![
                "widget",
                "preview",
                "--dir",
                folder,
                "--shot",
                "o.png",
                "--data",
                "nope.json",
            ],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot, a --config with no =",
            vec![
                "widget", "preview", "--dir", folder, "--shot", "o.png", "--config", "nokey",
            ],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot, no such folder",
            vec!["widget", "preview", "--dir", "nowhere", "--shot", "o.png"],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot, a theme that is neither",
            vec![
                "widget", "preview", "--dir", folder, "--shot", "o.png", "--theme", "blue",
            ],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot, not a .png",
            vec!["widget", "preview", "--dir", folder, "--shot", "o.jpg"],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot, a width that is no number",
            vec![
                "widget", "preview", "--dir", folder, "--shot", "o.png", "--width", "8x",
            ],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --theme with no --shot",
            vec!["widget", "preview", "--dir", folder, "--theme", "dark"],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot --clear",
            vec!["widget", "preview", "--clear", "--shot", "o.png"],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot with no --dir",
            vec!["widget", "preview", "--shot", "o.png", "a", "b"],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot with no value",
            vec!["widget", "preview", "--dir", folder, "--shot"],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --libs with no value",
            vec![
                "widget", "preview", "--dir", folder, "--shot", "o.png", "--libs", "",
            ],
            vec![],
            vec![],
            2,
            0,
        ),
        run(
            "preview --shot=o.png: no such flag",
            vec!["widget", "preview", "--dir", folder, "--shot=o.png"],
            vec![],
            vec![],
            2,
            0,
        ),
        Case {
            compare: Compare::Shell,
            ..run(
                "preview --shot -h: help, nothing sent, nothing run",
                vec![
                    "widget", "preview", "--dir", folder, "--shot", "o.png", "-h",
                ],
                vec![],
                vec![],
                0,
                0,
            )
        },
        Case {
            compare: Compare::Shell,
            ..run(
                "preview --shot --help: help, nothing sent, nothing run",
                vec![
                    "widget", "preview", "--help", "--shot", "o.png", "--dir", folder,
                ],
                vec![],
                vec![],
                0,
                0,
            )
        },
    ];
    for c in &cases {
        rig.check(c);
    }
}

/// `PATH` as the tests run with it, without `node`: each folder that has
/// one is shadowed by links to everything else in it.
fn path_without_node(scratch: &std::path::Path) -> String {
    let path = std::env::var_os("PATH").expect("a PATH to run the tests with");
    let mut dirs = Vec::new();
    for (i, dir) in std::env::split_paths(&path).enumerate() {
        if std::fs::symlink_metadata(dir.join("node")).is_err() {
            dirs.push(dir);
            continue;
        }
        let shadow = scratch.join(format!("no-node-{i}"));
        std::fs::create_dir_all(&shadow).unwrap();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let name = entry.unwrap().file_name();
            if name != "node" {
                std::os::unix::fs::symlink(dir.join(&name), shadow.join(&name)).unwrap();
            }
        }
        dirs.push(shadow);
    }
    std::env::join_paths(dirs)
        .unwrap()
        .into_string()
        .expect("a UTF-8 PATH")
}

/// No `node` on `PATH`: the line, exit 3, nothing sent.
#[test]
fn zen_widget_shot_with_no_node() {
    let rig = shot_rig();
    let inst = rig.dir.join("inst").to_string_lossy().into_owned();
    let folder = leak(format!("{}/staged-w", rig.proj.display()));
    let path = path_without_node(&rig.dir);
    let c = with(
        vec![("K5_CLI_SCRIPT_DIR", inst), ("PATH", path)],
        case(
            "preview --shot, no node",
            words(vec![
                "widget", "preview", "--dir", folder, "--shot", "o.png",
            ]),
            &[],
            vec![],
            fixture,
            3,
            0,
        ),
    );
    rig.check(&c);
}

/// The usage lines name the command and the product by the brand (in a
/// build with `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn zen_widget_shot_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let product = k2_client::brand::PRODUCT_NAME;
    let rig = shot_rig();
    for (args, want) in [
        (
            &["widget", "preview", "--shot", "o.png"][..],
            format!("Error: {cli} zen widget preview --dir <folder> [<name>] --shot <png> [--theme dark|light] [--width N] [--height N]\n"),
        ),
        (
            &["widget", "preview", "--shot", "o.png", "--libs"],
            format!("Error: --libs needs {product}'s zen-lib folder\n"),
        ),
        (
            &["widget", "preview", "--shot", "o.png", "--x"],
            format!("Error: unexpected '--x' ({cli} zen widget preview --help)\n"),
        ),
    ] {
        let c = case("brand", args, &[], vec![], fixture, 2, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(2), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}
