// @vitest-environment jsdom
// Settings → Notifications (prd-thread-push-notifications-v1 §9, S5).
// Rosson 2026-10-11: name-only pushes; Admins see people but can't change
// them; the server bell silences ticket pushes too; desktop + phone = one
// person. The page renders the daemon's truth from `/cli/notify/*`, hides
// on a server without `thread-push-v1`, and refetches on
// `notify_prefs_changed`.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

type Json = Record<string, unknown>

const cli = vi.hoisted(() => ({
  whoami: {} as Json,
  prefs: {} as Json,
  admin: {} as Json,
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: Json }>,
  adminError: null as Error | null,
  postError: null as Error | null,
}))

const events = vi.hoisted(() => ({ handlers: new Set<() => void>() }))

// `/cli/settings` for the Message previews row. `settings` is what the
// focused server's GET returns; `updates` records each write.
const appSettings = vi.hoisted(() => ({
  settings: {} as Json,
  updates: [] as Json[],
  updateError: null as Error | null,
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => appSettings.settings),
  settingsUpdate: vi.fn(async (u: Json) => {
    appSettings.updates.push(u)
    if (appSettings.updateError) throw appSettings.updateError
    appSettings.settings = { ...appSettings.settings, ...u }
    return appSettings.settings
  }),
}))

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
    cli.gets.push(route)
    if (route === 'auth/whoami') return cli.whoami
    if (route === 'notify/prefs') return cli.prefs
    if (route === 'notify/admin') {
      if (cli.adminError) throw cli.adminError
      return cli.admin
    }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Json) => {
    cli.posts.push({ route, body })
    if (cli.postError) throw cli.postError
    if (route === 'notify/prefs') return cli.prefs
    if (route === 'notify/admin/set') return { ok: true }
    throw new Error(`unexpected POST ${route}`)
  }),
}))

vi.mock('@/stores/session-events', async (orig) => ({
  ...(await orig<typeof import('@/stores/session-events')>()),
  onNotifyPrefsChanged: (_scope: unknown, fn: () => void) => {
    events.handlers.add(fn)
    return () => void events.handlers.delete(fn)
  },
}))

import { NotificationsSection, NOTIFICATIONS_MANIFEST, ownerHint } from './NotificationsSection'
import { PUSH_PREVIEWS_HINT, PUSH_PREVIEWS_LABEL, pushPreviewsValue } from './PushPreviewsRow'
import { parseNotifyPrefs, sourceLabel } from './notify-api'
import { useIsYouAuthor } from './useNotifyPrefs'
import { settingsNav } from '../Settings'
import { SECTION_LABELS } from '../searchManifest'
import { noteServerVersion, __resetServerScopesForTests } from '@/kessel/server-scope'

function ws(projectId: string, name: string, over: Json = {}): Json {
  return { projectId, name, handle: name, threadPush: true, source: 'fallback', ...over }
}

function prefs(over: Json = {}): Json {
  return {
    ok: true,
    principal: 'owner',
    server: { enabled: true, source: 'fallback' },
    default: true,
    devices: 2,
    push: 'ready',
    workspaces: [
      ws('p-k2', 'k2'),
      ws('p-sales', 'sales', { threadPush: false, source: 'user', updatedBy: 'owner', updatedAt: 1760190000 }),
      ws('p-ops', 'ops', { threadPush: false, source: 'user', updatedBy: 'gina', updatedAt: 1760190001 }),
      ws('p-docs', 'docs', { threadPush: false, source: 'default' }),
    ],
    ...over,
  }
}

function admin(over: Json = {}): Json {
  return {
    ok: true,
    defaults: [
      { projectId: 'p-k2', name: 'k2', threadPush: true, source: 'fallback' },
      { projectId: 'p-docs', name: 'docs', threadPush: false, source: 'default' },
    ],
    people: [
      {
        principal: 'gina',
        role: 'admin',
        disabled: false,
        devices: 1,
        server: { enabled: false, source: 'user' },
        workspaces: [{ projectId: 'p-k2', threadPush: false, source: 'user', updatedBy: 'gina', updatedAt: 1 }],
      },
      {
        principal: 'owner',
        role: 'owner',
        disabled: false,
        devices: 2,
        server: { enabled: true, source: 'fallback' },
        workspaces: [{ projectId: 'p-k2', threadPush: true, source: 'fallback' }],
      },
      {
        principal: 'sam',
        role: 'member',
        disabled: true,
        devices: 0,
        server: { enabled: true, source: 'fallback' },
        workspaces: [{ projectId: 'p-k2', threadPush: true, source: 'default' }],
      },
    ],
    ...over,
  }
}

function asOwnerToken(): void {
  cli.whoami = { username: null, owner: true, role: 'owner' }
}
function asAdmin(): void {
  cli.whoami = { username: 'gina', owner: false, role: 'admin' }
  cli.prefs = prefs({ principal: 'gina' })
}
function asMember(): void {
  cli.whoami = { username: 'sam', owner: false, role: 'member' }
  cli.prefs = prefs({ principal: 'sam' })
}

beforeEach(() => {
  __resetServerScopesForTests()
  noteServerVersion('local', null, ['thread-push-v1'])
  asOwnerToken()
  cli.prefs = prefs()
  cli.admin = admin()
  cli.gets = []
  cli.posts = []
  cli.adminError = null
  cli.postError = null
  events.handlers.clear()
  appSettings.settings = {}
  appSettings.updates = []
  appSettings.updateError = null
})

afterEach(() => {
  cleanup()
  __resetServerScopesForTests()
})

async function loaded(): Promise<void> {
  await waitFor(() => expect(screen.getByTestId('notifications-server-bell')).toBeTruthy())
}

function row(projectId: string): HTMLElement {
  return screen.getByTestId(`notifications-ws-${projectId}`)
}

function switchIn(el: HTMLElement): HTMLButtonElement {
  return within(el).getByRole('switch') as HTMLButtonElement
}

describe('Settings → Notifications: gating', () => {
  it('sits directly after General, only on a server with thread-push-v1', () => {
    const on = settingsNav({ zenGardens: false, notifications: true }).flatMap((b) => (b.kind === 'item' ? [b.id] : []))
    expect(on[0]).toBe('general')
    expect(on[1]).toBe('notifications')
    const off = settingsNav({ zenGardens: false, notifications: false }).flatMap((b) =>
      b.kind === 'item' ? [b.id] : [],
    )
    expect(off).not.toContain('notifications')
    expect(settingsNav({ zenGardens: false }).some((b) => b.kind === 'item' && b.id === 'notifications')).toBe(false)
    expect(SECTION_LABELS.notifications).toBe('Notifications')
    expect(NOTIFICATIONS_MANIFEST.every((e) => e.section === 'notifications')).toBe(true)
  })

  it('a server without thread-push-v1 fetches nothing and shows no switches', async () => {
    noteServerVersion('local', null, [])
    render(<NotificationsSection />)
    expect(screen.getByTestId('notifications-settings-unavailable')).toBeTruthy()
    expect(screen.queryAllByRole('switch')).toHaveLength(0)
    await act(async () => {
      await Promise.resolve()
    })
    expect(cli.gets).toEqual([])
    expect(events.handlers.size).toBe(0)
  })
})

describe('Settings → Notifications: your notifications', () => {
  it('shows the status line, the bell with the phone count, and each workspace’s effective value + source', async () => {
    render(<NotificationsSection />)
    await loaded()
    expect(screen.getByTestId('notifications-status').textContent).toBe('Push: Ready · 2 phones registered')
    expect(switchIn(screen.getByTestId('notifications-server-bell')).getAttribute('aria-checked')).toBe('true')

    expect(switchIn(row('p-k2')).getAttribute('aria-checked')).toBe('true')
    expect(within(row('p-k2')).getByText('Default (on)')).toBeTruthy()
    expect(switchIn(row('p-sales')).getAttribute('aria-checked')).toBe('false')
    expect(within(row('p-sales')).getByText('Set by you')).toBeTruthy()
    expect(within(row('p-ops')).getByText('Set by gina')).toBeTruthy()
    expect(within(row('p-docs')).getByText('Default (off)')).toBeTruthy()
    // Reset only where the person has their own row.
    expect(within(row('p-k2')).queryByText('Reset')).toBeNull()
    expect(within(row('p-docs')).queryByText('Reset')).toBeNull()
    expect(within(row('p-sales')).getByText('Reset')).toBeTruthy()
    expect(within(row('p-ops')).getByText('Reset')).toBeTruthy()
  })

  it('dormant / air-gap / no phone read as such', async () => {
    cli.prefs = prefs({ push: 'dormant', devices: 0 })
    render(<NotificationsSection />)
    await loaded()
    expect(screen.getByTestId('notifications-status').textContent).toBe(
      'Push: Off: no push service · No phone yet: install K2 Companion',
    )
    cleanup()
    cli.prefs = prefs({ push: 'airgap', devices: 1 })
    render(<NotificationsSection />)
    await loaded()
    expect(screen.getByTestId('notifications-status').textContent).toBe('Push: Off: air-gap · 1 phone registered')
  })

  it('the bell and a workspace switch write the caller’s own row (no principal in the body)', async () => {
    render(<NotificationsSection />)
    await loaded()
    cli.prefs = prefs({ server: { enabled: false, source: 'user' } })
    fireEvent.click(switchIn(screen.getByTestId('notifications-server-bell')))
    await waitFor(() =>
      expect(switchIn(screen.getByTestId('notifications-server-bell')).getAttribute('aria-checked')).toBe('false'),
    )
    fireEvent.click(switchIn(row('p-k2')))
    await waitFor(() => expect(cli.posts).toHaveLength(2))
    expect(cli.posts).toEqual([
      { route: 'notify/prefs', body: { server: { enabled: false } } },
      { route: 'notify/prefs', body: { workspace: { projectId: 'p-k2', threadPush: false } } },
    ])
  })

  it('Reset sends threadPush:null and paints the answer (back to the default)', async () => {
    render(<NotificationsSection />)
    await loaded()
    const answer = prefs()
    ;(answer.workspaces as Json[])[2] = ws('p-ops', 'ops', { threadPush: true, source: 'fallback' })
    cli.prefs = answer
    fireEvent.click(within(row('p-ops')).getByText('Reset'))
    await waitFor(() => expect(within(row('p-ops')).getByText('Default (on)')).toBeTruthy())
    expect(cli.posts).toEqual([{ route: 'notify/prefs', body: { workspace: { projectId: 'p-ops', threadPush: null } } }])
    expect(switchIn(row('p-ops')).getAttribute('aria-checked')).toBe('true')
    expect(within(row('p-ops')).queryByText('Reset')).toBeNull()
  })

  it('a failed write says so and keeps the daemon’s value', async () => {
    render(<NotificationsSection />)
    await loaded()
    cli.postError = new Error('usage: unknown projectId')
    fireEvent.click(switchIn(row('p-k2')))
    await waitFor(() =>
      expect(screen.getByTestId('notifications-save-error').textContent).toBe('usage: unknown projectId'),
    )
    expect(switchIn(row('p-k2')).getAttribute('aria-checked')).toBe('true')
  })

  it('refetches on notify_prefs_changed', async () => {
    render(<NotificationsSection />)
    await loaded()
    expect(events.handlers.size).toBeGreaterThan(0)
    cli.prefs = prefs({ devices: 3 })
    act(() => {
      for (const fn of events.handlers) fn()
    })
    await waitFor(() =>
      expect(screen.getByTestId('notifications-status').textContent).toBe('Push: Ready · 3 phones registered'),
    )
  })
})

describe('Settings → Notifications: roles', () => {
  it('a Member sees only their own notifications: no Defaults, no People, no admin GET', async () => {
    asMember()
    render(<NotificationsSection />)
    await loaded()
    await waitFor(() => expect(cli.gets).toContain('auth/whoami'))
    await act(async () => {
      await Promise.resolve()
    })
    expect(screen.queryByTestId('notifications-defaults')).toBeNull()
    expect(screen.queryByTestId('notifications-people')).toBeNull()
    expect(cli.gets).not.toContain('notify/admin')
  })

  it('an Admin sees People read-only (no switches) and may change workspace defaults', async () => {
    asAdmin()
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-people')).toBeTruthy())
    await waitFor(() => expect(screen.getByTestId('notifications-person-sam')).toBeTruthy())
    expect(screen.getByTestId('notifications-people-readonly')).toBeTruthy()
    const people = screen.getByTestId('notifications-people')
    expect(within(people).queryAllByRole('switch')).toHaveLength(0)
    expect(within(people).queryAllByText('Reset')).toHaveLength(0)
    // Owner first, disabled people dimmed and marked.
    const cards = within(people).getAllByTestId(/^notifications-person-[a-z]+$/)
    expect(cards.map((c) => c.getAttribute('data-testid'))).toEqual([
      'notifications-person-owner',
      'notifications-person-gina',
      'notifications-person-sam',
    ])
    expect(screen.getByTestId('notifications-person-sam').className).toContain('opacity-50')
    expect(screen.getByTestId('notifications-person-sam').textContent).toContain('Disabled')
    expect(screen.getByTestId('notifications-person-gina').textContent).toContain('(you)')
    expect(screen.getByTestId('notifications-person-gina').textContent).toContain('Server: off')

    const def = screen.getByTestId('notifications-default-p-docs')
    fireEvent.click(switchIn(def))
    await waitFor(() => expect(cli.posts).toHaveLength(1))
    fireEvent.click(within(def).getByText('Reset'))
    await waitFor(() => expect(cli.posts).toHaveLength(2))
    expect(cli.posts).toEqual([
      { route: 'notify/admin/set', body: { default: { projectId: 'p-docs', threadPush: true } } },
      { route: 'notify/admin/set', body: { default: { projectId: 'p-docs', threadPush: null } } },
    ])
  })

  it('the owner can change other people (bell, workspace, Reset) but not via their own card', async () => {
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-person-gina')).toBeTruthy())
    expect(screen.queryByTestId('notifications-people-readonly')).toBeNull()
    expect(within(screen.getByTestId('notifications-person-owner')).queryAllByRole('switch')).toHaveLength(0)

    const gina = screen.getByTestId('notifications-person-gina')
    const [bell] = within(gina).getAllByRole('switch')
    fireEvent.click(bell)
    await waitFor(() => expect(cli.posts).toHaveLength(1))
    const k2 = screen.getByTestId('notifications-person-gina-ws-p-k2')
    expect(within(k2).getByText('Set by gina')).toBeTruthy()
    fireEvent.click(within(k2).getByText('Reset'))
    await waitFor(() => expect(cli.posts).toHaveLength(2))
    fireEvent.click(switchIn(screen.getByTestId('notifications-person-sam-ws-p-k2')))
    await waitFor(() => expect(cli.posts).toHaveLength(3))
    expect(cli.posts).toEqual([
      { route: 'notify/admin/set', body: { principal: 'gina', server: { enabled: true } } },
      { route: 'notify/admin/set', body: { principal: 'gina', workspace: { projectId: 'p-k2', threadPush: null } } },
      { route: 'notify/admin/set', body: { principal: 'sam', workspace: { projectId: 'p-k2', threadPush: false } } },
    ])
  })

  it('the admin view refetches on notify_prefs_changed too', async () => {
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-person-sam')).toBeTruthy())
    cli.admin = admin({ people: [] })
    act(() => {
      for (const fn of events.handlers) fn()
    })
    await waitFor(() => expect(screen.queryByTestId('notifications-person-sam')).toBeNull())
  })
})

describe('Settings → Notifications: owner hint (person key, Q6)', () => {
  it('shows "Also you: z3thon" from aliases when viewing as the owner token', async () => {
    cli.prefs = prefs({ aliases: ['z3thon'] })
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-owner-hint')).toBeTruthy())
    expect(screen.getByTestId('notifications-owner-hint').textContent).toBe('Also you: z3thon')
  })

  it('lists every linked login, comma-separated', async () => {
    cli.prefs = prefs({ aliases: ['rosson', 'z3thon'] })
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-owner-hint')).toBeTruthy())
    expect(screen.getByTestId('notifications-owner-hint').textContent).toBe('Also you: rosson, z3thon')
  })

  it('a linked login (principal owner, not the owner token) sees "Also you" too', async () => {
    asAdmin()
    cli.prefs = prefs({ principal: 'owner', aliases: ['z3thon'] })
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-owner-hint')).toBeTruthy())
    expect(screen.getByTestId('notifications-owner-hint').textContent).toBe('Also you: z3thon')
  })

  it('no hint when nothing maps (single owner token, no Owner login)', async () => {
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-person-sam')).toBeTruthy())
    expect(screen.queryByTestId('notifications-owner-hint')).toBeNull()
  })

  it('no hint for a Connect login, even one with aliases', async () => {
    asAdmin()
    cli.prefs = prefs({ principal: 'gina', aliases: ['rosson'] })
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-person-sam')).toBeTruthy())
    expect(screen.queryByTestId('notifications-owner-hint')).toBeNull()
  })

  it('with two or more Owner logins the hint says the owner token is separate', async () => {
    const a = admin()
    ;(a.people as Json[]).push(
      { principal: 'rosson', role: 'owner', disabled: false, devices: 1, server: { enabled: true }, workspaces: [] },
      { principal: 'julie', role: 'owner', disabled: false, devices: 1, server: { enabled: true }, workspaces: [] },
    )
    cli.admin = a
    render(<NotificationsSection />)
    await waitFor(() => expect(screen.getByTestId('notifications-owner-hint')).toBeTruthy())
    expect(screen.getByTestId('notifications-owner-hint').textContent).toContain('2 logins have the Owner role')
  })

  it('ownerHint is pure: aliases only when the viewer is the owner person', () => {
    const p = parseNotifyPrefs(prefs({ aliases: ['rosson'] }))
    expect(ownerHint({ role: 'owner', ownerToken: true }, p, null)).toEqual({ kind: 'aliases', logins: ['rosson'] })
    expect(ownerHint({ role: 'admin', ownerToken: false }, p, null)).toEqual({ kind: 'aliases', logins: ['rosson'] })
    const gina = parseNotifyPrefs(prefs({ principal: 'gina', aliases: ['rosson'] }))
    expect(ownerHint({ role: 'admin', ownerToken: false }, gina, null)).toBeNull()
    expect(ownerHint({ role: 'owner', ownerToken: true }, parseNotifyPrefs(prefs()), null)).toBeNull()
  })
})

function YouProbe({ authors }: { authors: string[] }): React.JSX.Element {
  const isYou = useIsYouAuthor()
  return (
    <ul>
      {authors.map((a) => (
        <li key={a} data-testid={`you-${a}`}>
          {isYou(a) ? 'You' : a}
        </li>
      ))}
    </ul>
  )
}

describe('"You" for owner-linked logins (chat/ticket author stamps)', () => {
  it('owner viewer: posts by a linked login read "You"; others keep their name', async () => {
    cli.prefs = prefs({ aliases: ['z3thon'] })
    render(<YouProbe authors={['owner', 'z3thon', 'gina']} />)
    await waitFor(() => expect(screen.getByTestId('you-z3thon').textContent).toBe('You'))
    expect(screen.getByTestId('you-owner').textContent).toBe('You')
    expect(screen.getByTestId('you-gina').textContent).toBe('gina')
  })

  it('logged in AS the linked login (principal owner): its own posts and the owner token posts are "You"', async () => {
    asAdmin()
    cli.prefs = prefs({ principal: 'owner', aliases: ['z3thon'] })
    render(<YouProbe authors={['owner', 'z3thon']} />)
    await waitFor(() => expect(screen.getByTestId('you-z3thon').textContent).toBe('You'))
    expect(screen.getByTestId('you-owner').textContent).toBe('You')
  })

  it('another person never sees the owner-linked login as "You"', async () => {
    asAdmin()
    cli.prefs = prefs({ principal: 'gina', aliases: ['z3thon'] })
    render(<YouProbe authors={['z3thon']} />)
    await waitFor(() => expect(cli.gets).toContain('notify/prefs'))
    expect(screen.getByTestId('you-z3thon').textContent).toBe('z3thon')
  })

  it('a server without Thread push: today\'s rule (only owner is "You"), no fetch', async () => {
    noteServerVersion('local', null, [])
    render(<YouProbe authors={['owner', 'z3thon']} />)
    expect(screen.getByTestId('you-owner').textContent).toBe('You')
    expect(screen.getByTestId('you-z3thon').textContent).toBe('z3thon')
    expect(cli.gets).not.toContain('notify/prefs')
  })
})

describe('notify-api shapes', () => {
  it('sourceLabel: Default (on/off), Set by you, Set by X', () => {
    expect(sourceLabel({ source: 'fallback', updatedBy: null, threadPush: true }, 'owner')).toBe('Default (on)')
    expect(sourceLabel({ source: 'default', updatedBy: null, threadPush: false }, 'owner')).toBe('Default (off)')
    expect(sourceLabel({ source: 'user', updatedBy: 'owner', threadPush: false }, 'owner')).toBe('Set by you')
    expect(sourceLabel({ source: 'user', updatedBy: 'gina', threadPush: false }, 'owner')).toBe('Set by gina')
  })

  it('parseNotifyPrefs keeps updatedBy only for user rows and reads push readiness', () => {
    const p = parseNotifyPrefs(prefs())
    expect(p.push).toBe('ready')
    expect(p.devices).toBe(2)
    expect(p.workspaces.map((w) => [w.projectId, w.source, w.updatedBy])).toEqual([
      ['p-k2', 'fallback', null],
      ['p-sales', 'user', 'owner'],
      ['p-ops', 'user', 'gina'],
      ['p-docs', 'default', null],
    ])
    expect(parseNotifyPrefs({ push: 'nope' }).push).toBeNull()
  })
})

describe('Settings → Notifications: message previews (pushMessagePreviews)', () => {
  it('reads the key: a boolean is the value, absent or not a boolean is null (row hidden)', () => {
    expect(pushPreviewsValue({ pushMessagePreviews: true })).toBe(true)
    expect(pushPreviewsValue({ pushMessagePreviews: false })).toBe(false)
    expect(pushPreviewsValue({})).toBeNull()
    expect(pushPreviewsValue({ pushMessagePreviews: 'yes' })).toBeNull()
    expect(pushPreviewsValue(null)).toBeNull()
  })

  it('the owner sees the switch with the relay copy when the server has the key', async () => {
    appSettings.settings = { pushMessagePreviews: true }
    render(<NotificationsSection />)
    const sw = (await screen.findByRole('switch', { name: PUSH_PREVIEWS_LABEL })) as HTMLButtonElement
    expect(sw.getAttribute('aria-checked')).toBe('true')
    const row = screen.getByTestId('notifications-push-previews')
    expect(within(row).getByText(PUSH_PREVIEWS_HINT)).toBeTruthy()
    expect(PUSH_PREVIEWS_HINT).toContain("K2's push relay and Apple or Google")
    expect(screen.getByText('Message previews')).toBeTruthy()
  })

  it('hidden (title too) on a server whose settings lack the key', async () => {
    appSettings.settings = { defaultAgent: 'claude' }
    render(<NotificationsSection />)
    await loaded()
    await waitFor(() => expect(screen.getByTestId('notifications-defaults')).toBeTruthy())
    await act(async () => {
      await Promise.resolve()
    })
    expect(screen.queryByTestId('notifications-push-previews')).toBeNull()
    expect(screen.queryByText('Message previews')).toBeNull()
    expect(screen.queryByRole('switch', { name: PUSH_PREVIEWS_LABEL })).toBeNull()
  })

  it('an Admin sees it; a Member does not', async () => {
    appSettings.settings = { pushMessagePreviews: false }
    asAdmin()
    render(<NotificationsSection />)
    const sw = await screen.findByRole('switch', { name: PUSH_PREVIEWS_LABEL })
    expect(sw.getAttribute('aria-checked')).toBe('false')
    cleanup()
    asMember()
    render(<NotificationsSection />)
    await loaded()
    await act(async () => {
      await Promise.resolve()
    })
    expect(screen.queryByTestId('notifications-push-previews')).toBeNull()
  })

  it('the toggle writes pushMessagePreviews through /cli/settings/update', async () => {
    appSettings.settings = { pushMessagePreviews: true }
    render(<NotificationsSection />)
    const sw = await screen.findByRole('switch', { name: PUSH_PREVIEWS_LABEL })
    fireEvent.click(sw)
    await waitFor(() => expect(appSettings.updates).toEqual([{ pushMessagePreviews: false }]))
    await waitFor(() => expect(sw.getAttribute('aria-checked')).toBe('false'))
  })

  it('a refused write reverts the switch and shows why', async () => {
    appSettings.settings = { pushMessagePreviews: true }
    appSettings.updateError = new Error('settings_update 403: pushMessagePreviews needs Admin')
    render(<NotificationsSection />)
    const sw = await screen.findByRole('switch', { name: PUSH_PREVIEWS_LABEL })
    fireEvent.click(sw)
    await waitFor(() => expect(screen.getByText(/needs Admin/)).toBeTruthy())
    expect(sw.getAttribute('aria-checked')).toBe('true')
  })
})
