//! What the shell CLI does around a verb that asks the daemon, for the
//! binary's own families: its two gates, and what its EXIT trap prints
//! once the verb is done (`cli/src/prelude.sh`, the end of
//! `cli/src/cli-tools-policy.sh`, and `_permission_note` and
//! `_permission_note_flush` in `cli/src/cell-socket.sh`).
//!
//! In the shell's order:
//!
//! 1. [`gate`]: the virtual desktop's guard (`NO_OWNER_FALLBACK=1` and no
//!    passport: exit 3), then the connection gate (no port or no token:
//!    exit 1). Each prints the shell's three lines. Then [`plain_http`]:
//!    a daemon on another computer over plain http, one warning line (or
//!    a password flag refused, exit 2). The binary runs it just before
//!    the first thing it prints or sends.
//! 2. For a verb the shell's tool policy calls an `id` tool, in a session
//!    with `HOOK_SOCK` set (`_cli_check_session_passport`,
//!    `_cli_apply_auth_for_verb`): a passport the daemon no longer knows
//!    with its socket gone ([`stale_passport_lines`], exit 3), and no
//!    passport at all ([`require_passport_lines`], exit 3). The shared
//!    Codex server check that comes first is the shell's own
//!    ([`codex_env`]: a family hands such a line to the shell).
//! 3. The verb. Each answer it gets goes through [`Closing::answer`].
//! 4. [`Closing::flush`], the EXIT trap: when an answer refused the
//!    passport, a probe asks whether it is dead and says to restart the
//!    tab; a refusal's closing line when the command failed; the Org
//!    Chart's bulletin line when it worked.
//!
//! The texts are the shell's; `tests` below pins each to `cli/src`.

use crate::daemon::Settings;
use crate::exit::Exit;
use crate::out;
use crate::transport::permission::Note;
use crate::transport::{Auth, Client, Response};
use std::path::Path;

pub mod plain_http;

/// The no-owner refusal (LD-10: a virtual desktop, or a command under
/// `k2 vault run`): exit 3.
pub const VIRTUAL_DESKTOP: [&str; 2] = [
    concat!(
        "Error: ",
        k2_client::cli_name!(),
        " won't act as this server's owner here: no session passport (K2_NO_OWNER_FALLBACK=1, a ",
        k2_client::product_name!(),
        " virtual desktop or ",
        k2_client::cli_name!(),
        " vault run)."
    ),
    concat!(
        "Run ",
        k2_client::cli_name!(),
        " from a ",
        k2_client::product_name!(),
        " terminal tab or your signed-in device, not under vault run, or give the command the secret it needs."
    ),
];

/// The connection gate: exit 1.
pub const CANNOT_CONNECT: [&str; 3] = [
    concat!("Error: Cannot connect to ", k2_client::product_name!(), "."),
    concat!(
        "Either run this from a ",
        k2_client::product_name!(),
        " terminal, or ensure ",
        k2_client::product_name!(),
        " is running."
    ),
    "(Connection details: ~/.k2/heartbeat.port and ~/.k2/heartbeat.token)",
];

/// The line after a refused passport the daemon doesn't know.
pub const DEAD_PASSPORT: &str =
    "This session's passport isn't valid on this daemon. Restart this tab to get a new one.";

/// `_cli_require_session_passport <tool>`: an `id` tool in a session
/// with `HOOK_SOCK` set and no passport. Exit 3.
pub fn require_passport_lines(tool: &str) -> [String; 3] {
    [
        format!("Error: tool '{tool}' requires session ID (passport)."),
        concat!(
            "Run it from a ",
            k2_client::product_name!(),
            " agent session that carries its passport."
        )
        .to_string(),
        "No setting turns this off; locked production tools always need one.".to_string(),
    ]
}

/// `_cli_passport_stale_die stale <sock>`: the session's socket is gone
/// and the daemon refuses its passport. Exit 3.
pub fn stale_passport_lines(sock: &str) -> [String; 4] {
    [
        concat!(
            "Error: this shell's ",
            k2_client::product_name!(),
            " session credentials are stale: the session socket is gone"
        )
        .to_string(),
        format!("({sock}) and the daemon rejects the session token."),
        "If this is a Codex tab, the tool runs under a shared Codex app-server started by"
            .to_string(),
        "another session; restart Codex in this tab. Otherwise restart this tab.".to_string(),
    ]
}

/// `compgen -e CODEX_` lists a name: some exported variable's name
/// starts with `CODEX_`, so the shell looks for a shared Codex server in
/// the process's ancestry (`_cli_under_shared_codex_server`).
pub fn codex_env(names: impl IntoIterator<Item = std::ffi::OsString>) -> bool {
    names
        .into_iter()
        .any(|n| n.to_str().is_some_and(|n| n.starts_with("CODEX_")))
}

/// Step 3 for an `id` tool, in a session with `HOOK_SOCK` set (`hook_sock`):
/// `_cli_check_session_passport`'s stale check (a socket that is gone,
/// with a passport the probe finds refused: [`stale_passport_lines`]),
/// then `_cli_apply_auth_for_verb`'s refusal when there is no passport at
/// all ([`require_passport_lines`] for `tool`). Each prints its lines and
/// is exit 3. `None`: go on. The probe asks only when the port is one curl
/// takes ([`crate::families::curl_port`]).
pub fn id_tool(
    settings: &Settings,
    hook_sock: Option<&Path>,
    would_refuse: bool,
    tool: &str,
) -> Option<Exit> {
    let sock = hook_sock?;
    let passport = matches!(settings.auth, Auth::Passport { .. });
    if !crate::transport::client::is_socket(sock) && passport {
        if let Some(port) = settings
            .port_text
            .as_deref()
            .and_then(crate::families::curl_port)
        {
            let probe = Settings {
                port: Some(port),
                ..settings.clone()
            };
            if probe
                .client(would_refuse)
                .is_ok_and(|mut c| c.passport_rejected())
            {
                for l in stale_passport_lines(&sock.to_string_lossy()) {
                    out::stderr_line(&l);
                }
                return Some(Exit::Refused);
            }
        }
    }
    if !passport {
        for l in require_passport_lines(tool) {
            out::stderr_line(&l);
        }
        return Some(Exit::Refused);
    }
    None
}

/// The bulletin line is shown at most this often per agent, in seconds.
pub const BULLETIN_EVERY: u64 = 1800;

/// A gate that stops the command before its verb runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// `NO_OWNER_FALLBACK=1` and no passport.
    VirtualDesktop,
    /// No port, or no token at all.
    CannotConnect,
}

impl Gate {
    pub fn lines(self) -> &'static [&'static str] {
        match self {
            Gate::VirtualDesktop => &VIRTUAL_DESKTOP[..],
            Gate::CannotConnect => &CANNOT_CONNECT[..],
        }
    }

    pub fn exit(self) -> Exit {
        match self {
            Gate::VirtualDesktop => Exit::Refused,
            Gate::CannotConnect => Exit::Error,
        }
    }

    /// Prints the lines on stderr; the exit. The shell's gates exit
    /// before its plain-http step, so that step won't run.
    pub fn stop(self) -> Exit {
        plain_http::skip();
        for line in self.lines() {
            out::stderr_line(line);
        }
        self.exit()
    }
}

/// The gate for a verb that asks the daemon, if one stops it. The port
/// counts as the shell's text: set and not empty.
pub fn gate(s: &Settings) -> Option<Gate> {
    let no_token = s.auth == Auth::None;
    if s.no_owner_fallback && no_token {
        return Some(Gate::VirtualDesktop);
    }
    if s.port_text.as_deref().is_none_or(str::is_empty) || no_token {
        return Some(Gate::CannotConnect);
    }
    None
}

/// Whether `args` (the whole command line after the program's name) has
/// `-h` or `--help` in it and a gate stops it ([`gate`]). The shell's
/// gates read those words (`_k2_offline_verb` lets such a line by the
/// virtual desktop's guard, and some families skip the connection gate
/// for it), so a family that answers a help line itself
/// ([`crate::text::FAMILY_FORMS`]) hands it to the shell there, before
/// anything is printed or sent. Where no gate stops it, the words change
/// nothing in the gates.
pub fn help_line_gated(args: &[std::ffi::OsString], s: &Settings) -> bool {
    args.iter().any(|a| a == "-h" || a == "--help") && gate(s).is_some()
}

/// What the EXIT trap prints, gathered from the answers.
#[derive(Debug, Default)]
pub struct Closing {
    /// An answer refused the passport ("Invalid or missing auth token").
    passport_refused: bool,
    /// The last refusal's closing line.
    note: Option<Note>,
    /// The last unread bulletin's day and key.
    bulletin: Option<(String, String)>,
}

impl Closing {
    /// `_permission_note` on one answer. `passport`: the call had one.
    pub fn answer(&mut self, r: &Response, passport: bool) {
        if let Some(b) = bulletin(&r.body) {
            self.bulletin = Some(b);
        }
        if passport
            && contains(
                &r.body,
                crate::transport::client::PASSPORT_REFUSAL.as_bytes(),
            )
        {
            self.passport_refused = true;
        }
        if let Some(n) = &r.permission {
            self.note = Some(n.clone());
        }
    }

    /// `_permission_note_reset`: the refusal lines so far are dropped.
    pub fn reset_note(&mut self) {
        self.note = None;
    }

    /// `_permission_note_flush`, as the command ends with `exit`: the
    /// dead-passport line (after the probe), the closing line when the
    /// command failed, the bulletin line when it worked.
    pub fn flush(self, client: &mut Client, exit: Exit) {
        if self.passport_refused {
            client.use_healed_passport();
            if !client.force_tcp && client.passport_rejected() {
                out::stderr_line(DEAD_PASSPORT);
            }
        }
        if exit != Exit::Ok {
            if let Some(n) = &self.note {
                let _ = n.print(&mut std::io::stderr().lock());
            }
        }
        if let (Exit::Ok, Some((day, key))) = (exit, &self.bulletin) {
            bulletin_line(day, key, std::env::var_os("TMPDIR"), now());
        }
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// `_k2_note_dir` (`cli/src/cell-socket.sh`, 0.46.17): the first of
/// `TMPDIR` (when set and not empty), `/tmp` and `/var/tmp` that is a
/// folder this user may write in and enter, with one trailing `/` taken
/// off (`${d%/}`, `/` when that leaves nothing); `/tmp` when none is.
fn note_dir(tmpdir: Option<std::ffi::OsString>) -> std::path::PathBuf {
    let usable = |d: &std::path::Path| d.is_dir() && writable_folder(d);
    let tmpdir = tmpdir.filter(|d| !d.is_empty());
    for d in tmpdir
        .iter()
        .map(std::path::PathBuf::from)
        .chain(["/tmp".into(), "/var/tmp".into()])
    {
        if usable(&d) {
            let text = d.to_string_lossy();
            let trimmed = text.strip_suffix('/').unwrap_or(&text);
            return std::path::PathBuf::from(if trimmed.is_empty() { "/" } else { trimmed });
        }
    }
    std::path::PathBuf::from("/tmp")
}

/// `[ -w "$d" ] && [ -x "$d" ]`.
#[cfg(unix)]
fn writable_folder(d: &std::path::Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(d.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a valid NUL-terminated path and a plain mode.
    unsafe { libc::access(c.as_ptr(), libc::W_OK | libc::X_OK) == 0 }
}

#[cfg(not(unix))]
fn writable_folder(d: &std::path::Path) -> bool {
    d.is_dir()
}

/// The bulletin's line for `day`, once per [`BULLETIN_EVERY`] per `key`:
/// its stamp file is `<the note folder>/k2-bulletin.<key>` ([`note_dir`]),
/// holding the time it was last shown.
fn bulletin_line(day: &str, key: &str, tmpdir: Option<std::ffi::OsString>, now: u64) {
    let stamp = note_dir(tmpdir).join(format!("k2-bulletin.{key}"));
    // `read -r last`: the first line, spaces and tabs trimmed; anything
    // but digits counts as never shown.
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|t| {
            let first = t
                .split('\n')
                .next()
                .unwrap_or("")
                .trim_matches([' ', '\t'])
                .to_string();
            (!first.is_empty() && first.bytes().all(|b| b.is_ascii_digit()))
                .then(|| first.parse::<i128>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if i128::from(now) - last >= i128::from(BULLETIN_EVERY) {
        out::stderr_line(&format!(
            concat!(
                k2_client::product_name!(),
                " bulletin for {} is unread: k2 organization bulletin"
            ),
            day
        ));
        let _ = std::fs::write(&stamp, format!("{now}\n"));
    }
}

/// The Org Chart's mark on an agent's answer, as the shell's pattern
/// finds it: `"bulletin":{"day":"<[0-9-]+>","unread":true,"key":"<[0-9a-f]+>"`,
/// the first place it matches.
pub fn bulletin(body: &[u8]) -> Option<(String, String)> {
    const HEAD: &[u8] = br#""bulletin":{"day":""#;
    const MID: &[u8] = br#"","unread":true,"key":""#;
    let mut from = 0;
    while let Some(at) = find(&body[from..], HEAD) {
        let start = from + at + HEAD.len();
        from = from + at + 1;
        let rest = &body[start..];
        let day_len = rest
            .iter()
            .take_while(|b| b.is_ascii_digit() || **b == b'-')
            .count();
        if day_len == 0 || !rest[day_len..].starts_with(MID) {
            continue;
        }
        let after = &rest[day_len + MID.len()..];
        let key_len = after
            .iter()
            .take_while(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(*b))
            .count();
        if key_len == 0 || after.get(key_len) != Some(&b'"') {
            continue;
        }
        let text = |b: &[u8]| String::from_utf8_lossy(b).into_owned();
        return Some((text(&rest[..day_len]), text(&after[..key_len])));
    }
    None
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    find(hay, needle).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(file: &str) -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../cli/src")
            .join(file);
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// `echo "<line>" >&2` is in the file, as one line of its own.
    fn echoed(file: &str, line: &str) {
        let want = format!("echo \"{}\" >&2", line.replace('"', "\\\""));
        assert!(
            source(file).lines().any(|l| l.trim() == want),
            "no `{want}` in cli/src/{file}"
        );
    }

    #[test]
    fn the_passport_lines_are_the_shell_s() {
        for line in require_passport_lines("${tool_id}")
            .iter()
            .chain(stale_passport_lines("${sock}").iter())
        {
            echoed("cli-tools-policy.sh", line);
        }
        let policy = source("cli-tools-policy.sh");
        assert!(
            policy.contains("    exit 3\n}\n\n# COMPAT-58"),
            "_cli_require_session_passport's exit moved"
        );
        assert!(
            policy.contains(r#"_cli_require_session_passport "${tool_id:-$verb}""#),
            "the tool's name moved"
        );
        assert!(
            policy.contains(r#"[ -n "$(compgen -e CODEX_ 2>/dev/null)" ] || return 1"#),
            "the Codex check moved"
        );
        assert!(
            policy.contains(r#"if [ ! -S "$hook_sock" ] && _cli_scoped_token_rejected; then"#),
            "the stale check moved"
        );
        let names = |l: &[&str]| l.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert!(codex_env(names(&["PATH", "CODEX_THREAD_ID"])));
        assert!(!codex_env(names(&["PATH", "MY_CODEX_X", "codex_x"])));
    }

    #[test]
    fn the_lines_are_the_shell_s() {
        for line in VIRTUAL_DESKTOP.iter().chain(CANNOT_CONNECT.iter()) {
            echoed("cli-tools-policy.sh", line);
        }
        echoed("cell-socket.sh", DEAD_PASSPORT);
        let cell = source("cell-socket.sh");
        assert!(
            cell.contains(concat!(
                "echo \"",
                k2_client::product_name!(),
                " bulletin for ${day} is unread: k2 organization bulletin\" >&2"
            )),
            "the bulletin line moved"
        );
        assert!(
            cell.contains("if [ $((now - last)) -ge 1800 ]; then"),
            "the bulletin's half hour moved"
        );
        assert!(
            cell.contains(r#"stamp="${_K2_NOTE_DIR%/}/k2-bulletin.${key}""#),
            "the bulletin's stamp file moved"
        );
    }

    #[test]
    fn the_bulletin_mark() {
        let at = |s: &str| bulletin(s.as_bytes());
        assert_eq!(
            at(r#"{"ok":true,"bulletin":{"day":"2026-11-17","unread":true,"key":"ab12"}}"#),
            Some(("2026-11-17".into(), "ab12".into()))
        );
        assert_eq!(
            at(r#"{"bulletin":{"day":"2026-11-17","unread":false,"key":"ab12"}}"#),
            None
        );
        assert_eq!(
            at(r#"{"bulletin":{"day":"x","unread":true,"key":"ab12"}}"#),
            None
        );
        assert_eq!(
            at(r#"{"bulletin":{"day":"1","unread":true,"key":"AB"}}"#),
            None
        );
        assert_eq!(
            at(r#"{"bulletin":{"day":"1","unread":true,"key":"ab"#),
            None
        );
        assert_eq!(
            at(r#"{"bulletin": {"day":"1","unread":true,"key":"ab"}}"#),
            None,
            "the shell's pattern has no spaces"
        );
        // The first place it matches.
        assert_eq!(
            at(
                r#"{"a":{"bulletin":{"day":"","unread":true,"key":"1"}},"bulletin":{"day":"2","unread":true,"key":"3"}}"#
            ),
            Some(("2".into(), "3".into()))
        );
        assert_eq!(at(""), None);
    }

    /// `_k2_note_dir`'s order and its fallback, and its lines pinned.
    #[test]
    fn the_note_folder_is_the_shell_s() {
        let dir = std::env::temp_dir().join(format!("k5-notedir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(note_dir(Some(dir.clone().into())), dir);
        let mut slash = dir.clone().into_os_string();
        slash.push("/");
        assert_eq!(note_dir(Some(slash)), dir, "one trailing / goes");
        // A TMPDIR that is missing, empty or unset: the next usable one.
        let fallback = note_dir(None);
        assert!(
            fallback == std::path::Path::new("/tmp")
                || fallback == std::path::Path::new("/var/tmp")
        );
        assert_eq!(note_dir(Some(dir.join("gone").into())), fallback);
        assert_eq!(note_dir(Some("".into())), fallback);
        std::fs::remove_dir_all(&dir).unwrap();
        let cell = source("cell-socket.sh");
        for line in [
            r#"    for d in "${TMPDIR:-}" /tmp /var/tmp; do"#,
            r#"        if [ -n "$d" ] && [ -d "$d" ] && [ -w "$d" ] && [ -x "$d" ]; then"#,
            r#"            _K2_NOTE_DIR="${d%/}""#,
            r#"            [ -n "$_K2_NOTE_DIR" ] || _K2_NOTE_DIR="/""#,
            r#"    _K2_NOTE_DIR="/tmp""#,
        ] {
            assert!(
                cell.lines().any(|l| l == line),
                "_k2_note_dir moved: no line {line:?}"
            );
        }
    }

    #[test]
    fn the_bulletin_line_once_per_half_hour() {
        let dir = std::env::temp_dir().join(format!("k5-bulletin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stamp = dir.join("k2-bulletin.k1");
        bulletin_line("d", "k1", Some(dir.clone().into()), 5000);
        assert_eq!(std::fs::read_to_string(&stamp).unwrap(), "5000\n");
        bulletin_line(
            "d",
            "k1",
            Some(dir.clone().into()),
            5000 + BULLETIN_EVERY - 1,
        );
        assert_eq!(
            std::fs::read_to_string(&stamp).unwrap(),
            "5000\n",
            "shown again too soon"
        );
        bulletin_line("d", "k1", Some(dir.clone().into()), 5000 + BULLETIN_EVERY);
        assert_eq!(
            std::fs::read_to_string(&stamp).unwrap(),
            format!("{}\n", 5000 + BULLETIN_EVERY)
        );
        std::fs::write(&stamp, "junk\n").unwrap();
        bulletin_line("d", "k1", Some(dir.clone().into()), 2000);
        assert_eq!(
            std::fs::read_to_string(&stamp).unwrap(),
            "2000\n",
            "junk counts as never shown"
        );
        // A stamp from the future: not shown.
        std::fs::write(&stamp, " 9000\t\nx\n").unwrap();
        bulletin_line("d", "k1", Some(dir.clone().into()), 8000);
        assert_eq!(std::fs::read_to_string(&stamp).unwrap(), " 9000\t\nx\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn settings(port: Option<&str>, auth: Auth, no_owner_fallback: bool) -> Settings {
        Settings {
            host: "127.0.0.1".into(),
            port: port.and_then(|p| p.parse().ok()),
            port_text: port.map(str::to_string),
            no_owner_fallback,
            project: "/p".into(),
            auth,
            owner_from_disk: false,
            session: false,
            secrets_dir: None,
            home: "/h".into(),
        }
    }

    #[test]
    fn a_gated_help_line_is_the_shell_s() {
        let line = |w: &[&str]| -> Vec<std::ffi::OsString> { w.iter().map(Into::into).collect() };
        let owner = settings(Some("1"), Auth::Owner("o".into()), false);
        let guarded = settings(Some("1"), Auth::None, true);
        let no_port = settings(None, Auth::Owner("o".into()), false);
        for help in [
            &["read", "--help"][..],
            &["inbox", "list", "-h"],
            &["x", "-h", "y"],
        ] {
            assert!(!help_line_gated(&line(help), &owner), "{help:?}");
            assert!(help_line_gated(&line(help), &guarded), "{help:?}");
            assert!(help_line_gated(&line(help), &no_port), "{help:?}");
        }
        for plain in [&["read", "acme"][..], &["setup", "help"], &["x", "--helpx"]] {
            assert!(!help_line_gated(&line(plain), &guarded), "{plain:?}");
            assert!(!help_line_gated(&line(plain), &no_port), "{plain:?}");
        }
    }

    #[test]
    fn the_gates_in_the_shell_s_order() {
        let passport = || Auth::Passport {
            passport: crate::transport::Passport::new("s.x").unwrap(),
            sock: None,
        };
        assert_eq!(
            gate(&settings(Some("1"), Auth::Owner("o".into()), false)),
            None
        );
        assert_eq!(gate(&settings(Some("1"), passport(), true)), None);
        assert_eq!(
            gate(&settings(Some("1"), Auth::None, true)),
            Some(Gate::VirtualDesktop)
        );
        assert_eq!(
            gate(&settings(None, Auth::None, true)),
            Some(Gate::VirtualDesktop),
            "the guard comes first"
        );
        assert_eq!(
            gate(&settings(None, Auth::Owner("o".into()), false)),
            Some(Gate::CannotConnect)
        );
        assert_eq!(
            gate(&settings(Some(""), passport(), false)),
            Some(Gate::CannotConnect)
        );
        assert_eq!(
            gate(&settings(Some("1"), Auth::None, false)),
            Some(Gate::CannotConnect)
        );
        // A port that is no number still passes the gate: curl refuses it.
        assert_eq!(gate(&settings(Some("abc"), passport(), false)), None);
        assert_eq!(
            (
                Gate::VirtualDesktop.exit().code(),
                Gate::CannotConnect.exit().code()
            ),
            (3, 1)
        );
    }
}
