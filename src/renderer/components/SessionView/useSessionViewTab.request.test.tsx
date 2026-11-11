// @vitest-environment jsdom
//
// An opener's one-shot view ask (K5 opens in the Thread): the pinned Chat of
// that workspace shows the asked view over "New agents open in" and over a
// remembered view, takes the ask once its conversation is known and
// remembers it; another workspace, a pane without a workspace, another
// server's room and a stale ask ignore it.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, renderHook } from '@testing-library/react'

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: { activeHost: 'local' }) => unknown) => sel({ activeHost: 'local' }),
  activeHostKey: () => 'local',
}))

import { useSessionViewTab } from './useSessionViewTab'
import { LS_NEW_AGENT_VIEW } from './sessionViewTab'
import {
  SESSION_VIEW_REQUEST_TTL_MS,
  requestSessionView,
  sessionViewRequestFor,
  useSessionViewRequestStore,
} from '@/stores/session-view-request'

const PRIMARY = { isPrimary: true, hostKey: 'local' }

beforeEach(() => {
  localStorage.clear()
  useSessionViewRequestStore.setState({ pending: null })
  // This client opens new agents in Split view.
  localStorage.setItem(LS_NEW_AGENT_VIEW, JSON.stringify({ view: 'split', left: 'terminal', right: 'thread' }))
})

afterEach(() => cleanup())

describe('a view ask for a workspace', () => {
  it('a new agent opens in the asked view, not "New agents open in", and remembers it', () => {
    requestSessionView('ws-k5', 'thread')
    const hook = renderHook(({ conv }: { conv: string | null }) => useSessionViewTab(conv, PRIMARY, 'ws-k5'), {
      initialProps: { conv: null as string | null },
    })
    // Shown at once, before the conversation is known; still asked for.
    expect(hook.result.current.viewTab).toBe('thread')
    expect(useSessionViewRequestStore.getState().pending?.projectId).toBe('ws-k5')
    hook.rerender({ conv: 'conv-k5' })
    expect(hook.result.current.viewTab).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-tab:conv-k5')).toBe('thread')
    expect(useSessionViewRequestStore.getState().pending).toBeNull()
  })

  it('wins over a remembered view on an open pane, once', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-k5', 'terminal')
    const hook = renderHook(() => useSessionViewTab('conv-k5', PRIMARY, 'ws-k5'))
    expect(hook.result.current.viewTab).toBe('terminal')
    act(() => requestSessionView('ws-k5', 'thread'))
    expect(hook.result.current.viewTab).toBe('thread')
    expect(useSessionViewRequestStore.getState().pending).toBeNull()
    // Taken: the person's next pick sticks.
    act(() => hook.result.current.setViewTab('split'))
    expect(hook.result.current.viewTab).toBe('split')
  })

  it("another workspace's pane, a pane with no workspace and another server's room ignore it", () => {
    requestSessionView('ws-k5', 'thread')
    expect(renderHook(() => useSessionViewTab('conv-x', PRIMARY, 'ws-other')).result.current.viewTab).toBe('split')
    expect(renderHook(() => useSessionViewTab('conv-y', PRIMARY)).result.current.viewTab).toBe('split')
    expect(
      renderHook(() => useSessionViewTab('conv-z', { isPrimary: false, hostKey: 'box.k2.dev' }, 'ws-k5')).result.current
        .viewTab,
    ).toBe('split')
    expect(useSessionViewRequestStore.getState().pending?.projectId).toBe('ws-k5')
  })

  it('goes stale', () => {
    const pending = { projectId: 'ws-k5', view: 'thread' as const, at: 1_000 }
    expect(sessionViewRequestFor(pending, 'ws-k5', 1_000 + SESSION_VIEW_REQUEST_TTL_MS)).toBe(pending)
    expect(sessionViewRequestFor(pending, 'ws-k5', 1_001 + SESSION_VIEW_REQUEST_TTL_MS)).toBeNull()
    expect(sessionViewRequestFor(pending, null, 1_000)).toBeNull()
  })
})
