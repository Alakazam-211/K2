// @vitest-environment jsdom
//
// Hosted web client: no long-lived secret in persistent browser storage.
// Session tokens live in memory (+ sessionStorage for a reload of the same
// tab); passwords and refresh tokens are never kept; boot deletes the old
// `k2_secret:*` keys and the old localStorage token copy. The desktop path
// (real Tauri keychain) is untouched.
//
// `@tauri-apps/api/core` is the REAL web shim here (wrapped in a spy), the
// same module vite.config.web.ts aliases in the browser build.

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { readFileSync } from 'fs'
import { resolve } from 'path'
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'

const isWebMock = vi.hoisted(() => vi.fn(() => true))
vi.mock('@/lib/is-web', () => ({ isWebClient: () => isWebMock() }))

vi.mock('@tauri-apps/api/core', async () => {
  const shim = await vi.importActual<typeof import('@/shims/tauri-core')>('@/shims/tauri-core')
  return { ...shim, invoke: vi.fn(shim.invoke) }
})
vi.mock('@/components/TopBar/GateChrome', () => ({ default: () => <div data-testid="gate-chrome" /> }))

import { invoke } from '@tauri-apps/api/core'
import { __clearWebMemorySecretsForTests, WEB_MEMORY_SECRET_SERVICE } from '@/shims/tauri-core'
import { RemoteSignIn } from '@/components/RemoteSignIn'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  K2_CONNECT_KEYCHAIN_SERVICE,
  K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
  saveRememberedPassword,
  resolvePassword,
  type ConnectHost,
} from '@/stores/connect-host'
import { bootWebHostIfNeeded, WEB_ORIGIN_HOST_ID } from './boot-host'
import { WEB_SESSION_TOKEN_KEY, purgeStoredWebSecrets } from './session-token'

const invokeMock = vi.mocked(invoke)
const TOKEN = 'sess-web-7f3a'
const PASSWORD = 'pw-web-9c1d'

function localEntries(): Array<[string, string]> {
  const out: Array<[string, string]> = []
  for (let i = 0; i < localStorage.length; i++) {
    const k = localStorage.key(i)!
    out.push([k, localStorage.getItem(k)!])
  }
  return out
}

function activeToken(): string {
  const a = useConnectHostStore.getState().activeHost
  if (a === 'local') throw new Error('web client must never be on the local host')
  return a.token
}

/** A page load: in-memory state gone, browser storage as it was. */
function reloadPage(): void {
  __resetConnectHostStoreForTests()
  __clearWebMemorySecretsForTests()
}

beforeEach(() => {
  isWebMock.mockReturnValue(true)
  localStorage.clear()
  sessionStorage.clear()
  reloadPage()
  invokeMock.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  isWebMock.mockReturnValue(true)
})

describe('web sign-in keeps no secret in localStorage', () => {
  it('after sign-in: no k2_secret:* key, no token or password in localStorage; the tab keeps the token', async () => {
    const fetchMock = vi.fn(async () => ({
      ok: true,
      status: 200,
      headers: new Headers(),
      json: async () => ({ token: TOKEN, username: 'alice', expiresAt: '2026-12-01T00:00:00+00:00', mustChangePassword: false }),
      text: async () => '',
    }))
    vi.stubGlobal('fetch', fetchMock)

    bootWebHostIfNeeded()
    const host = useConnectHostStore.getState().activeHost as ConnectHost
    expect(host.id).toBe(WEB_ORIGIN_HOST_ID)
    expect(host.remember).toBe(false)

    render(<RemoteSignIn host={host} />)
    // No "remember password" on the web.
    expect(screen.queryByText(/Remember password/)).toBeNull()

    const [userInput, pwInput] = Array.from(document.querySelectorAll('input')).filter(
      (i) => i.type === 'text' || i.type === 'password',
    )
    fireEvent.change(userInput, { target: { value: 'alice' } })
    fireEvent.change(pwInput, { target: { value: PASSWORD } })
    fireEvent.submit(pwInput.closest('form')!)

    await waitFor(() => expect(activeToken()).toBe(TOKEN))
    // finalize (forget/remember password) runs after the token commits.
    await waitFor(() =>
      expect(useConnectHostStore.getState().hosts.find((h) => h.id === WEB_ORIGIN_HOST_ID)?.lastConnectedAt).not.toBeNull(),
    )

    const entries = localEntries()
    expect(entries.filter(([k]) => k.startsWith('k2_secret:'))).toEqual([])
    expect(entries.filter(([, v]) => v.includes(TOKEN) || v.includes(PASSWORD))).toEqual([])
    expect(localStorage.getItem(WEB_SESSION_TOKEN_KEY)).toBeNull()
    // Tab-scoped copy for a reload of this tab.
    expect(sessionStorage.getItem(WEB_SESSION_TOKEN_KEY)).toBe(TOKEN)
    // The password reached no storage of any kind.
    for (let i = 0; i < sessionStorage.length; i++) {
      expect(sessionStorage.getItem(sessionStorage.key(i)!)).not.toContain(PASSWORD)
    }
    expect(invokeMock).not.toHaveBeenCalledWith('k2_secret_set', expect.objectContaining({ service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE }))
  })
})

describe('load-time cleanup', () => {
  it('boot removes every k2_secret:* key and the old token copy, keeps the rest, and adopts none of them', () => {
    localStorage.setItem(`k2_secret:${K2_CONNECT_KEYCHAIN_SERVICE}:${WEB_ORIGIN_HOST_ID}`, 'old-host-token')
    localStorage.setItem(`k2_secret:${K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE}:${WEB_ORIGIN_HOST_ID}`, 'old-password')
    localStorage.setItem('k2_secret:dev.k2.connect.account:session-refresh-token', 'old-refresh')
    localStorage.setItem('k2_secret:com.k2so.connect.host-token:other', 'older-token')
    localStorage.setItem(WEB_SESSION_TOKEN_KEY, 'old-session')
    localStorage.setItem('k2.unrelated-pref', 'keep-me')

    bootWebHostIfNeeded()

    expect(localEntries().filter(([k]) => k.startsWith('k2_secret:'))).toEqual([])
    expect(localStorage.getItem(WEB_SESSION_TOKEN_KEY)).toBeNull()
    expect(localStorage.getItem('k2.unrelated-pref')).toBe('keep-me')
    expect(activeToken()).toBe('')
  })

  it('purgeStoredWebSecrets reports exactly what it removed', () => {
    localStorage.setItem('k2_secret:a:b', 'x')
    localStorage.setItem(WEB_SESSION_TOKEN_KEY, 'y')
    localStorage.setItem('k2.theme', 'dark')
    expect(purgeStoredWebSecrets().sort()).toEqual(['k2_secret:a:b', WEB_SESSION_TOKEN_KEY].sort())
    expect(localEntries()).toEqual([['k2.theme', 'dark']])
  })
})

describe('reload', () => {
  it('a reload of the same tab keeps the session (sessionStorage)', () => {
    sessionStorage.setItem(WEB_SESSION_TOKEN_KEY, TOKEN)
    bootWebHostIfNeeded()
    expect(activeToken()).toBe(TOKEN)
  })

  it('a new tab with only an old localStorage copy asks to sign in again', () => {
    localStorage.setItem(WEB_SESSION_TOKEN_KEY, 'old-session')
    localStorage.setItem(`k2_secret:${K2_CONNECT_KEYCHAIN_SERVICE}:${WEB_ORIGIN_HOST_ID}`, 'old-host-token')
    bootWebHostIfNeeded()
    expect(activeToken()).toBe('')
  })
})

describe('web keychain stand-in', () => {
  it('keeps a session token in memory only, until the page reloads', async () => {
    expect(WEB_MEMORY_SECRET_SERVICE).toBe(K2_CONNECT_KEYCHAIN_SERVICE)
    await invoke('k2_secret_set', { service: K2_CONNECT_KEYCHAIN_SERVICE, account: 'h1', secret: TOKEN })
    expect(await invoke('k2_secret_get', { service: K2_CONNECT_KEYCHAIN_SERVICE, account: 'h1' })).toBe(TOKEN)
    expect(localStorage.length).toBe(0)
    reloadPage()
    expect(await invoke('k2_secret_get', { service: K2_CONNECT_KEYCHAIN_SERVICE, account: 'h1' })).toBeNull()
  })

  it('never keeps a password or the account session', async () => {
    await invoke('k2_secret_set', { service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE, account: 'h1', secret: PASSWORD })
    await invoke('k2_secret_set', { service: 'dev.k2.connect.account', account: 'session', secret: 'refresh-1' })
    await invoke('k2_account_session_set', { refreshToken: 'refresh-2', email: 'a@b.c' })
    expect(await invoke('k2_secret_get', { service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE, account: 'h1' })).toBeNull()
    expect(await invoke('k2_secret_get', { service: 'dev.k2.connect.account', account: 'session' })).toBeNull()
    expect(await invoke('k2_account_session_get')).toBeNull()
    expect(localStorage.length).toBe(0)
    expect(sessionStorage.length).toBe(0)
  })

  it('delete forgets the in-memory token', async () => {
    await invoke('k2_secret_set', { service: K2_CONNECT_KEYCHAIN_SERVICE, account: 'h1', secret: TOKEN })
    await invoke('k2_secret_delete', { service: K2_CONNECT_KEYCHAIN_SERVICE, account: 'h1' })
    expect(await invoke('k2_secret_get', { service: K2_CONNECT_KEYCHAIN_SERVICE, account: 'h1' })).toBeNull()
  })

  it('remembered-password helpers are inert on the web and un-mark the host', async () => {
    useConnectHostStore.getState().addHost({
      id: 'h1', label: 'h1', hostname: 'h1.k2.dev', username: 'alice', port: 443, secure: true,
      token: '', remember: true, lastConnectedAt: null,
    })
    expect(await saveRememberedPassword('h1', PASSWORD)).toBeNull()
    expect(useConnectHostStore.getState().hosts.find((h) => h.id === 'h1')?.remember).toBe(false)
    expect(await resolvePassword('h1')).toBeNull()
    expect(invokeMock).not.toHaveBeenCalledWith('k2_secret_set', expect.objectContaining({ service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE }))
  })
})

describe('desktop path is untouched', () => {
  it('only the web Vite config swaps @tauri-apps/api/core for the shim', () => {
    const root = resolve(__dirname, '../../..')
    const desktop = readFileSync(resolve(root, 'vite.config.ts'), 'utf8')
    const web = readFileSync(resolve(root, 'vite.config.web.ts'), 'utf8')
    expect(desktop).not.toContain('tauri-core')
    expect(web).toContain("'@tauri-apps/api/core': shim('tauri-core.ts')")
  })

  it('desktop still stores a remembered password in the OS keychain and offers the toggle', async () => {
    isWebMock.mockReturnValue(false)
    expect(await saveRememberedPassword('h1', PASSWORD)).toBeNull()
    expect(invokeMock).toHaveBeenCalledWith('k2_secret_set', {
      service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
      account: 'h1',
      secret: PASSWORD,
    })
    await resolvePassword('h1')
    expect(invokeMock).toHaveBeenCalledWith('k2_secret_get', {
      service: K2_CONNECT_PASSWORD_KEYCHAIN_SERVICE,
      account: 'h1',
    })
    render(
      <RemoteSignIn
        host={{ id: 'h2', label: 'h2', hostname: 'h2.k2.dev', username: 'alice', port: 443, secure: true, token: '', remember: false, lastConnectedAt: null }}
      />,
    )
    expect(screen.getByText('Remember password (stored in your OS keychain)')).toBeTruthy()
  })
})
