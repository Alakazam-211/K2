// @vitest-environment jsdom
// Settings → General → You → "Your account" (0.46.0, Rosson 2026-10-17):
// a dropdown of the server's Owner/Admin Connect logins plus "None". It
// writes the existing owner-linked list (`POST /cli/owner/accounts`) and
// keeps today's floors: the owner token edits, Admin/Owner-role logins see
// it read-only, Members don't see it.

import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

type Json = Record<string, unknown>

const cli = vi.hoisted(() => ({
  whoami: {} as Json,
  users: [] as Json[],
  ownerAccounts: { ok: true, accounts: [] as string[], aliases: [] as string[] } as Json,
  gets: [] as string[],
  posts: [] as Array<{ route: string; body: Json }>,
  pushAvailable: true,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    cli.gets.push(route)
    if (route === 'auth/whoami') return cli.whoami
    if (route === 'users') return { users: cli.users }
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

vi.mock('@/kessel/server-scope', async (orig) => {
  const real = await orig<typeof import('@/kessel/server-scope')>()
  return { ...real, primaryScope: () => ({ label: 'This computer', isRemote: false }) }
})

vi.mock('./useNotifyPrefs', () => ({ threadPushAvailable: () => cli.pushAvailable }))

import {
  YourAccountRow,
  accountChoices,
  nextOwnerAccounts,
  yourAccountLabel,
  yourAccountValue,
  MANY_ACCOUNTS,
  NO_ACCOUNT,
} from './YourAccountRow'

beforeEach(() => {
  cli.whoami = { owner: true, role: 'owner', username: null }
  cli.users = [
    { username: 'z3thon', role: 'admin', disabled: false },
    { username: 'julie', role: 'owner', disabled: false },
    { username: 'gina', role: 'member', disabled: false },
    { username: 'old', role: 'admin', disabled: true },
  ]
  cli.ownerAccounts = { ok: true, accounts: [], aliases: [] }
  cli.gets = []
  cli.posts = []
  cli.pushAvailable = true
})

afterEach(() => cleanup())

function trigger(): HTMLButtonElement {
  return screen.getByLabelText('Your account') as HTMLButtonElement
}

function menuLabels(): string[] {
  const menu = screen.getByTestId('setting-dropdown-menu')
  return Array.from(menu.querySelectorAll('button')).map((b) => b.querySelector('span')?.textContent ?? '')
}

async function openMenu(): Promise<void> {
  act(() => void fireEvent.click(trigger()))
  await screen.findByTestId('setting-dropdown-menu')
}

describe('Your account helpers', () => {
  it('offers Owner and Admin logins only, sorted', () => {
    expect(accountChoices([
      { username: 'z3thon', role: 'admin' },
      { username: 'gina', role: 'member' },
      { username: 'julie', role: 'owner' },
    ]).map((c) => c.username)).toEqual(['julie', 'z3thon'])
  })

  it('labels and values: none, one, several', () => {
    expect(yourAccountLabel([])).toBe('None')
    expect(yourAccountValue([])).toBe(NO_ACCOUNT)
    expect(yourAccountLabel(['z3thon'])).toBe('z3thon')
    expect(yourAccountValue(['z3thon'])).toBe('z3thon')
    expect(yourAccountLabel(['z3thon', 'phone'])).toBe('phone + 1 more')
    expect(yourAccountValue(['phone', 'z3thon'])).toBe(MANY_ACCOUNTS)
  })

  it('picking: a login replaces the list; the shown one or "+ more" keeps several; None clears', () => {
    expect(nextOwnerAccounts([], 'z3thon')).toEqual(['z3thon'])
    expect(nextOwnerAccounts(['z3thon'], 'z3thon')).toBeNull()
    expect(nextOwnerAccounts(['phone', 'z3thon'], MANY_ACCOUNTS)).toBeNull()
    expect(nextOwnerAccounts(['phone', 'z3thon'], 'phone')).toBeNull()
    expect(nextOwnerAccounts(['phone', 'z3thon'], 'julie')).toEqual(['julie'])
    expect(nextOwnerAccounts(['z3thon'], NO_ACCOUNT)).toEqual([])
    expect(nextOwnerAccounts([], NO_ACCOUNT)).toBeNull()
  })
})

describe('Settings → General → Your account', () => {
  it('owner token: lists None + the Owner/Admin logins; picking one posts ownerAccounts = [it]', async () => {
    render(<YourAccountRow />)
    await screen.findByTestId('your-account-row')
    expect(trigger().textContent).toContain('None')
    expect(trigger().disabled).toBe(false)
    await openMenu()
    expect(menuLabels()).toEqual(['None', 'julie', 'z3thon'])
    act(() => void fireEvent.click(within(screen.getByTestId('setting-dropdown-menu')).getByText('z3thon')))
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'owner/accounts', body: { accounts: ['z3thon'] } }]))
    await waitFor(() => expect(trigger().textContent).toContain('z3thon'))
  })

  it('None clears the link', async () => {
    cli.ownerAccounts = { ok: true, accounts: ['z3thon'], aliases: ['z3thon'] }
    render(<YourAccountRow />)
    await waitFor(() => expect(trigger().textContent).toContain('z3thon'))
    await openMenu()
    act(() => void fireEvent.click(within(screen.getByTestId('setting-dropdown-menu')).getByText('None')))
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'owner/accounts', body: { accounts: [] } }]))
  })

  it('several linked: shows "z3thon + 1 more", explains it, and keeps them until a different login is picked', async () => {
    cli.ownerAccounts = { ok: true, accounts: ['gina', 'z3thon'], aliases: ['gina', 'z3thon'] }
    render(<YourAccountRow />)
    await waitFor(() => expect(trigger().textContent).toContain('gina + 1 more'))
    expect(screen.getByTestId('your-account-hint').textContent).toContain('they stay until you pick a different one')
    await openMenu()
    act(() => void fireEvent.click(within(screen.getByTestId('setting-dropdown-menu')).getByText('gina + 1 more')))
    expect(cli.posts).toEqual([])
    await openMenu()
    act(() => void fireEvent.click(within(screen.getByTestId('setting-dropdown-menu')).getByText('julie')))
    await waitFor(() => expect(cli.posts).toEqual([{ route: 'owner/accounts', body: { accounts: ['julie'] } }]))
  })

  it('Admin login: read-only, never posts', async () => {
    cli.whoami = { owner: false, role: 'admin', username: 'z3thon' }
    cli.ownerAccounts = { ok: true, accounts: ['z3thon'], aliases: ['z3thon'] }
    render(<YourAccountRow />)
    await waitFor(() => expect(trigger().textContent).toContain('z3thon'))
    expect(trigger().disabled).toBe(true)
    expect(screen.getByTestId('your-account-hint').textContent).toContain('Only the owner')
    expect(cli.posts).toEqual([])
  })

  it('Member login: hidden, and nothing past whoami is read', async () => {
    cli.whoami = { owner: false, role: 'member', username: 'gina' }
    render(<YourAccountRow />)
    await waitFor(() => expect(cli.gets).toEqual(['auth/whoami']))
    expect(screen.queryByTestId('your-account-row')).toBeNull()
  })

  it('a server without owner-linked accounts shows nothing and reads nothing', async () => {
    cli.pushAvailable = false
    render(<YourAccountRow />)
    await Promise.resolve()
    expect(screen.queryByTestId('your-account-row')).toBeNull()
    expect(cli.gets).toEqual([])
  })
})
