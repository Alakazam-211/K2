//! `open` without `--print`: `_hostchat_open_link OPENER LINK`
//! (`cli/src/hostchat-voice.sh`). The one-use owner link never goes on a
//! command line: the opener gets the path of a small redirect page in a
//! private folder (0700, from `mktemp -d`) that holds the link (0600).
//! The page is removed after a short wait by a child left running (the
//! browser may not have read it when the opener returns), and a page an
//! earlier run left (older than a minute, ours, same prefix) is removed
//! at the start of the next run.
//!
//! The programs the shell runs that matter here run here too: `mktemp`
//! (so its error line is the system's own), the opener, and `sleep` and
//! `rm` for the removal. The link and its code go to none of them: only
//! the page's path and the folder's do.

use crate::out::json::{Text, Value};
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The page folders' name, before mktemp's six characters.
pub const PREFIX: &str = "k2-hostchat-open.";

/// The page's wait when the test switch says nothing usable, and its most.
const WAIT: u64 = 30;

/// The opener this machine has, as the shell picks it: `open` on macOS,
/// else `xdg-open`, else none.
pub fn opener() -> Option<&'static str> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let on_path = |name: &str| crate::daemon::find_on_path(name, path.clone()).is_some();
    if cfg!(target_os = "macos") && on_path("open") {
        Some("open")
    } else if on_path("xdg-open") {
        Some("xdg-open")
    } else {
        None
    }
}

/// `_hostchat_open_link OPENER LINK`: `Err` where the shell returns 1
/// (whatever failed said so itself, or nothing).
pub fn open_link(opener: &str, link: &str) -> Result<(), ()> {
    let base = base();
    sweep(&base);
    let mut template = base.clone().into_vec();
    template.push(b'/');
    template.extend_from_slice(PREFIX.as_bytes());
    template.extend_from_slice(b"XXXXXX");
    let dir = mktemp_d(OsStr::from_bytes(&template)).ok_or(())?;
    let gone = |dir: &Path| {
        let _ = std::fs::remove_dir_all(dir);
    };
    if std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).is_err() {
        gone(&dir);
        return Err(());
    }
    let page = dir.join("open.html");
    if write_page(&page, link).is_err()
        || std::fs::set_permissions(&page, std::fs::Permissions::from_mode(0o600)).is_err()
    {
        gone(&dir);
        return Err(());
    }
    let wait = wait();
    let ran = if opener == "open" {
        Command::new("open").arg(&page).status()
    } else {
        Command::new(opener)
            .arg(&page)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    };
    if !ran.is_ok_and(|s| s.success()) {
        gone(&dir);
        return Err(());
    }
    // `( sleep "$wait"; rm -rf "$dir" ) </dev/null >/dev/null 2>&1 &`:
    // left running when this process ends.
    let _ = Command::new("sh")
        .arg("-c")
        .arg("sleep \"$1\"; rm -rf \"$2\"")
        .arg("sh")
        .arg(&wait)
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    Ok(())
}

/// `${TMPDIR:-/tmp}` with one trailing `/` taken off (`${base%/}`).
fn base() -> OsString {
    let mut b = std::env::var_os("TMPDIR")
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| OsString::from("/tmp"))
        .into_vec();
    if b.last() == Some(&b'/') {
        b.pop();
    }
    OsString::from_vec(b)
}

/// `find "$base" -mindepth 1 -maxdepth 1 -type d -name "<PREFIX>*" -user
/// "$(id -u)" -mmin +1 -exec rm -rf {} +`, quiet: every folder of ours
/// older than a minute. find never follows a link, the folder named
/// itself included.
fn sweep(base: &OsStr) {
    let base = Path::new(base);
    if !std::fs::symlink_metadata(base).is_ok_and(|m| m.is_dir()) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    // SAFETY: geteuid has no preconditions and cannot fail.
    let me = unsafe { libc::geteuid() };
    let now = std::time::SystemTime::now();
    for e in entries.flatten() {
        if !e.file_name().as_bytes().starts_with(PREFIX.as_bytes()) {
            continue;
        }
        let Ok(meta) = std::fs::symlink_metadata(e.path()) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age.as_secs_f64() > 60.0);
        if meta.is_dir() && meta.uid() == me && old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

/// `mktemp -d <template>`: the folder, or `None` where mktemp failed (it
/// said why on stderr).
fn mktemp_d(template: &OsStr) -> Option<PathBuf> {
    let out = Command::new("mktemp")
        .arg("-d")
        .arg(template)
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let mut p = out.stdout;
    while p.last() == Some(&b'\n') {
        p.pop();
    }
    Some(PathBuf::from(OsString::from_vec(p)))
}

/// The page, made 0600 (the shell's `umask 077` and `>`).
fn write_page(page: &Path, link: &str) -> std::io::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(page)?;
    f.write_all(page_text(link).as_bytes())
}

/// What the shell's Python prints for the page: the link escaped for the
/// attribute (`html.escape(link, quote=True)`) and for the script
/// (`json.dumps(link)` with `<` and `>` as `<` and `>`).
pub fn page_text(link: &str) -> String {
    let js = crate::confirm::dumps(&Value::Str(Text::from(link)))
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    format!(
        concat!(
            "<!doctype html><html><head><meta charset=\"utf-8\">",
            "<meta name=\"referrer\" content=\"no-referrer\">",
            "<meta http-equiv=\"refresh\" content=\"0;url={}\">",
            "<title>Opening chat</title></head><body>",
            "<script>location.replace({});</script>",
            "</body></html>\n"
        ),
        html_escape(link),
        js
    )
}

/// `html.escape(s, quote=True)`.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

/// `wait="$(_envn TEST_HOSTCHAT_OPEN_WAIT)"`, then 30 unless it is plain
/// digits no more than 30: the text `sleep` gets.
fn wait() -> OsString {
    let v = k2_client::env_names::first_nonempty_os_in(
        k2_client::env_names::TEST_HOSTCHAT_OPEN_WAIT,
        |n| std::env::var_os(n),
    )
    .map(|(_, v)| v);
    let digits = v.filter(|v| v.as_bytes().iter().all(u8::is_ascii_digit));
    match digits {
        Some(d) if small(d.as_bytes()) => d,
        _ => OsString::from(WAIT.to_string()),
    }
}

/// `[ "$wait" -le 30 ]` for plain digits (leading zeros allowed).
fn small(d: &[u8]) -> bool {
    let d = &d[d.iter().take_while(|b| **b == b'0').count()..];
    d.len() <= 2 && d.iter().fold(0, |n, b| n * 10 + u64::from(b - b'0')) <= WAIT
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_page_escapes_the_link_as_python_does() {
        let p = page_text("https://a.example/k2/owner-handoff#code=k2sho_1&x=<\"'>\u{e9}");
        assert_eq!(
            p,
            concat!(
                "<!doctype html><html><head><meta charset=\"utf-8\">",
                "<meta name=\"referrer\" content=\"no-referrer\">",
                "<meta http-equiv=\"refresh\" content=\"0;url=https://a.example/k2/owner-handoff#code=k2sho_1&amp;x=&lt;&quot;&#x27;&gt;\u{e9}\">",
                "<title>Opening chat</title></head><body>",
                "<script>location.replace(\"https://a.example/k2/owner-handoff#code=k2sho_1&x=\\u003c\\\"'\\u003e\\u00e9\");</script>",
                "</body></html>\n"
            )
        );
    }

    #[test]
    fn the_wait_is_digits_up_to_thirty() {
        assert!(small(b"0"));
        assert!(small(b"30"));
        assert!(small(b"0005"));
        assert!(!small(b"31"));
        assert!(!small(b"300"));
        assert!(!small(b"99999999999999999999999"));
    }
}
