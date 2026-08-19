// Composer Phase 1b — pure, dependency-free helpers for the compose bar.
// Kept in their own module (no React, no daemon client, no stores) so the
// keybinding + MsgResponse→status mapping are unit-testable in isolation
// (fail-loud) without dragging in the host-aware fetch client.

// ── Wire shape — mirrors `crates/k2-daemon/src/workspace_msg.rs::MsgResponse`
// (the canonical `{success, target_session_id, attempts, reason, hint}`
// JSON). The send-message route returns `success` / `pty_died` /
// `pty_stalled` from `send_message_to_session`, plus a `worker_join`
// failure shape if the blocking task panics (dispatcher.rs).
export interface MsgResponse {
  success: boolean
  target_session_id: string | null
  attempts: number
  reason: string | null
  hint: string | null
}

// ── Status lane state ────────────────────────────────────────────────
// Drives the small status row beneath the textarea:
//   injecting… → delivered ✓ / pty_died ⚠ / pty_stalled ⚠ (try again) / busy ⚠
export type ComposeStatus =
  | { kind: 'idle' }
  | { kind: 'injecting' }
  | { kind: 'delivered' }
  | { kind: 'pty_died'; hint: string | null }
  | { kind: 'pty_stalled'; hint: string | null }
  | { kind: 'busy'; reason: string | null; hint: string | null }
  | { kind: 'error'; message: string }

/**
 * Map a daemon `MsgResponse` onto the status-lane state. Pure + exported
 * so the mapping is unit-testable without rendering the component.
 *
 * `success` → delivered. Otherwise dispatch on the canonical reason code;
 * anything that is not `pty_died`/`pty_stalled` (e.g. the dispatcher's
 * `worker_join`) collapses to the generic `busy` lane so a new/unknown
 * reason code can never silently render as "delivered".
 */
export function mapMsgResponseToStatus(resp: MsgResponse): ComposeStatus {
  if (resp.success) return { kind: 'delivered' }
  switch (resp.reason) {
    case 'pty_died':
      return { kind: 'pty_died', hint: resp.hint }
    case 'pty_stalled':
      return { kind: 'pty_stalled', hint: resp.hint }
    default:
      return { kind: 'busy', reason: resp.reason, hint: resp.hint }
  }
}

/**
 * Enter = send, Shift+Enter = newline. Pure + exported so the keybinding
 * is unit-testable without a DOM. IME composition (`isComposing`) must
 * NOT submit — a CJK candidate-commit Enter is part of composing the
 * text, not a send.
 */
export function shouldSendOnKey(e: {
  key: string
  shiftKey: boolean
  isComposing: boolean
}): boolean {
  return e.key === 'Enter' && !e.shiftKey && !e.isComposing
}

/**
 * Esc / Ctrl+C from the compose box cancel the agent's current turn
 * by injecting the same PTY bytes the terminal would send. Compose
 * stays focused — no focus flip. Cmd+C is copy (not interrupt).
 * Mid-IME composition must not fire.
 */
export function composeInterruptSequence(e: {
  key: string
  ctrlKey: boolean
  metaKey: boolean
  altKey: boolean
  isComposing: boolean
}): string | null {
  if (e.isComposing) return null
  if (e.key === 'Escape' && !e.ctrlKey && !e.metaKey && !e.altKey) return '\x1b'
  if ((e.key === 'c' || e.key === 'C') && e.ctrlKey && !e.metaKey && !e.altKey) {
    return '\x03'
  }
  return null
}

/**
 * Composer 1c (D4) + #67 per-workspace — renderer-hide predicate. Mirrors
 * the DAEMON's capability gate (`authorize_send_message` +
 * `remote_instruct_opt_in_for_session`): the composer is permitted iff
 *   • the active host is LOCAL (the owner is always allowed), OR
 *   • the app-level `allowRemoteInstruct` master is ON (global, back-compat), OR
 *   • the ACTIVE WORKSPACE opted into remote instruction
 *     (`perWorkspaceAllow`, default OFF).
 *
 * This is DEFENSE-IN-DEPTH only — the daemon rejects an unauthorized send
 * with a 403 regardless of what the renderer shows, and enforces the
 * decision PER-WORKSPACE server-side. A connect-user's role is never below
 * Member (Member is the floor), so the renderer needs no role input; the
 * daemon enforces the `>= Member` floor and a future sub-Member role slots
 * in there. Pure + exported so the mapping is unit-testable without
 * mounting the component.
 */
export function composerPermitted(_input: {
  isLocalHost: boolean
  allowRemoteInstruct: boolean
  perWorkspaceAllow?: boolean
}): boolean {
  // The composer ALWAYS renders — including on remote hosts. Authorization is
  // enforced SERVER-SIDE: the daemon 403s an unauthorized send regardless of
  // what the renderer shows (the owner is always allowed; a connect-user is
  // gated per-workspace). Hiding the bar on remote just made it vanish, which
  // was confusing — so we keep it visible and let the daemon be the single
  // gate. (Rosson 2026-06-27: leave the composer enabled by default on remote
  // machines.) Input kept for call-site stability + the unit-test contract.
  return true
}

/**
 * Whether TerminalPane should mount the agent compose bar.
 *
 * Mount on the first paint (idle / spawning / connecting / ready / error)
 * so measure-first spawn sees the bar's real height — it is not a fixed
 * px (editor font size changes the row). Hide only on exited: the PTY
 * is gone. Send stays disabled until `sessionId` exists (the bar chrome
 * is still there). Soft-resync keeps the bar mounted (`ready` →
 * `connecting`) so focus/draft are not interrupted.
 */
export function shouldShowTerminalComposeBar(phase: {
  kind: string
  sessionId?: string
}): boolean {
  return phase.kind !== 'exited'
}

/** Wire shape of `GET /cli/terminal/compose-history`. */
export interface ComposeHistoryItem {
  id: string
  body: string
  author: string
  created_at: number
}

/**
 * Compose-bar send-history key. ArrowUp recalls only when the caret is
 * collapsed at offset 0. Mid-draft Up returns null so the caret can
 * move. ArrowDown always reports `newer`; the caller no-ops at draft
 * index `-1` (does not preventDefault).
 */
/** localStorage key for the compose-bar caret (per PTY session). */
export function composeCaretStorageKey(sessionId: string): string {
  return `k2:composer:caret:${sessionId}`
}

export function clampComposeCaret(offset: number, textLen: number): number {
  if (!Number.isFinite(offset)) return textLen
  return Math.max(0, Math.min(textLen, Math.floor(offset)))
}

/** Missing/invalid caret → end of text (keep typing). Always clamped. */
export function readComposeCaret(
  sessionId: string,
  textLen: number,
): { start: number; end: number } {
  const fallback = { start: textLen, end: textLen }
  if (!sessionId) return fallback
  try {
    const raw = localStorage.getItem(composeCaretStorageKey(sessionId))
    if (!raw) return fallback
    const parsed = JSON.parse(raw) as { start?: unknown; end?: unknown }
    const start = clampComposeCaret(Number(parsed.start), textLen)
    const end = clampComposeCaret(Number(parsed.end), textLen)
    return { start, end: Math.max(start, end) }
  } catch {
    return fallback
  }
}

export function writeComposeCaret(
  sessionId: string,
  start: number,
  end: number,
  textLen: number,
): void {
  if (!sessionId) return
  try {
    const s = clampComposeCaret(start, textLen)
    const e = clampComposeCaret(end, textLen)
    localStorage.setItem(
      composeCaretStorageKey(sessionId),
      JSON.stringify({ start: s, end: Math.max(s, e) }),
    )
  } catch {
    /* storage disabled */
  }
}

export function clearComposeCaret(sessionId: string): void {
  if (!sessionId) return
  try {
    localStorage.removeItem(composeCaretStorageKey(sessionId))
  } catch {
    /* storage disabled */
  }
}

export function composeHistoryKeyAction(input: {
  key: string
  selectionStart: number | null
  selectionEnd: number | null
}): 'older' | 'newer' | null {
  if (input.key === 'ArrowUp') {
    if (input.selectionStart === 0 && input.selectionEnd === 0) return 'older'
    return null
  }
  if (input.key === 'ArrowDown') return 'newer'
  return null
}

/**
 * Walk compose send history. `index` `-1` is the pre-Up draft;
 * `0` is newest. `items` is newest-first. `draft` is the stashed
 * pre-Up text (or empty) restored when walking back to `-1`.
 */
export function applyComposeHistoryNav(input: {
  action: 'older' | 'newer'
  index: number
  draft: string
  items: readonly string[]
}): { index: number; text: string; preventDefault: boolean } {
  const { action, index, draft, items } = input
  if (items.length === 0) {
    return { index: -1, text: draft, preventDefault: false }
  }
  if (action === 'older') {
    const next = index < 0 ? 0 : Math.min(index + 1, items.length - 1)
    return { index: next, text: items[next] ?? draft, preventDefault: true }
  }
  if (index < 0) {
    return { index: -1, text: draft, preventDefault: false }
  }
  if (index === 0) {
    return { index: -1, text: draft, preventDefault: true }
  }
  const next = index - 1
  return { index: next, text: items[next] ?? draft, preventDefault: true }
}

/** Cap before the compose textarea scrolls internally. */
export const COMPOSE_TEXTAREA_MAX_HEIGHT = 160

/**
 * Pixel height for the compose textarea. Empty draft stays one line —
 * never let the placeholder wrap drive `scrollHeight` (cold start /
 * hidden tabs / narrow first layout inflate to the max cap).
 */
export function composeTextareaHeight(opts: {
  value: string
  scrollHeight: number
  fontSize: number
  maxHeight?: number
}): number {
  const font = opts.fontSize > 0 ? opts.fontSize : 12
  const singleLine = Math.round(font * 1.4 + 8)
  const cap = opts.maxHeight ?? COMPOSE_TEXTAREA_MAX_HEIGHT
  if (!opts.value) return singleLine
  return Math.min(Math.max(opts.scrollHeight, singleLine), cap)
}
