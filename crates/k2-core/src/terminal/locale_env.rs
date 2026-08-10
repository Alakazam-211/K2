//! UTF-8 locale defaulting for daemon-spawned child processes.
//!
//! The daemon runs under launchd (macOS) / systemd (Linux), whose
//! service environments carry NO `LANG`/`LC_*` at all — so every
//! process inside a K2 terminal session runs in the C locale unless
//! the user's shell rc happens to set one (most don't; real terminal
//! emulators — Terminal.app, iTerm2, kitty, alacritty — export a
//! UTF-8 `LANG` into their children themselves, so rc files never
//! needed to).
//!
//! The user-visible failure that motivated this: TUIs that copy their
//! own selections shell out to `pbcopy`, and `pbcopy` under a C
//! locale decodes its UTF-8 stdin as **MacRoman** — box-drawing and
//! typography turn into `‚îå‚îÄ` / `‚Äî` mojibake on the pasteboard
//! while the screen looks perfect. Locale-less children also degrade
//! `sort`, `grep -i`, Python's stdio encoding detection, and anything
//! else that consults `nl_langinfo(CODESET)`.
//!
//! Policy — least-override, mirroring what terminal emulators do:
//!
//!   1. If the CALLER's child env already carries `LC_ALL`,
//!      `LC_CTYPE`, or `LANG`, do nothing (explicit wins).
//!   2. If the daemon's OWN environment carries one (dev-mode daemon
//!      launched from a real terminal), do nothing — alacritty's PTY
//!      spawn layers the child map on top of the inherited process
//!      env, so the child sees it already.
//!   3. Otherwise insert `LANG=<utf-8 locale>`: on macOS the user's
//!      `AppleLocale` (e.g. `en_US` → `en_US.UTF-8`) when it names a
//!      real locale, else `en_US.UTF-8` (always present on macOS); on
//!      other unixes `C.UTF-8` (glibc/musl builtin), else
//!      `en_US.UTF-8`.
//!
//! Only `LANG` is set — never `LC_ALL` — so a user rc that exports
//! its own locale still overrides cleanly.

use std::collections::HashMap;
use std::sync::OnceLock;

/// The env keys that make a locale "already chosen", in POSIX
/// precedence order. `LANGUAGE` is intentionally excluded — it only
/// affects message translation, not the codeset.
const LOCALE_KEYS: [&str; 3] = ["LC_ALL", "LC_CTYPE", "LANG"];

/// Insert `LANG=<utf-8 locale>` into `child_env` unless the caller or
/// the daemon's own environment already picked a locale. See the
/// module docs for the full policy.
pub fn ensure_utf8_locale(child_env: &mut HashMap<String, String>) {
    if LOCALE_KEYS.iter().any(|k| child_env.contains_key(*k)) {
        return;
    }
    if process_env_has_locale() {
        return;
    }
    child_env.insert("LANG".to_string(), utf8_locale_name().to_string());
}

/// Whether the daemon's own environment carries a non-empty locale
/// key (children inherit the process env underneath the explicit
/// child map, so a daemon launched from a real terminal needs no
/// insert).
fn process_env_has_locale() -> bool {
    LOCALE_KEYS
        .iter()
        .any(|k| std::env::var_os(k).is_some_and(|v| !v.is_empty()))
}

/// The UTF-8 locale to default to, computed once per process.
fn utf8_locale_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(compute_utf8_locale)
}

fn compute_utf8_locale() -> String {
    for cand in candidate_locales() {
        if locale_exists(&cand) {
            return cand;
        }
    }
    // Unvalidated last resort; exists on every macOS and virtually
    // every Linux with locales generated.
    "en_US.UTF-8".to_string()
}

/// Candidate UTF-8 locales in preference order.
fn candidate_locales() -> Vec<String> {
    let mut cands = Vec::new();
    #[cfg(target_os = "macos")]
    if let Some(apple) = apple_locale_lang() {
        cands.push(format!("{apple}.UTF-8"));
    }
    #[cfg(not(target_os = "macos"))]
    cands.push("C.UTF-8".to_string());
    cands.push("en_US.UTF-8".to_string());
    cands
}

/// The user's macOS locale identifier (`defaults read -g AppleLocale`,
/// e.g. `en_US`, `de_DE`), sanitized to a plain `ll_CC`/`ll` shape.
/// Extended identifiers (`zh-Hans_US`, `en_US@currency=EUR`) that
/// don't reduce to that shape return `None` — the caller falls back
/// rather than guessing a POSIX name that `newlocale` would reject
/// anyway.
#[cfg(target_os = "macos")]
fn apple_locale_lang() -> Option<String> {
    let out = std::process::Command::new("defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let raw = String::from_utf8(out.stdout).ok()?;
    let base = raw.trim().split('@').next()?;
    let plain = base
        .chars()
        .all(|c| c.is_ascii_alphabetic() || c == '_');
    if plain && !base.is_empty() { Some(base.to_string()) } else { None }
}

/// Whether `name` is a locale this system can actually construct.
/// Uses `newlocale(3)` (thread-safe, no process-locale mutation);
/// NULL ⇒ unknown locale.
#[cfg(unix)]
fn locale_exists(name: &str) -> bool {
    let Ok(cname) = std::ffi::CString::new(name) else {
        return false;
    };
    // SAFETY: newlocale with a null base is the documented way to
    // probe a locale name; a non-null result must be freed.
    unsafe {
        let loc = libc::newlocale(libc::LC_CTYPE_MASK, cname.as_ptr(), std::ptr::null_mut());
        if loc.is_null() {
            false
        } else {
            libc::freelocale(loc);
            true
        }
    }
}

/// Windows has no portable `newlocale` probe in our libc bindings — accept
/// common UTF-8 locale names so spawn env defaulting still works.
#[cfg(windows)]
fn locale_exists(name: &str) -> bool {
    matches!(
        name,
        "C.UTF-8" | "C.utf8" | "en_US.UTF-8" | "en_US.utf8" | "UTF-8"
    ) || name.ends_with(".UTF-8")
        || name.ends_with(".utf8")
}

#[cfg(test)]
mod tests {
    use super::*;

    // NOTE: no test manipulates the process env (std::env::set_var is
    // racy across the parallel test harness). The process-env branch
    // is exercised through `process_env_has_locale` only when the
    // harness environment genuinely lacks a locale — the caller-wins
    // branches below are env-independent either way.

    #[test]
    fn caller_lang_wins_env_untouched() {
        let mut env = HashMap::from([("LANG".to_string(), "ja_JP.UTF-8".to_string())]);
        ensure_utf8_locale(&mut env);
        assert_eq!(env.get("LANG").unwrap(), "ja_JP.UTF-8");
        assert_eq!(env.len(), 1);
    }

    #[test]
    fn caller_lc_all_wins_no_lang_inserted() {
        let mut env = HashMap::from([("LC_ALL".to_string(), "C".to_string())]);
        ensure_utf8_locale(&mut env);
        // An explicit C locale is a deliberate caller choice — never
        // "corrected" to UTF-8.
        assert!(!env.contains_key("LANG"));
    }

    #[test]
    fn caller_lc_ctype_wins_no_lang_inserted() {
        let mut env = HashMap::from([("LC_CTYPE".to_string(), "en_GB.UTF-8".to_string())]);
        ensure_utf8_locale(&mut env);
        assert!(!env.contains_key("LANG"));
    }

    #[test]
    fn inserted_locale_is_utf8_when_nothing_chose_one() {
        let mut env = HashMap::new();
        ensure_utf8_locale(&mut env);
        match env.get("LANG") {
            // Daemon-like environment (no locale): we must have
            // inserted a UTF-8 locale.
            Some(lang) => assert!(
                lang.ends_with("UTF-8"),
                "inserted LANG must be a UTF-8 locale, got {lang}"
            ),
            // Dev-shell environment (harness has LANG/LC_*): policy
            // step 2 — inherit, insert nothing.
            None => assert!(process_env_has_locale()),
        }
    }

    #[test]
    fn chosen_locale_is_constructible_and_utf8() {
        let name = utf8_locale_name();
        assert!(name.ends_with("UTF-8"), "got {name}");
        // en_US.UTF-8 / C.UTF-8 / AppleLocale-derived — whichever won,
        // the system must actually be able to build it (the whole
        // point is that children can setlocale() into it). The
        // unvalidated en_US.UTF-8 fallback also passes this on every
        // supported platform; if a platform ever lacks it we WANT the
        // loud failure here.
        assert!(locale_exists(name), "{name} not constructible");
    }

    #[test]
    fn locale_exists_rejects_nonsense() {
        assert!(!locale_exists("xx_YY.NOPE-99"));
        assert!(!locale_exists("bad\0name"));
    }
}
