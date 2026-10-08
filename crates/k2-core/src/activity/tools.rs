//! Which tool calls are shell commands (the per-turn `commands` count).
//!
//! One list for every harness, so the hook path and the transcript path
//! classify a call the same way. Names are matched exactly (each CLI's
//! own spelling). A name that is not here counts as a tool, not a
//! command: a new shell tool undercounts commands, it never inflates them.
//!
//! | Harness | Shell tool names | Read from |
//! |---|---|---|
//! | Claude | `Bash`, `PowerShell` (Windows) | hooks + transcript `tool_use` |
//! | Codex | `exec_command`, `shell`, `local_shell`, `shell_command`, `container.exec` | rollout `function_call` / `custom_tool_call` |
//! | Grok | `run_terminal_cmd`, `run_command`, `bash` | `chat_history.jsonl` `tool_calls` |
//! | Gemini | `run_shell_command` | `AfterTool` hook `tool_name` |
//! | Cursor | `run_terminal_cmd`; the `beforeShellExecution` hook | hooks |
//!
//! Not commands: Claude `BashOutput` / `KillShell` (they read or stop a
//! shell that is already counted), Codex `write_stdin` (input to a running
//! command).

use crate::agent_hooks::envelope::HookSource;

/// Shell / terminal tool names, every harness (see the module table).
pub const COMMAND_TOOLS: &[&str] = &[
    // Claude
    "Bash",
    "PowerShell",
    // Codex
    "exec_command",
    "shell",
    "local_shell",
    "shell_command",
    "container.exec",
    // Grok, Cursor
    "run_terminal_cmd",
    "run_command",
    "bash",
    // Gemini
    "run_shell_command",
];

/// Is a call to `name` a shell command?
pub fn is_command_tool(name: &str) -> bool {
    COMMAND_TOOLS.contains(&name.trim())
}

/// A non-Claude hook event that starts (or reports) one lead tool call:
/// `Some(is_command)`, else `None`. Claude hooks are handled event by
/// event in `claude::apply_hook` (they carry `tool_use_id`).
///
/// - Gemini `AfterTool` (its only tool hook K2 installs): one call of
///   `tool_name`.
/// - Cursor `beforeShellExecution`: one shell command;
///   `beforeMCPExecution`: one tool.
pub fn foreign_hook_tool(source: HookSource, event: &str, tool_name: Option<&str>) -> Option<bool> {
    match (source, event) {
        (HookSource::Gemini, "AfterTool") => Some(tool_name.is_some_and(is_command_tool)),
        (HookSource::Cursor, "beforeShellExecution") => Some(true),
        (HookSource::Cursor, "beforeMCPExecution") => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_shell_tools() {
        assert!(is_command_tool("Bash"));
        assert!(is_command_tool("PowerShell"));
        for not in ["BashOutput", "KillShell", "Read", "Edit", "Grep", "Task", "AskUserQuestion", "bash_output"] {
            assert!(!is_command_tool(not), "{not}");
        }
    }

    #[test]
    fn codex_shell_tools() {
        for name in ["exec_command", "shell", "local_shell", "shell_command", "container.exec"] {
            assert!(is_command_tool(name), "{name}");
        }
        for not in ["apply_patch", "write_stdin", "view_image", "web_search", "update_plan"] {
            assert!(!is_command_tool(not), "{not}");
        }
    }

    #[test]
    fn grok_shell_tools() {
        for name in ["run_terminal_cmd", "run_command", "bash"] {
            assert!(is_command_tool(name), "{name}");
        }
        for not in ["read_file", "edit_file", "search_replace", "grep_search"] {
            assert!(!is_command_tool(not), "{not}");
        }
    }

    #[test]
    fn gemini_shell_tools() {
        assert!(is_command_tool("run_shell_command"));
        assert_eq!(foreign_hook_tool(HookSource::Gemini, "AfterTool", Some("run_shell_command")), Some(true));
        assert_eq!(foreign_hook_tool(HookSource::Gemini, "AfterTool", Some("read_file")), Some(false));
        assert_eq!(foreign_hook_tool(HookSource::Gemini, "AfterTool", None), Some(false));
        assert_eq!(foreign_hook_tool(HookSource::Gemini, "BeforeAgent", None), None);
        assert_eq!(foreign_hook_tool(HookSource::Gemini, "AfterAgent", None), None);
    }

    #[test]
    fn cursor_shell_tools() {
        assert!(is_command_tool("run_terminal_cmd"));
        assert_eq!(foreign_hook_tool(HookSource::Cursor, "beforeShellExecution", None), Some(true));
        assert_eq!(foreign_hook_tool(HookSource::Cursor, "beforeMCPExecution", Some("x")), Some(false));
        assert_eq!(foreign_hook_tool(HookSource::Cursor, "beforeSubmitPrompt", None), None);
        assert_eq!(foreign_hook_tool(HookSource::Cursor, "stop", None), None);
        // A Claude event name under another source is not a tool here.
        assert_eq!(foreign_hook_tool(HookSource::Cursor, "PreToolUse", Some("Bash")), None);
    }

    #[test]
    fn names_are_exact() {
        assert!(!is_command_tool("BASH"));
        assert!(!is_command_tool(""));
        assert!(is_command_tool(" Bash "), "surrounding space is trimmed");
    }
}
