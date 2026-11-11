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

const KEY = 'dharma.k2.dev'
const DHARMA: ConnectHost = {
  id: 'id-dharma',
  label: 'dharma',
  hostname: 'dharma.k2.dev',
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
    hostId: 'id-dharma',
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
  useConnectHostStore.setState({ hosts: [DHARMA], activeHost: 'local' })
})

const CASES: Array<[string, Partial<HostEntry>, string, string, string]> = [
  ['offline', { reach: 'offline', boot: null }, 'dharma is offline. Retrying…', 'Retry', 'retry'],
  ['restarting', { reach: 'starting' }, 'dharma is restarting…', 'Retry', 'retry'],
  ['signin-required', { auth: 'signin-required' }, 'Sign in to dharma.', 'Sign in to dharma', 'sign-in'],
  ['kicked', { auth: 'kicked' }, 'Removed from dharma. Sign in again.', 'Sign in to dharma', 'sign-in'],
  [
    'version-too-old',
    { boot: { phase: 'ready', ready: true, version: '0.40.30', protocol: 1, instanceId: 'i', features: [], at: 1 } },
    'dharma is on v0.40.30. This room needs v0.41.0 or newer.',
    'Open dharma’s server',
    'open-server',
  ],
  ['no-access', { role: 'viewer' }, 'You don’t have access to this agent on dharma.', 'Open dharma’s server', 'open-server'],
]

describe('RoomFailureView', () => {
  for (const [kind, patch, title, label, action] of CASES) {
    it(`${kind}: shows "${title}" and one "${label}" button that calls ${action}`, () => {
      const failure = roomFailure({ serverLabel: 'dharma', saved: true, entry: entry(patch) })
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

describe('RoomFailureView look (Terminal strip, Glass tile; tokens only)', () => {
  const offline = (label = 'dharma') => {
    const f = roomFailure({
      serverLabel: label,
      saved: true,
      entry: entry({ reach: 'offline', boot: null }),
      nextCheckAt: 5_000,
      now: 0,
    })
    if (!f) throw new Error('no failure')
    return f
  }

  it('is a Glass tile and a Terminal strip on the surface, painted from tokens', () => {
    const { el, root } = mount(createElement(RoomFailureView, { failure: offline(), actions: spyActions() }))
    const alert = el.querySelector<HTMLElement>('[role="alert"]')!
    // Glass paints [data-material="tile"] (radius, edge, sheen, shadow);
    // Terminal ignores it and keeps the square strip with a bottom rule.
    expect(alert.getAttribute('data-material')).toBe('tile')
    expect(alert.className).toContain('bg-[var(--color-bg-surface)]')
    expect(alert.className).toContain('rounded-[var(--radius-box)]')
    expect(alert.className).toContain('border-b border-[var(--color-border)]')
    // No raw colours anywhere; the banner's own text uses the type scale
    // (the shared Button owns its size).
    expect(alert.outerHTML).not.toMatch(/#[0-9a-f]{3,6}\b/i)
    expect(alert.querySelector('div')!.outerHTML).not.toMatch(/text-\[\d+px\]/)
    const title = el.querySelector<HTMLElement>('[data-room-failure-title]')!
    expect(title.className).toContain('text-[var(--color-text-primary)]')
    const detail = el.querySelector<HTMLElement>('[data-room-failure-detail]')!
    expect(detail.textContent).toBe('Next try in 5 s.')
    expect(detail.className).toContain('text-[var(--color-text-muted)]')
    act(() => root.unmount())
    el.remove()
  })

  it('the action is the shared quiet Button (pill in Glass, cut corners in Terminal), not accent text', () => {
    const { el, root } = mount(createElement(RoomFailureView, { failure: offline(), actions: spyActions() }))
    const b = button(el)
    expect(b.getAttribute('data-k2-chamfer')).toBe('control')
    expect(b.className).toContain('rounded-[var(--radius-field)]')
    expect(b.className).toContain('border-[var(--color-control-edge)]')
    expect(b.className).toContain('k2-focus')
    expect(b.className).not.toContain('text-[var(--color-accent)]')
    act(() => root.unmount())
    el.remove()
  })

  it('a long server name wraps the title (two lines at most) and truncates the button label', () => {
    const long = 'a-very-long-server-name-that-would-push-the-button-off-the-edge.k2.dev'
    const failure = roomFailure({ serverLabel: long, saved: true, entry: entry({ auth: 'signin-required' }) })!
    const { el, root } = mount(createElement(RoomFailureView, { failure, actions: spyActions() }))
    const title = el.querySelector<HTMLElement>('[data-room-failure-title]')!
    expect(title.className).toContain('line-clamp-2')
    expect(title.className).toContain('[overflow-wrap:anywhere]')
    expect(title.getAttribute('title')).toBe(`Sign in to ${long}.`)
    const b = button(el)
    expect(b.className).toContain('max-w-[45%]')
    // Narrow (under 32rem): the button drops under the text, full width.
    expect(el.querySelector('[role="alert"]')!.className).toContain('@container')
    expect(el.querySelector('[data-room-failure-title]')!.parentElement!.className).toContain('@max-[32rem]:basis-full')
    expect(b.className).toContain('@max-[32rem]:max-w-full')
    expect(b.className).toContain('flex-shrink-0')
    expect(b.getAttribute('title')).toBe(`Sign in to ${long}`)
    expect(b.querySelector('span')?.className).toContain('truncate')
    act(() => root.unmount())
    el.remove()
  })

  it('every kind with an action renders its one quiet button; removed renders none', () => {
    const kinds: Array<Partial<HostEntry> | undefined> = [
      undefined,
      { reach: 'offline', boot: null },
      { reach: 'starting' },
      { auth: 'signing-in' },
      { auth: 'signin-required' },
      { auth: 'rotate-required' },
      { auth: 'kicked' },
      { boot: null },
      { role: 'viewer' },
    ]
    for (const patch of kinds) {
      const failure = roomFailure({ serverLabel: 'dharma', saved: true, entry: patch ? entry(patch) : undefined })!
      const { el, root } = mount(createElement(RoomFailureView, { failure, actions: spyActions() }))
      expect(el.querySelectorAll('[data-room-failure-action]'), failure.kind).toHaveLength(1)
      expect(button(el).getAttribute('data-k2-chamfer'), failure.kind).toBe('control')
      act(() => root.unmount())
      el.remove()
    }
    const removed = roomFailure({ serverLabel: 'dharma', saved: false, entry: undefined })!
    const { el, root } = mount(createElement(RoomFailureView, { failure: removed, actions: spyActions() }))
    expect(el.querySelector('[data-room-failure-action]')).toBeNull()
    act(() => root.unmount())
    el.remove()
  })
})

describe('RoomFailureGate (the pool drives it)', () => {
  it('floats the banner off the room by the pane gap (0 in Terminal, a tile gap in Glass)', () => {
    setEntry(entry({ reach: 'offline', boot: null }))
    const { el, root } = mount(createElement(RoomFailureGate, { hostKey: KEY, actions: spyActions() }, createElement('p', null, 'x')))
    expect(el.querySelector('[data-room-failing]')?.className).toContain('gap-[var(--gap-pane)]')
    act(() => root.unmount())
  })

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
    expect(signIn.mock.calls[0][0].id).toBe('id-dharma')
    expect(pickHost).not.toHaveBeenCalled()
    expect(h.invokes.filter((c) => c === 'k2_secret_delete' || c === 'connect_cli_token_delete')).toEqual([])
  })

  it('Sign in on a server that needs a new password opens the rotate step without switching', () => {
    const rotate = vi.fn()
    useConnectHostStore.setState({ requestPasswordRotation: rotate })
    setEntry(entry({ auth: 'rotate-required' }))
    roomFailureActions(KEY).signIn()
    expect(rotate).toHaveBeenCalledTimes(1)
    expect(rotate.mock.calls[0][0].id).toBe('id-dharma')
    expect(rotate.mock.calls[0][1]).toEqual({ activate: false })
  })

  it('Open B’s server switches this window to B — only on that click', () => {
    const pickHost = vi.fn()
    useConnectHostStore.setState({ pickHost })
    roomFailureActions(KEY).openServer()
    expect(pickHost).toHaveBeenCalledTimes(1)
    expect(pickHost.mock.calls[0][0].id).toBe('id-dharma')
  })
})
