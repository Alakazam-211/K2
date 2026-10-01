// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'

vi.mock('@/stores/connect-host', () => ({
  useConnectHostStore: (sel: (s: { activeHost: 'local' }) => unknown) =>
    sel({ activeHost: 'local' }),
  activeHostKey: () => 'local',
}))

import { useSessionViewTab } from './useSessionViewTab'

describe('useSessionViewTab', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('remembers the view and both split sides per session', () => {
    localStorage.setItem('local|k2:session-view-tab:conv-2', 'split')
    const first = renderHook(() => useSessionViewTab('conv-2'))
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
    const again = renderHook(() => useSessionViewTab('conv-2'))
    expect(again.result.current.viewTab).toBe('split')
    expect(again.result.current.splitLeft).toBe('chat')
    expect(again.result.current.splitRight).toBe('chat')
    expect(renderHook(() => useSessionViewTab('conv-other')).result.current.viewTab).toBe('terminal')
  })
})
