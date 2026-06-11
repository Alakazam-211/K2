/**
 * LLM CLI "working" detection via viewport text scanning.
 *
 * Claude Code, Codex, Gemini, Aider etc. each render a status line near
 * the bottom of their TUI while processing a request. The hint text in
 * that status line ("esc to interrupt", "Waiting for…", "Thinking…") is
 * the most stable signal — much more reliable than title-prefix glyphs,
 * which cycle rapidly and sometimes disappear mid-frame.
 *
 * We scan the last few rows of the rendered grid on each frame. If any
 * known hint appears → the pane is working. A short debounce window
 * handles the gap between frames (the hint isn't always present in every
 * single frame — e.g. during tool-call rendering it can blank out
 * briefly).
 *
 * Substrings are matched case-insensitively. They're chosen to be the
 * stable parts of each tool's status line — hint text rather than the
 * rotating verb/adjective, since verbs change across versions and hints
 * don't.
 */

export const WORKING_SIGNALS: readonly string[] = [
  'esc to interrupt',     // claude, codex
  'esc to cancel',        // gemini
  'waiting for ',         // aider ("Waiting for gpt-4o")
  'thinking...',          // goose, copilot (default), gemini fallback,
                          // pi-mono (defaultHiddenThinkingLabel),
                          // ollama reasoning models
  'pondering...',         // copilot
  'unravelling...',       // copilot
  'working...',           // opencode, pi-mono (defaultWorkingMessage)
  'agent is working',     // opencode (typed-mid-generation warning)
  ' is thinking...',      // catches "<model> is thinking..." patterns
  'planning next moves',  // cursor-agent
  'taking longer than expected', // cursor-agent (stall state)
  'loading...',           // llm-tui-rs (placeholder while streaming)
  '🤖: waiting',          // tenere ("🤖: Waiting <spinner>")
]

interface CompactLineLike {
  text: string
}

/**
 * Scan the last `windowRows` rows of a rendered grid for any working
 * signal. Returns true if any signal appears in the window.
 *
 * Gated by `displayOffset === 0` at the call site — if the user scrolled
 * up, the status line isn't in the visible grid so we can't detect it.
 */
export function detectWorkingSignal(
  lines: Map<number, CompactLineLike>,
  totalRows: number,
  windowRows = 15,
): boolean {
  const firstRow = Math.max(0, totalRows - windowRows)
  for (let r = firstRow; r < totalRows; r++) {
    const line = lines.get(r)
    if (!line?.text) continue
    const lower = line.text.toLowerCase()
    for (const sig of WORKING_SIGNALS) {
      if (lower.includes(sig)) return true
    }
  }
  return false
}
