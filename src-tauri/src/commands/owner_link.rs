//! Opens a one-use owner link in the browser without putting it on a
//! command line.
//!
//! The opener runs `open <arg>` on macOS and `xdg-open <arg>` on Linux, and
//! every local account can read a program's arguments. So the link never is
//! that argument: it goes into a small redirect page in a private folder
//! (0700, random name, page 0600), and the opener gets only the page's path.
//! The page is removed after `LIVE.page_life`, longer than the link takes to
//! load and shorter than its life. Folders an earlier call left behind (ours,
//! older than `LIVE.leftover_after`) are removed at the next call. The CLI's
//! `k2 hostchat open` does the same (`_hostchat_open_link`).

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Every folder this module makes starts with this.
const PREFIX: &str = "k2-owner-link.";
/// The page's name inside its folder.
const PAGE: &str = "open.html";

/// How long a page stays, and when a folder counts as a leftover.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Times {
    pub page_life: Duration,
    pub leftover_after: Duration,
}

/// The shipped times: 30 s for the page, a minute for leftovers (a second
/// call inside the first one's 30 s keeps the first page).
pub(crate) const LIVE: Times = Times {
    page_life: Duration::from_secs(30),
    leftover_after: Duration::from_secs(60),
};

/// Opens the owner link (`Settings → Chat → Open chat as owner`, and the
/// answer of a `sew.owner-handoff` permission card). Only K2's own windows
/// may call it; remote pages (Browser pane children) can't reach any app
/// command, because no capability grants a remote origin.
#[tauri::command]
pub async fn open_owner_link(webview: tauri::Webview, link: String) -> Result<(), String> {
    if !is_k2_ui_webview(webview.label()) {
        return Err("Only the app's own windows can open this link.".into());
    }
    let link = checked_link(&link)?;
    let base = std::env::temp_dir();
    tauri::async_runtime::spawn_blocking(move || {
        open_via_page(&base, &link, LIVE, |page| {
            tauri_plugin_opener::open_path(page, None::<&str>).map_err(|e| e.to_string())
        })
        .map(|_| ())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// K2's UI webviews, the same set `capabilities/default.json` lists:
/// `main`, `window-*`, `focus-*`.
pub(crate) fn is_k2_ui_webview(label: &str) -> bool {
    label == "main" || label.starts_with("window-") || label.starts_with("focus-")
}

/// The link as an absolute https URL, or an error that never repeats it.
pub(crate) fn checked_link(link: &str) -> Result<String, String> {
    let parsed = url::Url::parse(link.trim()).map_err(|_| "The link isn't a web address.".to_string())?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() {
        return Err("The link isn't an https address.".into());
    }
    Ok(parsed.as_str().to_string())
}

/// Writes the redirect page for `link` in a new private folder under `base`,
/// hands only the page's path to `open`, and removes the folder after
/// `times.page_life`. Returns the folder. Leftovers are swept first.
pub(crate) fn open_via_page<F>(base: &Path, link: &str, times: Times, open: F) -> Result<PathBuf, String>
where
    F: FnOnce(&Path) -> Result<(), String>,
{
    sweep_leftovers(base, times.leftover_after);
    let dir = make_private_dir(base).map_err(|e| format!("Couldn't make the page's folder: {e}"))?;
    let page = dir.join(PAGE);
    if let Err(e) = write_private_page(&page, &redirect_page(link)) {
        let _ = fs::remove_dir_all(&dir);
        return Err(format!("Couldn't write the page: {e}"));
    }
    if let Err(e) = open(&page) {
        let _ = fs::remove_dir_all(&dir);
        return Err(e);
    }
    let gone = dir.clone();
    let life = times.page_life;
    // If the thread can't start, the next call's sweep removes the folder.
    let _ = std::thread::Builder::new().name("owner-link-page".into()).spawn(move || {
        std::thread::sleep(life);
        let _ = fs::remove_dir_all(&gone);
    });
    Ok(dir)
}

/// The page: a no-referrer redirect to `link` (script first, meta refresh
/// as the fallback) and nothing else.
pub(crate) fn redirect_page(link: &str) -> String {
    let js = serde_json::to_string(link)
        .expect("a string always serializes")
        .replace('<', "\\u003c")
        .replace('>', "\\u003e");
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"referrer\" content=\"no-referrer\">\
         <meta http-equiv=\"refresh\" content=\"0;url={}\">\
         <title>Opening the link</title></head><body>\
         <script>location.replace({});</script>\
         </body></html>\n",
        html_attr(link),
        js
    )
}

fn html_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            c => out.push(c),
        }
    }
    out
}

/// A new folder with a random name, 0700. `create` (not `create_all`)
/// fails if the name exists, so nobody can hand us a folder in advance.
fn make_private_dir(base: &Path) -> std::io::Result<PathBuf> {
    let dir = base.join(format!("{PREFIX}{}", uuid::Uuid::new_v4().simple()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        fs::DirBuilder::new().mode(0o700).create(&dir)?;
        // The umask can only take bits away; set them exactly anyway.
        if let Err(e) = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)) {
            let _ = fs::remove_dir_all(&dir);
            return Err(e);
        }
    }
    #[cfg(not(unix))]
    fs::create_dir(&dir)?;
    Ok(dir)
}

fn write_private_page(page: &Path, body: &str) -> std::io::Result<()> {
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(page)?;
    f.write_all(body.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(page, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Removes our folders under `base` older than `older_than`. Only real
/// folders (never a link) that this user owns. Best effort: a folder that
/// can't be read or removed now is tried again at the next call.
fn sweep_leftovers(base: &Path, older_than: Duration) {
    let Ok(entries) = fs::read_dir(base) else { return };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_str().is_some_and(|n| n.starts_with(PREFIX)) {
            continue;
        }
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else { continue };
        if !meta.is_dir() || !owned_by_us(&meta) {
            continue;
        }
        let Ok(modified) = meta.modified() else { continue };
        if now.duration_since(modified).is_ok_and(|age| age > older_than) {
            let _ = fs::remove_dir_all(&path);
        }
    }
}

#[cfg(unix)]
fn owned_by_us(meta: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: geteuid has no preconditions and can't fail.
    meta.uid() == unsafe { libc::geteuid() }
}

#[cfg(not(unix))]
fn owned_by_us(_meta: &fs::Metadata) -> bool {
    // The temp folder is the user's own (%LOCALAPPDATA%\Temp).
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex;
    use std::time::Instant;

    const LINK: &str = "https://chat.example.test/k2/owner-handoff#code=k2sho_TESTSECRET0001";

    fn short(page_life: Duration) -> Times {
        Times { page_life, leftover_after: Duration::from_secs(60) }
    }

    fn wait_until_gone(path: &Path, limit: Duration) -> bool {
        let start = Instant::now();
        while start.elapsed() < limit {
            if !path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !path.exists()
    }

    #[cfg(unix)]
    fn mode_of(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::symlink_metadata(path).expect("metadata").permissions().mode() & 0o777
    }

    #[cfg(unix)]
    fn age(path: &Path, by: Duration) {
        let when = SystemTime::now() - by;
        fs::File::open(path).expect("open to set its time").set_modified(when).expect("set mtime");
    }

    /// The opener gets the page's path and nothing else; the page is 0600 in
    /// a 0700 folder, holds the link, and is gone after the wait.
    #[test]
    fn only_the_page_path_reaches_the_opener() {
        let base = tempfile::tempdir().expect("tempdir");
        let seen: Mutex<Vec<(OsString, String)>> = Mutex::new(Vec::new());
        let dir = open_via_page(base.path(), LINK, short(Duration::from_secs(2)), |arg| {
            // Read the page as the opener would see it, at the moment it runs.
            let body = fs::read_to_string(arg).map_err(|e| format!("the opener's argument isn't a readable page: {e}"))?;
            seen.lock().expect("lock").push((arg.as_os_str().to_owned(), body));
            Ok(())
        })
        .expect("opened");

        let seen = seen.into_inner().expect("lock");
        assert_eq!(seen.len(), 1, "the opener runs once");
        let (arg, body) = &seen[0];
        let page = dir.join(PAGE);
        assert_eq!(Path::new(arg), page.as_path(), "the opener gets the page's path");
        let arg_text = arg.to_string_lossy();
        assert!(!arg_text.contains("k2sho_"), "the code is not in the opener's argument: {arg_text}");
        assert!(!arg_text.contains("https:"), "the link is not in the opener's argument: {arg_text}");

        assert_eq!(dir.parent(), Some(base.path()), "the folder sits in the temp folder");
        let name = dir.file_name().and_then(|n| n.to_str()).expect("folder name");
        assert!(name.starts_with(PREFIX), "{name}");
        assert_eq!(name.len(), PREFIX.len() + 32, "a random 128-bit name: {name}");

        assert_eq!(body, &redirect_page(LINK), "the page is the redirect and nothing else");
        assert!(body.contains(&format!("location.replace(\"{LINK}\")")), "{body}");
        assert!(body.contains(&format!("<meta http-equiv=\"refresh\" content=\"0;url={LINK}\">")), "{body}");
        assert!(body.contains("<meta name=\"referrer\" content=\"no-referrer\">"), "{body}");

        #[cfg(unix)]
        {
            assert_eq!(mode_of(&dir), 0o700, "the folder is 0700");
            assert_eq!(mode_of(&page), 0o600, "the page is 0600");
        }

        assert!(page.exists(), "the page stays for the wait");
        assert!(wait_until_gone(&dir, Duration::from_secs(20)), "the folder is removed after the wait");
    }

    /// A second call removes our folders older than the leftover age, and
    /// leaves fresh ones, other names, links and their targets alone.
    #[cfg(unix)]
    #[test]
    fn leftovers_are_swept_at_the_next_call() {
        let base = tempfile::tempdir().expect("tempdir");
        let old = base.path().join(format!("{PREFIX}old"));
        fs::create_dir(&old).expect("old");
        fs::write(old.join(PAGE), "x").expect("old page");
        age(&old, Duration::from_secs(120));
        let fresh = base.path().join(format!("{PREFIX}fresh"));
        fs::create_dir(&fresh).expect("fresh");
        let other = base.path().join("someone-else.old");
        fs::create_dir(&other).expect("other");
        age(&other, Duration::from_secs(120));
        let elsewhere = tempfile::tempdir().expect("elsewhere");
        let target = elsewhere.path().join("keep");
        fs::create_dir(&target).expect("target");
        age(&target, Duration::from_secs(120));
        let link = base.path().join(format!("{PREFIX}link"));
        std::os::unix::fs::symlink(&target, &link).expect("symlink");

        let dir = open_via_page(base.path(), LINK, short(Duration::from_secs(1)), |_| Ok(())).expect("opened");

        assert!(!old.exists(), "an old folder of ours is removed");
        assert!(fresh.exists(), "a fresh folder (another call's page) stays");
        assert!(other.exists(), "a folder with another name stays");
        assert!(target.exists(), "a link's target is never removed");
        assert!(fs::symlink_metadata(&link).is_ok(), "a link with our name is left alone");
        assert!(wait_until_gone(&dir, Duration::from_secs(20)), "this call's folder is removed after its wait");
    }

    /// When the opener fails, the folder goes at once and the error is the
    /// opener's.
    #[test]
    fn a_failed_open_removes_the_page_at_once() {
        let base = tempfile::tempdir().expect("tempdir");
        let made: Mutex<Option<PathBuf>> = Mutex::new(None);
        let err = open_via_page(base.path(), LINK, short(Duration::from_secs(30)), |page| {
            *made.lock().expect("lock") = page.parent().map(Path::to_path_buf);
            Err("no browser".into())
        })
        .expect_err("the opener failed");
        assert_eq!(err, "no browser");
        let made = made.into_inner().expect("lock").expect("a folder was made");
        assert!(!made.exists(), "the folder is removed when the opener fails");
    }

    #[test]
    fn the_link_cannot_break_out_of_the_page() {
        let nasty = "https://x.example.test/a?b=\"'></script><script>alert(1)</script>&c=<b>";
        let page = redirect_page(nasty);
        assert!(!page.contains("<script>alert(1)"), "{page}");
        assert!(!page.contains("</script><script>"), "{page}");
        assert!(page.contains("content=\"0;url=https://x.example.test/a?b=&quot;&#x27;&gt;&lt;/script&gt;"), "{page}");
        assert!(page.contains("\\u003c/script\\u003e"), "{page}");
        assert_eq!(page.matches("<script>").count(), 1, "{page}");
    }

    #[test]
    fn only_https_links_are_opened() {
        assert_eq!(checked_link(LINK).as_deref(), Ok(LINK));
        for bad in [
            "http://chat.example.test/k2/owner-handoff#code=k2sho_TESTSECRET0001",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "k2sho_TESTSECRET0001",
            "https://",
        ] {
            let err = checked_link(bad).expect_err(bad);
            assert!(!err.contains("k2sho_") && !err.contains(bad), "the error never repeats the link: {err}");
        }
    }

    #[test]
    fn only_k2_ui_webviews_may_open_it() {
        for ok in ["main", "window-0b6f", "focus-acme"] {
            assert!(is_k2_ui_webview(ok), "{ok}");
        }
        for no in ["browser-main-foo", "browser-window-0b6f-1", "call-1", "", "mainx"] {
            assert!(!is_k2_ui_webview(no), "{no}");
        }
        // The same set the default capability names, and no remote origin
        // may call app commands (tauri checks the ACL for remote origins).
        let raw = include_str!("../../capabilities/default.json");
        let v: serde_json::Value = serde_json::from_str(raw).expect("capability JSON");
        assert_eq!(v["webviews"], serde_json::json!(["main", "window-*", "focus-*"]));
        assert!(v.get("remote").is_none(), "no remote origin is granted");
    }
}
