//! "What's New" — user-facing changelog popup state + content.
//!
//! 0.38.7 ships a popup that fires on the first Tauri-app launch after
//! a version upgrade. Content lives in [`WHATS_NEW_MARKDOWN`] — a copy
//! of `WHATS_NEW.md` from the repo root, embedded into the daemon
//! binary at build time.
//!
//! **Show-the-popup decision (`has_new`)** is `(last_seen, current]`-
//! based: there must be at least one section newer than what the user
//! last dismissed.
//!
//! **What the popup *contains* (`content`)**, however, is the full
//! current MAJOR.MINOR track up through `current_version`. Reasoning:
//! when a user lands on a minor track mid-stream (e.g. they install
//! 0.39.3 with `last_seen = 0.39.2`), the (last_seen, current] slice
//! would only include 0.39.3 — but the foundational entry that opened
//! the track (0.39.0) often holds migration / behavioural context the
//! user needs regardless of which patch they ended up on. Surfacing
//! the entire minor track lets the modal's ←/→ pagination walk every
//! 0.39.x entry, so first-time-on-a-minor-track users can read the
//! whole story.
//!
//! State lives in `~/.k2so/whats-new.state` — a single line containing
//! the last version the user dismissed the popup for. Absent file =
//! never dismissed = show on next launch.
//!
//! This module is intentionally pure: the daemon HTTP route in
//! `k2so-daemon::cli` is the only network surface; the CLI verb
//! `k2so whatsnew` and the renderer's modal both go through that
//! route. Daemon-first per the architecture invariant.

use std::cmp::Ordering;
use std::fs;
use std::path::PathBuf;

use serde::Serialize;

/// The user-facing changelog, embedded at build time.
///
/// **Release script contract:** every released version MUST have a
/// `## X.Y.Z — title` header in `WHATS_NEW.md` before
/// `scripts/release.sh` will proceed. See the script's step 1.5.
pub const WHATS_NEW_MARKDOWN: &str = include_str!("../../../WHATS_NEW.md");

/// One version's worth of changelog content.
#[derive(Debug, Clone)]
pub struct VersionSection {
    /// Version string in `X.Y.Z` form (e.g. `"0.38.7"`).
    pub version: String,
    /// Human title from the header (e.g. `"Update notes when K2SO updates"`).
    pub title: String,
    /// Full markdown for this section, including the header line.
    pub content: String,
}

/// Result of asking "should I show the popup right now?"
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct WhatsNewCheck {
    /// Version embedded in the daemon binary (truth source).
    pub current_version: String,
    /// Last version the user dismissed the popup for, if any.
    pub last_seen_version: Option<String>,
    /// True iff there's at least one unseen section worth showing.
    pub has_new: bool,
    /// The markdown payload to show in the modal. When `has_new` is
    /// true, this is the full current MAJOR.MINOR track up through
    /// `current_version` (e.g. all 0.39.x entries `<= 0.39.3`), so the
    /// modal can paginate back through every entry on the user's
    /// current minor track — not just the entries newer than
    /// `last_seen_version`. Empty string when `has_new` is false.
    pub content: String,
}

// ─────────────────────────────────────────────────────────────────────
// Parsing
// ─────────────────────────────────────────────────────────────────────

/// Parse `WHATS_NEW.md`-style markdown into a list of version
/// sections. Each `## X.Y.Z — title` (em-dash) or `## X.Y.Z - title`
/// (hyphen) starts a new section; any `##` whose first token isn't a
/// `X.Y.Z` is ignored as non-version content.
pub fn parse_changelog(md: &str) -> Vec<VersionSection> {
    let mut sections = Vec::new();
    let mut current: Option<(String, String, Vec<String>)> = None;

    for line in md.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            let trimmed = rest.trim();
            // Try `X.Y.Z — title` (em-dash) then `X.Y.Z - title` (hyphen).
            let (version_part, title) = if let Some(idx) = trimmed.find(" — ") {
                (&trimmed[..idx], trimmed[idx + " — ".len()..].to_string())
            } else if let Some(idx) = trimmed.find(" - ") {
                (&trimmed[..idx], trimmed[idx + " - ".len()..].to_string())
            } else {
                (trimmed, String::new())
            };

            // Validate version_part looks like X.Y.Z — digits + dots only,
            // contains at least one dot. Ignore non-version `##` (e.g. the
            // top-level intro section).
            if !version_part.is_empty()
                && version_part.contains('.')
                && version_part
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '.')
            {
                if let Some((v, t, lines)) = current.take() {
                    sections.push(VersionSection {
                        version: v,
                        title: t,
                        content: lines.join("\n"),
                    });
                }
                current = Some((version_part.to_string(), title, vec![line.to_string()]));
                continue;
            }
        }

        if let Some((_, _, ref mut lines)) = current {
            lines.push(line.to_string());
        }
    }

    if let Some((v, t, lines)) = current {
        sections.push(VersionSection {
            version: v,
            title: t,
            content: lines.join("\n"),
        });
    }

    sections
}

// ─────────────────────────────────────────────────────────────────────
// Version compare
// ─────────────────────────────────────────────────────────────────────

/// Compare two version strings as semver-lite (major.minor.patch).
///
/// Unparseable segments default to 0, so `compare_semver("0.38.x", "0.38.7")`
/// treats the first as `(0, 38, 0)` — strictly less than the second. Pre-release
/// suffixes (`0.38.7-rc1`) are stripped before parsing the patch component.
pub fn compare_semver(a: &str, b: &str) -> Ordering {
    let parse = |s: &str| -> (u32, u32, u32) {
        let mut parts = s.split('.');
        let major = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let minor = parts.next().and_then(|p| p.parse().ok()).unwrap_or(0);
        let patch = parts
            .next()
            .map(|p| p.split('-').next().unwrap_or(p))
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        (major, minor, patch)
    };
    parse(a).cmp(&parse(b))
}

// ─────────────────────────────────────────────────────────────────────
// Slice computation
// ─────────────────────────────────────────────────────────────────────

/// Compute the markdown for every section `v` such that
/// `last_seen < v <= current`. If `last_seen` is None, every section
/// `<= current` is included. Used by [`check_for_user`] only to
/// determine whether anything *newer than what the user last saw*
/// exists — i.e., whether to auto-fire the popup at all.
pub fn slice_unseen(
    sections: &[VersionSection],
    last_seen: Option<&str>,
    current: &str,
) -> String {
    sections
        .iter()
        .filter(|s| {
            let above_last_seen = match last_seen {
                Some(ls) => compare_semver(&s.version, ls) == Ordering::Greater,
                None => true,
            };
            let at_or_below_current = compare_semver(&s.version, current) != Ordering::Greater;
            above_last_seen && at_or_below_current
        })
        .map(|s| s.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Extract the MAJOR.MINOR prefix from a `X.Y.Z` version string.
/// Returns `"0.39"` for `"0.39.3"`, `"1.0"` for `"1.0.0"`, etc. If the
/// input has fewer than two dot-separated components, returns the
/// input unchanged (defensive — caller's [`current_version`] comes
/// from `CARGO_PKG_VERSION` so this branch shouldn't fire in
/// production).
fn minor_track(version: &str) -> String {
    let parts: Vec<&str> = version.split('.').collect();
    if parts.len() >= 2 {
        format!("{}.{}", parts[0], parts[1])
    } else {
        version.to_string()
    }
}

/// Compute the markdown for every section in `current`'s MAJOR.MINOR
/// track up through `current` itself — e.g. for `current = "0.39.3"`,
/// every section whose version starts with `"0.39."` and is `<= 0.39.3`.
///
/// This is what populates [`WhatsNewCheck::content`] when the popup
/// fires: even if `last_seen` was already inside the current minor
/// track (so the (last_seen, current] slice would be tiny), the modal
/// still receives the full track so its ←/→ pagination can walk every
/// entry. See the module-level doc for rationale.
pub fn slice_minor_track(sections: &[VersionSection], current: &str) -> String {
    let track = minor_track(current);
    sections
        .iter()
        .filter(|s| {
            minor_track(&s.version) == track
                && compare_semver(&s.version, current) != Ordering::Greater
        })
        .map(|s| s.content.clone())
        .collect::<Vec<_>>()
        .join("\n\n")
}

// ─────────────────────────────────────────────────────────────────────
// State file I/O (~/.k2so/whats-new.state)
// ─────────────────────────────────────────────────────────────────────

/// Path to the single-line state file. Lives next to other K2SO state
/// in `~/.k2so/`. Absent file = "user has never dismissed the popup."
pub fn state_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".k2").join("whats-new.state")
}

/// Read the last-seen version from the state file. Returns `None` if
/// the file doesn't exist, can't be read, or is empty.
pub fn read_last_seen() -> Option<String> {
    let path = state_path();
    let raw = fs::read_to_string(&path).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Write `version` as the new last-seen value. Creates the parent
/// directory if missing. Atomic: writes to a temp file then renames.
pub fn write_last_seen(version: &str) -> std::io::Result<()> {
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let tmp = path.with_extension("state.tmp");
    fs::write(&tmp, format!("{version}\n"))?;
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Clear the last-seen state. Forces the popup to show again on the
/// next app launch (or the next `k2so whatsnew` invocation). Idempotent
/// — succeeds even if the file doesn't exist.
pub fn clear_last_seen() -> std::io::Result<()> {
    let path = state_path();
    if path.exists() {
        fs::remove_file(&path)?;
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
// Orchestrator — what the daemon route returns
// ─────────────────────────────────────────────────────────────────────

/// One-shot "should the popup show right now?" check, given the
/// daemon's embedded version. Caller (daemon HTTP route, CLI verb)
/// surfaces the result.
pub fn check_for_user(current_version: &str) -> WhatsNewCheck {
    let last_seen = read_last_seen();
    let sections = parse_changelog(WHATS_NEW_MARKDOWN);
    // Decide whether to auto-fire: only when there's at least one
    // section newer than what the user last dismissed.
    let unseen = slice_unseen(&sections, last_seen.as_deref(), current_version);
    let has_new = !unseen.trim().is_empty();
    // What to ship to the modal: the full current minor-track up
    // through `current_version`. Lets the modal paginate back to the
    // start of the current minor (e.g. 0.39.0 from 0.39.3) even if
    // `last_seen` was already in the track — see module doc.
    let content = if has_new {
        slice_minor_track(&sections, current_version)
    } else {
        String::new()
    };
    WhatsNewCheck {
        current_version: current_version.to_string(),
        last_seen_version: last_seen,
        has_new,
        content,
    }
}

// ─────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"# K2SO — What's New

Intro prose here — should be ignored by the parser since the `#`
header isn't a version.

## 0.38.7 — Update notes when K2SO updates

You see this popup when K2SO updates.

- bullet a
- bullet b

## 0.38.6 — Inter-agent messages just work

msg is reliable now.

## 0.38.5 — Cmd+T tabs survive app updates

Tabs persist across updates.

## Intentionally non-version header

This should be merged into 0.38.5's content because the `##` token
isn't a version.

## 0.38.0 — Daemon-authoritative tabs

Daemon owns sessions.
"#;

    // ── Parser ───────────────────────────────────────────────────────

    #[test]
    fn parser_extracts_each_version_section() {
        let sections = parse_changelog(SAMPLE);
        let versions: Vec<&str> = sections.iter().map(|s| s.version.as_str()).collect();
        assert_eq!(versions, vec!["0.38.7", "0.38.6", "0.38.5", "0.38.0"]);
    }

    #[test]
    fn parser_extracts_titles() {
        let sections = parse_changelog(SAMPLE);
        assert_eq!(sections[0].title, "Update notes when K2SO updates");
        assert_eq!(sections[1].title, "Inter-agent messages just work");
        assert_eq!(sections[3].title, "Daemon-authoritative tabs");
    }

    #[test]
    fn parser_keeps_section_content_including_header() {
        let sections = parse_changelog(SAMPLE);
        let s = &sections[1];
        assert!(s.content.starts_with("## 0.38.6"));
        assert!(s.content.contains("msg is reliable now"));
    }

    #[test]
    fn parser_ignores_non_version_h2_headers() {
        // The `## Intentionally non-version header` block should be
        // absorbed into 0.38.5's content, not parsed as its own section.
        let sections = parse_changelog(SAMPLE);
        let s_385 = sections.iter().find(|s| s.version == "0.38.5").unwrap();
        assert!(
            s_385.content.contains("non-version header"),
            "non-version `##` should fold into the previous version section"
        );
    }

    #[test]
    fn parser_handles_hyphen_separator_too() {
        // `## X.Y.Z - title` (ASCII hyphen) is accepted as a fallback —
        // not every editor inserts em-dashes.
        let md = "## 1.2.3 - hello\nbody\n";
        let sections = parse_changelog(md);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].version, "1.2.3");
        assert_eq!(sections[0].title, "hello");
    }

    #[test]
    fn parser_handles_no_title_separator() {
        let md = "## 1.2.3\nbody\n";
        let sections = parse_changelog(md);
        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].version, "1.2.3");
        assert_eq!(sections[0].title, "");
    }

    #[test]
    fn parser_returns_empty_for_no_versions() {
        assert!(parse_changelog("").is_empty());
        assert!(parse_changelog("# Just an h1\n\nNo h2s here").is_empty());
    }

    // ── Version compare ──────────────────────────────────────────────

    #[test]
    fn semver_compare_orders_patches() {
        assert_eq!(compare_semver("0.38.7", "0.38.6"), Ordering::Greater);
        assert_eq!(compare_semver("0.38.6", "0.38.7"), Ordering::Less);
        assert_eq!(compare_semver("0.38.6", "0.38.6"), Ordering::Equal);
    }

    #[test]
    fn semver_compare_orders_minors() {
        assert_eq!(compare_semver("0.39.0", "0.38.99"), Ordering::Greater);
        assert_eq!(compare_semver("1.0.0", "0.99.99"), Ordering::Greater);
    }

    #[test]
    fn semver_compare_strips_prerelease_on_patch() {
        // `0.38.7-rc1` is treated as `0.38.7` for ordering — pre-release
        // suffixes don't change the bucket.
        assert_eq!(compare_semver("0.38.7-rc1", "0.38.7"), Ordering::Equal);
        assert_eq!(compare_semver("0.38.7-rc1", "0.38.6"), Ordering::Greater);
    }

    #[test]
    fn semver_compare_handles_malformed_inputs() {
        // Unparseable segments default to 0 — defensive, never panics.
        assert_eq!(compare_semver("not-a-version", "0.0.0"), Ordering::Equal);
        assert_eq!(compare_semver("1.x.y", "1.0.0"), Ordering::Equal);
    }

    // ── Slice computation ────────────────────────────────────────────

    #[test]
    fn slice_with_no_last_seen_includes_everything_up_to_current() {
        let sections = parse_changelog(SAMPLE);
        let out = slice_unseen(&sections, None, "0.38.7");
        for v in ["0.38.7", "0.38.6", "0.38.5", "0.38.0"] {
            assert!(out.contains(v), "expected slice to include {v}");
        }
    }

    #[test]
    fn slice_excludes_versions_already_seen() {
        let sections = parse_changelog(SAMPLE);
        // User last saw 0.38.5 → should only see 0.38.6 and 0.38.7.
        let out = slice_unseen(&sections, Some("0.38.5"), "0.38.7");
        assert!(out.contains("0.38.7"));
        assert!(out.contains("0.38.6"));
        assert!(!out.contains("## 0.38.5 "), "0.38.5 already seen — must be excluded");
        assert!(!out.contains("## 0.38.0 "), "0.38.0 already seen — must be excluded");
    }

    #[test]
    fn slice_returns_empty_when_user_is_current() {
        let sections = parse_changelog(SAMPLE);
        let out = slice_unseen(&sections, Some("0.38.7"), "0.38.7");
        assert!(out.trim().is_empty());
    }

    #[test]
    fn slice_excludes_versions_above_current() {
        // Defensive: if the user somehow has a `last_seen` ahead of the
        // daemon's `current_version` (e.g. downgrade), no future sections
        // leak.
        let sections = parse_changelog(SAMPLE);
        let out = slice_unseen(&sections, None, "0.38.5");
        assert!(!out.contains("## 0.38.7 "));
        assert!(!out.contains("## 0.38.6 "));
        assert!(out.contains("## 0.38.5 "));
        assert!(out.contains("## 0.38.0 "));
    }

    // ── Minor-track slice ────────────────────────────────────────────

    const MULTI_MINOR_SAMPLE: &str = r#"# K2SO — What's New

## 0.39.3 — patch three

three body

## 0.39.2 — patch two

two body

## 0.39.1 — patch one

one body

## 0.39.0 — minor open

minor-open body

## 0.38.13 — older minor

older body

## 0.38.0 — older minor open

older minor body
"#;

    #[test]
    fn minor_track_extracts_major_dot_minor() {
        assert_eq!(minor_track("0.39.3"), "0.39");
        assert_eq!(minor_track("0.39.0"), "0.39");
        assert_eq!(minor_track("1.0.0"), "1.0");
        assert_eq!(minor_track("0.38.13"), "0.38");
    }

    #[test]
    fn minor_track_defensive_on_malformed_input() {
        // Single-segment input passes through unchanged — no panic.
        assert_eq!(minor_track("garbage"), "garbage");
        assert_eq!(minor_track(""), "");
    }

    #[test]
    fn slice_minor_track_includes_only_current_minor_up_to_current() {
        let sections = parse_changelog(MULTI_MINOR_SAMPLE);
        // current = 0.39.3 → must include all 0.39.x, must exclude all 0.38.x.
        let out = slice_minor_track(&sections, "0.39.3");
        assert!(out.contains("## 0.39.3"), "must include current");
        assert!(out.contains("## 0.39.2"), "must include earlier in-track patch");
        assert!(out.contains("## 0.39.1"), "must include earlier in-track patch");
        assert!(out.contains("## 0.39.0"), "must include track-opener");
        assert!(!out.contains("## 0.38.13"), "must exclude older minor");
        assert!(!out.contains("## 0.38.0"), "must exclude older minor");
    }

    #[test]
    fn slice_minor_track_caps_at_current_excluding_future_patches() {
        let sections = parse_changelog(MULTI_MINOR_SAMPLE);
        // current = 0.39.1 → must include 0.39.0 and 0.39.1, must
        // exclude 0.39.2 / 0.39.3 (defensive: file has future patches
        // ahead of the daemon binary's version, e.g. dev environment).
        let out = slice_minor_track(&sections, "0.39.1");
        assert!(out.contains("## 0.39.1"));
        assert!(out.contains("## 0.39.0"));
        assert!(!out.contains("## 0.39.2"));
        assert!(!out.contains("## 0.39.3"));
    }

    #[test]
    fn slice_minor_track_at_track_opener_is_just_the_opener() {
        let sections = parse_changelog(MULTI_MINOR_SAMPLE);
        // current = 0.39.0 → only the track opener; no other 0.39.x
        // exists at-or-below 0.39.0.
        let out = slice_minor_track(&sections, "0.39.0");
        assert!(out.contains("## 0.39.0"));
        assert!(!out.contains("## 0.39.1"));
        assert!(!out.contains("## 0.38.13"));
    }

    #[test]
    fn slice_minor_track_empty_when_no_sections_in_track() {
        let sections = parse_changelog(MULTI_MINOR_SAMPLE);
        // Asking about a minor track that doesn't exist in the file
        // (e.g. 0.40.x) returns empty — no leakage from neighbouring
        // tracks.
        let out = slice_minor_track(&sections, "0.40.0");
        assert!(out.trim().is_empty(), "got: {out:?}");
    }

    // ── State file I/O ───────────────────────────────────────────────

    #[test]
    fn state_path_lives_in_dot_k2() {
        let p = state_path();
        let s = p.to_string_lossy();
        assert!(s.ends_with(".k2/whats-new.state"), "got {s}");
    }

    #[test]
    fn read_write_clear_state_roundtrips() {
        // Acquire the crate-wide HOME_LOCK so this test serializes with
        // every other `$HOME`-mutating test across modules (themes,
        // skill_layers, chat_history, app_settings). Phase 2 Unit 6's
        // 68-test backfill (commit b65a5195) added many tests that
        // mutate $HOME, exposing this test's pre-existing assumption
        // that "no other tests touch state_path" as false. Lock kills
        // the race.
        let _g = crate::themes::HOME_LOCK.lock();

        // Use a process-isolated HOME so we don't trample the real user's state.
        let tmp = std::env::temp_dir().join(format!(
            "k2so-whats-new-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev_home = std::env::var("HOME").ok();
        // SAFETY: HOME_LOCK above prevents any other test from racing
        // $HOME during this test's lifetime; restore happens before
        // the guard drops.
        unsafe { std::env::set_var("HOME", &tmp); }

        // Empty state initially.
        assert!(read_last_seen().is_none(), "fresh tmp should have no state");

        // Write a version, read it back.
        write_last_seen("0.38.6").unwrap();
        assert_eq!(read_last_seen().as_deref(), Some("0.38.6"));

        // Overwrite.
        write_last_seen("0.38.7").unwrap();
        assert_eq!(read_last_seen().as_deref(), Some("0.38.7"));

        // Clear.
        clear_last_seen().unwrap();
        assert!(read_last_seen().is_none());

        // Clear is idempotent.
        clear_last_seen().unwrap();

        // Restore HOME.
        // SAFETY: matches the set_var above; bounded to this test fn.
        unsafe {
            match prev_home {
                Some(h) => std::env::set_var("HOME", h),
                None => std::env::remove_var("HOME"),
            }
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    // ── End-to-end orchestrator ──────────────────────────────────────

    #[test]
    fn embedded_changelog_parses_cleanly() {
        // The real WHATS_NEW.md file must contain at least one parseable
        // version section. This guards against the release-script
        // contract being met but a malformed file still slipping
        // through.
        let sections = parse_changelog(WHATS_NEW_MARKDOWN);
        assert!(
            !sections.is_empty(),
            "WHATS_NEW.md must have at least one `## X.Y.Z` section"
        );
        for s in &sections {
            assert!(
                !s.version.is_empty(),
                "every section must have a non-empty version"
            );
        }
    }
}
