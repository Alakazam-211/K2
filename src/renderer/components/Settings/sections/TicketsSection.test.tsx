// @vitest-environment jsdom
// Settings → Tickets (0.45.4, Rosson 2026-10-10): the server's ticket rules
// (daemon-owned, owner/admin may change them) and the person's own
// "Open the Tickets page on" preference (this computer, per server+login).
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

const cli = vi.hoisted(() => ({
  settings: {} as Record<string, unknown>,
  posts: [] as Array<{ route: string; body: unknown }>,
  getError: null as Error | null,
  postError: null as Error | null,
}))

// Settings.tsx (for the sidebar order) pulls stores that listen on Tauri.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main', listen: async () => () => undefined }),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route !== 'tickets/settings') throw new Error(`unexpected GET ${route}`)
    if (cli.getError) throw cli.getError
    return cli.settings
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
    cli.posts.push({ route, body })
    if (cli.postError) throw cli.postError
    cli.settings = { ...cli.settings, ...(body as Record<string, unknown>) }
    if ((body as Record<string, unknown>).defaultAssignee === '') cli.settings.defaultAssignee = null
    return cli.settings
  }),
}))

import { TicketsSection, TICKETS_MANIFEST } from './TicketsSection'
import { parseTicketSettings } from './tickets-settings-api'
import { SECTION_LABELS } from '../searchManifest'
import { settingsNav } from '../Settings'
import { readTicketOpenOn, initialTicketScope, ticketPrefOwner } from '@/lib/ticket-open-pref'

function serverSettings(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ok: true,
    briefPolicy: 'warn',
    assigneePolicy: 'warn',
    defaultAssignee: null,
    defaultAssigneeKnown: true,
    values: ['off', 'warn', 'require'],
    people: [
      { value: 'owner', label: 'Rosson (owner)', role: 'owner', disabled: false },
      { value: 'julie', label: 'julie', role: 'member', disabled: false },
    ],
    canEdit: true,
    ...over,
  }
}

beforeEach(() => {
  localStorage.clear()
  cli.settings = serverSettings()
  cli.posts = []
  cli.getError = null
  cli.postError = null
})

afterEach(() => {
  cleanup()
})

function trigger(name: string): HTMLButtonElement {
  return screen.getByRole('button', { name }) as HTMLButtonElement
}

async function pick(name: string, option: string): Promise<void> {
  fireEvent.click(trigger(name))
  const menu = await screen.findByTestId('setting-dropdown-menu')
  fireEvent.click(within(menu).getByText(option))
}

async function loaded(): Promise<void> {
  await waitFor(() => expect(screen.getByRole('button', { name: 'Require an HTML brief' })).toBeTruthy())
}

describe('Settings → Tickets: where it lives', () => {
  it('sits directly beneath Projects in the sidebar and is searchable', () => {
    const ids = settingsNav({ zenGardens: false }).flatMap((b) => (b.kind === 'item' ? [b.id] : []))
    expect(ids[ids.indexOf('project-groups') + 1]).toBe('tickets')
    expect(settingsNav({ zenGardens: false }).find((b) => b.kind === 'item' && b.id === 'tickets')).toEqual({
      kind: 'item',
      id: 'tickets',
      label: 'Tickets',
    })
    expect(SECTION_LABELS.tickets).toBe('Tickets')
    expect(TICKETS_MANIFEST.map((e) => e.id)).toEqual([
      'tickets.require-brief',
      'tickets.require-assignee',
      'tickets.default-assignee',
      'tickets.open-on',
    ])
    expect(TICKETS_MANIFEST.every((e) => e.section === 'tickets')).toBe(true)
  })
})

describe('Settings → Tickets: server rules', () => {
  it('renders the daemon’s rules with the house controls', async () => {
    cli.settings = serverSettings({ briefPolicy: 'require', defaultAssignee: 'julie' })
    render(<TicketsSection />)
    await loaded()
    expect(screen.getByRole('heading', { name: 'Tickets' })).toBeTruthy()
    expect(screen.getByText('Server rules')).toBeTruthy()
    expect(screen.getByText('Just for you')).toBeTruthy()
    expect(trigger('Require an HTML brief').textContent).toContain('Require')
    expect(trigger('Require an assignee').textContent).toContain('Warn')
    expect(trigger('Default assignee').textContent).toContain('julie')
    expect(trigger('Open the Tickets page on').textContent).toContain('Waiting on me')
    for (const id of ['tickets.require-brief', 'tickets.require-assignee', 'tickets.default-assignee', 'tickets.open-on']) {
      expect(document.querySelector(`[data-settings-id="${id}"]`)).not.toBeNull()
    }
  })

  it('owner/admin: each dropdown saves exactly its own key', async () => {
    render(<TicketsSection />)
    await loaded()
    expect(screen.queryByTestId('tickets-settings-readonly')).toBeNull()

    await pick('Require an HTML brief', 'Require')
    await waitFor(() => expect(cli.posts).toHaveLength(1))
    expect(cli.posts[0]).toEqual({ route: 'tickets/settings', body: { briefPolicy: 'require' } })
    await waitFor(() => expect(trigger('Require an HTML brief').textContent).toContain('Require'))

    await pick('Require an assignee', 'Off')
    await waitFor(() => expect(cli.posts).toHaveLength(2))
    expect(cli.posts[1].body).toEqual({ assigneePolicy: 'off' })

    await pick('Default assignee', 'Rosson (owner)')
    await waitFor(() => expect(cli.posts).toHaveLength(3))
    expect(cli.posts[2].body).toEqual({ defaultAssignee: 'owner' })
    await waitFor(() => expect(trigger('Default assignee').textContent).toContain('Rosson (owner)'))

    await pick('Default assignee', 'Nobody')
    await waitFor(() => expect(cli.posts).toHaveLength(4))
    expect(cli.posts[3].body).toEqual({ defaultAssignee: '' })
    await waitFor(() => expect(trigger('Default assignee').textContent).toContain('Nobody'))
  })

  it('member: sees the rules, cannot change them', async () => {
    cli.settings = serverSettings({ canEdit: false })
    render(<TicketsSection />)
    await loaded()
    expect(screen.getByTestId('tickets-settings-readonly').textContent).toBe(
      'Only the owner or an admin can change these.',
    )
    for (const name of ['Require an HTML brief', 'Require an assignee', 'Default assignee']) {
      expect(trigger(name).disabled).toBe(true)
      fireEvent.click(trigger(name))
      expect(screen.queryByTestId('setting-dropdown-menu')).toBeNull()
    }
    expect(cli.posts).toEqual([])
    // Their own landing tab is still theirs to set.
    expect(trigger('Open the Tickets page on').disabled).toBe(false)
  })

  it('a refused save shows the daemon’s error', async () => {
    cli.postError = new Error('only the owner or an admin may change the ticket rules')
    render(<TicketsSection />)
    await loaded()
    await pick('Require an assignee', 'Require')
    expect((await screen.findByTestId('tickets-settings-save-error')).textContent).toContain(
      'only the owner or an admin',
    )
    expect(trigger('Require an assignee').textContent).toContain('Warn')
  })

  it('a server without the route says it could not load', async () => {
    cli.getError = new Error('404 not found')
    render(<TicketsSection />)
    expect((await screen.findByTestId('tickets-settings-error')).textContent).toContain('404 not found')
  })

  it('a default that is no longer a user still shows, and says so', async () => {
    cli.settings = serverSettings({ defaultAssignee: 'gone', defaultAssigneeKnown: false })
    render(<TicketsSection />)
    await loaded()
    expect(trigger('Default assignee').textContent).toContain('gone')
    expect(screen.getByText(/no longer on this server/)).toBeTruthy()
  })

  it('parses a short or odd body without inventing rights', () => {
    const s = parseTicketSettings({ briefPolicy: 'bogus', people: [{ value: '' }, null, { value: 'a' }] })
    expect(s.briefPolicy).toBe('warn')
    expect(s.canEdit).toBe(false)
    expect(s.defaultAssignee).toBeNull()
    expect(s.people.map((p) => p.value)).toEqual(['a'])
  })
})

describe('Settings → Tickets: just for you', () => {
  it('Open the Tickets page on: Waiting by default; the pick is stored per server + login', async () => {
    render(<TicketsSection />)
    await loaded()
    expect(readTicketOpenOn()).toBe('waiting')
    expect(initialTicketScope()).toBe('waiting')

    await pick('Open the Tickets page on', 'All')
    expect(readTicketOpenOn()).toBe('all')
    expect(initialTicketScope()).toBe('all')
    const { host, user } = ticketPrefOwner()
    expect(localStorage.getItem(`k2.tickets.openOn.${host}.${user}`)).toBe('all')
    // Another login on the same server keeps its own.
    expect(readTicketOpenOn({ host, user: 'julie' })).toBe('waiting')

    await pick('Open the Tickets page on', 'Remember my last tab')
    expect(readTicketOpenOn()).toBe('last')
    expect(cli.posts).toEqual([])
  })
})
