//! Harness-agnostic turns for the session-log chat face.
//!
//! Claude, Codex, Grok, and Gemini each have an adapter. The wire shape
//! is role, blocks, time, and a stable id. This is not continue-seed:
//! that path keeps two strings, drops tool calls, and caps length.
//! A line an adapter does not understand is skipped.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatTurn {
    pub id: String,
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    pub blocks: Vec<ChatBlock>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChatBlock {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        input: String,
    },
    ToolResult {
        id: String,
        content: String,
    },
    /// A thinking / reasoning record (TW10). `text` is the thinking or
    /// its summary when the harness wrote one; `redacted` when it wrote
    /// only a signature or encrypted content (never read). `duration` is
    /// milliseconds since the record before it, when both carry a time.
    /// The Chat view shows a collapsed "Thought" stub. Thread never
    /// carries this.
    Thinking {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        redacted: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        duration: Option<u64>,
    },
}

#[derive(Debug, Default)]
pub struct TranscriptCursor {
    line_no: u64,
    turns: Vec<ChatTurn>,
    by_id: HashMap<String, usize>,
    /// Codex event_msg text already shown via response_item (or earlier).
    codex_text: HashSet<(String, String)>,
    codex_calls: HashSet<String>,
    /// The previous timestamped record (unix ms), for thinking durations.
    last_time_ms: Option<i64>,
}

impl TranscriptCursor {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn turns(&self) -> &[ChatTurn] {
        &self.turns
    }

    /// Parse one JSONL line. Each item is a new or updated turn.
    /// A Gemini `$set` snapshot can carry more than one.
    pub fn push_line(&mut self, provider: &str, line: &str) -> Vec<ChatTurn> {
        self.line_no += 1;
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let n = self.line_no;
        match provider.trim() {
            "claude" => self.push_claude(n, line).into_iter().collect(),
            "codex" => self.push_codex(n, line).into_iter().collect(),
            "grok" => self.push_grok(n, line).into_iter().collect(),
            "gemini" => self.push_gemini(n, line),
            _ => Vec::new(),
        }
    }
}

/// Whole-file parse. Same ids collapse into one turn.
pub fn parse_chat_transcript(provider: &str, text: &str) -> Vec<ChatTurn> {
    let mut cursor = TranscriptCursor::default();
    for line in text.lines() {
        cursor.push_line(provider, line);
    }
    cursor.turns
}

impl TranscriptCursor {
    fn push_claude(&mut self, line_no: u64, line: &str) -> Option<ChatTurn> {
        let Ok(parsed) = serde_json::from_str::<Value>(line) else {
            return None;
        };
        let since_last = self.stamp(&parsed);
        if flag_true(&parsed, "isMeta") || flag_true(&parsed, "isCompactSummary") {
            return None;
        }
        let kind = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind != "user" && kind != "assistant" {
            return None;
        }
        let message = parsed.get("message").cloned().unwrap_or(Value::Null);
        if flag_true(&message, "isMeta") || flag_true(&message, "isCompactSummary") {
            return None;
        }
        let content = message
            .pointer("/content")
            .or_else(|| parsed.get("content"));
        let mut blocks = claude_blocks(content);
        if blocks.is_empty() {
            return None;
        }
        set_thinking_duration(&mut blocks, since_last);
        let id = if kind == "assistant" {
            message
                .get("id")
                .and_then(|v| v.as_str())
                .or_else(|| parsed.get("uuid").and_then(|v| v.as_str()))
                .map(str::to_string)
        } else {
            parsed
                .get("uuid")
                .and_then(|v| v.as_str())
                .or_else(|| message.get("id").and_then(|v| v.as_str()))
                .map(str::to_string)
        }
        .unwrap_or_else(|| format!("claude:{line_no}"));
        let time = parsed
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        self.upsert(ChatTurn {
            id,
            role: kind.to_string(),
            time,
            blocks,
        })
    }

    fn push_codex(&mut self, line_no: u64, line: &str) -> Option<ChatTurn> {
        let Ok(parsed) = serde_json::from_str::<Value>(line) else {
            return None;
        };
        let since_last = self.stamp(&parsed);
        let kind = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let time = parsed
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        match kind {
            "response_item" => {
                let payload = parsed.get("payload")?;
                if payload.get("type").and_then(|v| v.as_str()) == Some("reasoning") {
                    // TW10: the summary when present; `encrypted_content`
                    // is never read.
                    let id = payload
                        .get("id")
                        .and_then(|v| v.as_str())
                        .map(str::to_string)
                        .unwrap_or_else(|| format!("codex-reasoning:{line_no}"));
                    return self.upsert(ChatTurn {
                        id,
                        role: "assistant".into(),
                        time,
                        blocks: vec![thinking_block(summary_text(payload.get("summary")), since_last)],
                    });
                }
                self.codex_response_item(line_no, payload, time)
            }
            "event_msg" => {
                let payload = parsed.get("payload")?;
                self.codex_event(line_no, payload, time)
            }
            _ => None,
        }
    }

    fn codex_response_item(
        &mut self,
        line_no: u64,
        payload: &Value,
        time: Option<String>,
    ) -> Option<ChatTurn> {
        let ptype = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match ptype {
            "message" => {
                let role = payload.get("role").and_then(|v| v.as_str()).unwrap_or("");
                if role != "user" && role != "assistant" {
                    return None;
                }
                let blocks = text_blocks(payload.get("content"));
                if blocks.is_empty() {
                    return None;
                }
                let text = joined_text(&blocks);
                let id = payload
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("codex:{line_no}"));
                self.codex_text.insert((role.to_string(), text));
                self.upsert(ChatTurn {
                    id,
                    role: role.to_string(),
                    time,
                    blocks,
                })
            }
            "function_call" | "custom_tool_call" => {
                let name = payload
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if name.is_empty() {
                    return None;
                }
                let call_id = payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .or_else(|| payload.get("id").and_then(|v| v.as_str()))
                    .unwrap_or("")
                    .to_string();
                if call_id.is_empty() {
                    return None;
                }
                self.codex_calls.insert(call_id.clone());
                let input = json_text(
                    payload
                        .get("arguments")
                        .or_else(|| payload.get("input"))
                        .unwrap_or(&Value::Null),
                );
                self.upsert(ChatTurn {
                    id: format!("call:{call_id}"),
                    role: "assistant".into(),
                    time,
                    blocks: vec![ChatBlock::ToolCall {
                        id: call_id,
                        name,
                        input,
                    }],
                })
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = payload
                    .get("call_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if call_id.is_empty() {
                    return None;
                }
                let content = json_text(payload.get("output").unwrap_or(&Value::Null));
                if content.is_empty() {
                    return None;
                }
                self.upsert(ChatTurn {
                    id: format!("out:{call_id}"),
                    role: "tool".into(),
                    time,
                    blocks: vec![ChatBlock::ToolResult {
                        id: call_id,
                        content,
                    }],
                })
            }
            _ => None,
        }
    }

    fn codex_event(
        &mut self,
        line_no: u64,
        payload: &Value,
        time: Option<String>,
    ) -> Option<ChatTurn> {
        let etype = payload.get("type").and_then(|v| v.as_str()).unwrap_or("");
        // Usage ledger, not a chat message. task_* and settings are not either.
        if etype == "token_count"
            || etype == "task_started"
            || etype == "task_complete"
            || etype == "thread_settings_applied"
        {
            return None;
        }
        if etype == "user_message" || etype == "agent_message" {
            let role = if etype == "user_message" {
                "user"
            } else {
                "assistant"
            };
            let text = payload
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .trim()
                .to_string();
            if text.is_empty() || self.codex_text.contains(&(role.to_string(), text.clone())) {
                return None;
            }
            self.codex_text.insert((role.to_string(), text.clone()));
            return self.upsert(ChatTurn {
                id: format!("codex-event:{line_no}"),
                role: role.into(),
                time,
                blocks: vec![ChatBlock::Text { text }],
            });
        }
        if etype != "item_completed" {
            return None;
        }
        let item = payload.get("item")?;
        let itype = item.get("type").and_then(|v| v.as_str()).unwrap_or("");
        match itype {
            "UserMessage" | "AgentMessage" => {
                let role = if itype == "UserMessage" {
                    "user"
                } else {
                    "assistant"
                };
                let blocks = text_blocks(item.get("content"));
                if blocks.is_empty() {
                    return None;
                }
                let text = joined_text(&blocks);
                if self.codex_text.contains(&(role.to_string(), text.clone())) {
                    return None;
                }
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("codex-event:{line_no}"));
                self.codex_text.insert((role.to_string(), text));
                self.upsert(ChatTurn {
                    id,
                    role: role.into(),
                    time,
                    blocks,
                })
            }
            "CommandExecution" => {
                // response_item already carried the call. Don't double it.
                if !self.codex_calls.is_empty() {
                    return None;
                }
                let id = item
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("codex-exec:{line_no}"));
                let command = item
                    .get("command")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|p| p.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                let output = item
                    .get("aggregated_output")
                    .or_else(|| item.get("stdout"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if command.is_empty() && output.is_empty() {
                    return None;
                }
                let mut blocks = Vec::new();
                if !command.is_empty() {
                    blocks.push(ChatBlock::ToolCall {
                        id: id.clone(),
                        name: "exec".into(),
                        input: command,
                    });
                }
                if !output.is_empty() {
                    blocks.push(ChatBlock::ToolResult {
                        id: id.clone(),
                        content: output,
                    });
                }
                self.upsert(ChatTurn {
                    id,
                    role: "assistant".into(),
                    time,
                    blocks,
                })
            }
            _ => None,
        }
    }

    fn push_grok(&mut self, line_no: u64, line: &str) -> Option<ChatTurn> {
        let Ok(parsed) = serde_json::from_str::<Value>(line) else {
            return None;
        };
        if parsed.get("synthetic_reason").is_some() {
            return None;
        }
        let kind = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let time = parsed
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        match kind {
            "user" | "assistant" => {
                let mut blocks = text_blocks(parsed.get("content"));
                if kind == "assistant" {
                    if let Some(calls) = parsed.get("tool_calls").and_then(|v| v.as_array()) {
                        for call in calls {
                            let id = call
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let name = call
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if id.is_empty() || name.is_empty() {
                                continue;
                            }
                            let input = json_text(
                                call.get("arguments")
                                    .or_else(|| call.get("input"))
                                    .unwrap_or(&Value::Null),
                            );
                            blocks.push(ChatBlock::ToolCall { id, name, input });
                        }
                    }
                }
                if blocks.is_empty() {
                    return None;
                }
                let id = parsed
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("grok:{line_no}"));
                let role = if kind == "user" { "user" } else { "assistant" };
                self.upsert(ChatTurn {
                    id,
                    role: role.into(),
                    time,
                    blocks,
                })
            }
            "reasoning" => {
                let id = parsed
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|id| format!("grok-reasoning:{id}"))
                    .unwrap_or_else(|| format!("grok-reasoning:{line_no}"));
                self.upsert(ChatTurn {
                    id,
                    role: "assistant".into(),
                    time,
                    blocks: vec![thinking_block(summary_text(parsed.get("summary")), None)],
                })
            }
            "tool_result" => {
                let id = parsed
                    .get("tool_call_id")
                    .or_else(|| parsed.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                if id.is_empty() {
                    return None;
                }
                let content = json_text(parsed.get("content").unwrap_or(&Value::Null));
                if content.is_empty() {
                    return None;
                }
                self.upsert(ChatTurn {
                    id: format!("grok-out:{id}"),
                    role: "tool".into(),
                    time,
                    blocks: vec![ChatBlock::ToolResult { id, content }],
                })
            }
            _ => None,
        }
    }

    fn push_gemini(&mut self, line_no: u64, line: &str) -> Vec<ChatTurn> {
        let Ok(parsed) = serde_json::from_str::<Value>(line) else {
            return Vec::new();
        };
        if let Some(set) = parsed.get("$set") {
            let Some(messages) = set.get("messages").and_then(|v| v.as_array()) else {
                return Vec::new();
            };
            let mut out = Vec::new();
            for (i, message) in messages.iter().enumerate() {
                if let Some(turn) = self.gemini_message(line_no * 1000 + i as u64, message) {
                    out.push(turn);
                }
            }
            return out;
        }
        self.gemini_message(line_no, &parsed).into_iter().collect()
    }

    fn gemini_message(&mut self, line_no: u64, parsed: &Value) -> Option<ChatTurn> {
        let kind = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let role = match kind {
            "user" => "user",
            "gemini" => "assistant",
            _ => return None,
        };
        let blocks = text_blocks(parsed.get("content"));
        if blocks.is_empty() {
            return None;
        }
        let id = parsed
            .get("id")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_else(|| format!("gemini:{line_no}"));
        let time = parsed
            .get("timestamp")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        self.upsert(ChatTurn {
            id,
            role: role.into(),
            time,
            blocks,
        })
    }

    /// Milliseconds since the previous timestamped record, and remember
    /// this one's time.
    fn stamp(&mut self, parsed: &Value) -> Option<u64> {
        let at = parsed
            .get("timestamp")
            .and_then(|v| v.as_str())
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .map(|d| d.timestamp_millis());
        let since = at.zip(self.last_time_ms).map(|(a, b)| (a - b).max(0) as u64);
        if at.is_some() {
            self.last_time_ms = at;
        }
        since
    }

    fn upsert(&mut self, turn: ChatTurn) -> Option<ChatTurn> {
        if turn.blocks.is_empty() {
            return None;
        }
        if let Some(&idx) = self.by_id.get(&turn.id) {
            let existing = &mut self.turns[idx];
            let mut added = false;
            for block in turn.blocks {
                if !existing.blocks.iter().any(|have| same_block(have, &block)) {
                    existing.blocks.push(block);
                    added = true;
                }
            }
            if existing.time.is_none() {
                existing.time = turn.time;
            }
            if !added {
                return None;
            }
            return Some(existing.clone());
        }
        let idx = self.turns.len();
        self.by_id.insert(turn.id.clone(), idx);
        self.turns.push(turn.clone());
        Some(turn)
    }
}

fn flag_true(value: &Value, key: &str) -> bool {
    value.get(key).and_then(|v| v.as_bool()) == Some(true)
}

fn claude_blocks(content: Option<&Value>) -> Vec<ChatBlock> {
    let Some(content) = content else {
        return Vec::new();
    };
    if let Some(text) = content.as_str() {
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        return vec![ChatBlock::Text {
            text: text.to_string(),
        }];
    }
    let Some(parts) = content.as_array() else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    for part in parts {
        if let Some(text) = part.as_str() {
            let text = text.trim();
            if !text.is_empty() {
                blocks.push(ChatBlock::Text {
                    text: text.to_string(),
                });
            }
            continue;
        }
        let kind = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if kind == "tool_use" {
            let id = part
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let name = part
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() || name.is_empty() {
                continue;
            }
            let input = json_text(part.get("input").unwrap_or(&Value::Null));
            blocks.push(ChatBlock::ToolCall { id, name, input });
            continue;
        }
        if kind == "thinking" {
            // TW10: a signature-only block (no text) is redacted.
            let text = part.get("thinking").and_then(|v| v.as_str()).map(str::trim).filter(|t| !t.is_empty());
            blocks.push(thinking_block(text.map(str::to_string), None));
            continue;
        }
        if kind == "redacted_thinking" {
            blocks.push(thinking_block(None, None));
            continue;
        }
        if kind == "tool_result" {
            let id = part
                .get("tool_use_id")
                .or_else(|| part.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if id.is_empty() {
                continue;
            }
            let content = json_text(part.get("content").unwrap_or(&Value::Null));
            blocks.push(ChatBlock::ToolResult { id, content });
            continue;
        }
        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
            let text = text.trim();
            if !text.is_empty() {
                blocks.push(ChatBlock::Text {
                    text: text.to_string(),
                });
            }
        }
    }
    blocks
}

fn thinking_block(text: Option<String>, duration: Option<u64>) -> ChatBlock {
    ChatBlock::Thinking { redacted: text.is_none(), text, duration }
}

fn set_thinking_duration(blocks: &mut [ChatBlock], since_last: Option<u64>) {
    for block in blocks {
        if let ChatBlock::Thinking { duration, .. } = block {
            *duration = since_last;
        }
    }
}

/// A reasoning `summary`: a string, or parts with `text`. `None` when empty.
fn summary_text(summary: Option<&Value>) -> Option<String> {
    let text = match summary? {
        Value::String(s) => s.trim().to_string(),
        Value::Array(parts) => parts
            .iter()
            .filter_map(|p| p.as_str().or_else(|| p.get("text").and_then(|v| v.as_str())))
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"),
        _ => String::new(),
    };
    (!text.is_empty()).then_some(text)
}

fn text_blocks(content: Option<&Value>) -> Vec<ChatBlock> {
    let Some(content) = content else {
        return Vec::new();
    };
    if let Some(text) = content.as_str() {
        let text = text.trim();
        if text.is_empty() {
            return Vec::new();
        }
        return vec![ChatBlock::Text {
            text: text.to_string(),
        }];
    }
    let Some(parts) = content.as_array() else {
        return Vec::new();
    };
    let mut blocks = Vec::new();
    for part in parts {
        if let Some(text) = part.as_str() {
            let text = text.trim();
            if !text.is_empty() {
                blocks.push(ChatBlock::Text {
                    text: text.to_string(),
                });
            }
            continue;
        }
        let kind = part
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if kind == "tool_use" || kind == "function_call" {
            continue;
        }
        if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
            let text = text.trim();
            if !text.is_empty() && (kind.is_empty() || is_text_kind(&kind)) {
                blocks.push(ChatBlock::Text {
                    text: text.to_string(),
                });
            }
        }
    }
    blocks
}

fn is_text_kind(kind: &str) -> bool {
    matches!(kind, "text" | "input_text" | "output_text")
}

fn joined_text(blocks: &[ChatBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            ChatBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn json_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => text.clone(),
        Value::Array(parts) => {
            let texts: Vec<&str> = parts
                .iter()
                .filter_map(|part| {
                    part.get("text")
                        .and_then(|v| v.as_str())
                        .or_else(|| part.as_str())
                })
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .collect();
            if texts.is_empty() {
                value.to_string()
            } else {
                texts.join("\n")
            }
        }
        other => other.to_string(),
    }
}

fn same_block(a: &ChatBlock, b: &ChatBlock) -> bool {
    match (a, b) {
        (ChatBlock::Text { text: left }, ChatBlock::Text { text: right }) => left == right,
        (ChatBlock::ToolCall { id: left, .. }, ChatBlock::ToolCall { id: right, .. }) => {
            left == right
        }
        (ChatBlock::ToolResult { id: left, .. }, ChatBlock::ToolResult { id: right, .. }) => {
            left == right
        }
        (
            ChatBlock::Thinking { text: left, redacted: lr, .. },
            ChatBlock::Thinking { text: right, redacted: rr, .. },
        ) => left == right && lr == rr,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_continue::locate_continue_transcript;
    use std::fs;
    use std::path::PathBuf;

    struct HomeGuard {
        path: PathBuf,
        // Holds the ONE env lock; restores HOME + removes the dir on drop.
        _temp: crate::test_env::TempHome,
    }

    impl HomeGuard {
        fn new(_label: &str) -> Self {
            let temp = crate::test_env::TempHome::new();
            Self { path: temp.path().to_path_buf(), _temp: temp }
        }
    }

    fn text_of(turn: &ChatTurn) -> String {
        joined_text(&turn.blocks)
    }

    #[test]
    fn claude_fixture_merges_message_id_and_skips_injected() {
        let body = concat!(
            r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"text","text":"Hello"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"id":"msg1","role":"assistant","content":[{"type":"text","text":"Hi"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a2","timestamp":"2026-01-01T00:00:02Z","message":{"id":"msg1","role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}}]}}"#,
            "\n",
            r#"{"type":"user","uuid":"u2","timestamp":"2026-01-01T00:00:03Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"file.txt"}]}}"#,
            "\n",
            r#"{"type":"user","isMeta":true,"uuid":"u3","message":{"role":"user","content":"INJECTED_LINE"}}"#,
            "\n",
            r#"{"type":"user","isCompactSummary":true,"uuid":"u4","message":{"role":"user","content":"COMPACTION_SUMMARY_LINE"}}"#,
            "\n",
            r#"{"type":"attachment","uuid":"u5"}"#,
            "\n",
            "not json\n",
        );
        let turns = parse_chat_transcript("claude", body);
        let msg1: Vec<&ChatTurn> = turns.iter().filter(|t| t.id == "msg1").collect();
        assert_eq!(msg1.len(), 1, "same message id is one turn");
        assert_eq!(msg1[0].role, "assistant");
        assert_eq!(text_of(msg1[0]), "Hi");
        assert!(msg1[0].blocks.iter().any(|b| matches!(
            b,
            ChatBlock::ToolCall { id, name, .. } if id == "toolu_1" && name == "Bash"
        )));
        let user = turns.iter().find(|t| t.id == "u1").expect("user turn");
        assert_eq!(user.role, "user");
        assert_eq!(text_of(user), "Hello");
        assert_eq!(user.time.as_deref(), Some("2026-01-01T00:00:00Z"));
        let result = turns
            .iter()
            .find(|t| t.id == "u2")
            .expect("tool result turn");
        assert!(result.blocks.iter().any(|b| matches!(
            b,
            ChatBlock::ToolResult { id, content } if id == "toolu_1" && content == "file.txt"
        )));
        let dumped = serde_json::to_string(&turns).expect("json");
        assert!(!dumped.contains("INJECTED_LINE"));
        assert!(!dumped.contains("COMPACTION_SUMMARY_LINE"));
        assert!(!dumped.contains("This is a new chat continued"));
    }

    #[test]
    fn codex_fixture_uses_header_id_and_skips_token_count() {
        let home = HomeGuard::new("codex");
        let project = home.path.join("work");
        fs::create_dir_all(&project).expect("project");
        let project_s = project.to_string_lossy().into_owned();
        let day = home
            .path
            .join(".codex")
            .join("sessions")
            .join("2026")
            .join("09")
            .join("26");
        fs::create_dir_all(&day).expect("day");
        let sid = "sess-real-codex";
        let decoy = day.join(format!("{sid}.jsonl"));
        fs::write(
            &decoy,
            format!(
                "{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"other\",\"cwd\":{cwd}}}}}\n{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"DECOY_FILENAME\"}}]}}}}\n",
                cwd = serde_json::to_string(&project_s).expect("cwd"),
            ),
        )
        .expect("decoy");
        fs::write(
            home.path.join(".codex").join("history.jsonl"),
            "{\"session_id\":\"sess-real-codex\",\"text\":\"HISTORY_NOT_CHAT\"}\n",
        )
        .expect("history");
        let rollout = day.join("rollout-2026-09-26T00-00-00-not-the-id.jsonl");
        let cwd_json = serde_json::to_string(&project_s).expect("cwd json");
        let body = format!(
            concat!(
                r#"{{"type":"session_meta","payload":{{"id":"sess-real-codex","cwd":{cwd}}}}}"#,
                "\n",
                r#"{{"type":"response_item","timestamp":"2026-09-26T00:00:01Z","payload":{{"type":"message","id":"m-user","role":"user","content":[{{"type":"input_text","text":"Hi"}}]}}}}"#,
                "\n",
                r#"{{"type":"response_item","payload":{{"type":"message","id":"m-agent","role":"assistant","content":[{{"type":"output_text","text":"Hello"}}]}}}}"#,
                "\n",
                r#"{{"type":"response_item","payload":{{"type":"function_call","name":"exec_command","arguments":"{{\"cmd\":\"ls\"}}","call_id":"call_1"}}}}"#,
                "\n",
                r#"{{"type":"response_item","payload":{{"type":"function_call_output","call_id":"call_1","output":"listed"}}}}"#,
                "\n",
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":424242}}}}}}}}"#,
                "\n",
                r#"{{"type":"event_msg","payload":{{"type":"task_complete","last_agent_message":"Hello"}}}}"#,
                "\n",
            ),
            cwd = cwd_json,
        );
        fs::write(&rollout, &body).expect("rollout");
        let found = locate_continue_transcript("codex", sid, &project_s).expect("rollout path");
        assert_eq!(found, rollout);
        assert_ne!(found, decoy);
        let text = fs::read_to_string(&found).expect("read rollout");
        let turns = parse_chat_transcript("codex", &text);
        let dumped = serde_json::to_string(&turns).expect("json");
        assert!(dumped.contains("Hi"), "{dumped}");
        assert!(dumped.contains("Hello"), "{dumped}");
        assert!(!dumped.contains("424242"), "{dumped}");
        assert!(!dumped.contains("DECOY_FILENAME"), "{dumped}");
        assert!(!dumped.contains("HISTORY_NOT_CHAT"), "{dumped}");
        assert!(turns.iter().any(|t| t.blocks.iter().any(|b| matches!(
            b,
            ChatBlock::ToolCall { name, .. } if name == "exec_command"
        ))));
        assert!(turns.iter().any(|t| t.blocks.iter().any(|b| matches!(
            b,
            ChatBlock::ToolResult { content, .. } if content == "listed"
        ))));
        let hellos = turns.iter().filter(|t| text_of(t) == "Hello").count();
        assert_eq!(hellos, 1);
    }

    #[test]
    fn grok_fixture_reads_chat_history_not_updates() {
        let home = HomeGuard::new("grok");
        let project = home.path.join("work");
        fs::create_dir_all(&project).expect("project");
        let project_s = project.to_string_lossy().into_owned();
        let sid = "11111111-1111-1111-1111-111111111111";
        let dir = home
            .path
            .join(".grok")
            .join("sessions")
            .join("cwd")
            .join(sid);
        fs::create_dir_all(&dir).expect("session dir");
        let summary = serde_json::json!({
            "info": {"cwd": project_s, "session_kind": "main"}
        });
        fs::write(
            dir.join("summary.json"),
            serde_json::to_string(&summary).expect("summary"),
        )
        .expect("summary");
        let chat = concat!(
            "{\"type\":\"system\",\"content\":\"SYSTEM_PROMPT\"}\n",
            "{\"type\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"Ping\"}]}\n",
            "{\"type\":\"user\",\"synthetic_reason\":\"injected_context\",\"content\":\"SYNTHETIC_GROK\"}\n",
            "{\"type\":\"assistant\",\"content\":\"Pong\",\"tool_calls\":[{\"id\":\"call-g1\",\"name\":\"read_file\",\"arguments\":\"{\\\"target_file\\\":\\\"a\\\"}\"}]}\n",
            "{\"type\":\"tool_result\",\"tool_call_id\":\"call-g1\",\"content\":\"BODY\"}\n",
            "{\"type\":\"reasoning\",\"summary\":[{\"text\":\"hidden\"}]}\n",
        );
        fs::write(dir.join("chat_history.jsonl"), chat).expect("chat");
        fs::write(
            dir.join("updates.jsonl"),
            "{\"method\":\"session/update\",\"params\":{\"update\":{\"sessionUpdate\":\"user_message_chunk\",\"content\":\"FROM_UPDATES_LEDGER\"}}}\n",
        )
        .expect("updates");
        let found = locate_continue_transcript("grok", sid, &project_s).expect("grok path");
        assert!(found.ends_with("chat_history.jsonl"), "{}", found.display());
        assert!(!found.ends_with("updates.jsonl"));
        let turns = parse_chat_transcript("grok", &fs::read_to_string(&found).expect("read"));
        let dumped = serde_json::to_string(&turns).expect("json");
        assert!(dumped.contains("Ping"), "{dumped}");
        assert!(dumped.contains("Pong"), "{dumped}");
        assert!(dumped.contains("read_file"), "{dumped}");
        assert!(dumped.contains("BODY"), "{dumped}");
        assert!(!dumped.contains("SYNTHETIC_GROK"), "{dumped}");
        assert!(!dumped.contains("FROM_UPDATES_LEDGER"), "{dumped}");
        assert!(!dumped.contains("SYSTEM_PROMPT"), "{dumped}");
        // TW10: Grok reasoning is a Thinking block with its summary.
        assert!(
            turns.iter().any(|t| t.blocks.iter().any(|b| matches!(
                b,
                ChatBlock::Thinking { text: Some(text), redacted: false, duration: None } if text == "hidden"
            ))),
            "{dumped}"
        );
        let updates = fs::read_to_string(dir.join("updates.jsonl")).expect("updates");
        let from_ledger = parse_chat_transcript("grok", &updates);
        assert!(from_ledger.is_empty(), "{from_ledger:?}");
    }

    #[test]
    fn gemini_fixture_reads_user_and_gemini_not_prefix() {
        let home = HomeGuard::new("gemini");
        let project = home.path.join("work");
        fs::create_dir_all(&project).expect("project");
        let project_s = project.to_string_lossy().into_owned();
        let slug = "slug1";
        fs::create_dir_all(
            home.path
                .join(".gemini")
                .join("tmp")
                .join(slug)
                .join("chats"),
        )
        .expect("chats");
        fs::write(
            home.path.join(".gemini").join("projects.json"),
            serde_json::json!({"projects": {project_s.clone(): slug}}).to_string(),
        )
        .expect("projects");
        let sid = "17526a7e-9040-43b4-8b87-e6c75a6004a0";
        let wrong = home
            .path
            .join(".gemini")
            .join("tmp")
            .join(slug)
            .join("chats")
            .join("session-2026-01-01T00-00-00-17526a7e.jsonl");
        fs::write(
            &wrong,
            "{\"sessionId\":\"other-session-not-this\",\"lastUpdated\":\"2026-09-01T00:00:00Z\"}\n{\"type\":\"user\",\"id\":\"nope\",\"content\":[{\"text\":\"PREFIX_ONLY\"}]}\n",
        )
        .expect("wrong");
        let right = home
            .path
            .join(".gemini")
            .join("tmp")
            .join(slug)
            .join("chats")
            .join("session-2026-02-02T00-00-00-zzzzzzzz.jsonl");
        fs::write(
            &right,
            format!(
                concat!(
                    r#"{{"sessionId":"{sid}","lastUpdated":"2026-02-02T00:00:00Z"}}"#,
                    "\n",
                    r#"{{"type":"user","id":"gu","timestamp":"2026-02-02T00:00:01Z","content":[{{"text":"Hi gem"}}]}}"#,
                    "\n",
                    r#"{{"type":"gemini","id":"gg","timestamp":"2026-02-02T00:00:02Z","content":"Hello gem"}}"#,
                    "\n",
                    r#"{{"$set":{{"lastUpdated":"2026-02-02T00:00:03Z"}}}}"#,
                    "\n",
                ),
                sid = sid,
            ),
        )
        .expect("right");
        let found = locate_continue_transcript("gemini", sid, &project_s).expect("gemini path");
        assert_eq!(found, right);
        let turns = parse_chat_transcript("gemini", &fs::read_to_string(&found).expect("read"));
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, "user");
        assert_eq!(text_of(&turns[0]), "Hi gem");
        assert_eq!(turns[1].role, "assistant");
        assert_eq!(text_of(&turns[1]), "Hello gem");
        let dumped = serde_json::to_string(&turns).expect("json");
        assert!(!dumped.contains("PREFIX_ONLY"), "{dumped}");
    }

    /// T-S3e (TW10): thinking is a stub, never dropped. Claude
    /// signature-only and `redacted_thinking` → redacted; Codex encrypted
    /// reasoning with an empty summary → redacted, with a summary → text;
    /// Grok's summary → text. Durations come from record times.
    #[test]
    fn thinking_and_reasoning_become_thinking_blocks() {
        let claude = concat!(
            r#"{"type":"user","uuid":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":"Go"}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","timestamp":"2026-01-01T00:00:08Z","message":{"id":"m1","role":"assistant","content":[{"type":"thinking","thinking":"","signature":"c2ln"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a2","timestamp":"2026-01-01T00:00:09Z","message":{"id":"m1","role":"assistant","content":[{"type":"text","text":"Done"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a3","timestamp":"2026-01-01T00:00:12Z","message":{"id":"m2","role":"assistant","content":[{"type":"redacted_thinking","data":"ZW5j"},{"type":"thinking","thinking":"Plan the change."}]}}"#,
            "\n",
        );
        let turns = parse_chat_transcript("claude", claude);
        let m1 = turns.iter().find(|t| t.id == "m1").expect("m1");
        assert_eq!(
            m1.blocks,
            vec![
                ChatBlock::Thinking { text: None, redacted: true, duration: Some(8_000) },
                ChatBlock::Text { text: "Done".into() },
            ]
        );
        let m2 = turns.iter().find(|t| t.id == "m2").expect("m2");
        assert_eq!(
            m2.blocks,
            vec![
                ChatBlock::Thinking { text: None, redacted: true, duration: Some(3_000) },
                ChatBlock::Thinking { text: Some("Plan the change.".into()), redacted: false, duration: Some(3_000) },
            ]
        );

        let codex = concat!(
            r#"{"timestamp":"2026-01-01T00:00:00Z","type":"response_item","payload":{"type":"message","id":"mu","role":"user","content":[{"type":"input_text","text":"Go"}]}}"#,
            "\n",
            r#"{"timestamp":"2026-01-01T00:00:05Z","type":"response_item","payload":{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"ENCRYPTED_NEVER_READ"}}"#,
            "\n",
            r#"{"timestamp":"2026-01-01T00:00:06Z","type":"response_item","payload":{"type":"reasoning","id":"rs_2","summary":[{"type":"summary_text","text":"Check the tests."}],"encrypted_content":"ENCRYPTED_NEVER_READ"}}"#,
            "\n",
        );
        let turns = parse_chat_transcript("codex", codex);
        let rs1 = turns.iter().find(|t| t.id == "rs_1").expect("rs_1");
        assert_eq!(rs1.role, "assistant");
        assert_eq!(rs1.blocks, vec![ChatBlock::Thinking { text: None, redacted: true, duration: Some(5_000) }]);
        let rs2 = turns.iter().find(|t| t.id == "rs_2").expect("rs_2");
        assert_eq!(
            rs2.blocks,
            vec![ChatBlock::Thinking { text: Some("Check the tests.".into()), redacted: false, duration: Some(1_000) }]
        );
        let dumped = serde_json::to_string(&turns).expect("json");
        assert!(!dumped.contains("ENCRYPTED_NEVER_READ"), "{dumped}");

        let grok = "{\"type\":\"reasoning\",\"summary\":[{\"type\":\"summary_text\",\"text\":\"Look first.\"}]}\n{\"type\":\"reasoning\",\"summary\":[],\"encrypted_content\":\"x\"}\n";
        let turns = parse_chat_transcript("grok", grok);
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].blocks, vec![ChatBlock::Thinking { text: Some("Look first.".into()), redacted: false, duration: None }]);
        assert_eq!(turns[1].blocks, vec![ChatBlock::Thinking { text: None, redacted: true, duration: None }]);

        // Wire shape: `type: thinking`, absent optionals omitted.
        let wire = serde_json::to_value(&turns[1].blocks[0]).expect("wire");
        assert_eq!(wire, serde_json::json!({"type": "thinking", "redacted": true}));
    }

    #[test]
    fn unknown_provider_and_unknown_line_are_skipped() {
        let turns = parse_chat_transcript("cursor", "{\"type\":\"user\",\"content\":\"x\"}\n");
        assert!(turns.is_empty());
        let claude = parse_chat_transcript("claude", "{\"weird\":true}\n");
        assert!(claude.is_empty());
    }

    #[test]
    fn locate_misses_when_session_id_empty() {
        let found = locate_continue_transcript("claude", "  ", "/tmp");
        assert!(found.is_none());
    }
}
