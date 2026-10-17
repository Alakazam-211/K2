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

  it('remembers the view and both split sides per session', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-2', 'split')
    const first = renderHook(() => useSessionViewTab('conv-2', PRIMARY))
    expect(first.result.current.viewTab).toBe('split')
    expect(first.result.current.splitLeft).toBe('terminal')
    expect(first.result.current.splitRight).toBe('thread')

    act(() => {
      first.result.current.setSplitLeft('chat')
    })
    expect(first.result.current.splitLeft).toBe('chat')
    expect(first.result.current.splitRight).toBe('thread')

    act(() => {
      first.result.current.setSplitRight('chat')
    })
    expect(first.result.current.splitLeft).toBe('chat')
    expect(first.result.current.splitRight).toBe('chat')

    first.unmount()
    const again = renderHook(() => useSessionViewTab('conv-2', PRIMARY))
    expect(again.result.current.viewTab).toBe('split')
    expect(again.result.current.splitLeft).toBe('chat')
    expect(again.result.current.splitRight).toBe('chat')
    expect(renderHook(() => useSessionViewTab('conv-other', PRIMARY)).result.current.viewTab).toBe('terminal')
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
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'terminal', left: 'chat', right: 'thread' }))
    const hook = renderHook(() => useSessionViewTab('conv-term', PRIMARY))
    expect(hook.result.current.viewTab).toBe('terminal')
    act(() => {
      hook.result.current.setViewTab('split')
    })
    expect(hook.result.current.splitLeft).toBe('chat')
    expect(hook.result.current.splitRight).toBe('thread')
  })

  it('a conversation with a remembered view keeps it, sides included', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'split', left: 'thread', right: 'chatter' }))
    localStorage.setItem('local|k2:session-view-tab:conv-old', 'terminal')
    expect(renderHook(() => useSessionViewTab('conv-old', PRIMARY)).result.current.viewTab).toBe('terminal')

    localStorage.setItem('local|k2:session-view-tab:conv-old-split', 'split')
    localStorage.setItem('local|k2:session-view-split:conv-old-split', JSON.stringify({ left: 'chat', right: 'terminal' }))
    const kept = renderHook(() => useSessionViewTab('conv-old-split', PRIMARY))
    expect(kept.result.current.viewTab).toBe('split')
    expect(kept.result.current.splitLeft).toBe('chat')
    expect(kept.result.current.splitRight).toBe('terminal')

    // Old memory: Split with no side record keeps Terminal | Thread.
    localStorage.setItem('local|k2:session-view-tab:conv-legacy', 'split')
    const legacy = renderHook(() => useSessionViewTab('conv-legacy', PRIMARY))
    expect(legacy.result.current.splitLeft).toBe('terminal')
    expect(legacy.result.current.splitRight).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-split:conv-legacy')).toBeNull()
  })

  it('with no setting, new conversations still open in Terminal', () => {
    const hook = renderHook(() => useSessionViewTab('conv-plain', PRIMARY))
    expect(hook.result.current.viewTab).toBe('terminal')
    expect(hook.result.current.splitLeft).toBe('terminal')
    expect(hook.result.current.splitRight).toBe('thread')
  })
})
