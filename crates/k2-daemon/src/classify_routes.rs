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
//! This is the load-bearing detection primitive that `talk`
//! auto-routing (Phase 2, gated on a validation pass) will consume.
//! Phase 1 (this module) ships the endpoint + prompt + fast-path ONLY
//! — it does NOT wire `talk` routing.
//!
//! # Two-stage detection
//!
//! 1. **Regex fast-path** (≈0 ms, no model). If the screen carries an
//!    obvious, stable HITL marker (Claude's `Enter to select` menu
//!    footer, a numbered `❯ 1.` option cursor, the `☐ … ✔ Submit →`
//!    tab-bar, a `Do you want to proceed?` permission gate) we
//!    short-circuit to `is_hitl=true` and best-effort extract the
//!    questions with a cheap parse. `source="fastpath"`. This path is
//!    MODEL-INDEPENDENT, so obvious HITLs are caught even when the
//!    bundled LLM isn't installed.
//!
//! 2. **qwen classify + extract** (`source="model"`). If the fast-path
//!    doesn't fire AND the bundled model is loaded, run a temp-0-ish,
//!    greedy generation with [`CLASSIFY_SYSTEM_PROMPT`] and parse the
//!    strict-JSON reply.
//!
//! # Degrade-gracefully contract (Rosson)
//!
//! "If the LLM is not installed, stick with how it currently works."
//! So when the model is unavailable AND the fast-path didn't fire we
//! return `is_hitl: null` (JSON null = "unknown / no detector") with
//! `source="unavailable"`. The caller (`talk` auto-routing) keys on
//! `source=="unavailable"` to fall back to today's MANUAL behavior —
//! it must NOT auto-hold / auto-scout when there is simply no detector.
//!
//! # Safety bias (scoped)
//!
//! A held message is benign; an *eaten* message (sent into an open
//! HITL) is the only dangerous failure. So when the model RAN but
//! produced ambiguous / unparseable output we bias to
//! `is_hitl=true` (hold, don't send). The safety bias applies ONLY to
//! `source="model"`; it does NOT apply to `unavailable` (see above).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::cli_response::CliResponse;
use crate::llm_host;

/// How many lines of screen tail we feed the classifier. A Claude
/// select menu + its surrounding context comfortably fits in 60 rows
/// (a typical terminal is ~24-50 rows), and keeping the prompt tight
/// helps the 1.5B model stay on-task.
const CLASSIFY_SCREEN_LINES: usize = 60;

/// The classify + extract prompt. Kept as a clearly-marked constant so
/// "improve the detector over time" = "edit this prompt" (no code
/// change, no retrain). The model is the bundled Qwen2.5-1.5B-Instruct.
///
/// Contract the parser depends on: the model must emit ONE JSON object
/// with `is_hitl` / `kind` / `questions`. We still parse defensively
/// (fences stripped, braces sliced, safety-bias on failure) because a
/// 1.5B model will occasionally wrap it in prose.
pub const CLASSIFY_SYSTEM_PROMPT: &str = r#"You are a strict detector of HUMAN-IN-THE-LOOP (HITL) prompts on a terminal screen.

A HITL prompt is an interactive question that is WAITING for a human to choose or confirm before the program continues. Examples:
- A numbered selection menu (e.g. "1. ... 2. ... 3. ...", often with "Enter to select" or arrow-key hints).
- A yes/no permission gate (e.g. "Do you want to proceed?", "Allow this action?").
- A multi-question form with selectable options and a Submit button.

NOT a HITL:
- Normal command output, logs, a progress bar, a diff.
- A shell prompt waiting for the next command (e.g. "$", "%", "user@host:~$").
- An AI assistant's prose answer that is NOT asking the human to pick an option.

Read the TERMINAL SCREEN below and reply with ONE line of STRICT JSON and NOTHING else — no prose, no explanation, no markdown code fences:
{"is_hitl": true|false, "kind": "select|permission|none", "questions": [{"header": "the question text", "options": ["option 1", "option 2"]}]}

Rules:
- "kind": "select" for a choose-an-option menu/form, "permission" for a yes/no confirmation gate, "none" when it is not a HITL.
- "questions": one entry per distinct question visible on screen. "options" lists the selectable choices verbatim. Use an empty array when there are no questions or none is a HITL.
- If you are UNSURE whether the screen is waiting for human input, answer "is_hitl": true — it is safer to flag a HITL than to miss one.
- Output the JSON object ONLY."#;

// ─── Response shape ──────────────────────────────────────────────────

/// What stage produced the verdict. The caller routes on this:
/// `fastpath`/`model` carry a real `is_hitl`; `unavailable` means "no
/// detector — fall back to manual behavior".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassifySource {
    /// A deterministic marker matched — works WITHOUT the model.
    Fastpath,
    /// The bundled qwen model classified the screen.
    Model,
    /// Model not installed/loaded AND the fast-path didn't fire.
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
    /// Scout payload — the questions + options on screen (best-effort
    /// on the fast-path, model-extracted otherwise). Empty when none.
    pub questions: Vec<ClassifyQuestion>,
    /// Human-readable note on WHY (model-unavailable, parse-error,
    /// which marker fired). Omitted when empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl ClassifyResult {
    fn into_response(self) -> CliResponse {
        CliResponse::ok_json(
            serde_json::to_string(&self).unwrap_or_else(|_| {
                // Last-ditch: never fail the request. A serialize error
                // is itself "uncertain" → safety-bias hold.
                r#"{"is_hitl":true,"source":"model","kind":"none","questions":[],"detail":"serialize error"}"#
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

    // "Use local LLM to detect HITL" opt-in (Settings → General, default
    // OFF). This is the literal enable/disable for the bundled 1.5B model
    // on the HITL-detection path: OFF ⇒ the model NEVER runs, classify is
    // the regex fast-path only.
    let use_llm = k2_core::app_settings::load().use_llm_hitl_detection;

    classify_screen(&screen, use_llm).into_response()
}

/// Pure classification over already-rendered screen text. Split from
/// the HTTP handler so unit tests can exercise the fast-path + parse
/// robustness without a session or the network.
///
/// `use_llm` is the "Use local LLM to detect HITL" setting. When `false`
/// the bundled model is NEVER consulted: a no-marker screen returns
/// `source=Unavailable` / `is_hitl=null` so the caller (`talk`) falls
/// back to today's regex/manual behavior. The regex fast-path runs
/// regardless of the flag — it is free and model-independent.
fn classify_screen(screen: &str, use_llm: bool) -> ClassifyResult {
    // 1. Regex fast-path — model-independent, ≈0 ms. Always runs.
    if let Some(result) = fast_path_detect(screen) {
        return result;
    }

    // GATE: with the "Use local LLM to detect HITL" checkbox OFF, the
    // 1.5B model MUST NOT run. The fast-path didn't fire, so we report
    // `unavailable` (unknown — NOT "clear") and the caller uses today's
    // regex/manual behavior. We return BEFORE touching `llm_host` so no
    // inference is ever queued.
    if !use_llm {
        return ClassifyResult {
            is_hitl: None,
            source: ClassifySource::Unavailable,
            kind: "none".to_string(),
            questions: Vec::new(),
            detail: Some(
                "LLM HITL detection disabled (Settings → General → \
                 \"Use local LLM to detect HITL\"); fast-path did not fire \
                 — caller should use regex/manual behavior"
                    .to_string(),
            ),
        };
    }

    // 2. Model path — only if the bundled model is actually loaded
    //    (path set AND file exists). Mirrors `llm_routes::handle_check`
    //    / `LlmHost::status().loaded`. The worker loads the GGUF on
    //    demand, so "ensure loaded" == "a model file is ready to spawn".
    let host = llm_host::shared();
    if !host.status().loaded {
        // Degrade gracefully: no detector → is_hitl unknown (null).
        // The caller falls back to today's manual behavior. NOT a
        // safety-bias hold — that would break "no LLM → current
        // behavior".
        return ClassifyResult {
            is_hitl: None,
            source: ClassifySource::Unavailable,
            kind: "none".to_string(),
            questions: Vec::new(),
            detail: Some(
                "bundled classifier model not installed/loaded; \
                 fast-path did not fire — caller should use manual behavior"
                    .to_string(),
            ),
        };
    }

    // Run the model. Greedy-ish, deterministic generation (the bundled
    // worker samples at temp 0.1 — effectively greedy for this short
    // strict-JSON task).
    let raw = match host.generate(CLASSIFY_SYSTEM_PROMPT, screen, llm_host::default_timeout()) {
        Ok(s) => s,
        Err(e) => {
            // Model ran-path but errored (timeout/crash/queue-full). It
            // is the `source="model"` branch, so the SAFETY BIAS
            // applies → hold (is_hitl=true).
            return ClassifyResult {
                is_hitl: Some(true),
                source: ClassifySource::Model,
                kind: "none".to_string(),
                questions: Vec::new(),
                detail: Some(format!("model error (safety-bias hold): {}", e.message())),
            };
        }
    };

    parse_model_json(&raw)
}

// ─── Regex fast-path ─────────────────────────────────────────────────

/// Deterministic HITL markers we key on. Each is a stable UI string
/// from a known interactive prompt. Matching is case-insensitive on a
/// lowercased copy of the screen.
///
/// Markers (documented for the validation agent):
///   1. `enter to select`        — Claude Code select-menu footer
///      (`Enter to select · ↑/↓ to navigate · Esc to cancel`).
///   2. `↑/↓ to navigate`        — same footer, arrow-key hint.
///   3. `✔ submit` / `submit →`  — the multi-question tab-bar form's
///      Submit affordance (`☐ … ✔ Submit →`).
///   4. `do you want to proceed` / `do you want to` — Claude's
///      tool-permission confirmation gate.
///   5. a `❯`/`>`-cursored numbered option row (`❯ 1. …`) — the
///      arrow cursor sitting on a numbered choice.
const SELECT_MARKERS: &[&str] = &[
    "enter to select",
    "↑/↓ to navigate",
    "✔ submit",
    "submit →",
];

/// Permission-gate markers (yes/no confirmation). Checked before the
/// generic select markers so a confirmation prompt is `kind=permission`.
const PERMISSION_MARKERS: &[&str] = &[
    "do you want to proceed",
    "do you want to make this edit",
    "do you want to create",
    "do you want to",
];

/// Best-effort detector. Returns `Some(ClassifyResult)` (always
/// `is_hitl=true`, `source=Fastpath`) when a marker fires, else `None`
/// so the caller falls through to the model.
fn fast_path_detect(screen: &str) -> Option<ClassifyResult> {
    let lower = screen.to_lowercase();

    let permission_hit = PERMISSION_MARKERS.iter().find(|m| lower.contains(**m));
    let select_hit = SELECT_MARKERS.iter().find(|m| lower.contains(**m));
    // A `❯ 1.` / `> 1.` cursored numbered option is also a strong
    // select signal even without the footer text.
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

/// True if any line looks like an arrow-cursored numbered menu option,
/// e.g. `❯ 1. Yes` or `> 2. No`. The leading cursor glyph distinguishes
/// an active menu from ordinary numbered prose ("1. First, do X").
fn has_cursored_numbered_option(screen: &str) -> bool {
    screen.lines().any(|line| {
        let t = line.trim_start();
        let rest = t
            .strip_prefix('❯')
            .or_else(|| t.strip_prefix('>'))
            .map(str::trim_start);
        match rest {
            Some(r) => starts_with_numbered_option(r),
            None => false,
        }
    })
}

/// `"1. Yes"` / `"12) foo"` → true. A leading run of ASCII digits
/// followed by `.` or `)` and a space.
fn starts_with_numbered_option(s: &str) -> bool {
    let digits: String = s.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return false;
    }
    let after = &s[digits.len()..];
    matches!(after.as_bytes().first(), Some(b'.') | Some(b')'))
}

/// Cheap, best-effort question/option extraction for the fast-path. We
/// collect numbered option rows (`1. foo`, `❯ 2. bar`) into one
/// question whose `header` is the nearest preceding non-empty,
/// non-option line (typically the question text). This is intentionally
/// simple — the model path does the richer extraction; the fast-path
/// just needs a usable scout payload.
fn extract_questions_cheap(screen: &str) -> Vec<ClassifyQuestion> {
    let lines: Vec<&str> = screen.lines().collect();
    let mut options: Vec<String> = Vec::new();
    let mut first_option_idx: Option<usize> = None;

    for (i, raw) in lines.iter().enumerate() {
        let t = raw.trim();
        // strip an optional leading cursor glyph
        let body = t
            .strip_prefix('❯')
            .or_else(|| t.strip_prefix('>'))
            .map(str::trim_start)
            .unwrap_or(t);
        if starts_with_numbered_option(body) {
            // drop the "N. " / "N) " prefix to get the option label
            let digits: String = body.chars().take_while(|c| c.is_ascii_digit()).collect();
            let label = body[digits.len() + 1..].trim().to_string();
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
    let body = line
        .strip_prefix('❯')
        .or_else(|| line.strip_prefix('>'))
        .map(str::trim_start)
        .unwrap_or(line);
    starts_with_numbered_option(body)
}

// ─── Model-JSON parsing (robust) ─────────────────────────────────────

/// Intermediate shape the model is asked to emit. Every field is
/// `#[serde(default)]` so a partial object still deserializes.
#[derive(Debug, Deserialize)]
struct ModelClassify {
    #[serde(default)]
    is_hitl: bool,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    questions: Vec<ModelQuestion>,
}

#[derive(Debug, Deserialize)]
struct ModelQuestion {
    #[serde(default)]
    header: String,
    #[serde(default)]
    options: Vec<String>,
}

/// Parse the model's raw text into a `ClassifyResult`. Robust to prose
/// and markdown fences: we strip ```json fences, slice the outermost
/// `{ … }`, and deserialize. On ANY failure we apply the SAFETY BIAS —
/// `is_hitl=true`, `source=Model` — and never panic.
fn parse_model_json(raw: &str) -> ClassifyResult {
    let candidate = extract_json_object(raw);

    let parsed: Option<ModelClassify> =
        candidate.and_then(|c| serde_json::from_str::<ModelClassify>(&c).ok());

    match parsed {
        Some(m) => {
            let kind = m.kind.unwrap_or_else(|| {
                if m.is_hitl { "select".to_string() } else { "none".to_string() }
            });
            let questions = m
                .questions
                .into_iter()
                .map(|q| ClassifyQuestion {
                    header: q.header,
                    options: q.options,
                })
                .collect();
            ClassifyResult {
                is_hitl: Some(m.is_hitl),
                source: ClassifySource::Model,
                kind,
                questions,
                detail: None,
            }
        }
        None => ClassifyResult {
            // Model RAN but produced unparseable output → safety-bias hold.
            is_hitl: Some(true),
            source: ClassifySource::Model,
            kind: "none".to_string(),
            questions: Vec::new(),
            detail: Some("model output unparseable (safety-bias hold)".to_string()),
        },
    }
}

/// Pull the outermost `{ … }` JSON object out of a possibly-noisy
/// model reply: strips ```json / ``` fences, then slices from the first
/// `{` to the last `}`. Returns `None` if no brace pair is present.
fn extract_json_object(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    // Strip markdown code fences if present.
    let no_fence = trimmed
        .strip_prefix("```json")
        .or_else(|| trimmed.strip_prefix("```JSON"))
        .or_else(|| trimmed.strip_prefix("```"))
        .map(|s| s.trim_start())
        .unwrap_or(trimmed);
    let no_fence = no_fence.strip_suffix("```").unwrap_or(no_fence);

    let start = no_fence.find('{')?;
    let end = no_fence.rfind('}')?;
    if end < start {
        return None;
    }
    Some(no_fence[start..=end].to_string())
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

    // ── Model-JSON parsing / robustness ──────────────────────────────

    #[test]
    fn parse_well_formed_model_json() {
        let raw = r#"{"is_hitl": true, "kind": "select", "questions": [{"header": "Pick", "options": ["a", "b"]}]}"#;
        let r = parse_model_json(raw);
        assert_eq!(r.source, ClassifySource::Model);
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "select");
        assert_eq!(r.questions.len(), 1);
        assert_eq!(r.questions[0].header, "Pick");
        assert_eq!(r.questions[0].options, vec!["a", "b"]);
    }

    #[test]
    fn parse_well_formed_negative() {
        let raw = r#"{"is_hitl": false, "kind": "none", "questions": []}"#;
        let r = parse_model_json(raw);
        assert_eq!(r.is_hitl, Some(false));
        assert_eq!(r.kind, "none");
        assert!(r.questions.is_empty());
    }

    #[test]
    fn parse_json_wrapped_in_markdown_fences() {
        let raw = "```json\n{\"is_hitl\": true, \"kind\": \"permission\", \"questions\": []}\n```";
        let r = parse_model_json(raw);
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "permission");
    }

    #[test]
    fn parse_json_with_surrounding_prose() {
        let raw = "Sure! Here is the classification:\n{\"is_hitl\": false, \"kind\": \"none\", \"questions\": []}\nHope that helps.";
        let r = parse_model_json(raw);
        assert_eq!(r.is_hitl, Some(false));
    }

    #[test]
    fn parse_garbage_applies_safety_bias() {
        let r = parse_model_json("I cannot tell, sorry — no JSON here.");
        // model RAN, unparseable → safety bias HOLD
        assert_eq!(r.source, ClassifySource::Model);
        assert_eq!(r.is_hitl, Some(true), "garbage must safety-bias to hold");
        assert!(r.detail.is_some());
    }

    #[test]
    fn parse_empty_string_applies_safety_bias() {
        let r = parse_model_json("");
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.source, ClassifySource::Model);
    }

    #[test]
    fn parse_missing_kind_defaults_sensibly() {
        // is_hitl true but no kind → defaults to "select"
        let r = parse_model_json(r#"{"is_hitl": true, "questions": []}"#);
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "select");
    }

    // ── End-to-end classify_screen: fast-path short-circuits model ───

    #[test]
    fn classify_screen_uses_fastpath_without_model() {
        // This screen has an obvious marker, so classify_screen must
        // return a Fastpath verdict WITHOUT consulting the model — safe
        // to run in CI where no model is installed. The fast-path fires
        // regardless of the `use_llm` flag, so a marker hit is Fastpath
        // even with the LLM detection checkbox OFF.
        let screen = "Choose:\n❯ 1. Yes\n  2. No\nEnter to select";
        let r = classify_screen(screen, false);
        assert_eq!(r.source, ClassifySource::Fastpath);
        assert_eq!(r.is_hitl, Some(true));
    }

    // ── "Use local LLM to detect HITL" gating (checkbox OFF) ─────────

    #[test]
    fn checkbox_off_fastpath_marker_is_fastpath_no_model() {
        // Checkbox OFF + a fast-path marker → Fastpath (model never
        // consulted). This is the "regex still catches obvious HITLs even
        // with the LLM off" guarantee.
        let screen = "Edit src/main.rs?\nDo you want to proceed?\n❯ 1. Yes\n  2. No";
        let r = classify_screen(screen, false);
        assert_eq!(r.source, ClassifySource::Fastpath);
        assert_eq!(r.is_hitl, Some(true));
        assert_eq!(r.kind, "permission");
    }

    #[test]
    fn checkbox_off_no_marker_is_unavailable_and_never_invokes_model() {
        // Checkbox OFF + NO fast-path marker → the model MUST NOT run.
        // We assert the verdict is the model-independent `Unavailable`
        // branch (is_hitl=null), which classify_screen returns BEFORE it
        // ever touches `llm_host`. If the gate were missing this screen
        // would fall through to the model/unavailable host-dependent path;
        // pinning source=Unavailable + is_hitl=None proves the 1.5B is not
        // invoked when the checkbox is off.
        let screen = "rosson@host ~/proj % ls\nCargo.toml  src  target\nrosson@host ~/proj % ";
        let r = classify_screen(screen, false);
        assert_eq!(r.source, ClassifySource::Unavailable);
        assert_eq!(r.is_hitl, None);
        // Unavailable is "unknown", NOT "clear" — never claims is_hitl=false.
        assert_ne!(r.is_hitl, Some(false));
    }

    // The real qwen inference path (source=Model over ~20 live screens)
    // is the validation-agent's job and requires the model file, which
    // may be absent in CI — so it is gated behind `#[ignore]` (run with
    // `cargo test -- --ignored` on a box with the GGUF loaded). With the
    // checkbox ON and no fast-path marker, classify_screen MUST reach the
    // model path: it returns either `Model` (model loaded) or
    // `Unavailable` (model not loaded) — but it is the only configuration
    // in which the model branch is reachable at all.
    #[test]
    #[ignore = "requires the bundled GGUF model; run with --ignored"]
    fn checkbox_on_no_marker_reaches_model_path() {
        let screen = "Tell me about your day. It was fine, thanks for asking.";
        let r = classify_screen(screen, true);
        // With a real model loaded this is Model; if absent it degrades to
        // Unavailable. Either way it must NOT be Fastpath (no marker here).
        assert_ne!(r.source, ClassifySource::Fastpath);
        assert!(
            matches!(r.source, ClassifySource::Model | ClassifySource::Unavailable),
            "checkbox ON must route a non-marker screen to the model path"
        );
    }
}
