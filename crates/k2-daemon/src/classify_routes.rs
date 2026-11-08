//! GH #8 — daemon-side HITL detector + extractor.
//!
//! `GET /cli/terminal/classify?session=<id>&token=<token>`
//!
//! Given a live session's rendered screen text (the SAME text a human
//! or agent sees via `/cli/terminal/read`), decide whether the screen
//! is sitting on a **human-in-the-loop (HITL)** prompt — a select
//! menu, a yes/no permission gate, a multi-question form — and, when
//! it is, extract the question(s) + option(s).
//!
//! This is the detection primitive `k2 talk` auto-routing consumes.
//!
//! # Detection: regex fast-path
//!
//! If the screen carries an obvious, stable HITL marker (Claude's
//! `Enter to select` menu footer, a numbered `❯ 1.` option cursor, the
//! `☐ … ✔ Submit →` tab-bar, a `Do you want to proceed?` permission
//! gate) we answer `is_hitl=true` and best-effort extract the questions
//! with a cheap parse. `source="fastpath"`.
//!
//! Slice 5: the marker tables cover every Big-7 dialog captured in the
//! 2026-07 TUI signal studies
//! (`.k2/notes/tui-signal-study-grok-hermes-cursor.md`,
//! `.k2/notes/tui-signal-study-codex-gemini.md`): grok's always-approve
//! permission gate, hermes' Dangerous Command box, cursor-agent's
//! command-approval bar, and the codex/gemini/cursor trust dialogs. Two
//! dialogs are PENDING RECAPTURE (both agents were auth-broken during
//! the study): codex's exec-approval UI and gemini's shell-confirmation
//! strings. Their trust dialogs are covered, the mid-turn approval
//! wording is not yet verified.
//!
//! # Degrade-gracefully contract
//!
//! When no marker fires we return `is_hitl: null` (JSON null =
//! "unknown") with `source="unavailable"`. The caller (`talk`) keys on
//! that to fall back to its MANUAL behavior: it must NOT auto-hold or
//! auto-scout when nothing was detected.

use std::collections::HashMap;

use serde::Serialize;

use crate::cli_response::CliResponse;

/// How many lines of screen tail we classify. A Claude select menu and
/// its surrounding context comfortably fit in 60 rows (a typical
/// terminal is ~24-50 rows).
const CLASSIFY_SCREEN_LINES: usize = 60;

// ─── Response shape ──────────────────────────────────────────────────

/// What produced the verdict. The caller routes on this: `fastpath`
/// carries a real `is_hitl`; `unavailable` means "no marker matched,
/// fall back to manual behavior".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassifySource {
    /// A deterministic marker matched.
    Fastpath,
    /// No marker matched: the verdict is unknown.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClassifyQuestion {
    pub header: String,
    pub options: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassifyResult {
    /// Load-bearing field. `true` = HITL on screen (hold/scout),
    /// `false` = clear to send, `null` = unknown (no detector → caller
    /// uses today's manual behavior). Only `null` when
    /// `source == Unavailable`.
    pub is_hitl: Option<bool>,
    pub source: ClassifySource,
    /// "select" | "permission" | "none".
    pub kind: String,
    /// Scout payload: the questions and options on screen (best-effort).
    /// Empty when none.
    pub questions: Vec<ClassifyQuestion>,
    /// Human-readable note on WHY (which marker fired, or that none
    /// did). Omitted when empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ClassifyResult {
    fn into_response(self) -> CliResponse {
        CliResponse::ok_json(
            serde_json::to_string(&self).unwrap_or_else(|_| {
                // Last-ditch: never fail the request. A serialize error
                // is "uncertain", so hold rather than send into a prompt.
                r#"{"is_hitl":true,"source":"fastpath","kind":"none","questions":[],"detail":"serialize error"}"#
                    .to_string()
            }),
        )
    }
}

// ─── Handler ─────────────────────────────────────────────────────────

/// `GET /cli/terminal/classify?session=<id>&token=<token>`
///
/// Auth is enforced upstream by the `/cli/*` `token_ok` gate in
/// `routes::dispatcher`. Read-only GET — not in `post_allowed`, so no
/// 405 guard is needed here.
pub fn handle_classify(params: &HashMap<String, String>) -> CliResponse {
    // Resolve the session's rendered screen via the SAME path
    // `/cli/terminal/read` uses, so we classify exactly what a human
    // sees. A resolution failure (unknown workspace / no live session
    // / bad UUID) is a genuine caller error → surface the 400 as-is
    // (NOT a safety-bias hold: there's no message at stake yet).
    let lines = match crate::terminal_routes::resolve_screen_lines(params, CLASSIFY_SCREEN_LINES) {
        Ok(l) => l,
        Err(resp) => return resp,
    };
    let screen = lines.join("\n");

    classify_screen(&screen).into_response()
}

/// Pure classification over already-rendered screen text. Split from
/// the HTTP handler so unit tests can exercise it without a session or
/// the network.
///
/// The regex fast-path decides. A no-marker screen returns
/// `source=Unavailable` / `is_hitl=null` so the caller (`talk`) falls
/// back to its manual behavior.
fn classify_screen(screen: &str) -> ClassifyResult {
    if let Some(result) = fast_path_detect(screen) {
        return result;
    }
    // Unknown, NOT "clear": never claims is_hitl=false.
    ClassifyResult {
        is_hitl: None,
        source: ClassifySource::Unavailable,
        kind: "none".to_string(),
        questions: Vec::new(),
        detail: Some(
            "no fast-path marker on screen; caller should use manual behavior".to_string(),
        ),
    }
}

// ─── Regex fast-path ─────────────────────────────────────────────────

/// Deterministic HITL markers we key on. Each is a stable UI string
/// from a known interactive prompt, sorted/commented by agent.
/// Matching is case-insensitive on a lowercased copy of the screen.
/// Slice-5 additions are VERBATIM captures from the 2026-07 TUI signal
/// studies. Also load-bearing: a `❯`/`>`/`›`/`→`/`●`-cursored numbered
/// option row or a grok radio row fires the fast-path without any
/// footer text ([`has_cursored_numbered_option`]).
const SELECT_MARKERS: &[&str] = &[
    // claude — select-menu footer
    // (`Enter to select · ↑/↓ to navigate · Esc to cancel`). The
    // "enter to select" entry ALSO hits gemini's auth dialog footer
    // (`(Use Enter to select)` — study-confirmed).
    "enter to select",
    "↑/↓ to navigate",
    // claude — multi-question tab-bar form's Submit affordance
    // (`☐ … ✔ Submit →`).
    "✔ submit",
    "submit →",
    // codex — dialog footer (`Press enter to continue`); its dialogs
    // miss every other marker (cursor glyph is `›`, handled below).
    "press enter to continue",
    // grok — dialog footers use colon-compact hints with NO 'to'
    // (`↑/↓ Navigate`, `Enter:submit`), so claude's entries can't match.
    "↑/↓ navigate",
    "enter:submit",
    // hermes — gate footer (`↑/↓ to select, Enter to confirm (59s)`).
    "↑/↓ to select",
    // cursor-agent — approval-bar hint.
    "press the key shown",
];

/// Permission-gate markers (yes/no confirmation), sorted/commented by
/// agent. Checked before the generic select markers so a confirmation
/// prompt is `kind=permission`.
const PERMISSION_MARKERS: &[&str] = &[
    // claude — tool-permission confirmation gates.
    "do you want to proceed",
    "do you want to make this edit",
    "do you want to create",
    "do you want to",
    // codex + gemini + cursor — first-spawn trust dialogs ("Do you
    // trust the contents of this directory?" / "Do you trust the files
    // in this folder?" / cursor's Workspace Trust gate). These matched
    // NOTHING before slice 5.
    "do you trust",
    // grok — permission gate (title prefix `⚠ Action Required - `;
    // footer `1/3:select │ Ctrl+o:yolo │ Ctrl+c:cancel`; option row 3
    // `3 (○) No, reject (type to add feedback)`).
    "action required",
    "ctrl+o:yolo",
    "no, reject",
    // hermes — Dangerous Command box (header `⚠️ Dangerous Command`,
    // footer `↑/↓ to select, Enter to confirm (59s)` — 60s countdown
    // AUTO-DENIES on expiry).
    "dangerous command",
    "enter to confirm",
    // cursor-agent — command-approval bar (`Run this command?` /
    // `Not in allowlist: echo`) + transcript line while blocked.
    "run this command?",
    "not in allowlist:",
    "waiting for approval",
];

/// Best-effort detector. Returns `Some(ClassifyResult)` (always
/// `is_hitl=true`, `source=Fastpath`) when a marker fires, else `None`.
fn fast_path_detect(screen: &str) -> Option<ClassifyResult> {
    let lower = screen.to_lowercase();

    let permission_hit = PERMISSION_MARKERS.iter().find(|m| lower.contains(**m));
    let select_hit = SELECT_MARKERS.iter().find(|m| lower.contains(**m));
    // A cursored numbered option row (`❯ 1.` / `> 1.` / codex `› 1.` /
    // cursor-agent `→ 1.` / gemini `● 1.` / grok radio `1 (●)`) is
    // also a strong select signal even without the footer text.
    let cursor_option = has_cursored_numbered_option(screen);

    if permission_hit.is_none() && select_hit.is_none() && !cursor_option {
        return None;
    }

    let (kind, marker): (&str, String) = if let Some(m) = permission_hit {
        ("permission", (*m).to_string())
    } else if let Some(m) = select_hit {
        ("select", (*m).to_string())
    } else {
        ("select", "❯ numbered-option cursor".to_string())
    };

    let questions = extract_questions_cheap(screen);

    Some(ClassifyResult {
        is_hitl: Some(true),
        source: ClassifySource::Fastpath,
        kind: kind.to_string(),
        questions,
        detail: Some(format!("fast-path marker: {marker}")),
    })
}

/// Cursor glyphs that mark the ACTIVE row of a menu, per agent
/// (2026-07 TUI signal studies):
///   `❯` claude/hermes · `>` generic/legacy · `›` (U+203A) codex ·
///   `→` cursor-agent · `●` gemini radio (`● 1. Trust folder`).
/// Strips the glyph + following whitespace; `None` when the line does
/// not start with a cursor glyph.
fn strip_cursor_glyph(t: &str) -> Option<&str> {
    for g in ['❯', '>', '›', '→', '●'] {
        if let Some(rest) = t.strip_prefix(g) {
            return Some(rest.trim_start());
        }
    }
    None
}

/// True if any line looks like a cursored numbered menu option —
/// `❯ 1. Yes`, `> 2. No`, `› 1. Update now` (codex), `→ 1. Run`
/// (cursor-agent), `● 1. Trust folder` (gemini radio) — or a grok
/// radio row (`1 (●) Yes, proceed` / `3 (○) No, reject`, numbered with
/// NO arrow glyph). The leading cursor glyph / radio distinguishes an
/// active menu from ordinary numbered prose ("1. First, do X") — the
/// existing false-positive discipline: numbered rows only, cursor at
/// line start.
fn has_cursored_numbered_option(screen: &str) -> bool {
    screen.lines().any(|line| {
        let t = line.trim_start();
        match strip_cursor_glyph(t) {
            Some(r) => starts_with_numbered_option(r),
            // grok radio rows carry no cursor glyph; the `N (●)` /
            // `N (○)` radio IS the cursor.
            None => is_grok_radio_option(t),
        }
    })
}

/// `"1. Yes"` / `"12) foo"` / grok `"2 (○) Yes, proceed"` → true.
fn starts_with_numbered_option(s: &str) -> bool {
    numbered_option_label(s).is_some()
}

/// grok radio option row: leading ASCII digits, then ` (●)` (selected)
/// or ` (○)` (unselected) — `1 (●) Yes, and don't ask again …`.
fn is_grok_radio_option(s: &str) -> bool {
    grok_radio_label(s).is_some()
}

/// Split a numbered option row into its label:
/// `1. Yes` / `12) foo` → the text after the `.`/`)`;
/// grok radio `3 (○) No, reject` → the text after the radio.
/// `None` when the line is not a numbered option row.
fn numbered_option_label(s: &str) -> Option<&str> {
    let digits_len = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }
    let after = &s[digits_len..];
    if let Some(rest) = after.strip_prefix('.').or_else(|| after.strip_prefix(')')) {
        return Some(rest.trim_start());
    }
    grok_radio_label(s)
}

/// Label of a grok radio row (`2 (○) Yes, proceed` → `Yes, proceed`),
/// `None` when it isn't one.
fn grok_radio_label(s: &str) -> Option<&str> {
    let digits_len = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if digits_len == 0 {
        return None;
    }
    let after = &s[digits_len..];
    for radio in [" (●)", " (○)"] {
        if let Some(rest) = after.strip_prefix(radio) {
            return Some(rest.trim_start());
        }
    }
    None
}

/// Cheap, best-effort question/option extraction for the fast-path. We
/// collect numbered option rows (`1. foo`, `❯ 2. bar`, codex `› 1. …`,
/// gemini `● 1. …`, grok radio `2 (○) …`) into one question whose
/// `header` is the nearest preceding non-empty, non-option line
/// (typically the question text). This is intentionally simple: the
/// fast-path just needs a usable scout payload.
fn extract_questions_cheap(screen: &str) -> Vec<ClassifyQuestion> {
    let lines: Vec<&str> = screen.lines().collect();
    let mut options: Vec<String> = Vec::new();
    let mut first_option_idx: Option<usize> = None;

    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim();
        // strip an optional leading cursor glyph
        let body = strip_cursor_glyph(t).unwrap_or(t);
        if let Some(label) = numbered_option_label(body) {
            let label = label.trim().to_string();
            if !label.is_empty() {
                if first_option_idx.is_none() {
                    first_option_idx = Some(i);
                }
                options.push(label);
            }
        }
    }

    if options.is_empty() {
        return Vec::new();
    }

    // header = nearest preceding non-empty, non-option line.
    let header = first_option_idx
        .and_then(|idx| {
            lines[..idx]
                .iter()
                .rev()
                .map(|l| l.trim())
                .find(|l| !l.is_empty() && !looks_like_option(l))
                .map(|l| l.to_string())
        })
        .unwrap_or_default();

    vec![ClassifyQuestion { header, options }]
}

fn looks_like_option(line: &str) -> bool {
    let body = strip_cursor_glyph(line).unwrap_or(line);
    starts_with_numbered_option(body)
}

// ─── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fast-path: positive cases ────────────────────────────────────

    #[test]
    fn fastpath_fires_on_claude_select_menu() {
        let screen = "\
What would you like to do?

❯ 1. Refactor the module
  2. Add tests
  3. Skip

Enter to select · ↑/↓ to navigate · Esc to cancel";
        let r = fast_path_detect(screen).expect("select menu must fire fast-path");
        assert_eq!(r.source, ClassifySource::Fastpath);
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "select");
        // best-effort extraction picked up the 3 options + header
        assert_eq!(r.questions.len(), 1);
        let q = &r.questions[0];
        assert_eq!(q.header, "What would you like to do?");
        assert_eq!(
            q.options,
            vec!["Refactor the module", "Add tests", "Skip"]
        );
    }

    #[test]
    fn fastpath_fires_on_submit_tab_bar() {
        let screen = "\
☐ Sidekick   ☐ Superpower   ☐ Friday

  judgmental cat
  roomba Greg

✔ Submit →";
        let r = fast_path_detect(screen).expect("submit tab-bar must fire fast-path");
        assert_eq!(r.source, ClassifySource::Fastpath);
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "select");
    }

    #[test]
    fn fastpath_fires_on_permission_gate() {
        let screen = "\
Edit file src/main.rs?

Do you want to proceed?
❯ 1. Yes
  2. Yes, and don't ask again
  3. No";
        let r = fast_path_detect(screen).expect("permission gate must fire fast-path");
        assert_eq!(r.source, ClassifySource::Fastpath);
        assert_eq!(r.is_hitl, Some(true));
        // permission marker is checked before the select markers
        assert_eq!(r.kind, "permission");
        assert_eq!(r.questions[0].options, vec!["Yes", "Yes, and don't ask again", "No"]);
    }

    #[test]
    fn fastpath_fires_on_bare_cursored_option() {
        // No footer text, just an arrow-cursored numbered option.
        let screen = "Pick one:\n❯ 1. Alpha\n  2. Beta";
        let r = fast_path_detect(screen).expect("cursored option must fire fast-path");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "select");
    }

    // ── Fast-path: negative cases (must fall through) ────────────────

    #[test]
    fn fastpath_ignores_plain_shell_prompt() {
        let screen = "rosson@host ~/proj % ls\nCargo.toml  src  target\nrosson@host ~/proj % ";
        assert!(
            fast_path_detect(screen).is_none(),
            "a plain shell prompt must NOT fire the fast-path"
        );
    }

    #[test]
    fn fastpath_ignores_normal_claude_prose() {
        // Numbered prose without a cursor glyph or any HITL marker.
        let screen = "\
Here's what I changed:
1. Renamed the function
2. Added a guard clause

The build passes now.";
        assert!(
            fast_path_detect(screen).is_none(),
            "ordinary numbered prose must NOT fire the fast-path"
        );
    }

    #[test]
    fn numbered_prose_without_cursor_is_not_an_option_row() {
        assert!(!has_cursored_numbered_option("1. first\n2. second"));
        assert!(has_cursored_numbered_option("❯ 1. first"));
        assert!(has_cursored_numbered_option("> 2. second"));
    }

    #[test]
    fn slice5_cursor_glyphs_fire_the_option_row_detector() {
        // codex `›` (U+203A), cursor-agent `→`, gemini `●` radio.
        assert!(has_cursored_numbered_option("› 1. Update now (npm install -g)"));
        assert!(has_cursored_numbered_option("→ 1. Run (once)"));
        assert!(has_cursored_numbered_option("● 1. Trust folder"));
        // grok radio rows: numbered + `(●)`/`(○)`, NO arrow glyph.
        assert!(has_cursored_numbered_option(
            "  2 (○) Yes, proceed"
        ));
        assert!(has_cursored_numbered_option(
            "1 (●) Yes, and don't ask again for anything (always-approve mode)"
        ));
        // Radio-less parenthetical prose stays inert.
        assert!(!has_cursored_numbered_option("1 (of 3) items processed"));
        // A bare glyph without a numbered row stays inert.
        assert!(!has_cursored_numbered_option("→ see the docs"));
        assert!(!has_cursored_numbered_option("● streaming enabled"));
    }

    // ── Slice 5 — verbatim Big-7 dialog fixtures (2026-07 studies) ────

    #[test]
    fn grok_permission_gate_fires_permission_with_radio_options() {
        // Verbatim from tui-signal-study-grok-hermes-cursor.md — the
        // always-approve default makes this THE dialog K2 must never
        // blind-Enter (see workspace_msg's gate-hold).
        let screen = "\
Run rm -rf build?

1 (●) Yes, and don't ask again for anything (always-approve mode)
2 (○) Yes, proceed
3 (○) No, reject (type to add feedback)

1/3:select │ Ctrl+o:yolo │ Ctrl+c:cancel";
        let r = fast_path_detect(screen).expect("grok gate must fire fast-path");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "permission");
        assert_eq!(r.questions.len(), 1);
        assert_eq!(
            r.questions[0].options,
            vec![
                "Yes, and don't ask again for anything (always-approve mode)",
                "Yes, proceed",
                "No, reject (type to add feedback)",
            ]
        );
    }

    #[test]
    fn hermes_dangerous_command_box_fires_permission() {
        // Verbatim hermes gate (60s countdown auto-denies on expiry).
        let screen = "\
⚠️ Dangerous Command

  rm -rf /tmp/scratch

❯ 1. Allow once
  2. Allow for session
  3. Add to allowlist
  4. Deny

↑/↓ to select, Enter to confirm (59s)";
        let r = fast_path_detect(screen).expect("hermes gate must fire fast-path");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "permission");
        assert_eq!(
            r.questions[0].options,
            vec!["Allow once", "Allow for session", "Add to allowlist", "Deny"]
        );
    }

    #[test]
    fn cursor_agent_command_approval_fires_permission() {
        // Verbatim cursor-agent approval bar + transcript line.
        let screen = "\
$ echo test in .

Run this command?

Not in allowlist: echo

→ Run (once) (y)   Add Shell(echo) to allowlist? (tab)   Run Everything (shift+tab)   Skip (esc or n)

Waiting for approval...";
        let r = fast_path_detect(screen).expect("cursor approval must fire fast-path");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "permission");
    }

    #[test]
    fn codex_trust_dialog_fires_permission() {
        // "Do you trust" matched NOTHING before slice 5 (codex study;
        // cyan-highlight selection, no cursor glyph).
        let screen = "\
Do you trust the contents of this directory?

  /Users/rosson/scratch

  Yes, continue
  No, quit

Press enter to continue";
        let r = fast_path_detect(screen).expect("codex trust dialog must fire fast-path");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "permission");
    }

    #[test]
    fn gemini_trust_dialog_fires_permission_with_radio_cursor_options() {
        // Verbatim gemini trust dialog — `●` radio, NO footer.
        let screen = "\
Do you trust the files in this folder?

● 1. Trust folder
  2. Trust parent folder
  3. Don't trust";
        let r = fast_path_detect(screen).expect("gemini trust dialog must fire fast-path");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "permission");
        assert_eq!(
            r.questions[0].options,
            vec!["Trust folder", "Trust parent folder", "Don't trust"]
        );
    }

    #[test]
    fn codex_dialog_with_press_enter_to_continue_fires_select() {
        // codex update dialog: `›` cursor + "Press enter to continue"
        // footer — missed every pre-slice-5 SELECT marker.
        let screen = "\
A new version of codex is available.

› 1. Update now (npm install -g)
  2. Skip

Press enter to continue";
        let r = fast_path_detect(screen).expect("codex dialog must fire fast-path");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "select");
        assert_eq!(
            r.questions[0].options,
            vec!["Update now (npm install -g)", "Skip"]
        );
    }

    #[test]
    fn grok_colon_compact_footer_fires_select() {
        // grok dialog footers use colon-compact hints with no 'to'
        // ("↑/↓ Navigate", "Enter:submit") — claude's entries can't
        // match them.
        let screen = "\
Run Grok Build in a project directory?

1 (●) Use current directory
2 (○) Choose another directory

↑/↓ Navigate │ Enter:submit";
        let r = fast_path_detect(screen).expect("grok picker must fire fast-path");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "select");
    }

    // ── End-to-end classify_screen ───────────────────────────────────

    #[test]
    fn classify_screen_select_marker_is_fastpath() {
        let screen = "Choose:\n❯ 1. Yes\n  2. No\nEnter to select";
        let r = classify_screen(screen);
        assert_eq!(r.source, ClassifySource::Fastpath);
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "select");
    }

    #[test]
    fn classify_screen_permission_marker_is_fastpath() {
        let screen = "Edit src/main.rs?\nDo you want to proceed?\n❯ 1. Yes\n  2. No";
        let r = classify_screen(screen);
        assert_eq!(r.source, ClassifySource::Fastpath);
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "permission");
    }

    #[test]
    fn classify_screen_no_marker_is_unavailable_not_clear() {
        // No marker: the verdict is unknown (is_hitl=null), which `talk`
        // treats as "use manual behavior". It must never claim "clear".
        let screen = "rosson@host ~/proj % ls\nCargo.toml  src  target\nrosson@host ~/proj % ";
        let r = classify_screen(screen);
        assert_eq!(r.source, ClassifySource::Unavailable);
        assert_eq!(r.is_hitl, None);
        assert_eq!(r.kind, "none");
        assert!(r.questions.is_empty());
    }
}
