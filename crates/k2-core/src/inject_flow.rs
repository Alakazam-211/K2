//! Per-LLM preset inject keystroke flow (prd-llm-inject-keystroke-flow-v1).
//!
//! Write-side JSON lives on `agent_presets.inject_flow` (TEXT, NULLABLE,
//! no DEFAULT). NULL = [`DEFAULT_INJECT_FLOW`] — today's hardcoded
//! paste/150/CR/250/CR/120 sequence. The daemon interpreter compiles a
//! validated flow plus already-framed paste bytes into
//! [`InjectAction`]s; it does not own PTY types.

use serde::Deserialize;

/// Allowed step kinds. `paste` writes this inject's payload (never Cmd+V).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectKey {
    Paste,
    Esc,
    Space,
    Return,
}

impl InjectKey {
    pub fn as_str(self) -> &'static str {
        match self {
            InjectKey::Paste => "paste",
            InjectKey::Esc => "esc",
            InjectKey::Space => "space",
            InjectKey::Return => "return",
        }
    }

    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "paste" => Ok(InjectKey::Paste),
            "esc" => Ok(InjectKey::Esc),
            "space" => Ok(InjectKey::Space),
            "return" => Ok(InjectKey::Return),
            other => Err(format!(
                "unknown inject key {other:?}; expected paste|esc|space|return"
            )),
        }
    }
}

/// One saved keystroke step: write `key`, then wait `wait_ms` milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InjectStep {
    pub key: InjectKey,
    pub wait_ms: u64,
}

/// Compiled interpreter actions. One step becomes Write → Sleep → CheckAlive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InjectAction {
    Write(Vec<u8>),
    Sleep(u64),
    CheckAlive,
}

/// Default when the column is NULL or a stored flow is unreadable.
/// Matches today's hardcoded inject: paste, CR, CR at 150/250/120 ms.
pub const DEFAULT_INJECT_FLOW: [InjectStep; 3] = [
    InjectStep {
        key: InjectKey::Paste,
        wait_ms: 150,
    },
    InjectStep {
        key: InjectKey::Return,
        wait_ms: 250,
    },
    InjectStep {
        key: InjectKey::Return,
        wait_ms: 120,
    },
];

/// Grok default: paste then one Return. The second CR was insurance
/// before steer; an extra Return now starts a new turn.
pub const GROK_INJECT_FLOW: [InjectStep; 2] = [
    InjectStep {
        key: InjectKey::Paste,
        wait_ms: 150,
    },
    InjectStep {
        key: InjectKey::Return,
        wait_ms: 250,
    },
];

/// JSON twin of [`GROK_INJECT_FLOW`] for the built-in Grok seed/backfill.
pub const GROK_INJECT_FLOW_JSON: &str =
    r#"[{"key":"paste","waitMs":150},{"key":"return","waitMs":250}]"#;

pub const MAX_STEPS: usize = 16;
pub const MAX_WAIT_MS: u64 = 10_000;
pub const MAX_WAIT_SUM_MS: u64 = 5_000;

pub fn default_inject_flow() -> Vec<InjectStep> {
    DEFAULT_INJECT_FLOW.to_vec()
}

pub fn grok_inject_flow() -> Vec<InjectStep> {
    GROK_INJECT_FLOW.to_vec()
}

/// Basename of a live PTY or preset first token is Grok.
pub fn program_is_grok(live_program: Option<&str>) -> bool {
    match command_basename(live_program.unwrap_or("")) {
        Some(base) => {
            let b = base.to_ascii_lowercase();
            b == "grok" || b == "grok.exe" || b == "grok.cmd"
        }
        None => false,
    }
}

/// NULL / no-match default: Grok is paste+one Return; everyone else D5.
pub fn default_inject_flow_for_program(live_program: Option<&str>) -> Vec<InjectStep> {
    if program_is_grok(live_program) {
        grok_inject_flow()
    } else {
        default_inject_flow()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InjectStepDto {
    key: String,
    #[serde(rename = "waitMs")]
    wait_ms: u64,
}

/// Strict write-side validator. Unknown key / non-integer wait / extra
/// properties / missing waitMs / 0 or 2+ paste / out of range → `Err`
/// (HTTP 400, do not persist).
pub fn validate_inject_flow_json(raw: &str) -> Result<Vec<InjectStep>, String> {
    let dtos: Vec<InjectStepDto> = serde_json::from_str(raw)
        .map_err(|e| format!("inject_flow must be a JSON array of {{key, waitMs}} objects: {e}"))?;
    if !(1..=MAX_STEPS).contains(&dtos.len()) {
        return Err(format!(
            "inject_flow must have 1..=16 steps, got {}",
            dtos.len()
        ));
    }
    let mut steps = Vec::with_capacity(dtos.len());
    let mut paste_count = 0usize;
    let mut wait_sum: u64 = 0;
    for dto in dtos {
        if dto.wait_ms > MAX_WAIT_MS {
            return Err(format!("waitMs must be 0..=10000, got {}", dto.wait_ms));
        }
        wait_sum = wait_sum.saturating_add(dto.wait_ms);
        let key = InjectKey::parse(&dto.key)?;
        if key == InjectKey::Paste {
            paste_count += 1;
        }
        steps.push(InjectStep {
            key,
            wait_ms: dto.wait_ms,
        });
    }
    if wait_sum > MAX_WAIT_SUM_MS {
        return Err(format!("sum(waitMs) must be ≤ 5000, got {wait_sum}"));
    }
    if paste_count != 1 {
        return Err(format!(
            "inject_flow must contain exactly one paste step, got {paste_count}"
        ));
    }
    Ok(steps)
}

/// Read/inject path: NULL or malformed/out-of-range → log + default.
/// Never panics.
pub fn parse_inject_flow_or_default(raw: Option<&str>) -> Vec<InjectStep> {
    match raw {
        None => default_inject_flow(),
        Some(s) => match validate_inject_flow_json(s) {
            Ok(steps) => steps,
            Err(e) => {
                crate::log_debug!(
                    "[inject_flow] malformed or out of range ({e}) — using default paste/CR/CR"
                );
                default_inject_flow()
            }
        },
    }
}

/// Compile a validated flow plus already-framed paste bytes into the
/// write/sleep/alive sequence. Sanitize/frame the payload at the caller
/// *before* this; the paste step writes `paste_bytes` as-is.
pub fn compile_inject_flow(flow: &[InjectStep], paste_bytes: &[u8]) -> Vec<InjectAction> {
    let mut out = Vec::with_capacity(flow.len() * 3);
    for step in flow {
        let bytes = match step.key {
            InjectKey::Paste => paste_bytes.to_vec(),
            InjectKey::Esc => vec![0x1b],
            InjectKey::Space => vec![0x20],
            InjectKey::Return => vec![0x0d],
        };
        out.push(InjectAction::Write(bytes));
        out.push(InjectAction::Sleep(step.wait_ms));
        out.push(InjectAction::CheckAlive);
    }
    out
}

/// One preset row for D9 basename matching. Callers pass every row
/// (including disabled); this sorts by `sort_order`, then `label`.
#[derive(Debug, Clone)]
pub struct InjectFlowCandidate<'a> {
    pub command: &'a str,
    pub inject_flow: Option<&'a str>,
    pub sort_order: i64,
    pub label: &'a str,
}

/// Basename of a live PTY program (`LiveSession::command()` is program
/// only, not argv) or of a preset command's first token.
pub fn command_basename(program_or_token: &str) -> Option<&str> {
    let trimmed = program_or_token.trim();
    if trimmed.is_empty() {
        return None;
    }
    std::path::Path::new(trimmed)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|s| !s.is_empty())
}

fn preset_first_token_basename(command: &str) -> Option<&str> {
    command_basename(command.split_whitespace().next().unwrap_or(""))
}

/// First preset whose command first-token basename matches the live
/// program basename, `ORDER BY sort_order, label`. `None` = no match.
/// The inner option is the raw `inject_flow` column (NULL included).
pub fn match_preset_inject_flow<'a>(
    live_program: Option<&str>,
    presets: &[InjectFlowCandidate<'a>],
) -> Option<Option<&'a str>> {
    let live_base = command_basename(live_program.unwrap_or(""))?;
    let mut indexed: Vec<usize> = (0..presets.len()).collect();
    indexed.sort_by(|&a, &b| {
        let pa = &presets[a];
        let pb = &presets[b];
        pa.sort_order
            .cmp(&pb.sort_order)
            .then(pa.label.cmp(pb.label))
    });
    for i in indexed {
        let p = &presets[i];
        if preset_first_token_basename(p.command) == Some(live_base) {
            return Some(p.inject_flow);
        }
    }
    None
}

/// Resolve the flow the interpreter should play for `live_program`.
/// No match / NULL column / malformed JSON → [`default_inject_flow_for_program`].
pub fn resolve_inject_flow_for_program(
    live_program: Option<&str>,
    presets: &[InjectFlowCandidate<'_>],
) -> Vec<InjectStep> {
    match match_preset_inject_flow(live_program, presets) {
        None => default_inject_flow_for_program(live_program),
        Some(None) => default_inject_flow_for_program(live_program),
        Some(Some(raw)) => match validate_inject_flow_json(raw) {
            Ok(steps) => steps,
            Err(e) => {
                crate::log_debug!(
                    "[inject_flow] malformed or out of range ({e}) — using program default"
                );
                default_inject_flow_for_program(live_program)
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEFAULT_JSON: &str = r#"[{"key":"paste","waitMs":150},{"key":"return","waitMs":250},{"key":"return","waitMs":120}]"#;
    const GROK_JSON: &str = r#"[{"key":"esc","waitMs":0},{"key":"space","waitMs":50},{"key":"paste","waitMs":150},{"key":"return","waitMs":250},{"key":"return","waitMs":120}]"#;

    fn write_bytes<'a>(actions: &'a [InjectAction], i: usize) -> &'a [u8] {
        match &actions[i] {
            InjectAction::Write(b) => b,
            other => panic!("expected Write at {i}, got {other:?}"),
        }
    }

    fn sleep_ms(actions: &[InjectAction], i: usize) -> u64 {
        match &actions[i] {
            InjectAction::Sleep(ms) => *ms,
            other => panic!("expected Sleep at {i}, got {other:?}"),
        }
    }

    fn assert_check(actions: &[InjectAction], i: usize) {
        match &actions[i] {
            InjectAction::CheckAlive => {}
            other => panic!("expected CheckAlive at {i}, got {other:?}"),
        }
    }

    #[test]
    fn default_flow_is_paste_then_two_crs() {
        let flow = default_inject_flow();
        assert_eq!(flow.len(), 3);
        assert_eq!(flow[0].key, InjectKey::Paste);
        assert_eq!(flow[0].wait_ms, 150);
        assert_eq!(flow[1].key, InjectKey::Return);
        assert_eq!(flow[1].wait_ms, 250);
        assert_eq!(flow[2].key, InjectKey::Return);
        assert_eq!(flow[2].wait_ms, 120);
    }

    #[test]
    fn compiler_default_writes_paste_body_then_two_crs() {
        let actions = compile_inject_flow(&default_inject_flow(), b"hello");
        assert_eq!(actions.len(), 9);
        assert_eq!(write_bytes(&actions, 0), b"hello");
        assert_eq!(sleep_ms(&actions, 1), 150);
        assert_check(&actions, 2);
        assert_eq!(write_bytes(&actions, 3), b"\r");
        assert_eq!(sleep_ms(&actions, 4), 250);
        assert_check(&actions, 5);
        assert_eq!(write_bytes(&actions, 6), b"\r");
        assert_eq!(sleep_ms(&actions, 7), 120);
        assert_check(&actions, 8);
        assert_eq!(write_bytes(&actions, 3), &[0x0d]);
        assert_eq!(write_bytes(&actions, 6), &[0x0d]);
    }

    #[test]
    fn compiler_grok_experiment_is_esc_space_paste_then_two_crs() {
        let flow = validate_inject_flow_json(GROK_JSON).expect("grok experiment is valid");
        let actions = compile_inject_flow(&flow, b"payload");
        assert_eq!(actions.len(), 15);
        assert_eq!(write_bytes(&actions, 0), &[0x1b]);
        assert_eq!(sleep_ms(&actions, 1), 0);
        assert_check(&actions, 2);
        assert_eq!(write_bytes(&actions, 3), &[0x20]);
        assert_eq!(sleep_ms(&actions, 4), 50);
        assert_check(&actions, 5);
        assert_eq!(write_bytes(&actions, 6), b"payload");
        assert_eq!(sleep_ms(&actions, 7), 150);
        assert_check(&actions, 8);
        assert_eq!(write_bytes(&actions, 9), &[0x0d]);
        assert_eq!(sleep_ms(&actions, 10), 250);
        assert_check(&actions, 11);
        assert_eq!(write_bytes(&actions, 12), &[0x0d]);
        assert_eq!(sleep_ms(&actions, 13), 120);
        assert_check(&actions, 14);
    }

    #[test]
    fn compiler_does_not_append_a_trailing_sleep_after_the_loop() {
        let actions = compile_inject_flow(&default_inject_flow(), b"x");
        match actions.last() {
            Some(InjectAction::CheckAlive) => {}
            other => panic!("last action must be the last step's CheckAlive, got {other:?}"),
        }
    }

    #[test]
    fn validator_accepts_default_and_grok_json() {
        let d = validate_inject_flow_json(DEFAULT_JSON).expect("default json");
        assert_eq!(d, default_inject_flow());
        let g = validate_inject_flow_json(GROK_JSON).expect("grok json");
        assert_eq!(g.len(), 5);
        assert_eq!(g[0].key, InjectKey::Esc);
        assert_eq!(g[1].key, InjectKey::Space);
        assert_eq!(g[2].key, InjectKey::Paste);
    }

    #[test]
    fn validator_400s_on_zero_or_two_paste() {
        let zero = r#"[{"key":"return","waitMs":10}]"#;
        let err = validate_inject_flow_json(zero).expect_err("zero paste");
        assert!(err.contains("exactly one paste"), "{err}");
        let two = r#"[{"key":"paste","waitMs":10},{"key":"paste","waitMs":10}]"#;
        let err = validate_inject_flow_json(two).expect_err("two paste");
        assert!(err.contains("exactly one paste"), "{err}");
    }

    #[test]
    fn validator_400s_on_extra_property() {
        let raw = r#"[{"key":"paste","waitMs":10,"extra":true}]"#;
        let err = validate_inject_flow_json(raw).expect_err("extra property");
        assert!(
            err.contains("inject_flow must be a JSON array"),
            "extra keys must 400: {err}"
        );
    }

    #[test]
    fn validator_400s_on_unknown_key_missing_wait_and_range() {
        let unknown = r#"[{"key":"cmdv","waitMs":10}]"#;
        let err = validate_inject_flow_json(unknown).expect_err("unknown key");
        assert!(err.contains("unknown inject key"), "{err}");

        let missing = r#"[{"key":"paste"}]"#;
        let err = validate_inject_flow_json(missing).expect_err("missing waitMs");
        assert!(err.contains("inject_flow must be a JSON array"), "{err}");

        let float = r#"[{"key":"paste","waitMs":1.5}]"#;
        let err = validate_inject_flow_json(float).expect_err("float waitMs");
        assert!(err.contains("inject_flow must be a JSON array"), "{err}");

        let high = r#"[{"key":"paste","waitMs":10001}]"#;
        let err = validate_inject_flow_json(high).expect_err("wait ceiling");
        assert!(err.contains("waitMs"), "{err}");

        let empty = r#"[]"#;
        let err = validate_inject_flow_json(empty).expect_err("empty");
        assert!(err.contains("1..=16"), "{err}");

        let too_many = format!(
            "[{}]",
            (0..17)
                .map(|i| {
                    if i == 0 {
                        r#"{"key":"paste","waitMs":0}"#.to_string()
                    } else {
                        r#"{"key":"return","waitMs":0}"#.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(",")
        );
        let err = validate_inject_flow_json(&too_many).expect_err("17 steps");
        assert!(err.contains("1..=16"), "{err}");
    }

    #[test]
    fn validator_400s_when_wait_sum_exceeds_5000() {
        let raw = r#"[{"key":"paste","waitMs":5000},{"key":"return","waitMs":1}]"#;
        let err = validate_inject_flow_json(raw).expect_err("sum cap");
        assert!(err.contains("sum(waitMs)"), "{err}");
    }

    #[test]
    fn parse_or_default_uses_d5_on_null_and_malformed() {
        assert_eq!(parse_inject_flow_or_default(None), default_inject_flow());
        assert_eq!(
            parse_inject_flow_or_default(Some("not-json")),
            default_inject_flow()
        );
        assert_eq!(
            parse_inject_flow_or_default(Some(r#"[{"key":"paste","waitMs":10,"extra":1}]"#)),
            default_inject_flow()
        );
        let ok = parse_inject_flow_or_default(Some(GROK_JSON));
        assert_eq!(ok[0].key, InjectKey::Esc);
    }

    #[test]
    fn d9_usr_bin_claude_matches_preset_first_token() {
        let grok = r#"[{"key":"esc","waitMs":0},{"key":"paste","waitMs":10},{"key":"return","waitMs":10}]"#;
        let presets = [
            InjectFlowCandidate {
                command: "claude --dangerously-skip-permissions",
                inject_flow: Some(grok),
                sort_order: 0,
                label: "Claude",
            },
            InjectFlowCandidate {
                command: "grok --always-approve",
                inject_flow: Some(DEFAULT_JSON),
                sort_order: 1,
                label: "Grok",
            },
        ];
        let matched = match_preset_inject_flow(Some("/usr/bin/claude"), &presets);
        assert_eq!(matched, Some(Some(grok)));
        let grok_hit = match_preset_inject_flow(Some("grok"), &presets);
        assert_eq!(grok_hit, Some(Some(DEFAULT_JSON)));
        assert_eq!(
            match_preset_inject_flow(Some("claude"), &presets)
                .expect("claude matches")
                .expect("flow present"),
            grok
        );
    }

    #[test]
    fn d9_claude_and_grok_are_distinct() {
        let claude_flow = r#"[{"key":"paste","waitMs":1},{"key":"return","waitMs":1}]"#;
        let grok_flow = GROK_JSON;
        let presets = [
            InjectFlowCandidate {
                command: "claude",
                inject_flow: Some(claude_flow),
                sort_order: 0,
                label: "Claude",
            },
            InjectFlowCandidate {
                command: "grok",
                inject_flow: Some(grok_flow),
                sort_order: 1,
                label: "Grok",
            },
        ];
        assert_eq!(
            match_preset_inject_flow(Some("claude"), &presets),
            Some(Some(claude_flow))
        );
        assert_eq!(
            match_preset_inject_flow(Some("/opt/homebrew/bin/grok"), &presets),
            Some(Some(grok_flow))
        );
        assert_eq!(match_preset_inject_flow(Some("cat"), &presets), None);
        assert_eq!(
            resolve_inject_flow_for_program(Some("cat"), &presets),
            default_inject_flow()
        );
    }

    #[test]
    fn d9_two_claude_rows_share_first_by_sort_order_then_label() {
        let first = r#"[{"key":"paste","waitMs":11},{"key":"return","waitMs":11}]"#;
        let second = r#"[{"key":"paste","waitMs":22},{"key":"return","waitMs":22}]"#;
        let presets = [
            InjectFlowCandidate {
                command: "claude --yolo",
                inject_flow: Some(second),
                sort_order: 1,
                label: "Z Claude",
            },
            InjectFlowCandidate {
                command: "claude --dangerously-skip-permissions",
                inject_flow: Some(first),
                sort_order: 1,
                label: "A Claude",
            },
            InjectFlowCandidate {
                command: "/usr/local/bin/claude",
                inject_flow: Some(second),
                sort_order: 2,
                label: "Another",
            },
        ];
        let matched = match_preset_inject_flow(Some("/usr/bin/claude"), &presets);
        assert_eq!(
            matched,
            Some(Some(first)),
            "same sort_order: A Claude sorts before Z Claude"
        );
    }

    #[test]
    fn d9_disabled_rows_still_match_and_null_column_is_d5() {
        let presets = [InjectFlowCandidate {
            command: "claude",
            inject_flow: None,
            sort_order: 0,
            label: "Claude",
        }];
        assert_eq!(
            match_preset_inject_flow(Some("claude"), &presets),
            Some(None)
        );
        assert_eq!(
            resolve_inject_flow_for_program(Some("claude"), &presets),
            default_inject_flow()
        );
    }

    #[test]
    fn grok_default_is_paste_then_one_cr() {
        let flow = grok_inject_flow();
        assert_eq!(flow.len(), 2);
        assert_eq!(flow[0].key, InjectKey::Paste);
        assert_eq!(flow[0].wait_ms, 150);
        assert_eq!(flow[1].key, InjectKey::Return);
        assert_eq!(flow[1].wait_ms, 250);
        let parsed = validate_inject_flow_json(GROK_INJECT_FLOW_JSON).expect("grok json");
        assert_eq!(parsed, flow);
        assert!(program_is_grok(Some("grok")));
        assert!(program_is_grok(Some("/opt/homebrew/bin/grok")));
        assert!(program_is_grok(Some("grok.exe")));
        assert!(!program_is_grok(Some("claude")));
    }

    #[test]
    fn grok_null_column_and_unmatched_grok_use_one_cr() {
        let presets = [InjectFlowCandidate {
            command: "grok --always-approve",
            inject_flow: None,
            sort_order: 0,
            label: "Grok",
        }];
        assert_eq!(
            resolve_inject_flow_for_program(Some("grok"), &presets),
            grok_inject_flow()
        );
        assert_eq!(
            resolve_inject_flow_for_program(Some("/usr/bin/grok"), &[]),
            grok_inject_flow()
        );
        assert_eq!(
            resolve_inject_flow_for_program(Some("claude"), &[]),
            default_inject_flow()
        );
    }

    #[test]
    fn d9_malformed_stored_json_falls_back_to_d5() {
        let presets = [InjectFlowCandidate {
            command: "claude",
            inject_flow: Some(r#"[{"key":"paste","waitMs":10,"nope":1}]"#),
            sort_order: 0,
            label: "Claude",
        }];
        assert_eq!(
            resolve_inject_flow_for_program(Some("/usr/bin/claude"), &presets),
            default_inject_flow()
        );
    }
}
