// @vitest-environment jsdom
// Garden v3 (contract-sources-v1 §11.4, §12.3 "Cards") — the five built-in
// data-source cards, run byte for byte (kit.js + card.js from k2-core)
// against a fake `k2` that hands over fixture card views. Every card draws
// every PRD §12 state: updated N ago from the server's clock, never blank,
// unknown in its own colour, retrying n/of, recovered, muted and TEST
// badges, the sleep / restart gap, unreachable (last value greyed), not
// saved, removed, waiting for the first check, a down box as a red tile,
// and (@1 and @2) Ask an agent on red, stale and unknown; @3 (0.46.4) draws
// every state with no Ask an agent.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { zenSourceView, zenStaleView, type ZenSourceView } from './zen-sources'
import { fixtureItem, fixtureView } from './zen-sources-fixtures'
import type { SourceResult } from '@/lib/sources/sources-types'

const ROOT = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets')
const KIT = readFileSync(resolve(ROOT, 'sources-kit/kit.js'), 'utf8')
const KIT2 = readFileSync(resolve(ROOT, 'sources-kit-2/kit.js'), 'utf8')
const KIT3 = readFileSync(resolve(ROOT, 'sources-kit-3/kit.js'), 'utf8')
const CARDS = [
  'fleet-grid-1',
  'version-spread-1',
  'relays-dns-1',
  'deploys-1',
  'money-alerts-1',
  'fleet-grid-2',
  'version-spread-2',
  'relays-dns-2',
  'deploys-2',
  'money-alerts-2',
  'fleet-grid-3',
  'version-spread-3',
  'relays-dns-3',
  'deploys-3',
  'money-alerts-3',
] as const
type Card = (typeof CARDS)[number]
const kitFor = (card: Card): string => (card.endsWith('-3') ? KIT3 : card.endsWith('-2') ? KIT2 : KIT)
/** @3 keeps @2's page (`index.html`): only card.js and the kit changed. */
const pageDir = (card: Card): string => (card.endsWith('-3') ? card.replace(/-3$/, '-2') : card)

interface Sub {
  address: string
  cb: (v: unknown) => void
  onError: (e: unknown) => void
}

interface Mounted {
  card: HTMLElement
  subs: Sub[]
  asks: Array<[string, string | null]>
  ready: number
  push(v: unknown, i?: number): void
  fail(e: unknown, i?: number): void
}

async function flush(): Promise<void> {
  for (let i = 0; i < 8; i++) await Promise.resolve()
}

async function mount(card: Card, config: Record<string, string | number | boolean>): Promise<Mounted> {
  const html = readFileSync(resolve(ROOT, pageDir(card), 'index.html'), 'utf8')
  document.body.innerHTML = /<body>([\s\S]*)<\/body>/.exec(html)?.[1].replace(/<script[\s\S]*?<\/script>/g, '') ?? ''
  const subs: Sub[] = []
  const asks: Array<[string, string | null]> = []
  const m = { ready: 0 }
  let hello: (v: unknown) => void = () => undefined
  const k2 = {
    config: {} as Record<string, unknown>,
    connected: new Promise((r) => (hello = r)),
    ready: () => void (m.ready += 1),
    sources: {
      subscribe: (address: string, cb: (v: unknown) => void, onError: (e: unknown) => void) => {
        subs.push({ address, cb, onError })
        return () => undefined
      },
      ask: async (address: string, key: string | null) => {
        asks.push([address, key])
        return { opened: true }
      },
      history: async () => ({ field: 'count', res: 'raw', points: [{ t: 1, v: 3 }, { t: 2, v: null }, { t: 3, v: 4 }, { t: 4, v: 5 }] }),
    },
  }
  vi.stubGlobal('k2', k2)
  new Function(kitFor(card))()
  new Function(readFileSync(resolve(ROOT, card, 'card.js'), 'utf8'))()
  k2.config = config
  hello({})
  await flush()
  const el = document.getElementById('card')
  if (!el) throw new Error('no #card')
  return {
    card: el,
    subs,
    asks,
    get ready() {
      return m.ready
    },
    push: (v, i = 0) => subs[i].cb(structuredClone(v)),
    fail: (e, i = 0) => subs[i].onError(e),
  }
}

const NOW = 1_760_423_520
const view = (over: Partial<SourceResult> = {}, address = 'fleet'): ZenSourceView => zenSourceView(address, fixtureView(over))

/** The fixture each card is about, healthy. */
function healthy(card: Card): ZenSourceView {
  switch (card) {
    case 'relays-dns-1':
    case 'relays-dns-2':
    case 'relays-dns-3':
      return view({
        items: [
          fixtureItem('ohana', { fields: { version: '0.45.6', phase: 'ready', relay: 'k2e-01', up: true } }),
          fixtureItem('luzz', { fields: { version: '0.45.6', phase: 'ready', relay: 'k2e-02', up: true } }),
        ],
      })
    case 'deploys-1':
    case 'deploys-2':
    case 'deploys-3':
      return view({
        items: [
          fixtureItem('dpl_a', { fields: { state: 'READY', url: 'app-a.vercel.app', created: (NOW - 120) * 1000, target: 'production' } }),
          fixtureItem('dpl_b', { fields: { state: 'READY', url: 'app-b.vercel.app', created: (NOW - 7200) * 1000, target: 'production' } }),
        ],
      })
    case 'money-alerts-1':
    case 'money-alerts-2':
    case 'money-alerts-3':
      return view({ keep: 'states', fields: {}, items: [], counts: { ok: 0, warn: 0, crit: 0, unknown: 0 } })
    default:
      return view()
  }
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
  document.body.innerHTML = ''
})

describe.each(CARDS)('%s: every PRD §12 state', (card) => {
  it('updated N ago from serverNow, never the viewer clock; ready once', async () => {
    vi.useFakeTimers({ toFake: ['Date', 'setInterval'] })
    vi.setSystemTime(new Date('2001-01-01T00:00:00Z'))
    const m = await mount(card, { source: 'fleet' })
    expect(m.card.textContent).toContain('Loading')
    m.push(healthy(card))
    expect(m.card.textContent).toContain('updated 20 s ago')
    expect(m.ready).toBe(1)
    vi.advanceTimersByTime(60_000)
    expect(m.card.textContent).toContain('updated 1 min ago')
  })

  it('waiting, stale (greyed with the reason), unknown colour, retrying, recovered', async () => {
    const m = await mount(card, { source: 'fleet' })
    m.push({ ...healthy(card), phase: 'pending', display: 'pending', updatedAt: null, health: 'unknown' })
    expect(m.card.textContent).toContain('Waiting for the first check')
    m.push({ ...healthy(card), phase: 'stale', display: 'stale', reasonText: 'Timed out.' })
    expect(m.card.textContent).toContain('Stale: Timed out.')
    expect(m.card.querySelector('.k2c-greyed')).not.toBeNull()
    expect(m.card.querySelector('.k2c-chip')?.getAttribute('data-k2c-state')).toBe('stale')
    m.push({ ...healthy(card), health: 'unknown', display: 'unknown' })
    expect(m.card.querySelector('.k2c-chip')?.className).toContain('k2c-unknown')
    m.push({ ...healthy(card), soft: { count: 2, of: 3 } })
    expect(m.card.textContent).toContain('retrying 2/3')
    m.push({ ...healthy(card), recoveredAt: NOW - 300 })
    expect(m.card.textContent).toContain('recovered 5 min ago')
    m.push({ ...healthy(card), recoveredAt: NOW - 7200 })
    expect(m.card.textContent).not.toContain('recovered')
  })

  it('muted and TEST badges; the link’s host', async () => {
    const m = await mount(card, { source: 'fleet' })
    m.push({ ...healthy(card), muted: { until: NOW + 600, by: 'rosson', reason: 'roll' }, simulated: { state: 'crit', until: NOW + 3600, by: 'k2-dev-web' } })
    expect(m.card.textContent).toContain('Muted by rosson until')
    expect(m.card.querySelector('.k2c-test')?.textContent).toContain('TEST: crit')
    expect(m.card.textContent).toContain('↗ box.example.test')
  })

  it('the sleep gap and a restart, until a fresh check', async () => {
    const m = await mount(card, { source: 'fleet' })
    m.push({ ...healthy(card), phase: 'paused', display: 'paused', updatedAt: NOW - 9000, gap: { kind: 'host_slept', from: NOW - 8000, to: NOW - 10 } })
    expect(m.card.textContent).toMatch(/Paused while this computer slept \(\d\d:\d\d–\d\d:\d\d\)/)
    m.push({ ...healthy(card), gap: { kind: 'daemon_restarted', from: NOW - 100, to: NOW - 30 }, updatedAt: NOW - 200 })
    expect(m.card.textContent).toContain('K2 restarted')
    m.push({ ...healthy(card), gap: { kind: 'daemon_restarted', from: NOW - 100, to: NOW - 30 }, updatedAt: NOW - 5 })
    expect(m.card.textContent).not.toContain('K2 restarted')
  })

  it('another server: unreachable shows the last value greyed; not saved; removed; can’t read', async () => {
    const m = await mount(card, { source: 'fleet::akzm.k2.dev' })
    const last = zenStaleView(healthy(card), 'unreachable')
    m.fail({ code: 'unreachable', message: 'x', last })
    expect(m.card.textContent).toContain('akzm.k2.dev unreachable')
    expect(m.card.querySelector('.k2c-greyed')).not.toBeNull()
    m.fail({ code: 'unreachable', message: 'x' })
    expect(m.card.textContent).toContain('akzm.k2.dev unreachable')
    m.fail({ code: 'server_not_saved', message: 'x' })
    expect(m.card.textContent).toContain('Add akzm.k2.dev to your servers')
    m.fail({ code: 'removed', message: 'x' })
    expect(m.card.textContent).toContain('Removed on akzm.k2.dev')
    m.fail({ code: 'cap_not_granted', message: 'x' })
    expect(m.card.textContent).toContain('can’t read this source')
    // Never blank.
    expect(m.card.textContent?.trim().length).toBeGreaterThan(10)
  })

  it('any other refusal says why (never blank)', async () => {
    const m = await mount(card, { source: 'fleet' })
    m.fail({ code: 'failed', message: 'No source named fleet.' })
    expect(m.card.textContent).toContain('No source named fleet.')
  })

  it('a card with no source says how to add one', async () => {
    const m = await mount(card, {})
    expect(m.card.textContent).toContain('config.source')
    expect(m.subs).toHaveLength(0)
  })
})

describe('@3 (0.46.4): no Ask an agent', () => {
  const red = (card: Card): ZenSourceView => {
    const h = healthy(card)
    if (card.startsWith('money-alerts')) return { ...h, health: 'crit', display: 'crit' }
    if (card.startsWith('deploys')) {
      return view({
        health: 'crit',
        display: 'crit',
        items: [fixtureItem('dpl_x', { health: 'crit', fields: { state: 'ERROR', url: 'x.vercel.app', created: (NOW - 60) * 1000, target: 'production' } })],
        counts: { ok: 0, warn: 0, crit: 1, unknown: 0 },
      })
    }
    return view({
      health: 'crit',
      display: 'crit',
      items: [
        fixtureItem('ohana', { health: 'crit', reason: 'timeout', fields: { version: null, phase: null, relay: 'k2e-01', up: false } }),
        fixtureItem('luzz', { health: 'unknown', fields: { relay: 'k2e-02' } }),
      ],
      counts: { ok: 0, warn: 0, crit: 1, unknown: 1 },
    })
  }

  it.each(['fleet-grid', 'version-spread', 'relays-dns', 'deploys', 'money-alerts'] as const)(
    '%s: @2 offers Ask on red; @3 draws the same red with no Ask button',
    async (name) => {
      const two = await mount(`${name}-2` as Card, { source: 'fleet' })
      two.push(red(`${name}-2` as Card))
      const twoAsk = two.card.querySelectorAll('[data-k2c-ask]').length
      const twoText = two.card.textContent ?? ''
      vi.unstubAllGlobals()
      const three = await mount(`${name}-3` as Card, { source: 'fleet' })
      three.push(red(`${name}-3` as Card))
      expect(three.card.querySelectorAll('[data-k2c-ask]').length, `${name}@3`).toBe(0)
      expect(three.card.textContent ?? '').not.toContain('Ask')
      expect(three.asks).toEqual([])
      // The card still says what's wrong: the same words as @2, without Ask.
      expect((three.card.textContent ?? '').length, `${name}@3 isn't blank`).toBeGreaterThan(10)
      if (twoAsk > 0) expect(twoText.replace(/Ask an agent|Ask/g, '')).toBe(three.card.textContent)
    },
  )
})

describe('card specifics', () => {
  it('fleet grid: a down box is a red tile with its reason and Ask; unknown its own colour', async () => {
    const m = await mount('fleet-grid-1', { source: 'fleet' })
    m.push(
      view({
        health: 'crit',
        display: 'crit',
        items: [
          fixtureItem('alpha'),
          fixtureItem('ohana', { health: 'crit', reason: 'timeout', fields: { version: null, phase: null } }),
          fixtureItem('luzz', { health: 'unknown' }),
          fixtureItem('beta', { health: 'warn', fields: { version: '0.45.6', phase: 'draining' } }),
        ],
        counts: { ok: 1, warn: 1, crit: 1, unknown: 1 },
      }),
    )
    const tile = (k: string) => m.card.querySelector(`[data-k2c-item="${k}"]`)
    expect(tile('ohana')?.getAttribute('data-k2c-state')).toBe('crit')
    expect(tile('ohana')?.textContent).toContain('timeout')
    expect(tile('luzz')?.className).toContain('k2c-unknown')
    expect(tile('alpha')?.querySelector('[data-k2c-ask]')).toBeNull()
    expect(tile('beta')?.querySelector('[data-k2c-ask]')).toBeNull()
    expect(m.card.querySelector('[data-k2c-banner]')?.textContent).toBe('1 down · 1 warning · 1 unknown')
    ;(tile('ohana')?.querySelector('[data-k2c-ask]') as HTMLButtonElement).click()
    ;(tile('luzz')?.querySelector('[data-k2c-ask]') as HTMLButtonElement).click()
    expect(m.asks).toEqual([
      ['fleet', 'ohana'],
      ['fleet', 'luzz'],
    ])
  })

  it('fleet grid: labelField and detailFields from config; text stays text', async () => {
    const m = await mount('fleet-grid-1', { source: 'fleet', labelField: 'name', detailFields: 'phase' })
    m.push(view({ items: [fixtureItem('a', { fields: { name: '<b>Ohana</b>', phase: 'ready', version: '9' } })] }))
    const t = m.card.querySelector('[data-k2c-item="a"]')
    expect(t?.querySelector('.k2c-tile-name')?.textContent).toBe('<b>Ohana</b>')
    expect(t?.querySelector('b')).toBeNull()
    expect(t?.textContent).not.toContain('9')
  })

  it('version spread: bars per version, behind boxes amber, newest from config.latest, sparkline from history', async () => {
    const m = await mount('version-spread-1', { source: 'fleet', latest: 'release::akzm.k2.dev' })
    expect(m.subs.map((s) => s.address)).toEqual(['fleet', 'release::akzm.k2.dev'])
    m.push(
      view({
        items: [
          fixtureItem('a', { fields: { version: '0.45.6', phase: 'ready' } }),
          fixtureItem('b', { fields: { version: '0.45.5', phase: 'ready' } }),
          fixtureItem('c', { fields: { version: '0.45.6', phase: 'ready' } }),
          fixtureItem('d', { health: 'crit', reason: 'dns', fields: { version: null, phase: null } }),
        ],
      }),
    )
    m.push(view({ name: 'release', fields: { newest: 'v0.45.7' }, items: [] }, 'release::akzm.k2.dev'), 1)
    await flush()
    expect([...m.card.querySelectorAll('[data-k2c-version]')].map((b) => [b.getAttribute('data-k2c-version'), b.className.includes('k2c-warn')])).toEqual([
      ['0.45.6', true],
      ['0.45.5', true],
    ])
    expect(m.card.textContent).toContain('Newest release: v0.45.7')
    expect(m.card.textContent).toContain('3 behind · 1 unknown')
    expect(m.card.querySelector('[data-k2c-item="d"]')?.getAttribute('data-k2c-state')).toBe('crit')
    expect(m.card.querySelector('.k2c-spark path')?.getAttribute('d')).toMatch(/^M0\.0 .* M/)
  })

  it('relays & DNS: by relay from fleet-summary, by group from Gatus, else what it needs', async () => {
    const m = await mount('relays-dns-1', { source: 'fleet' })
    m.push(
      view({
        items: [
          fixtureItem('ohana', { fields: { relay: 'k2e-01', up: true } }),
          fixtureItem('luzz', { health: 'crit', fields: { relay: 'k2e-01', up: false } }),
          fixtureItem('acv', { fields: { relay: 'k2e-02', up: true } }),
        ],
      }),
    )
    expect(m.card.querySelector('[data-k2c-item="k2e-01"]')?.getAttribute('data-k2c-state')).toBe('crit')
    expect(m.card.querySelector('[data-k2c-item="k2e-01"]')?.textContent).toContain('1 down')
    expect(m.card.querySelector('[data-k2c-item="k2e-02"]')?.getAttribute('data-k2c-state')).toBe('ok')
    m.push(view({ items: [fixtureItem('api_dns', { health: 'crit', fields: { name: 'DNS', group: 'edge', up: false } })] }))
    expect(m.card.querySelector('[data-k2c-item="api_dns"] .k2c-tile-name')?.textContent).toBe('DNS')
    m.push(view())
    expect(m.card.querySelector('[data-k2c-needs]')?.textContent).toContain('Needs K2 Web’s fleet-summary or a Gatus endpoint')
  })

  it('deploys: newest first, at most config.rows, state and age from the server clock', async () => {
    const m = await mount('deploys-1', { source: 'deploys', rows: 1 })
    m.push(
      view({
        items: [
          fixtureItem('old', { health: 'ok', fields: { state: 'READY', url: 'https://old.vercel.app', created: (NOW - 9000) * 1000 } }),
          fixtureItem('new', { health: 'crit', fields: { state: 'ERROR', url: 'new.vercel.app', created: (NOW - 60) * 1000 } }),
        ],
        counts: { ok: 1, warn: 0, crit: 1, unknown: 0 },
      }),
    )
    const rows = [...m.card.querySelectorAll('[data-k2c-item]')]
    expect(rows.map((r) => r.getAttribute('data-k2c-item'))).toEqual(['new'])
    expect(rows[0].textContent).toContain('new.vercel.app')
    expect(rows[0].textContent).toContain('error · 1 min ago')
    expect(m.card.querySelector('[data-k2c-banner]')?.textContent).toBe('1 failed')
    ;(rows[0].querySelector('[data-k2c-ask]') as HTMLButtonElement).click()
    expect(m.asks).toEqual([['deploys', 'new']])
  })

  it('money alerts: states only says found / none and never a number; values show the count or 100+', async () => {
    const m = await mount('money-alerts-1', { source: 'money' })
    m.push(view({ keep: 'states', health: 'crit', display: 'crit', fields: {}, items: [] }))
    expect(m.card.querySelector('[data-k2c-banner]')?.textContent).toBe('Past-due subscriptions found')
    expect(m.card.textContent).not.toMatch(/\d+ past-due/)
    ;(m.card.querySelector('[data-k2c-ask]') as HTMLButtonElement).click()
    expect(m.asks).toEqual([['money', null]])
    m.push(view({ keep: 'states', health: 'ok', display: 'ok', fields: {}, items: [] }))
    expect(m.card.querySelector('[data-k2c-banner]')?.textContent).toBe('No past-due subscriptions')
    expect(m.card.querySelector('[data-k2c-ask]')).toBeNull()
    m.push(view({ keep: 'values', health: 'crit', display: 'crit', fields: { pastDue: 7, hasMore: false }, items: [] }))
    expect(m.card.querySelector('.k2c-number-value')?.textContent).toBe('7')
    m.push(view({ keep: 'values', health: 'crit', display: 'crit', fields: { pastDue: 100, hasMore: true }, items: [] }))
    expect(m.card.querySelector('.k2c-number-value')?.textContent).toBe('100+')
  })
})

// ── @2 (0.46.1): several sources, soft level, lists, K2 Web's ops shapes,
// and a refused subscription that subscribes again by itself.

/** A K2 Web ops boxes view (`/api/ops/fleet?part=boxes`), keyed by label. */
function opsBoxes(over: Partial<SourceResult> = {}): ZenSourceView {
  return view(
    {
      name: 'k2-ops-boxes',
      label: 'Ops: boxes',
      fields: { target_version: '0.46.0', behind: 2, far_behind: 1, boxes: 3, ts: NOW - 30 },
      items: [
        fixtureItem('ohana', { fields: { version: '0.44.4', online: true, phase: 'ready', customer: true, needs_go: 'rosson' } }),
        fixtureItem('akzm', { fields: { version: '0.46.0', online: true, phase: 'ready', customer: false, needs_go: null } }),
        fixtureItem('luzz', { health: 'warn', fields: { version: '0.45.9', online: true, phase: 'ready', customer: false, needs_go: null } }),
      ],
      counts: { ok: 2, warn: 1, crit: 0, unknown: 0 },
      ...over,
    },
    'k2-ops-boxes',
  )
}

describe('@2 cards', () => {
  it('fleet grid @2 merges sources by box: fields filled in order, the worst health, whose go', async () => {
    const m = await mount('fleet-grid-2', { source: 'fleet', sources: 'k2-ops-boxes, fleet' })
    expect(m.subs.map((s) => s.address)).toEqual(['fleet', 'k2-ops-boxes'])
    m.push(
      view({
        items: [
          fixtureItem('ohana', { fields: { version: '0.44.4', phase: 'ready' } }),
          fixtureItem('akzm', { health: 'crit', reason: 'timeout', fields: { version: null, phase: null } }),
          fixtureItem('luzz', { fields: { version: '0.45.9', phase: 'ready' } }),
        ],
        counts: { ok: 2, warn: 0, crit: 1, unknown: 0 },
      }),
    )
    // The second source hasn't answered: its own line, the first still draws.
    expect(m.card.querySelector('[data-k2c-source="k2-ops-boxes"]')?.textContent).toContain('Loading')
    m.push(opsBoxes(), 1)
    const tile = (k: string) => m.card.querySelector(`[data-k2c-item="${k}"]`)
    expect(tile('ohana')?.textContent).toContain('customer')
    expect(tile('ohana')?.textContent).toContain('go: rosson')
    expect(tile('akzm')?.getAttribute('data-k2c-state')).toBe('crit')
    expect(tile('akzm')?.textContent).toContain('0.46.0')
    expect(tile('luzz')?.getAttribute('data-k2c-state')).toBe('warn')
    expect(m.card.querySelectorAll('[data-k2c-item]')).toHaveLength(3)
    expect(m.card.querySelector('[data-k2c-banner]')?.textContent).toBe('1 down · 1 warning')
    expect(m.card.textContent).toContain('2 sources')
    expect(m.card.querySelector('[data-k2c-source]')).toBeNull()
    // One source stale: its own line; the other's boxes still draw.
    m.push(opsBoxes({ phase: 'stale', display: 'stale', reasonText: 'Timed out.' }), 1)
    expect(m.card.querySelector('[data-k2c-source="k2-ops-boxes"]')?.textContent).toBe('k2-ops-boxes: Stale: Timed out.')
    expect(m.card.querySelector('.k2c-greyed')).toBeNull()
    expect(tile('ohana')?.getAttribute('data-k2c-state')).toBe('ok')
    // Ask goes to the source the box came from.
    ;(tile('akzm')?.querySelector('[data-k2c-ask]') as HTMLButtonElement).click()
    expect(m.asks).toEqual([['fleet', 'akzm']])
  })

  it('@2 says "soft crit 2/3" from soft.level, on the card and on a tile', async () => {
    const m = await mount('fleet-grid-2', { source: 'k2-ops-boxes' })
    m.push(
      opsBoxes({
        soft: { count: 2, of: 3, level: 'crit' },
        items: [fixtureItem('ohana', { health: 'warn', soft: { count: 2, of: 3, level: 'crit' }, fields: { version: '0.44.4' } })],
      }),
    )
    expect(m.card.querySelector('.k2c-soft')?.textContent).toBe('soft crit 2/3')
    expect(m.card.querySelector('.k2c-soft')?.getAttribute('data-k2c-level')).toBe('crit')
    expect(m.card.querySelector('[data-k2c-item="ohana"]')?.textContent).toContain('soft crit 2/3')
    expect(m.card.textContent).not.toContain('retrying')
  })

  it('@2 keeps named lists out of the grid; config.list shows one', async () => {
    const certs = view({
      name: 'k2-ops-certs',
      items: [
        fixtureItem('k2.dev', { fields: { days: 60 } }),
        fixtureItem('certs/app.k2.dev', { list: 'certs', health: 'warn', fields: { cert_days: 12 } }),
      ],
    })
    let m = await mount('fleet-grid-2', { source: 'k2-ops-certs' })
    m.push(certs)
    expect([...m.card.querySelectorAll('[data-k2c-item]')].map((t) => t.getAttribute('data-k2c-item'))).toEqual(['k2.dev'])
    m = await mount('fleet-grid-2', { source: 'k2-ops-certs', list: 'certs', detailFields: 'cert_days' })
    m.push(certs)
    const t = m.card.querySelector('[data-k2c-item="certs/app.k2.dev"]')
    expect(t?.getAttribute('data-k2c-state')).toBe('warn')
    expect(t?.textContent).toContain('12')
    expect(m.card.querySelector('[data-k2c-banner]')?.textContent).toBe('1 warning')
  })

  it('@2 subscribes again by itself after a refusal: 5 s, then 10 s, back to 5 s on a view', async () => {
    vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'setTimeout', 'clearTimeout'] })
    const m = await mount('fleet-grid-2', { source: 'fleet' })
    expect(m.subs).toHaveLength(1)
    m.fail({ code: 'not_bound', message: 'x' })
    expect(m.card.textContent).toContain('Not bound to this card yet')
    vi.advanceTimersByTime(4_999)
    expect(m.subs).toHaveLength(1)
    vi.advanceTimersByTime(1)
    expect(m.subs).toHaveLength(2)
    expect(m.subs[1].address).toBe('fleet')
    m.fail({ code: 'not_bound', message: 'x' }, 1)
    vi.advanceTimersByTime(9_999)
    expect(m.subs).toHaveLength(2)
    vi.advanceTimersByTime(1)
    expect(m.subs).toHaveLength(3)
    m.push(healthy('fleet-grid-2'), 2)
    expect(m.card.querySelector('[data-k2c-item="alpha"]')).not.toBeNull()
    m.fail({ code: 'removed', message: 'x' }, 2)
    vi.advanceTimersByTime(5_000)
    expect(m.subs).toHaveLength(4)
  })

  it('@1 never subscribes again (as shipped)', async () => {
    vi.useFakeTimers({ toFake: ['Date', 'setInterval', 'setTimeout', 'clearTimeout'] })
    const m = await mount('fleet-grid-1', { source: 'fleet' })
    m.fail({ code: 'removed', message: 'x' })
    vi.advanceTimersByTime(120_000)
    expect(m.subs).toHaveLength(1)
  })

  it('version spread @2: the fleet target from a source, its behind / far behind counts, rated boxes keep their health', async () => {
    const m = await mount('version-spread-2', { source: 'k2-ops-boxes' })
    m.push(opsBoxes())
    await flush()
    expect(m.card.querySelector('[data-k2c-target]')?.textContent).toBe('Fleet target: 0.46.0 · 2 behind · 1 far behind')
    expect([...m.card.querySelectorAll('[data-k2c-version]')].map((b) => [b.getAttribute('data-k2c-version'), b.className.includes('k2c-warn')])).toEqual([
      ['0.46.0', false],
      ['0.45.9', true],
      ['0.44.4', true],
    ])
    expect(m.card.querySelector('.k2c-number')?.className).toContain('k2c-crit')
    expect(m.card.querySelector('[data-k2c-item="ohana"]')?.getAttribute('data-k2c-state')).toBe('warn')
    expect(m.card.querySelector('[data-k2c-item="akzm"]')).toBeNull()
  })

  it('version spread @2: config.target when no source names one; a release still wins', async () => {
    let m = await mount('version-spread-2', { source: 'fleet', target: '0.45.6' })
    m.push(view())
    await flush()
    expect(m.card.querySelector('[data-k2c-target]')?.textContent).toBe('Target: 0.45.6')
    m = await mount('version-spread-2', { source: 'k2-ops-boxes', latest: 'release' })
    m.push(opsBoxes())
    m.push(view({ name: 'release', fields: { newest: 'v0.46.1' }, items: [] }, 'release'), 1)
    await flush()
    expect(m.card.querySelector('[data-k2c-target]')?.textContent).toContain('Newest release: v0.46.1 · 3 behind')
  })

  it('relays & DNS @2: K2 Web ops edge relays, the DNS row and status-only checks', async () => {
    const m = await mount('relays-dns-2', { source: 'k2-ops-edge', sources: 'k2-dev-site,k2-connect' })
    m.push(
      view({
        name: 'k2-ops-edge',
        fields: { wildcard_a: 'primary', failover_armed: true, relays_ts: NOW - 20 },
        items: [
          fixtureItem('k2e-01', { fields: { role: 'primary', tunnels: 41, cp: 'active', frps: 'active' } }),
          fixtureItem('k2e-02', { health: 'crit', fields: { role: 'standby', tunnels: 0, cp: 'failed', frps: 'active' } }),
        ],
        counts: { ok: 1, warn: 0, crit: 1, unknown: 0 },
      }, 'k2-ops-edge'),
    )
    m.push(view({ name: 'k2-dev-site', fields: {}, items: [], counts: { ok: 0, warn: 0, crit: 0, unknown: 0 }, httpStatus: 200 }, 'k2-dev-site'), 1)
    m.push(view({ name: 'k2-connect', fields: {}, items: [], counts: { ok: 0, warn: 0, crit: 0, unknown: 0 }, httpStatus: 401 }, 'k2-connect'), 2)
    const tile = (k: string) => m.card.querySelector(`[data-k2c-item="${k}"]`)
    expect(tile('k2e-01')?.textContent).toContain('primary · 41 tunnels')
    expect(tile('k2e-02')?.getAttribute('data-k2c-state')).toBe('crit')
    expect(tile('dns')?.textContent).toContain('DNS: primary')
    expect(tile('dns')?.textContent).toContain('failover armed')
    expect(tile('k2-connect')?.textContent).toContain('HTTP 401')
    expect(tile('k2-connect')?.getAttribute('data-k2c-state')).toBe('ok')
    expect(tile('k2-dev-site')?.textContent).toContain('HTTP 200')
    m.push(view({ name: 'k2-connect', health: 'crit', display: 'crit', fields: {}, items: [], httpStatus: 502, reason: 'http_5xx' }, 'k2-connect'), 2)
    ;(tile('k2-connect')?.querySelector('[data-k2c-ask]') as HTMLButtonElement).click()
    expect(m.asks).toEqual([['k2-connect', null]])
  })

  it('relays & DNS @2 says what it needs for a shape it can’t draw', async () => {
    const m = await mount('relays-dns-2', { source: 'fleet' })
    m.push(view())
    expect(m.card.querySelector('[data-k2c-needs]')?.textContent).toContain('Needs K2 Web’s fleet-summary or ops edge')
  })

  it('deploys @2: what each part runs from K2 Web’s deploys fields', async () => {
    const m = await mount('deploys-2', { source: 'k2-ops-edge' })
    m.push(view({ fields: { web_sha: 'abcdef0123', cp_sha_k2e01: '1234567890', cp_sha_k2e02: '1234567890' }, items: [] }, 'k2-ops-edge'))
    expect(m.card.querySelector('[data-k2c-item="web"]')?.textContent).toContain('abcdef0')
    expect(m.card.querySelector('[data-k2c-item="cp_sha_k2e01"]')?.textContent).toContain('control plane k2e-01')
    expect(m.card.querySelector('[data-k2c-banner]')).toBeNull()
  })

  it('money alerts @2: K2 Web billing: past due, not kept live, canceled still routing, reconcile', async () => {
    const m = await mount('money-alerts-2', { source: 'k2-ops-billing' })
    m.push(
      view(
        {
          name: 'k2-ops-billing',
          health: 'crit',
          display: 'crit',
          fields: { past_due: 2, past_due_not_kept: 1, canceled_routing: 1, reconcile_ok: true, ts: NOW - 10 },
          items: [
            fixtureItem('edf', { health: 'warn', fields: { keep_live: false } }),
            fixtureItem('noir', { fields: { keep_live: true } }),
            fixtureItem('canceled/quillify', { list: 'canceled', health: 'crit', fields: { canceled_label: 'quillify' } }),
          ],
          counts: { ok: 1, warn: 1, crit: 1, unknown: 0 },
        },
        'k2-ops-billing',
      ),
    )
    const row = (k: string) => m.card.querySelector(`[data-k2c-item="${k}"]`)
    expect(m.card.querySelector('[data-k2c-banner]')?.textContent).toBe('Money needs a person now')
    expect(row('past_due')?.textContent).toContain('2 past due')
    expect(row('past_due')?.textContent).toContain('1 not kept live')
    expect(row('edf')?.getAttribute('data-k2c-state')).toBe('warn')
    expect(row('canceled_routing')?.textContent).toContain('1 canceled but still routing')
    expect(row('canceled/quillify')?.textContent).toContain('quillify')
    expect(row('reconcile')?.getAttribute('data-k2c-state')).toBe('ok')
    // Stripe's states-only source alone keeps @1's words.
    const s = await mount('money-alerts-2', { source: 'money' })
    s.push(view({ keep: 'states', health: 'crit', display: 'crit', fields: {}, items: [] }))
    expect(s.card.querySelector('[data-k2c-banner]')?.textContent).toBe('Past-due subscriptions found')
    expect(s.card.textContent).toContain('States only')
  })
})
