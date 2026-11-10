// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: { activeHost: 'local' }) => unknown) =>
    sel({ activeHost: 'local' }),
  activeHostKey: () => 'local',
}))

import { useSessionViewTab } from './useSessionViewTab'

const PRIMARY = { isPrimary: true, hostKey: 'local' }

describe('useSessionViewTab', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('remembers the revealed terminal per conversation, hidden by default', () => {
    const a = renderHook(() => useSessionViewTab('conv-a', PRIMARY))
    expect(a.result.current.terminalRevealed).toBe(false)
    act(() => a.result.current.setTerminalRevealed(true))
    expect(a.result.current.terminalRevealed).toBe(true)
    expect(localStorage.getItem('local|k2:session-view-terminal:conv-a')).toBe('1')
    a.unmount()

    const again = renderHook(() => useSessionViewTab('conv-a', PRIMARY))
    expect(again.result.current.terminalRevealed).toBe(true)
    const other = renderHook(() => useSessionViewTab('conv-b', PRIMARY))
    expect(other.result.current.terminalRevealed).toBe(false)
  })

  it('moving to another conversation reads its memory in the same render', () => {
    localStorage.setItem('local|k2:session-view-terminal:conv-a', '1')
    const seen: Array<[string, boolean]> = []
    const hook = renderHook(
      ({ key }: { key: string }) => {
        const v = useSessionViewTab(key, PRIMARY)
        seen.push([key, v.terminalRevealed])
        return v
      },
      { initialProps: { key: 'conv-a' } },
    )
    expect(hook.result.current.terminalRevealKey).toBe('local|k2:session-view-terminal:conv-a')
    hook.rerender({ key: 'conv-b' })
    expect(hook.result.current.terminalRevealed).toBe(false)
    expect(hook.result.current.terminalRevealKey).toBe('local|k2:session-view-terminal:conv-b')
    // The commit for conv-b never carried conv-a's open state.
    expect(seen.at(-1)).toEqual(['conv-b', false])
  })

  it('keeps the memory per server', () => {
    const a = renderHook(() => useSessionViewTab('conv-a', { isPrimary: false, hostKey: 'box.k2.dev' }))
    act(() => a.result.current.setTerminalRevealed(true))
    expect(localStorage.getItem('box.k2.dev|k2:session-view-terminal:conv-a')).toBe('1')
    expect(localStorage.getItem('local|k2:session-view-terminal:conv-a')).toBeNull()
  })

  it('remembers the view and both split sides per session', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-2', 'split')
    const first = renderHook(() => useSessionViewTab('conv-2', PRIMARY))
    expect(first.result.current.viewTab).toBe('split')
    expect(first.result.current.splitLeft).toBe('terminal')
    expect(first.result.current.splitRight).toBe('thread')

    act(() => {
      first.result.current.setSplitLeft('chatter')
    })
    expect(first.result.current.splitLeft).toBe('chatter')
    expect(first.result.current.splitRight).toBe('thread')

    act(() => {
      first.result.current.setSplitRight('chatter')
    })
    expect(first.result.current.splitLeft).toBe('chatter')
    expect(first.result.current.splitRight).toBe('chatter')

    first.unmount()
    const again = renderHook(() => useSessionViewTab('conv-2', PRIMARY))
    expect(again.result.current.viewTab).toBe('split')
    expect(again.result.current.splitLeft).toBe('chatter')
    expect(again.result.current.splitRight).toBe('chatter')
    // A conversation never shown here gets the default: Split view.
    expect(renderHook(() => useSessionViewTab('conv-other', PRIMARY)).result.current.viewTab).toBe('split')
  })
})

describe('useSessionViewTab — New agents open in', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('a conversation with no remembered view opens in the chosen view and remembers it', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'thread', left: 'terminal', right: 'thread' }))
    const hook = renderHook(() => useSessionViewTab('conv-new', PRIMARY))
    expect(hook.result.current.viewTab).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-tab:conv-new')).toBe('thread')

    // A later change to the setting does not move it.
    hook.unmount()
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'chatter', left: 'terminal', right: 'thread' }))
    expect(renderHook(() => useSessionViewTab('conv-new', PRIMARY)).result.current.viewTab).toBe('thread')
    expect(renderHook(() => useSessionViewTab('conv-newer', PRIMARY)).result.current.viewTab).toBe('chatter')
  })

  it('a pane still waiting for its conversation id already shows the chosen view', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'thread', left: 'terminal', right: 'thread' }))
    const hook = renderHook(({ key }) => useSessionViewTab(key, PRIMARY), {
      initialProps: { key: null as string | null },
    })
    expect(hook.result.current.viewTab).toBe('thread')
    hook.rerender({ key: 'conv-late' })
    expect(hook.result.current.viewTab).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-tab:conv-late')).toBe('thread')
  })

  it('Split view opens with the chosen sides', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'split', left: 'thread', right: 'chatter' }))
    const hook = renderHook(() => useSessionViewTab('conv-split', PRIMARY))
    expect(hook.result.current.viewTab).toBe('split')
    expect(hook.result.current.splitLeft).toBe('thread')
    expect(hook.result.current.splitRight).toBe('chatter')
    expect(JSON.parse(localStorage.getItem('local|k2:session-view-split:conv-split') ?? 'null')).toEqual({
      left: 'thread',
      right: 'chatter',
    })
  })

  it('the sides are kept for a later pick of Split even when the view is not Split', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'terminal', left: 'chatter', right: 'thread' }))
    const hook = renderHook(() => useSessionViewTab('conv-term', PRIMARY))
    expect(hook.result.current.viewTab).toBe('terminal')
    act(() => {
      hook.result.current.setViewTab('split')
    })
    expect(hook.result.current.splitLeft).toBe('chatter')
    expect(hook.result.current.splitRight).toBe('thread')
  })

  it('a conversation with a remembered view keeps it, sides included', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'split', left: 'thread', right: 'chatter' }))
    localStorage.setItem('local|k2:session-view-tab:conv-old', 'terminal')
    expect(renderHook(() => useSessionViewTab('conv-old', PRIMARY)).result.current.viewTab).toBe('terminal')

    localStorage.setItem('local|k2:session-view-tab:conv-old-split', 'split')
    localStorage.setItem('local|k2:session-view-split:conv-old-split', JSON.stringify({ left: 'chatter', right: 'terminal' }))
    const kept = renderHook(() => useSessionViewTab('conv-old-split', PRIMARY))
    expect(kept.result.current.viewTab).toBe('split')
    expect(kept.result.current.splitLeft).toBe('chatter')
    expect(kept.result.current.splitRight).toBe('terminal')

    // Old memory: Split with no side record keeps Terminal | Thread.
    localStorage.setItem('local|k2:session-view-tab:conv-legacy', 'split')
    const legacy = renderHook(() => useSessionViewTab('conv-legacy', PRIMARY))
    expect(legacy.result.current.splitLeft).toBe('terminal')
    expect(legacy.result.current.splitRight).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-split:conv-legacy')).toBeNull()
  })

  it('with no setting, new conversations open in Split view: Terminal left, Thread right (0.46.10)', () => {
    const hook = renderHook(() => useSessionViewTab('conv-plain', PRIMARY))
    expect(hook.result.current.viewTab).toBe('split')
    expect(hook.result.current.splitLeft).toBe('terminal')
    expect(hook.result.current.splitRight).toBe('thread')
    // Remembered on first show, so a later setting change does not move it.
    expect(localStorage.getItem('local|k2:session-view-tab:conv-plain')).toBe('split')
    expect(JSON.parse(localStorage.getItem('local|k2:session-view-split:conv-plain') ?? 'null')).toEqual({
      left: 'terminal',
      right: 'thread',
    })
  })

  it('with no setting, a pane still waiting for its conversation id already shows Split view', () => {
    const hook = renderHook(({ key }) => useSessionViewTab(key, PRIMARY), {
      initialProps: { key: null as string | null },
    })
    expect(hook.result.current.viewTab).toBe('split')
    expect(hook.result.current.splitLeft).toBe('terminal')
    expect(hook.result.current.splitRight).toBe('thread')
  })

  it('the new default leaves saved views alone: Terminal, Thread and a saved Split stay as saved', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-was-terminal', 'terminal')
    localStorage.setItem('local|k2:session-view-tab:conv-was-thread', 'thread')
    localStorage.setItem('local|k2:session-view-tab:conv-was-split', 'split')
    localStorage.setItem(
      'local|k2:session-view-split:conv-was-split',
      JSON.stringify({ left: 'thread', right: 'chatter' }),
    )
    expect(renderHook(() => useSessionViewTab('conv-was-terminal', PRIMARY)).result.current.viewTab).toBe('terminal')
    expect(renderHook(() => useSessionViewTab('conv-was-thread', PRIMARY)).result.current.viewTab).toBe('thread')
    const split = renderHook(() => useSessionViewTab('conv-was-split', PRIMARY))
    expect(split.result.current.viewTab).toBe('split')
    expect(split.result.current.splitLeft).toBe('thread')
    expect(split.result.current.splitRight).toBe('chatter')
    // Nothing rewritten.
    expect(localStorage.getItem('local|k2:session-view-tab:conv-was-terminal')).toBe('terminal')
    expect(localStorage.getItem('local|k2:session-view-tab:conv-was-thread')).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-split:conv-was-split')).toBe(
      JSON.stringify({ left: 'thread', right: 'chatter' }),
    )
  })

  it('a saved "New agents open in: Terminal" keeps winning over the new default', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'terminal', left: 'terminal', right: 'thread' }))
    expect(renderHook(() => useSessionViewTab('conv-pref-terminal', PRIMARY)).result.current.viewTab).toBe('terminal')
    expect(localStorage.getItem('k2.newAgentView')).toBe(
      JSON.stringify({ view: 'terminal', left: 'terminal', right: 'thread' }),
    )
  })
})

// The Chat view was deleted in 0.46.5; a saved Chat view or default opens
// the agent's Thread instead (Rosson 2026-10-27), and the fix is written back.
describe('useSessionViewTab — saved Chat view migrates to Threads (0.46.5)', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('a pane saved in chat mode opens the Thread and the saved view is rewritten', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-chat', 'chat')
    const hook = renderHook(() => useSessionViewTab('conv-chat', PRIMARY))
    expect(hook.result.current.viewTab).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-tab:conv-chat')).toBe('thread')
    expect(JSON.parse(localStorage.getItem('local|k2:session-view-split:conv-chat') ?? 'null')).toEqual({
      left: 'terminal',
      right: 'thread',
    })
  })

  it('a split saved with a Chat side opens the Thread beside the terminal, rewritten', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-split-chat', 'split')
    localStorage.setItem(
      'local|k2:session-view-split:conv-split-chat',
      JSON.stringify({ left: 'chat', right: 'terminal' }),
    )
    const hook = renderHook(() => useSessionViewTab('conv-split-chat', PRIMARY))
    expect(hook.result.current.viewTab).toBe('split')
    expect(hook.result.current.splitLeft).toBe('thread')
    expect(hook.result.current.splitRight).toBe('terminal')
    expect(localStorage.getItem('local|k2:session-view-tab:conv-split-chat')).toBe('split')
    expect(JSON.parse(localStorage.getItem('local|k2:session-view-split:conv-split-chat') ?? 'null')).toEqual({
      left: 'thread',
      right: 'terminal',
    })
  })

  it('a new agent whose "New agents open in" pick was Chat opens the Thread, and the pick is rewritten', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'chat', left: 'terminal', right: 'thread' }))
    const hook = renderHook(() => useSessionViewTab('conv-new-chat', PRIMARY))
    expect(hook.result.current.viewTab).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-tab:conv-new-chat')).toBe('thread')
    expect(JSON.parse(localStorage.getItem('k2.newAgentView') ?? 'null')).toEqual({
      view: 'thread',
      left: 'terminal',
      right: 'thread',
    })
  })

  it('the setters never store chat, even from an untyped caller', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-set', 'split')
    const hook = renderHook(() => useSessionViewTab('conv-set', PRIMARY))
    const legacy = 'chat' as unknown as 'thread'
    act(() => hook.result.current.setSplitLeft(legacy))
    act(() => hook.result.current.setViewTab(legacy))
    expect(localStorage.getItem('local|k2:session-view-tab:conv-set')).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-split:conv-set')).not.toContain('"chat"')
    expect(JSON.parse(localStorage.getItem('local|k2:session-view-split:conv-set') ?? 'null')).toEqual({
      left: 'terminal',
      right: 'thread',
    })
  })
})

describe('Thread view panels and split view', () => {
  it('an open Chatter or Heartbeats panel closes on the way to split view', () => {
    localStorage.clear()
    localStorage.setItem('local|k2:session-view-tab:conv-panel', 'thread')
    const hook = renderHook(() => useSessionViewTab('conv-panel', PRIMARY))
    act(() => hook.result.current.setThreadPanel('heartbeats'))
    expect(hook.result.current.threadPanel).toBe('heartbeats')
    act(() => hook.result.current.setViewTab('split'))
    expect(hook.result.current.viewTab).toBe('split')
    expect(hook.result.current.threadPanel).toBeNull()
    act(() => hook.result.current.setViewTab('thread'))
    expect(hook.result.current.threadPanel).toBeNull()
  })

  it('other views leave it as Thread view had it', () => {
    localStorage.clear()
    localStorage.setItem('local|k2:session-view-tab:conv-panel-2', 'thread')
    const hook = renderHook(() => useSessionViewTab('conv-panel-2', PRIMARY))
    act(() => hook.result.current.setThreadPanel('chatter'))
    act(() => hook.result.current.setViewTab('terminal'))
    expect(hook.result.current.threadPanel).toBe('chatter')
  })
})
