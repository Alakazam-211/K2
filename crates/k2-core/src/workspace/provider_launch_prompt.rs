//! Host-session launch-parameter prompt helpers (prd-host-session-launch-param-prompt-v1).
//!
//! Interactive stay-open CLI forms only — never one-shot print/exec modes
//! (`--print`, `-p`, `codex exec`, hermes `-q`/`-z`). Hermes and unknown
//! commands return `None` / `supports_launch_param == false` so the route
//! falls back to post-spawn inject (D10).
//!
//! Fire-once: callers attach the prompt only on the **ephemeral** exec argv;
//! durable `args_json` / `DaemonPtySession.args` must keep identity-only args.

use super::provider_resume::provider_resume_for_command;

/// Conservative ARG_MAX budget for a single trailing launch-prompt token.
/// Real `ARG_MAX` is much larger; this is a defensive soft ceiling so a
/// multi-MB guest policy + prompt falls back to inject instead of E2BIG.
const LAUNCH_PROMPT_SOFT_MAX_BYTES: usize = 100 * 1024;

/// Does this command support interactive positional first-turn / resume
/// launch-param prompts for host-sessions?
///
/// `true` for Claude, Codex, Grok, Gemini, Cursor Agent, Pi.
/// `false` for Hermes (no interactive first-turn argv) and unknowns.
pub fn supports_launch_param(command: &str) -> bool {
    match provider_resume_for_command(command).map(|p| p.provider) {
        Some("claude" | "codex" | "grok" | "gemini" | "cursor" | "pi") => true,
        // hermes: inject fallback; unknown: inject fallback
        _ => false,
    }
}

/// Append `prompt` as a single trailing interactive user-message arg after
/// identity flags already present in `args`.
///
/// Returns `None` when:
/// - command does not support launch-param;
/// - prompt is empty / whitespace-only;
/// - payload exceeds soft ARG_MAX budget;
/// - prompt starts with `-` and we cannot safely separate it.
///
/// Never injects `--print`, `-p`, `exec`, `-q`, or `-z`.
///
/// Prefer code SSOT resume flags (`--resume` / `--session` / `resume` subcommand)
/// already assembled by [`super::provider_resume::ProviderResume`]; this helper
/// only appends the prompt token (optionally after `--` for leading-dash text).
pub fn append_interactive_prompt(
    command: &str,
    args: &[String],
    prompt: &str,
) -> Option<Vec<String>> {
    if !supports_launch_param(command) {
        return None;
    }
    let prompt = prompt.trim();
    if prompt.is_empty() {
        return None;
    }
    if prompt.len() > LAUNCH_PROMPT_SOFT_MAX_BYTES {
        return None;
    }

    let mut out = args.to_vec();

    // Leading-dash prompts would be parsed as flags. Prefer `--` separator
    // (Claude / Grok / Gemini / Cursor / Pi / Codex all accept it as end-of-options).
    if prompt.starts_with('-') {
        out.push("--".to_string());
    }
    out.push(prompt.to_string());
    Some(out)
}

/// Redact trailing launch-prompt payload from args for debug logs.
///
/// When `prompt` is non-empty and equals the last arg (or the arg after a
/// trailing `--`), replace that token with `"<launch-prompt>"`. Never logs
/// the raw payload.
pub fn redact_launch_prompt_args(args: &[String], prompt: &str) -> Vec<String> {
    let prompt = prompt.trim();
    if prompt.is_empty() || args.is_empty() {
        return args.to_vec();
    }
    let mut out = args.to_vec();
    if out.last().map(|s| s.as_str()) == Some(prompt) {
        let n = out.len();
        out[n - 1] = "<launch-prompt>".to_string();
        // Also redact a preceding `--` we may have inserted for leading-dash text.
        if n >= 2 && out[n - 2] == "--" {
            // leave `--` — harmless and shows grammar; payload already redacted
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn supports_big7_interactive_roster() {
        for cmd in ["claude", "codex", "grok", "gemini", "cursor-agent", "pi"] {
            assert!(
                supports_launch_param(cmd),
                "{cmd} must support launch-param"
            );
        }
        // Path-qualified binary still matches.
        assert!(supports_launch_param("/usr/local/bin/claude"));
    }

    #[test]
    fn hermes_and_unknown_do_not_support() {
        assert!(!supports_launch_param("hermes"));
        assert!(!supports_launch_param("/opt/hermes"));
        assert!(!supports_launch_param("aider"));
        assert!(!supports_launch_param(""));
        assert!(!supports_launch_param("bash"));
    }

    #[test]
    fn claude_cold_trailing_prompt_no_print() {
        let base = args(&["--dangerously-skip-permissions", "--session-id", "SID"]);
        let out = append_interactive_prompt("claude", &base, "hello world").unwrap();
        assert_eq!(
            out,
            args(&[
                "--dangerously-skip-permissions",
                "--session-id",
                "SID",
                "hello world"
            ])
        );
        assert!(!out.iter().any(|a| a == "--print" || a == "-p"));
    }

    #[test]
    fn claude_resume_ssot_flag_plus_prompt() {
        let base = args(&["--dangerously-skip-permissions", "--resume", "SID"]);
        let out = append_interactive_prompt("claude", &base, "next turn").unwrap();
        assert_eq!(
            out,
            args(&[
                "--dangerously-skip-permissions",
                "--resume",
                "SID",
                "next turn"
            ])
        );
        // Code SSOT is --resume (not only -r).
        assert!(out.windows(2).any(|w| w[0] == "--resume" && w[1] == "SID"));
    }

    #[test]
    fn codex_resume_subcommand_plus_prompt() {
        let base = args(&["resume", "SID"]);
        let out = append_interactive_prompt("codex", &base, "continue please").unwrap();
        assert_eq!(out, args(&["resume", "SID", "continue please"]));
        assert!(!out.iter().any(|a| a == "exec"));
    }

    #[test]
    fn codex_cold_positional_prompt() {
        let out = append_interactive_prompt("codex", &[], "cold start").unwrap();
        assert_eq!(out, args(&["cold start"]));
    }

    #[test]
    fn pi_session_not_continue() {
        let base = args(&["--session", "SID"]);
        let out = append_interactive_prompt("pi", &base, "pi next").unwrap();
        assert_eq!(out, args(&["--session", "SID", "pi next"]));
        assert!(!out.iter().any(|a| a == "--continue"));
        // Must not invent pi's interactive picker --resume.
        assert!(!out.iter().any(|a| a == "--resume"));
    }

    #[test]
    fn gemini_cursor_grok_flag_resume_shapes() {
        for cmd in ["gemini", "cursor-agent", "grok"] {
            let base = args(&["--resume", "SID"]);
            let out = append_interactive_prompt(cmd, &base, "go").unwrap();
            assert_eq!(out, args(&["--resume", "SID", "go"]), "{cmd}");
            assert!(!out.iter().any(|a| a == "--print" || a == "-p" || a == "--single"));
        }
    }

    #[test]
    fn hermes_returns_none() {
        assert!(append_interactive_prompt("hermes", &args(&["--resume", "SID"]), "x").is_none());
    }

    #[test]
    fn prompt_stays_last_after_identity_splice_shape() {
        // spawn.rs splices `--append-system-prompt <brief>` onto identity
        // args, then appends the user prompt. The user text must stay last
        // (Claude treats a later flag as part of the prompt and would
        // mis-parse trailing flags).
        let identity = args(&[
            "--resume",
            "SID",
            "--append-system-prompt",
            "You are cell X",
        ]);
        let out = append_interactive_prompt("claude", &identity, "wake me").unwrap();
        assert_eq!(out.last().map(String::as_str), Some("wake me"));
        assert_eq!(
            out,
            args(&[
                "--resume",
                "SID",
                "--append-system-prompt",
                "You are cell X",
                "wake me",
            ])
        );
    }

    #[test]
    fn empty_prompt_returns_none() {
        assert!(append_interactive_prompt("claude", &[], "").is_none());
        assert!(append_interactive_prompt("claude", &[], "   ").is_none());
    }

    #[test]
    fn leading_dash_prompt_uses_double_dash_separator() {
        let out = append_interactive_prompt("claude", &args(&["--session-id", "SID"]), "-not-a-flag")
            .unwrap();
        assert_eq!(
            out,
            args(&["--session-id", "SID", "--", "-not-a-flag"])
        );
    }

    #[test]
    fn redact_hides_prompt_payload() {
        let secret = "TOP-SECRET-INTEGRATOR-PROMPT";
        let args = args(&["--resume", "SID", secret]);
        let redacted = redact_launch_prompt_args(&args, secret);
        assert_eq!(redacted, args_vec(&["--resume", "SID", "<launch-prompt>"]));
        assert!(!redacted.iter().any(|a| a.contains("TOP-SECRET")));
    }

    fn args_vec(xs: &[&str]) -> Vec<String> {
        args(xs)
    }

    #[test]
    fn soft_argmax_fallback() {
        let huge = "x".repeat(LAUNCH_PROMPT_SOFT_MAX_BYTES + 1);
        assert!(append_interactive_prompt("claude", &[], &huge).is_none());
    }
}
