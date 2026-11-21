//! `k5-cli heartbeat` against the shell CLI's `cli/k2 heartbeat`, on the
//! parity rig ([`common::parity`]): the same requests, stdout, stderr and
//! exit code.
//!
//! - The lists: `heartbeat` alone, `schedule`, `list` (`--json`,
//!   `--workspace`, `--archived`), `list-archived`; every column (zones,
//!   pauses, waits, auto-off rows), none, the daemon's errors, a card,
//!   odd values, one large list (above 256 KiB).
//! - `show`, `status`, `log`, `session` (read and set), `restart`,
//!   `enable` and `disable`, `remove` (`archive`, `--purge`),
//!   `unarchive`, `rename`, `use-pinned-session`, `signal fire` and
//!   `signal wakeup` (`--path-only`, the file there and not), `wakeup
//!   --edit` (`EDITOR`, else vi: stand-in editors), `add` and
//!   `edit` (the spec, `--tz`, the instructions), `remind` and
//!   `reminders` (the remind family's).
//! - The stops: usage lines, unknown verbs, the retired verbs, the spec's
//!   checks, `help`.
//! - The passport on the cell socket, no answer, the gates.
//! - Help never acts: `-h` or `--help` on a line that would change
//!   something prints help and sends nothing.

#![cfg(unix)]

mod common;

use common::parity::*;
use common::stub::Reply;

const VERB: &str = "heartbeat";

const ROWS: &str = r#"[{"name":"daily","frequency":"daily","specJson":"{\"time\":\"07:00\"}","enabled":true,"lastFired":"2026-11-19T07:00:01.123Z","nextFireAt":"2026-11-20T07:00:00Z","tz":"America/New_York","zoneLabel":"America/New_York"},{"name":"hourly-check","frequency":"hourly","specJson":"{\"every_seconds\":1800,\"start\":\"09:00\",\"end\":\"17:00\"}","enabled":true,"pausedUntil":"2026-11-21T00:00:00+00:00","waitReason":"busy","waitDetail":"a turn","nextFireAt":"2026-11-20T09:30:00+02:00"},{"name":"weekly","frequency":"weekly","specJson":"{\"days\":[\"mon\",\"fri\"],\"time\":\"9:30\"}","enabled":false,"disabledReason":"failures","waitSince":"2026-11-18T10:00:00Z","waitReason":"wakeup_empty","useWorkspaceSession":true},{"name":"monthly","frequency":"monthly","specJson":"{\"days_of_month\":[1,15],\"months\":[\"jan\",\"jul\"]}","enabled":false,"disabledReason":"custom","waitSince":"soon","waitReason":"schedule_error","scheduleError":"bad"},{"name":"y","frequency":"yearly","specJson":"{\"start\":\"08:00\"}","enabled":true,"waitReason":"wakeup_waiting","nextFireAt":"not a time","tz":"Not/AZone","lastSessionId":"s-1","sessionProvider":"codex"},{"name":"odd","frequency":"daily","specJson":"{\"every_seconds\":90}","enabled":0,"lastFired":"","tz":"server","wakeupPath":".k2/hb/WAKEUP.md"}]"#;

const ARCHIVED: &str = r#"[{"name":"old","frequency":"daily","archivedAt":"2026-11-01T10:00:00Z","lastFired":null},{"name":"older","frequency":"weekly","archivedAt":"","lastFired":"2026-10-01T09:00:00Z"}]"#;

const SHOWN: &str = r#"{"name":"daily","frequency":"daily","enabled":true,"useWorkspaceSession":false,"lastFired":null,"nextFireAt":"2026-11-20T07:00:00Z","tz":"Europe/Paris","waitReason":"busy","waitDetail":"x","waitSince":"2026-11-19","wakeupPath":".k2/heartbeats/daily/WAKEUP.md","createdAt":"2026-11-01","specJson":"{\"time\":\"07:00\",\"days\":[\"mon\"],\"n\":[1,2]}","instructions":"Do the thing.\nThen stop.\r\n"}"#;

const HISTORY: &str = r#"[{"firedAt":"2026-11-19T07:00:01Z","decision":"fired","actor":null,"reason":"on time"},{"firedAt":"2026-11-18T07:00:00Z","decision":"skipped_busy","actor":"user:ann","reason":null},{"decision":"fired","actor":"owner-token"},{"firedAt":"x","decision":"fired","actor":"app-token:t1","reason":"manual"}]"#;

const LOG: &str = r#"[{"firedAt":"2026-11-19T07:00:01Z","agentName":"daily","decision":"fired","inboxCount":3,"inboxPriority":"high","reason":"inbox"},{"firedAt":"2026-11-19T08:00:00Z","agentName":null,"decision":"skipped","inboxCount":0,"reason":null}]"#;

const ADDED: &str = r#"{"success":true,"name":"daily","wakeupAbs":"/srv/ws/.k2/heartbeats/daily/WAKEUP.md","instructionsWritten":true,"tz":"America/New_York","zoneLabel":"America/New_York","nextFireAt":"2026-11-21T12:00:00Z"}"#;

const SCHED_OK: &str = r#"{"transportInstalled":true,"staleSecs":30,"enabledCount":2}"#;

fn with(env: Vec<(&'static str, String)>, c: Case) -> Case {
    Case { env, ..c }
}

fn traceback(c: Case) -> Case {
    Case {
        compare: Compare::Traceback,
        ..c
    }
}

fn cell() -> Vec<(&'static str, String)> {
    with_passport(&[("K2_HOOK_SOCK", "{sock}")])
}

fn a_folder(r: &Rig) {
    owner_and_port(r);
    std::fs::create_dir_all(r.proj.join("sub")).unwrap();
}

fn a_wakeup_file(r: &Rig) {
    owner_and_port(r);
    std::fs::create_dir_all(r.proj.join(".k2/hb")).unwrap();
    std::fs::write(r.proj.join(".k2/hb/WAKEUP.md"), "Check the inbox.\n").unwrap();
}

fn instructions_file(r: &Rig) {
    owner_and_port(r);
    std::fs::write(r.proj.join("wake.md"), "Read the mail.\nReply.\n\n").unwrap();
}

#[test]
fn heartbeat_lists_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "list");
    let v = vec![
        // covers: heartbeat
        case(
            "bare: the list",
            &[],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: heartbeat schedule
        case(
            "schedule: the list",
            &["schedule"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        // covers: heartbeat schedule --json
        case(
            "schedule --json: not a verb",
            &["schedule", "--json"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "schedule list --json",
            &["schedule", "list", "--json"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list in another zone",
            &["list"],
            &[("TZ", "Asia/Tokyo")],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --json --workspace a name",
            &["list", "--json", "--workspace", "w x"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list --workspace a folder",
            &["schedule", "list", "--project", "sub"],
            &[],
            vec![ok("[]")],
            a_folder,
            0,
            1,
        ),
        case("none", &["list"], &[], vec![ok("[]")], owner_and_port, 0, 1),
        case(
            "the archived ones",
            &["list", "--archived"],
            &[],
            vec![ok(ARCHIVED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "list-archived --json",
            &["list-archived", "--json"],
            &[],
            vec![ok(ARCHIVED)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "no archived ones",
            &["schedule", "list", "--archived"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "archived: an error",
            &["list-archived"],
            &[],
            vec![ok(r#"{"error":"nope"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "owner only: exit 3",
            &["list"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a card",
            &["list"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "other_workspace: exit 3",
            &["list", "--workspace", "w"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"other_workspace","message":"not yours"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "workspace not found: exit 4",
            &["list"],
            &[],
            vec![Reply::Answer(
                404,
                r#"{"error":"workspace not found: x"}"#.into(),
            )],
            owner_and_port,
            4,
            1,
        ),
        case(
            "a dict with no error",
            &["list"],
            &[],
            vec![ok("{}")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["list"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        ),
        case("empty", &["list"], &[], vec![ok("")], owner_and_port, 1, 1),
        case(
            "a row that can't be shown",
            &["list"],
            &[],
            vec![ok(r#"[{"name":"a","specJson":"{}"},{"name":null}]"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "a spec that is not JSON",
            &["list"],
            &[],
            vec![ok(r#"[{"name":"a","specJson":"{"}]"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "odd values",
            &["list"],
            &[],
            vec![ok(
                r#"[{"name":5,"frequency":true,"specJson":"{\"days\":\"mo\",\"every_seconds\":\"-60\"}","zoneLabel":["a","b"],"enabled":1,"pausedUntil":"later","disabledReason":"x"}]"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "--archived with --workspace",
            &["list", "--archived", "--workspace", "w"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "--workspace with no value",
            &["list", "--workspace"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        with(
            cell(),
            case(
                "the cell socket",
                &["list"],
                &[],
                vec![ok(ROWS)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "the cell socket, --json",
                &["list", "--json"],
                &[],
                vec![ok("[]\n\n")],
                port_only,
                0,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn heartbeat_list_large() {
    let rig = Rig::new(VERB, "large");
    let rows: Vec<String> = (0..3000)
        .map(|i| {
            format!(
                r#"{{"name":"hb-{i:05}","frequency":"daily","specJson":"{{\"time\":\"0{}:00\"}}","enabled":true,"lastFired":"2026-11-19T07:00:00Z","nextFireAt":"2026-11-20T07:00:00Z","waitReason":"busy","waitDetail":"turn {i}"}}"#,
                i % 10
            )
        })
        .collect();
    let body = format!("[{}]", rows.join(","));
    assert!(body.len() > 256 * 1024, "{}", body.len());
    rig.check(&case(
        "a large list",
        &["list"],
        &[],
        vec![ok(&body)],
        owner_and_port,
        0,
        1,
    ));
}

#[test]
fn heartbeat_show_status_log_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "show");
    let v = vec![
        // covers: heartbeat show
        case(
            "show",
            &["show", "daily"],
            &[],
            vec![ok(SHOWN)],
            owner_and_port,
            0,
            1,
        ),
        // covers: heartbeat show --json
        case(
            "show --json",
            &["show", "daily", "--json"],
            &[],
            vec![ok(SHOWN)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show --workspace a name",
            &["show", "daily", "--workspace", "w"],
            &[],
            vec![ok(
                r#"{"name":"daily","specJson":null,"instructions":"  "}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "show --workspace a folder",
            &["show", "--workspace", "sub", "daily"],
            &[],
            vec![ok(r#"{"error":"no_such_heartbeat"}"#)],
            a_folder,
            1,
            1,
        ),
        case(
            "show: none",
            &["show", "x"],
            &[],
            vec![ok(r#"{"error":"no_such_heartbeat"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "show: other_workspace",
            &["show", "x"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"other_workspace","hint":"not yours"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "show: workspace not found",
            &["show", "x"],
            &[],
            vec![ok(r#"{"error":"workspace not found"}"#)],
            owner_and_port,
            4,
            1,
        ),
        case(
            "show: an older host",
            &["show", "x"],
            &[],
            vec![Reply::Answer(404, r#"{"error":"not found"}"#.into())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "show: a list",
            &["show", "x"],
            &[],
            vec![ok("[1]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "show: not JSON",
            &["show", "x"],
            &[],
            vec![ok("<")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "show: a spec that is not JSON",
            &["show", "x"],
            &[],
            vec![ok(r#"{"specJson":"{"}"#)],
            owner_and_port,
            1,
            1,
        )),
        traceback(case(
            "show: instructions that are not text",
            &["show", "x"],
            &[],
            vec![ok(r#"{"instructions":5}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "show: no name",
            &["show", "--json"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "show: --workspace with no value",
            &["show", "x", "--workspace", ""],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: heartbeat status
        case(
            "status",
            &["status", "y", "-n", "5"],
            &[],
            vec![ok(ROWS), ok(HISTORY)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "status: no history",
            &["status", "daily"],
            &[],
            vec![ok(ROWS), ok("[]")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "status: none by that name",
            &["status", "nope"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status: an error",
            &["status", "nope"],
            &[],
            vec![ok(r#"{"error":"down"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "status: history that is not JSON",
            &["status", "daily"],
            &[],
            vec![ok(ROWS), ok("x")],
            owner_and_port,
            1,
            2,
        ),
        case(
            "status: no name",
            &["status"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: heartbeat log
        case(
            "log",
            &["log", "--limit", "2"],
            &[],
            vec![ok(LOG)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log: none",
            &["log"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            0,
            1,
        ),
        case(
            "log: empty",
            &["log"],
            &[],
            vec![ok(" ")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "log: not JSON",
            &["log"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "log: a card",
            &["log"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "log: an error",
            &["log"],
            &[],
            vec![ok(r#"{"error":{"code":"x"}}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "log: not rows",
            &["log"],
            &[],
            vec![ok(r#"[1]"#)],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "log: a row that can't be shown",
            &["log"],
            &[],
            vec![ok(r#"[{"firedAt":null}]"#)],
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
fn heartbeat_writes_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "writes");
    let restarted = r#"{"name":"daily","stopped":true,"terminalId":"t1","clearedSessionId":"s1"}"#;
    let v = vec![
        // covers: heartbeat restart
        case(
            "restart",
            &["restart", "daily"],
            &[],
            vec![ok(restarted)],
            owner_and_port,
            0,
            1,
        ),
        // covers: heartbeat restart --json
        case(
            "restart --json",
            &["restart", "daily", "--json", "--workspace=w"],
            &[],
            vec![ok(restarted)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "restart: none running",
            &["restart", "daily", "--workspace", "sub"],
            &[],
            vec![ok(r#"{"stopped":false}"#)],
            a_folder,
            0,
            1,
        ),
        case(
            "restart: none by that name",
            &["restart", "x"],
            &[],
            vec![ok(r#"{"error":"no_such_heartbeat"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "restart: other_workspace",
            &["restart", "x"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"other_workspace","hint":"no"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "restart: role_required",
            &["restart", "x"],
            &[],
            vec![ok(r#"{"error":"role_required","message":"a person"}"#)],
            owner_and_port,
            3,
            1,
        ),
        case(
            "restart: not a heartbeat",
            &["restart", "x"],
            &[],
            vec![ok(r#"{"error":"not_a_heartbeat: x"}"#)],
            owner_and_port,
            2,
            1,
        ),
        case(
            "restart: not JSON",
            &["restart", "x"],
            &[],
            vec![ok("it's <b>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "restart: a list",
            &["restart", "x"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "restart: no name",
            &["restart"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "restart: an unknown flag",
            &["restart", "x", "--now"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "restart: two names",
            &["restart", "x", "y"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "enable",
            &["enable", "daily"],
            &[],
            vec![ok(r#"{"ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "schedule disable --workspace --json",
            &["schedule", "disable", "daily", "--workspace", "w", "--json"],
            &[],
            vec![ok(r#"{"ok":true,"workspace":"w","heartbeat":"daily"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "disable in another workspace",
            &["disable", "daily", "--project=w"],
            &[],
            vec![ok(r#"{"ok":true,"workspace":"w"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "enable: refused",
            &["enable", "daily"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"owner_only"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "enable: no name",
            &["enable"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "remove (archive)",
            &["remove", "daily"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "remove --purge",
            &["schedule", "remove", "--purge", "daily"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "archive: an error",
            &["archive", "daily"],
            &[],
            vec![ok(r#"{"success":false,"error":"busy"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "archive: a list",
            &["archive", "daily"],
            &[],
            vec![ok("[]")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "remove: no name",
            &["remove", "--purge"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "unarchive",
            &["schedule", "unarchive", "daily"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "unarchive: no name",
            &["unarchive"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "rename",
            &["rename", "a", "b c"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "rename: one name",
            &["rename", "a"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "use-pinned-session on",
            &["use-pinned-session", "on", "daily"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "use-pinned-session false: an error",
            &["use-pinned-session", "false", "daily"],
            &[],
            vec![ok(r#"{"error":"nope"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "use-pinned-session: a bad switch",
            &["use-pinned-session", "maybe", "daily"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        // covers: heartbeat signal
        case(
            "signal fire",
            &["signal", "fire", "daily", "--force"],
            &[],
            vec![ok(
                r#"{"success":true,"name":"daily","agent":"k","terminalId":"t9"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "fire: did not fire",
            &["fire", "daily"],
            &[],
            vec![ok(
                r#"{"success":false,"name":"daily","decision":"skipped_busy","reason":"a turn"}"#,
            )],
            owner_and_port,
            1,
            1,
        ),
        case(
            "fire: an error",
            &["fire", "daily"],
            &[],
            vec![ok(r#"{"error":"disabled"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "fire: an unknown flag",
            &["signal", "fire", "daily", "--now"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "fire: no name",
            &["fire"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "signal alone",
            &["signal"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        with(
            cell(),
            case(
                "fire on the cell socket",
                &["fire", "daily"],
                &[],
                vec![ok(r#"{"success":true}"#)],
                port_only,
                0,
                1,
            ),
        ),
        with(
            cell(),
            case(
                "restart on the cell socket",
                &["restart", "daily"],
                &[],
                vec![ok(restarted)],
                port_only,
                0,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn heartbeat_session_and_wakeup_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "session");
    let v = vec![
        // covers: heartbeat session
        case(
            "session: pinned",
            &["session", "weekly"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "session: a provider's",
            &["session", "y"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "session: auto",
            &["session", "daily"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "session: none by that name",
            &["session", "nope"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "session: not JSON",
            &["session", "nope"],
            &[],
            vec![ok("x")],
            owner_and_port,
            1,
            1,
        ),
        traceback(case(
            "session: an error object",
            &["session", "nope"],
            &[],
            vec![ok(r#"{"error":"down"}"#)],
            owner_and_port,
            1,
            1,
        )),
        case(
            "session --pinned",
            &["session", "daily", "--pinned"],
            &[],
            vec![ok(r#"{"success":true,"mode":"pinned"}"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "session --set",
            &[
                "session",
                "daily",
                "--set",
                "sales/reviewer",
                "--provider",
                "codex",
            ],
            &[],
            vec![ok(
                r#"{"success":true,"mode":"session","provider":"codex","sessionId":"abc"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "session --auto: an error",
            &["session", "daily", "--auto"],
            &[],
            vec![ok(r#"{"success":false,"error":"no"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "session --provider alone: the read",
            &["session", "daily", "--provider", "codex"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "session --pinned --provider",
            &["session", "daily", "--pinned", "--provider", "codex"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "session --set empty",
            &["session", "daily", "--set", ""],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "session: an unknown flag",
            &["session", "daily", "--x"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "session: no name",
            &["session"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "wakeup --path-only",
            &["signal", "wakeup", "odd", "--path-only"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "wakeup: the file",
            &["wakeup", "odd"],
            &[],
            vec![ok(ROWS)],
            a_wakeup_file,
            0,
            1,
        ),
        case(
            "wakeup: no file yet",
            &["wakeup", "odd"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "wakeup: no path on file",
            &["wakeup", "daily"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "wakeup: none by that name",
            &["wakeup", "nope"],
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "wakeup: no name",
            &["wakeup"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// `wakeup <name> --edit`: the editor (`EDITOR`, else vi when it is unset
/// or empty) opens the wakeup file; with no file yet, the note names that
/// editor first. Stand-in editors first on `PATH` log what they were run
/// with, and each side's log must be the other's.
#[test]
fn wakeup_edit_opens_the_editor_as_the_shell_does() {
    use std::os::unix::fs::PermissionsExt;
    let mut rig = Rig::new(VERB, "wedit");
    let log = rig.dir.join("editor.log");
    let bin = rig.dir.join("editors");
    std::fs::create_dir_all(&bin).unwrap();
    for prog in ["vi", "myedit"] {
        let file = bin.join(prog);
        let script = format!(
            "#!/bin/sh\nprintf '== {prog} [%s]\\n' \"$*\" >> '{}'\n",
            log.display()
        );
        std::fs::write(&file, script).unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    rig.path_first = Some(bin);
    rig.tool_log = Some(log);
    let edit: &'static [&'static str] = &["wakeup", "odd", "--edit"];
    let v = vec![
        case(
            "wakeup --edit, no EDITOR, no file yet: vi, named in the note",
            edit,
            &[],
            vec![ok(ROWS)],
            owner_and_port,
            0,
            1,
        ),
        with(
            vec![("EDITOR", String::new())],
            case(
                "wakeup --edit, an empty EDITOR, no file yet: vi",
                edit,
                &[],
                vec![ok(ROWS)],
                owner_and_port,
                0,
                1,
            ),
        ),
        with(
            vec![("EDITOR", "myedit".into())],
            case(
                "wakeup --edit, EDITOR set, no file yet: that editor",
                edit,
                &[],
                vec![ok(ROWS)],
                owner_and_port,
                0,
                1,
            ),
        ),
        // Last: its setup leaves the file there.
        case(
            "wakeup --edit, no EDITOR, the file: vi, no note",
            edit,
            &[],
            vec![ok(ROWS)],
            a_wakeup_file,
            0,
            1,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn heartbeat_add_and_edit_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "add");
    let v = vec![
        case(
            "add",
            &[
                "schedule",
                "add",
                "--name",
                "daily",
                "--daily",
                "--time",
                "07:00",
                "--tz",
                "America/New_York",
                "--instructions",
                "Check the inbox.",
            ],
            &[],
            vec![ok(ADDED), ok(SCHED_OK)],
            owner_and_port,
            0,
            2,
        ),
        case(
            "add: an instructions file, no transport",
            &[
                "add",
                "--name",
                "w",
                "--weekly",
                "--days",
                "Mon,fri",
                "--instructions-file",
                "wake.md",
            ],
            &[],
            vec![
                ok(r#"{"success":true,"instructionsWritten":true}"#),
                ok(r#"{"transportInstalled":false}"#),
            ],
            instructions_file,
            0,
            2,
        ),
        case(
            "add: kept the existing file, a stale scheduler",
            &[
                "add",
                "--name",
                "m",
                "--monthly",
                "--days",
                "1,15",
                "--instructions",
                "x",
            ],
            &[],
            vec![
                ok(
                    r#"{"success":true,"instructionsWritten":false,"instructionsKeptExisting":true,"wakeupBodyBytes":12,"tz":null}"#,
                ),
                ok(r#"{"staleSecs":1900,"enabledCount":1}"#),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "add: an empty file waits",
            &["add", "--name", "h", "--hourly", "--every", "30"],
            &[],
            vec![
                ok(r#"{"success":true,"waitReason":"wakeup_empty"}"#),
                ok("x"),
            ],
            owner_and_port,
            0,
            2,
        ),
        case(
            "add: an older daemon and --tz",
            &["add", "--name", "h", "--daily", "--tz", "Europe/Paris"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "add: an older daemon and --instructions",
            &["add", "--name", "h", "--daily", "--instructions", "x"],
            &[],
            vec![ok(r#"{"success":true,"wakeupAbs":"/w"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "add: the daemon's error",
            &["add", "--name", "h", "--daily"],
            &[],
            vec![Reply::Answer(
                403,
                r#"{"error":{"code":"forbidden","hint":"agents can't"}}"#.into(),
            )],
            owner_and_port,
            3,
            1,
        ),
        case(
            "add: not JSON",
            &["add", "--name", "h", "--daily"],
            &[],
            vec![ok("nope")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "add: no name",
            &["add", "--daily"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "add: no frequency",
            &["add", "--name", "h", "--time", "07:00"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "add: a bad time",
            &["add", "--name", "h", "--daily", "--time", "7"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: an unknown flag",
            &["add", "--name", "h", "--interval", "1"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: a one-time flag",
            &["add", "--name", "h", "--in", "5m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: bad days",
            &["add", "--name", "h", "--weekly", "--days", "mon,someday"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: bad month days",
            &[
                "add", "--name", "h", "--yearly", "--months", "jan", "--days", "0",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: bad months",
            &["add", "--name", "h", "--yearly", "--months", "smarch"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: a bad unit",
            &[
                "add", "--name", "h", "--hourly", "--every", "5", "--unit", "days",
            ],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: every 0",
            &["add", "--name", "h", "--hourly", "--every", "00"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: --tz with no value",
            &["add", "--name", "h", "--daily", "--tz"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "add: no instructions file",
            &[
                "add",
                "--name",
                "h",
                "--daily",
                "--instructions-file",
                "nope.md",
            ],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "edit --tz",
            &["edit", "daily", "--tz", "America/New_York"],
            &[],
            vec![ok(
                r#"{"success":true,"tz":"America/New_York","nextFireAt":"2026-11-21T12:00:00Z","tzFrom":"owner"}"#,
            )],
            owner_and_port,
            0,
            1,
        ),
        case(
            "schedule edit: a spec and instructions",
            &[
                "schedule",
                "edit",
                "--name",
                "daily",
                "--weekly",
                "--days",
                "sat",
                "--instructions-file",
                "wake.md",
            ],
            &[],
            vec![ok(r#"{"success":true}"#)],
            instructions_file,
            0,
            1,
        ),
        case(
            "edit: an older daemon",
            &["edit", "daily", "--instructions", "x"],
            &[],
            vec![ok(r#"{"success":false,"error":"missing frequency"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "edit --tz: an older daemon",
            &["edit", "daily", "--tz", ""],
            &[],
            vec![ok(r#"{"success":false,"error":"missing frequency"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "edit --tz: no tz in the answer",
            &["edit", "daily", "--tz", "server"],
            &[],
            vec![ok(r#"{"success":true}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "edit: an error",
            &["edit", "daily", "--daily"],
            &[],
            vec![ok(r#"{"error":"no"}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "edit: nothing to change",
            &["edit", "daily"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "edit: blank instructions",
            &["edit", "daily", "--instructions", " \t"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn heartbeat_stops_and_reminders_answer_as_the_shell_does() {
    let rig = Rig::new(VERB, "stops");
    let v = vec![
        case("help", &["help"], &[], vec![], owner_and_port, 0, 0),
        case("a retired verb", &["on"], &[], vec![], owner_and_port, 1, 0),
        case("noop", &["noop"], &[], vec![], owner_and_port, 1, 0),
        case("force", &["force"], &[], vec![], owner_and_port, 1, 0),
        case("set", &["set"], &[], vec![], owner_and_port, 1, 0),
        case("get", &["get"], &[], vec![], owner_and_port, 1, 0),
        case(
            "schedule off",
            &["schedule", "off"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "schedule daily",
            &["schedule", "daily"],
            &[],
            vec![],
            owner_and_port,
            1,
            0,
        ),
        case(
            "schedule nope",
            &["schedule", "nope"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        case(
            "an unknown verb",
            &["frobnicate"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // covers: heartbeat remind
        case(
            "remind",
            &["remind", "call back", "--in", "45m"],
            &[],
            vec![ok(r#"{"name":"r1","fires":"in 45m (13:45)","ok":true}"#)],
            owner_and_port,
            0,
            1,
        ),
        // covers: heartbeat remind --json
        case(
            "remind list --json",
            &["remind", "list", "--json"],
            &[],
            vec![ok(r#"[{"name":"r1","state":"pending"}]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "reminders",
            &["reminders"],
            &[],
            vec![ok(r#"[{"name":"r1","state":"pending","text":"x"}]"#)],
            owner_and_port,
            0,
            1,
        ),
        case(
            "HOOK_SOCK set and no passport",
            &["list"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        case("no port", &["list"], &[], vec![], owner_only, 1, 0),
        case(
            "nothing listens: restart holds nothing",
            &["restart", "x"],
            &[("K2_PORT", dead_port().as_str())],
            vec![],
            owner_only,
            1,
            0,
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

#[test]
fn heartbeat_with_no_answer() {
    let rig = Rig::new(VERB, "none");
    let v = vec![
        // bash32-pipe-reader: on macOS's /bin/bash 3.2 the list's reader
        // runs on the empty input and the command exits 1.
        Case {
            compare: Compare::Bash32PipeReader {
                stdout: "",
                stderr: "Error: Expecting value: line 1 column 1 (char 0)\n",
            },
            ..case(
                "an empty reply to the list",
                &["list"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            )
        },
        // bash32-pipe-reader: the fire's reader too.
        Case {
            compare: Compare::Bash32PipeReader {
                stdout: "",
                stderr: "Error: Expecting value: line 1 column 1 (char 0)\n",
            },
            ..case(
                "an empty reply to a fire",
                &["fire", "x"],
                &[],
                vec![Reply::Close],
                owner_and_port,
                141,
                1,
            )
        },
        // bash32-pipe-reader: on the cell socket the reader prints nothing
        // more on bash 3.2, and the command exits 1.
        Case {
            compare: Compare::Bash32PipeReader {
                stdout: "",
                stderr: "",
            },
            ..with(
                cell(),
                case(
                    "a fire the cell socket may have run",
                    &["fire", "x"],
                    &[],
                    vec![Reply::Close],
                    port_only,
                    141,
                    1,
                ),
            )
        },
        case(
            "not HTTP",
            &["log"],
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
}

/// Help never acts (K2's rule): `-h` or `--help` anywhere on a line that
/// would change something prints help and sends nothing. The binary hands
/// such a line to the shell, whose arm prints the help first.
#[test]
fn heartbeat_help_never_acts() {
    let rig = Rig::new(VERB, "helpnot");
    let shell = |name: &'static str, args: &'static [&'static str]| Case {
        compare: Compare::Shell,
        ..case(name, args, &[], vec![], owner_and_port, 0, 0)
    };
    for c in [
        shell("restart -h", &["restart", "daily", "-h"]),
        shell(
            "add --help",
            &["schedule", "add", "--name", "a", "--daily", "--help"],
        ),
        shell("fire -h", &["signal", "fire", "a", "-h"]),
        shell("enable --help", &["enable", "a", "--help"]),
        shell("disable -h", &["schedule", "disable", "-h", "a"]),
        shell("remove --help", &["remove", "a", "--purge", "--help"]),
        shell("unarchive -h", &["unarchive", "a", "-h"]),
        shell("edit -h", &["edit", "a", "--tz", "UTC", "-h"]),
        shell("rename --help", &["rename", "a", "b", "--help"]),
        shell(
            "use-pinned-session -h",
            &["use-pinned-session", "on", "a", "-h"],
        ),
        shell("session --set -h", &["session", "a", "--set", "-h"]),
        shell("remind --help", &["remind", "x", "--in", "5m", "--help"]),
        shell("wake -h", &["wake", "-h"]),
    ] {
        rig.check(&c);
    }
}

const NO_WORK: &str = r#"[{"name":"a","inboxCount":0}]"#;
const WORK: &str = r#"[{"name":"a","inboxCount":2}]"#;
const MANAGER_UP: &str = r#"[{"cwd":"/srv/ws/.k2/agents/manager","terminalId":"t-9"}]"#;

/// `heartbeat wake` (and `signal wake`): the noop, the running manager
/// (by its folder, by its terminal id), the launch and its poll, each
/// failure the block names (not a list, a type it can't compare or
/// walk, a refusal with its card, an error status, not JSON, a change
/// the daemon answered with an error, no answer, not HTTP), and the one
/// heal of a refused passport.
#[test]
fn heartbeat_wake_answers_as_the_shell_does() {
    let rig = Rig::new(VERB, "wake");
    let v = vec![
        case(
            "no inbox work: noop",
            &["wake"],
            &[],
            vec![ok(NO_WORK), ok("[]")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "signal wake, with words after it: noop",
            &["signal", "wake", "now", "--json"],
            &[],
            vec![ok("[]"), ok("[]")],
            owner_and_port,
            0,
            2,
        ),
        case(
            "an unfiled inbox item, the manager running: notified",
            &["wake"],
            &[],
            vec![
                ok(NO_WORK),
                ok(r#"[{"id":"i1"}]"#),
                ok(MANAGER_UP),
                ok(r#"{"ok":true}"#),
            ],
            owner_and_port,
            0,
            4,
        ),
        case(
            "the manager by its terminal id",
            &["wake"],
            &[],
            vec![
                ok(WORK),
                ok("[]"),
                ok(
                    r#"[{"cwd":"/x","terminalId":"agent-chat:p1:coordinator"},{"cwd":"/y","terminalId":"agent-chat:p1:manager:hb:d"}]"#,
                ),
                ok("{}"),
            ],
            owner_and_port,
            0,
            4,
        ),
        case(
            "the manager asleep: launched, then notified",
            &["wake"],
            &[],
            vec![
                ok(WORK),
                ok("[]"),
                ok(r#"[{"cwd":"/x","terminalId":"agent-chat:wt:w1"}]"#),
                ok(r#"{"launched":true}"#),
                ok(r#"[{"cwd":"/z","terminalId":"agent-chat-manager-7"}]"#),
                ok("{}"),
            ],
            owner_and_port,
            0,
            6,
        ),
        case(
            "the agents are not a list",
            &["wake"],
            &[],
            vec![ok(r#"{"agents":[]}"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "an inbox count that is a string",
            &["wake"],
            &[],
            vec![ok(r#"[{"inboxCount":"3"}]"#)],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the inbox items are not dicts",
            &["wake"],
            &[],
            vec![ok(NO_WORK), ok("[1]")],
            owner_and_port,
            1,
            2,
        ),
        case(
            "a session's cwd that is a list holding the manager's folder",
            &["wake"],
            &[],
            vec![
                ok(WORK),
                ok("[]"),
                ok(r#"[{"cwd":[".k2/agents/manager"],"terminalId":"t-1"}]"#),
                ok("{}"),
            ],
            owner_and_port,
            0,
            4,
        ),
        case(
            "a terminal id that is a number",
            &["wake"],
            &[],
            vec![ok(WORK), ok("[]"), ok(r#"[{"cwd":"/x","terminalId":7}]"#)],
            owner_and_port,
            1,
            3,
        ),
        case(
            "a refusal with a permission card",
            &["wake"],
            &[],
            vec![Reply::Answer(403, PERMISSION_ASK.into())],
            owner_and_port,
            3,
            1,
        ),
        case(
            "a 500 with no body",
            &["wake"],
            &[],
            vec![Reply::Answer(500, String::new())],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not JSON",
            &["wake"],
            &[],
            vec![ok("<html>")],
            owner_and_port,
            1,
            1,
        ),
        case(
            "the write answered with an error",
            &["wake"],
            &[],
            vec![
                ok(WORK),
                ok("[]"),
                ok(MANAGER_UP),
                ok(r#"{"error":{"code":"gone","hint":"terminal gone"}}"#),
            ],
            owner_and_port,
            1,
            4,
        ),
        case(
            "an empty reply",
            &["wake"],
            &[],
            vec![Reply::Close],
            owner_and_port,
            1,
            1,
        ),
        case(
            "not HTTP",
            &["wake"],
            &[],
            vec![Reply::Raw(GARBAGE.to_vec())],
            owner_and_port,
            1,
            1,
        ),
        with(
            with_passport(&[]),
            case(
                "a refused passport heals once, from the passport the refusal left",
                &["wake"],
                &[],
                vec![heal(&rig), ok("[]"), ok("[]")],
                port_only,
                0,
                3,
            ),
        ),
        with(
            with_passport(&[]),
            case(
                "a passport minted again before the run: refused, no second heal",
                &["wake"],
                &[],
                vec![Reply::Answer(403, REFUSED.into())],
                healed_on_disk,
                1,
                1,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// The lines name the command by the brand's name (in a build with
/// `K2_BRAND_DIR=brand/xy` they say `xy`).
#[test]
fn heartbeat_lines_say_the_brand_s_name() {
    let cli = k2_client::brand::CLI_NAME;
    let rig = Rig::new(VERB, "brand");
    for (args, want, code) in [
        (
            &["restart"][..],
            format!("Usage: {cli} heartbeat restart <name> [--workspace <ws>] [--json]\n"),
            2,
        ),
        (
            &["frobnicate"],
            format!("Unknown 'heartbeat' subcommand: frobnicate\nRun '{cli} help heartbeat' for the schedule/signal/inspection surface.\n"),
            2,
        ),
        (
            &["schedule", "off"],
            format!("Error: 'heartbeat schedule off' is no longer supported.\n\nUse instead:\n  {cli} heartbeat schedule disable <name>   (per-heartbeat pause)\n\nRun '{cli} help-deprecated' for the full map of retired verbs.\n"),
            1,
        ),
    ] {
        let c = case("brand", args, &[], vec![], owner_and_port, code, 0);
        let (out, log) = rig.binary(&c, false);
        assert_eq!(out.stderr, want, "{args:?}");
        assert_eq!(out.code, Some(code), "{args:?}");
        assert!(log.is_empty(), "{args:?}");
    }
}

/// `heartbeat remind` with no first word (or an empty one) and no time:
/// `cmd_help_remind` on stdout, exit 0, after the gate and the `heartbeat`
/// tool's session check.
#[test]
fn heartbeat_remind_alone_prints_the_remind_help() {
    let rig = Rig::new(VERB, "remindalone");
    let v = vec![
        // covers: heartbeat remind
        case(
            "remind alone",
            &["remind"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "remind, an empty word",
            &["remind", ""],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "remind, an empty word and text, no time",
            &["remind", "", "call back"],
            &[],
            vec![],
            owner_and_port,
            0,
            0,
        ),
        case(
            "remind alone, no port",
            &["remind"],
            &[],
            vec![],
            owner_only,
            1,
            0,
        ),
        case(
            "remind alone, HOOK_SOCK set and no passport",
            &["remind"],
            &[("K2_HOOK_SOCK", "{sock}")],
            vec![],
            owner_and_port,
            3,
            0,
        ),
        with(
            cell(),
            case(
                "remind alone, a session's passport",
                &["remind"],
                &[],
                vec![],
                port_only,
                0,
                0,
            ),
        ),
    ];
    for c in v {
        rig.check(&c);
    }
}

/// Rows that follow 99123b051 on the 0.46.17 queue ("message text after
/// -- is sent as typed"), not yet in this line's cli/src: today both
/// sides take a bare `--` for an unknown flag. When the replay brings
/// that commit, these rows fail on the shell's side; port `_remind_add`'s
/// `--` (the rest is the text) and the time scan stopping at `--`, then
/// turn each row to the new answer.
#[test]
fn follows_99123b051_remind_text_after_dashdash() {
    let rig = Rig::new(VERB, "dashdash");
    for c in [
        // follows 99123b051: the text "call back", sent.
        case(
            "remind --in 5m -- call back",
            &["remind", "--in", "5m", "--", "call", "back"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
        // follows 99123b051: no time before `--`, so "say when", exit 2.
        case(
            "remind call -- --in 5m",
            &["remind", "call", "--", "--in", "5m"],
            &[],
            vec![],
            owner_and_port,
            2,
            0,
        ),
    ] {
        rig.check(&c);
    }
}
