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
    // covers: vault status
    let mut session = daemon("in a session: the cell socket", &["status"], ok(STATUS), 0);
    session.env = vec![
        ("K2_HOOK_SOCK", "{sock}".to_string()),
        ("K2_HOOK_TOKEN", PASSPORT.to_string()),
    ];
    for c in [
        daemon("status", &["status"], ok(STATUS), 0),
        daemon(
            "status, little in it",
            &["status"],
            ok(r#"{"proxy":{"hint":"off"},"tamper":"ok"}"#),
            0,
        ),
        daemon(
            "closed",
            &["status"],
            ok(r#"{"open":false,"error":{"code":"locked","hint":"unlock"}}"#),
            0,
        ),
        // covers: vault status --json
        daemon("--json", &["status", "--json"], ok(STATUS), 0),
        daemon(
            "a permission card",
            &["status"],
            Reply::Answer(403, PERMISSION_ASK.into()),
            3,
        ),
        daemon(
            "not found",
            &["status"],
            Reply::Answer(404, r#"{"error":{"code":"nope","hint":"h"}}"#.into()),
            4,
        ),
        daemon(
            "a 410",
            &["status"],
            Reply::Answer(410, r#"{"error":"card_dismissed"}"#.into()),
            5,
        ),
        daemon(
            "a 422",
            &["status"],
            Reply::Answer(422, "not json".into()),
            2,
        ),
        daemon(
            "an empty 500",
            &["status"],
            Reply::Answer(500, String::new()),
            1,
        ),
        daemon("an empty reply", &["status"], Reply::Close, 1),
        traceback(daemon(
            "a 200 Python can't read",
            &["status"],
            ok("nope"),
            1,
        )),
        traceback(daemon("a 200 that is a list", &["status"], ok("[1]"), 1)),
        refused(
            "a flag status doesn't take",
            &["status", "--scope", "server"],
        ),
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
        // covers: vault list
        daemon("list", &["list"], ok(list), 0),
        daemon("ls, none", &["ls"], ok(r#"{"secrets":[]}"#), 0),
        daemon(
            "list, none of yours",
            &["list", "--system"],
            ok(r#"{"systemHidden":{"count":1,"why":"w"}}"#),
            0,
        ),
        daemon(
            "list --scope ws:a b",
            &["list", "--scope", "ws:a b&c", "--system"],
            ok(list),
            0,
        ),
        daemon("list: above 256 KiB", &["list"], ok(&big), 0),
        refused("a scope that isn't one", &["list", "--scope", "galaxy"]),
        // covers: vault show
        refused("--system on show", &["show", "A", "--system"]),
        daemon("show", &["show", "API", "--scope", "server"], ok(SECRET), 0),
        daemon(
            "show, an ovh signer",
            &["show", "B"],
            ok(r#"{"secret":{"name":"B","inject":{"sign":{"scheme":"ovh"}}}}"#),
            0,
        ),
        daemon(
            "show, oauth",
            &["show", "B"],
            ok(
                r#"{"secret":{"name":"B","inject":{"sign":{"scheme":"oauth-refresh","tokenUrl":"https://t"}},"oauth":{"expiresAt":"t5"}}}"#,
            ),
            0,
        ),
        daemon(
            "show, hmac and no sites",
            &["show", "B"],
            ok(
                r#"{"secret":{"name":"B","inject":{"sign":{"scheme":"hmac-sha256","header":"X"}}}}"#,
            ),
            0,
        ),
        daemon(
            "show, injected",
            &["show", "B"],
            ok(r#"{"secret":{"inject":{"dst":"header:A","fmt":"Bearer {}"}}}"#),
            0,
        ),
        // covers: vault show --json
        daemon("show --json", &["show", "B", "--json"], ok(SECRET), 0),
        refused("show with no name", &["show"]),
        daemon(
            "public-key",
            // covers: vault public-key
            &["public-key", "K"],
            ok(r#"{"publicKey":"ssh-ed25519 AAAA","fingerprint":"SHA256:x"}"#),
            0,
        ),
        traceback(daemon(
            "public-key, none in it",
            &["public-key", "K"],
            ok(r#"{}"#),
            1,
        )),
        daemon(
            "audit",
            // covers: vault audit
            &["audit", "API", "--since", "2d ago"],
            ok(
                r#"{"audit":[{"ts":"t","action":"fetch","outcome":"ok","actor":"me","targetHost":"h","count":2},{"ts":"t2","action":"get","outcome":"refused","actor":"x"}]}"#,
            ),
            0,
        ),
        daemon("audit, all", &["audit"], ok(r#"{"audit":[]}"#), 0),
        daemon(
            "requests",
            // covers: vault requests
            &["requests"],
            ok(
                r#"{"requests":[{"createdAt":"t","status":"open","name":"N","requestedBy":"ws","inject":{"dst":"header:X","fmt":"{}"}},{"name":"M"}]}"#,
            ),
            0,
        ),
        daemon("requests, none", &["requests"], ok("{}"), 0),
        daemon(
            "presets",
            // covers: vault presets
            &["presets"],
            ok(
                r#"{"presets":[{"id":"gh","label":"GitHub","hosts":["api.github.com"],"dst":"header:Authorization","fmt":"Bearer {}"},{"id":"ovh","label":"OVH","hosts":[],"sign":"ovh"}]}"#,
            ),
            0,
        ),
        daemon(
            "doctor, findings",
            // covers: vault doctor
            &["doctor", "--transcripts"],
            ok(
                r#"{"findings":[{"level":"warn","message":"m","files":["a","b","c","d","e","f"],"fileCount":9},{"level":"info","message":"n"}],"checkedNames":"A B","summary":"s","ok":false}"#,
            ),
            1,
        ),
        daemon("doctor, nothing", &["doctor"], ok(r#"{"ok":true}"#), 0),
        refused("--transcripts elsewhere", &["status", "--transcripts"]),
        daemon(
            "recipient",
            // covers: vault recipient
            &["recipient"],
            ok(r#"{"recipient":"age1xyz","vaultId":"v1"}"#),
            0,
        ),
        daemon(
            "proxy-url",
            // covers: vault proxy-url
            &["proxy-url", "API", "--ttl", "2h", "--scope", "session"],
            ok(
                r#"{"envName":"API_KEY","placeholder":"k2p_x","baseUrlEnv":"API_BASE","baseUrl":"http://127.0.0.1:1/x"}"#,
            ),
            0,
        ),
        daemon(
            "proxy-url --secret",
            &["proxy-url", "--secret", "API"],
            ok(r#"{"envName":"A","placeholder":"p","baseUrlEnv":"B","baseUrl":"u"}"#),
            0,
        ),
        refused(
            "proxy-url, two",
            &["proxy-url", "--secret", "A", "--secret", "B"],
        ),
    ] {
        rig.check(&c);
    }
}

#[test]
fn set_and_rotate_take_the_value_on_stdin() {
    let rig = VRig::new();
    let saved = r#"{"created":true,"ref":"k2v:API","secret":{"name":"API","latestVersion":1,"inject":{"sign":{"scheme":"hmac-sha256"}}},"trimmed":true,"use":"k2 vault fetch","ttlCut":true}"#;
    for c in [
        with_stdin(
            daemon(
                "set",
                // covers: vault set
                &["set", "API", "--for", "a.com, b.com,", "--kind", "token"],
                ok(saved),
                0,
            ),
            b"hunter2-value\n",
            &["hunter2-value", "aHVudGVyMi12YWx1ZQ=="],
        ),
        with_stdin(
            daemon(
                "set, two newlines kept",
                &["set", "API"],
                ok(r#"{"secret":{"name":"API"}}"#),
                0,
            ),
            b"multi\nline\n",
            &["multi\nline"],
        ),
        with_stdin(
            daemon(
                "set, signed",
                &[
                    "set",
                    "API",
                    "--sign",
                    "hmac-sha256-ts",
                    "--sign-prefix",
                    "",
                    "--sign-template",
                    "{ts}",
                    "--sign-header",
                    "X",
                    "--json",
                ],
                ok(saved),
                0,
            ),
            b"s3cr3t-signing-key",
            &["s3cr3t-signing-key"],
        ),
        with_stdin(
            daemon(
                "set, injected",
                &[
                    "set",
                    "API",
                    "--inject-header",
                    "X-Key",
                    "--inject-format",
                    "k {}",
                    "--ttl",
                    "2h",
                    "--rotate-every",
                    "30",
                    "--describe",
                    "d é",
                ],
                ok(saved),
                0,
            ),
            b"v",
            &[],
        ),
        with_stdin(refused("set, no value", &["set", "API"]), b"", &[]),
        with_stdin(refused("set, only a newline", &["set", "API"]), b"\n", &[]),
        refused("set, a value on argv", &["set", "API", "hunter2"]),
        refused("set --value=", &["set", "API", "--value=hunter2"]),
        refused(
            "set, signed and injected",
            &["set", "API", "--sign", "ovh", "--inject-dst", "header:X"],
        ),
        refused(
            "set, header and dst",
            &[
                "set",
                "API",
                "--inject-header",
                "X",
                "--inject-dst",
                "header:Y",
            ],
        ),
        refused(
            "set, signer options with no signer",
            &["set", "API", "--sign-header", "X"],
        ),
        traceback(with_stdin(
            refused(
                "set, --rotate-every not a number",
                &["set", "API", "--rotate-every", "soon"],
            ),
            b"v",
            &[],
        )),
        refused("set, no name", &["set"]),
        with_stdin(
            daemon(
                "set, refused",
                &["set", "API"],
                Reply::Answer(403, PERMISSION_ASK.into()),
                3,
            ),
            b"pw-refused",
            &["pw-refused"],
        ),
        with_stdin(
            daemon(
                "rotate",
                // covers: vault rotate
                &["rotate", "API"],
                ok(
                    r#"{"secret":{"name":"API"},"version":3,"replaced":[{"ok":true,"unit":"u","credName":"c","restarted":true},{"unit":"v","error":{"hint":"h"}}]}"#,
                ),
                0,
            ),
            b"new-value-9",
            &["new-value-9"],
        ),
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
        daemon(
            "hosts",
            &[
                "hosts", "API", "--add", "a.com", "--add", "b.com", "--remove", "c.com",
            ],
            ok(sec),
            0,
        ),
        daemon(
            "hosts, none left",
            // covers: vault hosts
            &["hosts", "API", "--remove", "a"],
            ok(r#"{"secret":{"name":"API"}}"#),
            0,
        ),
        refused("hosts, no name", &["hosts"]),
        daemon(
            "sign none",
            // covers: vault sign
            &["sign", "API", "none"],
            ok(r#"{"secret":{"name":"API"}}"#),
            0,
        ),
        daemon("sign ovh", &["sign", "API", "ovh"], ok(sec), 0),
        daemon(
            "sign hmac",
            &[
                "sign",
                "API",
                "hmac-sha256",
                "--sign-header",
                "X",
                "--sign-encoding",
                "base64",
                "--sign-prefix",
                "p=",
            ],
            ok(sec),
            0,
        ),
        refused(
            "sign none with options",
            &["sign", "API", "none", "--sign-value", ""],
        ),
        refused("sign with --sign", &["sign", "API", "ovh", "--sign", "ovh"]),
        refused("sign, no scheme", &["sign", "API"]),
        daemon(
            "agent-limit",
            &[
                "agent-limit",
                "API",
                "--burst",
                "05",
                "--per-minute",
                "60",
                "--no-forwarded",
            ],
            ok(sec),
            0,
        ),
        refused(
            "agent-limit, both",
            // covers: vault agent-limit
            &["agent-limit", "API", "--allow-forwarded", "--no-forwarded"],
        ),
        refused(
            "agent-limit, not a number",
            &["agent-limit", "API", "--burst", "x"],
        ),
        daemon(
            "delivery",
            // covers: vault delivery
            &["delivery", "API", "env", "--ceiling", "file"],
            ok(sec),
            0,
        ),
        refused(
            "delivery --env-name",
            &["delivery", "API", "env", "--env-name", "X"],
        ),
        refused(
            "delivery --file-name",
            &["delivery", "API", "file", "--file-name", "f"],
        ),
        refused("delivery, no mode", &["delivery", "API"]),
        // covers: vault revoke
        daemon("revoke --grant", &["revoke", "--grant", "g1"], ok(gone), 0),
        daemon(
            "revoke --from",
            &["revoke", "API", "--from", "ws:a", "--scope", "server"],
            ok(r#"{}"#),
            0,
        ),
        daemon(
            "revoke --json",
            // covers: vault revoke --json
            &["revoke", "API", "--from", "ws:a", "--json"],
            ok(gone),
            0,
        ),
        // covers: vault open
        daemon("open", &["open"], ok("{}"), 0),
        // covers: vault open --json
        daemon("open --json", &["open", "--json"], ok(r#"{"ok":true}"#), 0),
        // covers: vault disable
        daemon("disable", &["disable", "API"], ok(gone), 0),
        // covers: vault enable
        daemon("enable", &["enable", "API"], ok("{}"), 0),
        refused("enable, no name", &["enable"]),
        // covers: vault destroy
        daemon("destroy", &["destroy", "API", "--yes"], ok(gone), 0),
        daemon(
            "destroy, refused",
            &["destroy", "API"],
            Reply::Answer(
                400,
                r#"{"error":{"code":"confirm","hint":"add --yes"}}"#.into(),
            ),
            2,
        ),
        daemon(
            "get",
            // covers: vault get
            &["get", "API"],
            ok(r#"{"valueB64":"c2VjcmV0LXZhbHVlLTQyAA=="}"#),
            0,
        ),
        daemon(
            "get --json",
            // covers: vault get --json
            &["get", "API", "--json"],
            ok(r#"{"valueB64":"eA=="}"#),
            0,
        ),
        traceback(daemon("get, no value in it", &["get", "API"], ok("{}"), 1)),
        refused("get, no name", &["get"]),
    ] {
        rig.check(&c);
    }
}

#[test]
fn file_env_file_grant_and_service() {
    let rig = VRig::new();
    let waiting = r#"{"waiting":true,"card":"c1","waitSecs":0}"#;
    let mut confirm = daemon(
        "grant: the confirm over the vault socket",
        // covers: vault grant
        &["grant", "API", "--to", "ws:a", "--as", "env"],
        Reply::Answer(
            403,
            r#"{"error":{"code":"vault_confirm_required","hint":"confirm"}}"#.into(),
        ),
        0,
    );
    confirm.vault = Some(vec![
        ok(r#"{"secret":{"id":"sec-1"}}"#),
        ok(r#"{"methods":["vault-password"]}"#),
        ok(r#"{"token":"ctok-7788"}"#),
        ok(r#"{"grant":{"id":"g9"}}"#),
    ]);
    confirm.vault_requests = 4;
    confirm.stdin = b"vault-pw-5150\n".to_vec();
    confirm.secrets = vec!["vault-pw-5150", "ctok-7788"];
    let mut no_method = daemon(
        "grant: no confirm method",
        &["grant", "API", "--to", "ws:a"],
        Reply::Answer(403, r#"{"error":{"code":"vault_confirm_required"}}"#.into()),
        3,
    );
    no_method.vault = Some(vec![
        ok(r#"{"secret":{"id":"s"}}"#),
        ok(r#"{"methods":[]}"#),
    ]);
    no_method.vault_requests = 2;
    let mut in_session = daemon(
        "grant: a K2 terminal never confirms",
        &["grant", "API", "--to", "ws:a"],
        Reply::Answer(
            403,
            r#"{"error":{"code":"vault_confirm_required","hint":"ask a person"}}"#.into(),
        ),
        3,
    );
    in_session.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    for c in [
        daemon(
            "file",
            // covers: vault file
            &["file", "API", "--ttl", "90s"],
            ok(r#"{"path":"/s/API"}"#),
            0,
        ),
        daemon(
            "file --done",
            &["file", "--done", "/s/API"],
            ok(r#"{"removed":true}"#),
            0,
        ),
        daemon(
            "file: no answer yet",
            &["file", "API", "--wait", "1m"],
            ok(waiting),
            3,
        ),
        daemon(
            "file: an empty card",
            &["file", "API"],
            ok(r#"{"waiting":true,"card":"","waitSecs":30}"#),
            3,
        ),
        refused(
            "file, two secrets",
            &["file", "--secret", "A", "--secret", "B"],
        ),
        refused("file, nothing named", &["file"]),
        refused(
            "file, --wait and --done",
            &["file", "--done", "/x", "--wait", "1m"],
        ),
        daemon(
            "env-file",
            // covers: vault env-file
            &["env-file", "A", "B", "--secret", "C", "--scope", "server"],
            ok(r#"{"path":"/s/.env"}"#),
            0,
        ),
        refused("env-file, nothing named", &["env-file"]),
        daemon(
            "grant",
            &["grant", "API", "--to", "ws:a", "--ttl", "2h"],
            ok(
                r#"{"grant":{"id":"g1","expiresAt":"t"},"reach":"they get it now","warning":"another server's node"}"#,
            ),
            0,
        ),
        daemon(
            "grant: an old server drops --ttl",
            &["grant", "API", "--to", "ws:a", "--ttl", "2h"],
            ok(r#"{"grant":{"id":"g1"}}"#),
            1,
        ),
        daemon(
            "grant --json",
            // covers: vault grant --json
            &["grant", "API", "--to", "ws:a", "--json"],
            ok(r#"{"grant":{}}"#),
            0,
        ),
        refused("grant, no --to", &["grant", "API"]),
        confirm,
        no_method,
        in_session,
        daemon(
            "service",
            // covers: vault service
            &["service", "web"],
            ok(
                r#"{"service":"web","mode":"legacy","envFile":".env.k2","secrets":[{"name":"A","delivery":"env","envName":"A_KEY"}],"inheritedNames":["PATH","HOME"],"envRemovedIn":"0.47","restart":"restart web"}"#,
            ),
            0,
        ),
        daemon(
            "service clean",
            &["service", "web", "clean", "--env-file", ".env.k2"],
            ok(r#"{"service":"web","mode":"clean"}"#),
            0,
        ),
        refused("service, no name", &["service"]),
        daemon(
            "sync",
            // covers: vault sync
            &["sync"],
            ok(
                r#"{"dir":"/s","delivered":["a"],"present":["b"],"restart":["c"],"skipped":["d"],"hint":"h"}"#,
            ),
            1,
        ),
        // covers: vault sync --json
        daemon("sync --json", &["sync", "--json"], ok("{}"), 0),
        refused("sync, a word", &["sync", "now"]),
        daemon(
            "placements",
            // covers: vault placements
            &["placements", "API", "--scope", "server"],
            ok(
                r#"{"placements":[{"id":"p1","name":"API","version":1,"latestVersion":2,"state":"due","where":"h:/x","note":"n"},{"id":"p2","target":"systemd_cred","credName":"c","unit":"u"}],"needNewerK2":2}"#,
            ),
            0,
        ),
        daemon("placement list, none", &["placement"], ok("{}"), 0),
        daemon(
            "placement add",
            &[
                "placement",
                "add",
                "API",
                "--where",
                "h:/etc/x",
                "--note",
                "n",
            ],
            ok(r#"{"placement":{"name":"API","where":"h:/etc/x","version":2,"id":"p3"}}"#),
            0,
        ),
        daemon(
            "placement done",
            // covers: vault placement done
            &["placement", "done", "p3"],
            ok(r#"{"placement":{"name":"API","where":"w","version":3}}"#),
            0,
        ),
        // covers: vault placement rm
        daemon("placement rm", &["placement", "rm", "p3"], ok("{}"), 0),
        // covers: vault placement add
        refused("placement add, no --where", &["placement", "add", "API"]),
        refused(
            "placement done, a flag it doesn't take",
            &["placement", "done", "p", "--note", "x"],
        ),
        refused(
            "placements --scope, no name",
            &["placements", "--scope", "server"],
        ),
    ] {
        rig.check(&c);
    }
}

#[test]
fn fetch() {
    let rig = VRig::new();
    for c in [
        daemon(
            "fetch",
            // covers: vault fetch
            &["fetch", "--secret", "API", "https://api.example.com/x"],
            ok(r#"{"status":200,"body":"{\"ok\":true}\n"}"#),
            0,
        ),
        daemon(
            "fetch, everything",
            &[
                "fetch",
                "-s",
                "API",
                "--scope",
                "ws:proj",
                "-X",
                "POST",
                "-H",
                "Accept: a/b",
                "--json",
                "{\"a\":1}",
                "--max-bytes",
                "100",
                "--timeout",
                "9",
                "-i",
                "--dst",
                "header:X",
                "--fmt",
                "Bearer {}",
                "https://h/x",
            ],
            ok(r#"{"status":404,"headers":[["A","b"]],"body":"nope"}"#),
            0,
        ),
        daemon(
            "fetch, an error status",
            &["fetch", "-s", "API", "https://h/x", "-d", "a=1"],
            ok(r#"{"status":503,"bodyB64":"/wBoaQ==","truncated":true}"#),
            0,
        ),
        daemon(
            "fetch -o",
            &["fetch", "-s", "API", "https://h/x", "-o", "out.bin"],
            ok(r#"{"status":200,"out":"/x/out.bin","bytes":12}"#),
            0,
        ),
        daemon(
            "fetch --json-out",
            &["fetch", "-s", "API", "https://h/x", "--json-out"],
            ok(r#"{"status":200}"#),
            0,
        ),
        daemon(
            "fetch, refused",
            &["fetch", "-s", "API", "https://h/x"],
            Reply::Answer(403, PERMISSION_ASK.into()),
            3,
        ),
        refused("fetch, no URL", &["fetch", "-s", "API"]),
        refused("fetch, no secret", &["fetch", "https://h/x"]),
        refused("fetch, two URLs", &["fetch", "-s", "A", "u1", "u2"]),
        refused(
            "fetch, a bad header",
            &["fetch", "-s", "A", "u", "-H", "nocolon"],
        ),
        refused(
            "fetch, a bad scope",
            &["fetch", "-s", "A", "u", "--scope", "ws:"],
        ),
        refused("fetch, an unknown flag", &["fetch", "-s", "A", "u", "-q"]),
        traceback(refused(
            "fetch, --max-bytes not a number",
            &["fetch", "-s", "A", "u", "--max-bytes", "lots"],
        )),
    ] {
        rig.check(&c);
    }
}

/// A person's verb: the vault socket answers, the password on stdin.
fn person(
    name: &'static str,
    args: &[&'static str],
    replies: Vec<Reply>,
    stdin: &[u8],
    code: i32,
) -> VCase {
    let mut c = VCase::new(name, args);
    c.vault_requests = replies.len();
    c.vault = Some(replies);
    c.stdin = stdin.to_vec();
    c.code = code;
    c
}

#[test]
fn the_person_verbs() {
    let rig = VRig::new();
    let pw = |m: &str| {
        ok(&format!(
            r#"{{"methods":["{m}"],"confirmText":"rotate the key"}}"#
        ))
    };
    let mut reveal = person(
        "reveal",
        // covers: vault reveal
        &["reveal", "API", "--scope", "server"],
        vec![
            ok(r#"{"secret":{"id":"sec-9"}}"#),
            pw("vault-password"),
            ok(r#"{"token":"tk-4242"}"#),
            ok(r#"{"valueB64":"cmV2ZWFsZWQtdmFsdWU="}"#),
        ],
        b"pw-reveal-1\n",
        0,
    );
    reveal.secrets = vec!["pw-reveal-1", "tk-4242", "revealed-value"];
    let mut in_k2 = refused("reveal in a K2 terminal", &["reveal", "API"]);
    in_k2.env = vec![("K2_HOOK_TOKEN", PASSPORT.to_string())];
    in_k2.code = 3;
    // covers: vault rekey
    let mut no_sock = refused("rekey with no vault socket", &["rekey"]);
    no_sock.vault = None;
    no_sock.code = 1;
    for c in [
        reveal,
        person(
            "reveal: the secret isn't there",
            &["reveal", "API"],
            vec![Reply::Answer(
                404,
                r#"{"error":{"code":"not_found","hint":"no API"}}"#.into(),
            )],
            b"",
            4,
        ),
        person(
            "reveal: a wrong password",
            &["reveal", "API"],
            vec![
                ok(r#"{"secret":{"id":"s"}}"#),
                pw("vault-password"),
                Reply::Answer(
                    403,
                    r#"{"error":{"code":"bad_password","hint":"try again"}}"#.into(),
                ),
            ],
            b"wrong\n",
            3,
        ),
        person(
            "reveal: Touch ID",
            &["reveal", "API"],
            vec![
                ok(r#"{"secret":{"id":"s"}}"#),
                pw("local-auth"),
                ok(r#"{"token":"t"}"#),
                ok(r#"{"valueB64":"eA=="}"#),
            ],
            b"",
            0,
        ),
        person(
            "reveal: an owner login",
            &["reveal", "API"],
            vec![
                ok(r#"{"secret":{"id":"s"}}"#),
                pw("owner-login-password"),
                ok(r#"{"token":"t"}"#),
                ok(r#"{"valueB64":"eA=="}"#),
            ],
            b"amy\npw-login-77\n",
            0,
        ),
        person(
            "kek rotate",
            // covers: vault kek rotate
            &["kek", "rotate"],
            vec![pw("vault-password"), ok(r#"{"token":"t"}"#), ok("{}")],
            b"pw\n",
            0,
        ),
        person(
            "rekey: this terminal",
            &["rekey"],
            vec![pw("terminal"), ok(r#"{"token":"t"}"#), ok("{}")],
            b"",
            0,
        ),
        person(
            "rekey: no method",
            &["rekey"],
            vec![ok(r#"{"methods":[]}"#)],
            b"",
            3,
        ),
        person(
            "rekey: confirm/start refused",
            &["rekey"],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad","hint":"h"}}"#.into(),
            )],
            b"",
            3,
        ),
        refused("kek, no rotate", &["kek"]),
        refused("rekey takes no flags", &["rekey", "--json"]),
        in_k2,
        no_sock,
        daemon(
            "recovery",
            // covers: vault recovery
            &["recovery"],
            ok(r#"{"recovery":"confirmed","recoveryConfirmedAt":"t","recoveryHint":"h"}"#),
            0,
        ),
        daemon(
            "recovery status",
            &["recovery", "status"],
            ok(r#"{"recovery":"none"}"#),
            0,
        ),
        refused("recovery, an unknown word", &["recovery", "nope"]),
    ] {
        rig.check(&c);
    }
}

#[test]
fn confirm_password_set() {
    let rig = VRig::new();
    let mut set = person(
        "confirm-password set, the first",
        // covers: vault confirm-password set
        &["confirm-password", "set"],
        vec![
            ok(r#"{"confirmPassword":false}"#),
            ok(r#"{"methods":["local-auth"],"confirmText":"set a vault password"}"#),
            ok(r#"{"token":"tk-1"}"#),
            ok("{}"),
        ],
        b"new-password-123\nnew-password-123\n",
        0,
    );
    set.secrets = vec!["new-password-123", "tk-1"];
    for c in [
        set,
        person(
            "confirm-password set, a change",
            &["confirm-password", "set"],
            vec![
                ok(r#"{"confirmPassword":true}"#),
                ok(r#"{"methods":["vault-password"]}"#),
                ok(r#"{"token":"t"}"#),
                ok("{}"),
            ],
            b"old-pw\nnew-pw-1234\nnew-pw-1234\n",
            0,
        ),
        person(
            "confirm-password set, they differ",
            &["confirm-password", "set"],
            vec![
                ok("{}"),
                ok(r#"{"methods":["terminal"]}"#),
                ok(r#"{"token":"t"}"#),
            ],
            b"aaaaaaaaaa\nbbbbbbbbbb\n",
            2,
        ),
        person(
            "confirm-password set, refused",
            &["confirm-password", "set"],
            vec![
                ok("{}"),
                ok(r#"{"methods":["terminal"]}"#),
                ok(r#"{"token":"t"}"#),
                Reply::Answer(422, r#"{"error":{"code":"too_short","hint":"10+"}}"#.into()),
            ],
            b"short\nshort\n",
            2,
        ),
        refused("confirm-password, no set", &["confirm-password"]),
    ] {
        rig.check(&c);
    }
}
