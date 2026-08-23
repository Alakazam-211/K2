//! ModelFlag splice — apply a launch-time model id onto a resolved agent argv.
//!
//! Precedence (prd-workspace-default-model-and-api-model-override-v1):
//! API request `model` (non-empty after trim) > workspace `default_model` >
//! today's preset/harness. Empty workspace model + omitted API model must
//! stay **byte-identical** to today's spawn argv.
//!
//! Unknown binaries: log once, do not invent flags (`IgnoredUnsupported`).

use rusqlite::{params, Connection};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelApplyMode {
    WorkspaceDefault,
    ApiOverride,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelApplyResult {
    Applied,
    IgnoredUnsupported,
    AlreadyPinnedSkipped,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    Api,
    Workspace,
    #[serde(rename = "none")]
    None,
    IgnoredUnsupported,
    IgnoredLive,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelDecision {
    pub args: Vec<String>,
    /// The id actually spliced (None when we did not rewrite argv).
    pub applied: Option<String>,
    pub source: ModelSource,
    pub result: ModelApplyResult,
}

impl ModelDecision {
    fn unchanged(args: &[String], source: ModelSource, result: ModelApplyResult) -> Self {
        Self {
            args: args.to_vec(),
            applied: None,
            source,
            result,
        }
    }
}

/// How an existing model flag occupies `args`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelFlagForm {
    /// `--model` followed by a value at `index + 1` (or missing).
    LongSeparated,
    /// `--model=value` at `index`.
    LongEquals,
    /// `-m` followed by a value at `index + 1`.
    ShortSeparated,
    /// `-c` followed by `model=<id>` at `index + 1` (NOT `model_reasoning_effort=`).
    ConfigSeparated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExistingModelFlag {
    pub index: usize,
    pub form: ModelFlagForm,
    /// Number of argv slots this flag occupies (1 or 2).
    pub span: usize,
}

/// First argv token of `command`, with directory and trailing `.exe` stripped.
pub fn binary_token(command: &str) -> &str {
    let first = command.split_whitespace().next().unwrap_or(command).trim();
    let name = first.rsplit('/').next().unwrap_or(first);
    let name = name.rsplit('\\').next().unwrap_or(name);
    let stripped = name
        .strip_suffix(".exe")
        .or_else(|| name.strip_suffix(".EXE"))
        .unwrap_or(name);
    stripped
}

fn nonempty<'a>(s: Option<&'a str>) -> Option<&'a str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

/// True when argv already carries dead-resume grammar
/// (`--resume` / `codex resume` / pi `--session`). `--session-id` is premint,
/// not resume.
pub fn args_look_like_dead_resume(command: &str, args: &[String]) -> bool {
    if args
        .iter()
        .any(|a| a == "--resume" || a.starts_with("--resume="))
    {
        return true;
    }
    if args.iter().any(|a| a == "--session") {
        return true;
    }
    let bin = binary_token(command);
    if bin == "codex" && args.iter().any(|a| a == "resume") {
        return true;
    }
    false
}

fn is_config_model_pin(token: &str) -> bool {
    token.starts_with("model=") && !token.starts_with("model_reasoning_effort")
}

/// Locate every model-pin flag. `-c model_reasoning_effort=…` is NOT a pin.
pub fn existing_model_flag_indices<S: AsRef<str>>(args: &[S]) -> Vec<ExistingModelFlag> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_ref();
        if a == "--model" {
            let span = if i + 1 < args.len() { 2 } else { 1 };
            out.push(ExistingModelFlag {
                index: i,
                form: ModelFlagForm::LongSeparated,
                span,
            });
            i += span;
            continue;
        }
        if let Some(rest) = a.strip_prefix("--model=") {
            let _ = rest;
            out.push(ExistingModelFlag {
                index: i,
                form: ModelFlagForm::LongEquals,
                span: 1,
            });
            i += 1;
            continue;
        }
        if a == "-m" {
            let span = if i + 1 < args.len() { 2 } else { 1 };
            out.push(ExistingModelFlag {
                index: i,
                form: ModelFlagForm::ShortSeparated,
                span,
            });
            i += span;
            continue;
        }
        if a == "-c" && i + 1 < args.len() && is_config_model_pin(args[i + 1].as_ref()) {
            out.push(ExistingModelFlag {
                index: i,
                form: ModelFlagForm::ConfigSeparated,
                span: 2,
            });
            i += 2;
            continue;
        }
        i += 1;
    }
    out
}

/// Preferred insert flag for a known binary. `None` → do not invent flags.
fn insert_flag_for(bin: &str, args: &[String]) -> Option<&'static str> {
    match bin {
        "claude" => Some("--model"),
        "codex" | "grok" | "gemini" => Some("-m"),
        "cursor-agent" | "agent" | "pi" => Some("--model"),
        "hermes" => {
            if args.iter().any(|a| a == "chat") {
                Some("--model")
            } else {
                None
            }
        }
        _ => None,
    }
}

fn log_unsupported_once(bin: &str) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    let mut seen = match SEEN.lock() {
        Ok(g) => g,
        Err(p) => p.into_inner(),
    };
    if seen.iter().any(|s| s == bin) {
        return;
    }
    seen.push(bin.to_string());
    log_debug!("[model_splice] unknown binary {bin}: no model flag invented (IgnoredUnsupported)");
}

fn replace_model_flags(args: &[String], model: &str) -> Vec<String> {
    let flags = existing_model_flag_indices(args);
    if flags.is_empty() {
        return args.to_vec();
    }
    let mut out = Vec::with_capacity(args.len());
    let mut i = 0;
    let mut replaced = false;
    while i < args.len() {
        if let Some(f) = flags.iter().find(|f| f.index == i) {
            if !replaced {
                match f.form {
                    ModelFlagForm::LongEquals => {
                        out.push(format!("--model={model}"));
                    }
                    ModelFlagForm::LongSeparated => {
                        out.push("--model".into());
                        out.push(model.to_string());
                    }
                    ModelFlagForm::ShortSeparated => {
                        out.push("-m".into());
                        out.push(model.to_string());
                    }
                    ModelFlagForm::ConfigSeparated => {
                        out.push(args[i].clone());
                        out.push(format!("model={model}"));
                    }
                }
                replaced = true;
            }
            i += f.span;
        } else {
            out.push(args[i].clone());
            i += 1;
        }
    }
    out
}

fn insert_model_flag(args: &[String], flag: &str, model: &str, bin: &str) -> Vec<String> {
    let mut out = args.to_vec();
    if bin == "hermes" {
        if let Some(i) = out.iter().position(|a| a == "chat") {
            out.insert(i + 1, flag.to_string());
            out.insert(i + 2, model.to_string());
            return out;
        }
    }
    out.push(flag.to_string());
    out.push(model.to_string());
    out
}

/// Apply `model` onto `args` according to `mode`. Never mutates `command`.
/// Empty/whitespace `model` → identical argv + `None`.
pub fn apply_model(
    command: &str,
    args: &[String],
    model: Option<&str>,
    mode: ModelApplyMode,
) -> (Vec<String>, ModelApplyResult) {
    let Some(model) = nonempty(model) else {
        return (args.to_vec(), ModelApplyResult::None);
    };
    let bin = binary_token(command);
    if bin == "hermes" && !args.iter().any(|a| a == "chat") {
        return (args.to_vec(), ModelApplyResult::IgnoredUnsupported);
    }
    let Some(flag) = insert_flag_for(bin, args) else {
        log_unsupported_once(bin);
        return (args.to_vec(), ModelApplyResult::IgnoredUnsupported);
    };
    let existing = existing_model_flag_indices(args);
    if !existing.is_empty() {
        return match mode {
            ModelApplyMode::WorkspaceDefault => {
                (args.to_vec(), ModelApplyResult::AlreadyPinnedSkipped)
            }
            ModelApplyMode::ApiOverride => {
                (replace_model_flags(args, model), ModelApplyResult::Applied)
            }
        };
    }
    (
        insert_model_flag(args, flag, model, bin),
        ModelApplyResult::Applied,
    )
}

/// Resolve effective model + apply.
///
/// `api_model`: trimmed non-empty from the request, else None.
/// `resume`: true for dead-resume (new PTY with resume grammar).
/// `force_on_resume`: workspace checkbox.
/// `workspace_default`: `projects.default_model`.
pub fn decide_and_apply(
    command: &str,
    args: &[String],
    api_model: Option<&str>,
    workspace_default: Option<&str>,
    resume: bool,
    force_on_resume: bool,
) -> ModelDecision {
    let api = nonempty(api_model);
    let ws = nonempty(workspace_default);
    let (chosen, mode, source) = if resume {
        if let Some(id) = api {
            (Some(id), ModelApplyMode::ApiOverride, ModelSource::Api)
        } else if force_on_resume {
            if let Some(id) = ws {
                (
                    Some(id),
                    ModelApplyMode::WorkspaceDefault,
                    ModelSource::Workspace,
                )
            } else {
                return ModelDecision::unchanged(args, ModelSource::None, ModelApplyResult::None);
            }
        } else {
            return ModelDecision::unchanged(args, ModelSource::None, ModelApplyResult::None);
        }
    } else if let Some(id) = api {
        (Some(id), ModelApplyMode::ApiOverride, ModelSource::Api)
    } else if let Some(id) = ws {
        (
            Some(id),
            ModelApplyMode::WorkspaceDefault,
            ModelSource::Workspace,
        )
    } else {
        return ModelDecision::unchanged(args, ModelSource::None, ModelApplyResult::None);
    };
    let (new_args, result) = apply_model(command, args, chosen, mode);
    match result {
        ModelApplyResult::Applied => ModelDecision {
            args: new_args,
            applied: chosen.map(str::to_string),
            source,
            result,
        },
        ModelApplyResult::IgnoredUnsupported => ModelDecision {
            args: new_args,
            applied: None,
            source: ModelSource::IgnoredUnsupported,
            result,
        },
        ModelApplyResult::AlreadyPinnedSkipped => ModelDecision::unchanged(
            args,
            ModelSource::None,
            ModelApplyResult::AlreadyPinnedSkipped,
        ),
        ModelApplyResult::None => {
            ModelDecision::unchanged(args, ModelSource::None, ModelApplyResult::None)
        }
    }
}

/// `SELECT default_model, force_model_on_resume FROM projects WHERE path = ?1 OR id = ?1`.
/// Empty/whitespace `default_model` → None. Missing row → (None, false).
pub fn load_workspace_model(conn: &Connection, project_path_or_id: &str) -> (Option<String>, bool) {
    let row: Result<(Option<String>, i64), rusqlite::Error> = conn.query_row(
        "SELECT default_model, force_model_on_resume FROM projects WHERE path = ?1 OR id = ?1",
        params![project_path_or_id],
        |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, i64>(1).unwrap_or(0),
            ))
        },
    );
    match row {
        Ok((model, force)) => {
            let model = model.and_then(|s| {
                let t = s.trim();
                if t.is_empty() {
                    None
                } else {
                    Some(t.to_string())
                }
            });
            (model, force != 0)
        }
        Err(_) => (None, false),
    }
}

/// K2-direct spawn helper: load workspace default and splice (no API model).
/// No-op when neither default_model nor (resume+checkbox) apply — argv unchanged.
pub fn splice_model_for_workspace_spawn(
    ws_path: &str,
    command: &str,
    args: &mut Vec<String>,
    resume: bool,
) {
    if command.trim().is_empty() {
        return;
    }
    let (ws_default, force) = {
        let db = crate::db::shared();
        let conn = db.lock();
        load_workspace_model(&conn, ws_path)
    };
    if nonempty(ws_default.as_deref()).is_none() {
        return;
    }
    let decision = decide_and_apply(command, args, None, ws_default.as_deref(), resume, force);
    *args = decision.args;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(args: &[&str]) -> Vec<String> {
        args.iter().map(|a| a.to_string()).collect()
    }

    fn has_pair(args: &[String], flag: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == flag && w[1] == value)
    }

    fn has_eq(args: &[String], flag_eq: &str) -> bool {
        args.iter().any(|a| a == flag_eq)
    }

    /// 1. Workspace opus + claude preset no model flag → `--model` `opus`, Applied.
    #[test]
    fn workspace_opus_claude_no_flag_applies() {
        let args = s(&["--dangerously-skip-permissions"]);
        let (out, result) = apply_model(
            "claude",
            &args,
            Some("opus"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(
            result,
            ModelApplyResult::Applied,
            "expected Applied, got {result:?}"
        );
        assert!(
            has_pair(&out, "--model", "opus"),
            "argv must contain --model opus; got {out:?}"
        );
        assert!(
            out.iter().any(|a| a == "--dangerously-skip-permissions"),
            "existing flags must be preserved; got {out:?}"
        );
    }

    /// 2. Preset already `--model haiku`, workspace opus, WorkspaceDefault → stays haiku.
    #[test]
    fn workspace_default_leaves_preset_pin() {
        let args = s(&["--model", "haiku"]);
        let (out, result) = apply_model(
            "claude",
            &args,
            Some("opus"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(
            result,
            ModelApplyResult::AlreadyPinnedSkipped,
            "expected AlreadyPinnedSkipped, got {result:?}"
        );
        assert_eq!(out, args, "preset pin must be byte-identical; got {out:?}");
        assert!(has_pair(&out, "--model", "haiku"));
        assert!(
            !has_pair(&out, "--model", "opus"),
            "workspace default must not stomp preset pin; got {out:?}"
        );
    }

    /// 3. Same + ApiOverride sonnet → becomes sonnet.
    #[test]
    fn api_override_replaces_preset_pin() {
        let args = s(&["--model", "haiku"]);
        let (out, result) =
            apply_model("claude", &args, Some("sonnet"), ModelApplyMode::ApiOverride);
        assert_eq!(
            result,
            ModelApplyResult::Applied,
            "expected Applied, got {result:?}"
        );
        assert!(
            has_pair(&out, "--model", "sonnet"),
            "API override must become sonnet; got {out:?}"
        );
        assert!(
            !has_pair(&out, "--model", "haiku"),
            "old pin must be gone; got {out:?}"
        );
    }

    /// 4. API omit, workspace None → identical argv, None.
    #[test]
    fn omit_both_is_byte_identical() {
        let args = s(&["--dangerously-skip-permissions", "--session-id", "abc"]);
        let d = decide_and_apply("claude", &args, None, None, false, false);
        assert_eq!(
            d.result,
            ModelApplyResult::None,
            "expected None, got {:?}",
            d.result
        );
        assert_eq!(d.source, ModelSource::None);
        assert_eq!(d.applied, None);
        assert_eq!(
            d.args, args,
            "omit both must be byte-identical; got {:?}",
            d.args
        );
        let (out, result) = apply_model("claude", &args, None, ModelApplyMode::WorkspaceDefault);
        assert_eq!(result, ModelApplyResult::None);
        assert_eq!(out, args);
    }

    /// 5. Unknown binary + workspace model → no flag, IgnoredUnsupported.
    #[test]
    fn unknown_binary_is_ignored_unsupported() {
        let args = s(&["-c"]);
        let (out, result) = apply_model(
            "/usr/local/bin/my-custom-agent",
            &args,
            Some("opus"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(
            result,
            ModelApplyResult::IgnoredUnsupported,
            "expected IgnoredUnsupported, got {result:?}"
        );
        assert_eq!(
            out, args,
            "unknown binary must not invent flags; got {out:?}"
        );
        let d = decide_and_apply("my-custom-agent", &args, None, Some("opus"), false, false);
        assert_eq!(d.source, ModelSource::IgnoredUnsupported);
        assert_eq!(d.applied, None);
        assert_eq!(d.args, args);
    }

    /// 6. Codex + model → `-m` present; existing `-c model_reasoning_effort=high` preserved.
    #[test]
    fn codex_inserts_dash_m_and_preserves_effort() {
        let args = s(&["-c", "model_reasoning_effort=high"]);
        let (out, result) =
            apply_model("codex", &args, Some("o3"), ModelApplyMode::WorkspaceDefault);
        assert_eq!(
            result,
            ModelApplyResult::Applied,
            "expected Applied, got {result:?}"
        );
        assert!(
            has_pair(&out, "-m", "o3"),
            "codex must splice -m o3; got {out:?}"
        );
        assert!(
            has_pair(&out, "-c", "model_reasoning_effort=high"),
            "effort -c must be preserved; got {out:?}"
        );
        let pins = existing_model_flag_indices(&out);
        assert_eq!(
            pins.len(),
            1,
            "effort -c must not count as a model pin; pins={pins:?} args={out:?}"
        );
    }

    /// 7. Codex `-c model=foo` counts as pin for WorkspaceDefault.
    #[test]
    fn codex_config_model_is_a_pin() {
        let args = s(&["-c", "model=foo"]);
        let (out, result) =
            apply_model("codex", &args, Some("o3"), ModelApplyMode::WorkspaceDefault);
        assert_eq!(
            result,
            ModelApplyResult::AlreadyPinnedSkipped,
            "expected AlreadyPinnedSkipped, got {result:?}"
        );
        assert_eq!(out, args);
        let (out, result) = apply_model("codex", &args, Some("o3"), ModelApplyMode::ApiOverride);
        assert_eq!(result, ModelApplyResult::Applied);
        assert!(
            has_pair(&out, "-c", "model=o3"),
            "API override must replace -c model=; got {out:?}"
        );
        assert!(
            !has_pair(&out, "-c", "model=foo"),
            "old -c model=foo must be gone; got {out:?}"
        );
    }

    /// 8. Resume + workspace only + force=false → no splice.
    #[test]
    fn resume_workspace_only_no_force_does_not_splice() {
        let args = s(&["--resume", "sid-1"]);
        let d = decide_and_apply("claude", &args, None, Some("opus"), true, false);
        assert_eq!(
            d.result,
            ModelApplyResult::None,
            "expected None, got {:?}",
            d.result
        );
        assert_eq!(d.source, ModelSource::None);
        assert_eq!(
            d.args, args,
            "must not splice on resume without checkbox; got {:?}",
            d.args
        );
    }

    /// 9. Resume + workspace + force=true → splice.
    #[test]
    fn resume_workspace_with_force_splices() {
        let args = s(&["--resume", "sid-1"]);
        let d = decide_and_apply("claude", &args, None, Some("opus"), true, true);
        assert_eq!(
            d.result,
            ModelApplyResult::Applied,
            "expected Applied, got {:?}",
            d.result
        );
        assert_eq!(d.source, ModelSource::Workspace);
        assert_eq!(d.applied.as_deref(), Some("opus"));
        assert!(
            has_pair(&d.args, "--model", "opus"),
            "force-on-resume must splice; got {:?}",
            d.args
        );
        assert!(has_pair(&d.args, "--resume", "sid-1"));
    }

    /// 10. Resume + API model even if force=false → splice API.
    #[test]
    fn resume_api_model_splices_even_without_force() {
        let args = s(&["--resume", "sid-1"]);
        let d = decide_and_apply("claude", &args, Some("sonnet"), Some("opus"), true, false);
        assert_eq!(
            d.result,
            ModelApplyResult::Applied,
            "expected Applied, got {:?}",
            d.result
        );
        assert_eq!(d.source, ModelSource::Api);
        assert_eq!(d.applied.as_deref(), Some("sonnet"));
        assert!(
            has_pair(&d.args, "--model", "sonnet"),
            "API model on resume must splice; got {:?}",
            d.args
        );
        assert!(
            !has_pair(&d.args, "--model", "opus"),
            "API must win over workspace on resume; got {:?}",
            d.args
        );
    }

    /// 11. grok `-m`, gemini `-m`, pi `--model`, cursor-agent `--model`.
    #[test]
    fn family_flags_for_grok_gemini_pi_cursor() {
        let empty: Vec<String> = Vec::new();
        let (grok, r) = apply_model(
            "grok",
            &empty,
            Some("grok-4"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(r, ModelApplyResult::Applied);
        assert!(
            has_pair(&grok, "-m", "grok-4"),
            "grok must use -m; got {grok:?}"
        );

        let (gemini, r) = apply_model(
            "gemini",
            &empty,
            Some("gemini-2.5-pro"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(r, ModelApplyResult::Applied);
        assert!(
            has_pair(&gemini, "-m", "gemini-2.5-pro"),
            "gemini must use -m; got {gemini:?}"
        );

        let (pi, r) = apply_model(
            "pi",
            &empty,
            Some("pi-model"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(r, ModelApplyResult::Applied);
        assert!(
            has_pair(&pi, "--model", "pi-model"),
            "pi must use --model; got {pi:?}"
        );

        let (ca, r) = apply_model(
            "/opt/cursor-agent",
            &empty,
            Some("composer-1"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(r, ModelApplyResult::Applied);
        assert!(
            has_pair(&ca, "--model", "composer-1"),
            "cursor-agent must use --model; got {ca:?}"
        );

        let (agent, r) = apply_model(
            "agent.exe",
            &empty,
            Some("composer-1"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(r, ModelApplyResult::Applied);
        assert!(
            has_pair(&agent, "--model", "composer-1"),
            "agent must use --model; got {agent:?}"
        );
    }

    /// 12. hermes with `chat` in args → `--model`; bare hermes → IgnoredUnsupported.
    #[test]
    fn hermes_chat_applies_bare_ignored() {
        let with_chat = s(&["chat"]);
        let (out, result) = apply_model(
            "hermes",
            &with_chat,
            Some("hermes-3"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(
            result,
            ModelApplyResult::Applied,
            "expected Applied, got {result:?}"
        );
        assert_eq!(
            out,
            s(&["chat", "--model", "hermes-3"]),
            "--model must be inserted after chat; got {out:?}"
        );

        let bare: Vec<String> = Vec::new();
        let (out, result) = apply_model(
            "hermes",
            &bare,
            Some("hermes-3"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(
            result,
            ModelApplyResult::IgnoredUnsupported,
            "bare hermes must be IgnoredUnsupported, got {result:?}"
        );
        assert_eq!(out, bare, "bare hermes must not invent chat; got {out:?}");
    }

    /// 13. `--model=haiku` equals-form detected as pin.
    #[test]
    fn equals_form_is_a_pin() {
        let args = s(&["--model=haiku"]);
        let pins = existing_model_flag_indices(&args);
        assert_eq!(pins.len(), 1, "equals-form must be a pin; pins={pins:?}");
        assert_eq!(pins[0].form, ModelFlagForm::LongEquals);
        let (out, result) = apply_model(
            "claude",
            &args,
            Some("opus"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(result, ModelApplyResult::AlreadyPinnedSkipped);
        assert_eq!(out, args);
        let (out, result) =
            apply_model("claude", &args, Some("sonnet"), ModelApplyMode::ApiOverride);
        assert_eq!(result, ModelApplyResult::Applied);
        assert!(
            has_eq(&out, "--model=sonnet"),
            "API replace must keep equals-form; got {out:?}"
        );
        assert!(
            !has_eq(&out, "--model=haiku"),
            "old equals pin must be gone; got {out:?}"
        );
    }

    /// 14. Strip duplicate `--model` after API replace.
    #[test]
    fn api_replace_strips_duplicate_model_flags() {
        let args = s(&["--model", "haiku", "--verbose", "--model", "opus"]);
        let (out, result) =
            apply_model("claude", &args, Some("sonnet"), ModelApplyMode::ApiOverride);
        assert_eq!(result, ModelApplyResult::Applied);
        let model_flags = out
            .iter()
            .filter(|a| *a == "--model" || a.starts_with("--model="))
            .count();
        assert_eq!(model_flags, 1, "duplicates must be stripped; got {out:?}");
        assert!(has_pair(&out, "--model", "sonnet"), "got {out:?}");
        assert!(
            !out.iter().any(|a| a == "haiku" || a == "opus"),
            "old values must be gone; got {out:?}"
        );
        assert!(
            out.iter().any(|a| a == "--verbose"),
            "unrelated flags must survive; got {out:?}"
        );
    }

    /// 15. Whitespace-only model treated as omit.
    #[test]
    fn whitespace_only_model_is_omit() {
        let args = s(&["--dangerously-skip-permissions"]);
        let (out, result) =
            apply_model("claude", &args, Some("   \t"), ModelApplyMode::ApiOverride);
        assert_eq!(result, ModelApplyResult::None);
        assert_eq!(out, args);
        let d = decide_and_apply("claude", &args, Some("  "), Some(" \n"), false, false);
        assert_eq!(d.result, ModelApplyResult::None);
        assert_eq!(d.args, args);
        // Whitespace API falls through to workspace.
        let d = decide_and_apply("claude", &args, Some("  "), Some("opus"), false, false);
        assert_eq!(d.result, ModelApplyResult::Applied);
        assert_eq!(d.source, ModelSource::Workspace);
        assert!(has_pair(&d.args, "--model", "opus"));
    }

    /// 16. Empty args + claude + opus inserts `--model opus` (append).
    #[test]
    fn empty_args_claude_appends_model() {
        let args: Vec<String> = Vec::new();
        let (out, result) = apply_model(
            "claude",
            &args,
            Some("opus"),
            ModelApplyMode::WorkspaceDefault,
        );
        assert_eq!(result, ModelApplyResult::Applied);
        assert_eq!(
            out,
            s(&["--model", "opus"]),
            "append is the insert position; got {out:?}"
        );
    }

    #[test]
    fn binary_token_strips_path_and_exe() {
        assert_eq!(binary_token("/usr/bin/claude"), "claude");
        assert_eq!(binary_token(r"C:\Tools\claude.exe"), "claude");
        assert_eq!(
            binary_token("claude --dangerously-skip-permissions"),
            "claude"
        );
        assert_eq!(binary_token("  /opt/bin/codex.exe  "), "codex");
    }

    #[test]
    fn grok_replace_accepts_long_model_flag() {
        let args = s(&["--model", "grok-3"]);
        let (out, result) = apply_model("grok", &args, Some("grok-4"), ModelApplyMode::ApiOverride);
        assert_eq!(result, ModelApplyResult::Applied);
        assert!(
            has_pair(&out, "--model", "grok-4"),
            "replace keeps existing form; got {out:?}"
        );
        assert!(!has_pair(&out, "--model", "grok-3"));
    }

    #[test]
    fn dead_resume_detection() {
        assert!(args_look_like_dead_resume("claude", &s(&["--resume", "x"])));
        assert!(args_look_like_dead_resume("claude", &s(&["--resume=x"])));
        assert!(args_look_like_dead_resume("codex", &s(&["resume", "x"])));
        assert!(args_look_like_dead_resume("pi", &s(&["--session", "x"])));
        assert!(
            !args_look_like_dead_resume("claude", &s(&["--session-id", "x"])),
            "premint --session-id is not resume"
        );
        assert!(!args_look_like_dead_resume(
            "claude",
            &s(&["--dangerously-skip-permissions"])
        ));
    }

    #[test]
    fn api_wins_over_workspace_on_fresh() {
        let args = s(&[]);
        let d = decide_and_apply("claude", &args, Some("sonnet"), Some("opus"), false, false);
        assert_eq!(d.source, ModelSource::Api);
        assert_eq!(d.applied.as_deref(), Some("sonnet"));
        assert!(has_pair(&d.args, "--model", "sonnet"));
    }

    #[test]
    fn load_workspace_model_roundtrip() {
        crate::db::init_for_tests();
        let id = uuid::Uuid::new_v4().to_string();
        let path = format!("/tmp/k2-model-splice-{id}");
        {
            let db = crate::db::shared();
            let conn = db.lock();
            conn.execute(
                "INSERT INTO projects (id, name, path) VALUES (?1, ?2, ?3)",
                rusqlite::params![&id, "ms", &path],
            )
            .expect("insert project");
            let (m, force) = load_workspace_model(&conn, &path);
            assert_eq!(m, None, "unset default_model must be None");
            assert!(!force, "force_model_on_resume defaults to 0");
            conn.execute(
                "UPDATE projects SET default_model = ?1, force_model_on_resume = 1 WHERE id = ?2",
                rusqlite::params!["opus", &id],
            )
            .expect("set model");
            let (m, force) = load_workspace_model(&conn, &id);
            assert_eq!(m.as_deref(), Some("opus"), "load by id must work");
            assert!(force);
            conn.execute(
                "UPDATE projects SET default_model = '   ' WHERE id = ?1",
                rusqlite::params![&id],
            )
            .expect("whitespace model");
            let (m, _) = load_workspace_model(&conn, &path);
            assert_eq!(m, None, "whitespace default_model must load as None");
        }
    }
}
