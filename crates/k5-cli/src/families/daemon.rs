//! `k5 daemon status [--json]`, as the shell CLI's `cmd_daemon_status`
//! answers it (`cli/src/daemon.sh`): what this machine's daemon service
//! is doing. `k5 daemon` alone is `daemon status`, as the shell's router
//! takes `status` for a missing or empty word. `start`,
//! `stop`, `restart`, `survive`, `log`, `uninstall` and the usage for any
//! other word are [`life`]'s; `install` is [`install`]'s.
//!
//! What runs, in the shell's order: the settings, the two gates, the
//! `daemon-admin` tool (an `id` tool: a session with no passport is
//! refused, exit 3), then the status. Nothing here changes anything: on
//! Linux the unit comes from the daemon's own cgroup (its pid in
//! `daemon.lock`) or `systemctl [--user] cat`, its state from `systemctl
//! is-active` and `show -p MainPID`; on macOS from `launchctl print`. Then
//! `daemon.port` and `daemon.token` in the K2 home, and one GET of
//! `/boot-status` on that port (no credential, one try, connect 2 s, at
//! most 4 s; the hand-over headers, the owner's source among them) for
//! the k2-ptyd lines.
//!
//! The shell answers these itself ([`run`] returns `None`): `-h` or
//! `--help` anywhere, a word that is not UTF-8, `HOOK_SOCK` set with a
//! `CODEX_` variable exported, a `python3` the binary can't tell, what
//! [`life::parse`] hands on, and, for [`life`]'s verbs, a port that is
//! set but is no port curl takes.

pub mod install;
pub mod life;

use crate::daemon::{self, Settings};
use crate::exit::Exit;
use crate::families::Shown;
use crate::out::json::Value;
use crate::out::pylib::json::loads;
use crate::out::{self, py};
use crate::passport;
use crate::prelude;
use crate::python;
use crate::route::{Owner, Verb};
use k2_client::brand::{CLI_NAME as CLI, PRODUCT_NAME as PRODUCT};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The router arm's words.
pub const WORDS: &[&str] = &["daemon"];
/// Every form comes here.
pub const OWNER: Owner = Owner::Native;
/// No verb rows: every form is [`OWNER`]'s.
pub const VERBS: &[Verb] = &[];

/// What the words ask for.
enum Ask {
    /// `status`, and whether `--json` follows it.
    Status(bool),
    /// A verb of [`life`]'s.
    Life(life::Life),
    /// `install` and the words after it.
    Install(Vec<String>),
}

/// The router's daemon arm: what the words after the noun ask for, or
/// `None` when the shell answers ([`life::parse`]'s). `${2:-status}`: no
/// word, or an empty one, is the status.
fn ask(words: &[&str]) -> Option<Ask> {
    Some(match words {
        [] => Ask::Status(false),
        ["status" | "", rest @ ..] => Ask::Status(rest.first() == Some(&"--json")),
        ["install", rest @ ..] => Ask::Install(rest.iter().map(|w| w.to_string()).collect()),
        [verb, rest @ ..] => Ask::Life(life::parse(verb, rest)?),
    })
}

/// `daemon`'s tool in the shell's policy.
pub const TOOL: &str = "daemon-admin";

/// `k5 daemon [status] ...`: `args` is the whole command line after
/// `argv[0]`.
pub fn run(args: &[OsString]) -> Option<Exit> {
    let hook_sock = crate::families::hook_sock();
    if hook_sock.is_some() && prelude::codex_env(std::env::vars_os().map(|(n, _)| n)) {
        return None;
    }
    let words: Vec<&str> = args
        .iter()
        .skip(1)
        .map(|a| a.to_str())
        .collect::<Option<_>>()?;
    if words.iter().any(|w| matches!(*w, "-h" | "--help")) {
        return None;
    }
    let ask = ask(&words)?;
    if !python::family_native(python::here()) {
        return None;
    }
    let settings = match daemon::current() {
        Ok(s) => s,
        Err(e) => {
            out::stderr_line(&format!("Error: {e}"));
            return Some(Exit::Error);
        }
    };
    // `$BASE_URL` with a port curl wouldn't take: the shell's.
    let port = settings
        .port_text
        .as_deref()
        .and_then(crate::families::curl_port);
    let odd_port = settings.port_text.as_deref().is_some_and(|t| !t.is_empty()) && port.is_none();
    if matches!(ask, Ask::Life(_) | Ask::Install(_)) && odd_port {
        return None;
    }
    // The policy refuses `daemon install` under air-gap before the guard
    // and the session's checks.
    if matches!(ask, Ask::Install(_)) {
        if let Some(stop) = install::airgap_refusal()? {
            return Some(stop);
        }
    }
    match prelude::gate(&settings) {
        // `install` skips the connection gate (DAEMON_INSTALL_EARLY_BYPASS):
        // it provisions a box with no daemon yet. The guard still stands.
        Some(prelude::Gate::CannotConnect) if matches!(ask, Ask::Install(_)) => {}
        Some(gate) => return Some(gate.stop()),
        None => {}
    }
    let would_refuse = passport::current(args).would_refuse();
    if let Some(stop) = prelude::id_tool(&settings, hook_sock.as_deref(), would_refuse, TOOL) {
        return Some(stop);
    }
    match ask {
        Ask::Status(json) => Some(status(&settings, json, would_refuse).print()),
        Ask::Install(words) => install::run(&settings, &words),
        Ask::Life(v) => {
            let mut client = settings.client_on(port?, would_refuse);
            Some(life::run(&settings, &mut client, v))
        }
    }
}

/// What a program printed on stdout, its trailing newlines gone, and
/// whether it exited 0. A program that can't start is a failure.
fn capture(prog: &str, args: &[&str]) -> (String, bool) {
    match Command::new(prog)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
    {
        Ok(o) => {
            let mut t = String::from_utf8_lossy(&o.stdout).into_owned();
            while t.ends_with('\n') {
                t.pop();
            }
            (t, o.status.success())
        }
        Err(_) => (String::new(), false),
    }
}

/// `_daemon_systemctl <scope> <args…>`.
fn systemctl(scope: &str, args: &[&str]) -> (String, bool) {
    let mut all = Vec::with_capacity(args.len() + 1);
    if scope == "user" {
        all.push("--user");
    }
    all.extend_from_slice(args);
    capture("systemctl", &all)
}

/// `command -v <prog>`: on `PATH` as an executable file.
fn on_path(prog: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|d| {
        let p = d.join(prog);
        std::fs::metadata(&p).is_ok_and(|m| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                m.is_file() && m.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                m.is_file()
            }
        })
    })
}

/// `tr -dc '0-9' < daemon.lock`.
fn lock_pid(home: &Path) -> String {
    std::fs::read(home.join("daemon.lock"))
        .map(|b| {
            b.into_iter()
                .filter(u8::is_ascii_digit)
                .map(char::from)
                .collect()
        })
        .unwrap_or_default()
}

/// `_daemon_linux_unit`: `(scope, unit)`, or `None`.
fn linux_unit(home: &Path) -> Option<(String, String)> {
    let pid = lock_pid(home);
    if !pid.is_empty() {
        let cgroup = PathBuf::from(format!("/proc/{pid}/cgroup"));
        if let Ok(text) = std::fs::read(&cgroup) {
            let text = String::from_utf8_lossy(&text);
            let cg = text
                .split('\n')
                .find(|l| l.starts_with("0::"))
                .unwrap_or("")
                .to_string();
            let unit = cg
                .strip_prefix("0::")
                .unwrap_or(&cg)
                .split('/')
                .rfind(|c| c.ends_with(".service") && !c.starts_with("user@"))
                .unwrap_or("")
                .to_string();
            let scope = if cg.contains("/user@") {
                "user"
            } else {
                "system"
            };
            if !unit.is_empty() {
                return Some((scope.into(), unit));
            }
        }
    }
    if !on_path("systemctl") {
        return None;
    }
    for scope in ["user", "system"] {
        for u in ["k2-daemon.service", "k2so-daemon.service"] {
            if systemctl(scope, &["cat", u]).1 {
                return Some((scope.into(), u.into()));
            }
        }
    }
    None
}

/// `tr -d '[:space:]' < <file>`, or nothing.
fn file_text(p: &Path) -> String {
    std::fs::read(p)
        .map(|b| {
            String::from_utf8_lossy(&b)
                .chars()
                .filter(|c| !matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{b}' | '\u{c}'))
                .collect()
        })
        .unwrap_or_default()
}

/// `[ -f daemon.token ] && [ -s daemon.token ]`.
fn token_present(home: &Path) -> bool {
    std::fs::metadata(home.join("daemon.token")).is_ok_and(|m| m.is_file() && m.len() > 0)
}

/// `_daemon_status_ptyd <port>`: `(ptyd, adopted)`, or `None`. Its
/// `_k2_curl` sends no credential but the hand-over headers, the owner's
/// source among them.
fn ptyd(s: &Settings, port: &str, would_refuse: bool) -> Option<(String, String)> {
    if port.is_empty() || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let port: u16 = port.parse().ok().filter(|p| *p != 0)?;
    let local = Settings {
        host: "127.0.0.1".to_string(),
        ..s.clone()
    };
    let mut c = local.client_on(port, would_refuse);
    let r = c.get_update_version("/boot-status").ok()?;
    let raw = String::from_utf8_lossy(&r.body);
    let raw = if raw.is_empty() { "null".into() } else { raw };
    let b = loads(&raw).ok()?;
    let Value::Object(pairs) = &b else {
        return None;
    };
    let Some(p @ Value::Str(_)) = crate::families::get(pairs, "ptyd") else {
        return None;
    };
    let adopted = match crate::families::get(pairs, "adopted") {
        Some(Value::Int(d)) => d.clone(),
        Some(Value::Bool(b)) => u8::from(*b).to_string(),
        _ => "0".into(),
    };
    Some((py::str_of(p).replace('\t', " "), adopted))
}

fn ptyd_lines(o: &mut String, p: &Option<(String, String)>) {
    if let Some((line, adopted)) = p {
        o.push_str(&format!(
            "  Agents survive: {line}  (k2-ptyd; {CLI} daemon survive status)\n  Came back at the last restart: {adopted}\n"
        ));
    }
}

/// `_daemon_status_json KEY VALUE ...`: one line of JSON, every value a
/// string written as `_setup_jstr` writes it, so a quote, a backslash or
/// a newline in a path, a unit or the daemon's answer stays valid JSON.
/// Plain values give the shell's old `printf '%s'` bytes.
pub fn json_line(pairs: &[(&str, &str)]) -> String {
    let body: Vec<String> = pairs
        .iter()
        .map(|(k, v)| format!("\"{k}\":{}", crate::families::setup::jstr(v)))
        .collect();
    format!("{{{}}}\n", body.join(","))
}

/// `cmd_daemon_status [--json]`.
pub fn status(s: &Settings, json: bool, would_refuse: bool) -> Shown {
    let home = &s.home;
    let log = home.join("daemon.stdout.log");
    let log = log.to_string_lossy();
    if cfg!(target_os = "linux") {
        let (mut scope, mut unit, mut active, mut pid) = (
            String::new(),
            String::new(),
            "unknown".to_string(),
            String::new(),
        );
        if let Some((sc, u)) = linux_unit(home) {
            active = systemctl(&sc, &["is-active", &u]).0;
            pid = systemctl(&sc, &["show", "-p", "MainPID", "--value", &u]).0;
            if pid == "0" {
                pid.clear();
            }
            scope = sc;
            unit = u;
        }
        let port = file_text(&home.join("daemon.port"));
        let token = if token_present(home) { "yes" } else { "no" };
        let p = ptyd(s, &port, would_refuse);
        if json {
            let (pl, pa) = p.clone().unwrap_or_default();
            return Shown::out(json_line(&[
                ("installed", if unit.is_empty() { "no" } else { "yes" }),
                ("running", if active == "active" { "yes" } else { "no" }),
                ("pid", pid.as_str()),
                ("port", port.as_str()),
                ("token", token),
                ("unit", unit.as_str()),
                ("scope", scope.as_str()),
                ("log", &*log),
                ("ptyd", pl.as_str()),
                ("adopted", pa.as_str()),
            ]));
        }
        let mut o = format!("{PRODUCT} Daemon\n");
        if unit.is_empty() {
            o.push_str("  Unit:          none found (k2-daemon.service, user or system)\n");
        } else {
            o.push_str(&format!("  Unit:          {unit} ({scope})\n"));
            let a = if active.is_empty() {
                "unknown"
            } else {
                &active
            };
            let pid_part = if pid.is_empty() {
                String::new()
            } else {
                format!("  (PID {pid})")
            };
            o.push_str(&format!("  State:         {a}{pid_part}\n"));
        }
        if !port.is_empty() {
            o.push_str(&format!("  Port:          {port}\n"));
        }
        o.push_str(&format!(
            "  Auth token:    {}\n",
            if token == "yes" { "present" } else { "missing" }
        ));
        ptyd_lines(&mut o, &p);
        return Shown::out(o);
    }
    let user_home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let agents = user_home.join("Library/LaunchAgents");
    let (mut label, mut plist) = ("dev.k2.daemon", agents.join("dev.k2.daemon.plist"));
    let old = agents.join("com.k2so.k2so-daemon.plist");
    if !plist.is_file() && old.is_file() {
        label = "com.k2so.k2so-daemon";
        plist = old;
    }
    let installed = if plist.is_file() { "yes" } else { "no" };
    let uid = capture("id", &["-u"]).0;
    let printed = capture("launchctl", &["print", &format!("gui/{uid}/{label}")]).0;
    let (mut pid, mut last_exit) = (String::new(), String::new());
    if !printed.is_empty() {
        pid = awk_value(&printed, "pid", |v| v.replace(' ', ""));
        last_exit = awk_value(&printed, "last exit code", |v| {
            v.trim_matches(|c| c == ' ' || c == '\t').to_string()
        });
    }
    let running = if !pid.is_empty() && pid != "0" {
        "yes"
    } else {
        "no"
    };
    let port = file_text(&home.join("daemon.port"));
    let token = token_present(home);
    let p = ptyd(s, &port, would_refuse);
    let plist = plist.to_string_lossy();
    if json {
        let (pl, pa) = p.clone().unwrap_or_default();
        return Shown::out(json_line(&[
            ("installed", installed),
            ("running", running),
            ("pid", pid.as_str()),
            ("port", port.as_str()),
            ("token", if token { "yes" } else { "no" }),
            ("last_exit_code", last_exit.as_str()),
            ("plist", &*plist),
            ("log", &*log),
            ("ptyd", pl.as_str()),
            ("adopted", pa.as_str()),
        ]));
    }
    let mut o = format!("{PRODUCT} Daemon\n  Installed:     {installed}  ({plist})\n");
    o.push_str(&format!(
        "  Running:       {running}{}\n",
        if running == "yes" {
            format!("  (PID {pid})")
        } else {
            String::new()
        }
    ));
    if running == "yes" {
        if !port.is_empty() {
            o.push_str(&format!("  Port:          {port}\n"));
        }
        o.push_str(&format!(
            "  Auth token:    {}\n",
            if token { "present" } else { "missing" }
        ));
    }
    ptyd_lines(&mut o, &p);
    if !last_exit.is_empty() && last_exit != "0" {
        o.push_str(&format!(
            "  Last exit code: {last_exit}  (launchd restarted after a non-zero exit)\n"
        ));
    }
    o.push_str(&format!("  Log file:      {log}\n"));
    if installed == "no" {
        o.push_str(&format!(
            "\n  Daemon plist is not installed. Open {PRODUCT} \u{2192} Settings \u{2192} General \u{2192} Server \u{2192} {PRODUCT} server\n  to install the launch agent.\n"
        ));
    }
    Shown::out(o)
}

/// `awk -F'=' '/^[[:space:]]*<key>[[:space:]]*=/{…; print $2; exit}'`:
/// the text between the first and second `=` of the first such line.
fn awk_value(text: &str, key: &str, tidy: impl Fn(&str) -> String) -> String {
    for line in text.split('\n') {
        let rest = line.trim_start_matches(|c: char| c.is_ascii_whitespace());
        let Some(after) = rest.strip_prefix(key) else {
            continue;
        };
        let after = after.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if !after.starts_with('=') {
            continue;
        }
        let field = line.split('=').nth(1).unwrap_or("");
        return tidy(field);
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::out::json::Text;

    fn status_of(words: &[&str]) -> Option<bool> {
        match ask(words) {
            Some(Ask::Status(json)) => Some(json),
            _ => None,
        }
    }

    #[test]
    fn the_noun_alone_or_an_empty_word_is_the_status() {
        assert_eq!(status_of(&[]), Some(false));
        assert_eq!(status_of(&[""]), Some(false));
        assert_eq!(status_of(&["", "--json"]), Some(true));
        assert_eq!(status_of(&["status"]), Some(false));
        assert_eq!(status_of(&["status", "--json"]), Some(true));
        assert_eq!(status_of(&["status", "x", "--json"]), Some(false));
        // `--json` alone is no verb: the usage, as the shell's `*)` arm.
        assert!(matches!(
            ask(&["--json"]),
            Some(Ask::Life(life::Life::Usage))
        ));
    }

    #[test]
    fn plain_values_give_the_shell_s_old_bytes() {
        assert_eq!(
            json_line(&[("installed", "yes"), ("pid", "4242"), ("port", "")]),
            "{\"installed\":\"yes\",\"pid\":\"4242\",\"port\":\"\"}\n"
        );
    }

    #[test]
    fn odd_values_stay_valid_json_and_read_back_as_given() {
        let odd = [
            ("pid", "42\"4\\2"),
            ("ptyd", "on \"q\" back\\slash\nline two"),
            ("port", "12\"3\\4"),
            ("log", "/h/ho\"me\\x/.k2/daemon.stdout.log\t\u{1}"),
        ];
        let line = json_line(&odd);
        assert!(line.ends_with('\n') && !line[..line.len() - 1].contains('\n'));
        assert_eq!(
            line,
            "{\"pid\":\"42\\\"4\\\\2\",\"ptyd\":\"on \\\"q\\\" back\\\\slash\\nline two\",\"port\":\"12\\\"3\\\\4\",\"log\":\"/h/ho\\\"me\\\\x/.k2/daemon.stdout.log\\t\\u0001\"}\n"
        );
        let Ok(Value::Object(pairs)) = loads(line.trim_end()) else {
            panic!("not a JSON object: {line}");
        };
        for (k, v) in odd {
            match crate::families::get(&pairs, k) {
                Some(Value::Str(got)) => assert_eq!(*got, Text::from(v), "{k}"),
                other => panic!("{k}: {other:?}"),
            }
        }
    }
}
