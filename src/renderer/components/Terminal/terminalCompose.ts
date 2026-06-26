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
