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
  'esc to interrupt',     // claude; codex CONFIRMED verbatim
                          // ("• Working (0s • esc to interrupt)",
                          // 2026-07 TUI signal study)
  'esc to cancel',        // gemini (unverified-but-kept — working phase
                          // not captured in the 2026-07 study; recapture
                          // pending auth fix)
  'esc:cancel',           // grok busy footer "Esc:cancel │
                          // Ctrl+.:shortcuts" — NO space before the
                          // colon, so 'esc to cancel' can't match
  'starting session…',    // grok startup line (U+2026 ellipsis)
  'thinking…',            // grok transcript "◆ Thinking…" (U+2026 —
                          // the ASCII 'thinking...' entry misses it)
  'msg=interrupt',        // hermes busy footer "msg=interrupt · /queue
                          // · /bg · /steer · Ctrl+C cancel" — its
                          // SINGLE stable busy signal (spinner verb +
                          // kaomoji rotate; no titles, no bell)
  'ctrl+c to stop',       // cursor-agent mid-turn input bar (best
                          // signal; also present during its two-step
                          // rejection-reason composer — still mid-turn)
  'waiting for ',         // aider ("Waiting for gpt-4o"); grok status
                          // row ("⠙ Waiting for response… 0.0s")
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

/**
 * Grok announces an open permission gate in the terminal TITLE — prefix
 * `⚠ Action Required - ` (2026-07 TUI signal study) — the cleanest HITL
 * signal any studied agent emits, and grok's ONLY permission source
 * (it has no lifecycle hooks). Matched against the raw OSC title in
 * TerminalPane's title handler; drives the same `permission` pane state
 * Claude's hook drives, and CLEARS it when the prefix goes away (see
 * `recordTitlePermission` in stores/active-agents.ts).
 */
export const GROK_PERMISSION_TITLE_RE = /^⚠ Action Required/

interface CompactLineLike {
  text: string
}

/**
 * Scan the last `windowRows` rows of a rendered grid for any working
 * signal. Returns true if any signal appears in the window.
 *
 * Deliberately NOT gated on `displayOffset === 0` at the call site —
 * the false-positive cost (showing 'working' while scrolled up) is much
 * smaller than the false-negative cost (no spinner ever). See the
 * call-site comment in TerminalPane's recordActivityFromSnapshot.
 *
 * Per-agent coverage note (2026-07 TUI signal studies): hermes emits NO
 * titles ever and no bell, so this phrase table + output cadence IS its
 * working detection — which is exactly the safe default every unknown
 * agent degrades to.
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
