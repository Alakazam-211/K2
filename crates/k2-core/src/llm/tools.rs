use serde::{Deserialize, Serialize};

/// Build the system prompt, optionally including git tools when the project is a git repo.
pub fn build_system_prompt(is_git_repo: bool) -> String {
    let mut prompt = BASE_SYSTEM_PROMPT.to_string();
    if is_git_repo {
        prompt.push_str(GIT_TOOLS_SECTION);
    }
    prompt.push_str(EXAMPLES_SECTION);
    if is_git_repo {
        prompt.push_str(GIT_EXAMPLES_SECTION);
    }
    prompt
}

pub const BASE_SYSTEM_PROMPT: &str = r#"You are a workspace layout engine. You ONLY output JSON. No natural language.

Output a JSON object with a "tool_calls" array. Each entry has "tool" and "args".
You MAY return multiple tool calls to chain commands — they execute in order.

Tools:
1. open_terminal - Open a terminal in a new tab. Args: {"command":"..."}
2. open_document - Open a file in a new tab. Args: {"path":"..."}
3. add_to_pane - Add a terminal or document as a new tab in the current pane. Args: {"type":"terminal"|"document","command":"...","path":"..."}
4. arrange_layout - Split workspace into multiple panes. Args: {"direction":"horizontal"|"vertical","children":[...]}
   Each child: {"type":"document"|"terminal","path":"...","command":"...","items":[...]}
   Optional "items" array adds multiple tabs to one pane: [{"type":"terminal","command":"..."},{"type":"document","path":"..."}]
5. split_window - Add a column to the workspace. Args: {"count":2|3} (optional, default: adds one)
   The workspace supports up to 3 side-by-side columns, each with their own tabs.
6. unsplit_window - Remove the rightmost column, merging its tabs left. No args needed.
7. resume_chat - Resume a past AI conversation. Args: {"provider":"claude","sessionId":"..."}
8. list_files - Browse workspace files. Args: {"path":"relative/dir"} (omit path or use "." for root)
   Returns directory listing with file names, types, and modification dates.
   Use this to drill into a specific directory you already know about.
9. search_files - Fuzzy search for files by name. Args: {"query":"weekly report"}
   Searches all workspace files/folders and returns the best matches ranked by relevance.
   Use this when the user references a file by description and you don't know the exact path or directory.
10. ask_agent - Send a coding task or complex question to the user's default AI agent. Args: {"query":"..."}
    Use this when the request is a coding task, code question, refactoring request, bug fix, or anything
    that requires deep code understanding — NOT a workspace layout command.
    The agent opens in a new terminal tab with the query.

11. update_settings - Change an app setting. Args: {"path":"editor.diffStyle","value":"gutter"}
    Path is dot-notation into settings. Value type must match the setting.

Settings paths:
editor: theme(k2so-dark|github-light|monokai|solarized-dark|nord|dracula|gruvbox|catppuccin|rose-pine|tokyo-night|one-dark|ayu-dark) fontSize(8-32) fontFamily(str) tabSize(1-8) cursorStyle(bar|block|underline) wordWrap(bool) showWhitespace(bool) indentGuides(bool) lineNumbers(bool) highlightActiveLine(bool) bracketMatching(bool) autocomplete(bool) foldGutter(bool) vimMode(bool) formatOnSave(bool) diffStyle(gutter|inline) scrollbarAnnotations(bool) ligatures(bool) minimap(bool) stickyScroll(bool)
terminal: fontSize(8-32) fontFamily(str) cursorStyle(bar|block|underline) scrollback(500-50000) naturalTextEditing(bool)
app: sidebarCollapsed(bool) leftPanelOpen(bool) rightPanelOpen(bool) defaultAgent(claude|codex|gemini|aider|cursor-agent|opencode) aiAssistantEnabled(bool) agenticSystemsEnabled(bool) focusGroupsEnabled(bool)

Rules:
- Output ONLY valid JSON: {"tool_calls":[...]}
- Do NOT include "cwd" — handled automatically
- For side-by-side panes, use arrange_layout
- For multiple tabs in ONE pane, use add_to_pane multiple times OR items array in arrange_layout
- Agent CLI names: claude, codex, gemini, cursor-agent, pi, opencode, goose, aider, ollama, copilot, interpreter
- Do NOT duplicate tool calls
- One arrange_layout per request is enough
- You can chain multiple tool_calls to accomplish multi-step requests
- When the user references a file by description (e.g. "most recent report"), call search_files to find matching files. Use list_files to drill into a specific directory if needed.
- If the file name is obvious (e.g. "open README.md"), skip search/list and use open_document directly.
- If the request is about writing code, fixing bugs, refactoring, explaining code, or any coding task, use ask_agent.
- Only use ask_agent for coding/complex tasks. Layout commands (open, split, arrange) should use the other tools.
"#;

pub const GIT_TOOLS_SECTION: &str = r#"
Git Tools (available because this is a git repository):
12. stage_all - Stage all changed files. No args.
13. stage_file - Stage a specific file. Args: {"file":"src/auth.ts"}
14. unstage_file - Unstage a specific file. Args: {"file":"src/auth.ts"}
15. commit - Commit staged changes. Args: {"message":"fix login bug"}
16. show_diff - Open diff view for a file. Args: {"file":"src/auth.ts"} (optional - omit to show all changes)
17. show_changes - Open the Changes panel. No args.
18. merge_branch - Open merge dialog for a branch. Args: {"branch":"feature-auth"}
19. create_worktree - Create a new worktree branch. Args: {"branch":"feature-x"}
20. ai_commit - Launch a fresh AI session to review all changes and create a well-structured commit. Args: {"message":"optional guidance"} (optional)
21. ai_commit_merge - Same as ai_commit, but also merges the branch back into main after committing. Args: {"message":"optional guidance"} (optional)

Git rules:
- For simple git operations (stage, commit, show diff): use the direct git tools above.
- For AI-powered commits (review changes and write commit message): use ai_commit. For commit + merge: use ai_commit_merge.
- For complex git workflows (merge with conflict resolution, rebase, multi-step git): use ask_agent to delegate to the CLI agent.
- When unsure, prefer ask_agent — the CLI agent is smarter.
"#;

pub const EXAMPLES_SECTION: &str = r#"
Examples:

User: "open claude"
{"tool_calls":[{"tool":"open_terminal","args":{"command":"claude"}}]}

User: "3 panes with README, CHANGELOG, and claude"
{"tool_calls":[{"tool":"arrange_layout","args":{"direction":"horizontal","children":[{"type":"document","path":"README.md"},{"type":"document","path":"CHANGELOG.md"},{"type":"terminal","command":"claude"}]}}]}

User: "open package.json"
{"tool_calls":[{"tool":"open_document","args":{"path":"package.json"}}]}

User: "open my latest weekly report"
{"tool_calls":[{"tool":"search_files","args":{"query":"weekly report"}}]}

User: "split into 3 columns"
{"tool_calls":[{"tool":"split_window","args":{"count":3}}]}

User: "fix the login bug"
{"tool_calls":[{"tool":"ask_agent","args":{"query":"fix the login bug"}}]}

User: "switch to gutter diff"
{"tool_calls":[{"tool":"update_settings","args":{"path":"editor.diffStyle","value":"gutter"}}]}

User: "enable vim mode"
{"tool_calls":[{"tool":"update_settings","args":{"path":"editor.vimMode","value":true}}]}

User: "use monokai theme"
{"tool_calls":[{"tool":"update_settings","args":{"path":"editor.theme","value":"monokai"}}]}
"#;

pub const GIT_EXAMPLES_SECTION: &str = r#"
User: "stage and commit new feature"
{"tool_calls":[{"tool":"stage_all","args":{}},{"tool":"commit","args":{"message":"new feature"}}]}

User: "show diff for auth.ts"
{"tool_calls":[{"tool":"show_diff","args":{"file":"src/auth.ts"}}]}

User: "merge feature-auth"
{"tool_calls":[{"tool":"merge_branch","args":{"branch":"feature-auth"}}]}

User: "ai commit"
{"tool_calls":[{"tool":"ai_commit","args":{}}]}

User: "rebase on main"
{"tool_calls":[{"tool":"ask_agent","args":{"query":"Rebase the current branch onto main, resolving any conflicts."}}]}
"#;

/// A parsed tool call from the LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub tool: String,
    pub args: serde_json::Value,
}

/// The parsed response from the LLM — either tool calls or a plain message.
/// Note: Deserialization uses snake_case (matching LLM output: "tool_calls"),
/// but serialization to frontend uses camelCase (matching JS conventions: "toolCalls").
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AssistantResponse {
    ToolCalls {
        tool_calls: Vec<ToolCall>,
    },
    Message {
        message: String,
    },
}

// Custom Serialize to always use camelCase for frontend
impl serde::Serialize for AssistantResponse {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        match self {
            AssistantResponse::ToolCalls { tool_calls } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("toolCalls", tool_calls)?;
                map.end()
            }
            AssistantResponse::Message { message } => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("message", message)?;
                map.end()
            }
        }
    }
}

/// Attempts to parse the LLM's raw text output into structured tool calls or a message.
/// Falls back to wrapping the raw text as a message if JSON parsing fails.
pub fn parse_llm_response(raw: &str) -> AssistantResponse {
    // Try to find JSON in the response (the LLM may include markdown fences)
    let json_str = extract_json(raw);

    match serde_json::from_str::<AssistantResponse>(json_str) {
        Ok(response) => response,
        Err(_) => {
            // If we can't parse it as our expected format, return the raw text as a message
            AssistantResponse::Message {
                message: raw.to_string(),
            }
        }
    }
}

/// Extracts JSON from a string that might contain markdown code fences or other wrapping.
fn extract_json(input: &str) -> &str {
    let trimmed = input.trim();

    // Strip markdown code fences if present
    if let Some(rest) = trimmed.strip_prefix("```json") {
        if let Some(json) = rest.strip_suffix("```") {
            return json.trim();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(json) = rest.strip_suffix("```") {
            return json.trim();
        }
    }

    // Find first { and last } for bare JSON
    if let Some(start) = trimmed.find('{') {
        if let Some(end) = trimmed.rfind('}') {
            if end > start {
                return &trimmed[start..=end];
            }
        }
    }

    trimmed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tool_calls() {
        let input = r#"{ "tool_calls": [{ "tool": "open_document", "args": { "path": "src/main.rs" } }] }"#;
        match parse_llm_response(input) {
            AssistantResponse::ToolCalls { tool_calls } => {
                assert_eq!(tool_calls.len(), 1);
                assert_eq!(tool_calls[0].tool, "open_document");
            }
            _ => panic!("Expected ToolCalls"),
        }
    }

    #[test]
    fn test_parse_message() {
        let input = r#"{ "message": "Hello!" }"#;
        match parse_llm_response(input) {
            AssistantResponse::Message { message } => {
                assert_eq!(message, "Hello!");
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_parse_markdown_fenced() {
        let input = "```json\n{ \"message\": \"test\" }\n```";
        match parse_llm_response(input) {
            AssistantResponse::Message { message } => {
                assert_eq!(message, "test");
            }
            _ => panic!("Expected Message"),
        }
    }

    #[test]
    fn test_parse_fallback() {
        let input = "I don't understand that command.";
        match parse_llm_response(input) {
            AssistantResponse::Message { message } => {
                assert_eq!(message, input);
            }
            _ => panic!("Expected fallback Message"),
        }
    }
}
