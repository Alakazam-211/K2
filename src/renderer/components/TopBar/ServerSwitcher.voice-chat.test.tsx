// @vitest-environment jsdom
//
// prd-voice-chat-only-members-v1 §8.2: a server whose login is Voice & Chat
// only is never offered by the switcher (nor Cmd+L, which opens it). The
// other servers stay, and the hint is all it takes before any check.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useServerSwitcherStore } from '@/stores/server-switcher'

vi.mock('@/stores/settings', () => ({
  useSettingsStore: (sel: (s: { openSettings: () => void }) => unknown) => sel({ openSettings: () => {} }),
}))

vi.mock('@/lib/remote-session', () => ({
  reviveRemoteSession: vi.fn(),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

import ServerSwitcher from './ServerSwitcher'

function host(id: string, access?: 'voice_chat'): ConnectHost {
  return {
    id,
    label: id,
    hostname: `${id}.k2.dev`,
    port: 443,
    secure: true,
    token: 'tok',
    remember: false,
    lastConnectedAt: null,
    ...(access ? { access } : {}),
  }
}

afterEach(() => {
  cleanup()
  act(() => useServerSwitcherStore.getState().setOpen(false))
})

describe('the switcher and Voice & Chat only servers', () => {
  it('lists every server except the Voice & Chat only one', () => {
    act(() => {
      useConnectHostStore.setState({ activeHost: 'local', hosts: [host('alpha'), host('review', 'voice_chat'), host('zed')] })
    })
    const { container } = render(<ServerSwitcher />)
    act(() => useServerSwitcherStore.getState().setOpen(true))
    const rows = Array.from(container.querySelectorAll<HTMLElement>('button[role="option"]')).map((b) => b.textContent ?? '')
    expect(rows.some((t) => t.includes('alpha'))).toBe(true)
    expect(rows.some((t) => t.includes('zed'))).toBe(true)
    expect(rows.some((t) => t.includes('review'))).toBe(false)
  })

  it('a server whose hint clears (upgraded to Member) comes back', () => {
    act(() => {
      useConnectHostStore.setState({ activeHost: 'local', hosts: [host('review', 'voice_chat')] })
    })
    const { container } = render(<ServerSwitcher />)
    act(() => useServerSwitcherStore.getState().setOpen(true))
    const has = (): boolean =>
      Array.from(container.querySelectorAll<HTMLElement>('button[role="option"]')).some((b) => (b.textContent ?? '').includes('review'))
    expect(has()).toBe(false)
    act(() => useConnectHostStore.getState().setHostAccess('review', 'member'))
    expect(useConnectHostStore.getState().hosts[0].access).toBeUndefined()
    expect(has()).toBe(true)
  })
})
