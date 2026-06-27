// Composer Phase 1b — unit tests for the two load-bearing pure helpers:
//   1. Enter-vs-Shift+Enter send keybinding (`shouldSendOnKey`).
//   2. MsgResponse → status-lane mapping (`mapMsgResponseToStatus`).
// Both must fail loud — no silent fallthrough that renders a failure as
// "delivered".

import { describe, it, expect } from 'vitest'
import {
  type MsgResponse,
  composerPermitted,
  mapMsgResponseToStatus,
  shouldSendOnKey,
} from './terminalCompose'

// ── Enter = send, Shift+Enter = newline ──────────────────────────────

describe('shouldSendOnKey', () => {
  it('plain Enter sends', () => {
    expect(shouldSendOnKey({ key: 'Enter', shiftKey: false, isComposing: false })).toBe(true)
  })

  it('Shift+Enter does NOT send (newline)', () => {
    expect(shouldSendOnKey({ key: 'Enter', shiftKey: true, isComposing: false })).toBe(false)
  })

  it('Enter mid-IME-composition does NOT send', () => {
    expect(shouldSendOnKey({ key: 'Enter', shiftKey: false, isComposing: true })).toBe(false)
  })

  it('a non-Enter key never sends', () => {
    expect(shouldSendOnKey({ key: 'a', shiftKey: false, isComposing: false })).toBe(false)
    expect(shouldSendOnKey({ key: 'Tab', shiftKey: false, isComposing: false })).toBe(false)
  })
})

// ── MsgResponse → status lane ────────────────────────────────────────

function resp(over: Partial<MsgResponse>): MsgResponse {
  return {
    success: false,
    target_session_id: null,
    attempts: 1,
    reason: null,
    hint: null,
    ...over,
  }
}

describe('mapMsgResponseToStatus', () => {
  it('success → delivered', () => {
    const s = mapMsgResponseToStatus(resp({ success: true, target_session_id: 'abc-123' }))
    expect(s.kind).toBe('delivered')
  })

  it('pty_died → pty_died (carries hint)', () => {
    const s = mapMsgResponseToStatus(resp({ reason: 'pty_died', hint: 'crashed' }))
    expect(s).toEqual({ kind: 'pty_died', hint: 'crashed' })
  })

  it('pty_stalled → pty_stalled (carries hint)', () => {
    const s = mapMsgResponseToStatus(resp({ reason: 'pty_stalled', hint: 'retry shortly' }))
    expect(s).toEqual({ kind: 'pty_stalled', hint: 'retry shortly' })
  })

  it('worker_join (dispatcher panic) → busy', () => {
    const s = mapMsgResponseToStatus(resp({ reason: 'worker_join', hint: 'join error' }))
    expect(s).toEqual({ kind: 'busy', reason: 'worker_join', hint: 'join error' })
  })

  it('an UNKNOWN failure reason collapses to busy — never silently delivered', () => {
    const s = mapMsgResponseToStatus(resp({ success: false, reason: 'some_future_code' }))
    expect(s.kind).toBe('busy')
    expect(s.kind).not.toBe('delivered')
  })

  it('a failure with a null reason still maps to busy (never delivered)', () => {
    const s = mapMsgResponseToStatus(resp({ success: false, reason: null }))
    expect(s.kind).toBe('busy')
  })
})

// ── Composer 1c (D4) — renderer-hide predicate ───────────────────────
// Mirrors the daemon's authorize_send_message gate. The composer shows iff
// the host is local (owner) OR the host opted into remote instruction. The
// daemon enforces the real gate; this is defense-in-depth only.

describe('composerPermitted', () => {
  it('owner (local host) is permitted even with remote-instruct OFF', () => {
    expect(composerPermitted({ isLocalHost: true, allowRemoteInstruct: false })).toBe(true)
  })

  it('owner (local host) is permitted with remote-instruct ON', () => {
    expect(composerPermitted({ isLocalHost: true, allowRemoteInstruct: true })).toBe(true)
  })

  it('remote host is HIDDEN by default (opt-in OFF — the safe default)', () => {
    expect(composerPermitted({ isLocalHost: false, allowRemoteInstruct: false })).toBe(false)
  })

  it('remote host is permitted once the app-level master opts in', () => {
    expect(composerPermitted({ isLocalHost: false, allowRemoteInstruct: true })).toBe(true)
  })

  // #67 — per-workspace refinement: a remote host with the app-level master
  // OFF is still permitted when the ACTIVE WORKSPACE opted in, and stays
  // hidden when neither the master nor the workspace opted in.
  it('remote host is permitted when the active workspace opts in (app-level off)', () => {
    expect(
      composerPermitted({ isLocalHost: false, allowRemoteInstruct: false, perWorkspaceAllow: true })
    ).toBe(true)
  })

  it('remote host is HIDDEN when neither the master nor the workspace opted in', () => {
    expect(
      composerPermitted({ isLocalHost: false, allowRemoteInstruct: false, perWorkspaceAllow: false })
    ).toBe(false)
  })

  it('omitting perWorkspaceAllow keeps the safe default (hidden) on a remote host', () => {
    expect(composerPermitted({ isLocalHost: false, allowRemoteInstruct: false })).toBe(false)
  })
})
