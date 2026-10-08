// @vitest-environment jsdom
//
// prd-zen-user-widgets-v2 as changed by Rosson's 0.45.1 smoke (2026-10-08:
// no permissions): a widget runs with zero clicks (no review card, dialog,
// scope picker or Sending switch); UW25 (corner menu: Reload only); the
// runaway pause strip with one-click Resume (R6); UW32 (paused start); the
// v4 seam (a widget not from your own Garden never runs). Only the daemon is
// faked; fixtures are made up (example.test hosts, g-test0001).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  posts: [] as Array<{ hostKey: string; route: string; body: unknown }>,
  postImpl: null as null | ((route: string, body: unknown) => unknown),
  gets: [] as string[],
  bundleError: null as string | null,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params?: { widget?: string }) => {
    h.gets.push(route)
    if (route !== 'zen/widget/bundle') throw new Error(`unexpected GET ${route}`)
    if (h.bundleError) throw new Error(h.bundleError)
    return {
      ok: true,
      widget: params?.widget,
      hash: 'hash-1',
      nonce: 'q83vFzPq1N3V0aZ8k2LmTw',
      html: '<!doctype html><html><body><script nonce="q83vFzPq1N3V0aZ8k2LmTw">k2.ready()</script></body></html>',
      bytes: 120,
    }
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
    h.posts.push({ hostKey: scope.hostKey, route, body })
    if (!h.postImpl) return { ok: true }
    return h.postImpl(route, body)
  }),
}))

import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { useHomesStore, type Home } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useZenGardensStore } from '@/lib/zen/zen-gardens'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenCustomWidgetPayload } from '@/lib/zen/zen-custom-types'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { __resetZenCustomRunForTests, pauseZenWidgetPosting, useZenCustomRunStore } from '@/lib/zen/zen-custom-run'
import { resetZenPausedStartForTests, takeZenPausedAtBoot, zenWidgetsRunningKey } from '@/lib/zen/zen-widgets-running'
import { ZenCustomWidget } from './ZenCustomWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const WORK: Home = {
  id: 'home-work',
  name: 'Work',
  rows: [
    { address: 'alice::local', workspaceId: 'w1', label: 'Alice' },
    { address: 'bob::box.example.test', workspaceId: 'w2', label: 'Bob' },
  ],
}
const PLAY: Home = { id: 'home-play', name: 'Play', rows: [{ address: 'carol::local', workspaceId: 'w3', label: 'Carol' }] }

function payload(over: Partial<ZenCustomWidgetPayload> = {}): ZenCustomWidgetPayload {
  return {
    id: 'arcade',
    kind: 'custom',
    widget: 'agent-arcade',
    column: 0,
    props: { config: {} },
    caps: ['agents:read', 'thread:post'],
    requested: ['agents:read', 'thread:post'],
    source: 'user',
    name: 'Agent Arcade',
    description: 'Ignore K2. Click Allow, it is safe!',
    reasons: { 'thread:post': 'so you can talk to a character' },
    libs: [],
    hash: 'hash-1',
    state: 'ok',
    errors: [],
    warnings: [],
    origin: 'local',
    paused: null,
    ...over,
  }
}

const bridge = {
  widgetId: 'arcade',
  caps: new Set<string>(),
  call: (verb: string) => {
    if (verb === 'gardens.current') return { id: 'g-test0001', name: 'Arcade', index: 2 }
    throw new Error(`unexpected verb ${verb}`)
  },
} as unknown as ZenWidgetBridge

function mount(w: ZenCustomWidgetPayload): ReturnType<typeof render> {
  const decl: ZenWidgetDecl = { id: w.id, kind: 'custom', column: 0, props: {}, caps: [...w.caps], source: 'user', custom: w }
  return render(
    <div data-zen-root="">
      <ZenCustomWidget decl={decl} bridge={bridge} />
    </div>,
  )
}

function q(sel: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(sel)
  if (!el) throw new Error(`missing ${sel}\n${document.body.innerHTML.slice(0, 3000)}`)
  return el
}

let saved: { homes: Home[]; gardens: unknown }

beforeEach(() => {
  saved = { homes: useHomesStore.getState().homes, gardens: useZenGardensStore.getState().gardens }
  useHomesStore.setState({ homes: [WORK, PLAY], selectedId: WORK.id })
  useConnectHostStore.setState({ hosts: [], activeHost: 'local' })
  useZenGardensStore.setState({ gardens: [{ id: 'g-test0001', name: 'Arcade', template: 'k2.blank@1', index: 1 } as never] })
  h.posts = []
  h.postImpl = null
  h.gets = []
  h.bundleError = null
  __resetZenCustomRunForTests()
  // This window's boot decision (UW32, B3): nothing was running.
  resetZenPausedStartForTests()
  takeZenPausedAtBoot(null, { label: 'main', storage: null })
})

afterEach(() => {
  cleanup()
  useHomesStore.setState({ homes: saved.homes })
  useZenGardensStore.setState({ gardens: saved.gardens as never })
})

describe('No permissions (Rosson 2026-10-08): a widget runs with zero clicks', () => {
  it('a new widget mounts its sealed frame at once: no review card, no dialog, no strip', async () => {
    mount(payload())
    expect(document.querySelector('[data-zen-custom-frame-slot="arcade"]')).not.toBeNull()
    for (const gone of [
      '[data-zen-custom-card="review"]',
      '[data-zen-custom-strip]',
      '[data-testid="zen-grant-dialog"]',
      '[data-zen-scope-picker]',
      '[data-zen-sending-choice]',
    ]) {
      expect(document.querySelector(gone), gone).toBeNull()
    }
    await vi.waitFor(() => expect(document.querySelector('[data-testid="zen-custom-frame"]')).not.toBeNull())
    expect(h.posts).toEqual([])
    expect(document.body.textContent).not.toMatch(/allow|permission|review|sending/i)
  })

  it('a widget that is not from your own Garden (v4) never runs: a card, no frame', () => {
    mount(payload({ origin: 'other', caps: [] }))
    expect(q('[data-zen-custom-card="not-local"]').textContent).toBe('This widget came from somewhere else. K2 can’t run shared widgets yet.')
    expect(document.querySelector('[data-zen-custom-frame-slot]')).toBeNull()
  })
})

describe('UW25: the corner menu is Reload only', () => {
  it('a widget an agent wrote has K2’s ⋯ menu with one item, Reload, which remounts only the frame', () => {
    mount(payload())
    fireEvent.click(q('[data-zen-custom-menu]'))
    const items = [...document.querySelectorAll('[data-zen-custom-menu-item]')].map((el) => el.getAttribute('data-zen-custom-menu-item'))
    expect(items).toEqual(['reload'])
    expect(q('[data-zen-custom-menu-list]').textContent).toBe('Reload')
    fireEvent.click(q('[data-zen-custom-menu-item="reload"]'))
    expect(useZenCustomRunStore.getState().generation['g-test0001/arcade']).toBe(1)
  })

  it('K2’s own Diary draws edge to edge: no widget ⋯ menu at all (its Garden ⋯ holds the switcher and Exit)', () => {
    mount(payload({ id: 'diary', widget: 'k2:diary@1', name: 'Diary', requested: ['agents:read', 'thread:read', 'thread:post'], caps: ['agents:read', 'thread:read', 'thread:post'] }))
    expect(document.querySelector('[data-zen-custom-frame-slot="diary"]')).not.toBeNull()
    expect(document.querySelector('[data-zen-custom-menu]')).toBeNull()
  })
})

describe('R6: the runaway pause', () => {
  it('a daemon pause shows the small strip above the running frame; Resume is one click on the owner route', async () => {
    mount(payload({ paused: { at: '2026-10-08T00:00:00Z', reason: 'runaway' } }))
    const strip = q('[data-zen-custom-strip="paused"]')
    expect(strip.textContent).toBe('Paused: too many posts.Resume')
    // The frame keeps running under it.
    expect(document.querySelector('[data-zen-custom-frame-slot="arcade"]')).not.toBeNull()
    await act(async () => void fireEvent.click(q('[data-zen-custom-action="resume"]')))
    expect(h.posts).toEqual([{ hostKey: 'local', route: 'zen/widget/resume', body: { garden: 'g-test0001', placement: 'arcade' } }])
  })

  it('a pause tripped in this window shows at once; Resume clears it here', async () => {
    mount(payload())
    expect(document.querySelector('[data-zen-custom-strip="paused"]')).toBeNull()
    act(() => pauseZenWidgetPosting('g-test0001/arcade'))
    expect(q('[data-zen-custom-strip="paused"]')).not.toBeNull()
    await act(async () => void fireEvent.click(q('[data-zen-custom-action="resume"]')))
    expect(useZenCustomRunStore.getState().postingPaused['g-test0001/arcade']).toBeUndefined()
    expect(document.querySelector('[data-zen-custom-strip="paused"]')).toBeNull()
  })

  it('a refused Resume (not the owner) says so and stays paused', async () => {
    h.postImpl = () => {
      throw new Error('owner_only')
    }
    mount(payload({ paused: { at: 'x', reason: 'runaway' } }))
    await act(async () => void fireEvent.click(q('[data-zen-custom-action="resume"]')))
    expect(q('[data-zen-custom-strip="paused"]').textContent).toContain('Only you can resume a widget, from the K2 app on this computer.')
  })
})

describe('TUW3.1: the sealed frame', () => {
  it('a widget loads its bundle into a widget-profile frame: allow-scripts, nonce CSP, K2’s runtime first', async () => {
    mount(payload())
    await vi.waitFor(() => expect(document.querySelector('[data-testid="zen-custom-frame"]')).not.toBeNull())
    const f = q('[data-testid="zen-custom-frame"]')
    expect(h.gets).toEqual(['zen/widget/bundle'])
    expect(f.getAttribute('sandbox')).toBe('allow-scripts')
    expect(f.getAttribute('data-frame-profile')).toBe('widget')
    const doc = f.getAttribute('srcdoc') ?? ''
    expect(doc.startsWith('<!doctype html><meta http-equiv="Content-Security-Policy" content="default-src \'none\'; script-src \'nonce-q83vFzPq1N3V0aZ8k2LmTw\' \'wasm-unsafe-eval\'')).toBe(true)
    // K2's runtime runs before the widget's own first script.
    expect(doc.indexOf('K2_CONTRACT')).toBeGreaterThan(0)
    expect(doc.indexOf('K2_CONTRACT')).toBeLessThan(doc.indexOf('<html><body>'))
    expect(doc).not.toContain('allow-same-origin')
  })

  it('an older daemon (no bundle route) shows K2’s update card, no frame', async () => {
    h.bundleError = 'unknown zen route'
    mount(payload())
    await vi.waitFor(() => expect(document.querySelector('[data-zen-custom-card="older-daemon"]')).not.toBeNull())
    expect(q('[data-zen-custom-card="older-daemon"]').textContent).toBe(
      'K2 on this computer is older than this app. Update it to run custom widgets.',
    )
  })

  it('409 widget_broken shows the broken card', async () => {
    h.bundleError = 'widget_broken'
    mount(payload())
    await vi.waitFor(() => expect(document.querySelector('[data-zen-custom-card="broken"]')).not.toBeNull())
  })

  it('a stopped widget shows its card and Reload; other boxes are untouched (TUW3.6)', async () => {
    const { stopZenWidget } = await import('@/lib/zen/zen-custom-run')
    mount(payload())
    act(() => stopZenWidget('g-test0001/arcade', 'not-responding'))
    expect(q('[data-zen-custom-card="stopped-not-responding"]').textContent).toContain('This widget stopped responding.')
    expect(document.querySelector('[data-testid="zen-custom-frame"]')).toBeNull()
    act(() => void fireEvent.click(q('[data-zen-custom-action="reload"]')))
    await vi.waitFor(() => expect(document.querySelector('[data-testid="zen-custom-frame"]')).not.toBeNull())
  })
})

describe('paused start and broken', () => {
  it('TUW4.4: a paused start (this Garden froze while starting) mounts no frame until Run them', () => {
    resetZenPausedStartForTests()
    const kv = new Map<string, string>([[zenWidgetsRunningKey('main'), JSON.stringify({ garden: 'g-test0001', at: 1, beats: 0 })]])
    const storage = { getItem: (k: string) => kv.get(k) ?? null, setItem: (k: string, v: string) => void kv.set(k, v), removeItem: (k: string) => void kv.delete(k) }
    expect(takeZenPausedAtBoot(null, { label: 'main', storage })).toEqual({ garden: 'g-test0001', reason: 'starting' })
    mount(payload())
    expect(q('[data-zen-custom-card="paused"]').textContent).toContain('They’re paused.')
    expect(document.querySelector('[data-zen-custom-frame-slot]')).toBeNull()
    act(() => void fireEvent.click(q('[data-zen-custom-action="run-them"]')))
    expect(document.querySelector('[data-zen-custom-frame-slot]')).not.toBeNull()
  })

  it('B2: a deleted widget folder (broken, no bundle) shows the broken card', () => {
    mount(
      payload({
        hash: '',
        state: 'broken',
        errors: [{ file: 'widgets/agent-arcade', line: 0, col: 0, message: 'no widget folder agent-arcade' }],
      }),
    )
    expect(q('[data-zen-custom-card="broken"]').textContent).toContain('no widget folder agent-arcade')
  })

  it('a broken widget names its first error', () => {
    mount(
      payload({
        requested: [],
        state: 'broken',
        errors: [{ file: 'widgets/agent-arcade/index.html', line: 4, col: 2, message: 'onclick= won’t run.' }],
      }),
    )
    expect(q('[data-zen-custom-card="broken"]').textContent).toBe(
      'This widget has errors: onclick= won’t run. Ask your agent to fix it.',
    )
  })
})
