//! [`k5_cli::cards::enabled`] against the shell's own reader: the
//! `python3 -c '...'` of `_permission_probe` (`cli/src/permission.sh`),
//! run on each body under a C locale and a UTF-8 one, with the exit code
//! (0: cards on) compared to the binary's answer for the way
//! [`k5_cli::cards::decode`] says that `python3` reads its stdin.

#![cfg(unix)]

mod common;

use k5_cli::cards::{decode, enabled};
use k5_cli::offline::Env;
use std::io::Write;
use std::process::{Command, Stdio};

/// The Python text of `_permission_probe`'s reader.
fn reader() -> String {
    let src = std::fs::read_to_string(common::cli_src().join("permission.sh"))
        .expect("read cli/src/permission.sh");
    let probe = common::block(&src, "_permission_probe() {", "}");
    let (_, rest) = probe
        .split_once("| python3 -c '\n")
        .expect("the probe pipes the answer into python3 -c");
    let (code, _) = rest
        .split_once("\n' 2>/dev/null")
        .expect("the reader's end");
    assert!(code.contains("json.load(sys.stdin)"), "{code}");
    code.to_string()
}

fn python(code: &str, body: &[u8], locale: &str) -> bool {
    let mut child = Command::new("python3")
        .arg("-c")
        .arg(code)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").expect("PATH"))
        .env("LC_ALL", locale)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run python3");
    child
        .stdin
        .take()
        .expect("python3's stdin")
        .write_all(body)
        .expect("write the body");
    let out = child.wait_with_output().expect("wait for python3");
    let code = out.status.code().expect("python3 exited");
    assert!(
        code == 0 || code == 1,
        "python3 exited {code}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    code == 0
}

#[test]
fn the_reader_is_the_shell_s() {
    let code = reader();
    // macOS: a UTF-8 locale Python reads strictly (it always has this
    // one). Linux: C.UTF-8, built into glibc, one of the names Python's
    // locale coercion sets, which it reads as it reads C.
    let utf8 = if cfg!(target_os = "macos") {
        "en_US.UTF-8"
    } else {
        "C.UTF-8"
    };
    let bodies: &[&[u8]] = &[
        br#"{"enabled":true}"#,
        br#"{"enabled": true, "server": "on"}"#,
        b" \n{\"enabled\":true}\n\n",
        b"{\"enabled\":true}\r\n",
        br#"{"enabled":false}"#,
        br#"{"enabled":false,"enabled":true}"#,
        br#"{"enabled":true,"enabled":false}"#,
        br#"{"enabled":"true"}"#,
        br#"{"enabled":1}"#,
        br#"{"enabled":1.0}"#,
        br#"{"enabled":null}"#,
        br#"{}"#,
        br#"[]"#,
        br#"[{"enabled":true}]"#,
        br#"true"#,
        br#""enabled""#,
        br#"{"Enabled":true}"#,
        br#"{"enabled":true,"n":NaN,"i":-Infinity,"big":1e999}"#,
        br#"{"enabled":true}{}"#,
        br#"{"enabled":true,}"#,
        br#"{'enabled':true}"#,
        b"",
        b"\n",
        b"not json",
        "\u{feff}{\"enabled\":true}".as_bytes(),
        "{\"enabled\":true,\"note\":\"caf\u{e9} \u{2014}\"}".as_bytes(),
        b"{\"enabled\":true,\"x\":\"\xff\xfe\"}",
        b"{\"enabled\":true}\xff",
        b"{\"enabled\":true,\"x\":\"a\tb\"}",
        b"{\"enabled\":true,\"x\":\"\\ud83d\"}",
        br#"{"enabled":true,"deep":[[[[[[[[[[[[[[[[[[[[1]]]]]]]]]]]]]]]]]]]]}"#,
    ];
    let mut rows = 0;
    for locale in ["C", utf8] {
        let how = decode(&Env::of(&[("LC_ALL", locale)])).expect("a locale the binary reads");
        for body in bodies {
            let want = python(&code, body, locale);
            assert_eq!(
                enabled(body, how),
                want,
                "{locale} ({how:?}): {:?}",
                String::from_utf8_lossy(body)
            );
            rows += 1;
        }
    }
    assert_eq!(rows, 2 * bodies.len());
    // The shell's `$(…)` drops a NUL before Python reads the body.
    let how = decode(&Env::of(&[("LC_ALL", "C")])).unwrap();
    assert!(enabled(b"{\"enabled\":\0true}", how));
    assert!(python(&code, b"{\"enabled\":true}", "C"));
}
