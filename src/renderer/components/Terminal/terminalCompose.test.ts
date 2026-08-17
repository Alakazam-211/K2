// @vitest-environment jsdom
// Composer Phase 1b — unit tests for the two load-bearing pure helpers:
//   1. Enter-vs-Shift+Enter send keybinding (`shouldSendOnKey`).
//   2. MsgResponse → status-lane mapping (`mapMsgResponseToStatus`).
// Both must fail loud — no silent fallthrough that renders a failure as
// "delivered".

import { describe, it, expect, beforeEach } from 'vitest'
import {
  type MsgResponse,
  applyComposeHistoryNav,
  composeHistoryKeyAction,
  composeInterruptSequence,
  composerPermitted,
  mapMsgResponseToStatus,
  shouldSendOnKey,
  shouldShowTerminalComposeBar,
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

// ── Esc / Ctrl+C from compose cancel the turn (PTY bytes) ───────────

describe('composeInterruptSequence', () => {
  const idle = { ctrlKey: false, metaKey: false, altKey: false, isComposing: false }

  it('Escape injects ESC', () => {
    expect(composeInterruptSequence({ ...idle, key: 'Escape' })).toBe('\x1b')
  })

  it('Ctrl+C injects ETX', () => {
    expect(composeInterruptSequence({ ...idle, key: 'c', ctrlKey: true })).toBe('\x03')
    expect(composeInterruptSequence({ ...idle, key: 'C', ctrlKey: true })).toBe('\x03')
  })

  it('Cmd+C is copy — not interrupt', () => {
    expect(composeInterruptSequence({ ...idle, key: 'c', metaKey: true })).toBeNull()
    expect(
      composeInterruptSequence({ ...idle, key: 'c', ctrlKey: true, metaKey: true }),
    ).toBeNull()
  })

  it('does not fire mid-IME composition', () => {
    expect(composeInterruptSequence({ ...idle, key: 'Escape', isComposing: true })).toBeNull()
    expect(
      composeInterruptSequence({ ...idle, key: 'c', ctrlKey: true, isComposing: true }),
    ).toBeNull()
  })

  it('plain letters and Enter are not interrupts', () => {
    expect(composeInterruptSequence({ ...idle, key: 'a' })).toBeNull()
    expect(composeInterruptSequence({ ...idle, key: 'Enter' })).toBeNull()
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
  // CONTRACT (2026-06-27): the composer ALWAYS renders. Authorization moved
  // entirely server-side — the daemon 403s an unauthorized send regardless of
  // what the renderer shows (owner always allowed; connect-user gated
  // per-workspace). The renderer no longer hides the bar (a vanishing composer
  // was confusing, and remote-instruct is the K2-as-a-server default).
  it('local host (owner) is permitted', () => {
    expect(composerPermitted({ isLocalHost: true, allowRemoteInstruct: false })).toBe(true)
  })

  it('remote host is permitted by default — composer no longer vanishes on remote', () => {
    expect(composerPermitted({ isLocalHost: false, allowRemoteInstruct: false })).toBe(true)
  })

  it('remote host is permitted even with every opt-in OFF (daemon is the gate)', () => {
    expect(
      composerPermitted({ isLocalHost: false, allowRemoteInstruct: false, perWorkspaceAllow: false })
    ).toBe(true)
  })

  it('remote host is permitted with the app-level master / per-workspace ON too', () => {
    expect(composerPermitted({ isLocalHost: false, allowRemoteInstruct: true })).toBe(true)
    expect(
      composerPermitted({ isLocalHost: false, allowRemoteInstruct: false, perWorkspaceAllow: true })
    ).toBe(true)
  })
})

import {
  clampComposeCaret,
  clearComposeCaret,
  composeCaretStorageKey,
  readComposeCaret,
  writeComposeCaret,
} from './terminalCompose'
import { insertIntoDraft } from './TerminalComposeBar'

describe('compose caret persistence', () => {
  const sid = 'sess-caret-test'

  beforeEach(() => {
    clearComposeCaret(sid)
  })

  it('defaults to the end of the draft when nothing is stored', () => {
    expect(readComposeCaret(sid, 8)).toEqual({ start: 8, end: 8 })
  })

  it('round-trips a mid-draft caret and clamps past the end', () => {
    writeComposeCaret(sid, 3, 5, 10)
    expect(localStorage.getItem(composeCaretStorageKey(sid))).toContain('3')
    expect(readComposeCaret(sid, 10)).toEqual({ start: 3, end: 5 })
    expect(readComposeCaret(sid, 4)).toEqual({ start: 3, end: 4 })
  })

  it('clamps negative / NaN offsets', () => {
    expect(clampComposeCaret(-2, 5)).toBe(0)
    expect(clampComposeCaret(99, 5)).toBe(5)
    expect(clampComposeCaret(Number.NaN, 5)).toBe(5)
  })
})

describe('insertIntoDraft', () => {
  it('appends with a separating space', () => {
    expect(insertIntoDraft('look at', '/tmp/a.txt ', null)).toBe('look at /tmp/a.txt ')
  })

  it('inserts at caret', () => {
    expect(insertIntoDraft('ab', '/x ', 1)).toBe('a /x b')
  })

  it('replaces empty draft', () => {
    expect(insertIntoDraft('', '/tmp/a.txt ', null)).toBe('/tmp/a.txt ')
  })
})

// ── Soft-resync: keep compose bar mounted while sessionId is known ───
// Soft-resync flips phase ready → connecting briefly. Unmounting the bar
// would drop focus even though the draft is localStorage-backed.

describe('shouldShowTerminalComposeBar', () => {
  it('shows on ready with sessionId', () => {
    expect(
      shouldShowTerminalComposeBar({ kind: 'ready', sessionId: 'sess-abc' }),
    ).toBe(true)
  })

  it('shows on connecting with sessionId (soft-resync reconnect)', () => {
    expect(
      shouldShowTerminalComposeBar({ kind: 'connecting', sessionId: 'sess-abc' }),
    ).toBe(true)
  })

  it('hides on exited even when sessionId is present', () => {
    expect(
      shouldShowTerminalComposeBar({ kind: 'exited', sessionId: 'sess-abc' }),
    ).toBe(false)
  })

  it('hides on idle / error / connecting without sessionId', () => {
    expect(shouldShowTerminalComposeBar({ kind: 'idle' })).toBe(false)
    expect(shouldShowTerminalComposeBar({ kind: 'error' })).toBe(false)
    expect(shouldShowTerminalComposeBar({ kind: 'connecting' })).toBe(false)
    expect(
      shouldShowTerminalComposeBar({ kind: 'ready', sessionId: '' }),
    ).toBe(false)
  })
})

// ── Compose send-history caret / key helper ──────────────────────────
// ArrowUp only when caret is collapsed at offset 0. Mid-draft Up must
// NOT claim the key. Down at index -1 restores the pre-Up draft.

describe('composeHistoryKeyAction', () => {
  it('ArrowUp at collapsed offset 0 recalls older', () => {
    expect(
      composeHistoryKeyAction({ key: 'ArrowUp', selectionStart: 0, selectionEnd: 0 }),
    ).toBe('older')
  })

  it('ArrowUp mid-draft does NOT recall (caret can move)', () => {
    expect(
      composeHistoryKeyAction({ key: 'ArrowUp', selectionStart: 3, selectionEnd: 3 }),
    ).toBeNull()
  })

  it('ArrowUp with a selection at 0 does NOT recall', () => {
    expect(
      composeHistoryKeyAction({ key: 'ArrowUp', selectionStart: 0, selectionEnd: 4 }),
    ).toBeNull()
  })

  it('ArrowDown reports newer (caller no-ops at draft index -1)', () => {
    expect(
      composeHistoryKeyAction({ key: 'ArrowDown', selectionStart: 0, selectionEnd: 0 }),
    ).toBe('newer')
    expect(
      composeHistoryKeyAction({ key: 'ArrowDown', selectionStart: 2, selectionEnd: 2 }),
    ).toBe('newer')
  })

  it('non-arrow keys are ignored', () => {
    expect(
      composeHistoryKeyAction({ key: 'Enter', selectionStart: 0, selectionEnd: 0 }),
    ).toBeNull()
  })
})

describe('applyComposeHistoryNav', () => {
  const items = ['newest', 'older', 'oldest']

  it('first Up from draft index -1 shows newest', () => {
    const next = applyComposeHistoryNav({
      action: 'older',
      index: -1,
      draft: 'wip',
      items,
    })
    expect(next).toEqual({ index: 0, text: 'newest', preventDefault: true })
  })

  it('further Up walks older, then pins at oldest', () => {
    const mid = applyComposeHistoryNav({
      action: 'older',
      index: 0,
      draft: 'wip',
      items,
    })
    expect(mid).toEqual({ index: 1, text: 'older', preventDefault: true })
    const end = applyComposeHistoryNav({
      action: 'older',
      index: 2,
      draft: 'wip',
      items,
    })
    expect(end).toEqual({ index: 2, text: 'oldest', preventDefault: true })
  })

  it('Down from newest restores the pre-Up draft', () => {
    const next = applyComposeHistoryNav({
      action: 'newer',
      index: 0,
      draft: 'wip',
      items,
    })
    expect(next).toEqual({ index: -1, text: 'wip', preventDefault: true })
  })

  it('Down at draft index -1 does not preventDefault', () => {
    const next = applyComposeHistoryNav({
      action: 'newer',
      index: -1,
      draft: 'wip',
      items,
    })
    expect(next.preventDefault).toBe(false)
    expect(next.index).toBe(-1)
    expect(next.text).toBe('wip')
  })

  it('empty history never claims the key', () => {
    const up = applyComposeHistoryNav({
      action: 'older',
      index: -1,
      draft: '',
      items: [],
    })
    expect(up.preventDefault).toBe(false)
    expect(up.index).toBe(-1)
  })
})
