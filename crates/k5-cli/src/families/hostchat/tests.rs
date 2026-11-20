use super::*;
use crate::exit::Exit;
use k2_client::brand::PRODUCT_NAME;

fn out(verb: &str, body: &str) -> String {
    let r = print::print(body, verb);
    assert_eq!((r.exit, r.stderr.as_str()), (Exit::Ok, ""), "{verb} {body}");
    r.stdout
}

#[test]
fn status_lines_as_the_shell_prints_them() {
    assert_eq!(
        out("status", r#"{"state":"running","installedVersion":"0.6.0","alarms":["a","b"],"voice":{"cert":{"daysLeft":3}}}"#),
        "Chat / Voice Hosting: running\nversion:         0.6.0\nalarms:          a, b\nTURN cert:       3 days left\n"
    );
    assert_eq!(
        out("status", r#"{"state":"x","pinnedVersion":null}"#),
        "Chat / Voice Hosting: x\nversion:         pin None\n"
    );
    assert_eq!(
        out("job", r#"{"ok":true,"a":[],"b":"","c":null,"d":{"e":[1,"\u00e9"]},"f":0}"#),
        "a:               []\nd:               {\"e\": [1, \"\\u00e9\"]}\nf:               0\n"
    );
}

#[test]
fn a_raise_part_way() {
    let r = print::print(r#"{"state":"x","mirror":{"kind":"dir","path":3}}"#, "status");
    assert_eq!(r.exit, Exit::Error);
    assert!(r.stdout.starts_with("Chat / Voice Hosting: x\n"), "{}", r.stdout);
    assert!(r.stderr.ends_with("TypeError: unsupported operand type(s) for +: 'int' and 'str'\n"), "{}", r.stderr);
}

#[test]
fn compute_json_puts_a_token_first() {
    assert_eq!(
        cj(&[("phase", Field::S("drop-prev".into())), ("confirmToken", Field::S("t1".into()))]),
        r#"{"confirmToken": "t1", "phase": "drop-prev"}"#
    );
    assert_eq!(
        cj(&[("purge", Field::B("1".into())), ("keep", Field::B("0".into())), ("host", Field::S("aToken=b".into()))]),
        r#"{"host": "aToken=b", "purge": true, "keep": false}"#
    );
}

/// Every line that names the command names it by the brand's name (in a
/// build with `K2_BRAND_DIR=brand/xy` they say `xy`), never a typed `k2`
/// or `k5`.
#[test]
fn the_lines_say_the_brand_s_name() {
    let named = |line: &str| {
        assert!(line.contains(&format!("{CLI_NAME} ")), "{line}");
        if CLI_NAME != "k2" {
            assert!(!line.contains("k2 ") && !line.contains("k5 "), "{line}");
        }
    };
    let lines: &[&[&str]] = &[
        &["nope"],
        &["status", "--x"],
        &["status", "--voice"],
        &["status", "--media-tarball", "m"],
        &["upgrade", "--accept-data-loss"],
        &["act-as"],
        &["config", "--setup-url"],
    ];
    for &words in lines {
        let Plan::Bad(line) = parse(words) else {
            panic!("not a usage line: {words:?}")
        };
        named(&line);
    }
    named(&out("status", r#"{"upgradeAvailable":true}"#));
    named(&out("status", r#"{"recoveryKeyInVault":true}"#));
    named(&out("rollback", r#"{"mediaCommand":"x"}"#));
    let tie = out("status", r#"{"mirror":{"tie":{"state":"pending"}}}"#);
    assert!(tie.contains(&format!("{PRODUCT_NAME} retries")), "{tie}");
}
