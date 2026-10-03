// @vitest-environment jsdom
// Home M4 — render tests for a remote room's failure states (MS45): the
// pool drives the banner, each state has its one button, the buttons act
// on that server only, and a failing room never shows anything else.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const h = vi.hoisted(() => ({ invokes: [] as string[] }))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (cmd: string) => {
    h.invokes.push(cmd)
    return null
  }),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { createElement, act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { RoomFailureGate, RoomFailureView, roomFailureActions, type RoomFailureActions } from './RoomFailure'
import { hostPool } from '@/lib/host-pool-instance'
import type { HostEntry } from '@/lib/host-pool'
import { roomFailure } from '@/lib/room-failure'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const KEY = 'dtl.k2.dev'
const DTL: ConnectHost = {
  id: 'id-dtl',
  label: 'dtl',
  hostname: 'dtl.k2.dev',
  port: 443,
  secure: true,
  username: 'rosson',
  token: '',
  remember: true,
  lastConnectedAt: null,
}

function entry(patch: Partial<HostEntry>): HostEntry {
  return {
    hostKey: KEY,
    saved: true,
    hostId: 'id-dtl',
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.41.6', protocol: 1, instanceId: 'i1', features: [], at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: null,
    ...patch,
  }
}

function setEntry(e: HostEntry): void {
  hostPool.store.setState({ entries: { [KEY]: e } })
}

function spyActions(): RoomFailureActions & { calls: string[] } {
  const calls: string[] = []
  return {
    calls,
    retry: () => calls.push('retry'),
    signIn: () => calls.push('sign-in'),
    openServer: () => calls.push('open-server'),
  }
}

function mount(node: React.ReactElement): { el: HTMLElement; root: Root } {
  const el = document.createElement('div')
  document.body.appendChild(el)
  const root = createRoot(el)
  act(() => root.render(node))
  return { el, root }
}

function button(el: HTMLElement): HTMLButtonElement {
  const b = el.querySelector<HTMLButtonElement>('[data-room-failure-action]')
  if (!b) throw new Error('no action button rendered')
  return b
}

beforeEach(() => {
  h.invokes = []
  hostPool.store.setState({ entries: {} })
  useConnectHostStore.setState({ hosts: [DTL], activeHost: 'local' })
})

const CASES: Array<[string, Partial<HostEntry>, string, string, string]> = [
  ['offline', { reach: 'offline', boot: null }, 'dtl is offline. Retrying…', 'Retry', 'retry'],
  ['restarting', { reach: 'starting' }, 'dtl is restarting…', 'Retry', 'retry'],
  ['signin-required', { auth: 'signin-required' }, 'Sign in to dtl.', 'Sign in to dtl', 'sign-in'],
  ['kicked', { auth: 'kicked' }, 'Removed from dtl. Sign in again.', 'Sign in to dtl', 'sign-in'],
  [
    'version-too-old',
    { boot: { phase: 'ready', ready: true, version: '0.40.30', protocol: 1, instanceId: 'i', features: [], at: 1 } },
    'dtl is on v0.40.30. This room needs v0.41.0 or newer.',
    'Open dtl’s server',
    'open-server',
  ],
  ['no-access', { role: 'viewer' }, 'You don’t have access to this agent on dtl.', 'Open dtl’s server', 'open-server'],
]

describe('RoomFailureView', () => {
  for (const [kind, patch, title, label, action] of CASES) {
    it(`${kind}: shows "${title}" and one "${label}" button that calls ${action}`, () => {
      const failure = roomFailure({ serverLabel: 'dtl', saved: true, entry: entry(patch) })
      if (!failure) throw new Error(`no failure for ${kind}`)
      const actions = spyActions()
      const { el, root } = mount(createElement(RoomFailureView, { failure, actions }))
      const alert = el.querySelector('[role="alert"]')
      if (!alert) throw new Error('no alert rendered')
      expect(alert.getAttribute('data-room-failure')).toBe(kind)
      expect(alert.textContent).toContain(title)
      expect(el.querySelectorAll('[data-room-failure-action]')).toHaveLength(1)
      expect(button(el).textContent).toBe(label)
      act(() => button(el).click())
      expect(actions.calls).toEqual([action])
      act(() => root.unmount())
      el.remove()
    })
  }
})

describe('RoomFailureGate (the pool drives it)', () => {
  it('a usable server renders the room as is', () => {
    setEntry(entry({}))
    const actions = spyActions()
    const { el, root } = mount(
      createElement(RoomFailureGate, { hostKey: KEY, actions }, createElement('p', { id: 'pane' }, 'B pane')),
    )
    expect(el.querySelector('[role="alert"]')).toBeNull()
    expect(el.querySelector('#pane')?.textContent).toBe('B pane')
    act(() => root.unmount())
  })

  it('B going offline shows the banner over the last frame, dimmed and inert; back live removes it', () => {
    setEntry(entry({}))
    const actions = spyActions()
    const { el, root } = mount(
      createElement(RoomFailureGate, { hostKey: KEY, actions }, createElement('p', { id: 'pane' }, 'B pane')),
    )
    act(() => setEntry(entry({ reach: 'offline', boot: null })))
    expect(el.querySelector('[data-room-failure]')?.getAttribute('data-room-failure')).toBe('offline')
    // The last frame stays, but cannot be used.
    const pane = el.querySelector('#pane')
    if (!pane) throw new Error('the last frame must stay')
    const wrap = pane.parentElement
    if (!wrap) throw new Error('no wrapper')
    expect(wrap.hasAttribute('inert')).toBe(true)
    expect(wrap.className).toContain('pointer-events-none')
    act(() => setEntry(entry({})))
    expect(el.querySelector('[role="alert"]')).toBeNull()
    act(() => root.unmount())
  })

  it('a server removed from this computer: the removed copy, no button', () => {
    useConnectHostStore.setState({ hosts: [] })
    const { el, root } = mount(createElement(RoomFailureGate, { hostKey: KEY }, createElement('p', null, 'x')))
    expect(el.querySelector('[data-room-failure]')?.getAttribute('data-room-failure')).toBe('removed')
    expect(el.querySelector('[data-room-failure-action]')).toBeNull()
    act(() => root.unmount())
  })
})

describe('roomFailureActions act on that server only (MS5, MS36)', () => {
  it('Retry checks that server', () => {
    const check = vi.spyOn(hostPool, 'check').mockResolvedValue(entry({}))
    roomFailureActions(KEY).retry()
    expect(check).toHaveBeenCalledWith(KEY)
    check.mockRestore()
  })

  it('Sign in opens the non-switching sign-in for that server, and deletes nothing', () => {
    const signIn = vi.fn()
    const pickHost = vi.fn()
    useConnectHostStore.setState({ signInForManagement: signIn, pickHost })
    setEntry(entry({ auth: 'kicked' }))
    roomFailureActions(KEY).signIn()
    expect(signIn).toHaveBeenCalledTimes(1)
    expect(signIn.mock.calls[0][0].id).toBe('id-dtl')
    expect(pickHost).not.toHaveBeenCalled()
    expect(h.invokes.filter((c) => c === 'k2_secret_delete' || c === 'connect_cli_token_delete')).toEqual([])
  })

  it('Sign in on a server that needs a new password opens the rotate step without switching', () => {
    const rotate = vi.fn()
    useConnectHostStore.setState({ requestPasswordRotation: rotate })
    setEntry(entry({ auth: 'rotate-required' }))
    roomFailureActions(KEY).signIn()
    expect(rotate).toHaveBeenCalledTimes(1)
    expect(rotate.mock.calls[0][0].id).toBe('id-dtl')
    expect(rotate.mock.calls[0][1]).toEqual({ activate: false })
  })

  it('Open B’s server switches this window to B — only on that click', () => {
    const pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost })
    roomFailureActions(KEY).openServer()
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(pickHost.mock.calls[0][0].id).toBe('id-dtl')
  })
})
