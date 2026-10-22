// @vitest-environment jsdom
//
// prd-garden-access-v1 G26a (A5): the widget's ⋯ menu in Zen gains
// History… with Garden access on, reading "History · not signed" when the
// live version fails the signer gate in this Garden; it opens the widget
// history modal, whose filters ask the daemon for their kinds and whose Sign
// signs for this Garden. With the switch off the menu is as 0.46.3 (Reload
// only). Only the daemon is faked; fixtures are made up (g-test0001).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  gets: [] as Array<{ route: string; params: unknown }>,
  posts: [] as Array<{ route: string; body: unknown }>,
  gardenAccess: false as boolean | undefined,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params?: Record<string, unknown>) => {
    h.gets.push({ route, params })
    if (route === 'zen/widget/bundle') {
      return {
        ok: true,
        widget: params?.widget,
        hash: 'hash-1',
        nonce: 'q83vFzPq1N3V0aZ8k2LmTw',
        html: '<!doctype html><html><body><script nonce="q83vFzPq1N3V0aZ8k2LmTw">k2.ready()</script></body></html>',
        bytes: 120,
      }
    }
    if (route === 'settings/get') return h.gardenAccess === undefined ? {} : { gardenAccess: h.gardenAccess }
    if (route === 'zen/widget/history') {
      return {
        ok: true,
        widget: params?.widget,
        items: [
          { id: 'w-2', at: 1_792_000_000, kind: 'access_asked', source: 'event', actor: { kind: 'agent', id: 'p-ana', name: 'Ana' }, widget: null, detail: {} },
          { id: 'w-1', at: 1_791_990_000, kind: 'signed', source: 'stamp', actor: { kind: 'owner' }, widget: 'agent-arcade', detail: {} },
        ],
        next: null,
        unsigned: [{ widget: 'agent-arcade', hash: 'hash-1', gardens: ['g-test0001'] }],
      }
    }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string, body?: unknown) => {
    h.posts.push({ route, body })
    return { ok: true }
  }),
}))

import { act } from 'react'
import { cleanup, fireEvent, render, waitFor, screen } from '@testing-library/react'
import { useHomesStore } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useZenGardensStore } from '@/lib/zen/zen-gardens'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenCustomWidgetPayload } from '@/lib/zen/zen-custom-types'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { __resetZenCustomRunForTests } from '@/lib/zen/zen-custom-run'
import { resetZenPausedStartForTests, takeZenPausedAtBoot } from '@/lib/zen/zen-widgets-running'
import { __resetGardenAccessForTests, GARDEN_HISTORY_FILTER_KINDS } from '@/lib/zen/garden-access-api'
import { ZenCustomWidget, zenWidgetHistoryLabel } from './ZenCustomWidget'
import { GARDEN_NOT_SIGNED_TEXT } from './ZenGardenHistoryModal'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

function payload(over: Partial<ZenCustomWidgetPayload> = {}): ZenCustomWidgetPayload {
  return {
    id: 'arcade',
    kind: 'custom',
    widget: 'agent-arcade',
    column: 0,
    props: { config: {} },
    caps: ['agents:read'],
    requested: ['agents:read'],
    source: 'user',
    name: 'Agent Arcade',
    description: null,
    reasons: {},
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
    if (verb === 'gardens.current') return { id: 'g-test0001', name: 'Arcade', index: 1 }
    throw new Error(`unexpected verb ${verb}`)
  },
} as unknown as ZenWidgetBridge

function mount(w: ZenCustomWidgetPayload): void {
  const decl: ZenWidgetDecl = { id: w.id, kind: 'custom', column: 0, props: {}, caps: [...w.caps], source: 'user', custom: w }
  render(
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

function items(): string[] {
  return [...document.querySelectorAll('[data-zen-custom-menu-item]')].map((el) => el.getAttribute('data-zen-custom-menu-item') ?? '')
}

beforeEach(() => {
  h.gets = []
  h.posts = []
  h.gardenAccess = false
  useHomesStore.setState({ homes: [], selectedId: null } as never)
  useConnectHostStore.setState({ hosts: [], activeHost: 'local' } as never)
  useZenGardensStore.setState({ gardens: [{ id: 'g-test0001', name: 'Arcade', template: 'k2.blank@1', index: 1 } as never] })
  __resetZenCustomRunForTests()
  __resetGardenAccessForTests()
  resetZenPausedStartForTests()
  takeZenPausedAtBoot(null, { label: 'main', storage: null })
})

afterEach(() => cleanup())

describe('zenWidgetHistoryLabel (pure)', () => {
  it('off or built-in: none; on: History…; on and unsigned: History · not signed', () => {
    expect(zenWidgetHistoryLabel({ on: false, builtin: false, signed: undefined })).toBeNull()
    expect(zenWidgetHistoryLabel({ on: true, builtin: true, signed: false })).toBeNull()
    expect(zenWidgetHistoryLabel({ on: true, builtin: false, signed: true })).toBe('History…')
    expect(zenWidgetHistoryLabel({ on: true, builtin: false, signed: undefined })).toBe('History…')
    expect(zenWidgetHistoryLabel({ on: true, builtin: false, signed: false })).toBe('History · not signed')
  })
})

describe('the widget ⋯ menu', () => {
  it('switch off: Reload only, as 0.46.3 (the switch is read when the menu opens, not on render)', async () => {
    mount(payload())
    expect(h.gets.map((g) => g.route)).not.toContain('settings/get')
    await act(async () => fireEvent.click(q('[data-zen-custom-menu]')))
    await waitFor(() => expect(h.gets.map((g) => g.route)).toContain('settings/get'))
    expect(items()).toEqual(['reload'])
    expect(q('[data-zen-custom-menu-list]').textContent).toBe('Reload')
  })

  it('switch on: History… opens the widget’s history modal', async () => {
    h.gardenAccess = true
    mount(payload({ signed: true }))
    await act(async () => fireEvent.click(q('[data-zen-custom-menu]')))
    await waitFor(() => expect(items()).toEqual(['reload', 'history']))
    expect(q('[data-zen-custom-menu-item="history"]').textContent).toBe('History…')
    await act(async () => fireEvent.click(q('[data-zen-custom-menu-item="history"]')))
    const modal = await waitFor(() => screen.getByTestId('zen-garden-history-modal'))
    expect(q('[data-garden-history-title]').textContent).toBe('agent-arcade history')
    expect(modal.textContent).toContain('Ana asked to edit')
    expect(modal.textContent).toContain('You signed agent-arcade')
    expect(h.gets.filter((g) => g.route === 'zen/widget/history').map((g) => g.params)).toEqual([{ widget: 'agent-arcade', limit: 50 }])
  })

  it('an unsigned version: "History · not signed"; the modal says why and Sign signs it for this Garden', async () => {
    mount(payload({ signed: false }))
    await act(async () => fireEvent.click(q('[data-zen-custom-menu]')))
    expect(q('[data-zen-custom-menu-item="history"]').textContent).toBe('History · not signed')
    // No banner on the widget itself (G26a): only the modal says so.
    expect(document.body.textContent).not.toContain(GARDEN_NOT_SIGNED_TEXT)
    await act(async () => fireEvent.click(q('[data-zen-custom-menu-item="history"]')))
    await waitFor(() => expect(document.querySelector('[data-garden-not-signed="agent-arcade"]')).not.toBeNull())
    expect(q('[data-garden-not-signed="agent-arcade"]').textContent).toBe(`${GARDEN_NOT_SIGNED_TEXT}Sign`)
    await act(async () => fireEvent.click(q('[data-garden-sign="agent-arcade"]')))
    expect(h.posts).toEqual([{ route: 'zen/widget/sign', body: { widget: 'agent-arcade', garden: 'g-test0001' } }])
  })

  it('the modal’s filters ask the daemon for their kinds', async () => {
    mount(payload({ signed: true }))
    await act(async () => fireEvent.click(q('[data-zen-custom-menu]')))
    await act(async () => fireEvent.click(q('[data-zen-custom-menu-item="history"]')))
    await waitFor(() => screen.getByTestId('zen-garden-history-modal'))
    for (const f of ['changes', 'access', 'asks'] as const) {
      await act(async () => fireEvent.click(q(`[data-garden-history-filter="${f}"]`)))
      expect(q(`[data-garden-history-filter="${f}"]`).getAttribute('aria-pressed')).toBe('true')
      const last = h.gets.filter((g) => g.route === 'zen/widget/history').at(-1)
      expect(last?.params).toEqual({ widget: 'agent-arcade', limit: 50, kind: (GARDEN_HISTORY_FILTER_KINDS[f] ?? []).join(',') })
    }
    expect(GARDEN_HISTORY_FILTER_KINDS.asks).toEqual(['access_asked', 'access_not_now', 'delete_asked', 'delete_kept'])
    expect(GARDEN_HISTORY_FILTER_KINDS.access).toEqual(['access_added', 'access_removed'])
  })

  it('K2’s built-ins never get History (nobody edits them)', () => {
    mount(payload({ id: 'diary', widget: 'k2:diary@1', name: 'Diary', signed: true }))
    expect(document.querySelector('[data-zen-custom-menu]')).toBeNull()
  })
})
