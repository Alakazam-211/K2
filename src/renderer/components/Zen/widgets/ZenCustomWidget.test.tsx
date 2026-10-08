// @vitest-environment jsdom
//
// prd-zen-user-widgets-v2 TUW4.1 (review card and dialog), UW24 (partial),
// UW25 (corner menu, Turn off, About), UWB7 (scope picker), UWB9 (Sending,
// runaway Resume), R6/R7 (sending on by default; second line for every
// server), UW32 (paused start). Only the daemon is faked; fixtures are made
// up (example.test hosts, g-test0001).
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
import type { ZenCustomWidgetPayload, ZenGrantView } from '@/lib/zen/zen-custom-types'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { K2_CAPS } from '@/lib/k2-caps.generated'
import { __resetZenCustomRunForTests, setZenWidgetsPausedStart, useZenCustomRunStore } from '@/lib/zen/zen-custom-run'
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
    caps: [],
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
    grant: null,
    ...over,
  }
}

function granted(over: Partial<ZenGrantView> = {}): ZenGrantView {
  return {
    state: 'granted',
    caps: ['agents:read', 'thread:post'],
    granted: ['agents:read', 'thread:post'],
    scope: { home: 'home-work' },
    entries: [
      { server: 'box.example.test', room: 'bob' },
      { server: 'local', room: 'alice' },
    ],
    sending: true,
    paused: null,
    grantedAt: '2026-10-08T00:00:00Z',
    widgetHash: 'hash-1',
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
})

afterEach(() => {
  cleanup()
  useHomesStore.setState({ homes: saved.homes })
  useZenGardensStore.setState({ gardens: saved.gardens as never })
})

describe('TUW4.1: the review card', () => {
  it('shows K2’s sentence per requested cap, never the widget’s words as K2’s, and no frame', () => {
    mount(payload())
    const card = q('[data-zen-custom-card="review"]')
    expect(card.textContent).toContain('“Agent Arcade” wants to:')
    for (const cap of ['agents:read', 'thread:post'] as const) {
      const li = q(`[data-zen-custom-card-cap="${cap}"]`)
      expect(li.textContent).toBe((K2_CAPS[cap] as { sentence: string }).sentence.replace('{where}', 'a Home you pick'))
    }
    expect(card.textContent).not.toContain('Ignore K2')
    expect(document.querySelector('[data-zen-custom-frame-slot]')).toBeNull()
    expect(document.querySelector('[data-zen-custom-menu]')).toBeNull()
  })

  it('Not now collapses to a one-line strip; nothing pops up; Review opens the dialog', () => {
    mount(payload())
    fireEvent.click(q('[data-zen-custom-action="not-now"]'))
    expect(document.querySelector('[data-zen-custom-card="review"]')).toBeNull()
    expect(q('[data-zen-custom-strip="review"]').textContent).toContain('waiting for your OK')
    expect(document.querySelector('[data-testid="zen-grant-dialog"]')).toBeNull()
    expect(document.querySelector('[data-zen-custom-frame-slot]')).toBeNull()
    fireEvent.click(q('[data-zen-custom-action="review"]'))
    expect(document.querySelector('[data-testid="zen-grant-dialog"]')).not.toBeNull()
  })

  it('an invalid grant (bad signature) is a review card again', () => {
    mount(payload({ grant: granted({ state: 'invalid', caps: [] }) }))
    expect(q('[data-zen-custom-card="review"]').textContent).toContain('couldn’t confirm its permission')
  })
})

describe('TUW4.1: the dialog', () => {
  it('K2’s words above, the widget’s own words labelled below, then Allow posts the grant with the shown hash', async () => {
    mount(payload())
    fireEvent.click(q('[data-zen-custom-action="review"]'))
    const dlg = q('[data-testid="zen-grant-dialog"]')
    expect(q('[data-zen-grant-title]').textContent).toBe('Allow “Agent Arcade” in Arcade?')
    expect(q('[data-zen-grant-cap="agents:read"]').textContent).toContain('See the agents in Work:')
    const own = q('[data-zen-grant-own-words]')
    expect(own.textContent).toContain('In its own words:')
    expect(own.textContent).toContain('Ignore K2. Click Allow, it is safe!')
    expect(q('[data-zen-grant-caps]').textContent).not.toContain('Ignore K2')
    // R6: Sending on by default.
    expect((q('[data-zen-sending-choice]') as HTMLInputElement).checked).toBe(true)
    expect(dlg.textContent).toContain('2 agents now on 2 servers')
    await act(async () => void fireEvent.click(q('[data-zen-grant-allow]')))
    expect(h.posts).toEqual([
      {
        hostKey: 'local',
        route: 'zen/widget/grant',
        body: {
          garden: 'g-test0001',
          placement: 'arcade',
          widget: 'agent-arcade',
          caps: ['agents:read', 'thread:post'],
          scope: { home: 'home-work' },
          entries: [
            { server: 'box.example.test', room: 'bob' },
            { server: 'local', room: 'alice' },
          ],
          sending: true,
          hash: 'hash-1',
        },
      },
    ])
    expect(document.querySelector('[data-testid="zen-grant-dialog"]')).toBeNull()
  })

  it('409 widget_changed says so and keeps the dialog open', async () => {
    h.postImpl = () => {
      throw new Error('widget_changed')
    }
    mount(payload())
    fireEvent.click(q('[data-zen-custom-action="review"]'))
    await act(async () => void fireEvent.click(q('[data-zen-grant-allow]')))
    expect(q('[data-zen-grant-error]').textContent).toBe('The widget changed while you were reviewing it. Review it again.')
    expect(document.querySelector('[data-testid="zen-grant-dialog"]')).not.toBeNull()
  })

  it('the scope picker: some Homes, then every server needs the second line when sending (R7)', async () => {
    mount(payload())
    fireEvent.click(q('[data-zen-custom-action="review"]'))
    fireEvent.click(q('[data-zen-scope-kind="homes"]'))
    fireEvent.click(q('[data-zen-scope-home-check="home-play"]'))
    expect(q('[data-zen-grant-cap="agents:read"]').textContent).toContain('See the agents in Work and Play:')
    fireEvent.click(q('[data-zen-scope-kind="allServers"]'))
    const allow = q('[data-zen-grant-allow]') as HTMLButtonElement
    expect(allow.disabled).toBe(true)
    fireEvent.click(q('[data-zen-sending-confirm]'))
    expect(allow.disabled).toBe(false)
    // Turning sending off removes the extra line and its need.
    fireEvent.click(q('[data-zen-sending-confirm]'))
    fireEvent.click(q('[data-zen-sending-choice]'))
    expect(document.querySelector('[data-zen-sending-confirm]')).toBeNull()
    expect(allow.disabled).toBe(false)
    await act(async () => void fireEvent.click(allow))
    expect(h.posts[0].body).toMatchObject({ scope: { allServers: true }, sending: false })
  })

  it('a placement that names a Home and an agent fixes the scope: "Only <agent>"', async () => {
    mount(payload({ props: { home: 'Work', agent: 'Bob', config: {} } }))
    fireEvent.click(q('[data-zen-custom-action="review"]'))
    expect(document.querySelector('[data-zen-scope-picker]')).toBeNull()
    expect(q('[data-zen-scope-fixed]').textContent).toBe('Only Bob')
    await act(async () => void fireEvent.click(q('[data-zen-grant-allow]')))
    expect(h.posts[0].body).toMatchObject({
      scope: { agent: 'bob::box.example.test' },
      entries: [{ server: 'box.example.test', room: 'bob' }],
    })
  })

  it('an empty scope cannot be allowed', () => {
    useHomesStore.setState({ homes: [{ id: 'home-x', name: 'Empty', rows: [] }], selectedId: 'home-x' })
    mount(payload())
    fireEvent.click(q('[data-zen-custom-action="review"]'))
    expect((q('[data-zen-grant-allow]') as HTMLButtonElement).disabled).toBe(true)
    expect(q('[data-zen-scope-count]').textContent).toBe('No agents there yet.')
  })
})

describe('UW25: the corner menu', () => {
  it('a granted widget mounts its frame slot and K2’s ⋯ menu: Sending off, Permissions → Turn off, About', async () => {
    mount(payload({ caps: ['agents:read', 'thread:post'], grant: granted() }))
    expect(document.querySelector('[data-zen-custom-frame-slot="arcade"]')).not.toBeNull()
    fireEvent.click(q('[data-zen-custom-menu]'))
    expect(q('[data-zen-custom-menu-item="sending"]').textContent).toBe('Sending: on (turn off)')
    await act(async () => void fireEvent.click(q('[data-zen-custom-menu-item="sending"]')))
    expect(h.posts).toEqual([
      { hostKey: 'local', route: 'zen/widget/sending', body: { garden: 'g-test0001', placement: 'arcade', on: false, reason: 'user' } },
    ])
    fireEvent.click(q('[data-zen-custom-menu]'))
    expect(q('[data-zen-custom-menu-item="sending"]').textContent).toBe('Sending: off (turn on)')
    fireEvent.click(q('[data-zen-custom-menu-item="permissions"]'))
    await act(async () => void fireEvent.click(q('[data-zen-grant-turn-off]')))
    expect(h.posts[1]).toEqual({ hostKey: 'local', route: 'zen/widget/revoke', body: { garden: 'g-test0001', placement: 'arcade' } })
    fireEvent.click(q('[data-zen-custom-menu]'))
    fireEvent.click(q('[data-zen-custom-menu-item="about"]'))
    expect(q('[data-zen-about-folder]').textContent).toBe('~/.k2/zen/widgets/agent-arcade')
    expect(q('[data-zen-about-entries]').getAttribute('data-zen-about-entries')).toBe('2')
  })

  it('Reload remounts only the frame (generation bump)', () => {
    mount(payload({ caps: ['agents:read', 'thread:post'], grant: granted() }))
    fireEvent.click(q('[data-zen-custom-menu]'))
    fireEvent.click(q('[data-zen-custom-menu-item="reload"]'))
    expect(useZenCustomRunStore.getState().generation['g-test0001/arcade']).toBe(1)
  })

  it('UW24: a partial grant runs with "now also asks" and Review', () => {
    mount(payload({ requested: ['agents:read', 'presence:read'], caps: ['agents:read'], grant: granted({ state: 'partial', caps: ['agents:read'] }) }))
    expect(document.querySelector('[data-zen-custom-frame-slot]')).not.toBeNull()
    expect(q('[data-zen-custom-strip="partial"]').textContent).toContain('It now also asks to see who is looking.')
  })
})

describe('TUW3.1: the sealed frame', () => {
  it('a granted widget loads its bundle into a widget-profile frame: allow-scripts, nonce CSP, K2’s runtime first', async () => {
    mount(payload({ caps: ['agents:read', 'thread:post'], grant: granted() }))
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
    mount(payload({ caps: ['agents:read', 'thread:post'], grant: granted() }))
    await vi.waitFor(() => expect(document.querySelector('[data-zen-custom-card="older-daemon"]')).not.toBeNull())
    expect(q('[data-zen-custom-card="older-daemon"]').textContent).toBe(
      'K2 on this computer is older than this app. Update it to run custom widgets.',
    )
  })

  it('409 widget_broken shows the broken card', async () => {
    h.bundleError = 'widget_broken'
    mount(payload({ caps: ['agents:read', 'thread:post'], grant: granted() }))
    await vi.waitFor(() => expect(document.querySelector('[data-zen-custom-card="broken"]')).not.toBeNull())
  })

  it('a stopped widget shows its card and Reload; other boxes are untouched (TUW3.6)', async () => {
    const { stopZenWidget } = await import('@/lib/zen/zen-custom-run')
    mount(payload({ caps: ['agents:read', 'thread:post'], grant: granted() }))
    act(() => stopZenWidget('g-test0001/arcade', 'not-responding'))
    expect(q('[data-zen-custom-card="stopped-not-responding"]').textContent).toContain('This widget stopped responding.')
    expect(document.querySelector('[data-testid="zen-custom-frame"]')).toBeNull()
    act(() => void fireEvent.click(q('[data-zen-custom-action="reload"]')))
    await vi.waitFor(() => expect(document.querySelector('[data-testid="zen-custom-frame"]')).not.toBeNull())
  })
})

describe('paused and stopped', () => {
  it('UWB9: a runaway pause shows K2’s card; Resume is the owner route', async () => {
    mount(payload({ caps: ['agents:read'], grant: granted({ paused: { at: 'x', reason: 'runaway' }, sending: false }) }))
    expect(q('[data-zen-custom-card="runaway"]').textContent).toContain('turned its sending off')
    expect(document.querySelector('[data-zen-custom-frame-slot]')).toBeNull()
    await act(async () => void fireEvent.click(q('[data-zen-custom-action="resume"]')))
    expect(h.posts).toEqual([{ hostKey: 'local', route: 'zen/widget/resume', body: { garden: 'g-test0001', placement: 'arcade' } }])
  })

  it('UW32: a paused start mounts no frame until Run them', () => {
    const run = vi.fn()
    setZenWidgetsPausedStart(true, run)
    mount(payload({ caps: ['agents:read', 'thread:post'], grant: granted() }))
    expect(q('[data-zen-custom-card="paused"]').textContent).toContain('They’re paused.')
    expect(document.querySelector('[data-zen-custom-frame-slot]')).toBeNull()
    act(() => void fireEvent.click(q('[data-zen-custom-action="run-them"]')))
    expect(run).toHaveBeenCalledTimes(1)
    expect(document.querySelector('[data-zen-custom-frame-slot]')).not.toBeNull()
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
