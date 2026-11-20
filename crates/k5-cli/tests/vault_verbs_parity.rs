//! `k5-cli vault <verb>` for the verbs `cmd_vault`'s own parser reads
//! (`families::vault::verbs`), against `cli/k2 vault <verb>` on the vault
//! rig ([`common::vault`]): each verb's request and printer, its usage
//! lines, the daemon's refusals (a permission card's closing line too),
//! no answer, `--json`, a body Python can't read, a session's cell
//! socket, a value on stdin that no child sees, and one list above
//! 256 KiB.

#![cfg(unix)]

mod common;

use common::parity::{PASSPORT, PERMISSION_ASK};
use common::stub::Reply;
use common::vault::{VCase, VCompare, VRig};

fn ok(body: &str) -> Reply {
    Reply::Answer(200, body.to_string())
}

/// One request to the daemon, this answer, this exit.
fn daemon(name: &'static str, args: &[&'static str], reply: Reply, code: i32) -> VCase {
    let mut c = VCase::new(name, args);
    c.daemon = vec![reply];
    c.daemon_requests = 1;
    c.code = code;
    c
}

/// Nothing sent: a usage line, exit 2.
fn refused(name: &'static str, args: &[&'static str]) -> VCase {
    let mut c = VCase::new(name, args);
    c.code = 2;
    c
}

fn traceback(mut c: VCase) -> VCase {
    c.compare = VCompare::Traceback;
    c
}

fn with_stdin(mut c: VCase, stdin: &[u8], secrets: &[&'static str]) -> VCase {
    c.stdin = stdin.to_vec();
    c.secrets = secrets.to_vec();
    c
}

const STATUS: &str = r#"{"open":true,"keyHomeLabel":"Keychain","keyHome":"keychain","rollbackGuard":"on","recovery":"confirmed","recoveryConfirmedAt":"2026-01-01","confirmPassword":true,"proxy":{"port":8899,"moved":true,"hint":"moved from 8898"},"holders":[{"kind":"box","fingerprint":"ab12"},{"kind":"recovery","fingerprint":"cd34","createdAt":"2026-02-02"},{"kind":"other"}],"counts":{"user":3,"system":2,"grants":1,"pendingRequests":0},"tamper":"bad","newerRows":2,"rollback":true,"movedOut":{"at":"t1","moveKey":"mk"},"honest":"é honest words","recoveryHint":"keep it"}"#;

const SECRET: &str = r#"{"secret":{"id":"s1","name":"API","kind":"token","status":"active","latestVersion":2,"ref":"k2v:API","hosts":["https://api.example.com","b.example"],"inject":{"sign":{"scheme":"hmac-sha256-ts","template":"{ts}{body}","header":"X-Sig","valueFmt":"v1={sig}","tsHeader":"X-Ts"}},"defaultDelivery":"fetch","deliveryCeiling":"env","agentCanSee":true,"createdBy":"rosa","humanGiven":true,"handoff":{"from":"bob::h","deliveryId":"d1","decidedBy":"amy","acceptedCeiling":"file"},"oauth":{}},"use":"k2 vault fetch --secret API URL","canTake":"more words","agentPolicy":{"burst":2,"perMinute":30,"allowForwarded":false},"grants":[{"consumerLabel":"ws:a","delivery":"env","envName":"API_KEY","expiresAt":"t9"},{"consumerLabel":"job:x","delivery":"file"}],"placements":[{"target":"systemd_cred","credName":"api","unit":"web.service","version":1,"state":"stale"},{"where":"h:/etc/x","version":2,"latestVersion":3,"state":"due","id":"p1"},{"where":"h:/y","version":1,"state":"inactive","secretStatus":"disabled"}],"audit":[{"ts":"t1","action":"fetch","outcome":"ok","actor":"a","count":3},{"ts":"t2","action":"set","outcome":"ok","actor":"b"}]}"#;

#[test]
fn status_and_its_refusals() {
    let rig = VRig::new();
    let mut session = daemon("in a session: the cell socket", &["status"], ok(STATUS), 0);
    session.env = vec![
        ("K2_HOOK_SOCK", "{sock}".to_string()),
        ("K2_HOOK_TOKEN", PASSPORT.to_string()),
    ];
    for c in [
        daemon("status", &["status"], ok(STATUS), 0),
        daemon("status, little in it", &["status"], ok(r#"{"proxy":{"hint":"off"},"tamper":"ok"}"#), 0),
        daemon("closed", &["status"], ok(r#"{"open":false,"error":{"code":"locked","hint":"unlock"}}"#), 0),
        daemon("--json", &["status", "--json"], ok(STATUS), 0),
        daemon("a permission card", &["status"], Reply::Answer(403, PERMISSION_ASK.into()), 3),
        daemon("not found", &["status"], Reply::Answer(404, r#"{"error":{"code":"nope","hint":"h"}}"#.into()), 4),
        daemon("a 410", &["status"], Reply::Answer(410, r#"{"error":"card_dismissed"}"#.into()), 5),
        daemon("a 422", &["status"], Reply::Answer(422, "not json".into()), 2),
        daemon("an empty 500", &["status"], Reply::Answer(500, String::new()), 1),
        daemon("an empty reply", &["status"], Reply::Close, 1),
        traceback(daemon("a 200 Python can't read", &["status"], ok("nope"), 1)),
        traceback(daemon("a 200 that is a list", &["status"], ok("[1]"), 1)),
        refused("a flag status doesn't take", &["status", "--scope", "server"]),
        session,
    ] {
        rig.check(&c);
    }
}

#[test]
fn list_show_and_the_reads() {
    let rig = VRig::new();
    let list = r#"{"secrets":[{"name":"A","scope":{"kind":"server"},"kind":"token","defaultDelivery":"fetch","latestVersion":1,"hosts":["https://a.example:443","b"],"status":"active"},{"name":"BéB","scope":{"kind":"workspace","id":"w1"},"workspace":"proj","kind":"password","defaultDelivery":"env","latestVersion":12,"agentCanSee":true,"namespace":"system","status":"disabled","inject":{"sign":{"scheme":"ovh"}},"oauth":{"state":"expired"}},{"name":"C","scope":{"kind":"session","id":"0123456789"},"kind":"key"}],"systemHidden":{"count":4,"why":"they are K2's"}}"#;
    let big = format!(
        r#"{{"secrets":[{}]}}"#,
        (0..2600)
            .map(|i| format!(r#"{{"name":"SECRET_{i:05}_PADDING_PADDING_PADDING","scope":{{"kind":"server"}},"kind":"token","defaultDelivery":"fetch","latestVersion":{i},"hosts":["https://host-{i}.example.com:8443/path"],"status":"active"}}"#))
            .collect::<Vec<_>>()
            .join(",")
    );
    assert!(big.len() > 256 * 1024);
    for c in [
        daemon("list", &["list"], ok(list), 0),
        daemon("ls, none", &["ls"], ok(r#"{"secrets":[]}"#), 0),
        daemon("list, none of yours", &["list", "--system"], ok(r#"{"systemHidden":{"count":1,"why":"w"}}"#), 0),
        daemon("list --scope ws:a b", &["list", "--scope", "ws:a b&c", "--system"], ok(list), 0),
        daemon("list: above 256 KiB", &["list"], ok(&big), 0),
        refused("a scope that isn't one", &["list", "--scope", "galaxy"]),
        refused("--system on show", &["show", "A", "--system"]),
        daemon("show", &["show", "API", "--scope", "server"], ok(SECRET), 0),
        daemon("show, an ovh signer", &["show", "B"], ok(r#"{"secret":{"name":"B","inject":{"sign":{"scheme":"ovh"}}}}"#), 0),
        daemon("show, oauth", &["show", "B"], ok(r#"{"secret":{"name":"B","inject":{"sign":{"scheme":"oauth-refresh","tokenUrl":"https://t"}},"oauth":{"expiresAt":"t5"}}}"#), 0),
        daemon("show, hmac and no sites", &["show", "B"], ok(r#"{"secret":{"name":"B","inject":{"sign":{"scheme":"hmac-sha256","header":"X"}}}}"#), 0),
        daemon("show, injected", &["show", "B"], ok(r#"{"secret":{"inject":{"dst":"header:A","fmt":"Bearer {}"}}}"#), 0),
        daemon("show --json", &["show", "B", "--json"], ok(SECRET), 0),
        refused("show with no name", &["show"]),
        daemon("public-key", &["public-key", "K"], ok(r#"{"publicKey":"ssh-ed25519 AAAA","fingerprint":"SHA256:x"}"#), 0),
        traceback(daemon("public-key, none in it", &["public-key", "K"], ok(r#"{}"#), 1)),
        daemon("audit", &["audit", "API", "--since", "2d ago"], ok(r#"{"audit":[{"ts":"t","action":"fetch","outcome":"ok","actor":"me","targetHost":"h","count":2},{"ts":"t2","action":"get","outcome":"refused","actor":"x"}]}"#), 0),
        daemon("audit, all", &["audit"], ok(r#"{"audit":[]}"#), 0),
        daemon("requests", &["requests"], ok(r#"{"requests":[{"createdAt":"t","status":"open","name":"N","requestedBy":"ws","inject":{"dst":"header:X","fmt":"{}"}},{"name":"M"}]}"#), 0),
        daemon("requests, none", &["requests"], ok("{}"), 0),
        daemon("presets", &["presets"], ok(r#"{"presets":[{"id":"gh","label":"GitHub","hosts":["api.github.com"],"dst":"header:Authorization","fmt":"Bearer {}"},{"id":"ovh","label":"OVH","hosts":[],"sign":"ovh"}]}"#), 0),
        daemon("doctor, findings", &["doctor", "--transcripts"], ok(r#"{"findings":[{"level":"warn","message":"m","files":["a","b","c","d","e","f"],"fileCount":9},{"level":"info","message":"n"}],"checkedNames":"A B","summary":"s","ok":false}"#), 1),
        daemon("doctor, nothing", &["doctor"], ok(r#"{"ok":true}"#), 0),
        refused("--transcripts elsewhere", &["status", "--transcripts"]),
        daemon("recipient", &["recipient"], ok(r#"{"recipient":"age1xyz","vaultId":"v1"}"#), 0),
        daemon("proxy-url", &["proxy-url", "API", "--ttl", "2h", "--scope", "session"], ok(r#"{"envName":"API_KEY","placeholder":"k2p_x","baseUrlEnv":"API_BASE","baseUrl":"http://127.0.0.1:1/x"}"#), 0),
        daemon("proxy-url --secret", &["proxy-url", "--secret", "API"], ok(r#"{"envName":"A","placeholder":"p","baseUrlEnv":"B","baseUrl":"u"}"#), 0),
        refused("proxy-url, two", &["proxy-url", "--secret", "A", "--secret", "B"]),
    ] {
        rig.check(&c);
    }
}

#[test]
fn set_and_rotate_take_the_value_on_stdin() {
    let rig = VRig::new();
    let saved = r#"{"created":true,"ref":"k2v:API","secret":{"name":"API","latestVersion":1,"inject":{"sign":{"scheme":"hmac-sha256"}}},"trimmed":true,"use":"k2 vault fetch","ttlCut":true}"#;
    for c in [
        with_stdin(daemon("set", &["set", "API", "--for", "a.com, b.com,", "--kind", "token"], ok(saved), 0), b"hunter2-value\n", &["hunter2-value", "aHVudGVyMi12YWx1ZQ=="]),
        with_stdin(daemon("set, two newlines kept", &["set", "API"], ok(r#"{"secret":{"name":"API"}}"#), 0), b"multi\nline\n", &["multi\nline"]),
        with_stdin(
            daemon("set, signed", &["set", "API", "--sign", "hmac-sha256-ts", "--sign-prefix", "", "--sign-template", "{ts}", "--sign-header", "X", "--json"], ok(saved), 0),
            b"s3cr3t-signing-key",
            &["s3cr3t-signing-key"],
        ),
        with_stdin(daemon("set, injected", &["set", "API", "--inject-header", "X-Key", "--inject-format", "k {}", "--ttl", "2h", "--rotate-every", "30", "--describe", "d é"], ok(saved), 0), b"v", &[]),
        with_stdin(refused("set, no value", &["set", "API"]), b"", &[]),
        with_stdin(refused("set, only a newline", &["set", "API"]), b"\n", &[]),
        refused("set, a value on argv", &["set", "API", "hunter2"]),
        refused("set --value=", &["set", "API", "--value=hunter2"]),
        refused("set, signed and injected", &["set", "API", "--sign", "ovh", "--inject-dst", "header:X"]),
        refused("set, header and dst", &["set", "API", "--inject-header", "X", "--inject-dst", "header:Y"]),
        refused("set, signer options with no signer", &["set", "API", "--sign-header", "X"]),
        traceback(with_stdin(refused("set, --rotate-every not a number", &["set", "API", "--rotate-every", "soon"]), b"v", &[])),
        refused("set, no name", &["set"]),
        with_stdin(daemon("set, refused", &["set", "API"], Reply::Answer(403, PERMISSION_ASK.into()), 3), b"pw-refused", &["pw-refused"]),
        with_stdin(daemon("rotate", &["rotate", "API"], ok(r#"{"secret":{"name":"API"},"version":3,"replaced":[{"ok":true,"unit":"u","credName":"c","restarted":true},{"unit":"v","error":{"hint":"h"}}]}"#), 0), b"new-value-9", &["new-value-9"]),
        refused("rotate, a value on argv", &["rotate", "API", "x"]),
        refused("rotate, no name", &["rotate"]),
    ] {
        rig.check(&c);
    }
}

#[test]
fn the_policy_writes() {
    let rig = VRig::new();
    let sec = r#"{"secret":{"name":"API","hosts":["a","b"],"defaultDelivery":"env","deliveryCeiling":"file","inject":{"sign":{"scheme":"ovh"}}},"agentPolicy":{"burst":5,"perMinute":60,"allowForwarded":true}}"#;
    let gone = r#"{"recall":"2 files were deleted","stillRunning":[{"short":"j1","node":"n","env":"API","state":"running"}]}"#;
    for c in [
        daemon("hosts", &["hosts", "API", "--add", "a.com", "--add", "b.com", "--remove", "c.com"], ok(sec), 0),
        daemon("hosts, none left", &["hosts", "API", "--remove", "a"], ok(r#"{"secret":{"name":"API"}}"#), 0),
        refused("hosts, no name", &["hosts"]),
        daemon("sign none", &["sign", "API", "none"], ok(r#"{"secret":{"name":"API"}}"#), 0),
        daemon("sign ovh", &["sign", "API", "ovh"], ok(sec), 0),
        daemon("sign hmac", &["sign", "API", "hmac-sha256", "--sign-header", "X", "--sign-encoding", "base64", "--sign-prefix", "p="], ok(sec), 0),
        refused("sign none with options", &["sign", "API", "none", "--sign-value", ""]),
        refused("sign with --sign", &["sign", "API", "ovh", "--sign", "ovh"]),
        refused("sign, no scheme", &["sign", "API"]),
        daemon("agent-limit", &["agent-limit", "API", "--burst", "05", "--per-minute", "60", "--no-forwarded"], ok(sec), 0),
        refused("agent-limit, both", &["agent-limit", "API", "--allow-forwarded", "--no-forwarded"]),
        refused("agent-limit, not a number", &["agent-limit", "API", "--burst", "x"]),
        daemon("delivery", &["delivery", "API", "env", "--ceiling", "file"], ok(sec), 0),
        refused("delivery --env-name", &["delivery", "API", "env", "--env-name", "X"]),
        refused("delivery --file-name", &["delivery", "API", "file", "--file-name", "f"]),
        refused("delivery, no mode", &["delivery", "API"]),
        daemon("revoke --grant", &["revoke", "--grant", "g1"], ok(gone), 0),
        daemon("revoke --from", &["revoke", "API", "--from", "ws:a", "--scope", "server"], ok(r#"{}"#), 0),
        daemon("revoke --json", &["revoke", "API", "--from", "ws:a", "--json"], ok(gone), 0),
        daemon("open", &["open"], ok("{}"), 0),
        daemon("open --json", &["open", "--json"], ok(r#"{"ok":true}"#), 0),
        daemon("disable", &["disable", "API"], ok(gone), 0),
        daemon("enable", &["enable", "API"], ok("{}"), 0),
        refused("enable, no name", &["enable"]),
        daemon("destroy", &["destroy", "API", "--yes"], ok(gone), 0),
        daemon("destroy, refused", &["destroy", "API"], Reply::Answer(400, r#"{"error":{"code":"confirm","hint":"add --yes"}}"#.into()), 2),
        daemon("get", &["get", "API"], ok(r#"{"valueB64":"c2VjcmV0LXZhbHVlLTQyAA=="}"#), 0),
        daemon("get --json", &["get", "API", "--json"], ok(r#"{"valueB64":"eA=="}"#), 0),
        traceback(daemon("get, no value in it", &["get", "API"], ok("{}"), 1)),
        refused("get, no name", &["get"]),
    ] {
        rig.check(&c);
    }
}
