// ── Layout constants ──────────────────────────────────────────────────
export const TOPBAR_HEIGHT = 38
export const SIDEBAR_MIN_WIDTH = 180
export const SIDEBAR_MAX_WIDTH = 400
export const SIDEBAR_DEFAULT_WIDTH = 240

// ── Terminal font size constants ──────────────────────────────────────
export const TERMINAL_FONT_SIZE_MIN = 10
export const TERMINAL_FONT_SIZE_MAX = 24
export const TERMINAL_FONT_SIZE_DEFAULT = 13

// ── App constants ─────────────────────────────────────────────────────
export const APP_NAME = 'K2SO'

// ── File size limits ──────────────────────────────────────────────────
export const MAX_FILE_SIZE = 10 * 1024 * 1024  // 10MB
export const MAX_ICON_SIZE = 256 * 1024  // 256KB

// ── Polling intervals ────────────────────────────────────────────────
// Five sidebar components call `useGitInfo` for the same workspace
// path (App, IconRail, SectionItem, Sidebar ×4). At a 5s interval
// that's a `repo.statuses()` per workspace every second across the
// renderer — and each call walks the worktree's tracked-file tree
// loading `.gitattributes` per directory. On a JS workspace with
// thousands of tracked files that pegs the Tauri main at 200% CPU.
// Bumping to 30s drops the rate 6× while a "is the workspace dirty?"
// indicator that lags by tens of seconds is fine for the UX (the
// dot just turns red a little later than it could). Real dirty-state
// visibility happens in the commit panel which fetches on demand.
// Future: deduplicate multi-component polling via a shared timer +
// watch worktree mtime so we only re-fire when the filesystem
// actually changed.
export const GIT_POLL_INTERVAL = 30000  // 30 seconds
export const FILE_POLL_INTERVAL = 2000  // 2 seconds

// ── Terminal ─────────────────────────────────────────────────────────
export const TERMINAL_SCROLLBACK = 5000

// ── Update check ────────────────────────────────────────────────────
export const UPDATE_CHECK_INTERVAL = 3 * 60 * 60 * 1000  // 3 hours

// ── UI timing ────────────────────────────────────────────────────────
export const CONTEXT_MENU_DISMISS_DELAY = 50  // ms

// ── Resumable CLI tools ─────────────────────────────────────────────
// Tools that support session resume.
//
// Two shapes:
//   - flag-style: `<command> <preset-args> <resumeFlag> <uuid>` — the
//     resumed PTY launches with the same preset args as a fresh start
//     (auth flags etc.). Used by Claude/Cursor/Gemini/Pi.
//   - subcommand-style: `<command> <resumeSubcommand> <uuid>` — preset
//     args are dropped because the saved session carries its own
//     model/permissions, and the resume subcommand only accepts a
//     subset of flags. Used by Codex (`codex resume <uuid>` since v0.125).
export interface ResumableCliTool {
  resumeFlag?: string
  resumeSubcommand?: string
  provider: string
}
export const RESUMABLE_CLI_TOOLS: Record<string, ResumableCliTool> = {
  'claude': { resumeFlag: '--resume', provider: 'claude' },
  'cursor-agent': { resumeFlag: '--resume', provider: 'cursor' },
  'gemini': { resumeFlag: '--resume', provider: 'gemini' },
  // Pi uses `--session <uuid>` for deterministic resume — `--resume`
  // is its interactive picker (no id arg), so don't confuse them.
  'pi': { resumeFlag: '--session', provider: 'pi' },
  // Codex resume is a subcommand: `codex resume <uuid>`.
  'codex': { resumeSubcommand: 'resume', provider: 'codex' },
}

// ── Agent activity ────────────────────────────────────────────────
export const AGENT_IDLE_THRESHOLD_MS = 5000  // 5 seconds without grid output → idle

// ── Known agent commands ────────────────────────────────────────────
// CLI tools considered "active agents" for close-warning purposes.
// When one of these is the foreground process in a terminal, the user
// is warned before closing the tab or quitting the app.
export const KNOWN_AGENT_COMMANDS = new Set([
  // Cloud CLI agents
  'claude', 'cursor-agent', 'codex', 'gemini', 'copilot',
  'aider', 'opencode', 'gpt', 'goose', 'pi',
  // Local/on-device LLM tools
  'ollama', 'llamafile', 'llama-cli', 'interpreter',
  'tenere', 'llm-tui-rs',
])
