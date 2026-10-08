// prd-zen-garden-sync-defaults-v1 GS52 (renderer) — `zen-sync.ts`: the
// boundary parsers, the side card's show rule, the card text, the New
// badge's shorts, and the preview parameter `fetchZenPage` sends. Only the
// daemon (`daemon-cli`) is faked. Fail loudly.

import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST'; hostKey: string; route: string; data: unknown }>,
  answers: new Map<string, unknown>(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
    if (!h.answers.has(route)) throw new Error(`unexpected GET ${route}`)
    const a = h.answers.get(route)
    if (a instanceof Error) throw a
    return a
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
    return { ok: true }
  }),
}))

vi.mock('@/stores/session-events', () => ({
  onZenChanged: vi.fn(() => () => undefined),
  subscribeToActiveState: vi.fn(() => () => undefined),
}))

import {
  __resetZenSyncForTests,
  loadZenSync,
  markZenNewsSeen,
  parseZenNews,
  parseZenSyncList,
  useZenSyncPreviewStore,
  useZenSyncStore,
  ZEN_NEWS_CARD_MIN_WIDTH,
  zenNewCatalogShorts,
  zenNewsCardModel,
  zenNewsCardText,
  zenSyncPreviewParam,
  ZenSyncParseError,
  type ZenNews,
} from './zen-sync'
import { fetchZenPage } from './zen-api'

const SYNC_ANSWER = {
  ok: true,
  liveDefaults: 'd-3f9a12c0b1e4d7a2',
  k2Version: '0.45.1',
  previousDefaults: null,
  migratedAt: '2026-10-20T09:00:00Z',
  gardens: [
    {
      id: 'g-1',
      name: 'Garden 1',
      index: 1,
      page: { mode: 'synced', newerDefault: false },
      theme: { mode: 'synced', newerDefault: false },
      ownChanges: [],
      ownChangesError: null,
      undo: null,
      keepPrevious: { page: false, theme: false },
      themeName: 'basic',
      themeLabel: 'Basic',
      themeBuiltin: true,
      themeBase: { name: 'basic', label: 'Basic' },
      gardenTheme: null,
      damaged: [],
    },
    {
      id: 'g-2',
      name: 'Garden 2',
      index: 2,
      page: { mode: 'copy', defaults: 'd-3f9a12c0b1e4d7a2', since: '2026-10-20T09:00:00Z', reason: 'upgrade', newerDefault: true, k2Version: '0.45.1' },
      theme: { mode: 'copy', defaults: 'd-3f9a12c0b1e4d7a2', reason: 'upgrade', newerDefault: false, k2Version: '0.45.1' },
      ownChanges: ['layout', 'widget'],
      ownChangesError: null,
      undo: { parts: ['page'], at: '2026-10-21T10:00:00Z' },
      keepPrevious: { page: false, theme: false },
      themeName: 'hearth',
      themeLabel: 'Hearth',
      themeBuiltin: false,
      themeBase: { name: 'basic', label: 'Basic' },
      gardenTheme: 'hearth',
      damaged: [],
    },
  ],
}

const NEWS_ANSWER = {
  ok: true,
  items: [
    { id: 'update:d-1111222233334444:g-1', kind: 'update', garden: 'g-1', gardenName: 'Garden 1', part: 'page', previous: 'd-3f9a12c0b1e4d7a2', keepPrevious: true, at: 'x' },
    { id: 'catalog:diary', kind: 'catalog', short: 'diary', label: 'Diary', description: 'Write to one agent at a time; replies appear in handwriting.', template: 'k2.diary@1', version: 1 },
    { id: 'future:x', kind: 'future-kind' },
  ],
  copiesWithNewerDefault: 2,
  liveDefaults: 'd-1111222233334444',
  previousDefaults: 'd-3f9a12c0b1e4d7a2',
}

afterEach(() => {
  h.calls.length = 0
  h.answers.clear()
  __resetZenSyncForTests()
})

describe('zen-sync parsing (GS30, GS31, GS43a)', () => {
  it('parses GET /cli/zen/sync rows', () => {
    const list = parseZenSyncList(SYNC_ANSWER)
    expect(list.liveDefaults).toBe('d-3f9a12c0b1e4d7a2')
    expect(list.gardens).toHaveLength(2)
    const [g1, g2] = list.gardens
    expect(g1.page).toEqual({ mode: 'synced', defaults: null, since: null, reason: null, newerDefault: false, k2Version: null })
    expect(g2.page.mode).toBe('copy')
    expect(g2.page.newerDefault).toBe(true)
    expect(g2.theme.k2Version).toBe('0.45.1')
    expect(g2.ownChanges).toEqual(['layout', 'widget'])
    expect(g2.undo).toEqual({ parts: ['page'], at: '2026-10-21T10:00:00Z' })
    expect(g2.themeBase.label).toBe('Basic')
  })

  it('refuses a malformed answer loudly', () => {
    const bad = structuredClone(SYNC_ANSWER) as { gardens: Array<{ page: { mode: string } }> }
    bad.gardens[0].page.mode = 'sideways'
    expect(() => parseZenSyncList(bad)).toThrow(ZenSyncParseError)
    expect(() => parseZenSyncList({ ok: true })).toThrow(/liveDefaults/)
    expect(() => parseZenNews({ items: [{ kind: 'update', id: 'u', garden: 'g', gardenName: 'G', part: 'all', previous: 'p', keepPrevious: true }], copiesWithNewerDefault: 0 })).toThrow(/part/)
  })

  it('parses news, newest first, and skips kinds it does not know', () => {
    const news = parseZenNews(NEWS_ANSWER)
    expect(news.items.map((i) => i.id)).toEqual(['update:d-1111222233334444:g-1', 'catalog:diary'])
    expect(news.copiesWithNewerDefault).toBe(2)
    expect(zenNewCatalogShorts(news)).toEqual(['diary'])
    expect(zenNewCatalogShorts(null)).toEqual([])
  })
})

describe('the What’s new side card rule (GS42, GS43)', () => {
  const news: ZenNews = parseZenNews(NEWS_ANSWER)

  it('shows only when the dialog is visible, news is unseen and the window is wide enough', () => {
    expect(zenNewsCardModel(news, { dialogVisible: false, viewportWidth: 1400 })).toBeNull()
    expect(zenNewsCardModel(news, { dialogVisible: true, viewportWidth: ZEN_NEWS_CARD_MIN_WIDTH - 1 })).toBeNull()
    expect(zenNewsCardModel(null, { dialogVisible: true, viewportWidth: 1400 })).toBeNull()
    expect(zenNewsCardModel({ items: [], copiesWithNewerDefault: 3 }, { dialogVisible: true, viewportWidth: 1400 })).toBeNull()
    const m = zenNewsCardModel(news, { dialogVisible: true, viewportWidth: ZEN_NEWS_CARD_MIN_WIDTH })
    expect(m).not.toBeNull()
    expect(m?.item.id).toBe('update:d-1111222233334444:g-1')
    expect(m?.more).toBe(1)
    expect(m?.copiesLine).toBe(true)
    expect(m?.shownIds).toEqual(['update:d-1111222233334444:g-1'])
  })

  it('words the two kinds of item the PRD way', () => {
    const [update, catalog] = news.items
    expect(zenNewsCardText(catalog)).toEqual({
      title: 'New in the Garden catalog',
      lead: 'Diary.',
      body: 'Write to one agent at a time; replies appear in handwriting.',
      button: 'See it in New Garden',
    })
    expect(zenNewsCardText(update)).toEqual({
      title: 'Garden 1 has a fresh look',
      lead: null,
      body: 'It syncs with K2’s default, so it got this release’s improvements.',
      button: 'Keep my previous look',
    })
    const noPrev = { ...update, keepPrevious: false } as typeof update
    expect(zenNewsCardText(noPrev).button).toBeNull()
  })
})

describe('loading and gestures go to THIS computer’s daemon', () => {
  it('loads both routes from the local scope', async () => {
    h.answers.set('zen/sync', SYNC_ANSWER)
    h.answers.set('zen/news', NEWS_ANSWER)
    await loadZenSync()
    const s = useZenSyncStore.getState()
    expect(s.supported).toBe(true)
    expect(s.list?.gardens).toHaveLength(2)
    expect(s.news?.items).toHaveLength(2)
    expect(h.calls.every((c) => c.hostKey === 'local')).toBe(true)
  })

  it('an older daemon (no zen-sync-v1) means no switches and no card', async () => {
    h.answers.set('zen/sync', new Error('{"error":"unknown zen route","path":"/cli/zen/sync"}'))
    h.answers.set('zen/news', new Error('{"error":"unknown zen route","path":"/cli/zen/news"}'))
    await loadZenSync()
    const s = useZenSyncStore.getState()
    expect(s.supported).toBe(false)
    expect(s.list).toBeNull()
    expect(s.news).toBeNull()
    expect(s.failure).toBeNull()
  })

  it('marking news seen posts the ids and drops them locally', async () => {
    useZenSyncStore.setState({ news: parseZenNews(NEWS_ANSWER) })
    await markZenNewsSeen(['catalog:diary'])
    expect(h.calls).toEqual([{ method: 'POST', hostKey: 'local', route: 'zen/news/seen', data: { ids: ['catalog:diary'] } }])
    expect(useZenSyncStore.getState().news?.items.map((i) => i.id)).toEqual(['update:d-1111222233334444:g-1'])
    await markZenNewsSeen([])
    expect(h.calls).toHaveLength(1)
  })
})

describe('preview (GS25): a parameter on GET zen/get, never a write', () => {
  it('fetchZenPage adds preview= only for the previewed Garden', async () => {
    const page = {
      ok: true,
      schema: 1,
      version: 'v',
      garden: { id: 'g-2', name: 'Garden 2', index: 2 },
      page: { template: 'k2.texting@1', layout: { kind: 'columns', split: [34, 66], minWidths: [240, 360] }, widgets: [], controls: [] },
      theme: {},
      chrome: {},
      motion: {},
      errors: [],
      warnings: [],
      lastGoodAt: null,
    }
    h.answers.set('zen/get', page)
    await fetchZenPage('g-2')
    expect(h.calls.at(-1)?.data).toEqual({ garden: 'g-2' })
    useZenSyncPreviewStore.getState().start('g-2', 'page')
    expect(zenSyncPreviewParam('g-2')).toBe('page')
    expect(zenSyncPreviewParam('g-1')).toBeUndefined()
    await fetchZenPage('g-2')
    expect(h.calls.at(-1)?.data).toEqual({ garden: 'g-2', preview: 'page' })
    await fetchZenPage('g-1')
    expect(h.calls.at(-1)?.data).toEqual({ garden: 'g-1' })
    useZenSyncPreviewStore.getState().end()
    expect(zenSyncPreviewParam('g-2')).toBeUndefined()
    expect(h.calls.filter((c) => c.method === 'POST')).toEqual([])
  })
})
