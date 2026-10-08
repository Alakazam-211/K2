// @vitest-environment jsdom
//
// prd-zen-garden-sync-defaults-v1 GS52 (renderer) — the Garden news card,
// its dismissal with the What's new dialog, the Settings switches, the
// preview bar and the New Garden "New" badge. Only the edges are faked:
// the daemon (`daemon-cli`), the app socket, Tauri and Markdown.
//
// Asserted (fail loudly):
//   - the card sits on the LEFT (mirror of "Enjoying K2?"), shows one item,
//     only when the dialog is open, news is unseen and the window is at
//     least 1000 px wide, and asks THIS computer's daemon;
//   - closing the dialog posts `zen/news/seen` with exactly the ids the
//     card showed; a hidden card marks nothing;
//   - the card's button marks seen, does its action and closes the dialog;
//   - Settings: turning sync off writes at once; turning it on asks first
//     (Preview in Zen / Turn on) and only Turn on writes;
//   - the preview bar's "Back to my copy" never writes;
//   - New Garden shows "New" on an entry with unseen catalog news.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  calls: [] as Array<{ method: 'GET' | 'POST' | 'POSTQ'; hostKey: string; route: string; data: unknown }>,
  news: null as unknown,
  sync: null as unknown,
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn(async () => undefined), revealItemInDir: vi.fn(async () => undefined) }))
vi.mock('../Markdown/Markdown', () => ({ default: ({ children }: { children: string }) => <div>{children}</div> }))
vi.mock('@/stores/session-events', async (importOriginal) => {
  const mod = await importOriginal<typeof import('@/stores/session-events')>()
  return { ...mod, onZenChanged: () => () => undefined, subscribeToActiveState: () => () => undefined }
})
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string, params?: unknown) => {
    h.calls.push({ method: 'GET', hostKey: scope.hostKey, route, data: params })
    if (route === 'whats_new') return { current_version: '0.45.1', last_seen_version: '0.45.0', has_new: true, content: '## 0.45.1 — Gardens\n\nBody.\n' }
    if (route === 'zen/news') return h.news
    if (route === 'zen/sync') return h.sync
    if (route === 'zen/templates') return { ok: true, templates: TEMPLATES }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (scope: { hostKey: string }, route: string, body?: unknown) => {
    h.calls.push({ method: 'POST', hostKey: scope.hostKey, route, data: body })
    return { ok: true }
  }),
  daemonCliPostQuery: vi.fn(async (scope: { hostKey: string }, route: string) => {
    h.calls.push({ method: 'POSTQ', hostKey: scope.hostKey, route, data: null })
    return { ok: true }
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { act } from 'react'
import { cleanup, fireEvent, render, waitFor } from '@testing-library/react'
import WhatsNewModal from './WhatsNewModal'
import { ZenGardenNewsCard } from './ZenGardenNewsCard'
import { ZenGardenSyncControls } from '@/components/Settings/sections/ZenGardenSyncControls'
import { ZenSyncPreviewBar } from '@/components/Zen/ZenSyncPreviewBar'
import { useZenOpenNewGardenRequest, ZenNewGarden } from '@/components/Zen/widgets/ZenNewGarden'
import { __resetZenTemplatesForTests, useZenCatalogBadges } from '@/lib/zen/zen-templates'
import { __resetZenAvailableForTests } from '@/lib/zen/zen-platform'
import {
  __resetZenSyncForTests,
  parseZenNews,
  useZenSyncPreviewStore,
  useZenSyncStore,
  ZEN_OPEN_NEW_GARDEN_EVENT,
} from '@/lib/zen/zen-sync'
import { useState } from 'react'
import { useSettingsStore } from '@/stores/settings'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const TEMPLATES = [
  { id: 'k2.texting@1', short: 'texting', label: 'Start with the default', description: '', section: 'start', needsGrant: null, newUsers: true },
  { id: 'k2.blank@1', short: 'blank', label: 'Start empty and ask my agent', description: '', section: 'start', needsGrant: null, newUsers: true },
  {
    id: 'k2.diary@1',
    short: 'diary',
    label: 'Diary',
    description: 'Write to one agent at a time.',
    section: 'catalog',
    needsGrant: { widget: 'k2:diary@1', caps: ['agents:read', 'thread:read', 'thread:post'] },
    newUsers: true,
  },
]

const DIARY = {
  id: 'catalog:diary',
  kind: 'catalog',
  short: 'diary',
  label: 'Diary',
  description: 'Write to one agent at a time; replies appear in handwriting.',
  template: 'k2.diary@1',
  version: 1,
}
const UPDATE = {
  id: 'update:d-1111222233334444:g-1',
  kind: 'update',
  garden: 'g-1',
  gardenName: 'Garden 1',
  part: 'page',
  previous: 'd-3f9a12c0b1e4d7a2',
  keepPrevious: true,
}

function row(id: string, name: string, page: 'synced' | 'copy', theme: 'synced' | 'copy'): unknown {
  const part = (mode: string): unknown =>
    mode === 'copy' ? { mode, defaults: 'd-3f9a12c0b1e4d7a2', newerDefault: false, k2Version: '0.45.1' } : { mode, newerDefault: false }
  return {
    id,
    name,
    index: 1,
    page: part(page),
    theme: part(theme),
    ownChanges: [],
    undo: null,
    keepPrevious: { page: false, theme: false },
    themeName: 'basic',
    themeLabel: 'Basic',
    themeBuiltin: true,
    themeBase: { name: 'basic', label: 'Basic' },
  }
}

function setWidth(w: number): void {
  Object.defineProperty(window, 'innerWidth', { value: w, configurable: true, writable: true })
}

function el(selector: string): HTMLElement {
  const found = document.querySelector(selector)
  if (!(found instanceof HTMLElement)) throw new Error(`no ${selector}\n${document.body.innerHTML.slice(0, 2000)}`)
  return found
}

function posts(route: string): unknown[] {
  return h.calls.filter((c) => c.method === 'POST' && c.route === route).map((c) => c.data)
}

beforeEach(() => {
  h.calls.length = 0
  h.news = { ok: true, items: [DIARY], copiesWithNewerDefault: 0 }
  h.sync = { ok: true, liveDefaults: 'd-3f9a12c0b1e4d7a2', k2Version: '0.45.1', previousDefaults: null, gardens: [] }
  Object.defineProperty(window.navigator, 'platform', { value: 'MacIntel', configurable: true })
  __resetZenAvailableForTests()
  __resetZenSyncForTests()
  __resetZenTemplatesForTests()
  useSettingsStore.setState({ settingsOpen: false })
  setWidth(1280)
})

afterEach(() => {
  cleanup()
})

describe('the Garden news card (GS40–GS43)', () => {
  it('sits on the left, asks the local daemon, and reports what it shows', async () => {
    const shown: string[][] = []
    render(<ZenGardenNewsCard dialogVisible onShown={(ids) => shown.push(ids)} onAction={() => undefined} />)
    await waitFor(() => el('[data-zen-news-card]'))
    const card = el('[data-zen-news-card]')
    expect(card.style.right).toBe('100%')
    expect(card.style.left).toBe('')
    expect(card.style.marginRight).toBe('-10px')
    expect(card.style.width).toBe('176px')
    expect(card.textContent).toContain('New in the Garden catalog')
    expect(card.textContent).toContain('Diary.')
    expect(el('[data-zen-news-action]').textContent).toBe('See it in New Garden')
    expect(h.calls.find((c) => c.route === 'zen/news')?.hostKey).toBe('local')
    expect(shown.at(-1)).toEqual(['catalog:diary'])
  })

  it('is hidden below 1000 px and then marks nothing', async () => {
    setWidth(999)
    const shown: string[][] = []
    render(<ZenGardenNewsCard dialogVisible onShown={(ids) => shown.push(ids)} onAction={() => undefined} />)
    await waitFor(() => expect(h.calls.some((c) => c.route === 'zen/news')).toBe(true))
    expect(document.querySelector('[data-zen-news-card]')).toBeNull()
    expect(shown.every((ids) => ids.length === 0)).toBe(true)
  })

  it('shows one item, the more line and the copies line', async () => {
    h.news = { ok: true, items: [UPDATE, DIARY], copiesWithNewerDefault: 1 }
    render(<ZenGardenNewsCard dialogVisible onShown={() => undefined} onAction={() => undefined} />)
    await waitFor(() => el('[data-zen-news-card]'))
    expect(el('[data-zen-news-card]').textContent).toContain('Garden 1 has a fresh look')
    expect(el('[data-zen-news-action]').textContent).toBe('Keep my previous look')
    expect(el('[data-zen-news-settings]').textContent).toBe('+ 1 more in Settings → Gardens')
    expect(el('[data-zen-news-copies]').textContent).toContain('Your own copies didn’t change')
  })

  it('its button marks seen, does the action and closes the dialog', async () => {
    const closed = vi.fn()
    render(<ZenGardenNewsCard dialogVisible onShown={() => undefined} onAction={closed} />)
    await waitFor(() => el('[data-zen-news-action]'))
    await act(async () => void fireEvent.click(el('[data-zen-news-action]')))
    await waitFor(() => expect(closed).toHaveBeenCalledTimes(1))
    expect(posts('zen/news/seen')).toEqual([{ ids: ['catalog:diary'] }])
    expect(useSettingsStore.getState().settingsOpen).toBe(true)
    expect(useSettingsStore.getState().activeSection).toBe('zen-gardens')
  })
})

describe('What’s new dismissal marks the card’s news seen (GS43)', () => {
  it('Got it posts zen/news/seen with exactly the shown ids', async () => {
    h.news = { ok: true, items: [UPDATE, DIARY], copiesWithNewerDefault: 0 }
    render(<WhatsNewModal mode="button-only" />)
    await act(async () => void window.dispatchEvent(new Event('k2so:show-whats-new')))
    await waitFor(() => el('[data-zen-news-card]'))
    await act(async () => void fireEvent.click(Array.from(document.querySelectorAll('button')).find((b) => b.textContent === 'Got it') as HTMLElement))
    await waitFor(() => expect(posts('zen/news/seen')).toEqual([{ ids: [UPDATE.id] }]))
    expect(h.calls.some((c) => c.route === 'whats_new/mark_seen')).toBe(true)
    expect(document.querySelector('[data-zen-news-card]')).toBeNull()
  })

  it('a narrow window shows no card and closing marks nothing', async () => {
    setWidth(900)
    render(<WhatsNewModal mode="button-only" />)
    await act(async () => void window.dispatchEvent(new Event('k2so:show-whats-new')))
    await waitFor(() => expect(h.calls.some((c) => c.route === 'zen/news')).toBe(true))
    await act(async () => void fireEvent.click(Array.from(document.querySelectorAll('button')).find((b) => b.textContent === 'Got it') as HTMLElement))
    await waitFor(() => expect(h.calls.some((c) => c.route === 'whats_new/mark_seen')).toBe(true))
    expect(posts('zen/news/seen')).toEqual([])
  })
})

describe('Settings → Gardens switches (GS33–GS34a)', () => {
  it('off writes at once; on asks first and only Turn on writes', async () => {
    h.sync = {
      ok: true,
      liveDefaults: 'd-3f9a12c0b1e4d7a2',
      k2Version: '0.45.1',
      previousDefaults: null,
      gardens: [row('g-1', 'Garden 1', 'synced', 'copy')],
    }
    render(<ZenGardenSyncControls garden={{ id: 'g-1', name: 'Garden 1' }} />)
    await waitFor(() => el('[data-zen-sync-controls="g-1"]'))
    const page = el('[data-testid="zen-sync-page-g-1"] [role="switch"]')
    expect(page.getAttribute('aria-checked')).toBe('true')
    expect(el('[data-zen-sync-part="theme"]').textContent).toContain('Sync theme with K2’s Basic')
    expect(el('[data-zen-sync-part="theme"]').textContent).toContain('K2 themes from 0.45.1.')
    await act(async () => void fireEvent.click(page))
    await waitFor(() => expect(posts('zen/garden/sync')).toEqual([{ garden: 'g-1', part: 'page', sync: false }]))
    const theme = el('[data-testid="zen-sync-theme-g-1"] [role="switch"]')
    await act(async () => void fireEvent.click(theme))
    el('[data-zen-sync-confirm="theme"]')
    expect(posts('zen/garden/sync')).toHaveLength(1)
    await act(async () => void fireEvent.click(el('[data-zen-sync-turn-on="theme"]')))
    await waitFor(() => expect(posts('zen/garden/sync')).toEqual([
      { garden: 'g-1', part: 'page', sync: false },
      { garden: 'g-1', part: 'theme', sync: true },
    ]))
  })

  it('draws nothing when the daemon has no Garden sync', async () => {
    h.sync = null
    const { container } = render(<ZenGardenSyncControls garden={{ id: 'g-1', name: 'Garden 1' }} />)
    await waitFor(() => expect(h.calls.some((c) => c.route === 'zen/sync')).toBe(true))
    expect(container.innerHTML).toBe('')
  })
})

describe('preview bar (GS34) and the New badge (GS43b)', () => {
  it('Back to my copy ends the preview and never writes', async () => {
    useZenSyncPreviewStore.getState().start('g-2', 'page')
    render(<ZenSyncPreviewBar gardenId="g-2" gardenName="Garden 2" part="page" />)
    expect(el('[data-zen-sync-preview-bar]').textContent).toContain('Previewing K2’s default for Garden 2')
    await act(async () => void fireEvent.click(el('[data-zen-sync-preview-back]')))
    expect(useZenSyncPreviewStore.getState().preview).toBeNull()
    expect(h.calls.filter((c) => c.method !== 'GET')).toEqual([])
  })

  it('Turn sync on writes the previewed part', async () => {
    useZenSyncPreviewStore.getState().start('g-2', 'theme')
    render(<ZenSyncPreviewBar gardenId="g-2" gardenName="Garden 2" part="theme" />)
    await act(async () => void fireEvent.click(el('[data-zen-sync-preview-on]')))
    await waitFor(() => expect(posts('zen/garden/sync')).toEqual([{ garden: 'g-2', part: 'theme', sync: true }]))
    await waitFor(() => expect(useZenSyncPreviewStore.getState().preview).toBeNull())
  })

  it('New Garden marks an entry with unseen catalog news (B4\'s catalog badge, driven by the news)', async () => {
    useZenSyncStore.setState({ news: parseZenNews({ items: [DIARY], copiesWithNewerDefault: 0 }) })
    expect(useZenCatalogBadges.getState().badges).toEqual({ diary: 'New' })
    const bridge = { gardens: { list: () => [], create: vi.fn(async () => undefined) } } as unknown as ZenWidgetBridge
    render(<ZenNewGarden bridge={bridge} onDone={() => undefined} />)
    await act(async () => void fireEvent.click(el('[data-zen-new-garden]')))
    const input = el('[data-zen-new-garden-name]') as HTMLInputElement
    await act(async () => void fireEvent.change(input, { target: { value: 'Mine' } }))
    await act(async () => void fireEvent.keyDown(input, { key: 'Enter' }))
    await waitFor(() => expect(el('[data-zen-new-garden-choice="diary"]').querySelector('[data-zen-catalog-badge="diary"]')?.textContent).toBe('New'))
    expect(el('[data-zen-new-garden-choice="texting"]').querySelector('[data-zen-catalog-badge]')).toBeNull()
    // Seen news clears the badge this module set.
    useZenSyncStore.setState({ news: parseZenNews({ items: [], copiesWithNewerDefault: 0 }) })
    expect(useZenCatalogBadges.getState().badges).toEqual({})
  })

  it('"See it in New Garden" opens New Garden with that entry highlighted', async () => {
    const bridge = { gardens: { list: () => [], create: vi.fn(async () => undefined) } } as unknown as ZenWidgetBridge
    function Switcher(): React.JSX.Element {
      const [highlight, setHighlight] = useState<string | null>(null)
      useZenOpenNewGardenRequest(true, setHighlight)
      return <ZenNewGarden key={highlight ?? ''} bridge={bridge} onDone={() => undefined} highlight={highlight} />
    }
    render(<Switcher />)
    el('[data-zen-new-garden]')
    await act(async () => void window.dispatchEvent(new CustomEvent(ZEN_OPEN_NEW_GARDEN_EVENT, { detail: { short: 'diary' } })))
    const input = el('[data-zen-new-garden-name]') as HTMLInputElement
    await act(async () => void fireEvent.change(input, { target: { value: 'Mine' } }))
    await act(async () => void fireEvent.keyDown(input, { key: 'Enter' }))
    await waitFor(() => expect(el('[data-zen-new-garden-choice="diary"]').hasAttribute('data-zen-new-garden-highlight')).toBe(true))
    expect(el('[data-zen-new-garden-choice="texting"]').hasAttribute('data-zen-new-garden-highlight')).toBe(false)
  })
})
