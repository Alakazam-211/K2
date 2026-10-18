// @vitest-environment jsdom
// Garden v3 (contract-sources-v1 §11.4, §12.3 "Cards") — the five built-in
// data-source cards, run byte for byte (kit.js + card.js from k2-core)
// against a fake `k2` that hands over fixture card views. Every card draws
// every PRD §12 state: updated N ago from the server's clock, never blank,
// unknown in its own colour, retrying n/of, recovered, muted and TEST
// badges, the sleep / restart gap, unreachable (last value greyed), not
// saved, removed, waiting for the first check, a down box as a red tile,
// and Ask an agent on red, stale and unknown.
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { zenSourceView, zenStaleView, type ZenSourceView } from './zen-sources'
import { fixtureItem, fixtureView } from './zen-sources-fixtures'
import type { SourceResult } from '@/lib/sources/sources-types'

const ROOT = resolve(__dirname, '../../../../crates/k2-core/src/zen/builtin_widgets')
const KIT = readFileSync(resolve(ROOT, 'sources-kit/kit.js'), 'utf8')
const CARDS = ['fleet-grid-1', 'version-spread-1', 'relays-dns-1', 'deploys-1', 'money-alerts-1'] as const
type Card = (typeof CARDS)[number]

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
  const html = readFileSync(resolve(ROOT, card, 'index.html'), 'utf8')
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
  new Function(KIT)()
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
      return view({
        items: [
          fixtureItem('ohana', { fields: { version: '0.45.6', phase: 'ready', relay: 'k2e-01', up: true } }),
          fixtureItem('luzz', { fields: { version: '0.45.6', phase: 'ready', relay: 'k2e-02', up: true } }),
        ],
      })
    case 'deploys-1':
      return view({
        items: [
          fixtureItem('dpl_a', { fields: { state: 'READY', url: 'app-a.vercel.app', created: (NOW - 120) * 1000, target: 'production' } }),
          fixtureItem('dpl_b', { fields: { state: 'READY', url: 'app-b.vercel.app', created: (NOW - 7200) * 1000, target: 'production' } }),
        ],
      })
    case 'money-alerts-1':
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
