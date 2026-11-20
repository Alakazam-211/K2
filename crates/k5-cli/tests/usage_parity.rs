//! `k5-cli usage` against the shell CLI's `cli/k2 usage`, on the parity
//! rig ([`common::parity`]): the same requests, stdout, stderr and exit
//! code for every form.
//!
//! - The help (alone, `help`, `-h` anywhere), with no daemon set up,
//!   behind the virtual desktop's guard, in a session with no passport.
//! - The words and each verb's checks: a value flag with none, an unknown
//!   flag, an argument, `--here` (from the folder and from
//!   `K2_PROJECT_PATH`), the lists of `--harness`, `--tool`, `--by`,
//!   `--limit`, the flags a verb doesn't take, an unknown verb.
//! - `tokens` (groups, harnesses, days, costs, `--tz` and the computer's
//!   zone), `turns` (times by `TZ`, thousands of rows above 256 KiB),
//!   `limits` (times counted from now) and `limits --refresh` (a POST),
//!   each with `--json`; a reader's traceback.
//! - A refusal with a permission card (its closing line), 400, 404, 500;
//!   a passport over TCP and on the cell socket (a POST there is never
//!   sent again); no answer, and no daemon.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "usage";

fn reply(body: String) -> Reply {
    Reply::Answer(200, body)
}

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn waited(c: Case) -> Case {
    Case {
        compare: Compare::Waited,
        ..c
    }
}

fn tokens() -> String {
    let days: Vec<String> = (1..=16)
        .map(|i| {
            format!(
                r#"{{"day":"2026-11-{i:02}","requests":{i},"output_tokens":{}}}"#,
                i * 1_000_250
            )
        })
        .collect();
    format!(
        r#"{{"workspace":"acme é","tz":"Europe/Oslo","total":{{"requests":1234,"input_tokens":2500000000,"output_tokens":999,"cache_read_tokens":"1500","cache_write_tokens":true,"cost":{{"usd":12.345,"unpriced_requests":3}}}},
        "harnesses":[{{"harness":"claude","requests":5,"label":"Claude Code"}},{{"harness":"codex","input_tokens":1250,"cost":{{"usd":0.004}}}}],
        "days":[{}]}}"#,
        days.join(",")
    )
}

fn grouped() -> String {
    r#"{"tz":"UTC","total":{},"groups":[{"label":"a very long workspace label that goes on","requests":10},{"key":"k","requests":999999},{"requests":1000000}]}"#.into()
}

fn turns(n: usize) -> String {
    let rows: Vec<String> = (0..n)
        .map(|i| {
            format!(
                r#"{{"recorded_ms":{},"harness":"claude","model":"claude-model-{i}-with-a-long-name","workspace":"/Users/x/acme{}/","input_tokens":{i},"output_tokens":6,"cache_read_tokens":7,"cache_write_tokens":8,"cost_usd":{}}}"#,
                1_763_553_600_123u64 + (i as u64) * 61_000,
                i % 7,
                if i % 5 == 0 { "null".to_string() } else { format!("0.{i:04}") }
            )
        })
        .collect();
    format!(r#"{{"rows":[{}],"next_cursor":"c9"}}"#, rows.join(","))
}

/// `secs` since the epoch as `YYYY-MM-DDTHH:MM:SSZ`.
fn rfc3339(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let t = secs.rem_euclid(86400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        t / 3600,
        t % 3600 / 60,
        t % 60
    )
}

/// Times in the middle of their minute from now, so both sides read the
/// same counts.
fn limits() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let at = |mins: i64| rfc3339(now + mins * 60 + if mins < 0 { -30 } else { 30 });
    format!(
        r#"{{"airgap":true,"refresh":{{"retryAfterSecs":40,"outcome":"failed"}},"tools":[
 {{"tool":"claude","display":"Claude Code","supported":true,"logins":[
   {{"label":"me@example.com and a long tail","isDefault":true,"accountId":"a1","source":"live","checkedAt":"{}",
    "windows":[{{"label":"5h","usedPercent":42.5,"resetsAt":"{}"}},{{"label":"week","used":0.125,"resetsAt":"{}"}},{{"label":"x","usedPercent":3,"resetsAt":"{}"}}]}},
   {{"label":"b","accountId":"a2","state":"probe_failed","error":"boom","pinned":true,"checkedAt":"{}"}},
   {{"label":"c","state":"expired"}}]}},
 {{"tool":"codex","display":"Codex","supported":false,"logins":[{{"label":"c"}}]}},
 {{"tool":"grok","display":"Grok","supported":true,"logins":[]}}]}}"#,
        at(-30),
        at(89),
        at(3 * 1440 + 100),
        at(-5),
        at(-5 * 60 - 7)
    )
}

#[test]
fn usage_help_and_words() {
    let rig = Rig::new(VERB, "words");
    let mut v = Vec::new();
    for (name, args) in [
        ("the help: usage alone", &[][..]),
        ("help", &["help"]),
        ("-h", &["-h"]),
        ("--help after a flag", &["tokens", "--tz", "--help"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
    v.push(case(
        "the help with no daemon set up",
        &["limits", "-h"],
        &[],
        vec![],
        nothing,
        0,
        0,
    ));
    v.push(case(
        "the help behind the guard",
        &["help"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    v.push(case(
        "the help in a session with no passport",
        &[],
        &[("K2_HOOK_SOCK", "{sock}")],
        vec![],
        owner_and_port,
        3,
        0,
    ));
    for (name, args) in [
        ("a value flag at the end", &["tokens", "--since"][..]),
        ("an empty value", &["tokens", "--model", ""]),
        ("an unknown flag", &["turns", "--bogus"]),
        ("an argument", &["tokens", "acme"]),
        (
            "--here and --workspace",
            &["tokens", "--here", "--workspace", "w"],
        ),
        (
            "--workspace and --outside",
            &["tokens", "--workspace", "w", "--outside"],
        ),
        ("a harness", &["tokens", "--harness", "pi"]),
        ("a tool", &["limits", "--tool", "pi"]),
        ("a grouping", &["tokens", "--by", "day"]),
        ("a limit of 0", &["turns", "--limit", "0"]),
        ("a limit of 1000", &["turns", "--limit", "1000"]),
        ("tokens --refresh", &["tokens", "--refresh"]),
        ("turns --tz", &["turns", "--tz", "UTC"]),
        ("turns --outside", &["turns", "--outside"]),
        ("limits --here", &["limits", "--here"]),
        ("an unknown verb", &["nope"]),
    ] {
        v.push(case(name, args, &[], vec![], owner_and_port, 2, 0));
    }
    v.push(case(
        "a usage error with no daemon set up",
        &["nope"],
        &[],
        vec![],
        nothing,
        1,
        0,
    ));
    v.push(case(
        "the guard",
        &["limits"],
        &[("K2_NO_OWNER_FALLBACK", "1")],
        vec![],
        port_only,
        3,
        0,
    ));
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn usage_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "verbs");
    let tz = [("TZ", "America/Denver")];
    let v = vec![
        // covers: usage tokens
        case(
            "tokens",
            &["tokens"],
            &tz,
            vec![reply(tokens())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tokens, the computer's zone",
            &[
                "tokens",
                "--harness",
                "claude",
                "--login",
                "a b",
                "--model",
                "m",
                "--since",
                "7d",
                "--until",
                "now",
            ],
            &[],
            vec![reply(tokens())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tokens --here, by workspace",
            &["tokens", "--here", "--by", "workspace", "--tz", "UTC"],
            &[],
            vec![reply(grouped())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tokens --outside",
            &["tokens", "--outside", "--tz", "UTC"],
            &[("K2_PROJECT_PATH", "/srv/acme")],
            vec![ok(r#"{"total":null,"groups":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "tokens --here with K2_PROJECT_PATH",
            &["tokens", "--here", "--tz", "UTC"],
            &[("K2_PROJECT_PATH", "/srv/acme é")],
            vec![ok(r#"{}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: usage tokens --json
        case(
            "tokens --json",
            &["tokens", "--json", "--tz", "UTC"],
            &[],
            vec![ok("{\"x\":1}\n")],
            owner_and_port,
            0,
            1,
        ),
        // covers: usage turns
        case(
            "turns",
            &["turns", "--limit", "20", "--before", "c 1"],
            &tz,
            vec![reply(turns(3))],
            owner_and_port,
            0,
            1,
        ),
        case(
            "turns, none",
            &["turns"],
            &tz,
            vec![ok(r#"{"rows":[]}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "turns, thousands of rows",
            &["turns"],
            &[("TZ", "UTC")],
            vec![reply(turns(3000))],
            owner_and_port,
            0,
            1,
        ),
        // covers: usage turns --json
        case(
            "turns --json, thousands of rows",
            &["turns", "--json"],
            &[],
            vec![reply(turns(3000))],
            owner_and_port,
            0,
            1,
        ),
        // covers: usage limits
        case(
            "limits",
            &["limits"],
            &[],
            vec![reply(limits())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "limits --tool --login",
            &["limits", "--tool", "claude", "--login", "a2"],
            &[],
            vec![reply(limits())],
            owner_and_port,
            0,
            1,
        ),
        // covers: usage limits --json
        case(
            "limits --json",
            &["limits", "--json"],
            &[],
            vec![reply(limits())],
            owner_and_port,
            0,
            1,
        ),
        case(
            "limits --refresh: a POST",
            &["limits", "--refresh", "--tool", "codex", "--login", "é"],
            &[],
            vec![reply(limits())],
            owner_and_port,
            0,
            1,
        ),
        traceback(case(
            "tokens, not JSON",
            &["tokens", "--tz", "UTC"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "tokens, a count that is no number",
            &["tokens", "--tz", "UTC"],
            &[],
            vec![ok(r#"{"total":{"requests":"x1"}}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "limits, a time with no zone",
            &["limits"],
            &[],
            vec![ok(
                r#"{"tools":[{"supported":true,"display":"C","logins":[{"checkedAt":"2026-11-19T11:00:00"}]}]}"#,
            )],
            owner_and_port,
            1,
            1,
        )),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn usage_refusals_and_channels() {
    let rig = Rig::new(VERB, "refuse");
    let cell = || with_passport(&[("K2_HOOK_SOCK", "{sock}")]);
    let v = vec![
        case(
            "403 with a permission card: its closing line",
            &["limits"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "400",
            &["turns"],
            &[],
            vec![Reply::Answer(
                400,
                r#"{"error":{"code":"bad_since","hint":"say 7d"}}"#.into(),
            )],
            owner_and_port,
            2,
            1,
        ),
        case(
            "404",
            &["limits"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"nope"}"#.into())],
            owner_and_port,
            4,
            1,
        ),
        case(
            "500, not JSON",
            &["tokens", "--tz", "UTC"],
            &[],
            vec![Reply::Answer(500, "boom".into())],
            owner_and_port,
            1,
            1,
        ),
        with(
            with_passport(&[]),
            case(
                "a passport over TCP",
                &["tokens", "--tz", "UTC"],
                &[],
                vec![reply(grouped())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a refused passport: the probe and its line",
                &["limits"],
                &[],
                vec![
                    Reply::Answer(403, REFUSED.into()),
                    Reply::Answer(403, REFUSED.into()),
                ],
                port_only,
                3,
                2,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket",
                &["limits"],
                &[],
                vec![reply(limits())],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "no answer there: a GET goes on to TCP",
                &["tokens", "--tz", "UTC"],
                &[],
                vec![Reply::Close, reply(grouped())],
                port_only,
                0,
                2,
            ),
        ),
        waited(with(
            cell(),
            case(
                "no answer there: the refresh is never sent again",
                &["limits", "--refresh"],
                &[],
                vec![Reply::Close],
                port_only,
                1,
                1,
            ),
        )),
        case(
            "an empty reply",
            &["turns"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["turns"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
    let port = dead_port();
    rig.check(&case(
        "nothing listens: each try's 000",
        &["limits"],
        &[("K2_PORT", port.as_str())],
        vec![],
        owner_only,
        1,
        0,
    ));
}

/// Help never acts: `limits --refresh` (the one verb that asks the daemon
/// to do something) with `-h` or `--help` anywhere prints the help and
/// sends nothing.
#[test]
fn usage_help_never_acts() {
    let rig = Rig::new(VERB, "helpacts");
    for (name, args) in [
        ("-h first", &["limits", "-h", "--refresh"][..]),
        (
            "--help last",
            &["limits", "--refresh", "--tool", "codex", "--help"],
        ),
        (
            "-h as a flag's value",
            &["limits", "--refresh", "--login", "-h"],
        ),
    ] {
        rig.check(&case(name, args, &[], vec![], owner_and_port, 0, 0));
    }
}
