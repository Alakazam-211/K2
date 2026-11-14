// @vitest-environment jsdom
// The pickers and the members sheet (prd-home-people-channels-v1 §6, §7,
// §13; slice S6).
//
//   1. Add a person lists people only: never App users, disabled logins or
//      agents (the daemon lists none; anything else in an answer is
//      dropped), "Setting up…" for one not ready, and adds a `person` row.
//   2. A Voice & Chat only server is offered for people and channels.
//   3. New channel: invite-only by default; a pending channel's id is
//      waited for by its slug, then the row goes on the Home.
//   4. Add a channel: the server's channels (browse), never an archived one.
//   5. The members sheet hides what a role can't do: a Member (or a Voice &
//      Chat only login) sees no Remove or Rename on another's channel; an
//      Owner or Admin sees both.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string, _p?: Record<string, unknown>): Promise<unknown> => ({})),
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true })),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: Record<string, unknown>) => h.get(s, r, p),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/home-chat/events-socket', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-chat/events-socket')>()),
  subscribeHomeChatEvents: () => () => undefined,
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { AddChannelPanel, AddPersonPanel, NewChannelPanel, chatServers, waitForChannelId } from './HomeChatPickers'
import { HomeChatMembersSheet } from './HomeChatMembersSheet'
import { resetHomeChatForTests } from '@/stores/home-chat'
import { useHomesStore, type HomeRow } from '@/stores/homes'
import type { ConnectHost } from '@/stores/connect-host'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

let people: Record<string, unknown> = {}
let members: Record<string, unknown> = {}

beforeEach(() => {
  resetHomeChatForTests()
  h.get.mockReset()
  h.post.mockReset()
  const home = useHomesStore.getState().homes[0]
  useHomesStore.getState().setRowOrder(home.id, home.rows.map((r) => r.address))
  for (const r of [...home.rows]) useHomesStore.getState().removeRow(home.id, r.address)
  people = {
    ok: true,
    me: { ext: 'k2:connect:me', memberId: '1' },
    people: [
      { ext: 'k2:owner', username: 'owner', displayName: 'Rae', ready: true, memberId: '2' },
      { ext: 'k2:connect:a', username: 'alex', displayName: 'Alex', ready: true, memberId: '100' },
      { ext: 'k2:connect:b', username: 'bea', displayName: 'Bea', ready: false, memberId: null },
      // Never sent by the daemon; dropped if an answer ever carried them.
      { ext: 'k2:workspace:5d1c', username: 'atlas', displayName: 'Atlas agent', ready: true, memberId: '7' },
      { ext: 'k2:principal:app-1', username: 'guest', displayName: 'App guest', ready: true, memberId: '8' },
    ],
  }
  members = { ok: true, conv: '600', slug: 'c-0123456789', state: 'active', name: 'design', open: false, creator: 'k2:connect:a', people: [{ ext: 'k2:connect:me', state: 'in' }, { ext: 'k2:connect:a', state: 'in' }], agents: [] }
  h.get.mockImplementation(async (_s, route) => {
    switch (route) {
      case 'home-chat/status':
        return { ok: true, on: true, reason: null, flags: {} }
      case 'home-chat/summary':
        return { ok: true, chat: 'live', convs: [] }
      case 'home-chat/people':
        return people
      case 'home-chat/channels':
        return {
          ok: true,
          channels: [
            { conv: '600', name: 'design', open: false, archived: false, home: true },
            { conv: '601', name: 'general', open: true, archived: false, home: true },
            { conv: '602', name: 'old', open: true, archived: true, home: true },
          ],
        }
      case 'home-chat/channels/members':
        return members
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  })
})
afterEach(() => {
  cleanup()
  resetHomeChatForTests()
})

function home() {
  return useHomesStore.getState().homes[0]
}

/** The sections are presentational: a pick reaches this callback, which
 *  (as the Add to Home modal does) writes the row. */
const picked: HomeRow[] = []
function add(row: HomeRow): boolean {
  picked.push(row)
  return useHomesStore.getState().addRow(home().id, row)
}

describe('Add a person', () => {
  it('lists people only, says who is still being set up, and adds a person row', async () => {
    render(<AddPersonPanel home={home()} onAddRow={add} />)
    await screen.findByText('Alex')
    expect(screen.getByText('Rae')).toBeTruthy()
    expect(screen.queryByText('Atlas agent')).toBeNull()
    expect(screen.queryByText('App guest')).toBeNull()
    expect(screen.getByText('Setting up…')).toBeTruthy()
    await act(async () => {
      fireEvent.click(screen.getByText('Alex'))
    })
    await waitFor(() => expect(home().rows).toEqual([{ kind: 'person', address: 'k2:connect:a::local', workspaceId: null, label: 'Alex' }]))
    // Bea isn't ready: a click adds nothing.
    await act(async () => {
      fireEvent.click(screen.getByText('Bea'))
    })
    expect(home().rows).toHaveLength(1)
  })

  it('a server without chat says so', async () => {
    h.get.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/status') return { ok: true, on: false, reason: 'mac', flags: {} }
      throw new Error(`unexpected GET ${route}`)
    })
    render(<AddPersonPanel home={home()} onAddRow={add} />)
    expect(await screen.findByText('No chat on this server (chat runs on Linux servers).')).toBeTruthy()
  })
})

describe('the servers asked', () => {
  it('a Voice & Chat only server is offered for people and channels', () => {
    const hosts = [
      { id: '1', label: 'Lab', hostname: 'lab.example.test', port: 443, secure: true, token: 't', access: 'voice_chat' },
      { id: '2', label: 'Box', hostname: 'box.example.test', port: 443, secure: true, token: 't' },
    ] as unknown as ConnectHost[]
    const s = chatServers('local', 'This computer', hosts)
    expect(s.map((x) => x.label)).toEqual(['This computer', 'Box', 'Lab'])
  })
})

describe('channels', () => {
  it('Add a channel lists the server’s live channels and adds a channel row', async () => {
    render(<AddChannelPanel home={home()} onAddRow={add} />)
    await screen.findByText('general')
    expect(screen.queryByText('old')).toBeNull()
    await act(async () => {
      fireEvent.click(screen.getByText('general'))
    })
    expect(home().rows).toEqual([{ kind: 'channel', address: '601::local', workspaceId: null, label: 'general' }])
  })

  it('New channel: invite-only by default, people from the server, and the row once Sew answers', async () => {
    h.post.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/channels/create') return { ok: true, slug: 'c-0123456789', conv: null, state: 'pending' }
      return { ok: true }
    })
    let asks = 0
    h.get.mockImplementation(async (_s, route, p) => {
      if (route === 'home-chat/status') return { ok: true, on: true, reason: null, flags: {} }
      if (route === 'home-chat/summary') return { ok: true, chat: 'live', convs: [] }
      if (route === 'home-chat/people') return people
      if (route === 'home-chat/channels/members') {
        expect(p).toEqual({ slug: 'c-0123456789' })
        asks += 1
        return { ...members, conv: asks > 1 ? '650' : null, state: asks > 1 ? 'active' : 'pending' }
      }
      throw new Error(`unexpected GET ${route}`)
    })
    vi.useFakeTimers({ shouldAdvanceTime: true })
    render(<NewChannelPanel home={home()} onAddRow={add} />)
    const name = await screen.findByTestId('home-chat-new-channel-name')
    // Invite only is the default, on the segmented privacy choice.
    expect(screen.getByTestId('home-chat-invite-only').getAttribute('aria-checked')).toBe('true')
    expect(screen.getByTestId('home-chat-open').getAttribute('aria-checked')).toBe('false')
    await screen.findByText('Alex')
    expect(screen.queryByText('Atlas agent')).toBeNull()
    await act(async () => {
      fireEvent.change(name, { target: { value: 'Launch' } })
    })
    await act(async () => {
      fireEvent.click(screen.getByText('Alex'))
    })
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-new-channel-create'))
    })
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1500)
    })
    vi.useRealTimers()
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/channels/create', { name: 'Launch', open: false, members: ['k2:connect:a'] })
    await waitFor(() => expect(home().rows).toEqual([{ kind: 'channel', address: '650::local', workspaceId: null, label: 'Launch' }]))
  })

  it('New channel uses the shared field, checkbox and segmented privacy; Open says so and makes an open channel', async () => {
    h.post.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/channels/create') return { ok: true, slug: 'c-1', conv: '651', state: 'active' }
      return { ok: true }
    })
    h.get.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/status') return { ok: true, on: true, reason: null, flags: {} }
      if (route === 'home-chat/summary') return { ok: true, chat: 'live', convs: [] }
      if (route === 'home-chat/people') return people
      throw new Error(`unexpected GET ${route}`)
    })
    render(<NewChannelPanel home={home()} onAddRow={add} />)
    const name = await screen.findByTestId('home-chat-new-channel-name')
    expect(name.getAttribute('data-k2-chamfer')).toBe('control')
    await screen.findByText('Alex')
    expect(document.querySelector('[data-new-channel-field="people"] input[type="checkbox"]')?.classList.contains('k2-checkbox')).toBe(true)
    expect(screen.getByTestId('home-chat-open').closest('[role="radiogroup"]')?.classList.contains('k2-segmented')).toBe(true)
    expect(document.querySelector('[data-new-channel-privacy-hint]')?.textContent).toContain('Only the people in it see it.')
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-open'))
    })
    expect(document.querySelector('[data-new-channel-privacy-hint]')?.textContent).toContain('Everyone on this server can find it and join.')
    await act(async () => {
      fireEvent.change(name, { target: { value: 'Lobby' } })
    })
    await act(async () => {
      fireEvent.keyDown(name, { key: 'Enter' })
    })
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/channels/create', { name: 'Lobby', open: true, members: [] })
    await waitFor(() => expect(home().rows).toEqual([{ kind: 'channel', address: '651::local', workspaceId: null, label: 'Lobby' }]))
  })

  it('waiting for a channel id gives up after its time', async () => {
    h.get.mockImplementation(async () => ({ ...members, conv: null, state: 'pending' }))
    let t = 0
    const id = await waitForChannelId('local', 'c-0123456789', { sleep: async (ms) => void (t += ms), now: () => t })
    expect(id).toBeNull()
  })
})

describe('the members sheet hides what a role can’t do (§13)', () => {
  const channel = { kind: 'channel' as const, hostKey: 'local', conv: '600', address: '600::local' }

  it('a Member (or a Voice & Chat only login) on another’s channel: Add people and Leave only', async () => {
    render(<HomeChatMembersSheet target={channel} onClose={() => undefined} onChanged={() => undefined} />)
    await screen.findByTestId('home-chat-member-list')
    await waitFor(() => expect(screen.getByTestId('home-chat-member-list').textContent).toContain('Alex'))
    expect(screen.getByTestId('home-chat-add-people')).toBeTruthy()
    expect(screen.getByTestId('home-chat-leave')).toBeTruthy()
    expect(screen.queryByTestId('home-chat-remove-member')).toBeNull()
    expect(screen.queryByTestId('home-chat-rename')).toBeNull()
    expect(screen.queryByTestId('home-chat-delete')).toBeNull()
    expect(screen.getByTestId('home-chat-no-agents')).toBeTruthy()
  })

  it('an Owner or Admin: Remove others, Rename, Delete', async () => {
    people = { ...people, people: (people.people as Array<Record<string, unknown>>).map((p) => ({ ...p, role: 'member' })) }
    render(<HomeChatMembersSheet target={channel} onClose={() => undefined} onChanged={() => undefined} />)
    await waitFor(() => expect(screen.getAllByTestId('home-chat-remove-member')).toHaveLength(1))
    expect(screen.getByTestId('home-chat-rename')).toBeTruthy()
    expect(screen.getByTestId('home-chat-delete')).toBeTruthy()
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-remove-member'))
    })
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/channels/remove', { conv: '600', ext: 'k2:connect:a' })
  })

  it('agents with access show with an eye mark (granting is a later slice)', async () => {
    members = { ...members, agents: [{ agent: 'ws-1', name: 'Atlas' }] }
    render(<HomeChatMembersSheet target={channel} onClose={() => undefined} onChanged={() => undefined} />)
    const list = await screen.findByTestId('home-chat-agent-list')
    // The eye is a line icon now (K2 Design), named for screen readers.
    expect(list.querySelector('[aria-label="Can read and post"] svg')).not.toBeNull()
    expect(list.textContent).toContain('Atlas')
  })
})
