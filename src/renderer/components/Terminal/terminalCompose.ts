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
