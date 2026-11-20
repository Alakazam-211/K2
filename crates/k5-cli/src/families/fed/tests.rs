use super::pair::{a2b_base64, b64url, token_body};
use super::peers::{casefold, outbox_reader, peers_reader};
use super::pubkey::sha256;
use super::*;
use crate::families::shell_python;

fn cps(s: &str) -> Vec<u32> {
    s.chars().map(u32::from).collect()
}

#[test]
fn the_words() {
    assert_eq!(parse(&[]), Form::Usage);
    assert_eq!(parse(&["nope"]), Form::Usage);
    assert_eq!(
        parse(&["send", "a::b::c"]),
        Form::Send {
            to: "a::b::c".into(),
            text: String::new()
        }
    );
    assert_eq!(
        parse(&[
            "pair-start",
            "--label",
            "L",
            "--url=http://h:1",
            "--subdomain=s"
        ]),
        Form::PairStart(Ok(["L".into(), "s".into(), "http://h:1".into()]))
    );
    // `shift 2` with one word left fails under `set -e`: exit 1, no line.
    assert_eq!(
        parse(&["pair-start", "--label"]),
        Form::PairStart(Err((Exit::Error, None)))
    );
    assert_eq!(
        parse(&["pair-start", "--pubkey", "x"]),
        Form::PairStart(Err((
            Exit::Usage,
            Some(format!(
                "{} fed pair-start: unknown arg: --pubkey",
                cli_name!()
            ))
        )))
    );
    assert_eq!(
        parse(&["pair-request", "--pubkey=k.pem", "--base-url", "u"]),
        Form::PairRequest(Ok([
            String::new(),
            String::new(),
            "u".into(),
            "k.pem".into()
        ]))
    );
    assert!(parse(&["pair-join", "-"]).reads_stdin());
    assert!(!parse(&["pair-join", "k2pair1:x"]).reads_stdin());
    assert_eq!(parse(&["enable"]), Form::Switch(true));
    assert_eq!(parse(&["disable", "x"]), Form::Switch(false));
}

#[test]
fn the_owner_s_verbs() {
    for w in [
        "pair-start",
        "pair-join",
        "pair-finish",
        "pair-verify",
        "confirm",
        "outbox",
    ] {
        assert!(parse(&[w]).owner_only().is_some(), "{w}");
    }
    for w in [
        "send",
        "pair-request",
        "pubkey",
        "peers",
        "status",
        "enable",
        "",
    ] {
        assert!(parse(&[w]).owner_only().is_none(), "{w}");
    }
}

#[test]
fn base64_as_cpython_reads_it() {
    assert_eq!(a2b_base64(b"YWJj").unwrap(), b"abc");
    assert_eq!(a2b_base64(b"YWI=").unwrap(), b"ab");
    // Bytes outside the alphabet are skipped; the first full pad ends it.
    assert_eq!(a2b_base64(b"Y W\nJj").unwrap(), b"abc");
    assert_eq!(a2b_base64(b"YWI=YWJj").unwrap(), b"ab");
    assert_eq!(a2b_base64(b"YW").unwrap_err(), "Incorrect padding");
    assert_eq!(
        a2b_base64(b"YWJjZ===").unwrap_err(),
        "Invalid base64-encoded string: number of data characters (5) cannot be 1 more than a multiple of 4"
    );
    assert_eq!(b64url(b"abc"), "YWJj");
    assert_eq!(b64url(b"ab"), "YWI");
    assert_eq!(b64url(&[0xfb, 0xff]), "-_8");
}

#[test]
fn a_token_as_the_shell_s_python_reads_it() {
    let good = format!("k2pair1:{}", b64url(r#"{"a":[1,"é"]}"#.as_bytes()));
    assert_eq!(
        token_body("pair-join", &cps(&format!("  {good}\n"))).unwrap(),
        "{\"offer\": {\"a\": [1, \"\\u00e9\"]}}"
    );
    let err = |step: &str, t: &str| token_body(step, &cps(t)).unwrap_err();
    let e = err("pair-finish", "k2pair1:abc");
    assert_eq!(
        e.stderr,
        format!(
            "{} fed pair-finish: expected a k2pair2:... token (got k2pair1:...)\n",
            cli_name!()
        )
    );
    assert_eq!(e.exit, Exit::Usage);
    let e = err("pair-verify", "k2pair3:YWJjZ");
    assert_eq!(
        e.stderr,
        format!("{} fed pair-verify: the token is damaged (Invalid base64-encoded string: number of data characters (5) cannot be 1 more than a multiple of 4)\n", cli_name!())
    );
    let e = err("pair-verify", &format!("k2pair3:{}", b64url(b"nope")));
    assert_eq!(
        e.stderr,
        format!(
            "{} fed pair-verify: the token is damaged (Expecting value: line 1 column 1 (char 0))\n",
            cli_name!()
        )
    );
    let e = err("pair-verify", &format!("k2pair3:{}", b64url(&[0xff])));
    assert!(
        e.stderr
            .contains("('utf-8' codec can't decode byte 0xff in position 0: invalid start byte)"),
        "{}",
        e.stderr
    );
}

#[test]
fn the_pairing_answers() {
    let r = pair::reader("pair-start", br#"{"offer":{"x":1}}"#);
    assert_eq!(
        r.stdout,
        format!("Send this to the other server owner (valid 10 minutes, single use):\nk2pair1:{}\nThey run: {} fed pair-join --token-stdin (and paste it), and send you back a k2pair2: token.\n", b64url(br#"{"x":1}"#), cli_name!())
    );
    let r = pair::reader("pair-verify", br#"{"sas":"12-34","peer_fingerprint":"ab"}"#);
    assert_eq!(
        r.stdout,
        format!(
            "Your pairing code: 12-34\nIf both codes match, run: {} fed confirm ab 12-34\n",
            cli_name!()
        )
    );
    let r = pair::reader("pair-finish", br#"{"reveal":{}}"#);
    assert_eq!(r.exit, Exit::Error);
    assert!(r
        .stderr
        .ends_with("  File \"<string>\", line 24, in <module>\nKeyError: 'sas'\n"));
    let r = pair::reader("pair-join", br#"{"error":"nope"}"#);
    assert_eq!(r.stderr, format!("{} fed pair-join: nope\n", cli_name!()));
    let r = pair::reader("pair-join", b"[1]");
    assert_eq!(r.stderr, format!("{} fed pair-join: [1]\n", cli_name!()));
    let r = pair::reader("pair-join", b"");
    assert_eq!(r.exit, Exit::Error);
    assert!(r.stderr.contains("no answer from the daemon"));
}

#[test]
fn the_switch_readers() {
    let r = switch_reader(br#"{"federationEnabled":true}"#, "");
    assert_eq!(
        r.stdout,
        "Connect agents across servers: on (this server)\n"
    );
    let r = switch_reader(b"{}", " status");
    assert_eq!(
        r.stdout,
        "Connect agents across servers: off (this server)\n"
    );
    let r = switch_reader(b"[]", " status");
    assert_eq!(
        r.stderr,
        format!(
            "{} fed status: unexpected response from /cli/settings/get\n",
            cli_name!()
        )
    );
}

#[test]
fn sha256_vectors() {
    let hex = |b: [u8; 32]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    assert_eq!(
        hex(sha256(b"")),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        hex(sha256(b"abc")),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        hex(sha256(&[b'a'; 1000])),
        "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3"
    );
}

#[test]
fn casefold_folds_what_lower_does_not() {
    assert_eq!(casefold("Straße"), "strasse");
    assert_eq!(casefold("ΣΑΣ ς"), "σασ σ");
    assert_eq!(casefold("ﬁle"), "file");
}

#[test]
fn peers_in_order() {
    let raw = br#"{"peers":[{"label":"zeta","subdomain":"z","fingerprint":"0123456789abcdefXYZ","trust":"pinned"},{"subdomain":"Alpha","base_url":"https://a.example/x","repair_recommended":true,"address_notice":{"status":"pending","target":{"subdomain":"new"},"attempts":2,"last_error":"down"}},{"fingerprint":"ffff","address_notice":{"status":"failed","target":{"base_url":"http://b"}}}]}"#;
    let r = peers_reader(raw);
    let cli = cli_name!();
    assert_eq!(r.exit, Exit::Ok);
    assert_eq!(
        r.stdout,
        format!("Peers (3):\n  https://a.example/x  trust=?  fp=\u{2026}\n      older pairing code: messages still work; pairing again recommended (Settings \u{2192} Network \u{2192} Connected Servers \u{2192} Pair again, or {cli} fed pair-start)\n      address notice: pending -> new (attempts 2; last error: down)\n  \u{2014}  trust=?  fp=ffff\u{2026}\n      address notice: FAILED -> http://b (?): \n  z.k2.dev  trust=pinned  fp=0123456789abcdef\u{2026}  label=zeta\n")
    );
    let r = peers_reader(br#"{"peers":[]}"#);
    assert!(r.stdout.starts_with("No paired servers yet.\n"));
    let r = peers_reader(br#"{"error":{"code":"federation_off"}}"#);
    assert_eq!(r.stderr, format!("{cli} fed peers: federation_off\n"));
    let r = peers_reader(br#"{"peers":["x"]}"#);
    assert_eq!(r.exit, Exit::Error);
    assert!(r.stderr.ends_with("  File \"<string>\", line 27, in <module>\n  File \"<string>\", line 27, in <lambda>\n  File \"<string>\", line 19, in _shown\nAttributeError: 'str' object has no attribute 'get'\n"), "{}", r.stderr);
    let r = peers_reader(br#"{"peers":7}"#);
    assert!(r
        .stderr
        .ends_with("TypeError: 'int' object is not iterable\n"));
    let r = peers_reader(b"nope");
    assert_eq!(
        r.stderr,
        format!("{cli} fed peers: could not parse response\n")
    );
}

#[test]
fn the_outbox() {
    let raw = br#"{"peers":[{"label":"acme","queued":2,"oldest_age_secs":30,"dead":1,"dead_letters":[{"msg_uuid":"0123456789","reason":"gone"}]},{"fingerprint":"abcdefabcdefabcdef"}],"total_queued":2}"#;
    let r = outbox_reader(raw);
    assert_eq!(
        r.stdout,
        "acme: 2 queued (oldest 30s), 1 dead-lettered\n  dead 01234567\u{2026} \u{2014} gone\nabcdefabcdef: 0 queued, 0 dead-lettered\ntotal queued: 2\n"
    );
    let r = outbox_reader(br#"{"peers":[]}"#);
    assert_eq!(
        r.stdout,
        "outbox: empty (no queued or dead-lettered cross-server messages)\n"
    );
    let r = outbox_reader(br#"{"error":"off"}"#);
    assert_eq!(
        (r.stderr.as_str(), r.exit),
        (
            format!("{} fed outbox: off\n", cli_name!()).as_str(),
            Exit::Error
        )
    );
    let r = outbox_reader(b"7");
    assert!(r.stderr.ends_with(
        "  File \"<string>\", line 6, in <module>\nTypeError: argument of type 'int' is not iterable\n"
    ));
}

/// The lines the tracebacks name are the shell's lines.
#[test]
fn the_traceback_lines_are_the_shell_s() {
    let peers = shell_python("federation.sh", "\"/cli/federation/peers\"");
    for (line, has) in [
        (19, "return ((p.get('label') or '').strip()"),
        (20, "(p.get('fingerprint') or '')[:12]"),
        (22, "b = (p.get('base_url')"),
        (26, "return b.split('/')[0]"),
        (27, "peers = sorted(peers"),
        (34, "sub = (p.get('subdomain') or '').strip()"),
        (35, "base = (p.get('base_url')"),
        (46, "fp = (p.get('fingerprint') or '')[:16]"),
        (53, "st = n.get('status')"),
        (54, "tgt = n.get('target')"),
        (55, "new_addr = (tgt.get('subdomain')"),
    ] {
        assert!(
            peers[line - 1].contains(has),
            "peers line {line}: {}",
            peers[line - 1]
        );
    }
    // The outbox's Python ends on its last line (`…\")"`), so it is cut
    // here: from its `-c "` to the `;;` after it.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/federation.sh");
    let text = std::fs::read_to_string(&path).unwrap();
    let body = &text[text.find("/cli/federation/outbox\"").expect("the outbox")..];
    let body = &body[body.find("python3 -c \"").expect("its Python") + "python3 -c \"".len()..];
    let body = &body[..body.find("\n            ;;").expect("its end")];
    let outbox: Vec<&str> = body.split('\n').collect();
    for (line, has) in [
        (6, "if 'error' in d:"),
        (7, "str(d['error'])"),
        (8, "peers = d.get('peers', [])"),
        (12, "for p in peers:"),
        (13, "who = p.get('label')"),
        (14, "p.get('queued',0)"),
        (15, "p.get('oldest_age_secs')"),
        (18, "p.get('dead',0)"),
        (20, "p.get('dead_letters', [])[:5]"),
        (21, "dl.get('msg_uuid','')[:8]"),
        (22, "d.get('total_queued',0)"),
    ] {
        assert!(
            outbox[line - 1].contains(has),
            "outbox line {line}: {}",
            outbox[line - 1]
        );
    }
}
