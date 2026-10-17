// @vitest-environment jsdom
// Owner-linked accounts UI (prd-thread-push-notifications-v1 §3; Rosson
// 2026-10-11). 0.46.0 (Rosson 2026-10-17): the per-row "This is me
// (owner)" checkbox on Admin Access is gone; the owner picks their login in
// Settings → General → Your account (YourAccountRow.test.tsx) and Admin
// Access shows an "Owner's account" tag on that row. The list still
// writes POST /cli/owner/accounts with the whole list. Identity only:
// nothing here touches a role.
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

type Json = Record<string, unknown>

const cli = vi.hoisted(() => ({
  ownerAccounts: { ok: true, accounts: [] as string[], aliases: [] as string[] } as Json,
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: Json }>,
  whoami: {} as Json,
  users: [] as Json[],
  fetches: [] as Array<{ url: string; method: string; body: unknown }>,
  pushAvailable: true,
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
    if (route === 'owner/accounts') return cli.ownerAccounts
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Json) => {
    cli.posts.push({ route, body })
    if (route === 'owner/accounts') {
      const accounts = body.accounts as string[]
      cli.ownerAccounts = { ok: true, accounts, aliases: accounts }
      return cli.ownerAccounts
    }
    throw new Error(`unexpected POST ${route}`)
  }),
}))

vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => ({ port: 1, token: 'tok' })),
  getLocalDaemonWs: vi.fn(async () => ({ port: 1, token: 'tok' })),
  daemonHttpBase: () => 'http://daemon.test',
  invalidateDaemonWs: vi.fn(),
}))

vi.mock('./useNotifyPrefs', async (orig) => ({
  ...(await orig<typeof import('./useNotifyPrefs')>()),
  threadPushAvailable: () => cli.pushAvailable,
}))

import { OWNER_ACCOUNT_TAG, OwnerAccountTag, ownerLinkAccess, useOwnerAccounts } from './OwnerAccountToggle'
import { isYouAuthor, toggledOwnerAccounts, parseOwnerAccounts } from './notify-api'
import { K2ConnectSection } from './K2ConnectSection'

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } })
}

beforeEach(() => {
  cli.ownerAccounts = { ok: true, accounts: [], aliases: [] }
  cli.gets = []
  cli.posts = []
  cli.fetches = []
  cli.whoami = { owner: true, role: 'owner' }
  cli.users = [
    { username: 'z3thon', disabled: false, role: 'admin' },
    { username: 'gina', disabled: false, role: 'member' },
  ]
  cli.pushAvailable = true
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string, init?: RequestInit) => {
      const method = init?.method ?? 'GET'
      cli.fetches.push({ url, method, body: init?.body ? JSON.parse(String(init.body)) : undefined })
      const path = new URL(url).pathname
      if (path === '/cli/auth/whoami') return json(cli.whoami)
      if (path === '/cli/users') return json({ users: cli.users })
      if (path === '/cli/users/policy') return json({ minLength: 8 })
      throw new Error(`unexpected fetch ${method} ${path}`)
    }),
  )
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe('ownerLinkAccess', () => {
  it('edit for the owner token only; read for Admin+ logins; hidden for Members', () => {
    expect(ownerLinkAccess(true, 'owner')).toBe('edit')
    // An Owner-ROLE Connect login is not the owner token: the daemon 403s it.
    expect(ownerLinkAccess(false, 'owner')).toBe('read')
    expect(ownerLinkAccess(false, 'admin')).toBe('read')
    expect(ownerLinkAccess(false, 'member')).toBe('hidden')
    expect(ownerLinkAccess(false, null)).toBe('hidden')
  })
})

describe('OwnerAccountTag', () => {
  it('renders the tag only on the linked row', () => {
    const { container, rerender } = render(<OwnerAccountTag username="gina" linked={false} />)
    expect(container.innerHTML).toBe('')
    rerender(<OwnerAccountTag username="z3thon" linked />)
    expect(screen.getByTestId('owner-account-tag-z3thon').textContent).toBe(OWNER_ACCOUNT_TAG)
    expect(OWNER_ACCOUNT_TAG).toBe("Owner's account")
  })
})

function Harness({ enabled }: { enabled: boolean }): React.JSX.Element {
  const s = useOwnerAccounts(enabled)
  return (
    <div>
      {s.loaded &&
        ['z3thon', 'gina'].map((u) => (
          <button key={u} type="button" data-testid={`flip-${u}`} data-linked={String(s.isLinked(u))} onClick={() => void s.setLinked(u, !s.isLinked(u))}>
            {u}
          </button>
        ))}
      {s.loaded && (
        <button type="button" data-testid="set-gina" onClick={() => void s.setAccounts(['gina'])}>
          only gina
        </button>
      )}
      <span data-testid="aliases">{s.aliases.join(',')}</span>
    </div>
  )
}

describe('useOwnerAccounts', () => {
  it('loads the list; a flip POSTs the whole list; setAccounts replaces it', async () => {
    cli.ownerAccounts = { ok: true, accounts: ['gina'], aliases: ['gina'] }
    render(<Harness enabled />)
    const z3 = await screen.findByTestId('flip-z3thon')
    expect(z3.dataset.linked).toBe('false')
    expect(screen.getByTestId('flip-gina').dataset.linked).toBe('true')

    fireEvent.click(z3)
    await waitFor(() => expect(cli.posts).toHaveLength(1))
    expect(cli.posts[0]).toEqual({ route: 'owner/accounts', body: { accounts: ['gina', 'z3thon'] } })
    await waitFor(() => expect(screen.getByTestId('flip-z3thon').dataset.linked).toBe('true'))
    expect(screen.getByTestId('aliases').textContent).toBe('gina,z3thon')

    fireEvent.click(screen.getByTestId('set-gina'))
    await waitFor(() => expect(cli.posts).toHaveLength(2))
    expect(cli.posts[1]).toEqual({ route: 'owner/accounts', body: { accounts: ['gina'] } })
    await waitFor(() => expect(screen.getByTestId('flip-z3thon').dataset.linked).toBe('false'))
  })

  it('disabled: fetches nothing', async () => {
    render(<Harness enabled={false} />)
    await Promise.resolve()
    expect(cli.gets).toEqual([])
    expect(screen.queryByTestId('flip-z3thon')).toBeNull()
  })
})

describe('Settings → Admin Access → Users: the "Owner\'s account" tag', () => {
  it('owner token: no "This is me (owner)" checkbox; the linked row shows the tag', async () => {
    cli.ownerAccounts = { ok: true, accounts: ['z3thon'], aliases: ['z3thon'] }
    render(<K2ConnectSection panel="people" />)
    expect((await screen.findByTestId('owner-account-tag-z3thon')).textContent).toBe("Owner's account")
    expect(screen.queryByTestId('owner-account-tag-gina')).toBeNull()
    expect(screen.queryByLabelText('z3thon: This is me (owner)')).toBeNull()
    expect(screen.queryByText('This is me (owner)')).toBeNull()
    expect(screen.queryByTestId('owner-link-explainer')).toBeNull()
    expect(cli.posts).toEqual([])
  })

  it('no linked login: no tag anywhere', async () => {
    render(<K2ConnectSection panel="people" />)
    await screen.findByText('z3thon')
    await waitFor(() => expect(cli.gets).toContain('owner/accounts'))
    expect(document.querySelector('[data-testid^="owner-account-tag-"]')).toBeNull()
  })

  it('Admin login sees the tag too (read-only data), never POSTs', async () => {
    cli.whoami = { owner: false, role: 'admin', username: 'sam' }
    cli.ownerAccounts = { ok: true, accounts: ['z3thon'], aliases: ['z3thon'] }
    render(<K2ConnectSection panel="people" />)
    await screen.findByTestId('owner-account-tag-z3thon')
    expect(cli.posts).toEqual([])
  })

  it('Member login: no tag, no owner/accounts read', async () => {
    cli.whoami = { owner: false, role: 'member', username: 'gina' }
    render(<K2ConnectSection panel="people" />)
    await screen.findByText('User management is handled by an administrator.')
    expect(document.querySelector('[data-testid^="owner-account-tag-"]')).toBeNull()
    expect(cli.gets).not.toContain('owner/accounts')
  })

  it('a server without Thread push shows no tag', async () => {
    cli.pushAvailable = false
    cli.ownerAccounts = { ok: true, accounts: ['z3thon'], aliases: ['z3thon'] }
    render(<K2ConnectSection panel="people" />)
    await screen.findByText('z3thon')
    expect(screen.queryByTestId('owner-account-tag-z3thon')).toBeNull()
    expect(cli.gets).not.toContain('owner/accounts')
  })
})

describe('notify-api owner-account helpers', () => {
  it('toggledOwnerAccounts adds/removes, lowercases, sorts, dedupes', () => {
    expect(toggledOwnerAccounts(['gina'], 'Z3thon', true)).toEqual(['gina', 'z3thon'])
    expect(toggledOwnerAccounts(['gina', 'z3thon'], 'z3thon', true)).toEqual(['gina', 'z3thon'])
    expect(toggledOwnerAccounts(['gina', 'z3thon'], 'z3thon', false)).toEqual(['gina'])
  })

  it('parseOwnerAccounts tolerates junk', () => {
    expect(parseOwnerAccounts({ accounts: ['Z3thon', 3, ''], aliases: 'x' })).toEqual({ accounts: ['z3thon'], aliases: [] })
  })

  it('isYouAuthor: owner always; linked logins only when the viewer is the owner person', () => {
    const asOwner = { principal: 'owner', aliases: ['z3thon'] }
    expect(isYouAuthor('owner', null)).toBe(true)
    expect(isYouAuthor('z3thon', asOwner)).toBe(true)
    expect(isYouAuthor('Z3thon', asOwner)).toBe(true)
    expect(isYouAuthor('gina', asOwner)).toBe(false)
    expect(isYouAuthor('z3thon', { principal: 'gina', aliases: ['z3thon'] })).toBe(false)
    expect(isYouAuthor('z3thon', null)).toBe(false)
  })
})
