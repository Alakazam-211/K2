// @vitest-environment jsdom
//
// An opener's one-shot view ask (Kessel opens in the Thread): the pinned Chat of
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
    requestSessionView('ws-kessel', 'thread')
    const hook = renderHook(({ conv }: { conv: string | null }) => useSessionViewTab(conv, PRIMARY, 'ws-kessel'), {
      initialProps: { conv: null as string | null },
    })
    // Shown at once, before the conversation is known; still asked for.
    expect(hook.result.current.viewTab).toBe('thread')
    expect(useSessionViewRequestStore.getState().pending?.projectId).toBe('ws-kessel')
    hook.rerender({ conv: 'conv-kessel' })
    expect(hook.result.current.viewTab).toBe('thread')
    expect(localStorage.getItem('local|k2:session-view-tab:conv-kessel')).toBe('thread')
    expect(useSessionViewRequestStore.getState().pending).toBeNull()
  })

  it('wins over a remembered view on an open pane, once', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-kessel', 'terminal')
    const hook = renderHook(() => useSessionViewTab('conv-kessel', PRIMARY, 'ws-kessel'))
    expect(hook.result.current.viewTab).toBe('terminal')
    act(() => requestSessionView('ws-kessel', 'thread'))
    expect(hook.result.current.viewTab).toBe('thread')
    expect(useSessionViewRequestStore.getState().pending).toBeNull()
    // Taken: the person's next pick sticks.
    act(() => hook.result.current.setViewTab('split'))
    expect(hook.result.current.viewTab).toBe('split')
  })

  it("another workspace's pane, a pane with no workspace and another server's room ignore it", () => {
    requestSessionView('ws-kessel', 'thread')
    expect(renderHook(() => useSessionViewTab('conv-x', PRIMARY, 'ws-other')).result.current.viewTab).toBe('split')
    expect(renderHook(() => useSessionViewTab('conv-y', PRIMARY)).result.current.viewTab).toBe('split')
    expect(
      renderHook(() => useSessionViewTab('conv-z', { isPrimary: false, hostKey: 'box.k2.dev' }, 'ws-kessel')).result.current
        .viewTab,
    ).toBe('split')
    expect(useSessionViewRequestStore.getState().pending?.projectId).toBe('ws-kessel')
  })

  it('goes stale', () => {
    const pending = { projectId: 'ws-kessel', view: 'thread' as const, at: 1_000 }
    expect(sessionViewRequestFor(pending, 'ws-kessel', 1_000 + SESSION_VIEW_REQUEST_TTL_MS)).toBe(pending)
    expect(sessionViewRequestFor(pending, 'ws-kessel', 1_001 + SESSION_VIEW_REQUEST_TTL_MS)).toBeNull()
    expect(sessionViewRequestFor(pending, null, 1_000)).toBeNull()
  })
})
