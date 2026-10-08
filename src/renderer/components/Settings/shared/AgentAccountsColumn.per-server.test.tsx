// @vitest-environment jsdom
//
// Settings → LLMs "Tokens" is per server (Rosson 2026-10-08): with the
// window on another server it must show THAT server's subscriptions and
// API tokens, never this computer's. 0.45.0 kept every server's list under
// the one `primary` key and never dropped it on a server switch, and a
// failed load kept the previous (local) list on screen.
//
// The REAL connect-host store and primary scope run here; only the HTTP
// layer is mocked, and it records the server each call went to (the
// primary scope's host key read at call time, as the real layer does).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, screen, cleanup, act, waitFor, fireEvent, within } from '@testing-library/react'

type Call = { hostKey: string; route: string; body?: unknown }

const h = vi.hoisted(() => ({
  gets: [] as Call[],
  posts: [] as Call[],
  /** Per-server answer to GET llm/accounts/list. */
  list: null as null | ((hostKey: string) => Promise<unknown>),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (scope: { hostKey: string }, route: string) => {
    const hostKey = scope.hostKey
    if (route === 'llm/accounts/list') h.gets.push({ hostKey, route })
    // Other host-scoped stores (themes, …) reload on a switch too; only the
    // token list is this file's business.
    if (route !== 'llm/accounts/list') return Promise.reject(new Error(`not under test: ${route}`))
    if (!h.list) throw new Error('no list fixture')
    return h.list(hostKey)
  },
  daemonCliPost: async (scope: { hostKey: string }, route: string, body: unknown) => {
    h.posts.push({ hostKey: scope.hostKey, route, body })
    return {}
  },
}))

vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: async () => {} }))

import { AgentAccountsColumn } from './AgentAccountsColumn'
import { __resetConnectHostStoreForTests, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { primaryScope } from '@/kessel/server-scope'
import {
  errorText,
  resetLlmAccountsForTests,
  switchLogin,
  useLlmAccountsStore,
  watchLlmAccounts,
} from '@/stores/llm-accounts'

const REMOTE: ConnectHost = {
  id: 'h-z3',
  label: 'Z3 build',
  hostname: 'z3thon.k2.dev',
  username: 'rosson',
  port: 443,
  secure: true,
  token: 'session-token',
  remember: false,
  lastConnectedAt: null,
}
const REMOTE_KEY = 'z3thon.k2.dev'

function account(id: string, label: string, active: boolean): Record<string, unknown> {
  return {
    id,
    tool: 'claude',
    label,
    kind: 'subscription',
    active,
    state: 'signed_in',
    detail: null,
    email: null,
    org: null,
    plan: 'max',
    expiresAt: null,
    refreshedAt: null,
    lastUsedAt: null,
    createdAt: 1,
    createdBy: null,
    usage: null,
    usageCheckedAt: null,
  }
}

function listDoc(id: string, label: string, spare?: [string, string]): unknown {
  return {
    tools: [
      {
        tool: 'claude',
        display: 'Claude',
        supported: true,
        subscription: true,
        apiKeys: true,
        activeId: id,
        pins: [],
        loginMethod: 'live_swap',
        accounts: spare ? [account(id, label, true), account(spare[0], spare[1], false)] : [account(id, label, true)],
      },
    ],
    logins: [],
    airgap: false,
    switchNote: '',
  }
}

const LOCAL_DOC = listDoc('acc_local', 'my-laptop-max')
const REMOTE_DOC = listDoc('acc_remote', 'z3-build-max', ['acc_remote_spare', 'z3-spare'])

function byHost(local: () => Promise<unknown>, remote: () => Promise<unknown>): (k: string) => Promise<unknown> {
  return (hostKey) => {
    if (hostKey === 'local') return local()
    if (hostKey === REMOTE_KEY) return remote()
    throw new Error(`unexpected server ${hostKey}`)
  }
}

function deferred<T>(): { promise: Promise<T>; resolve: (v: T) => void } {
  let resolve!: (v: T) => void
  const promise = new Promise<T>((r) => {
    resolve = r
  })
  return { promise, resolve }
}

function switchTo(host: 'local' | ConnectHost): void {
  useConnectHostStore.setState({ activeHost: host })
}

function primaryEntry(): ReturnType<typeof useLlmAccountsStore.getState>['entries'][string] | undefined {
  return useLlmAccountsStore.getState().entries[primaryScope().id]
}

const stops: Array<() => void> = []

beforeEach(() => {
  __resetConnectHostStoreForTests()
  resetLlmAccountsForTests()
  h.gets.length = 0
  h.posts.length = 0
  h.list = byHost(
    async () => LOCAL_DOC,
    async () => REMOTE_DOC,
  )
})

afterEach(() => {
  for (const stop of stops.splice(0)) stop()
  cleanup()
  __resetConnectHostStoreForTests()
})

describe('LLM tokens store follows the window server', () => {
  it('a server switch drops this computer\'s list at once and loads the new server\'s', async () => {
    stops.push(watchLlmAccounts(primaryScope()))
    await waitFor(() => expect(primaryEntry()?.doc?.tools[0].accounts[0].id).toBe('acc_local'))
    expect(h.gets).toEqual([{ hostKey: 'local', route: 'llm/accounts/list' }])

    const pending = deferred<unknown>()
    h.list = byHost(
      async () => LOCAL_DOC,
      () => pending.promise,
    )
    switchTo(REMOTE)
    // Synchronously gone: nothing renders the old server's tokens.
    expect(primaryEntry()).toBeUndefined()
    expect(h.gets.at(-1)).toEqual({ hostKey: REMOTE_KEY, route: 'llm/accounts/list' })

    pending.resolve(REMOTE_DOC)
    await waitFor(() => expect(primaryEntry()?.doc?.tools[0].accounts[0].id).toBe('acc_remote'))
    expect(primaryEntry()?.error).toBeNull()
  })

  it('a load from the previous server that answers after the switch never lands', async () => {
    const slowLocal = deferred<unknown>()
    h.list = byHost(
      () => slowLocal.promise,
      async () => REMOTE_DOC,
    )
    stops.push(watchLlmAccounts(primaryScope()))
    expect(h.gets).toEqual([{ hostKey: 'local', route: 'llm/accounts/list' }])

    switchTo(REMOTE)
    await waitFor(() => expect(primaryEntry()?.doc?.tools[0].accounts[0].id).toBe('acc_remote'))

    slowLocal.resolve(LOCAL_DOC)
    await act(async () => {
      await slowLocal.promise
    })
    expect(primaryEntry()?.doc?.tools[0].accounts[0].id).toBe('acc_remote')
  })

  it('a server without tokens (older than 0.45.0) shows why, never this computer\'s list', async () => {
    stops.push(watchLlmAccounts(primaryScope()))
    await waitFor(() => expect(primaryEntry()?.doc).not.toBeNull())

    h.list = byHost(
      async () => LOCAL_DOC,
      async () => {
        throw new Error('route_unclassified')
      },
    )
    switchTo(REMOTE)
    await waitFor(() => expect(primaryEntry()?.error).not.toBeNull())
    expect(primaryEntry()?.doc).toBeNull()
    expect(primaryEntry()?.error).toBe(
      'Z3 build runs a K2 older than 0.45.0, which has no LLM tokens. Update that server to manage its tokens here.',
    )
  })

  it('no page open: a switch drops the list and loads nothing', async () => {
    const stop = watchLlmAccounts(primaryScope())
    await waitFor(() => expect(primaryEntry()?.doc).not.toBeNull())
    stop()
    const before = h.gets.length
    switchTo(REMOTE)
    expect(primaryEntry()).toBeUndefined()
    expect(h.gets).toHaveLength(before)
  })

  it('a gesture goes to the window\'s server at the time of the click', async () => {
    switchTo(REMOTE)
    await switchLogin(primaryScope(), 'acc_remote')
    expect(h.posts).toEqual([{ hostKey: REMOTE_KEY, route: 'llm/accounts/switch', body: { id: 'acc_remote' } }])

    switchTo('local')
    await switchLogin(primaryScope(), 'acc_local')
    expect(h.posts.at(-1)).toEqual({ hostKey: 'local', route: 'llm/accounts/switch', body: { id: 'acc_local' } })
  })

  it('a role refusal and an old server read as sentences, not codes', () => {
    expect(errorText(new Error('role_required'))).toBe("Your login on this server isn't allowed to change LLM tokens.")
    expect(errorText(new Error('route_unclassified'))).toBe(
      'This server runs a K2 older than 0.45.0, which has no LLM tokens.',
    )
    expect(errorText(new Error('route not found: /cli/llm/accounts/switch'))).toBe(
      'This server runs a K2 older than 0.45.0, which has no LLM tokens.',
    )
  })
})

describe('Tokens column follows the window server', () => {
  it('shows the server name and swaps this computer\'s tokens for the remote\'s on a switch', async () => {
    render(<AgentAccountsColumn />)
    await screen.findByTestId('llm-account-acc_local')
    expect(screen.getByTestId('llm-server').textContent).toBe('Server: This computer')

    const pending = deferred<unknown>()
    h.list = byHost(
      async () => LOCAL_DOC,
      () => pending.promise,
    )
    act(() => switchTo(REMOTE))
    expect(screen.queryByTestId('llm-account-acc_local')).toBeNull()
    expect(screen.getByTestId('llm-server').textContent).toBe('Server: Z3 build')
    expect(screen.getByTestId('llm-accounts-loading')).toBeTruthy()

    await act(async () => {
      pending.resolve(REMOTE_DOC)
      await pending.promise
    })
    await screen.findByTestId('llm-account-acc_remote')
    expect(document.body.textContent).not.toContain('my-laptop-max')

    // The page's gestures go to the remote too.
    const spare = screen.getByTestId('llm-account-acc_remote_spare')
    fireEvent.click(within(spare).getByRole('button', { name: 'Use this token' }))
    await waitFor(() =>
      expect(h.posts).toEqual([{ hostKey: REMOTE_KEY, route: 'llm/accounts/switch', body: { id: 'acc_remote_spare' } }]),
    )
  })

  it('a remote that fails shows the error and no tool rows, never the local tokens', async () => {
    render(<AgentAccountsColumn />)
    await screen.findByTestId('llm-account-acc_local')

    h.list = byHost(
      async () => LOCAL_DOC,
      async () => {
        throw new Error('route_unclassified')
      },
    )
    act(() => switchTo(REMOTE))
    const err = await screen.findByTestId('llm-accounts-error')
    expect(err.textContent).toContain('Z3 build runs a K2 older than 0.45.0')
    expect(screen.queryByTestId('llm-account-acc_local')).toBeNull()
    expect(screen.queryByTestId('llm-tool-claude')).toBeNull()
  })
})
