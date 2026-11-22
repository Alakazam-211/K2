//! `k5 daemon install`, as the shell CLI's `cmd_daemon_install` answers
//! it (`cli/src/daemon-install.sh`): fetch the standalone daemon of a
//! signed release, check it with minisign against the update keys and
//! its sha256, put it in a bin folder and write a supervisor unit (or,
//! on Linux, update the binary an installed unit runs and restart it).
//!
//! The steps are the shell's, in its order, with its words:
//!
//! 1. `_k2_refuse_if_airgap` (exit 1), then the words: `--version`,
//!    `--bin-dir` and `--manifest-url` take a value (one as the last word
//!    is `_need_value`'s "Error: <flag> takes a value", exit 2),
//!    `--no-service`, `--dry-run`; any other word is exit 2.
//! 2. The manifest's URL (`--manifest-url`, else the release of
//!    `--version`, else the newest), a version older than
//!    [`FIRST_U2_VERSION`] refused, the platform from `uname -s` and
//!    `uname -m`.
//! 3. The manifest: a `file://` one read with `cat`, else fetched
//!    ([`fetch`]) into a `mktemp` file and read with `cat`.
//!    Its fields are read as the shell's `sed` reads them ([`field`],
//!    [`top_version`]).
//! 4. Linux, unless `--no-service`: an installed unit
//!    (`_daemon_linux_unit`) means its `ExecStart=` binary is the one to
//!    replace ([`exec_path`]).
//! 5. `--dry-run` prints what it would do. Else: `minisign` on `PATH`, a
//!    `mktemp -d` folder, the binary and its `sig2` signature fetched
//!    ([`fetch`], `cp` for `file://`, or the manifest's own text),
//!    a signature that is not minisig text base64-decoded with `base64`,
//!    then [`verify`]: prehashed only, `minisign -V` with each update key,
//!    and the signed trusted comment naming exactly this asset and
//!    version. Then the sha256 (`sha256sum`, else `shasum -a 256`).
//! 6. `mkdir -p`, `cp` to `<bin>.new`, `chmod +x`, `mv -f` over the
//!    binary; then the unit's restart, or the systemd user unit or the
//!    launchd plist written, and the closing lines.
//!
//! Every program is found on `PATH` by name, as the shell finds it, with
//! the shell's arguments, so the parity rig's stand-ins answer for each.
//! The binary runs the same tools the shell runs for each step that
//! fetches, verifies or moves a file, so a failure prints the tool's own
//! words. The temporary folder is removed on every way out, as the
//! shell's EXIT trap does.
//!
//! No token, passport or daemon answer is involved: install talks to no
//! daemon.

use super::life::{launchd, user_home};
use super::{capture, linux_unit, on_path, systemctl};
use crate::daemon::Settings;
use crate::exit::Exit;
use crate::families::update;
use crate::offline::Env;
use crate::out;
use k2_client::brand::{
    CLI_NAME as CLI, DAEMON_FEED_URL, PRODUCT_NAME as PRODUCT, RELEASE_DOWNLOAD_BASE,
};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The update keys (`_DAEMON_UPDATE_PUBKEYS`): primaries first, the
/// offline backup last. The daemon's own list,
/// `k2_release_keys::UPDATER_KEYS`, compiled from this build's brand file
/// (`[update]`): K2's keys in a k2 or k5 build, the brand's own in any
/// other. A test pins the k2 brand's keys to the shell's line.
pub const UPDATE_KEYS: &[k2_release_keys::TrustedKey] = k2_release_keys::UPDATER_KEYS;

/// The first release signed with the update keys
/// (`_DAEMON_FIRST_U2_VERSION`).
pub const FIRST_U2_VERSION: &str = "0.46.6";

/// Writes a line on stdout; a broken stdout ends the command as the
/// shell's `echo` does.
macro_rules! say {
    ($($t:tt)*) => {
        if let Err(e) = out::stdout_line(&format!($($t)*)) {
            return Some(out::after_stdout_error(&e));
        }
    };
}

/// Writes lines on stderr, then returns `exit`.
macro_rules! fail {
    ($exit:expr; $($line:expr),+ $(,)?) => {{
        $(out::stderr_line(&$line);)+
        return Some($exit);
    }};
}

/// `install`'s flags.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Opts {
    pub version: String,
    pub bin_dir: String,
    pub manifest_url: String,
    pub no_service: bool,
    pub dry_run: bool,
}

/// The words after `install`, read as the shell's loop reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Words {
    Opts(Opts),
    /// A word the loop doesn't take: exit 2.
    Unknown(String),
    /// A value flag as the last word (`_need_value`): "Error: <flag>
    /// takes a value", exit 2.
    Dangling(String),
}

/// `cmd_daemon_install`'s `while` loop (`-h` and `--help` never get
/// here: the front hands those lines to the shell).
pub fn parse(rest: &[String]) -> Words {
    let mut o = Opts::default();
    let mut i = 0;
    while i < rest.len() {
        let w = rest[i].as_str();
        match w {
            "--version" | "--bin-dir" | "--manifest-url" => {
                let Some(v) = rest.get(i + 1) else {
                    return Words::Dangling(w.to_string());
                };
                let v = v.clone();
                match w {
                    "--version" => o.version = v,
                    "--bin-dir" => o.bin_dir = v,
                    _ => o.manifest_url = v,
                }
                i += 2;
            }
            "--no-service" => {
                o.no_service = true;
                i += 1;
            }
            "--dry-run" => {
                o.dry_run = true;
                i += 1;
            }
            _ => return Words::Unknown(w.to_string()),
        }
    }
    Words::Opts(o)
}

/// `_daemon_version_lt <a> <b>`: `a` is an older `x.y.z` than `b`; a
/// version that is not `x.y.z` (a `v` before it allowed) is older. Each
/// part is compared as awk compares `x + 0`: as a number.
pub fn version_lt(a: &str, b: &str) -> bool {
    fn parts(s: &str) -> Option<[f64; 3]> {
        let s = s.strip_prefix('v').unwrap_or(s);
        let p: Vec<&str> = s.split('.').collect();
        if p.len() != 3
            || p.iter()
                .any(|x| x.is_empty() || !x.bytes().all(|c| c.is_ascii_digit()))
        {
            return None;
        }
        Some([p[0].parse().ok()?, p[1].parse().ok()?, p[2].parse().ok()?])
    }
    let (Some(x), Some(y)) = (parts(a), parts(b)) else {
        return true;
    };
    for (p, q) in x.iter().zip(y.iter()) {
        if p < q {
            return true;
        }
        if p > q {
            return false;
        }
    }
    false
}

/// `[[:space:]]` in the C locale, less the newline `tr` already turned
/// into a space.
fn space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Where `"<name>"`, spaces, `:`, spaces and then `open` match at `at`:
/// the index just past `open`.
fn key_at(hay: &[u8], at: usize, name: &[u8], open: u8) -> Option<usize> {
    let mut i = at;
    if hay.get(i) != Some(&b'"') || !hay[i + 1..].starts_with(name) {
        return None;
    }
    i += 1 + name.len();
    if hay.get(i) != Some(&b'"') {
        return None;
    }
    i += 1;
    while hay.get(i).copied().is_some_and(space) {
        i += 1;
    }
    if hay.get(i) != Some(&b':') {
        return None;
    }
    i += 1;
    while hay.get(i).copied().is_some_and(space) {
        i += 1;
    }
    (hay.get(i) == Some(&open)).then_some(i + 1)
}

/// `sed -n 's/.*"<name>"[[:space:]]*:[[:space:]]*<open>\([^<close>]*\)<close>.*/\1/p'`
/// on one line: the greedy `.*` makes it the last place that matches.
fn last_match<'a>(hay: &'a [u8], name: &str, open: u8, close: u8) -> Option<&'a [u8]> {
    (0..hay.len()).rev().find_map(|at| {
        let start = key_at(hay, at, name.as_bytes(), open)?;
        let len = hay[start..].iter().position(|b| *b == close)?;
        Some(&hay[start..start + len])
    })
}

/// `tr '\n' ' '`.
fn one_line(text: &[u8]) -> Vec<u8> {
    text.iter()
        .map(|b| if *b == b'\n' { b' ' } else { *b })
        .collect()
}

/// `_daemon_manifest_version`: the last `"version": "…"` anywhere.
pub fn top_version(manifest: &[u8]) -> String {
    let line = one_line(manifest);
    last_match(&line, "version", b'"', b'"')
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_default()
}

/// `_daemon_manifest_field <json> <platform> <field>`: the last
/// `"<platform>": {…}` object (no `}` inside it), then the last
/// `"<field>": "…"` in it. Empty when either is missing.
pub fn field(manifest: &[u8], platform: &str, name: &str) -> String {
    let line = one_line(manifest);
    let Some(obj) = last_match(&line, platform, b'{', b'}').filter(|o| !o.is_empty()) else {
        return String::new();
    };
    last_match(obj, name, b'"', b'"')
        .map(|v| String::from_utf8_lossy(v).into_owned())
        .unwrap_or_default()
}

/// `_daemon_unit_exec_path`'s reading of `systemctl cat`'s output: the
/// last non-empty `ExecStart=` value, its first word (up to a space),
/// without systemd's `-@:+!` prefixes, every `%h` made `home`.
pub fn exec_path(cat: &str, home: &str) -> Option<String> {
    let line = cat
        .split('\n')
        .filter_map(|l| {
            let l = l.trim_start_matches(|c: char| c.is_ascii() && space(c as u8));
            let v = l.strip_prefix("ExecStart=")?;
            Some(v.trim_start_matches(|c: char| c.is_ascii() && space(c as u8)))
        })
        .rfind(|v| !v.is_empty())?;
    let word = line.split(' ').next().unwrap_or("");
    let word = word.trim_start_matches(['-', '@', ':', '+', '!']);
    let word = word.replace("%h", home);
    (!word.is_empty()).then_some(word)
}

/// `dirname <path>`.
pub fn dirname(p: &str) -> String {
    let b = p.as_bytes();
    let mut end = b.len();
    while end > 1 && b[end - 1] == b'/' {
        end -= 1;
    }
    while end > 0 && b[end - 1] != b'/' {
        end -= 1;
    }
    if end == 0 {
        return ".".into();
    }
    while end > 1 && b[end - 1] == b'/' {
        end -= 1;
    }
    p[..end].to_string()
}

/// `$(prog args…)` with stderr shown: its stdout less trailing newlines
/// (and the NULs the shell drops), and its exit code (`128 + n` for a
/// signal, 127 when it can't start).
fn output(prog: &str, args: &[&str]) -> (Vec<u8>, i32) {
    output_os(prog, &args.iter().map(OsStr::new).collect::<Vec<_>>())
}

/// [`output`] with arguments that need not be UTF-8.
fn output_os(prog: &str, args: &[&OsStr]) -> (Vec<u8>, i32) {
    match Command::new(prog)
        .args(args)
        .stderr(Stdio::inherit())
        .output()
    {
        Ok(o) => {
            let mut b: Vec<u8> = o.stdout.into_iter().filter(|c| *c != 0).collect();
            while b.last() == Some(&b'\n') {
                b.pop();
            }
            (b, code_of(o.status))
        }
        Err(_) => (Vec::new(), 127),
    }
}

/// `prog args…` with this process's stdin, stdout and stderr: its exit
/// code.
fn status(prog: &str, args: &[&str]) -> i32 {
    match Command::new(prog).args(args).status() {
        Ok(s) => code_of(s),
        Err(_) => 127,
    }
}

/// `prog args… 2>/dev/null`, stdout shown: whether it exited 0.
fn quiet_ok(prog: &str, args: &[&str]) -> bool {
    Command::new(prog)
        .args(args)
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn code_of(s: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(sig) = s.signal() {
            return 128 + sig;
        }
    }
    s.code().unwrap_or(1)
}

/// `_daemon_fetch <url> <file>`: download `url` into `file`. A URL on
/// one of GitHub's hosts (the release's manifest, binary and signature)
/// follows only GitHub's own redirects, https only, at most
/// [`update::MAX_REDIRS`] ([`github_get`]). Any other URL is one the
/// person or their manifest named (a mirror, `--manifest-url`): `curl
/// -fsSL`. Whether it got it.
fn fetch(url: &str, file: &str) -> bool {
    if update::github_url_ok(url.as_bytes()) {
        github_get(url, file)
    } else {
        status("curl", &["-fsSL", url, "-o", file]) == 0
    }
}

/// `_k2_github_get <url> <file>`: `_k2_github_curl "$1" -fsS -o "$2"`.
/// One curl per hop, `--no-location --proto =https`, where a redirect
/// points read from its stdout (`-w %{redirect_url}`); curl's own error
/// on stderr. A redirect off GitHub's hosts, to anything but https, or
/// past the cap is refused before it is contacted: `Error:` and the
/// reason ([`update::refusal`]). No token is sent.
fn github_get(url: &str, file: &str) -> bool {
    let mut url = OsString::from(url);
    let mut hops = 0;
    loop {
        let mut args: Vec<&OsStr> = update::CURL_GUARD.iter().map(|a| OsStr::new(*a)).collect();
        args.extend(["-fsS", "-o", file].map(OsStr::new));
        args.extend(update::CURL_WRITE_OUT.iter().map(|a| OsStr::new(*a)));
        args.push(url.as_os_str());
        let (next, code) = output_os("curl", &args);
        if code != 0 {
            return false;
        }
        if next.is_empty() {
            return true;
        }
        hops += 1;
        if let Some(why) = update::refusal(hops, &next) {
            out::stderr_line(&format!("Error: {why}"));
            return false;
        }
        url = os_of(next);
    }
}

/// The bytes `$(…)` kept, as a path or argument.
fn os_of(bytes: Vec<u8>) -> OsString {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        OsString::from_vec(bytes)
    }
    #[cfg(not(unix))]
    {
        OsString::from(String::from_utf8_lossy(&bytes).into_owned())
    }
}

/// `${TMPDIR:-/tmp}`.
fn tmp_root() -> String {
    std::env::var_os("TMPDIR")
        .filter(|t| !t.is_empty())
        .map(|t| t.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/tmp".into())
}

/// The exit for a failed command under the shell's `set -e`: its code.
fn set_e(code: i32) -> Exit {
    Exit::from_curl(code)
}

/// The `mktemp -d` folder, removed when this goes (the shell's
/// `trap "rm -rf '$tmp'" EXIT`).
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// `_daemon_retired_key_msg`.
fn retired_lines() -> [String; 2] {
    [
        format!("Versions before {FIRST_U2_VERSION} are signed with the retired update key, which this {CLI} doesn't trust."),
        format!("Install {FIRST_U2_VERSION} or newer."),
    ]
}

/// `_daemon_platform_key`: `<os>-<arch>`, or its error line.
fn platform() -> Result<String, String> {
    let (s, _) = capture("uname", &["-s"]);
    let os = match s.as_str() {
        "Darwin" => "macos",
        "Linux" => "linux",
        _ => {
            return Err(format!(
                "Unsupported OS for standalone daemon: {s} (expected Darwin or Linux)"
            ))
        }
    };
    let (m, _) = capture("uname", &["-m"]);
    let arch = match m.as_str() {
        "arm64" | "aarch64" => "aarch64",
        "x86_64" | "amd64" => "x86_64",
        _ => {
            return Err(format!(
                "Unsupported CPU arch for standalone daemon: {m} (expected arm64/aarch64 or x86_64)"
            ))
        }
    };
    Ok(format!("{os}-{arch}"))
}

/// `_k2_refuse_if_airgap`: its line, then exit 1, when air-gap is on;
/// `Some(None)` when it is off. `None`: the shell's (no `HOME`).
pub(super) fn airgap_refusal() -> Option<Option<Exit>> {
    let (state, file) = crate::airgap::state(&Env::current())?;
    Some(crate::airgap::refusal(state, &file).map(|mut line| {
        line.push(b'\n');
        let _ = out::write_flushed(&mut std::io::stderr().lock(), &line);
        Exit::Error
    }))
}

/// The lines of `grep '^<prefix>'` over the trusted comment split at
/// its tabs, as `$(…)` keeps them.
fn tagged(tc: &str, prefix: &str) -> String {
    format!("{tc}\n")
        .replace('\t', "\n")
        .split('\n')
        .filter(|l| l.starts_with(prefix))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `_daemon_verify_update_sig <bin> <minisig> <asset> <version>`: `Ok`
/// when one update key signed it and its trusted comment names exactly
/// this asset and version; else the line that says why.
pub fn verify(bin: &str, sig: &str, asset: &str, ver: &str) -> Result<(), String> {
    // `sed -n 2p`: the second line.
    let text = std::fs::read(sig).unwrap_or_default();
    let second = text.split(|b| *b == b'\n').nth(1).unwrap_or_default();
    if !(second.starts_with(b"RU") && matches!(second.get(2), Some(b'Q' | b'R' | b'S' | b'T'))) {
        return Err("The signature is not a prehashed minisign signature. Refusing.".into());
    }
    for k in UPDATE_KEYS {
        let o = match Command::new("minisign")
            .args(["-V", "-P", k.public_key_b64, "-m", bin, "-x", sig, "-Q"])
            .stderr(Stdio::null())
            .output()
        {
            Ok(o) if o.status.success() => o,
            _ => continue,
        };
        let mut tc: Vec<u8> = o.stdout.into_iter().filter(|c| *c != 0).collect();
        while tc.last() == Some(&b'\n') {
            tc.pop();
        }
        let tc = String::from_utf8_lossy(&tc);
        let files = tagged(&tc, "file:");
        let vers = tagged(&tc, "version:");
        if files != format!("file:{asset}") {
            let f = files.strip_prefix("file:").unwrap_or(&files);
            return Err(format!(
                "The signature is for '{f}', not {asset}. Refusing."
            ));
        }
        if vers != format!("version:{ver}") {
            let v = vers.strip_prefix("version:").unwrap_or(&vers);
            return Err(format!(
                "The signature is for version '{v}', not {ver}. Refusing."
            ));
        }
        return Ok(());
    }
    Err(format!("Not signed by a {PRODUCT} update key."))
}

/// `_daemon_sha256 <file>`: the hex, or `None` (the tool's own words, or
/// the line when there is no tool, already on stderr).
fn sha256(file: &str) -> Option<String> {
    let (o, code) = if on_path("sha256sum") {
        output("sha256sum", &[file])
    } else if on_path("shasum") {
        output("shasum", &["-a", "256", file])
    } else {
        out::stderr_line("Neither sha256sum nor shasum found \u{2014} cannot verify checksum.");
        return None;
    };
    // `| awk '{print $1}'`, under pipefail.
    let text = String::from_utf8_lossy(&o);
    let mut hex = text
        .split('\n')
        .map(|l| l.split([' ', '\t']).find(|w| !w.is_empty()).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n");
    while hex.ends_with('\n') {
        hex.pop();
    }
    (code == 0).then_some(hex)
}

/// The systemd user unit the shell writes, for `bin`.
pub fn unit_text(bin: &str) -> String {
    format!(
        r#"[Unit]
Description={PRODUCT} standalone daemon
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={bin}
Restart=always
RestartSec=2
# Agents run in this unit's cgroup. The default OOMPolicy=stop turns one
# OOM-killed agent into a stop + restart of the whole daemon; continue lets
# only that process die. (The stage H drop-in carries the same line.)
OOMPolicy=continue
# KillMode=process (k2-ptyd, prd-k2-ptyd-v1 P87): a stop, restart or crash
# signals only the daemon. k2-ptyd, the agents it hosts, frpc and Apps live
# on in the unit's cgroup and the next daemon adopts them; an explicit stop
# ends the agents itself (the daemon reads the systemd stop job). With
# control-group a restart would kill every agent. Runtime stage H's drop-in
# (<unit>.d/k2-handoff.conf) carries the same line, plus NotifyAccess=main
# and ExecReload=SIGUSR2 for the no-downtime systemctl reload.
KillMode=process
# A hand-off successor (systemctl reload) is not systemd's child: systemd
# can't wait for it and would SIGKILL it at stop/restart, skipping its
# graceful shutdown (k2-ptyd: a stop must end agents, a restart keeps
# them). systemd waits for its own ExecStop: SIGTERM the main process
# and wait until it is gone. The daemon hands off only with this line.
ExecStop=/bin/sh -c 'kill -TERM "$$MAINPID" 2>/dev/null; while kill -0 "$$MAINPID" 2>/dev/null; do sleep 0.2; done' k2-wait-mainpid
# The unit's own runtime dir (/run/k2; XDG_RUNTIME_DIR/k2 for a user unit)
# holds frpc.toml and agents' secret files: systemd-logind deletes
# /run/user/<uid> and the user's /dev/shm files when their last login
# session ends. Preserve keeps it across restarts (frpc and agents outlive
# the daemon process). (The stage H drop-in carries the same lines.)
RuntimeDirectory=k2
RuntimeDirectoryMode=0700
RuntimeDirectoryPreserve=yes

[Install]
WantedBy=default.target
"#
    )
}

/// The launchd plist the shell writes, for `bin`, with `label` and the
/// logs under `home`'s K2 folder.
pub fn plist_text(label: &str, bin: &str, home: &str) -> String {
    let k2 = k2_client::paths::HOME_DIR_NAME;
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{bin}</string>
    </array>
    <key>KeepAlive</key>
    <true/>
    <key>RunAtLoad</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{home}/{k2}/daemon.stdout.log</string>
    <key>StandardErrorPath</key>
    <string>{home}/{k2}/daemon.stderr.log</string>
</dict>
</plist>
"#
    )
}

/// `cmd_daemon_install …` after the router's gates; `words` are the
/// words after `install`. `None`: the shell's (no `HOME`).
pub fn run(s: &Settings, words: &[String]) -> Option<Exit> {
    if let Some(stop) = airgap_refusal()? {
        return Some(stop);
    }
    let o = match parse(words) {
        Words::Opts(o) => o,
        Words::Unknown(w) => {
            fail!(Exit::Usage; format!("{CLI} daemon install: unknown argument: {w}"))
        }
        Words::Dangling(flag) => fail!(Exit::Usage; format!("Error: {flag} takes a value")),
    };
    let home = user_home();
    let version = o
        .version
        .strip_prefix('v')
        .unwrap_or(&o.version)
        .to_string();
    let bin_dir_given = !o.bin_dir.is_empty();
    let mut bin_dir = if bin_dir_given {
        o.bin_dir.clone()
    } else {
        format!("{home}/.local/bin")
    };

    // The manifest's URL: the flag, the release asked for, the newest.
    let manifest_url = if !o.manifest_url.is_empty() {
        o.manifest_url.clone()
    } else if !version.is_empty() {
        if version_lt(&version, FIRST_U2_VERSION) {
            let [a, b] = retired_lines();
            fail!(Exit::Error; a, b);
        }
        format!("{RELEASE_DOWNLOAD_BASE}/v{version}/daemon-latest.json")
    } else {
        DAEMON_FEED_URL.to_string()
    };

    let platform = match platform() {
        Ok(p) => p,
        Err(line) => fail!(Exit::Error; line),
    };

    let manifest = if let Some(mpath) = manifest_url.strip_prefix("file://") {
        if !Path::new(mpath).is_file() {
            fail!(Exit::Error; format!("Manifest not found: {mpath}"));
        }
        let (m, code) = output("cat", &[mpath]);
        if code != 0 {
            return Some(set_e(code));
        }
        m
    } else {
        if !on_path("curl") {
            fail!(Exit::Error; "curl is required to fetch the daemon manifest.");
        }
        let template = format!("{}/k2-daemon-manifest.XXXXXX", tmp_root());
        let (mfile, code) = output("mktemp", &[template.as_str()]);
        if code != 0 {
            fail!(Exit::Error; "Failed to create temp file.");
        }
        let mfile = os_of(mfile);
        let mfile = mfile.to_string_lossy();
        if !fetch(&manifest_url, &mfile) {
            let _ = std::fs::remove_file(&*mfile);
            fail!(Exit::Error; format!("Failed to fetch daemon manifest: {manifest_url}"));
        }
        // `manifest=$(cat "$mfile")`: a cat that fails ends it, as `set -e`
        // does, before the `rm -f`.
        let (m, code) = output("cat", &[&*mfile]);
        if code != 0 {
            return Some(set_e(code));
        }
        let _ = std::fs::remove_file(&*mfile);
        m
    };

    let mver = top_version(&manifest);
    let url = field(&manifest, &platform, "url");
    // Only the update keys' signature (`sig2`) counts.
    let sig = field(&manifest, &platform, "sig2");
    let sha = field(&manifest, &platform, "sha256");
    if url.is_empty() || sha.is_empty() || mver.is_empty() {
        let or = |v: &str, none: &str| {
            if v.is_empty() {
                none.to_string()
            } else {
                v.to_string()
            }
        };
        fail!(Exit::Error;
            format!("Manifest has no complete artifact for platform '{platform}'."),
            format!("  url={} sha256={}", or(url.as_str(), "<missing>"), or(sha.as_str(), "<missing>")),
            format!("  (manifest version: {})", or(mver.as_str(), "<unknown>")),
        );
    }
    if sig.is_empty() || version_lt(&mver, FIRST_U2_VERSION) {
        let [a, b] = retired_lines();
        fail!(Exit::Error; a, b);
    }
    if !version.is_empty() && mver != version.strip_prefix('v').unwrap_or(&version) {
        fail!(Exit::Error; format!("The manifest is for {mver}, not the requested {version}. Refusing."));
    }
    let asset = format!("k2-daemon-{platform}");
    let mut bin_path = format!("{bin_dir}/k2-daemon");

    let mac = capture("uname", &["-s"]).0 == "Darwin";
    let svc_path = if mac {
        format!("{home}/Library/LaunchAgents/{}.plist", launchd().0)
    } else {
        format!("{home}/.config/systemd/user/k2-daemon.service")
    };

    // Linux with a unit installed: update the binary it runs, in place.
    let mut existing: Option<(String, String)> = None;
    if !mac && !o.no_service {
        if let Some((scope, unit)) = linux_unit(&s.home).filter(|(_, u)| !u.is_empty()) {
            let ctl = if scope == "user" { "--user " } else { "" };
            let (cat, _) = systemctl(&scope, &["cat", unit.as_str()]);
            let Some(exe) = exec_path(&cat, &home) else {
                fail!(Exit::Error;
                    format!("{CLI} daemon install: found the {scope} unit {unit} but could not read its ExecStart= binary."),
                    format!("  Check it with: systemctl {ctl}cat {unit}"),
                );
            };
            if bin_dir_given && dirname(&exe) != bin_dir {
                say!("Note: --bin-dir {bin_dir} is ignored \u{2014} the installed {scope} unit {unit} runs {exe}, which is updated in place.");
            }
            bin_dir = dirname(&exe);
            bin_path = exe;
            existing = Some((scope, unit));
        }
    }

    if o.dry_run {
        say!("{CLI} daemon install \u{2014} DRY RUN (nothing downloaded or written)");
        say!("  platform key:   {platform}");
        say!("  manifest url:   {manifest_url}");
        say!("  manifest ver:   {mver}");
        say!(
            "  requested ver:  {}",
            if version.is_empty() {
                "<latest>"
            } else {
                &version
            }
        );
        say!("  binary url:     {url}");
        say!("  sha256:         {sha}");
        say!(
            "  sig2 (minisign): {}...",
            sig.split(' ').next().unwrap_or("")
        );
        say!("  install path:   {bin_path}");
        say!("  minisign verify: minisign -V -P <{PRODUCT} update key> (file:{asset} version:{mver})");
        if o.no_service {
            say!("  service:        (skipped \u{2014} --no-service)");
        } else if let Some((scope, unit)) = &existing {
            let ctl = if scope == "user" { "--user " } else { "" };
            say!("  service unit:   installed {scope} unit {unit} (kept; binary updated in place)");
            say!("  restart cmd:    systemctl {ctl}restart {unit}");
        } else if mac {
            say!("  service unit:   launchd \u{2192} {svc_path}");
            say!("  enable cmds:    launchctl bootstrap gui/$(id -u) {svc_path}");
        } else {
            say!("  service unit:   systemd \u{2192} {svc_path}");
            say!("  enable cmds:    systemctl --user daemon-reload && systemctl --user enable --now k2-daemon");
        }
        return Some(Exit::Ok);
    }

    // The real install. minisign is required: never an unverified binary.
    if !on_path("minisign") {
        fail!(Exit::Error;
            "minisign is required to verify the daemon binary, but it was not found.",
            "Install it and re-run:",
            "  macOS:  brew install minisign",
            "  Debian: sudo apt-get install minisign",
            "  Other:  https://jedisct1.github.io/minisign/",
            "Refusing to install an unverified binary.",
        );
    }
    let (tmp, code) = output(
        "mktemp",
        &["-d", &format!("{}/k2-daemon-install.XXXXXX", tmp_root())],
    );
    if code != 0 {
        fail!(Exit::Error; "Failed to create temp dir.");
    }
    let tmp = String::from_utf8_lossy(&tmp).into_owned();
    let _scratch = Scratch(PathBuf::from(&tmp));
    let dl_bin = format!("{tmp}/k2-daemon");
    let dl_sig = format!("{tmp}/k2-daemon.minisig");

    say!("Downloading daemon binary ({platform}, manifest ver {mver})...");
    let got = match url.strip_prefix("file://") {
        Some(src) => status("cp", &[src, &dl_bin]),
        None => i32::from(!fetch(&url, &dl_bin)),
    };
    if got != 0 {
        fail!(Exit::Error; format!("Failed to download binary: {url}"));
    }

    // `sig2`: a URL, a file, or the signature's own text.
    if sig.starts_with("http://") || sig.starts_with("https://") {
        if !fetch(&sig, &dl_sig) {
            fail!(Exit::Error; format!("Failed to download signature: {sig}"));
        }
    } else if let Some(src) = sig.strip_prefix("file://") {
        let code = status("cp", &[src, &dl_sig]);
        if code != 0 {
            return Some(set_e(code));
        }
    } else if let Err(e) = std::fs::write(&dl_sig, format!("{sig}\n")) {
        out::stderr_line(&format!("{dl_sig}: {e}"));
        return Some(Exit::Error);
    }
    let head = std::fs::read(&dl_sig).unwrap_or_default();
    if !head.starts_with(b"untrusted comment") {
        let dec = format!("{dl_sig}.dec");
        let mut decoded = false;
        for args in [["-d", dl_sig.as_str()], ["-D", dl_sig.as_str()]] {
            let Ok(f) = std::fs::File::create(&dec) else {
                continue;
            };
            if Command::new("base64")
                .args(args)
                .stdout(f)
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
            {
                decoded = true;
                break;
            }
        }
        if !decoded {
            decoded = quiet_ok("base64", &["-D", "-i", &dl_sig, "-o", &dec]);
        }
        if decoded {
            let code = status("mv", &[&dec, &dl_sig]);
            if code != 0 {
                return Some(set_e(code));
            }
        } else {
            let _ = std::fs::remove_file(&dec);
            fail!(Exit::Error; "Signature is not a minisig and base64-decode failed.");
        }
    }

    say!("Verifying minisign signature ({PRODUCT} update keys)...");
    if let Err(why) = verify(&dl_bin, &dl_sig, &asset, &mver) {
        fail!(Exit::Error;
            why,
            format!("MINISIGN VERIFICATION FAILED for {url}"),
            "Refusing to install. The binary may be corrupt or tampered with.",
        );
    }

    say!("Verifying sha256...");
    let Some(got_sha) = sha256(&dl_bin) else {
        return Some(Exit::Error);
    };
    if got_sha != sha {
        fail!(Exit::Error;
            format!("SHA256 MISMATCH for {url}"),
            format!("  expected: {sha}"),
            format!("  got:      {got_sha}"),
            "Refusing to install.",
        );
    }

    say!("Verified. Installing to {bin_path} ...");
    let new = format!("{bin_path}.new");
    for (prog, args) in [
        ("mkdir", vec!["-p", bin_dir.as_str()]),
        ("cp", vec![dl_bin.as_str(), new.as_str()]),
        ("chmod", vec!["+x", new.as_str()]),
        ("mv", vec!["-f", new.as_str(), bin_path.as_str()]),
    ] {
        let code = status(prog, &args);
        if code != 0 {
            return Some(set_e(code));
        }
    }

    if let Some((scope, unit)) = &existing {
        say!("Restarting the {scope} unit {unit} ...");
        let restarted = if scope == "user" {
            status("systemctl", &["--user", "restart", unit.as_str()]) == 0
        } else {
            quiet_ok("systemctl", &["restart", unit.as_str()])
                || (capture("id", &["-u"]).0 != "0"
                    && on_path("sudo")
                    && quiet_ok("sudo", &["-n", "systemctl", "restart", unit.as_str()]))
        };
        if restarted {
            say!("Restarted {unit} (now running {bin_path}).");
        } else {
            let how = if scope == "system" {
                format!("Restart it as root: sudo systemctl restart {unit}")
            } else {
                format!("Restart it with: systemctl --user restart {unit}")
            };
            fail!(Exit::Error;
                format!("Installed {bin_path}, but could not restart the {scope} unit {unit}."),
                how,
            );
        }
    } else if !o.no_service {
        let code = status("mkdir", &["-p", &dirname(&svc_path)]);
        if code != 0 {
            return Some(set_e(code));
        }
        let text = if mac {
            plist_text(launchd().0, &bin_path, &home)
        } else {
            unit_text(&bin_path)
        };
        if let Err(e) = std::fs::write(&svc_path, text) {
            // The shell's redirect fails with its own script line: a
            // named difference (install-unit-write-error-text).
            out::stderr_line(&format!("{svc_path}: {e}"));
            return Some(Exit::Error);
        }
        if mac {
            say!("Wrote launchd plist: {svc_path}");
            say!("Bootstrap + start it with:");
            say!("  launchctl bootstrap gui/$(id -u) {svc_path}");
        } else {
            say!("Wrote systemd user unit: {svc_path}");
            say!("Enable + start it with:");
            say!("  systemctl --user daemon-reload && systemctl --user enable --now k2-daemon");
        }
    }

    say!("");
    say!("Standalone {PRODUCT} daemon installed: {bin_path}");
    say!("NOTE: pairing this headless box to a {PRODUCT} Connect account from the");
    say!("      CLI is a follow-up \u{2014} token bootstrap is still an open PRD");
    say!("      question, so this installer does NOT pair the daemon. Once");
    say!("      the daemon is running, pair it via the documented {PRODUCT} Connect");
    say!("      flow.");
    Some(Exit::Ok)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> String {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/daemon-install.sh");
        std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
    }

    /// The `pubkey = "..."` lines of a brand file's `[update]` table, in
    /// order.
    fn brand_update_keys(id: &str) -> Vec<String> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../brand/{id}/brand.toml"));
        let text =
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
        let table = text
            .split("\n[")
            .find(|t| t.starts_with("update]\n"))
            .unwrap_or_else(|| panic!("{} has no [update] table", p.display()));
        let keys: Vec<String> = table
            .split("pubkey = \"")
            .skip(1)
            .map(|rest| rest.split('"').next().expect("a closing quote").to_string())
            .collect();
        assert!(!keys.is_empty(), "{} lists no update key", p.display());
        keys
    }

    fn compiled_keys() -> Vec<String> {
        UPDATE_KEYS
            .iter()
            .map(|k| k.public_key_b64.to_string())
            .collect()
    }

    #[test]
    fn the_keys_are_this_build_s_brand_s() {
        let id = k2_client::brand::BRAND_ID;
        assert_eq!(compiled_keys(), brand_update_keys(id), "brand {id}");
        if id != "k2" && id != "k5" {
            // Another brand's k5 binary never carries K2's keys.
            for ours in brand_update_keys("k2") {
                assert!(
                    !compiled_keys().contains(&ours),
                    "brand {id} carries K2's key {ours}"
                );
            }
        }
    }

    #[test]
    fn the_keys_and_the_first_version_are_the_shell_s() {
        let sh = shell();
        // The shell is the k2 brand's: its line is brand/k2's keys, which a
        // k2 build compiles in (the_keys_are_this_build_s_brand_s).
        let keys = format!(
            "_DAEMON_UPDATE_PUBKEYS=\"{}\"\n",
            brand_update_keys("k2").join(" ")
        );
        assert!(
            sh.contains(&keys),
            "the update keys moved in daemon-install.sh"
        );
        let first = format!("_DAEMON_FIRST_U2_VERSION=\"{FIRST_U2_VERSION}\"\n");
        assert!(sh.contains(&first), "the first U2 version moved");
        // The shell builds its release URLs from prelude.sh's repo.
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../cli/src/prelude.sh");
        let prelude =
            std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()));
        let repo = k2_client::brand::RELEASE_REPO;
        assert!(
            prelude.contains(&format!("\n_K2_RELEASE_REPO=\"{repo}\"\n")),
            "prelude.sh's release repo is not {repo}"
        );
        let sh = sh.replace("${_K2_RELEASE_REPO}", repo);
        assert!(sh.contains(&format!("_DAEMON_DEFAULT_LATEST_URL=\"{DAEMON_FEED_URL}\"")));
        assert!(sh.contains(&format!(
            "_DAEMON_VERSION_URL_TMPL=\"{RELEASE_DOWNLOAD_BASE}/v%s/daemon-latest.json\""
        )));
    }

    #[test]
    fn the_unit_and_the_plist_are_the_shell_s() {
        let sh = shell();
        let unit = unit_text("$bin_path").replace("$$MAINPID", "\\$\\$MAINPID");
        let unit = unit.replace(&format!("Description={PRODUCT} "), "Description=K2 ");
        assert!(
            sh.contains(&format!("<<EOF\n{unit}EOF\n")),
            "the unit text moved"
        );
        let plist = plist_text("${_daemon_label}", "$bin_path", "$HOME");
        assert!(
            sh.contains(&format!("<<EOF\n{plist}EOF\n")),
            "the plist text moved"
        );
    }

    #[test]
    fn the_words_are_read_as_the_shell_reads_them() {
        fn w(l: &[&str]) -> Words {
            parse(&l.iter().map(|s| s.to_string()).collect::<Vec<_>>())
        }
        assert_eq!(w(&[]), Words::Opts(Opts::default()));
        assert_eq!(
            w(&[
                "--version",
                "v1.2.3",
                "--dry-run",
                "--bin-dir",
                "",
                "--no-service"
            ]),
            Words::Opts(Opts {
                version: "v1.2.3".into(),
                dry_run: true,
                no_service: true,
                ..Opts::default()
            })
        );
        assert_eq!(
            w(&["--manifest-url"]),
            Words::Dangling("--manifest-url".into())
        );
        assert_eq!(
            w(&["--bogus", "--version"]),
            Words::Unknown("--bogus".into())
        );
        assert_eq!(w(&["--dry-run", "x"]), Words::Unknown("x".into()));
    }

    #[test]
    fn versions_compare_as_awk_does() {
        assert!(version_lt("0.46.5", "0.46.6"));
        assert!(!version_lt("0.46.6", "0.46.6"));
        assert!(!version_lt("v0.47.0", "0.46.6"));
        assert!(!version_lt("1.0.0", "0.46.6"));
        assert!(version_lt("banana", "0.46.6"));
        assert!(version_lt("0.46", "0.46.6"));
        assert!(version_lt("0.46.6.1", "0.46.6"));
        assert!(!version_lt("00.46.010", "0.46.6"));
    }

    #[test]
    fn manifest_fields_are_the_last_match_as_sed_finds_it() {
        let m = br#"{
  "version": "0.46.9",
  "platforms": {
    "linux-x86_64": { "url": "https://a/x", "sha256": "ab", "sig2": "https://a/x.sig2" },
    "macos-aarch64": {"url":"m", "sha256" :	"cd"}
  }
}"#;
        assert_eq!(top_version(m), "0.46.9");
        assert_eq!(field(m, "linux-x86_64", "url"), "https://a/x");
        assert_eq!(field(m, "linux-x86_64", "sig2"), "https://a/x.sig2");
        assert_eq!(field(m, "macos-aarch64", "sha256"), "cd");
        assert_eq!(field(m, "macos-aarch64", "sig2"), "");
        assert_eq!(field(m, "linux-aarch64", "url"), "");
        // The last object of that name wins, and inside it the last field.
        let twice = br#"{"p":{"url":"a","url":"b"},"p":{"url":"c"}}"#;
        assert_eq!(field(twice, "p", "url"), "c");
        assert_eq!(field(br#"{"p":{}}"#, "p", "url"), "");
        // A "version" inside a platform object counts too.
        assert_eq!(
            top_version(br#"{"version":"1.0.0","p":{"version":"2.0.0"}}"#),
            "2.0.0"
        );
    }

    #[test]
    fn the_unit_s_binary_is_read_as_the_shell_reads_it() {
        let cat = "# /x\n[Service]\nExecStart=/old/k2\n  ExecStart=\n ExecStart= -@%h/bin/k2-daemon --flag\n";
        assert_eq!(exec_path(cat, "/H"), Some("/H/bin/k2-daemon".into()));
        assert_eq!(exec_path("ExecStart=\n", "/H"), None);
        assert_eq!(exec_path("[Unit]\n", "/H"), None);
        assert_eq!(exec_path("ExecStart=!!\n", "/H"), None);
    }

    #[test]
    fn dirname_is_coreutils_dirname() {
        for (p, d) in [
            ("/a/b", "/a"),
            ("/a/b/", "/a"),
            ("/a", "/"),
            ("/", "/"),
            ("a", "."),
            ("", "."),
            ("a//b", "a"),
        ] {
            assert_eq!(dirname(p), d, "{p}");
        }
    }
}
