// @vitest-environment jsdom
// A person or channel row on Home (prd-home-people-channels-v1 §5.4, §15):
// a DM's badge is a number, a channel's a dot, both from the server's
// summary; a server without chat greys the row and says so; clicking opens
// the chat in the main area (never a room or a Thread).

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string): Promise<unknown> => ({})),
  status: { ok: true, on: true, reason: null as string | null, flags: {} },
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string) => h.get(s, r),
  daemonCliPost: vi.fn(async () => ({ ok: true })),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/home-chat/events-socket', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-chat/events-socket')>()),
  subscribeHomeChatEvents: () => () => undefined,
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { HomeChatHomeRow } from './HomeChatRow'
import { resetHomeChatForTests, useHomeChatStore } from '@/stores/home-chat'
import type { Home } from '@/stores/homes'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const HOME: Home = {
  id: 'h1',
  name: 'Work',
  rows: [
    { kind: 'person', address: 'k2:connect:a::local', workspaceId: null, label: 'Alex' },
    { kind: 'channel', address: '600::local', workspaceId: null, label: 'design' },
  ],
}

beforeEach(() => {
  resetHomeChatForTests()
  h.status = { ok: true, on: true, reason: null, flags: {} }
  h.get.mockImplementation(async (_s, route) => {
    if (route === 'home-chat/status') return h.status
    if (route === 'home-chat/summary')
      return {
        chat: 'live',
        convs: [
          { conv: '500', kind: 'dm', unread: true, badge: { kind: 'count', n: 4 }, with: 'k2:connect:a' },
          { conv: '600', kind: 'channel', unread: true, badge: { kind: 'dot' } },
        ],
      }
    if (route === 'home-chat/people')
      return { me: { ext: 'k2:owner', memberId: '1' }, people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex Ng', ready: true, memberId: '100' }] }
    throw new Error(`unexpected GET ${route}`)
  })
})
afterEach(() => {
  cleanup()
  resetHomeChatForTests()
})

describe('Home chat rows', () => {
  it('a DM shows its number and the person’s name; a channel its dot', async () => {
    render(
      <>
        <HomeChatHomeRow home={HOME} row={HOME.rows[0]} />
        <HomeChatHomeRow home={HOME} row={HOME.rows[1]} />
      </>,
    )
    await waitFor(() => expect(document.querySelector('[data-home-chat-badge="count"] [data-unread-count]')?.getAttribute('data-unread-count')).toBe('4'))
    expect(document.querySelector('[data-home-chat-badge="dot"]')).not.toBeNull()
    await screen.findByText('Alex Ng')
  })

  it('clicking opens the chat in the main area', async () => {
    render(<HomeChatHomeRow home={HOME} row={HOME.rows[1]} />)
    await waitFor(() => expect(useHomeChatStore.getState().byHost.local?.status?.on).toBe(true))
    await act(async () => {
      fireEvent.click(screen.getByText('design'))
    })
    expect(useHomeChatStore.getState().open).toEqual({ kind: 'channel', hostKey: 'local', conv: '600', address: '600::local' })
  })

  it('a server without chat greys the row and says so; clicking does nothing', async () => {
    h.status = { ok: true, on: false, reason: 'mac', flags: {} }
    render(<HomeChatHomeRow home={HOME} row={HOME.rows[1]} />)
    await screen.findByText('No chat on this server')
    await act(async () => {
      fireEvent.click(screen.getByText('design'))
    })
    expect(useHomeChatStore.getState().open).toBeNull()
  })
})
