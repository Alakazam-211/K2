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
  composeTextareaHeight,
  composeMessagePlaceholder,
  composeAgentNameFromProjects,
  extractImagePathsFromDraft,
  removePathFromDraft,
  COMPOSE_SLASH_COMMANDS,
  normalizeComposeSlashCommand,
  composeCanSend,
  composeSlashTypeaheadQuery,
  filterComposeSlashCommands,
  composeSlashMenuOpenFromDraft,
  consumeComposeSlashToken,
  composeSlashExactCommand,
  composeSlashSpaceCommit,
  composeSlashBackspaceClearsCommand,
  composeSlashMenuKeyAction,
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

describe('extractImagePathsFromDraft / removePathFromDraft', () => {
  it('finds quoted and bare image paths, skips pdf/txt', () => {
    expect(
      extractImagePathsFromDraft("see '/tmp/Screen Shot.png' and /opt/a.jpg notes.txt /x.pdf"),
    ).toEqual(['/tmp/Screen Shot.png', '/opt/a.jpg'])
  })

  it('removes a quoted path from the draft', () => {
    expect(removePathFromDraft("look at '/tmp/a.png' please", '/tmp/a.png')).toBe(
      'look at please',
    )
  })
})

describe('COMPOSE_SLASH_COMMANDS / normalizeComposeSlashCommand', () => {
  it('lists only /compact and /goal', () => {
    expect(COMPOSE_SLASH_COMMANDS.map((c) => c.command)).toEqual(['/compact', '/goal'])
  })

  it('normalizes optional slash and case to canonical forms', () => {
    expect(normalizeComposeSlashCommand('/compact')).toBe('/compact')
    expect(normalizeComposeSlashCommand('COMPACT')).toBe('/compact')
    expect(normalizeComposeSlashCommand('  /Compact  ')).toBe('/compact')
    expect(normalizeComposeSlashCommand('/GOAL')).toBe('/goal')
    expect(normalizeComposeSlashCommand('goal')).toBe('/goal')
  })

  it('empty / whitespace is no command', () => {
    expect(normalizeComposeSlashCommand('')).toBeNull()
    expect(normalizeComposeSlashCommand('   ')).toBeNull()
    expect(normalizeComposeSlashCommand(null)).toBeNull()
    expect(normalizeComposeSlashCommand(undefined)).toBeNull()
  })

  it('unknown commands are rejected (not normalized)', () => {
    expect(normalizeComposeSlashCommand('/exit')).toBeNull()
    expect(normalizeComposeSlashCommand('/loop')).toBeNull()
    expect(normalizeComposeSlashCommand('/compact now')).toBeNull()
    expect(normalizeComposeSlashCommand('garbage')).toBeNull()
  })
})

describe('composeSlashTypeaheadQuery', () => {
  it('opens only on a leading first-token slash', () => {
    expect(composeSlashTypeaheadQuery('/')).toBe('/')
    expect(composeSlashTypeaheadQuery('/comp')).toBe('/comp')
    expect(composeSlashTypeaheadQuery('/compact')).toBe('/compact')
    expect(composeSlashTypeaheadQuery('/c')).toBe('/c')
  })

  it('does not open mid-sentence, with a leading space, or after whitespace', () => {
    expect(composeSlashTypeaheadQuery('hello')).toBeNull()
    expect(composeSlashTypeaheadQuery(' /c')).toBeNull()
    expect(composeSlashTypeaheadQuery('/c more')).toBeNull()
    expect(composeSlashTypeaheadQuery('see /compact')).toBeNull()
    expect(composeSlashTypeaheadQuery('/compact please')).toBeNull()
    expect(composeSlashTypeaheadQuery('')).toBeNull()
  })
})

describe('filterComposeSlashCommands', () => {
  it('shows both commands for empty / lone slash', () => {
    expect(filterComposeSlashCommands('/').map((c) => c.command)).toEqual([
      '/compact',
      '/goal',
    ])
    expect(filterComposeSlashCommands('').map((c) => c.command)).toEqual([
      '/compact',
      '/goal',
    ])
  })

  it('filters by case-insensitive prefix, with or without repeating the slash', () => {
    expect(filterComposeSlashCommands('/c').map((c) => c.command)).toEqual(['/compact'])
    expect(filterComposeSlashCommands('/C').map((c) => c.command)).toEqual(['/compact'])
    expect(filterComposeSlashCommands('/g').map((c) => c.command)).toEqual(['/goal'])
    expect(filterComposeSlashCommands('comp').map((c) => c.command)).toEqual(['/compact'])
    expect(filterComposeSlashCommands('/comp').map((c) => c.command)).toEqual(['/compact'])
  })

  it('unknown prefixes including /exit yield no matches', () => {
    expect(filterComposeSlashCommands('/x')).toEqual([])
    expect(filterComposeSlashCommands('/exit')).toEqual([])
    expect(filterComposeSlashCommands('/e')).toEqual([])
  })
})

describe('composeSlashMenuOpenFromDraft', () => {
  it('is open for / and matching prefixes, closed for /x and non-queries', () => {
    expect(composeSlashMenuOpenFromDraft('/')).toBe(true)
    expect(composeSlashMenuOpenFromDraft('/c')).toBe(true)
    expect(composeSlashMenuOpenFromDraft('/x')).toBe(false)
    expect(composeSlashMenuOpenFromDraft('/exit')).toBe(false)
    expect(composeSlashMenuOpenFromDraft('see /compact')).toBe(false)
    expect(composeSlashTypeaheadQuery('/tmp/foo')).toBe('/tmp/foo')
    expect(composeSlashMenuOpenFromDraft('/tmp/foo')).toBe(false)
  })
})

describe('consumeComposeSlashToken', () => {
  it('consumes the first /token and trims leftover leading space', () => {
    expect(consumeComposeSlashToken('/c')).toBe('')
    expect(consumeComposeSlashToken('/compact')).toBe('')
    expect(consumeComposeSlashToken('/compact please')).toBe('please')
    expect(consumeComposeSlashToken('/compact  please')).toBe('please')
    expect(consumeComposeSlashToken('/goal hello')).toBe('hello')
  })

  it('leaves a non-slash draft untouched', () => {
    expect(consumeComposeSlashToken('hello')).toBe('hello')
    expect(consumeComposeSlashToken('')).toBe('')
    expect(consumeComposeSlashToken('see /compact')).toBe('see /compact')
  })
})

describe('composeSlashExactCommand / space-commit', () => {
  it('returns the canonical command only for an exact unique token', () => {
    expect(composeSlashExactCommand('/compact')).toBe('/compact')
    expect(composeSlashExactCommand('/goal')).toBe('/goal')
    expect(composeSlashExactCommand('/COMPACT')).toBe('/compact')
    expect(composeSlashExactCommand('/compact please')).toBe('/compact')
    expect(composeSlashExactCommand('/c')).toBeNull()
    expect(composeSlashExactCommand('/')).toBeNull()
    expect(composeSlashExactCommand('/exit')).toBeNull()
  })

  it('space-commits exact /compact or /goal and leaves the remainder', () => {
    expect(composeSlashSpaceCommit('/compact ')).toEqual({
      command: '/compact',
      remainder: '',
    })
    expect(composeSlashSpaceCommit('/compact please')).toEqual({
      command: '/compact',
      remainder: 'please',
    })
    expect(composeSlashSpaceCommit('/goal hello')).toEqual({
      command: '/goal',
      remainder: 'hello',
    })
  })

  it('does not space-commit a non-exact prefix; caller leaves /c in the draft', () => {
    expect(composeSlashSpaceCommit('/c ')).toBeNull()
    expect(composeSlashSpaceCommit('/exit ')).toBeNull()
    expect(composeSlashSpaceCommit('/compact')).toBeNull()
  })
})

describe('composeSlashBackspaceClearsCommand', () => {
  it('clears when the draft is empty and a command is selected', () => {
    expect(
      composeSlashBackspaceClearsCommand({
        draft: '',
        command: '/compact',
        key: 'Backspace',
      }),
    ).toBe(true)
    expect(
      composeSlashBackspaceClearsCommand({
        draft: '',
        command: '/goal',
        key: 'Delete',
      }),
    ).toBe(true)
  })

  it('does not clear while there is still text, no command, or other keys', () => {
    expect(
      composeSlashBackspaceClearsCommand({
        draft: '/',
        command: '/compact',
        key: 'Backspace',
      }),
    ).toBe(false)
    expect(
      composeSlashBackspaceClearsCommand({
        draft: '',
        command: null,
        key: 'Backspace',
      }),
    ).toBe(false)
    expect(
      composeSlashBackspaceClearsCommand({
        draft: '',
        command: '/compact',
        key: 'Enter',
      }),
    ).toBe(false)
    expect(
      composeSlashBackspaceClearsCommand({
        draft: '',
        command: '/compact',
        key: 'Backspace',
        isComposing: true,
      }),
    ).toBe(false)
  })
})

describe('composeSlashMenuKeyAction', () => {
  it('Enter with matches selects (does not send)', () => {
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 2,
        highlight: 0,
        key: 'Enter',
      }),
    ).toEqual({ kind: 'select' })
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 1,
        highlight: 0,
        key: 'Enter',
        isComposing: true,
      }),
    ).toBeNull()
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 1,
        highlight: 0,
        key: 'Enter',
        shiftKey: true,
      }),
    ).toBeNull()
  })

  it('Enter with 0 matches or menu closed does not steal send', () => {
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 0,
        highlight: 0,
        key: 'Enter',
      }),
    ).toBeNull()
    expect(
      composeSlashMenuKeyAction({
        menuOpen: false,
        matchCount: 2,
        highlight: 0,
        key: 'Enter',
      }),
    ).toBeNull()
  })

  it('arrows move highlight with clamp and no wrap (not send-history)', () => {
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 2,
        highlight: 0,
        key: 'ArrowDown',
      }),
    ).toEqual({ kind: 'move', highlight: 1 })
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 2,
        highlight: 1,
        key: 'ArrowDown',
      }),
    ).toEqual({ kind: 'move', highlight: 1 })
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 2,
        highlight: 1,
        key: 'ArrowUp',
      }),
    ).toEqual({ kind: 'move', highlight: 0 })
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 2,
        highlight: 0,
        key: 'ArrowUp',
      }),
    ).toEqual({ kind: 'move', highlight: 0 })
    expect(
      composeSlashMenuKeyAction({
        menuOpen: false,
        matchCount: 2,
        highlight: 0,
        key: 'ArrowUp',
      }),
    ).toBeNull()
  })

  it('Escape closes the menu', () => {
    expect(
      composeSlashMenuKeyAction({
        menuOpen: true,
        matchCount: 2,
        highlight: 0,
        key: 'Escape',
      }),
    ).toEqual({ kind: 'close' })
  })
})

describe('composeCanSend', () => {
  it('empty draft + /compact can send', () => {
    expect(composeCanSend({ draft: '', sending: false, command: '/compact' })).toBe(true)
  })

  it('empty draft + no command cannot send', () => {
    expect(composeCanSend({ draft: '', sending: false })).toBe(false)
    expect(composeCanSend({ draft: '   ', sending: false, command: null })).toBe(false)
  })

  it('sending is never sendable', () => {
    expect(composeCanSend({ draft: 'hi', sending: true })).toBe(false)
    expect(composeCanSend({ draft: '', sending: true, command: '/compact' })).toBe(false)
  })

  it('non-empty draft can send without a command', () => {
    expect(composeCanSend({ draft: 'hi', sending: false })).toBe(true)
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

describe('composeTextareaHeight', () => {
  it('keeps an empty draft at one line even if placeholder scrollHeight is huge', () => {
    expect(composeTextareaHeight({ value: '', scrollHeight: 160, fontSize: 12 })).toBe(
      Math.round(12 * 1.4 + 8),
    )
    expect(composeTextareaHeight({ value: '', scrollHeight: 400, fontSize: 15 })).toBe(
      Math.round(15 * 1.4 + 8),
    )
  })

  it('grows with real content up to the cap', () => {
    expect(composeTextareaHeight({ value: 'hi', scrollHeight: 40, fontSize: 12 })).toBe(40)
    expect(composeTextareaHeight({ value: 'hi\n\n\n', scrollHeight: 400, fontSize: 12 })).toBe(160)
  })
})

describe('shouldShowTerminalComposeBar', () => {
  it('shows on ready with sessionId', () => {
    expect(
      shouldShowTerminalComposeBar({ kind: 'ready', sessionId: 'sess-abc' }),
    ).toBe(true)
  })

  it('shows on idle / spawning so first paint reserves bar height', () => {
    expect(shouldShowTerminalComposeBar({ kind: 'idle' })).toBe(true)
    expect(shouldShowTerminalComposeBar({ kind: 'spawning' })).toBe(true)
    expect(shouldShowTerminalComposeBar({ kind: 'connecting' })).toBe(true)
    expect(shouldShowTerminalComposeBar({ kind: 'error' })).toBe(true)
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
})

describe('composeMessagePlaceholder', () => {
  it('uses the workspace agent name', () => {
    expect(composeMessagePlaceholder('sales')).toBe('Message sales')
  })

  it('trims whitespace', () => {
    expect(composeMessagePlaceholder('  K2  ')).toBe('Message K2')
  })

  it('falls back when the name is missing', () => {
    expect(composeMessagePlaceholder('')).toBe('Message the agent')
    expect(composeMessagePlaceholder('   ')).toBe('Message the agent')
    expect(composeMessagePlaceholder(null)).toBe('Message the agent')
    expect(composeMessagePlaceholder(undefined)).toBe('Message the agent')
  })
})

describe('composeAgentNameFromProjects', () => {
  const projects = [
    {
      path: '/ws/sales',
      name: 'sales',
      workspaces: [{ worktreePath: '/ws/sales-wt' }],
    },
    { path: '/ws/k2', name: 'K2' },
  ]

  it('matches the project path', () => {
    expect(composeAgentNameFromProjects(projects, '/ws/sales')).toBe('sales')
  })

  it('matches a worktree path to the parent workspace name', () => {
    expect(composeAgentNameFromProjects(projects, '/ws/sales-wt')).toBe('sales')
  })

  it('returns empty when the path is unknown', () => {
    expect(composeAgentNameFromProjects(projects, '/ws/nope')).toBe('')
    expect(composeAgentNameFromProjects(projects, '')).toBe('')
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
