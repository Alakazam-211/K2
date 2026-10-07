// @vitest-environment jsdom
// Home: the session view memory (Terminal / Chat / Thread / split and both
// split sides) is keyed by the ROOM's server, not the window's. Agent A on
// server X opened directly (server switcher → primary room on X) and from
// My Home (a room pinned to X while the window is on another server) read
// and write the same `<X>|k2:session-view-*:<session>` entries.
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import {
  primaryScope,
  remoteRoomScope,
  scopeForHost,
  viewOnlyScope,
  __resetServerScopesForTests,
} from '@/kessel/server-scope'
import { useSessionViewTab, sessionViewHostKey } from './useSessionViewTab'

const X: ConnectHost = {
  id: 'id-x',
  label: 'X',
  hostname: 'x.k2.dev',
  port: 443,
  secure: true,
  token: 'tok-x',
  remember: false,
  lastConnectedAt: null,
}

const SESSION = 'conv-agent-a'

function windowOn(active: 'local' | ConnectHost): void {
  useConnectHostStore.setState({ hosts: [X], activeHost: active } as never)
}

beforeEach(() => {
  localStorage.clear()
  __resetServerScopesForTests()
  windowOn('local')
})

describe('session view memory follows the room server (Home)', () => {
  it('a Home room on X uses X, not the window server', () => {
    expect(sessionViewHostKey(remoteRoomScope(scopeForHost(X)), 'local')).toBe('x.k2.dev')
    expect(sessionViewHostKey(viewOnlyScope(scopeForHost(X)), 'local')).toBe('x.k2.dev')
    expect(sessionViewHostKey(primaryScope(), 'x.k2.dev')).toBe('x.k2.dev')
    expect(sessionViewHostKey(primaryScope(), 'local')).toBe('local')
  })

  it('a view chosen directly on X shows when the agent is opened from Home', () => {
    windowOn(X)
    const direct = renderHook(() => useSessionViewTab(SESSION, primaryScope()))
    expect(direct.result.current.viewTab).toBe('terminal')
    act(() => direct.result.current.setViewTab('split'))
    act(() => direct.result.current.setSplitLeft('chat'))
    act(() => direct.result.current.setSplitRight('terminal'))
    direct.unmount()
    expect(localStorage.getItem(`x.k2.dev|k2:session-view-tab:${SESSION}`)).toBe('split')

    windowOn('local')
    const home = renderHook(() => useSessionViewTab(SESSION, remoteRoomScope(scopeForHost(X))))
    expect(home.result.current.viewTab).toBe('split')
    expect(home.result.current.splitLeft).toBe('chat')
    expect(home.result.current.splitRight).toBe('terminal')
    home.unmount()
    expect(localStorage.getItem(`local|k2:session-view-tab:${SESSION}`)).toBeNull()
  })

  it('a view chosen from Home shows when connecting to X directly', () => {
    windowOn('local')
    const home = renderHook(() => useSessionViewTab(SESSION, remoteRoomScope(scopeForHost(X))))
    act(() => home.result.current.setViewTab('chat'))
    home.unmount()
    expect(localStorage.getItem(`x.k2.dev|k2:session-view-tab:${SESSION}`)).toBe('chat')
    expect(localStorage.getItem(`local|k2:session-view-tab:${SESSION}`)).toBeNull()

    windowOn(X)
    const direct = renderHook(() => useSessionViewTab(SESSION, primaryScope()))
    expect(direct.result.current.viewTab).toBe('chat')
  })

  it('local server behaviour is unchanged', () => {
    windowOn('local')
    const local = renderHook(() => useSessionViewTab(SESSION, primaryScope()))
    act(() => local.result.current.setViewTab('thread'))
    local.unmount()
    expect(localStorage.getItem(`local|k2:session-view-tab:${SESSION}`)).toBe('thread')
    // X's agent with the same session id keeps its own entry.
    const remote = renderHook(() => useSessionViewTab(SESSION, remoteRoomScope(scopeForHost(X))))
    expect(remote.result.current.viewTab).toBe('terminal')
    // A Home room on this computer reads the same `local|` entry.
    const localRoom = renderHook(() => useSessionViewTab(SESSION, scopeForHost('local')))
    expect(localRoom.result.current.viewTab).toBe('thread')
  })

  it('the primary room re-reads after a server switch', () => {
    localStorage.setItem(`x.k2.dev|k2:session-view-tab:${SESSION}`, 'thread')
    windowOn('local')
    const pane = renderHook(() => useSessionViewTab(SESSION, primaryScope()))
    expect(pane.result.current.viewTab).toBe('terminal')
    act(() => windowOn(X))
    expect(pane.result.current.viewTab).toBe('thread')
  })
})
